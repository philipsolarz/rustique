//! The rule (w2-runtime §4.1, prototype `benches/runtime/gcadapt.py` v4) and how it acts.
//!
//! **Rule.** Young and middle collections are untouched. A full collection (GIL build:
//! generation 2; free-threaded build: every collection, which walks the whole heap) may run
//! once the old generation has grown by a factor `G` since the last one. CPython's own rule
//! is `G = 1/4` (`gc.c` `gc_select_generation`: `long_lived_pending >= long_lived_total / 4`;
//! `gc_free_threading.c` `gc_should_collect`: `count >= long_lived_total / 4`). The policy
//! sets `G = 2^k / 4`, `k ∈ 0..=3`, from what full collections *find*: yield < 1% → `k + 1`;
//! yield ≥ 5% → `k = 0`; in between `k − 1`.
//!
//! **Pause cap.** A deferred full collection is released as soon as it would examine more
//! than 1.1× the objects of the largest legitimate full collection so far (one run under
//! CPython's rule counts with all it examined; one the policy deferred only with its
//! survivors, so deferred garbage never raises the cap). Size of the next full collection:
//! GIL `heap_size` (tracked objects: exactly what a full collection examines); FT survivors
//! + `young.count` (net allocations).
//!
//! **Mechanism.** The policy writes `gcstate->long_lived_total` — the denominator of
//! CPython's own 25% rule — and nothing else: `2^k ×` the survivors CPython recorded (GIL),
//! `max(survivors, 4 × limit)` (FT). Thresholds, counts, the enabled flag and the frozen set
//! stay the program's, so `gc.get_threshold()` & co. report exactly what they report without
//! the tier. CPython resets the field at every full collection (GIL) / every collection
//! (FT); the observer re-applies the policy's value in that collection's `stop` callback.
//! Readers of the field (`grep long_lived_total`, 3.14.9 and 3.15.1): the generation choice
//! (GIL) and the trigger (FT), which are the point; FT's mark phase prefetch heuristic
//! (`use_prefetch = long_lived_total > 200000`), which runs after the `start` callback — the
//! observer restores CPython's value there; `_testinternalcapi.get_long_lived_total()`
//! (CPython's internal test module, not a documented API).
//!
//! **Stepping aside** (`program_tuned`): the program's own `gc.set_threshold`, `enable`,
//! `disable`, `freeze`, `unfreeze` or `set_debug` call ends the policy for the rest of the
//! process; the field gets CPython's value back at once. Explicit `gc.collect()` is never
//! intercepted (its collections feed the rule like any other).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU8, AtomicUsize, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use super::layout::{self, Layout};

const FT: bool = cfg!(Py_GIL_DISABLED);

/// Parameters of the rule (`_t_gc.set_params`, research only).
#[derive(Debug, Clone, Copy)]
pub(super) struct Params {
    pub y_low: f64,
    pub y_high: f64,
    pub kmax: u32,
    /// 0: no pause cap.
    pub cap: f64,
    /// false: never step aside (research: exercise the policy under test suites that tune
    /// the GC).
    pub step_aside: bool,
}

pub(super) const DEFAULT_PARAMS: Params = Params { y_low: 0.01, y_high: 0.05, kmax: 3, cap: 1.1, step_aside: true };

#[derive(Debug)]
struct Rule {
    p: Params,
    k: u32,
    /// Objects examined by the largest legitimate full collection.
    max_full: isize,
    fulls: u64,
    /// GIL: generation-1 collections run where CPython's rule would have run a full one.
    deferred: u64,
    /// GIL: `heap_size` at the start of the running full collection (3.14 has no
    /// `candidates` in the callback info).
    start_heap: isize,
    /// (k, yield) of the last full collections.
    history: Vec<(u32, f64)>,
    /// First-full-collection consistency check: (examined, collected, survivors).
    first: Option<(isize, isize, isize)>,
}

