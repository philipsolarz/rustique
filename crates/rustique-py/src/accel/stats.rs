//! Per-target counters: fast-path hits and fallbacks by reason.
//!
//! Off by default: a kernel pays one relaxed load of [`ON`] and a predictable branch per
//! call. When switched on (`RUSTIQUE_STATS=1` / `rustique.stats(True)`), counts are relaxed
//! atomic increments: exact in total, no ordering, and on free-threaded builds contended
//! targets bounce one cache line between threads (acceptable for a diagnostic mode).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub static ON: AtomicBool = AtomicBool::new(false);

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
}

pub const REASONS: [&str; 5] = ["args", "deps", "tools", "interp", "deopt"];

#[derive(Debug)]
pub struct Counters {
    pub hits: AtomicU64,
    pub fallback: [AtomicU64; 5],
}

impl Counters {
    pub const fn new() -> Self {
        Counters { hits: AtomicU64::new(0), fallback: [const { AtomicU64::new(0) }; 5] }
    }

    #[inline(always)]
    pub fn hit(&self) {
        if ON.load(Ordering::Relaxed) {
            self.hits.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[inline(always)]
    pub fn fallback(&self, why: Reason) {
        if ON.load(Ordering::Relaxed) {
            self.fallback[why as usize].fetch_add(1, Ordering::Relaxed);
        }
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
