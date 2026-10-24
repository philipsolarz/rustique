//! Conversions between Python objects and native elements (ARCHITECTURE §3.3–3.4, §4.1):
//! element materialisation, exact-type classification, bulk ingest fast paths and search
//! probes.
//!
//! Bulk reads of a builtin `list` happen inside one critical section on the source list and
//! only touch exact `int`/`float`/`bool`/`str` items with calls that cannot run Python code
//! or block; anything else bails out to the general (per-element `Schema::coerce`) path
//! after taking strong references.

use std::ops::Range;
use std::sync::Arc;

use compact_str::CompactString;
use pyo3::buffer::PyUntypedBuffer;
use pyo3::exceptions::PyMemoryError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::sync::critical_section::with_critical_section;
use pyo3::types::{PyBool, PyList, PyTuple};
use rustique_core::{Column, DType, Element, Value, with_column};

use crate::list::List;
use crate::schema::{CoerceError, Coerced, Schema};
use crate::storage::Storage;

// ---------------------------------------------------------------------------------------
// Native → Python
// ---------------------------------------------------------------------------------------

/// A native element type that materialises as a Python builtin (`int`, `float`, `bool`,
/// `str`). Safe to call under a Rustique lock: these objects are not GC-tracked and their
/// creation runs no Python code.
pub trait ToPython {
    fn to_python<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny>;
}

macro_rules! impl_to_python_small_int {
    ($($t:ty),*) => {$(
        impl ToPython for $t {
            #[inline]
            fn to_python<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
                // SAFETY: returns a new reference; NULL (MemoryError) panics in
                // `from_owned_ptr`, exactly like PyO3's own integer conversions.
                unsafe { Bound::from_owned_ptr(py, ffi::PyLong_FromLongLong(*self as i64)) }
            }
        }
    )*};
}

impl_to_python_small_int!(i8, i16, i32, i64, u8, u16, u32);

impl ToPython for u64 {
    #[inline]
    fn to_python<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        // SAFETY: as above.
        unsafe { Bound::from_owned_ptr(py, ffi::PyLong_FromUnsignedLongLong(*self)) }
    }
}

impl ToPython for bool {
    #[inline]
    fn to_python<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        PyBool::new(py, *self).to_owned().into_any()
    }
}

impl ToPython for f64 {
    #[inline]
    fn to_python<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        // SAFETY: as above.
        unsafe { Bound::from_owned_ptr(py, ffi::PyFloat_FromDouble(*self)) }
    }
}

impl ToPython for f32 {
    #[inline]
    fn to_python<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        f64::from(*self).to_python(py)
    }
}

impl ToPython for CompactString {
    #[inline]
    fn to_python<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        // SAFETY: the bytes are valid UTF-8 of the given length; returns a new reference.
        unsafe {
            Bound::from_owned_ptr(
                py,
                ffi::PyUnicode_FromStringAndSize(self.as_ptr().cast(), self.len() as ffi::Py_ssize_t),
            )
        }
    }
}

/// `column[index]` as a Python object (`index` must be in range).
#[inline]
pub fn column_item<'py>(py: Python<'py>, column: &Column, index: usize) -> Bound<'py, PyAny> {
    with_column!(column, v => v[index].to_python(py))
}

/// Materialise `column[range]` as Python objects.
pub fn column_objects(py: Python<'_>, column: &Column, range: Range<usize>) -> Vec<Py<PyAny>> {
    with_column!(column, v => v[range].iter().map(|x| x.to_python(py).unbind()).collect())
}

pub fn value_to_python<'py>(py: Python<'py>, value: &Value) -> Bound<'py, PyAny> {
    match value {
        Value::Bool(x) => x.to_python(py),
        Value::I8(x) => x.to_python(py),
        Value::I16(x) => x.to_python(py),
        Value::I32(x) => x.to_python(py),
        Value::I64(x) => x.to_python(py),
        Value::U8(x) => x.to_python(py),
        Value::U16(x) => x.to_python(py),
        Value::U32(x) => x.to_python(py),
        Value::U64(x) => x.to_python(py),
        Value::F32(x) => x.to_python(py),
        Value::F64(x) => x.to_python(py),
        Value::Str(x) => x.to_python(py),
    }
}

/// Name of the Python type native elements of `dtype` read back as.
pub fn python_type_name(dtype: DType) -> &'static str {
    match dtype {
        DType::Bool => "bool",
        DType::F32 | DType::F64 => "float",
        DType::Str => "str",
        _ => "int",
    }
}

// ---------------------------------------------------------------------------------------
// Python → native (single objects)
// ---------------------------------------------------------------------------------------

/// `type(obj).__name__`, for error messages.
pub fn type_name(obj: &Bound<'_, PyAny>) -> String {
    obj.get_type().name().map(|n| n.to_string()).unwrap_or_else(|_| "object".to_owned())
}

#[inline]
pub fn as_bool(obj: &Bound<'_, PyAny>) -> Option<bool> {
    let p = obj.as_ptr();
    // SAFETY: pointer comparisons with the bool singletons.
    unsafe {
        if p == ffi::Py_True() {
            Some(true)
        } else if p == ffi::Py_False() {
            Some(false)
        } else {
            None
        }
    }
}