impl Rule {
    const fn new(p: Params) -> Self {
        Rule { p, k: 0, max_full: 0, fulls: 0, deferred: 0, start_heap: -1, history: Vec::new(), first: None }
    }

    /// After a full collection that freed `col` objects: `y_den` objects examined (the
    /// yield's denominator: `candidates`), `size` its size in the unit of the pause cap
    /// (GIL: tracked objects examined; FT: objects in the GC heap, the unit of
    /// `long_lived_total`).
    fn on_full(&mut self, y_den: isize, size: isize, col: isize) {
        let y = if y_den > 0 { col as f64 / y_den as f64 } else { 0.0 };
        let legit = if self.k == 0 { size } else { size - col };
        self.max_full = self.max_full.max(legit);
        self.fulls += 1;
        if self.history.len() >= 40 {
            self.history.remove(0);
        }
        self.history.push((self.k, (y * 1e4).round() / 1e4));
        if y < self.p.y_low {
            self.k = (self.k + 1).min(self.p.kmax);
        } else if y >= self.p.y_high {
            self.k = 0;
        } else if self.k > 0 {
            self.k -= 1;
        }
    }

    fn capped(&self, next_size: isize) -> bool {
        self.p.cap > 0.0 && next_size as f64 >= self.p.cap * self.max_full as f64
    }

    /// GIL: the `long_lived_total` that makes CPython's rule wait for growth `G`.
    fn gil_field(&self, real: isize, heap: isize) -> isize {
        if self.k == 0 || self.capped(heap) { real } else { real << self.k }
    }

    /// FT: collect when `young.count >= limit` (and CPython's `count > threshold0`).
    fn ft_field(&self, real: isize, t0: isize) -> isize {
        if self.k == 0 {
            return real;
        }
        let mut limit = t0.max((real << self.k) >> 2);
        if self.p.cap > 0.0 {
            let room = (self.p.cap * self.max_full as f64) as isize - real;
            limit = t0.max(limit.min(room));
        }
        real.max(limit.saturating_mul(4))
    }
}

static RULE: Mutex<Rule> = Mutex::new(Rule::new(DEFAULT_PARAMS));
static PARAMS: Mutex<Params> = Mutex::new(DEFAULT_PARAMS);

/// The policy acts (tier installed and armed, not stepped aside).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// `long_lived_total` holds the policy's value (not CPython's).
static DIRTY: AtomicBool = AtomicBool::new(false);
/// CPython's `long_lived_total` as of its last write (−1: unknown).
static REAL: AtomicIsize = AtomicIsize::new(-1);
/// Research switch (`Params::step_aside`).
static STEP_ASIDE: AtomicBool = AtomicBool::new(true);
/// Why the policy stepped aside for good (0: it did not).
static STEPPED: AtomicU8 = AtomicU8::new(0);
/// `gc.set_threshold`'s method def and the hook installed there: the policy acts only while
/// the hooks are in place (installed by the registry, not disabled).
static HOOK_DEF: AtomicPtr<ffi::PyMethodDef> = AtomicPtr::new(std::ptr::null_mut());
static HOOK_KERNEL: AtomicUsize = AtomicUsize::new(0);

/// Reasons for stepping aside (`STEPPED`).
pub(super) const REASONS: &[&str] = &[
    "",
    "gc.set_threshold",
    "gc.enable",
    "gc.disable",
    "gc.freeze",
    "gc.unfreeze",
    "gc.set_debug",
    "GC disabled before the tier was enabled",
    "thresholds changed before the tier was enabled",
    "debug flags set before the tier was enabled",
    "objects frozen before the tier was enabled",
    "collector state check failed at the first full collection",
];
pub(super) const R_DISABLED_AT_ENABLE: u8 = 7;
pub(super) const R_THRESHOLDS_AT_ENABLE: u8 = 8;
pub(super) const R_DEBUG_AT_ENABLE: u8 = 9;
pub(super) const R_FROZEN_AT_ENABLE: u8 = 10;
const R_CHECK: u8 = 11;

