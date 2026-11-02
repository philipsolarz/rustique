//! How the tier sees collections without an entry in `gc.callbacks`.
//!
//! CPython invokes the list that `gcstate->callbacks` points to (`invoke_gc_callback`,
//! `Python/gc.c`, `Python/gc_free_threading.c`); the `gc` module's `callbacks` attribute is
//! that same list object, bound once when the module executes. The tier points
//! `gcstate->callbacks` at a private one-element list `[observer]` and keeps the original
//! list (the *program's* list, whatever the module attribute is later rebound to). For every
//! phase of every collection CPython calls the observer with its phase string and info
//! dict; the observer updates the policy (`policy::on_phase`) and then calls every entry of
//! the program's list exactly as `invoke_gc_callback` would have: by index, re-reading the
//! length before each entry (mutations during the loop behave the same), with the same
//! `(phase, info)` argument array, reporting a raising callback with
//! `PyErr_FormatUnraisable("Exception ignored while calling GC callback %R", cb)` and
//! carrying on. So `gc.callbacks` is the program's list, unchanged in identity, length,
//! contents and reference count.
//!
//! The swap is made with the collector's `collecting` flag held (no collection, and
//! therefore no callback invocation, can be in flight): at install, at `disable()` and at
//! exit, never while CPython iterates the private list. When the flag cannot be taken (a
//! `disable()` from inside a GC callback, or a concurrent collection on a free-threaded
//! build) the observer stays in place and only forwards (policy passive) until the next
//! restore.

use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;

use super::layout::Layout;
use super::policy;

unsafe extern "C" {
    fn PyErr_FormatUnraisable(format: *const std::ffi::c_char, ...);
}

/// The program's list (the one `gcstate->callbacks` held at the swap): its reference moved
/// from the collector state to here while the private list is installed.
static PROGRAM: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
/// `[observer]` (created once, never freed).
static PRIVATE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static OBSERVER: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static SWAPPED: AtomicBool = AtomicBool::new(false);

const FT: bool = cfg!(Py_GIL_DISABLED);

static mut OBSERVER_DEF: ffi::PyMethodDef = ffi::PyMethodDef {
    ml_name: c"_rustique_gc_observer".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunctionFast: observe },
    ml_flags: ffi::METH_FASTCALL,
    ml_doc: std::ptr::null(),
};

pub(super) fn swapped() -> bool {
    SWAPPED.load(Ordering::Acquire)
}

/// The program's callbacks list while the private list is installed (borrowed).
pub(super) fn program_list() -> *mut ffi::PyObject {
    PROGRAM.load(Ordering::Acquire)
}

pub(super) fn private_list() -> *mut ffi::PyObject {
    PRIVATE.load(Ordering::Acquire)
}

/// The observer, called by CPython with `(phase, info)` (METH_FASTCALL).
unsafe extern "C" fn observe(
    _module: *mut ffi::PyObject,
    args: *mut *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
) -> *mut ffi::PyObject {
    // SAFETY: called by CPython's `invoke_gc_callback` on an attached thread with the
    // two-element argument array it passes to every callback.
    unsafe {
        if nargs == 2 {
            let (phase, info) = (*args, *args.add(1));
            let py = Python::assume_attached();
            policy::on_phase(py, phase, info);
            repair_module_attribute(py);
            forward(args.cast_const());
        }
        ffi::Py_NewRef(ffi::Py_None())
    }
}

/// `invoke_gc_callback`'s loop, over the program's list.
///
/// # Safety
/// Attached thread; `args` is CPython's `(phase, info)` array.
unsafe fn forward(args: *const *mut ffi::PyObject) {
    let list = PROGRAM.load(Ordering::Acquire);
    if list.is_null() {
        return;
    }
    // SAFETY: `list` is kept alive by PROGRAM's reference; per the caller otherwise.
    unsafe {
        let mut i: ffi::Py_ssize_t = 0;
        loop {
            let cb = if FT {
                // gc_free_threading.c: `_PyList_GetItemRef`, stop on NULL.
                if i >= ffi::PyList_Size(list) {
                    break;
                }
                let cb = ffi::PyList_GetItemRef(list, i);
                if cb.is_null() {
                    ffi::PyErr_Clear();
                    break;
                }
                cb
            } else {
                // gc.c: `PyList_GET_ITEM` + `Py_INCREF`.
                if i >= ffi::PyList_GET_SIZE(list) {
                    break;
                }
                ffi::Py_NewRef(ffi::PyList_GET_ITEM(list, i))
            };
            let r = ffi::PyObject_Vectorcall(cb, args, 2, std::ptr::null_mut());
            if r.is_null() {
                PyErr_FormatUnraisable(c"Exception ignored while calling GC callback %R".as_ptr(), cb);
            } else {
                ffi::Py_DECREF(r);
            }
            ffi::Py_DECREF(cb);
            i += 1;
        }
    }
}

