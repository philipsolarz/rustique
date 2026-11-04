//! Framework glue of the data-model groups on top of `accel`:
//!
//! - [`AddrCache`]: verdicts keyed by an object address (watched dicts), the same design
//!   as `accel::ClassCache` (used for per-class verdicts).
//! - [`watch_dict_invalidates`]: `accel::guard::watch_dict_invalidates` (any event on a
//!   watched dict clears the verdict; no strong reference to the dict).
//! - [`Dynamic`]: `accel::dynamic` sites of one target (functions created after install).

use std::sync::atomic::{AtomicU64, Ordering::*};

use pyo3::ffi;

use super::common::*;

// ---------------------------------------------------------------------------------------
// AddrCache
// ---------------------------------------------------------------------------------------

/// Verdicts about objects that are not classes (a watched dict: "its keys compare in C
/// only"), keyed by the object's address: `N` slots in two-way sets (multiplicative hash),
/// an epoch for invalidations. A watched object's deallocation clears its slot before the
/// address can be reused.
pub struct AddrCache<const N: usize> {
    slots: [AtomicU64; N],
    epoch: AtomicU64,
}

impl<const N: usize> AddrCache<N> {
    pub const fn new() -> Self {
        AddrCache { slots: [const { AtomicU64::new(0) }; N], epoch: AtomicU64::new(0) }
    }

    #[inline(always)]
    fn set(&self, key: usize) -> usize {
        let h = ((key as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize;
        (h % (N / 2)) * 2
    }

    /// The current epoch (start of a verdict).
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Acquire)
    }

    /// Is the object at `key` marked good?
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

    /// Forget `key`'s verdict (an invalidation event).
    pub fn clear(&self, key: usize) {
        self.epoch.fetch_add(1, AcqRel);
        let i = self.set(key);
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

impl<const N: usize> Default for AddrCache<N> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------------------
// Dict invalidation watch (accel's: one dict watcher id for every target)
// ---------------------------------------------------------------------------------------

impl<const N: usize> crate::accel::guard::Invalidate for AddrCache<N> {
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