fn rule() -> std::sync::MutexGuard<'static, Rule> {
    RULE.lock().unwrap_or_else(|p| p.into_inner())
}

#[inline]
fn hooks_installed() -> bool {
    let def = HOOK_DEF.load(Ordering::Acquire);
    if def.is_null() {
        return false;
    }
    // SAFETY: a static method table entry of the `gc` module (process lifetime); the
    // registry swaps `ml_meth` with atomic stores.
    let cur = unsafe { (*((&raw const (*def).ml_meth) as *const AtomicUsize)).load(Ordering::Acquire) };
    cur == HOOK_KERNEL.load(Ordering::Relaxed)
}

/// Give CPython its own value back if the field holds the policy's.
fn restore(l: &Layout) {
    if DIRTY.swap(false, Ordering::SeqCst) {
        let real = REAL.load(Ordering::SeqCst);
        if real >= 0 {
            l.set_isize(l.off.long_lived_total, real);
        }
    }
}

/// Write the policy's value (`want`) given CPython's (`real`), then re-check that the policy
/// is still active (a concurrent step-aside on a free-threaded build restores after us).
fn apply(l: &Layout, real: isize, want: isize) {
    if want == real {
        restore(l);
        return;
    }
    l.set_isize(l.off.long_lived_total, want);
    DIRTY.store(true, Ordering::SeqCst);
    if !ACTIVE.load(Ordering::SeqCst) {
        restore(l);
    }
}

/// Arm the policy at install (`fresh`: the program has not tuned the GC). `def`/`kernel`:
/// the `gc.set_threshold` entry and the hook the registry will put there.
pub(super) fn arm(l: &Layout, fresh: Option<u8>, def: *mut ffi::PyMethodDef, kernel: usize) {
    restore(l);
    if let Some(r) = fresh {
        let _ = STEPPED.compare_exchange(0, r, Ordering::SeqCst, Ordering::SeqCst);
    }
    REAL.store(l.isize(l.off.long_lived_total), Ordering::SeqCst);
    *rule() = Rule::new(*PARAMS.lock().unwrap_or_else(|p| p.into_inner()));
    HOOK_KERNEL.store(kernel, Ordering::Relaxed);
    HOOK_DEF.store(def, Ordering::Release);
    ACTIVE.store(STEPPED.load(Ordering::SeqCst) == 0, Ordering::SeqCst);
}

/// Stop acting (not permanent: at exit, or when the target is released).
pub(super) fn disarm() {
    ACTIVE.store(false, Ordering::SeqCst);
    if let Some(l) = layout::get() {
        restore(l);
    }
}

/// The program tuned the GC: step aside for the rest of the process.
pub(super) fn program_tuned(reason: u8) {
    if !STEP_ASIDE.load(Ordering::Relaxed) {
        return;
    }
    let _ = STEPPED.compare_exchange(0, reason, Ordering::SeqCst, Ordering::SeqCst);
    disarm();
}

/// The hooks' fast test: is there a policy to hand control back from? (One relaxed load:
/// once the policy stepped aside, or while it is passive, a hooked call costs CPython's
/// function plus this.)
#[inline(always)]
pub(super) fn watching() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub(super) fn stepped_aside() -> Option<&'static str> {
    match STEPPED.load(Ordering::SeqCst) {
        0 => None,
        r => Some(REASONS.get(r as usize).copied().unwrap_or("?")),
    }
}

pub(super) fn set_params(p: Params) {
    STEP_ASIDE.store(p.step_aside, Ordering::Relaxed);
    *PARAMS.lock().unwrap_or_else(|e| e.into_inner()) = p;
    rule().p = p;
}

