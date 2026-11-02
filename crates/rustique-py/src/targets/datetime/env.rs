//! The environment the datetime kernels rely on, captured at install in the main
//! interpreter, and the per-call checks that it still holds.
//!
//! - [`frame_ok`]: `PyImport_Import` (behind `datetime.strptime`, `strftime`'s
//!   `timetuple()`/`wrap_strftime`, `time.strptime`) takes `__import__` from the *calling
//!   frame's* `globals['__builtins__']`. The kernels require that to be the main
//!   interpreter's builtins module or dict, which also proves the call runs in the main
//!   interpreter (a subinterpreter has its own builtins). No frame (a call from C with no
//!   Python frame) falls back.
//! - [`lc_time_is_c`] / [`thread_locale_is_global`]: glibc's global `LC_TIME` name pointer
//!   equals the static `_nl_C_name` (only "C"/"POSIX" have it; it is never freed, and we
//!   only compare the pointer, so a concurrent `setlocale` cannot make us read freed
//!   memory); `uselocale(0)` is `LC_GLOBAL_LOCALE` (no per-thread locale), for names
//!   formatted by `wcsftime`.
//! - Dependencies (module globals, builtins, `sys.modules` entries) are watched by
//!   [`super::watch`] and re-validated by value whenever the generation moves.

use std::ffi::CStr;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple};

use crate::accel::{Prepare, Refusal};

// ---------------------------------------------------------------------------------------
// glibc locale
// ---------------------------------------------------------------------------------------

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    use super::*;
    pub const LC_TIME: c_int = 2;
    /// `(locale_t) -1`
    pub const LC_GLOBAL_LOCALE: *mut c_void = usize::MAX as *mut c_void;
    unsafe extern "C" {
        pub fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
        pub fn uselocale(loc: *mut c_void) -> *mut c_void;
        pub fn newlocale(mask: c_int, locale: *const c_char, base: *mut c_void) -> *mut c_void;
    }
    /// glibc's public `struct __locale_struct` (bits/types/__locale_t.h).
    #[repr(C)]
    pub struct LocaleStruct {
        pub locales: [*mut c_void; 13],
        pub ctype_b: *const u16,
        pub ctype_tolower: *const c_int,
        pub ctype_toupper: *const c_int,
        pub names: [*const c_char; 13],
    }
}

/// glibc's `_nl_C_name` (the name of the C/POSIX locale), 0 if unknown.
static C_NAME: AtomicUsize = AtomicUsize::new(0);

/// Find `_nl_C_name` without touching the process locale: `newlocale(LC_TIME_MASK, "C",
/// NULL)` returns glibc's static C locale object, whose names all point at it.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn find_c_name() -> Result<usize, String> {
    // SAFETY: plain libc calls; the returned object is glibc's static C locale (freeing it
    // is a no-op, so we never do).
    unsafe {
        let loc = glibc::newlocale(1 << glibc::LC_TIME, c"C".as_ptr(), std::ptr::null_mut());
        if loc.is_null() {
            return Err("newlocale(C) failed".into());
        }
        let names = &(*loc.cast::<glibc::LocaleStruct>()).names;
        let p = names[glibc::LC_TIME as usize];
        // Layout sanity: every category of the C object is the same static "C" string.
        if p.is_null() || CStr::from_ptr(p).to_bytes() != b"C" || (0..6).any(|i| names[i] != p) {
            return Err("unexpected glibc locale layout".into());
        }
        // If LC_TIME currently reads "C", it must be that very pointer.
        let cur = glibc::setlocale(glibc::LC_TIME, std::ptr::null());
        if !cur.is_null() && CStr::from_ptr(cur).to_bytes() == b"C" && cur.cast_const() != p {
            return Err("LC_TIME \"C\" is not glibc's static name".into());
        }
        Ok(p as usize)
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn find_c_name() -> Result<usize, String> {
    Err("only validated on Linux/glibc".into())
}

/// Is the process-wide `LC_TIME` the C/POSIX locale? (glibc returns the name pointer without
/// locking; only compared, never dereferenced.)
#[inline(always)]
pub fn lc_time_is_c() -> bool {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: query form of setlocale.
        let p = unsafe { glibc::setlocale(glibc::LC_TIME, std::ptr::null()) } as usize;
        p == C_NAME.load(Ordering::Relaxed) && p != 0
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        false
    }
}

