//! `ListIterator` / `ListReverseIterator`: hand-written iterator types.
//!
//! Iterators are created and destroyed once per `iter(x)`, `list(x)`, `a, b = x`, `g(*x)`,
//! `zip`/`enumerate` over a list, so their life cycle is a hot path. PyO3's class machinery
//! costs about three times CPython's `list_iterator` there (generic allocation that zeroes
//! the object and goes through `object.__new__`, a trampolined `tp_dealloc`), so these two
//! types are defined directly with the C API: `PyObject_GC_New`, a plain struct, and raw
//! slots (`tp_iternext`, `tp_dealloc`, `tp_traverse`, `tp_clear`) that follow the rules
//! of `slots` (no unwinding, no `Py<T>` drops outside `Python::attach`).

use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;

use super::slots::{fallback, py, shield};
use super::{IterState, List, iter_next_ptr, iter_reduce, iter_setstate};
use crate::sync::PyMutex;

type Obj = *mut ffi::PyObject;

/// The object layout of both iterator types.
#[repr(C)]
struct ListIterObject {
    ob_base: ffi::PyObject,
    state: PyMutex<IterState>,
}

/// The two type objects (forward, reverse), created at module initialisation.
static TYPES: [AtomicPtr<ffi::PyTypeObject>; 2] = [AtomicPtr::new(ptr::null_mut()), AtomicPtr::new(ptr::null_mut())];

/// Free lists of dead iterator objects (GIL builds; CPython keeps one for its list
/// iterators too). Accessed only while attached, i.e. under the GIL. A cached object is
/// untracked, keeps its type's reference, and holds no state.
#[cfg(not(Py_GIL_DISABLED))]
mod freelist {
    use std::cell::UnsafeCell;

    use super::Obj;

    const MAX: usize = 16;

    pub(super) struct FreeList(UnsafeCell<(usize, [Obj; MAX])>);

    // SAFETY: only used under the GIL (see the module docs).
    unsafe impl Sync for FreeList {}

    pub(super) static LISTS: [FreeList; 2] = [
        FreeList(UnsafeCell::new((0, [std::ptr::null_mut(); MAX]))),
        FreeList(UnsafeCell::new((0, [std::ptr::null_mut(); MAX]))),
    ];

    impl FreeList {
        /// A cached object with a reference count of one, if any.
        ///
        /// # Safety
        /// The caller must hold the GIL.
        #[inline(always)]
        pub(super) unsafe fn pop(&self) -> Option<Obj> {
            // SAFETY: GIL-protected (see above); a cached object is a dead object of the
            // right type whose memory is still allocated.
            unsafe {
                let (len, items) = &mut *self.0.get();
                if *len == 0 {
                    return None;
                }
                *len -= 1;
                let obj = items[*len];
                // `_Py_NewReference` on GIL builds: the full refcount word becomes 1.
                obj.cast::<u64>().write(1);
                Some(obj)
            }
        }

        /// Cache a dead, untracked object; `false` when full.
        ///
        /// # Safety
        /// The caller must hold the GIL; `obj` must be dead, untracked and stateless.
        #[inline(always)]
        pub(super) unsafe fn push(&self, obj: Obj) -> bool {
            // SAFETY: GIL-protected (see above).
            unsafe {
                let (len, items) = &mut *self.0.get();
                if *len == MAX {
                    return false;
                }
                items[*len] = obj;
                *len += 1;
                true
            }
        }
    }
}

/// # Safety
/// `slf` must be a live iterator of one of the two types.
#[inline(always)]
unsafe fn state<'a>(slf: Obj) -> &'a PyMutex<IterState> {
    // SAFETY: see above.
    unsafe { &(*slf.cast::<ListIterObject>()).state }
}

/// A new iterator over `list` (`forward`: from index 0; otherwise from the end).
pub(super) fn new_iterator<'py>(
    py: Python<'py>,
    list: &Bound<'py, List>,
    forward: bool,
) -> PyResult<Bound<'py, PyAny>> {
    // SAFETY: a live list; a new reference or null with an exception set.
    unsafe { Bound::from_owned_ptr_or_err(py, new_iterator_ptr(list.as_ptr(), forward)) }
}

