//! Typed, unboxed storage.

use std::fmt;

use compact_str::CompactString;

use crate::dtype::DType;
use crate::slice::{self, SliceError, SliceIndices};

/// One native element.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Str(CompactString),
}

/// Typed, contiguous storage for one native element type.
#[derive(Clone, Debug, PartialEq)]
pub enum Column {
    Bool(Vec<bool>),
    I8(Vec<i8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    I64(Vec<i64>),
    U8(Vec<u8>),
    U16(Vec<u16>),
    U32(Vec<u32>),
    U64(Vec<u64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
    Str(Vec<CompactString>),
}

/// Errors from structural column operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColumnError {
    /// A value or column of the wrong dtype was supplied (a bug in the caller).
    TypeMismatch { expected: DType, found: DType },
    /// Position outside `0..len` (or `0..=len` for insertion).
    IndexOutOfRange { index: usize, len: usize },
    Slice(SliceError),
}

impl fmt::Display for ColumnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ColumnError::TypeMismatch { expected, found } => {
                write!(f, "expected {expected} value, found {found}")
            }
            ColumnError::IndexOutOfRange { index, len } => {
                write!(f, "index {index} out of range for length {len}")
            }
            ColumnError::Slice(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ColumnError {}

impl From<SliceError> for ColumnError {
    fn from(e: SliceError) -> Self {
        ColumnError::Slice(e)
    }
}

/// Dispatch `$body` over every column variant, binding the inner `Vec<T>` to `$v`.
/// `$body` is type-checked separately for each `T`.
#[macro_export]
macro_rules! with_column {
    ($col:expr, $v:ident => $body:expr) => {
        match $col {
            $crate::Column::Bool($v) => $body,
            $crate::Column::I8($v) => $body,
            $crate::Column::I16($v) => $body,
            $crate::Column::I32($v) => $body,
            $crate::Column::I64($v) => $body,
            $crate::Column::U8($v) => $body,
            $crate::Column::U16($v) => $body,
            $crate::Column::U32($v) => $body,
            $crate::Column::U64($v) => $body,
            $crate::Column::F32($v) => $body,
            $crate::Column::F64($v) => $body,
            $crate::Column::Str($v) => $body,
        }
    };
}

/// Like [`with_column!`], but rebuilds a column of the same variant from `$body`'s `Vec`.
#[macro_export]
macro_rules! map_column {
    ($col:expr, $v:ident => $body:expr) => {
        match $col {
            $crate::Column::Bool($v) => $crate::Column::Bool($body),
            $crate::Column::I8($v) => $crate::Column::I8($body),
            $crate::Column::I16($v) => $crate::Column::I16($body),
            $crate::Column::I32($v) => $crate::Column::I32($body),
            $crate::Column::I64($v) => $crate::Column::I64($body),
            $crate::Column::U8($v) => $crate::Column::U8($body),
            $crate::Column::U16($v) => $crate::Column::U16($body),
            $crate::Column::U32($v) => $crate::Column::U32($body),
            $crate::Column::U64($v) => $crate::Column::U64($body),
            $crate::Column::F32($v) => $crate::Column::F32($body),
            $crate::Column::F64($v) => $crate::Column::F64($body),
            $crate::Column::Str($v) => $crate::Column::Str($body),
        }
    };
}

/// Dispatch over two columns of the *same* variant; evaluates `$mismatch` otherwise.
#[macro_export]
macro_rules! with_column_pair {
    ($a:expr, $b:expr, ($x:ident, $y:ident) => $body:expr, _ => $mismatch:expr) => {
        match ($a, $b) {
            ($crate::Column::Bool($x), $crate::Column::Bool($y)) => $body,
            ($crate::Column::I8($x), $crate::Column::I8($y)) => $body,
            ($crate::Column::I16($x), $crate::Column::I16($y)) => $body,
            ($crate::Column::I32($x), $crate::Column::I32($y)) => $body,
            ($crate::Column::I64($x), $crate::Column::I64($y)) => $body,
            ($crate::Column::U8($x), $crate::Column::U8($y)) => $body,
            ($crate::Column::U16($x), $crate::Column::U16($y)) => $body,
            ($crate::Column::U32($x), $crate::Column::U32($y)) => $body,
            ($crate::Column::U64($x), $crate::Column::U64($y)) => $body,
            ($crate::Column::F32($x), $crate::Column::F32($y)) => $body,
            ($crate::Column::F64($x), $crate::Column::F64($y)) => $body,
            ($crate::Column::Str($x), $crate::Column::Str($y)) => $body,
            _ => $mismatch,
        }
    };
}

/// A Rust type that has a native column representation.
pub trait Element: Clone + Send + Sync + 'static {
    const DTYPE: DType;
    fn into_value(self) -> Value;
    fn from_value(value: Value) -> Result<Self, Value>;
    fn slice(column: &Column) -> Option<&[Self]>;
    fn vec_mut(column: &mut Column) -> Option<&mut Vec<Self>>;
    fn into_column(values: Vec<Self>) -> Column;
    fn from_column(column: Column) -> Result<Vec<Self>, Column>;
}

macro_rules! impl_element {
    ($($ty:ty => $variant:ident),* $(,)?) => {$(
        impl Element for $ty {
            const DTYPE: DType = DType::$variant;
            #[inline]
            fn into_value(self) -> Value { Value::$variant(self) }
            #[inline]
            fn from_value(value: Value) -> Result<Self, Value> {
                match value { Value::$variant(x) => Ok(x), other => Err(other) }
            }
            #[inline]
            fn slice(column: &Column) -> Option<&[Self]> {
                match column { Column::$variant(v) => Some(v), _ => None }
            }
            #[inline]
            fn vec_mut(column: &mut Column) -> Option<&mut Vec<Self>> {
                match column { Column::$variant(v) => Some(v), _ => None }
            }
            #[inline]
            fn into_column(values: Vec<Self>) -> Column { Column::$variant(values) }
            #[inline]
            fn from_column(column: Column) -> Result<Vec<Self>, Column> {
                match column { Column::$variant(v) => Ok(v), other => Err(other) }
            }
        }
        impl From<Vec<$ty>> for Column {
            fn from(values: Vec<$ty>) -> Self { Column::$variant(values) }
        }
        impl From<$ty> for Value {
            fn from(value: $ty) -> Self { Value::$variant(value) }
        }
    )*};
}

impl_element! {
    bool => Bool, i8 => I8, i16 => I16, i32 => I32, i64 => I64,
    u8 => U8, u16 => U16, u32 => U32, u64 => U64, f32 => F32, f64 => F64,
    CompactString => Str,
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Str(CompactString::from(value))
    }
}