/// The value of an `int` (subclasses and `bool` included) as `i64`; `Ok(None)` when it
/// does not fit. Never calls `__index__` for `int` instances.
#[inline]
pub fn long_as_i64(obj: &Bound<'_, PyAny>) -> PyResult<Option<i64>> {
    let mut overflow = 0;
    // SAFETY: valid object; the overflow flag is written by CPython.
    let v = unsafe { ffi::PyLong_AsLongLongAndOverflow(obj.as_ptr(), &mut overflow) };
    if overflow != 0 {
        return Ok(None);
    }
    if v == -1
        && let Some(e) = PyErr::take(obj.py())
    {
        return Err(e);
    }
    Ok(Some(v))
}

/// The value of an `int` as `i128` (enough for every fixed-width dtype); `Ok(None)` when
/// it is out of the `i64::MIN..=u64::MAX` range.
pub fn long_as_i128(obj: &Bound<'_, PyAny>) -> PyResult<Option<i128>> {
    if let Some(v) = long_as_i64(obj)? {
        return Ok(Some(i128::from(v)));
    }
    // SAFETY: valid `int` object (callers check `PyLong_Check`).
    let v = unsafe { ffi::PyLong_AsUnsignedLongLong(obj.as_ptr()) };
    if v == u64::MAX && PyErr::take(obj.py()).is_some() {
        return Ok(None);
    }
    Ok(Some(i128::from(v)))
}

/// `x` as `f32`, or `None` when a finite value overflows the `f32` range.
#[inline]
pub fn f64_to_f32(x: f64) -> Option<f32> {
    let y = x as f32;
    if y.is_infinite() && x.is_finite() { None } else { Some(y) }
}

pub enum StrValue {
    Native(CompactString),
    /// A `str` containing lone surrogates (not representable as UTF-8).
    Surrogates,
    NotStr,
}

pub fn as_str(obj: &Bound<'_, PyAny>) -> StrValue {
    let p = obj.as_ptr();
    // SAFETY: type checks on a valid object; `exact_str` requires an exact `str`.
    unsafe {
        if ffi::PyUnicode_CheckExact(p) != 0 {
            return match exact_str(p) {
                Some(s) => StrValue::Native(s),
                None => StrValue::Surrogates,
            };
        }
        if ffi::PyUnicode_Check(p) == 0 {
            return StrValue::NotStr;
        }
        // A `str` subclass: go through the C API, which handles any layout.
        let mut size: ffi::Py_ssize_t = 0;
        let data = ffi::PyUnicode_AsUTF8AndSize(p, &mut size);
        if data.is_null() {
            PyErr::take(obj.py());
            return StrValue::Surrogates;
        }
        let bytes = std::slice::from_raw_parts(data.cast::<u8>(), size as usize);
        StrValue::Native(CompactString::from(std::str::from_utf8_unchecked(bytes)))
    }
}

/// UTF-8 copy of an exact `str`, read from its PEP 393 storage without calling into the
/// interpreter (so it is safe inside a critical section and never caches UTF-8 in the
/// source object). `None` if the string contains lone surrogates.
///
/// # Safety
/// `p` must point to a live object whose type is exactly `str`.
pub unsafe fn exact_str(p: *mut ffi::PyObject) -> Option<CompactString> {
    // SAFETY: guaranteed by the caller; exact `str` objects are immutable PEP 393 strings
    // whose data pointer covers `length` code units of `kind` bytes each.
    unsafe {
        let len = ffi::PyUnicode_GET_LENGTH(p) as usize;
        if len == 0 {
            return Some(CompactString::default());
        }
        let data = ffi::PyUnicode_DATA(p);
        match ffi::PyUnicode_KIND(p) {
            ffi::PyUnicode_1BYTE_KIND => {
                let bytes = std::slice::from_raw_parts(data.cast::<u8>(), len);
                if bytes.is_ascii() {
                    return Some(CompactString::from(std::str::from_utf8_unchecked(bytes)));
                }
                let mut s = CompactString::with_capacity(len + bytes.iter().filter(|b| !b.is_ascii()).count());
                s.extend(bytes.iter().map(|&b| char::from(b)));
                Some(s)
            }
            ffi::PyUnicode_2BYTE_KIND => {
                let units = std::slice::from_raw_parts(data.cast::<u16>(), len);
                let mut s = CompactString::with_capacity(len * 2);
                for &u in units {
                    // Canonical 2-byte strings never hold surrogate pairs: any surrogate
                    // code unit is a lone surrogate code point.
                    s.push(char::from_u32(u32::from(u))?);
                }
                Some(s)
            }
            _ => {
                let units = std::slice::from_raw_parts(data.cast::<u32>(), len);
                let mut s = CompactString::with_capacity(len * 4);
                for &u in units {
                    s.push(char::from_u32(u)?);
                }
                Some(s)
            }
        }
    }
}

