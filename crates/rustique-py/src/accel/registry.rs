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
use super::spec::{Category, GroupSpec, KNOWN_TIERS, Refusal, TargetSpec};
use super::stats;
use super::stubs;
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
    /// Sites hold first-call stubs; the target validates (prepare in full) at its first call.
    Armed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Disabled => "disabled",
            State::Pending => "pending-import",
            State::Installed => "installed",
            State::Refused => "refused",
            State::Armed => "armed",
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
    /// labels are formatted by `status()` only, once: `site_labels`).
    site_names: Vec<(String, Kind)>,
    site_labels: Option<Py<PyList>>,
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
    /// Armed: the stub indices of its cell sites, and its module (validation needs it).
    stubs: Vec<usize>,
    armed_module: Option<Py<PyModule>>,
}

impl Rt {
    const fn new() -> Self {
        Rt {
            state: State::Disabled,
            reason: None,
            refusal: None,
            sites: Vec::new(),
            site_names: Vec::new(),
            site_labels: None,
            pin_owner: 0,
            tracked: Vec::new(),
            suspended: None,
            suspensions: 0,
            group: None,
            flags: Vec::new(),
            stubs: Vec::new(),
            armed_module: None,
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
    /// Opt-in tiers requested (union of every `enable()` since the last full `disable()`).
    tiers: Vec<String>,
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
    /// The pass in which the group's prepare last ran in arming mode.
    armed: u64,
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
        tiers: Vec::new(),
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
            ml |= install(py, reg, i, Some(module.clone()), definitive);
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
    run_hooks(py);
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

/// Restore a target's sites to the originals (kept installed: pins stay active). Its
/// dynamic sites retire at their next call (`accel::dynamic`).
fn suspend(spec: &TargetSpec, rt: &mut Rt, why: Suspended) -> bool {
    spec.counters.set_live(false);
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
    all_specs()[i].counters.set_live(failed.is_none());
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
            ml |= suspend(spec, rt, Suspended::Fallback);
            continue;
        }
        if rt.tracked.is_empty() {
            continue;
        }
        let ok = rt.tracked.iter().all(|f| f.ok());
        if !ok && rt.suspended.is_none() {
            ml |= suspend(spec, rt, Suspended::Deps);
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
            reg.groups.push(Group { spec: g, owner: 0, users: 0, flags: Vec::new(), refused: None, armed: 0 });
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
    reg.rt[i].site_labels = None;
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
    if staged.declared.is_empty() && spec.on_install.is_none() {
        // (A target with only dynamic sites installs them from its `on_install` hook.)
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
    spec.counters.set_live(true);
    queue_hook(i);
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

// ---------------------------------------------------------------------------------------
// The on_install hook
// ---------------------------------------------------------------------------------------

/// Targets installed since the hooks last ran: their `on_install` runs outside the registry
/// lock and outside every watcher ([`run_hooks`]).
static HOOKS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn queue_hook(i: usize) {
    if all_specs()[i].on_install.is_some() {
        HOOKS.lock().unwrap_or_else(|p| p.into_inner()).push(i);
        // A lazy activation (possibly inside the `sys.modules` watcher): the pending call
        // runs the hook soon after; registry operations run it too.
        schedule_pending();
    }
}

/// Run queued `on_install` hooks of targets still installed (attached thread, registry
/// lock not held by this thread, not in a watcher). Left queued when this thread holds the
/// registry (a pending call during a `prepare`).
fn run_hooks(py: Python<'_>) {
    if HOLDING.get() {
        return;
    }
    loop {
        let batch = std::mem::take(&mut *HOOKS.lock().unwrap_or_else(|p| p.into_inner()));
        if batch.is_empty() {
            return;
        }
        for i in batch {
            let still = match REG.lock(py) {
                Ok(mut g) => registry(&mut g).rt[i].state == State::Installed,
                Err(_) => false,
            };
            if still && let Some(f) = all_specs()[i].on_install {
                let _ = std::panic::catch_unwind(f_with_py(f));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Validate on first call (docs/ACCELERATION.md §3.2)
// ---------------------------------------------------------------------------------------
//
// By default a target is *armed* at `enable()` or at its module's import: its `prepare` runs
// in arming mode (sites resolved and declared; fingerprints, self-tests, pins and layout
// calibrations skipped) and its sites get first-call stubs that forward to the originals.
// The first call of any of its sites validates it in full (`prepare` again, pins, the
// kernels swapped in by compare-and-swap, or a refusal that restores the originals) and is
// then dispatched to whatever the site holds. A program pays validation only for targets it
// calls. Free-threaded builds validate eagerly (a vectorcall swap at a first call would
// stop the world in steady state); targets with opcode-handler sites, or sites of an
// arity no stub covers, too. `RUSTIQUE_VALIDATE=eager` / `_accel._set_validation("eager")`
// validate everything at install (CI, the test-suite); `_accel.validate()` validates armed
// targets now.

static EAGER: AtomicBool = AtomicBool::new(false);
/// The policy was taken from `RUSTIQUE_VALIDATE` or set explicitly.
static POLICY: std::sync::Once = std::sync::Once::new();

/// Validate at install (`true`) or at first call (`false`, the default; always eager on
/// free-threaded builds).
pub fn set_eager(on: bool) {
    POLICY.call_once(|| {});
    store_eager(on);
}

/// [`set_eager`] without marking the policy explicit (the environment's value).
pub(crate) fn store_eager(on: bool) {
    EAGER.store(on, Ordering::Relaxed);
}

/// The first registry operation reads `RUSTIQUE_VALIDATE` (unless set explicitly before).
fn init_policy(py: Python<'_>) {
    POLICY.call_once(|| super::switch::apply_validate_env(py));
}

pub fn eager() -> bool {
    cfg!(Py_GIL_DISABLED) || EAGER.load(Ordering::Relaxed)
}

/// An armed cell site, as its stub sees it.
struct ArmedCell {
    cell: std::sync::atomic::AtomicUsize,
    /// Target index + 1 (0: free).
    target: std::sync::atomic::AtomicUsize,
    original: std::sync::atomic::AtomicUsize,
}

static CELLS: [ArmedCell; stubs::POOL * stubs::ARITIES] = [const {
    ArmedCell {
        cell: std::sync::atomic::AtomicUsize::new(0),
        target: std::sync::atomic::AtomicUsize::new(0),
        original: std::sync::atomic::AtomicUsize::new(0),
    }
}; stubs::POOL * stubs::ARITIES];

/// Free stub indices per arity (lazily filled).
static FREE: Mutex<Vec<Vec<usize>>> = Mutex::new(Vec::new());

fn alloc_stub(arity: u8) -> Option<usize> {
    let a = usize::from(arity);
    if !(1..=stubs::ARITIES).contains(&a) {
        return None;
    }
    let mut f = FREE.lock().unwrap_or_else(|p| p.into_inner());
    if f.is_empty() {
        *f = (0..stubs::ARITIES).map(|ar| (ar * stubs::POOL..(ar + 1) * stubs::POOL).rev().collect()).collect();
    }
    f[a - 1].pop()
}

fn free_stub(k: usize) {
    CELLS[k].target.store(0, Ordering::Release);
    FREE.lock().unwrap_or_else(|p| p.into_inner())[k / stubs::POOL].push(k);
}

/// Armed vectorcall sites: (function, target index).
static VC_ARMED: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// What stub `k` calls: the site's current entry point once it no longer holds the stub,
/// else (after trying to validate the target) the original.
pub(crate) fn cell_dispatch(k: usize) -> usize {
    let e = &CELLS[k];
    let cell = e.cell.load(Ordering::Acquire);
    let stub = stubs::addr(k);
    // SAFETY: the address of a validated, aligned pointer-sized entry point (kept alive
    // while its target is armed, and never freed for static tables).
    let current = || unsafe { (*(cell as *const std::sync::atomic::AtomicUsize)).load(Ordering::Acquire) };
    let cur = current();
    if cur != stub {
        return cur;
    }
    let t = e.target.load(Ordering::Acquire);
    if t != 0 {
        first_call(t - 1);
    }
    let cur = current();
    if cur == stub { e.original.load(Ordering::Acquire) } else { cur }
}

/// The vectorcall of armed Python functions.
///
/// # Safety
/// Called by CPython as a function's vectorcall.
pub(crate) unsafe extern "C" fn stub_vectorcall(
    f: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kw: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let target = VC_ARMED.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|e| e.0 == f as usize).map(|e| e.1);
    if let Some(i) = target {
        first_call(i);
    }
    // SAFETY: a live function object (the callee); its current vectorcall.
    unsafe {
        let vc = (*f.cast::<ffi::PyFunctionObject>()).vectorcall.map_or(0, |v| v as usize);
        let vc =
            if vc == stub_vectorcall as *const () as usize || vc == 0 { prepare::function_vectorcall() } else { vc };
        std::mem::transmute::<usize, ffi::vectorcallfunc>(vc)(f, args, nargsf, kw)
    }
}

/// Validate armed target `i` now, if the registry is free (else its call runs the
/// original and a later call tries again).
fn first_call(i: usize) {
    let Some(mut g) = REG.try_lock() else { return };
    // SAFETY: stubs run on attached threads.
    let py = unsafe { Python::assume_attached() };
    let reg = registry(&mut g);
    let mut ml = false;
    if reg.rt[i].state == State::Armed {
        prepare::clear_pass_caches();
        reg.pass += 1;
        ml = validate_armed(py, reg, i);
        prepare::clear_pass_caches();
    }
    drop(g);
    if ml {
        clear_internal_caches();
    }
}

/// Install target `i` by the current policy: armed (validated at first call) or validated
/// now.
fn install(
    py: Python<'_>,
    reg: &mut Registry,
    i: usize,
    module: Option<Bound<'_, PyModule>>,
    definitive: bool,
) -> bool {
    if eager() {
        return activate(py, reg, i, module, definitive);
    }
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| arm(py, reg, i, module.clone(), definitive)));
    match outcome {
        Ok(Some(ml)) => ml,
        // Not armable (or arming failed): validate now.
        Ok(None) | Err(_) => activate(py, reg, i, module, definitive),
    }
}

/// Arm target `i`: `Some(ml)` when armed or refused, `None` to validate it now instead.
fn arm(
    py: Python<'_>,
    reg: &mut Registry,
    i: usize,
    module: Option<Bound<'_, PyModule>>,
    definitive: bool,
) -> Option<bool> {
    let spec = all_specs()[i];
    if version::target_gate(spec.versions, spec.free_threaded).is_err()
        || (!spec.jit_safe && JIT.load(Ordering::Relaxed))
    {
        return None; // `activate` refuses with the reason
    }
    let refuse_now = |reg: &mut Registry, r: Refusal| {
        let rt = &mut reg.rt[i];
        if definitive {
            rt.state = State::Refused;
        }
        rt.reason = Some(r.reason().to_owned());
        rt.refusal = Some(r.category());
    };
    // Shared declarations run in arming mode too (once per pass): targets read what their
    // group's prepare resolved (classes, helpers).
    if let Some(g) = spec.shared
        && let Err(r) = arm_group(py, reg, g, spec, module.clone())
    {
        refuse_now(reg, r);
        return Some(false);
    }
    let mut cx = Prepare::new(py, spec, module.clone(), false);
    cx.arming = true;
    let outcome = (spec.prepare)(py, &mut cx);
    let decls = std::mem::take(&mut cx.sites);
    drop(cx);
    if let Err(r) = outcome {
        release_sites(decls.into_iter().map(|d| d.site));
        refuse_now(reg, r);
        return Some(false);
    }
    if decls.is_empty() || decls.iter().any(|d| d.site.kind.is_cell() && d.arity == 0) {
        release_sites(decls.into_iter().map(|d| d.site));
        return None;
    }
    reg.rt[i].site_names = decls.iter().map(|d| (d.site.name.clone(), d.site.kind)).collect();
    reg.rt[i].site_labels = None;
    // Stubs for every site, then swap them in all-or-nothing.
    let mut stubs_used: Vec<usize> = Vec::new();
    let mut sites: Vec<Site> = Vec::with_capacity(decls.len());
    let mut ok = true;
    for d in decls {
        let mut site = d.site;
        if !ok {
            site.release();
            continue;
        }
        if site.kind.is_cell() {
            match alloc_stub(d.arity) {
                Some(k) => {
                    CELLS[k].cell.store(site.addr, Ordering::Release);
                    CELLS[k].original.store(site.original, Ordering::Release);
                    CELLS[k].target.store(i + 1, Ordering::Release);
                    site.kernel = stubs::addr(k);
                    stubs_used.push(k);
                }
                None => {
                    ok = false;
                    site.release();
                    continue;
                }
            }
        } else {
            site.kernel = stub_vectorcall as *const () as usize;
            VC_ARMED.lock().unwrap_or_else(|p| p.into_inner()).push((site.addr, i));
        }
        match site.install() {
            Ok(()) => sites.push(site),
            Err(_) => {
                ok = false;
                site.release();
            }
        }
    }
    if !ok {
        for mut s in sites.into_iter().rev() {
            s.uninstall(true);
            s.release();
        }
        for k in stubs_used {
            free_stub(k);
        }
        VC_ARMED.lock().unwrap_or_else(|p| p.into_inner()).retain(|e| e.1 != i);
        return None;
    }
    let ml = sites.iter().any(|s| s.kind == Kind::MlMeth);
    let rt = &mut reg.rt[i];
    rt.sites = sites;
    rt.stubs = stubs_used;
    rt.armed_module = module.map(Bound::unbind);
    rt.state = State::Armed;
    rt.reason = None;
    rt.refusal = None;
    Some(ml)
}

/// Run group `g`'s prepare in arming mode (no pins, no validation), once per registry pass,
/// unless it is already active in full.
fn arm_group(
    py: Python<'_>,
    reg: &mut Registry,
    g: &'static GroupSpec,
    spec: &'static TargetSpec,
    module: Option<Bound<'_, PyModule>>,
) -> Result<(), Refusal> {
    let gi = match reg.groups.iter().position(|x| std::ptr::eq(x.spec, g)) {
        Some(gi) => gi,
        None => {
            reg.groups.push(Group { spec: g, owner: 0, users: 0, flags: Vec::new(), refused: None, armed: 0 });
            reg.groups.len() - 1
        }
    };
    let pass = reg.pass;
    let st = &reg.groups[gi];
    if st.owner != 0 || st.armed == pass {
        return Ok(());
    }
    if let Some((p, r)) = &st.refused
        && *p == pass
    {
        return Err(r.clone());
    }
    let mut cx = Prepare::new(py, spec, module, false);
    cx.arming = true;
    let r = (g.prepare)(py, &mut cx);
    release_sites(cx.sites.drain(..).map(|d| d.site));
    drop(cx);
    let st = &mut reg.groups[gi];
    match r {
        Ok(()) => {
            st.armed = pass;
            Ok(())
        }
        Err(r) => {
            st.refused = Some((pass, r.clone()));
            Err(r)
        }
    }
}

/// Put the originals back over an armed target's stubs.
fn disarm(reg: &mut Registry, i: usize) -> bool {
    let rt = &mut reg.rt[i];
    let ml = rt.sites.iter().any(|s| s.kind == Kind::MlMeth);
    for mut s in rt.sites.drain(..).rev() {
        s.uninstall(true);
        s.release();
    }
    for k in rt.stubs.drain(..) {
        free_stub(k);
    }
    rt.armed_module = None;
    VC_ARMED.lock().unwrap_or_else(|p| p.into_inner()).retain(|e| e.1 != i);
    ml
}

/// Validate armed target `i` in full: the originals come back over the stubs, then the
/// normal activation (prepare, pins, compare-and-swap of the kernels) runs.
fn validate_armed(py: Python<'_>, reg: &mut Registry, i: usize) -> bool {
    let module = reg.rt[i].armed_module.take().map(|m| m.into_bound(py));
    let ml = disarm(reg, i);
    reg.rt[i].state = State::Disabled;
    ml | activate(py, reg, i, module, true)
}

/// `_accel.validate(names)`: validate every armed target among `names` (all if `None`) now.
pub fn validate(py: Python<'_>, names: Option<Vec<String>>) -> PyResult<()> {
    if let Some(n) = &names {
        check_names(n)?;
    }
    let mut g = REG.lock(py)?;
    let reg = registry(&mut g);
    let mut ml = drain_deferred(py, reg);
    prepare::clear_pass_caches();
    reg.pass += 1;
    for (i, spec) in all_specs().iter().enumerate() {
        if reg.rt[i].state == State::Armed && names.as_deref().is_none_or(|n| matches(spec, n)) {
            ml |= validate_armed(py, reg, i);
        }
    }
    prepare::clear_pass_caches();
    drop(g);
    if ml {
        clear_internal_caches();
    }
    after_unlock(py);
    run_hooks(py);
    Ok(())
}

fn release_sites(sites: impl Iterator<Item = Site>) {
    for s in sites {
        s.release();
    }
}

fn site_label(name: &str, kind: Kind) -> String {
    format!("{name} ({})", kind.label())
}

/// Restore target `i`'s sites and drop its pins. Returns whether an `ml_meth` site was
/// restored.
fn deactivate(reg: &mut Registry, i: usize) -> bool {
    if reg.rt[i].state == State::Armed {
        let ml = disarm(reg, i);
        let rt = &mut reg.rt[i];
        rt.state = State::Disabled;
        rt.reason = None;
        rt.refusal = None;
        return ml;
    }
    let rt = &mut reg.rt[i];
    let was_installed = rt.state == State::Installed;
    all_specs()[i].counters.set_live(false);
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

/// `names` names this target, its group or its tier.
fn matches(spec: &TargetSpec, names: &[String]) -> bool {
    names.iter().any(|n| n == spec.name || n == spec.group || spec.opt_in == Some(n.as_str()))
}

fn selected(spec: &TargetSpec, only: Option<&[String]>, exclude: &[String], tiers: &[String]) -> bool {
    if let Some(t) = spec.opt_in
        && !tiers.iter().any(|x| x == t)
    {
        return false;
    }
    let inc = match only {
        Some(o) => matches(spec, o),
        None => !spec.hidden(),
    };
    inc && !matches(spec, exclude)
}

/// A known name: target, group or tier (including tiers with no target in this build).
pub(crate) fn known_name(n: &str) -> bool {
    all_specs().iter().any(|s| s.name == n || s.group == n || s.opt_in == Some(n)) || KNOWN_TIERS.contains(&n)
}

/// Every tier name known to this build (sorted).
pub(crate) fn tier_names() -> Vec<&'static str> {
    let mut t: Vec<&'static str> = KNOWN_TIERS.to_vec();
    t.extend(all_specs().iter().filter_map(|s| s.opt_in));
    t.sort_unstable();
    t.dedup();
    t
}

pub(crate) fn check_tiers(names: &[String]) -> PyResult<()> {
    let known = tier_names();
    let unknown: Vec<&str> = names.iter().map(String::as_str).filter(|n| !known.contains(n)).collect();
    if unknown.is_empty() {
        return Ok(());
    }
    Err(PyValueError::new_err(format!("unknown rustique tier: {}; tiers: {}", unknown.join(", "), known.join(", "))))
}

pub(crate) fn check_names(names: &[String]) -> PyResult<()> {
    let unknown: Vec<&String> = names.iter().filter(|n| !known_name(n)).collect();
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
    enable_tiers(py, only, exclude, Vec::new())
}

/// [`enable`] with opt-in tiers requested too (added to the ones already requested).
pub fn enable_tiers(
    py: Python<'_>,
    only: Option<Vec<String>>,
    exclude: Vec<String>,
    tiers: Vec<String>,
) -> PyResult<()> {
    if let Some(o) = &only {
        check_names(o)?;
    }
    check_names(&exclude)?;
    check_tiers(&tiers)?;
    init_policy(py);
    let mut g = REG.lock(py)?;
    let reg = registry(&mut g);
    for t in tiers {
        if !reg.tiers.contains(&t) {
            reg.tiers.push(t);
        }
    }
    let tiers = reg.tiers.clone();
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
        if !selected(spec, only.as_deref(), &exclude, &tiers) {
            continue;
        }
        if reg.rt[i].suspended == Some(Suspended::Fallback) {
            ml |= resume(reg, i);
            continue;
        }
        if reg.rt[i].state == State::Installed && reg.rt[i].sites.iter().any(Site::displaced) {
            // An entry point was rewritten over the kernel: validate afresh.
            ml |= deactivate(reg, i);
        }
        if reg.rt[i].state == State::Armed && eager() {
            ml |= validate_armed(py, reg, i);
            continue;
        }
        if matches!(reg.rt[i].state, State::Installed | State::Pending | State::Armed) {
            continue;
        }
        match spec.module {
            None => ml |= install(py, reg, i, None, true),
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
            ml |= install(py, reg, i, Some(m), true);
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
    run_hooks(py);
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
        reg.tiers.clear();
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
    reg.tiers.clear();
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

/// Names of the installed targets among `names` (groups or target names; all if `None`), armed
/// ones included (their sites are swapped; they validate at their first call).
pub fn installed(py: Python<'_>, names: Option<&[String]>) -> PyResult<Vec<&'static str>> {
    let mut g = REG.lock(py)?;
    let reg = registry(&mut g);
    let ml = drain_deferred(py, reg);
    let out = all_specs()
        .iter()
        .enumerate()
        .filter(|(i, s)| {
            matches!(reg.rt[*i].state, State::Installed | State::Armed) && names.is_none_or(|n| matches(s, n))
        })
        .map(|(_, s)| s.name)
        .collect();
    drop(g);
    if ml {
        clear_internal_caches();
    }
    after_unlock(py);
    run_hooks(py);
    Ok(out)
}

/// Per-target immutable status values (name, group, tier, module, versions), built once
/// (strings shared across targets).
struct StaticStatus {
    name: Py<pyo3::types::PyString>,
    group: Py<pyo3::types::PyString>,
    tier: Option<Py<pyo3::types::PyString>>,
    module: Option<Py<pyo3::types::PyString>>,
    /// Shared by every target with the same versions: copied into each entry.
    versions: Py<PyList>,
}

/// Empty entries with every key in order (`name`, `group`, `state`, `reason`, `tier`,
/// `refusal`, `sites`, `versions`, `checkable`[, `module`]), values `None`: `status()` copies
/// one (a presized dict, keys cloned at once) and sets the values in place.
struct Templates {
    plain: Py<PyDict>,
    module: Py<PyDict>,
}

/// `"3.14"`, `"3.14t"`: version labels without `core::fmt`.
fn version_label(major: u8, minor: u8, t: bool) -> String {
    let mut s = major.to_string();
    s.push('.');
    s.push_str(&minor.to_string());
    if t {
        s.push('t');
    }
    s
}

fn templates(py: Python<'_>) -> PyResult<&'static Templates> {
    static CELL: pyo3::sync::PyOnceLock<Templates> = pyo3::sync::PyOnceLock::new();
    CELL.get_or_try_init(py, || {
        let make = |module: bool| -> PyResult<Py<PyDict>> {
            let d = PyDict::new(py);
            for k in ["name", "group", "state", "reason", "tier", "refusal", "sites", "versions", "checkable"] {
                d.set_item(prepare::interned(py, k), py.None())?;
            }
            if module {
                d.set_item(intern!(py, "module"), py.None())?;
            }
            Ok(d.unbind())
        };
        Ok(Templates { plain: make(false)?, module: make(true)? })
    })
}

fn static_status(py: Python<'_>) -> PyResult<&'static [StaticStatus]> {
    static CELL: pyo3::sync::PyOnceLock<Vec<StaticStatus>> = pyo3::sync::PyOnceLock::new();
    CELL.get_or_try_init(py, || {
        let mut strs: Vec<(&'static str, Py<pyo3::types::PyString>)> = Vec::new();
        let mut text = |t: &'static str| -> Py<pyo3::types::PyString> {
            if let Some((_, o)) = strs.iter().find(|(k, _)| *k == t) {
                return o.clone_ref(py);
            }
            let o = pyo3::types::PyString::new(py, t).unbind();
            strs.push((t, o.clone_ref(py)));
            o
        };
        let mut lists: Vec<((usize, bool), Py<PyList>)> = Vec::new();
        let mut out = Vec::with_capacity(all_specs().len());
        for spec in all_specs() {
            let key = (spec.versions.as_ptr() as usize, spec.free_threaded);
            let versions = match lists.iter().find(|(k, _)| *k == key) {
                Some((_, l)) => l.clone_ref(py),
                None => {
                    let mut v: Vec<String> = Vec::new();
                    for &(a, b) in spec.versions {
                        v.push(version_label(a, b, false));
                        if spec.free_threaded {
                            v.push(version_label(a, b, true));
                        }
                    }
                    let l = PyList::new(py, v)?.unbind();
                    lists.push((key, l.clone_ref(py)));
                    l
                }
            };
            out.push(StaticStatus {
                name: pyo3::types::PyString::new(py, spec.name).unbind(),
                group: text(spec.group),
                tier: spec.opt_in.map(&mut text),
                module: spec.module.map(&mut text),
                versions,
            });
        }
        Ok(out)
    })
    .map(Vec::as_slice)
}

/// A shallow copy of a list (each status entry owns its lists).
fn list_copy<'py>(l: &Bound<'py, PyList>) -> PyResult<Bound<'py, PyAny>> {
    // SAFETY: attached thread; a live list; a new reference or NULL with an exception.
    unsafe { Bound::from_owned_ptr_or_err(l.py(), ffi::PyList_GetSlice(l.as_ptr(), 0, ffi::PY_SSIZE_T_MAX)) }
}

fn state_str(py: Python<'_>, s: State) -> &Bound<'_, pyo3::types::PyString> {
    match s {
        State::Disabled => intern!(py, "disabled"),
        State::Pending => intern!(py, "pending-import"),
        State::Installed => intern!(py, "installed"),
        State::Refused => intern!(py, "refused"),
        State::Armed => intern!(py, "armed"),
    }
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
        let rt = &mut reg.rt[i];
        if spec.hidden() && rt.state == State::Disabled && rt.reason.is_none() {
            continue;
        }
        // A target of a tier nobody requested: disabled, saying how to turn it on.
        let tier_off = spec.opt_in.filter(|t| rt.state == State::Disabled && !reg.tiers.iter().any(|x| x == t));
        enabled |= matches!(rt.state, State::Installed | State::Pending | State::Armed);
        let fixed = &static_status(py)?[i];
        let t = templates(py)?;
        let template = if fixed.module.is_some() { &t.module } else { &t.plain };
        // SAFETY: attached thread; a live dict; a new reference or NULL with an exception.
        let d =
            unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyDict_Copy(template.as_ptr())) }?.cast_into::<PyDict>()?;
        d.set_item(intern!(py, "name"), fixed.name.bind(py))?;
        d.set_item(intern!(py, "group"), fixed.group.bind(py))?;
        d.set_item(intern!(py, "state"), state_str(py, rt.state))?;
        if let Some(t) = &fixed.tier {
            d.set_item(intern!(py, "tier"), t.bind(py))?;
        }
        d.set_item(intern!(py, "checkable"), spec.checkable)?;
        if let Some(m) = &fixed.module {
            d.set_item(intern!(py, "module"), m.bind(py))?;
        }
        match tier_off {
            Some(t) if rt.reason.is_none() => {
                d.set_item(intern!(py, "reason"), format!("opt-in tier: RUSTIQUE_TIERS={t}"))?
            }
            Some(_) | None => {
                if let Some(r) = rt.reason.as_deref() {
                    d.set_item(intern!(py, "reason"), r)?;
                }
            }
        }
        if rt.state == State::Refused
            && let Some(c) = rt.refusal
        {
            d.set_item(intern!(py, "refusal"), c.as_str())?;
        }
        if rt.site_labels.is_none() {
            rt.site_labels = Some(PyList::new(py, rt.site_names.iter().map(|(n, k)| site_label(n, *k)))?.unbind());
        }
        if let Some(l) = &rt.site_labels {
            d.set_item(intern!(py, "sites"), list_copy(l.bind(py))?)?;
        }
        d.set_item(intern!(py, "versions"), list_copy(fixed.versions.bind(py))?)?;
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
        if rt.state == State::Installed && rt.suspended.is_none() && rt.sites.iter().any(Site::displaced) {
            // CPython (or someone else) rewrote an entry point over the kernel.
            d.set_item(intern!(py, "suspended"), "conflict")?;
            d.set_item(
                "reason",
                "an entry point was rewritten over the kernel: the original runs until the next enable()",
            )?;
        }
        let dynamic = spec.counters.dynamic.load(Ordering::Relaxed);
        if dynamic != 0 {
            d.set_item(intern!(py, "dynamic_sites"), dynamic)?;
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
    process.set_item("tiers", reg.tiers.clone())?;
    process.set_item("validation", if eager() { "eager" } else { "first-call" })?;
    let out = PyDict::new(py);
    out.set_item("targets", targets)?;
    out.set_item("process", process)?;
    drop(g);
    after_unlock(py);
    run_hooks(py);
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
