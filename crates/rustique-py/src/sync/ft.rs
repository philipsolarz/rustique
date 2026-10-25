//! Free-threaded builds: a parking_lot reader–writer lock that is only ever *tried* while
//! attached, with detached, fair waiting (see the module docs of `sync`).

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
    pub(super) fn try_read_uncontested(&self) -> Option<ReadGuard<'_, T>> {
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
        acquire_contended(
            py,
            Some(&self.starving),
            Access::Shared,
            || self.inner.try_read(),
            || !self.inner.is_locked_exclusive(),
        )
    }

    #[cold]
    #[inline(never)]
    fn write_contended(&self, py: Python<'_>, held: Option<WriteGuard<'_, T>>) -> WriteGuard<'_, T> {
        drop(held);
        acquire_contended(
            py,
            Some(&self.starving),
            Access::Exclusive,
            || self.inner.try_write(),
            || !self.inner.is_locked(),
        )
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

    pub fn into_inner(self) -> T {
        self.inner.into_inner()
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
