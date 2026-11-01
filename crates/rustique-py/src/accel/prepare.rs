//! [`Prepare`]: what a target's `prepare` uses to validate the running interpreter and
//! declare its sites and dependency pins (docs/ACCELERATION.md §3, §9). Nothing is swapped
//! until `prepare` returns `Ok`; then the registry installs every declared site
//! all-or-nothing (compare-and-swap from the originals captured here) and activates the
//! pins.
//!
//! **Module targets** (`TargetSpec::module` is `Some`) may be prepared inside CPython's
//! `sys.modules` dict watcher (lazy activation, §3.3): their `prepare` must not import,
//! must not run Python code (no calls of Python functions, no `getattr` on objects with
//! Python-level `__getattr__`/properties) and must not release the GIL. Use the lookups
//! offered here ([`Prepare::module_attr`], [`Prepare::get`], [`Prepare::type_attr`],
//! [`Prepare::loaded_module`]), which are plain dict reads. Self-tests may call C
//! originals and kernels only.

use std::ffi::{CStr, c_int};

use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyFunction, PyModule, PyString, PyType};

use super::fingerprint::{self, Fingerprints};
use super::guard::{DepFlag, PendingPin};
use super::site::{self, Kind, Site, SlotName};
use super::spec::{FnPtr, Original, Refusal, TargetSpec};
use super::version;

/// One declared site (its `Original` cell already holds the original entry point).
pub(crate) struct SiteDecl {
    pub site: Site,
}

/// A fingerprint observed while preparing (for the dev tool's record mode).
#[derive(Debug, Clone)]
pub struct FingerprintRecord {
    pub what: String,
    pub observed: String,
    pub expected: Option<&'static str>,
}

/// `ml_flags` calling-convention bits → C arity of `ml_meth`.
fn meth_arity(flags: c_int) -> Option<usize> {
    let conv = flags & site::CONV_MASK;
    Some(match conv {
        x if x == ffi::METH_NOARGS || x == ffi::METH_O || x == ffi::METH_VARARGS => 2,
        x if x == ffi::METH_VARARGS | ffi::METH_KEYWORDS => 3,
        x if x == ffi::METH_FASTCALL => 3,
        x if x == ffi::METH_FASTCALL | ffi::METH_KEYWORDS => 4,
        x if x == ffi::METH_METHOD | ffi::METH_FASTCALL | ffi::METH_KEYWORDS => 5,
        _ => return None,
    })
}

/// Declarations and validation context of one target's install.
pub struct Prepare<'py> {
    py: Python<'py>,
    spec: &'static TargetSpec,
    module: Option<Bound<'py, PyModule>>,
    pub(crate) sites: Vec<SiteDecl>,
    pub(crate) pins: Vec<PendingPin>,
    pub(crate) records: Vec<FingerprintRecord>,
    /// Record mode (dev tool): fingerprint mismatches are recorded, not refused.
    pub(crate) record: bool,
}

impl<'py> Prepare<'py> {
    pub(crate) fn new(
        py: Python<'py>,
        spec: &'static TargetSpec,
        module: Option<Bound<'py, PyModule>>,
        record: bool,
    ) -> Self {
        Prepare { py, spec, module, sites: Vec::new(), pins: Vec::new(), records: Vec::new(), record }
    }

