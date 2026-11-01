//! Plumbing shared by the utils groups (`random`, `textwrap`, `urllib`, `statistics`,
//! `ipaddress`): pure-Python function targets installed with `PyFunction_SetVectorcall`
//! (docs/ACCELERATION.md §3.7), their per-call guards (§4) and a private install/uninstall
//! used until accel-core's `TargetSpec` registry lands (then `prepare` maps onto it).
//!
//! OWNER: utils workstream.
//!
//! A [`FnTarget`] names one function (`random.Random.randint`), its kernel, the minors it is
//! validated on with the fingerprint of the original's code per minor, and the module
//! globals/builtins the original reads at call time (pinned through `accel::guard::pin`).
//! Install, per target and all-or-nothing: version gate, exact `function` object found in
//! the module or class `__dict__` (never through `getattr`), fingerprint, the original
//! vectorcall is `_PyFunction_Vectorcall`, target-specific preparation and self-test,
//! dependency pins, then the site swap.
//!
//! Per call a kernel checks, before anything observable: its own argument subset, then
//! [`Guard::ready`]: the function still has the code/defaults/kwdefaults it was validated
//! with (assignments to `__code__`/`__defaults__` keep the patched vectorcall), the pinned
//! dependencies are intact, and no tool listens (global `PY_START` or local events on the
//! code), so tools see CPython's frames. Otherwise it calls the saved original with the
//! unchanged arguments.

use std::ffi::c_int;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering::Relaxed};

use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyFrozenSet, PyList, PyString, PyTuple};

use crate::accel::guard::{self, DepFlag, Tools};
use crate::accel::site::Site;
use crate::accel::stats::{self, Counters, Reason};
use crate::accel::version;

pub type O = *mut ffi::PyObject;

unsafe extern "C" {
    /// `PyAPI_FUNC` in `Include/internal/pycore_object.h` (3.14, 3.15): the lookup behind
    /// `LOAD_ATTR` for a method call. Returns 1 and the unbound descriptor when the type
    /// provides a method descriptor that the instance does not shadow, else 0 and the
    /// attribute (or NULL with an error).
    pub fn _PyObject_GetMethod(obj: O, name: O, method: *mut O) -> c_int;
    /// `PyAPI_FUNC` in `Include/cpython/object.h`: MRO lookup, borrowed (compare only).
    pub fn _PyType_Lookup(tp: *mut ffi::PyTypeObject, name: O) -> O;
}

/// `_PyFunction_Vectorcall`, read from the first function patched (every other site must
/// hold the same pointer before it is patched).
static PYFUNC_VC: AtomicUsize = AtomicUsize::new(0);

/// Call the saved original with the unchanged arguments.
///
/// # Safety
/// Vectorcall arguments as received by a kernel installed by [`install_group`].
#[inline(always)]
pub unsafe fn call_original(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: the saved `_PyFunction_Vectorcall` (set before any kernel is installed).
    unsafe { std::mem::transmute::<usize, ffi::vectorcallfunc>(PYFUNC_VC.load(Relaxed))(func, args, nargsf, kw) }
}

#[inline(always)]
pub fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

#[inline(always)]
pub unsafe fn is_type(o: O, t: *mut ffi::PyTypeObject) -> bool {
    // SAFETY: live object.
    unsafe { ffi::Py_TYPE(o) == t }
}

/// Per-call guard state of one target.
pub struct Guard {
    code: AtomicPtr<ffi::PyObject>,
    defaults: AtomicPtr<ffi::PyObject>,
    kwdefaults: AtomicPtr<ffi::PyObject>,
    tools: Tools,
    pub deps: DepFlag,
    pub counters: Counters,
}

impl Guard {
    pub const fn new() -> Self {
        Guard {
            code: AtomicPtr::new(std::ptr::null_mut()),
            defaults: AtomicPtr::new(std::ptr::null_mut()),
            kwdefaults: AtomicPtr::new(std::ptr::null_mut()),
            tools: Tools::new(),
            deps: DepFlag::new(),
            counters: Counters::new(),
        }
    }

