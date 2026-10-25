//! Raw CPython slots and methods for `List`'s per-element hot paths.
//!
//! PyO3's slot and method trampolines cost more than the work of `x[i]`, `x.append(v)` or
//! `len(x)` (thread-local attach bookkeeping, argument extraction, a type check of `self`,
//! an indirect call, error translation). The functions here are installed directly into
//! the type objects PyO3 created (see [`install`]): the slot pointers, and the type
//! dictionary's descriptors (`__getitem__` etc. become wrapper descriptors of these
//! functions, so explicit dunder calls and Python subclasses use them too). Methods that
//! CPython's specializer can call directly (`CALL_METHOD_DESCRIPTOR_O`/`_FAST`, which
//! require `type(self)` to be exactly the descriptor's class) become `METH_O` /
//! `METH_FASTCALL` method descriptors. On 3.15, `_tp_iteritem` gives `for` loops the
//! interpreter's virtual iteration (no iterator object at all).
//!
//! Rules for code in this module:
//! - Nothing unwinds into C: every entry point runs inside [`shield`], which turns a panic
//!   into `PanicException`.
//! - The thread is attached, but PyO3 does not know it (its attach count may be zero), so a
//!   `Py<T>` dropped here would be deferred to PyO3's reference pool instead of being
//!   decref'd. Hot paths therefore never drop a `Py<T>` (displaced objects are dropped as
//!   `Bound`, which decrefs immediately); everything else runs inside `Python::attach`
//!   ([`fallback`]), as do all error paths that carry Python objects.
//! - The usual three-phase discipline holds: no Python code runs while a storage guard is
//!   held.

use std::ffi::{CStr, c_int, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::ptr;

use pyo3::ffi;
use pyo3::panic::PanicException;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use rustique_core::kernels;
use rustique_core::slice::{SliceIndices, clamp_insert_index, normalize_index};

use super::{List, Side, Storage, compare_lengths, native_compare};
use crate::convert::{self, Probe};

type Obj = *mut ffi::PyObject;

unsafe extern "C" {
    /// `PyDescr_NewWrapper` (not in pyo3-ffi): a slot wrapper descriptor (`__getitem__`
    /// etc.) of `wrapped` with the slot definition `base`.
    fn PyDescr_NewWrapper(ty: *mut ffi::PyTypeObject, base: *mut ffi::wrapperbase, wrapped: *mut c_void) -> Obj;
}

// ---------------------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------------------

/// Run `f`; a panic becomes `PanicException` and the result `on_panic`.
#[inline(always)]
pub(crate) fn shield<R>(on_panic: R, f: impl FnOnce() -> R) -> R {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            report_panic(payload);
            on_panic
        }
    }
}

#[cold]
#[inline(never)]
fn report_panic(payload: Box<dyn std::any::Any + Send>) {
    let message = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic in rustique".to_owned()
    };
    Python::attach(|py| PanicException::new_err(message).restore(py));
    // The payload may own anything: drop it attached as well.
    Python::attach(|_| drop(payload));
}

/// Run `f` with PyO3's attach bookkeeping (so `Py` drops are immediate) and set its error.
#[cold]
#[inline(never)]
pub(crate) fn fallback<R>(error: R, f: impl FnOnce(Python<'_>) -> PyResult<R>) -> R {
    Python::attach(|py| match f(py) {
        Ok(r) => r,
        Err(e) => {
            e.restore(py);
            error
        }
    })
}

/// Set `exc` with a static message.
#[cold]
#[inline(never)]
pub(crate) fn raise(exc: Obj, message: &CStr) {
    // SAFETY: attached; `exc` is an exception type, `message` NUL-terminated.
    unsafe { ffi::PyErr_SetString(exc, message.as_ptr()) }
}

/// Set an error carried by a `PyErr`.
#[cold]
#[inline(never)]
pub(crate) fn restore(e: PyErr) {
    Python::attach(|py| e.restore(py));
}

/// The token for an attached thread (raw slots are only called attached).
#[inline(always)]
pub(crate) fn py<'py>() -> Python<'py> {
    // SAFETY: CPython calls slots and methods with the thread attached.
    unsafe { Python::assume_attached() }
}

/// The `List` value of a `List` instance (exact or subclass).
///
/// # Safety
/// `slf` must be a live instance of `List` (slots are only reachable through `List`'s type
/// and its subclasses; method descriptors check the type of `self`).
#[inline(always)]
pub(super) unsafe fn this<'a>(slf: Obj) -> &'a List {
    // SAFETY: see above.
    unsafe {
        let bound = pyo3::Borrowed::from_ptr_or_opt(py(), slf).unwrap_unchecked().cast_unchecked::<List>();
        &*std::ptr::from_ref(bound.get())
    }
}

/// `slf` as a `Bound<List>` (for the PyO3-level implementations).
///
/// # Safety
/// As [`this`].
#[inline(always)]
unsafe fn bound<'py>(py: Python<'py>, slf: Obj) -> Bound<'py, List> {
    // SAFETY: as above; a new strong reference.
    unsafe { Bound::from_borrowed_ptr(py, slf).cast_into_unchecked::<List>() }
}

