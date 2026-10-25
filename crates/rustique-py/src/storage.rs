//! Storage strategies (ARCHITECTURE §3.3): native columns with transparent demotion to
//! object storage.
//!
//! OWNER: list agent. Items marked `INTERFACE` are used by dict/set/expr code and must keep
//! their signatures.
//!
//! Every method here is safe to call under a Rustique lock: no Python code runs (only
//! `int`/`float`/`bool`/`str` objects are created, and only increfs happen). Python objects
//! that a mutation removes are handed back as [`Displaced`] so the caller can drop them
//! after unlocking.

use std::{mem, ptr};

use pyo3::exceptions::PyMemoryError;
use pyo3::prelude::*;
use rustique_core::slice::{self, SliceError, SliceIndices};
use rustique_core::{Column, DType, Element, Value, with_column};

use crate::convert::{self, ToPython, column_item, column_objects};
use crate::schema::{Coerced, Schema};

/// INTERFACE. The physical representation of a sequence of elements.
#[derive(Debug)]
pub enum Storage {
    /// Untyped and empty: the first stored value picks the strategy.
    Empty,
    /// Unboxed native values.
    Native(Column),
    /// Python objects (heterogeneous data, unions, nested containers, big ints, …).
    Object(Vec<Py<PyAny>>),
}

/// INTERFACE. Python objects displaced by a mutation. Must be dropped *after* releasing
/// any Rustique lock (dropping decrefs and may run `__del__`).
pub type Displaced = Vec<Py<PyAny>>;

/// Push `value` if it has the column's dtype; hands it back otherwise.
#[inline]
fn try_push(column: &mut Column, value: Value) -> Result<(), Value> {
    with_column!(column, v => {
        v.push(Element::from_value(value)?);
        Ok(())
    })
}

#[inline]
fn try_insert(column: &mut Column, index: usize, value: Value) -> Result<(), Value> {
    with_column!(column, v => {
        v.insert(index, Element::from_value(value)?);
        Ok(())
    })
}

#[inline]
fn try_set(column: &mut Column, index: usize, value: Value) -> Result<(), Value> {
    with_column!(column, v => {
        v[index] = Element::from_value(value)?;
        Ok(())
    })
}

fn single(value: Value) -> Column {
    let mut column = Column::with_capacity(value.dtype(), 4);
    // The column was created with the value's own dtype.
    let _ = try_push(&mut column, value);
    column
}

impl Storage {
    /// INTERFACE. Initial storage for a schema: `Empty` for `Any`, a native column for
    /// schemas with a preferred dtype, otherwise `Object`.
    pub fn for_schema(schema: &Schema) -> Storage {
        if schema.is_any() {
            return Storage::Empty;
        }
        match schema.preferred_dtype() {
            Some(dtype) => Storage::Native(Column::new(dtype)),
            None => Storage::Object(Vec::new()),
        }
    }

    /// INTERFACE.
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Storage::Empty => 0,
            Storage::Native(c) => c.len(),
            Storage::Object(v) => v.len(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// INTERFACE. Strategy name: `"empty"`, a dtype name (`"i64"`, `"str"`, …) or `"object"`.
    pub fn strategy(&self) -> &'static str {
        match self {
            Storage::Empty => "empty",
            Storage::Native(c) => c.dtype().name(),
            Storage::Object(_) => "object",
        }
    }

    /// INTERFACE. The native dtype, if stored natively.
    pub fn dtype(&self) -> Option<DType> {
        match self {
            Storage::Native(c) => Some(c.dtype()),
            _ => None,
        }
    }