    /// Everything but the argument subset: the function is the one validated, the pinned
    /// dependencies are intact, no tool listens. Counts the reason when it says no.
    ///
    /// # Safety
    /// `func` is the patched function object (attached thread).
    #[inline(always)]
    pub unsafe fn ready(&self, func: O) -> bool {
        // SAFETY: `func` is a function object (the kernel is installed on it only).
        unsafe {
            let f = func.cast::<ffi::PyFunctionObject>();
            if (*f).func_code != self.code.load(Relaxed)
                || (*f).func_defaults != self.defaults.load(Relaxed)
                || (*f).func_kwdefaults != self.kwdefaults.load(Relaxed)
                || !self.deps.ok()
            {
                self.counters.fallback(Reason::Deps);
                return false;
            }
            if !self.tools.quiet() || !guard::code_locally_quiet((*f).func_code) {
                self.counters.fallback(Reason::Tools);
                return false;
            }
            true
        }
    }

    fn capture(&self, func: &Bound<'_, PyAny>) {
        let f = func.as_ptr().cast::<ffi::PyFunctionObject>();
        // SAFETY: exact function object (checked by the caller).
        unsafe {
            self.code.store((*f).func_code, Relaxed);
            self.defaults.store((*f).func_defaults, Relaxed);
            self.kwdefaults.store((*f).func_kwdefaults, Relaxed);
        }
    }
}

impl Default for Guard {
    fn default() -> Self {
        Self::new()
    }
}

/// Target-specific preparation: fill the kernel's statics, run the self-test. `Ok(Err)` is
/// a refusal (reason), `Err` an unexpected Python error (also a refusal).
pub type Prepare = fn(Python<'_>, &Bound<'_, PyModule>) -> PyResult<Result<(), String>>;

/// One pure-Python function target.
pub struct FnTarget {
    /// Dotted target name, e.g. `random.randint`.
    pub name: &'static str,
    pub group: &'static str,
    /// Module that defines the function (must already be imported).
    pub module: &'static str,
    /// Attribute path inside the module, through `__dict__`s: `["Random", "randint"]`.
    pub path: &'static [&'static str],
    pub kernel: ffi::vectorcallfunc,
    /// `(minor, fingerprint of the original's code)` for each validated 3.x minor.
    pub fingerprints: &'static [(u8, u64)],
    /// Validated on free-threaded builds too.
    pub free_threaded: bool,
    /// `(module, global)` pairs the original reads at call time, pinned to their objects.
    pub deps: &'static [(&'static str, &'static str)],
    pub guard: &'static Guard,
    pub prepare: Option<Prepare>,
}

/// Install state of one target (what `status()` reports).
#[derive(Debug)]
pub struct Entry {
    pub name: &'static str,
    pub group: &'static str,
    pub state: &'static str,
    pub reason: Option<String>,
    pub site: Option<Site>,
    pub versions: Vec<u8>,
}

/// The install state of a group (one per group, a static).
pub type GroupState = Mutex<Vec<Entry>>;

fn sys_module<'py>(py: Python<'py>, name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    let modules = py.import("sys")?.getattr("modules")?;
    let m = modules.cast::<PyDict>()?.get_item(name)?;
    Ok(m)
}

/// The object at `path` inside module `m`, through `__dict__` lookups only.
fn resolve<'py>(m: &Bound<'py, PyAny>, path: &[&str]) -> PyResult<Option<Bound<'py, PyAny>>> {
    let mut cur = m.clone();
    for part in path {
        let d = cur.getattr("__dict__")?;
        let next = if let Ok(d) = d.cast::<PyDict>() {
            d.get_item(*part)?
        } else {
            // mappingproxy of a class
            match d.get_item(*part) {
                Ok(v) => Some(v),
                Err(e) if e.is_instance_of::<pyo3::exceptions::PyKeyError>(m.py()) => None,
                Err(e) => return Err(e),
            }
        };
        match next {
            Some(n) => cur = n,
            None => return Ok(None),
        }
    }
    Ok(Some(cur))
}

