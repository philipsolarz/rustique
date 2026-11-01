//! Runtime shared by the `posixpath`, `fnmatch` and `pathlib` groups: inline access to
//! `str`/`bytes` buffers, result builders that keep CPython's identity rules, code
//! fingerprints, and the per-target vectorcall plumbing (guards, counters, fallback,
//! install/uninstall) until accel-core's `TargetSpec` registry lands.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyFrozenSet, PyList, PyString, PyTuple};

use crate::accel::guard::{self, DepFlag, Tools};
use crate::accel::site::Site;
use crate::accel::stats::{self, Counters, Reason};
use crate::accel::version;
pub use rustique_core::paths::Unit;

pub type O = *mut ffi::PyObject;

// ---------------------------------------------------------------------------------------
// str / bytes buffers
// ---------------------------------------------------------------------------------------

/// Where the kind/compact/ascii bits of `PyASCIIObject.state` live and where compact data
/// starts (measured at install against `PyUnicode_KIND`/`PyUnicode_DATA`, which are
/// out-of-line calls on 3.14+; same method as `takeover.rs`).
#[derive(Clone, Copy, Debug)]
struct StrLayout {
    kind_shift: u32,
    compact_mask: u32,
    ascii_mask: u32,
    ascii_data: usize,
    compact_data: usize,
}

static STR_LAYOUT: OnceLock<Option<StrLayout>> = OnceLock::new();

fn calibrate_str(py: Python<'_>) -> Option<StrLayout> {
    let samples: Vec<Bound<'_, PyString>> = ["ascii-sample", "lat\u{e9}n-1", "ucs-\u{101}-2", "ucs-\u{1F600}-4", "x"]
        .iter()
        .map(|s| PyString::new(py, &format!("{s}{}", py.version_info().minor)))
        .collect();
    // SAFETY: exact str objects kept alive by `samples`.
    let facts: Vec<(u32, u32, usize, bool)> = samples
        .iter()
        .map(|s| unsafe {
            let p = s.as_ptr();
            let data = ffi::PyUnicode_DATA(p) as usize - p as usize;
            (str_state(p), ffi::PyUnicode_KIND(p), data, s.to_str().is_ok_and(|t| t.is_ascii()))
        })
        .collect();
    let kind_shift = (0..29).find(|&sh| facts.iter().all(|&(st, kind, _, _)| (st >> sh) & 7 == kind))?;
    let layout = StrLayout {
        kind_shift,
        compact_mask: 1 << (kind_shift + 3),
        ascii_mask: 1 << (kind_shift + 4),
        ascii_data: size_of::<ffi::PyASCIIObject>(),
        compact_data: size_of::<ffi::PyCompactUnicodeObject>(),
    };
    let ok = facts.iter().all(|&(st, _, data, ascii)| {
        st & layout.compact_mask != 0
            && (st & layout.ascii_mask != 0) == ascii
            && data == if ascii { layout.ascii_data } else { layout.compact_data }
    });
    ok.then_some(layout)
}

/// Calibrate once; true if the inline layout is usable (otherwise the out-of-line API is).
pub fn init_str_layout(py: Python<'_>) -> bool {
    STR_LAYOUT.get_or_init(|| calibrate_str(py)).is_some()
}

/// # Safety
/// `op` is a `str` (exact or subclass).
#[inline(always)]
unsafe fn str_state(op: O) -> u32 {
    // SAFETY: the kind/compact/ascii bits never change after creation; volatile because
    // free-threaded builds update the `interned` bits of the same word atomically.
    unsafe { std::ptr::read_volatile(&raw const (*op.cast::<ffi::PyASCIIObject>()).state) }
}

