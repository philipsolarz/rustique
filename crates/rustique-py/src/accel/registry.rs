//! The target registry (docs/ACCELERATION.md §3, §6, §9): selection (`only`/`exclude`),
//! the per-target gate, all-or-nothing install from `prepare`'s declarations, lazy
//! activation on import, `disable()`, status and counters.
//!
//! Locking. One registry lock (a CPython `PyMutex`: a thread blocking on it detaches, so
//! it never deadlocks against the GIL, a critical section or a stop-the-world). It is
//! never taken while the pin tables lock (`guard`, a leaf) is held. The `sys.modules`
//! watcher must not block, detach or release the GIL (it runs inside a dict mutation, under
//! the dict's critical section on free-threaded builds: parking would suspend that section
//! mid-mutation), so it only *tries* the lock; when the lock is busy it queues the module,
//! and whoever holds the lock drains the queue before (and right after) releasing it. On
//! free-threaded builds the watcher always queues: activation can take other dicts'
//! critical sections and stop the world (`PyFunction_SetVectorcall`), so it runs from a
//! pending call (any thread, between bytecodes) or the next registry operation.
//!
//! Suspensions (net benefit, §5.6) are performed by the thread whose fallback asks for one,
//! right there (the kernel is attached and outside any watcher), when the registry is free;
//! otherwise by the registry's holder or a pending call.

use std::cell::{Cell, UnsafeCell};
use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::ffi;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule};

use super::guard;
use super::prepare::{self, FingerprintRecord, Prepare};
use super::site::{Kind, Site};
use super::spec::{Category, GroupSpec, Refusal, TargetSpec};
use super::stats;
use super::version;

// ---------------------------------------------------------------------------------------
// The lock
// ---------------------------------------------------------------------------------------

struct PyLock<T> {
    bits: AtomicU8,
    data: UnsafeCell<T>,
}

// SAFETY: `data` is only reached through a guard, which exists for one holder at a time.
unsafe impl<T: Send> Sync for PyLock<T> {}

struct Guard<'a, T> {
    lock: &'a PyLock<T>,
}

thread_local! {
    static HOLDING: Cell<bool> = const { Cell::new(false) };
}

impl<T> PyLock<T> {
    const fn new(v: T) -> Self {
        PyLock { bits: AtomicU8::new(0), data: UnsafeCell::new(v) }
    }

    fn raw(&self) -> *mut ffi::PyMutex {
        // `PyMutex` is `#[repr(transparent)]` over its `AtomicU8` bits.
        (&raw const self.bits).cast_mut().cast()
    }

    /// Blocking acquire (detaches while waiting). Errors on re-entry from the holder.
    fn lock(&self, _py: Python<'_>) -> PyResult<Guard<'_, T>> {
        if HOLDING.get() {
            return Err(PyRuntimeError::new_err("rustique: re-entrant call into the accel registry"));
        }
        guard::assert_not_in_tables("the registry lock");
        // SAFETY: a valid PyMutex that never moves (static).
        unsafe { ffi::PyMutex_Lock(self.raw()) };
        HOLDING.set(true);
        Ok(Guard { lock: self })
    }

    /// Non-blocking acquire (CPython's own fast path: 0 → locked).
    fn try_lock(&self) -> Option<Guard<'_, T>> {
        if HOLDING.get() {
            return None;
        }
        self.bits.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).ok()?;
        HOLDING.set(true);
        Some(Guard { lock: self })
    }
}

impl<T> std::ops::Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: exclusive while the guard lives.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> std::ops::DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: exclusive while the guard lives.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        HOLDING.set(false);
        // SAFETY: held by this guard.
        unsafe { ffi::PyMutex_Unlock(self.lock.raw()) };
    }
}

// ---------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Disabled,
    Pending,
    Installed,
    Refused,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Disabled => "disabled",
            State::Pending => "pending-import",
            State::Installed => "installed",
            State::Refused => "refused",
        }
    }
}

struct Rt {
    state: State,
    reason: Option<String>,
    /// Category of the refusal (state `Refused`, or a pending target's last attempt).
    refusal: Option<Category>,
    /// Installed sites (empty unless `Installed`).
    sites: Vec<Site>,
    /// Names and kinds of the sites last declared (kept for status after refusal/disable;
    /// labels are formatted by `status()` only).
    site_names: Vec<(String, Kind)>,
    /// Pin owner id of the installed pins (0: none).
    pin_owner: u64,
    /// Flags of the pins, tracked for deopt-while-unpinned (targets with a vectorcall
    /// site); empty otherwise.
    tracked: Vec<&'static guard::DepFlag>,
    /// Sites restored to the originals: a pinned dependency changed, or the kernel fell
    /// back persistently.
    suspended: Option<Suspended>,
    /// Times suspended since install.
    suspensions: u32,
    /// Index in `Registry::groups` of the shared declarations this target uses.
    group: Option<usize>,
    /// Flags of the target's own pins (status: `deps_intact`).
    flags: Vec<&'static guard::DepFlag>,
}

impl Rt {
    const fn new() -> Self {
        Rt {
            state: State::Disabled,
            reason: None,
            refusal: None,
            sites: Vec::new(),
            site_names: Vec::new(),
            pin_owner: 0,
            tracked: Vec::new(),
            suspended: None,
            suspensions: 0,
            group: None,
            flags: Vec::new(),
        }
    }
}

/// Why a vectorcall target's sites are temporarily restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Suspended {
    /// A pinned dependency changed; resumed when every pin re-arms.
    Deps,
    /// The net-benefit score reached its floor ([`stats::STREAK_LIMIT`] consecutive
    /// fallbacks from fresh, or mostly-fallback traffic); resumed by the next `enable()`.
    Fallback,
}

impl Suspended {
    fn as_str(self) -> &'static str {
        match self {
            Suspended::Deps => "deps",
            Suspended::Fallback => "fallback",
        }
    }
}

struct Registry {
    rt: Vec<Rt>,
    only: Option<Vec<String>>,
    exclude: Vec<String>,
    groups: Vec<Group>,
    /// Incremented at the start of each registry pass (a group's refusal is remembered for
    /// the pass only).
    pass: u64,
}

/// A [`GroupSpec`]'s state: its pins' owner while any user is installed.
struct Group {
    spec: &'static GroupSpec,
    owner: u64,
    users: u32,
    flags: Vec<&'static guard::DepFlag>,
    refused: Option<(u64, Refusal)>,
}

static REG: PyLock<Option<Registry>> = PyLock::new(None);

