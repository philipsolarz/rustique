//! Cheap per-call guards deciding whether a call is in a kernel's exact subset.
//!
//! - [`Tools`]: is any `sys.monitoring` tool (that includes `sys.settrace`,
//!   `sys.setprofile`, cProfile, coverage's sysmon core) listening to `PY_START`? Then a
//!   kernel replacing *Python* code must run the original, so tools see CPython's frames.
//! - [`in_main_interpreter`]: kernels on process-global sites (static tables) that hold
//!   per-interpreter objects must fall back in other interpreters.
//! - [`DepWatch`]: "nobody replaced a dependency" (`json.encoder.c_make_encoder`,
//!   `posixpath._get_sep`, `os.fspath`, ...) as one relaxed load per call, kept up to date
//!   by a dict watcher instead of re-reading module globals on every call.

use std::cell::UnsafeCell;
use std::ffi::c_int;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};

use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PyMonitoringState {
    pub active: u8,
    pub opaque: u8,
}

unsafe extern "C" {
    fn PyMonitoring_EnterScope(
        state_array: *mut PyMonitoringState,
        version: *mut u64,
        event_types: *const u8,
        length: ffi::Py_ssize_t,
    ) -> c_int;
    fn PyDict_AddWatcher(
        cb: unsafe extern "C" fn(c_int, *mut ffi::PyObject, *mut ffi::PyObject, *mut ffi::PyObject) -> c_int,
    ) -> c_int;
}

pub const PY_MONITORING_EVENT_PY_START: u8 = 0;

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

/// No tool listens *locally* on this code object (`sys.monitoring.set_local_events`, what a
/// debugger breakpoint or a per-function profiler does). Reads `_co_monitoring` (NULL until a
/// tool ever instrumented the code) and its first member `local_monitors.tools[0..11]`
/// (internal layout, identical for the 11 local events in 3.14 and 3.15; gated by
/// `version::supported`).
///
/// # Safety
/// `code` is a live code object; layout validated by `version::code_layout_ok`.
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

static MAIN_INTERP: AtomicPtr<ffi::PyInterpreterState> = AtomicPtr::new(std::ptr::null_mut());

/// Record the interpreter that installed the kernels.
pub fn set_main_interpreter() {
    // SAFETY: attached thread.
    MAIN_INTERP.store(unsafe { ffi::PyInterpreterState_Get() }, Ordering::Relaxed);
}

/// Is the calling thread in the interpreter that installed the kernels?
///
/// # Safety
/// Attached thread.
#[inline(always)]
pub unsafe fn in_main_interpreter() -> bool {
    // SAFETY: attached thread.
    unsafe { ffi::PyInterpreterState_Get() == MAIN_INTERP.load(Ordering::Relaxed) }
}

// ---------------------------------------------------------------------------------------
// Dependency watch
// ---------------------------------------------------------------------------------------

/// "All dependencies of a target are the objects it was validated against."
#[derive(Debug)]
pub struct DepFlag(AtomicBool);

impl DepFlag {
    pub const fn new() -> Self {
        DepFlag(AtomicBool::new(false))
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

struct Entry {
    dict: usize,
    key: usize,      // strong ref to the key (str)
    expected: usize, // borrowed identity only (compared, never dereferenced)
    flag: &'static DepFlag,
    intact: bool,
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static WATCHER: AtomicI32 = AtomicI32::new(-1);
/// Extra observer for lazy patching (sys.modules / module dicts), see `proto`.
pub static EXTRA_CALLBACK: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

pub type ExtraCb = unsafe fn(c_int, *mut ffi::PyObject, *mut ffi::PyObject, *mut ffi::PyObject);

fn recompute(entries: &[Entry], flag: &'static DepFlag) {
    let all = entries.iter().filter(|e| std::ptr::eq(e.flag, flag)).all(|e| e.intact);
    flag.0.store(all, Ordering::Relaxed);
}

unsafe extern "C" fn on_dict_event(
    event: c_int,
    dict: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
    new_value: *mut ffi::PyObject,
) -> c_int {
    // Never raise, never run Python code: compare pointers and (for keys) string equality.
    let extra = EXTRA_CALLBACK.load(Ordering::Acquire);
    if !extra.is_null() {
        // SAFETY: set only to an `ExtraCb`.
        unsafe { std::mem::transmute::<*mut (), ExtraCb>(extra)(event, dict, key, new_value) };
    }
    let Ok(mut entries) = ENTRIES.lock() else { return 0 };
    let mut touched: Vec<&'static DepFlag> = Vec::new();
    for e in entries.iter_mut().filter(|e| e.dict == dict as usize) {
        let hit = match event {
            // ADDED, MODIFIED, DELETED
            0..=2 => {
                // SAFETY: both are str (module dict keys and our key); no Python code runs
                // for exact str comparison.
                !key.is_null()
                    && (key as usize == e.key || unsafe { ffi::PyUnicode_Compare(key, e.key as *mut _) } == 0)
            }
            // CLONED, CLEARED, DEALLOCATED
            _ => true,
        };
        if hit {
            e.intact = event <= 1 && new_value as usize == e.expected;
            touched.push(e.flag);
        }
    }
    // SAFETY: attached thread; PyUnicode_Compare does not set errors for two str objects.
    unsafe { ffi::PyErr_Clear() };
    for f in touched {
        recompute(&entries, f);
    }
    0
}

/// The process's (main interpreter's) dict watcher id, registered once.
pub fn watcher_id() -> PyResult<c_int> {
    let id = WATCHER.load(Ordering::Acquire);
    if id >= 0 {
        return Ok(id);
    }
    // SAFETY: attached thread (callers hold the GIL / are attached).
    let id = unsafe { PyDict_AddWatcher(on_dict_event) };
    if id < 0 {
        return Err(PyRuntimeError::new_err("no dict watcher id left"));
    }
    WATCHER.store(id, Ordering::Release);
    Ok(id)
}

/// Pin `dict[key]` to its current value for `flag`: the flag is true while every pinned
/// entry still holds the object it held here.
pub fn pin(dict: &Bound<'_, pyo3::types::PyDict>, key: &str, flag: &'static DepFlag) -> PyResult<()> {
    let py = dict.py();
    let id = watcher_id()?;
    let value = dict.get_item(key)?.ok_or_else(|| PyRuntimeError::new_err(format!("missing dependency {key}")))?;
    let k = pyo3::types::PyString::intern(py, key);
    // SAFETY: attached thread; a live dict.
    if unsafe { ffi::PyDict_Watch(id, dict.as_ptr()) } < 0 {
        return Err(PyErr::fetch(py));
    }
    let mut entries = ENTRIES.lock().map_err(|_| PyRuntimeError::new_err("poisoned"))?;
    entries.push(Entry {
        dict: dict.as_ptr() as usize,
        key: k.into_ptr() as usize,
        expected: value.as_ptr() as usize,
        flag,
        intact: true,
    });
    recompute(&entries, flag);
    Ok(())
}
