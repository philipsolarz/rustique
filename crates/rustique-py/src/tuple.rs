//! The `Tuple` class: an immutable sequence with native storage strategies and builtin
//! `tuple` semantics.
//!
//! OWNER: list agent.
//!
//! Immutability makes `Tuple` lock-free: the storage is fixed at construction, so every
//! reader on every thread reads it directly (no lock, no mutation counter). Elements are
//! stored like an untyped `List`'s (`i64`, `f64`, `bool`, `str` columns or objects), with
//! one difference: a float column never holds a NaN (such tuples store objects), so element
//! identity, equality and hashing are exactly the builtin's (`hash(Tuple(t)) == hash(t)`
//! bit for bit, using CPython's xxHash-based tuple hash, cached like CPython's).
//!
//! Hot paths are raw C slots (as for `List`, see `list::slots`): `len`, `t[i]`, `in`,
//! iteration (virtual iteration on 3.15), hashing, comparison and construction.

use std::ffi::c_int;
use std::ptr;
use std::sync::atomic::{AtomicIsize, AtomicPtr, Ordering};

use pyo3::exceptions::{PyIndexError, PyTypeError, PyValueError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::pyclass::CompareOp;
use pyo3::types::{PyDict, PyTuple, PyType};
use pyo3::{PyTraverseError, ffi, intern};
use rustique_core::kernels;
use rustique_core::slice::{SliceIndices, normalize_index};
use rustique_core::{Column, pyhash};

use crate::convert::{self, Probe};
use crate::list::alloc::{RawClass, RawDrop};
use crate::list::slots::{compact_int, fallback, none, py, raise, restore, shield};
use crate::list::{compare_lengths, native_compare};
use crate::schema;
use crate::storage::{ElemCache, Storage};
use crate::sync::PyMutex;

type Obj = *mut ffi::PyObject;

/// Hash not computed yet (CPython's `ob_hash` sentinel).
const NO_HASH: isize = -1;

/// An immutable sequence with builtin `tuple` semantics and native storage.
#[pyclass(module = "rustique", frozen, sequence, subclass, immutable_type)]
pub struct Tuple {
    storage: Storage,
    /// Cached `hash(self)`, [`NO_HASH`] until computed (relaxed accesses, like CPython).
    hash: AtomicIsize,
    /// Boxed copies of native elements read repeatedly (never invalidated: immutable).
    cache: ElemCache,
}

/// Raw allocation of `Tuple` instances.
static TUPLE_CLASS: RawClass<Tuple> = RawClass::new();

impl RawDrop for Tuple {
    fn drop_attached(self, py: Python<'_>) {
        if let Storage::Object(objects) = self.storage {
            for o in objects {
                drop(o.into_bound(py));
            }
        }
    }
}

/// Storage for a `Tuple`: like an untyped `List`, but a float column holding a NaN becomes
/// object storage (NaN equality and hashing go by object identity).
fn tuple_storage(py: Python<'_>, storage: Storage) -> Storage {
    match storage {
        Storage::Native(Column::F64(v)) if v.iter().any(|x| x.is_nan()) => {
            let column = Column::F64(v);
            Storage::Object(convert::column_objects(py, &column, 0..column.len()))
        }
        other => other,
    }
}

impl Tuple {
    fn new_value(storage: Storage) -> Tuple {
        Tuple { storage, hash: AtomicIsize::new(NO_HASH), cache: ElemCache::new() }
    }

    /// A new exact `Tuple` holding `storage` (which must be NaN-free if a float column).
    /// Tuples of native values are untracked by the GC (they cannot be part of a cycle).
    pub fn from_storage(py: Python<'_>, storage: Storage) -> PyResult<Bound<'_, Tuple>> {
        let native = !matches!(storage, Storage::Object(_));
        let tuple = TUPLE_CLASS.new_bound(py, Tuple::new_value(storage))?;
        if native {
            // SAFETY: a tracked object of this module's GC type.
            unsafe { ffi::PyObject_GC_UnTrack(tuple.as_ptr().cast()) };
        }
        Ok(tuple)
    }

    /// The elements of any iterable as `Tuple` storage.
    fn ingest(py: Python<'_>, iterable: &Bound<'_, PyAny>) -> PyResult<Storage> {
        if let Ok(t) = iterable.cast::<Tuple>() {
            return Ok(t.get().storage.clone_ref(py));
        }
        // SAFETY: exact type checks on a live object.
        let (exact_list, exact_tuple) = unsafe {
            (ffi::PyList_CheckExact(iterable.as_ptr()) != 0, ffi::PyTuple_CheckExact(iterable.as_ptr()) != 0)
        };
        let storage = convert::ingest(py, &schema::any(), iterable)?;
        Ok(match storage {
            // A builtin source keeps its NaN objects themselves (identity).
            Storage::Native(Column::F64(ref v)) if (exact_list || exact_tuple) && v.iter().any(|x| x.is_nan()) => {
                convert::ingest_objects_raw(py, iterable)?
            }
            other => tuple_storage(py, other),
        })
    }

    /// Number of elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.storage.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The storage (immutable).
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// New reference to element `i` (in range) for an element-wise read, through the
    /// boxed element cache.
    #[inline(always)]
    fn read_item(&self, py: Python<'_>, i: usize) -> Obj {
        if let Storage::Native(_) = self.storage {
            self.cache.note_read(self.len());
        }
        self.cache.item(py, &self.storage, i)
    }

    /// New reference to element `i` (in range).
    #[inline]
    fn item<'py>(&self, py: Python<'py>, i: usize) -> Bound<'py, PyAny> {
        // SAFETY: `item_ptr` returns a new reference.
        unsafe { Bound::from_owned_ptr(py, self.storage.item_ptr(py, i)) }
    }

    /// A builtin `tuple` with the same elements.
    pub fn to_pytuple<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let n = self.len();
        // SAFETY: a fresh tuple of `n` slots, each filled once with a new reference.
        unsafe {
            let t = Bound::from_owned_ptr_or_err(py, ffi::PyTuple_New(n as ffi::Py_ssize_t))?;
            for i in 0..n {
                ffi::PyTuple_SET_ITEM(t.as_ptr(), i as ffi::Py_ssize_t, self.storage.item_ptr(py, i));
            }
            Ok(t.cast_into_unchecked())
        }
    }

    /// `hash(self)`: CPython's `tuplehash` (xxHash lanes over the element hashes), cached.
    fn hash_value(&self, py: Python<'_>) -> PyResult<isize> {
        let cached = self.hash.load(Ordering::Relaxed);
        if cached != NO_HASH {
            return Ok(cached);
        }
        // `_PyTuple_HASH_XXPRIME_*` and `_PyTuple_HASH_XXROTATE` (Include/internal/pycore_tuple.h).
        #[cfg(target_pointer_width = "64")]
        let (p1, p2, p5, rotate): (usize, usize, usize, fn(usize) -> usize) =
            (11400714785074694791, 14029467366897019727, 2870177450012600261, |x| x.rotate_left(31));
        #[cfg(not(target_pointer_width = "64"))]
        let (p1, p2, p5, rotate): (usize, usize, usize, fn(usize) -> usize) =
            (2654435761, 2246822519, 374761393, |x| x.rotate_left(13));
        let mut acc: usize = p5;
        let mut lane = |h: isize| {
            acc = acc.wrapping_add((h as usize).wrapping_mul(p2));
            acc = rotate(acc);
            acc = acc.wrapping_mul(p1);
        };
        match &self.storage {
            Storage::Empty => {}
            Storage::Native(Column::I64(v)) => v.iter().for_each(|&x| lane(pyhash::hash_i64(x))),
            Storage::Native(Column::Bool(v)) => v.iter().for_each(|&x| lane(pyhash::hash_bool(x))),
            Storage::Native(Column::F64(v)) => {
                for &x in v {
                    // Float columns of a `Tuple` hold no NaN (see `tuple_storage`).
                    lane(pyhash::hash_f64(x).unwrap_or(0));
                }
            }
            storage => {
                for i in 0..storage.len() {
                    let item = self.item(py, i);
                    lane(item.hash()?);
                }
            }
        }
        let n = self.len();
        acc = acc.wrapping_add(n ^ (p5 ^ 3527539));
        let mut h = acc as isize;
        if h == -1 {
            h = 1546275796;
        }
        self.hash.store(h, Ordering::Relaxed);
        Ok(h)
    }

    /// The elements of `other` as a comparison operand: a `Tuple`'s storage or a builtin
    /// tuple's (or subclass's) item array; `None` for anything else.
    fn compare_with(py: Python<'_>, slf: &Tuple, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<Py<PyAny>> {
        let result = |v: bool| Ok(pyo3::types::PyBool::new(py, v).to_owned().into_any().unbind());
        enum Other<'a, 'py> {
            Tuple(&'a Tuple),
            Builtin(&'a Bound<'py, PyAny>),
        }
        let theirs = if let Ok(t) = other.cast::<Tuple>() {
            Other::Tuple(t.get())
        // SAFETY: type check on a live object.
        } else if unsafe { ffi::PyTuple_Check(other.as_ptr()) } != 0 {
            Other::Builtin(other)
        } else {
            return Ok(py.NotImplemented());
        };
        // Native fast paths.
        match &theirs {
            Other::Tuple(t) => {
                if slf.is_empty() || t.is_empty() {
                    return result(compare_lengths(slf.len(), t.len(), op));
                }
                if let (Storage::Native(a), Storage::Native(b)) = (&slf.storage, &t.storage)
                    && let Some(r) = native_compare(a, b, op)
                {
                    return result(r);
                }
            }
            Other::Builtin(b) => {
                // SAFETY: type check on a live object.
                if matches!(slf.storage, Storage::Native(_)) && unsafe { ffi::PyTuple_CheckExact(b.as_ptr()) } != 0 {
                    let converted = convert::ingest(py, &schema::any(), b)?;
                    if let (Storage::Native(a), Storage::Native(c)) = (&slf.storage, &converted)
                        && let Some(r) = native_compare(a, c, op)
                    {
                        return result(r);
                    }
                }
            }
        }
        // CPython's `tuple_richcompare`: the first index where the items differ (identity
        // implies equality), then the lengths or that pair decide.
        let (la, lb) = match &theirs {
            Other::Tuple(t) => (slf.len(), t.len()),
            Other::Builtin(b) => (slf.len(), b.len()?),
        };
        let get = |i: usize| -> Bound<'_, PyAny> {
            match &theirs {
                Other::Tuple(t) => t.item(py, i),
                // SAFETY: `i` is below the (immutable) length of a tuple or subclass.
                Other::Builtin(b) => unsafe {
                    Bound::from_borrowed_ptr(py, ffi::PyTuple_GET_ITEM(b.as_ptr(), i as isize))
                },
            }
        };
        let mut i = 0;
        while i < la && i < lb {
            let (x, y) = (slf.item(py, i), get(i));
            if !equal_ptr(py, x.clone(), y.as_ptr())? {
                return match op {
                    CompareOp::Eq => result(false),
                    CompareOp::Ne => result(true),
                    _ => Ok(x.rich_compare(&y, op)?.unbind()),
                };
            }
            i += 1;
        }
        result(compare_lengths(la, lb, op))
    }

    /// Python-level search: the first index in `start..stop` holding an element equal to
    /// `value` (`PyObject_RichCompareBool`: identity, then `==`).
    fn find(&self, py: Python<'_>, value: &Bound<'_, PyAny>, start: usize, stop: usize) -> PyResult<Option<usize>> {
        let stop = stop.min(self.len());
        if start >= stop {
            return Ok(None);
        }
        let probe = convert::probe(value);
        match (&self.storage, &probe) {
            (Storage::Native(column), Probe::Native(needle)) => return Ok(kernels::find(column, needle, start, stop)),
            (Storage::Native(_), Probe::Never) | (Storage::Empty, _) => return Ok(None),
            _ => {}
        }
        for i in start..stop {
            if equal_ptr(py, self.storage_ptr(py, i), value.as_ptr())? {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    /// Element `i` as an object that stays alive during a comparison (a borrowed stored
    /// object or a new materialised one).
    fn storage_ptr<'py>(&self, py: Python<'py>, i: usize) -> Bound<'py, PyAny> {
        self.item(py, i)
    }
}

