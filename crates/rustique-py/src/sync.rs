//! Locking discipline (ARCHITECTURE §6).
//!
//! Every container keeps its mutable state in a [`PyRwLock`]. Acquisition first tries the
//! lock without blocking; when it is contended the thread waits *detached* from the
//! interpreter and then retries. A thread never blocks on a Rustique lock while attached:
//! on free-threaded builds a stop-the-world GC (or another thread's attach) could
//! otherwise wait on us while we wait on a lock holder that is itself paused.
//!
//! Rules for code holding a guard: no Python code (no `__eq__`/`__hash__`/iteration, no
//! decref of arbitrary objects, no allocation of GC-tracked objects). Creating `int`,
//! `float`, `str` and `bool` objects is fine.

use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use pyo3::prelude::*;

pub type ReadGuard<'a, T> = RwLockReadGuard<'a, T>;
pub type WriteGuard<'a, T> = RwLockWriteGuard<'a, T>;

/// A reader–writer lock that never blocks while attached to the interpreter.
#[derive(Debug, Default)]
pub struct PyRwLock<T> {
    inner: RwLock<T>,
}

impl<T> PyRwLock<T> {
    pub const fn new(value: T) -> Self {
        PyRwLock { inner: RwLock::new(value) }
    }

    /// Non-blocking shared access (used by `__traverse__`, which must never block).
    #[inline]
    pub fn try_read(&self) -> Option<ReadGuard<'_, T>> {
        self.inner.try_read()
    }

    /// Non-blocking exclusive access.
    #[inline]
    pub fn try_write(&self) -> Option<WriteGuard<'_, T>> {
        self.inner.try_write()
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

impl<T: Send + Sync> PyRwLock<T> {
    /// Shared access; waits detached from the interpreter when contended.
    #[inline]
    pub fn read(&self, py: Python<'_>) -> ReadGuard<'_, T> {
        match self.inner.try_read() {
            Some(guard) => guard,
            None => self.read_contended(py),
        }
    }

    /// Exclusive access; waits detached from the interpreter when contended.
    #[inline]
    pub fn write(&self, py: Python<'_>) -> WriteGuard<'_, T> {
        match self.inner.try_write() {
            Some(guard) => guard,
            None => self.write_contended(py),
        }
    }

    #[cold]
    #[inline(never)]
    fn read_contended(&self, py: Python<'_>) -> ReadGuard<'_, T> {
        loop {
            // Wait until the lock is free without holding the interpreter, then retry the
            // non-blocking acquisition while attached again.
            py.detach(|| drop(self.inner.read()));
            if let Some(guard) = self.inner.try_read() {
                return guard;
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn write_contended(&self, py: Python<'_>) -> WriteGuard<'_, T> {
        loop {
            py.detach(|| drop(self.inner.write()));
            if let Some(guard) = self.inner.try_write() {
                return guard;
            }
        }
    }
}

/// Shared access to two locks at once without risking lock-order deadlocks: the first lock
/// is acquired normally, the second only with `try_read`; on failure both are released and
/// the thread waits (detached) for the second before retrying. `a` and `b` must be
/// different locks (callers handle the aliasing case with a single guard).
pub fn read_both<'a, A: Send + Sync, B: Send + Sync>(
    py: Python<'_>,
    a: &'a PyRwLock<A>,
    b: &'a PyRwLock<B>,
) -> (ReadGuard<'a, A>, ReadGuard<'a, B>) {
    loop {
        let ga = a.read(py);
        if let Some(gb) = b.inner.try_read() {
            return (ga, gb);
        }
        drop(ga);
        py.detach(|| drop(b.inner.read()));
    }
}

/// Monotonic mutation counter. Writers bump it (under their write lock) on every change;
/// code that snapshots data under a lock and works on it outside (comparisons, sorting with
/// a key, Python-level searches) re-validates against it before committing results.
#[derive(Debug, Default)]
pub struct MutationCounter(AtomicU64);

impl MutationCounter {
    pub const fn new() -> Self {
        MutationCounter(AtomicU64::new(0))
    }

    #[inline]
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    #[inline]
    pub fn bump(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}