/// Exact-type classification for untyped containers (§3.3): exact `int` fitting i64,
/// `float`, `bool`, `str` without lone surrogates are stored natively, anything else as an
/// object. Runs no Python code.
#[inline]
pub fn classify(obj: &Bound<'_, PyAny>) -> Coerced {
    // SAFETY: `obj` is valid; every helper is applied to an object of the exact type it
    // expects.
    match unsafe { classify_ptr(obj.as_ptr()) } {
        Some(v) => Coerced::Native(v),
        None => Coerced::Object(obj.clone().unbind()),
    }
}

/// # Safety
/// `p` must point to a live object.
#[inline]
unsafe fn classify_ptr(p: *mut ffi::PyObject) -> Option<Value> {
    // SAFETY: guaranteed by the caller; each read matches the checked exact type.
    unsafe {
        let ty = ffi::Py_TYPE(p);
        if ty == &raw mut ffi::PyLong_Type {
            let mut overflow = 0;
            let v = ffi::PyLong_AsLongLongAndOverflow(p, &mut overflow);
            (overflow == 0).then_some(Value::I64(v))
        } else if ty == &raw mut ffi::PyFloat_Type {
            Some(Value::F64(ffi::PyFloat_AS_DOUBLE(p)))
        } else if ty == &raw mut ffi::PyBool_Type {
            Some(Value::Bool(p == ffi::Py_True()))
        } else if ty == &raw mut ffi::PyUnicode_Type {
            exact_str(p).map(Value::Str)
        } else {
            None
        }
    }
}

/// Whether `iter(obj)` can succeed (`tp_iter` or the sequence protocol), without calling it.
pub fn is_iterable(obj: &Bound<'_, PyAny>) -> bool {
    let p = obj.as_ptr();
    // SAFETY: reading the type's slot and a protocol check on a valid object.
    unsafe { (*ffi::Py_TYPE(p)).tp_iter.is_some() || ffi::PySequence_Check(p) != 0 }
}

// ---------------------------------------------------------------------------------------
// Bulk ingest
// ---------------------------------------------------------------------------------------

/// Failure of a bulk conversion. For exceptions raised by the source iterator itself,
/// `partial` holds the elements converted before the failure (`list.extend` keeps them).
#[derive(Debug)]
pub struct IngestError {
    pub error: PyErr,
    pub partial: Option<Storage>,
}

impl From<PyErr> for IngestError {
    fn from(error: PyErr) -> Self {
        IngestError { error, partial: None }
    }
}

impl From<IngestError> for PyErr {
    fn from(e: IngestError) -> Self {
        e.error
    }
}

/// `List` or `List[T]`, for error messages.
pub fn container_name(schema: &Schema) -> String {
    if schema.is_any() { "List".to_owned() } else { format!("List[{schema}]") }
}

fn coerce_failure(schema: &Schema, index: usize, e: CoerceError) -> IngestError {
    e.into_pyerr(&container_name(schema), Some(index)).into()
}

/// Convert any iterable into storage for `schema`, using the fast paths of §4.1: exact
/// `list`/`tuple`, `range`, rustique `List`, buffer-protocol objects, generic iterables.
pub fn ingest(py: Python<'_>, schema: &Arc<Schema>, obj: &Bound<'_, PyAny>) -> Result<Storage, IngestError> {
    let p = obj.as_ptr();
    // SAFETY: exact type checks on a valid object.
    let (is_list, is_tuple, is_range) = unsafe {
        (
            ffi::PyList_CheckExact(p) != 0,
            ffi::PyTuple_CheckExact(p) != 0,
            ffi::Py_TYPE(p) == &raw mut ffi::PyRange_Type,
        )
    };
    if is_list {
        // SAFETY: checked exact type above.
        return ingest_list(py, schema, unsafe { obj.cast_unchecked::<PyList>() });
    }
    if is_tuple {
        // SAFETY: checked exact type above.
        return ingest_tuple(py, schema, unsafe { obj.cast_unchecked::<PyTuple>() });
    }
    if is_range && let Some(storage) = ingest_range(py, schema, obj)? {
        return Ok(storage);
    }
    if let Ok(list) = obj.cast::<List>() {
        let source = list.get();
        return convert_storage(py, source.snapshot(py), source.schema(), schema);
    }
    if let Some(storage) = ingest_buffer(py, schema, obj)? {
        return Ok(storage);
    }
    ingest_iter(py, schema, obj)
}

/// Generic path: iterate and coerce element by element.
fn ingest_iter(py: Python<'_>, schema: &Arc<Schema>, obj: &Bound<'_, PyAny>) -> Result<Storage, IngestError> {
    let iter = obj.try_iter()?;
    let mut storage = Storage::for_schema(schema);
    for (i, item) in iter.enumerate() {
        let item = match item {
            Ok(item) => item,
            Err(error) => return Err(IngestError { error, partial: Some(storage) }),
        };
        match schema.coerce(&item) {
            Ok(value) => storage.push(py, value),
            Err(e) => return Err(coerce_failure(schema, i, e)),
        }
    }
    Ok(storage)
}