/// # Safety
/// `p` must be a live object.
#[inline(always)]
unsafe fn any<'py>(py: Python<'py>, p: Obj) -> Bound<'py, PyAny> {
    // SAFETY: a new strong reference to a live object.
    unsafe { Bound::from_borrowed_ptr(py, p) }
}

/// The value of an exact, compact `int` (`|v| < 2**30`), read from its digits.
///
/// # Safety
/// `p` must be a live object.
#[inline(always)]
pub(crate) unsafe fn compact_int(p: Obj) -> Option<isize> {
    // SAFETY: the layout of `PyLongObject` (CPython 3.12+): the header, then `lv_tag`
    // (sign in bits 0-1, digit count from bit 3) and 30-bit digits.
    unsafe {
        if ffi::PyLong_CheckExact(p) == 0 {
            return None;
        }
        let value = p.cast::<u8>().add(size_of::<ffi::PyObject>()).cast::<usize>();
        let tag = *value;
        if tag >= (2 << 3) {
            return None;
        }
        let sign = 1 - (tag & 3) as isize;
        Some(sign * *value.add(1).cast::<u32>() as isize)
    }
}

/// An index argument (`list.pop`, `list.insert`): `operator.index(p)` as `Py_ssize_t`
/// (`OverflowError` when it does not fit); `None` with an exception set on failure.
///
/// # Safety
/// `p` must be a live object.
#[inline(always)]
unsafe fn index_arg(p: Obj) -> Option<isize> {
    // SAFETY: as above.
    unsafe { compact_int(p).or_else(|| index_arg_slow(p)) }
}

#[cold]
#[inline(never)]
unsafe fn index_arg_slow(p: Obj) -> Option<isize> {
    // SAFETY: C-API calls on a live object; `PyNumber_Index` may run `__index__` (no
    // Rustique lock is held by callers).
    unsafe {
        let index = ffi::PyNumber_Index(p);
        if index.is_null() {
            return None;
        }
        let v = ffi::PyLong_AsSsize_t(index);
        ffi::Py_DECREF(index);
        if v == -1 && !ffi::PyErr_Occurred().is_null() { None } else { Some(v) }
    }
}

#[inline(always)]
pub(crate) fn none() -> Obj {
    // SAFETY: `None` is immortal; a new reference.
    unsafe {
        let p = ffi::Py_None();
        ffi::Py_INCREF(p);
        p
    }
}

/// Store `value` into untyped storage where the strategy obviously fits, without the
/// generic classification: object storage keeps the object, `i64`/`f64` columns take an
/// exact `int`/`float` natively. `slot` is the index to replace, or `None` to append.
/// Returns the displaced object (a reference to release after unlocking), or `Err(())`
/// when the generic path must handle the value.
///
/// # Safety
/// `value` must be a live object; `slot` must be in range.
#[inline(always)]
unsafe fn store_fast(storage: &mut Storage, slot: Option<usize>, value: Obj) -> Result<Obj, ()> {
    use rustique_core::Column;
    // SAFETY: exact type checks before reading values; `value` is live.
    unsafe {
        match storage {
            Storage::Object(objects) => {
                ffi::Py_INCREF(value);
                let object = Bound::from_owned_ptr(py(), value).unbind();
                Ok(match slot {
                    Some(k) => std::mem::replace(&mut objects[k], object).into_ptr(),
                    None => {
                        objects.push(object);
                        ptr::null_mut()
                    }
                })
            }
            Storage::Native(Column::I64(v)) => {
                let x = match compact_int(value) {
                    Some(x) => x as i64,
                    None if ffi::PyLong_CheckExact(value) != 0 => {
                        let mut overflow = 0;
                        let x = ffi::PyLong_AsLongLongAndOverflow(value, &mut overflow);
                        if overflow != 0 {
                            return Err(());
                        }
                        x
                    }
                    None => return Err(()),
                };
                match slot {
                    Some(k) => v[k] = x,
                    None => v.push(x),
                }
                Ok(ptr::null_mut())
            }
            Storage::Native(Column::F64(v)) if ffi::PyFloat_CheckExact(value) != 0 => {
                let x = ffi::PyFloat_AS_DOUBLE(value);
                match slot {
                    Some(k) => v[k] = x,
                    None => v.push(x),
                }
                Ok(ptr::null_mut())
            }
            _ => Err(()),
        }
    }
}

/// Release a displaced reference (null is a no-op) after unlocking.
#[inline(always)]
fn release(p: Obj) {
    if !p.is_null() {
        // SAFETY: an owned reference handed back by `store_fast`.
        unsafe { ffi::Py_DECREF(p) }
    }
}

