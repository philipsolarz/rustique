//! Raw CPython slots for `Dict`'s hottest operations (ARCHITECTURE §4.2, §6).
//!
//! PyO3 reaches every slot through a generic trampoline (panic guard, attach-count
//! bookkeeping in thread-local storage, argument extraction, result conversion): about 90
//! instructions per call, more than the lookup itself. For the operations that dominate
//! per-element code (`d[k]`, `k in d`, `d[k] = v`, `len(d)`, `d.get(k)`, iteration) the
//! `Dict` type gets small `extern "C"` slot functions that handle the common case — an exact
//! `int` or `str` key, no Python code to run — and hand everything else to the original
//! PyO3 implementation, which they keep and call unchanged. Semantics are therefore
//! defined by the PyO3 methods; the fast paths only skip work whose outcome is known.
//!
//! Rules for the fast paths:
//! - they run attached to the interpreter (CPython calls slots only then) but without
//!   PyO3's attach count, so they never drop a `Py<T>` (only `Bound`s and raw pointers, which
//!   decref directly), and never run Python code;
//! - they use the container lock through `sync.rs`'s public API (the same discipline as
//!   every other method) and release it before any decref;
//! - a panic falls back to the original implementation (which reports it as usual).
//!
//! The slots are installed once, when the module is initialised, before any instance or
//! subclass exists; Python subclasses inherit them like any C slot (and override them by
//! defining the dunder in Python).

use std::ffi::{CStr, c_int};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use pyo3::prelude::*;
use pyo3::{Borrowed, ffi};

use super::keys::FastKey;
use super::{Dict, DictIterator};

/// The PyO3 implementations the fast paths fall back to.
struct Originals {
    subscript: ffi::binaryfunc,
    ass_subscript: ffi::objobjargproc,
    contains: ffi::objobjproc,
    length: ffi::lenfunc,
    iternext: ffi::iternextfunc,
    iter: ffi::getiterfunc,
    init: ffi::initproc,
    /// The original `Dict.get` and `Dict.setdefault` method descriptors.
    get: Py<PyAny>,
    setdefault: Py<PyAny>,
}

static ORIGINALS: OnceLock<Originals> = OnceLock::new();

#[inline(always)]
fn originals() -> &'static Originals {
    // Installed before the type is exposed to Python (see `install`).
    match ORIGINALS.get() {
        Some(o) => o,
        None => unreachable!("rustique: Dict fast paths used before installation"),
    }
}

/// The `Dict` behind `slf`.
///
/// # Safety
/// `slf` must be a live instance of `Dict` (or a subclass), as CPython guarantees for the
/// `self` argument of a slot of `Dict`.
#[inline(always)]
unsafe fn dict_ref<'a>(py: Python<'_>, slf: *mut ffi::PyObject) -> &'a Dict {
    // SAFETY: guaranteed by the caller; the `Dict` lives as long as the object, which the
    // caller keeps alive for the duration of the slot call.
    unsafe {
        let b = Borrowed::from_ptr(py, slf).cast_unchecked::<Dict>();
        &*std::ptr::from_ref(b.get())
    }
}

/// Run a fast path; `None` (or a panic) means "use the original implementation".
#[inline(always)]
fn attempt<R>(f: impl FnOnce() -> Option<R>) -> Option<R> {
    catch_unwind(AssertUnwindSafe(f)).ok().flatten()
}

/// `d[k]`.
unsafe extern "C" fn subscript(slf: *mut ffi::PyObject, key: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: CPython calls slots attached, with a live `Dict` as `slf` and a live `key`.
    unsafe {
        let py = Python::assume_attached();
        let fast = attempt(|| {
            let probe = FastKey::of(key)?;
            let dict = dict_ref(py, slf);
            let inner = dict.inner.read(py);
            // A miss raises `KeyError`: leave that to the original implementation.
            let pos = inner.table.find_fast(&probe)??;
            inner.value_ptr(py, pos)
        });
        match fast {
            Some(value) => value,
            None => (originals().subscript)(slf, key),
        }
    }
}

/// `k in d`.
unsafe extern "C" fn contains(slf: *mut ffi::PyObject, key: *mut ffi::PyObject) -> c_int {
    // SAFETY: as for `subscript`.
    unsafe {
        let py = Python::assume_attached();
        let fast = attempt(|| {
            let probe = FastKey::of(key)?;
            let dict = dict_ref(py, slf);
            let inner = dict.inner.read(py);
            inner.table.find_fast(&probe)
        });
        match fast {
            Some(found) => c_int::from(found.is_some()),
            None => (originals().contains)(slf, key),
        }
    }
}