    /// INTERFACE. Append one coerced value, demoting to object storage if it does not fit.
    /// Creating Python objects for demotion is allowed under a lock (no user code runs).
    pub fn push(&mut self, py: Python<'_>, value: Coerced) {
        match (&mut *self, value) {
            (Storage::Native(column), Coerced::Native(v)) => {
                if let Err(v) = try_push(column, v) {
                    self.demote(py);
                    self.push(py, Coerced::Native(v));
                }
            }
            (Storage::Object(objects), value) => objects.push(to_object(py, value)),
            (Storage::Empty, Coerced::Native(v)) => *self = Storage::Native(single(v)),
            (Storage::Empty, Coerced::Object(o)) => *self = Storage::Object(vec![o]),
            (Storage::Native(_), Coerced::Object(o)) => {
                self.demote(py);
                self.push(py, Coerced::Object(o));
            }
        }
    }

    /// Whether a value of this classification is stored without demotion.
    #[inline]
    fn fits(&self, value: &Coerced) -> bool {
        match (self, value) {
            (Storage::Native(column), Coerced::Native(v)) => column.dtype() == v.dtype(),
            (Storage::Native(_), Coerced::Object(_)) => false,
            _ => true,
        }
    }

    /// Untyped containers: how `obj` is stored. Object storage keeps the object itself
    /// (preserving identity); otherwise its exact-type classification. Runs no Python code.
    #[inline]
    fn adopt(&self, obj: &Bound<'_, PyAny>) -> Coerced {
        match self {
            Storage::Object(_) => Coerced::Object(obj.clone().unbind()),
            // A value that would demote native storage is stored as the original object.
            // (`classify` already returns the object itself for non-native values: reuse it,
            // so no `Py` is dropped here — raw slots must not drop `Py` handles, see
            // `list::slots`.)
            _ => match convert::classify(obj) {
                value if self.fits(&value) => value,
                Coerced::Native(_) => Coerced::Object(obj.clone().unbind()),
                object => object,
            },
        }
    }

    /// `push` for untyped containers; a demoting value is stored as the original object.
    pub fn push_any(&mut self, py: Python<'_>, obj: &Bound<'_, PyAny>) {
        let value = self.adopt(obj);
        self.push(py, value);
    }

    /// `insert` for untyped containers (see [`Storage::push_any`]).
    pub fn insert_any(&mut self, py: Python<'_>, index: usize, obj: &Bound<'_, PyAny>) {
        let value = self.adopt(obj);
        self.insert(py, index, value);
    }

    /// `set_one` for untyped containers (see [`Storage::push_any`]).
    pub fn set_any(&mut self, py: Python<'_>, index: usize, obj: &Bound<'_, PyAny>) -> Option<Py<PyAny>> {
        let value = self.adopt(obj);
        self.set_one(py, index, value)
    }

    /// Insert at a position in `0..=len` (callers clamp first).
    pub fn insert(&mut self, py: Python<'_>, index: usize, value: Coerced) {
        match (&mut *self, value) {
            (Storage::Native(column), Coerced::Native(v)) => {
                if let Err(v) = try_insert(column, index, v) {
                    self.demote(py);
                    self.insert(py, index, Coerced::Native(v));
                }
            }
            (Storage::Object(objects), value) => objects.insert(index, to_object(py, value)),
            (Storage::Empty, value) => self.push(py, value),
            (Storage::Native(_), value) => {
                self.demote(py);
                self.insert(py, index, value);
            }
        }
    }

    /// INTERFACE. The element at a normalised index as a Python object.
    pub fn get(&self, py: Python<'_>, index: usize) -> Option<Py<PyAny>> {
        (index < self.len()).then(|| self.item(py, index).unbind())
    }

