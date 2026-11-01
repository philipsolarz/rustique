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

// SAFETY: plain addresses of objects kept alive by the entry's strong references.
unsafe impl Send for Entry {}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static DICT_WATCHER: AtomicI32 = AtomicI32::new(-1);
static TYPE_WATCHER: AtomicI32 = AtomicI32::new(-1);
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
/// `sys.modules` of the main interpreter (watched for lazy activation).
pub(crate) static SYS_MODULES: AtomicUsize = AtomicUsize::new(0);

/// A pin requested by a target's `prepare`, activated at install.
pub(crate) struct PendingPin {
    pub dict: Py<PyDict>,
    pub key: Py<PyString>,
    /// `None`: pin "absent".
    pub expected: Option<Py<PyAny>>,
    pub flag: &'static DepFlag,
    pub ty: Option<Py<PyType>>,
}

fn recompute(entries: &[Entry], flag: &'static DepFlag) {
    let all = entries.iter().filter(|e| std::ptr::eq(e.flag, flag)).all(|e| e.intact && !e.broken);
    flag.0.store(all, Ordering::Relaxed);
}

/// Lock the pin table. Held only for short, Python-free sections (never across a call that
/// could run Python code, release the GIL or block), so the watcher callbacks may wait on
/// it.
fn entries() -> std::sync::MutexGuard<'static, Vec<Entry>> {
    ENTRIES.lock().unwrap_or_else(|p| p.into_inner())
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
        return 0;
    }
    let mut entries = entries();
    let mut touched: Vec<&'static DepFlag> = Vec::new();
    for e in entries.iter_mut().filter(|e| e.dict == dict as usize) {
        match event {
            // ADDED, MODIFIED, DELETED
            0..=2 => {
                // SAFETY: exact-str comparison runs no Python code and cannot fail; other
                // key types fail the exact check first.
                let same = !key.is_null()
                    && (key as usize == e.key
                        || unsafe {
                            ffi::PyUnicode_CheckExact(key) != 0 && ffi::PyUnicode_Compare(key, e.key as *mut _) == 0
                        });
                if same {
                    e.intact = if event == 2 { e.expected == 0 } else { new_value as usize == e.expected };
                    touched.push(e.flag);
                }
            }
            // CLONED, CLEARED, DEALLOCATED: wholesale replacement; never re-arms.
            _ => {
                e.broken = true;
                touched.push(e.flag);
            }
        }
    }
    for f in touched {
        recompute(&entries, f);
    }
    0
}

unsafe extern "C" fn on_type_event(ty: *mut ffi::PyTypeObject) -> c_int {
    // A class with attribute pins was modified. Attribute changes are seen precisely by the
    // dict watcher on its `__dict__`; here only an MRO change matters (`__bases__`
    // assignment): the pinned lookup path is no longer the one validated.
    let mut entries = entries();
    let mut touched: Vec<&'static DepFlag> = Vec::new();
    for e in entries.iter_mut().filter(|e| e.ty == ty as usize) {
        // SAFETY: the entry holds a strong reference to the type.
        if unsafe { (*ty).tp_mro } as usize != e.mro {
            e.broken = true;
            touched.push(e.flag);
        }
    }
    for f in touched {
        recompute(&entries, f);
    }
    0
}

/// The process's (main interpreter's) dict watcher id, registered once.
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

/// Activate `pins` under a new owner id; returns the owner id. Each pin's state is read now
/// (a change between `prepare` and install shows as a broken pin).
pub(crate) fn activate(py: Python<'_>, pins: Vec<PendingPin>) -> PyResult<u64> {
    let owner = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
    if pins.is_empty() {
        return Ok(owner);
    }
    let did = dict_watcher_id()?;
    for p in &pins {
        // SAFETY: attached thread; live dict.
        if unsafe { ffi::PyDict_Watch(did, p.dict.as_ptr()) } < 0 {
            return Err(PyErr::fetch(py));
        }
        if let Some(ty) = &p.ty {
            let tid = type_watcher_id()?;
            // SAFETY: attached thread; live type.
            if unsafe { PyType_Watch(tid, ty.as_ptr()) } < 0 {
                return Err(PyErr::fetch(py));
            }
        }
    }
    let mut new = Vec::with_capacity(pins.len());
    for p in pins {
        let mut cur: *mut ffi::PyObject = std::ptr::null_mut();
        // SAFETY: attached thread; exact dict and str key: no Python code runs.
        let r = unsafe { ffi::PyDict_GetItemRef(p.dict.as_ptr(), p.key.as_ptr(), &mut cur) };
        if r < 0 {
            return Err(PyErr::fetch(py));
        }
        let expected = p.expected.as_ref().map_or(0, |o| o.as_ptr() as usize);
        let intact = cur as usize == expected;
        if !cur.is_null() {
            // SAFETY: the new reference returned by PyDict_GetItemRef.
            unsafe { ffi::Py_DECREF(cur) };
        }
        let (ty, mro) = match &p.ty {
            // SAFETY: live type object.
            Some(t) => {
                let mro = unsafe { (*t.as_ptr().cast::<ffi::PyTypeObject>()).tp_mro } as usize;
                (t.clone_ref(py).into_ptr() as usize, mro)
            }
            None => (0, 0),
        };
        new.push(Entry {
            owner,
            dict: p.dict.into_ptr() as usize,
            key: p.key.into_ptr() as usize,
            expected: p.expected.map_or(0, |o| o.into_ptr() as usize),
            flag: p.flag,
            intact,
            broken: false,
            ty,
            mro,
        });
    }
    let mut entries = entries();
    let flags: Vec<&'static DepFlag> = new.iter().map(|e| e.flag).collect();
    entries.extend(new);
    for f in flags {
        recompute(&entries, f);
    }
    Ok(owner)
}

