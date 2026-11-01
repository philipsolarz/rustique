//! `object.__reduce_ex__(x, 4)` computed natively for plain classes, when the result is
//! provably the one CPython's `reduce_newobj` (Objects/typeobject.c) would build.
//!
//! A class qualifies when every lookup `object.__reduce_ex__` performs resolves to
//! `object`'s own entry and is free of user code: `__reduce_ex__`, `__reduce__` and
//! `__getstate__` are `object`'s method descriptors (and the instance dict does not shadow
//! the first and the last), there is no `__getnewargs_ex__`/`__getnewargs__`, the class's
//! `__slotnames__` is already cached, attribute access is the generic one, and the metatype
//! is `type`. Then the value is `(copyreg.__newobj__, (cls,), state, None, None)` with
//! `state` = the instance `__dict__` (materialised exactly when CPython materialises it) or
//! `None`, paired with a dict of the slot values when there are any — the same objects
//! `object_getstate_default` returns. Every check runs before anything observable, so a
//! class that does not qualify takes CPython's own path with nothing done twice.

use std::ffi::c_int;
use std::ptr::null_mut;

use pyo3::ffi;

use super::rt::*;

unsafe extern "C" {
    /// Public since 3.13 (cpython/object.h).
    fn PyObject_VisitManagedDict(obj: O, visit: ffi::visitproc, arg: *mut std::ffi::c_void) -> c_int;
}

/// `_PyObject_GetMethod` (PyAPI_FUNC, internal header; same signature in 3.14 and 3.15).
pub type GetMethodFn = unsafe extern "C" fn(O, O, *mut O) -> c_int;

#[inline(always)]
fn addr<T>(f: Option<T>) -> usize
where
    T: Copy,
{
    match f {
        // SAFETY: function pointers are pointer-sized.
        Some(f) => unsafe { std::mem::transmute_copy::<T, usize>(&f) },
        None => 0,
    }
}

/// Lookups of `cls` that decide eligibility (all through the type cache, no user code).
#[inline(always)]
unsafe fn class_ok(st: &St, cls: *mut ffi::PyTypeObject) -> bool {
    // SAFETY: a live, ready type.
    unsafe {
        if ty(cls.cast()) != &raw mut ffi::PyType_Type {
            return false;
        }
        let obj = &raw mut ffi::PyBaseObject_Type;
        let flags = ffi::PyType_GetFlags(cls);
        if addr((*cls).tp_getattro) != addr((*obj).tp_getattro)
            || (*cls).tp_itemsize != 0
            || (*cls).tp_new.is_none()
            || flags & (ffi::Py_TPFLAGS_LIST_SUBCLASS | ffi::Py_TPFLAGS_DICT_SUBCLASS) != 0
        {
            return false;
        }
        // The instance dict, if any, must be the managed one (`_PyObject_IsInstanceDictEmpty`
        // is emulated for it only).
        if (*cls).tp_dictoffset != 0 && flags & ffi::Py_TPFLAGS_MANAGED_DICT == 0 {
            return false;
        }
        _PyType_Lookup(cls, st.s_reduce_ex) == st.obj_reduce_ex
            && _PyType_Lookup(cls, st.s_reduce) == st.obj_reduce
            && _PyType_Lookup(cls, st.s_getstate) == st.obj_getstate
            && _PyType_Lookup(cls, st.s_getnewargs_ex).is_null()
            && _PyType_Lookup(cls, st.s_getnewargs).is_null()
    }
}

/// `_PyObject_GetMethod(x, name)` is exactly `object`'s descriptor `want` (not shadowed by
/// the instance dict). Pure: the class entry is a method descriptor.
#[inline(always)]
unsafe fn unshadowed(st: &St, x: O, name: O, want: O) -> R<bool> {
    let mut m: O = null_mut();
    // SAFETY: live object; the class-level entry was checked to be a method descriptor.
    unsafe {
        let r = (st.get_method)(x, name, &mut m);
        if m.is_null() {
            return if ffi::PyErr_Occurred().is_null() { Ok(false) } else { Err(()) };
        }
        decref(m);
        Ok(r == 1 && m == want)
    }
}

unsafe extern "C" fn count_visit(_o: O, arg: *mut std::ffi::c_void) -> c_int {
    // SAFETY: `arg` is the counter below.
    unsafe { *arg.cast::<usize>() += 1 };
    0
}

