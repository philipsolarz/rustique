//! Group `set`: target `set.__reduce__` (the `ml_meth` of `set.__reduce__` and
//! `frozenset.__reduce__`: two `PyMethodDef` entries, one C function). Free-threaded
//! builds only. Doc: `docs/targets/set.md`.
//!
//! The defect it removes (3.14t to 3.16a1t; `docs/atlas/w2-threads.md` §1.2):
//! `set.__reduce__` is an Argument Clinic `@critical_section` method, so it runs with the
//! set's per-object lock held, and its body calls `PySequence_List(so)` -> `_list_extend`
//! -> `Py_BEGIN_CRITICAL_SECTION2(list, so)`. The two-mutex fast path fails on the set's
//! mutex (held by this very thread) and `_PyCriticalSection2_BeginSlow` skips re-locking
//! only when the previous section was a two-mutex section on the same pair, so the thread
//! contends with itself: `PyMutex_Lock` spins 40 x `sched_yield`, then parks; detaching
//! suspends the outer section (unlocking the set) and the lock is taken. About 34.5 us per
//! call instead of 0.6 us. Every `copy.deepcopy` of a set/frozenset (copy group off),
//! `copy.copy` of a set subclass and pickling of a set at protocol < 4 (or of a subclass)
//! goes there (`object.__reduce_ex__` -> `set.__reduce__`).
//!
//! Kernel: CPython's own body (`set___reduce___impl`), call for call, without the clinic
//! wrapper's outer critical section:
//! `keys = PySequence_List(so)` (the same list, grown exactly as the original's, so the same
//! capacity and `sys.getsizeof`; it takes its own (list, set) critical section, now
//! uncontended), `args = (keys,)`, `state = _PyObject_GetState(so)` (the single
//! `__getstate__` lookup a user `__getattribute__` sees; default state from `__dict__` and
//! slots), result `(type(so), args, state)`. Errors are CPython's own, raised at the same
//! points (a subclass's `__iter__` or `__getstate__` may have run: the kernel never
//! restarts). Not observable: the set's lock is no longer held while the state is read; it
//! guards only the set's table, which the state does not read (the instance `__dict__` has
//! its own lock), and user `__iter__`/`__getstate__` code cannot tell whether the set's
//! lock is held (CPython already releases it whenever such code blocks).
//!
//! GIL builds: the clinic section is a no-op there, so the kernel would make the same calls
//! as the original (measured 0.993-1.008 per call, `docs/atlas/w2-threads.md` §4.1): no
//! gain, so the target is `not-needed` there rather than add install work and a site.

use pyo3::ffi;
use pyo3::prelude::*;

use crate::accel::{Counters, Original, Prepare, Refusal, TargetSpec};

pub static COUNTERS: Counters = Counters::new();
static ORIG_SET: Original<ffi::PyCFunction> = Original::new();
static ORIG_FROZENSET: Original<ffi::PyCFunction> = Original::new();
/// `_PyObject_GetState(obj)` (exported by CPython for `_datetime`/`_pickle`).
static GETSTATE: Original<ffi::reprfunc> = Original::new();

/// Fingerprints of the method entries (`ml_name`, `ml_flags`, `ml_doc`), recorded with
/// `python -m rustique._takeover._dev fingerprint set.__reduce__ frozenset.__reduce__`
/// (identical for both entries and on GIL and free-threaded builds).
const FP_REDUCE: crate::accel::Fingerprints = &[(3, 14, "9794b5c54ea800a6"), (3, 15, "9794b5c54ea800a6")];

/// `set___reduce___impl(so)` without the outer critical section.
///
/// # Safety
/// `so` is a set or frozenset instance (the receiver of a set method); attached thread.
#[inline(always)]
unsafe fn reduce(so: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: per the caller; every call is CPython's public or exported C API, with
    // CPython's error handling (NULL propagates, temporaries released).
    unsafe {
        COUNTERS.hit();
        let keys = ffi::PySequence_List(so);
        if keys.is_null() {
            return std::ptr::null_mut();
        }
        let args = ffi::PyTuple_Pack(1, keys);
        ffi::Py_DECREF(keys);
        if args.is_null() {
            return std::ptr::null_mut();
        }
        let state = GETSTATE.get()(so);
        if state.is_null() {
            ffi::Py_DECREF(args);
            return std::ptr::null_mut();
        }
        let r = ffi::PyTuple_Pack(3, ffi::Py_TYPE(so).cast::<ffi::PyObject>(), args, state);
        ffi::Py_DECREF(args);
        ffi::Py_DECREF(state);
        r
    }
}

