//! `copy.copy`, and the parts of `_reconstruct` shared with `deepcopy`.

use std::ptr::null_mut;

use pyo3::ffi;

use super::rt::*;

/// `cls in s` for one of copy's type sets: the fast answer for a pointer-hashed type when
/// the set is known intact (`fast`), the real set lookup otherwise.
#[inline(always)]
pub unsafe fn in_type_set(
    st: &St,
    set: O,
    fast: bool,
    known: &[*mut ffi::PyTypeObject],
    cls: *mut ffi::PyTypeObject,
) -> R<bool> {
    // SAFETY: live set and type.
    unsafe {
        if fast {
            if known.contains(&cls) {
                return Ok(true);
            }
            if meta_hash(cls.cast()) == st.default_type_hash {
                return Ok(false);
            }
        }
        match ffi::PySet_Contains(set, cls.cast()) {
            r if r < 0 => Err(()),
            r => Ok(r != 0),
        }
    }
}

/// The `state` branch of `_reconstruct` once `state` is final (deep-copied or not):
/// `__setstate__`, or `__dict__.update` plus slot `setattr`s.
pub unsafe fn set_state(st: &St, y: O, sc: O) -> R<()> {
    // SAFETY: live objects; each step is the operation copy.py performs.
    unsafe {
        let probe = getattr_opt(y, st.s_setstate)?;
        if !probe.is_null() {
            decref(probe);
            decref(call_method1(y, st.s_setstate, sc)?);
            return Ok(());
        }
        let mut state = Own(incref(sc));
        let mut slotstate = Own(null_mut());
        let is_tuple = ty(sc) == &raw mut ffi::PyTuple_Type
            || match ffi::PyObject_IsInstance(sc, (&raw mut ffi::PyTuple_Type).cast()) {
                r if r < 0 => return Err(()),
                r => r != 0,
            };
        if is_tuple {
            let n = ffi::PyObject_Size(sc);
            if n < 0 {
                return Err(());
            }
            if n == 2 {
                let (a, b) = unpack2(sc)?;
                state = a;
                slotstate = b;
            }
        }
        if state.get() != ffi::Py_None() {
            let dd = Own::new(ffi::PyObject_GetAttr(y, st.s_dict))?;
            if ty(dd.get()) == &raw mut ffi::PyDict_Type && ty(state.get()) == &raw mut ffi::PyDict_Type {
                if ffi::PyDict_Merge(dd.get(), state.get(), 1) < 0 {
                    return Err(());
                }
            } else {
                decref(call_method1(dd.get(), st.s_update, state.get())?);
            }
        }
        if !slotstate.get().is_null() && slotstate.get() != ffi::Py_None() {
            let items = Own::new(call_method0(slotstate.get(), st.s_items)?)?;
            let it = Own::new(ffi::PyObject_GetIter(items.get()))?;
            while let Some(kv) = iter_next(it.get())? {
                let kv = Own(kv);
                let (k, v) = unpack2(kv.get())?;
                if ffi::PyObject_SetAttr(y, k.get(), v.get()) < 0 {
                    return Err(());
                }
            }
        }
        Ok(())
    }
}

/// `copyreg.__newobj__(cls, *args)` when `func` is the original (its frame emulated), else
/// `func(*args)`. `args`: a tuple or list.
pub unsafe fn call_func(st: &St, depth: &mut Depth, func: O, args: O, newobj_ok: bool) -> R<O> {
    // SAFETY: live objects.
    unsafe {
        let n = if ty(args) == &raw mut ffi::PyTuple_Type {
            ffi::PyTuple_GET_SIZE(args)
        } else if ty(args) == &raw mut ffi::PyList_Type {
            ffi::PyList_GET_SIZE(args)
        } else {
            -1
        };
        if func == st.newobj && n >= 1 && newobj_ok {
            depth.push()?;
            let r = (|| {
                let t = if ty(args) == &raw mut ffi::PyTuple_Type {
                    Own(incref(args))
                } else {
                    Own::new(ffi::PyList_AsTuple(args))?
                };
                let cls = ffi::PyTuple_GET_ITEM(t.get(), 0);
                let new = Own::new(ffi::PyObject_GetAttr(cls, st.s_new))?;
                nn(ffi::PyObject_Call(new.get(), t.get(), null_mut()))
            })();
            depth.pop();
            return r;
        }
        call_star(func, args)
    }
}