/// `d[k] = v` and `del d[k]`.
unsafe extern "C" fn ass_subscript(
    slf: *mut ffi::PyObject,
    key: *mut ffi::PyObject,
    value: *mut ffi::PyObject,
) -> c_int {
    // SAFETY: as for `subscript`; `value` is NULL for deletion, else a live object.
    unsafe {
        let py = Python::assume_attached();
        let dict = dict_ref(py, slf);
        let fast = attempt(|| {
            let probe = FastKey::of(key)?;
            if value.is_null() { dict.del_fast(py, &probe) } else { dict.set_fast(py, &probe, Borrowed::from_ptr(py, value)) }
        });
        if fast.is_some() {
            return 0;
        }
        (originals().ass_subscript)(slf, key, value)
    }
}

/// `len(d)`.
unsafe extern "C" fn length(slf: *mut ffi::PyObject) -> ffi::Py_ssize_t {
    // SAFETY: as for `subscript`.
    unsafe {
        let py = Python::assume_attached();
        let fast = attempt(|| {
            let len = dict_ref(py, slf).inner.read(py).table.len();
            ffi::Py_ssize_t::try_from(len).ok()
        });
        match fast {
            Some(len) => len,
            None => (originals().length)(slf),
        }
    }
}

/// `Dict.__init__`: a no-op right after `Dict(...)` (whose `__new__` consumed the same
/// argument tuple), the original otherwise.
unsafe extern "C" fn init(slf: *mut ffi::PyObject, args: *mut ffi::PyObject, kwargs: *mut ffi::PyObject) -> c_int {
    // SAFETY: CPython calls `tp_init` attached with a live instance of `Dict` (or subclass).
    unsafe {
        let py = Python::assume_attached();
        let pending = &dict_ref(py, slf).init_args;
        if pending.load(Ordering::Relaxed) != 0 && pending.swap(0, Ordering::Relaxed) == args as usize {
            return 0;
        }
        (originals().init)(slf, args, kwargs)
    }
}

/// `next(it)` for key, value and item iterators.
unsafe extern "C" fn iternext(slf: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: CPython calls `tp_iternext` attached with a live `DictIterator`.
    unsafe {
        let py = Python::assume_attached();
        let fast = attempt(|| {
            let it = Borrowed::from_ptr(py, slf).cast_unchecked::<DictIterator>();
            let it: &DictIterator = &*std::ptr::from_ref(it.get());
            Some(it.next_ptr(py))
        });
        match fast {
            Some(item) => item,
            None => (originals().iternext)(slf),
        }
    }
}

/// `iter(d)`.
unsafe extern "C" fn iter(slf: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: CPython calls `tp_iter` attached with a live `Dict` (or subclass).
    unsafe {
        let py = Python::assume_attached();
        let fast = attempt(|| {
            let dict = Borrowed::from_ptr(py, slf).cast_unchecked::<Dict>();
            let it = Dict::iter_of(&dict, super::ItemKind::Keys, false);
            Some(match Bound::new(py, it) {
                Ok(it) => it.into_ptr(),
                Err(err) => {
                    err.restore(py);
                    std::ptr::null_mut()
                }
            })
        });
        match fast {
            Some(it) => it,
            None => (originals().iter)(slf),
        }
    }
}

/// `d.get(k[, default])` (`METH_FASTCALL`).
unsafe extern "C" fn get(
    slf: *mut ffi::PyObject,
    args: *mut *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
) -> *mut ffi::PyObject {
    // SAFETY: CPython passes a live `Dict` (checked by the method descriptor) and `nargs`
    // live arguments.
    unsafe {
        let py = Python::assume_attached();
        if nargs == 1 || nargs == 2 {
            let key = *args;
            let fast = attempt(|| {
                let probe = FastKey::of(key)?;
                let dict = dict_ref(py, slf);
                let inner = dict.inner.read(py);
                match inner.table.find_fast(&probe)? {
                    Some(pos) => inner.value_ptr(py, pos),
                    None => {
                        let default = if nargs == 2 { *args.add(1) } else { ffi::Py_None() };
                        Some(ffi::Py_NewRef(default))
                    }
                }
            });
            if let Some(value) = fast {
                return value;
            }
        }
        call_original(&originals().get, slf, args, nargs)
    }
}

/// Call an original method descriptor with `self` prepended to `args`.
///
/// # Safety
/// Attached; `args` points to `nargs` live objects.
unsafe fn call_original(
    descr: &Py<PyAny>,
    slf: *mut ffi::PyObject,
    args: *mut *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
) -> *mut ffi::PyObject {
    // SAFETY: guaranteed by the caller.
    unsafe {
        let n = usize::try_from(nargs).unwrap_or(0);
        let call: Vec<*mut ffi::PyObject> = std::iter::once(slf).chain((0..n).map(|i| *args.add(i))).collect();
        ffi::PyObject_Vectorcall(descr.as_ptr(), call.as_ptr(), call.len(), std::ptr::null_mut())
    }
}