impl Value {
    pub fn dtype(&self) -> DType {
        match self {
            Value::Bool(_) => DType::Bool,
            Value::I8(_) => DType::I8,
            Value::I16(_) => DType::I16,
            Value::I32(_) => DType::I32,
            Value::I64(_) => DType::I64,
            Value::U8(_) => DType::U8,
            Value::U16(_) => DType::U16,
            Value::U32(_) => DType::U32,
            Value::U64(_) => DType::U64,
            Value::F32(_) => DType::F32,
            Value::F64(_) => DType::F64,
            Value::Str(_) => DType::Str,
        }
    }

    /// The value as `i128` for integer and bool values.
    pub fn as_i128(&self) -> Option<i128> {
        Some(match *self {
            Value::Bool(b) => b as i128,
            Value::I8(x) => x as i128,
            Value::I16(x) => x as i128,
            Value::I32(x) => x as i128,
            Value::I64(x) => x as i128,
            Value::U8(x) => x as i128,
            Value::U16(x) => x as i128,
            Value::U32(x) => x as i128,
            Value::U64(x) => x as i128,
            _ => return None,
        })
    }

    /// The value as `f64` for numeric values (possibly lossy for large integers).
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::F32(x) => Some(x as f64),
            Value::F64(x) => Some(x),
            Value::Str(_) => None,
            ref other => other.as_i128().map(|i| i as f64),
        }
    }

    /// Build an integer value of `dtype` from an `i128`, if it is in range.
    pub fn int_of(dtype: DType, x: i128) -> Option<Value> {
        let (lo, hi) = dtype.int_range()?;
        if x < lo || x > hi {
            return None;
        }
        Some(match dtype {
            DType::I8 => Value::I8(x as i8),
            DType::I16 => Value::I16(x as i16),
            DType::I32 => Value::I32(x as i32),
            DType::I64 => Value::I64(x as i64),
            DType::U8 => Value::U8(x as u8),
            DType::U16 => Value::U16(x as u16),
            DType::U32 => Value::U32(x as u32),
            DType::U64 => Value::U64(x as u64),
            _ => unreachable!("int_range is only defined for integer dtypes"),
        })
    }
}

