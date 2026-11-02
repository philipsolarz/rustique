//! `object.__reduce_ex__(x, 4)` computed natively for plain classes, when the result is
//! provably the one CPython's `reduce_newobj` (Objects/typeobject.c) would build.
//!
//! A class qualifies when every lookup `object.__reduce_ex__` performs resolves to
//! `object`'s own entry and is free of user code: `__reduce_ex__`, `__reduce__` and
//! `__getstate__` are `object`'s method descriptors (and the instance dict does not shadow
//! the first and the last), there is no `__getnewargs_ex__`/`__getnewargs__`, the class's
//! `__slotnames__` is cached or computable by [`emulate_slotnames`], attribute access is
//! the generic one, and the metatype is `type`. Then the value is `(copyreg.__newobj__, (cls,), state, None, None)` with
//! `state` = the instance `__dict__` (materialised exactly when CPython materialises it) or
//! `None`, paired with a dict of the slot values when there are any — the same objects
//! `object_getstate_default` returns. Every check runs before anything observable, so a
//! class that does not qualify takes CPython's own path with nothing done twice.

use std::ffi::c_int;
use std::ptr::null_mut;

use pyo3::ffi;

use super::rt::*;

python_extern! {
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

/// Lookups of `cls` that decide eligibility (all through the type cache, no user code):
/// `object.__reduce_ex__(4)` resolves to `object`'s own machinery, and `_reconstruct` then
/// runs only C code: `object.__new__`, the standard `__dict__`, member-descriptor slots, no
/// `__setstate__`; with `deep`, no finalizer either (a copy may be discarded on restart).
#[inline(always)]
unsafe fn class_ok(st: &St, cls: *mut ffi::PyTypeObject, deep: bool) -> bool {
    // SAFETY: a live, ready type.
    unsafe {
        if ty(cls.cast()) != &raw mut ffi::PyType_Type {
            return false;
        }
        let obj = &raw mut ffi::PyBaseObject_Type;
        let flags = ffi::PyType_GetFlags(cls);
        if !generic_getattr(cls)
            || (*cls).tp_itemsize != 0
            || addr((*cls).tp_new) != addr((*obj).tp_new)
            || flags & (ffi::Py_TPFLAGS_LIST_SUBCLASS | ffi::Py_TPFLAGS_DICT_SUBCLASS | ffi::Py_TPFLAGS_IS_ABSTRACT)
                != 0
        {
            return false;
        }
        if (*cls).tp_dictoffset != 0 && flags & ffi::Py_TPFLAGS_MANAGED_DICT == 0 {
            return false;
        }
        if deep && ((*cls).tp_finalize.is_some() || (*cls).tp_del.is_some() || !_PyType_Lookup(cls, st.s_del).is_null())
        {
            return false;
        }
        if flags & ffi::Py_TPFLAGS_MANAGED_DICT != 0 {
            let d = _PyType_Lookup(cls, st.s_dict);
            if d.is_null() || ty(d) != &raw mut ffi::PyGetSetDescr_Type {
                return false;
            }
        }
        _PyType_Lookup(cls, st.s_reduce_ex) == st.obj_reduce_ex
            && _PyType_Lookup(cls, st.s_reduce) == st.obj_reduce
            && _PyType_Lookup(cls, st.s_getstate) == st.obj_getstate
            && _PyType_Lookup(cls, st.s_new) == st.object_new
            && _PyType_Lookup(cls, st.s_getnewargs_ex).is_null()
            && _PyType_Lookup(cls, st.s_getnewargs).is_null()
            && _PyType_Lookup(cls, st.s_setstate).is_null()
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
///
/// `slot_frame`: the `copyreg._slotnames` frame CPython enters for a class whose
/// `__slotnames__` is not cached yet would not hit the recursion limit.
pub unsafe fn plain_reduce(st: &St, x: O, cls: *mut ffi::PyTypeObject, deep: bool, slot_frame: bool) -> R<Option<O>> {
    // SAFETY: attached thread; live objects.
    unsafe {
        if st.get_method_addr == 0 || !class_ok(st, cls, deep) {
            return Ok(None);
        }
        if !unshadowed(st, x, st.s_reduce_ex, st.obj_reduce_ex)? || !unshadowed(st, x, st.s_getstate, st.obj_getstate)?
        {
            return Ok(None);
        }
        // `_PyType_GetSlotNames`: the cached `cls.__dict__['__slotnames__']` (a list or None),
        // or `copyreg._slotnames(cls)`, which computes and caches it.
        let tdict = Own::new(ffi::PyType_GetDict(cls))?;
        let mut slotnames = Own(dict_get(tdict.get(), st.s_slotnames)?);
        if slotnames.get().is_null() {
            if !slot_frame || !helper_intact(st, H_SLOTNAMES) || !emulate_slotnames(st, cls)? {
                return Ok(None);
            }
            slotnames = Own(dict_get(tdict.get(), st.s_slotnames)?);
        }
        let sn = slotnames.get();
        if sn.is_null() || (sn != ffi::Py_None() && ty(sn) != &raw mut ffi::PyList_Type) {
            return Ok(None);
        }
        let nslots = if sn == ffi::Py_None() { 0 } else { ffi::PyList_GET_SIZE(sn) };
        if nslots > 0 {
            // `getattr(x, name)` / `setattr(y, name, v)` must be the slots' member descriptors.
            let obj = &raw mut ffi::PyBaseObject_Type;
            if addr((*cls).tp_setattro) != addr((*obj).tp_setattro) {
                return Ok(None);
            }
            for i in 0..nslots {
                let n = ffi::PyList_GET_ITEM(sn, i);
                if ty(n) != &raw mut ffi::PyUnicode_Type {
                    return Ok(None);
                }
                let d = _PyType_Lookup(cls, n);
                if d.is_null() || ty(d) != &raw mut ffi::PyMemberDescr_Type {
                    return Ok(None);
                }
            }
        }
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

/// `copyreg._slotnames(cls)` for a class (metatype `type`) whose `__slotnames__` is not
/// cached: the names of the `__slots__` along the MRO (mangled, without `__dict__` and
/// `__weakref__`), then `cls.__slotnames__ = names` — the write CPython makes, with no user
/// code involved. `Ok(false)`: a `__slots__` value that is not an exact str/tuple/list of
/// exact strs, a non-heap class defining `__slots__`, or a class that refuses the write
/// (nothing observable happened; CPython runs the function itself).
pub unsafe fn emulate_slotnames(st: &St, cls: *mut ffi::PyTypeObject) -> R<bool> {
    // SAFETY: a live, ready type with metatype `type` (class_ok).
    unsafe {
        let names = Own::new(ffi::PyList_New(0))?;
        // `hasattr(cls, "__slots__")`: `type` defines no `__slots__`, so the MRO entry decides.
        let a = _PyType_Lookup(cls, st.s_slots);
        if !a.is_null() {
            if (*ty(a)).tp_descr_get.is_some() {
                return Ok(false);
            }
            let mro = (*cls).tp_mro;
            if mro.is_null() || ty(mro) != &raw mut ffi::PyTuple_Type {
                return Ok(false);
            }
            for i in 0..ffi::PyTuple_GET_SIZE(mro) {
                let c = ffi::PyTuple_GET_ITEM(mro, i).cast::<ffi::PyTypeObject>();
                let cd = Own::new(ffi::PyType_GetDict(c))?;
                let slots = Own(dict_get(cd.get(), st.s_slots)?);
                let sv = slots.get();
                if sv.is_null() {
                    continue;
                }
                if ffi::PyType_GetFlags(c) & ffi::Py_TPFLAGS_HEAPTYPE == 0 {
                    return Ok(false);
                }
                let items: Vec<O> = if ty(sv) == &raw mut ffi::PyUnicode_Type {
                    vec![sv]
                } else if ty(sv) == &raw mut ffi::PyTuple_Type {
                    (0..ffi::PyTuple_GET_SIZE(sv)).map(|j| ffi::PyTuple_GET_ITEM(sv, j)).collect()
                } else if ty(sv) == &raw mut ffi::PyList_Type {
                    (0..ffi::PyList_GET_SIZE(sv)).map(|j| ffi::PyList_GET_ITEM(sv, j)).collect()
                } else {
                    return Ok(false);
                };
                // Strong references: the list cannot change meanwhile (no code runs), but
                // keep the borrowed items alive regardless.
                let items: Vec<Own> = items.into_iter().map(|o| Own(incref(o))).collect();
                for n in &items {
                    let n = n.get();
                    if ty(n) != &raw mut ffi::PyUnicode_Type {
                        return Ok(false);
                    }
                    let mut len: ffi::Py_ssize_t = 0;
                    let p = ffi::PyUnicode_AsUTF8AndSize(n, &mut len);
                    if p.is_null() {
                        ffi::PyErr_Clear();
                        return Ok(false);
                    }
                    let b = std::slice::from_raw_parts(p.cast::<u8>(), len as usize);
                    if b == b"__dict__" || b == b"__weakref__" {
                        continue;
                    }
                    let item = if b.starts_with(b"__") && !b.ends_with(b"__") {
                        // `'_%s%s' % (c.__name__.lstrip('_'), name)`, unless that is empty.
                        let cn = Own::new(ffi::PyType_GetName(c))?;
                        let mut clen: ffi::Py_ssize_t = 0;
                        let cp = ffi::PyUnicode_AsUTF8AndSize(cn.get(), &mut clen);
                        if cp.is_null() {
                            ffi::PyErr_Clear();
                            return Ok(false);
                        }
                        let cb = std::slice::from_raw_parts(cp.cast::<u8>(), clen as usize);
                        let k = cb.iter().take_while(|&&ch| ch == b'_').count();
                        if k == cb.len() {
                            Own(incref(n))
                        } else {
                            let mut m = Vec::with_capacity(1 + cb.len() - k + b.len());
                            m.push(b'_');
                            m.extend_from_slice(&cb[k..]);
                            m.extend_from_slice(b);
                            Own::new(ffi::PyUnicode_FromStringAndSize(m.as_ptr().cast(), m.len() as ffi::Py_ssize_t))?
                        }
                    } else {
                        Own(incref(n))
                    };
                    if ffi::PyList_Append(names.get(), item.get()) < 0 {
                        return Err(());
                    }
                }
            }
        }
        // `try: cls.__slotnames__ = names / except: pass`; a class refusing it (immutable)
        // is left to CPython.
        if ffi::PyObject_SetAttr(cls.cast(), st.s_slotnames, names.get()) < 0 {
            ffi::PyErr_Clear();
            return Ok(false);
        }
        Ok(true)
    }
}