/// `d.setdefault(k[, default])` (`METH_FASTCALL`).
unsafe extern "C" fn setdefault(
    slf: *mut ffi::PyObject,
    args: *mut *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
) -> *mut ffi::PyObject {
    // SAFETY: as for `get`.
    unsafe {
        let py = Python::assume_attached();
        if nargs == 1 || nargs == 2 {
            let key = *args;
            let fast = attempt(|| {
                let probe = FastKey::of(key)?;
                let default = if nargs == 2 { *args.add(1) } else { ffi::Py_None() };
                dict_ref(py, slf).setdefault_fast(py, &probe, Borrowed::from_ptr(py, default))
            });
            if let Some(value) = fast {
                return value;
            }
        }
        call_original(&originals().setdefault, slf, args, nargs)
    }
}

/// A `PyMethodDef` that can live in a `static`.
struct MethodDef(ffi::PyMethodDef);

// SAFETY: the definition is immutable after construction and only read by CPython.
unsafe impl Sync for MethodDef {}

static GET_DOC: &CStr = c"get($self, key, default=None, /)\n--\n\nReturn the value for key if key is in the dictionary, else default.";

static GET_DEF: MethodDef = MethodDef(ffi::PyMethodDef {
    ml_name: c"get".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunctionFast: get },
    ml_flags: ffi::METH_FASTCALL,
    ml_doc: GET_DOC.as_ptr(),
});

static SETDEFAULT_DOC: &CStr = c"setdefault($self, key, default=None, /)\n--\n\nInsert key with a value of default if key is not in the dictionary.\n\nReturn the value for key if key is in the dictionary, else default.";

static SETDEFAULT_DEF: MethodDef = MethodDef(ffi::PyMethodDef {
    ml_name: c"setdefault".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunctionFast: setdefault },
    ml_flags: ffi::METH_FASTCALL,
    ml_doc: SETDEFAULT_DOC.as_ptr(),
});

/// Install the fast paths on `Dict` and `DictIterator`. Called once from module init.
pub(super) fn install(py: Python<'_>) -> PyResult<()> {
    if ORIGINALS.get().is_some() {
        return Ok(());
    }
    let dict_type = py.get_type::<Dict>();
    let iter_type = py.get_type::<DictIterator>();
    let tp = dict_type.as_type_ptr();
    let itp = iter_type.as_type_ptr();
    let get_name = pyo3::intern!(py, "get");
    let setdefault_name = pyo3::intern!(py, "setdefault");
    let original_get = dict_type.getattr(get_name)?.unbind();
    let original_setdefault = dict_type.getattr(setdefault_name)?.unbind();
    // SAFETY: `tp`/`itp` are the ready heap types PyO3 created for these classes; their slot
    // tables live inside the heap type objects and are writable. No instance or subclass
    // exists yet and module initialisation runs on one thread, so nothing reads the slots
    // while they change.
    unsafe {
        let mapping = (*tp).tp_as_mapping;
        let sequence = (*tp).tp_as_sequence;
        if mapping.is_null() || sequence.is_null() {
            return Err(pyo3::exceptions::PySystemError::new_err("rustique: Dict lacks mapping slots"));
        }
        let missing = || pyo3::exceptions::PySystemError::new_err("rustique: Dict slot missing");
        let originals = Originals {
            subscript: (*mapping).mp_subscript.ok_or_else(missing)?,
            ass_subscript: (*mapping).mp_ass_subscript.ok_or_else(missing)?,
            contains: (*sequence).sq_contains.ok_or_else(missing)?,
            length: (*mapping).mp_length.ok_or_else(missing)?,
            iternext: (*itp).tp_iternext.ok_or_else(missing)?,
            init: (*tp).tp_init.ok_or_else(missing)?,
            iter: (*tp).tp_iter.ok_or_else(missing)?,
            get: original_get,
            setdefault: original_setdefault,
        };
        if ORIGINALS.set(originals).is_err() {
            return Ok(());
        }
        (*mapping).mp_subscript = Some(subscript);
        (*mapping).mp_ass_subscript = Some(ass_subscript);
        (*mapping).mp_length = Some(length);
        (*sequence).sq_contains = Some(contains);
        if !(*sequence).sq_length.is_none() {
            (*sequence).sq_length = Some(length);
        }
        (*itp).tp_iternext = Some(iternext);
        (*tp).tp_init = Some(init);
        (*tp).tp_iter = Some(iter);

        let tp_dict = Borrowed::from_ptr(py, (*tp).tp_dict).cast_unchecked::<pyo3::types::PyDict>();
        for (name, def) in [(get_name, &GET_DEF), (setdefault_name, &SETDEFAULT_DEF)] {
            let descr = ffi::PyDescr_NewMethod(tp, std::ptr::from_ref(&def.0).cast_mut());
            tp_dict.set_item(name, Bound::from_owned_ptr_or_err(py, descr)?)?;
        }
        ffi::PyType_Modified(tp);
    }
    Ok(())
}