/// Coerce owned objects into `storage` (used after a fast path bails out).
fn ingest_objects<'py>(
    py: Python<'py>,
    schema: &Schema,
    mut storage: Storage,
    items: impl IntoIterator<Item = Bound<'py, PyAny>>,
    offset: usize,
) -> Result<Storage, IngestError> {
    for (i, item) in items.into_iter().enumerate() {
        match schema.coerce(&item) {
            Ok(value) => storage.push(py, value),
            Err(e) => return Err(coerce_failure(schema, offset + i, e)),
        }
    }
    Ok(storage)
}

/// Outcome of a fast pass over borrowed items.
enum Fast {
    Done(Storage),
    /// The fast pass stopped at `offset`; `rest` holds strong references to the remaining
    /// items, to be converted on the general path.
    Bail { storage: Storage, rest: Vec<Py<PyAny>>, offset: usize },
}

fn ingest_list(py: Python<'_>, schema: &Arc<Schema>, list: &Bound<'_, PyList>) -> Result<Storage, IngestError> {
    let p = list.as_ptr();
    // One critical section for the whole bulk read (a no-op on GIL builds). Inside it we
    // only use calls that cannot run Python code or block, so it is never suspended.
    let fast = with_critical_section(list.as_any(), || {
        // SAFETY: we hold the list's critical section; `ob_item` has `size` valid borrowed
        // pointers until the list is mutated, which cannot happen while it is held.
        let items: &[*mut ffi::PyObject] = unsafe {
            let n = ffi::PyList_GET_SIZE(p) as usize;
            if n == 0 { &[] } else { std::slice::from_raw_parts((*p.cast::<ffi::PyListObject>()).ob_item, n) }
        };
        if schema.is_pure() {
            // SAFETY: live borrowed items, see above.
            unsafe { fast_items(py, schema, items) }
        } else {
            // SAFETY: as above; strong references survive the critical section.
            let rest = items.iter().map(|&o| unsafe { Bound::from_borrowed_ptr(py, o).unbind() }).collect();
            Fast::Bail { storage: Storage::for_schema(schema), rest, offset: 0 }
        }
    });
    finish_fast(py, schema, fast)
}

fn ingest_tuple(py: Python<'_>, schema: &Arc<Schema>, tuple: &Bound<'_, PyTuple>) -> Result<Storage, IngestError> {
    if !schema.is_pure() {
        return ingest_objects(py, schema, Storage::for_schema(schema), tuple.iter(), 0);
    }
    let p = tuple.as_ptr();
    // SAFETY: tuples are immutable; the items stay alive as long as `tuple` does.
    let fast = unsafe {
        let n = ffi::PyTuple_GET_SIZE(p) as usize;
        let items = if n == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts((*p.cast::<ffi::PyTupleObject>()).ob_item.as_ptr(), n)
        };
        fast_items(py, schema, items)
    };
    finish_fast(py, schema, fast)
}

fn finish_fast(py: Python<'_>, schema: &Schema, fast: Fast) -> Result<Storage, IngestError> {
    match fast {
        Fast::Done(storage) => Ok(storage),
        Fast::Bail { storage, rest, offset } => {
            ingest_objects(py, schema, storage, rest.into_iter().map(|o| o.into_bound(py)), offset)
        }
    }
}

/// Fast conversion of borrowed items for a pure schema. Handles exact builtin types only
/// and never raises; anything unusual bails out with strong references to the rest.
///
/// # Safety
/// Every pointer in `items` must be a live object for the duration of the call.
unsafe fn fast_items(py: Python<'_>, schema: &Schema, items: &[*mut ffi::PyObject]) -> Fast {
    // SAFETY: forwarded from the caller.
    unsafe {
        match schema {
            Schema::Any => Fast::Done(any_items(py, items)),
            Schema::Int => typed_items(py, schema, items, |p| {
                let ty = ffi::Py_TYPE(p);
                if ty == &raw mut ffi::PyLong_Type {
                    let mut overflow = 0;
                    let v = ffi::PyLong_AsLongLongAndOverflow(p, &mut overflow);
                    (overflow == 0).then_some(v)
                } else if ty == &raw mut ffi::PyBool_Type {
                    Some(i64::from(p == ffi::Py_True()))
                } else {
                    None
                }
            }),
            Schema::Float => typed_items(py, schema, items, |p| {
                let ty = ffi::Py_TYPE(p);
                if ty == &raw mut ffi::PyFloat_Type {
                    Some(ffi::PyFloat_AS_DOUBLE(p))
                } else if ty == &raw mut ffi::PyLong_Type {
                    let mut overflow = 0;
                    let v = ffi::PyLong_AsLongLongAndOverflow(p, &mut overflow);
                    (overflow == 0).then_some(v as f64)
                } else {
                    None
                }
            }),
            Schema::Bool => typed_items(py, schema, items, |p| {
                (ffi::Py_TYPE(p) == &raw mut ffi::PyBool_Type).then(|| p == ffi::Py_True())
            }),
            Schema::Str => typed_items(py, schema, items, |p| {
                if ffi::Py_TYPE(p) == &raw mut ffi::PyUnicode_Type { exact_str(p) } else { None }
            }),
            Schema::Fixed(dtype) => fixed_items(py, schema, *dtype, items),
            _ => Fast::Bail {
                storage: Storage::for_schema(schema),
                rest: items.iter().map(|&o| Bound::from_borrowed_ptr(py, o).unbind()).collect(),
                offset: 0,
            },
        }
    }
}