/// Remove the pins of `owner`, unwatching dicts and types left without pins.
pub(crate) fn deactivate(owner: u64) {
    let removed: Vec<Entry> = {
        let mut entries = entries();
        let (gone, kept): (Vec<Entry>, Vec<Entry>) = entries.drain(..).partition(|e| e.owner == owner);
        *entries = kept;
        for e in &gone {
            recompute(&entries, e.flag);
        }
        gone
    };
    for e in removed {
        let (dict_used, ty_used) = {
            let entries = entries();
            (entries.iter().any(|x| x.dict == e.dict), e.ty != 0 && entries.iter().any(|x| x.ty == e.ty))
        };
        // SAFETY: attached thread (callers hold the registry lock); the entry's strong
        // references are released here.
        unsafe {
            if !dict_used && e.dict != SYS_MODULES.load(Ordering::Relaxed) {
                let id = DICT_WATCHER.load(Ordering::Acquire);
                if id >= 0 && PyDict_Unwatch(id, e.dict as *mut _) < 0 {
                    ffi::PyErr_Clear();
                }
            }
            if e.ty != 0 && !ty_used {
                let id = TYPE_WATCHER.load(Ordering::Acquire);
                if id >= 0 && PyType_Unwatch(id, e.ty as *mut _) < 0 {
                    ffi::PyErr_Clear();
                }
            }
            ffi::Py_DECREF(e.dict as *mut _);
            ffi::Py_DECREF(e.key as *mut _);
            if e.expected != 0 {
                ffi::Py_DECREF(e.expected as *mut _);
            }
            if e.ty != 0 {
                ffi::Py_DECREF(e.ty as *mut _);
            }
        }
    }
}

/// Pin state of `owner` for diagnostics: (key, absent-pin, intact, broken).
pub(crate) fn describe(py: Python<'_>, owner: u64) -> Vec<(String, bool, bool, bool)> {
    let entries = entries();
    entries
        .iter()
        .filter(|e| e.owner == owner)
        .map(|e| {
            // SAFETY: live str key (strong reference held by the entry).
            let k = unsafe { Bound::from_borrowed_ptr(py, e.key as *mut ffi::PyObject) }.to_string();
            (k, e.expected == 0, e.intact, e.broken)
        })
        .collect()
}

/// Watch `sys.modules` for lazy activation (idempotent).
pub(crate) fn watch_sys_modules(py: Python<'_>, sys_modules: &Bound<'_, PyAny>) -> PyResult<()> {
    let id = dict_watcher_id()?;
    SYS_MODULES.store(sys_modules.as_ptr() as usize, Ordering::Relaxed);
    // SAFETY: attached thread; live dict.
    if unsafe { ffi::PyDict_Watch(id, sys_modules.as_ptr()) } < 0 {
        return Err(PyErr::fetch(py));
    }
    Ok(())
}

/// Stop watching `sys.modules` (nothing pending).
pub(crate) fn unwatch_sys_modules() {
    let d = SYS_MODULES.load(Ordering::Relaxed);
    let id = DICT_WATCHER.load(Ordering::Acquire);
    if d != 0 && id >= 0 {
        let pinned = entries().iter().any(|e| e.dict == d);
        if pinned {
            return;
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
        vec![PendingPin {
            dict: dict.clone().unbind(),
            key: PyString::intern(py, key).unbind(),
            expected: Some(value.unbind()),
            flag,
            ty: None,
        }],
    )?;
    Ok(())
}