/// `TypeError` for a wrong number of positional arguments (CPython's `_PyArg_CheckPositional`).
#[cold]
#[inline(never)]
fn arg_count_error(name: &str, min: isize, max: isize, got: isize) {
    let message = if min == max {
        format!("{name} expected {min} argument{}, got {got}", if min == 1 { "" } else { "s" })
    } else if got < min {
        format!("{name} expected at least {min} argument{}, got {got}", if min == 1 { "" } else { "s" })
    } else {
        format!("{name} expected at most {max} argument{}, got {got}", if max == 1 { "" } else { "s" })
    };
    restore(pyo3::exceptions::PyTypeError::new_err(message));
}

// SAFETY (for every slot below): CPython calls them attached, with `slf` a live `List`
// (or subclass) instance and the other arguments live objects (or NULL where the slot's
// protocol allows it).

// ---------------------------------------------------------------------------------------
// Sequence / mapping slots
// ---------------------------------------------------------------------------------------

unsafe extern "C" fn sq_length(slf: Obj) -> ffi::Py_ssize_t {
    // SAFETY: see above.
    unsafe { this(slf) }.len() as ffi::Py_ssize_t
}

unsafe extern "C" fn nb_bool(slf: Obj) -> c_int {
    // SAFETY: see above.
    c_int::from(!unsafe { this(slf) }.is_empty())
}

/// `x[i]` for an in-range normalised or raw index; `None` when out of range.
#[inline(always)]
fn item_at(list: &List, i: isize) -> Option<Obj> {
    let py = py();
    let data = list.read(py);
    normalize_index(i, data.len()).map(|k| list.read_item(py, &data, k))
}

unsafe extern "C" fn mp_subscript(slf: Obj, key: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        // SAFETY: see above.
        if let Some(i) = unsafe { compact_int(key) } {
            return match item_at(unsafe { this(slf) }, i) {
                Some(item) => item,
                None => {
                    // SAFETY: reading a static exception type.
                    raise(unsafe { ffi::PyExc_IndexError }, c"list index out of range");
                    ptr::null_mut()
                }
            };
        }
        // SAFETY: see above.
        if unsafe { ffi::PySlice_Check(key) } != 0 {
            return get_slice(unsafe { this(slf) }, key);
        }
        // SAFETY: see above.
        fallback(ptr::null_mut(), |py| unsafe {
            Ok(List::__getitem__(&bound(py, slf), &any(py, key))?.into_ptr())
        })
    })
}

/// `x[start:stop:step]` as a new `List` of the same schema.
fn get_slice(list: &List, key: Obj) -> Obj {
    let py = py();
    let (mut start, mut stop, mut step) = (0, 0, 0);
    // SAFETY: `key` is a slice; this may run `__index__` (no lock is held) and raises
    // `ValueError` for a zero step.
    if unsafe { ffi::PySlice_Unpack(key, &mut start, &mut stop, &mut step) } < 0 {
        return ptr::null_mut();
    }
    let storage = {
        let data = list.read(py);
        match SliceIndices::new(Some(start), Some(stop), Some(step), data.len()) {
            Ok(s) => data.take_slice(py, &s),
            Err(_) => unreachable!("PySlice_Unpack rejects a zero step"),
        }
    };
    super::LIST_CLASS.alloc(py, List::new_with(list.schema.clone(), storage))
}

unsafe extern "C" fn sq_item(slf: Obj, i: ffi::Py_ssize_t) -> Obj {
    shield(ptr::null_mut(), || {
        // `PySequence_GetItem` already added the length to negative indices.
        let item = if i < 0 { None } else { item_at(unsafe { this(slf) }, i) };
        item.unwrap_or_else(|| {
            // SAFETY: reading a static exception type.
            raise(unsafe { ffi::PyExc_IndexError }, c"list index out of range");
            ptr::null_mut()
        })
    })
}

/// `x[i] = value` (`value` non-null) or `del x[i]` for an untyped or pure-typed list;
/// `None` when the slow path must handle it.
#[inline(always)]
fn assign_item(list: &List, i: isize, value: Obj) -> Option<c_int> {
    let py = py();
    if !value.is_null()
        && list.schema.is_any()
        && let Some(mut data) = list.write_fast(py)
        && let Some(k) = normalize_index(i, data.len())
        // SAFETY: a live value; `k` is in range.
        && let Ok(displaced) = unsafe { store_fast(&mut data, Some(k), value) }
    {
        // SAFETY: exclusive access (the write guard).
        unsafe { list.cache.invalidate(k) };
        list.version.bump_exclusive();
        drop(data);
        release(displaced);
        return Some(0);
    }
    let coerced = if value.is_null() || list.schema.is_any() {
        None
    } else if list.schema.is_pure() {
        // Pure schemas run no Python code (see `Schema::is_pure`).
        // SAFETY: a live object.
        // SAFETY: a live object.
        let object = unsafe { any(py, value) };
        match list.coerce_one(&object) {
            Ok(coerced) => Some(coerced),
            Err(e) => {
                restore(e);
                return Some(-1);
            }
        }
    } else {
        return None;
    };
    let displaced = {
        let mut data = match list.mutate(py) {
            Ok(data) => data,
            Err(e) => {
                // A rejected `Coerced::Object` is dropped attached.
                Python::attach(|py| {
                    drop(coerced);
                    e.restore(py);
                });
                return Some(-1);
            }
        };
        let Some(k) = normalize_index(i, data.len()) else {
            drop(data);
            Python::attach(|_| drop(coerced));
            // SAFETY: reading a static exception type.
            raise(unsafe { ffi::PyExc_IndexError }, c"list assignment index out of range");
            return Some(-1);
        };
        if value.is_null() {
            data.delete(k)
        } else {
            match coerced {
                Some(coerced) => data.set_one(py, k, coerced),
                None => {
                    // SAFETY: a live object.
                    let object = unsafe { any(py, value) };
                    data.set_any(py, k, &object)
                }
            }
        }
    };
    // Displaced objects are dropped unlocked, through `Bound` (an immediate decref).
    drop(displaced.map(|o| o.into_bound(py)));
    Some(0)
}