/// Fill a `Vec<T>` while `convert` succeeds; bail out at the first item it rejects.
///
/// # Safety
/// As [`fast_items`].
unsafe fn typed_items<T: Element>(
    py: Python<'_>,
    schema: &Schema,
    items: &[*mut ffi::PyObject],
    convert: impl Fn(*mut ffi::PyObject) -> Option<T>,
) -> Fast {
    let mut out = Vec::with_capacity(items.len());
    for (i, &p) in items.iter().enumerate() {
        match convert(p) {
            Some(v) => out.push(v),
            None => {
                let mut storage = Storage::for_schema(schema);
                if let Storage::Native(column) = &mut storage
                    && let Some(vec) = column.as_vec_mut::<T>()
                {
                    *vec = out;
                }
                return Fast::Bail {
                    storage,
                    // SAFETY: live borrowed items (caller contract).
                    rest: items[i..].iter().map(|&o| unsafe { Bound::from_borrowed_ptr(py, o).unbind() }).collect(),
                    offset: i,
                };
            }
        }
    }
    Fast::Done(Storage::Native(T::into_column(out)))
}

/// # Safety
/// As [`fast_items`].
unsafe fn fixed_items(py: Python<'_>, schema: &Schema, dtype: DType, items: &[*mut ffi::PyObject]) -> Fast {
    macro_rules! ints {
        ($t:ty) => {
            typed_items::<$t>(py, schema, items, |p| {
                if ffi::Py_TYPE(p) != &raw mut ffi::PyLong_Type {
                    return None;
                }
                let mut overflow = 0;
                let v = ffi::PyLong_AsLongLongAndOverflow(p, &mut overflow);
                if overflow != 0 { None } else { <$t>::try_from(v).ok() }
            })
        };
    }
    // SAFETY: exact-type checked reads of live objects.
    unsafe {
        match dtype {
            DType::I8 => ints!(i8),
            DType::I16 => ints!(i16),
            DType::I32 => ints!(i32),
            DType::I64 => ints!(i64),
            DType::U8 => ints!(u8),
            DType::U16 => ints!(u16),
            DType::U32 => ints!(u32),
            DType::U64 => ints!(u64),
            DType::F64 => typed_items::<f64>(py, schema, items, |p| {
                (ffi::Py_TYPE(p) == &raw mut ffi::PyFloat_Type).then(|| ffi::PyFloat_AS_DOUBLE(p))
            }),
            DType::F32 => typed_items::<f32>(py, schema, items, |p| {
                if ffi::Py_TYPE(p) == &raw mut ffi::PyFloat_Type { f64_to_f32(ffi::PyFloat_AS_DOUBLE(p)) } else { None }
            }),
            DType::Bool | DType::Str => Fast::Bail {
                storage: Storage::for_schema(schema),
                rest: items.iter().map(|&o| Bound::from_borrowed_ptr(py, o).unbind()).collect(),
                offset: 0,
            },
        }
    }
}

/// Untyped ingest in two passes: classify exact types (pointer compares only), then fill
/// the chosen column directly. Mixed contents, big ints or lone surrogates give object
/// storage.
///
/// # Safety
/// As [`fast_items`].
unsafe fn any_items(py: Python<'_>, items: &[*mut ffi::PyObject]) -> Storage {
    let Some(&first) = items.first() else {
        return Storage::Empty;
    };
    // SAFETY: live borrowed items (caller contract); every read matches the exact type.
    unsafe {
        let ty = ffi::Py_TYPE(first);
        let homogeneous = items.iter().all(|&p| ffi::Py_TYPE(p) == ty);
        if homogeneous {
            let native = if ty == &raw mut ffi::PyLong_Type {
                let mut out = Vec::with_capacity(items.len());
                let mut fits = true;
                for &p in items {
                    let mut overflow = 0;
                    let v = ffi::PyLong_AsLongLongAndOverflow(p, &mut overflow);
                    if overflow != 0 {
                        fits = false;
                        break;
                    }
                    out.push(v);
                }
                fits.then(|| Column::I64(out))
            } else if ty == &raw mut ffi::PyFloat_Type {
                Some(Column::F64(items.iter().map(|&p| ffi::PyFloat_AS_DOUBLE(p)).collect()))
            } else if ty == &raw mut ffi::PyBool_Type {
                let t = ffi::Py_True();
                Some(Column::Bool(items.iter().map(|&p| p == t).collect()))
            } else if ty == &raw mut ffi::PyUnicode_Type {
                items.iter().map(|&p| exact_str(p)).collect::<Option<Vec<_>>>().map(Column::Str)
            } else {
                None
            };
            if let Some(column) = native {
                return Storage::Native(column);
            }
        }
        Storage::Object(items.iter().map(|&p| Bound::from_borrowed_ptr(py, p).unbind()).collect())
    }
}