/// Install every target of a group that is not installed yet; returns nothing — the
/// outcome of each target is in `state` (and `status()`).
pub fn install_group(py: Python<'_>, targets: &[FnTarget], state: &GroupState) -> PyResult<()> {
    guard::set_main_interpreter();
    let mut entries = state.lock().map_err(|_| PyRuntimeError::new_err("poisoned"))?;
    for t in targets {
        let pos = entries.iter().position(|e| e.name == t.name);
        if let Some(p) = pos
            && entries[p].state == "installed"
        {
            continue;
        }
        let versions: Vec<u8> = t.fingerprints.iter().map(|(m, _)| *m).collect();
        let (st, reason, site) = match install_one(py, t, &versions) {
            Ok(Ok(site)) => ("installed", None, Some(site)),
            Ok(Err((st, why))) => (st, Some(why), None),
            Err(e) => ("refused", Some(format!("error: {e}")), None),
        };
        let entry = Entry { name: t.name, group: t.group, state: st, reason, site, versions };
        match pos {
            Some(p) => entries[p] = entry,
            None => entries.push(entry),
        }
    }
    Ok(())
}

type Refusal = (&'static str, String);

fn install_one(py: Python<'_>, t: &FnTarget, versions: &[u8]) -> PyResult<Result<Site, Refusal>> {
    if let Err(why) = version::supported(py, versions)? {
        return Ok(Err(("refused", why)));
    }
    if cfg!(Py_GIL_DISABLED) && !t.free_threaded {
        return Ok(Err(("refused", "not validated on free-threaded builds".into())));
    }
    let Some(module) = sys_module(py, t.module)? else {
        return Ok(Err(("pending-import", format!("{} not imported", t.module))));
    };
    let Some(func) = resolve(&module, t.path)? else {
        return Ok(Err(("refused", format!("{}.{} not found", t.module, t.path.join(".")))));
    };
    // SAFETY: type check of a live object.
    if unsafe { !is_type(func.as_ptr(), &raw mut ffi::PyFunction_Type) } {
        return Ok(Err(("refused", "not a pure-Python function".into())));
    }
    let minor = version::compiled_minor();
    let fp = code_fingerprint(&func.getattr("__code__")?)?;
    match t.fingerprints.iter().find(|(m, _)| *m == minor) {
        Some((_, want)) if *want == fp => {}
        _ => return Ok(Err(("refused", format!("fingerprint {fp:#018x} not validated")))),
    }
    // SAFETY: exact function object.
    let vc = unsafe { (*func.as_ptr().cast::<ffi::PyFunctionObject>()).vectorcall.map_or(0, |f| f as usize) };
    let saved = PYFUNC_VC.load(Relaxed);
    if saved == 0 {
        if vc == 0 {
            return Ok(Err(("refused", "function has no vectorcall".into())));
        }
        PYFUNC_VC.store(vc, Relaxed);
    } else if saved != vc {
        return Ok(Err(("refused", "vectorcall is not _PyFunction_Vectorcall (already patched?)".into())));
    }
    if let Some(prep) = t.prepare {
        match prep(py, module.cast::<PyModule>()?) {
            Ok(Ok(())) => {}
            Ok(Err(why)) => return Ok(Err(("refused", why))),
            Err(e) => return Ok(Err(("refused", format!("self-test error: {e}")))),
        }
    }
    for (m, key) in t.deps {
        let Some(dm) = sys_module(py, m)? else {
            return Ok(Err(("refused", format!("dependency module {m} not imported"))));
        };
        let d = dm.getattr("__dict__")?;
        guard::pin(d.cast::<PyDict>()?, key, &t.guard.deps)
            .map_err(|e| PyRuntimeError::new_err(format!("pin {m}.{key}: {e}")))?;
    }
    t.guard.capture(&func);
    let mut site = Site::vectorcall(&func, t.kernel as usize)?;
    if let Err(e) = site.install() {
        let why = e.to_string();
        site.release();
        return Ok(Err(("refused", why)));
    }
    Ok(Ok(site))
}

/// Restore every installed target of a group (and the functions' specializer versions).
pub fn uninstall_group(state: &GroupState) -> PyResult<()> {
    let mut entries = state.lock().map_err(|_| PyRuntimeError::new_err("poisoned"))?;
    for e in entries.iter_mut() {
        if let Some(mut site) = e.site.take() {
            site.uninstall(true);
            site.release();
        }
        e.state = "disabled";
        e.reason = None;
    }
    Ok(())
}

/// `rustique.status()`-shaped entries of a group.
pub fn status_list<'py>(py: Python<'py>, state: &GroupState) -> PyResult<Bound<'py, PyList>> {
    let entries = state.lock().map_err(|_| PyRuntimeError::new_err("poisoned"))?;
    let out = PyList::empty(py);
    for e in entries.iter() {
        let d = PyDict::new(py);
        d.set_item("name", e.name)?;
        d.set_item("group", e.group)?;
        d.set_item("state", e.state)?;
        d.set_item("reason", e.reason.as_deref())?;
        let sites: Vec<String> = e.site.iter().map(|s| format!("vectorcall:{}", s.name)).collect();
        d.set_item("sites", sites)?;
        d.set_item("versions", e.versions.iter().map(|m| format!("3.{m}")).collect::<Vec<_>>())?;
        out.append(d)?;
    }
    Ok(out)
}

