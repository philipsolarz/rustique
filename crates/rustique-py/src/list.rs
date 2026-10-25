//! The `List` class (ARCHITECTURE §4.1).
//!
//! OWNER: list agent. Items marked `INTERFACE` are used by other modules.
//!
//! Concurrency (§6): the storage lives behind a [`PyRwLock`]. Every operation is
//! three-phase: Python-facing preparation (coercion, iteration, `__index__`, user `__eq__`)
//! happens without the lock; the commit under the lock is pure Rust (plus creation of
//! `int`/`float`/`bool`/`str` objects); displaced objects are dropped after unlocking.

use std::ffi::{CStr, c_int, c_void};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pyo3::exceptions::{PyBufferError, PyIndexError, PyMemoryError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::pyclass::CompareOp;
use pyo3::types::{PyBool, PyDict, PyList, PyString, PyTuple, PyType};
use pyo3::{PyTraverseError, ffi, intern};
use rustique_core::kernels::{self, CmpOp, KernelError, Sum};
use rustique_core::slice::{SliceError, SliceIndices, clamp_insert_index, normalize_index};
use rustique_core::{Column, DType, with_column};

use crate::convert::{self, IngestError, Probe, ToPython};
use crate::schema::{self, Coerced, Schema};
use crate::storage::{ElemCache, Storage};
use crate::sync::{MutationCounter, MutexGuard, PyMutex, PyRwLock, ReadGuard, WriteGuard, read_both};

/// Native kernels on at least this many elements run detached from the interpreter.
const DETACH_THRESHOLD: usize = 1 << 16;
/// Sorting goes parallel (rayon) from this size: always detach then.
const SORT_DETACH_THRESHOLD: usize = 1 << 14;
/// How often `remove` re-runs a Python-level search invalidated by concurrent mutation.
const MAX_RETRIES: usize = 64;

/// A list with Python `list` semantics and native, adaptive storage.
#[pyclass(module = "rustique", frozen, sequence, subclass, immutable_type)]
pub struct List {
    schema: Arc<Schema>,
    data: PyRwLock<Storage>,
    version: MutationCounter,
    /// Length as of the last completed mutation (lock-free `len()`).
    len: AtomicUsize,
    /// Live buffer exports; while non-zero every mutation raises `BufferError`.
    exports: AtomicUsize,
    /// Python-level sorts in progress (their elements are out of the list): no exports.
    sorting: AtomicUsize,
    /// Boxed copies of native elements read repeatedly (see `ElemCache`); cleared by every
    /// mutation except the ones that maintain it (append, item assignment, `pop()`).
    cache: ElemCache,
}

/// Exclusive access for a mutation. Mutable access marks the storage as changed; on
/// release (still under the lock) a change bumps the mutation counter and publishes the
/// new length. Failed and no-op operations only read, so they leave the counter alone.
struct Mutation<'a> {
    guard: WriteGuard<'a, Storage>,
    list: &'a List,
    changed: bool,
}

impl Mutation<'_> {
    /// Publish the change (if any) now and return the resulting mutation counter.
    fn finish(mut self) -> u64 {
        self.publish();
        self.list.version.get()
    }

    fn publish(&mut self) {
        if self.changed {
            self.changed = false;
            self.list.len.store(self.guard.len(), Ordering::Release);
            self.list.version.bump_exclusive();
        }
    }
}

impl Deref for Mutation<'_> {
    type Target = Storage;
    fn deref(&self) -> &Storage {
        &self.guard
    }
}

impl DerefMut for Mutation<'_> {
    fn deref_mut(&mut self) -> &mut Storage {
        if !self.changed {
            // SAFETY: the write guard gives exclusive access; attached.
            unsafe { self.list.cache.clear() };
        }
        self.changed = true;
        &mut self.guard
    }
}

impl Drop for Mutation<'_> {
    fn drop(&mut self) {
        self.publish();
    }
}

/// A subscript: an index or the raw (unadjusted) `start, stop, step` of a slice.
enum Index {
    Int(isize),
    Slice(isize, isize, isize),
}

impl Index {
    fn parse(obj: &Bound<'_, PyAny>) -> PyResult<Index> {
        let py = obj.py();
        let p = obj.as_ptr();
        // SAFETY: type checks and C-API conversions on a valid object.
        unsafe {
            if ffi::PyLong_CheckExact(p) != 0 {
                let i = ffi::PyLong_AsSsize_t(p);
                if i == -1 && PyErr::take(py).is_some() {
                    return Err(PyIndexError::new_err("cannot fit 'int' into an index-sized integer"));
                }
                return Ok(Index::Int(i));
            }
            if ffi::PySlice_Check(p) != 0 {
                let (mut start, mut stop, mut step) = (0, 0, 0);
                if ffi::PySlice_Unpack(p, &mut start, &mut stop, &mut step) < 0 {
                    return Err(PyErr::fetch(py));
                }
                return Ok(Index::Slice(start, stop, step));
            }
            if ffi::PyIndex_Check(p) != 0 {
                let i = ffi::PyNumber_AsSsize_t(p, ffi::PyExc_IndexError);
                if i == -1
                    && let Some(e) = PyErr::take(py)
                {
                    return Err(e);
                }
                return Ok(Index::Int(i));
            }
        }
        Err(PyTypeError::new_err(format!("list indices must be integers or slices, not {}", convert::type_name(obj))))
    }
}

/// Adjust an unpacked slice to `len` (`PySlice_AdjustIndices`).
fn adjust(start: isize, stop: isize, step: isize, len: usize) -> PyResult<SliceIndices> {
    SliceIndices::new(Some(start), Some(stop), Some(step), len).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// An argument that supports `__index__` (anything else makes binary operators return
/// `NotImplemented`).
struct IndexArg<'py>(Bound<'py, PyAny>);

impl<'a, 'py> FromPyObject<'a, 'py> for IndexArg<'py> {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
        // SAFETY: protocol check on a valid object.
        if unsafe { ffi::PyIndex_Check(obj.as_ptr()) } != 0 {
            Ok(IndexArg(obj.to_owned()))
        } else {
            Err(PyTypeError::new_err("expected an integer"))
        }
    }
}

impl IndexArg<'_> {
    /// The value as `isize`; `OverflowError` when it does not fit (like list repetition).
    fn value(&self) -> PyResult<isize> {
        // SAFETY: `__index__`-capable object.
        let n = unsafe { ffi::PyNumber_AsSsize_t(self.0.as_ptr(), ffi::PyExc_OverflowError) };
        if n == -1
            && let Some(e) = PyErr::take(self.0.py())
        {
            return Err(e);
        }
        Ok(n)
    }
}

/// `start`/`stop` of `list.index`: omitted (the default applies) or an object that must
/// support `__index__` (`None` is rejected, like `list.index`).
enum SliceBound<'py> {
    Omitted,
    Given(Bound<'py, PyAny>),
}

impl<'a, 'py> FromPyObject<'a, 'py> for SliceBound<'py> {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
        Ok(SliceBound::Given(obj.to_owned()))
    }
}

impl SliceBound<'_> {
    /// The bound via `__index__` (clamped to `isize`), `default` if omitted. May run Python
    /// code, so callers convert both bounds before looking at the list (as CPython does).
    fn value(self, default: isize) -> PyResult<isize> {
        let SliceBound::Given(obj) = self else {
            return Ok(default);
        };
        // SAFETY: protocol check and clamping conversion on a valid object.
        unsafe {
            if ffi::PyIndex_Check(obj.as_ptr()) == 0 {
                return Err(PyTypeError::new_err("slice indices must be integers or have an __index__ method"));
            }
            let v = ffi::PyNumber_AsSsize_t(obj.as_ptr(), std::ptr::null_mut());
            if v == -1
                && let Some(e) = PyErr::take(obj.py())
            {
                return Err(e);
            }
            Ok(v)
        }
    }
}

/// A `list.index` bound against the current length: negative values count from the end.
fn clamp_bound(v: isize, len: usize) -> usize {
    if v < 0 { v.saturating_add(len as isize).max(0) as usize } else { v as usize }
}

/// Whether comparing `p` with another such object runs no Python code: an exact `int`,
/// `float`, `str` or `bool`, or `None` (`bytes` is left out: `bytes == str` may warn).
#[inline]
fn is_scalar(p: *mut ffi::PyObject) -> bool {
    // SAFETY: a type pointer read and comparisons on a live object.
    unsafe {
        let ty = ffi::Py_TYPE(p);
        ty == &raw mut ffi::PyLong_Type
            || ty == &raw mut ffi::PyUnicode_Type
            || ty == &raw mut ffi::PyFloat_Type
            || ty == &raw mut ffi::PyBool_Type
            || p == ffi::Py_None()
    }
}

