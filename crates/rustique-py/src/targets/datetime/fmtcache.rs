//! Compiled formats keyed by the format `str` object: a lock-free, insert-only table of
//! (strong reference to the key, leaked value) consulted by pointer first (a few
//! instructions for the literal formats programs pass), backed by a per-thread cache keyed
//! by content for everything else (formats built at run time).
//!
//! Keys are kept alive by the table, so an equal pointer is the same (immutable) string.

use std::cell::RefCell;
use std::sync::atomic::{AtomicPtr, Ordering};

use pyo3::ffi;

const SLOTS: usize = 64;

pub struct Table<T: 'static> {
    keys: [AtomicPtr<ffi::PyObject>; SLOTS],
    vals: [AtomicPtr<Option<T>>; SLOTS],
}

impl<T> Table<T> {
    pub const fn new() -> Self {
        Table {
            keys: [const { AtomicPtr::new(std::ptr::null_mut()) }; SLOTS],
            vals: [const { AtomicPtr::new(std::ptr::null_mut()) }; SLOTS],
        }
    }

    /// The entry for this very object, if any (`Some(None)`: known unsupported).
    #[inline(always)]
    pub fn get(&self, key: *mut ffi::PyObject) -> Option<&'static Option<T>> {
        for i in 0..SLOTS {
            let k = self.keys[i].load(Ordering::Acquire);
            if k.is_null() {
                return None;
            }
            if k == key {
                let v = self.vals[i].load(Ordering::Acquire);
                // SAFETY: values are leaked and never freed.
                return (!v.is_null()).then(|| unsafe { &*v });
            }
        }
        None
    }

    /// Remember `val` for `key` (no-op when full or already present).
    ///
    /// # Safety
    /// Attached thread; `key` a live `str`.
    pub unsafe fn insert(&self, key: *mut ffi::PyObject, val: Option<T>) -> Option<&'static Option<T>> {
        for i in 0..SLOTS {
            let k = self.keys[i].load(Ordering::Acquire);
            if k == key {
                return None; // another thread inserted it
            }
            if k.is_null() {
                // SAFETY: the table keeps a strong reference for the process lifetime.
                unsafe { ffi::Py_INCREF(key) };
                if self.keys[i].compare_exchange(std::ptr::null_mut(), key, Ordering::AcqRel, Ordering::Acquire).is_ok()
                {
                    let v = Box::into_raw(Box::new(val));
                    self.vals[i].store(v, Ordering::Release);
                    // SAFETY: leaked.
                    return Some(unsafe { &*v });
                }
                // SAFETY: undo our reference; slot taken meanwhile.
                unsafe { ffi::Py_DECREF(key) };
            }
        }
        None
    }
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-thread content-keyed cache (formats built at run time).
pub struct Local<T: 'static> {
    entries: RefCell<Vec<(Box<[u8]>, std::rc::Rc<Option<T>>)>>,
}

impl<T> Local<T> {
    pub const fn new() -> Self {
        Local { entries: RefCell::new(Vec::new()) }
    }

    pub fn get_or(&self, fmt: &[u8], make: impl FnOnce() -> Option<T>) -> std::rc::Rc<Option<T>> {
        let mut c = self.entries.borrow_mut();
        if let Some((_, v)) = c.iter().find(|(f, _)| &**f == fmt) {
            return v.clone();
        }
        let v = std::rc::Rc::new(make());
        if c.len() >= 16 {
            c.clear();
        }
        c.push((fmt.into(), v.clone()));
        v
    }
}

impl<T> Default for Local<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A cached value: from the table (static) or the per-thread cache.
pub enum Cached<T: 'static> {
    Static(&'static T),
    Local(std::rc::Rc<Option<T>>),
}

impl<T> std::ops::Deref for Cached<T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Cached::Static(t) => t,
            Cached::Local(rc) => rc.as_ref().as_ref().expect("Cached::Local holds Some"),
        }
    }
}

/// Look `fmt` up by identity, then by content; compile (`make`) on a miss. `None`: the
/// format is not ASCII or not supported.
///
/// # Safety
/// Attached thread; `fmt` a live exact `str`.
#[inline(always)]
pub unsafe fn lookup<T: Clone>(
    table: &'static Table<T>,
    local: &'static std::thread::LocalKey<Local<T>>,
    fmt: *mut ffi::PyObject,
    make: impl FnOnce(&[u8]) -> Option<T>,
) -> Option<Cached<T>> {
    if let Some(v) = table.get(fmt) {
        return v.as_ref().map(Cached::Static);
    }
    // SAFETY: per the caller.
    unsafe { miss(table, local, fmt, make) }
}

#[cold]
#[inline(never)]
unsafe fn miss<T: Clone>(
    table: &'static Table<T>,
    local: &'static std::thread::LocalKey<Local<T>>,
    fmt: *mut ffi::PyObject,
    make: impl FnOnce(&[u8]) -> Option<T>,
) -> Option<Cached<T>> {
    // SAFETY: per the caller.
    unsafe {
        let fb = super::ustr::ascii(fmt)?;
        let rc = local.with(|l| l.get_or(fb, || make(fb)));
        if let Some(v) = table.insert(fmt, rc.as_ref().clone()) {
            return v.as_ref().map(Cached::Static);
        }
        rc.is_some().then_some(Cached::Local(rc))
    }
}