/// A new iterator over the `List` `list` as a new reference, or null with an exception set.
#[inline]
pub(super) fn new_iterator_ptr(list: Obj, forward: bool) -> Obj {
    let which = usize::from(!forward);
    // SAFETY: `list` is a live `List`; `ty` is one of the iterator types (created before
    // any list exists); the object is fully initialised before it is tracked or returned.
    unsafe {
        let len = super::slots::this(list).len() as isize;
        #[cfg(not(Py_GIL_DISABLED))]
        let cached = freelist::LISTS[which].pop();
        #[cfg(Py_GIL_DISABLED)]
        let cached: Option<Obj> = None;
        let obj = match cached {
            Some(obj) => obj.cast::<ListIterObject>(),
            None => {
                let obj = ffi::PyObject_GC_New::<ListIterObject>(TYPES[which].load(Ordering::Acquire));
                if obj.is_null() {
                    return ptr::null_mut();
                }
                obj
            }
        };
        ffi::Py_INCREF(list);
        let list = Bound::from_owned_ptr(py(), list).cast_into_unchecked::<List>().unbind();
        let index = if forward { 0 } else { len - 1 };
        ptr::write(&raw mut (*obj).state, PyMutex::new(IterState { list: Some(list), index }));
        ffi::PyObject_GC_Track(obj.cast());
        obj.cast()
    }
}

// SAFETY (for the slots below): CPython calls them attached with `slf` a live instance of
// the type they are installed on.

unsafe extern "C" fn next_forward(slf: Obj) -> Obj {
    // SAFETY: see above.
    shield(ptr::null_mut(), || iter_next_ptr(py(), unsafe { state(slf) }, true))
}

unsafe extern "C" fn next_reverse(slf: Obj) -> Obj {
    // SAFETY: see above.
    shield(ptr::null_mut(), || iter_next_ptr(py(), unsafe { state(slf) }, false))
}

unsafe extern "C" fn dealloc(slf: Obj) {
    shield((), || {
        // SAFETY: see above; the object is dead, so its state is read out exactly once.
        unsafe {
            ffi::PyObject_GC_UnTrack(slf.cast());
            let ty = ffi::Py_TYPE(slf);
            let state = ptr::read(&raw const (*slf.cast::<ListIterObject>()).state);
            // An immediate decref (through `Bound`): the list may be freed right here.
            drop(state.into_inner().list.map(|list| list.into_bound(py())));
            #[cfg(not(Py_GIL_DISABLED))]
            {
                let which = usize::from(ty != TYPES[0].load(Ordering::Relaxed));
                if freelist::LISTS[which].push(slf) {
                    return;
                }
            }
            ffi::PyObject_GC_Del(slf.cast());
            ffi::Py_DECREF(ty.cast());
        }
    })
}

unsafe extern "C" fn traverse(slf: Obj, visit: ffi::visitproc, arg: *mut c_void) -> c_int {
    // Never blocks: an iterator whose state is busy is skipped (at worst a cycle survives
    // this collection).
    // SAFETY: see above.
    unsafe {
        if let Some(st) = state(slf).try_lock()
            && let Some(list) = &st.list
        {
            let r = visit(list.as_ptr(), arg);
            if r != 0 {
                return r;
            }
        }
        visit(ffi::Py_TYPE(slf).cast(), arg)
    }
}

unsafe extern "C" fn clear(slf: Obj) -> c_int {
    shield(0, || {
        let py = py();
        // SAFETY: see above.
        let list = unsafe { state(slf) }.lock(py).list.take();
        drop(list.map(|list| list.into_bound(py)));
        0
    })
}

unsafe extern "C" fn length_hint_forward(slf: Obj, _: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        fallback(ptr::null_mut(), |py| {
            // SAFETY: see above.
            let st = unsafe { state(slf) }.lock(py);
            let n = match &st.list {
                Some(list) if st.index >= 0 => list.get().len().saturating_sub(st.index as usize),
                _ => 0,
            };
            drop(st);
            Ok(n.into_pyobject(py)?.into_ptr())
        })
    })
}

unsafe extern "C" fn length_hint_reverse(slf: Obj, _: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        fallback(ptr::null_mut(), |py| {
            // SAFETY: see above.
            let st = unsafe { state(slf) }.lock(py);
            let n = match &st.list {
                Some(list) if st.index >= 0 && list.get().len() as isize > st.index => st.index as usize + 1,
                _ => 0,
            };
            drop(st);
            Ok(n.into_pyobject(py)?.into_ptr())
        })
    })
}

unsafe extern "C" fn reduce_forward(slf: Obj, _: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        fallback(ptr::null_mut(), |py| {
            // SAFETY: see above.
            let state = unsafe { state(slf) };
            Ok(iter_reduce(py, state, true)?.into_ptr())
        })
    })
}

unsafe extern "C" fn reduce_reverse(slf: Obj, _: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        fallback(ptr::null_mut(), |py| {
            // SAFETY: see above.
            let state = unsafe { state(slf) };
            Ok(iter_reduce(py, state, false)?.into_ptr())
        })
    })
}