/// Targets whose lazy validation failed after install (`degrade`): spec address → reason.
static DEGRADED: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
static ANY_DEGRADED: AtomicBool = AtomicBool::new(false);

/// Mark an installed target as degraded: a validation it performs lazily (after install,
/// e.g. on first use) failed, so its kernels fall back. Idempotent (the first reason
/// stays), cheap, callable from kernels (no Python code, a short process-local lock). The
/// state stays "installed" and counters keep counting fallbacks; `status()` shows
/// `"degraded": reason` until the target is disabled.
pub fn degrade(spec: &'static TargetSpec, reason: &str) {
    let key = spec as *const TargetSpec as usize;
    let mut d = DEGRADED.lock().unwrap_or_else(|p| p.into_inner());
    if !d.iter().any(|(k, _)| *k == key) {
        d.push((key, reason.to_owned()));
        ANY_DEGRADED.store(true, Ordering::Relaxed);
    }
}

fn degraded_reason(spec: &'static TargetSpec) -> Option<String> {
    if !ANY_DEGRADED.load(Ordering::Relaxed) {
        return None;
    }
    let key = spec as *const TargetSpec as usize;
    DEGRADED.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(k, _)| *k == key).map(|(_, r)| r.clone())
}

fn clear_degraded(spec: &'static TargetSpec) {
    if ANY_DEGRADED.load(Ordering::Relaxed) {
        let key = spec as *const TargetSpec as usize;
        DEGRADED.lock().unwrap_or_else(|p| p.into_inner()).retain(|(k, _)| *k != key);
    }
}

/// Every registered target, in `targets::ALL` order.
pub fn all_specs() -> &'static [&'static TargetSpec] {
    static ALL: OnceLock<Vec<&'static TargetSpec>> = OnceLock::new();
    ALL.get_or_init(|| crate::targets::ALL.iter().flat_map(|g| g.iter().copied()).collect())
}

fn registry<'a>(g: &'a mut Guard<'_, Option<Registry>>) -> &'a mut Registry {
    g.get_or_insert_with(|| Registry {
        rt: all_specs().iter().map(|_| Rt::new()).collect(),
        groups: Vec::new(),
        pass: 0,
        only: None,
        exclude: Vec::new(),
    })
}

// ---------------------------------------------------------------------------------------
// Lazy activation bookkeeping (watcher side; plain Mutex, never held across Python code)
// ---------------------------------------------------------------------------------------

