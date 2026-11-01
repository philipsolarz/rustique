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
use super::spec::{Category, FnPtr, Original, Refusal, TargetSpec};
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
        self.module.as_ref().ok_or_else(|| Refusal::with(Category::Other, "target has no module"))
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
        let k = interned(self.py, key);
        self.get_opt_s(dict, &k)
    }

    /// [`Self::get_opt`] with a pre-interned key.
    pub fn get_opt_s(
        &self,
        dict: &Bound<'_, PyDict>,
        key: &Bound<'_, PyString>,
    ) -> Result<Option<Bound<'py, PyAny>>, Refusal> {
        let mut out: *mut ffi::PyObject = std::ptr::null_mut();
        // SAFETY: attached thread; exact dict and str key: no Python code runs.
        let r = unsafe { ffi::PyDict_GetItemRef(dict.as_ptr(), key.as_ptr(), &mut out) };
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
        let m = self
            .get_opt(&modules, name)?
            .ok_or_else(|| Refusal::with(Category::Deps, format!("module {name} not imported")))?;
        let m = m
            .cast_into::<PyModule>()
            .map_err(|_| Refusal::with(Category::Deps, format!("sys.modules[{name:?}] is not a module")))?;
        if module_initializing(&m) {
            return Err(Refusal::with(Category::Deps, format!("module {name} is still initializing")));
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

    /// `what` (a readable name) is only built for a refusal or in record mode.
    fn check_fingerprint(
        &mut self,
        what: impl FnOnce() -> String,
        observed: String,
        expected: Fingerprints,
    ) -> Result<(), Refusal> {
        let (major, minor) = version::running();
        let want = fingerprint::expected_for(expected, major, minor);
        let ok = want == Some(observed.as_str());
        if self.record {
            self.records.push(FingerprintRecord { what: what(), observed, expected: want });
            return Ok(());
        }
        if ok {
            return Ok(());
        }
        let what = what();
        match want {
            _ if ok => Ok(()),
            None => Err(Refusal::with(
                Category::Version,
                format!("{what}: no fingerprint recorded for CPython {major}.{minor}"),
            )),
            Some(w) => {
                Err(Refusal::with(Category::Fingerprint, format!("{what}: fingerprint {observed} != expected {w}")))
            }
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
        self.check_fingerprint(|| describe(func), observed, expected)
    }

    /// Require the `ml_name`/`ml_flags`/`ml_doc` fingerprint of a C function or method
    /// descriptor to be the one recorded for the running minor version.
    pub fn fingerprint_c(&mut self, obj: &Bound<'_, PyAny>, expected: Fingerprints) -> Result<(), Refusal> {
        let observed = fingerprint::c_object(obj)?;
        self.check_fingerprint(|| describe(obj), observed, expected)
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
        self.check_fingerprint(|| what.to_owned(), observed, expected)
    }

    /// The canonical code fingerprint of `obj` (function, code object, or C function):
    /// for targets that check fingerprints themselves.
    pub fn fingerprint_of(&self, obj: &Bound<'_, PyAny>) -> Result<String, Refusal> {
        Ok(fingerprint::any(obj)?)
    }

    /// Run `f` once per registry pass (one `enable()`, one lazy activation) for `key`, and
    /// replay its outcome for every other target that asks: lets a group validate shared
    /// dependencies (fingerprints of common helpers, layouts, self-tests) once. Declarations
    /// (sites, pins) must still be made by each target.
    pub fn memo(&mut self, key: &'static str, f: impl FnOnce(&mut Self) -> Result<(), Refusal>) -> Result<(), Refusal> {
        if let Some(r) = memo_get(key) {
            return r;
        }
        let r = f(self);
        memo_put(key, r.clone());
        r
    }

    /// Run a behavioural self-test (kernel vs original on probe inputs; the `Original`s of
    /// sites declared so far are already set). `Ok(false)` or an error refuses.
    pub fn self_test(&mut self, what: &str, f: impl FnOnce(Python<'py>) -> PyResult<bool>) -> Result<(), Refusal> {
        match f(self.py) {
            Ok(true) => Ok(()),
            Ok(false) => Err(Refusal::with(Category::Selftest, format!("self-test failed: {what}"))),
            Err(e) => Err(Refusal::with(Category::Selftest, format!("self-test error: {what}: {}", e.value(self.py)))),
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
        original.capture(site.original).map_err(|r| Refusal::with(r.category(), format!("{}: {r}", site.name)))?;
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
        let def = unsafe { site::method_def(obj.as_ptr()) }.ok_or_else(|| {
            Refusal::with(Category::Conflict, format!("{}: not a builtin function or method", describe(obj)))
        })?;
        // SAFETY: a live method def.
        unsafe { self.ml_meth_def(&c_name(obj, def), def, kernel, original) }
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
                return Err(Refusal::with(
                    Category::Conflict,
                    format!("{what}: ml_flags {flags:#x} take {a} arguments, kernel takes {}", F::ARITY),
                ));
            }
            None => return Err(Refusal::with(Category::Conflict, format!("{what}: unsupported ml_flags {flags:#x}"))),
        }
        // SAFETY: field address of a live def.
        let addr = unsafe { (&raw mut (*def).ml_meth) as usize };
        if !crate::patch::writable(addr, size_of::<usize>()) {
            return Err(Refusal::with(Category::Layout, format!("{what}: method table is read-only")));
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
            return Err(Refusal::with(
                Category::Conflict,
                format!("{what}: slot takes {} arguments, kernel takes {}", slot.arity(), F::ARITY),
            ));
        }
        if tp == &raw mut ffi::PyDict_Type && matches!(slot, SlotName::MpSubscript | SlotName::MpAssSubscript) {
            return Err(Refusal::with(
                Category::Other,
                format!("{what}: compared by pointer in the specializer (contract §3.6)"),
            ));
        }
        // SAFETY: a ready type.
        let addr = unsafe { site::slot_addr(tp, slot) }
            .ok_or_else(|| Refusal::with(Category::Layout, format!("{what}: slot is empty")))?;
        if !crate::patch::writable(addr, size_of::<usize>()) {
            return Err(Refusal::with(Category::Layout, format!("{what}: type object is read-only")));
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
                return Err(Refusal::with(
                    Category::Conflict,
                    format!("{what}: slot differs from its wrapper descriptor (patched?)"),
                ));
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
        let what = || describe(func);
        if func.cast_exact::<PyFunction>().is_err() {
            return Err(Refusal::with(Category::Conflict, format!("{}: not a pure-Python function", what())));
        }
        // SAFETY: an exact function object: its code is a live code object.
        let code =
            unsafe { Bound::from_borrowed_ptr(self.py, (*func.as_ptr().cast::<ffi::PyFunctionObject>()).func_code) };
        // The local-monitoring layout the tools guard reads: calibrated on the first
        // vectorcall site of the process (a probe code object; C calls only).
        if !version::calibrate_monitoring(self.py) {
            return Err(Refusal::with(Category::Layout, format!("{}: local monitoring layout not calibrated", what())));
        }
        if !version::code_layout_cached(&code)? {
            return Err(Refusal::with(Category::Layout, format!("{}: PyCodeObject layout differs", what())));
        }
        let site = Site::vectorcall(func, kernel as usize).map_err(|e| Refusal::from(format!("{}: {e}", what())))?;
        let current = site.original;
        let reference = function_vectorcall();
        if current != reference {
            site.release();
            return Err(Refusal::with(
                Category::Conflict,
                format!("{}: vectorcall is not _PyFunction_Vectorcall (patched?)", what()),
            ));
        }
        if let Err(r) = original.capture(current) {
            site.release();
            return Err(Refusal::with(r.category(), format!("{}: {r}", what())));
        }
        if self.sites.iter().any(|d| d.site.addr == site.addr && d.site.kind == Kind::Vectorcall) {
            site.release();
            return Err(Refusal::from(format!("{}: site declared twice", what())));
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
        let k = interned(self.py, key);
        self.pin_s(dict, &k, flag)
    }

    /// [`Self::pin`] with a pre-interned key (e.g. `pyo3::intern!(py, "fspath")`): no key
    /// lookup in the interning cache.
    pub fn pin_s(
        &mut self,
        dict: &Bound<'_, PyDict>,
        key: &Bound<'_, PyString>,
        flag: &'static DepFlag,
    ) -> Result<&'static DepFlag, Refusal> {
        let mut out: *mut ffi::PyObject = std::ptr::null_mut();
        // SAFETY: attached thread; exact dict and str key: no Python code runs.
        let r = unsafe { ffi::PyDict_GetItemRef(dict.as_ptr(), key.as_ptr(), &mut out) };
        if r < 0 {
            return Err(PyErr::fetch(self.py).into());
        }
        if out.is_null() {
            return Err(refuse_missing(&key.to_string_lossy()));
        }
        // SAFETY: the new reference returned by PyDict_GetItemRef.
        let value = unsafe { Bound::from_owned_ptr(self.py, out) }.unbind();
        self.pins.push(PendingPin::Key {
            dict: dict.clone().unbind(),
            key: key.clone().unbind(),
            expected: Some(value),
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
        let k = interned(self.py, key);
        // SAFETY: attached thread; exact dict and str key: no Python code runs.
        let r = unsafe { ffi::PyDict_Contains(dict.as_ptr(), k.as_ptr()) };
        if r < 0 {
            return Err(PyErr::fetch(self.py).into());
        }
        if r == 1 {
            return Err(Refusal::with(Category::Deps, format!("{key} is present (expected absent)")));
        }
        self.pins.push(PendingPin::Key {
            dict: dict.clone().unbind(),
            key: k.unbind(),
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
        let k = interned(self.py, name);
        self.pin_global_s(globals, &k, flag)
    }

    /// [`Self::pin_global`] with a pre-interned name.
    pub fn pin_global_s(
        &mut self,
        globals: &Bound<'_, PyDict>,
        name: &Bound<'_, PyString>,
        flag: &'static DepFlag,
    ) -> Result<Bound<'py, PyAny>, Refusal> {
        if let Some(v) = self.get_opt_s(globals, name)? {
            self.pins.push(PendingPin::Key {
                dict: globals.clone().unbind(),
                key: name.clone().unbind(),
                expected: Some(v.clone().unbind()),
                flag,
                ty: None,
            });
            return Ok(v);
        }
        let builtins = self.builtins_of(globals)?;
        let v = self.get_opt_s(&builtins, name)?.ok_or_else(|| refuse_missing(&name.to_string_lossy()))?;
        self.pins.push(PendingPin::Key {
            dict: globals.clone().unbind(),
            key: name.clone().unbind(),
            expected: None,
            flag,
            ty: None,
        });
        self.pins.push(PendingPin::Key {
            dict: builtins.unbind(),
            key: name.clone().unbind(),
            expected: Some(v.clone().unbind()),
            flag,
            ty: None,
        });
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
            return Err(Refusal::with(Category::Layout, format!("{name}: type has no MRO")));
        }
        // SAFETY: borrowed tuple kept alive by the type.
        let mro = unsafe { Bound::from_borrowed_ptr(self.py, mro) };
        let mro = mro.cast_into::<pyo3::types::PyTuple>().map_err(PyErr::from)?;
        let key = interned(self.py, name).unbind();
        for base in mro.iter() {
            let base = base.cast_into::<PyType>().map_err(PyErr::from)?;
            let d = type_dict(&base)?;
            let v = self.get_opt(&d, name)?;
            self.pins.push(PendingPin::Key {
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

    /// Snapshot pin: `flag` is intact while `dict`'s contents equal their current state
    /// (keys by str equality, values by identity): any added, replaced or deleted key
    /// deviates, and the pin re-arms exactly when every deviation is undone (`mock.patch`
    /// exiting). Clearing or wholesale replacement breaks it until re-pinned. Covers
    /// instance dicts (materialize `obj.__dict__` first), class dicts and module dicts.
    /// Every key must be an exact `str`; at most 64 simultaneous deviations are tracked
    /// (beyond that the pin breaks).
    pub fn pin_dict(&mut self, dict: &Bound<'_, PyDict>, flag: &'static DepFlag) -> Result<&'static DepFlag, Refusal> {
        let mut snap = Vec::with_capacity(dict.len());
        for (k, v) in dict.iter() {
            let k =
                k.cast_into_exact::<PyString>().map_err(|_| Refusal::with(Category::Deps, "pin_dict: non-str key"))?;
            snap.push((k.unbind(), v.unbind()));
        }
        self.pins.push(PendingPin::Dict { dict: dict.clone().unbind(), snap, flag });
        Ok(flag)
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
            return Err(Refusal::with(
                Category::Symbol,
                format!("symbol {} not exported by this interpreter", name.to_string_lossy()),
            ));
        }
        cell.capture(addr).map_err(|r| r.or_category(Category::Symbol))?;
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

/// Interned key strings by content (process lifetime): pins and lookups reuse one `str`
/// object per name (no allocation, cached hash).
static KEYS: std::sync::Mutex<Option<std::collections::HashMap<String, Py<PyString>, foldhash::fast::FixedState>>> =
    std::sync::Mutex::new(None);

/// The interned `str` for `key`, created once per process.
pub(crate) fn interned<'py>(py: Python<'py>, key: &str) -> Bound<'py, PyString> {
    let mut g = KEYS.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(Default::default);
    if let Some(k) = m.get(key) {
        return k.bind(py).clone();
    }
    let k = PyString::intern(py, key);
    m.insert(key.to_owned(), k.clone().unbind());
    k
}

static MEMO: std::sync::Mutex<Vec<(&'static str, Result<(), Refusal>)>> = std::sync::Mutex::new(Vec::new());

/// Interpreter exit: drop the interned keys, memoized validations and cached code
/// fingerprints (which hold the code objects).
pub(crate) fn teardown() {
    let keys = KEYS.lock().unwrap_or_else(|p| p.into_inner()).take();
    drop(keys);
    if let Ok(mut m) = MEMO.lock() {
        m.clear();
    }
    super::fingerprint::clear_cache();
}

fn memo_get(key: &str) -> Option<Result<(), Refusal>> {
    MEMO.lock().ok()?.iter().find(|(k, _)| *k == key).map(|(_, r)| r.clone())
}

fn memo_put(key: &'static str, r: Result<(), Refusal>) {
    if let Ok(mut m) = MEMO.lock() {
        m.push((key, r));
    }
}

/// Start/end of a registry pass: forget memoized validations (code fingerprints are cached
/// for the process: the cache keeps the code objects alive, so an address is never reused;
/// the loaded objects' segments are kept while the loader's counters stay the same).
pub(crate) fn clear_pass_caches() {
    if let Ok(mut m) = MEMO.lock() {
        m.clear();
    }
    crate::patch::new_pass();
}

fn refuse_missing(key: &str) -> Refusal {
    Refusal::with(Category::Deps, format!("missing {key}"))
}

/// `sys.modules` (the interpreter's module dict; no import, no string lookup).
pub(crate) fn sys_modules(py: Python<'_>) -> Result<Bound<'_, PyDict>, Refusal> {
    // SAFETY: attached thread; a borrowed reference to the interpreter's modules dict.
    let m = unsafe { ffi::PyImport_GetModuleDict() };
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
    let Ok(Some(spec)) = d.get_item(interned(py, "__spec__")) else { return false };
    if spec.is_none() {
        return false;
    }
    let mut out: *mut ffi::PyObject = std::ptr::null_mut();
    let key = interned(py, "_initializing");
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

/// `_PyFunction_Vectorcall`, read once from a fresh function object (`PyFunction_New` on
/// an existing code object: no compilation).
pub(crate) fn function_vectorcall() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static VC: AtomicUsize = AtomicUsize::new(0);
    let v = VC.load(Ordering::Relaxed);
    if v != 0 {
        return v;
    }
    Python::attach(|py| {
        // Any code object will do: `list.__init__` has none, so take this module's
        // `function_vectorcall`-independent probe: the code of `sys.modules`' loader is not
        // guaranteed either; build a minimal code object instead.
        // SAFETY: static C strings; new references checked; the function is dropped at once.
        let v = unsafe {
            let code = PyCode_NewEmpty(c"<rustique>".as_ptr(), c"probe".as_ptr(), 1);
            if code.is_null() {
                ffi::PyErr_Clear();
                0
            } else {
                let globals = ffi::PyDict_New();
                let f = if globals.is_null() { std::ptr::null_mut() } else { ffi::PyFunction_New(code, globals) };
                let v = if f.is_null() {
                    ffi::PyErr_Clear();
                    0
                } else {
                    let v = (*f.cast::<ffi::PyFunctionObject>()).vectorcall.map_or(0, |p| p as usize);
                    ffi::Py_DECREF(f);
                    v
                };
                ffi::Py_XDECREF(globals);
                ffi::Py_DECREF(code);
                v
            }
        };
        let _ = py;
        VC.store(v, Ordering::Relaxed);
        v
    })
}

unsafe extern "C" {
    fn PyCode_NewEmpty(
        filename: *const std::ffi::c_char,
        funcname: *const std::ffi::c_char,
        firstlineno: std::ffi::c_int,
    ) -> *mut ffi::PyObject;
}

/// `Type.method` / `module.function` of a C original, without Python calls.
fn c_name(obj: &Bound<'_, PyAny>, def: *mut ffi::PyMethodDef) -> String {
    // SAFETY: a live method def; type-checked layout reads of descriptors / functions.
    unsafe {
        let name = std::ffi::CStr::from_ptr((*def).ml_name).to_string_lossy();
        let p = obj.as_ptr();
        let t = ffi::Py_TYPE(p);
        if t == &raw mut ffi::PyMethodDescr_Type || t == &raw mut ffi::PyClassMethodDescr_Type {
            let owner = (*p.cast::<ffi::PyDescrObject>()).d_type;
            if !owner.is_null() {
                let tn = std::ffi::CStr::from_ptr((*owner).tp_name).to_string_lossy();
                return format!("{tn}.{name}");
            }
        } else if ffi::PyCFunction_Check(p) != 0 {
            let m = (*p.cast::<ffi::PyCFunctionObject>()).m_module;
            if !m.is_null()
                && ffi::PyUnicode_Check(m) != 0
                && let Ok(mn) = Bound::from_borrowed_ptr(obj.py(), m).cast_into::<PyString>()
            {
                return format!("{}.{name}", mn.to_string_lossy());
            }
        }
        name.into_owned()
    }
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