unsafe extern "C" fn mp_ass_subscript(slf: Obj, key: Obj, value: Obj) -> c_int {
    shield(-1, || {
        // SAFETY: see above.
        if let Some(i) = unsafe { compact_int(key) }
            && let Some(r) = assign_item(unsafe { this(slf) }, i, value)
        {
            return r;
        }
        // SAFETY: see above.
        fallback(-1, |py| unsafe {
            let list = bound(py, slf);
            if value.is_null() {
                list.get().__delitem__(py, &any(py, key))?;
            } else {
                List::__setitem__(&list, &any(py, key), &any(py, value))?;
            }
            Ok(0)
        })
    })
}

unsafe extern "C" fn sq_ass_item(slf: Obj, i: ffi::Py_ssize_t, value: Obj) -> c_int {
    shield(-1, || {
        // `PySequence_SetItem` already added the length to negative indices.
        if i < 0 {
            // SAFETY: reading a static exception type.
            raise(unsafe { ffi::PyExc_IndexError }, c"list assignment index out of range");
            return -1;
        }
        // SAFETY: see above.
        if let Some(r) = assign_item(unsafe { this(slf) }, i, value) {
            return r;
        }
        // SAFETY: see above.
        fallback(-1, |py| unsafe {
            let list = bound(py, slf);
            let key = i.into_pyobject(py)?.into_any();
            if value.is_null() {
                list.get().__delitem__(py, &key)?;
            } else {
                List::__setitem__(&list, &key, &any(py, value))?;
            }
            Ok(0)
        })
    })
}

unsafe extern "C" fn sq_contains(slf: Obj, value: Obj) -> c_int {
    shield(-1, || {
        let py = py();
        // SAFETY: see above.
        let list = unsafe { this(slf) };
        // SAFETY: a live object.
        let object = unsafe { any(py, value) };
        let probe = convert::probe(&object);
        {
            let data = list.read(py);
            match (&*data, &probe) {
                (Storage::Native(column), Probe::Native(needle)) => {
                    return c_int::from(kernels::find(column, needle, 0, column.len()).is_some());
                }
                (Storage::Native(_), Probe::Never) | (Storage::Empty, _) => return 0,
                _ => {}
            }
        }
        // Python equality (see `List::scan_equal`).
        let mut found = false;
        match list.scan_equal(py, &object, 0, usize::MAX, |_, _| {
            found = true;
            true
        }) {
            Ok(()) => c_int::from(found),
            Err(e) => {
                restore(e);
                -1
            }
        }
    })
}

// ---------------------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------------------

unsafe extern "C" fn tp_iter(slf: Obj) -> Obj {
    shield(ptr::null_mut(), || super::iter::new_iterator_ptr(slf, true))
}

/// Virtual iteration (3.15): the element at `index` and the next index, or null and
/// `index` once past the end (`-1` signals an error, never raised here).
#[cfg(Py_3_15)]
unsafe extern "C" fn iteritem(slf: Obj, index: ffi::Py_ssize_t) -> ffi::_PyObjectIndexPair {
    shield(ffi::_PyObjectIndexPair { object: ptr::null_mut(), index: -1 }, || {
        let py = py();
        // SAFETY: see above.
        let data = unsafe { this(slf) }.read(py);
        if (index as usize) < data.len() {
            let list = unsafe { this(slf) };
            ffi::_PyObjectIndexPair { object: list.read_item(py, &data, index as usize), index: index + 1 }
        } else {
            ffi::_PyObjectIndexPair { object: ptr::null_mut(), index }
        }
    })
}

// ---------------------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------------------

