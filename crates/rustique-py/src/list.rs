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
use pyo3::types::{PyBool, PyList, PyString, PyTuple, PyType};
use pyo3::{PyTraverseError, ffi, intern};
use rustique_core::kernels::{self, CmpOp, KernelError, Sum};
use rustique_core::slice::{SliceIndices, clamp_insert_index, normalize_index};
use rustique_core::{Column, DType, with_column};

use crate::convert::{self, IngestError, Probe, ToPython};
use crate::schema::{self, Coerced, Schema};
use crate::storage::Storage;
use crate::sync::{MutationCounter, MutexGuard, PyMutex, PyRwLock, ReadGuard, WriteGuard, read_both};

/// Elements snapshotted per lock acquisition when comparing with Python `==`.
const CHUNK: usize = 256;
/// Native kernels on at least this many elements run detached from the interpreter.
const DETACH_THRESHOLD: usize = 1 << 16;
/// Sorting goes parallel (rayon) from this size: always detach then.
const SORT_DETACH_THRESHOLD: usize = 1 << 14;
/// How often `remove` re-runs a Python-level search invalidated by concurrent mutation.
const MAX_RETRIES: usize = 64;

/// A list with Python `list` semantics and native, adaptive storage.
#[pyclass(module = "rustique", frozen, sequence)]
pub struct List {
    schema: Arc<Schema>,
    data: PyRwLock<Storage>,
    version: MutationCounter,
    /// Length as of the last completed mutation (lock-free `len()`).
    len: AtomicUsize,
    /// Live buffer exports; while non-zero every mutation raises `BufferError`.
    exports: AtomicUsize,
}

/// Exclusive access for a mutation. On release (still under the lock) it bumps the
/// mutation counter and publishes the new length.
struct Mutation<'a> {
    guard: WriteGuard<'a, Storage>,
    list: &'a List,
}

impl Deref for Mutation<'_> {
    type Target = Storage;
    fn deref(&self) -> &Storage {
        &self.guard
    }
}

impl DerefMut for Mutation<'_> {
    fn deref_mut(&mut self) -> &mut Storage {
        &mut self.guard
    }
}

impl Drop for Mutation<'_> {
    fn drop(&mut self) {
        self.list.len.store(self.guard.len(), Ordering::Release);
        self.list.version.bump_exclusive();
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
        Err(PyTypeError::new_err(format!(
            "list indices must be integers or slices, not {}",
            convert::type_name(obj)
        )))
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

/// `start`/`stop` of `list.index`: `__index__`, clamped, negative values from the end.
fn clamp_bound(bound: SliceBound<'_>, len: usize, default: isize) -> PyResult<usize> {
    let v = match bound {
        SliceBound::Omitted => default,
        SliceBound::Given(obj) => {
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
                v
            }
        }
    };
    Ok(if v < 0 { v.saturating_add(len as isize).max(0) as usize } else { v as usize })
}

/// Whether `obj` is a builtin scalar (`int`, `float`, `bool`, `str`, `bytes`, `None`):
/// comparing two of them runs no Python code, so it cannot mutate a list.
fn is_builtin_scalar(obj: &Bound<'_, PyAny>) -> bool {
    let p = obj.as_ptr();
    // SAFETY: type pointer reads and comparisons on a valid object.
    unsafe {
        let ty = ffi::Py_TYPE(p);
        ty == &raw mut ffi::PyLong_Type
            || ty == &raw mut ffi::PyUnicode_Type
            || ty == &raw mut ffi::PyFloat_Type
            || ty == &raw mut ffi::PyBool_Type
            || ty == &raw mut ffi::PyBytes_Type
            || p == ffi::Py_None()
    }
}

