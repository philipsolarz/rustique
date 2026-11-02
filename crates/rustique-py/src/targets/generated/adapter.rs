//! Framework glue of the data-model groups on top of `accel`:
//!
//! - [`ClassCache`]: two-way set-associative per-class verdicts keyed by `tp_version_tag`
//!   (tags are never reused within a process; a class modified while a verdict is computed
//!   is never cached under the old tag; tag 0 always misses), with an epoch for
//!   invalidations, plus verdicts keyed by an object address (watched dicts). Same design as
//!   `accel::guard::ClassCache` (which grew from this one); kept here for the address-keyed
//!   verdicts.
//! - [`watch_dict_invalidates`]: `accel::guard::watch_dict_invalidates` (any event on a
//!   watched dict clears the verdict; no strong reference to the dict).
//! - [`Dynamic`]: `accel::dynamic` sites of one target (functions created after install).

use std::sync::atomic::{AtomicU64, Ordering::*};

use pyo3::ffi;

use super::common::*;

// ---------------------------------------------------------------------------------------
// ClassCache
// ---------------------------------------------------------------------------------------

/// Per-class facts: `N` slots of `(tag << 32) | payload`, two-way set-associative, the set
/// chosen by a multiplicative hash of the type's address (`N` even). Tags identify a class
/// (and its state) uniquely, so a slot holding the type's current tag is its verdict.
pub struct ClassCache<const N: usize> {
    slots: [AtomicU64; N],
    epoch: AtomicU64,
}

impl<const N: usize> ClassCache<N> {
    pub const fn new() -> Self {
        ClassCache { slots: [const { AtomicU64::new(0) }; N], epoch: AtomicU64::new(0) }
    }

    #[inline(always)]
    fn set(&self, tp: usize) -> usize {
        let h = ((tp as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize;
        (h % (N / 2)) * 2
    }

    /// The payload cached for `tp` at its current tag.
    #[inline(always)]
    pub(crate) unsafe fn get(&self, tp: Tp) -> Option<u32> {
        // SAFETY: live type.
        let tag = unsafe { tag_of(tp) };
        if tag == 0 {
            return None;
        }
        let i = self.set(tp as usize);
        let v = self.slots[i].load(Relaxed);
        if (v >> 32) as u32 == tag {
            return Some(v as u32);
        }
        let v = self.slots[i + 1].load(Relaxed);
        ((v >> 32) as u32 == tag).then_some(v as u32)
    }

    /// Start computing a verdict: the type's tag (assigned if missing) and the epoch.
    pub(crate) unsafe fn begin(&self, tp: Tp) -> (u32, u64) {
        let e = self.epoch.load(Acquire);
        // SAFETY: attached thread, live type.
        (unsafe { ensure_tag(tp) }, e)
    }

    /// Cache `payload` for `tp` if its tag is still `tag` and no invalidation happened since
    /// [`Self::begin`].
    pub(crate) unsafe fn put(&self, tp: Tp, tag: u32, payload: u32, epoch_seen: u64) {
        // SAFETY: live type.
        if tag == 0 || unsafe { tag_of(tp) } != tag {
            return;
        }
        let i = self.set(tp as usize);
        let way = if self.slots[i].load(Relaxed) == 0 {
            i
        } else if self.slots[i + 1].load(Relaxed) == 0 {
            i + 1
        } else {
            i + (tag as usize & 1)
        };
        let s = &self.slots[way];
        let v = (u64::from(tag) << 32) | u64::from(payload);
        s.store(v, Release);
        if self.epoch.load(Acquire) != epoch_seen {
            let _ = s.compare_exchange(v, 0, AcqRel, Relaxed);
        }
    }

    /// The current epoch (start of a verdict keyed by an address).
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Acquire)
    }

    /// Is the object at `key` (a non-type object, e.g. a watched dict) marked good?
    #[inline(always)]
    pub fn get_addr(&self, key: usize) -> bool {
        let i = self.set(key);
        let k = key as u64;
        k != 0 && (self.slots[i].load(Relaxed) == k || self.slots[i + 1].load(Relaxed) == k)
    }

    /// Mark the object at `key` good unless an invalidation happened since `epoch_seen`.
    pub fn put_addr(&self, key: usize, epoch_seen: u64) {
        let i = self.set(key);
        let way = if self.slots[i].load(Relaxed) == 0 { i } else { i + 1 };
        let k = key as u64;
        self.slots[way].store(k, Release);
        if self.epoch.load(Acquire) != epoch_seen {
            let _ = self.slots[way].compare_exchange(k, 0, AcqRel, Relaxed);
        }
    }

