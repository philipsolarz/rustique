//! `copy.copy` for the calls in which no user code runs: `None` from [`copy`] means
//! "restart in the original" (nothing observable happened).
//!
//! Native: `copy`'s atomic types, `list`/`dict`/`set`/`bytearray` (`cls.copy(x)`),
//! `issubclass(cls, type)`, `__copy__` that is exactly `return self` (Enum), and plain
//! objects (`object.__reduce_ex__(4)` emulated, then `y.__dict__.update(x.__dict__)` and
//! the slots). Anything else — a user `__copy__`, a registered reductor, a custom reduce
//! protocol — restarts before anything was done.

use pyo3::ffi;

use super::reduce::{object_new_alloc, plain_reduce};
use super::rt::*;

/// `cls in s` for one of copy's type sets, decided purely: `known` is the set's validated
/// content, valid while `fast` (3.15's frozensets; 3.14's mutable sets as snapshotted).
/// Err: the answer would need a lookup that could run user code.
#[inline(always)]
unsafe fn in_type_set(fast: bool, known: &[*mut ffi::PyTypeObject], cls: *mut ffi::PyTypeObject) -> R<bool> {
    if !fast {
        return Err(());
    }
    if known.contains(&cls) {
        return Ok(true);
    }
    // SAFETY: live type.
    if unsafe { meta_hash(cls.cast()) } != default_type_hash() {
        return Err(());
    }
    Ok(false)
}

/// `getattr(cls, "__copy__", None)` decided purely: Ok(true) when it is `return self`,
/// Ok(false) when absent or None, Err otherwise.
#[inline(always)]
unsafe fn copy_hook(st: &St, cls: *mut ffi::PyTypeObject) -> R<bool> {
    // SAFETY: live type objects.
    unsafe {
        let meta = ty(cls.cast());
        // `type.__getattribute__`, and no data descriptor `__copy__` on the metatype.
        let type_getattro = ffi::PyType_Type.tp_getattro.map(|f| f as usize);
        if (*meta).tp_getattro.map(|f| f as usize) != type_getattro || !_PyType_Lookup(meta, st.s_copy).is_null() {
            return Err(());
        }
        let d = _PyType_Lookup(cls, st.s_copy);
        if d.is_null() || d == ffi::Py_None() {
            return Ok(false);
        }
        if is_return_self(d, 1) {
            return Ok(true);
        }
        Err(())
    }
}

/// `copy.copy(x)`; `None`: restart in the original.
pub unsafe fn copy(st: &St, x: O, fast_sets: bool, budget: i32) -> Option<O> {
    // SAFETY: attached thread, live x.
    let r = unsafe { copy_body(st, x, fast_sets, budget) };
    match r {
        Ok(y) => Some(y),
        Err(()) => {
            // SAFETY: attached thread.
            unsafe { ffi::PyErr_Clear() };
            None
        }
    }
}

unsafe fn copy_body(st: &St, x: O, fast_sets: bool, budget: i32) -> R<O> {
    // SAFETY: as copy.
    unsafe {
        // copy's own frame.
        if budget <= 0 {
            return Err(());
        }
        let cls = ty(x);
        if in_type_set(fast_sets, &st.copy_atomic_types, cls)? {
            return Ok(incref(x));
        }
        if in_type_set(fast_sets, &st.copy_container_types, cls)? {
            // `cls.copy(x)`: the C methods of the four exact types.
            if cls == &raw mut ffi::PyList_Type {
                return nn(ffi::PyList_GetSlice(x, 0, ffi::PY_SSIZE_T_MAX));
            }
            if cls == &raw mut ffi::PyDict_Type {
                return nn(ffi::PyDict_Copy(x));
            }
            if cls == &raw mut ffi::PySet_Type {
                return nn(ffi::PySet_New(x));
            }
            if cls == &raw mut ffi::PyByteArray_Type {
                let m = Own::new(ffi::PyObject_GetAttr(cls.cast(), st.s_copy_method))?;
                return call1(m.get(), x);
            }
            return Err(());
        }
        if ffi::PyType_IsSubtype(cls, &raw mut ffi::PyType_Type) != 0 {
            return Ok(incref(x));
        }
        if copy_hook(st, cls)? {
            // `def __copy__(self): return self` (Enum): its frame only.
            if budget - 1 <= 0 {
                return Err(());
            }
            return Ok(incref(x));
        }
        let r = dict_get(st.dispatch_table, cls.cast())?;
        if !r.is_null() {
            decref(r);
            return Err(());
        }
        let Some(rv) = plain_reduce(st, x, cls, false)? else { return Err(()) };
        let rv = Own(rv);
        if !helper_intact(st, H_RECONSTRUCT) || !helper_intact(st, H_NEWOBJ) {
            return Err(());
        }
        // `_reconstruct(x, None, copyreg.__newobj__, (cls,), state)`: its frame and
        // `__newobj__`'s.
        if budget - 2 <= 0 {
            return Err(());
        }
        let state = ffi::PyTuple_GET_ITEM(rv.get(), 2);
        // `y.__dict__.update(state)` with `state` = `x.__dict__` itself: inserting its keys
        // into a fresh dict must not compare them through user `__eq__`.
        let dict_part = if ty(state) == &raw mut ffi::PyTuple_Type { ffi::PyTuple_GET_ITEM(state, 0) } else { state };
        if dict_part != ffi::Py_None() {
            let mut pos = 0;
            let mut k: O = std::ptr::null_mut();
            let mut v: O = std::ptr::null_mut();
            while ffi::PyDict_Next(dict_part, &mut pos, &mut k, &mut v) != 0 {
                if ty(k) != &raw mut ffi::PyUnicode_Type {
                    return Err(());
                }
            }
        }
        let y = Own::new(object_new_alloc(st, cls.cast()).unwrap_or(Err(()))?)?;
        if state != ffi::Py_None() {
            super::deep::apply_state(st, y.get(), state)?;
        }
        Ok(y.into_raw())
    }
}