    pub fn py(&self) -> Python<'py> {
        self.py
    }

    pub fn spec(&self) -> &'static TargetSpec {
        self.spec
    }

    /// The fully executed module of a module target (`TargetSpec::module`).
    pub fn module(&self) -> Result<&Bound<'py, PyModule>, Refusal> {
        self.module.as_ref().ok_or_else(|| Refusal::new("target has no module"))
    }

    /// The running `(major, minor)`.
    pub fn version(&self) -> (u8, u8) {
        version::running()
    }

    // -----------------------------------------------------------------------------------
    // Lookups that never run Python code.
    // -----------------------------------------------------------------------------------

    /// `dict[key]` (exact dict read), refusing when absent.
    pub fn get(&self, dict: &Bound<'_, PyDict>, key: &str) -> Result<Bound<'py, PyAny>, Refusal> {
        self.get_opt(dict, key)?.ok_or_else(|| refuse_missing(key))
    }

    /// `dict[key]` or `None` (exact dict read).
    pub fn get_opt(&self, dict: &Bound<'_, PyDict>, key: &str) -> Result<Option<Bound<'py, PyAny>>, Refusal> {
        let k = PyString::intern(self.py, key);
        let mut out: *mut ffi::PyObject = std::ptr::null_mut();
        // SAFETY: attached thread; exact dict and str key: no Python code runs.
        let r = unsafe { ffi::PyDict_GetItemRef(dict.as_ptr(), k.as_ptr(), &mut out) };
        if r < 0 {
            return Err(PyErr::fetch(self.py).into());
        }
        // SAFETY: a new reference or NULL.
        Ok(unsafe { Bound::from_owned_ptr_or_opt(self.py, out) })
    }

    /// The module's own global `name` (its `__dict__`, never `__getattr__`).
    pub fn module_attr(&self, name: &str) -> Result<Bound<'py, PyAny>, Refusal> {
        let m = self.module()?;
        self.get(&m.dict(), name)
    }

    /// An already imported, fully executed module from `sys.modules`; refuses if absent
    /// (never imports).
    pub fn loaded_module(&self, name: &str) -> Result<Bound<'py, PyModule>, Refusal> {
        let modules = sys_modules(self.py)?;
        let m = self.get_opt(&modules, name)?.ok_or_else(|| Refusal::from(format!("module {name} not imported")))?;
        let m =
            m.cast_into::<PyModule>().map_err(|_| Refusal::from(format!("sys.modules[{name:?}] is not a module")))?;
        if module_initializing(&m) {
            return Err(Refusal::from(format!("module {name} is still initializing")));
        }
        Ok(m)
    }

    /// `ty.__dict__[name]`: the class's own entry (no MRO walk, no descriptors run).
    pub fn type_attr(&self, ty: &Bound<'_, PyType>, name: &str) -> Result<Bound<'py, PyAny>, Refusal> {
        self.get(&type_dict(ty)?, name)
    }

    /// `__builtins__` as seen by code whose globals are `globals` (what `LOAD_GLOBAL`
    /// falls back to): `globals['__builtins__']` (a module's dict, or a dict), else the
    /// interpreter's builtins.
    pub fn builtins_of(&self, globals: &Bound<'_, PyDict>) -> Result<Bound<'py, PyDict>, Refusal> {
        if let Some(b) = self.get_opt(globals, "__builtins__")? {
            if let Ok(m) = b.cast::<PyModule>() {
                return Ok(m.dict());
            }
            if let Ok(d) = b.cast_into::<PyDict>() {
                return Ok(d);
            }
        }
        let builtins = self.loaded_module("builtins")?;
        Ok(builtins.dict())
    }

    // -----------------------------------------------------------------------------------
    // Fingerprints and self-tests.
    // -----------------------------------------------------------------------------------

    fn check_fingerprint(&mut self, what: String, observed: String, expected: Fingerprints) -> Result<(), Refusal> {
        let (major, minor) = version::running();
        let want = fingerprint::expected_for(expected, major, minor);
        let ok = want == Some(observed.as_str());
        if self.record {
            self.records.push(FingerprintRecord { what, observed, expected: want });
            return Ok(());
        }
        match want {
            _ if ok => Ok(()),
            None => Err(Refusal::from(format!("{what}: no fingerprint recorded for CPython {major}.{minor}"))),
            Some(w) => Err(Refusal::from(format!("{what}: fingerprint {observed} != expected {w}"))),
        }
    }

    /// Require the canonical code fingerprint of a pure-Python function (or code object)
    /// to be the one recorded for the running minor version.
    pub fn fingerprint_code(&mut self, func: &Bound<'_, PyAny>, expected: Fingerprints) -> Result<(), Refusal> {
        let observed = if func.cast_exact::<PyFunction>().is_ok() {
            fingerprint::function(func)?
        } else {
            fingerprint::code(func)?
        };
        self.check_fingerprint(describe(func), observed, expected)
    }

    /// Require the `ml_name`/`ml_flags`/`ml_doc` fingerprint of a C function or method
    /// descriptor to be the one recorded for the running minor version.
    pub fn fingerprint_c(&mut self, obj: &Bound<'_, PyAny>, expected: Fingerprints) -> Result<(), Refusal> {
        let observed = fingerprint::c_object(obj)?;
        self.check_fingerprint(describe(obj), observed, expected)
    }

    /// Same as [`Self::fingerprint_c`] for a method table entry found directly.
    ///
    /// # Safety
    /// `def` is a live `PyMethodDef`.
    pub unsafe fn fingerprint_def(
        &mut self,
        what: &str,
        def: *const ffi::PyMethodDef,
        expected: Fingerprints,
    ) -> Result<(), Refusal> {
        // SAFETY: per the caller.
        let observed = unsafe { fingerprint::method_def(def) };
        self.check_fingerprint(what.to_owned(), observed, expected)
    }

    /// The canonical code fingerprint of `obj` (function, code object, or C function):
    /// for targets that check fingerprints themselves.
    pub fn fingerprint_of(&self, obj: &Bound<'_, PyAny>) -> Result<String, Refusal> {
        Ok(fingerprint::any(obj)?)
    }

    /// Run a behavioural self-test (kernel vs original on probe inputs; the `Original`s of
    /// sites declared so far are already set). `Ok(false)` or an error refuses.
    pub fn self_test(&mut self, what: &str, f: impl FnOnce(Python<'py>) -> PyResult<bool>) -> Result<(), Refusal> {
        match f(self.py) {
            Ok(true) => Ok(()),
            Ok(false) => Err(Refusal::from(format!("self-test failed: {what}"))),
            Err(e) => Err(Refusal::from(format!("self-test error: {what}: {}", e.value(self.py)))),
        }
    }

    // -----------------------------------------------------------------------------------
    // Sites.
    // -----------------------------------------------------------------------------------

    fn push_site<F: FnPtr>(&mut self, site: Site, original: &'static Original<F>) -> Result<(), Refusal> {
        if self.sites.iter().any(|d| d.site.addr == site.addr && d.site.kind == site.kind) {
            return Err(Refusal::from(format!("{}: site declared twice", site.name)));
        }
        if site.original == 0 {
            return Err(Refusal::from(format!("{}: empty entry point", site.name)));
        }
        original.capture(site.original).map_err(|r| Refusal::from(format!("{}: {r}", site.name)))?;
        self.sites.push(SiteDecl { site });
        Ok(())
    }

    /// Declare the `ml_meth` of the `PyMethodDef` behind `obj` (method descriptor,
    /// class-method descriptor, builtin function or bound builtin method). Patch every alias
    /// entry with its own call. The kernel's C arity must match `ml_flags`.
    pub fn ml_meth<F: FnPtr>(
        &mut self,
        obj: &Bound<'_, PyAny>,
        kernel: F,
        original: &'static Original<F>,
    ) -> Result<(), Refusal> {
        // SAFETY: attached thread.
        let def = unsafe { site::method_def(obj.as_ptr()) }
            .ok_or_else(|| Refusal::from(format!("{}: not a builtin function or method", describe(obj))))?;
        // SAFETY: a live method def.
        unsafe { self.ml_meth_def(&describe(obj), def, kernel, original) }
    }

    /// Declare the `ml_meth` of a method table entry found directly (e.g. by walking a
    /// module's `m_methods`).
    ///
    /// # Safety
    /// `def` is a live `PyMethodDef` that lives as long as the process (static tables).
    pub unsafe fn ml_meth_def<F: FnPtr>(
        &mut self,
        what: &str,
        def: *mut ffi::PyMethodDef,
        kernel: F,
        original: &'static Original<F>,
    ) -> Result<(), Refusal> {
        // SAFETY: per the caller.
        let flags = unsafe { (*def).ml_flags };
        match meth_arity(flags) {
            Some(a) if a == F::ARITY => {}
            Some(a) => {
                return Err(Refusal::from(format!(
                    "{what}: ml_flags {flags:#x} take {a} arguments, kernel takes {}",
                    F::ARITY
                )));
            }
            None => return Err(Refusal::from(format!("{what}: unsupported ml_flags {flags:#x}"))),
        }
        // SAFETY: field address of a live def.
        let addr = unsafe { (&raw mut (*def).ml_meth) as usize };
        if !crate::patch::writable(addr, size_of::<usize>()) {
            return Err(Refusal::from(format!("{what}: method table is read-only")));
        }
        // SAFETY: aligned pointer-sized field.
        let current =
            unsafe { (*(addr as *const std::sync::atomic::AtomicUsize)).load(std::sync::atomic::Ordering::Acquire) };
        let site = Site {
            name: what.to_owned(),
            kind: Kind::MlMeth,
            addr,
            original: current,
            kernel: kernel.addr(),
            installed: false,
        };
        self.push_site(site, original)
    }

    /// Declare a type slot. Refuses slots the specializer compares by pointer (`dict`'s
    /// `mp_subscript`/`mp_ass_subscript`), and a slot whose current function is not the one
    /// the type's own wrapper descriptor reports (someone else patched it).
    pub fn slot<F: FnPtr>(
        &mut self,
        ty: &Bound<'_, PyType>,
        slot: SlotName,
        kernel: F,
        original: &'static Original<F>,
    ) -> Result<(), Refusal> {
        let tp = ty.as_type_ptr();
        let what = format!("{}.{}", ty.qualname().map(|q| q.to_string()).unwrap_or_default(), slot.c_name());
        if slot.arity() != F::ARITY {
            return Err(Refusal::from(format!(
                "{what}: slot takes {} arguments, kernel takes {}",
                slot.arity(),
                F::ARITY
            )));
        }
        if tp == &raw mut ffi::PyDict_Type && matches!(slot, SlotName::MpSubscript | SlotName::MpAssSubscript) {
            return Err(Refusal::from(format!("{what}: compared by pointer in the specializer (contract §3.6)")));
        }
        // SAFETY: a ready type.
        let addr =
            unsafe { site::slot_addr(tp, slot) }.ok_or_else(|| Refusal::from(format!("{what}: slot is empty")))?;
        if !crate::patch::writable(addr, size_of::<usize>()) {
            return Err(Refusal::from(format!("{what}: type object is read-only")));
        }
        // SAFETY: aligned pointer-sized field.
        let current =
            unsafe { (*(addr as *const std::sync::atomic::AtomicUsize)).load(std::sync::atomic::Ordering::Acquire) };
        if let Some(dunder) = slot.dunder()
            && let Some(w) = self.get_opt(&type_dict(ty)?, dunder)?
            // SAFETY: type check of a live object.
            && unsafe { ffi::Py_TYPE(w.as_ptr()) } == &raw mut ffi::PyWrapperDescr_Type
        {
            // SAFETY: exact wrapper descriptor.
            let wrapped = unsafe { (*w.as_ptr().cast::<ffi::PyWrapperDescrObject>()).d_wrapped } as usize;
            if wrapped != current {
                return Err(Refusal::from(format!("{what}: slot differs from its wrapper descriptor (patched?)")));
            }
        }
        let site =
            Site { name: what, kind: Kind::Slot, addr, original: current, kernel: kernel.addr(), installed: false };
        self.push_site(site, original)
    }

    /// Declare the vectorcall of an exact pure-Python function (found in a module or class
    /// `__dict__`). The function's current vectorcall must be `_PyFunction_Vectorcall`
    /// (nobody else replaced it); it is captured into `original` (use
    /// [`super::FUNCTION_VECTORCALL`] to share one cell across targets). Also calibrates
    /// the code-object layout the tools guard reads, against this function's own code.
    pub fn vectorcall(
        &mut self,
        func: &Bound<'_, PyAny>,
        kernel: ffi::vectorcallfunc,
        original: &'static Original<ffi::vectorcallfunc>,
    ) -> Result<(), Refusal> {
        let what = describe(func);
        if func.cast_exact::<PyFunction>().is_err() {
            return Err(Refusal::from(format!("{what}: not a pure-Python function")));
        }
        let code = func.getattr("__code__")?;
        if !version::code_layout_ok_for(&code)? {
            return Err(Refusal::from(format!("{what}: PyCodeObject layout differs")));
        }
        let site = Site::vectorcall(func, kernel as usize).map_err(|e| Refusal::from(format!("{what}: {e}")))?;
        let current = site.original;
        let reference = function_vectorcall();
        if current != reference {
            site.release();
            return Err(Refusal::from(format!("{what}: vectorcall is not _PyFunction_Vectorcall (patched?)")));
        }
        if let Err(r) = original.capture(current) {
            site.release();
            return Err(Refusal::from(format!("{what}: {r}")));
        }
        if self.sites.iter().any(|d| d.site.addr == site.addr && d.site.kind == Kind::Vectorcall) {
            site.release();
            return Err(Refusal::from(format!("{what}: site declared twice")));
        }
        self.sites.push(SiteDecl { site });
        Ok(())
    }

    /// `_PyFunction_Vectorcall`: what every exact pure-Python function's vectorcall is.
    pub fn original_vectorcall(&self) -> ffi::vectorcallfunc {
        // SAFETY: read from a live function object at first use.
        unsafe { std::mem::transmute::<usize, ffi::vectorcallfunc>(function_vectorcall()) }
    }

    // -----------------------------------------------------------------------------------
    // Dependency pins.
    // -----------------------------------------------------------------------------------

    /// Pin `dict[key]` to its current value: `flag.ok()` turns false when the entry is
    /// replaced or deleted, and true again when the same object is put back. The target
    /// must have validated that value (fingerprint, identity with a known object).
    pub fn pin(
        &mut self,
        dict: &Bound<'_, PyDict>,
        key: &str,
        flag: &'static DepFlag,
    ) -> Result<&'static DepFlag, Refusal> {
        let value = self.get(dict, key)?;
        self.pins.push(PendingPin {
            dict: dict.clone().unbind(),
            key: PyString::intern(self.py, key).unbind(),
            expected: Some(value.unbind()),
            flag,
            ty: None,
        });
        Ok(flag)
    }

    /// Pin "`key` is absent from `dict`" (a module global that would shadow a builtin):
    /// false while the key exists, true again when it is deleted. Refuses if present now.
    pub fn pin_absent(
        &mut self,
        dict: &Bound<'_, PyDict>,
        key: &str,
        flag: &'static DepFlag,
    ) -> Result<&'static DepFlag, Refusal> {
        if self.get_opt(dict, key)?.is_some() {
            return Err(Refusal::from(format!("{key} is present (expected absent)")));
        }
        self.pins.push(PendingPin {
            dict: dict.clone().unbind(),
            key: PyString::intern(self.py, key).unbind(),
            expected: None,
            flag,
            ty: None,
        });
        Ok(flag)
    }

    /// Pin exactly what `LOAD_GLOBAL name` resolves to for code with these `globals`: the
    /// module global if it exists; otherwise "absent from `globals`" plus the builtins
    /// entry (`builtins_of(globals)[name]`). Returns the object it resolves to now.
    pub fn pin_global(
        &mut self,
        globals: &Bound<'_, PyDict>,
        name: &str,
        flag: &'static DepFlag,
    ) -> Result<Bound<'py, PyAny>, Refusal> {
        if let Some(v) = self.get_opt(globals, name)? {
            self.pin(globals, name, flag)?;
            return Ok(v);
        }
        let builtins = self.builtins_of(globals)?;
        let v = self.get(&builtins, name)?;
        self.pin_absent(globals, name, flag)?;
        self.pin(&builtins, name, flag)?;
        Ok(v)
    }

    /// Pin the class attribute `ty.name` as found by the MRO walk: every class in
    /// `ty.__mro__` before the one defining `name` pins "absent", the defining one pins
    /// its value; an MRO change (`__bases__` assignment) breaks the pin for good. Returns
    /// the attribute's current raw value (the `__dict__` entry, descriptors not run).
    pub fn pin_attr(
        &mut self,
        ty: &Bound<'_, PyType>,
        name: &str,
        flag: &'static DepFlag,
    ) -> Result<Bound<'py, PyAny>, Refusal> {
        // SAFETY: a ready type: tp_mro is a tuple.
        let mro = unsafe { (*ty.as_type_ptr()).tp_mro };
        if mro.is_null() {
            return Err(Refusal::from(format!("{name}: type has no MRO")));
        }
        // SAFETY: borrowed tuple kept alive by the type.
        let mro = unsafe { Bound::from_borrowed_ptr(self.py, mro) };
        let mro = mro.cast_into::<pyo3::types::PyTuple>().map_err(PyErr::from)?;
        let key = PyString::intern(self.py, name).unbind();
        for base in mro.iter() {
            let base = base.cast_into::<PyType>().map_err(PyErr::from)?;
            let d = type_dict(&base)?;
            let v = self.get_opt(&d, name)?;
            self.pins.push(PendingPin {
                dict: d.unbind(),
                key: key.clone_ref(self.py),
                expected: v.as_ref().map(|v| v.clone().unbind()),
                flag,
                ty: Some(ty.clone().unbind()),
            });
            if let Some(v) = v {
                return Ok(v);
            }
        }
        Err(refuse_missing(name))
    }

    // -----------------------------------------------------------------------------------
    // Internal-but-exported symbols.
    // -----------------------------------------------------------------------------------

    /// Resolve an exported symbol of the running interpreter (e.g. `_PyObject_GetMethod`,
    /// internal headers but `PyAPI_FUNC`) into `cell`; refuses the target when missing.
    /// The caller is responsible for the signature `F` (validate per minor version).
    pub fn symbol<F: FnPtr>(&mut self, name: &CStr, cell: &'static Original<F>) -> Result<F, Refusal> {
        let addr = dlsym_default(name);
        if addr == 0 {
            return Err(Refusal::from(format!("symbol {} not exported by this interpreter", name.to_string_lossy())));
        }
        cell.capture(addr)?;
        Ok(cell.get())
    }
}