/// Element equality for searches and list comparison: identity, then NaN equals NaN
/// (the documented List semantics, identical for every strategy), then Python `==`.
fn elements_equal(item: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<bool> {
    // SAFETY: valid objects, attached.
    match unsafe { elements_equal_ptr(item.as_ptr(), value.as_ptr()) } {
        -1 => Err(PyErr::fetch(item.py())),
        r => Ok(r == 1),
    }
}

/// [`elements_equal`] on raw pointers: 1 if equal, 0 if not, -1 with an exception set.
///
/// # Safety
/// `a` and `b` must be live objects; the caller must be attached.
#[inline]
unsafe fn elements_equal_ptr(a: *mut ffi::PyObject, b: *mut ffi::PyObject) -> c_int {
    // SAFETY: exact type checks and value reads on live objects.
    unsafe {
        if a == b {
            return 1;
        }
        if ffi::PyFloat_CheckExact(a) != 0
            && ffi::PyFloat_CheckExact(b) != 0
            && ffi::PyFloat_AS_DOUBLE(a).is_nan()
            && ffi::PyFloat_AS_DOUBLE(b).is_nan()
        {
            return 1;
        }
        ffi::PyObject_RichCompareBool(a, b, ffi::Py_EQ)
    }
}

/// `PyObject_RichCompareBool(a, b, op)`: identity implies equality.
fn compare_bool(a: &Bound<'_, PyAny>, b: &Bound<'_, PyAny>, op: c_int) -> PyResult<bool> {
    // SAFETY: valid objects; -1 signals an exception.
    match unsafe { ffi::PyObject_RichCompareBool(a.as_ptr(), b.as_ptr(), op) } {
        -1 => Err(PyErr::fetch(a.py())),
        r => Ok(r == 1),
    }
}

pub(crate) fn compare_lengths(a: usize, b: usize, op: CompareOp) -> bool {
    op.matches(a.cmp(&b))
}

fn cmp_op(op: CompareOp) -> Option<CmpOp> {
    match op {
        CompareOp::Lt => Some(CmpOp::Lt),
        CompareOp::Le => Some(CmpOp::Le),
        CompareOp::Gt => Some(CmpOp::Gt),
        CompareOp::Ge => Some(CmpOp::Ge),
        CompareOp::Eq | CompareOp::Ne => None,
    }
}

/// Native comparison of two columns, or `None` where Python raises `TypeError` (ordering a
/// `str` against a number); the object path then raises it with CPython's message.
pub(crate) fn native_compare(a: &Column, b: &Column, op: CompareOp) -> Option<bool> {
    match cmp_op(op) {
        Some(cmp) => kernels::try_lex_compare(a, b, cmp),
        None => Some(kernels::equal(a, b) == matches!(op, CompareOp::Eq)),
    }
}

fn buffer_format(dtype: DType) -> &'static CStr {
    match dtype {
        DType::Bool => c"?",
        DType::I8 => c"b",
        DType::I16 => c"h",
        DType::I32 => c"i",
        DType::I64 => c"q",
        DType::U8 => c"B",
        DType::U16 => c"H",
        DType::U32 => c"I",
        DType::U64 => c"Q",
        DType::F32 => c"f",
        DType::F64 => c"d",
        DType::Str => c"",
    }
}

/// One side of a Python-level comparison: a rustique `List` or a builtin `list` (or
/// subclass, read through its item array like CPython's `list_richcompare`), both read live.
enum Side<'a, 'py> {
    List(&'a List),
    Builtin(&'a Bound<'py, PyList>),
}

impl<'py> Side<'_, 'py> {
    fn len(&self) -> usize {
        match self {
            Side::List(list) => list.len(),
            Side::Builtin(list) => list.len(),
        }
    }

    /// New reference to element `i` of the current contents, or `None` past the end.
    fn get(&self, py: Python<'py>, i: usize) -> Option<Bound<'py, PyAny>> {
        match self {
            Side::List(list) => list.get_ref(py, i),
            Side::Builtin(list) => (i < list.len()).then(|| list.get_item(i).ok()).flatten(),
        }
    }
}

impl List {
    fn new_with(schema: Arc<Schema>, storage: Storage) -> List {
        List {
            schema,
            len: AtomicUsize::new(storage.len()),
            data: PyRwLock::new(storage),
            version: MutationCounter::new(),
            exports: AtomicUsize::new(0),
            sorting: AtomicUsize::new(0),
            cache: ElemCache::new(),
        }
    }

