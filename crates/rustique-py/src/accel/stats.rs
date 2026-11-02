//! Per-target counters: fast-path hits and fallbacks by reason, and the check-mode switch
//! (docs/ACCELERATION.md §6).
//!
//! Off by default: a kernel pays one relaxed byte load of [`MODE`] and a predictable branch
//! per call (+3 Ir, measured). When counting is on (`rustique.stats(True)`), counts are
//! relaxed atomic increments: exact in total, no ordering, and on free-threaded builds
//! contended targets bounce one cache line between threads (acceptable for a diagnostic
//! mode). Check mode (`check.rs`) shares the same byte, so a kernel that supports it still
//! pays one load when everything is off ([`Counters::hit_check`]).

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU64, Ordering};

/// Bit set: [`STATS`], [`CHECK`], [`CHECK_RAISE`].
pub static MODE: AtomicU8 = AtomicU8::new(0);
pub const STATS: u8 = 1;
pub const CHECK: u8 = 2;
pub const CHECK_RAISE: u8 = 4;

/// Fallback reasons shared by all targets (index into [`Counters::fallback`]).
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Reason {
    /// Argument outside the exact subset (type, kwargs, size...).
    Args = 0,
    /// A dependency was replaced (monkeypatch).
    Deps = 1,
    /// A tracing / profiling / monitoring tool is listening.
    Tools = 2,
    /// Not the interpreter that installed the kernel.
    Interp = 3,
    /// The kernel started, met something it cannot decide, and restarted in the original.
    Deopt = 4,
    /// An error condition detected before anything observable: the original re-runs the
    /// call and raises CPython's own error.
    Error = 5,
    /// Check mode found the kernel's result different from the original's (the original's
    /// result was returned).
    Check = 6,
}

pub const REASONS: [&str; 7] = ["args", "deps", "tools", "interp", "deopt", "error", "check"];

/// Consecutive fallbacks, from a fresh (or balanced) state, after which a vectorcall target
/// is suspended (its original vectorcall restored until the next `enable()`): the floor of
/// the net-benefit score is `-STREAK_LIMIT × fallback_debit`. See [`Counters::fallback_streak`].
pub const STREAK_LIMIT: i32 = 64;

/// The net-benefit score's ceiling: after a long run of hits it takes
/// `SCORE_CEILING / debit + STREAK_LIMIT` consecutive fallbacks to suspend.
pub const SCORE_CEILING: i32 = 256;

/// Default [`TargetSpec::hit_credit`](super::TargetSpec::hit_credit) /
/// [`fallback_debit`](super::TargetSpec::fallback_debit): a target stays installed while
/// hits are at least `debit / (credit + debit)` = 20% of its calls.
pub const HIT_CREDIT: u8 = 4;
pub const FALLBACK_DEBIT: u8 = 1;

/// Hits and fallbacks by [`Reason`] of one target (or several sharing it), and the
/// net-benefit score of vectorcall targets.
#[derive(Debug)]
pub struct Counters {
    pub hits: AtomicU64,
    pub fallback: [AtomicU64; 7],
    /// Net benefit of the kernel: a hit adds `credit`, a fallback (except `Deps`) subtracts
    /// `debit`, clamped to `[floor, SCORE_CEILING]`; reaching the floor suspends. Racy
    /// load/store: approximate under contention, by design.
    pub score: AtomicI32,
    credit: AtomicI32,
    debit: AtomicI32,
    floor: AtomicI32,
    /// The score reached its floor: the registry suspends the target.
    pub suspend_requested: AtomicBool,
    /// Times the registry suspended the target for persistent fallback.
    pub suspensions: AtomicU64,
    /// The target's sites hold its kernels (installed, validated, not suspended): set by
    /// the registry. Dynamic sites check it per call and retire when it is false.
    live: AtomicBool,
    /// Dynamic sites installed and not yet retired (status `dynamic_sites`).
    pub dynamic: AtomicU64,
}

fn set_bits(bits: u8, on: bool) {
    if on {
        MODE.fetch_or(bits, Ordering::Relaxed);
    } else {
        MODE.fetch_and(!bits, Ordering::Relaxed);
    }
}

/// Switch counting on or off for every target (`rustique.stats(True)`).
pub fn set_enabled(on: bool) {
    set_bits(STATS, on);
}

pub fn enabled() -> bool {
    MODE.load(Ordering::Relaxed) & STATS != 0
}

/// Check mode: `None` off, `Some(false)` compare and return the original's result on a
/// mismatch, `Some(true)` raise `RustiqueCheckError` on a mismatch.
pub fn set_check(mode: Option<bool>) {
    set_bits(CHECK | CHECK_RAISE, false);
    match mode {
        None => {}
        Some(false) => set_bits(CHECK, true),
        Some(true) => set_bits(CHECK | CHECK_RAISE, true),
    }
}

pub fn check_mode() -> Option<bool> {
    let m = MODE.load(Ordering::Relaxed);
    (m & CHECK != 0).then_some(m & CHECK_RAISE != 0)
}

/// Is check mode on? (One relaxed byte load.)
#[inline(always)]
pub fn checking() -> bool {
    MODE.load(Ordering::Relaxed) & CHECK != 0
}

impl Counters {
    pub const fn new() -> Self {
        Counters {
            hits: AtomicU64::new(0),
            fallback: [const { AtomicU64::new(0) }; 7],
            score: AtomicI32::new(0),
            credit: AtomicI32::new(HIT_CREDIT as i32),
            debit: AtomicI32::new(FALLBACK_DEBIT as i32),
            floor: AtomicI32::new(-STREAK_LIMIT * FALLBACK_DEBIT as i32),
            suspend_requested: AtomicBool::new(false),
            suspensions: AtomicU64::new(0),
            live: AtomicBool::new(false),
            dynamic: AtomicU64::new(0),
        }
    }