unsafe extern "C" fn tp_richcompare(slf: Obj, other: Obj, op: c_int) -> Obj {
    shield(ptr::null_mut(), || {
        let py = py();
        let compare_op = match op {
            ffi::Py_LT => pyo3::pyclass::CompareOp::Lt,
            ffi::Py_LE => pyo3::pyclass::CompareOp::Le,
            ffi::Py_EQ => pyo3::pyclass::CompareOp::Eq,
            ffi::Py_NE => pyo3::pyclass::CompareOp::Ne,
            ffi::Py_GT => pyo3::pyclass::CompareOp::Gt,
            _ => pyo3::pyclass::CompareOp::Ge,
        };
        // Fast path: two lists with native (or empty) storage.
        // SAFETY: see above.
        let list_type = py.get_type::<List>();
        if unsafe { ffi::PyObject_TypeCheck(other, list_type.as_type_ptr()) } != 0 && slf != other {
            let (a, b) = unsafe { (this(slf), this(other)) };
            let (da, db) = crate::sync::read_both(py, &a.data, &b.data);
            let r = if da.is_empty() || db.is_empty() {
                Some(compare_lengths(da.len(), db.len(), compare_op))
            } else if let (Storage::Native(ca), Storage::Native(cb)) = (&*da, &*db) {
                native_compare(ca, cb, compare_op)
            } else {
                None
            };
            if let Some(r) = r {
                // SAFETY: new references to the bool singletons.
                return unsafe {
                    let p = if r { ffi::Py_True() } else { ffi::Py_False() };
                    ffi::Py_INCREF(p);
                    p
                };
            }
            drop((da, db));
            // Python-level comparison (raw-slot safe: no `Py` drops, errors restored).
            return match List::richcompare_python(py, Side::List(a), Side::List(b), compare_op) {
                Ok(r) => r.into_ptr(),
                Err(e) => {
                    restore(e);
                    ptr::null_mut()
                }
            };
        }
        // SAFETY: see above.
        fallback(ptr::null_mut(), |py| unsafe {
            Ok(List::__richcmp__(&bound(py, slf), &any(py, other), compare_op)?.into_ptr())
        })
    })
}

// ---------------------------------------------------------------------------------------
// Arithmetic: `x + y`, `x * n`
// ---------------------------------------------------------------------------------------

/// PyO3's `nb_add`/`nb_multiply` (the general `__add__`/`__radd__`/`__mul__`/`__rmul__`
/// dispatch), used for every case the raw fast paths below do not handle.
static PYO3_NB_ADD: std::sync::atomic::AtomicPtr<()> = std::sync::atomic::AtomicPtr::new(ptr::null_mut());
static PYO3_NB_MULTIPLY: std::sync::atomic::AtomicPtr<()> = std::sync::atomic::AtomicPtr::new(ptr::null_mut());

#[inline(always)]
fn is_list(p: Obj) -> bool {
    // SAFETY: a type check on a live object against the (live) `List` type.
    unsafe { ffi::PyObject_TypeCheck(p, super::LIST_TYPE.load(std::sync::atomic::Ordering::Relaxed)) != 0 }
}

/// `x + y` for two lists of the same schema whose storages concatenate without conversion.
unsafe extern "C" fn nb_add(a: Obj, b: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        let py = py();
        if is_list(a) && is_list(b) {
            // SAFETY: both are lists.
            let (la, lb) = unsafe { (this(a), this(b)) };
            if std::sync::Arc::ptr_eq(&la.schema, &lb.schema) {
                let storage = if a == b {
                    let data = la.read(py);
                    data.concat(py, &data)
                } else {
                    let (da, db) = crate::sync::read_both(py, &la.data, &lb.data);
                    da.concat(py, &db)
                };
                if let Some(storage) = storage {
                    return super::LIST_CLASS.alloc(py, List::new_with(la.schema.clone(), storage));
                }
            }
        }
        // SAFETY: PyO3's own slot function for this type.
        unsafe {
            let f: ffi::binaryfunc = std::mem::transmute(PYO3_NB_ADD.load(std::sync::atomic::Ordering::Relaxed));
            f(a, b)
        }
    })
}

/// `x * n` / `n * x` for a compact `int` `n`.
unsafe extern "C" fn nb_multiply(a: Obj, b: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        let py = py();
        // SAFETY: type checks and reads on live objects.
        let (list, n) = unsafe {
            if is_list(a) && let Some(n) = compact_int(b) {
                (a, n)
            } else if is_list(b) && let Some(n) = compact_int(a) {
                (b, n)
            } else {
                let f: ffi::binaryfunc = std::mem::transmute(PYO3_NB_MULTIPLY.load(std::sync::atomic::Ordering::Relaxed));
                return f(a, b);
            }
        };
        // SAFETY: a list.
        let list = unsafe { this(list) };
        let repeated = list.read(py).repeat(py, n);
        match repeated {
            Ok(storage) => super::LIST_CLASS.alloc(py, List::new_with(list.schema.clone(), storage)),
            Err(e) => {
                restore(e);
                ptr::null_mut()
            }
        }
    })
}

/// `x.copy()`.
unsafe extern "C" fn copy(slf: Obj, _: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        let py = py();
        // SAFETY: see above.
        let list = unsafe { this(slf) };
        let storage = list.read(py).clone_ref(py);
        super::LIST_CLASS.alloc(py, List::new_with(list.schema.clone(), storage))
    })
}

// ---------------------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------------------