    /// Wrap storage that is valid for `schema` in a new `List` (no conversion).
    pub fn from_storage(py: Python<'_>, schema: Arc<Schema>, storage: Storage) -> PyResult<Bound<'_, List>> {
        LIST_CLASS.new_bound(py, List::new_with(schema, storage))
    }

    /// INTERFACE (native-tier rewrite, WS1): an untyped `List` from a *temporary* builtin
    /// `list`, e.g. the result of a list display or comprehension passed straight in.
    /// Native items are converted as usual; otherwise the item references are moved out of
    /// the temporary (no per-item reference counting) and it is left empty.
    ///
    /// The steal only happens when `list` is an exact `list` that this reference is the
    /// only one to (`PyUnstable_Object_IsUniquelyReferenced`); otherwise the items are
    /// copied like `List(list)`. Callers must still only pass true temporaries: since
    /// CPython 3.14 a local variable can lend its list to the stack without a reference
    /// (`LOAD_FAST_BORROW`), so a reference count of one does not prove that no variable
    /// refers to the list.
    pub fn from_temporary_list<'py>(py: Python<'py>, list: Bound<'py, PyList>) -> PyResult<Bound<'py, List>> {
        let p = list.as_ptr();
        // SAFETY: type and uniqueness checks on a live object.
        let unique = unsafe { ffi::PyList_CheckExact(p) != 0 && ffi::PyUnstable_Object_IsUniquelyReferenced(p) == 1 };
        let storage = if unique {
            // SAFETY: exact and uniquely referenced (checked) and a temporary (contract).
            unsafe { convert::take_temporary_list(py, &list) }
        } else {
            convert::ingest(py, &schema::any(), list.as_any())?
        };
        drop(list);
        Self::from_storage(py, schema::any(), storage)
    }

    /// INTERFACE. Construct a `List` with `schema` from an optional iterable.
    pub fn from_iterable<'py>(
        py: Python<'py>,
        schema: Arc<Schema>,
        iterable: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, List>> {
        let storage = match iterable {
            Some(obj) => convert::ingest(py, &schema, obj)?,
            None => Storage::for_schema(&schema),
        };
        Self::from_storage(py, schema, storage)
    }

    /// INTERFACE. Wrap a native column without per-element conversion (O(1)). The list is
    /// typed by the column's dtype: `List[int]`, `List[float]`, `List[bool]`, `List[str]`,
    /// or `List[rs.<dtype>]` for the fixed-width types.
    pub fn from_column(py: Python<'_>, column: Column) -> PyResult<Bound<'_, List>> {
        let schema = schema::intern(match column.dtype() {
            DType::Bool => Schema::Bool,
            DType::I64 => Schema::Int,
            DType::F64 => Schema::Float,
            DType::Str => Schema::Str,
            dtype => Schema::Fixed(dtype),
        });
        Self::from_storage(py, schema, Storage::Native(column))
    }

    /// INTERFACE. The element schema.
    pub fn schema(&self) -> &Arc<Schema> {
        &self.schema
    }

    /// Run `f` on the native column under the read lock (`None` for non-native storage).
    /// `f` must not call into Python.
    pub fn with_column<R>(&self, py: Python<'_>, f: impl FnOnce(&Column) -> R) -> Option<R> {
        match &*self.read(py) {
            Storage::Native(column) => Some(f(column)),
            _ => None,
        }
    }

    /// An owned copy of the storage (native: column copy; object: increfs).
    pub fn snapshot(&self, py: Python<'_>) -> Storage {
        self.read(py).clone_ref(py)
    }

    /// Current number of elements (no lock needed).
    #[inline]
    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    fn read(&self, py: Python<'_>) -> ReadGuard<'_, Storage> {
        self.data.read(py)
    }

    /// The write guard for a fast-path mutation, `None` while a buffer export exists (the
    /// caller then takes the general path, which raises). The caller publishes the change
    /// (`publish_len`, or `version.bump_exclusive` when the length is unchanged).
    #[inline(always)]
    fn write_fast(&self, py: Python<'_>) -> Option<WriteGuard<'_, Storage>> {
        let guard = self.data.write(py);
        (self.exports.load(Ordering::Acquire) == 0).then_some(guard)
    }

    /// Publish a mutation made through `write_fast` that changed the length (still under
    /// the write guard).
    #[inline(always)]
    fn publish_len(&self, len: usize) {
        self.len.store(len, Ordering::Release);
        self.version.bump_exclusive();
    }

    #[inline]
    fn mutate(&self, py: Python<'_>) -> PyResult<Mutation<'_>> {
        let guard = self.data.write(py);
        if self.exports.load(Ordering::Acquire) != 0 {
            return Err(PyBufferError::new_err("cannot modify List while a buffer export exists"));
        }
        Ok(Mutation { guard, list: self, changed: false })
    }

    /// Convert values about to be added to this list. Untyped lists that already store
    /// objects keep the incoming objects themselves (identity), since the strategy can
    /// never become native again.
    fn ingest_for_update(&self, py: Python<'_>, obj: &Bound<'_, PyAny>) -> Result<Storage, IngestError> {
        if self.schema.is_any() && matches!(&*self.read(py), Storage::Object(_)) {
            return convert::ingest_objects_raw(py, obj);
        }
        convert::ingest(py, &self.schema, obj)
    }

    fn container(&self) -> String {
        convert::container_name(&self.schema)
    }

    #[inline]
    fn coerce_one(&self, obj: &Bound<'_, PyAny>) -> PyResult<Coerced> {
        self.schema.coerce(obj).map_err(|e| e.into_pyerr(&self.container(), None))
    }

    /// New reference to element `i` of the current list, or `None` past the end.
    /// New reference to element `k` (in range) for an element-wise read, through the
    /// boxed element cache.
    #[inline(always)]
    fn read_item(&self, py: Python<'_>, data: &Storage, k: usize) -> *mut ffi::PyObject {
        if let Storage::Native(_) = data {
            self.cache.note_read(data.len());
        }
        self.cache.item(py, data, k)
    }

    #[inline]
    fn get_ref<'py>(&self, py: Python<'py>, i: usize) -> Option<Bound<'py, PyAny>> {
        let data = self.read(py);
        // SAFETY: `item_ptr` returns a new reference.
        (i < data.len()).then(|| unsafe { Bound::from_owned_ptr(py, data.item_ptr(py, i)) })
    }

    /// First index in `start..stop` whose element equals `value`.
    fn find(&self, py: Python<'_>, value: &Bound<'_, PyAny>, start: usize, stop: usize) -> PyResult<Option<usize>> {
        let probe = convert::probe(value);
        {
            let data = self.read(py);
            let stop = stop.min(data.len());
            if start >= stop {
                return Ok(None);
            }
            match (&*data, &probe) {
                (Storage::Native(column), Probe::Native(needle)) => {
                    return Ok(kernels::find(column, needle, start, stop));
                }
                (Storage::Native(_), Probe::Never) | (Storage::Empty, _) => return Ok(None),
                _ => {}
            }
        }
        Ok(self.find_python(py, value, start, stop)?.map(|(i, _)| i))
    }

    /// Python-level search. Returns the index and the element that compared equal.
    fn find_python(
        &self,
        py: Python<'_>,
        value: &Bound<'_, PyAny>,
        start: usize,
        stop: usize,
    ) -> PyResult<Option<(usize, Py<PyAny>)>> {
        let mut found = None;
        self.scan_equal(py, value, start, stop, |i, item| {
            // SAFETY: a live element (borrowed under the lock, or owned by the caller).
            found = Some((i, unsafe { Bound::from_borrowed_ptr(py, item) }.unbind()));
            true
        })?;
        Ok(found)
    }

    /// Compare `self[start..stop]` with `value` by Python equality (identity, NaN, `==`),
    /// calling `on_equal(index, item)` for each equal element until it returns `true`.
    ///
    /// Pairs of builtin scalars (`int`, `float`, `str`, `bool`, `None`) are compared under
    /// the read lock: such comparisons run no Python code, so a scan over them is atomic
    /// with respect to other threads' mutations (like CPython's `list.remove`, which holds
    /// the list's critical section). Any other comparison runs unlocked on a reference to
    /// the element, and the scan then continues at the next index of the *current* list (a
    /// comparison may run Python code that changes it; CPython re-reads `ob_item[i]`).
    /// `on_equal` must not run Python code. Runs no `Py` drop (raw-slot safe).
    fn scan_equal(
        &self,
        py: Python<'_>,
        value: &Bound<'_, PyAny>,
        start: usize,
        stop: usize,
        mut on_equal: impl FnMut(usize, *mut ffi::PyObject) -> bool,
    ) -> PyResult<()> {
        let v = value.as_ptr();
        let scalar = is_scalar(v);
        let mut i = start;
        loop {
            let pending = {
                let data = self.read(py);
                let end = stop.min(data.len());
                loop {
                    if i >= end {
                        return Ok(());
                    }
                    match &*data {
                        Storage::Object(objects) if scalar && is_scalar(objects[i].as_ptr()) => {
                            let p = objects[i].as_ptr();
                            // SAFETY: live objects; scalar comparisons run no Python code.
                            match unsafe { elements_equal_ptr(p, v) } {
                                0 => {}
                                1 => {
                                    if on_equal(i, p) {
                                        return Ok(());
                                    }
                                }
                                _ => {
                                    drop(data);
                                    return Err(PyErr::fetch(py));
                                }
                            }
                            i += 1;
                        }
                        _ => break data.item_ptr(py, i),
                    }
                }
            };
            // SAFETY: `item_ptr` returned a new reference.
            let item = unsafe { Bound::from_owned_ptr(py, pending) };
            if elements_equal(&item, value)? && on_equal(i, item.as_ptr()) {
                return Ok(());
            }
            i += 1;
        }
    }

    fn count_of(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<usize> {
        let probe = convert::probe(value);
        {
            let data = self.read(py);
            match (&*data, &probe) {
                (Storage::Native(column), Probe::Native(needle)) => {
                    return Ok(if column.len() >= DETACH_THRESHOLD {
                        py.detach(|| kernels::count(column, needle))
                    } else {
                        kernels::count(column, needle)
                    });
                }
                (Storage::Native(_), Probe::Never) | (Storage::Empty, _) => return Ok(0),
                _ => {}
            }
        }
        let mut count = 0;
        self.scan_equal(py, value, 0, usize::MAX, |_, _| {
            count += 1;
            false
        })?;
        Ok(count)
    }

    /// A new builtin `list` with the current elements.
    fn to_pylist<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        loop {
            let n = self.read(py).len();
            // Allocate the (GC-tracked) list outside the lock, fill it under the lock.
            // SAFETY: PyList_New returns a new reference with `n` NULL slots.
            let list = unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyList_New(n as ffi::Py_ssize_t))? };
            let data = self.read(py);
            if data.len() != n {
                continue;
            }
            let p = list.as_ptr();
            // SAFETY: `list` is a fresh list of length `n` that nobody else can see; each
            // slot is filled exactly once with a new (stolen) reference.
            unsafe {
                match &*data {
                    Storage::Empty => {}
                    Storage::Native(column) => with_column!(column, v => {
                        for (i, x) in v.iter().enumerate() {
                            ffi::PyList_SET_ITEM(p, i as ffi::Py_ssize_t, x.to_python(py).into_ptr());
                        }
                    }),
                    Storage::Object(objects) => {
                        for (i, o) in objects.iter().enumerate() {
                            ffi::PyList_SET_ITEM(p, i as ffi::Py_ssize_t, o.clone_ref(py).into_ptr());
                        }
                    }
                }
            }
            drop(data);
            // SAFETY: `list` was created as a list above.
            return Ok(unsafe { list.cast_into_unchecked() });
        }
    }

    /// `repr` of the elements as `[a, b, …]`. Like CPython's `list_repr`, each element is
    /// taken from the current list (an element's `__repr__` may change it). Native
    /// elements' reprs run no Python code.
    fn repr_items<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyString>> {
        if !matches!(&*self.read(py), Storage::Object(_)) {
            return self.to_pylist(py)?.repr();
        }
        let mut parts = Vec::new();
        let mut i = 0;
        while let Some(item) = self.get_ref(py, i) {
            parts.push(item.repr()?);
            i += 1;
        }
        let body = PyString::new(py, ", ").call_method1(intern!(py, "join"), (parts,))?;
        PyString::new(py, "[").add(body)?.add("]")?.cast_into::<PyString>().map_err(Into::into)
    }

    /// Python-level comparison (CPython's `list_richcompare`): find the first index where
    /// the elements differ (identity implies equality), comparing outside the locks. Each
    /// step reads the current lists (a comparison may run Python code that changes them).
    fn richcompare_python(py: Python<'_>, a: Side<'_, '_>, b: Side<'_, '_>, op: CompareOp) -> PyResult<Py<PyAny>> {
        let result = |v: bool| Ok(PyBool::new(py, v).to_owned().into_any().unbind());
        if a.len() != b.len() && matches!(op, CompareOp::Eq | CompareOp::Ne) {
            return result(matches!(op, CompareOp::Ne));
        }
        let mut i = 0;
        while let Some((x, y)) = Self::next_pair(py, &a, &b, &mut i)? {
            if !elements_equal(&x, &y)? {
                return match op {
                    CompareOp::Eq => result(false),
                    CompareOp::Ne => result(true),
                    _ => Ok(x.rich_compare(&y, op)?.unbind()),
                };
            }
            i += 1;
        }
        // No more items to compare: the (current) lengths decide.
        result(compare_lengths(a.len(), b.len(), op))
    }

    /// The pair of elements at index `*i` (new references), or `None` once either side is
    /// exhausted. Between two rustique lists, pairs of identical objects or equal builtin
    /// scalars are skipped under both read locks (no Python code runs, so that part of the
    /// comparison is atomic, like CPython's critical section on both lists).
    fn next_pair<'py>(
        py: Python<'py>,
        a: &Side<'_, 'py>,
        b: &Side<'_, 'py>,
        i: &mut usize,
    ) -> PyResult<Option<(Bound<'py, PyAny>, Bound<'py, PyAny>)>> {
        if let (Side::List(la), Side::List(lb)) = (a, b)
            && !std::ptr::eq(*la, *lb)
        {
            let (da, db) = read_both(py, &la.data, &lb.data);
            if let (Storage::Object(oa), Storage::Object(ob)) = (&*da, &*db) {
                while *i < oa.len() && *i < ob.len() {
                    let (p, q) = (oa[*i].as_ptr(), ob[*i].as_ptr());
                    if p != q && !(is_scalar(p) && is_scalar(q)) {
                        break;
                    }
                    // SAFETY: live objects; identical or scalar comparisons run no Python code.
                    match unsafe { elements_equal_ptr(p, q) } {
                        1 => *i += 1,
                        0 => break,
                        _ => {
                            drop((da, db));
                            return Err(PyErr::fetch(py));
                        }
                    }
                }
            }
            if *i >= da.len() || *i >= db.len() {
                return Ok(None);
            }
            // SAFETY: `item_ptr` returns new references.
            return Ok(Some(unsafe {
                (Bound::from_owned_ptr(py, da.item_ptr(py, *i)), Bound::from_owned_ptr(py, db.item_ptr(py, *i)))
            }));
        }
        Ok(match (a.get(py, *i), b.get(py, *i)) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        })
    }