/// `PyObject_RichCompareBool(a, b, Py_EQ)`.
fn equal_ptr(py: Python<'_>, a: Bound<'_, PyAny>, b: Obj) -> PyResult<bool> {
    // SAFETY: live objects.
    match unsafe { ffi::PyObject_RichCompareBool(a.as_ptr(), b, ffi::Py_EQ) } {
        -1 => Err(PyErr::fetch(py)),
        r => Ok(r == 1),
    }
}

/// A subscript of a tuple: an index or a slice (`TypeError` otherwise).
fn parse_subscript(key: &Bound<'_, PyAny>) -> PyResult<Result<isize, (isize, isize, isize)>> {
    let py = key.py();
    let p = key.as_ptr();
    // SAFETY: type checks and C-API conversions on a live object.
    unsafe {
        if ffi::PySlice_Check(p) != 0 {
            let (mut start, mut stop, mut step) = (0, 0, 0);
            if ffi::PySlice_Unpack(p, &mut start, &mut stop, &mut step) < 0 {
                return Err(PyErr::fetch(py));
            }
            return Ok(Err((start, stop, step)));
        }
        if ffi::PyIndex_Check(p) != 0 {
            let i = ffi::PyNumber_AsSsize_t(p, ffi::PyExc_IndexError);
            if i == -1
                && let Some(e) = PyErr::take(py)
            {
                return Err(e);
            }
            return Ok(Ok(i));
        }
    }
    Err(PyTypeError::new_err(format!("tuple indices must be integers or slices, not {}", convert::type_name(key))))
}