/// Does this thread use the global locale (no `uselocale`)? `wcsftime` uses the thread's.
#[inline(always)]
pub fn thread_locale_is_global() -> bool {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: query form of uselocale.
        unsafe { glibc::uselocale(std::ptr::null_mut()) == glibc::LC_GLOBAL_LOCALE }
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        false
    }
}

// ---------------------------------------------------------------------------------------
// Captured objects (main interpreter; strong references kept for the process lifetime)
// ---------------------------------------------------------------------------------------

/// What `time.strftime` needs: the `time` module (its function target installs at startup,
/// before anything else is imported). Watches nothing.
#[derive(Debug)]
pub struct TimeEnv {
    pub time_mod: usize,
    pub time_dict: usize,
    pub time_strftime: usize,
    pub struct_time: usize,
}

static TIME_ENV: OnceLock<TimeEnv> = OnceLock::new();

pub fn time_env() -> Option<&'static TimeEnv> {
    TIME_ENV.get()
}

/// Objects every datetime target needs; `usize` addresses of objects we keep alive.
#[derive(Debug)]
pub struct Env {
    pub builtins_mod: usize,
    pub builtins_dict: usize,
    pub import_fn: usize,
    pub sys_modules: usize,
    pub time_mod: usize,
    pub time_dict: usize,
    pub time_strftime: usize,
    pub struct_time: usize,
    pub module_type: usize,
    /// Interned "__builtins__" (immortal static string).
    pub k_builtins: usize,
    /// Originals of the builtins `_strptime`/`locale` code calls, by name.
    pub builtin_objs: Vec<(&'static str, usize)>,
}

static ENV: OnceLock<Env> = OnceLock::new();

pub fn env() -> Option<&'static Env> {
    ENV.get()
}

fn keep(o: &Bound<'_, PyAny>) -> usize {
    o.clone().into_ptr() as usize
}

fn is_builtin_fn(o: &Bound<'_, PyAny>, name: &str, owner: &Bound<'_, PyAny>) -> bool {
    // SAFETY: type-checked reads of a builtin function object.
    unsafe {
        let p = o.as_ptr();
        ffi::PyCFunction_Check(p) != 0 && {
            let f = p.cast::<ffi::PyCFunctionObject>();
            let def = (*f).m_ml;
            !def.is_null()
                && CStr::from_ptr((*def).ml_name).to_bytes() == name.as_bytes()
                && (*f).m_self == owner.as_ptr()
        }
    }
}