    /// Stable permutation that sorts `keys` with Python `<` (`reverse` keeps equal
    /// elements in their original order, like `list.sort`).
    fn sort_permutation(py: Python<'_>, keys: &[Py<PyAny>], reverse: bool) -> PyResult<Vec<usize>> {
        let n = keys.len();
        let mut perm: Vec<usize> = if reverse { (0..n).rev().collect() } else { (0..n).collect() };
        if let Some(native) = NativeKeys::classify(py, keys) {
            match native {
                NativeKeys::Int(k) => perm.sort_by_key(|&i| k[i]),
                NativeKeys::Float(k) => {
                    perm.sort_by(|&a, &b| k[a].partial_cmp(&k[b]).unwrap_or(std::cmp::Ordering::Equal))
                }
                NativeKeys::Str(k) => perm.sort_by(|&a, &b| k[a].cmp(&k[b])),
            }
        } else {
            merge_sort(&mut perm, |a, b| compare_bool(keys[a].bind(py), keys[b].bind(py), ffi::Py_LT))?;
        }
        if reverse {
            perm.reverse();
        }
        Ok(perm)
    }
}

/// Sort keys that can be compared natively with exactly Python's ordering.
enum NativeKeys {
    Int(Vec<i64>),
    /// No NaN (otherwise `<` is not a total order and we defer to Python).
    Float(Vec<f64>),
    Str(Vec<compact_str::CompactString>),
}

impl NativeKeys {
    fn classify(py: Python<'_>, keys: &[Py<PyAny>]) -> Option<NativeKeys> {
        let first = keys.first()?.bind(py);
        let first_value = match convert::classify(first) {
            Coerced::Native(v) => v,
            Coerced::Object(_) => return None,
        };
        let all = |f: &mut dyn FnMut(rustique_core::Value) -> bool| {
            keys.iter().all(|k| match convert::classify(k.bind(py)) {
                Coerced::Native(v) => f(v),
                Coerced::Object(_) => false,
            })
        };
        use rustique_core::Value;
        match first_value {
            Value::I64(_) | Value::Bool(_) => {
                let mut out = Vec::with_capacity(keys.len());
                all(&mut |v| match v {
                    Value::I64(x) => {
                        out.push(x);
                        true
                    }
                    Value::Bool(b) => {
                        out.push(i64::from(b));
                        true
                    }
                    _ => false,
                })
                .then_some(NativeKeys::Int(out))
            }
            Value::F64(_) => {
                let mut out = Vec::with_capacity(keys.len());
                all(&mut |v| match v {
                    Value::F64(x) if !x.is_nan() => {
                        out.push(x);
                        true
                    }
                    _ => false,
                })
                .then_some(NativeKeys::Float(out))
            }
            Value::Str(_) => {
                let mut out = Vec::with_capacity(keys.len());
                all(&mut |v| match v {
                    Value::Str(s) => {
                        out.push(s);
                        true
                    }
                    _ => false,
                })
                .then_some(NativeKeys::Str(out))
            }
            _ => None,
        }
    }
}

/// Stable natural merge sort over indices with a fallible `lt`. Robust to inconsistent
/// comparisons (always terminates with a permutation, never panics); presorted input
/// costs about `n` comparisons.
fn merge_sort(v: &mut [usize], mut lt: impl FnMut(usize, usize) -> PyResult<bool>) -> PyResult<()> {
    const RUN: usize = 32;
    let n = v.len();
    let mut start = 0;
    while start < n {
        let end = (start + RUN).min(n);
        // Binary insertion sort, skipping elements already in place.
        for j in start + 1..end {
            let x = v[j];
            if !lt(x, v[j - 1])? {
                continue;
            }
            let (mut lo, mut hi) = (start, j - 1);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if lt(x, v[mid])? {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            v.copy_within(lo..j, lo + 1);
            v[lo] = x;
        }
        start = end;
    }
    let mut buf = vec![0usize; n];
    let mut width = RUN;
    while width < n {
        let mut lo = 0;
        while lo + width < n {
            let mid = lo + width;
            let hi = (lo + 2 * width).min(n);
            if lt(v[mid], v[mid - 1])? {
                let (mut i, mut j, mut k) = (lo, mid, lo);
                while i < mid && j < hi {
                    if lt(v[j], v[i])? {
                        buf[k] = v[j];
                        j += 1;
                    } else {
                        buf[k] = v[i];
                        i += 1;
                    }
                    k += 1;
                }
                buf[k..k + (mid - i)].copy_from_slice(&v[i..mid]);
                k += mid - i;
                buf[k..k + (hi - j)].copy_from_slice(&v[j..hi]);
                v[lo..hi].copy_from_slice(&buf[lo..hi]);
            }
            lo = hi;
        }
        width *= 2;
    }
    Ok(())
}

/// Sort Python objects with CPython's own `list.sort` (timsort, `key`/`reverse`, exact
/// comparison and exception semantics): the references move into a temporary builtin list,
/// are sorted there and move back (no reference counting per element).
///
/// INTERFACE HOOK (WS5): `objsort::sort_objects` implements the same contract for the
/// builtins workstream; when it lands, this function can delegate to it.
///
/// Returns the objects (sorted, or in the permutation `list.sort` left them in when it
/// raised) and the result.
fn sort_objects(
    py: Python<'_>,
    objects: Vec<Py<PyAny>>,
    key: Option<&Bound<'_, PyAny>>,
    reverse: bool,
) -> (Vec<Py<PyAny>>, PyResult<()>) {
    let n = objects.len();
    // SAFETY: a new list of `n` empty slots, each filled once with a reference moved out
    // of `objects`; nobody else can reach the list.
    let list = match unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyList_New(n as ffi::Py_ssize_t)) } {
        Ok(list) => list,
        Err(e) => return (objects, Err(e)),
    };
    for (i, o) in objects.into_iter().enumerate() {
        // SAFETY: see above.
        unsafe { ffi::PyList_SET_ITEM(list.as_ptr(), i as ffi::Py_ssize_t, o.into_ptr()) };
    }
    let result = (|| {
        let kwargs = PyDict::new(py);
        if let Some(key) = key {
            kwargs.set_item(intern!(py, "key"), key)?;
        }
        if reverse {
            kwargs.set_item(intern!(py, "reverse"), true)?;
        }
        list.call_method(intern!(py, "sort"), (), Some(&kwargs)).map(drop)
    })();
    // Move the references back out and leave the temporary empty (its deallocation then
    // releases nothing). `list.sort` keeps every element whether or not it raised.
    // SAFETY: the list is private; its size is read after the sort finished.
    let objects = unsafe {
        let p = list.as_ptr();
        let len = ffi::PyList_GET_SIZE(p) as usize;
        let items = (*p.cast::<ffi::PyListObject>()).ob_item;
        let out = (0..len).map(|i| Bound::from_owned_ptr(py, *items.add(i)).unbind()).collect::<Vec<_>>();
        (*p.cast::<ffi::PyVarObject>()).ob_size = 0;
        out
    };
    (objects, result)
}

/// Sort taken-out list elements with Python semantics (`key` calls, Python `<` unless the
/// keys compare natively). Returns the sorted storage, or the elements in their original
/// order (or, for object storage, the permutation `list.sort` left) together with the
/// exception that stopped the sort.
fn sort_storage(
    py: Python<'_>,
    elements: Storage,
    key: Option<&Bound<'_, PyAny>>,
    reverse: bool,
) -> (Storage, PyResult<()>) {
    let elements = match elements {
        Storage::Object(objects) => {
            let (objects, result) = sort_objects(py, objects, key, reverse);
            return (Storage::Object(objects), result);
        }
        other => other,
    };
    let perm = {
        let materialised = match &elements {
            Storage::Native(column) => Some(convert::column_objects(py, column, 0..column.len())),
            _ => None,
        };
        let objects: &[Py<PyAny>] = match (&materialised, &elements) {
            (Some(objects), _) | (None, Storage::Object(objects)) => objects,
            _ => &[],
        };
        let perm = match key {
            Some(f) => objects
                .iter()
                .map(|e| f.call1((e,)).map(Bound::unbind))
                .collect::<PyResult<Vec<_>>>()
                .and_then(|keys| List::sort_permutation(py, &keys, reverse)),
            None => List::sort_permutation(py, objects, reverse),
        };
        match perm {
            Ok(perm) => perm,
            Err(e) => return (elements, Err(e)),
        }
    };
    let sorted = match elements {
        Storage::Native(column) => Storage::Native(column.take(&perm)),
        Storage::Object(objects) => {
            let mut slots: Vec<Option<Py<PyAny>>> = objects.into_iter().map(Some).collect();
            Storage::Object(perm.iter().filter_map(|&i| slots[i].take()).collect())
        }
        Storage::Empty => Storage::Empty,
    };
    (sorted, Ok(()))
}

#[pymethods]
impl List {
    /// `List.__new__` creates an empty list and `__init__` fills it, like `list` (so
    /// subclasses and `x.__init__(iterable)` behave like the builtin). `List(iterable)`
    /// itself takes a vectorcall fast path doing both at once (see `slots`).
    #[new]
    #[pyo3(signature = (*_args, **_kwargs), text_signature = "(iterable=(), /)")]
    fn py_new(_args: &Bound<'_, PyTuple>, _kwargs: Option<&Bound<'_, PyDict>>) -> Self {
        List::new_with(schema::any(), Storage::Empty)
    }

