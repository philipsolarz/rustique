//! # rustique-core
//!
//! The Python-independent heart of Rustique: typed native storage ([`Column`]), Python
//! slice semantics ([`slice`]), bulk kernels ([`kernels`]), the vectorised expression
//! engine ([`expr`]), a CPython-style compact hash index ([`index`]) and CPython-compatible
//! hashing/formatting of numbers ([`pyhash`], [`pyfmt`]).
//!
//! Nothing in this crate links against Python; the `rustique-py` crate adds the bindings.

pub mod column;
pub mod dtype;
pub mod expr;
pub mod index;
pub mod kernels;
pub mod pyfmt;
pub mod pyhash;
pub mod slice;

pub use column::{Column, ColumnError, Element, Value};
pub use compact_str::CompactString;
pub use dtype::DType;
pub use slice::{SliceError, SliceIndices};