/// `range` with int bounds: generated natively for schemas that store ints or floats.
fn ingest_range(py: Python<'_>, schema: &Arc<Schema>, range: &Bound<'_, PyAny>) -> Result<Option<Storage>, IngestError> {
    let target = match &**schema {
        Schema::Any | Schema::Int => DType::I64,
        Schema::Float => DType::F64,
        Schema::Fixed(d) if d.is_numeric() => *d,
        _ => return Ok(None),
    };
    let bound = |name: &str| -> PyResult<Option<i64>> { long_as_i64(&range.getattr(name)?) };
    let (Some(start), Some(stop), Some(step)) = (bound("start")?, bound("stop")?, bound("step")?) else {
        return Ok(None);
    };
    let len = range_len(start, stop, step);
    if len == 0 {
        return Ok(Some(Storage::for_schema(schema)));
    }
    let last = i128::from(start) + (len as i128 - 1) * i128::from(step);
    if let Some((lo, hi)) = target.int_range() {
        let out_of_range = |x: i128| x < lo || x > hi;
        if out_of_range(i128::from(start)) || out_of_range(last) {
            // Ranges are monotonic: find the first offending element.
            let index = (0..len).find(|&k| out_of_range(i128::from(start) + k as i128 * i128::from(step))).unwrap_or(0);
            let value = i128::from(start) + index as i128 * i128::from(step);
            let e = CoerceError::Overflow { message: format!("{value} is out of range for {target}") };
            return Err(coerce_failure(schema, index, e));
        }
    }
    let column = generate(py, target, len, |k| i128::from(start) + k as i128 * i128::from(step))?;
    Ok(Some(Storage::Native(column)))
}