    #[classmethod]
    fn __class_getitem__<'py>(cls: &Bound<'py, PyType>, item: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        crate::typing::list_alias(cls.py(), item)
    }

    #[classattr]
    const __hash__: Option<Py<PyAny>> = None;

    /// The storage strategy: `"empty"`, `"bool"`, `"i8"` … `"f64"`, `"str"` or `"object"`.
    #[getter]
    fn strategy(&self, py: Python<'_>) -> &'static str {
        self.read(py).strategy()
    }

    /// The alias describing this list's element type (`List[int]`, or `List` if untyped).
    #[getter]
    fn schema_type<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if self.schema.is_any() {
            return Ok(py.get_type::<List>().into_any());
        }
        crate::typing::list_alias_for(py, &self.schema)
    }

    fn __len__(&self) -> usize {
        self.len()
    }

    fn __bool__(&self) -> bool {
        !self.is_empty()
    }

    fn __getitem__<'py>(slf: &Bound<'py, Self>, index: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        let this = slf.get();
        match Index::parse(index)? {
            Index::Int(i) => {
                let data = this.read(py);
                match normalize_index(i, data.len()) {
                    Some(k) => Ok(data.item(py, k)),
                    None => Err(PyIndexError::new_err("list index out of range")),
                }
            }
            Index::Slice(start, stop, step) => {
                let storage = {
                    let data = this.read(py);
                    data.take_slice(py, &adjust(start, stop, step, data.len())?)
                };
                Ok(List::from_storage(py, this.schema.clone(), storage)?.into_any())
            }
        }
    }

    fn __setitem__(slf: &Bound<'_, Self>, index: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = slf.py();
        let this = slf.get();
        match Index::parse(index)? {
            Index::Int(i) => {
                let typed = if this.schema.is_any() { None } else { Some(this.coerce_one(value)?) };
                let displaced = {
                    let mut data = this.mutate(py)?;
                    let Some(k) = normalize_index(i, data.len()) else {
                        return Err(PyIndexError::new_err("list assignment index out of range"));
                    };
                    match typed {
                        Some(coerced) => data.set_one(py, k, coerced),
                        None => data.set_any(py, k, value),
                    }
                };
                drop(displaced);
                Ok(())
            }
            Index::Slice(start, stop, step) => {
                if !convert::is_iterable(value) {
                    // CPython (3.13+) uses this message for simple slices too.
                    return Err(PyTypeError::new_err("must assign iterable to extended slice"));
                }
                let values = this.ingest_for_update(py, value)?;
                let result = {
                    let mut data = this.mutate(py)?;
                    let s = adjust(start, stop, step, data.len())?;
                    // Rejections and no-ops leave the list (and its mutation counter) alone;
                    // `values` is dropped after the guard.
                    if s.step != 1 && values.len() != s.len {
                        let e = SliceError::LengthMismatch { expected: s.len, got: values.len() };
                        return Err(PyValueError::new_err(e.to_string()));
                    }
                    if s.len == 0 && values.is_empty() {
                        return Ok(());
                    }
                    data.assign_slice(py, &s, values)
                };
                // Displaced or rejected objects are dropped only now, unlocked.
                match result {
                    Ok(displaced) => drop(displaced),
                    Err((e, rejected)) => {
                        drop(rejected);
                        return Err(PyValueError::new_err(e.to_string()));
                    }
                }
                Ok(())
            }
        }
    }

    fn __delitem__(&self, py: Python<'_>, index: &Bound<'_, PyAny>) -> PyResult<()> {
        let displaced = match Index::parse(index)? {
            Index::Int(i) => {
                let mut data = self.mutate(py)?;
                let Some(k) = normalize_index(i, data.len()) else {
                    return Err(PyIndexError::new_err("list assignment index out of range"));
                };
                data.delete(k).into_iter().collect()
            }
            Index::Slice(start, stop, step) => {
                let mut data = self.mutate(py)?;
                let s = adjust(start, stop, step, data.len())?;
                if s.len == 0 {
                    return Ok(());
                }
                data.delete_slice(&s)
            }
        };
        drop(displaced);
        Ok(())
    }

    fn __contains__(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(self.find(py, value, 0, usize::MAX)?.is_some())
    }

    fn __iter__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        iter::new_iterator(slf.py(), slf, true)
    }

    fn __reversed__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        iter::new_iterator(slf.py(), slf, false)
    }

    fn __richcmp__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let this = slf.get();
        let result = |b: bool| Ok(PyBool::new(py, b).to_owned().into_any().unbind());
        if let Ok(other_list) = other.cast::<List>() {
            if slf.is(other) {
                return result(matches!(op, CompareOp::Eq | CompareOp::Le | CompareOp::Ge));
            }
            let that = other_list.get();
            {
                let (a, b) = read_both(py, &this.data, &that.data);
                if a.is_empty() || b.is_empty() {
                    return result(compare_lengths(a.len(), b.len(), op));
                }
                if let (Storage::Native(ca), Storage::Native(cb)) = (&*a, &*b)
                    && let Some(r) = native_compare(ca, cb, op)
                {
                    return result(r);
                }
            }
            return List::richcompare_python(py, Side::List(this), Side::List(that), op);
        }
        // SAFETY: type check on a valid object.
        if unsafe { ffi::PyList_Check(other.as_ptr()) } == 0 {
            return Ok(py.NotImplemented());
        }
        // SAFETY: checked above.
        let theirs = unsafe { other.cast_unchecked::<PyList>() };
        // An exact builtin list of native-classifiable items whose length allows equality:
        // classify it like an untyped List and compare natively.
        // SAFETY: type check on a valid object.
        let exact = unsafe { ffi::PyList_CheckExact(other.as_ptr()) } != 0;
        let worth = !matches!(op, CompareOp::Eq | CompareOp::Ne) || theirs.len() == this.len();
        if exact && worth && matches!(&*this.read(py), Storage::Native(_)) {
            let converted = convert::ingest(py, &schema::any(), other)?;
            if let Storage::Native(cb) = &converted
                && let Storage::Native(ca) = &*this.read(py)
                && let Some(r) = native_compare(ca, cb, op)
            {
                return result(r);
            }
        }
        List::richcompare_python(py, Side::List(this), Side::Builtin(theirs), op)
    }

    fn __repr__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        // SAFETY: recursion guard keyed on this object; balanced by Py_ReprLeave below.
        match unsafe { ffi::Py_ReprEnter(slf.as_ptr()) } {
            0 => {}
            r if r > 0 => return Ok(PyString::new(py, "[...]").into_any()),
            _ => return Err(PyErr::fetch(py)),
        }
        let this = slf.get();
        let result = (|| {
            let items = this.repr_items(py)?;
            let prefix = if this.schema.is_any() { "List(".to_owned() } else { format!("List[{}](", this.schema) };
            PyString::new(py, &prefix).add(items)?.add(")")
        })();
        // SAFETY: matches the successful Py_ReprEnter above.
        unsafe { ffi::Py_ReprLeave(slf.as_ptr()) };
        result
    }

    fn __add__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let this = slf.get();
        let (schema, mut left, right) = if let Ok(other_list) = other.cast::<List>() {
            let that = other_list.get();
            let (left, right) = (this.snapshot(py), that.snapshot(py));
            if Arc::ptr_eq(&this.schema, &that.schema) {
                (this.schema.clone(), left, right)
            } else {
                (schema::any(), left.normalize_any(py), right.normalize_any(py))
            }
        // SAFETY: type check on a valid object.
        } else if unsafe { ffi::PyList_Check(other.as_ptr()) } != 0 {
            let right = convert::ingest(py, &schema::any(), other)?;
            (schema::any(), this.snapshot(py).normalize_any(py), right)
        } else {
            return Ok(py.NotImplemented());
        };
        left.extend(py, right);
        Ok(List::from_storage(py, schema, left)?.into_any().unbind())
    }

    fn __radd__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        // SAFETY: type check on a valid object.
        if unsafe { ffi::PyList_Check(other.as_ptr()) } == 0 {
            return Ok(py.NotImplemented());
        }
        let mut left = convert::ingest(py, &schema::any(), other)?;
        left.extend(py, slf.get().snapshot(py).normalize_any(py));
        Ok(List::from_storage(py, schema::any(), left)?.into_any().unbind())
    }

    fn __iadd__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<()> {
        self.extend(py, other)
    }

    fn __mul__(slf: &Bound<'_, Self>, n: IndexArg<'_>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let this = slf.get();
        let n = n.value()?;
        let storage = this.read(py).repeat(py, n)?;
        Ok(List::from_storage(py, this.schema.clone(), storage)?.into_any().unbind())
    }

    fn __rmul__(slf: &Bound<'_, Self>, n: IndexArg<'_>) -> PyResult<Py<PyAny>> {
        Self::__mul__(slf, n)
    }

    fn __imul__(&self, py: Python<'_>, n: IndexArg<'_>) -> PyResult<()> {
        let n = n.value()?;
        let displaced = {
            let mut data = self.mutate(py)?;
            if n == 1 || data.is_empty() {
                return Ok(());
            }
            if n <= 0 {
                data.clear()
            } else {
                let repeated = data.repeat(py, n)?;
                // The old references are dropped unlocked, like any displaced objects (the
                // repetition holds its own references to the same objects).
                std::mem::replace(&mut *data, repeated).into_displaced()
            }
        };
        drop(displaced);
        Ok(())
    }

    fn __copy__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, List>> {
        self.copy(py)
    }

    fn __deepcopy__<'py>(slf: &Bound<'py, Self>, memo: &Bound<'py, PyAny>) -> PyResult<Bound<'py, List>> {
        let py = slf.py();
        let this = slf.get();
        let objects = match this.snapshot(py) {
            Storage::Object(objects) => objects,
            native => return List::from_storage(py, this.schema.clone(), native),
        };
        let copy = List::from_storage(py, this.schema.clone(), Storage::Object(Vec::new()))?;
        // Register before copying the elements so self-references resolve to the copy.
        memo.set_item(slf.as_ptr() as usize, &copy)?;
        let deepcopy = py.import(intern!(py, "copy"))?.getattr(intern!(py, "deepcopy"))?;
        let mut copies = Vec::with_capacity(objects.len());
        for o in &objects {
            copies.push(deepcopy.call1((o, memo))?.unbind());
        }
        drop(objects);
        let previous = std::mem::replace(&mut *copy.get().mutate(py)?, Storage::Object(copies));
        drop(previous);
        Ok(copy)
    }

    /// Pickle as `List()`/`List[T]()` plus list items: pickle memoizes the empty list
    /// before saving its items (then restored with `extend`), so self-references and
    /// shared elements round-trip.
    #[allow(clippy::type_complexity)]
    fn __reduce__<'py>(
        slf: &Bound<'py, Self>,
    ) -> PyResult<(Bound<'py, PyAny>, Bound<'py, PyTuple>, Option<Py<PyAny>>, Bound<'py, PyAny>)> {
        let py = slf.py();
        let this = slf.get();
        let items = this.to_pylist(py)?.into_any().try_iter()?.into_any();
        Ok((this.schema_type(py)?, PyTuple::empty(py), None, items))
    }

    fn __sizeof__(&self, py: Python<'_>) -> usize {
        fn bytes<T>(v: &Vec<T>) -> usize {
            v.capacity() * size_of::<T>()
        }
        let payload = match &*self.read(py) {
            Storage::Empty => 0,
            Storage::Native(column) => with_column!(column, v => bytes(v)),
            Storage::Object(objects) => bytes(objects),
        };
        // SAFETY: reading the basic size of this class's type object.
        let base = unsafe { (*py.get_type::<List>().as_type_ptr()).tp_basicsize } as usize;
        base + payload
    }

    /// Append `object` to the end of the list.
    fn append(&self, py: Python<'_>, object: &Bound<'_, PyAny>) -> PyResult<()> {
        if self.schema.is_any() {
            self.mutate(py)?.push_any(py, object);
        } else {
            let value = self.coerce_one(object)?;
            self.mutate(py)?.push(py, value);
        }
        Ok(())
    }

    /// Extend the list by appending all the items from the iterable.
    ///
    /// The iterable is consumed before anything is appended (one atomic append; items
    /// before an exception from the iterator are kept). Unlike CPython's item-by-item
    /// append, an iterable that mutates this list does not interleave with it.
    fn extend(&self, py: Python<'_>, iterable: &Bound<'_, PyAny>) -> PyResult<()> {
        let (storage, error) = match self.ingest_for_update(py, iterable) {
            Ok(storage) => (storage, None),
            Err(IngestError { error, partial: Some(storage) }) => (storage, Some(error)),
            Err(IngestError { error, partial: None }) => return Err(error),
        };
        if !storage.is_empty() {
            self.mutate(py)?.extend(py, storage);
        }
        error.map_or(Ok(()), Err)
    }

    /// Insert `object` before `index`.
    fn insert(&self, py: Python<'_>, index: isize, object: &Bound<'_, PyAny>) -> PyResult<()> {
        let typed = if self.schema.is_any() { None } else { Some(self.coerce_one(object)?) };
        let mut data = self.mutate(py)?;
        let k = clamp_insert_index(index, data.len());
        match typed {
            Some(value) => data.insert(py, k, value),
            None => data.insert_any(py, k, object),
        }
        Ok(())
    }

    /// Remove the first occurrence of `value` (`ValueError` if absent).
    fn remove(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let missing = || PyValueError::new_err("list.remove(x): x not in list");
        let probe = convert::probe(value);
        if !matches!(probe, Probe::Python) {
            // Native search and removal under one write lock.
            let mut data = self.mutate(py)?;
            let found = match (&*data, &probe) {
                (Storage::Native(column), Probe::Native(needle)) => {
                    Some(kernels::find(column, needle, 0, column.len()))
                }
                (Storage::Native(_), _) | (Storage::Empty, _) => Some(None),
                (Storage::Object(_), _) => None,
            };
            match found {
                Some(Some(i)) => {
                    // Native storage: nothing is displaced.
                    let _ = data.delete(i);
                    return Ok(());
                }
                Some(None) => return Err(missing()),
                None => {}
            }
        }
        // Python equality: compare outside the lock, then remove the element that compared
        // equal (re-located if concurrent mutation moved it; searched again if it is gone).
        // CPython instead deletes whatever is at the matching index by then; removing the
        // matched element is the deliberate, thread-safe choice.
        for _ in 0..MAX_RETRIES {
            let Some((i, item)) = self.find_python(py, value, 0, usize::MAX)? else {
                return Err(missing());
            };
            let displaced = {
                let mut data = self.mutate(py)?;
                let Some(k) = data.locate(i, item.bind(py)) else {
                    continue;
                };
                data.delete(k)
            };
            drop(displaced);
            return Ok(());
        }
        Err(PyRuntimeError::new_err("List changed during remove()"))
    }

    /// Remove and return the item at `index` (default last).
    #[pyo3(signature = (index = -1, /))]
    fn pop<'py>(&self, py: Python<'py>, index: isize) -> PyResult<Bound<'py, PyAny>> {
        let item = {
            let mut data = self.mutate(py)?;
            let len = data.len();
            if len == 0 {
                return Err(PyIndexError::new_err("pop from empty list"));
            }
            let Some(k) = normalize_index(index, len) else {
                return Err(PyIndexError::new_err("pop index out of range"));
            };
            data.remove(py, k)
        };
        Ok(item.into_bound(py))
    }

    /// Remove all items.
    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        let displaced = {
            let mut data = self.mutate(py)?;
            if data.is_empty() {
                return Ok(());
            }
            data.clear()
        };
        drop(displaced);
        Ok(())
    }

    /// Return the first index of `value` in `self[start:stop]` (`ValueError` if absent).
    #[pyo3(
        signature = (value, start = SliceBound::Omitted, stop = SliceBound::Omitted, /),
        text_signature = "($self, value, start=0, stop=sys.maxsize, /)"
    )]
    fn index(
        &self,
        py: Python<'_>,
        value: &Bound<'_, PyAny>,
        start: SliceBound<'_>,
        stop: SliceBound<'_>,
    ) -> PyResult<usize> {
        let (start, stop) = (start.value(0)?, stop.value(isize::MAX)?);
        let len = self.len();
        let (start, stop) = (clamp_bound(start, len), clamp_bound(stop, len));
        self.find(py, value, start, stop)?.ok_or_else(|| PyValueError::new_err("list.index(x): x not in list"))
    }

    /// Return the number of occurrences of `value`.
    fn count(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<usize> {
        self.count_of(py, value)
    }

    /// Reverse the list in place.
    fn reverse(&self, py: Python<'_>) -> PyResult<()> {
        let mut data = self.mutate(py)?;
        if data.len() > 1 {
            data.reverse();
        }
        Ok(())
    }

    /// Return a shallow copy.
    fn copy<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, List>> {
        List::from_storage(py, self.schema.clone(), self.snapshot(py))
    }

    /// Sort the list in place (stable). Native storage without `key` sorts in Rust.
    ///
    /// Sorts that run Python code (a `key`, or `<` on objects) behave like CPython's: the
    /// list appears empty meanwhile, the sorted elements are put back at the end, and if
    /// the list was changed in between, that content is discarded and `ValueError` raised.
    #[pyo3(signature = (*, key = None, reverse = None))]
    fn sort(&self, py: Python<'_>, key: Option<&Bound<'_, PyAny>>, reverse: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let reverse = match reverse {
            None => false,
            Some(r) => r.is_truthy()?,
        };
        let key = key.filter(|k| !k.is_none());
        if key.is_none() {
            let mut data = self.mutate(py)?;
            match &*data {
                Storage::Empty => return Ok(()),
                Storage::Native(column) if column.len() < 2 => return Ok(()),
                Storage::Native(_) => {
                    if let Storage::Native(column) = &mut *data {
                        if column.len() >= SORT_DETACH_THRESHOLD {
                            py.detach(|| kernels::sort(column, reverse));
                        } else {
                            kernels::sort(column, reverse);
                        }
                    }
                    return Ok(());
                }
                Storage::Object(_) => {}
            }
        }
        // Take the elements out, leaving an empty list of the same strategy.
        let (elements, version) = {
            let mut data = self.mutate(py)?;
            if data.is_empty() {
                return Ok(());
            }
            let placeholder = data.empty_like();
            let elements = std::mem::replace(&mut *data, placeholder);
            // Under the write lock, like `__getbuffer__`'s check: no export of the
            // placeholder can exist when the elements are put back.
            self.sorting.fetch_add(1, Ordering::Relaxed);
            (elements, data.finish())
        };
        let (elements, result) = sort_storage(py, elements, key, reverse);
        let (discarded, modified) = {
            // Not `mutate`: the elements must go back, and no export can exist (see above).
            let mut data = Mutation { guard: self.data.write(py), list: self, changed: false };
            let modified = self.version.get() != version;
            self.sorting.fetch_sub(1, Ordering::Relaxed);
            (std::mem::replace(&mut *data, elements), modified)
        };
        drop(discarded);
        result?;
        if modified {
            return Err(PyValueError::new_err("list modified during sort"));
        }
        Ok(())
    }

    /// Sum of the elements (Python semantics; exact for ints).
    fn sum<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        {
            let data = self.read(py);
            match &*data {
                Storage::Empty => return Ok(0i64.to_python(py)),
                Storage::Native(column) => {
                    let result = if column.len() >= DETACH_THRESHOLD {
                        py.detach(|| kernels::sum(column))
                    } else {
                        kernels::sum(column)
                    };
                    return match result {
                        Ok(Sum::Int(x)) => Ok(x.into_pyobject(py)?.into_any()),
                        Ok(Sum::Float(x)) => Ok(x.to_python(py)),
                        Err(KernelError::Unsupported { dtype, .. }) => Err(PyTypeError::new_err(format!(
                            "unsupported operand type(s) for +: 'int' and '{}'",
                            convert::python_type_name(dtype)
                        ))),
                    };
                }
                Storage::Object(_) => {}
            }
        }
        let items = self.to_pylist(py)?;
        py.import(intern!(py, "builtins"))?.getattr(intern!(py, "sum"))?.call1((items,))
    }

    /// Smallest element (`ValueError` if empty).
    fn min<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.extreme(py, false)
    }

    /// Largest element (`ValueError` if empty).
    fn max<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.extreme(py, true)
    }

    /// A builtin `list` with the same elements.
    fn to_list<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        self.to_pylist(py)
    }

    /// New list of `f(x)` for each element (`f`: an `X` expression or a callable).
    fn map<'py>(&self, py: Python<'py>, f: &Bound<'py, PyAny>) -> PyResult<Bound<'py, List>> {
        let snapshot = self.snapshot(py);
        let (storage, schema) = crate::expr::map_storage(py, snapshot, &self.schema, f)?;
        List::from_storage(py, schema, storage)
    }

    /// New list of the elements for which `f(x)` is true.
    fn filter<'py>(&self, py: Python<'py>, f: &Bound<'py, PyAny>) -> PyResult<Bound<'py, List>> {
        let snapshot = self.snapshot(py);
        let storage = crate::expr::filter_storage(py, snapshot, &self.schema, f)?;
        List::from_storage(py, self.schema.clone(), storage)
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        // Never block inside the GC: if a writer holds the lock, skip (at worst a cycle
        // survives this collection).
        if let Some(data) = self.data.try_read()
            && let Storage::Object(objects) = &*data
        {
            for o in objects {
                visit.call(o)?;
            }
        }
        Ok(())
    }

    fn __clear__(&self) {
        Python::attach(|py| {
            let taken = {
                let mut data = self.data.write(py);
                let taken = match &mut *data {
                    Storage::Object(objects) => std::mem::take(objects),
                    _ => Vec::new(),
                };
                self.len.store(data.len(), Ordering::Release);
                self.version.bump_exclusive();
                taken
            };
            drop(taken);
        });
    }

    /// Read-only buffer export of numeric/bool storage.
    unsafe fn __getbuffer__(slf: Bound<'_, Self>, view: *mut ffi::Py_buffer, flags: c_int) -> PyResult<()> {
        if view.is_null() {
            return Err(PyBufferError::new_err("View is null"));
        }
        if (flags & ffi::PyBUF_WRITABLE) == ffi::PyBUF_WRITABLE {
            return Err(PyBufferError::new_err("List buffers are read-only"));
        }
        let py = slf.py();
        let this = slf.get();
        let (ptr, count, item, format) = {
            let data = this.read(py);
            let parts = match &*data {
                Storage::Native(column) => column.raw_parts().map(|(p, n, s)| (p, n, s, buffer_format(column.dtype()))),
                _ => None,
            };
            let Some(parts) = parts else {
                return Err(PyBufferError::new_err(format!(
                    "List with '{}' strategy does not support the buffer protocol",
                    data.strategy()
                )));
            };
            if this.sorting.load(Ordering::Relaxed) != 0 {
                return Err(PyBufferError::new_err("cannot export a List while it is being sorted"));
            }
            // Under the lock: no writer can be mid-mutation, and every later writer sees it.
            this.exports.fetch_add(1, Ordering::AcqRel);
            parts
        };
        let meta: *mut [ffi::Py_ssize_t; 2] =
            Box::into_raw(Box::new([count as ffi::Py_ssize_t, item as ffi::Py_ssize_t]));
        // SAFETY: `view` is a valid Py_buffer to fill; `ptr` stays valid while exports > 0
        // (no mutation can happen) and the view holds a strong reference to the list.
        unsafe {
            (*view).obj = slf.into_any().into_ptr();
            (*view).buf = ptr as *mut c_void;
            (*view).len = (count * item) as ffi::Py_ssize_t;
            (*view).readonly = 1;
            (*view).itemsize = item as ffi::Py_ssize_t;
            (*view).format = if (flags & ffi::PyBUF_FORMAT) == ffi::PyBUF_FORMAT {
                format.as_ptr().cast_mut()
            } else {
                std::ptr::null_mut()
            };
            (*view).ndim = 1;
            (*view).shape =
                if (flags & ffi::PyBUF_ND) == ffi::PyBUF_ND { (*meta).as_mut_ptr() } else { std::ptr::null_mut() };
            (*view).strides = if (flags & ffi::PyBUF_STRIDES) == ffi::PyBUF_STRIDES {
                (*meta).as_mut_ptr().add(1)
            } else {
                std::ptr::null_mut()
            };
            (*view).suboffsets = std::ptr::null_mut();
            (*view).internal = meta.cast();
        }
        Ok(())
    }

    unsafe fn __releasebuffer__(&self, view: *mut ffi::Py_buffer) {
        // SAFETY: `internal` was set to a leaked box by `__getbuffer__` for this view.
        unsafe { drop(Box::from_raw((*view).internal.cast::<[ffi::Py_ssize_t; 2]>())) };
        self.exports.fetch_sub(1, Ordering::AcqRel);
    }
}