#[pymethods]
impl Tuple {
    /// `Tuple(iterable=(), /)`. Like `tuple.__new__`, keyword arguments are rejected unless
    /// a subclass defines its own `__init__`.
    #[new]
    #[classmethod]
    #[pyo3(signature = (*args, **kwargs), text_signature = "(iterable=(), /)")]
    fn py_new(
        cls: &Bound<'_, PyType>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let py = cls.py();
        if kwargs.is_some_and(|k| !k.is_empty()) {
            let own_init = cls.getattr(intern!(py, "__init__"))?;
            let object_init = py.get_type::<pyo3::types::PyAny>().getattr(intern!(py, "__init__"))?;
            if own_init.is(&object_init) {
                return Err(PyTypeError::new_err("Tuple() takes no keyword arguments"));
            }
        }
        let storage = match args.len() {
            0 => Storage::Empty,
            1 => Tuple::ingest(py, &args.get_item(0)?)?,
            n => return Err(PyTypeError::new_err(format!("Tuple expected at most 1 argument, got {n}"))),
        };
        Ok(Tuple::new_value(storage))
    }

    #[classmethod]
    fn __class_getitem__<'py>(cls: &Bound<'py, PyType>, item: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = cls.py();
        py.import(intern!(py, "types"))?.getattr(intern!(py, "GenericAlias"))?.call1((cls, item))
    }

    /// The storage strategy: `"empty"`, `"bool"`, `"i64"`, `"f64"`, `"str"` or `"object"`.
    #[getter]
    fn strategy(&self) -> &'static str {
        self.storage.strategy()
    }

    fn __len__(&self) -> usize {
        self.len()
    }

    fn __getitem__<'py>(slf: &Bound<'py, Self>, key: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        let this = slf.get();
        match parse_subscript(key)? {
            Ok(i) => match normalize_index(i, this.len()) {
                Some(k) => Ok(this.item(py, k)),
                None => Err(PyIndexError::new_err("tuple index out of range")),
            },
            Err((start, stop, step)) => {
                let s = SliceIndices::new(Some(start), Some(stop), Some(step), this.len())
                    .map_err(|e| PyValueError::new_err(e.to_string()))?;
                // SAFETY: type check on a live object.
                let exact = unsafe { ffi::Py_TYPE(slf.as_ptr()) } == TUPLE_TYPE.load(Ordering::Relaxed);
                if exact && s.step == 1 && s.len == this.len() {
                    return Ok(slf.clone().into_any());
                }
                Ok(Tuple::from_storage(py, this.storage.take_slice(py, &s))?.into_any())
            }
        }
    }

    fn __contains__(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(self.find(py, value, 0, usize::MAX)?.is_some())
    }

    fn __iter__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        // SAFETY: a new reference or null with an exception set.
        unsafe { Bound::from_owned_ptr_or_err(slf.py(), new_iterator(slf.as_ptr())) }
    }

    fn __hash__(&self, py: Python<'_>) -> PyResult<isize> {
        self.hash_value(py)
    }

    fn __richcmp__(&self, py: Python<'_>, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<Py<PyAny>> {
        Tuple::compare_with(py, self, other, op)
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        // SAFETY: recursion guard keyed on this object; balanced by Py_ReprLeave below.
        match unsafe { ffi::Py_ReprEnter(slf.as_ptr()) } {
            0 => {}
            r if r > 0 => return Ok("Tuple((...))".to_owned()),
            _ => return Err(PyErr::fetch(py)),
        }
        let result = (|| Ok(format!("Tuple({})", slf.get().to_pytuple(py)?.repr()?)))();
        // SAFETY: matches the successful Py_ReprEnter above.
        unsafe { ffi::Py_ReprLeave(slf.as_ptr()) };
        result
    }

    fn __add__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        let right = if let Ok(t) = other.cast::<Tuple>() {
            t.get().storage.clone_ref(py)
        // SAFETY: type check on a live object.
        } else if unsafe { ffi::PyTuple_Check(other.as_ptr()) } != 0 {
            Tuple::ingest(py, other)?
        } else {
            return Ok(py.NotImplemented().into_bound(py));
        };
        let mut left = slf.get().storage.clone_ref(py);
        left.extend(py, right);
        Ok(Tuple::from_storage(py, left)?.into_any())
    }

    fn __radd__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        // SAFETY: type check on a live object.
        if unsafe { ffi::PyTuple_Check(other.as_ptr()) } == 0 {
            return Ok(py.NotImplemented().into_bound(py));
        }
        let mut left = Tuple::ingest(py, other)?;
        left.extend(py, slf.get().storage.clone_ref(py));
        Ok(Tuple::from_storage(py, left)?.into_any())
    }

    fn __mul__<'py>(slf: &Bound<'py, Self>, n: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        // SAFETY: protocol check and conversion on a live object.
        let n = unsafe {
            if ffi::PyIndex_Check(n.as_ptr()) == 0 {
                return Ok(py.NotImplemented().into_bound(py));
            }
            let v = ffi::PyNumber_AsSsize_t(n.as_ptr(), ffi::PyExc_OverflowError);
            if v == -1
                && let Some(e) = PyErr::take(py)
            {
                return Err(e);
            }
            v
        };
        let this = slf.get();
        // SAFETY: a type pointer read.
        let exact = unsafe { ffi::Py_TYPE(slf.as_ptr()) } == TUPLE_TYPE.load(Ordering::Relaxed);
        if exact && (n == 1 || this.is_empty()) {
            return Ok(slf.clone().into_any());
        }
        Ok(Tuple::from_storage(py, this.storage.repeat(py, n)?)?.into_any())
    }

    fn __rmul__<'py>(slf: &Bound<'py, Self>, n: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        Self::__mul__(slf, n)
    }

    /// Return number of occurrences of value.
    fn count(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<usize> {
        let probe = convert::probe(value);
        match (&self.storage, &probe) {
            (Storage::Native(column), Probe::Native(needle)) => return Ok(kernels::count(column, needle)),
            (Storage::Native(_), Probe::Never) | (Storage::Empty, _) => return Ok(0),
            _ => {}
        }
        let mut n = 0;
        for i in 0..self.len() {
            if equal_ptr(py, self.storage_ptr(py, i), value.as_ptr())? {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Return first index of value (`ValueError` if absent).
    #[pyo3(signature = (value, start = None, stop = None, /), text_signature = "($self, value, start=0, stop=sys.maxsize, /)")]
    fn index(
        &self,
        py: Python<'_>,
        value: &Bound<'_, PyAny>,
        start: Option<&Bound<'_, PyAny>>,
        stop: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<usize> {
        let bound = |b: Option<&Bound<'_, PyAny>>, default: isize| -> PyResult<isize> {
            let Some(b) = b else { return Ok(default) };
            // SAFETY: protocol check and clamping conversion on a live object.
            unsafe {
                if ffi::PyIndex_Check(b.as_ptr()) == 0 {
                    return Err(PyTypeError::new_err("slice indices must be integers or have an __index__ method"));
                }
                let v = ffi::PyNumber_AsSsize_t(b.as_ptr(), ptr::null_mut());
                if v == -1
                    && let Some(e) = PyErr::take(py)
                {
                    return Err(e);
                }
                Ok(v)
            }
        };
        let (start, stop) = (bound(start, 0)?, bound(stop, isize::MAX)?);
        let len = self.len() as isize;
        let clamp = |v: isize| if v < 0 { (v.saturating_add(len)).max(0) as usize } else { v as usize };
        self.find(py, value, clamp(start), clamp(stop))?
            .ok_or_else(|| PyValueError::new_err("tuple.index(x): x not in tuple"))
    }

    /// Pickle as `Tuple(tuple(self))` (plus the instance dict of a subclass instance).
    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyTuple>> {
        let py = slf.py();
        let items = slf.get().to_pytuple(py)?;
        let state = slf.getattr(intern!(py, "__dict__")).ok();
        (slf.get_type(), (items,), state).into_pyobject(py)
    }

    fn __getnewargs__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        (self.to_pytuple(py)?,).into_pyobject(py)
    }

    /// `copy.copy`: an exact `Tuple` is its own copy (like `tuple`).
    fn __copy__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        // SAFETY: a type pointer read.
        if unsafe { ffi::Py_TYPE(slf.as_ptr()) } == TUPLE_TYPE.load(Ordering::Relaxed) {
            return Ok(slf.clone().into_any());
        }
        let copy = slf.get_type().call1((slf.get().to_pytuple(py)?,))?;
        if let Ok(dict) = slf.getattr(intern!(py, "__dict__")) {
            copy.getattr(intern!(py, "__dict__"))?.call_method1(intern!(py, "update"), (dict,))?;
        }
        Ok(copy)
    }

    /// `copy.deepcopy`: the same tuple if every element deep-copies to itself.
    fn __deepcopy__<'py>(slf: &Bound<'py, Self>, memo: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        let this = slf.get();
        if !matches!(this.storage, Storage::Object(_)) {
            return Self::__copy__(slf);
        }
        let deepcopy = py.import(intern!(py, "copy"))?.getattr(intern!(py, "deepcopy"))?;
        let mut copies = Vec::with_capacity(this.len());
        let mut same = true;
        for i in 0..this.len() {
            let item = this.item(py, i);
            let copy = deepcopy.call1((&item, memo))?;
            same &= copy.is(&item);
            copies.push(copy.unbind());
        }
        if same {
            return Self::__copy__(slf);
        }
        let storage = tuple_storage(py, Storage::Object(copies));
        // SAFETY: a type pointer read.
        if unsafe { ffi::Py_TYPE(slf.as_ptr()) } == TUPLE_TYPE.load(Ordering::Relaxed) {
            return Ok(Tuple::from_storage(py, storage)?.into_any());
        }
        let items = Tuple::new_value(storage).to_pytuple(py)?;
        slf.get_type().call1((items,))
    }

    fn __sizeof__(&self, py: Python<'_>) -> usize {
        fn bytes<T>(v: &Vec<T>) -> usize {
            v.capacity() * size_of::<T>()
        }
        let payload = match &self.storage {
            Storage::Empty => 0,
            Storage::Native(column) => rustique_core::with_column!(column, v => bytes(v)),
            Storage::Object(objects) => bytes(objects),
        };
        // SAFETY: reading the basic size of this class's type object.
        let base = unsafe { (*py.get_type::<Tuple>().as_type_ptr()).tp_basicsize } as usize;
        base + payload
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        if let Storage::Object(objects) = &self.storage {
            for o in objects {
                visit.call(o)?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------------------

/// Iterator state: the tuple (released once exhausted) and the next index.
struct TupleIterState {
    tuple: Option<Py<Tuple>>,
    index: usize,
}

#[repr(C)]
struct TupleIterObject {
    ob_base: ffi::PyObject,
    state: PyMutex<TupleIterState>,
}

static ITER_TYPE: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static TUPLE_TYPE: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(ptr::null_mut());

/// # Safety
/// `slf` must be a live tuple iterator.
#[inline(always)]
unsafe fn iter_state<'a>(slf: Obj) -> &'a PyMutex<TupleIterState> {
    // SAFETY: see above.
    unsafe { &(*slf.cast::<TupleIterObject>()).state }
}

/// A new iterator over the `Tuple` `tuple` (a new reference, or null with an exception).
fn new_iterator(tuple: Obj) -> Obj {
    // SAFETY: `tuple` is a live `Tuple`; the object is fully initialised before it is
    // tracked or returned.
    unsafe {
        let obj = ffi::PyObject_GC_New::<TupleIterObject>(ITER_TYPE.load(Ordering::Acquire));
        if obj.is_null() {
            return ptr::null_mut();
        }
        ffi::Py_INCREF(tuple);
        let tuple = Bound::from_owned_ptr(py(), tuple).cast_into_unchecked::<Tuple>().unbind();
        ptr::write(&raw mut (*obj).state, PyMutex::new(TupleIterState { tuple: Some(tuple), index: 0 }));
        ffi::PyObject_GC_Track(obj.cast());
        obj.cast()
    }
}

unsafe extern "C" fn iter_next(slf: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        let py = py();
        // SAFETY: installed on the tuple iterator type.
        let mut st = unsafe { iter_state(slf) }.lock(py);
        if let Some(t) = &st.tuple {
            let t = t.get();
            if st.index < t.len() {
                let item = t.read_item(py, st.index);
                st.index += 1;
                return item;
            }
        }
        let finished = st.tuple.take();
        drop(st);
        drop(finished.map(|t| t.into_bound(py)));
        ptr::null_mut()
    })
}

unsafe extern "C" fn iter_dealloc(slf: Obj) {
    shield((), || {
        // SAFETY: a dead tuple iterator; its state is read out exactly once.
        unsafe {
            ffi::PyObject_GC_UnTrack(slf.cast());
            let ty = ffi::Py_TYPE(slf);
            let state = ptr::read(&raw const (*slf.cast::<TupleIterObject>()).state);
            drop(state.into_inner().tuple.map(|t| t.into_bound(py())));
            ffi::PyObject_GC_Del(slf.cast());
            ffi::Py_DECREF(ty.cast());
        }
    })
}

unsafe extern "C" fn iter_traverse(slf: Obj, visit: ffi::visitproc, arg: *mut std::ffi::c_void) -> c_int {
    // SAFETY: a live tuple iterator; never blocks.
    unsafe {
        if let Some(st) = iter_state(slf).try_lock()
            && let Some(t) = &st.tuple
        {
            let r = visit(t.as_ptr(), arg);
            if r != 0 {
                return r;
            }
        }
        visit(ffi::Py_TYPE(slf).cast(), arg)
    }
}

unsafe extern "C" fn iter_clear(slf: Obj) -> c_int {
    shield(0, || {
        let py = py();
        // SAFETY: a live tuple iterator.
        let t = unsafe { iter_state(slf) }.lock(py).tuple.take();
        drop(t.map(|t| t.into_bound(py)));
        0
    })
}

unsafe extern "C" fn iter_length_hint(slf: Obj, _: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        fallback(ptr::null_mut(), |py| {
            // SAFETY: a live tuple iterator.
            let st = unsafe { iter_state(slf) }.lock(py);
            let n = st.tuple.as_ref().map_or(0, |t| t.get().len().saturating_sub(st.index));
            drop(st);
            Ok(n.into_pyobject(py)?.into_ptr())
        })
    })
}