fn gather<T: Clone>(v: &[T], indices: &[usize]) -> Vec<T> {
    indices.iter().map(|&i| v[i].clone()).collect()
}

fn select<T: Clone>(v: &[T], mask: &[bool]) -> Vec<T> {
    v.iter().zip(mask).filter(|(_, keep)| **keep).map(|(x, _)| x.clone()).collect()
}

fn type_mismatch(expected: DType, value: &Value) -> ColumnError {
    ColumnError::TypeMismatch { expected, found: value.dtype() }
}

impl Column {
    pub fn new(dtype: DType) -> Column {
        Column::with_capacity(dtype, 0)
    }

    pub fn with_capacity(dtype: DType, capacity: usize) -> Column {
        match dtype {
            DType::Bool => Column::Bool(Vec::with_capacity(capacity)),
            DType::I8 => Column::I8(Vec::with_capacity(capacity)),
            DType::I16 => Column::I16(Vec::with_capacity(capacity)),
            DType::I32 => Column::I32(Vec::with_capacity(capacity)),
            DType::I64 => Column::I64(Vec::with_capacity(capacity)),
            DType::U8 => Column::U8(Vec::with_capacity(capacity)),
            DType::U16 => Column::U16(Vec::with_capacity(capacity)),
            DType::U32 => Column::U32(Vec::with_capacity(capacity)),
            DType::U64 => Column::U64(Vec::with_capacity(capacity)),
            DType::F32 => Column::F32(Vec::with_capacity(capacity)),
            DType::F64 => Column::F64(Vec::with_capacity(capacity)),
            DType::Str => Column::Str(Vec::with_capacity(capacity)),
        }
    }

    /// Build a column of `dtype` from values that must all have that dtype.
    pub fn from_values(dtype: DType, values: impl IntoIterator<Item = Value>) -> Result<Column, ColumnError> {
        let values = values.into_iter();
        let mut column = Column::with_capacity(dtype, values.size_hint().0);
        for value in values {
            column.push(value)?;
        }
        Ok(column)
    }

    pub fn dtype(&self) -> DType {
        match self {
            Column::Bool(_) => DType::Bool,
            Column::I8(_) => DType::I8,
            Column::I16(_) => DType::I16,
            Column::I32(_) => DType::I32,
            Column::I64(_) => DType::I64,
            Column::U8(_) => DType::U8,
            Column::U16(_) => DType::U16,
            Column::U32(_) => DType::U32,
            Column::U64(_) => DType::U64,
            Column::F32(_) => DType::F32,
            Column::F64(_) => DType::F64,
            Column::Str(_) => DType::Str,
        }
    }

