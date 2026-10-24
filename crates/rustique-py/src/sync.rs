//! Locking discipline (ARCHITECTURE §6).
//!
//! Every container keeps its mutable state in a [`PyRwLock`]. A thread only ever *tries*
//! the lock, and only while attached to the interpreter. When the lock is contended it
//! waits — detached from the interpreter after a few short spins — *without touching the
//! lock*: it only observes the lock state (spinning while the lock changes hands quickly,
//! then sleeping for longer and longer, capped low), then tries again while attached. Hence:
//!
//! - a thread never blocks on a Rustique lock while attached: on free-threaded builds a
//!   stop-the-world pause (or another thread's attach) could otherwise wait on us while we
//!   wait on a lock holder that is itself paused;
//! - a lock changes state only on attached threads. During a stop-the-world GC pause every
//!   other thread is paused or detached, so each container's lock state is frozen and
//!   `__traverse__` (which only `try_read`s) sees the same thing in every GC phase. A
//!   detached thread that acquired or queued on the lock (setting the writer bit) could make
//!   one phase see a container's elements and another miss them, so live objects would be
//!   treated as garbage (weakrefs cleared, finalizers run).
//!
//! Fairness without queueing on the lock: a waiter that has waited a while counts as
//! *starving* ([`Starvation`]); while any thread starves for a lock, nobody else keeps it
//! (a newcomer's fast path releases it at once and waits) and starving readers let
//! starving writers go first, so neither a thread that releases and immediately
//! re-acquires nor a stream of readers can keep a waiter out.
//!
//! Rules for code holding a guard: no Python code (no `__eq__`/`__hash__`/iteration, no
//! decref of arbitrary objects, no allocation of GC-tracked objects). Creating `int`,
//! `float`, `str` and `bool` objects is fine.

use std::hint;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use pyo3::prelude::*;

pub type ReadGuard<'a, T> = RwLockReadGuard<'a, T>;
pub type WriteGuard<'a, T> = RwLockWriteGuard<'a, T>;
pub use parking_lot::MutexGuard;

/// How a waiter spends its time without touching the lock: a few short spins while
/// attached (critical sections are usually tiny), then, detached, spinning while the lock
/// has been unavailable only briefly and sleeps that grow with that time (a quarter of it,
/// capped). A waiter that has waited [`Backoff::PATIENCE`] starves (see [`Starvation`]);
/// it then polls eagerly, since others are deferring to it.
struct Backoff {
    /// Rounds spun while attached.
    spins: u32,
    /// Start of the wait.
    since: Instant,
    /// Since when the lock has looked unavailable to this waiter.
    blocked: Instant,
}

impl Backoff {
    const ATTACHED_SPINS: u32 = 4;
    /// Detached spinning before sleeping (starving waiters: [`Backoff::STARVING_SPIN`]).
    const SPIN: Duration = Duration::from_micros(50);
    const STARVING_SPIN: Duration = Duration::from_millis(1);
    const PATIENCE: Duration = Duration::from_micros(200);
    /// Longest sleep between two looks at the lock (starving waiters: `STARVING_SLEEP`).
    const MAX_SLEEP: Duration = Duration::from_micros(500);
    const STARVING_SLEEP: Duration = Duration::from_micros(50);

    fn new() -> Self {
        let now = Instant::now();
        Backoff { spins: 0, since: now, blocked: now }
    }

    fn impatient(&self) -> bool {
        self.spins >= Self::ATTACHED_SPINS && self.since.elapsed() >= Self::PATIENCE
    }

    /// One round of waiting, until `ready()` (an observation of the lock state that must
    /// not acquire it) holds or the round ends.
    fn wait(&mut self, py: Python<'_>, starving: bool, ready: impl Fn() -> bool + Sync) {
        if self.spins < Self::ATTACHED_SPINS {
            spin(1 << self.spins);
            self.spins += 1;
            return;
        }
        if ready() {
            self.blocked = Instant::now();
            return;
        }
        let blocked = self.blocked.elapsed();
        let (spin_for, max_sleep) =
            if starving { (Self::STARVING_SPIN, Self::STARVING_SLEEP) } else { (Self::SPIN, Self::MAX_SLEEP) };
        py.detach(|| {
            if blocked < spin_for {
                for _ in 0..64 {
                    if ready() {
                        return;
                    }
                    spin(16);
                }
            } else {
                thread::sleep((blocked / 4).min(max_sleep));
            }
        });
    }
}

#[inline]
fn spin(n: u32) {
    for _ in 0..n {
        hint::spin_loop();
    }
}

/// Kind of access a waiter wants.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Shared,
    Exclusive,
}

/// Waiters starving for a lock: readers in the low half, writers in the high half. While
/// any thread starves, only starving threads keep the lock (newcomers let go at once), and
/// starving readers let starving writers go first (so readers cannot keep a starving writer out).
#[derive(Debug, Default)]
struct Starvation(AtomicU64);

impl Starvation {
    const WRITER: u64 = 1 << 32;

    const fn new() -> Self {
        Starvation(AtomicU64::new(0))
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.0.load(Ordering::Relaxed) == 0
    }

    /// Whether a waiter may try the lock now.
    fn may_try(&self, access: Access, starving: bool) -> bool {
        let state = self.0.load(Ordering::Relaxed);
        state == 0 || (starving && (access == Access::Exclusive || state < Self::WRITER))
    }

    fn unit(access: Access) -> u64 {
        if access == Access::Exclusive { Self::WRITER } else { 1 }
    }
}