unsafe extern "C" fn iter_reduce(slf: Obj, _: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        fallback(ptr::null_mut(), |py| {
            let iter = py.import(intern!(py, "builtins"))?.getattr(intern!(py, "iter"))?;
            let (tuple, index) = {
                // SAFETY: a live tuple iterator.
                let st = unsafe { iter_state(slf) }.lock(py);
                (st.tuple.as_ref().map(|t| t.clone_ref(py)), st.index)
            };
            let reduced = match tuple {
                Some(t) => (iter, (t,), index).into_pyobject(py)?,
                None => (iter, (Tuple::from_storage(py, Storage::Empty)?,)).into_pyobject(py)?,
            };
            Ok(reduced.into_ptr())
        })
    })
}

unsafe extern "C" fn iter_setstate(slf: Obj, value: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        fallback(ptr::null_mut(), |py| {
            // SAFETY: a live object; -1 with an exception set signals an error.
            let index = unsafe { ffi::PyLong_AsSsize_t(value) };
            if index == -1
                && let Some(e) = PyErr::take(py)
            {
                return Err(e);
            }
            // SAFETY: a live tuple iterator.
            let mut st = unsafe { iter_state(slf) }.lock(py);
            if let Some(t) = &st.tuple {
                st.index = index.clamp(0, t.get().len() as isize) as usize;
            }
            Ok(none())
        })
    })
}

