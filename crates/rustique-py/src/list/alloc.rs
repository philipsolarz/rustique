//! Raw allocation and deallocation of PyO3 class instances (`List`, `Tuple`).
//!
//! PyO3 allocates through `tp_alloc` after calling `object.__new__` (generic, zeroing) and
//! deallocates through a trampolined `tp_dealloc`: together about three times what CPython
//! spends on a `list` (which also recycles objects through a free list). For classes whose
//! instances are created on hot paths (slicing, copies, concatenation, constructors), this
//! module creates *exact* instances directly — `PyObject_GC_New` (or a recycled object),
//! the Rust value written at its offset in PyO3's layout, `PyObject_GC_Track` — and
//! installs a raw `tp_dealloc` for them. Instances of Python subclasses keep PyO3's
//! deallocator (saved at installation).
//!
//! The layout is PyO3's own: the value's offset is measured on a real instance at
//! installation, and the class's other layout slots (borrow checker, thread checker, dict,
//! weakref list) must be zero-sized, which [`RawClass::install`] checks via the basic size.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;

type Obj = *mut ffi::PyObject;

/// A value that can be dropped from a raw `tp_dealloc` (where PyO3 may not know that the
/// thread is attached, so a `Py<T>` drop would be deferred to PyO3's reference pool).
pub(crate) trait RawDrop {
    /// Drop `self`, releasing Python references immediately.
    fn drop_attached(self, py: Python<'_>);
}

/// Recycled dead objects of one class (GIL builds; accessed under the GIL only). A cached
/// object is untracked, holds no value and keeps its reference to the type.
#[cfg(not(Py_GIL_DISABLED))]
struct FreeList(UnsafeCell<(usize, [Obj; FreeList::MAX])>);

#[cfg(not(Py_GIL_DISABLED))]
impl FreeList {
    const MAX: usize = 32;

    const fn new() -> Self {
        FreeList(UnsafeCell::new((0, [ptr::null_mut(); Self::MAX])))
    }

    /// # Safety
    /// The caller must hold the GIL.
    #[inline(always)]
    unsafe fn pop(&self) -> Option<Obj> {
        // SAFETY: GIL-protected; a cached object's memory is still allocated.
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

    /// # Safety
    /// The caller must hold the GIL; `obj` must be dead, untracked and hold no value.
    #[inline(always)]
    unsafe fn push(&self, obj: Obj) -> bool {
        // SAFETY: GIL-protected.
        unsafe {
            let (len, items) = &mut *self.0.get();
            if *len == Self::MAX {
                return false;
            }
            items[*len] = obj;
            *len += 1;
            true
        }
    }
}

/// Raw allocation support for one PyO3 class `T`.
pub(crate) struct RawClass<T> {
    ty: AtomicPtr<ffi::PyTypeObject>,
    offset: AtomicUsize,
    pyo3_dealloc: AtomicPtr<()>,
    #[cfg(not(Py_GIL_DISABLED))]
    free: FreeList,
    _value: PhantomData<fn(T)>,
}

// SAFETY: the atomics are set once at installation; the free list is only used under the
// GIL (GIL builds).
unsafe impl<T> Sync for RawClass<T> {}

impl<T: pyo3::PyClass + RawDrop> RawClass<T> {
    pub(crate) const fn new() -> Self {
        RawClass {
            ty: AtomicPtr::new(ptr::null_mut()),
            offset: AtomicUsize::new(0),
            pyo3_dealloc: AtomicPtr::new(ptr::null_mut()),
            #[cfg(not(Py_GIL_DISABLED))]
            free: FreeList::new(),
            _value: PhantomData,
        }
    }

    /// Measure the layout on `sample` (a fresh exact instance) and install the raw
    /// deallocator `dealloc` (which must call [`RawClass::dealloc`] on this class).
    pub(crate) fn install(&self, sample: &Bound<'_, T>, value: *const T, dealloc: ffi::destructor) -> PyResult<()> {
        // SAFETY: the type of a live object.
        let ty = unsafe { ffi::Py_TYPE(sample.as_ptr()) };
        let offset = value as usize - sample.as_ptr() as usize;
        // SAFETY: reading the size of a ready type object.
        let basicsize = unsafe { (*ty).tp_basicsize } as usize;
        if offset + size_of::<T>() > basicsize || basicsize - (offset + size_of::<T>()) >= align_of::<T>().max(8) {
            return Err(pyo3::exceptions::PyRuntimeError::new_err("rustique: unexpected PyO3 class layout"));
        }
        self.offset.store(offset, Ordering::Relaxed);
        // SAFETY: module initialisation, before other instances exist; the slot is replaced
        // by a function with the same signature.
        unsafe {
            let old = (*ty).tp_dealloc.map_or(ptr::null_mut(), |f| f as *mut ());
            self.pyo3_dealloc.store(old, Ordering::Relaxed);
            (*ty).tp_dealloc = Some(dealloc);
            ffi::PyType_Modified(ty);
        }
        self.ty.store(ty, Ordering::Release);
        Ok(())
    }

    /// A new exact instance holding `value` (a new reference), or null with an exception.
    #[inline]
    pub(crate) fn alloc(&self, py: Python<'_>, value: T) -> Obj {
        let ty = self.ty.load(Ordering::Acquire);
        // SAFETY: `ty` is the installed class; the object is fully initialised (the value
        // written at its measured offset, every other layout slot zero-sized) before it is
        // tracked by the GC or returned.
        unsafe {
            #[cfg(not(Py_GIL_DISABLED))]
            let cached = self.free.pop();
            #[cfg(Py_GIL_DISABLED)]
            let cached: Option<Obj> = None;
            let obj = match cached {
                Some(obj) => obj,
                None => {
                    let obj = ffi::PyObject_GC_New::<ffi::PyObject>(ty);
                    if obj.is_null() {
                        value.drop_attached(py);
                        return ptr::null_mut();
                    }
                    obj
                }
            };
            ptr::write(obj.cast::<u8>().add(self.offset.load(Ordering::Relaxed)).cast::<T>(), value);
            ffi::PyObject_GC_Track(obj.cast());
            obj
        }
    }

    /// `alloc` as a `Bound`.
    #[inline]
    pub(crate) fn new_bound<'py>(&self, py: Python<'py>, value: T) -> PyResult<Bound<'py, T>> {
        // SAFETY: a new reference to an instance of `T`, or null with an exception set.
        unsafe { Ok(Bound::from_owned_ptr_or_err(py, self.alloc(py, value))?.cast_into_unchecked()) }
    }

    /// The raw `tp_dealloc` body.
    ///
    /// # Safety
    /// `slf` must be a dead instance of `T` (or of a subclass); called by CPython attached.
    #[inline]
    pub(crate) unsafe fn dealloc(&self, slf: Obj) {
        // SAFETY: see above.
        unsafe {
            let ty = ffi::Py_TYPE(slf);
            if ty != self.ty.load(Ordering::Relaxed) {
                // A Python subclass instance: PyO3's deallocator handles its layout.
                let f: ffi::destructor = std::mem::transmute(self.pyo3_dealloc.load(Ordering::Relaxed));
                return f(slf);
            }
            ffi::PyObject_GC_UnTrack(slf.cast());
            let value = ptr::read(slf.cast::<u8>().add(self.offset.load(Ordering::Relaxed)).cast::<T>());
            value.drop_attached(Python::assume_attached());
            #[cfg(not(Py_GIL_DISABLED))]
            if self.free.push(slf) {
                return;
            }
            ffi::PyObject_GC_Del(slf.cast());
            ffi::Py_DECREF(ty.cast());
        }
    }
}