/// `_reconstruct(x, None, *rv)`.
unsafe fn reconstruct_shallow(st: &St, depth: &mut Depth, x: O, rv: O) -> R<O> {
    // SAFETY: live objects.
    unsafe {
        let n = if ty(rv) == &raw mut ffi::PyTuple_Type { ffi::PyTuple_GET_SIZE(rv) } else { -1 };
        if !(2..=5).contains(&n) || !helper_intact(st, H_RECONSTRUCT) {
            let args = Own::new(ffi::PyList_New(0))?;
            if ffi::PyList_Append(args.get(), x) < 0 || ffi::PyList_Append(args.get(), ffi::Py_None()) < 0 {
                return Err(());
            }
            if ffi::PyList_Extend(args.get(), rv) < 0 {
                let t = ty(rv);
                if ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError) != 0
                    && (*t).tp_iter.is_none()
                    && ffi::PySequence_Check(rv) == 0
                {
                    ffi::PyErr_Clear();
                    ffi::PyErr_Format(
                        ffi::PyExc_TypeError,
                        c"Value after * must be an iterable, not %.200s".as_ptr(),
                        (*t).tp_name,
                    );
                }
                return Err(());
            }
            let t = Own::new(ffi::PyList_AsTuple(args.get()))?;
            return nn(ffi::PyObject_Call(st.reconstruct, t.get(), null_mut()));
        }
        let item = |i: ffi::Py_ssize_t| if i < n { ffi::PyTuple_GET_ITEM(rv, i) } else { ffi::Py_None() };
        let (func, args, state, listiter, dictiter) = (item(0), item(1), item(2), item(3), item(4));
        depth.push()?; // the `_reconstruct` frame
        let r = (|| {
            let y = Own::new(call_func(st, depth, func, args, helper_intact(st, H_NEWOBJ))?)?;
            if state != ffi::Py_None() {
                set_state(st, y.get(), state)?;
            }
            if listiter != ffi::Py_None() {
                let it = Own::new(ffi::PyObject_GetIter(listiter))?;
                while let Some(item) = iter_next(it.get())? {
                    let item = Own(item);
                    if ty(y.get()) == &raw mut ffi::PyList_Type {
                        if ffi::PyList_Append(y.get(), item.get()) < 0 {
                            return Err(());
                        }
                    } else {
                        decref(call_method1(y.get(), st.s_append, item.get())?);
                    }
                }
            }
            if dictiter != ffi::Py_None() {
                let it = Own::new(ffi::PyObject_GetIter(dictiter))?;
                while let Some(pair) = iter_next(it.get())? {
                    let pair = Own(pair);
                    let (k, v) = unpack2(pair.get())?;
                    if ffi::PyObject_SetItem(y.get(), k.get(), v.get()) < 0 {
                        return Err(());
                    }
                }
            }
            Ok(y.into_raw())
        })();
        depth.pop();
        r
    }
}

/// `copy.copy(x)` from its first line (the caller checked the guards).
pub unsafe fn copy(st: &St, x: O, fast_sets: bool) -> R<O> {
    // SAFETY: attached thread, live x.
    unsafe {
        let mut depth = Depth::new(st);
        depth.push()?; // copy's own frame
        let r = copy_body(st, &mut depth, x, fast_sets);
        depth.unwind();
        r
    }
}

unsafe fn copy_body(st: &St, depth: &mut Depth, x: O, fast_sets: bool) -> R<O> {
    // SAFETY: as copy.
    unsafe {
        let cls = ty(x);
        if in_type_set(st, st.copy_atomic, fast_sets, &st.copy_atomic_types, cls)? {
            return Ok(incref(x));
        }
        if in_type_set(st, st.copy_containers, fast_sets, &st.copy_container_types, cls)? {
            // cls.copy(x)
            if cls == &raw mut ffi::PyList_Type {
                return nn(ffi::PyList_GetSlice(x, 0, ffi::PY_SSIZE_T_MAX));
            }
            if cls == &raw mut ffi::PyDict_Type {
                return nn(ffi::PyDict_Copy(x));
            }
            let m = Own::new(ffi::PyObject_GetAttr(cls.cast(), st.s_copy_method))?;
            return call1(m.get(), x);
        }
        if ffi::PyType_IsSubtype(cls, &raw mut ffi::PyType_Type) != 0 {
            return Ok(incref(x));
        }
        let copier = Own(getattr_opt(cls.cast(), st.s_copy)?);
        if !copier.get().is_null() && copier.get() != ffi::Py_None() {
            if is_return_self(st, copier.get(), 1) {
                depth.touch()?;
                return Ok(incref(x));
            }
            return call1(copier.get(), x);
        }
        let rv = {
            let reductor = Own(dict_get(st.dispatch_table, cls.cast())?);
            if !reductor.get().is_null() && truth(reductor.get())? {
                call1(reductor.get(), x)?
            } else {
                drop(reductor);
                let r = Own(getattr_opt(x, st.s_reduce_ex)?);
                if !r.get().is_null() && r.get() != ffi::Py_None() {
                    call1(r.get(), st.int4)?
                } else {
                    let r = Own(getattr_opt(x, st.s_reduce)?);
                    if !r.get().is_null() && truth(r.get())? {
                        call0(r.get())?
                    } else {
                        return raise_copy_error(st, c"un(shallow)copyable object of type %S".as_ptr(), cls);
                    }
                }
            }
        };
        let rv = Own(rv);
        if is_str_instance(rv.get())? {
            return Ok(incref(x));
        }
        reconstruct_shallow(st, depth, x, rv.get())
    }
}