/// `List(iterable=())`: allocation and ingest in one step (bypassing `__new__`/`__init__`).
unsafe extern "C" fn tp_vectorcall(callable: Obj, args: *const Obj, nargsf: usize, kwnames: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        // Fast path: no argument, or one exact builtin list/tuple (raw-slot safe).
        // SAFETY: vectorcall protocol: `nargs` positional arguments, then the keywords.
        let nargs = unsafe { ffi::PyVectorcall_NARGS(nargsf) };
        // SAFETY: `kwnames` is NULL or a tuple.
        if kwnames.is_null() || unsafe { ffi::PyTuple_GET_SIZE(kwnames) } == 0 {
            let py = py();
            let storage = match nargs {
                0 => Some(Storage::Empty),
                1 => {
                    // SAFETY: one live positional argument.
                    let arg = unsafe { any(py, *args) };
                    convert::ingest_exact_seq_any(py, &arg)
                }
                _ => None,
            };
            if let Some(storage) = storage {
                return super::LIST_CLASS.alloc(py, List::new_with(crate::schema::any(), storage));
            }
        }
        fallback(ptr::null_mut(), |py| {
            // SAFETY: vectorcall protocol: `nargs` positional arguments, then the keywords.
            let nargs = unsafe { ffi::PyVectorcall_NARGS(nargsf) };
            // SAFETY: `kwnames` is NULL or a tuple.
            if !kwnames.is_null() && unsafe { ffi::PyTuple_GET_SIZE(kwnames) } != 0 {
                return Err(pyo3::exceptions::PyTypeError::new_err("List() takes no keyword arguments"));
            }
            // Only `List` itself has this slot (`tp_vectorcall` is not inherited).
            let _ = callable;
            let storage = match nargs {
                0 => Storage::Empty,
                1 => {
                    // SAFETY: one live positional argument.
                    let iterable = unsafe { any(py, *args) };
                    convert::ingest(py, &crate::schema::any(), &iterable)?
                }
                n => return Err(pyo3::exceptions::PyTypeError::new_err(format!("List expected at most 1 argument, got {n}"))),
            };
            Ok(List::from_storage(py, crate::schema::any(), storage)?.into_ptr())
        })
    })
}

/// `List.__init__(self, iterable=(), /)`.
unsafe extern "C" fn tp_init(slf: Obj, args: Obj, kwargs: Obj) -> c_int {
    shield(-1, || {
        // SAFETY: see above; `args` is a tuple, `kwargs` NULL or a dict.
        fallback(-1, |py| unsafe {
            let args = any(py, args).cast_into_unchecked::<PyTuple>();
            let kwargs = (!kwargs.is_null()).then(|| any(py, kwargs).cast_into_unchecked::<PyDict>());
            this(slf).init(py, &args, kwargs.as_ref())?;
            Ok(0)
        })
    })
}

// ---------------------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------------------

unsafe extern "C" fn append(slf: Obj, value: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        let py = py();
        // SAFETY: see above.
        let list = unsafe { this(slf) };
        // SAFETY: a live object.
        let object = unsafe { any(py, value) };
        if list.schema.is_any() {
            if let Some(mut data) = list.write_fast(py)
                // SAFETY: a live value.
                && unsafe { store_fast(&mut data, None, value) }.is_ok()
            {
                // SAFETY: exclusive access (the write guard).
                unsafe { list.cache.push() };
                list.publish_len(data.len());
                return none();
            }
            match list.mutate(py) {
                Ok(mut data) => data.push_any(py, &object),
                Err(e) => {
                    restore(e);
                    return ptr::null_mut();
                }
            }
            return none();
        }
        fallback(ptr::null_mut(), |py| {
            list.append(py, &object)?;
            Ok(none())
        })
    })
}

unsafe extern "C" fn pop(slf: Obj, args: *mut Obj, nargs: ffi::Py_ssize_t) -> Obj {
    shield(ptr::null_mut(), || {
        let py = py();
        let index = match nargs {
            0 => -1,
            // SAFETY: one live positional argument.
            1 => match unsafe { index_arg(*args) } {
                Some(i) => i,
                None => return ptr::null_mut(),
            },
            n => {
                arg_count_error("pop", 0, 1, n);
                return ptr::null_mut();
            }
        };
        // SAFETY: see above.
        let list = unsafe { this(slf) };
        // Fast path: `pop()` from native storage hands back the cached box if there is one.
        if let Some(mut data) = list.write_fast(py)
            && let Storage::Native(column) = &mut *data
            && let len = column.len()
            && len > 0
            && normalize_index(index, len) == Some(len - 1)
        {
            // SAFETY: exclusive access (the write guard).
            let item = unsafe { list.cache.pop(len) }
                .unwrap_or_else(|| crate::convert::column_item(py, column, len - 1).into_ptr());
            rustique_core::with_column!(column, v => drop(v.pop()));
            list.publish_len(len - 1);
            return item;
        }
        let mut data = match list.mutate(py) {
            Ok(data) => data,
            Err(e) => {
                restore(e);
                return ptr::null_mut();
            }
        };
        let len = data.len();
        if len == 0 {
            drop(data);
            // SAFETY: reading a static exception type.
            raise(unsafe { ffi::PyExc_IndexError }, c"pop from empty list");
            return ptr::null_mut();
        }
        let Some(k) = normalize_index(index, len) else {
            drop(data);
            // SAFETY: reading a static exception type.
            raise(unsafe { ffi::PyExc_IndexError }, c"pop index out of range");
            return ptr::null_mut();
        };
        data.remove(py, k).into_ptr()
    })
}