    /// The element at `index` (must be in range).
    #[inline]
    pub fn item<'py>(&self, py: Python<'py>, index: usize) -> Bound<'py, PyAny> {
        match self {
            Storage::Native(column) => column_item(py, column, index),
            Storage::Object(objects) => objects[index].bind(py).clone(),
            Storage::Empty => unreachable!("index checked against an empty storage"),
        }
    }

    /// A new reference to the element at `index` (must be in range): the stored object, or
    /// the native value materialised. Runs no Python code.
    #[inline]
    pub fn item_ptr(&self, py: Python<'_>, index: usize) -> *mut pyo3::ffi::PyObject {
        match self {
            Storage::Object(objects) => {
                let p = objects[index].as_ptr();
                // SAFETY: a live object owned by this storage; the new reference is returned.
                unsafe { pyo3::ffi::Py_INCREF(p) };
                p
            }
            Storage::Native(column) => column_item(py, column, index).into_ptr(),
            Storage::Empty => unreachable!("index checked against an empty storage"),
        }
    }

    /// INTERFACE. Replace the element at a normalised index; returns displaced objects.
    pub fn set(&mut self, py: Python<'_>, index: usize, value: Coerced) -> Displaced {
        self.set_one(py, index, value).into_iter().collect()
    }

    /// Replace the element at `index` (in range); returns the displaced object, if any.
    pub fn set_one(&mut self, py: Python<'_>, index: usize, value: Coerced) -> Option<Py<PyAny>> {
        match (&mut *self, value) {
            (Storage::Native(column), Coerced::Native(v)) => match try_set(column, index, v) {
                Ok(()) => None,
                Err(v) => {
                    self.demote(py);
                    self.set_one(py, index, Coerced::Native(v))
                }
            },
            (Storage::Object(objects), value) => Some(mem::replace(&mut objects[index], to_object(py, value))),
            (Storage::Native(_), value) => {
                self.demote(py);
                self.set_one(py, index, value)
            }
            // Unreachable (no index is in range); never drop an object under the lock.
            (Storage::Empty, Coerced::Object(o)) => Some(o),
            (Storage::Empty, Coerced::Native(_)) => None,
        }
    }

    /// INTERFACE. Remove the element at a normalised index, returning it as a Python object.
    pub fn remove(&mut self, py: Python<'_>, index: usize) -> Py<PyAny> {
        match self {
            Storage::Native(column) => {
                let item = column_item(py, column, index).unbind();
                with_column!(column, v => drop(v.remove(index)));
                item
            }
            Storage::Object(objects) => objects.remove(index),
            Storage::Empty => unreachable!("index checked against an empty storage"),
        }
    }

    /// Remove the element at `index` without materialising it; returns the displaced
    /// object for object storage.
    pub fn delete(&mut self, index: usize) -> Option<Py<PyAny>> {
        match self {
            Storage::Native(column) => {
                with_column!(column, v => drop(v.remove(index)));
                None
            }
            Storage::Object(objects) => Some(objects.remove(index)),
            Storage::Empty => None,
        }
    }

    /// Position of an element previously read from this storage: the identical object for
    /// object storage, an equal native value otherwise; `hint` is checked first.
    pub fn locate(&self, hint: usize, item: &Bound<'_, PyAny>) -> Option<usize> {
        match self {
            Storage::Empty => None,
            Storage::Object(objects) => {
                if objects.get(hint).is_some_and(|o| o.is(item)) {
                    return Some(hint);
                }
                objects.iter().position(|o| o.is(item))
            }
            Storage::Native(column) => {
                // `probe`, not `classify`: a materialised `u64` above `i64::MAX` is an exact
                // `int` that classifies as an object but probes natively.
                let convert::Probe::Native(value) = convert::probe(item) else {
                    return None;
                };
                if hint < column.len() && rustique_core::kernels::find(column, &value, hint, hint + 1).is_some() {
                    return Some(hint);
                }
                rustique_core::kernels::find(column, &value, 0, column.len())
            }
        }
    }

    /// INTERFACE. Convert to object storage in place (no-op if already object storage).
    pub fn demote(&mut self, py: Python<'_>) {
        match self {
            Storage::Object(_) => {}
            Storage::Empty => *self = Storage::Object(Vec::new()),
            Storage::Native(column) => {
                let objects = column_objects(py, column, 0..column.len());
                *self = Storage::Object(objects);
            }
        }
    }

    /// INTERFACE. Clone the storage (native: copies the column; object: increfs).
    pub fn clone_ref(&self, py: Python<'_>) -> Storage {
        match self {
            Storage::Empty => Storage::Empty,
            Storage::Native(column) => Storage::Native(column.clone()),
            Storage::Object(objects) => Storage::Object(objects.iter().map(|o| o.clone_ref(py)).collect()),
        }
    }

    /// Clone `self[range]` (native: copies; object: increfs).
    pub fn clone_range(&self, py: Python<'_>, range: std::ops::Range<usize>) -> Storage {
        match self {
            Storage::Empty => Storage::Empty,
            Storage::Native(column) => Storage::Native(with_column!(column, v => Column::from(v[range].to_vec()))),
            Storage::Object(objects) => Storage::Object(objects[range].iter().map(|o| o.clone_ref(py)).collect()),
        }
    }

    /// Python objects for `self[range]` (materialised or increfed).
    pub fn objects(&self, py: Python<'_>, range: std::ops::Range<usize>) -> Vec<Py<PyAny>> {
        match self {
            Storage::Empty => Vec::new(),
            Storage::Native(column) => column_objects(py, column, range),
            Storage::Object(objects) => objects[range].iter().map(|o| o.clone_ref(py)).collect(),
        }
    }

    /// Append Python objects for `self[range]` to `out`, in reverse order if `reversed`.
    pub fn extend_objects(
        &self,
        py: Python<'_>,
        range: std::ops::Range<usize>,
        reversed: bool,
        out: &mut Vec<Py<PyAny>>,
    ) {
        match self {
            Storage::Empty => {}
            Storage::Native(column) => with_column!(column, v => {
                let items = &v[range];
                out.reserve(items.len());
                if reversed {
                    out.extend(items.iter().rev().map(|x| x.to_python(py).unbind()));
                } else {
                    out.extend(items.iter().map(|x| x.to_python(py).unbind()));
                }
            }),
            Storage::Object(objects) => {
                let items = &objects[range];
                if reversed {
                    out.extend(items.iter().rev().map(|o| o.clone_ref(py)));
                } else {
                    out.extend(items.iter().map(|o| o.clone_ref(py)));
                }
            }
        }
    }

    /// All elements as Python objects, consuming the storage.
    pub fn into_objects(self, py: Python<'_>) -> Vec<Py<PyAny>> {
        match self {
            Storage::Empty => Vec::new(),
            Storage::Native(column) => column_objects(py, &column, 0..column.len()),
            Storage::Object(objects) => objects,
        }
    }

    /// Remove all elements, keeping the strategy.
    pub fn clear(&mut self) -> Displaced {
        match self {
            Storage::Empty => Vec::new(),
            Storage::Native(column) => {
                *column = Column::new(column.dtype());
                Vec::new()
            }
            Storage::Object(objects) => mem::take(objects),
        }
    }

    pub fn reverse(&mut self) {
        match self {
            Storage::Empty => {}
            Storage::Native(column) => column.reverse(),
            Storage::Object(objects) => objects.reverse(),
        }
    }

    /// `self[s]` as new storage of the same strategy.
    pub fn take_slice(&self, py: Python<'_>, s: &SliceIndices) -> Storage {
        match self {
            Storage::Empty => Storage::Empty,
            Storage::Native(column) => Storage::Native(column.take_slice(s)),
            Storage::Object(objects) => Storage::Object(if s.step == 1 {
                let start = s.start as usize;
                objects[start..start + s.len].iter().map(|o| o.clone_ref(py)).collect()
            } else {
                s.positions().map(|i| objects[i].clone_ref(py)).collect()
            }),
        }
    }

    /// `del self[s]`.
    pub fn delete_slice(&mut self, s: &SliceIndices) -> Displaced {
        match self {
            Storage::Empty => Vec::new(),
            Storage::Native(column) => {
                drop(column.delete_slice(s));
                Vec::new()
            }
            Storage::Object(objects) => slice::delete_slice(objects, s),
        }
    }

    /// `self[s] = values` with list semantics; demotes when the strategies differ. On a
    /// length mismatch nothing changes and the rejected `values` are handed back with the
    /// error: they may hold Python objects, which the caller drops after unlocking.
    pub fn assign_slice(
        &mut self,
        py: Python<'_>,
        s: &SliceIndices,
        values: Storage,
    ) -> Result<Displaced, (SliceError, Storage)> {
        if s.step != 1 && values.len() != s.len {
            return Err((SliceError::LengthMismatch { expected: s.len, got: values.len() }, values));
        }
        // From here on the assignment cannot fail: the length matches and `s` is adjusted.
        match (&mut *self, values) {
            (Storage::Empty, values) => {
                // An empty untyped list: the assigned values decide the strategy.
                if !values.is_empty() {
                    *self = values;
                }
                Ok(Vec::new())
            }
            (Storage::Native(column), Storage::Native(new)) if new.dtype() == column.dtype() => {
                // Native values: dropping a (never expected) rejected column runs no code.
                column.assign_slice(s, new).map(drop).map_err(|e| match e {
                    rustique_core::ColumnError::Slice(e) => (e, Storage::Empty),
                    _ => (SliceError::ZeroStep, Storage::Empty),
                })?;
                Ok(Vec::new())
            }
            (Storage::Native(column), values) if values.is_empty() => {
                let empty = Column::new(column.dtype());
                column.assign_slice(s, empty).map(drop).map_err(|_| (SliceError::ZeroStep, values))?;
                Ok(Vec::new())
            }
            (Storage::Object(objects), values) => slice::try_assign_slice(objects, s, values.into_objects(py))
                .map_err(|(e, rejected)| (e, Storage::Object(rejected))),
            (Storage::Native(_), values) => {
                self.demote(py);
                self.assign_slice(py, s, values)
            }
        }
    }

    /// Append all of `other` (`list.extend`), demoting when the strategies differ.
    pub fn extend(&mut self, py: Python<'_>, other: Storage) {
        if other.is_empty() {
            return;
        }
        match (&mut *self, other) {
            (Storage::Empty, other) => *self = other,
            (Storage::Native(column), Storage::Native(new)) if new.dtype() == column.dtype() => {
                // Same dtype: cannot fail.
                let _ = column.append(new);
            }
            (Storage::Object(objects), other) => objects.extend(other.into_objects(py)),
            (Storage::Native(_), other) => {
                self.demote(py);
                self.extend(py, other);
            }
        }
    }

    /// `self + other` as new storage when no conversion is needed (same native dtype, both
    /// object storage, or either side empty); `None` otherwise. Runs no Python code.
    pub fn concat(&self, py: Python<'_>, other: &Storage) -> Option<Storage> {
        Some(match (self, other) {
            (_, Storage::Empty) => self.clone_ref(py),
            (Storage::Empty, _) => other.clone_ref(py),
            (Storage::Native(a), Storage::Native(b)) => {
                Storage::Native(rustique_core::with_column_pair!(a, b, (x, y) => {
                let mut v = Vec::with_capacity(x.len() + y.len());
                v.extend_from_slice(x);
                v.extend_from_slice(y);
                Column::from(v)
            }, _ => return None))
            }
            (Storage::Object(a), Storage::Object(b)) => {
                let mut v = Vec::with_capacity(a.len() + b.len());
                v.extend(a.iter().chain(b).map(|o| o.clone_ref(py)));
                Storage::Object(v)
            }
            _ => return None,
        })
    }

    /// `self * n` (list semantics: `n <= 0` gives an empty sequence). `MemoryError` when
    /// the result cannot be allocated.
    pub fn repeat(&self, py: Python<'_>, n: isize) -> PyResult<Storage> {
        let len = self.len();
        if n <= 0 || len == 0 {
            return Ok(match self {
                Storage::Native(column) => Storage::Native(Column::new(column.dtype())),
                Storage::Object(_) => Storage::Object(Vec::new()),
                Storage::Empty => Storage::Empty,
            });
        }
        let total = checked_total(len, n as usize, self.item_size())?;
        Ok(match self {
            Storage::Empty => Storage::Empty,
            Storage::Native(column) => {
                Storage::Native(with_column!(column, v => Column::from(repeat_slice(v, total)?)))
            }
            Storage::Object(objects) => {
                let mut out = reserve(total)?;
                while out.len() < total {
                    out.extend(objects.iter().map(|o| o.clone_ref(py)));
                }
                Storage::Object(out)
            }
        })
    }

    /// The Python objects this storage holds (none for native storage), to be dropped by
    /// the caller after unlocking.
    pub fn into_displaced(self) -> Displaced {
        match self {
            Storage::Object(objects) => objects,
            _ => Vec::new(),
        }
    }

    /// Empty storage with the same strategy.
    pub fn empty_like(&self) -> Storage {
        match self {
            Storage::Empty => Storage::Empty,
            Storage::Native(column) => Storage::Native(Column::new(column.dtype())),
            Storage::Object(_) => Storage::Object(Vec::new()),
        }
    }

    fn item_size(&self) -> usize {
        match self {
            Storage::Native(column) => column.dtype().item_size().unwrap_or(size_of::<compact_str::CompactString>()),
            _ => size_of::<Py<PyAny>>(),
        }
    }

    /// Re-express storage for an untyped container: only the `bool`/`i64`/`f64`/`str`
    /// strategies (and objects) exist there, so narrower dtypes from typed sources widen.
    pub fn normalize_any(self, py: Python<'_>) -> Storage {
        let Storage::Native(column) = self else {
            return self;
        };
        match column.dtype() {
            DType::Bool | DType::I64 | DType::F64 | DType::Str => Storage::Native(column),
            DType::F32 => Storage::Native(Column::F64(
                column.as_slice::<f32>().unwrap_or(&[]).iter().map(|&x| f64::from(x)).collect(),
            )),
            DType::U64 => {
                let values = column.as_slice::<u64>().unwrap_or(&[]);
                if values.iter().all(|&x| i64::try_from(x).is_ok()) {
                    Storage::Native(Column::I64(values.iter().map(|&x| x as i64).collect()))
                } else {
                    Storage::Object(convert::column_objects(py, &column, 0..column.len()))
                }
            }
            _ => match convert::cast_column(&column, DType::I64, "int") {
                Ok(widened) => Storage::Native(widened),
                Err(_) => Storage::Object(convert::column_objects(py, &column, 0..column.len())),
            },
        }
    }
}

