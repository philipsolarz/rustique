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

use std::cell::UnsafeCell;
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

unsafe extern "C" {
    fn PyMonitoring_EnterScope(
        state_array: *mut PyMonitoringState,
        version: *mut u64,
        event_types: *const u8,
        length: ffi::Py_ssize_t,
    ) -> c_int;
    fn PyDict_AddWatcher(cb: DictWatchCallback) -> c_int;
    fn PyDict_Unwatch(watcher_id: c_int, dict: *mut ffi::PyObject) -> c_int;
    fn PyType_AddWatcher(cb: TypeWatchCallback) -> c_int;
    fn PyType_Watch(watcher_id: c_int, ty: *mut ffi::PyObject) -> c_int;
    fn PyType_Unwatch(watcher_id: c_int, ty: *mut ffi::PyObject) -> c_int;
}

pub const PY_MONITORING_EVENT_PY_START: u8 = 0;

// ---------------------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------------------

/// Monitoring scope of one target: `PyMonitoring_EnterScope` returns after one compare when
/// the interpreter's global monitoring version has not changed since the last call.
///
/// Shared by all threads: the API writes `version`/`state` non-atomically; concurrent
/// writers store the same values (a benign race, as in Cython's use of the same API).
pub struct Tools {
    version: UnsafeCell<u64>,
    state: UnsafeCell<PyMonitoringState>,
}

// SAFETY: see the type docs (benign same-value races, written only by CPython's API).
unsafe impl Sync for Tools {}

impl Tools {
    pub const fn new() -> Self {
        Tools { version: UnsafeCell::new(0), state: UnsafeCell::new(PyMonitoringState { active: 0, opaque: 0 }) }
    }