unsafe extern "C" fn setstate_forward(slf: Obj, value: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        fallback(ptr::null_mut(), |py| {
            // SAFETY: see above; `value` is a live object.
            unsafe { iter_setstate(py, state(slf), &Bound::from_borrowed_ptr(py, value), true)? };
            Ok(super::slots::none())
        })
    })
}

unsafe extern "C" fn setstate_reverse(slf: Obj, value: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        fallback(ptr::null_mut(), |py| {
            // SAFETY: see above; `value` is a live object.
            unsafe { iter_setstate(py, state(slf), &Bound::from_borrowed_ptr(py, value), false)? };
            Ok(super::slots::none())
        })
    })
}

macro_rules! method {
    ($name:literal, $f:expr, $flags:expr, $doc:literal) => {
        ffi::PyMethodDef {
            ml_name: $name.as_ptr(),
            ml_meth: ffi::PyMethodDefPointer { PyCFunction: $f },
            ml_flags: $flags,
            ml_doc: $doc.as_ptr(),
        }
    };
}

static mut FORWARD_METHODS: [ffi::PyMethodDef; 4] = [
    method!(
        c"__length_hint__",
        length_hint_forward,
        ffi::METH_NOARGS,
        c"Private method returning an estimate of len(list(it))."
    ),
    method!(c"__reduce__", reduce_forward, ffi::METH_NOARGS, c"Return state information for pickling."),
    method!(c"__setstate__", setstate_forward, ffi::METH_O, c"Set state information for unpickling."),
    ffi::PyMethodDef::zeroed(),
];

static mut REVERSE_METHODS: [ffi::PyMethodDef; 4] = [
    method!(
        c"__length_hint__",
        length_hint_reverse,
        ffi::METH_NOARGS,
        c"Private method returning an estimate of len(list(it))."
    ),
    method!(c"__reduce__", reduce_reverse, ffi::METH_NOARGS, c"Return state information for pickling."),
    method!(c"__setstate__", setstate_reverse, ffi::METH_O, c"Set state information for unpickling."),
    ffi::PyMethodDef::zeroed(),
];

/// Create one iterator type.
///
/// # Safety
/// `methods` must be a static, NUL-terminated method table.
unsafe fn create_type(
    py: Python<'_>,
    name: &'static std::ffi::CStr,
    next: ffi::iternextfunc,
    methods: *mut ffi::PyMethodDef,
) -> PyResult<*mut ffi::PyTypeObject> {
    let mut slots = [
        ffi::PyType_Slot { slot: ffi::Py_tp_dealloc, pfunc: dealloc as *mut c_void },
        ffi::PyType_Slot { slot: ffi::Py_tp_traverse, pfunc: traverse as *mut c_void },
        ffi::PyType_Slot { slot: ffi::Py_tp_clear, pfunc: clear as *mut c_void },
        ffi::PyType_Slot { slot: ffi::Py_tp_iter, pfunc: ffi::PyObject_SelfIter as *mut c_void },
        ffi::PyType_Slot { slot: ffi::Py_tp_iternext, pfunc: next as *mut c_void },
        ffi::PyType_Slot { slot: ffi::Py_tp_methods, pfunc: methods.cast() },
        ffi::PyType_Slot { slot: 0, pfunc: ptr::null_mut() },
    ];
    let mut spec = ffi::PyType_Spec {
        name: name.as_ptr(),
        basicsize: size_of::<ListIterObject>() as c_int,
        itemsize: 0,
        flags: (ffi::Py_TPFLAGS_DEFAULT
            | ffi::Py_TPFLAGS_HAVE_GC
            | ffi::Py_TPFLAGS_IMMUTABLETYPE
            | ffi::Py_TPFLAGS_DISALLOW_INSTANTIATION) as _,
        slots: slots.as_mut_ptr(),
    };
    // SAFETY: a valid spec; the type object is a new reference, kept for the life of the
    // process (the statics below).
    unsafe { Ok(Bound::from_owned_ptr_or_err(py, ffi::PyType_FromSpec(&mut spec))?.into_ptr().cast()) }
}

/// Create the iterator types (once, at module initialisation).
pub(super) fn install(py: Python<'_>) -> PyResult<()> {
    // SAFETY: static method tables; module initialisation runs once.
    unsafe {
        let forward = create_type(py, c"rustique.ListIterator", next_forward, (&raw mut FORWARD_METHODS).cast())?;
        let reverse =
            create_type(py, c"rustique.ListReverseIterator", next_reverse, (&raw mut REVERSE_METHODS).cast())?;
        TYPES[0].store(forward, Ordering::Release);
        TYPES[1].store(reverse, Ordering::Release);
    }
    Ok(())
}
