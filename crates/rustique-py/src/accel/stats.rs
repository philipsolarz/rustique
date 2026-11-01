//! Per-target counters: fast-path hits and fallbacks by reason, and the check-mode switch
//! (docs/ACCELERATION.md §6).
//!
//! Off by default: a kernel pays one relaxed byte load of [`MODE`] and a predictable branch
//! per call (+3 Ir, measured). When counting is on (`rustique.stats(True)`), counts are
//! relaxed atomic increments: exact in total, no ordering, and on free-threaded builds
//! contended targets bounce one cache line between threads (acceptable for a diagnostic
//! mode). Check mode (`check.rs`) shares the same byte, so a kernel that supports it still
//! pays one load when everything is off ([`Counters::hit_check`]).

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

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

/// Hits and fallbacks by [`Reason`] of one target (or several sharing it).
#[derive(Debug)]
pub struct Counters {
    pub hits: AtomicU64,
    pub fallback: [AtomicU64; 7],
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
        Counters { hits: AtomicU64::new(0), fallback: [const { AtomicU64::new(0) }; 7] }
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