    /// True when no tool listens to `PY_START` in this interpreter (global events).
    ///
    /// # Safety
    /// Attached thread.
    #[inline(always)]
    pub unsafe fn quiet(&self) -> bool {
        // SAFETY: valid pointers to this scope's storage.
        unsafe {
            let ev = PY_MONITORING_EVENT_PY_START;
            PyMonitoring_EnterScope(self.state.get(), self.version.get(), &ev, 1);
            (*self.state.get()).active == 0
        }
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
/// tool ever instrumented the code) and its first member `local_monitors.tools[0..11]`
/// (internal layout, identical for the 11 local events in 3.14 and 3.15; calibrated by the
/// registry for every target with a vectorcall site).
///
/// # Safety
/// `code` is a live code object; layout calibrated (`version::code_layout_ok`).
#[inline(always)]
pub unsafe fn code_locally_quiet(code: *mut ffi::PyObject) -> bool {
    // SAFETY: per the caller.
    unsafe {
        let mon = (*code.cast::<crate::accel::version::RawCode>())._co_monitoring;
        if mon.is_null() {
            return true;
        }
        let lo = std::ptr::read_unaligned(mon.cast::<u64>());
        let hi = std::ptr::read_unaligned(mon.add(8).cast::<u16>()) as u64 | (*mon.add(10) as u64) << 16;
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

/// One pinned `dict[key]` (a module global, a builtin, or one class `__dict__` along an
/// MRO): intact while the key maps to `expected` (or is absent, for absence pins).
struct Entry {
    /// Owner (one target's install), for removal.
    owner: u64,
    /// The watched dict (strong reference: its address cannot be reused while pinned).
    dict: usize,
    /// The key (strong reference to an interned str).
    key: usize,
    /// The pinned value (strong reference), or 0 to pin "absent".
    expected: usize,
    flag: &'static DepFlag,
    /// Holds the expected value right now.
    intact: bool,
    /// Never re-arms (the dict was cleared or replaced wholesale, or the class's MRO
    /// changed).
    broken: bool,
    /// Attribute pins: the class whose MRO the pin follows (strong ref) and its `tp_mro`
    /// at pin time (identity only); 0 for dict pins.
    ty: usize,
    mro: usize,
}

/// A whole-dict snapshot pin: intact while the dict's contents equal the snapshot (keys
/// by str equality, values by identity). The callback keeps the set of deviating keys.
struct Snapshot {
    owner: u64,
    dict: usize,
    flag: &'static DepFlag,
    /// `(key, value)` strong references; keys are exact str.
    snap: Vec<(usize, usize)>,
    /// Keys currently deviating from the snapshot (strong references).
    dev: Vec<usize>,
    broken: bool,
}

/// A generation watch: `gen` is bumped on every change of `dict[key]` (and on wholesale
/// changes of the dict). Process lifetime; deduplicated.
struct GenWatch {
    dict: usize,
    key: usize,
    counter: &'static AtomicU64,
}

/// Deviations beyond this many keys break a snapshot pin (bounded memory).
const MAX_DEVIATIONS: usize = 64;

#[derive(Default)]
struct Tables {
    pins: Vec<Entry>,
    snaps: Vec<Snapshot>,
    gens: Vec<GenWatch>,
    /// Flags whose flips must reach the registry (vectorcall targets: deopt while
    /// unpinned), as addresses.
    tracked: Vec<usize>,
}

// SAFETY: plain addresses of objects kept alive by the tables' strong references.
unsafe impl Send for Tables {}

static TABLES: Mutex<Tables> =
    Mutex::new(Tables { pins: Vec::new(), snaps: Vec::new(), gens: Vec::new(), tracked: Vec::new() });
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
        ty: Option<Py<PyType>>,
    },
    Dict {
        dict: Py<PyDict>,
        snap: Vec<(Py<PyString>, Py<PyAny>)>,
        flag: &'static DepFlag,
    },
}

impl PendingPin {
    pub(crate) fn flag(&self) -> &'static DepFlag {
        match self {
            PendingPin::Key { flag, .. } | PendingPin::Dict { flag, .. } => flag,
        }
    }
}

/// Recompute `flag` from every pin using it; returns whether its value flipped.
fn recompute(t: &Tables, flag: &'static DepFlag) -> bool {
    let all = t.pins.iter().filter(|e| std::ptr::eq(e.flag, flag)).all(|e| e.intact && !e.broken)
        && t.snaps.iter().filter(|e| std::ptr::eq(e.flag, flag)).all(|e| e.dev.is_empty() && !e.broken);
    flag.0.swap(all, Ordering::Relaxed) != all
}

/// Recompute `flags`; if a tracked flag flipped, ask the registry to resynchronise.
fn recompute_all(t: &Tables, flags: &[&'static DepFlag]) {
    let mut flipped = false;
    for f in flags {
        if recompute(t, f) && t.tracked.contains(&(*f as *const DepFlag as usize)) {
            flipped = true;
        }
    }
    if flipped {
        super::registry::schedule_resync();
    }
}

/// Lock the tables. Held only for short, Python-free sections (never across a call that
/// could run Python code, release the GIL or block), so the watcher callbacks may wait on
/// it.
fn tables() -> std::sync::MutexGuard<'static, Tables> {
    TABLES.lock().unwrap_or_else(|p| p.into_inner())
}

/// `key` (from a dict event) equals the str `ours`. No Python code runs: identity, then
/// exact-str comparison.
///
/// # Safety
/// Attached thread; live objects.
#[inline]
unsafe fn same_key(key: *mut ffi::PyObject, ours: usize) -> bool {
    // SAFETY: per the caller; `PyUnicode_Compare` of two exact str cannot fail.
    unsafe {
        !key.is_null()
            && (key as usize == ours
                || (ffi::PyUnicode_CheckExact(key) != 0 && ffi::PyUnicode_Compare(key, ours as *mut _) == 0))
    }
}

unsafe extern "C" fn on_dict_event(
    event: c_int,
    dict: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
    new_value: *mut ffi::PyObject,
) -> c_int {
    // Rules (CPython's PyDict_WatchCallback): never raise, never run Python code, never
    // release the GIL or block on Python state; the dict is not modified yet.
    if dict as usize == SYS_MODULES.load(Ordering::Relaxed) {
        // SAFETY: forwarded from CPython's callback.
        unsafe { super::registry::on_sys_modules_event(event, dict, key, new_value) };
    }
    let wholesale = event > 2; // CLONED, CLEARED, DEALLOCATED
    let mut t = tables();
    let mut touched: Vec<&'static DepFlag> = Vec::new();
    for g in t.gens.iter().filter(|g| g.dict == dict as usize) {
        // SAFETY: attached thread; live objects.
        if wholesale || unsafe { same_key(key, g.key) } {
            g.counter.fetch_add(1, Ordering::Relaxed);
        }
    }
    for e in t.pins.iter_mut().filter(|e| e.dict == dict as usize) {
        if wholesale {
            e.broken = true;
            touched.push(e.flag);
        // SAFETY: attached thread; live objects.
        } else if unsafe { same_key(key, e.key) } {
            e.intact = if event == 2 { e.expected == 0 } else { new_value as usize == e.expected };
            touched.push(e.flag);
        }
    }
    for e in t.snaps.iter_mut().filter(|e| e.dict == dict as usize) {
        touched.push(e.flag);
        if wholesale || e.broken {
            e.broken = true;
            continue;
        }
        // SAFETY: attached thread; live objects (strong references held by the snapshot).
        unsafe {
            let in_snap = e.snap.iter().find(|&&(k, _)| same_key(key, k)).map(|&(_, v)| v);
            let deviates = match event {
                2 => in_snap.is_some(),
                _ => in_snap != Some(new_value as usize),
            };
            let pos = e.dev.iter().position(|&k| same_key(key, k));
            match (deviates, pos) {
                (true, None) => {
                    if e.dev.len() >= MAX_DEVIATIONS {
                        e.broken = true;
                    } else {
                        e.dev.push(ffi::Py_NewRef(key) as usize);
                    }
                }
                (false, Some(i)) => {
                    // Decref of a str key: no Python code runs.
                    ffi::Py_DECREF(e.dev.swap_remove(i) as *mut _);
                }
                _ => {}
            }
        }
    }
    if !touched.is_empty() {
        touched.sort_by_key(|f| *f as *const DepFlag as usize);
        touched.dedup_by_key(|f| *f as *const DepFlag as usize);
        recompute_all(&t, &touched);
    }
    0
}

unsafe extern "C" fn on_type_event(ty: *mut ffi::PyTypeObject) -> c_int {
    // A class with attribute pins was modified. Attribute changes are seen precisely by the
    // dict watcher on its `__dict__`; here only an MRO change matters (`__bases__`
    // assignment): the pinned lookup path is no longer the one validated.
    let mut t = tables();
    let mut touched: Vec<&'static DepFlag> = Vec::new();
    for e in t.pins.iter_mut().filter(|e| e.ty == ty as usize) {
        // SAFETY: the entry holds a strong reference to the type.
        if unsafe { (*ty).tp_mro } as usize != e.mro {
            e.broken = true;
            touched.push(e.flag);
        }
    }
    recompute_all(&t, &touched);
    0
}

/// The process's (main interpreter's) dict watcher id, registered once. Every dict watch
/// of Rustique goes through it.
pub fn dict_watcher_id() -> PyResult<c_int> {
    let id = DICT_WATCHER.load(Ordering::Acquire);
    if id >= 0 {
        return Ok(id);
    }
    // SAFETY: attached thread (callers are attached).
    let id = unsafe { PyDict_AddWatcher(on_dict_event) };
    if id < 0 {
        return Err(Python::attach(PyErr::fetch));
    }
    DICT_WATCHER.store(id, Ordering::Release);
    Ok(id)
}

fn type_watcher_id() -> PyResult<c_int> {
    let id = TYPE_WATCHER.load(Ordering::Acquire);
    if id >= 0 {
        return Ok(id);
    }
    // SAFETY: attached thread.
    let id = unsafe { PyType_AddWatcher(on_type_event) };
    if id < 0 {
        return Err(Python::attach(PyErr::fetch));
    }
    TYPE_WATCHER.store(id, Ordering::Release);
    Ok(id)
}

/// Pre-registry API (prototype users): the dict watcher id.
pub fn watcher_id() -> PyResult<c_int> {
    dict_watcher_id()
}

fn watch_dict(py: Python<'_>, dict: *mut ffi::PyObject) -> PyResult<()> {
    let id = dict_watcher_id()?;
    // SAFETY: attached thread; live dict.
    if unsafe { ffi::PyDict_Watch(id, dict) } < 0 {
        return Err(PyErr::fetch(py));
    }
    Ok(())
}

/// Generation watch: bump `gen` (relaxed) whenever `dict[key]` is added, replaced or
/// deleted, or the dict is cleared or replaced wholesale. For dependencies that are
/// legitimately replaced at run time (`_strptime._TimeRE_cache`, `time.tzname`): kernels
/// cache the generation they validated at and revalidate by value when it moves.
/// Idempotent per `(dict, key, gen)`; lives as long as the process.
pub fn watch(dict: &Bound<'_, PyDict>, key: &str, gen_: &'static AtomicU64) -> PyResult<()> {
    let py = dict.py();
    let k = PyString::intern(py, key);
    {
        let t = tables();
        if t.gens.iter().any(|g| {
            g.dict == dict.as_ptr() as usize
                && std::ptr::eq(g.counter, gen_)
                // SAFETY: attached thread; live objects.
                && unsafe { same_key(k.as_ptr(), g.key) }
        }) {
            return Ok(());
        }
    }
    watch_dict(py, dict.as_ptr())?;
    tables().gens.push(GenWatch { dict: dict.clone().into_ptr() as usize, key: k.into_ptr() as usize, counter: gen_ });
    Ok(())
}

/// Activate `pins` under a new owner id; returns the owner id. Each pin's state is read now
/// (a change between `prepare` and install shows as a deviation).
pub(crate) fn activate(py: Python<'_>, pins: Vec<PendingPin>) -> PyResult<u64> {
    let owner = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
    if pins.is_empty() {
        return Ok(owner);
    }
    for p in &pins {
        match p {
            PendingPin::Key { dict, ty, .. } => {
                watch_dict(py, dict.as_ptr())?;
                if let Some(ty) = ty {
                    let tid = type_watcher_id()?;
                    // SAFETY: attached thread; live type.
                    if unsafe { PyType_Watch(tid, ty.as_ptr()) } < 0 {
                        return Err(PyErr::fetch(py));
                    }
                }
            }
            PendingPin::Dict { dict, .. } => watch_dict(py, dict.as_ptr())?,
        }
    }
    let mut new_pins = Vec::new();
    let mut new_snaps = Vec::new();
    let mut flags = Vec::new();
    for p in pins {
        flags.push(p.flag());
        match p {
            PendingPin::Key { dict, key, expected, flag, ty } => {
                let cur = dict.bind(py).get_item(key.bind(py))?;
                let exp = expected.as_ref().map_or(0, |o| o.as_ptr() as usize);
                let intact = cur.as_ref().map_or(0, |c| c.as_ptr() as usize) == exp;
                let (ty, mro) = match ty {
                    Some(t) => {
                        // SAFETY: live type object.
                        let mro = unsafe { (*t.as_ptr().cast::<ffi::PyTypeObject>()).tp_mro } as usize;
                        (t.into_ptr() as usize, mro)
                    }
                    None => (0, 0),
                };
                new_pins.push(Entry {
                    owner,
                    dict: dict.into_ptr() as usize,
                    key: key.into_ptr() as usize,
                    expected: expected.map_or(0, |o| o.into_ptr() as usize),
                    flag,
                    intact,
                    broken: false,
                    ty,
                    mro,
                });
            }
            PendingPin::Dict { dict, snap, flag } => {
                // Deviations of the current contents from the prepare-time snapshot.
                let d = dict.bind(py);
                let mut dev = Vec::new();
                for (k, v) in d.iter() {
                    let same = snap.iter().any(|(sk, sv)| {
                        sv.as_ptr() == v.as_ptr()
                            // SAFETY: attached thread; same_key runs no Python code (identity,
                            // then exact-str comparison).
                            && unsafe { same_key(k.as_ptr(), sk.as_ptr() as usize) }
                    });
                    if !same {
                        dev.push(k.into_ptr() as usize);
                    }
                }
                for (sk, _) in &snap {
                    if !d.contains(sk.bind(py))? {
                        dev.push(sk.clone_ref(py).into_ptr() as usize);
                    }
                }
                let broken = dev.len() > MAX_DEVIATIONS;
                new_snaps.push(Snapshot {
                    owner,
                    dict: dict.into_ptr() as usize,
                    flag,
                    snap: snap.into_iter().map(|(k, v)| (k.into_ptr() as usize, v.into_ptr() as usize)).collect(),
                    dev,
                    broken,
                });
            }
        }
    }
    let mut t = tables();
    t.pins.extend(new_pins);
    t.snaps.extend(new_snaps);
    for f in &flags {
        recompute(&t, f);
    }
    Ok(owner)
}

/// Report flips of `flags` to the registry (deopt while unpinned), or stop.
pub(crate) fn track(flags: &[&'static DepFlag], on: bool) {
    let mut t = tables();
    for f in flags {
        let a = *f as *const DepFlag as usize;
        if on {
            t.tracked.push(a);
        } else if let Some(i) = t.tracked.iter().position(|&x| x == a) {
            t.tracked.swap_remove(i);
        }
    }
}

/// Remove the pins of `owner`, unwatching dicts and types left without any watch.
pub(crate) fn deactivate(owner: u64) {
    let (pins, snaps): (Vec<Entry>, Vec<Snapshot>) = {
        let mut t = tables();
        let (gone, kept): (Vec<Entry>, Vec<Entry>) =
            std::mem::take(&mut t.pins).into_iter().partition(|e| e.owner == owner);
        t.pins = kept;
        let (sgone, skept): (Vec<Snapshot>, Vec<Snapshot>) =
            std::mem::take(&mut t.snaps).into_iter().partition(|e| e.owner == owner);
        t.snaps = skept;
        let flags: Vec<&'static DepFlag> = gone.iter().map(|e| e.flag).chain(sgone.iter().map(|e| e.flag)).collect();
        for f in flags {
            recompute(&t, f);
        }
        (gone, sgone)
    };
    let mut dicts: Vec<usize> = pins.iter().map(|e| e.dict).chain(snaps.iter().map(|e| e.dict)).collect();
    dicts.sort_unstable();
    dicts.dedup();
    let mut types: Vec<usize> = pins.iter().filter(|e| e.ty != 0).map(|e| e.ty).collect();
    types.sort_unstable();
    types.dedup();
    {
        let t = tables();
        let sys_modules = SYS_MODULES.load(Ordering::Relaxed);
        for &d in &dicts {
            let used = d == sys_modules
                || t.pins.iter().any(|e| e.dict == d)
                || t.snaps.iter().any(|e| e.dict == d)
                || t.gens.iter().any(|e| e.dict == d);
            let id = DICT_WATCHER.load(Ordering::Acquire);
            // SAFETY: attached thread; the dict is alive (strong references below).
            if !used && id >= 0 && unsafe { PyDict_Unwatch(id, d as *mut _) } < 0 {
                // SAFETY: attached thread.
                unsafe { ffi::PyErr_Clear() };
            }
        }
        for &ty in &types {
            let id = TYPE_WATCHER.load(Ordering::Acquire);
            // SAFETY: attached thread; the type is alive (strong references below).
            if !t.pins.iter().any(|e| e.ty == ty) && id >= 0 && unsafe { PyType_Unwatch(id, ty as *mut _) } < 0 {
                // SAFETY: attached thread.
                unsafe { ffi::PyErr_Clear() };
            }
        }
    }
    // SAFETY: attached thread (callers hold the registry lock); releasing the strong
    // references taken at activation, outside the tables lock (a value's finalizer may
    // run Python code).
    unsafe {
        for e in pins {
            ffi::Py_DECREF(e.dict as *mut _);
            ffi::Py_DECREF(e.key as *mut _);
            if e.expected != 0 {
                ffi::Py_DECREF(e.expected as *mut _);
            }
            if e.ty != 0 {
                ffi::Py_DECREF(e.ty as *mut _);
            }
        }
        for e in snaps {
            ffi::Py_DECREF(e.dict as *mut _);
            for (k, v) in e.snap {
                ffi::Py_DECREF(k as *mut _);
                ffi::Py_DECREF(v as *mut _);
            }
            for k in e.dev {
                ffi::Py_DECREF(k as *mut _);
            }
        }
    }
}

/// Pin state of `owner` for diagnostics: (key, absent-pin, intact, broken); snapshot pins
/// show as `("<dict>", false, no deviation, broken)`.
pub(crate) fn describe(py: Python<'_>, owner: u64) -> Vec<(String, bool, bool, bool)> {
    let t = tables();
    let mut out: Vec<(String, bool, bool, bool)> = t
        .pins
        .iter()
        .filter(|e| e.owner == owner)
        .map(|e| {
            // SAFETY: live str key (strong reference held by the entry).
            let k = unsafe { Bound::from_borrowed_ptr(py, e.key as *mut ffi::PyObject) }.to_string();
            (k, e.expected == 0, e.intact, e.broken)
        })
        .collect();
    out.extend(
        t.snaps.iter().filter(|e| e.owner == owner).map(|e| ("<dict>".to_owned(), false, e.dev.is_empty(), e.broken)),
    );
    out
}

/// Flags of `owner`'s pins.
pub(crate) fn flags_of(owner: u64) -> Vec<&'static DepFlag> {
    let t = tables();
    let mut v: Vec<&'static DepFlag> = t
        .pins
        .iter()
        .filter(|e| e.owner == owner)
        .map(|e| e.flag)
        .chain(t.snaps.iter().filter(|e| e.owner == owner).map(|e| e.flag))
        .collect();
    v.sort_by_key(|f| *f as *const DepFlag as usize);
    v.dedup_by_key(|f| *f as *const DepFlag as usize);
    v
}

/// Watch `sys.modules` for lazy activation (idempotent).
pub(crate) fn watch_sys_modules(py: Python<'_>, sys_modules: &Bound<'_, PyAny>) -> PyResult<()> {
    SYS_MODULES.store(sys_modules.as_ptr() as usize, Ordering::Relaxed);
    watch_dict(py, sys_modules.as_ptr())
}

/// Stop watching `sys.modules` (nothing pending), unless something else watches it.
pub(crate) fn unwatch_sys_modules() {
    let d = SYS_MODULES.load(Ordering::Relaxed);
    let id = DICT_WATCHER.load(Ordering::Acquire);
    if d != 0 && id >= 0 {
        {
            let t = tables();
            if t.pins.iter().any(|e| e.dict == d)
                || t.snaps.iter().any(|e| e.dict == d)
                || t.gens.iter().any(|e| e.dict == d)
            {
                return;
            }
        }
        // SAFETY: attached thread (callers hold the registry lock); `sys.modules` lives as
        // long as the interpreter.
        unsafe {
            if PyDict_Unwatch(id, d as *mut _) < 0 {
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
            key: PyString::intern(py, key).unbind(),
            expected: Some(value.unbind()),
            flag,
            ty: None,
        }],
    )?;
    Ok(())
}