/// Number of elements of `range(start, stop, step)` (`step != 0`).
pub fn range_len(start: i64, stop: i64, step: i64) -> usize {
    let (start, stop, step) = (i128::from(start), i128::from(stop), i128::from(step));
    let n = if step > 0 && start < stop {
        (stop - start - 1) / step + 1
    } else if step < 0 && start > stop {
        (start - stop - 1) / (-step) + 1
    } else {
        0
    };
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// Build a numeric column of `len` elements `value(k)`; all values must be in range for
/// `dtype` (checked by the caller). Large columns are generated detached.
pub fn generate(
    py: Python<'_>,
    dtype: DType,
    len: usize,
    value: impl Fn(usize) -> i128 + Send + Sync,
) -> PyResult<Column> {
    let item = dtype.item_size().unwrap_or(8);
    if len.checked_mul(item).is_none_or(|bytes| bytes > isize::MAX as usize) {
        return Err(PyMemoryError::new_err(()));
    }
    macro_rules! fill {
        ($t:ty, $conv:expr) => {{
            let mut v: Vec<$t> = Vec::new();
            v.try_reserve_exact(len).map_err(|_| PyMemoryError::new_err(()))?;
            let conv = $conv;
            let run = |v: &mut Vec<$t>| v.extend((0..len).map(|k| conv(value(k))));
            if len >= 1 << 16 { py.detach(|| run(&mut v)) } else { run(&mut v) }
            Column::from(v)
        }};
    }
    Ok(match dtype {
        DType::I8 => fill!(i8, |x: i128| x as i8),
        DType::I16 => fill!(i16, |x: i128| x as i16),
        DType::I32 => fill!(i32, |x: i128| x as i32),
        DType::I64 => fill!(i64, |x: i128| x as i64),
        DType::U8 => fill!(u8, |x: i128| x as u8),
        DType::U16 => fill!(u16, |x: i128| x as u16),
        DType::U32 => fill!(u32, |x: i128| x as u32),
        DType::U64 => fill!(u64, |x: i128| x as u64),
        DType::F32 => fill!(f32, |x: i128| x as f32),
        DType::F64 => fill!(f64, |x: i128| x as f64),
        DType::Bool => fill!(bool, |x: i128| x != 0),
        DType::Str => return Err(pyo3::exceptions::PyTypeError::new_err("cannot generate str values")),
    })
}

/// Objects exporting a 1-D buffer whose format matches a numeric dtype (`array.array`,
/// NumPy, `bytes`, `memoryview`): one copy, no per-element objects.
fn ingest_buffer(py: Python<'_>, schema: &Arc<Schema>, obj: &Bound<'_, PyAny>) -> Result<Option<Storage>, IngestError> {
    if !(schema.is_any() || schema.preferred_dtype().is_some_and(|d| d != DType::Str)) {
        return Ok(None);
    }
    // SAFETY: protocol check on a valid object.
    if unsafe { ffi::PyObject_CheckBuffer(obj.as_ptr()) } == 0 {
        return Ok(None);
    }
    let Ok(buffer) = PyUntypedBuffer::get(obj) else {
        // Not exportable right now (e.g. a `List` with object strategy): iterate instead.
        return Ok(None);
    };
    let Some(column) = buffer_column(py, &buffer)? else {
        buffer.release(py);
        return Ok(None);
    };
    buffer.release(py);
    let storage = Storage::Native(column);
    if schema.is_any() {
        Ok(Some(storage.normalize_any(py)))
    } else {
        cast_native(py, storage, schema).map(Some)
    }
}

fn buffer_column(py: Python<'_>, buffer: &PyUntypedBuffer) -> PyResult<Option<Column>> {
    if buffer.dimensions() != 1 {
        return Ok(None);
    }
    let Some(dtype) = buffer.format().to_str().ok().and_then(DType::from_buffer_format) else {
        return Ok(None);
    };
    if dtype.item_size() != Some(buffer.item_size()) {
        return Ok(None);
    }
    macro_rules! copy {
        ($t:ty) => {
            match buffer.as_typed::<$t>() {
                Ok(typed) => Column::from(typed.to_vec(py)?),
                Err(_) => return Ok(None),
            }
        };
    }
    Ok(Some(match dtype {
        DType::I8 => copy!(i8),
        DType::I16 => copy!(i16),
        DType::I32 => copy!(i32),
        DType::I64 => copy!(i64),
        DType::U8 => copy!(u8),
        DType::U16 => copy!(u16),
        DType::U32 => copy!(u32),
        DType::U64 => copy!(u64),
        DType::F32 => copy!(f32),
        DType::F64 => copy!(f64),
        DType::Bool => {
            // Read bytes, never `bool`s: a buffer may hold bytes other than 0/1.
            if !buffer.is_c_contiguous() {
                return Ok(None);
            }
            let n = buffer.item_count();
            // SAFETY: a C-contiguous 1-D buffer of `n` one-byte items, kept alive by
            // `buffer` for the duration of the copy.
            let bytes = unsafe { std::slice::from_raw_parts(buffer.buf_ptr().cast::<u8>(), n) };
            Column::Bool(bytes.iter().map(|&b| b != 0).collect())
        }
        DType::Str => return Ok(None),
    }))
}

// ---------------------------------------------------------------------------------------
// Storage conversion between schemas
// ---------------------------------------------------------------------------------------

/// Convert storage whose elements are valid for `from` into storage for `to`.
pub fn convert_storage(
    py: Python<'_>,
    storage: Storage,
    from: &Arc<Schema>,
    to: &Arc<Schema>,
) -> Result<Storage, IngestError> {
    if Arc::ptr_eq(from, to) {
        return Ok(storage);
    }
    if to.is_any() {
        return Ok(storage.normalize_any(py));
    }
    cast_native(py, storage, to)
}

/// Element-wise conversion of any storage to the typed schema `to`; native columns are
/// converted without creating Python objects where possible.
fn cast_native(py: Python<'_>, storage: Storage, to: &Schema) -> Result<Storage, IngestError> {
    let column = match storage {
        Storage::Empty => return Ok(Storage::for_schema(to)),
        Storage::Native(column) if to.preferred_dtype().is_some() => column,
        other => {
            let objects = other.into_objects(py);
            return ingest_objects(py, to, Storage::for_schema(to), objects.into_iter().map(|o| o.into_bound(py)), 0);
        }
    };
    let target = to.preferred_dtype().unwrap_or(DType::I64);
    if column.dtype() == target {
        return Ok(Storage::Native(column));
    }
    match cast_column(&column, target) {
        Ok(converted) => Ok(Storage::Native(converted)),
        // `int` demotes to object storage instead of overflowing: redo element-wise.
        Err((_, CoerceError::Overflow { .. })) if matches!(to, Schema::Int) => {
            let objects = Storage::Native(column).into_objects(py);
            ingest_objects(py, to, Storage::for_schema(to), objects.into_iter().map(|o| o.into_bound(py)), 0)
        }
        Err((index, e)) => Err(coerce_failure(to, index, e)),
    }
}

/// Numeric view of a column element, for dtype conversions.
trait NumElem {
    fn as_int(&self) -> Option<i128>;
    fn as_float(&self) -> Option<f64>;
}

macro_rules! impl_num_elem_int {
    ($($t:ty),*) => {$(
        impl NumElem for $t {
            #[inline]
            fn as_int(&self) -> Option<i128> { Some(*self as i128) }
            #[inline]
            fn as_float(&self) -> Option<f64> { Some(*self as f64) }
        }
    )*};
}

impl_num_elem_int!(i8, i16, i32, i64, u8, u16, u32, u64);

impl NumElem for bool {
    fn as_int(&self) -> Option<i128> {
        Some(i128::from(*self))
    }
    fn as_float(&self) -> Option<f64> {
        Some(f64::from(u8::from(*self)))
    }
}

impl NumElem for f32 {
    fn as_int(&self) -> Option<i128> {
        None
    }
    fn as_float(&self) -> Option<f64> {
        Some(f64::from(*self))
    }
}

impl NumElem for f64 {
    fn as_int(&self) -> Option<i128> {
        None
    }
    fn as_float(&self) -> Option<f64> {
        Some(*self)
    }
}

impl NumElem for CompactString {
    fn as_int(&self) -> Option<i128> {
        None
    }
    fn as_float(&self) -> Option<f64> {
        None
    }
}

/// Convert a column to `target` with the coercion rules of the typed schemas: ints/bools
/// into integer dtypes (range-checked), numbers into float dtypes; `bool`/`str` targets
/// accept only their own dtype. Errors carry the failing index.
pub fn cast_column(column: &Column, target: DType) -> Result<Column, (usize, CoerceError)> {
    let got = python_type_name(column.dtype());
    let type_error = |i: usize| {
        (i, CoerceError::Type { expected: target_name(target).to_owned(), got: got.to_owned() })
    };
    if column.dtype() == target {
        return Ok(column.clone());
    }
    macro_rules! ints {
        ($t:ty) => {
            with_column!(column, v => {
                let mut out: Vec<$t> = Vec::with_capacity(v.len());
                for (i, x) in v.iter().enumerate() {
                    let Some(w) = x.as_int() else { return Err(type_error(i)) };
                    match <$t>::try_from(w) {
                        Ok(t) => out.push(t),
                        Err(_) => {
                            let message = format!("{w} is out of range for {}", target_name(target));
                            return Err((i, CoerceError::Overflow { message }));
                        }
                    }
                }
                Column::from(out)
            })
        };
    }
    Ok(match target {
        DType::I8 => ints!(i8),
        DType::I16 => ints!(i16),
        DType::I32 => ints!(i32),
        DType::I64 => ints!(i64),
        DType::U8 => ints!(u8),
        DType::U16 => ints!(u16),
        DType::U32 => ints!(u32),
        DType::U64 => ints!(u64),
        DType::F64 => with_column!(column, v => {
            let mut out = Vec::with_capacity(v.len());
            for (i, x) in v.iter().enumerate() {
                out.push(x.as_float().ok_or_else(|| type_error(i))?);
            }
            Column::F64(out)
        }),
        DType::F32 => with_column!(column, v => {
            let mut out = Vec::with_capacity(v.len());
            for (i, x) in v.iter().enumerate() {
                let f = x.as_float().ok_or_else(|| type_error(i))?;
                out.push(f64_to_f32(f).ok_or_else(|| {
                    (i, CoerceError::Overflow { message: format!("{f} is out of range for f32") })
                })?);
            }
            Column::F32(out)
        }),
        DType::Bool | DType::Str => {
            if column.is_empty() {
                Column::new(target)
            } else {
                return Err(type_error(0));
            }
        }
    })
}

fn target_name(dtype: DType) -> &'static str {
    match dtype {
        DType::I64 => "int",
        DType::F64 => "float",
        other => other.name(),
    }
}