/// The code units of a `str`.
#[derive(Clone, Copy, Debug)]
pub enum Text<'a> {
    U1(&'a [u8]),
    U2(&'a [u16]),
    U4(&'a [u32]),
}

/// A `str` buffer and its `PyUnicode_MAX_CHAR_VALUE` class.
#[derive(Clone, Copy, Debug)]
pub struct StrView<'a> {
    pub text: Text<'a>,
    pub maxchar: u32,
}

impl StrView<'_> {
    #[inline(always)]
    pub fn len(&self) -> usize {
        match self.text {
            Text::U1(s) => s.len(),
            Text::U2(s) => s.len(),
            Text::U4(s) => s.len(),
        }
    }
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Run a generic `fn(&[T]) -> R` on a [`Text`].
#[macro_export]
macro_rules! with_text {
    ($t:expr, |$s:ident| $body:expr) => {
        match $t {
            $crate::targets::posixpath::rt::Text::U1($s) => $body,
            $crate::targets::posixpath::rt::Text::U2($s) => $body,
            $crate::targets::posixpath::rt::Text::U4($s) => $body,
        }
    };
}

/// View of a `str` (exact or subclass) buffer.
///
/// # Safety
/// `op` is a live `str`; the view must not outlive it.
#[inline(always)]
pub unsafe fn str_view<'a>(op: O) -> StrView<'a> {
    // SAFETY: per the caller; offsets verified by `calibrate_str`.
    unsafe {
        let len = (*op.cast::<ffi::PyASCIIObject>()).length as usize;
        let (kind, data, ascii) = match STR_LAYOUT.get() {
            Some(Some(l)) => {
                let st = str_state(op);
                if st & l.compact_mask != 0 {
                    let ascii = st & l.ascii_mask != 0;
                    let off = if ascii { l.ascii_data } else { l.compact_data };
                    ((st >> l.kind_shift) & 7, op.cast::<u8>().add(off) as *const c_void, ascii)
                } else {
                    let ascii = st & l.ascii_mask != 0;
                    ((st >> l.kind_shift) & 7, ffi::PyUnicode_DATA(op) as *const c_void, ascii)
                }
            }
            _ => {
                let kind = ffi::PyUnicode_KIND(op);
                let data = ffi::PyUnicode_DATA(op) as *const c_void;
                let ascii = kind == 1 && std::slice::from_raw_parts(data.cast::<u8>(), len).is_ascii();
                (kind, data, ascii)
            }
        };
        match kind {
            1 => StrView {
                text: Text::U1(std::slice::from_raw_parts(data.cast(), len)),
                maxchar: if ascii { 0x7f } else { 0xff },
            },
            2 => StrView { text: Text::U2(std::slice::from_raw_parts(data.cast(), len)), maxchar: 0xffff },
            _ => StrView { text: Text::U4(std::slice::from_raw_parts(data.cast(), len)), maxchar: 0x10ffff },
        }
    }
}

/// The bytes of a `bytes` (exact or subclass).
///
/// # Safety
/// `op` is a live `bytes`; the slice must not outlive it.
#[inline(always)]
pub unsafe fn bytes_view<'a>(op: O) -> &'a [u8] {
    // SAFETY: per the caller.
    unsafe {
        let n = ffi::Py_SIZE(op) as usize;
        std::slice::from_raw_parts(ffi::PyBytes_AS_STRING(op).cast::<u8>(), n)
    }
}

#[inline(always)]
pub unsafe fn exact_str(o: O) -> bool {
    // SAFETY: live object.
    unsafe { ffi::Py_TYPE(o) == &raw mut ffi::PyUnicode_Type }
}

#[inline(always)]
pub unsafe fn exact_bytes(o: O) -> bool {
    // SAFETY: live object.
    unsafe { ffi::Py_TYPE(o) == &raw mut ffi::PyBytes_Type }
}

/// `p[start:end]` of a `str`, exactly as slicing does it (`PyUnicode_Substring`: the
/// object itself for the whole of an exact str, the empty and latin-1 singletons).
///
/// # Safety
/// `op` is a live `str`, `start <= end <= len`.
#[inline(always)]
pub unsafe fn substr(op: O, start: usize, end: usize) -> O {
    // SAFETY: per the caller.
    unsafe { ffi::PyUnicode_Substring(op, start as ffi::Py_ssize_t, end as ffi::Py_ssize_t) }
}

/// `p[start:end]` of a `bytes`, as `bytes_subscript` does it (the object itself for the
/// whole of an exact bytes; `PyBytes_FromStringAndSize` otherwise, whose empty and
/// one-byte results are singletons).
///
/// # Safety
/// `op` is a live `bytes`, `start <= end <= len`.
#[inline(always)]
pub unsafe fn subbytes(op: O, start: usize, end: usize) -> O {
    // SAFETY: per the caller.
    unsafe {
        let s = bytes_view(op);
        if start == 0 && end == s.len() && exact_bytes(op) {
            return ffi::Py_NewRef(op);
        }
        ffi::PyBytes_FromStringAndSize(s.as_ptr().add(start).cast(), (end - start) as ffi::Py_ssize_t)
    }
}

/// Copy code units into the buffer of a fresh string of `kind` at `at`.
///
/// # Safety
/// `dst` points to a buffer of the right kind with room for `at + src.len()` units, and
/// every code point of `src` fits `kind`.
#[inline(always)]
pub unsafe fn write_units<S: Unit>(dst: *mut c_void, kind: u32, at: usize, src: &[S]) {
    // SAFETY: per the caller; same-size copies are plain memcpy.
    unsafe {
        let n = src.len();
        match kind {
            1 if size_of::<S>() == 1 => std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u8>(), dst.cast::<u8>().add(at), n),
            2 if size_of::<S>() == 2 => std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u16>(), dst.cast::<u16>().add(at), n),
            4 if size_of::<S>() == 4 => std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u32>(), dst.cast::<u32>().add(at), n),
            1 => {
                let d = dst.cast::<u8>().add(at);
                for (k, &c) in src.iter().enumerate() {
                    *d.add(k) = c.cp() as u8;
                }
            }
            2 => {
                let d = dst.cast::<u16>().add(at);
                for (k, &c) in src.iter().enumerate() {
                    *d.add(k) = c.cp() as u16;
                }
            }
            _ => {
                let d = dst.cast::<u32>().add(at);
                for (k, &c) in src.iter().enumerate() {
                    *d.add(k) = c.cp();
                }
            }
        }
    }
}

