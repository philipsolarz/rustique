//! Cheap per-call guards deciding whether a call is in a kernel's exact subset
//! (docs/ACCELERATION.md §4).
//!
//! - [`Tools`] / [`tools_quiet`]: is any `sys.monitoring` tool (that includes
//!   `sys.settrace`, `sys.setprofile`, cProfile, coverage's sysmon core) listening to
//!   `PY_START`, or listening locally on the original's code object (a debugger
//!   breakpoint)? Then a kernel replacing *Python* code runs the original, so tools see
//!   CPython's frames. 34 + ~4 Ir.
//! - [`in_main_interpreter`]: kernels on process-global sites that hold per-interpreter
//!   objects fall back in other interpreters (~30 Ir: design kernels not to need it).
//! - [`DepFlag`]: "nobody replaced a dependency" (`json.encoder.c_make_encoder`,
//!   `posixpath._get_sep`, `JSONEncoder.default`, a builtin shadowed by a module global,
//!   ...) as one relaxed load per call. Kept up to date by one dict watcher (module dicts,
//!   `builtins.__dict__`, class `__dict__`s; the same watcher drives lazy activation on
//!   `sys.modules`) and one type watcher (a class whose MRO changes breaks its attribute
//!   pins). A pin re-arms when the original state is put back (`mock.patch` exiting, a
//!   shadowing global deleted again).
//!
//! Pins are requested by `Prepare::pin*` and activated by the registry when the target
//! installs; they are removed on uninstall. Rustique registers one dict watcher id and one
//! type watcher id of its own; CPython's watchers (globals/builtins for the specializer,
//! type watcher 0 for the optimizer) use other ids and are unaffected.

use std::cell::Cell;
use std::ffi::c_int;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyType};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PyMonitoringState {
    pub active: u8,
    pub opaque: u8,
}

type DictWatchCallback =
    unsafe extern "C" fn(c_int, *mut ffi::PyObject, *mut ffi::PyObject, *mut ffi::PyObject) -> c_int;
type TypeWatchCallback = unsafe extern "C" fn(*mut ffi::PyTypeObject) -> c_int;

python_extern! {
    fn PyMonitoring_EnterScope(
        state_array: *mut PyMonitoringState,
        version: *mut u64,
        event_types: *const u8,
        length: ffi::Py_ssize_t,
    ) -> c_int;
    fn PyDict_AddWatcher(cb: DictWatchCallback) -> c_int;
    fn PyType_AddWatcher(cb: TypeWatchCallback) -> c_int;
    fn PyDict_ClearWatcher(watcher_id: c_int) -> c_int;
    fn PyType_ClearWatcher(watcher_id: c_int) -> c_int;
    fn PyType_Watch(watcher_id: c_int, ty: *mut ffi::PyObject) -> c_int;
    fn PyType_Unwatch(watcher_id: c_int, ty: *mut ffi::PyObject) -> c_int;
}

pub const PY_MONITORING_EVENT_PY_START: u8 = 0;

/// `_PY_MONITORING_UNGROUPED_EVENTS` (3.14 and 3.15): PY_START … STOP_ITERATION, RAISE,
/// EXCEPTION_HANDLED, PY_UNWIND, PY_THROW, RERAISE. The grouped events (C_RETURN, C_RAISE,
/// BRANCH) are set through CALL / BRANCH_LEFT+RIGHT, which are in this range.
pub const UNGROUPED_EVENTS: usize = 16;
const ALL_EVENTS: [u8; UNGROUPED_EVENTS] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Number of local events in `_Py_LocalMonitors.tools` (`_co_monitoring`'s first member):
/// `_PY_MONITORING_LOCAL_EVENTS` = 11 on 3.14, `_PY_MONITORING_UNGROUPED_EVENTS` = 16 on
/// 3.15 (RAISE, EXCEPTION_HANDLED, PY_UNWIND, PY_THROW, RERAISE became local events).
/// Calibrated on the first vectorcall site prepared (`version::calibrate_monitoring`).
pub const LOCAL_EVENTS: usize = if cfg!(Py_3_15) { 16 } else { 11 };

// ---------------------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------------------

/// Monitoring scope of one target: is any `sys.monitoring` tool listening, globally, to any
/// event a Python function's execution can produce (all ungrouped events: PY_START, LINE,
/// CALL, RAISE, ...; `sys.settrace`/`setprofile`, cProfile and coverage are tools too)?
///
/// One atomic word: `(monitoring version << 1) | busy`, published as a unit. The fast path
/// asks CPython for the interpreter's monitoring version (`PyMonitoring_EnterScope` with no
/// events and a stack-local version: it only writes our local) and compares; when the
/// version moved, the verdict is recomputed from a *private* state array filled by one
/// `EnterScope` call, so a thread never pairs a new version with another thread's
/// half-written states (docs/atlas/w2-threads.md §3.4 S3: possible on weakly ordered CPUs
/// with shared arrays). Race-free on every build; no `unsafe impl Sync`.
pub struct Tools {
    state: AtomicU64,
}

/// No verdict yet (no monitoring version is `2^63 - 1`).
const TOOLS_UNSET: u64 = u64::MAX;

impl Tools {
    pub const fn new() -> Self {
        Tools { state: AtomicU64::new(TOOLS_UNSET) }
    }

    /// True when no tool listens globally to any ungrouped event in this interpreter.
    ///
    /// # Safety
    /// Attached thread.
    #[inline(always)]
    pub unsafe fn quiet(&self) -> bool {
        let mut v: u64 = 0;
        // SAFETY: attached thread; no events: CPython writes only `v` (the interpreter's
        // monitoring version; left 0 when that is 0).
        unsafe { PyMonitoring_EnterScope(std::ptr::null_mut(), &mut v, std::ptr::null(), 0) };
        let s = self.state.load(Ordering::Relaxed);
        if s >> 1 == v && s != TOOLS_UNSET {
            return s & 1 == 0;
        }
        // SAFETY: per the caller.
        unsafe { self.recompute() }
    }

    #[cold]
    unsafe fn recompute(&self) -> bool {
        let mut states = [PyMonitoringState { active: 0, opaque: 0 }; UNGROUPED_EVENTS];
        // A version CPython never has: `EnterScope` fills every state and stores the
        // version it read them at.
        let mut v = TOOLS_UNSET;
        // SAFETY: attached thread; private arrays of the declared length.
        unsafe {
            PyMonitoring_EnterScope(states.as_mut_ptr(), &mut v, ALL_EVENTS.as_ptr(), UNGROUPED_EVENTS as ffi::Py_ssize_t)
        };
        let busy = states.iter().any(|s| s.active != 0);
        self.state.store((v << 1) | u64::from(busy), Ordering::Relaxed);
        !busy
    }
}

impl Default for Tools {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Tools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tools")
    }
}

