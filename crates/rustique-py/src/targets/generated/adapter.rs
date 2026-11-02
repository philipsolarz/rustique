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

/// Per-class facts: `N` slots of `(tag << 32) | payload`, indexed by the type's address.
pub struct ClassCache<const N: usize> {
    slots: [AtomicU64; N],
    epoch: AtomicU64,
}

impl<const N: usize> ClassCache<N> {
    pub const fn new() -> Self {
        ClassCache { slots: [const { AtomicU64::new(0) }; N], epoch: AtomicU64::new(0) }
    }

    #[inline(always)]
    fn slot(&self, tp: Tp) -> &AtomicU64 {
        &self.slots[(tp as usize >> 4) % N]
    }

    /// The payload cached for `tp` at its current tag.
    #[inline(always)]
    pub unsafe fn get(&self, tp: Tp) -> Option<u32> {
        // SAFETY: live type.
        let tag = unsafe { tag_of(tp) };
        let v = self.slot(tp).load(Relaxed);
        (tag != 0 && (v >> 32) as u32 == tag).then_some(v as u32)
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
        let s = self.slot(tp);
        let v = (u64::from(tag) << 32) | u64::from(payload);
        s.store(v, Release);
        if self.epoch.load(Acquire) != epoch_seen {
            let _ = s.compare_exchange(v, 0, AcqRel, Relaxed);
        }
    }

    /// Forget `tp`'s verdict (an invalidation event).
    pub fn clear(&self, tp: usize) {
        self.epoch.fetch_add(1, AcqRel);
        self.slots[(tp >> 4) % N].store(0, Release);
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
pub struct Dynamic {
    /// The hook function (strong reference) and its kernel.
    hook: AtomicUsize,
    hook_kernel: AtomicUsize,
    /// Weak references to the patched functions, with the kernel each got.
    sites: Mutex<Vec<(usize, usize)>>,
    prune_at: AtomicUsize,
}

impl Dynamic {
    pub const fn new() -> Self {
        Dynamic {
            hook: AtomicUsize::new(0),
            hook_kernel: AtomicUsize::new(0),
            sites: Mutex::new(Vec::new()),
            prune_at: AtomicUsize::new(64),
        }
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
            if !is_function(func) || vectorcall_of(func) != _PyFunction_Vectorcall as usize {
                return false;
            }
            let wr = ffi::PyWeakref_NewRef(func, std::ptr::null_mut());
            if wr.is_null() {
                ffi::PyErr_Clear();
                return false;
            }
            let Ok(mut v) = self.sites.lock() else {
                ffi::Py_DECREF(wr);
                return false;
            };
            v.push((wr as usize, kernel as usize));
            let n = v.len();
            if n >= self.prune_at.load(Relaxed) {
                prune(&mut v);
                self.prune_at.store((v.len() * 2).max(64), Relaxed);
            }
            drop(v);
            PyFunction_SetVectorcall(func.cast(), kernel);
            true
        }
    }

    /// A kernel found its target not installed: give `func` its CPython vectorcall (and
    /// specializer version) back, then the kernel falls back.
    #[cold]
    pub unsafe fn retire(&self, func: P, kernel: ffi::vectorcallfunc) {
        // SAFETY: attached thread, live function.
        unsafe { restore_one(func, kernel as usize) }
    }

    /// Restore every live patched function (exit, or an explicit reset).
    pub fn restore_all(&self) {
        let sites = match self.sites.lock() {
            Ok(mut v) => std::mem::take(&mut *v),
            Err(_) => return,
        };
        // SAFETY: attached thread (exit hook / Python-facing call); our weak references.
        unsafe {
            for (wr, k) in sites {
                let mut f: P = std::ptr::null_mut();
                if PyWeakref_GetRef(wr as P, &mut f) > 0 && !f.is_null() {
                    restore_one(f, k);
                    ffi::Py_DECREF(f);
                } else {
                    ffi::PyErr_Clear();
                }
                ffi::Py_DECREF(wr as P);
            }
        }
    }

    /// Exit: restore everything and drop the hook reference.
    pub fn release(&self) {
        self.restore_all();
        let h = self.hook.swap(0, AcqRel);
        if h != 0 {
            // SAFETY: the reference we held.
            unsafe { ffi::Py_DECREF(h as P) };
        }
    }

    /// Number of patched functions still alive (status/tests).
    pub fn live(&self) -> usize {
        let Ok(mut v) = self.sites.lock() else { return 0 };
        // SAFETY: attached thread (Python-facing caller).
        unsafe { prune(&mut v) };
        v.len()
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
            PyFunction_SetVectorcall(f.cast(), _PyFunction_Vectorcall);
            crate::accel::site::restore_func_version(f.cast());
        }
    }
}

/// Drop the entries whose function died.
unsafe fn prune(v: &mut Vec<(usize, usize)>) {
    v.retain(|&(wr, _)| {
        let mut f: P = std::ptr::null_mut();
        // SAFETY: our weak reference.
        let live = unsafe { PyWeakref_GetRef(wr as P, &mut f) } > 0;
        // SAFETY: as above.
        unsafe {
            if live {
                ffi::Py_DECREF(f);
            } else {
                ffi::PyErr_Clear();
                ffi::Py_DECREF(wr as P);
            }
        }
        live
    });
}
