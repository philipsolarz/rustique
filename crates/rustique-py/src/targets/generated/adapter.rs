//! Local stand-ins for framework APIs that `accel` adds in its merge "C" (announced to this
//! workstream; same names and semantics, so switching is a rename):
//!
//! - [`ClassCache`]: direct-mapped per-class verdicts keyed by `tp_version_tag` (tags are
//!   never reused within a process; a class modified while a verdict is computed is never
//!   cached under the old tag; tag 0 always misses), with an epoch for invalidations.
//! - [`watch_dict_invalidates`]: any event on a watched dict clears the class's slot and
//!   bumps the cache's epoch; no strong reference to the dict is kept (DEALLOCATED forgets
//!   it). Until accel's version lands this uses a dict watcher id of its own.
//! - [`Dynamic`]: vectorcall sites on functions created after install (generated methods,
//!   closures made by stdlib decorators), held weakly, restored at exit; while the owning
//!   target is not installed (disabled, suspended) the kernels fall back and give the
//!   function its CPython vectorcall back at its next call.

use std::collections::HashMap;
use std::ffi::c_int;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering::*};

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
    pub unsafe fn get(&self, tp: Tp) -> Option<u32> {
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
    pub unsafe fn begin(&self, tp: Tp) -> (u32, u64) {
        let e = self.epoch.load(Acquire);
        // SAFETY: attached thread, live type.
        (unsafe { ensure_tag(tp) }, e)
    }

    /// Cache `payload` for `tp` if its tag is still `tag` and no invalidation happened since
    /// [`Self::begin`].
    pub unsafe fn put(&self, tp: Tp, tag: u32, payload: u32, epoch_seen: u64) {
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
// Dict invalidation watch (temporary: own watcher id until accel's lands)
// ---------------------------------------------------------------------------------------

type DictWatchCallback = unsafe extern "C" fn(c_int, P, P, P) -> c_int;

python_extern! {
    fn PyDict_AddWatcher(cb: DictWatchCallback) -> c_int;
    fn PyDict_Watch(watcher_id: c_int, dict: P) -> c_int;
}

const DICT_EVENT_DEALLOCATED: c_int = 5;

type Entry = (&'static dyn Invalidate, usize);
static WATCHED: Mutex<Option<HashMap<usize, Vec<Entry>>>> = Mutex::new(None);
static WATCHER: AtomicI32 = AtomicI32::new(-1);

unsafe extern "C" fn on_dict_event(event: c_int, dict: P, _key: P, _new: P) -> c_int {
    let Ok(mut g) = WATCHED.lock() else { return 0 };
    let Some(m) = g.as_mut() else { return 0 };
    if event == DICT_EVENT_DEALLOCATED {
        if let Some(v) = m.remove(&(dict as usize)) {
            for (c, tp) in v {
                c.invalidate(tp);
            }
        }
    } else if let Some(v) = m.get(&(dict as usize)) {
        for (c, tp) in v {
            c.invalidate(*tp);
        }
    }
    0
}

/// Any change to `dict` (or its deallocation) clears `cache`'s verdict for `tp`. Call it
/// while computing the verdict (between `begin` and `put`).
pub unsafe fn watch_dict_invalidates(dict: P, cache: &'static dyn Invalidate, tp: Tp) -> bool {
    let mut id = WATCHER.load(Acquire);
    if id < 0 {
        // SAFETY: attached thread.
        id = unsafe { PyDict_AddWatcher(on_dict_event) };
        if id < 0 {
            // SAFETY: attached thread.
            unsafe { ffi::PyErr_Clear() };
            return false;
        }
        if let Err(prev) = WATCHER.compare_exchange(-1, id, AcqRel, Acquire) {
            id = prev;
        }
    }
    let Ok(mut g) = WATCHED.lock() else { return false };
    let m = g.get_or_insert_with(HashMap::new);
    let v = m.entry(dict as usize).or_default();
    let key = (cache as *const dyn Invalidate as *const () as usize, tp as usize);
    if !v.iter().any(|(c, t)| (*c as *const dyn Invalidate as *const () as usize, *t) == key) {
        v.push((cache, tp as usize));
    }
    drop(g);
    // SAFETY: attached thread; a live exact dict.
    unsafe {
        if PyDict_Watch(id, dict) < 0 {
            ffi::PyErr_Clear();
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------------------
// Dynamic vectorcall sites
// ---------------------------------------------------------------------------------------

/// The dynamic sites of one target: functions patched after install, owned by the target's
/// static "hook" site (the generator function whose vectorcall Rustique replaced). The
/// target counts as installed while the hook still carries its kernel.
///
/// No registry of the patched functions is kept: a per-site weak reference costs ~300 Ir
/// at class/decorator creation, enough to break never-slower on `contextmanager(f)`
/// (measured 1.066). A kernel that finds its target not installed (disabled, suspended,
/// exited) gives its function CPython's vectorcall back at that call ([`Self::retire`]);
/// until then the function falls back at C-call cost.
pub struct Dynamic {
    /// The hook function (strong reference) and its kernel.
    hook: AtomicUsize,
    hook_kernel: AtomicUsize,
    installed: AtomicUsize,
}

impl Dynamic {
    pub const fn new() -> Self {
        Dynamic { hook: AtomicUsize::new(0), hook_kernel: AtomicUsize::new(0), installed: AtomicUsize::new(0) }
    }

    /// Record the hook site (from `prepare`; the registry swaps it right after).
    pub fn set_hook(&self, hook: P, kernel: ffi::vectorcallfunc) {
        // SAFETY: a live function: keep it alive for `alive()` reads.
        let new = unsafe { incref(hook) } as usize;
        let old = self.hook.swap(new, AcqRel);
        self.hook_kernel.store(kernel as usize, Release);
        if old != 0 {
            // SAFETY: the reference we held.
            unsafe { ffi::Py_DECREF(old as P) };
        }
    }

    /// Is the owning target installed (its hook carries the kernel)?
    #[inline(always)]
    pub fn alive(&self) -> bool {
        let h = self.hook.load(Acquire);
        // SAFETY: a function object kept alive by this struct.
        h != 0 && unsafe { vectorcall_of(h as P) } == self.hook_kernel.load(Relaxed)
    }

    /// Give `func` `kernel` as vectorcall (it must still have CPython's). False if the
    /// target is not installed or the function is not an exact, unpatched function.
    pub unsafe fn install(&self, func: P, kernel: ffi::vectorcallfunc) -> bool {
        // SAFETY: per `install_forced`.
        self.alive() && unsafe { self.install_forced(func, kernel) }
    }

    /// [`Self::install`] from the target's `prepare` (the hook is swapped right after it
    /// returns; if the swap fails, the kernels find the target dead and retire).
    pub unsafe fn install_forced(&self, func: P, kernel: ffi::vectorcallfunc) -> bool {
        // SAFETY: attached thread, live object.
        unsafe {
            if !is_function(func) || vectorcall_of(func) != function_vectorcall() {
                return false;
            }
            PyFunction_SetVectorcall(func.cast(), kernel);
        }
        self.installed.fetch_add(1, Relaxed);
        true
    }

    /// A kernel found its target not installed: give `func` its CPython vectorcall (and
    /// specializer version) back, then the kernel falls back.
    #[cold]
    pub unsafe fn retire(&self, func: P, kernel: ffi::vectorcallfunc) {
        // SAFETY: attached thread, live function.
        unsafe { restore_one(func, kernel as usize) }
    }

    /// Exit: drop the hook reference (patched functions retire at their next call).
    pub fn release(&self) {
        let h = self.hook.swap(0, AcqRel);
        if h != 0 {
            // SAFETY: the reference we held.
            unsafe { ffi::Py_DECREF(h as P) };
        }
    }

    /// Number of functions given a kernel since the process started (status/tests).
    pub fn live(&self) -> usize {
        self.installed.load(Relaxed)
    }
}

impl Default for Dynamic {
    fn default() -> Self {
        Self::new()
    }
}

unsafe fn restore_one(f: P, kernel: usize) {
    // SAFETY: attached thread, live function.
    unsafe {
        if is_function(f) && vectorcall_of(f) == kernel {
            PyFunction_SetVectorcall(f.cast(), func_vectorcall());
            crate::accel::site::restore_func_version(f.cast());
        }
    }
}