#[derive(Default)]
struct Lazy {
    /// Module names some target is waiting for.
    pending: Vec<&'static str>,
    /// Last module object popped from `sys.modules` per pending name (identity only).
    popped: Vec<(&'static str, usize)>,
    /// Modules that became ready while the registry was busy, or on a free-threaded build
    /// (strong references; whether the event was importlib's definitive re-insert).
    deferred: Vec<(&'static str, usize, bool)>,
}

static LAZY: Mutex<Lazy> = Mutex::new(Lazy { pending: Vec::new(), popped: Vec::new(), deferred: Vec::new() });
static PENDING_ANY: AtomicBool = AtomicBool::new(false);

fn lazy() -> std::sync::MutexGuard<'static, Lazy> {
    LAZY.lock().unwrap_or_else(|p| p.into_inner())
}

fn set_pending(names: Vec<&'static str>) {
    let mut l = lazy();
    PENDING_ANY.store(!names.is_empty(), Ordering::Relaxed);
    l.popped.retain(|(n, _)| names.contains(n));
    l.pending = names;
}

/// The `sys.modules` part of the shared dict watcher.
///
/// # Safety
/// Called from CPython's dict watcher callback (attached thread, inside a dict mutation:
/// no Python code, no blocking, no GIL release).
pub(crate) unsafe fn on_sys_modules_event(
    event: c_int,
    dict: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
    new_value: *mut ffi::PyObject,
) {
    if !PENDING_ANY.load(Ordering::Relaxed) || key.is_null() {
        return;
    }
    // SAFETY: attached thread; type checks and ASCII comparisons run no Python code.
    unsafe {
        if ffi::PyUnicode_CheckExact(key) == 0 {
            return;
        }
        // Module names of targets are ASCII: compare the key's PEP 393 data in place (no
        // allocation, no UTF-8 cache attached to the caller's string).
        if ffi::PyUnicode_KIND(key) != ffi::PyUnicode_1BYTE_KIND {
            return;
        }
        let len = ffi::PyUnicode_GET_LENGTH(key) as usize;
        let data = std::slice::from_raw_parts(ffi::PyUnicode_DATA(key).cast::<u8>(), len);
        let name = {
            let l = lazy();
            l.pending.iter().copied().find(|n| n.as_bytes() == data)
        };
        let Some(name) = name else { return };
        let saved = ffi::PyErr_GetRaisedException();
        match event {
            // DELETED: remember which module object leaves (importlib pops the module after
            // exec, then re-inserts it).
            2 => {
                let mut cur: *mut ffi::PyObject = std::ptr::null_mut();
                if ffi::PyDict_GetItemRef(dict, key, &mut cur) > 0 {
                    let mut l = lazy();
                    l.popped.retain(|(n, _)| *n != name);
                    l.popped.push((name, cur as usize));
                    drop(l);
                    ffi::Py_DECREF(cur);
                }
            }
            // ADDED / MODIFIED
            0 | 1 if !new_value.is_null() && ffi::PyModule_Check(new_value) != 0 => {
                let reinsert = {
                    let mut l = lazy();
                    let hit = l.popped.iter().any(|&(n, p)| n == name && p == new_value as usize);
                    l.popped.retain(|(n, _)| *n != name);
                    hit
                };
                let py = Python::assume_attached();
                let module = Bound::from_borrowed_ptr(py, new_value).cast_into_unchecked::<PyModule>();
                if reinsert || !prepare::module_initializing(&module) {
                    module_ready(py, name, module, reinsert);
                }
            }
            _ => {}
        }
        ffi::PyErr_SetRaisedException(saved);
    }
}

/// A pending module finished executing (`definitive`: importlib's post-exec re-insert;
/// otherwise a direct insert, where a refusal leaves the target pending).
fn module_ready(py: Python<'_>, name: &'static str, module: Bound<'_, PyModule>, definitive: bool) {
    if cfg!(Py_GIL_DISABLED) {
        // Never inside the watcher on free-threaded builds (module docs).
        lazy().deferred.push((name, module.into_ptr() as usize, definitive));
        schedule_pending();
        return;
    }
    match REG.try_lock() {
        Some(mut g) => {
            let reg = registry(&mut g);
            let ml = activate_module(py, reg, name, &module, definitive);
            let ml2 = drain_deferred(py, reg);
            drop(g);
            if ml || ml2 {
                clear_internal_caches();
            }
            after_unlock(py);
        }
        None => {
            // Busy: the holder drains this before or right after releasing the lock.
            lazy().deferred.push((name, module.into_ptr() as usize, definitive));
        }
    }
}

/// Install every pending target of module `name` from `module`. Returns whether an
/// `ml_meth` site was installed (JIT caches must then be cleared).
fn activate_module(
    py: Python<'_>,
    reg: &mut Registry,
    name: &str,
    module: &Bound<'_, PyModule>,
    definitive: bool,
) -> bool {
    prepare::clear_pass_caches();
    reg.pass += 1;
    let mut ml = false;
    for (i, spec) in all_specs().iter().enumerate() {
        if spec.module == Some(name) && reg.rt[i].state == State::Pending {
            ml |= activate(py, reg, i, Some(module.clone()), definitive);
        }
    }
    refresh_pending(reg);
    prepare::clear_pass_caches();
    ml
}

fn drain_deferred(py: Python<'_>, reg: &mut Registry) -> bool {
    let mut ml = false;
    loop {
        let batch: Vec<(&'static str, usize, bool)> = std::mem::take(&mut lazy().deferred);
        if batch.is_empty() {
            return ml;
        }
        for (name, ptr, definitive) in batch {
            // SAFETY: the strong reference taken when queued.
            let module =
                unsafe { Bound::from_owned_ptr(py, ptr as *mut ffi::PyObject).cast_into_unchecked::<PyModule>() };
            ml |= activate_module(py, reg, name, &module, definitive);
        }
    }
}

/// After releasing the lock: modules queued and resyncs requested meanwhile are handled by
/// whoever can take it.
fn after_unlock(py: Python<'_>) {
    guard::release_deferred();
    while !lazy().deferred.is_empty() || NEED_RESYNC.load(Ordering::Acquire) {
        let Some(mut g) = REG.try_lock() else { return };
        let reg = registry(&mut g);
        let mut ml = drain_deferred(py, reg);
        if NEED_RESYNC.swap(false, Ordering::AcqRel) {
            ml |= resync(py, reg, false).0;
        }
        drop(g);
        if ml {
            clear_internal_caches();
        }
    }
}

// ---------------------------------------------------------------------------------------
// Suspensions: deopt while unpinned, net benefit (vectorcall targets)
// ---------------------------------------------------------------------------------------
//
// A vectorcall site costs +180–260 Ir per fallback call (callers lose the inlined frame
// push). Two policies restore the original vectorcall (and specializer version) while a
// target is not paying off:
// - Net benefit (§5.6): the kernel's score reached its floor. The falling-back thread
//   suspends the target right away (`suspend_from_kernel`); resumed by the next `enable()`.
// - A pinned dependency changed (`mock.patch`, a shadowing global). On GIL builds the
//   target is suspended as soon as a kernel falls back for it (or a pending call runs) and
//   resumed when every pin re-arms (cheap pointer swaps). On free-threaded builds swaps stop
//   the world (~1 ms with busy threads), so nothing is swapped in steady state for a
//   dependency: the kernel stays installed, its deps guard routes calls to the original,
//   and those fallbacks debit the net-benefit score like any other, so a dependency that
//   stays changed ends in a net-benefit suspension (one swap); resumed by the next
//   `enable()`.
// Dict-watcher callbacks never swap; they set flags and schedule a pending call.

/// A target suspended for its dependencies more often than this stays uninstalled
/// ("unstable dependencies"; GIL builds).
const MAX_SUSPENSIONS: u32 = 16;

static SCHEDULED: AtomicBool = AtomicBool::new(false);
static NEED_RESYNC: AtomicBool = AtomicBool::new(false);

/// `_PyEval_AddPendingCall` (exported internal API, 3.14 and 3.15), resolved at the first
/// `enable()` on free-threaded builds: a pending call any thread of the interpreter can run
/// (`Py_AddPendingCall` targets the main thread only, which may be blocked in `join()`).
static ANY_THREAD_PENDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
type AddPendingCall = unsafe extern "C" fn(
    *mut ffi::PyInterpreterState,
    extern "C" fn(*mut std::ffi::c_void) -> c_int,
    *mut std::ffi::c_void,
    c_int,
) -> c_int;

fn resolve_pending_api() {
    if cfg!(Py_GIL_DISABLED) && ANY_THREAD_PENDING.load(Ordering::Relaxed) == 0 {
        let a = prepare::lookup_symbol(c"_PyEval_AddPendingCall");
        ANY_THREAD_PENDING.store(a, Ordering::Relaxed);
    }
}

/// Run `resync_pending` at the next eval-breaker check (of any thread on free-threaded
/// builds, of the main thread otherwise). Callable from any attached thread, including from
/// a dict watcher callback (no Python code; CPython's pending-call lock is held for a few
/// instructions).
fn schedule_pending() {
    if SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    let any = ANY_THREAD_PENDING.load(Ordering::Relaxed);
    // SAFETY: attached thread; the function has `_PyEval_AddPendingCall`'s signature (3.14,
    // 3.15: `(interp, func, arg, flags)`; flags 0 = any thread); `Py_AddPendingCall` may be
    // called from any thread.
    let r = unsafe {
        if any != 0 {
            let f = std::mem::transmute::<usize, AddPendingCall>(any);
            f(ffi::PyInterpreterState_Get(), resync_pending, std::ptr::null_mut(), 0)
        } else {
            ffi::Py_AddPendingCall(Some(resync_pending), std::ptr::null_mut())
        }
    };
    if r < 0 {
        SCHEDULED.store(false, Ordering::Release);
    }
}

/// Called by the dependency tables when a tracked flag flips (any thread, inside a dict
/// watcher callback: no blocking, no Python code).
pub(crate) fn schedule_resync() {
    NEED_RESYNC.store(true, Ordering::Release);
    schedule_pending();
}

extern "C" fn resync_pending(_arg: *mut std::ffi::c_void) -> c_int {
    SCHEDULED.store(false, Ordering::Release);
    // SAFETY: pending calls run on an attached thread of the main interpreter, between
    // bytecodes.
    let py = unsafe { Python::assume_attached() };
    // The registry may be held (even by this thread, while a `prepare` runs Python code):
    // then its holder picks the request up in `after_unlock`.
    after_unlock(py);
    0
}

/// A kernel's net-benefit score reached its floor: suspend on this thread now (the kernel
/// is attached, outside any watcher, and about to call the original) when the registry is
/// free; otherwise its holder or a pending call does. Only swaps happen here: nothing is
/// released (a `Py_DECREF` could run a finalizer inside the call).
pub(crate) fn suspend_from_kernel() {
    NEED_RESYNC.store(true, Ordering::Release);
    // SAFETY: kernels run attached.
    let py = unsafe { Python::assume_attached() };
    let Some(mut g) = REG.try_lock() else {
        schedule_pending();
        return;
    };
    NEED_RESYNC.store(false, Ordering::Release);
    let reg = registry(&mut g);
    let (ml, more) = resync(py, reg, true);
    drop(g);
    if ml {
        clear_internal_caches();
    }
    if more || !lazy().deferred.is_empty() {
        NEED_RESYNC.store(true, Ordering::Release);
        schedule_pending();
    }
}

/// A kernel fell back because a pinned dependency changed (GIL builds): perform the
/// pending dependency suspension on this thread.
#[inline]
pub(crate) fn deps_fallback() {
    if NEED_RESYNC.load(Ordering::Relaxed) {
        suspend_from_kernel();
    }
}

/// Restore a target's sites to the originals (kept installed: pins stay active).
fn suspend(rt: &mut Rt, why: Suspended) -> bool {
    for s in rt.sites.iter_mut().rev() {
        s.uninstall(true);
    }
    rt.suspended = Some(why);
    rt.sites.iter().any(|s| s.kind == Kind::MlMeth)
}

/// Reinstall a suspended target's sites. On failure (someone else changed an entry point
/// meanwhile) the target is refused.
fn resume(reg: &mut Registry, i: usize) -> bool {
    let rt = &mut reg.rt[i];
    let mut failed = None;
    for s in rt.sites.iter_mut() {
        if let Err(e) = s.install() {
            failed = Some(e.to_string());
            break;
        }
    }
    let mut ml = rt.sites.iter().any(|s| s.kind == Kind::MlMeth);
    rt.suspended = None;
    all_specs()[i].counters.rearm();
    if let Some(f) = failed {
        ml |= deactivate(reg, i);
        let rt = &mut reg.rt[i];
        rt.state = State::Refused;
        rt.reason = Some(format!("cannot resume: {f}"));
        rt.refusal = Some(Category::Conflict);
    }
    ml
}

/// Suspend or resume targets according to their pins and net-benefit scores. Returns
/// whether an `ml_meth` site changed, and (`from_kernel`: only swaps were allowed) whether
/// work remains for a later pass.
fn resync(_py: Python<'_>, reg: &mut Registry, from_kernel: bool) -> (bool, bool) {
    let mut ml = false;
    let mut more = false;
    for i in 0..reg.rt.len() {
        let spec = all_specs()[i];
        let rt = &mut reg.rt[i];
        if rt.state != State::Installed || rt.sites.is_empty() {
            continue;
        }
        let fallback = spec.counters.suspend_requested.swap(false, Ordering::AcqRel);
        if fallback && rt.suspended.is_none() {
            spec.counters.suspensions.fetch_add(1, Ordering::Relaxed);
            ml |= suspend(rt, Suspended::Fallback);
            continue;
        }
        if rt.tracked.is_empty() {
            continue;
        }
        let ok = rt.tracked.iter().all(|f| f.ok());
        if !ok && rt.suspended.is_none() {
            ml |= suspend(rt, Suspended::Deps);
            rt.suspensions += 1;
        }
        if rt.suspended == Some(Suspended::Deps) && rt.suspensions > MAX_SUSPENSIONS {
            if from_kernel {
                // Releasing pins may run finalizers: not inside a call.
                more = true;
                continue;
            }
            let n = rt.suspensions;
            ml |= deactivate(reg, i);
            let rt = &mut reg.rt[i];
            rt.state = State::Refused;
            rt.reason = Some(format!("unstable dependencies (suspended {n} times)"));
            rt.refusal = Some(Category::Deps);
        } else if ok && rt.suspended == Some(Suspended::Deps) {
            ml |= resume(reg, i);
        }
    }
    (ml, more)
}

fn refresh_pending(reg: &Registry) {
    let names: Vec<&'static str> = all_specs()
        .iter()
        .enumerate()
        .filter(|(i, _)| reg.rt[*i].state == State::Pending)
        .filter_map(|(_, s)| s.module)
        .collect();
    let mut names = names;
    names.sort_unstable();
    names.dedup();
    if names.is_empty() {
        guard::unwatch_sys_modules();
    }
    set_pending(names);
}

// ---------------------------------------------------------------------------------------
// Install / uninstall of one target
// ---------------------------------------------------------------------------------------

fn jit_enabled(py: Python<'_>) -> bool {
    // The JIT is chosen at startup (PYTHON_JIT) and cannot be switched on later.
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        // SAFETY: borrowed lookup in the sys module dict.
        let jit = unsafe { ffi::PySys_GetObject(c"_jit".as_ptr()) };
        if jit.is_null() {
            return false;
        }
        // SAFETY: live borrowed object.
        let jit = unsafe { Bound::from_borrowed_ptr(py, jit) };
        jit.call_method0("is_enabled").and_then(|r| r.is_truthy()).unwrap_or(false)
    })
}