/// Native values become Python objects when stored in object storage.
#[inline]
fn to_object(py: Python<'_>, value: Coerced) -> Py<PyAny> {
    match value {
        Coerced::Native(v) => convert::value_to_python(py, &v).unbind(),
        Coerced::Object(o) => o,
    }
}

/// `len * n` elements of `item_size` bytes, or `MemoryError` if that cannot exist.
fn checked_total(len: usize, n: usize, item_size: usize) -> PyResult<usize> {
    len.checked_mul(n)
        .filter(|total| total.checked_mul(item_size.max(1)).is_some_and(|bytes| bytes <= isize::MAX as usize))
        .ok_or_else(|| PyMemoryError::new_err(()))
}

fn reserve<T>(total: usize) -> PyResult<Vec<T>> {
    let mut out = Vec::new();
    out.try_reserve_exact(total).map_err(|_| PyMemoryError::new_err(()))?;
    Ok(out)
}

fn repeat_slice<T: Clone>(v: &[T], total: usize) -> PyResult<Vec<T>> {
    let mut out = reserve(total)?;
    while out.len() < total {
        out.extend_from_slice(v);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Boxed element cache
// ---------------------------------------------------------------------------------------

/// Boxed copies of native elements, so that element-wise reads (`x[i]`, iteration,
/// unpacking) of a native container read repeatedly cost an incref instead of creating
/// a new `int`/`float`/`str` object each time.
///
/// A container enables its cache once it has served more element reads than it has
/// elements (plus a margin) since it was last invalidated (see [`ElemCache::note_read`]):
/// code that reads every element once never pays for it. The slots are filled on demand.
/// Cached objects are exact `int`/`float`/`bool`/`str` objects, whose deallocation runs no
/// Python code, so the cache may be cleared under a container lock.
///
/// Concurrency: the slot array is created by any reader (racing creators: one wins) and
/// slots are filled by readers with atomic stores (compare-exchange on free-threaded
/// builds); every other change (clearing, pushing, popping, replacing slots) needs the
/// container's exclusive access, which the `unsafe` methods require of their callers.
/// All methods must be called attached.
#[derive(Default)]
pub struct ElemCache {
    slots: std::sync::atomic::AtomicPtr<Vec<std::sync::atomic::AtomicPtr<pyo3::ffi::PyObject>>>,
    /// Element reads since the cache was last cleared (racy counting is fine).
    reads: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Debug for ElemCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElemCache").field("enabled", &!self.slots.load(Relaxed).is_null()).finish()
    }
}

#[cfg(not(Py_GIL_DISABLED))]
use std::sync::atomic::Ordering::Release;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed};