unsafe extern "C" fn insert(slf: Obj, args: *mut Obj, nargs: ffi::Py_ssize_t) -> Obj {
    shield(ptr::null_mut(), || {
        let py = py();
        if nargs != 2 {
            arg_count_error("insert", 2, 2, nargs);
            return ptr::null_mut();
        }
        // SAFETY: two live positional arguments.
        let (index, value) = unsafe { (*args, *args.add(1)) };
        // SAFETY: as above; may run `__index__` (no lock held).
        let Some(index) = (unsafe { index_arg(index) }) else {
            return ptr::null_mut();
        };
        // SAFETY: see above.
        let list = unsafe { this(slf) };
        // SAFETY: a live object.
        let object = unsafe { any(py, value) };
        if list.schema.is_any() {
            match list.mutate(py) {
                Ok(mut data) => {
                    let k = clamp_insert_index(index, data.len());
                    data.insert_any(py, k, &object);
                }
                Err(e) => {
                    restore(e);
                    return ptr::null_mut();
                }
            }
            return none();
        }
        fallback(ptr::null_mut(), |py| {
            list.insert(py, index, &object)?;
            Ok(none())
        })
    })
}

unsafe extern "C" fn extend(slf: Obj, iterable: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        // SAFETY: see above.
        fallback(ptr::null_mut(), |py| unsafe {
            this(slf).extend(py, &any(py, iterable))?;
            Ok(none())
        })
    })
}

// ---------------------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------------------

static mut APPEND_DEF: ffi::PyMethodDef = ffi::PyMethodDef {
    ml_name: c"append".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunction: append },
    ml_flags: ffi::METH_O,
    ml_doc: c"Append object to the end of the list.".as_ptr(),
};

static mut EXTEND_DEF: ffi::PyMethodDef = ffi::PyMethodDef {
    ml_name: c"extend".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunction: extend },
    ml_flags: ffi::METH_O,
    ml_doc: c"Extend list by appending elements from the iterable.".as_ptr(),
};

static mut POP_DEF: ffi::PyMethodDef = ffi::PyMethodDef {
    ml_name: c"pop".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunctionFast: pop },
    ml_flags: ffi::METH_FASTCALL,
    ml_doc: c"pop($self, index=-1, /)\n--\n\nRemove and return item at index (default last).\n\nRaises IndexError if list is empty or index is out of range."
        .as_ptr(),
};

static mut COPY_DEF: ffi::PyMethodDef = ffi::PyMethodDef {
    ml_name: c"copy".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunction: copy },
    ml_flags: ffi::METH_NOARGS,
    ml_doc: c"Return a shallow copy of the list.".as_ptr(),
};

static mut INSERT_DEF: ffi::PyMethodDef = ffi::PyMethodDef {
    ml_name: c"insert".as_ptr(),
    ml_meth: ffi::PyMethodDefPointer { PyCFunctionFast: insert },
    ml_flags: ffi::METH_FASTCALL,
    ml_doc: c"insert($self, index, object, /)\n--\n\nInsert object before index.".as_ptr(),
};

/// The type dictionary of `ty` (a new reference).
///
/// # Safety
/// `ty` must be a ready type object.
unsafe fn type_dict<'py>(py: Python<'py>, ty: *mut ffi::PyTypeObject) -> PyResult<Bound<'py, PyDict>> {
    // SAFETY: as above.
    unsafe { Ok(Bound::from_owned_ptr_or_err(py, ffi::PyType_GetDict(ty))?.cast_into_unchecked()) }
}

/// Replace the slot wrapper descriptor `name` in `dict` by one that wraps `wrapped` with the
/// same slot definition as the descriptor found in `source` (usually `dict` itself).
///
/// # Safety
/// `wrapped` must have the C signature of the slot that descriptor wraps.
unsafe fn rewrap(
    py: Python<'_>,
    ty: *mut ffi::PyTypeObject,
    dict: &Bound<'_, PyDict>,
    source: &Bound<'_, PyDict>,
    name: &str,
    wrapped: *mut c_void,
) -> PyResult<()> {
    let descr = source
        .get_item(name)?
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err(format!("rustique: no {name} slot to replace")))?;
    // SAFETY: type check, then the layout of a wrapper descriptor; the new descriptor shares
    // the (static) slot definition.
    unsafe {
        if ffi::Py_TYPE(descr.as_ptr()) != &raw mut ffi::PyWrapperDescr_Type {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(format!("rustique: {name} is not a slot wrapper")));
        }
        let base = (*descr.as_ptr().cast::<ffi::PyWrapperDescrObject>()).d_base;
        let new = Bound::from_owned_ptr_or_err(py, PyDescr_NewWrapper(ty, base, wrapped))?;
        dict.set_item(name, new)
    }
}