/// Capture what `time.strftime` needs (idempotent): the C locale's name and the `time`
/// module. Plain dict reads.
pub fn capture_time(cx: &Prepare<'_>) -> Result<&'static TimeEnv, Refusal> {
    if let Some(t) = TIME_ENV.get() {
        return Ok(t);
    }
    let py = cx.py();
    C_NAME.store(find_c_name().map_err(Refusal::from)?, Ordering::Relaxed);
    let time = cx.loaded_module("time")?;
    let tdict = time.dict();
    let strftime =
        cx.get_opt_s(&tdict, pyo3::intern!(py, "strftime"))?.ok_or_else(|| Refusal::new("time.strftime missing"))?;
    if !is_builtin_fn(&strftime, "strftime", time.as_any()) {
        return Err(Refusal::new("time.strftime is not CPython's"));
    }
    let struct_time = cx
        .get_opt_s(&tdict, pyo3::intern!(py, "struct_time"))?
        .ok_or_else(|| Refusal::new("time.struct_time missing"))?;
    let t = TimeEnv {
        time_mod: keep(time.as_any()),
        time_dict: keep(tdict.as_any()),
        time_strftime: keep(&strftime),
        struct_time: keep(&struct_time),
    };
    Ok(TIME_ENV.get_or_init(|| t))
}

/// Capture the environment (idempotent). Fails if anything is not CPython's own. Runs in
/// `prepare` (possibly inside the `sys.modules` watcher): plain dict reads only.
pub fn capture(cx: &Prepare<'_>) -> Result<&'static Env, Refusal> {
    if let Some(e) = ENV.get() {
        return Ok(e);
    }
    let py = cx.py();
    let t = capture_time(cx)?;
    let builtins = cx.loaded_module("builtins")?;
    let bdict = builtins.dict();
    let sys = cx.loaded_module("sys")?;
    let modules = cx
        .get_opt_s(&sys.dict(), pyo3::intern!(py, "modules"))?
        .ok_or_else(|| Refusal::new("sys.modules missing"))?
        .cast_into::<PyDict>()
        .map_err(PyErr::from)?;
    let builtin = |name: &'static str, key: &Bound<'_, PyString>| -> Result<usize, Refusal> {
        let o = cx.get_opt_s(&bdict, key)?.ok_or_else(|| Refusal::from(format!("builtins.{name} missing")))?;
        if !is_builtin_fn(&o, name, builtins.as_any()) {
            return Err(Refusal::from(format!("builtins.{name} is not CPython's")));
        }
        Ok(keep(&o))
    };
    let import_fn = builtin("__import__", pyo3::intern!(py, "__import__"))?;
    let isinstance = builtin("isinstance", pyo3::intern!(py, "isinstance"))?;
    let len = builtin("len", pyo3::intern!(py, "len"))?;
    // Builtins read by `_strptime._strptime`, its wrappers and `locale.getlocale` & co.
    // The builtin types and exceptions are CPython's static objects (no lookups needed);
    // the three builtin functions are read from the dict. Revalidation compares the dict's
    // entries with these by identity.
    // SAFETY: static type objects and exception pointers of the running interpreter.
    let statics: [(&'static str, usize); 8] = unsafe {
        [
            ("enumerate", (&raw mut ffi::PyEnum_Type) as usize),
            ("str", (&raw mut ffi::PyUnicode_Type) as usize),
            ("int", (&raw mut ffi::PyLong_Type) as usize),
            ("tuple", (&raw mut ffi::PyTuple_Type) as usize),
            ("range", (&raw mut ffi::PyRange_Type) as usize),
            ("ValueError", ffi::PyExc_ValueError as usize),
            ("TypeError", ffi::PyExc_TypeError as usize),
            ("KeyError", ffi::PyExc_KeyError as usize),
        ]
    };
    let mut builtin_objs: Vec<(&'static str, usize)> = statics.to_vec();
    builtin_objs.extend([("__import__", import_fn), ("isinstance", isinstance), ("len", len)]);
    let k = PyString::intern(py, "__builtins__");
    let e = Env {
        builtins_mod: keep(builtins.as_any()),
        builtins_dict: keep(bdict.as_any()),
        import_fn,
        sys_modules: keep(modules.as_any()),
        time_mod: t.time_mod,
        time_dict: t.time_dict,
        time_strftime: t.time_strftime,
        struct_time: t.struct_time,
        module_type: keep(builtins.get_type().as_any()),
        k_builtins: keep(k.as_any()),
        builtin_objs,
    };
    // (Identity with the captured builtins is checked by `common_ok` before a kernel's
    // first hit and whenever a watched dict changes; nothing to look up here.)
    // Watch what the `_datetime` targets depend on (`time.strftime` needs none of it, so a
    // process that never imports `datetime` registers no watch).
    // builtins and the time module as whole dicts (one watch each; they change rarely),
    // `sys.modules` per key (it changes on every import; `_strptime` reaches `locale`
    // through its own global, never through `sys.modules`).
    crate::accel::guard::watch_dict_generation(&bdict, &super::watch::GEN)?;
    super::watch::watch(&modules, &["time", "_strptime", "zoneinfo", "_zoneinfo"])?;
    // SAFETY: captured dict (kept alive).
    let tdict = unsafe { obj(py, t.time_dict) }.cast_into::<PyDict>().map_err(PyErr::from)?;
    crate::accel::guard::watch_dict_generation(&tdict, &super::watch::GEN)?;
    Ok(ENV.get_or_init(|| e))
}

// ---------------------------------------------------------------------------------------
// Per-call checks
// ---------------------------------------------------------------------------------------

/// The calling frame's `globals['__builtins__']` is the main interpreter's builtins (so
/// `PyImport_Import` would call the original `__import__`, which `watch` pins).
///
/// # Safety
/// Attached thread.
#[inline(always)]
pub unsafe fn frame_ok(e: &Env) -> bool {
    // SAFETY: attached thread; pointers compared only.
    unsafe {
        let g = ffi::PyEval_GetGlobals();
        if g.is_null() || ffi::PyDict_CheckExact(g) == 0 {
            return false;
        }
        let b = ffi::PyDict_GetItemWithError(g, e.k_builtins as *mut ffi::PyObject);
        if b.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        b as usize == e.builtins_dict || b as usize == e.builtins_mod
    }
}

// ---------------------------------------------------------------------------------------
// Validation helpers (run when the watch generation moved)
// ---------------------------------------------------------------------------------------

/// `dict[key]` read inside the dict's critical section (see `watch` for why).
pub fn item<'py>(dict: &Bound<'py, PyDict>, key: &str) -> Option<Bound<'py, PyAny>> {
    pyo3::sync::critical_section::with_critical_section(dict.as_any(), || dict.get_item(key).ok().flatten())
}

/// Borrow a captured object.
///
/// # Safety
/// `addr` is an object captured by this module (kept alive).
pub unsafe fn obj<'py>(py: Python<'py>, addr: usize) -> Bound<'py, PyAny> {
    // SAFETY: per the caller.
    unsafe { Bound::from_borrowed_ptr(py, addr as *mut ffi::PyObject) }
}

