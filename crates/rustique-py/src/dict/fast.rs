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
//! - a panic in a read-only fast path falls back to the original implementation (which
//!   reports it as usual); a panic in a fast path that may have changed something raises
//!   `SystemError` instead of retrying.
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
    /// The `Dict` type object (a static heap type: alive for the life of the process).
    dict_type: usize,
    subscript: ffi::binaryfunc,
    ass_subscript: ffi::objobjargproc,
    contains: ffi::objobjproc,
    length: ffi::lenfunc,
    iternext: ffi::iternextfunc,
    iter: ffi::getiterfunc,
    init: ffi::initproc,
    /// The original `Dict.get`, `setdefault`, `pop` and `popitem` method descriptors.
    get: Py<PyAny>,
    setdefault: Py<PyAny>,
    pop: Py<PyAny>,
    popitem: Py<PyAny>,
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

/// Run a read-only fast path; `None` (or a panic) means "use the original implementation".
#[inline(always)]
fn attempt<R>(f: impl FnOnce() -> Option<R>) -> Option<R> {
    catch_unwind(AssertUnwindSafe(f)).ok().flatten()
}

/// Outcome of a fast path that may change state (see [`attempt_mut`]).
enum Fast<R> {
    Done(R),
    Fallback,
    /// A bug: reported as `SystemError` (retrying could apply a change twice).
    Panicked,
}

/// Run a fast path that may change state before deciding: `None` means "nothing was
/// changed, use the original implementation".
#[inline(always)]
fn attempt_mut<R>(f: impl FnOnce() -> Option<R>) -> Fast<R> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Some(r)) => Fast::Done(r),
        Ok(None) => Fast::Fallback,
        Err(_) => Fast::Panicked,
    }
}

/// Raise the `SystemError` for a panicked fast path.
///
/// # Safety
/// Attached to the interpreter.
#[cold]
unsafe fn panicked() {
    // SAFETY: attached; static NUL-terminated message.
    unsafe { ffi::PyErr_SetString(ffi::PyExc_SystemError, c"rustique: internal error (panic) in a Dict fast path".as_ptr()) }
}