    pub fn len(&self) -> usize {
        with_column!(self, v => v.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn reserve(&mut self, additional: usize) {
        with_column!(self, v => v.reserve(additional))
    }

    /// Typed view of the data, e.g. `column.as_slice::<i64>()`.
    pub fn as_slice<T: Element>(&self) -> Option<&[T]> {
        T::slice(self)
    }

    /// Typed mutable access to the underlying vector.
    pub fn as_vec_mut<T: Element>(&mut self) -> Option<&mut Vec<T>> {
        T::vec_mut(self)
    }

    /// Element at `index` (already normalised, i.e. non-negative).
    pub fn get(&self, index: usize) -> Option<Value> {
        with_column!(self, v => v.get(index).cloned().map(Element::into_value))
    }

    pub fn set(&mut self, index: usize, value: Value) -> Result<Value, ColumnError> {
        let len = self.len();
        let dtype = self.dtype();
        with_column!(self, v => {
            let slot = v.get_mut(index).ok_or(ColumnError::IndexOutOfRange { index, len })?;
            let new = Element::from_value(value).map_err(|value| type_mismatch(dtype, &value))?;
            Ok(std::mem::replace(slot, new).into_value())
        })
    }

    pub fn push(&mut self, value: Value) -> Result<(), ColumnError> {
        let dtype = self.dtype();
        with_column!(self, v => {
            v.push(Element::from_value(value).map_err(|value| type_mismatch(dtype, &value))?);
            Ok(())
        })
    }

    /// Insert at `index` (`0..=len`; callers apply `clamp_insert_index` first).
    pub fn insert(&mut self, index: usize, value: Value) -> Result<(), ColumnError> {
        let len = self.len();
        if index > len {
            return Err(ColumnError::IndexOutOfRange { index, len });
        }
        let dtype = self.dtype();
        with_column!(self, v => {
            v.insert(index, Element::from_value(value).map_err(|value| type_mismatch(dtype, &value))?);
            Ok(())
        })
    }

    pub fn remove(&mut self, index: usize) -> Result<Value, ColumnError> {
        let len = self.len();
        if index >= len {
            return Err(ColumnError::IndexOutOfRange { index, len });
        }
        Ok(with_column!(self, v => v.remove(index).into_value()))
    }

    pub fn pop(&mut self) -> Option<Value> {
        with_column!(self, v => v.pop().map(Element::into_value))
    }

    pub fn truncate(&mut self, len: usize) {
        with_column!(self, v => v.truncate(len))
    }

    pub fn clear(&mut self) {
        with_column!(self, v => v.clear())
    }

    /// Append a copy of `other` (same dtype).
    pub fn extend_from(&mut self, other: &Column) -> Result<(), ColumnError> {
        let expected = self.dtype();
        with_column_pair!(self, other, (a, b) => { a.extend_from_slice(b); Ok(()) },
            _ => Err(ColumnError::TypeMismatch { expected, found: other.dtype() }))
    }

    /// Append `other` by move (same dtype).
    pub fn append(&mut self, other: Column) -> Result<(), ColumnError> {
        let expected = self.dtype();
        let found = other.dtype();
        with_column_pair!(self, other, (a, b) => { let mut b = b; a.append(&mut b); Ok(()) },
            _ => Err(ColumnError::TypeMismatch { expected, found }))
    }

    pub fn iter(&self) -> impl Iterator<Item = Value> + '_ {
        (0..self.len()).map(move |i| self.get(i).expect("index in range"))
    }

    /// `self[s]` as a new column.
    pub fn take_slice(&self, s: &SliceIndices) -> Column {
        map_column!(self, v => slice::take_slice(v, s))
    }

    /// `del self[s]`; returns the removed elements.
    pub fn delete_slice(&mut self, s: &SliceIndices) -> Column {
        map_column!(self, v => slice::delete_slice(v, s))
    }

    /// `self[s] = values` (same dtype); returns the replaced elements.
    pub fn assign_slice(&mut self, s: &SliceIndices, values: Column) -> Result<Column, ColumnError> {
        let expected = self.dtype();
        let found = values.dtype();
        with_column_pair!(self, values, (a, b) => {
            let removed = slice::assign_slice(a, s, b)?;
            Ok(Element::into_column(removed))
        }, _ => Err(ColumnError::TypeMismatch { expected, found }))
    }

    /// Gather the elements at `indices` (all must be in range).
    pub fn take(&self, indices: &[usize]) -> Column {
        map_column!(self, v => gather(v, indices))
    }

    /// Keep the elements whose mask entry is `true` (`mask.len() == self.len()`).
    pub fn filter(&self, mask: &[bool]) -> Column {
        debug_assert_eq!(mask.len(), self.len());
        map_column!(self, v => select(v, mask))
    }

    pub fn reverse(&mut self) {
        with_column!(self, v => v.reverse())
    }

    /// `self * n` with list semantics.
    pub fn repeat(&self, n: isize) -> Column {
        map_column!(self, v => slice::repeat(v, n))
    }

    /// Raw bytes of a fixed-size column: `(pointer, element count, item size)`.
    /// `None` for `Str`. The pointer is valid until the column is mutated or dropped.
    pub fn raw_parts(&self) -> Option<(*const u8, usize, usize)> {
        let item = self.dtype().item_size()?;
        let ptr = with_column!(self, v => v.as_ptr().cast::<u8>());
        Some((ptr, self.len(), item))
    }
}

impl Default for Column {
    fn default() -> Self {
        Column::I64(Vec::new())
    }
}

impl From<Vec<&str>> for Column {
    fn from(values: Vec<&str>) -> Self {
        Column::Str(values.into_iter().map(CompactString::from).collect())
    }
}

impl FromIterator<i64> for Column {
    fn from_iter<I: IntoIterator<Item = i64>>(iter: I) -> Self {
        Column::I64(iter.into_iter().collect())
    }
}

impl FromIterator<f64> for Column {
    fn from_iter<I: IntoIterator<Item = f64>>(iter: I) -> Self {
        Column::F64(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_get_set_remove() {
        let mut c = Column::new(DType::I64);
        c.push(Value::I64(1)).unwrap();
        c.push(Value::I64(2)).unwrap();
        assert_eq!(c.push(Value::F64(1.0)), Err(ColumnError::TypeMismatch { expected: DType::I64, found: DType::F64 }));
        assert_eq!(c.get(1), Some(Value::I64(2)));
        assert_eq!(c.set(0, Value::I64(10)).unwrap(), Value::I64(1));
        c.insert(1, Value::I64(5)).unwrap();
        assert_eq!(c.as_slice::<i64>(), Some(&[10, 5, 2][..]));
        assert_eq!(c.remove(0).unwrap(), Value::I64(10));
        assert_eq!(c.pop(), Some(Value::I64(2)));
        assert_eq!(c.len(), 1);
        assert!(c.insert(5, Value::I64(0)).is_err());
    }

    #[test]
    fn strings() {
        let mut c = Column::from(vec!["a", "bb"]);
        c.push(Value::from("ccc")).unwrap();
        assert_eq!(c.dtype(), DType::Str);
        assert_eq!(c.get(2), Some(Value::Str("ccc".into())));
        assert!(c.raw_parts().is_none());
    }

    #[test]
    fn slices_and_bulk() {
        let c = Column::from(vec![0i32, 1, 2, 3, 4, 5]);
        let s = SliceIndices::new(None, None, Some(-2), 6).unwrap();
        assert_eq!(c.take_slice(&s), Column::from(vec![5i32, 3, 1]));
        let mut d = c.clone();
        let removed = d.delete_slice(&SliceIndices::new(Some(1), Some(3), None, 6).unwrap());
        assert_eq!(removed, Column::from(vec![1i32, 2]));
        assert_eq!(d, Column::from(vec![0i32, 3, 4, 5]));
        let mut e = c.clone();
        e.assign_slice(&SliceIndices::new(Some(0), Some(2), None, 6).unwrap(), Column::from(vec![9i32])).unwrap();
        assert_eq!(e, Column::from(vec![9i32, 2, 3, 4, 5]));
        assert_eq!(c.take(&[5, 0]), Column::from(vec![5i32, 0]));
        assert_eq!(c.filter(&[true, false, true, false, false, false]), Column::from(vec![0i32, 2]));
        assert_eq!(Column::from(vec![1u8, 2]).repeat(2), Column::from(vec![1u8, 2, 1, 2]));
        let mut f = Column::from(vec![1.0f64]);
        f.extend_from(&Column::from(vec![2.0f64])).unwrap();
        assert!(f.extend_from(&Column::from(vec![1i64])).is_err());
        f.reverse();
        assert_eq!(f.as_slice::<f64>(), Some(&[2.0, 1.0][..]));
        let (_, n, size) = f.raw_parts().unwrap();
        assert_eq!((n, size), (2, 8));
    }

    #[test]
    fn int_of_ranges() {
        assert_eq!(Value::int_of(DType::I8, 127), Some(Value::I8(127)));
        assert_eq!(Value::int_of(DType::I8, 128), None);
        assert_eq!(Value::int_of(DType::U8, -1), None);
        assert_eq!(Value::I16(-3).as_i128(), Some(-3));
        assert_eq!(Value::F32(1.5).as_f64(), Some(1.5));
    }
}