/// Element equality for searches and list comparison: identity, then NaN equals NaN
/// (the documented List semantics, identical for every strategy), then Python `==`.
fn elements_equal(item: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<bool> {
    if item.is(value) {
        return Ok(true);
    }
    // SAFETY: exact type checks and value reads on valid objects.
    unsafe {
        let (a, b) = (item.as_ptr(), value.as_ptr());
        if ffi::PyFloat_CheckExact(a) != 0
            && ffi::PyFloat_CheckExact(b) != 0
            && ffi::PyFloat_AS_DOUBLE(a).is_nan()
            && ffi::PyFloat_AS_DOUBLE(b).is_nan()
        {
            return Ok(true);
        }
    }
    compare_bool(item, value, ffi::Py_EQ)
}

/// `PyObject_RichCompareBool(a, b, op)`: identity implies equality.
fn compare_bool(a: &Bound<'_, PyAny>, b: &Bound<'_, PyAny>, op: c_int) -> PyResult<bool> {
    // SAFETY: valid objects; -1 signals an exception.
    match unsafe { ffi::PyObject_RichCompareBool(a.as_ptr(), b.as_ptr(), op) } {
        -1 => Err(PyErr::fetch(a.py())),
        r => Ok(r == 1),
    }
}

fn compare_lengths(a: usize, b: usize, op: CompareOp) -> bool {
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
fn native_compare(a: &Column, b: &Column, op: CompareOp) -> Option<bool> {
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

/// One side of a Python-level comparison: a rustique `List` (read in chunks) or a
/// snapshot of a builtin `list`.
enum Side<'a> {
    List(&'a List),
    Items(Vec<Py<PyAny>>),
}

impl Side<'_> {
    fn chunk(&self, py: Python<'_>, start: usize) -> Chunk {
        match self {
            Side::List(list) => list.chunk(py, start, usize::MAX),
            Side::Items(items) => {
                let end = items.len().min(start.saturating_add(CHUNK));
                let chunk = items.get(start..end).unwrap_or(&[]).iter().map(|o| o.clone_ref(py)).collect();
                Chunk { items: chunk, len: items.len(), version: 0 }
            }
        }
    }

    /// Whether this side changed since a chunk was read from it at `version`.
    fn changed(&self, version: u64) -> bool {
        match self {
            Side::List(list) => list.version.get() != version,
            Side::Items(_) => false,
        }
    }
}

/// Elements read under one lock acquisition, with the list's length and mutation counter
/// at that moment.
struct Chunk {
    items: Vec<Py<PyAny>>,
    len: usize,
    version: u64,
}

impl List {
    fn new_with(schema: Arc<Schema>, storage: Storage) -> List {
        List {
            schema,
            len: AtomicUsize::new(storage.len()),
            data: PyRwLock::new(storage),
            version: MutationCounter::new(),
            exports: AtomicUsize::new(0),
        }
    }

    /// Wrap storage that is valid for `schema` in a new `List` (no conversion).
    pub fn from_storage(py: Python<'_>, schema: Arc<Schema>, storage: Storage) -> PyResult<Bound<'_, List>> {
        Bound::new(py, List::new_with(schema, storage))
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

    #[inline]
    fn mutate(&self, py: Python<'_>) -> PyResult<Mutation<'_>> {
        let guard = self.data.write(py);
        if self.exports.load(Ordering::Acquire) != 0 {
            return Err(PyBufferError::new_err("cannot modify List while a buffer export exists"));
        }
        Ok(Mutation { guard, list: self })
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

    /// Python objects for `self[start..min(start + CHUNK, stop, len)]`.
    fn chunk(&self, py: Python<'_>, start: usize, stop: usize) -> Chunk {
        let data = self.read(py);
        let len = data.len();
        let end = stop.min(len).min(start.saturating_add(CHUNK));
        let items = if start < end { data.objects(py, start..end) } else { Vec::new() };
        Chunk { items, len, version: self.version.get() }
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

    /// Python-level search, comparing outside the lock. Returns the index and the element
    /// that compared equal.
    ///
    /// Like CPython, a comparison that runs Python code (`__eq__`) is followed by the
    /// *current* list: if the list changed meanwhile, the rest of the chunk read ahead is
    /// abandoned and the search continues at the next index of the live list. Comparisons
    /// of builtin scalars run no Python code (in CPython they are atomic with the scan), so
    /// they keep using the chunk, which is a consistent snapshot.
    fn find_python(
        &self,
        py: Python<'_>,
        value: &Bound<'_, PyAny>,
        start: usize,
        stop: usize,
    ) -> PyResult<Option<(usize, Py<PyAny>)>> {
        let builtin_value = is_builtin_scalar(value);
        let mut i = start;
        loop {
            let chunk = self.chunk(py, i, stop);
            if chunk.items.is_empty() {
                return Ok(None);
            }
            for item in chunk.items {
                let item_ref = item.bind(py);
                let ran_python = !(builtin_value && is_builtin_scalar(item_ref));
                if elements_equal(item_ref, value)? {
                    return Ok(Some((i, item)));
                }
                i += 1;
                if ran_python && self.version.get() != chunk.version {
                    break;
                }
            }
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
        // Live semantics as in `find_python`.
        let builtin_value = is_builtin_scalar(value);
        let (mut i, mut count) = (0, 0);
        loop {
            let chunk = self.chunk(py, i, usize::MAX);
            if chunk.items.is_empty() {
                return Ok(count);
            }
            for item in &chunk.items {
                let item = item.bind(py);
                let ran_python = !(builtin_value && is_builtin_scalar(item));
                if elements_equal(item, value)? {
                    count += 1;
                }
                i += 1;
                if ran_python && self.version.get() != chunk.version {
                    break;
                }
            }
        }
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

    /// Python-level comparison (CPython's `list_richcompare`): find the first index where
    /// the elements differ (identity implies equality), comparing outside the locks. Each
    /// comparison sees the current lists (see `find_python`).
    fn richcompare_python(py: Python<'_>, a: Side<'_>, b: Side<'_>, op: CompareOp) -> PyResult<Py<PyAny>> {
        let mut i = 0;
        loop {
            let (ca, cb) = (a.chunk(py, i), b.chunk(py, i));
            if i == 0 && ca.len != cb.len && matches!(op, CompareOp::Eq | CompareOp::Ne) {
                return Ok(PyBool::new(py, matches!(op, CompareOp::Ne)).to_owned().into_any().unbind());
            }
            let m = ca.items.len().min(cb.items.len());
            if m == 0 {
                // One side is exhausted: the (current) lengths decide.
                return Ok(PyBool::new(py, compare_lengths(ca.len, cb.len, op)).to_owned().into_any().unbind());
            }
            for (x, y) in ca.items.iter().zip(&cb.items) {
                let (x, y) = (x.bind(py), y.bind(py));
                if !elements_equal(x, y)? {
                    return match op {
                        CompareOp::Eq => Ok(PyBool::new(py, false).to_owned().into_any().unbind()),
                        CompareOp::Ne => Ok(PyBool::new(py, true).to_owned().into_any().unbind()),
                        _ => Ok(x.rich_compare(y, op)?.unbind()),
                    };
                }
                i += 1;
                let ran_python = !(is_builtin_scalar(x) && is_builtin_scalar(y));
                if ran_python && (a.changed(ca.version) || b.changed(cb.version)) {
                    break;
                }
            }
        }
    }

    /// Stable permutation that sorts `keys` with Python `<` (`reverse` keeps equal
    /// elements in their original order, like `list.sort`).
    fn sort_permutation(py: Python<'_>, keys: &[Py<PyAny>], reverse: bool) -> PyResult<Vec<usize>> {
        let n = keys.len();
        let mut perm: Vec<usize> = if reverse { (0..n).rev().collect() } else { (0..n).collect() };
        if let Some(native) = NativeKeys::classify(py, keys) {
            match native {
                NativeKeys::Int(k) => perm.sort_by_key(|&i| k[i]),
                NativeKeys::Float(k) => perm.sort_by(|&a, &b| k[a].partial_cmp(&k[b]).unwrap_or(std::cmp::Ordering::Equal)),
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

#[pymethods]
impl List {
    #[new]
    #[pyo3(signature = (*args), text_signature = "(iterable=(), /)")]
    fn py_new(py: Python<'_>, args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        let schema = schema::any();
        let storage = match args.len() {
            0 => Storage::Empty,
            1 => convert::ingest(py, &schema, &args.get_item(0)?)?,
            n => return Err(PyTypeError::new_err(format!("List expected at most 1 argument, got {n}"))),
        };
        Ok(List::new_with(schema, storage))
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
                    return Err(PyTypeError::new_err("must assign iterable to extended slice"));
                }
                let values = this.ingest_for_update(py, value)?;
                let result = {
                    let mut data = this.mutate(py)?;
                    let s = adjust(start, stop, step, data.len())?;
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
                data.delete_slice(&s)
            }
        };
        drop(displaced);
        Ok(())
    }

    fn __contains__(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(self.find(py, value, 0, usize::MAX)?.is_some())
    }

    fn __iter__(slf: &Bound<'_, Self>) -> ListIterator {
        ListIterator { list: slf.clone().unbind(), state: PyMutex::new(IterState::new(0)) }
    }

    fn __reversed__(slf: &Bound<'_, Self>) -> ListReverseIterator {
        let len = slf.get().len();
        let next = if len == 0 { EXHAUSTED } else { len - 1 };
        ListReverseIterator { list: slf.clone().unbind(), state: PyMutex::new(IterState::new(next)) }
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
        // A builtin list: classify it like an untyped List; compare natively if possible.
        let theirs = convert::ingest(py, &schema::any(), other)?;
        if let Storage::Native(cb) = &theirs {
            let data = this.read(py);
            match &*data {
                Storage::Native(ca) => {
                    if let Some(r) = native_compare(ca, cb, op) {
                        return result(r);
                    }
                }
                Storage::Empty => return result(compare_lengths(0, cb.len(), op)),
                Storage::Object(_) => {}
            }
        }
        if theirs.is_empty() {
            return result(compare_lengths(this.len(), 0, op));
        }
        List::richcompare_python(py, Side::List(this), Side::Items(theirs.into_objects(py)), op)
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
            let items = this.to_pylist(py)?.repr()?;
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
        let displaced = self.mutate(py)?.repeat_in_place(py, n)?;
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
                (Storage::Native(column), Probe::Native(needle)) => Some(kernels::find(column, needle, 0, column.len())),
                (Storage::Native(_), _) | (Storage::Empty, _) => Some(None),
                (Storage::Object(_), _) => None,
            };
            match found {
                Some(Some(i)) => {
                    data.delete(i);
                    return Ok(());
                }
                Some(None) => return Err(missing()),
                None => {}
            }
        }
        // Python equality: compare outside the lock, then remove the element that compared
        // equal (re-located if concurrent mutation moved it; searched again if it is gone).
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
        let displaced = self.mutate(py)?.clear();
        drop(displaced);
        Ok(())
    }

    /// Return the first index of `value` in `self[start:stop]` (`ValueError` if absent).
    #[pyo3(
        signature = (value, start = SliceBound::Omitted, stop = SliceBound::Omitted, /),
        text_signature = "($self, value, start=0, stop=sys.maxsize, /)"
    )]
    fn index(&self, py: Python<'_>, value: &Bound<'_, PyAny>, start: SliceBound<'_>, stop: SliceBound<'_>) -> PyResult<usize> {
        let len = self.len();
        let start = clamp_bound(start, len, 0)?;
        let stop = clamp_bound(stop, len, isize::MAX)?;
        self.find(py, value, start, stop)?.ok_or_else(|| PyValueError::new_err("list.index(x): x not in list"))
    }

    /// Return the number of occurrences of `value`.
    fn count(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<usize> {
        self.count_of(py, value)
    }

    /// Reverse the list in place.
    fn reverse(&self, py: Python<'_>) -> PyResult<()> {
        self.mutate(py)?.reverse();
        Ok(())
    }

    /// Return a shallow copy.
    fn copy<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, List>> {
        List::from_storage(py, self.schema.clone(), self.snapshot(py))
    }

    /// Sort the list in place (stable). Native storage without `key` sorts in Rust.
    #[pyo3(signature = (*, key = None, reverse = None))]
    fn sort(&self, py: Python<'_>, key: Option<&Bound<'_, PyAny>>, reverse: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let reverse = match reverse {
            None => false,
            Some(r) => r.is_truthy()?,
        };
        let key = key.filter(|k| !k.is_none());
        if key.is_none() {
            let mut data = self.mutate(py)?;
            match &mut *data {
                Storage::Empty => return Ok(()),
                Storage::Native(column) => {
                    if column.len() >= SORT_DETACH_THRESHOLD {
                        py.detach(|| kernels::sort(column, reverse));
                    } else {
                        kernels::sort(column, reverse);
                    }
                    return Ok(());
                }
                Storage::Object(_) => {}
            }
        }
        // General path: snapshot, compute keys and order outside the lock, then commit if
        // nothing changed meanwhile.
        let (version, snapshot) = {
            let data = self.read(py);
            (self.version.get(), data.clone_ref(py))
        };
        let elements = match &snapshot {
            Storage::Object(_) => None,
            Storage::Native(column) => Some(convert::column_objects(py, column, 0..column.len())),
            Storage::Empty => return Ok(()),
        };
        let elements_ref: &[Py<PyAny>] = match (&elements, &snapshot) {
            (Some(e), _) => e,
            (None, Storage::Object(objects)) => objects,
            _ => &[],
        };
        let keys;
        let keys_ref: &[Py<PyAny>] = match key {
            Some(f) => {
                keys = elements_ref.iter().map(|e| f.call1((e,)).map(Bound::unbind)).collect::<PyResult<Vec<_>>>()?;
                &keys
            }
            None => elements_ref,
        };
        let perm = List::sort_permutation(py, keys_ref, reverse)?;
        let sorted = match snapshot {
            Storage::Native(column) => Storage::Native(column.take(&perm)),
            Storage::Object(objects) => {
                let mut slots: Vec<Option<Py<PyAny>>> = objects.into_iter().map(Some).collect();
                Storage::Object(perm.iter().filter_map(|&i| slots[i].take()).collect())
            }
            Storage::Empty => Storage::Empty,
        };
        let previous = {
            let mut data = self.mutate(py)?;
            if self.version.get() != version {
                return Err(PyValueError::new_err("list modified during sort"));
            }
            std::mem::replace(&mut *data, sorted)
        };
        drop(previous);
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
            // Under the lock: no writer can be mid-mutation, and every later writer sees it.
            this.exports.fetch_add(1, Ordering::AcqRel);
            parts
        };
        let meta: *mut [ffi::Py_ssize_t; 2] = Box::into_raw(Box::new([count as ffi::Py_ssize_t, item as ffi::Py_ssize_t]));
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

/// Marks an exhausted iterator.
const EXHAUSTED: usize = usize::MAX;
/// Elements an iterator materialises per visit of the list (grows from 8 to this).
const MAX_BATCH: usize = 64;

/// Iterator position plus a batch of elements read ahead under one lock acquisition. The
/// batch is only used while the list's mutation counter is unchanged, so iteration stays
/// exactly as live as CPython's (any mutation makes the next step re-read the list).
struct IterState {
    /// Index of the next element to yield, or `EXHAUSTED`.
    next: usize,
    /// Upcoming elements in reverse yield order (popped from the end).
    batch: Vec<Py<PyAny>>,
    /// Mutation counter value the batch was read at.
    version: u64,
    /// Size of the next batch.
    batch_size: usize,
}

impl IterState {
    fn new(next: usize) -> Self {
        IterState { next, batch: Vec::new(), version: 0, batch_size: 8 }
    }

    /// The next element from the read-ahead batch while it is current (the hot path: no
    /// list access, no allocation).
    #[inline]
    fn pop_current(&mut self, list: &List, forward: bool) -> Option<Py<PyAny>> {
        if self.batch.is_empty() || self.version != list.version.get() {
            return None;
        }
        let item = self.batch.pop();
        self.advance(forward);
        item
    }

    /// Re-read the list into the batch (it is used up or stale) and yield the next element.
    /// Takes the guard so that stale elements are dropped only after unlocking.
    #[inline(never)]
    fn refill_and_pop<'py>(
        py: Python<'py>,
        mut state: MutexGuard<'_, IterState>,
        list: &List,
        forward: bool,
    ) -> Option<Bound<'py, PyAny>> {
        let stale = if state.batch.is_empty() { Vec::new() } else { std::mem::take(&mut state.batch) };
        let mut item = None;
        if state.next != EXHAUSTED {
            let data = list.read(py);
            let len = data.len();
            if state.next >= len {
                state.next = EXHAUSTED;
            } else {
                let next = state.next;
                let range = if forward {
                    next..len.min(next + state.batch_size)
                } else {
                    (next + 1).saturating_sub(state.batch_size)..next + 1
                };
                // The batch is popped from the end: store forward batches reversed.
                data.extend_objects(py, range, forward, &mut state.batch);
                state.version = list.version.get();
                drop(data);
                state.batch_size = (state.batch_size * 2).min(MAX_BATCH);
                item = state.batch.pop();
                state.advance(forward);
            }
        }
        drop(state);
        drop(stale);
        item.map(|o| o.into_bound(py))
    }

    /// Yield the next element (`forward` selects the direction).
    #[inline]
    fn next<'py>(py: Python<'py>, state: &PyMutex<IterState>, list: &List, forward: bool) -> Option<Bound<'py, PyAny>> {
        let mut guard = state.lock(py);
        match guard.pop_current(list, forward) {
            Some(item) => {
                drop(guard);
                Some(item.into_bound(py))
            }
            None => IterState::refill_and_pop(py, guard, list, forward),
        }
    }

    fn advance(&mut self, forward: bool) {
        self.next = match (forward, self.next) {
            (true, n) => n + 1,
            (false, 0) => EXHAUSTED,
            (false, n) => n - 1,
        };
    }

    fn traverse(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        for o in &self.batch {
            visit.call(o)?;
        }
        Ok(())
    }
}

/// Live iterator over a `List` (CPython `list_iterator` semantics: sees appends, stops
/// when the index passes the current length, stays exhausted).
#[pyclass(module = "rustique", frozen, name = "ListIterator")]
pub struct ListIterator {
    list: Py<List>,
    state: PyMutex<IterState>,
}

#[pymethods]
impl ListIterator {
    fn __iter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    fn __next__<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyAny>> {
        IterState::next(py, &self.state, self.list.get(), true)
    }

    fn __length_hint__(&self, py: Python<'_>) -> usize {
        match self.state.lock(py).next {
            EXHAUSTED => 0,
            i => self.list.get().len().saturating_sub(i),
        }
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.list)?;
        match self.state.try_lock() {
            Some(state) => state.traverse(&visit),
            None => Ok(()),
        }
    }

    fn __clear__(&self) {
        let batch = Python::attach(|py| std::mem::take(&mut self.state.lock(py).batch));
        drop(batch);
    }
}

/// Live reverse iterator (CPython `list_reverseiterator` semantics).
#[pyclass(module = "rustique", frozen, name = "ListReverseIterator")]
pub struct ListReverseIterator {
    list: Py<List>,
    state: PyMutex<IterState>,
}

#[pymethods]
impl ListReverseIterator {
    fn __iter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    fn __next__<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyAny>> {
        IterState::next(py, &self.state, self.list.get(), false)
    }

    fn __length_hint__(&self, py: Python<'_>) -> usize {
        match self.state.lock(py).next {
            EXHAUSTED => 0,
            i if i < self.list.get().len() => i + 1,
            _ => 0,
        }
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.list)?;
        match self.state.try_lock() {
            Some(state) => state.traverse(&visit),
            None => Ok(()),
        }
    }

    fn __clear__(&self) {
        let batch = Python::attach(|py| std::mem::take(&mut self.state.lock(py).batch));
        drop(batch);
    }
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
                out.push(convert::f64_to_f32(x).ok_or_else(|| pyo3::exceptions::PyOverflowError::new_err(format!("arange() value {x} is out of range for f32")))?);
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
        let n = if s > 0 && a < b {
            (b - a - 1) / s + 1
        } else if s < 0 && a > b {
            (a - b - 1) / (-s) + 1
        } else {
            0
        };
        let len = usize::try_from(n).map_err(|_| PyMemoryError::new_err(()))?;
        if len > 0
            && let Some((lo, hi)) = target.int_range()
        {
            let last = a + (n - 1) * s;
            for x in [a, last] {
                if x < lo || x > hi {
                    return Err(pyo3::exceptions::PyOverflowError::new_err(format!(
                        "arange() value {x} is out of range for {}",
                        if matches!(*schema, Schema::Int) { "int" } else { target.name() }
                    )));
                }
            }
        }
        convert::generate(py, target, len, |k| a + k as i128 * s)?
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
    if len >= DETACH_THRESHOLD { py.detach(|| run(&mut v)) } else { run(&mut v) }
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

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<List>()?;
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