static mut ITER_METHODS: [ffi::PyMethodDef; 4] = [
    ffi::PyMethodDef {
        ml_name: c"__length_hint__".as_ptr(),
        ml_meth: ffi::PyMethodDefPointer { PyCFunction: iter_length_hint },
        ml_flags: ffi::METH_NOARGS,
        ml_doc: c"Private method returning an estimate of len(list(it)).".as_ptr(),
    },
    ffi::PyMethodDef {
        ml_name: c"__reduce__".as_ptr(),
        ml_meth: ffi::PyMethodDefPointer { PyCFunction: iter_reduce },
        ml_flags: ffi::METH_NOARGS,
        ml_doc: c"Return state information for pickling.".as_ptr(),
    },
    ffi::PyMethodDef {
        ml_name: c"__setstate__".as_ptr(),
        ml_meth: ffi::PyMethodDefPointer { PyCFunction: iter_setstate },
        ml_flags: ffi::METH_O,
        ml_doc: c"Set state information for unpickling.".as_ptr(),
    },
    ffi::PyMethodDef::zeroed(),
];

// ---------------------------------------------------------------------------------------
// Raw slots
// ---------------------------------------------------------------------------------------

// SAFETY (for every slot below): CPython calls them attached, with `slf` a live `Tuple`
// (or subclass) instance and the other arguments live objects.