impl ElemCache {
    /// Containers longer than this never cache (bounds the memory a cache can pin).
    pub const MAX_LEN: usize = 1 << 22;

    pub const fn new() -> Self {
        ElemCache {
            slots: std::sync::atomic::AtomicPtr::new(ptr::null_mut()),
            reads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    #[inline(always)]
    fn slots(&self) -> Option<&Vec<std::sync::atomic::AtomicPtr<pyo3::ffi::PyObject>>> {
        // SAFETY: a non-null pointer is a live boxed vector, freed only with exclusive
        // access (`clear`/drop), which excludes the caller's shared access.
        unsafe { self.slots.load(Acquire).as_ref() }
    }

    /// Whether the cache is enabled.
    #[inline(always)]
    pub fn is_enabled(&self) -> bool {
        !self.slots.load(Relaxed).is_null()
    }

    /// Count one element read of a container of `len` elements; enable the cache once the
    /// reads exceed `len + 16`. Callers have shared access to the container.
    #[inline(always)]
    pub fn note_read(&self, len: usize) {
        if self.is_enabled() {
            return;
        }
        let reads = self.reads.load(Relaxed) + 1;
        self.reads.store(reads, Relaxed);
        if reads > len + 16 && len <= Self::MAX_LEN {
            self.enable(len);
        }
    }

    #[cold]
    #[inline(never)]
    fn enable(&self, len: usize) {
        let mut slots = Vec::with_capacity(len);
        slots.resize_with(len, || std::sync::atomic::AtomicPtr::new(ptr::null_mut()));
        let new = Box::into_raw(Box::new(slots));
        if self.slots.compare_exchange(ptr::null_mut(), new, AcqRel, Acquire).is_err() {
            // SAFETY: never published.
            drop(unsafe { Box::from_raw(new) });
        }
    }

    /// A new reference to element `i` of `storage` (in range), from the cache when it is
    /// enabled and holds the element. Callers have shared access to the container.
    #[inline(always)]
    pub fn item(&self, py: Python<'_>, storage: &Storage, i: usize) -> *mut pyo3::ffi::PyObject {
        let Storage::Native(column) = storage else {
            return storage.item_ptr(py, i);
        };
        let Some(slot) = self.slots().and_then(|s| s.get(i)) else {
            return column_item(py, column, i).into_ptr();
        };
        let p = slot.load(Acquire);
        if !p.is_null() {
            // SAFETY: the cache owns a reference; a new one is returned.
            unsafe { pyo3::ffi::Py_INCREF(p) };
            return p;
        }
        let new = column_item(py, column, i).into_ptr();
        #[cfg(not(Py_GIL_DISABLED))]
        {
            // Readers are serialised by the GIL (detached kernels never touch the cache).
            slot.store(new, Release);
            // SAFETY: one reference for the cache, one returned.
            unsafe { pyo3::ffi::Py_INCREF(new) };
            new
        }
        #[cfg(Py_GIL_DISABLED)]
        match slot.compare_exchange(ptr::null_mut(), new, AcqRel, Acquire) {
            Ok(_) => {
                // SAFETY: one reference for the cache, one returned.
                unsafe { pyo3::ffi::Py_INCREF(new) };
                new
            }
            Err(existing) => {
                // SAFETY: `new` is unshared (an exact builtin scalar: its deallocation runs
                // no Python code); `existing` is owned by the cache.
                unsafe {
                    pyo3::ffi::Py_DECREF(new);
                    pyo3::ffi::Py_INCREF(existing);
                }
                existing
            }
        }
    }

    /// Drop the cache (and reset the read count).
    ///
    /// # Safety
    /// The caller must have exclusive access to the container and be attached.
    #[inline]
    pub unsafe fn clear(&self) {
        self.reads.store(0, Relaxed);
        if self.slots.load(Relaxed).is_null() {
            return;
        }
        let old = self.slots.swap(ptr::null_mut(), AcqRel);
        if !old.is_null() {
            // SAFETY: exclusive access: no reader holds the vector.
            unsafe { release_slots(*Box::from_raw(old)) };
        }
    }

    /// Mirror an append to the container.
    ///
    /// # Safety
    /// As [`ElemCache::clear`].
    #[inline]
    pub unsafe fn push(&self) {
        // SAFETY: exclusive access (caller contract).
        if let Some(slots) = unsafe { self.slots.load(Relaxed).as_mut() } {
            if slots.len() >= Self::MAX_LEN {
                // SAFETY: as above.
                unsafe { self.clear() };
            } else {
                slots.push(std::sync::atomic::AtomicPtr::new(ptr::null_mut()));
            }
        }
    }

    /// Mirror a replacement of element `i` by the native value of `value` (an exact `int`,
    /// `float` or `str` equal to the stored element, so it becomes the cached box: like a
    /// builtin list, `x[i] = v; x[i] is v` holds), or empty the slot (`value` null).
    ///
    /// # Safety
    /// As [`ElemCache::clear`]; `value` must be null or a live object as described.
    #[inline]
    pub unsafe fn replace(&self, i: usize, value: *mut pyo3::ffi::PyObject) {
        if let Some(slot) = self.slots().and_then(|s| s.get(i)) {
            if !value.is_null() {
                // SAFETY: a live object; the cache takes a reference.
                unsafe { pyo3::ffi::Py_INCREF(value) };
            }
            let old = slot.swap(value, Relaxed);
            if !old.is_null() {
                // SAFETY: owned by the cache; an exact builtin scalar.
                unsafe { pyo3::ffi::Py_DECREF(old) };
            }
        }
    }

    /// Mirror removing the last of `len` elements; returns its cached object (a new
    /// reference transferred from the cache) if there was one.
    ///
    /// # Safety
    /// As [`ElemCache::clear`].
    #[inline]
    pub unsafe fn pop(&self, len: usize) -> Option<*mut pyo3::ffi::PyObject> {
        // SAFETY: exclusive access (caller contract).
        let slots = unsafe { self.slots.load(Relaxed).as_mut() }?;
        if slots.len() != len {
            // Out of step with the container (never expected): start over.
            // SAFETY: as above.
            unsafe { self.clear() };
            return None;
        }
        let p = slots.pop()?.into_inner();
        (!p.is_null()).then_some(p)
    }
}

/// Release every cached object of a slot vector.
///
/// # Safety
/// Attached; the vector is no longer shared.
unsafe fn release_slots(slots: Vec<std::sync::atomic::AtomicPtr<pyo3::ffi::PyObject>>) {
    for slot in slots.iter() {
        let p = slot.load(Relaxed);
        if !p.is_null() {
            // SAFETY: owned by the cache; exact builtin scalars.
            unsafe { pyo3::ffi::Py_DECREF(p) };
        }
    }
}

impl Drop for ElemCache {
    fn drop(&mut self) {
        let old = *self.slots.get_mut();
        if !old.is_null() {
            // SAFETY: the owner is being dropped (attached: containers die in tp_dealloc).
            unsafe { release_slots(*Box::from_raw(old)) };
        }
    }
}