impl List {
    /// `list.__init__`: clear, then extend with the optional iterable (keeping the schema).
    fn init(&self, py: Python<'_>, args: &Bound<'_, PyTuple>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<()> {
        if kwargs.is_some_and(|k| !k.is_empty()) {
            return Err(PyTypeError::new_err("List() takes no keyword arguments"));
        }
        let iterable = match args.len() {
            0 => None,
            1 => Some(args.get_item(0)?),
            n => return Err(PyTypeError::new_err(format!("List expected at most 1 argument, got {n}"))),
        };
        self.clear(py)?;
        match iterable {
            Some(iterable) => self.extend(py, &iterable),
            None => Ok(()),
        }
    }

    fn extreme<'py>(&self, py: Python<'py>, max: bool) -> PyResult<Bound<'py, PyAny>> {
        let name = if max { "max" } else { "min" };
        {
            let data = self.read(py);
            match &*data {
                Storage::Empty => {
                    return Err(PyValueError::new_err(format!("{name}() iterable argument is empty")));
                }
                Storage::Native(column) => {
                    let run = || if max { kernels::max(column) } else { kernels::min(column) };
                    let value = if column.len() >= DETACH_THRESHOLD { py.detach(run) } else { run() };
                    return match value {
                        Some(v) => Ok(convert::value_to_python(py, &v)),
                        None => Err(PyValueError::new_err(format!("{name}() iterable argument is empty"))),
                    };
                }
                Storage::Object(_) => {}
            }
        }
        let items = self.to_pylist(py)?;
        py.import(intern!(py, "builtins"))?.getattr(name)?.call1((items,))
    }
}