/// # Safety
/// `slf` must be a live `Tuple` (or subclass) instance.
#[inline(always)]
unsafe fn this<'a>(slf: Obj) -> &'a Tuple {
    // SAFETY: see above.
    unsafe {
        let bound = pyo3::Borrowed::from_ptr_or_opt(py(), slf).unwrap_unchecked().cast_unchecked::<Tuple>();
        &*std::ptr::from_ref(bound.get())
    }
}

unsafe extern "C" fn sq_length(slf: Obj) -> ffi::Py_ssize_t {
    // SAFETY: see above.
    unsafe { this(slf) }.len() as ffi::Py_ssize_t
}

unsafe extern "C" fn mp_subscript(slf: Obj, key: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        // SAFETY: see above.
        if let Some(i) = unsafe { compact_int(key) } {
            // SAFETY: see above.
            let t = unsafe { this(slf) };
            return match normalize_index(i, t.len()) {
                Some(k) => t.read_item(py(), k),
                None => {
                    // SAFETY: reading a static exception type.
                    raise(unsafe { ffi::PyExc_IndexError }, c"tuple index out of range");
                    ptr::null_mut()
                }
            };
        }
        fallback(ptr::null_mut(), |py| {
            // SAFETY: see above.
            let (slf, key) = unsafe {
                (Bound::from_borrowed_ptr(py, slf).cast_into_unchecked::<Tuple>(), Bound::from_borrowed_ptr(py, key))
            };
            Ok(Tuple::__getitem__(&slf, &key)?.into_ptr())
        })
    })
}