#[cfg(unix)]
fn dlsym_default(name: &CStr) -> usize {
    unsafe extern "C" {
        fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    }
    #[cfg(target_os = "macos")]
    let handle = -2isize as *mut std::ffi::c_void; // RTLD_DEFAULT
    #[cfg(not(target_os = "macos"))]
    let handle = std::ptr::null_mut(); // RTLD_DEFAULT
    // SAFETY: NUL-terminated name; dlsym is thread-safe.
    unsafe { dlsym(handle, name.as_ptr()) as usize }
}

#[cfg(not(unix))]
fn dlsym_default(_name: &CStr) -> usize {
    0
}

fn refuse_missing(key: &str) -> Refusal {
    Refusal::from(format!("missing {key}"))
}

/// `sys.modules` (borrowed from the sys module dict; no import).
pub(crate) fn sys_modules(py: Python<'_>) -> Result<Bound<'_, PyDict>, Refusal> {
    // SAFETY: attached thread; borrowed reference from the sys module dict.
    let m = unsafe { ffi::PySys_GetObject(c"modules".as_ptr()) };
    if m.is_null() {
        return Err(Refusal::new("sys.modules missing"));
    }
    // SAFETY: a live borrowed object.
    let m = unsafe { Bound::from_borrowed_ptr(py, m) };
    m.cast_into::<PyDict>().map_err(|_| Refusal::new("sys.modules is not a dict"))
}

/// A class's own `__dict__` (the real dict, not the mappingproxy).
pub(crate) fn type_dict<'py>(ty: &Bound<'py, PyType>) -> Result<Bound<'py, PyDict>, Refusal> {
    // SAFETY: a ready type; PyType_GetDict returns a new reference.
    let d = unsafe { ffi::PyType_GetDict(ty.as_type_ptr()) };
    // SAFETY: new reference or NULL.
    let d = unsafe { Bound::from_owned_ptr_or_err(ty.py(), d) }?;
    d.cast_into::<PyDict>().map_err(|_| Refusal::new("type dict is not a dict"))
}

