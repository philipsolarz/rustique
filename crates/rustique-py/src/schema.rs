//! Element schemas: compiled descriptions of Python type expressions (ARCHITECTURE §3.4),
//! their cache, and the `List[int]`-style alias objects (§3.5).
//!
//! OWNER: schema agent. Interface items marked `INTERFACE` are used by other modules and
//! must keep their signatures.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyType;
use rustique_core::{DType, Value};

/// INTERFACE. A compiled element type.
#[derive(Debug)]
pub enum Schema {
    /// No validation; untyped containers use adaptive storage strategies.
    Any,
    /// Python `int`: stored as i64, demoting to object storage on overflow.
    Int,
    /// Python `float` (ints are converted).
    Float,
    /// Python `bool`.
    Bool,
    /// Python `str`.
    Str,
    /// Rust fixed-width type (`rs.i8`, …, `rs.f64`).
    Fixed(DType),
    /// `T | None`.
    Optional(Arc<Schema>),
    /// `A | B | …` (members in declaration order).
    Union(Vec<Arc<Schema>>),
    /// Nested rustique `List[T]`.
    List(Arc<Schema>),
    /// Nested rustique `Dict[K, V]`.
    Dict(Arc<Schema>, Arc<Schema>),
    /// Nested rustique `Set[T]`.
    Set(Arc<Schema>),
    /// Any other class: `isinstance` check.
    Class(Py<PyType>),
}

impl PartialEq for Schema {
    fn eq(&self, other: &Self) -> bool {
        use Schema::*;
        match (self, other) {
            (Any, Any) | (Int, Int) | (Float, Float) | (Bool, Bool) | (Str, Str) => true,
            (Fixed(a), Fixed(b)) => a == b,
            (Optional(a), Optional(b)) | (List(a), List(b)) | (Set(a), Set(b)) => a == b,
            (Union(a), Union(b)) => a == b,
            (Dict(k1, v1), Dict(k2, v2)) => k1 == k2 && v1 == v2,
            (Class(a), Class(b)) => a.as_ptr() == b.as_ptr(),
            _ => false,
        }
    }
}

impl Eq for Schema {}

impl Hash for Schema {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Schema::Fixed(d) => d.hash(state),
            Schema::Optional(s) | Schema::List(s) | Schema::Set(s) => s.hash(state),
            Schema::Union(members) => members.hash(state),
            Schema::Dict(k, v) => {
                k.hash(state);
                v.hash(state);
            }
            Schema::Class(c) => (c.as_ptr() as usize).hash(state),
            Schema::Any | Schema::Int | Schema::Float | Schema::Bool | Schema::Str => {}
        }
    }
}

impl Schema {
    /// INTERFACE. The native dtype a container with this schema stores, if any.
    /// `Int` prefers `I64` (and may demote to object storage); `Any` is adaptive (`None`).
    pub fn preferred_dtype(&self) -> Option<DType> {
        match self {
            Schema::Int => Some(DType::I64),
            Schema::Float => Some(DType::F64),
            Schema::Bool => Some(DType::Bool),
            Schema::Str => Some(DType::Str),
            Schema::Fixed(d) => Some(*d),
            _ => None,
        }
    }

    pub fn is_any(&self) -> bool {
        matches!(self, Schema::Any)
    }
}

/// Python-flavoured rendering: `int`, `i8`, `int | None`, `List[str]`, `Dict[str, float]`.
impl fmt::Display for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Schema::Any => f.write_str("Any"),
            Schema::Int => f.write_str("int"),
            Schema::Float => f.write_str("float"),
            Schema::Bool => f.write_str("bool"),
            Schema::Str => f.write_str("str"),
            Schema::Fixed(d) => f.write_str(d.name()),
            Schema::Optional(s) => write!(f, "{s} | None"),
            Schema::Union(members) => {
                for (i, m) in members.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    write!(f, "{m}")?;
                }
                Ok(())
            }
            Schema::List(s) => write!(f, "List[{s}]"),
            Schema::Dict(k, v) => write!(f, "Dict[{k}, {v}]"),
            Schema::Set(s) => write!(f, "Set[{s}]"),
            Schema::Class(c) => Python::attach(|py| {
                let name = c.bind(py).qualname().map(|n| n.to_string()).unwrap_or_else(|_| "?".into());
                f.write_str(&name)
            }),
        }
    }
}

/// INTERFACE. Result of validating/converting one Python object against a schema.
#[derive(Debug)]
pub enum Coerced {
    /// Store natively. For typed schemas the value's dtype equals `preferred_dtype()`;
    /// for `Any` it is the exact-type classification (`int`→I64, `float`→F64, `bool`→Bool,
    /// `str`→Str).
    Native(Value),
    /// Store as a Python object (already validated / converted, e.g. a nested `List`).
    Object(Py<PyAny>),
}

/// INTERFACE. Why a value was rejected.
#[derive(Debug)]
pub enum CoerceError {
    /// Wrong type → `TypeError("{container} expects {expected}, got {got}[ at index i]")`.
    Type { expected: String, got: String },
    /// Out of range for a fixed-width type → `OverflowError`.
    Overflow { message: String },
    /// A Python exception raised while converting (e.g. inside a nested container).
    Python(PyErr),
}

impl CoerceError {
    /// INTERFACE. Convert to a Python exception. `container` is e.g. `"List[int]"`.
    pub fn into_pyerr(self, container: &str, index: Option<usize>) -> PyErr {
        let _ = (container, index);
        todo!("schema agent")
    }
}

impl From<PyErr> for CoerceError {
    fn from(e: PyErr) -> Self {
        CoerceError::Python(e)
    }
}

impl Schema {
    /// INTERFACE. Validate and convert one element.
    pub fn coerce(&self, obj: &Bound<'_, PyAny>) -> Result<Coerced, CoerceError> {
        let _ = obj;
        todo!("schema agent")
    }
}

/// INTERFACE. Compile a Python type expression into an interned schema
/// (equal schemas share one `Arc`).
pub fn compile(ty: &Bound<'_, PyAny>) -> PyResult<Arc<Schema>> {
    let _ = ty;
    todo!("schema agent")
}

/// INTERFACE. The shared `Any` schema.
pub fn any() -> Arc<Schema> {
    todo!("schema agent")
}

/// INTERFACE. `List[item]`: the cached alias object for the element type expression `item`.
pub fn list_alias<'py>(py: Python<'py>, item: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, item);
    todo!("schema agent")
}

/// INTERFACE. `Dict[key, value]` alias (`params` is the subscript, normally a 2-tuple).
pub fn dict_alias<'py>(py: Python<'py>, params: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, params);
    todo!("schema agent")
}

/// INTERFACE. `Set[item]` alias.
pub fn set_alias<'py>(py: Python<'py>, item: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, item);
    todo!("schema agent")
}

/// INTERFACE. The alias object describing an existing schema (used by `schema_type`).
/// For `Any` returns the bare class (`List`, `Dict`, `Set`).
pub fn list_alias_for<'py>(py: Python<'py>, schema: &Arc<Schema>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, schema);
    todo!("schema agent")
}

pub fn dict_alias_for<'py>(py: Python<'py>, key: &Arc<Schema>, value: &Arc<Schema>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, key, value);
    todo!("schema agent")
}

pub fn set_alias_for<'py>(py: Python<'py>, schema: &Arc<Schema>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, schema);
    todo!("schema agent")
}

pub fn register(_m: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}