/// Iterator position (CPython's `listiterobject`/`listreviterobject`): the list and the
/// index of the next element, `-1` once exhausted. The list is released when the iterator
/// is exhausted, like CPython on GIL builds (free-threaded CPython keeps it; here the state
/// lock makes releasing it safe on both builds).
struct IterState {
    list: Option<Py<List>>,
    index: isize,
}

/// The next element as a new reference, or null once exhausted (no exception set). Runs no
/// Python code and drops no `Py` handle while not exhausted (raw-slot safe, see `slots`).
#[inline]
fn iter_next_ptr(py: Python<'_>, state: &PyMutex<IterState>, forward: bool) -> *mut ffi::PyObject {
    let mut st = state.lock(py);
    let item = match &st.list {
        Some(list) if st.index >= 0 => {
            let list = list.get();
            let data = list.read(py);
            let i = st.index as usize;
            (i < data.len()).then(|| list.read_item(py, &data, i))
        }
        Some(_) => None,
        None => return std::ptr::null_mut(),
    };
    match item {
        Some(p) => {
            st.index += if forward { 1 } else { -1 };
            p
        }
        None => iter_exhausted(py, st),
    }
}

#[cold]
#[inline(never)]
fn iter_exhausted(py: Python<'_>, mut st: MutexGuard<'_, IterState>) -> *mut ffi::PyObject {
    st.index = -1;
    let finished = st.list.take();
    drop(st);
    // Dropped through `Bound`: an immediate decref even in raw slots.
    drop(finished.map(|list| list.into_bound(py)));
    std::ptr::null_mut()
}

/// `__reduce__` of a list iterator: `(iter, (list,), index)` / `(reversed, (list,), index)`
/// while it can still yield, `(iter, (List(),))` once exhausted (CPython uses an empty
/// builtin list there).
fn iter_reduce<'py>(py: Python<'py>, state: &PyMutex<IterState>, forward: bool) -> PyResult<Bound<'py, PyTuple>> {
    let builtins = py.import(intern!(py, "builtins"))?;
    let (list, index) = {
        let st = state.lock(py);
        (st.list.as_ref().map(|l| l.clone_ref(py)), st.index)
    };
    match list {
        Some(list) if index >= 0 || !forward => {
            let f = builtins.getattr(if forward { intern!(py, "iter") } else { intern!(py, "reversed") })?;
            (f, (list,), index).into_pyobject(py)
        }
        _ => {
            let empty = Bound::new(py, List::new_with(schema::any(), Storage::Empty))?;
            (builtins.getattr(intern!(py, "iter"))?, (empty,)).into_pyobject(py)
        }
    }
}