/// [`write_units`] for a [`Text`].
///
/// # Safety
/// As [`write_units`].
#[inline(always)]
pub unsafe fn write_text(dst: *mut c_void, kind: u32, at: usize, src: Text<'_>) {
    // SAFETY: forwarded.
    unsafe {
        match src {
            Text::U1(s) => write_units(dst, kind, at, s),
            Text::U2(s) => write_units(dst, kind, at, s),
            Text::U4(s) => write_units(dst, kind, at, s),
        }
    }
}

/// A fresh, uninitialised `str` of `len` code points whose kind and ascii flag follow
/// `maxchar` (as `PyUnicode_New`): `(object, data, kind)`; null object on MemoryError.
///
/// # Safety
/// Attached thread. The caller fills every code point before exposing the object.
#[inline(always)]
pub unsafe fn new_str(len: usize, maxchar: u32) -> (O, *mut c_void, u32) {
    // SAFETY: attached thread.
    unsafe {
        let o = ffi::PyUnicode_New(len as ffi::Py_ssize_t, maxchar);
        if o.is_null() {
            return (o, std::ptr::null_mut(), 0);
        }
        let v = str_view(o);
        let (data, kind) = match v.text {
            Text::U1(s) => (s.as_ptr() as *mut c_void, 1),
            Text::U2(s) => (s.as_ptr() as *mut c_void, 2),
            Text::U4(s) => (s.as_ptr() as *mut c_void, 4),
        };
        (o, data, kind)
    }
}