/// `d[k]`.
unsafe extern "C" fn subscript(slf: *mut ffi::PyObject, key: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: CPython calls slots attached, with a live `Dict` as `slf` and a live `key`.
    unsafe {
        let py = Python::assume_attached();
        let fast = attempt(|| {
            let dict = dict_ref(py, slf);
            let inner = dict.inner.read(py);
            // A miss raises `KeyError`: leave that to the original implementation.
            let pos = inner.table.find_object(key)??;
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
            let dict = dict_ref(py, slf);
            let inner = dict.inner.read(py);
            inner.table.find_object(key)
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
        let fast = attempt_mut(|| {
            if value.is_null() {
                dict.del_fast(py, &FastKey::of(key)?)
            } else {
                dict.set_fast(py, key, Borrowed::from_ptr(py, value))
            }
        });
        match fast {
            Fast::Done(()) => 0,
            Fast::Fallback => (originals().ass_subscript)(slf, key, value),
            Fast::Panicked => {
                panicked();
                -1
            }
        }
    }
}

/// `len(d)`.
unsafe extern "C" fn length(slf: *mut ffi::PyObject) -> ffi::Py_ssize_t {
    // SAFETY: as for `subscript`.
    unsafe {
        let py = Python::assume_attached();
        let fast = attempt(|| ffi::Py_ssize_t::try_from(dict_ref(py, slf).len(py)).ok());
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
        let fast = attempt_mut(|| {
            let it = Borrowed::from_ptr(py, slf).cast_unchecked::<DictIterator>();
            let it: &DictIterator = &*std::ptr::from_ref(it.get());
            Some(it.next_ptr(py))
        });
        match fast {
            Fast::Done(item) => item,
            Fast::Fallback => (originals().iternext)(slf),
            Fast::Panicked => {
                panicked();
                std::ptr::null_mut()
            }
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
                let dict = dict_ref(py, slf);
                let inner = dict.inner.read(py);
                match inner.table.find_object(key)? {
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

/// `d.pop(k[, default])` (`METH_FASTCALL`).
unsafe extern "C" fn pop(slf: *mut ffi::PyObject, args: *mut *mut ffi::PyObject, nargs: ffi::Py_ssize_t) -> *mut ffi::PyObject {
    // SAFETY: as for `get`.
    unsafe {
        let py = Python::assume_attached();
        if nargs == 1 || nargs == 2 {
            let fast = attempt_mut(|| {
                let default = (nargs == 2).then(|| *args.add(1));
                dict_ref(py, slf).pop_fast(py, *args, default)
            });
            match fast {
                Fast::Done(value) => return value,
                Fast::Fallback => {}
                Fast::Panicked => {
                    panicked();
                    return std::ptr::null_mut();
                }
            }
        }
        call_original(&originals().pop, slf, args, nargs)
    }
}

/// `d.popitem()` (`METH_NOARGS`).
unsafe extern "C" fn popitem(slf: *mut ffi::PyObject, _: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: CPython passes a live `Dict` (checked by the method descriptor).
    unsafe {
        let py = Python::assume_attached();
        match attempt_mut(|| dict_ref(py, slf).popitem_fast(py)) {
            Fast::Done(item) => item,
            Fast::Fallback => call_original(&originals().popitem, slf, std::ptr::null_mut(), 0),
            Fast::Panicked => {
                panicked();
                std::ptr::null_mut()
            }
        }
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
            let fast = attempt_mut(|| {
                let probe = FastKey::of(key)?;
                let default = if nargs == 2 { *args.add(1) } else { ffi::Py_None() };
                dict_ref(py, slf).setdefault_fast(py, &probe, Borrowed::from_ptr(py, default))
            });
            match fast {
                Fast::Done(value) => return value,
                Fast::Fallback => {}
                Fast::Panicked => {
                    panicked();
                    return std::ptr::null_mut();
                }
            }
        }
        call_original(&originals().setdefault, slf, args, nargs)
    }
}

/// `Dict(...)`: the type's vectorcall (`tp_vectorcall`, used by `CALL_BUILTIN_CLASS` and
/// every call of the type; subclasses do not inherit it). `Dict()` and `Dict(x)` build
/// directly, without an argument tuple or the `__new__`/`__init__` pair; anything else goes
/// through `type.__call__`.
unsafe extern "C" fn vectorcall(
    callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: CPython passes the type object, `nargs` live positional arguments followed by
    // the keyword values named by `kwnames` (or NULL).
    unsafe {
        let py = Python::assume_attached();
        let nargs = ffi::PyVectorcall_NARGS(nargsf);
        if kwnames.is_null() && nargs <= 1 && callable as usize == originals().dict_type {
            let fast = attempt_mut(|| {
                let arg = (nargs == 1).then(|| Borrowed::from_ptr(py, *args).to_owned());
                let built = super::Dict::build(py, crate::schema::any(), crate::schema::any(), arg.as_ref(), None)
                    .and_then(|d| Bound::new(py, d));
                Some(match built {
                    Ok(d) => d.into_ptr(),
                    Err(err) => {
                        err.restore(py);
                        std::ptr::null_mut()
                    }
                })
            });
            match fast {
                Fast::Done(d) => return d,
                Fast::Fallback => {}
                Fast::Panicked => {
                    panicked();
                    return std::ptr::null_mut();
                }
            }
        }
        // `type.__call__(Dict, *args, **kwargs)`: `__new__` then `__init__`.
        let nkw = if kwnames.is_null() { 0 } else { ffi::PyTuple_GET_SIZE(kwnames) };
        let tuple = ffi::PyTuple_New(nargs);
        if tuple.is_null() {
            return std::ptr::null_mut();
        }
        for i in 0..nargs {
            let item = *args.offset(i);
            ffi::PyTuple_SET_ITEM(tuple, i, ffi::Py_NewRef(item));
        }
        let mut kwargs = std::ptr::null_mut();
        if nkw > 0 {
            kwargs = ffi::PyDict_New();
            for i in 0..nkw {
                if kwargs.is_null()
                    || ffi::PyDict_SetItem(kwargs, ffi::PyTuple_GET_ITEM(kwnames, i), *args.offset(nargs + i)) < 0
                {
                    ffi::Py_XDECREF(kwargs);
                    ffi::Py_DECREF(tuple);
                    return std::ptr::null_mut();
                }
            }
        }
        let result = match ffi::PyType_Type.tp_call {
            Some(call) => call(callable, tuple, kwargs),
            None => std::ptr::null_mut(),
        };
        ffi::Py_DECREF(tuple);
        ffi::Py_XDECREF(kwargs);
        result
    }
}

/// `Dict._from_builtin(d)` (for the native tier's rewriter, TAKEOVER.md §9): build from a
/// fresh builtin `dict` temporary (its keys are adopted without lookups); anything else is
/// `cls(d)`.
unsafe extern "C" fn from_builtin(cls: *mut ffi::PyObject, arg: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: CPython passes the class (`METH_CLASS`) and one live argument (`METH_O`).
    unsafe {
        if cls as usize == originals().dict_type {
            return vectorcall(cls, &raw const arg, 1, std::ptr::null_mut());
        }
        ffi::PyObject_CallOneArg(cls, arg)
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

static FROM_BUILTIN_DEF: MethodDef = MethodDef(ffi::PyMethodDef {
    ml_name: c"_from_builtin".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunction: from_builtin },
    ml_flags: ffi::METH_O | ffi::METH_CLASS,
    ml_doc: c"_from_builtin($cls, d, /)\n--\n\nA Dict with the items of the builtin dict d (a fresh temporary).".as_ptr(),
});

static POP_DEF: MethodDef = MethodDef(ffi::PyMethodDef {
    ml_name: c"pop".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunctionFast: pop },
    ml_flags: ffi::METH_FASTCALL,
    ml_doc: c"pop($self, key, default=<unrepresentable>, /)\n--\n\nD.pop(k[,d]) -> v, remove specified key and return the corresponding value.\n\nIf the key is not found, return the default if given; otherwise,\nraise a KeyError.".as_ptr(),
});

static POPITEM_DEF: MethodDef = MethodDef(ffi::PyMethodDef {
    ml_name: c"popitem".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunction: popitem },
    ml_flags: ffi::METH_NOARGS,
    ml_doc: c"popitem($self, /)\n--\n\nRemove and return a (key, value) pair as a 2-tuple.\n\nPairs are returned in LIFO (last-in, first-out) order.\nRaises KeyError if the dict is empty.".as_ptr(),
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
    let pop_name = pyo3::intern!(py, "pop");
    let popitem_name = pyo3::intern!(py, "popitem");
    let original_pop = dict_type.getattr(pop_name)?.unbind();
    let original_popitem = dict_type.getattr(popitem_name)?.unbind();
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
            dict_type: tp as usize,
            subscript: (*mapping).mp_subscript.ok_or_else(missing)?,
            ass_subscript: (*mapping).mp_ass_subscript.ok_or_else(missing)?,
            contains: (*sequence).sq_contains.ok_or_else(missing)?,
            length: (*mapping).mp_length.ok_or_else(missing)?,
            iternext: (*itp).tp_iternext.ok_or_else(missing)?,
            init: (*tp).tp_init.ok_or_else(missing)?,
            iter: (*tp).tp_iter.ok_or_else(missing)?,
            get: original_get,
            setdefault: original_setdefault,
            pop: original_pop,
            popitem: original_popitem,
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
        for (name, def) in
            [(get_name, &GET_DEF), (setdefault_name, &SETDEFAULT_DEF), (pop_name, &POP_DEF), (popitem_name, &POPITEM_DEF)]
        {
            let descr = ffi::PyDescr_NewMethod(tp, std::ptr::from_ref(&def.0).cast_mut());
            tp_dict.set_item(name, Bound::from_owned_ptr_or_err(py, descr)?)?;
        }
        let descr = ffi::PyDescr_NewClassMethod(tp, std::ptr::from_ref(&FROM_BUILTIN_DEF.0).cast_mut());
        tp_dict.set_item(pyo3::intern!(py, "_from_builtin"), Bound::from_owned_ptr_or_err(py, descr)?)?;
        (*tp).tp_vectorcall = Some(vectorcall);
        ffi::PyType_Modified(tp);
    }
    Ok(())
}