    /// Forget `tp`'s verdict (an invalidation event).
    pub fn clear(&self, tp: usize) {
        self.epoch.fetch_add(1, AcqRel);
        let i = self.set(tp);
        self.slots[i].store(0, Release);
        self.slots[i + 1].store(0, Release);
    }

    /// Forget everything (disable/exit).
    pub fn clear_all(&self) {
        self.epoch.fetch_add(1, AcqRel);
        for s in &self.slots {
            s.store(0, Release);
        }
    }
}

impl<const N: usize> Default for ClassCache<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// What a dict event invalidates.
pub trait Invalidate: Sync {
    fn invalidate(&self, tp: usize);
}

impl<const N: usize> Invalidate for ClassCache<N> {
    fn invalidate(&self, tp: usize) {
        self.clear(tp);
    }
}

// ---------------------------------------------------------------------------------------
// Dict invalidation watch (accel's: one dict watcher id for every target)
// ---------------------------------------------------------------------------------------

impl<const N: usize> crate::accel::guard::Invalidate for ClassCache<N> {
    fn invalidate(&self, tp: usize) {
        self.clear(tp);
    }
}

/// Any change to `dict` (or its deallocation) clears `cache`'s verdict for `tp`
/// (`accel::guard::watch_dict_invalidates`: no strong reference to the dict). Call it while
/// computing the verdict (between `begin` and `put`).
pub(crate) unsafe fn watch_dict_invalidates(
    dict: P,
    cache: &'static dyn crate::accel::guard::Invalidate,
    tp: Tp,
) -> bool {
    // SAFETY: attached thread; a live exact dict.
    unsafe {
        let py = pyo3::Python::assume_attached();
        let Ok(d) = pyo3::Bound::from_borrowed_ptr(py, dict).cast_into::<pyo3::types::PyDict>() else { return false };
        match crate::accel::guard::watch_dict_invalidates(&d, cache, tp as usize) {
            Ok(()) => true,
            Err(_) => {
                ffi::PyErr_Clear();
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Dynamic vectorcall sites
// ---------------------------------------------------------------------------------------

/// The dynamic sites of one target: a thin layer over `accel::dynamic` (no per-site
/// registry: a weak reference per site cost ~300 Ir and broke never-slower on
/// `contextmanager(f)`; a kernel whose target is no longer live — disabled, suspended,
/// exited — gives its function CPython's vectorcall back at that call and falls back).
pub struct Dynamic {
    spec: &'static crate::accel::TargetSpec,
}

impl Dynamic {
    pub const fn new(spec: &'static crate::accel::TargetSpec) -> Self {
        Dynamic { spec }
    }

    /// Is the owning target live (installed with its kernels in place)?
    #[inline(always)]
    pub fn alive(&self) -> bool {
        self.spec.counters.live()
    }

    /// Give `func` `kernel` as vectorcall (it must still have CPython's). False if the
    /// target is not live or the function is not an exact, unpatched function.
    pub(crate) unsafe fn install(&self, func: P, kernel: ffi::vectorcallfunc) -> bool {
        // SAFETY: attached thread; a live object.
        unsafe {
            let py = pyo3::Python::assume_attached();
            let f = pyo3::Bound::from_borrowed_ptr(py, func);
            crate::accel::dynamic::install(py, self.spec, &f, kernel, &crate::accel::FUNCTION_VECTORCALL)
        }
    }

    /// A kernel found its target not live: give `func` its CPython vectorcall (and
    /// specializer version) back, then the kernel falls back.
    #[cold]
    pub(crate) unsafe fn retire(&self, func: P, kernel: ffi::vectorcallfunc) {
        // SAFETY: per `accel::dynamic::retire`.
        unsafe { crate::accel::dynamic::retire(func, kernel, &crate::accel::FUNCTION_VECTORCALL, self.spec.counters) }
    }

    /// Exit: nothing to release (kept for the targets' `release` hooks).
    pub fn release(&self) {}

    /// Sites installed and not retired (status/tests).
    pub fn live(&self) -> usize {
        self.spec.counters.dynamic.load(Relaxed) as usize
    }
}