unsafe extern "C" fn sq_item(slf: Obj, i: ffi::Py_ssize_t) -> Obj {
    shield(ptr::null_mut(), || {
        // SAFETY: see above.
        let t = unsafe { this(slf) };
        if i >= 0 && (i as usize) < t.len() {
            return t.read_item(py(), i as usize);
        }
        // SAFETY: reading a static exception type.
        raise(unsafe { ffi::PyExc_IndexError }, c"tuple index out of range");
        ptr::null_mut()
    })
}

unsafe extern "C" fn sq_contains(slf: Obj, value: Obj) -> c_int {
    shield(-1, || {
        let py = py();
        // SAFETY: see above.
        let t = unsafe { this(slf) };
        // SAFETY: a live object.
        let value = unsafe { Bound::from_borrowed_ptr(py, value) };
        match t.find(py, &value, 0, usize::MAX) {
            Ok(found) => c_int::from(found.is_some()),
            Err(e) => {
                restore(e);
                -1
            }
        }
    })
}

unsafe extern "C" fn tp_iter(slf: Obj) -> Obj {
    shield(ptr::null_mut(), || new_iterator(slf))
}

#[cfg(Py_3_15)]
unsafe extern "C" fn iteritem(slf: Obj, index: ffi::Py_ssize_t) -> ffi::_PyObjectIndexPair {
    shield(ffi::_PyObjectIndexPair { object: ptr::null_mut(), index: -1 }, || {
        // SAFETY: see above.
        let t = unsafe { this(slf) };
        if (index as usize) < t.len() {
            ffi::_PyObjectIndexPair { object: t.read_item(py(), index as usize), index: index + 1 }
        } else {
            ffi::_PyObjectIndexPair { object: ptr::null_mut(), index }
        }
    })
}

unsafe extern "C" fn tp_hash(slf: Obj) -> ffi::Py_hash_t {
    shield(-1, || {
        // SAFETY: see above.
        let t = unsafe { this(slf) };
        let cached = t.hash.load(Ordering::Relaxed);
        if cached != NO_HASH {
            return cached;
        }
        match t.hash_value(py()) {
            Ok(h) => h,
            Err(e) => {
                restore(e);
                -1
            }
        }
    })
}

unsafe extern "C" fn tp_richcompare(slf: Obj, other: Obj, op: c_int) -> Obj {
    shield(ptr::null_mut(), || {
        let compare_op = match op {
            ffi::Py_LT => CompareOp::Lt,
            ffi::Py_LE => CompareOp::Le,
            ffi::Py_EQ => CompareOp::Eq,
            ffi::Py_NE => CompareOp::Ne,
            ffi::Py_GT => CompareOp::Gt,
            _ => CompareOp::Ge,
        };
        fallback(ptr::null_mut(), |py| {
            // SAFETY: see above.
            let other = unsafe { Bound::from_borrowed_ptr(py, other) };
            // SAFETY: see above.
            let this = unsafe { this(slf) };
            Ok(Tuple::compare_with(py, this, &other, compare_op)?.into_ptr())
        })
    })
}

/// `Tuple(iterable=())` for the exact class: an exact `Tuple` argument is returned itself.
unsafe extern "C" fn tp_vectorcall(_callable: Obj, args: *const Obj, nargsf: usize, kwnames: Obj) -> Obj {
    shield(ptr::null_mut(), || {
        // SAFETY: vectorcall protocol.
        let nargs = unsafe { ffi::PyVectorcall_NARGS(nargsf) };
        // SAFETY: `kwnames` is NULL or a tuple.
        if !kwnames.is_null() && unsafe { ffi::PyTuple_GET_SIZE(kwnames) } != 0 {
            restore(PyTypeError::new_err("Tuple() takes no keyword arguments"));
            return ptr::null_mut();
        }
        if nargs == 1 {
            // SAFETY: one live positional argument.
            let arg = unsafe { *args };
            // SAFETY: a type pointer read.
            if unsafe { ffi::Py_TYPE(arg) } == TUPLE_TYPE.load(Ordering::Relaxed) {
                // SAFETY: a live object; a new reference is returned.
                unsafe { ffi::Py_INCREF(arg) };
                return arg;
            }
            // An exact builtin list or tuple: converted without Python code or `Py` drops.
            let py = py();
            // SAFETY: a live object.
            let source = unsafe { Bound::from_borrowed_ptr(py, arg) };
            if let Some(storage) = convert::ingest_exact_seq_any(py, &source) {
                let storage = match storage {
                    Storage::Native(Column::F64(ref v)) if v.iter().any(|x| x.is_nan()) => {
                        match convert::ingest_objects_raw(py, &source) {
                            Ok(objects) => objects,
                            Err(e) => {
                                restore(e.error);
                                return ptr::null_mut();
                            }
                        }
                    }
                    other => other,
                };
                return match Tuple::from_storage(py, storage) {
                    Ok(t) => t.into_ptr(),
                    Err(e) => {
                        restore(e);
                        ptr::null_mut()
                    }
                };
            }
        } else if nargs == 0 {
            return match Tuple::from_storage(py(), Storage::Empty) {
                Ok(t) => t.into_ptr(),
                Err(e) => {
                    restore(e);
                    ptr::null_mut()
                }
            };
        }
        fallback(ptr::null_mut(), |py| {
            let storage = match nargs {
                0 => Storage::Empty,
                1 => {
                    // SAFETY: one live positional argument.
                    let iterable = unsafe { Bound::from_borrowed_ptr(py, *args) };
                    Tuple::ingest(py, &iterable)?
                }
                n => return Err(PyTypeError::new_err(format!("Tuple expected at most 1 argument, got {n}"))),
            };
            Ok(Tuple::from_storage(py, storage)?.into_ptr())
        })
    })
}