/// `info[key]` as a Py_ssize_t (the callback info dict CPython builds), or `None`.
///
/// # Safety
/// Attached thread; `info` is an exact dict or NULL.
unsafe fn info_get(info: *mut ffi::PyObject, key: &Bound<'_, pyo3::types::PyString>) -> Option<isize> {
    if info.is_null() {
        return None;
    }
    // SAFETY: per the caller; exact dict, exact str key: no Python code runs.
    unsafe {
        if ffi::PyDict_CheckExact(info) == 0 {
            return None;
        }
        let mut v: *mut ffi::PyObject = std::ptr::null_mut();
        if ffi::PyDict_GetItemRef(info, key.as_ptr(), &mut v) <= 0 {
            ffi::PyErr_Clear();
            return None;
        }
        let n = if ffi::PyLong_CheckExact(v) != 0 { ffi::PyLong_AsSsize_t(v) } else { -1 };
        ffi::Py_DECREF(v);
        if n == -1 && !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(n)
    }
}

/// One phase of one collection (called by the observer before the program's callbacks).
///
/// # Safety
/// Attached thread, inside `invoke_gc_callback` (the collector's `collecting` flag is held:
/// calls are serialized); `phase` is CPython's phase string, `info` its info dict.
pub(super) unsafe fn on_phase(py: Python<'_>, phase: *mut ffi::PyObject, info: *mut ffi::PyObject) {
    let Some(l) = layout::get() else { return };
    if !(ACTIVE.load(Ordering::SeqCst) && hooks_installed()) {
        restore(l);
        return;
    }
    // "start" (5 characters) or "stop" (4).
    // SAFETY: CPython's phase string.
    let start = unsafe { ffi::PyUnicode_GetLength(phase) } == 5;
    // SAFETY: per the caller.
    let generation = unsafe { info_get(info, pyo3::intern!(py, "generation")) }.unwrap_or(-1);
    let Ok(mut r) = RULE.try_lock() else { return };
    if FT {
        if start {
            // The mark phase's prefetch heuristic reads the field: CPython's value.
            restore(l);
            return;
        }
        // CPython has just written the survivors of this (whole-heap) collection.
        DIRTY.store(false, Ordering::SeqCst);
        let real = l.isize(l.off.long_lived_total);
        // SAFETY: per the caller.
        let col = unsafe {
            info_get(info, pyo3::intern!(py, "collected")).unwrap_or(0)
                + info_get(info, pyo3::intern!(py, "uncollectable")).unwrap_or(0)
        };
        // FT's `long_lived_total` counts every object of the GC heap (untracked and frozen
        // ones too: a collection walks them all); `candidates` only the marked ones.
        let size = real + col;
        // SAFETY: per the caller.
        let y_den = unsafe { info_get(info, pyo3::intern!(py, "candidates")) }.unwrap_or(size);
        if !first_check(l, &mut r, size, col, real) {
            return;
        }
        r.on_full(y_den, size, col);
        REAL.store(real, Ordering::SeqCst);
        let t0 = l.i32(l.off.thresholds[0]) as isize;
        let want = r.ft_field(real, t0);
        drop(r);
        apply(l, real, want);
        return;
    }
    if start {
        if generation == 2 {
            r.start_heap = l.isize(l.off.heap_size);
        } else if generation == 1 && r.k > 0 {
            // CPython's own rule (with its real total) would have collected generation 2.
            let counts = l.counts();
            let thr2 = l.i32(l.off.thresholds[2]);
            let real = REAL.load(Ordering::SeqCst);
            // `gc_select_generation` ran just before this callback (counts not yet updated).
            if counts[2] > thr2 && real >= 0 && l.isize(l.off.long_lived_pending) >= real / 4 {
                r.deferred += 1;
                super::COUNTERS.hit();
            }
        }
        return;
    }
    let heap = l.isize(l.off.heap_size);
    let (real, want) = if generation == 2 {
        DIRTY.store(false, Ordering::SeqCst);
        let real = l.isize(l.off.long_lived_total);
        // SAFETY: per the caller.
        let col = unsafe {
            info_get(info, pyo3::intern!(py, "collected")).unwrap_or(0)
                + info_get(info, pyo3::intern!(py, "uncollectable")).unwrap_or(0)
        };
        // SAFETY: per the caller.
        let examined = match unsafe { info_get(info, pyo3::intern!(py, "candidates")) } {
            Some(c) => c,
            None if r.start_heap >= 0 => r.start_heap,
            None => real + col,
        };
        if !first_check(l, &mut r, examined, col, real) {
            return;
        }
        r.on_full(examined, examined, col);
        REAL.store(real, Ordering::SeqCst);
        (real, r.gil_field(real, heap))
    } else if generation == 1 && r.k > 0 {
        let real = REAL.load(Ordering::SeqCst);
        if real < 0 {
            return;
        }
        (real, r.gil_field(real, heap))
    } else {
        return;
    };
    drop(r);
    apply(l, real, want);
}