unsafe extern "C" fn set_kernel(so: *mut ffi::PyObject, _arg: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: METH_NOARGS method of `set`: `so` is a set (or subclass) instance.
    unsafe { reduce(so) }
}

unsafe extern "C" fn frozenset_kernel(so: *mut ffi::PyObject, _arg: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: METH_NOARGS method of `frozenset`: `so` is a frozenset (or subclass) instance.
    unsafe { reduce(so) }
}

fn prepare(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if !cfg!(Py_GIL_DISABLED) {
        return Err(Refusal::not_needed("only free-threaded builds need it"));
    }
    cx.symbol(c"_PyObject_GetState", &GETSTATE)?;
    let set_t = py.get_type::<pyo3::types::PySet>();
    let fset_t = py.get_type::<pyo3::types::PyFrozenSet>();
    let m1 = cx.type_attr(&set_t, "__reduce__")?;
    let m2 = cx.type_attr(&fset_t, "__reduce__")?;
    cx.fingerprint_c(&m1, FP_REDUCE)?;
    cx.fingerprint_c(&m2, FP_REDUCE)?;
    cx.ml_meth(&m1, set_kernel as ffi::PyCFunction, &ORIG_SET)?;
    cx.ml_meth(&m2, frozenset_kernel as ffi::PyCFunction, &ORIG_FROZENSET)?;
    cx.self_test("set.__reduce__ probes", |py| {
        // Probes of subclasses whose `__slotnames__` is cached in the class: the default
        // state then never calls `copyreg._slotnames` (which would import `copyreg` at
        // `enable()`; exact sets always reach it, their static type cannot cache it).
        let ty = py.get_type::<pyo3::types::PyType>();
        let ns = |slots: bool| -> PyResult<Bound<'_, pyo3::types::PyDict>> {
            let d = pyo3::types::PyDict::new(py);
            d.set_item("__slotnames__", pyo3::types::PyList::empty(py))?;
            if slots {
                d.set_item("__slots__", pyo3::types::PyTuple::empty(py))?;
            }
            Ok(d)
        };
        let s_cls = ty.call1(("_RqSetProbe", (set_t.clone(),), ns(true)?))?;
        let f_cls = ty.call1(("_RqFrozenProbe", (fset_t.clone(),), ns(false)?))?;
        let s = s_cls.call1(([1, 2, 3],))?;
        let e = s_cls.call0()?;
        let f = f_cls.call1((["a", "b"],))?;
        f.setattr("x", 1)?;
        // SAFETY: the original METH_NOARGS functions and the kernels on the same receivers.
        unsafe {
            for (o, orig, k) in [
                (&s, ORIG_SET.get(), set_kernel as ffi::PyCFunction),
                (&e, ORIG_SET.get(), set_kernel as ffi::PyCFunction),
                (&f, ORIG_FROZENSET.get(), frozenset_kernel as ffi::PyCFunction),
            ] {
                let a = Bound::from_owned_ptr_or_err(py, orig(o.as_ptr(), std::ptr::null_mut()))?;
                let b = Bound::from_owned_ptr_or_err(py, k(o.as_ptr(), std::ptr::null_mut()))?;
                if !a.eq(&b)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    })
}

pub static SET_REDUCE: TargetSpec = TargetSpec {
    name: "set.__reduce__",
    group: "set",
    module: None,
    versions: &[(3, 14), (3, 15)],
    free_threaded: true,
    counters: &COUNTERS,
    prepare,
    ..TargetSpec::DEFAULT
};

/// The `set` group.
pub static TARGETS: &[&TargetSpec] = &[&SET_REDUCE];