/// A `str` from code points that are not all ASCII-known in advance: the kind follows the
/// actual maximum (as `PyUnicode_FromKindAndData`/`_PyUnicode_FromUCS*`), so one-character
/// latin-1 results are the shared singletons.
///
/// # Safety
/// Attached thread.
pub unsafe fn str_from_units<T: Unit>(units: &[T]) -> O {
    // SAFETY: attached thread; the buffer has the size passed.
    unsafe {
        let kind = match size_of::<T>() {
            1 => ffi::PyUnicode_1BYTE_KIND,
            2 => ffi::PyUnicode_2BYTE_KIND,
            _ => ffi::PyUnicode_4BYTE_KIND,
        };
        ffi::PyUnicode_FromKindAndData(kind as std::ffi::c_int, units.as_ptr().cast(), units.len() as ffi::Py_ssize_t)
    }
}

// ---------------------------------------------------------------------------------------
// Code fingerprints (path- and line-independent)
// ---------------------------------------------------------------------------------------

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    fn tag(&mut self, t: u8) {
        self.bytes(&[t]);
    }
}

fn feed_str(h: &mut Fnv, s: &Bound<'_, PyString>) -> PyResult<()> {
    let enc = s.call_method1("encode", ("utf-8", "surrogatepass"))?;
    let b = enc.cast::<PyBytes>()?.as_bytes();
    h.u64(b.len() as u64);
    h.bytes(b);
    Ok(())
}

fn feed_obj(h: &mut Fnv, o: &Bound<'_, PyAny>) -> PyResult<()> {
    let py = o.py();
    if o.is_none() {
        h.tag(0);
    } else if o.is(&*pyo3::types::PyBool::new(py, true)) {
        h.tag(1);
    } else if o.is(&*pyo3::types::PyBool::new(py, false)) {
        h.tag(2);
    } else if let Ok(s) = o.cast::<PyString>() {
        h.tag(3);
        feed_str(h, s)?;
    } else if let Ok(b) = o.cast::<PyBytes>() {
        h.tag(4);
        h.u64(b.as_bytes().len() as u64);
        h.bytes(b.as_bytes());
    } else if let Ok(t) = o.cast::<PyTuple>() {
        h.tag(5);
        h.u64(t.len() as u64);
        for x in t.iter() {
            feed_obj(h, &x)?;
        }
    } else if let Ok(fs) = o.cast::<PyFrozenSet>() {
        // Iteration order depends on string hashes: fingerprint the sorted item hashes.
        h.tag(6);
        let mut items = Vec::new();
        for x in fs.iter() {
            let mut sub = Fnv::new();
            feed_obj(&mut sub, &x)?;
            items.push(sub.0);
        }
        items.sort_unstable();
        h.u64(items.len() as u64);
        for v in items {
            h.u64(v);
        }
    } else if o.hasattr("co_code")? {
        h.tag(7);
        feed_code(h, o)?;
    } else {
        // int, float, complex, Ellipsis, ...: type name and repr.
        h.tag(8);
        feed_str(h, &o.get_type().name()?.cast_into::<PyString>()?)?;
        feed_str(h, &o.repr()?)?;
    }
    Ok(())
}

fn feed_code(h: &mut Fnv, code: &Bound<'_, PyAny>) -> PyResult<()> {
    for attr in ["co_code", "co_exceptiontable"] {
        let b = code.getattr(attr)?;
        let b = b.cast::<PyBytes>()?.as_bytes();
        h.u64(b.len() as u64);
        h.bytes(b);
    }
    for attr in ["co_argcount", "co_posonlyargcount", "co_kwonlyargcount", "co_flags"] {
        h.u64(code.getattr(attr)?.extract::<i64>()? as u64);
    }
    for attr in ["co_names", "co_varnames", "co_cellvars", "co_freevars", "co_consts"] {
        feed_obj(h, &code.getattr(attr)?)?;
    }
    Ok(())
}

