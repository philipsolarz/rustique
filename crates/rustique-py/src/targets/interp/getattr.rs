//! `interp.getattr` and `interp.hasattr`: `ml_meth` kernels on `builtins.getattr` /
//! `builtins.hasattr` (METH_FASTCALL) that resolve through the interp tier's attribute cache
//! (`rq_getattr_fast`, c/rq_attrcache.h). `getattr(o, name)` is `PyObject_GetAttr`,
//! `getattr(o, name, default)` and `hasattr(o, name)` are `PyObject_GetOptionalAttr`; for a type
//! with `PyObject_GenericGetAttr` both run `_PyObject_GenericGetAttrWithDict`, whose resolution a
//! cache hit replays (an attribute absent from the MRO and the instance is a hit only for the
//! suppressing forms: CPython returns the default without raising). Everything else (wrong
//! argument counts, non-str or non-immortal names, hooks, data descriptors, misses) calls the
//! original with the unchanged arguments.

use std::ffi::c_int;
use std::sync::atomic::{AtomicUsize, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;

use crate::accel::{Category, Counters, Fingerprints, Original, Prepare, Reason, Refusal, TargetSpec};

type GetattrFast = unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject, c_int, *mut *mut ffi::PyObject) -> c_int;

/// The handler set's fast path, set by `prepare` before a kernel is reachable.
static FAST: AtomicUsize = AtomicUsize::new(0);
static ORIG_GETATTR: Original<ffi::PyCFunctionFast> = Original::new();
static ORIG_HASATTR: Original<ffi::PyCFunctionFast> = Original::new();
static C_GETATTR: Counters = Counters::new();
static C_HASATTR: Counters = Counters::new();

/// `ml_name`/`ml_flags`/`ml_doc` of the originals (3.15.1, 3.15.1t, 3.15.2, 3.15.2t).
const FP_GETATTR: Fingerprints = &[(3, 15, "bfebeb637da8b8cb")];
const FP_HASATTR: Fingerprints = &[(3, 15, "790a34eabfc9408a")];

#[inline(always)]
fn fast() -> GetattrFast {
    // SAFETY: set to the set's `getattr_fast` before the kernels are installed; never cleared.
    unsafe { std::mem::transmute::<usize, GetattrFast>(FAST.load(Ordering::Relaxed)) }
}

unsafe extern "C" fn k_getattr(slf: *mut ffi::PyObject, args: *mut *mut ffi::PyObject, nargs: ffi::Py_ssize_t) -> *mut ffi::PyObject {
    if nargs == 2 || nargs == 3 {
        let mut res = std::ptr::null_mut();
        // SAFETY: METH_FASTCALL: `nargs` valid argument pointers.
        let r = unsafe { fast()(*args, *args.add(1), c_int::from(nargs == 3), &mut res) };
        if r > 0 {
            C_GETATTR.hit();
            if res.is_null() {
                // SAFETY: the default argument, a live object.
                return unsafe { ffi::Py_NewRef(*args.add(2)) };
            }
            return res;
        }
        if r < 0 {
            return std::ptr::null_mut();
        }
    }
    C_GETATTR.fallback(Reason::Args);
    // SAFETY: the original builtin with the unchanged arguments.
    unsafe { ORIG_GETATTR.get()(slf, args, nargs) }
}

unsafe extern "C" fn k_hasattr(slf: *mut ffi::PyObject, args: *mut *mut ffi::PyObject, nargs: ffi::Py_ssize_t) -> *mut ffi::PyObject {
    if nargs == 2 {
        let mut res = std::ptr::null_mut();
        // SAFETY: METH_FASTCALL: two valid argument pointers.
        let r = unsafe { fast()(*args, *args.add(1), 1, &mut res) };
        if r > 0 {
            C_HASATTR.hit();
            // SAFETY: `res` is a new reference or NULL; the bool singletons are immortal.
            unsafe {
                if res.is_null() {
                    return ffi::Py_NewRef(ffi::Py_False());
                }
                ffi::Py_DECREF(res);
                return ffi::Py_NewRef(ffi::Py_True());
            }
        }
        if r < 0 {
            return std::ptr::null_mut();
        }
    }
    C_HASATTR.fallback(Reason::Args);
    // SAFETY: the original builtin with the unchanged arguments.
    unsafe { ORIG_HASATTR.get()(slf, args, nargs) }
}

fn declare(cx: &mut Prepare<'_>, name: &str, fp: Fingerprints, kernel: ffi::PyCFunctionFast, orig: &'static Original<ffi::PyCFunctionFast>) -> Result<(), Refusal> {
    let loc = rustique_interp::locate().map_err(|r| super::refusal(&r))?;
    FAST.store(loc.getattr_fast() as usize, Ordering::Release);
    let builtins = cx.loaded_module("builtins")?;
    let f = cx.get(&builtins.dict(), name)?;
    cx.fingerprint_c(&f, fp)?;
    cx.ml_meth(&f, kernel, orig).map_err(|r| r.or_category(Category::Conflict))
}

fn prepare_getattr(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    declare(cx, "getattr", FP_GETATTR, k_getattr, &ORIG_GETATTR)
}

fn prepare_hasattr(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    declare(cx, "hasattr", FP_HASATTR, k_hasattr, &ORIG_HASATTR)
}

pub static GETATTR: TargetSpec = TargetSpec {
    name: "interp.getattr",
    group: super::GROUP,
    module: None,
    versions: super::VERSIONS,
    free_threaded: true,
    counters: &C_GETATTR,
    prepare: prepare_getattr,
    ..TargetSpec::DEFAULT
};

pub static HASATTR: TargetSpec = TargetSpec {
    name: "interp.hasattr",
    group: super::GROUP,
    module: None,
    versions: super::VERSIONS,
    free_threaded: true,
    counters: &C_HASATTR,
    prepare: prepare_hasattr,
    ..TargetSpec::DEFAULT
};