static JIT: AtomicBool = AtomicBool::new(false);

/// The message of a caught panic.
fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_owned()
    }
}

/// Sites swapped and pins activated by an activation in progress: rolled back when dropped
/// uncommitted, so a refusal, an error or a panic anywhere leaves nothing behind.
struct Staged {
    sites: Vec<Site>,
    /// Sites declared but not (yet) swapped: released only.
    declared: Vec<Site>,
    owner: u64,
}

impl Staged {
    fn commit(mut self) -> (Vec<Site>, u64) {
        let sites = std::mem::take(&mut self.sites);
        let owner = std::mem::replace(&mut self.owner, 0);
        (sites, owner)
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        for mut s in self.sites.drain(..).rev() {
            s.uninstall(true);
            s.release();
        }
        for s in self.declared.drain(..) {
            s.release();
        }
        if self.owner != 0 {
            guard::deactivate(self.owner);
        }
    }
}

/// Make the shared declarations `g` active (prepared and pinned once); returns its index.
fn ensure_group(
    py: Python<'_>,
    reg: &mut Registry,
    g: &'static GroupSpec,
    spec: &'static TargetSpec,
    module: Option<Bound<'_, PyModule>>,
) -> Result<usize, Refusal> {
    let gi = match reg.groups.iter().position(|x| std::ptr::eq(x.spec, g)) {
        Some(gi) => gi,
        None => {
            reg.groups.push(Group { spec: g, owner: 0, users: 0, flags: Vec::new(), refused: None });
            reg.groups.len() - 1
        }
    };
    let pass = reg.pass;
    let st = &mut reg.groups[gi];
    if st.owner != 0 {
        return Ok(gi);
    }
    if let Some((p, r)) = &st.refused
        && *p == pass
    {
        return Err(r.clone());
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut cx = Prepare::new(py, spec, module, false);
        let r = match (g.prepare)(py, &mut cx) {
            Ok(()) if cx.sites.is_empty() => guard::activate(py, std::mem::take(&mut cx.pins))
                .map_err(|e| Refusal::with(Category::Deps, format!("shared {}: pins: {}", g.name, e.value(py)))),
            Ok(()) => Err(Refusal::new(format!("shared {}: declares sites", g.name))),
            Err(r) => Err(r),
        };
        release_sites(cx.sites.drain(..).map(|d| d.site));
        r
    }));
    let result = match outcome {
        Ok(r) => r,
        Err(p) => Err(Refusal::new(format!("shared {}: panic while preparing: {}", g.name, panic_message(&*p)))),
    };
    let st = &mut reg.groups[gi];
    match result {
        Ok((owner, flags)) => {
            st.owner = owner;
            st.flags = flags;
            st.refused = None;
            Ok(gi)
        }
        Err(r) => {
            st.refused = Some((pass, r.clone()));
            Err(r)
        }
    }
}

