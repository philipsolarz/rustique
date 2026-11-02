//! Dynamic vectorcall sites: functions created after a target was installed (generated
//! `__init__`/`__new__` of dataclasses and namedtuples, `contextmanager` helpers), patched
//! at their first use or by an adoption scan (`TargetSpec::on_install`).
//!
//! No registry entry per site (a weak reference and a lock would cost more than the
//! fallback saves on cheap entry points): a site is retired lazily. Every kernel installed
//! here checks its target's [`Counters::live`](super::Counters::live) on each call (one
//! relaxed load) and, when the target is no longer live (`disable()`, a suspension, the
//! exit hook), calls [`retire`] and the original. Until its next call a retired function
//! keeps the kernel pointer, which is invisible: the kernel only ever runs CPython's code
//! then. `status()["targets"][i]["dynamic_sites"]` counts sites installed and not retired.
//!
//! ```ignore
//! unsafe extern "C" fn k(f: O, a: *const O, n: usize, kw: O) -> O {
//!     if !C.live() {
//!         accel::dynamic::retire(f, k, &accel::FUNCTION_VECTORCALL, &C);
//!         return accel::FUNCTION_VECTORCALL.get()(f, a, n, kw);
//!     }
//!     ...
//! }
//! ```

use std::sync::atomic::Ordering;

use pyo3::ffi;
use pyo3::prelude::*;

use super::spec::{Original, TargetSpec};
use super::stats::Counters;

/// Put `kernel` behind `func`'s vectorcall for target `spec`, which must be live. `func` must
/// be an exact pure-Python function whose vectorcall is `original`'s
/// (`_PyFunction_Vectorcall`: [`super::FUNCTION_VECTORCALL`], captured by the target's
/// `prepare` with `Prepare::vectorcall` or [`capture_function_vectorcall`]). Returns
/// whether the site was installed (`false`, nothing changed: target not live, not a plain
/// function, or already patched). Uses `PyFunction_SetVectorcall` (stops the world on
/// free-threaded builds). Attached thread, outside watchers; callable from a kernel. The
/// kernel's guards are the target's own (tools, pins), plus the `live` check (module docs).
pub fn install(
    _py: Python<'_>,
    spec: &'static TargetSpec,
    func: &Bound<'_, PyAny>,
    kernel: ffi::vectorcallfunc,
    original: &'static Original<ffi::vectorcallfunc>,
) -> bool {
    if !spec.counters.live() || !original.is_set() {
        return false;
    }
    let p = func.as_ptr();
    // SAFETY: type check, then a layout read and the swap on an exact function object.
    unsafe {
        if ffi::Py_TYPE(p) != &raw mut ffi::PyFunction_Type {
            return false;
        }
        let cur = (*p.cast::<ffi::PyFunctionObject>()).vectorcall.map_or(0, |v| v as usize);
        if cur != original.addr() {
            return false;
        }
        super::site::set_vectorcall(p, kernel as usize);
    }
    spec.counters.dynamic.fetch_add(1, Ordering::Relaxed);
    true
}

/// Give `func` its original vectorcall (and specializer version) back, if it still holds
/// `kernel`: called by a dynamic kernel whose target is no longer live, before it calls the
/// original. Not during finalization (the call just falls back then).
///
/// # Safety
/// Attached thread; `func` is the live function object the kernel was called with.
pub unsafe fn retire(
    func: *mut ffi::PyObject,
    kernel: ffi::vectorcallfunc,
    original: &'static Original<ffi::vectorcallfunc>,
    counters: &Counters,
) {
    // SAFETY: per the caller.
    unsafe {
        if ffi::Py_IsFinalizing() != 0 {
            return;
        }
        let cur = (*func.cast::<ffi::PyFunctionObject>()).vectorcall.map_or(0, |v| v as usize);
        if cur != kernel as usize {
            return;
        }
        super::site::set_vectorcall(func, original.addr());
        super::site::restore_func_version(func.cast());
    }
    let _ = counters.dynamic.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
}

/// Capture `_PyFunction_Vectorcall` into `cell` (for targets that only have dynamic sites).
pub fn capture_function_vectorcall(cell: &'static Original<ffi::vectorcallfunc>) -> bool {
    cell.capture(super::prepare::function_vectorcall()).is_ok()
}