/// Replace method `def.ml_name` of `ty` by a method descriptor of `def`.
///
/// # Safety
/// `def` must be a valid, static method definition for instances of `ty`.
unsafe fn set_method(
    py: Python<'_>,
    ty: *mut ffi::PyTypeObject,
    dict: &Bound<'_, PyDict>,
    def: *mut ffi::PyMethodDef,
) -> PyResult<()> {
    // SAFETY: as above.
    unsafe {
        let descr = Bound::from_owned_ptr_or_err(py, ffi::PyDescr_NewMethod(ty, def))?;
        let name = CStr::from_ptr((*def).ml_name).to_str().unwrap_or_default();
        dict.set_item(name, descr)
    }
}

/// Replace the slot wrappers `name` of `ty` (a PyO3 class) by wrappers of the given raw
/// functions (each with the C signature of its slot).
///
/// # Safety
/// Module initialisation only; each function must match its slot's signature.
pub(crate) unsafe fn rewrap_slots(py: Python<'_>, ty: *mut ffi::PyTypeObject, pairs: &[(&str, *mut c_void)]) -> PyResult<()> {
    // SAFETY: as above.
    unsafe {
        let dict = type_dict(py, ty)?;
        for (name, f) in pairs {
            rewrap(py, ty, &dict, &dict, name, *f)?;
        }
    }
    Ok(())
}

/// Install the raw slots and methods on `List` and its iterator types.
pub(super) fn install(py: Python<'_>) -> PyResult<()> {
    let list_type = py.get_type::<List>();
    let ty = list_type.as_type_ptr();
    // SAFETY: called once at module initialisation, before any instance exists or any
    // other code can observe the types; each function has its slot's C signature, and
    // heap types own their `tp_as_*` tables.
    unsafe {
        let dict = type_dict(py, ty)?;
        let fp = |f: *const ()| f as *mut c_void;
        rewrap(py, ty, &dict, &dict, "__len__", fp(sq_length as *const ()))?;
        rewrap(py, ty, &dict, &dict, "__bool__", fp(nb_bool as *const ()))?;
        rewrap(py, ty, &dict, &dict, "__getitem__", fp(mp_subscript as *const ()))?;
        rewrap(py, ty, &dict, &dict, "__setitem__", fp(mp_ass_subscript as *const ()))?;
        rewrap(py, ty, &dict, &dict, "__delitem__", fp(mp_ass_subscript as *const ()))?;
        rewrap(py, ty, &dict, &dict, "__contains__", fp(sq_contains as *const ()))?;
        rewrap(py, ty, &dict, &dict, "__iter__", fp(tp_iter as *const ()))?;
        for name in ["__lt__", "__le__", "__eq__", "__ne__", "__gt__", "__ge__"] {
            rewrap(py, ty, &dict, &dict, name, fp(tp_richcompare as *const ()))?;
        }
        let builtin_list_dict = type_dict(py, &raw mut ffi::PyList_Type)?;
        rewrap(py, ty, &dict, &builtin_list_dict, "__init__", fp(tp_init as *const ()))?;
        set_method(py, ty, &dict, &raw mut APPEND_DEF)?;
        set_method(py, ty, &dict, &raw mut EXTEND_DEF)?;
        set_method(py, ty, &dict, &raw mut POP_DEF)?;
        set_method(py, ty, &dict, &raw mut INSERT_DEF)?;
        set_method(py, ty, &dict, &raw mut COPY_DEF)?;

        let seq = (*ty).tp_as_sequence;
        (*seq).sq_length = Some(sq_length);
        (*seq).sq_item = Some(sq_item);
        (*seq).sq_ass_item = Some(sq_ass_item);
        (*seq).sq_contains = Some(sq_contains);
        let map = (*ty).tp_as_mapping;
        (*map).mp_subscript = Some(mp_subscript);
        (*map).mp_ass_subscript = Some(mp_ass_subscript);
        let num = (*ty).tp_as_number;
        (*num).nb_bool = Some(nb_bool);
        // `x + y` / `x * n`: raw fast paths in front of PyO3's dispatch (the dict's
        // `__add__`/`__radd__`/`__mul__`/`__rmul__` wrappers stay PyO3's).
        if let (Some(add), Some(mul)) = ((*num).nb_add, (*num).nb_multiply) {
            PYO3_NB_ADD.store(add as *mut (), std::sync::atomic::Ordering::Relaxed);
            PYO3_NB_MULTIPLY.store(mul as *mut (), std::sync::atomic::Ordering::Relaxed);
            (*num).nb_add = Some(nb_add);
            (*num).nb_multiply = Some(nb_multiply);
        }
        (*ty).tp_iter = Some(tp_iter);
        (*ty).tp_richcompare = Some(tp_richcompare);
        (*ty).tp_init = Some(tp_init);
        (*ty).tp_vectorcall = Some(tp_vectorcall);
        #[cfg(Py_3_15)]
        {
            (*ty)._tp_iteritem = Some(iteritem);
        }
        ffi::PyType_Modified(ty);

    }
    Ok(())
}