/// No tool listens *locally* on this code object (`sys.monitoring.set_local_events`, what a
/// debugger breakpoint or a per-function profiler does). Reads `_co_monitoring` (NULL until a
/// tool ever instrumented the code) and its first member `local_monitors.tools[LOCAL_EVENTS]`
/// (internal layout: 11 events on 3.14, 16 on 3.15; calibrated with the first vectorcall site).
///
/// # Safety
/// `code` is a live code object; layout calibrated (`version::code_layout_ok_for`,
/// `version::calibrate_monitoring`).
#[inline(always)]
pub unsafe fn code_locally_quiet(code: *mut ffi::PyObject) -> bool {
    // SAFETY: per the caller.
    unsafe {
        let mon = (*code.cast::<crate::accel::version::RawCode>())._co_monitoring;
        if mon.is_null() {
            return true;
        }
        let lo = std::ptr::read_unaligned(mon.cast::<u64>());
        let hi = if LOCAL_EVENTS == 16 {
            std::ptr::read_unaligned(mon.add(8).cast::<u64>())
        } else {
            std::ptr::read_unaligned(mon.add(8).cast::<u16>()) as u64 | (*mon.add(10) as u64) << 16
        };
        lo == 0 && hi == 0
    }
}

/// The tools guard of a Python-function kernel: no global `PY_START` listener and no local
/// listener on `func`'s code. `func` is the callable the kernel received (the patched
/// function object).
///
/// # Safety
/// Attached thread; `func` is a live `PyFunctionObject` whose site was installed by the
/// registry (which calibrated the code layout).
#[inline(always)]
pub unsafe fn tools_quiet(tools: &Tools, func: *mut ffi::PyObject) -> bool {
    // SAFETY: per the caller.
    unsafe { tools.quiet() && code_locally_quiet((*func.cast::<ffi::PyFunctionObject>()).func_code) }
}

// ---------------------------------------------------------------------------------------
// Interpreter
// ---------------------------------------------------------------------------------------

static MAIN_INTERP: AtomicPtr<ffi::PyInterpreterState> = AtomicPtr::new(std::ptr::null_mut());

/// Record the interpreter that installs the kernels (the main interpreter: the extension
/// refuses to load anywhere else).
pub fn set_main_interpreter() {
    // SAFETY: attached thread.
    MAIN_INTERP.store(unsafe { ffi::PyInterpreterState_Get() }, Ordering::Relaxed);
}

/// Is the calling thread in the interpreter that installed the kernels? (~30 Ir.)
///
/// # Safety
/// Attached thread.
#[inline(always)]
pub unsafe fn in_main_interpreter() -> bool {
    // SAFETY: attached thread.
    unsafe { ffi::PyInterpreterState_Get() == MAIN_INTERP.load(Ordering::Relaxed) }
}

// ---------------------------------------------------------------------------------------
// Dependency pins
// ---------------------------------------------------------------------------------------

/// "Every dependency pinned with this flag is in the state it was validated in." One
/// relaxed load per call. A flag may be shared by several targets; it is true while it has
/// no pins.
#[derive(Debug)]
pub struct DepFlag(AtomicBool);