// ---------------------------------------------------------------------------------------
// Search probes
// ---------------------------------------------------------------------------------------

/// How a value searched for (`in`, `index`, `count`, `remove`) compares with elements.
pub enum Probe {
    /// A builtin `int`/`float`/`bool`/`str`: compared natively against native storage
    /// (Python equality: `1.0` finds `1`, `True` finds `1`).
    Native(Value),
    /// A builtin that no native element can equal (huge int, string with lone surrogates).
    Never,
    /// Anything else: Python `==` per element.
    Python,
}

pub fn probe(obj: &Bound<'_, PyAny>) -> Probe {
    let py = obj.py();
    let p = obj.as_ptr();
    // SAFETY: each read matches the checked exact type.
    unsafe {
        let ty = ffi::Py_TYPE(p);
        if ty == &raw mut ffi::PyBool_Type {
            Probe::Native(Value::Bool(p == ffi::Py_True()))
        } else if ty == &raw mut ffi::PyFloat_Type {
            Probe::Native(Value::F64(ffi::PyFloat_AS_DOUBLE(p)))
        } else if ty == &raw mut ffi::PyUnicode_Type {
            exact_str(p).map_or(Probe::Never, |s| Probe::Native(Value::Str(s)))
        } else if ty == &raw mut ffi::PyLong_Type {
            match long_as_i128(obj) {
                Ok(Some(v)) => match i64::try_from(v) {
                    Ok(v) => Probe::Native(Value::I64(v)),
                    Err(_) => Probe::Native(Value::U64(v as u64)),
                },
                Ok(None) => big_int_probe(py, obj),
                Err(_) => Probe::Never,
            }
        } else {
            Probe::Python
        }
    }
}

/// A huge int can only equal a float that represents it exactly.
fn big_int_probe(py: Python<'_>, obj: &Bound<'_, PyAny>) -> Probe {
    // SAFETY: `obj` is an exact int.
    let x = unsafe { ffi::PyLong_AsDouble(obj.as_ptr()) };
    if x == -1.0 && PyErr::take(py).is_some() {
        return Probe::Never;
    }
    // int/float comparison in CPython is exact and runs no Python code.
    match x.to_python(py).eq(obj) {
        Ok(true) => Probe::Native(Value::F64(x)),
        _ => Probe::Never,
    }
}