/// Is `module` still executing (importlib sets `__spec__._initializing` around exec)?
/// Reads the spec's instance dict only (no Python code).
pub(crate) fn module_initializing(module: &Bound<'_, PyModule>) -> bool {
    let py = module.py();
    let d = module.dict();
    let Ok(Some(spec)) = d.get_item("__spec__") else { return false };
    if spec.is_none() {
        return false;
    }
    let mut out: *mut ffi::PyObject = std::ptr::null_mut();
    let key = PyString::intern(py, "_initializing");
    // SAFETY: attached thread; generic attribute read on a ModuleSpec instance (a plain
    // class: instance dict lookup, no Python code); `out` receives a new reference.
    let r = unsafe { ffi::PyObject_GetOptionalAttr(spec.as_ptr(), key.as_ptr(), &mut out) };
    if r < 0 {
        // SAFETY: attached thread.
        unsafe { ffi::PyErr_Clear() };
        return false;
    }
    if out.is_null() {
        return false;
    }
    // SAFETY: new reference.
    let v = unsafe { Bound::from_owned_ptr(py, out) };
    v.is_truthy().unwrap_or(false)
}

/// `_PyFunction_Vectorcall`, read once from a live function object.
pub(crate) fn function_vectorcall() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static VC: AtomicUsize = AtomicUsize::new(0);
    let v = VC.load(Ordering::Relaxed);
    if v != 0 {
        return v;
    }
    // A function object created by the interpreter at startup: the `__init__` of a
    // builtin-module class is C, so use a lambda compiled once instead.
    Python::attach(|py| {
        let f = py.eval(c"lambda: None", None, None).ok();
        let v = f.map_or(0, |f| {
            // SAFETY: an exact function object.
            unsafe { (*f.as_ptr().cast::<ffi::PyFunctionObject>()).vectorcall }.map_or(0, |p| p as usize)
        });
        VC.store(v, Ordering::Relaxed);
        v
    })
}

/// A readable name for an object in refusal reasons.
fn describe(obj: &Bound<'_, PyAny>) -> String {
    // `__qualname__`/`__name__` of functions, descriptors and types are C getters.
    let q = obj.getattr("__qualname__").or_else(|_| obj.getattr("__name__"));
    match q {
        Ok(q) => {
            let module = obj.getattr("__module__").ok().and_then(|m| m.extract::<String>().ok());
            match module {
                Some(m) if !m.is_empty() => format!("{m}.{q}"),
                _ => q.to_string(),
            }
        }
        Err(_) => obj.get_type().name().map(|n| n.to_string()).unwrap_or_else(|_| "?".into()),
    }
}

/// Raise for misuse from Python-facing helpers.
#[allow(dead_code)]
pub(crate) fn runtime_error(msg: impl Into<String>) -> PyErr {
    PyRuntimeError::new_err(msg.into())
}