/// Counters of the targets of a group: `{name: {"hits": n, "args": n, ...}}`.
pub fn counters_dict<'py>(py: Python<'py>, targets: &[FnTarget]) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    for t in targets {
        let (hits, fb) = t.guard.counters.snapshot();
        let d = PyDict::new(py);
        d.set_item("hits", hits)?;
        for (k, v) in fb {
            d.set_item(k, v)?;
        }
        out.set_item(t.name, d)?;
    }
    Ok(out)
}

pub fn reset_counters(targets: &[FnTarget]) {
    for t in targets {
        t.guard.counters.reset();
    }
}

pub fn set_stats(on: bool) {
    stats::ON.store(on, Relaxed);
}

// ---------------------------------------------------------------------------------------
// Code fingerprint (contract §3.2): path- and line-independent hash of a code object.
// ---------------------------------------------------------------------------------------

struct Fnv(u64);

impl Fnv {
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 ^= b.len() as u64;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
    }
    fn tag(&mut self, t: &str) {
        self.bytes(t.as_bytes());
    }
}

fn hash_const(h: &mut Fnv, c: &Bound<'_, PyAny>) -> PyResult<()> {
    // SAFETY: type pointer comparison.
    if unsafe { is_type(c.as_ptr(), &raw mut ffi::PyCode_Type) } {
        h.tag("code");
        return hash_code(h, c);
    }
    if let Ok(t) = c.cast::<PyTuple>() {
        h.tag("tuple");
        h.bytes(&(t.len() as u64).to_le_bytes());
        for x in t.iter() {
            hash_const(h, &x)?;
        }
        return Ok(());
    }
    if let Ok(fs) = c.cast::<PyFrozenSet>() {
        h.tag("frozenset");
        let mut reprs: Vec<String> = fs.iter().map(|x| x.repr().map(|r| r.to_string())).collect::<PyResult<_>>()?;
        reprs.sort();
        for r in reprs {
            h.tag(&r);
        }
        return Ok(());
    }
    h.tag(&c.get_type().name()?.to_string());
    h.tag(&c.repr()?.to_string());
    Ok(())
}

fn hash_code(h: &mut Fnv, code: &Bound<'_, PyAny>) -> PyResult<()> {
    for attr in ["co_argcount", "co_posonlyargcount", "co_kwonlyargcount", "co_flags"] {
        h.bytes(&code.getattr(attr)?.extract::<i64>()?.to_le_bytes());
    }
    h.tag(&code.getattr("co_name")?.extract::<String>()?);
    h.bytes(code.getattr("co_code")?.cast::<PyBytes>()?.as_bytes());
    h.bytes(code.getattr("co_exceptiontable")?.cast::<PyBytes>()?.as_bytes());
    for attr in ["co_names", "co_varnames", "co_freevars", "co_cellvars"] {
        let t = code.getattr(attr)?;
        let t = t.cast::<PyTuple>()?;
        h.tag(attr);
        for s in t.iter() {
            h.tag(&s.cast::<PyString>()?.to_cow()?);
        }
    }
    h.tag("consts");
    for c in code.getattr("co_consts")?.cast::<PyTuple>()?.iter() {
        hash_const(h, &c)?;
    }
    Ok(())
}

/// Canonical fingerprint of a code object: bytecode, exception table, names, argument
/// counts, flags and constants (recursively); independent of file name and line numbers.
pub fn code_fingerprint(code: &Bound<'_, PyAny>) -> PyResult<u64> {
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    hash_code(&mut h, code)?;
    Ok(h.0)
}

/// `code_fingerprint(func.__code__)` for Python (used to record fingerprints per build).
#[pyfunction]
pub fn fingerprint(func: &Bound<'_, PyAny>) -> PyResult<u64> {
    code_fingerprint(&func.getattr("__code__")?)
}