/// `state` of `object_getstate_default(x, required=1)`: `None` or the instance dict
/// (materialised exactly when CPython materialises it). New reference.
#[inline(always)]
unsafe fn instance_state(x: O, cls: *mut ffi::PyTypeObject) -> R<O> {
    // SAFETY: live object of a qualifying class.
    unsafe {
        if ffi::PyType_GetFlags(cls) & ffi::Py_TPFLAGS_MANAGED_DICT == 0 {
            return Ok(incref(ffi::Py_None()));
        }
        // No managed dict and no inline value: empty without materialising anything.
        let mut n: usize = 0;
        PyObject_VisitManagedDict(x, count_visit, (&raw mut n).cast());
        if n == 0 {
            return Ok(incref(ffi::Py_None()));
        }
        // Non-empty inline values (CPython materialises them), or an existing dict (or a
        // single attribute holding one): `PyObject_GenericGetDict` is then what CPython calls,
        // or returns the dict that already exists.
        let d = nn(ffi::PyObject_GenericGetDict(x, null_mut()))?;
        if ffi::PyDict_Size(d) == 0 {
            decref(d);
            return Ok(incref(ffi::Py_None()));
        }
        Ok(d)
    }
}

/// The reduce value of plain `x`, or `None` when `x` does not qualify (nothing observable
/// happened then).
pub unsafe fn plain_reduce(st: &St, x: O, cls: *mut ffi::PyTypeObject) -> R<Option<O>> {
    // SAFETY: attached thread; live objects.
    unsafe {
        if st.get_method_addr == 0 || !class_ok(st, cls) {
            return Ok(None);
        }
        if !unshadowed(st, x, st.s_reduce_ex, st.obj_reduce_ex)? || !unshadowed(st, x, st.s_getstate, st.obj_getstate)?
        {
            return Ok(None);
        }
        // `_PyType_GetSlotNames`: the cached `cls.__dict__['__slotnames__']` (a list or None).
        let tdict = Own::new(ffi::PyType_GetDict(cls))?;
        let slotnames = Own(dict_get(tdict.get(), st.s_slotnames)?);
        let sn = slotnames.get();
        if sn.is_null() || (sn != ffi::Py_None() && ty(sn) != &raw mut ffi::PyList_Type) {
            return Ok(None);
        }
        let nslots = if sn == ffi::Py_None() { 0 } else { ffi::PyList_GET_SIZE(sn) };
        // `required`: the basicsize check ("cannot pickle 'X' object" otherwise).
        let obj = &raw mut ffi::PyBaseObject_Type;
        let mut basicsize = (*obj).tp_basicsize;
        if (*cls).tp_dictoffset != 0 && ffi::PyType_GetFlags(cls) & ffi::Py_TPFLAGS_MANAGED_DICT == 0 {
            basicsize += size_of::<O>() as isize;
        }
        if (*cls).tp_weaklistoffset > 0 {
            basicsize += size_of::<O>() as isize;
        }
        basicsize += size_of::<O>() as isize * nslots;
        if (*cls).tp_basicsize > basicsize {
            return Ok(None);
        }
        // From here on the steps are object_getstate_default's, in its order.
        let mut state = Own::new(instance_state(x, cls)?)?;
        if nslots > 0 {
            let slots = Own::new(ffi::PyDict_New())?;
            let mut i = 0;
            while i < nslots {
                let name = Own(incref(ffi::PyList_GET_ITEM(sn, i)));
                let v = getattr_opt(x, name.get())?;
                if !v.is_null() {
                    let v = Own(v);
                    if ffi::PyDict_SetItem(slots.get(), name.get(), v.get()) < 0 {
                        return Err(());
                    }
                }
                if nslots != ffi::PyList_GET_SIZE(sn) {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        c"__slotnames__ changed size during iteration".as_ptr(),
                    );
                    return Err(());
                }
                i += 1;
            }
            if ffi::PyDict_Size(slots.get()) > 0 {
                state = Own::new(ffi::PyTuple_Pack(2, state.get(), slots.get()))?;
            }
        }
        let args = Own::new(ffi::PyTuple_Pack(1, cls.cast::<ffi::PyObject>()))?;
        let none = ffi::Py_None();
        Ok(Some(nn(ffi::PyTuple_Pack(5, st.newobj, args.get(), state.get(), none, none))?))
    }
}

/// `object.__new__(cls)` as `copyreg.__newobj__(cls)` reaches it (`cls.__new__(cls)`),
/// when that is exactly `cls->tp_alloc(cls, 0)`: `cls.__new__` is `object.__new__`, the
/// metatype is `type`, and the class is not abstract (whose error CPython raises).
#[inline(always)]
pub unsafe fn object_new_alloc(st: &St, cls: O) -> Option<R<O>> {
    // SAFETY: live object.
    unsafe {
        if ffi::PyType_Check(cls) == 0 || ty(cls) != &raw mut ffi::PyType_Type {
            return None;
        }
        let t = cls.cast::<ffi::PyTypeObject>();
        let obj = &raw mut ffi::PyBaseObject_Type;
        if addr((*t).tp_new) != addr((*obj).tp_new)
            || ffi::PyType_GetFlags(t) & ffi::Py_TPFLAGS_IS_ABSTRACT != 0
            || _PyType_Lookup(t, st.s_new) != st.object_new
        {
            return None;
        }
        let alloc = (*t).tp_alloc?;
        Some(nn(alloc(t, 0)))
    }
}
