//! Generation watch (interim, until accel-core's shared dict watcher offers it): a dict
//! watcher bumps [`GEN`] whenever a watched dict entry (or any entry of a dict watched as a
//! whole) is added, replaced or removed, or the dict is cleared, cloned or deallocated.
//! Kernels cache the generation at which they last validated their dependencies by value
//! and compare it with one relaxed load per call.
//!
//! Ordering: CPython notifies watchers *before* applying the change. A validator therefore
//! reads [`GEN`] first and then reads each watched dict inside that dict's critical section
//! (free-threaded builds: the writer holds it across notify and store), so it either sees
//! the new value or validates under the old generation.

use std::ffi::c_int;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;

unsafe extern "C" {
    fn PyDict_AddWatcher(
        cb: unsafe extern "C" fn(c_int, *mut ffi::PyObject, *mut ffi::PyObject, *mut ffi::PyObject) -> c_int,
    ) -> c_int;
}

/// Bumped on every change of a watched entry. Starts at 1 so that 0 means "never validated".
pub static GEN: AtomicU64 = AtomicU64::new(1);

struct Entry {
    dict: usize,
    /// Strong reference to the key (an exact str), or 0 for "every key".
    key: usize,
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static WATCHER: AtomicI32 = AtomicI32::new(-1);

#[inline]
pub fn current() -> u64 {
    GEN.load(Ordering::Acquire)
}

unsafe extern "C" fn on_event(
    event: c_int,
    dict: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
    _new_value: *mut ffi::PyObject,
) -> c_int {
    // Never raise, never run Python code.
    let Ok(mut entries) = ENTRIES.lock() else { return 0 };
    let mut hit = false;
    for e in entries.iter().filter(|e| e.dict == dict as usize) {
        hit |= match event {
            // ADDED, MODIFIED, DELETED
            0..=2 => {
                e.key == 0
                    || key as usize == e.key
                    // SAFETY: both exact str: the comparison runs no Python code.
                    || (!key.is_null()
                        && unsafe { ffi::PyUnicode_CheckExact(key) } != 0
                        && unsafe { ffi::PyUnicode_Compare(key, e.key as *mut _) } == 0)
            }
            // CLONED, CLEARED, DEALLOCATED
            _ => true,
        };
    }
    if hit {
        GEN.fetch_add(1, Ordering::AcqRel);
    }
    if event == 5 {
        // DEALLOCATED: forget the dict (its address may be reused).
        entries.retain(|e| e.dict != dict as usize);
    }
    0
}

fn watcher_id() -> PyResult<c_int> {
    let id = WATCHER.load(Ordering::Acquire);
    if id >= 0 {
        return Ok(id);
    }
    // SAFETY: attached thread.
    let id = unsafe { PyDict_AddWatcher(on_event) };
    if id < 0 {
        return Err(PyRuntimeError::new_err("no dict watcher id left"));
    }
    WATCHER.store(id, Ordering::Release);
    Ok(id)
}

/// Watch `dict[key]` (`key = None`: every entry). Idempotent per (dict, key). All Python
/// operations happen before the entry lock is taken (the callback may wait on it while
/// attached; it must never wait for a thread that needs a stop-the-world).
pub fn watch(dict: &Bound<'_, PyDict>, key: Option<&str>) -> PyResult<()> {
    let py = dict.py();
    let id = watcher_id()?;
    let k = key.map(|k| pyo3::types::PyString::intern(py, k));
    // SAFETY: attached thread; a live dict.
    if unsafe { ffi::PyDict_Watch(id, dict.as_ptr()) } < 0 {
        return Err(PyErr::fetch(py));
    }
    let d = dict.as_ptr() as usize;
    let kp = k.as_ref().map_or(0, |k| k.as_ptr() as usize);
    {
        let mut entries = ENTRIES.lock().map_err(|_| PyRuntimeError::new_err("poisoned"))?;
        if entries.iter().any(|e| e.dict == d && e.key == kp) {
            return Ok(());
        }
        entries.push(Entry { dict: d, key: kp });
    }
    // The entry keeps the (interned) key alive.
    if let Some(k) = k {
        let _ = k.into_ptr();
    }
    // A new watch means "validate again".
    GEN.fetch_add(1, Ordering::AcqRel);
    Ok(())
}