// ---------------------------------------------------------------------------------------
// Small helpers for kernels.
// ---------------------------------------------------------------------------------------

/// A new reference to an interned str kept for the life of the process.
pub fn interned(py: Python<'_>, s: &str) -> O {
    PyString::intern(py, s).into_ptr()
}

/// Store a strong reference in a static slot (the previous one, if any, is leaked: kernels
/// may still read it on another thread).
pub fn keep(slot: &AtomicPtr<ffi::PyObject>, obj: &Bound<'_, PyAny>) {
    slot.store(obj.clone().into_ptr(), Relaxed);
}

/// Value of an exact `int` (or `bool`) that fits an `i64`, without side effects.
///
/// # Safety
/// Live object; attached thread.
#[inline(always)]
pub unsafe fn int_value(o: O) -> Option<i64> {
    // SAFETY: type-checked conversions; overflow reported through the flag (no error set).
    unsafe {
        let t = ffi::Py_TYPE(o);
        if t == &raw mut ffi::PyLong_Type {
            let mut overflow: c_int = 0;
            let v = ffi::PyLong_AsLongLongAndOverflow(o, &mut overflow);
            if overflow == 0 && !(v == -1 && !ffi::PyErr_Occurred().is_null()) {
                return Some(v);
            }
            None
        } else if t == &raw mut ffi::PyBool_Type {
            Some(i64::from(o == ffi::Py_True()))
        } else {
            None
        }
    }
}

/// An `int` object for an `i128` (small ints come from CPython's cache).
///
/// # Safety
/// Attached thread.
#[inline]
pub unsafe fn long_from_i128(v: i128) -> O {
    // SAFETY: plain constructors.
    unsafe {
        if let Ok(x) = i64::try_from(v) {
            ffi::PyLong_FromLongLong(x)
        } else {
            let bytes = v.to_ne_bytes();
            ffi::PyLong_FromNativeBytes(bytes.as_ptr().cast(), 16, -1)
        }
    }
}

/// Run `f` holding `o`'s critical section on free-threaded builds (nothing on GIL builds,
/// where no other thread runs while the kernel executes native code).
///
/// # Safety
/// Attached thread; `f` must not run Python code.
#[inline(always)]
pub unsafe fn with_cs<R>(o: O, f: impl FnOnce() -> R) -> R {
    #[cfg(Py_GIL_DISABLED)]
    {
        let mut cs = std::mem::MaybeUninit::<ffi::PyCriticalSection>::uninit();
        // SAFETY: the section lives on this frame until `End`.
        unsafe {
            ffi::PyCriticalSection_Begin(cs.as_mut_ptr(), o);
            let r = f();
            ffi::PyCriticalSection_End(cs.as_mut_ptr());
            r
        }
    }
    #[cfg(not(Py_GIL_DISABLED))]
    {
        let _ = o;
        f()
    }
}

/// [`with_cs`] for two objects (`b` may be NULL or equal to `a`).
///
/// # Safety
/// As [`with_cs`].
#[inline(always)]
pub unsafe fn with_cs2<R>(a: O, b: O, f: impl FnOnce() -> R) -> R {
    #[cfg(Py_GIL_DISABLED)]
    {
        // SAFETY: as above.
        unsafe {
            if b.is_null() || a == b {
                return with_cs(a, f);
            }
            let mut cs = std::mem::MaybeUninit::<ffi::PyCriticalSection2>::uninit();
            ffi::PyCriticalSection2_Begin(cs.as_mut_ptr(), a, b);
            let r = f();
            ffi::PyCriticalSection2_End(cs.as_mut_ptr());
            r
        }
    }
    #[cfg(not(Py_GIL_DISABLED))]
    {
        let _ = (a, b);
        f()
    }
}

/// Is `kw` (a kwnames tuple entry) the interned name `name`? Exact str only.
///
/// # Safety
/// Live str objects.
#[inline(always)]
pub unsafe fn name_is(kw: O, name: O) -> bool {
    // SAFETY: both str.
    unsafe { kw == name || (is_type(kw, &raw mut ffi::PyUnicode_Type) && ffi::PyUnicode_Compare(kw, name) == 0) }
}
