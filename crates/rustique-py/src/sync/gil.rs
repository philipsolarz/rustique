//! GIL builds: the lock is a borrow flag (see the module docs of `sync`).
//!
//! Soundness: every attached thread holds the GIL, so attached threads never run
//! concurrently. The borrow flag is only changed by attached threads (a detached thread may
//! *hold* a borrow, taken before it detached and released after it re-attached, and may
//! *observe* the flag while waiting), so its updates need no atomic read-modify-write: a
//! relaxed load and store suffice, ordered between threads by the GIL hand-off. Callers of
//! `try_read`/`try_write` (e.g. `__traverse__`) are attached as well.
//!
//! A borrow conflicts only with a borrow held by a thread that detached while holding it
//! (or with one held further up the current thread's stack, which the three-phase
//! discipline rules out: no Python code runs while a guard is held). A conflicting
//! attached thread waits detached, so the holder can re-attach and release.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::thread;
use std::time::Duration;

use pyo3::prelude::*;

/// Borrow state of a [`PyRwLock`]: `WRITER`, `0` (free) or the number of readers.
const WRITER: isize = -1;

/// Wait detached (the GIL released) until `free()` holds or a short while has passed.
#[cold]
#[inline(never)]
fn wait_detached(py: Python<'_>, round: &mut u32, free: impl Fn() -> bool + Sync) {
    let r = *round;
    *round = r.saturating_add(1);
    py.detach(|| {
        for _ in 0..64 {
            if free() {
                return;
            }
            if r < 4 {
                thread::yield_now();
            } else {
                thread::sleep(Duration::from_micros(u64::from(r.min(64)) * 8));
            }
        }
    });
}

/// A reader–writer lock for GIL builds (see the module docs).
#[derive(Default)]
pub struct PyRwLock<T> {
    state: AtomicIsize,
    data: UnsafeCell<T>,
}

// SAFETY: like `std::sync::RwLock`: the lock hands out `&T` to several threads only for
// `T: Sync`, and `&mut T` to one thread at a time.
unsafe impl<T: Send> Send for PyRwLock<T> {}
// SAFETY: as above.
unsafe impl<T: Send + Sync> Sync for PyRwLock<T> {}

impl<T: std::fmt::Debug> std::fmt::Debug for PyRwLock<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyRwLock").field("state", &self.state.load(Ordering::Relaxed)).finish_non_exhaustive()
    }
}

impl<T> PyRwLock<T> {
    pub const fn new(value: T) -> Self {
        PyRwLock { state: AtomicIsize::new(0), data: UnsafeCell::new(value) }
    }

    /// Non-blocking shared access (used by `__traverse__`, which must never block). The
    /// caller must be attached.
    #[inline]
    pub fn try_read(&self) -> Option<ReadGuard<'_, T>> {
        let s = self.state.load(Ordering::Relaxed);
        if s >= 0 {
            self.state.store(s + 1, Ordering::Relaxed);
            Some(ReadGuard { lock: self, _not_send: PhantomData })
        } else {
            None
        }
    }

    /// Non-blocking exclusive access. The caller must be attached.
    #[inline]
    pub fn try_write(&self) -> Option<WriteGuard<'_, T>> {
        if self.state.load(Ordering::Relaxed) == 0 {
            self.state.store(WRITER, Ordering::Relaxed);
            Some(WriteGuard { lock: self, _not_send: PhantomData })
        } else {
            None
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }

    #[inline]
    pub(super) fn try_read_uncontested(&self) -> Option<ReadGuard<'_, T>> {
        self.try_read()
    }

    /// Whether a writer holds the lock (an observation; may be made while detached).
    #[inline]
    pub fn is_locked_exclusive(&self) -> bool {
        self.state.load(Ordering::Relaxed) == WRITER
    }
}