/// Resolve a dotted path `module.attr[.attr...]` from `sys.modules` (no import).
pub fn resolve<'py>(py: Python<'py>, path: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    let mut it = path.split('.');
    let module = it.next().unwrap_or_default();
    let modules = py.import("sys")?.getattr("modules")?;
    let Ok(mut obj) = modules.get_item(module) else { return Ok(None) };
    for (k, attr) in it.enumerate() {
        // Module attributes come from the module dict (no `__getattr__` hooks).
        obj = if k == 0 {
            match obj.getattr("__dict__")?.cast_into::<PyDict>()?.get_item(attr)? {
                Some(v) => v,
                None => return Ok(None),
            }
        } else {
            obj.getattr(attr)?
        };
    }
    Ok(Some(obj))
}

/// Fingerprint of the code of the functions named by `paths` (each a dotted path to a
/// function, or to an object with `__wrapped__`).
pub fn fingerprint(py: Python<'_>, paths: &[&str]) -> PyResult<Option<u64>> {
    let mut h = Fnv::new();
    let mut any = false;
    for p in paths {
        // A function absent on this version (e.g. 3.15's `genericpath._commonprefix` on
        // 3.14) hashes as a marker: the fingerprint stays per-version deterministic.
        let Some(f) = resolve(py, p)? else {
            h.tag(10);
            continue;
        };
        any = true;
        let f = if f.hasattr("__code__")? { f } else { f.getattr("__wrapped__")? };
        h.tag(9);
        feed_code(&mut h, &f.getattr("__code__")?)?;
    }
    Ok(any.then_some(h.0))
}

// ---------------------------------------------------------------------------------------
// Targets: vectorcall kernels with guards and fallback
// ---------------------------------------------------------------------------------------

/// Run-time state of one target, read by its kernel.
pub struct Rt {
    pub counters: Counters,
    pub tools: Tools,
    pub deps: DepFlag,
    /// The function's vectorcall before install (`_PyFunction_Vectorcall`).
    pub orig: AtomicUsize,
}

impl Rt {
    pub const fn new() -> Self {
        Rt { counters: Counters::new(), tools: Tools::new(), deps: DepFlag::new(), orig: AtomicUsize::new(0) }
    }

    /// Call the original with the unchanged arguments.
    ///
    /// # Safety
    /// Vectorcall arguments as received by the kernel.
    #[inline(always)]
    pub unsafe fn fallback(&self, why: Reason, func: O, args: *const O, nargsf: usize, kw: O) -> O {
        self.counters.fallback(why);
        // SAFETY: `orig` holds the function's former vectorcall (set before install).
        unsafe { std::mem::transmute::<usize, ffi::vectorcallfunc>(self.orig.load(Relaxed))(func, args, nargsf, kw) }
    }

    /// Dependencies intact and no tool listening (globally or on this function's code).
    ///
    /// # Safety
    /// Attached thread; `func` is the patched function.
    #[inline(always)]
    pub unsafe fn ready(&self, func: O) -> Result<(), Reason> {
        if !self.deps.ok() {
            return Err(Reason::Deps);
        }
        // SAFETY: attached thread; `func` is a function object.
        unsafe {
            if !self.tools.quiet() || !guard::code_locally_quiet((*func.cast::<ffi::PyFunctionObject>()).func_code) {
                return Err(Reason::Tools);
            }
        }
        Ok(())
    }
}

impl Default for Rt {
    fn default() -> Self {
        Self::new()
    }
}

#[inline(always)]
pub fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

/// Static description of one target.
pub struct Spec {
    pub name: &'static str,
    /// Dotted path of the patched function (`posixpath.join`, `pathlib.PurePath.__str__`).
    pub func: &'static str,
    pub kernel: ffi::vectorcallfunc,
    pub rt: &'static Rt,
    /// Functions whose code the kernel reproduces (the patched one first).
    pub code: &'static [&'static str],
    /// Expected fingerprint per minor version.
    pub expected: &'static [(u8, u64)],
    /// Module-dict entries the original reads at call time: (module, key).
    pub pins: &'static [(&'static str, &'static str)],
    /// Measured within 1.05 under `PYTHON_JIT=1` too.
    pub jit_safe: bool,
    /// Extra validation and setup (constants, C dependencies, self-tests).
    pub prepare: Option<fn(Python<'_>) -> PyResult<Result<(), String>>>,
}