/// One user of group `gi` fewer; the last one releases its pins.
fn release_group(reg: &mut Registry, gi: usize) {
    let st = &mut reg.groups[gi];
    st.users = st.users.saturating_sub(1);
    if st.users == 0 && st.owner != 0 {
        guard::deactivate(st.owner);
        st.owner = 0;
        st.flags.clear();
    }
}

/// Gate, prepare and install target `i`. Returns whether an `ml_meth` site was installed.
/// Never panics out (`enable()`, the import hook): a panic anywhere becomes a refusal, and
/// whatever was swapped or pinned is rolled back.
fn activate(
    py: Python<'_>,
    reg: &mut Registry,
    i: usize,
    module: Option<Bound<'_, PyModule>>,
    definitive: bool,
) -> bool {
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| activate_inner(py, reg, i, module, definitive)));
    match outcome {
        Ok(ml) => ml,
        Err(p) => {
            // `Staged` rolled the sites and pins back while unwinding.
            let spec = all_specs()[i];
            if let Some(g) = spec.shared
                && let Some(gi) = reg.groups.iter().position(|x| std::ptr::eq(x.spec, g))
                && reg.groups[gi].users == 0
            {
                reg.groups[gi].users = 1;
                release_group(reg, gi);
            }
            let rt = &mut reg.rt[i];
            if definitive {
                rt.state = State::Refused;
            }
            rt.reason = Some(format!("panic while activating: {}", panic_message(&*p)));
            rt.refusal = Some(Category::Other);
            false
        }
    }
}

fn activate_inner(
    py: Python<'_>,
    reg: &mut Registry,
    i: usize,
    module: Option<Bound<'_, PyModule>>,
    definitive: bool,
) -> bool {
    let spec = all_specs()[i];
    let refuse = |reg: &mut Registry, r: Refusal| {
        let rt = &mut reg.rt[i];
        if definitive {
            rt.state = State::Refused;
        }
        rt.reason = Some(r.reason().to_owned());
        rt.refusal = Some(r.category());
        false
    };
    if let Err(r) = version::target_gate(spec.versions, spec.free_threaded) {
        return refuse(reg, r);
    }
    if !spec.jit_safe && JIT.load(Ordering::Relaxed) {
        return refuse(reg, Refusal::with(Category::Jit, "jit"));
    }
    let group = match spec.shared {
        None => None,
        Some(g) => match ensure_group(py, reg, g, spec, module.clone()) {
            Ok(gi) => Some(gi),
            Err(r) => return refuse(reg, r),
        },
    };
    // The group's pins are kept only while a user is installed: release them if this
    // target does not get there.
    let refuse = |reg: &mut Registry, r: Refusal| {
        if let Some(gi) = group
            && reg.groups[gi].users == 0
        {
            reg.groups[gi].users = 1;
            release_group(reg, gi);
        }
        refuse(reg, r)
    };
    let mut cx = Prepare::new(py, spec, module, false);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.prepare)(py, &mut cx)));
    // Labels are formatted by `status()` only (no `format!` per site at install).
    reg.rt[i].site_names = cx.sites.iter().map(|d| (d.site.name.clone(), d.site.kind)).collect();
    let mut staged = Staged {
        sites: Vec::with_capacity(cx.sites.len()),
        declared: cx.sites.drain(..).map(|d| d.site).collect(),
        owner: 0,
    };
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(r)) => return refuse(reg, r),
        Err(p) => return refuse(reg, Refusal::new(format!("prepare panicked: {}", panic_message(&*p)))),
    }
    if staged.declared.is_empty() {
        return refuse(reg, Refusal::new("declared no sites"));
    }
    // All-or-nothing: CAS each site from its validated original.
    let declared = std::mem::take(&mut staged.declared);
    let mut failure = None;
    let mut rest = declared.into_iter();
    for mut s in rest.by_ref() {
        match s.install() {
            Ok(()) => staged.sites.push(s),
            Err(e) => {
                failure = Some(format!("{}: {}", s.name, e.value(py)));
                s.release();
                break;
            }
        }
    }
    staged.declared.extend(rest);
    if let Some(f) = failure {
        return refuse(reg, Refusal::with(Category::Conflict, f));
    }
    match guard::activate(py, std::mem::take(&mut cx.pins)) {
        Ok((owner, flags)) => {
            staged.owner = owner;
            reg.rt[i].flags = flags;
        }
        Err(e) => return refuse(reg, Refusal::with(Category::Deps, format!("pins: {}", e.value(py)))),
    }
    let (installed, owner) = staged.commit();
    let ml = installed.iter().any(|s| s.kind == Kind::MlMeth);
    let has_vc = installed.iter().any(|s| s.kind == Kind::Vectorcall);
    let group_flags = match group {
        Some(gi) => {
            reg.groups[gi].users += 1;
            reg.groups[gi].flags.clone()
        }
        None => Vec::new(),
    };
    let rt = &mut reg.rt[i];
    rt.group = group;
    rt.sites = installed;
    rt.pin_owner = owner;
    rt.state = State::Installed;
    rt.reason = None;
    rt.refusal = None;
    rt.suspended = None;
    rt.suspensions = 0;
    spec.counters.arm(spec.hit_credit, spec.fallback_debit);
    // Deopt while unpinned: GIL builds only (see "Suspensions").
    if has_vc && !cfg!(Py_GIL_DISABLED) {
        rt.tracked = rt.flags.clone();
        for f in group_flags {
            if !rt.tracked.iter().any(|t| std::ptr::eq(*t, f)) {
                rt.tracked.push(f);
            }
        }
        guard::track(&rt.tracked, true);
        if !rt.tracked.iter().all(|f| f.ok()) {
            // A dependency already changed between prepare and install.
            NEED_RESYNC.store(true, Ordering::Release);
        }
    }
    ml
}