/// Marks the current thread as starving for a lock until dropped.
struct Starving<'a>(&'a Starvation, u64);

impl<'a> Starving<'a> {
    fn new(starvation: &'a Starvation, access: Access) -> Self {
        let unit = Starvation::unit(access);
        starvation.0.fetch_add(unit, Ordering::Relaxed);
        Starving(starvation, unit)
    }
}

impl Drop for Starving<'_> {
    fn drop(&mut self) {
        self.0.0.fetch_sub(self.1, Ordering::Relaxed);
    }
}

/// The contended path of every acquisition: alternate `try_lock` (while attached) with
/// backoff rounds that only observe the lock (`free`), deferring to starving waiters until
/// the thread starves itself.
#[cold]
#[inline(never)]
fn acquire_contended<G>(
    py: Python<'_>,
    starvation: Option<&Starvation>,
    access: Access,
    try_lock: impl Fn() -> Option<G>,
    free: impl Fn() -> bool + Sync,
) -> G {
    let mut backoff = Backoff::new();
    let mut me: Option<Starving<'_>> = None;
    let may_try = |starving: bool| starvation.is_none_or(|s| s.may_try(access, starving));
    loop {
        let starving = me.is_some();
        backoff.wait(py, starving, || free() && may_try(starving));
        if may_try(starving)
            && let Some(guard) = try_lock()
        {
            return guard;
        }
        if me.is_none()
            && let Some(starvation) = starvation
            && backoff.impatient()
        {
            me = Some(Starving::new(starvation, access));
        }
    }
}

/// A reader–writer lock that never blocks while attached to the interpreter and is never
/// acquired or queued on while detached (see the module docs).
#[derive(Debug, Default)]
#[repr(C)]
pub struct PyRwLock<T> {
    /// Threads starving for this lock (only they may keep it). Next to the lock word, so
    /// the fast paths touch one cache line.
    starving: Starvation,
    inner: RwLock<T>,
}

impl<T> PyRwLock<T> {
    pub const fn new(value: T) -> Self {
        PyRwLock { starving: Starvation::new(), inner: RwLock::new(value) }
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

    #[inline]
    fn uncontested(&self) -> bool {
        self.starving.is_empty()
    }

    /// A newcomer's attempt: fails while the lock is contended (see [`Starvation`]).
    fn try_read_uncontested(&self) -> Option<ReadGuard<'_, T>> {
        if self.uncontested() { self.inner.try_read() } else { None }
    }
}

impl<T: Send + Sync> PyRwLock<T> {
    /// Shared access; waits detached from the interpreter when contended.
    #[inline]
    pub fn read(&self, py: Python<'_>) -> ReadGuard<'_, T> {
        // The fairness check follows the acquisition: its load then costs nothing extra
        // (the lock word's cache line is already owned), and while threads starve the
        // newcomer holds the lock only for an instant.
        match self.inner.try_read() {
            Some(guard) if self.uncontested() => guard,
            held => self.read_contended(py, held),
        }
    }

    /// Exclusive access; waits detached from the interpreter when contended.
    #[inline]
    pub fn write(&self, py: Python<'_>) -> WriteGuard<'_, T> {
        match self.inner.try_write() {
            Some(guard) if self.uncontested() => guard,
            held => self.write_contended(py, held),
        }
    }

    #[cold]
    #[inline(never)]
    fn read_contended(&self, py: Python<'_>, held: Option<ReadGuard<'_, T>>) -> ReadGuard<'_, T> {
        drop(held);
        acquire_contended(py, Some(&self.starving), Access::Shared, || self.inner.try_read(), || !self.inner.is_locked_exclusive())
    }

    #[cold]
    #[inline(never)]
    fn write_contended(&self, py: Python<'_>, held: Option<WriteGuard<'_, T>>) -> WriteGuard<'_, T> {
        drop(held);
        acquire_contended(py, Some(&self.starving), Access::Exclusive, || self.inner.try_write(), || !self.inner.is_locked())
    }
}

/// A mutex that never blocks while attached to the interpreter and is never acquired or
/// queued on while detached (see [`PyRwLock`]). It guards short per-object state (an
/// iterator's position), so it skips the starvation bookkeeping.
#[derive(Debug, Default)]
pub struct PyMutex<T> {
    inner: Mutex<T>,
}

impl<T> PyMutex<T> {
    pub const fn new(value: T) -> Self {
        PyMutex { inner: Mutex::new(value) }
    }

    /// Non-blocking access (for `__traverse__`).
    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.inner.try_lock()
    }
}

impl<T: Send> PyMutex<T> {
    #[inline]
    pub fn lock(&self, py: Python<'_>) -> MutexGuard<'_, T> {
        match self.inner.try_lock() {
            Some(guard) => guard,
            None => self.lock_contended(py),
        }
    }

    #[cold]
    #[inline(never)]
    fn lock_contended(&self, py: Python<'_>) -> MutexGuard<'_, T> {
        acquire_contended(py, None, Access::Exclusive, || self.inner.try_lock(), || !self.inner.is_locked())
    }
}

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
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

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
        let held = lock.try_read().unwrap();
        let writer = {
            let lock = lock.clone();
            thread::spawn(move || Python::attach(|py| *lock.write(py) += 1))
        };
        let until = Instant::now() + Duration::from_millis(100);
        while Instant::now() < until {
            assert!(lock.try_read().is_some(), "a waiting writer made try_read fail");
        }
        assert!(!writer.is_finished());
        drop(held);
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
