//! Check mode (`rustique._takeover.check("warn" | "raise")`): every fast-path result of a
//! `checkable` target is compared with the original's result for the same arguments.
//!
//! Only for targets whose exact subset runs no user code and has no side effects
//! (`TargetSpec::checkable`): running the original a second time must be unobservable.
//! On a mismatch the target's `fallback["check"]` counter grows and, in `"warn"` mode, the
//! original's result (or exception) is returned; in `"raise"` mode
//! `rustique._rustique._accel.RustiqueCheckError` is raised.
//!
//! Kernel pattern:
//!
//! ```ignore
//! let r = fast_path(args);
//! if COUNTERS.hit_check() {
//!     return unsafe { accel::check::verify(&TARGET, r, &[a, b], || ORIG.get()(m, a, b)) };
//! }
//! r
//! ```

use pyo3::ffi;
use pyo3::prelude::*;

use super::spec::TargetSpec;
use super::stats;

pyo3::create_exception!(
    rustique,
    RustiqueCheckError,
    pyo3::exceptions::PyAssertionError,
    "A Rustique kernel's result differed from CPython's original (check mode \"raise\")."
);

const MAX_DEPTH: u32 = 32;

/// Structural sameness of a kernel's result `a` and the original's `b`: exact types,
/// float bits (NaN-aware), `==` for scalars, element-wise for tuple/list/dict (dict order
/// included), identity where `b` is one of the arguments (CPython returned an argument
/// as is).
///
/// # Safety
/// Attached thread; live objects.
unsafe fn same(a: *mut ffi::PyObject, b: *mut ffi::PyObject, args: &[*mut ffi::PyObject], depth: u32) -> bool {
    if a == b {
        return true;
    }
    if depth == 0 && args.contains(&b) {
        return false;
    }
    if depth > MAX_DEPTH {
        return false;
    }
    // SAFETY: per the caller; type checks before layout reads.
    unsafe {
        let t = ffi::Py_TYPE(a);
        if t != ffi::Py_TYPE(b) {
            return false;
        }
        if t == &raw mut ffi::PyFloat_Type {
            let (x, y) = (ffi::PyFloat_AS_DOUBLE(a), ffi::PyFloat_AS_DOUBLE(b));
            return x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan());
        }
        if t == &raw mut ffi::PyComplex_Type {
            let (x, y) = (ffi::PyComplex_AsCComplex(a), ffi::PyComplex_AsCComplex(b));
            let eq = |p: f64, q: f64| p.to_bits() == q.to_bits() || (p.is_nan() && q.is_nan());
            return eq(x.real, y.real) && eq(x.imag, y.imag);
        }
        if t == &raw mut ffi::PyTuple_Type {
            let n = ffi::PyTuple_GET_SIZE(a);
            return n == ffi::PyTuple_GET_SIZE(b)
                && (0..n).all(|i| same(ffi::PyTuple_GET_ITEM(a, i), ffi::PyTuple_GET_ITEM(b, i), args, depth + 1));
        }
        if t == &raw mut ffi::PyList_Type {
            let n = ffi::PyList_GET_SIZE(a);
            return n == ffi::PyList_GET_SIZE(b)
                && (0..n).all(|i| same(ffi::PyList_GET_ITEM(a, i), ffi::PyList_GET_ITEM(b, i), args, depth + 1));
        }
        if t == &raw mut ffi::PyDict_Type {
            if ffi::PyDict_Size(a) != ffi::PyDict_Size(b) {
                return false;
            }
            let (mut pa, mut pb) = (0, 0);
            let (mut ka, mut va, mut kb, mut vb) =
                (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            while ffi::PyDict_Next(a, &mut pa, &mut ka, &mut va) != 0 {
                if ffi::PyDict_Next(b, &mut pb, &mut kb, &mut vb) == 0
                    || !same(ka, kb, args, depth + 1)
                    || !same(va, vb, args, depth + 1)
                {
                    return false;
                }
            }
            return true;
        }
        match ffi::PyObject_RichCompareBool(a, b, ffi::Py_EQ) {
            1 => true,
            0 => false,
            _ => {
                ffi::PyErr_Clear();
                false
            }
        }
    }
}