unsafe extern "C" fn tuple_dealloc(slf: Obj) {
    // SAFETY: installed as `Tuple`'s `tp_dealloc`.
    shield((), || unsafe { TUPLE_CLASS.dealloc(slf) })
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add_class::<Tuple>()?;
    let ty = py.get_type::<Tuple>().as_type_ptr();
    TUPLE_TYPE.store(ty, Ordering::Release);
    let sample = Bound::new(py, Tuple::new_value(Storage::Empty))?;
    TUPLE_CLASS.install(&sample, sample.get(), tuple_dealloc)?;
    drop(sample);
    // SAFETY: module initialisation, before any other code can observe the types; each
    // function has its slot's C signature and the method table is static.
    unsafe {
        // The iterator type.
        let mut slots = [
            ffi::PyType_Slot { slot: ffi::Py_tp_dealloc, pfunc: iter_dealloc as *mut std::ffi::c_void },
            ffi::PyType_Slot { slot: ffi::Py_tp_traverse, pfunc: iter_traverse as *mut std::ffi::c_void },
            ffi::PyType_Slot { slot: ffi::Py_tp_clear, pfunc: iter_clear as *mut std::ffi::c_void },
            ffi::PyType_Slot { slot: ffi::Py_tp_iter, pfunc: ffi::PyObject_SelfIter as *mut std::ffi::c_void },
            ffi::PyType_Slot { slot: ffi::Py_tp_iternext, pfunc: iter_next as *mut std::ffi::c_void },
            ffi::PyType_Slot { slot: ffi::Py_tp_methods, pfunc: (&raw mut ITER_METHODS).cast() },
            ffi::PyType_Slot { slot: 0, pfunc: ptr::null_mut() },
        ];
        let mut spec = ffi::PyType_Spec {
            name: c"rustique.TupleIterator".as_ptr(),
            basicsize: size_of::<TupleIterObject>() as c_int,
            itemsize: 0,
            flags: (ffi::Py_TPFLAGS_DEFAULT
                | ffi::Py_TPFLAGS_HAVE_GC
                | ffi::Py_TPFLAGS_IMMUTABLETYPE
                | ffi::Py_TPFLAGS_DISALLOW_INSTANTIATION) as _,
            slots: slots.as_mut_ptr(),
        };
        let iter_type = Bound::from_owned_ptr_or_err(py, ffi::PyType_FromSpec(&mut spec))?;
        ITER_TYPE.store(iter_type.into_ptr().cast(), Ordering::Release);

        // Raw slots on `Tuple` (and its type dict's slot wrappers).
        let fp = |f: *const ()| f as *mut std::ffi::c_void;
        crate::list::slots::rewrap_slots(
            py,
            ty,
            &[
                ("__len__", fp(sq_length as *const ())),
                ("__getitem__", fp(mp_subscript as *const ())),
                ("__contains__", fp(sq_contains as *const ())),
                ("__iter__", fp(tp_iter as *const ())),
                ("__hash__", fp(tp_hash as *const ())),
                ("__lt__", fp(tp_richcompare as *const ())),
                ("__le__", fp(tp_richcompare as *const ())),
                ("__eq__", fp(tp_richcompare as *const ())),
                ("__ne__", fp(tp_richcompare as *const ())),
                ("__gt__", fp(tp_richcompare as *const ())),
                ("__ge__", fp(tp_richcompare as *const ())),
            ],
        )?;
        let seq = (*ty).tp_as_sequence;
        (*seq).sq_length = Some(sq_length);
        (*seq).sq_item = Some(sq_item);
        (*seq).sq_contains = Some(sq_contains);
        (*(*ty).tp_as_mapping).mp_subscript = Some(mp_subscript);
        (*ty).tp_iter = Some(tp_iter);
        (*ty).tp_hash = Some(tp_hash);
        (*ty).tp_richcompare = Some(tp_richcompare);
        (*ty).tp_vectorcall = Some(tp_vectorcall);
        #[cfg(Py_3_15)]
        {
            (*ty)._tp_iteritem = Some(iteritem);
        }
        ffi::PyType_Modified(ty);
        // C-level messages using `tp_name` read like the builtin's (as for `List`).
        (*ty).tp_name = c"Tuple".as_ptr();
    }
    Ok(())
}
