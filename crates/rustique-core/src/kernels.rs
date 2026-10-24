//! Bulk operations on columns, with Python semantics.
//!
//! OWNER: kernels agent. The signatures below are the INTERFACE used by the bindings.

use std::fmt;

use crate::column::{Column, Value};
use crate::dtype::DType;

/// Result of [`sum`]: integer columns (and bool) sum exactly into `i128`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sum {
    Int(i128),
    Float(f64),
}

/// Ordering operator for [`lex_compare`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelError {
    /// The operation is not defined for this dtype (e.g. `sum` of strings).
    Unsupported { op: &'static str, dtype: DType },
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KernelError::Unsupported { op, dtype } => write!(f, "{op} is not supported for {dtype} data"),
        }
    }
}

impl std::error::Error for KernelError {}

/// `sum(column)`; bool counts as int. Strings are unsupported.
pub fn sum(column: &Column) -> Result<Sum, KernelError> {
    let _ = column;
    todo!("kernels agent")
}

/// Python `min()` semantics (first minimal element by `<`; `None` when empty).
pub fn min(column: &Column) -> Option<Value> {
    let _ = column;
    todo!("kernels agent")
}

/// Python `max()` semantics.
pub fn max(column: &Column) -> Option<Value> {
    let _ = column;
    todo!("kernels agent")
}

/// Stable in-place sort (`list.sort(reverse=...)` semantics; NaNs sort last and never panic).
pub fn sort(column: &mut Column, reverse: bool) {
    let _ = (column, reverse);
    todo!("kernels agent")
}

/// Stable argsort with the same ordering as [`sort`].
pub fn argsort(column: &Column, reverse: bool) -> Vec<usize> {
    let _ = (column, reverse);
    todo!("kernels agent")
}

/// First position in `start..stop` whose element equals `needle`
/// (numeric equality across dtypes is exact, like Python; NaN equals NaN).
pub fn find(column: &Column, needle: &Value, start: usize, stop: usize) -> Option<usize> {
    let _ = (column, needle, start, stop);
    todo!("kernels agent")
}

/// Number of elements equal to `needle` (same equality as [`find`]).
pub fn count(column: &Column, needle: &Value) -> usize {
    let _ = (column, needle);
    todo!("kernels agent")
}

/// Element-wise equality of two columns of any dtypes (lengths must match for `true`).
pub fn equal(a: &Column, b: &Column) -> bool {
    let _ = (a, b);
    todo!("kernels agent")
}

/// Python list ordering: first differing position decides via `op`; otherwise lengths.
pub fn lex_compare(a: &Column, b: &Column, op: CmpOp) -> bool {
    let _ = (a, b, op);
    todo!("kernels agent")
}