/// The first full collection seen checks the calibrated fields against what CPython
/// reports for it: survivors (`long_lived_total`) cannot exceed what was examined (FT:
/// `examined` is survivors + freed, so only the signs are checked). On failure the policy
/// steps aside for good.
fn first_check(l: &Layout, r: &mut Rule, examined: isize, col: isize, real: isize) -> bool {
    if r.first.is_some() {
        return true;
    }
    r.first = Some((examined, col, real));
    if real < 0 || examined < 0 || col < 0 || real > examined + 1024 {
        let _ = STEPPED.compare_exchange(0, R_CHECK, Ordering::SeqCst, Ordering::SeqCst);
        ACTIVE.store(false, Ordering::SeqCst);
        restore(l);
        return false;
    }
    true
}

/// `_t_gc.report()`: the policy's state, for tests and benchmarks.
pub(super) fn report<'py>(py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("active", ACTIVE.load(Ordering::SeqCst))?;
    d.set_item("armed", ACTIVE.load(Ordering::SeqCst) && hooks_installed())?;
    d.set_item("hooks", hooks_installed())?;
    d.set_item("stepped_aside", stepped_aside())?;
    d.set_item("dirty", DIRTY.load(Ordering::SeqCst))?;
    d.set_item("real_total", REAL.load(Ordering::SeqCst))?;
    d.set_item("ft", FT)?;
    d.set_item("forwarding", super::observer::swapped())?;
    if let Ok(r) = RULE.try_lock() {
        d.set_item("k", r.k)?;
        d.set_item("max_full", r.max_full)?;
        d.set_item("fulls", r.fulls)?;
        d.set_item("deferred", r.deferred)?;
        d.set_item("history", r.history.clone())?;
        d.set_item("first_full", r.first)?;
        d.set_item("params", (r.p.y_low, r.p.y_high, r.p.kmax, r.p.cap, r.p.step_aside))?;
    }
    if let Some(l) = layout::get() {
        let (enabled, thr, debug) = l.config();
        d.set_item("long_lived_total", l.isize(l.off.long_lived_total))?;
        d.set_item("heap_size", if FT { -1 } else { l.isize(l.off.heap_size) })?;
        d.set_item("long_lived_pending", if FT { -1 } else { l.isize(l.off.long_lived_pending) })?;
        d.set_item("thresholds", thr)?;
        d.set_item("counts", l.counts())?;
        d.set_item("enabled", enabled)?;
        d.set_item("debug", debug)?;
        d.set_item("frozen", l.frozen())?;
        // SAFETY: borrowed pointer read from the collector state; compared by identity only.
        d.set_item("callbacks_ptr", l.ptr(l.off.callbacks) as usize)?;
        d.set_item("layout", l.describe())?;
    }
    d.set_item("program_list", super::observer::program_list() as usize)?;
    d.set_item("private_list", super::observer::private_list() as usize)?;
    Ok(d)
}
