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

use std::mem;

use pyo3::exceptions::PyMemoryError;
use pyo3::prelude::*;
use rustique_core::slice::{self, SliceError, SliceIndices};
use rustique_core::{Column, DType, Element, Value, with_column};

use crate::convert::{self, column_item, column_objects};
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
            _ => match convert::classify(obj) {
                value if self.fits(&value) => value,
                _ => Coerced::Object(obj.clone().unbind()),
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
            (Storage::Empty, _) => None,
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
                let Coerced::Native(value) = convert::classify(item) else {
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

    /// `self[s] = values` with list semantics; demotes when the strategies differ.
    pub fn assign_slice(&mut self, py: Python<'_>, s: &SliceIndices, values: Storage) -> Result<Displaced, SliceError> {
        if s.step != 1 && values.len() != s.len {
            return Err(SliceError::LengthMismatch { expected: s.len, got: values.len() });
        }
        match (&mut *self, values) {
            (Storage::Empty, values) => {
                // An empty untyped list: the assigned values decide the strategy.
                if !values.is_empty() {
                    *self = values;
                }
                Ok(Vec::new())
            }
            (Storage::Native(column), Storage::Native(new)) if new.dtype() == column.dtype() => {
                column.assign_slice(s, new).map(drop).map_err(|e| match e {
                    rustique_core::ColumnError::Slice(e) => e,
                    _ => SliceError::ZeroStep,
                })?;
                Ok(Vec::new())
            }
            (Storage::Native(column), values) if values.is_empty() => {
                let empty = Column::new(column.dtype());
                column.assign_slice(s, empty).map(drop).map_err(|_| SliceError::ZeroStep)?;
                Ok(Vec::new())
            }
            (Storage::Object(objects), values) => slice::assign_slice(objects, s, values.into_objects(py)),
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
            Storage::Native(column) => Storage::Native(with_column!(column, v => Column::from(repeat_slice(v, total)?))),
            Storage::Object(objects) => {
                let mut out = reserve(total)?;
                while out.len() < total {
                    out.extend(objects.iter().map(|o| o.clone_ref(py)));
                }
                Storage::Object(out)
            }
        })
    }

    /// `self *= n` in place.
    pub fn repeat_in_place(&mut self, py: Python<'_>, n: isize) -> PyResult<Displaced> {
        if n <= 0 {
            return Ok(self.clear());
        }
        if n == 1 || self.is_empty() {
            return Ok(Vec::new());
        }
        let repeated = self.repeat(py, n)?;
        *self = repeated;
        Ok(Vec::new())
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
            DType::F32 => Storage::Native(Column::F64(column.as_slice::<f32>().unwrap_or(&[]).iter().map(|&x| f64::from(x)).collect())),
            DType::U64 => {
                let values = column.as_slice::<u64>().unwrap_or(&[]);
                if values.iter().all(|&x| i64::try_from(x).is_ok()) {
                    Storage::Native(Column::I64(values.iter().map(|&x| x as i64).collect()))
                } else {
                    Storage::Object(convert::column_objects(py, &column, 0..column.len()))
                }
            }
            _ => match convert::cast_column(&column, DType::I64) {
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