impl<T: Send + Sync> PyRwLock<T> {
    /// Shared access; waits detached when a writer holds the lock.
    #[inline]
    pub fn read(&self, py: Python<'_>) -> ReadGuard<'_, T> {
        match self.try_read() {
            Some(guard) => guard,
            None => self.read_contended(py),
        }
    }

    /// Exclusive access; waits detached while the lock is held.
    #[inline]
    pub fn write(&self, py: Python<'_>) -> WriteGuard<'_, T> {
        match self.try_write() {
            Some(guard) => guard,
            None => self.write_contended(py),
        }
    }

    #[cold]
    #[inline(never)]
    fn read_contended(&self, py: Python<'_>) -> ReadGuard<'_, T> {
        let mut round = 0;
        loop {
            wait_detached(py, &mut round, || self.state.load(Ordering::Relaxed) >= 0);
            if let Some(guard) = self.try_read() {
                return guard;
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn write_contended(&self, py: Python<'_>) -> WriteGuard<'_, T> {
        let mut round = 0;
        loop {
            wait_detached(py, &mut round, || self.state.load(Ordering::Relaxed) == 0);
            if let Some(guard) = self.try_write() {
                return guard;
            }
        }
    }
}

/// Shared access to a [`PyRwLock`]'s data.
pub struct ReadGuard<'a, T> {
    lock: &'a PyRwLock<T>,
    _not_send: PhantomData<*const ()>,
}

// SAFETY: a shared reference to the guard only gives `&T`.
unsafe impl<T: Sync> Sync for ReadGuard<'_, T> {}

impl<T> Deref for ReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: a reader borrow is registered, so no `&mut T` exists.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for ReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        let state = &self.lock.state;
        state.store(state.load(Ordering::Relaxed) - 1, Ordering::Relaxed);
    }
}

/// Exclusive access to a [`PyRwLock`]'s data.
pub struct WriteGuard<'a, T> {
    lock: &'a PyRwLock<T>,
    _not_send: PhantomData<*const ()>,
}

// SAFETY: a shared reference to the guard only gives `&T`.
unsafe impl<T: Sync> Sync for WriteGuard<'_, T> {}

impl<T> Deref for WriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the writer borrow is registered: no other reference exists.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for WriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as above.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for WriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.state.store(0, Ordering::Relaxed);
    }
}

/// A mutex for GIL builds: a borrow flag, like [`PyRwLock`]. It guards short per-object
/// state (an iterator's position).
#[derive(Default)]
pub struct PyMutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: like `std::sync::Mutex`.
unsafe impl<T: Send> Send for PyMutex<T> {}
// SAFETY: as above.
unsafe impl<T: Send> Sync for PyMutex<T> {}

impl<T> std::fmt::Debug for PyMutex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyMutex").field("locked", &self.locked.load(Ordering::Relaxed)).finish_non_exhaustive()
    }
}

impl<T> PyMutex<T> {
    pub const fn new(value: T) -> Self {
        PyMutex { locked: AtomicBool::new(false), data: UnsafeCell::new(value) }
    }

    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }

    /// Non-blocking access (for `__traverse__`). The caller must be attached.
    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self.locked.load(Ordering::Relaxed) {
            None
        } else {
            self.locked.store(true, Ordering::Relaxed);
            Some(MutexGuard { lock: self, _not_send: PhantomData })
        }
    }
}

impl<T: Send> PyMutex<T> {
    #[inline]
    pub fn lock(&self, py: Python<'_>) -> MutexGuard<'_, T> {
        match self.try_lock() {
            Some(guard) => guard,
            None => self.lock_contended(py),
        }
    }

    #[cold]
    #[inline(never)]
    fn lock_contended(&self, py: Python<'_>) -> MutexGuard<'_, T> {
        let mut round = 0;
        loop {
            wait_detached(py, &mut round, || !self.locked.load(Ordering::Relaxed));
            if let Some(guard) = self.try_lock() {
                return guard;
            }
        }
    }
}

/// Exclusive access to a [`PyMutex`]'s data.
pub struct MutexGuard<'a, T> {
    lock: &'a PyMutex<T>,
    _not_send: PhantomData<*const ()>,
}

// SAFETY: a shared reference to the guard only gives `&T`.
unsafe impl<T: Sync> Sync for MutexGuard<'_, T> {}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the lock is held: no other reference exists.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as above.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Relaxed);
    }
}