/// Exceptions are the same: same type and same `str()`.
///
/// # Safety
/// Attached thread; live exception objects.
unsafe fn same_exc(a: *mut ffi::PyObject, b: *mut ffi::PyObject) -> bool {
    // SAFETY: per the caller.
    unsafe {
        if ffi::Py_TYPE(a) != ffi::Py_TYPE(b) {
            return false;
        }
        let (sa, sb) = (ffi::PyObject_Str(a), ffi::PyObject_Str(b));
        let eq = !sa.is_null() && !sb.is_null() && ffi::PyUnicode_Compare(sa, sb) == 0;
        ffi::Py_XDECREF(sa);
        ffi::Py_XDECREF(sb);
        ffi::PyErr_Clear();
        eq
    }
}

/// Describe a result for the error message (`repr`, or the exception).
///
/// # Safety
/// Attached thread; `o` live or NULL with `exc` live.
unsafe fn describe(py: Python<'_>, o: *mut ffi::PyObject, exc: *mut ffi::PyObject) -> String {
    // SAFETY: per the caller.
    unsafe {
        let obj = if o.is_null() { exc } else { o };
        if obj.is_null() {
            return "<nothing>".into();
        }
        let r = Bound::from_borrowed_ptr(py, obj).repr().map(|r| r.to_string()).unwrap_or_else(|_| "?".into());
        if o.is_null() { format!("raised {r}") } else { r }
    }
}

/// Compare a fast path's outcome (`ours`: a new reference, or NULL with an exception set)
/// with the original's for the same arguments (`original()`: same convention). Returns the
/// outcome to hand back to CPython (see the module docs).
///
/// # Safety
/// Attached thread; `args` are the call's live arguments; `original` calls the target's
/// saved original with the unchanged arguments.
pub unsafe fn verify(
    target: &TargetSpec,
    ours: *mut ffi::PyObject,
    args: &[*mut ffi::PyObject],
    original: impl FnOnce() -> *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: per the caller; every reference below is owned and released once.
    unsafe {
        let py = Python::assume_attached();
        let our_exc = if ours.is_null() { ffi::PyErr_GetRaisedException() } else { std::ptr::null_mut() };
        let theirs = original();
        let their_exc = if theirs.is_null() { ffi::PyErr_GetRaisedException() } else { std::ptr::null_mut() };
        let ok = match (ours.is_null(), theirs.is_null()) {
            (false, false) => same(ours, theirs, args, 0),
            (true, true) => !our_exc.is_null() && !their_exc.is_null() && same_exc(our_exc, their_exc),
            _ => false,
        };
        if ok {
            ffi::Py_XDECREF(theirs);
            ffi::Py_XDECREF(their_exc);
            if ours.is_null() {
                ffi::PyErr_SetRaisedException(our_exc);
            }
            return ours;
        }
        target.counters.mismatch();
        if stats::check_mode() == Some(true) {
            let msg = format!(
                "rustique check: {}: kernel {} != original {}",
                target.name,
                describe(py, ours, our_exc),
                describe(py, theirs, their_exc)
            );
            ffi::Py_XDECREF(ours);
            ffi::Py_XDECREF(our_exc);
            ffi::Py_XDECREF(theirs);
            ffi::Py_XDECREF(their_exc);
            RustiqueCheckError::new_err(msg).restore(py);
            return std::ptr::null_mut();
        }
        ffi::Py_XDECREF(ours);
        ffi::Py_XDECREF(our_exc);
        if theirs.is_null() {
            ffi::PyErr_SetRaisedException(their_exc);
        }
        theirs
    }
}