/// `__setstate__` of a list iterator: the index, clamped like CPython's.
fn iter_setstate(py: Python<'_>, state: &PyMutex<IterState>, index: &Bound<'_, PyAny>, forward: bool) -> PyResult<()> {
    // SAFETY: valid object; -1 with an exception set signals an error.
    let mut index = unsafe { ffi::PyLong_AsSsize_t(index.as_ptr()) };
    if index == -1
        && let Some(e) = PyErr::take(py)
    {
        return Err(e);
    }
    let mut st = state.lock(py);
    if let Some(list) = &st.list {
        let len = list.get().len() as isize;
        index = if forward { index.clamp(0, len) } else { index.clamp(-1, len - 1) };
        st.index = index;
    }
    Ok(())
}

/// `arange(start, stop=None, step=1, dtype=int)`: a typed `List` generated natively, like
/// `List[dtype](range(start, stop, step))` (float dtypes also accept float arguments).
#[pyfunction]
#[pyo3(signature = (start, stop = None, step = None, dtype = None))]
pub fn arange<'py>(
    py: Python<'py>,
    start: &Bound<'py, PyAny>,
    stop: Option<&Bound<'py, PyAny>>,
    step: Option<&Bound<'py, PyAny>>,
    dtype: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, List>> {
    let (schema, target) = arange_dtype(py, dtype)?;
    let zero = 0i64.to_python(py);
    let one = 1i64.to_python(py);
    let (start, stop) = match stop {
        Some(stop) if !stop.is_none() => (start, stop),
        _ => (&zero, start),
    };
    let step = step.unwrap_or(&one);
    // SAFETY: type checks on valid objects.
    let any_float = [start, stop, step].iter().any(|x| unsafe { ffi::PyFloat_Check(x.as_ptr()) } != 0);
    let column = if target.is_float() && any_float {
        let (a, b, s): (f64, f64, f64) = (start.extract()?, stop.extract()?, step.extract()?);
        if s == 0.0 {
            return Err(PyValueError::new_err("arange() arg 3 must not be zero"));
        }
        let n = ((b - a) / s).ceil();
        if !n.is_finite() {
            return Err(PyValueError::new_err("arange() cannot compute the length of a non-finite range"));
        }
        let len = if n <= 0.0 { 0 } else { n as usize };
        let values = generate_floats(py, len, |k| a + k as f64 * s)?;
        if target == DType::F32 {
            let mut out = Vec::with_capacity(values.len());
            for x in values {
                out.push(convert::f64_to_f32(x).ok_or_else(|| {
                    let x = rustique_core::pyfmt::float_repr(x);
                    pyo3::exceptions::PyOverflowError::new_err(format!("arange() value {x} is out of range for f32"))
                })?);
            }
            Column::F32(out)
        } else {
            Column::F64(values)
        }
    } else {
        let (a, b, s): (i128, i128, i128) = (start.extract()?, stop.extract()?, step.extract()?);
        if s == 0 {
            return Err(PyValueError::new_err("arange() arg 3 must not be zero"));
        }
        // Distances between i128 values fit u128; every generated value lies in
        // `[a, b)` (or `(b, a]`), so it fits i128 even where intermediate products do not.
        let span = if s > 0 && a < b {
            b.abs_diff(a)
        } else if s < 0 && a > b {
            a.abs_diff(b)
        } else {
            0
        };
        let n = if span == 0 { 0 } else { (span - 1) / s.unsigned_abs() + 1 };
        let len = isize::try_from(n)
            .map_err(|_| pyo3::exceptions::PyOverflowError::new_err("arange() result has too many items"))?
            as usize;
        if let Some((lo, hi)) = target.int_range()
            && let Some(k) = convert::first_out_of_range(a, s, len, lo, hi)
        {
            // Element `k` exists, so it lies between `a` and `b`: no overflow.
            let offset = k as u128 * s.unsigned_abs();
            let x = if s > 0 { a.checked_add_unsigned(offset) } else { a.checked_sub_unsigned(offset) };
            return Err(pyo3::exceptions::PyOverflowError::new_err(format!(
                "arange() value {} is out of range for {}",
                x.unwrap_or(a),
                if matches!(*schema, Schema::Int) { "int" } else { target.name() }
            )));
        }
        // Wrapping arithmetic is exact here: the true value of every element fits i128.
        convert::generate(py, target, len, |k| a.wrapping_add((k as i128).wrapping_mul(s)))?
    };
    List::from_storage(py, schema, Storage::Native(column))
}

fn generate_floats(py: Python<'_>, len: usize, value: impl Fn(usize) -> f64 + Send + Sync) -> PyResult<Vec<f64>> {
    if len.checked_mul(8).is_none_or(|bytes| bytes > isize::MAX as usize) {
        return Err(PyMemoryError::new_err(()));
    }
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| PyMemoryError::new_err(()))?;
    let run = |v: &mut Vec<f64>| v.extend((0..len).map(&value));
    if len >= DETACH_THRESHOLD {
        py.detach(|| run(&mut v))
    } else {
        run(&mut v)
    }
    Ok(v)
}

fn arange_dtype(py: Python<'_>, dtype: Option<&Bound<'_, PyAny>>) -> PyResult<(Arc<Schema>, DType)> {
    let Some(dtype) = dtype.filter(|d| !d.is_none()) else {
        return Ok((schema::intern(Schema::Int), DType::I64));
    };
    if dtype.is(py.get_type::<pyo3::types::PyInt>()) {
        return Ok((schema::intern(Schema::Int), DType::I64));
    }
    if dtype.is(py.get_type::<pyo3::types::PyFloat>()) {
        return Ok((schema::intern(Schema::Float), DType::F64));
    }
    if let Ok(ty) = dtype.cast::<PyType>()
        && !ty.is(py.get_type::<PyBool>())
        && let Some(d) = crate::scalars::dtype_of_type(ty)
        && d.is_numeric()
    {
        return Ok((schema::intern(Schema::Fixed(d)), d));
    }
    Err(PyTypeError::new_err(format!(
        "arange() dtype must be int, float or a rustique numeric scalar type, not {}",
        dtype.repr()?
    )))
}

pub(crate) mod alloc;
mod iter;
pub(crate) mod slots;

/// Raw allocation of `List` instances (see `alloc`).
static LIST_CLASS: alloc::RawClass<List> = alloc::RawClass::new();

/// The `List` type object (set at module initialisation).
static LIST_TYPE: std::sync::atomic::AtomicPtr<ffi::PyTypeObject> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

impl alloc::RawDrop for List {
    fn drop_attached(self, py: Python<'_>) {
        let List { data, .. } = self;
        // Elements are released through `Bound` (an immediate decref).
        if let Storage::Object(objects) = data.into_inner() {
            for o in objects {
                drop(o.into_bound(py));
            }
        }
    }
}

unsafe extern "C" fn list_dealloc(slf: *mut ffi::PyObject) {
    // SAFETY: installed as `List`'s `tp_dealloc`.
    slots::shield((), || unsafe { LIST_CLASS.dealloc(slf) })
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<List>()?;
    LIST_TYPE.store(m.py().get_type::<List>().as_type_ptr(), Ordering::Release);
    let sample = Bound::new(m.py(), List::new_with(schema::any(), Storage::Empty))?;
    LIST_CLASS.install(&sample, sample.get(), list_dealloc)?;
    drop(sample);
    iter::install(m.py())?;
    slots::install(m.py())?;
    // PyO3 names the type `rustique.List` at the C level; CPython messages that use
    // `tp_name` (e.g. "unhashable type: 'List'") should read like the builtin's. `__module__`
    // and `__qualname__` (repr, pickling) are unaffected.
    // SAFETY: the type object was just created during module initialisation, before any
    // other code can observe it; the new name is a static C string.
    unsafe {
        (*m.py().get_type::<List>().as_type_ptr()).tp_name = c"List".as_ptr();
    }
    m.add_function(wrap_pyfunction!(arange, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `from_temporary_list` moves object references out of a uniquely referenced list and
    /// copies (leaving the source intact) when the list is shared.
    #[test]
    fn from_temporary_list_steals_only_unique_lists() {
        Python::initialize();
        Python::attach(|py| {
            let module = pyo3::types::PyModule::new(py, "_rustique").unwrap();
            crate::list::register(&module).unwrap();
            let make = || {
                py.eval(c"[object(), 1, 'a']", None, None).unwrap().cast_into::<PyList>().unwrap()
            };
            // Unique: moved (the object's refcount is unchanged by the move).
            let temp = make();
            let first = temp.get_item(0).unwrap().unbind();
            // SAFETY: reading the reference count of a live object.
            let refcnt = |o: &Py<PyAny>| unsafe { ffi::Py_REFCNT(o.as_ptr()) };
            let before = refcnt(&first);
            let list = List::from_temporary_list(py, temp).unwrap();
            assert_eq!(list.get().len(), 3);
            assert_eq!(refcnt(&first), before);
            assert!(list.get().get_ref(py, 0).unwrap().is(first.bind(py)));
            // Shared: copied, the source keeps its items.
            let shared = make();
            let keep = shared.clone();
            let list = List::from_temporary_list(py, shared).unwrap();
            assert_eq!(list.get().len(), 3);
            assert_eq!(keep.len(), 3);
            // Native items are converted.
            let ints = py.eval(c"[1, 2, 3]", None, None).unwrap().cast_into::<PyList>().unwrap();
            let list = List::from_temporary_list(py, ints).unwrap();
            assert_eq!(list.get().read(py).strategy(), "i64");
        });
    }
}