fn release_sites(sites: impl Iterator<Item = Site>) {
    for s in sites {
        s.release();
    }
}

fn site_label(name: &str, kind: Kind) -> String {
    let kind = match kind {
        Kind::MlMeth => "ml_meth",
        Kind::Slot => "slot",
        Kind::Vectorcall => "vectorcall",
    };
    format!("{name} ({kind})")
}

/// Restore target `i`'s sites and drop its pins. Returns whether an `ml_meth` site was
/// restored.
fn deactivate(reg: &mut Registry, i: usize) -> bool {
    let rt = &mut reg.rt[i];
    let was_installed = rt.state == State::Installed;
    let ml = rt.sites.iter().any(|s| s.kind == Kind::MlMeth);
    for mut s in rt.sites.drain(..).rev() {
        s.uninstall(true);
        s.release();
    }
    if !rt.tracked.is_empty() {
        guard::track(&rt.tracked, false);
        rt.tracked.clear();
    }
    if rt.pin_owner != 0 {
        guard::deactivate(rt.pin_owner);
        rt.pin_owner = 0;
    }
    rt.flags.clear();
    clear_degraded(all_specs()[i]);
    if let Some(gi) = rt.group.take() {
        release_group(reg, gi);
    }
    let rt = &mut reg.rt[i];
    rt.state = State::Disabled;
    rt.reason = None;
    rt.refusal = None;
    rt.suspended = None;
    if was_installed && let Some(f) = all_specs()[i].on_disable {
        // A panic in the hook must not leave the registry half-updated.
        let _ = std::panic::catch_unwind(f_with_py(f));
    }
    ml
}