/// A re-executed `gc` module (`importlib.reload(gc)`, or a fresh import after
/// `del sys.modules['gc']`) binds its `callbacks` attribute to whatever
/// `gcstate->callbacks` holds: put the program's list back there (where plain CPython would
/// have bound it), moving entries the program appended to the private list meanwhile.
///
/// # Safety
/// Attached thread, inside the observer (CPython iterates the private list at index 0).
unsafe fn repair_module_attribute(py: Python<'_>) {
    let private = PRIVATE.load(Ordering::Acquire);
    let program = PROGRAM.load(Ordering::Acquire);
    if private.is_null() || program.is_null() {
        return;
    }
    // Only PRIVATE and the collector state hold the private list; a third reference is a
    // module namespace that bound it (one load instead of three dict lookups per phase).
    // SAFETY: a live object (PRIVATE's reference).
    if unsafe { ffi::Py_REFCNT(private) } <= 2 {
        return;
    }
    // SAFETY: plain dict reads on the interpreter's modules dict and a module's dict (no
    // Python code: exact dicts, str keys).
    unsafe {
        let modules = ffi::PyImport_GetModuleDict();
        if modules.is_null() {
            return;
        }
        let mut m: *mut ffi::PyObject = std::ptr::null_mut();
        if ffi::PyDict_GetItemRef(modules, pyo3::intern!(py, "gc").as_ptr(), &mut m) <= 0 {
            ffi::PyErr_Clear();
            return;
        }
        if ffi::PyModule_Check(m) != 0 {
            let d = ffi::PyModule_GetDict(m);
            let mut cur: *mut ffi::PyObject = std::ptr::null_mut();
            let key = pyo3::intern!(py, "callbacks").as_ptr();
            if ffi::PyDict_GetItemRef(d, key, &mut cur) > 0 {
                if cur == private {
                    // Entries after the observer were appended by the program: CPython would
                    // have put them in its own list, in this order.
                    let n = ffi::PyList_Size(private);
                    if n > 1 {
                        let extra = ffi::PyList_GetSlice(private, 1, n);
                        if !extra.is_null() {
                            let end = ffi::PyList_Size(program);
                            ffi::PyList_SetSlice(program, end, end, extra);
                            ffi::Py_DECREF(extra);
                        }
                        ffi::PyList_SetSlice(private, 1, n, std::ptr::null_mut());
                    }
                    ffi::PyDict_SetItem(d, key, program);
                }
                ffi::Py_DECREF(cur);
            }
            ffi::PyErr_Clear();
        }
        ffi::Py_DECREF(m);
    }
}

/// Create the observer and the private list (once per process). Both are untracked: they
/// never appear in `gc.get_objects()` / `gc.get_referrers()`.
fn ensure_private(_py: Python<'_>) -> Result<*mut ffi::PyObject, String> {
    let p = PRIVATE.load(Ordering::Acquire);
    if !p.is_null() {
        return Ok(p);
    }
    // SAFETY: attached thread; a static method def that lives as long as the process.
    unsafe {
        let obs = ffi::PyCFunction_NewEx(&raw mut OBSERVER_DEF, std::ptr::null_mut(), std::ptr::null_mut());
        if obs.is_null() {
            ffi::PyErr_Clear();
            return Err("cannot create the observer".into());
        }
        let list = ffi::PyList_New(1);
        if list.is_null() {
            ffi::PyErr_Clear();
            ffi::Py_DECREF(obs);
            return Err("cannot create the observer list".into());
        }
        // Steals a reference to `obs`; OBSERVER keeps a borrowed pointer.
        ffi::PyList_SetItem(list, 0, obs);
        ffi::PyObject_GC_UnTrack(obs.cast());
        ffi::PyObject_GC_UnTrack(list.cast());
        OBSERVER.store(obs, Ordering::Release);
        PRIVATE.store(list, Ordering::Release);
        Ok(list)
    }
}

/// Point `gcstate->callbacks` at the private list (idempotent). Fails when a collection is
/// running (an `enable()` from inside a GC callback, or a concurrent collection on a
/// free-threaded build).
pub(super) fn install(py: Python<'_>, l: &Layout) -> Result<(), String> {
    if SWAPPED.load(Ordering::Acquire) {
        return Ok(());
    }
    let private = ensure_private(py)?;
    // On a free-threaded build another thread may be collecting: wait for it (up to 0.2 s).
    // On the GIL build a held flag means `enable()` runs inside a GC callback: refuse now.
    let mut tries = if FT { 200 } else { 1 };
    while !l.try_hold_collecting() {
        tries -= 1;
        if tries == 0 {
            return Err("a garbage collection is running".into());
        }
        // Detach while waiting so the collecting thread can finish.
        py.detach(|| std::thread::sleep(std::time::Duration::from_millis(1)));
    }
    let cur = l.ptr(l.off.callbacks);
    // SAFETY: attached thread; type check of the live list the collector holds.
    let ok = !cur.is_null() && unsafe { ffi::PyList_CheckExact(cur) } != 0 && cur != private;
    if ok {
        // The collector's reference to its list moves to PROGRAM; the collector gets a new
        // reference to the private list (it releases it in `_PyGC_Fini` if never restored).
        PROGRAM.store(cur, Ordering::Release);
        // SAFETY: attached thread; a live object.
        unsafe { ffi::Py_INCREF(private) };
        l.set_ptr(l.off.callbacks, private);
        SWAPPED.store(true, Ordering::Release);
    }
    l.release_collecting();
    if ok { Ok(()) } else { Err("gcstate->callbacks is not the expected list".into()) }
}

/// Put the program's list back into `gcstate->callbacks`. Returns false when
/// that is not possible now (a collection is running): the observer then keeps forwarding.
pub(super) fn uninstall(_py: Python<'_>, l: &Layout) -> bool {
    if !SWAPPED.load(Ordering::Acquire) {
        return true;
    }
    if !l.try_hold_collecting() {
        return false;
    }
    let private = PRIVATE.load(Ordering::Acquire);
    let done = l.ptr(l.off.callbacks) == private;
    if done {
        l.set_ptr(l.off.callbacks, PROGRAM.load(Ordering::Acquire));
        PROGRAM.store(std::ptr::null_mut(), Ordering::Release);
        SWAPPED.store(false, Ordering::Release);
        // SAFETY: attached thread; the collector's reference to the private list (PRIVATE
        // keeps its own, so this never frees it).
        unsafe { ffi::Py_DECREF(private) };
    }
    l.release_collecting();
    done
}