pub struct Installed {
    pub name: &'static str,
    pub site: Site,
}

/// Per-group registry: installed sites and the last status of each target.
pub struct Group {
    pub name: &'static str,
    pub specs: &'static [Spec],
    pub installed: Mutex<Vec<Installed>>,
    pub status: Mutex<Vec<(&'static str, String, Option<String>)>>,
}

fn jit_enabled(py: Python<'_>) -> PyResult<bool> {
    let sys = py.import("sys")?;
    match sys.getattr("_jit") {
        Ok(j) => j.call_method0("is_enabled")?.extract(),
        Err(_) => Ok(false),
    }
}

impl Group {
    pub const fn new(name: &'static str, specs: &'static [Spec]) -> Self {
        Group { name, specs, installed: Mutex::new(Vec::new()), status: Mutex::new(Vec::new()) }
    }

    fn validate<'py>(&self, py: Python<'py>, spec: &Spec, force_jit: bool) -> PyResult<Result<Bound<'py, PyAny>, String>> {
        if let Err(why) = version::supported(py, &[14, 15])? {
            return Ok(Err(why));
        }
        if !force_jit && !spec.jit_safe && jit_enabled(py)? {
            return Ok(Err("jit".into()));
        }
        let Some(func) = resolve(py, spec.func)? else {
            return Ok(Err("pending-import".into()));
        };
        // SAFETY: type check only.
        if unsafe { ffi::Py_TYPE(func.as_ptr()) } != &raw mut ffi::PyFunction_Type {
            return Ok(Err(format!("{} is not a pure-Python function", spec.func)));
        }
        let minor = py.version_info().minor;
        let Some(fp) = fingerprint(py, spec.code)? else {
            return Ok(Err("dependency missing".into()));
        };
        match spec.expected.iter().find(|(m, _)| *m == minor) {
            Some((_, want)) if *want == fp => {}
            Some(_) => return Ok(Err(format!("code fingerprint differs ({fp:#018x})"))),
            None => return Ok(Err(format!("CPython 3.{minor} not validated"))),
        }
        if !init_str_layout(py) {
            return Ok(Err("str layout not calibrated".into()));
        }
        if let Some(prep) = spec.prepare
            && let Err(why) = prep(py)?
        {
            return Ok(Err(why));
        }
        for (module, key) in spec.pins {
            let Some(m) = resolve(py, module)? else {
                return Ok(Err(format!("dependency module {module} not imported")));
            };
            let d = m.getattr("__dict__")?.cast_into::<PyDict>()?;
            if d.get_item(*key)?.is_none() {
                return Ok(Err(format!("dependency {module}.{key} missing")));
            }
        }
        Ok(Ok(func))
    }

    /// Install the named targets (all of the group when `names` is None). Each target is
    /// all-or-nothing; a refused target does not affect the others.
    pub fn install(&self, py: Python<'_>, names: Option<Vec<String>>, force_jit: bool) -> PyResult<Vec<(String, String, Option<String>)>> {
        guard::set_main_interpreter();
        let mut installed = self.installed.lock().map_err(|_| PyRuntimeError::new_err("poisoned"))?;
        let mut status = self.status.lock().map_err(|_| PyRuntimeError::new_err("poisoned"))?;
        for spec in self.specs {
            if names.as_ref().is_some_and(|n| !n.iter().any(|x| x == spec.name)) {
                continue;
            }
            if installed.iter().any(|i| i.name == spec.name) {
                continue;
            }
            status.retain(|(n, _, _)| *n != spec.name);
            match self.validate(py, spec, force_jit)? {
                Err(why) => {
                    let state = if why == "pending-import" { "pending-import" } else { "refused" };
                    status.push((spec.name, state.into(), Some(why)));
                }
                Ok(func) => {
                    for (module, key) in spec.pins {
                        let d = resolve(py, module)?.expect("validated").getattr("__dict__")?.cast_into::<PyDict>()?;
                        guard::pin(&d, key, &spec.rt.deps)?;
                    }
                    let mut site = Site::vectorcall(&func, spec.kernel as usize)?;
                    spec.rt.orig.store(site.original, Relaxed);
                    match site.install() {
                        Ok(()) => {
                            installed.push(Installed { name: spec.name, site });
                            status.push((spec.name, "installed".into(), None));
                        }
                        Err(e) => {
                            site.release();
                            status.push((spec.name, "refused".into(), Some(e.to_string())));
                        }
                    }
                }
            }
        }
        Ok(status.iter().map(|(n, s, r)| ((*n).to_owned(), s.clone(), r.clone())).collect())
    }

    pub fn uninstall(&self) -> PyResult<()> {
        let mut installed = self.installed.lock().map_err(|_| PyRuntimeError::new_err("poisoned"))?;
        for mut i in installed.drain(..).rev() {
            i.site.uninstall(true);
            i.site.release();
        }
        let mut status = self.status.lock().map_err(|_| PyRuntimeError::new_err("poisoned"))?;
        for s in status.iter_mut() {
            if s.1 == "installed" {
                s.1 = "disabled".into();
            }
        }
        Ok(())
    }

    pub fn status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let status = self.status.lock().map_err(|_| PyRuntimeError::new_err("poisoned"))?;
        let out = PyList::empty(py);
        for spec in self.specs {
            let d = PyDict::new(py);
            d.set_item("name", spec.name)?;
            d.set_item("group", self.name)?;
            let (state, reason) = match status.iter().find(|(n, _, _)| *n == spec.name) {
                Some((_, s, r)) => (s.clone(), r.clone()),
                None => ("disabled".to_owned(), None),
            };
            d.set_item("state", state)?;
            d.set_item("reason", reason)?;
            d.set_item("sites", vec![spec.func])?;
            d.set_item("versions", spec.expected.iter().map(|(m, _)| format!("3.{m}")).collect::<Vec<_>>())?;
            d.set_item("jit_safe", spec.jit_safe)?;
            out.append(d)?;
        }
        Ok(out)
    }

    pub fn counters<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        for spec in self.specs {
            let (hits, fb) = spec.rt.counters.snapshot();
            let d = PyDict::new(py);
            d.set_item("hits", hits)?;
            for (k, v) in fb {
                d.set_item(k, v)?;
            }
            out.set_item(spec.name, d)?;
        }
        Ok(out)
    }

    pub fn reset_counters(&self) {
        for spec in self.specs {
            spec.rt.counters.reset();
        }
    }

    pub fn fingerprints<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        for spec in self.specs {
            out.set_item(spec.name, fingerprint(py, spec.code)?.map(|v| format!("{v:#018x}")))?;
        }
        Ok(out)
    }
}

pub fn set_stats(on: bool) {
    stats::ON.store(on, Relaxed);
}

/// Strong reference kept for the life of the process (constants returned by identity).
pub fn leak(o: &Bound<'_, PyAny>) -> usize {
    o.clone().into_ptr() as usize
}

/// The constant of `code.co_consts` equal to `value` with exactly `value`'s type.
pub fn code_const<'py>(func: &Bound<'py, PyAny>, value: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyAny>>> {
    let consts = func.getattr("__code__")?.getattr("co_consts")?;
    for c in consts.cast_into::<PyTuple>()?.iter() {
        if c.get_type().is(&value.get_type()) && c.eq(value)? {
            return Ok(Some(c));
        }
    }
    Ok(None)
}