/// `on_disable(py)` as an unwind-safe closure (deactivate runs attached).
fn f_with_py(f: fn(Python<'_>)) -> impl FnOnce() + std::panic::UnwindSafe {
    // SAFETY: the registry runs on an attached thread.
    move || f(unsafe { Python::assume_attached() })
}

/// `sys._clear_internal_caches()`: 3.15's tier 2 copies `ml_meth` into traces (M§4).
/// Only when the JIT is enabled: without it no executor (trace) exists, and the call
/// would also flush the type attribute cache for nothing. (The JIT cannot be switched on
/// after startup.)
fn clear_internal_caches() {
    if !JIT.load(Ordering::Relaxed) {
        return;
    }
    // SAFETY: attached thread; a C function from the sys module dict (no Python code).
    unsafe {
        let f = ffi::PySys_GetObject(c"_clear_internal_caches".as_ptr());
        if f.is_null() {
            return;
        }
        let r = ffi::PyObject_CallNoArgs(f);
        if r.is_null() {
            ffi::PyErr_Clear();
        } else {
            ffi::Py_DECREF(r);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------------------

fn matches(spec: &TargetSpec, names: &[String]) -> bool {
    names.iter().any(|n| n == spec.name || n == spec.group)
}

fn selected(spec: &TargetSpec, only: Option<&[String]>, exclude: &[String]) -> bool {
    let inc = match only {
        Some(o) => matches(spec, o),
        None => !spec.hidden(),
    };
    inc && !matches(spec, exclude)
}

pub(crate) fn check_names(names: &[String]) -> PyResult<()> {
    let unknown: Vec<&String> =
        names.iter().filter(|n| !all_specs().iter().any(|s| s.name == n.as_str() || s.group == n.as_str())).collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let mut groups: Vec<&str> = all_specs().iter().filter(|s| !s.hidden()).map(|s| s.group).collect();
    groups.sort_unstable();
    groups.dedup();
    Err(PyValueError::new_err(format!(
        "unknown rustique target or group: {}; groups: {}",
        unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
        groups.join(", ")
    )))
}

// ---------------------------------------------------------------------------------------
// Public operations
// ---------------------------------------------------------------------------------------

/// Install the selected targets (additive and idempotent: installed and pending targets
/// stay; refused or disabled ones are retried). Module targets whose module is not
/// imported yet become pending. Never imports anything.
pub fn enable(py: Python<'_>, only: Option<Vec<String>>, exclude: Vec<String>) -> PyResult<()> {
    if let Some(o) = &only {
        check_names(o)?;
    }
    check_names(&exclude)?;
    let mut g = REG.lock(py)?;
    let reg = registry(&mut g);
    prepare::clear_pass_caches();
    reg.pass += 1;
    JIT.store(jit_enabled(py), Ordering::Relaxed);
    resolve_pending_api();
    let sys_modules = prepare::sys_modules(py).map_err(|r| PyRuntimeError::new_err(r.reason().to_owned()))?;
    let mut ml = drain_deferred(py, reg);
    // Module targets become pending first and `sys.modules` is watched before it is read,
    // so a module finishing its import on another thread meanwhile is either seen here or
    // queued by the watcher (drained below).
    let mut check: Vec<usize> = Vec::new();
    for (i, spec) in all_specs().iter().enumerate() {
        if !selected(spec, only.as_deref(), &exclude) {
            continue;
        }
        if reg.rt[i].suspended == Some(Suspended::Fallback) {
            ml |= resume(reg, i);
            continue;
        }
        if matches!(reg.rt[i].state, State::Installed | State::Pending) {
            continue;
        }
        match spec.module {
            None => ml |= activate(py, reg, i, None, true),
            Some(_) => {
                reg.rt[i].state = State::Pending;
                reg.rt[i].reason = None;
                reg.rt[i].refusal = None;
                check.push(i);
            }
        }
    }
    if !check.is_empty() {
        refresh_pending(reg);
        guard::watch_sys_modules(py, sys_modules.as_any())?;
    }
    for i in check {
        let Some(name) = all_specs()[i].module else { continue };
        if reg.rt[i].state != State::Pending {
            continue;
        }
        let m = sys_modules.get_item(prepare::interned(py, name))?.and_then(|m| m.cast_into::<PyModule>().ok());
        if let Some(m) = m
            && !prepare::module_initializing(&m)
        {
            ml |= activate(py, reg, i, Some(m), true);
        }
    }
    reg.only = only;
    reg.exclude = exclude;
    refresh_pending(reg);
    ml |= drain_deferred(py, reg);
    prepare::clear_pass_caches();
    drop(g);
    if ml {
        clear_internal_caches();
    }
    after_unlock(py);
    // Release everything before finalization (registered once, outside the lock).
    super::exit::ensure_hook(py);
    Ok(())
}

/// Restore every site of the matching targets (all when `names` is `None`) and forget
/// pending ones.
pub fn disable(py: Python<'_>, names: Option<Vec<String>>) -> PyResult<()> {
    if let Some(n) = &names {
        check_names(n)?;
    }
    let mut g = REG.lock(py)?;
    JIT.store(jit_enabled(py), Ordering::Relaxed);
    let reg = registry(&mut g);
    let mut ml = drain_deferred(py, reg);
    let mut any = false;
    for (i, spec) in all_specs().iter().enumerate() {
        if let Some(n) = &names
            && !matches(spec, n)
        {
            continue;
        }
        any |= reg.rt[i].state == State::Installed;
        ml |= deactivate(reg, i);
    }
    if names.is_none() {
        reg.only = None;
        reg.exclude = Vec::new();
    }
    refresh_pending(reg);
    drop(g);
    if ml || (any && names.is_none()) {
        clear_internal_caches();
    }
    after_unlock(py);
    Ok(())
}

/// Interpreter exit (the atexit hook, `exit::exit_hook`): restore every site, drop every
/// pin, watch and cached reference, clear every dependency flag, and call the targets'
/// `release` hooks. Finalization then neither keeps module objects alive through Rustique
/// nor pays dict-watcher events while it empties module dicts. A later `enable()` works
/// again (and registers the hook again).
pub fn teardown(py: Python<'_>) -> PyResult<()> {
    let mut g = REG.lock(py)?;
    JIT.store(jit_enabled(py), Ordering::Relaxed);
    let reg = registry(&mut g);
    // Modules queued by the watcher meanwhile: dropped, not activated.
    let deferred = std::mem::take(&mut lazy().deferred);
    let mut ml = false;
    // Pins are dropped all at once by `guard::teardown` below.
    guard::begin_teardown();
    for i in 0..reg.rt.len() {
        ml |= deactivate(reg, i);
    }
    reg.only = None;
    reg.exclude = Vec::new();
    refresh_pending(reg);
    NEED_RESYNC.store(false, Ordering::Release);
    guard::unwatch_sys_modules();
    // Under the registry lock, like `disable()`: no activation can add pins meanwhile.
    guard::teardown();
    prepare::teardown();
    drop(g);
    // SAFETY: attached thread; the strong references taken when the modules were queued.
    unsafe {
        for (_, p, _) in deferred {
            ffi::Py_DECREF(p as *mut ffi::PyObject);
        }
    }
    guard::release_deferred();
    let mut seen: Vec<usize> = Vec::new();
    for spec in all_specs() {
        if let Some(f) = spec.release
            && !seen.contains(&(f as usize))
        {
            seen.push(f as usize);
            f(py);
        }
    }
    if ml {
        clear_internal_caches();
    }
    Ok(())
}

/// Names of the installed targets among `names` (groups or target names; all if `None`).
pub fn installed(py: Python<'_>, names: Option<&[String]>) -> PyResult<Vec<&'static str>> {
    let mut g = REG.lock(py)?;
    let reg = registry(&mut g);
    let ml = drain_deferred(py, reg);
    let out = all_specs()
        .iter()
        .enumerate()
        .filter(|(i, s)| reg.rt[*i].state == State::Installed && names.is_none_or(|n| matches(s, n)))
        .map(|(_, s)| s.name)
        .collect();
    drop(g);
    if ml {
        clear_internal_caches();
    }
    after_unlock(py);
    Ok(out)
}

/// Per-target immutable status values (name, group, module, versions), built once.
struct StaticStatus {
    name: Py<pyo3::types::PyString>,
    group: Py<pyo3::types::PyString>,
    module: Option<Py<pyo3::types::PyString>>,
    versions: Vec<Py<pyo3::types::PyString>>,
}

fn static_status(py: Python<'_>) -> &'static [StaticStatus] {
    static CELL: pyo3::sync::PyOnceLock<Vec<StaticStatus>> = pyo3::sync::PyOnceLock::new();
    CELL.get_or_init(py, || {
        // Groups, modules and version strings repeat across targets: one str object each
        // (this runs inside the public `enable()`, which returns the status; startup cell).
        let mut shared: Vec<(String, Py<pyo3::types::PyString>)> = Vec::new();
        let mut text = |t: &str| -> Py<pyo3::types::PyString> {
            if let Some((_, o)) = shared.iter().find(|(k, _)| k == t) {
                return o.clone_ref(py);
            }
            let o = pyo3::types::PyString::new(py, t).unbind();
            shared.push((t.to_owned(), o.clone_ref(py)));
            o
        };
        all_specs()
            .iter()
            .map(|s| StaticStatus {
                name: pyo3::types::PyString::new(py, s.name).unbind(),
                group: text(s.group),
                module: s.module.map(&mut text),
                versions: versions_of(s).iter().map(|v| text(v)).collect(),
            })
            .collect()
    })
}

fn versions_of(spec: &TargetSpec) -> Vec<String> {
    let mut v = Vec::new();
    for (a, b) in spec.versions {
        v.push(format!("{a}.{b}"));
        if spec.free_threaded {
            v.push(format!("{a}.{b}t"));
        }
    }
    v
}

/// `{"targets": [...], "process": {...}}` (docs/ACCELERATION.md §6.2). The Python layer
/// adds the native tier and the startup switch source.
pub fn status(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let mut g = REG.lock(py)?;
    let reg = registry(&mut g);
    // Modules queued by the watcher are activated first (free-threaded builds always queue).
    if drain_deferred(py, reg) {
        clear_internal_caches();
    }
    let targets = PyList::empty(py);
    let mut enabled = false;
    for (i, spec) in all_specs().iter().enumerate() {
        let rt = &reg.rt[i];
        if spec.hidden() && rt.state == State::Disabled && rt.reason.is_none() {
            continue;
        }
        enabled |= matches!(rt.state, State::Installed | State::Pending);
        let d = PyDict::new(py);
        let fixed = &static_status(py)[i];
        d.set_item(intern!(py, "name"), fixed.name.bind(py))?;
        d.set_item(intern!(py, "group"), fixed.group.bind(py))?;
        d.set_item(intern!(py, "state"), rt.state.as_str())?;
        d.set_item(intern!(py, "reason"), rt.reason.as_deref())?;
        d.set_item(
            intern!(py, "refusal"),
            if rt.state == State::Refused { rt.refusal.map(Category::as_str) } else { None },
        )?;
        d.set_item(intern!(py, "sites"), PyList::new(py, rt.site_names.iter().map(|(n, k)| site_label(n, *k)))?)?;
        d.set_item(intern!(py, "versions"), PyList::new(py, fixed.versions.iter().map(|v| v.bind(py)))?)?;
        d.set_item(intern!(py, "checkable"), spec.checkable)?;
        if let Some(m) = &fixed.module {
            d.set_item(intern!(py, "module"), m.bind(py))?;
        }
        if rt.pin_owner != 0 || rt.group.is_some() {
            let group_ok = rt.group.is_none_or(|gi| reg.groups[gi].flags.iter().all(|f| f.ok()));
            let ok = group_ok && rt.flags.iter().all(|f| f.ok());
            d.set_item(intern!(py, "deps_intact"), ok)?;
        }
        if let Some(gi) = rt.group {
            d.set_item(intern!(py, "shared"), reg.groups[gi].spec.name)?;
        }
        if rt.state == State::Installed
            && let Some(why) = degraded_reason(spec)
        {
            d.set_item(intern!(py, "degraded"), why)?;
        }
        if let Some(why) = rt.suspended {
            d.set_item(intern!(py, "suspended"), why.as_str())?;
            d.set_item(
                "reason",
                match why {
                    Suspended::Deps => "a pinned dependency changed: original restored until it is back",
                    Suspended::Fallback => "persistent fallback: original restored until the next enable()",
                },
            )?;
        }
        targets.append(d)?;
    }
    let process = PyDict::new(py);
    #[allow(clippy::useless_conversion)] // `unsigned long`: 32-bit on Windows
    // SAFETY: immutable after startup.
    let hv = u64::from(unsafe { ffi::Py_Version });
    process.set_item("python", format!("{}.{}.{}", (hv >> 24) & 0xff, (hv >> 16) & 0xff, (hv >> 8) & 0xff))?;
    process.set_item("hexversion", hv)?;
    process.set_item("build", if version::FREE_THREADED { "free-threaded" } else { "default" })?;
    process.set_item("jit", jit_enabled(py))?;
    process.set_item("rustique", env!("CARGO_PKG_VERSION"))?;
    process.set_item("interpreter", version::process_gate().err().map(|r| r.reason().to_owned()))?;
    process.set_item("enabled", enabled)?;
    process.set_item("stats", stats::enabled())?;
    process.set_item("check", stats::check_mode().map(|r| if r { "raise" } else { "warn" }))?;
    process.set_item("only", reg.only.clone())?;
    process.set_item("exclude", reg.exclude.clone())?;
    let out = PyDict::new(py);
    out.set_item("targets", targets)?;
    out.set_item("process", process)?;
    drop(g);
    after_unlock(py);
    Ok(out)
}

/// Counters of every target that counted something or is installed:
/// `{name: {"hits": n, "fallback": {reason: n}}}`. `enabled` switches counting; `reset`
/// zeroes the counters after the snapshot.
pub fn stats(py: Python<'_>, enabled: Option<bool>, reset: bool) -> PyResult<Bound<'_, PyDict>> {
    if let Some(on) = enabled {
        stats::set_enabled(on);
    }
    let out = PyDict::new(py);
    for spec in all_specs() {
        let (hits, fb) = spec.counters.snapshot();
        if spec.hidden() && hits == 0 && fb.iter().all(|(_, n)| *n == 0) {
            continue;
        }
        let d = PyDict::new(py);
        d.set_item("hits", hits)?;
        let f = PyDict::new(py);
        for (k, v) in fb {
            f.set_item(k, v)?;
        }
        d.set_item("fallback", f)?;
        d.set_item("suspensions", spec.counters.suspensions.load(Ordering::Relaxed))?;
        out.set_item(spec.name, d)?;
    }
    if reset {
        for spec in all_specs() {
            spec.counters.reset();
        }
    }
    Ok(out)
}

/// One row of [`fingerprint_report`]: (target, what, observed, expected).
pub type FingerprintRow = (String, String, String, Option<String>);

/// Dev tool: run `prepare` of the selected targets in record mode (nothing installed) and
/// return every fingerprint they check: `[(target, what, observed, expected|None)]`.
/// Module targets need their module imported (the dev tool imports them).
pub fn fingerprint_report(py: Python<'_>, only: Option<Vec<String>>) -> PyResult<Vec<FingerprintRow>> {
    let sys_modules = prepare::sys_modules(py).map_err(|r| PyRuntimeError::new_err(r.reason().to_owned()))?;
    version::calibrate_monitoring(py);
    let _g = REG.lock(py)?;
    let mut out = Vec::new();
    for spec in all_specs() {
        let sel = match &only {
            Some(o) => matches(spec, o),
            None => !spec.hidden(),
        };
        if !sel {
            continue;
        }
        let module = match spec.module {
            None => None,
            Some(m) => match sys_modules.get_item(m)?.and_then(|m| m.cast_into::<PyModule>().ok()) {
                Some(m) => Some(m),
                None => {
                    out.push((spec.name.to_owned(), format!("module {m} not imported"), String::new(), None));
                    continue;
                }
            },
        };
        let mut cx = Prepare::new(py, spec, module, true);
        let r = (spec.prepare)(py, &mut cx);
        let recs: Vec<FingerprintRecord> = std::mem::take(&mut cx.records);
        release_sites(cx.sites.into_iter().map(|d| d.site));
        if let Err(e) = r {
            out.push((spec.name.to_owned(), format!("prepare refused: {e}"), String::new(), None));
        }
        for rec in recs {
            out.push((spec.name.to_owned(), rec.what, rec.observed, rec.expected.map(str::to_owned)));
        }
    }
    Ok(out)
}

/// Pins of an installed target (diagnostics/tests): `[(key, absent, intact, broken)]`.
pub fn pins(py: Python<'_>, name: &str) -> PyResult<Vec<(String, bool, bool, bool)>> {
    let mut g = REG.lock(py)?;
    let reg = registry(&mut g);
    let i = all_specs()
        .iter()
        .position(|s| s.name == name)
        .ok_or_else(|| PyValueError::new_err(format!("unknown target {name}")))?;
    let owner = reg.rt[i].pin_owner;
    Ok(if owner == 0 { Vec::new() } else { guard::describe(py, owner) })
}