/// Common dependencies of every target that imports through `PyImport_Import` and uses
/// the `time` module: builtins, `sys.modules['time']`, `time.strftime`, `time.struct_time`.
pub fn common_ok(py: Python<'_>, e: &Env) -> bool {
    // SAFETY: captured objects.
    let (bdict, modules, tdict) = unsafe {
        (
            obj(py, e.builtins_dict).cast_into::<PyDict>().ok(),
            obj(py, e.sys_modules).cast_into::<PyDict>().ok(),
            obj(py, e.time_dict).cast_into::<PyDict>().ok(),
        )
    };
    let (Some(bdict), Some(modules), Some(tdict)) = (bdict, modules, tdict) else { return false };
    let is = |o: Option<Bound<'_, PyAny>>, addr: usize| o.is_some_and(|o| o.as_ptr() as usize == addr);
    e.builtin_objs.iter().all(|&(name, addr)| is(item(&bdict, name), addr))
        && is(item(&modules, "time"), e.time_mod)
        && is(item(&tdict, "strftime"), e.time_strftime)
        && is(item(&tdict, "struct_time"), e.struct_time)
        // SAFETY: captured module.
        && unsafe { ffi::Py_TYPE(e.time_mod as *mut ffi::PyObject) } as usize == e.module_type
}

/// `obj` is an exact tuple of exact `str`.
pub fn is_str_tuple(o: &Bound<'_, PyAny>) -> bool {
    o.cast::<PyTuple>()
        .is_ok_and(|t| t.is_exact_instance_of::<PyTuple>() && t.iter().all(|s| s.is_exact_instance_of::<PyString>()))
}