impl DepFlag {
    pub const fn new() -> Self {
        DepFlag(AtomicBool::new(true))
    }
    #[inline(always)]
    pub fn ok(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl Default for DepFlag {
    fn default() -> Self {
        Self::new()
    }
}

// The tables below are indexed by dict (one slot per watched dict) and, within a dict, by
// key. A pin is shared by every target that pins the same `(dict, key)`: each target is a
// `Dep` of the key slot (its own flag, expected value and owner), so registering a pin that
// another target already has costs a lookup and a push. Flags keep counters of their
// non-intact deps, so a transition updates a flag in O(1).

type FastMap<K, V> = std::collections::HashMap<K, V, foldhash::fast::FixedState>;

/// One target's interest in a pinned `dict[key]`.
struct Dep {
    owner: u64,
    flag: &'static DepFlag,
    /// The pinned value (strong reference), or 0 to pin "absent".
    expected: usize,
    /// Attribute pins: the class whose MRO the pin follows and its `tp_mro` as validated
    /// (strong references: neither address can be reused while pinned); 0 for dict pins.
    ty: usize,
    mro: usize,
    /// Holds the expected value right now and was never broken.
    good: bool,
    broken: bool,
}

struct KeySlot {
    /// Interned str key (strong reference).
    key: usize,
    deps: Vec<Dep>,
}

/// A str key (strong reference) and its hash (`None`: not a str, compared by identity).
type Key = (usize, Option<isize>);

/// A whole-dict snapshot pin: intact while the dict's contents equal the snapshot (keys
/// by str equality, values by identity). The callback keeps the set of deviating keys.
struct Snapshot {
    owner: u64,
    flag: &'static DepFlag,
    /// `(key, value)` strong references; keys are exact str.
    snap: Vec<(Key, usize)>,
    /// Keys currently deviating from the snapshot (strong references).
    dev: Vec<Key>,
    broken: bool,
    good: bool,
}

/// A generation watch: `counter` is bumped on every change of `dict[key]` (`key` 0: of any
/// key) and on wholesale changes of the dict. Process lifetime; deduplicated.
struct GenWatch {
    key: Key,
    counter: &'static AtomicU64,
}

struct DictSlot {
    /// Strong reference: its address cannot be reused while watched.
    dict: usize,
    /// Pinned keys by str hash: an event for an unpinned key costs one hash read and one
    /// map probe.
    keys: FastMap<isize, Vec<KeySlot>>,
    snaps: Vec<Snapshot>,
    gens: Vec<GenWatch>,
}

/// Deviations beyond this many keys break a snapshot pin (bounded memory).
const MAX_DEVIATIONS: usize = 64;

#[derive(Default)]
struct FlagCount {
    deps: u32,
    bad: u32,
}

#[derive(Default)]
struct Tables {
    dicts: FastMap<usize, DictSlot>,
    /// Non-intact deps per flag (by address).
    counts: FastMap<usize, FlagCount>,
    /// Flags whose flips must reach the registry (vectorcall targets: deopt while
    /// unpinned), as addresses.
    tracked: Vec<usize>,
    /// Every flag that ever had a dep (cleared at exit and when finalization starts).
    all_flags: Vec<usize>,
}

// SAFETY: plain addresses of objects kept alive by the tables' strong references.
unsafe impl Send for Tables {}

static TABLES: Mutex<Option<Tables>> = Mutex::new(None);
/// The interpreter is finalizing: every flag is false and the watchers return at once
/// (module dicts are being cleared; nothing is re-armed any more).
static CLOSED: AtomicBool = AtomicBool::new(false);
/// The registry is tearing down (exit hook): per-owner `deactivate`/`track` are skipped,
/// [`teardown`] drops the whole tables at once (one pass instead of one per owner).
static BULK: AtomicBool = AtomicBool::new(false);

/// Start an exit teardown: see [`BULK`]. Ended by [`teardown`].
pub(crate) fn begin_teardown() {
    BULK.store(true, Ordering::Release);
}
static DICT_WATCHER: AtomicI32 = AtomicI32::new(-1);
static TYPE_WATCHER: AtomicI32 = AtomicI32::new(-1);
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
/// `sys.modules` of the main interpreter (watched for lazy activation).
pub(crate) static SYS_MODULES: AtomicUsize = AtomicUsize::new(0);

/// A pin requested by a target's `prepare`, activated at install.
pub(crate) enum PendingPin {
    Key {
        dict: Py<PyDict>,
        key: Py<PyString>,
        /// `None`: pin "absent".
        expected: Option<Py<PyAny>>,
        flag: &'static DepFlag,
        /// Attribute pins: the class whose MRO the pin follows and its `__mro__` tuple as
        /// `prepare` walked it.
        ty: Option<(Py<PyType>, Py<PyAny>)>,
    },
    Dict {
        dict: Py<PyDict>,
        snap: Vec<(Py<PyString>, Py<PyAny>)>,
        flag: &'static DepFlag,
    },
}

// ---------------------------------------------------------------------------------------
// The pin-table lock is a leaf (docs/atlas/w2-threads.md §3.4 S2)
// ---------------------------------------------------------------------------------------
//
// The dict and type watcher callbacks take it while CPython holds the mutated dict's
// critical section (and, for class dicts, the global type lock). So while it is held,
// nothing may take a per-object critical section or the type lock, detach (any contended
// `PyMutex`, including a `Py_DECREF` that queues to another thread's biased refcount or
// frees an object with a finalizer), allocate Python objects, run Python code or stop the
// world. Code that needs both takes the dict's critical section *first* and this lock
// second: the same order as the watchers. Every CPython call in this module that could
// break the rule calls [`leaf_check`] first; a violation is counted (`_accel._lock_violations()`,
// asserted zero by the test-suite) and is a debug assertion.

thread_local! {
    static IN_TABLES: Cell<bool> = const { Cell::new(false) };
}

static LOCK_VIOLATIONS: AtomicU64 = AtomicU64::new(0);

/// The pin tables, locked. Held only for short, CPython-free sections.
struct TablesGuard(std::sync::MutexGuard<'static, Option<Tables>>);

impl std::ops::Deref for TablesGuard {
    type Target = Option<Tables>;
    fn deref(&self) -> &Option<Tables> {
        &self.0
    }
}

impl std::ops::DerefMut for TablesGuard {
    fn deref_mut(&mut self) -> &mut Option<Tables> {
        &mut self.0
    }
}

impl Drop for TablesGuard {
    fn drop(&mut self) {
        IN_TABLES.set(false);
    }
}

fn tables() -> TablesGuard {
    if IN_TABLES.get() {
        // Re-entry (a watcher firing on the thread that holds the lock) would deadlock.
        lock_violation("pin tables re-entered");
    }
    let g = TABLES.lock().unwrap_or_else(|p| p.into_inner());
    IN_TABLES.set(true);
    TablesGuard(g)
}

#[cold]
fn lock_violation(what: &str) {
    LOCK_VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    debug_assert!(false, "rustique: lock order violated: {what} while holding the pin tables");
}

/// Call before any CPython call that may lock, detach or run code (see above).
#[inline]
fn leaf_check(what: &str) {
    if IN_TABLES.get() {
        lock_violation(what);
    }
}

/// For other locks of the framework (the registry lock): taking them while holding the pin
/// tables violates the order too.
pub(crate) fn assert_not_in_tables(what: &str) {
    leaf_check(what);
}

/// Lock-order violations seen in this process (tests assert 0).
pub fn lock_violations() -> u64 {
    LOCK_VIOLATIONS.load(Ordering::Relaxed)
}

/// References a watcher callback dropped: released at the next safe point (a registry
/// operation, or the registry's pending call). A `Py_DECREF` inside a watcher could park on
/// a free-threaded build (biased refcounting's queue lock), and parking would suspend the
/// critical section of the dict being mutated, in the middle of its mutation. On GIL builds
/// the str keys released here run no code and take no lock: released at once.
static WATCHER_RELEASE: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn release_from_watcher(refs: Vec<usize>) {
    if cfg!(Py_GIL_DISABLED) {
        WATCHER_RELEASE.lock().unwrap_or_else(|p| p.into_inner()).extend(refs);
    } else {
        release_refs(refs);
    }
}

/// Release what watcher callbacks deferred (attached thread, no lock held, not in a
/// watcher).
pub(crate) fn release_deferred() {
    let refs = std::mem::take(&mut *WATCHER_RELEASE.lock().unwrap_or_else(|p| p.into_inner()));
    if !refs.is_empty() {
        release_refs(refs);
    }
}

/// Release strong references collected under the tables lock, after it was released.
fn release_refs(refs: Vec<usize>) {
    leaf_check("Py_DECREF");
    for o in refs {
        // SAFETY: attached thread; strong references owned by the caller.
        unsafe { ffi::Py_DECREF(o as *mut ffi::PyObject) };
    }
}

fn flag_addr(f: &'static DepFlag) -> usize {
    f as *const DepFlag as usize
}

impl Tables {
    /// A dep of `flag` was added (`good` or not).
    fn add_dep(&mut self, flag: &'static DepFlag, good: bool) {
        let a = flag_addr(flag);
        if !self.all_flags.contains(&a) {
            self.all_flags.push(a);
        }
        let c = self.counts.entry(a).or_default();
        c.deps += 1;
        c.bad += u32::from(!good);
        flag.0.store(c.bad == 0 && !CLOSED.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// A dep of `flag` was removed (it was `good` or not).
    fn remove_dep(&mut self, flag: &'static DepFlag, good: bool) {
        let a = flag_addr(flag);
        if let Some(c) = self.counts.get_mut(&a) {
            c.deps = c.deps.saturating_sub(1);
            c.bad = c.bad.saturating_sub(u32::from(!good));
            flag.0.store(c.bad == 0 && !CLOSED.load(Ordering::Relaxed), Ordering::Relaxed);
            if c.deps == 0 {
                self.counts.remove(&a);
            }
        }
    }

    /// A dep of `flag` went from `was` to `now`; returns whether the flag flipped.
    fn transition(&mut self, flag: &'static DepFlag, was: bool, now: bool) -> bool {
        if was == now {
            return false;
        }
        let Some(c) = self.counts.get_mut(&flag_addr(flag)) else { return false };
        let before = c.bad == 0;
        if now {
            c.bad = c.bad.saturating_sub(1);
        } else {
            c.bad += 1;
        }
        let after = c.bad == 0;
        flag.0.store(after, Ordering::Relaxed);
        before != after && self.tracked.contains(&flag_addr(flag))
    }
}

/// The str hash of `key` (exact str or a subclass: `str`'s own hash of the contents,
/// cached in the object; no Python code), `None` for any other key.
///
/// # Safety
/// Attached thread; `key` live or NULL.
#[inline]
unsafe fn str_hash(key: *mut ffi::PyObject) -> Option<isize> {
    // SAFETY: per the caller; `str.__hash__` (C) cannot fail or run Python code.
    unsafe {
        if key.is_null() || ffi::PyUnicode_Check(key) == 0 {
            return None;
        }
        let hash = ffi::PyUnicode_Type.tp_hash?;
        let h = hash(key);
        (h != -1).then_some(h)
    }
}

/// `(key, hash)` from a dict event equals `ours`: identity, then hash, then contents (two
/// str: no Python code). A non-str key matches by identity only.
///
/// # Safety
/// Attached thread; live objects.
#[inline]
unsafe fn same_key(key: *mut ffi::PyObject, hash: Option<isize>, ours: Key) -> bool {
    // SAFETY: per the caller; `PyUnicode_Compare` of two str cannot fail.
    key as usize == ours.0
        || (hash.is_some()
            && hash == ours.1
            && unsafe { ffi::PyUnicode_Compare(key, ours.0 as *mut ffi::PyObject) } == 0)
}

/// Finalization started (`Py_IsFinalizing`): clear every flag once, so kernels reached
/// while module dicts are emptied run CPython's code, and stop tracking. No Python code,
/// no reference released (the dict watcher callback is running).
#[cold]
fn close_for_finalization() {
    if CLOSED.swap(true, Ordering::AcqRel) {
        return;
    }
    let guard = tables();
    if let Some(t) = guard.as_ref() {
        for &a in &t.all_flags {
            // SAFETY: flags are statics (addresses of `&'static DepFlag`).
            unsafe { &*(a as *const DepFlag) }.0.store(false, Ordering::Relaxed);
        }
        for slot in t.dicts.values() {
            for g in &slot.gens {
                g.counter.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Is the interpreter finalizing? (Then the watchers stop.)
#[inline]
fn finalizing() -> bool {
    // SAFETY: reads the runtime's finalizing state (no GIL requirement).
    CLOSED.load(Ordering::Relaxed) || unsafe { ffi::Py_IsFinalizing() } != 0
}

unsafe extern "C" fn on_dict_event(
    event: c_int,
    dict: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
    new_value: *mut ffi::PyObject,
) -> c_int {
    // Rules (CPython's PyDict_WatchCallback): never raise, never run Python code, never
    // release the GIL or block on Python state; the dict is not modified yet.
    if finalizing() {
        // Module dicts are being cleared (hundreds of events): nothing to keep up to date.
        close_for_finalization();
        return 0;
    }
    if dict as usize == SYS_MODULES.load(Ordering::Relaxed) {
        // SAFETY: forwarded from CPython's callback.
        unsafe { super::registry::on_sys_modules_event(event, dict, key, new_value) };
    }
    let wholesale = event > 2; // CLONED, CLEARED, DEALLOCATED
    let mut guard = tables();
    let Some(t) = guard.as_mut() else { return 0 };
    let Some(slot) = t.dicts.get_mut(&(dict as usize)) else { return 0 };
    // SAFETY: attached thread; the event's key is live.
    let hash = if wholesale { None } else { unsafe { str_hash(key) } };
    // A key that is not a str may still compare equal to a pinned one (a custom `__eq__`
    // with the same hash replaces that entry): which one cannot be told without running
    // Python code, so every pin of the dict breaks.
    let unknown = !wholesale && hash.is_none();
    let mut changes: Vec<(&'static DepFlag, bool, bool)> = Vec::new();
    // Keys leaving a snapshot's deviation set: released after the lock (leaf rule).
    let mut release: Vec<usize> = Vec::new();
    for g in &slot.gens {
        // SAFETY: attached thread; live objects.
        if wholesale || unknown || g.key.0 == 0 || unsafe { same_key(key, hash, g.key) } {
            g.counter.fetch_add(1, Ordering::Relaxed);
        }
    }
    let update = |ks: &mut KeySlot, changes: &mut Vec<_>| {
        for d in ks.deps.iter_mut() {
            let was = d.good;
            if wholesale || unknown {
                d.broken = true;
            }
            let intact = !wholesale && if event == 2 { d.expected == 0 } else { new_value as usize == d.expected };
            d.good = intact && !d.broken;
            changes.push((d.flag, was, d.good));
        }
    };
    if wholesale || unknown {
        for ks in slot.keys.values_mut().flatten() {
            update(ks, &mut changes);
        }
    } else if let Some(bucket) = slot.keys.get_mut(&hash.unwrap_or(0)) {
        for ks in bucket.iter_mut() {
            // SAFETY: attached thread; live objects.
            if unsafe { same_key(key, hash, (ks.key, hash)) } {
                update(ks, &mut changes);
            }
        }
    }
    for sn in slot.snaps.iter_mut() {
        let was = sn.good;
        if wholesale || unknown || sn.broken {
            sn.broken = true;
        } else {
            // SAFETY: attached thread; live objects (strong references held by the snapshot).
            unsafe {
                let in_snap = sn.snap.iter().find(|&&(k, _)| same_key(key, hash, k)).map(|&(_, v)| v);
                let deviates = match event {
                    2 => in_snap.is_some(),
                    _ => in_snap != Some(new_value as usize),
                };
                let pos = sn.dev.iter().position(|&k| same_key(key, hash, k));
                match (deviates, pos) {
                    (true, None) => {
                        if sn.dev.len() >= MAX_DEVIATIONS {
                            sn.broken = true;
                        } else {
                            sn.dev.push((ffi::Py_NewRef(key) as usize, hash));
                        }
                    }
                    (false, Some(i)) => release.push(sn.dev.swap_remove(i).0),
                    _ => {}
                }
            }
        }
        sn.good = sn.dev.is_empty() && !sn.broken;
        changes.push((sn.flag, was, sn.good));
    }
    let mut flipped = false;
    for (f, was, now) in changes {
        flipped |= t.transition(f, was, now);
    }
    drop(guard);
    if !release.is_empty() {
        release_from_watcher(release);
    }
    if flipped {
        super::registry::schedule_resync();
    }
    0
}

unsafe extern "C" fn on_type_event(ty: *mut ffi::PyTypeObject) -> c_int {
    // A class with attribute pins was modified. Attribute changes are seen precisely by the
    // dict watcher on its `__dict__`; here only an MRO change matters (`__bases__`
    // assignment): the pinned lookup path is no longer the one validated.
    if finalizing() {
        return 0;
    }
    let mut guard = tables();
    let Some(t) = guard.as_mut() else { return 0 };
    let mut changes: Vec<(&'static DepFlag, bool, bool)> = Vec::new();
    for slot in t.dicts.values_mut() {
        for ks in slot.keys.values_mut().flatten() {
            for d in ks.deps.iter_mut().filter(|d| d.ty == ty as usize) {
                // SAFETY: the dep holds a strong reference to the type.
                if unsafe { (*ty).tp_mro } as usize != d.mro {
                    let was = d.good;
                    d.broken = true;
                    d.good = false;
                    changes.push((d.flag, was, false));
                }
            }
        }
    }
    let mut flipped = false;
    for (f, was, now) in changes {
        flipped |= t.transition(f, was, now);
    }
    drop(guard);
    if flipped {
        super::registry::schedule_resync();
    }
    0
}

/// The process's (main interpreter's) dict watcher id, registered once (two threads racing
/// here register one id; the loser clears its own). Every dict watch of Rustique goes
/// through it.
pub fn dict_watcher_id() -> PyResult<c_int> {
    let id = DICT_WATCHER.load(Ordering::Acquire);
    if id >= 0 {
        return Ok(id);
    }
    leaf_check("PyDict_AddWatcher");
    // SAFETY: attached thread (callers are attached).
    let id = unsafe { PyDict_AddWatcher(on_dict_event) };
    if id < 0 {
        return Err(Python::attach(PyErr::fetch));
    }
    match DICT_WATCHER.compare_exchange(-1, id, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Ok(id),
        Err(winner) => {
            // SAFETY: our own unused watcher id.
            if unsafe { PyDict_ClearWatcher(id) } < 0 {
                // SAFETY: attached thread.
                unsafe { ffi::PyErr_Clear() };
            }
            Ok(winner)
        }
    }
}

fn type_watcher_id() -> PyResult<c_int> {
    let id = TYPE_WATCHER.load(Ordering::Acquire);
    if id >= 0 {
        return Ok(id);
    }
    leaf_check("PyType_AddWatcher");
    // SAFETY: attached thread.
    let id = unsafe { PyType_AddWatcher(on_type_event) };
    if id < 0 {
        return Err(Python::attach(PyErr::fetch));
    }
    match TYPE_WATCHER.compare_exchange(-1, id, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Ok(id),
        Err(winner) => {
            // SAFETY: our own unused watcher id.
            if unsafe { PyType_ClearWatcher(id) } < 0 {
                // SAFETY: attached thread.
                unsafe { ffi::PyErr_Clear() };
            }
            Ok(winner)
        }
    }
}

/// Pre-registry API (prototype users): the dict watcher id.
pub fn watcher_id() -> PyResult<c_int> {
    dict_watcher_id()
}

/// The dict's slot (created with a strong reference to the dict). The dict must already be
/// watched ([`watch_locked`]). Runs under the tables lock: no CPython call but an incref.
fn dict_slot<'t>(t: &'t mut Tables, dict: *mut ffi::PyObject) -> &'t mut DictSlot {
    t.dicts.entry(dict as usize).or_insert_with(|| DictSlot {
        // SAFETY: a live dict (the caller holds a reference); an incref only.
        dict: unsafe { ffi::Py_NewRef(dict) } as usize,
        keys: FastMap::default(),
        snaps: Vec::new(),
        gens: Vec::new(),
    })
}

/// Watch `dict` (call under its critical section, before reading what is pinned: every
/// writer that takes the section after this sees the watcher bit, so no change between the
/// read and the registration in the tables can be missed).
///
/// # Safety
/// Attached thread; live dict; the caller holds the dict's critical section and not the
/// tables lock.
unsafe fn watch_locked(py: Python<'_>, id: c_int, dict: *mut ffi::PyObject) -> PyResult<()> {
    leaf_check("PyDict_Watch");
    // SAFETY: per the caller (an atomic OR of the watcher bit).
    if unsafe { ffi::PyDict_Watch(id, dict) } < 0 {
        return Err(PyErr::fetch(py));
    }
    Ok(())
}

/// Generation watch: bump `gen` (relaxed) whenever `dict[key]` is added, replaced or
/// deleted, or the dict is cleared or replaced wholesale. For dependencies that are
/// legitimately replaced at run time (`_strptime._TimeRE_cache`, `time.tzname`): kernels
/// cache the generation they validated at and revalidate by value when it moves. Every
/// change made after this returns bumps `gen` (read the value afterwards). Idempotent per
/// `(dict, key, gen)`; lives as long as the process.
pub fn watch(dict: &Bound<'_, PyDict>, key: &str, gen_: &'static AtomicU64) -> PyResult<()> {
    let py = dict.py();
    let k = super::prepare::interned(py, key);
    // SAFETY: attached thread; live str.
    let h = unsafe { str_hash(k.as_ptr()) };
    let id = dict_watcher_id()?;
    pyo3::sync::critical_section::with_critical_section(dict.as_any(), || -> PyResult<()> {
        // SAFETY: attached thread; we hold the dict's critical section, not the tables lock.
        unsafe { watch_locked(py, id, dict.as_ptr())? };
        let mut guard = tables();
        let t = guard.get_or_insert_with(Tables::default);
        let slot = dict_slot(t, dict.as_ptr());
        let known = slot.gens.iter().any(|g| {
            // SAFETY: attached thread; live objects (no Python code).
            std::ptr::eq(g.counter, gen_) && g.key.0 != 0 && unsafe { same_key(k.as_ptr(), h, g.key) }
        });
        if !known {
            // SAFETY: an incref only.
            slot.gens.push(GenWatch { key: (unsafe { ffi::Py_NewRef(k.as_ptr()) } as usize, h), counter: gen_ });
        }
        Ok(())
    })
}

/// Whole-dict generation watch: bump `gen` on every added, replaced or deleted key of
/// `dict` and on wholesale changes, so a kernel can cache a fact derived from a small dict
/// (a mapping it scans) between mutations. Idempotent per `(dict, gen)`; process lifetime.
pub fn watch_dict_generation(dict: &Bound<'_, PyDict>, gen_: &'static AtomicU64) -> PyResult<()> {
    let py = dict.py();
    let id = dict_watcher_id()?;
    pyo3::sync::critical_section::with_critical_section(dict.as_any(), || -> PyResult<()> {
        // SAFETY: attached thread; we hold the dict's critical section, not the tables lock.
        unsafe { watch_locked(py, id, dict.as_ptr())? };
        let mut guard = tables();
        let t = guard.get_or_insert_with(Tables::default);
        let slot = dict_slot(t, dict.as_ptr());
        if !slot.gens.iter().any(|g| g.key.0 == 0 && std::ptr::eq(g.counter, gen_)) {
            slot.gens.push(GenWatch { key: (0, None), counter: gen_ });
        }
        Ok(())
    })
}

/// Activate `pins` under a new owner id; returns the owner id and the distinct flags of
/// the pins. Pins already held by other targets are shared (one key slot per `(dict,
/// key)`). Each pin's state is read now (a change between `prepare` and install shows as a
/// deviation), under the pinned dict's critical section, which is held until the pin is in
/// the tables (lock order: dict section, then the tables lock, as in the watchers). On an
/// error nothing stays registered.
pub(crate) fn activate(py: Python<'_>, pins: Vec<PendingPin>) -> PyResult<(u64, Vec<&'static DepFlag>)> {
    let owner = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
    if pins.is_empty() {
        return Ok((owner, Vec::new()));
    }
    let mut flags: Vec<&'static DepFlag> = Vec::new();
    for p in &pins {
        let f = match p {
            PendingPin::Key { flag, .. } | PendingPin::Dict { flag, .. } => *flag,
        };
        if !flags.iter().any(|x| std::ptr::eq(*x, f)) {
            flags.push(f);
        }
    }
    match activate_pins(py, owner, pins) {
        Ok(()) => Ok((owner, flags)),
        Err(e) => {
            deactivate(owner);
            Err(e)
        }
    }
}

fn activate_pins(py: Python<'_>, owner: u64, pins: Vec<PendingPin>) -> PyResult<()> {
    let id = dict_watcher_id()?;
    for p in pins {
        // References dropped by this pin (read values, unused snapshot entries): released
        // outside the dict's section and the tables lock.
        let mut drop_later: Vec<Py<PyAny>> = Vec::new();
        match p {
            PendingPin::Key { dict, key, expected, flag, ty } => {
                // Class attribute pins: watch the class before its dicts (`PyType_Watch`
                // takes the type lock: never under a dict section or the tables lock).
                if let Some((tp, _)) = &ty {
                    let tid = type_watcher_id()?;
                    leaf_check("PyType_Watch");
                    // SAFETY: attached thread; live type; no section or table lock held.
                    if unsafe { PyType_Watch(tid, tp.as_ptr()) } < 0 {
                        return Err(PyErr::fetch(py));
                    }
                }
                let d = dict.bind(py);
                pyo3::sync::critical_section::with_critical_section(d.as_any(), || -> PyResult<()> {
                    // SAFETY: attached thread; we hold the dict's section, not the tables lock.
                    unsafe { watch_locked(py, id, d.as_ptr())? };
                    // Current value: an exact dict and an interned str key (no Python code);
                    // consistent with the watcher (writers wait for our section).
                    let mut cur: *mut ffi::PyObject = std::ptr::null_mut();
                    leaf_check("PyDict_GetItemRef");
                    // SAFETY: attached thread; live dict and key.
                    if unsafe { ffi::PyDict_GetItemRef(d.as_ptr(), key.as_ptr(), &mut cur) } < 0 {
                        return Err(PyErr::fetch(py));
                    }
                    let exp = expected.as_ref().map_or(0, |o| o.as_ptr() as usize);
                    let good_value = cur as usize == exp;
                    // SAFETY: the new reference from PyDict_GetItemRef (or NULL): dropped later.
                    if let Some(c) = unsafe { Bound::from_owned_ptr_or_opt(py, cur) } {
                        drop_later.push(c.unbind());
                    }
                    // SAFETY: attached thread; live str.
                    let h = unsafe { str_hash(key.as_ptr()) }.unwrap_or(0);
                    let mut guard = tables();
                    let t = guard.get_or_insert_with(Tables::default);
                    // An MRO change since `prepare` walked it: the pinned path is not the
                    // validated one (an MRO change after this is seen by the type watcher).
                    let (tyaddr, mro, mro_ok) = match &ty {
                        // SAFETY: live type; increfs only.
                        Some((tp, m)) => unsafe {
                            let now = (*tp.as_ptr().cast::<ffi::PyTypeObject>()).tp_mro;
                            (
                                ffi::Py_NewRef(tp.as_ptr()) as usize,
                                ffi::Py_NewRef(m.as_ptr()) as usize,
                                now as usize == m.as_ptr() as usize,
                            )
                        },
                        None => (0, 0, true),
                    };
                    let good = good_value && mro_ok;
                    let slot = dict_slot(t, d.as_ptr());
                    // Keys come from the interning cache (or `pyo3::intern!`): one object per
                    // name, so identity is the key slot's identity.
                    let kp = key.as_ptr() as usize;
                    let bucket = slot.keys.entry(h).or_default();
                    let ks = match bucket.iter().position(|k| k.key == kp) {
                        Some(i) => &mut bucket[i],
                        None => {
                            // SAFETY: an incref only.
                            bucket.push(KeySlot { key: unsafe { ffi::Py_NewRef(key.as_ptr()) } as usize, deps: Vec::new() });
                            bucket.last_mut().expect("just pushed")
                        }
                    };
                    ks.deps.push(Dep {
                        owner,
                        flag,
                        // SAFETY: an incref only (the dep's strong reference).
                        expected: expected.as_ref().map_or(0, |o| unsafe { ffi::Py_NewRef(o.as_ptr()) } as usize),
                        ty: tyaddr,
                        mro,
                        good,
                        broken: !mro_ok,
                    });
                    t.add_dep(flag, good);
                    Ok(())
                })?;
                drop_later.push(dict.into_any());
                drop_later.push(key.into_any());
                if let Some(e) = expected {
                    drop_later.push(e);
                }
                if let Some((tp, m)) = ty {
                    drop_later.push(tp.into_any());
                    drop_later.push(m);
                }
            }
            PendingPin::Dict { dict, snap, flag } => {
                let d = dict.bind(py);
                pyo3::sync::critical_section::with_critical_section(d.as_any(), || -> PyResult<()> {
                    // SAFETY: attached thread; we hold the dict's section, not the tables lock.
                    unsafe { watch_locked(py, id, d.as_ptr())? };
                    let snap: Vec<(Key, usize)> = snap
                        .iter()
                        .map(|(k, v)| {
                            // SAFETY: attached thread; live str.
                            let h = unsafe { str_hash(k.as_ptr()) };
                            (
                                // SAFETY: increfs only (the snapshot's strong references).
                                (unsafe { ffi::Py_NewRef(k.as_ptr()) } as usize, h),
                                unsafe { ffi::Py_NewRef(v.as_ptr()) } as usize,
                            )
                        })
                        .collect();
                    // Deviations of the current contents from the snapshot. `PyDict_Next`
                    // under the dict's section: one consistent view, no concurrent resize.
                    let mut dev: Vec<Key> = Vec::new();
                    let mut seen = 0usize;
                    let mut pos: ffi::Py_ssize_t = 0;
                    let mut k: *mut ffi::PyObject = std::ptr::null_mut();
                    let mut v: *mut ffi::PyObject = std::ptr::null_mut();
                    // SAFETY: attached thread; borrowed references valid under the section.
                    unsafe {
                        while ffi::PyDict_Next(d.as_ptr(), &mut pos, &mut k, &mut v) != 0 {
                            let h = str_hash(k);
                            let same = snap.iter().any(|&(sk, sv)| sv == v as usize && same_key(k, h, sk));
                            if same {
                                seen += 1;
                            } else {
                                dev.push((ffi::Py_NewRef(k) as usize, h));
                            }
                        }
                    }
                    // Snapshot keys no longer present (every present one matched exactly once:
                    // keys are unique, values compared by identity).
                    if seen < snap.len() {
                        for &((sk, h), _) in &snap {
                            let mut out: *mut ffi::PyObject = std::ptr::null_mut();
                            leaf_check("PyDict_GetItemRef");
                            // SAFETY: attached thread; str key, exact dict: no Python code.
                            let r = unsafe { ffi::PyDict_GetItemRef(d.as_ptr(), sk as *mut _, &mut out) };
                            if r < 0 {
                                // SAFETY: attached thread.
                                unsafe { ffi::PyErr_Clear() };
                            }
                            if r <= 0 {
                                // SAFETY: an incref only.
                                dev.push((unsafe { ffi::Py_NewRef(sk as *mut ffi::PyObject) } as usize, h));
                            } else {
                                // SAFETY: the new reference: dropped later.
                                drop_later.push(unsafe { Bound::from_owned_ptr(py, out) }.unbind());
                            }
                        }
                    }
                    let broken = dev.len() > MAX_DEVIATIONS;
                    let good = dev.is_empty() && !broken;
                    let mut guard = tables();
                    let t = guard.get_or_insert_with(Tables::default);
                    let slot = dict_slot(t, d.as_ptr());
                    slot.snaps.push(Snapshot { owner, flag, snap, dev, broken, good });
                    t.add_dep(flag, good);
                    Ok(())
                })?;
                drop_later.push(dict.into_any());
                for (k, v) in snap {
                    drop_later.push(k.into_any());
                    drop_later.push(v);
                }
            }
        }
        leaf_check("Py_DECREF");
        drop(drop_later);
    }
    Ok(())
}

/// Report flips of `flags` to the registry (deopt while unpinned), or stop.
pub(crate) fn track(flags: &[&'static DepFlag], on: bool) {
    if BULK.load(Ordering::Acquire) {
        return;
    }
    let mut guard = tables();
    let t = guard.get_or_insert_with(Tables::default);
    for f in flags {
        let a = flag_addr(f);
        if on {
            t.tracked.push(a);
        } else if let Some(i) = t.tracked.iter().position(|&x| x == a) {
            t.tracked.swap_remove(i);
        }
    }
}

/// Remove the pins of `owner`; key slots, dicts and types left without any interest are
/// released and unwatched.
pub(crate) fn deactivate(owner: u64) {
    if BULK.load(Ordering::Acquire) {
        return;
    }
    let mut release: Vec<usize> = Vec::new();
    let mut unwatch_dicts: Vec<usize> = Vec::new();
    let mut types: Vec<usize> = Vec::new();
    {
        let mut guard = tables();
        let Some(t) = guard.as_mut() else { return };
        let mut removed: Vec<(&'static DepFlag, bool)> = Vec::new();
        let sys_modules = SYS_MODULES.load(Ordering::Relaxed);
        t.dicts.retain(|_, slot| {
            for ks in slot.keys.values_mut().flatten() {
                ks.deps.retain(|d| {
                    if d.owner != owner {
                        return true;
                    }
                    removed.push((d.flag, d.good));
                    if d.expected != 0 {
                        release.push(d.expected);
                    }
                    if d.ty != 0 {
                        // The dep's strong references to the class and its validated MRO.
                        types.push(d.ty);
                        release.push(d.ty);
                        release.push(d.mro);
                    }
                    false
                });
            }
            slot.keys.retain(|_, bucket| {
                bucket.retain(|ks| {
                    if ks.deps.is_empty() {
                        release.push(ks.key);
                        false
                    } else {
                        true
                    }
                });
                !bucket.is_empty()
            });
            slot.snaps.retain(|sn| {
                if sn.owner != owner {
                    return true;
                }
                removed.push((sn.flag, sn.good));
                for &((k, _), v) in &sn.snap {
                    release.push(k);
                    release.push(v);
                }
                release.extend(sn.dev.iter().map(|k| k.0));
                false
            });
            let keep = !slot.keys.is_empty() || !slot.snaps.is_empty() || !slot.gens.is_empty();
            if !keep {
                if slot.dict != sys_modules {
                    unwatch_dicts.push(slot.dict);
                }
                release.push(slot.dict);
            }
            keep
        });
        for (f, good) in removed {
            t.remove_dep(f, good);
        }
        // Types still pinned by someone stay watched.
        types.retain(|ty| {
            !t.dicts.values().any(|s| s.keys.values().flatten().any(|k| k.deps.iter().any(|d| d.ty == *ty)))
        });
    }
    // SAFETY: attached thread (callers hold the registry lock); releasing the strong
    // references taken at activation, outside the tables lock (a value's finalizer may
    // run Python code).
    unsafe {
        let did = DICT_WATCHER.load(Ordering::Acquire);
        for d in unwatch_dicts {
            if did >= 0 && ffi::PyDict_Unwatch(did, d as *mut _) < 0 {
                ffi::PyErr_Clear();
            }
        }
        let tid = TYPE_WATCHER.load(Ordering::Acquire);
        types.sort_unstable();
        types.dedup();
        for ty in types {
            if tid >= 0 && PyType_Unwatch(tid, ty as *mut _) < 0 {
                ffi::PyErr_Clear();
            }
        }
        for o in release {
            ffi::Py_DECREF(o as *mut _);
        }
    }
}

/// Interpreter exit (the registry's exit hook, after every target was deactivated): drop
/// everything the tables still hold (generation watches, pre-registry pins), unwatch every
/// dict and type, and clear every flag that ever had a dep, so a kernel reached on a stale
/// path falls back. A later activation starts from empty tables.
pub(crate) fn teardown() {
    let taken = tables().take();
    BULK.store(false, Ordering::Release);
    let Some(t) = taken else { return };
    for &a in &t.all_flags {
        // SAFETY: flags are statics (addresses of `&'static DepFlag`).
        unsafe { &*(a as *const DepFlag) }.0.store(false, Ordering::Relaxed);
    }
    let mut release: Vec<usize> = Vec::new();
    let mut types: Vec<usize> = Vec::new();
    let mut dicts: Vec<usize> = Vec::new();
    for slot in t.dicts.into_values() {
        for ks in slot.keys.into_values().flatten() {
            release.push(ks.key);
            for d in ks.deps {
                if d.expected != 0 {
                    release.push(d.expected);
                }
                if d.ty != 0 {
                    types.push(d.ty);
                    release.push(d.ty);
                    release.push(d.mro);
                }
            }
        }
        for sn in slot.snaps {
            for ((k, _), v) in sn.snap {
                release.push(k);
                release.push(v);
            }
            release.extend(sn.dev.iter().map(|k| k.0));
        }
        for g in slot.gens {
            // Kernels caching a validation by generation revalidate once more.
            g.counter.fetch_add(1, Ordering::Relaxed);
            if g.key.0 != 0 {
                release.push(g.key.0);
            }
        }
        dicts.push(slot.dict);
    }
    // SAFETY: attached thread (the registry holds its lock, as for `deactivate`); the
    // tables' own strong references, released outside the tables lock (finalizers may run
    // Python code).
    unsafe {
        let did = DICT_WATCHER.load(Ordering::Acquire);
        for &d in &dicts {
            if did >= 0 && ffi::PyDict_Unwatch(did, d as *mut _) < 0 {
                ffi::PyErr_Clear();
            }
        }
        let tid = TYPE_WATCHER.load(Ordering::Acquire);
        types.sort_unstable();
        types.dedup();
        for &ty in &types {
            if tid >= 0 && PyType_Unwatch(tid, ty as *mut _) < 0 {
                ffi::PyErr_Clear();
            }
        }
        for o in release.into_iter().chain(dicts) {
            ffi::Py_DECREF(o as *mut _);
        }
    }
}

/// Pin state of `owner` for diagnostics: (key, absent-pin, intact, broken); snapshot pins
/// show as `("<dict>", false, no deviation, broken)`.
pub(crate) fn describe(py: Python<'_>, owner: u64) -> Vec<(String, bool, bool, bool)> {
    // Collected under the lock (increfs only), formatted after it.
    let mut rows: Vec<(usize, bool, bool, bool)> = Vec::new();
    {
        let guard = tables();
        let Some(t) = guard.as_ref() else { return Vec::new() };
        for slot in t.dicts.values() {
            for ks in slot.keys.values().flatten() {
                for d in ks.deps.iter().filter(|d| d.owner == owner) {
                    // SAFETY: an incref of the slot's live key.
                    rows.push((unsafe { ffi::Py_NewRef(ks.key as *mut ffi::PyObject) } as usize, d.expected == 0, d.good, d.broken));
                }
            }
            for sn in slot.snaps.iter().filter(|s| s.owner == owner) {
                rows.push((0, false, sn.dev.is_empty(), sn.broken));
            }
        }
    }
    rows.into_iter()
        .map(|(k, absent, good, broken)| {
            let name = if k == 0 {
                "<dict>".to_owned()
            } else {
                // SAFETY: the new reference taken above.
                unsafe { Bound::from_owned_ptr(py, k as *mut ffi::PyObject) }.to_string()
            };
            (name, absent, good, broken)
        })
        .collect()
}

/// Watch `sys.modules` for lazy activation (idempotent).
pub(crate) fn watch_sys_modules(py: Python<'_>, sys_modules: &Bound<'_, PyAny>) -> PyResult<()> {
    SYS_MODULES.store(sys_modules.as_ptr() as usize, Ordering::Relaxed);
    let id = dict_watcher_id()?;
    // Under the dict's section: every insert after this sees the watcher bit (the caller
    // registers the pending names first and checks `sys.modules` afterwards).
    pyo3::sync::critical_section::with_critical_section(sys_modules, || {
        // SAFETY: attached thread; we hold the dict's section, not the tables lock.
        unsafe { watch_locked(py, id, sys_modules.as_ptr()) }
    })
}

/// Stop watching `sys.modules` (nothing pending), unless something else watches it.
pub(crate) fn unwatch_sys_modules() {
    let d = SYS_MODULES.load(Ordering::Relaxed);
    let id = DICT_WATCHER.load(Ordering::Acquire);
    if d != 0 && id >= 0 {
        if tables().as_ref().is_some_and(|t| t.dicts.contains_key(&d)) {
            return;
        }
        // SAFETY: attached thread (callers hold the registry lock); `sys.modules` lives as
        // long as the interpreter.
        unsafe {
            if ffi::PyDict_Unwatch(id, d as *mut _) < 0 {
                ffi::PyErr_Clear();
            }
        }
    }
}

/// Pre-registry API (prototype users): pin `dict[key]` to its current value for `flag`,
/// active immediately and never removed.
pub fn pin(dict: &Bound<'_, PyDict>, key: &str, flag: &'static DepFlag) -> PyResult<()> {
    let py = dict.py();
    let value = dict.get_item(key)?.ok_or_else(|| PyRuntimeError::new_err(format!("missing dependency {key}")))?;
    activate(
        py,
        vec![PendingPin::Key {
            dict: dict.clone().unbind(),
            key: super::prepare::interned(py, key).unbind(),
            expected: Some(value.unbind()),
            flag,
            ty: None,
        }],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Per-type verdicts ("does this subclass override any of these names?")
// ---------------------------------------------------------------------------------------

python_extern! {
    fn PyUnstable_Type_AssignVersionTag(ty: *mut ffi::PyTypeObject) -> c_int;
}

const VERDICT_SLOTS: usize = 16;

/// Cache of "does class `C` (a subclass of `base`) define any of `names` before `base` in
/// its MRO?", keyed by the class's `tp_version_tag`. CPython gives every class version a
/// unique tag and resets it on any modification of the class or a base (`PyType_Modified`),
/// so a cached verdict can never outlive what it was computed from: no watcher needed.
/// Direct-mapped, 16 entries; a hit costs a few loads and compares.
///
/// ```ignore
/// static OVERRIDES: OverrideCache = OverrideCache::new(&["__fspath__", "__str__"]);
/// // prepare: OVERRIDES.set_base(&pure_path_type);
/// // kernel:  if Py_TYPE(p) != PURE_PATH && OVERRIDES.overrides(Py_TYPE(p)) != Some(false) { fallback }
/// ```
pub struct OverrideCache {
    names: &'static [&'static str],
    base: AtomicUsize,
    /// `(tag << 1) | verdict` per slot; tag 0 = empty. Tags are globally unique per class
    /// version, so a slot written concurrently for another class simply misses.
    slots: [AtomicU64; VERDICT_SLOTS],
}

impl OverrideCache {
    pub const fn new(names: &'static [&'static str]) -> Self {
        OverrideCache { names, base: AtomicUsize::new(0), slots: [const { AtomicU64::new(0) }; VERDICT_SLOTS] }
    }

    /// The class whose own definitions are the reference (kept alive by the caller, e.g.
    /// pinned or held by a static reference).
    pub fn set_base(&self, base: &Bound<'_, PyType>) {
        self.base.store(base.as_type_ptr() as usize, Ordering::Release);
        for s in &self.slots {
            s.store(0, Ordering::Relaxed);
        }
    }

    /// `Some(true)` if `ty` (or a class between it and the base in its MRO) defines one of
    /// the names, `Some(false)` if none does, `None` if `ty` is not a subclass of the base
    /// (or no base was set).
    ///
    /// # Safety
    /// Attached thread; `ty` a live type. Runs no Python code.
    #[inline]
    pub unsafe fn overrides(&self, ty: *mut ffi::PyTypeObject) -> Option<bool> {
        // SAFETY: per the caller.
        unsafe {
            let tag = std::ptr::read_volatile(&raw const (*ty).tp_version_tag);
            if tag != 0 {
                let v = self.slots[(ty as usize >> 4) % VERDICT_SLOTS].load(Ordering::Relaxed);
                if v >> 1 == u64::from(tag) {
                    return Some(v & 1 != 0);
                }
            }
            self.compute(ty)
        }
    }

    #[cold]
    unsafe fn compute(&self, ty: *mut ffi::PyTypeObject) -> Option<bool> {
        let base = self.base.load(Ordering::Acquire) as *mut ffi::PyTypeObject;
        if base.is_null() {
            return None;
        }
        // SAFETY: attached thread; live type objects; dict lookups with str keys run no
        // Python code.
        unsafe {
            if PyUnstable_Type_AssignVersionTag(ty) == 0 {
                ffi::PyErr_Clear();
            }
            let tag = std::ptr::read_volatile(&raw const (*ty).tp_version_tag);
            let py = Python::assume_attached();
            let mro = (*ty).tp_mro;
            if mro.is_null() {
                return None;
            }
            let mro = Bound::from_borrowed_ptr(py, mro);
            let Ok(mro) = mro.cast::<pyo3::types::PyTuple>() else { return None };
            let mut found = false;
            let mut reached = false;
            for cls in mro.iter() {
                if cls.as_ptr() == base.cast() {
                    reached = true;
                    break;
                }
                let Ok(cls) = cls.cast::<PyType>() else { return None };
                let d = ffi::PyType_GetDict(cls.as_type_ptr());
                if d.is_null() {
                    ffi::PyErr_Clear();
                    return None;
                }
                let d = Bound::from_owned_ptr(py, d);
                let Ok(d) = d.cast::<PyDict>() else { return None };
                found |= self.names.iter().any(|n| d.contains(*n).unwrap_or(true));
            }
            if !reached {
                return None;
            }
            let verdict = found;
            if tag != 0 {
                self.slots[(ty as usize >> 4) % VERDICT_SLOTS]
                    .store(u64::from(tag) << 1 | u64::from(verdict), Ordering::Relaxed);
            }
            Some(verdict)
        }
    }
}

impl std::fmt::Debug for OverrideCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OverrideCache({:?})", self.names)
    }
}
