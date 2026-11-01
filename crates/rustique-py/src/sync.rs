//! Locking discipline (ARCHITECTURE §6).
//!
//! Every container keeps its mutable state in a [`PyRwLock`]. Rules for code holding a
//! guard: no Python code (no `__eq__`/`__hash__`/iteration, no decref of arbitrary objects,
//! no allocation of GC-tracked objects). Creating `int`, `float`, `str` and `bool` objects
//! is fine. Operations are three-phase (prepare outside, commit under the guard, drop
//! displaced objects after releasing it).
//!
//! The lock has one API and two implementations:
//!
//! - **GIL builds** ([`gil`]): the GIL already serialises every attached thread and the
//!   three-phase discipline keeps Python code out of the guarded region, so the lock is a
//!   borrow flag (like `RefCell`) changed with plain loads and stores, never an atomic
//!   read-modify-write. It exists for work that runs *detached* while holding a guard (big
//!   kernels): a detached holder keeps its borrow, and attached threads that need a
//!   conflicting borrow wait detached (releasing the GIL) until the holder re-attaches and
//!   releases it. The flag only changes on attached threads, i.e. under the GIL.
//! - **Free-threaded builds** ([`ft`]): a parking_lot reader–writer lock. A thread only ever
//!   *tries* the lock, and only while attached to the interpreter. When the lock is
//!   contended it waits — detached from the interpreter after a few short spins — *without
//!   touching the lock*: it only observes the lock state (spinning while the lock changes
//!   hands quickly, then sleeping for longer and longer, capped low), then tries again
//!   while attached. Hence:
//!   - a thread never blocks on a Rustique lock while attached: a stop-the-world pause (or
//!     another thread's attach) could otherwise wait on us while we wait on a lock holder
//!     that is itself paused;
//!   - a lock changes state only on attached threads. During a stop-the-world GC pause
//!     every other thread is paused or detached, so each container's lock state is frozen
//!     and `__traverse__` (which only `try_read`s) sees the same thing in every GC phase. A
//!     detached thread that acquired or queued on the lock (setting the writer bit) could
//!     make one phase see a container's elements and another miss them, so live objects
//!     would be treated as garbage (weakrefs cleared, finalizers run).
//!
//!   Fairness without queueing on the lock: a waiter that has waited a while counts as
//!   *starving*; while any thread starves for a lock, nobody else keeps it (a newcomer's
//!   fast path releases it at once and waits) and starving readers let starving writers go
//!   first, so neither a thread that releases and immediately re-acquires nor a stream of
//!   readers can keep a waiter out.
//!
//! In both builds a guard may be held across `py.detach` (the borrow stays registered), and
//! `try_read`/`try_write` never block (for `__traverse__`).

use std::sync::atomic::{AtomicU64, Ordering};

use pyo3::prelude::*;

#[cfg(Py_GIL_DISABLED)]
mod ft;
#[cfg(not(Py_GIL_DISABLED))]
mod gil;

#[cfg(Py_GIL_DISABLED)]
pub use ft::{MutexGuard, PyMutex, PyRwLock, ReadGuard, WriteGuard};
#[cfg(not(Py_GIL_DISABLED))]
pub use gil::{MutexGuard, PyMutex, PyRwLock, ReadGuard, WriteGuard};

/// Shared access to two locks at once without risking lock-order deadlocks: one lock is
/// acquired normally (alternately `a` and `b`, so contention on either is waited out the
/// fair way), the other only tried; on failure the held one is released before retrying.
/// `a` and `b` must be different locks (callers handle the aliasing case with one guard).
pub fn read_both<'a, A: Send + Sync, B: Send + Sync>(
    py: Python<'_>,
    a: &'a PyRwLock<A>,
    b: &'a PyRwLock<B>,
) -> (ReadGuard<'a, A>, ReadGuard<'a, B>) {
    loop {
        let ga = a.read(py);
        if let Some(gb) = b.try_read_uncontested() {
            return (ga, gb);
        }
        drop(ga);
        let gb = b.read(py);
        if let Some(ga) = a.try_read_uncontested() {
            return (ga, gb);
        }
        drop(gb);
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

    /// [`MutationCounter::bump`] for callers holding the container's write lock (writers
    /// are serialised, so no read-modify-write instruction is needed).
    #[inline]
    pub fn bump_exclusive(&self) {
        self.0.store(self.0.load(Ordering::Relaxed).wrapping_add(1), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use std::hint;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    /// Wait for `handle` up to `limit`; `false` if it is still running (starved or hung).
    fn joined(handle: &thread::JoinHandle<()>, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while !handle.is_finished() {
            if Instant::now() > deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
        true
    }

    /// A writer waiting for a contended lock never touches it (no writer bit, no hold while
    /// detached): `try_read`, as used by `__traverse__`, keeps succeeding meanwhile.
    #[test]
    fn waiting_writer_leaves_the_lock_alone() {
        Python::initialize();
        let lock = Arc::new(PyRwLock::new(0u32));
        // The borrow is taken and released attached (GIL builds require it); in between,
        // this thread is detached while holding it, like a detached kernel.
        let held = Python::attach(|_| lock.try_read().unwrap());
        let writer = {
            let lock = lock.clone();
            thread::spawn(move || Python::attach(|py| *lock.write(py) += 1))
        };
        let until = Instant::now() + Duration::from_millis(100);
        while Instant::now() < until {
            let ok = Python::attach(|_| lock.try_read().is_some());
            assert!(ok, "a waiting writer made try_read fail");
        }
        assert!(!writer.is_finished());
        Python::attach(|_| drop(held));
        assert!(joined(&writer, Duration::from_secs(10)));
        assert_eq!(*lock.try_read().unwrap(), 1);
    }

    /// Hammer `lock` from `hot` threads (short holds, releasing the interpreter between
    /// iterations) while one thread needs `rounds` acquisitions of the other kind.
    fn starves(hot_writers: bool, rounds: usize) -> bool {
        Python::initialize();
        let lock = Arc::new(PyRwLock::new(0u64));
        let stop = Arc::new(AtomicBool::new(false));
        let hot: Vec<_> = (0..4)
            .map(|_| {
                let (lock, stop) = (lock.clone(), stop.clone());
                thread::spawn(move || {
                    Python::attach(|py| {
                        while !stop.load(Ordering::Relaxed) {
                            for _ in 0..100 {
                                if hot_writers {
                                    *lock.write(py) += 1;
                                } else {
                                    hint::black_box(*lock.read(py));
                                }
                            }
                            py.detach(|| ());
                        }
                    })
                })
            })
            .collect();
        let victim = {
            let lock = lock.clone();
            thread::spawn(move || {
                Python::attach(|py| {
                    for _ in 0..rounds {
                        if hot_writers {
                            hint::black_box(*lock.read(py));
                        } else {
                            *lock.write(py) += 1;
                        }
                    }
                })
            })
        };
        let done = joined(&victim, Duration::from_secs(20));
        stop.store(true, Ordering::Relaxed);
        for t in hot {
            t.join().unwrap();
        }
        !done
    }

    #[test]
    fn readers_do_not_starve_a_writer() {
        assert!(!starves(false, 200));
    }

    #[test]
    fn writers_do_not_starve_a_reader() {
        assert!(!starves(true, 200));
    }
}