    /// Whether the target is live (installed with its kernels in place): one relaxed load.
    /// Kernels of dynamic sites retire their site when it is not (`accel::dynamic::retire`).
    #[inline(always)]
    pub fn live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    pub(crate) fn set_live(&self, on: bool) {
        self.live.store(on, Ordering::Relaxed);
    }

    /// Set the score policy (the registry, from the installing target's spec; counters
    /// shared by several targets take the last installed one's) and start fresh.
    pub(crate) fn arm(&self, credit: u8, debit: u8) {
        let (credit, debit) = (i32::from(credit.max(1)), i32::from(debit.max(1)));
        self.credit.store(credit, Ordering::Relaxed);
        self.debit.store(debit, Ordering::Relaxed);
        self.floor.store(-STREAK_LIMIT * debit, Ordering::Relaxed);
        self.score.store(0, Ordering::Relaxed);
        self.suspend_requested.store(false, Ordering::Relaxed);
    }

    /// Start the score fresh (resume after a dependency suspension).
    pub(crate) fn rearm(&self) {
        self.score.store(0, Ordering::Relaxed);
    }

    /// Credit a hit: a load and a compare at the ceiling (the steady state of a target that
    /// pays off), a store below it.
    #[inline(always)]
    fn credit_hit(&self) {
        let s = self.score.load(Ordering::Relaxed);
        if s < SCORE_CEILING {
            let c = self.credit.load(Ordering::Relaxed);
            self.score.store((s + c).min(SCORE_CEILING), Ordering::Relaxed);
        }
    }

    /// [`Self::hit`] for vectorcall kernels: credits the net-benefit score.
    #[inline(always)]
    pub fn hit_streak(&self) {
        self.credit_hit();
        self.hit();
    }

    /// [`Self::hit_check`] for vectorcall kernels (credits the net-benefit score).
    #[inline(always)]
    pub fn hit_check_streak(&self) -> bool {
        self.credit_hit();
        self.hit_check()
    }

    /// [`Self::fallback`] for vectorcall kernels: every fallback debits the net-benefit
    /// score; when it reaches its floor the calling thread suspends the target (original
    /// vectorcall restored, so the remaining calls cost what CPython's do) until the next
    /// `enable()`. From a fresh state that is [`STREAK_LIMIT`] consecutive fallbacks; with
    /// default policy, traffic where fewer than 20% of the calls hit gets there too, however
    /// the hits are interleaved. On GIL builds a `Deps` fallback does not debit: a changed
    /// dependency suspends the target through its pins at once (this thread performs it)
    /// and resumes it when the pins re-arm. Free-threaded builds swap nothing in steady
    /// state for a dependency (a swap stops the world): there `Deps` debits like the others.
    #[inline(always)]
    pub fn fallback_streak(&self, why: Reason) {
        self.fallback(why);
        if matches!(why, Reason::Deps) && !cfg!(Py_GIL_DISABLED) {
            super::registry::deps_fallback();
        } else {
            self.debit_fallback();
        }
    }

    #[inline]
    fn debit_fallback(&self) {
        let s = self.score.load(Ordering::Relaxed);
        let floor = self.floor.load(Ordering::Relaxed);
        if s > floor {
            let n = (s - self.debit.load(Ordering::Relaxed)).max(floor);
            self.score.store(n, Ordering::Relaxed);
            if n == floor {
                self.request_suspend();
            }
        }
    }

    /// Ask the registry to suspend the target(s) using these counters (any site kind):
    /// their sites get their originals back until the next `enable()`. Performed on this
    /// thread when the registry is free (only pointer swaps happen: callable from a kernel or
    /// handler about to fall back, attached, outside any watcher), else by the registry's
    /// holder or a pending call. The net-benefit score calls it at its floor.
    #[cold]
    pub fn request_suspend(&self) {
        self.suspend_requested.store(true, Ordering::Release);
        super::registry::suspend_from_kernel();
    }

    #[inline(always)]
    pub fn hit(&self) {
        if MODE.load(Ordering::Relaxed) & STATS != 0 {
            self.hits.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Count a hit and tell whether check mode is on, with one load when both are off. For
    /// kernels of `checkable` targets: `if C.hit_check() { return check::verify(...) }`.
    #[inline(always)]
    pub fn hit_check(&self) -> bool {
        let m = MODE.load(Ordering::Relaxed);
        if m == 0 {
            return false;
        }
        if m & STATS != 0 {
            self.hits.fetch_add(1, Ordering::Relaxed);
        }
        m & CHECK != 0
    }

    #[inline(always)]
    pub fn fallback(&self, why: Reason) {
        if MODE.load(Ordering::Relaxed) & STATS != 0 {
            self.fallback[why as usize].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Count a check mismatch (always, also with counting off: mismatches are bugs).
    pub fn mismatch(&self) {
        self.fallback[Reason::Check as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (u64, Vec<(&'static str, u64)>) {
        (
            self.hits.load(Ordering::Relaxed),
            REASONS.iter().zip(&self.fallback).map(|(n, c)| (*n, c.load(Ordering::Relaxed))).collect(),
        )
    }

    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.suspensions.store(0, Ordering::Relaxed);
        for c in &self.fallback {
            c.store(0, Ordering::Relaxed);
        }
    }
}

impl Default for Counters {
    fn default() -> Self {
        Self::new()
    }
}
