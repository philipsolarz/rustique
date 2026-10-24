//! Element schemas (ARCHITECTURE §3.4): the compiled `Schema` type and per-element
//! validation/conversion (`coerce`). Compiling Python type expressions and the alias
//! objects live in `typing.rs`.
//!
//! OWNER: list agent. Interface items marked `INTERFACE` are used by other modules and
//! must keep their signatures.

use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use pyo3::exceptions::{PyOverflowError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyType;
use pyo3::{ffi, intern};
use rustique_core::{DType, Value};

use crate::convert;
use crate::dict::Dict;
use crate::list::List;
use crate::set::Set;

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
                let ty = c.bind(py);
                if ty.is(py.get_type::<pyo3::types::PyNone>()) {
                    return f.write_str("None");
                }
                let name = ty.qualname().map(|n| n.to_string()).unwrap_or_else(|_| "?".into());
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
        let at = match index {
            Some(i) => format!(" at index {i}"),
            None => String::new(),
        };
        match self {
            CoerceError::Type { expected, got } => {
                PyTypeError::new_err(format!("{container} expects {expected}, got {got}{at}"))
            }
            CoerceError::Overflow { message } => PyOverflowError::new_err(format!("{container}: {message}{at}")),
            CoerceError::Python(e) => e,
        }
    }
}

impl From<PyErr> for CoerceError {
    fn from(e: PyErr) -> Self {
        CoerceError::Python(e)
    }
}

impl Schema {
    /// INTERFACE. Validate and convert one element.
    ///
    /// Typed schemas never run user code for scalar elements; nested containers
    /// (`List[T]`, `Dict[K, V]`, `Set[T]`) and `Class` checks may (iteration,
    /// `__instancecheck__`), so callers must not hold a Rustique lock.
    pub fn coerce(&self, obj: &Bound<'_, PyAny>) -> Result<Coerced, CoerceError> {
        match self {
            Schema::Any => Ok(convert::classify(obj)),
            Schema::Int => coerce_int(obj),
            Schema::Float => coerce_float(obj).map(|x| Coerced::Native(Value::F64(x))),
            Schema::Bool => match convert::as_bool(obj) {
                Some(b) => Ok(Coerced::Native(Value::Bool(b))),
                None => Err(type_error("bool", obj)),
            },
            Schema::Str => coerce_str(obj),
            Schema::Fixed(dtype) => coerce_fixed(*dtype, obj),
            Schema::Optional(inner) => {
                if obj.is_none() {
                    return Ok(Coerced::Object(obj.clone().unbind()));
                }
                inner.coerce(obj).map_err(|e| match e {
                    CoerceError::Type { got, .. } => CoerceError::Type { expected: self.to_string(), got },
                    other => other,
                })
            }
            Schema::Union(members) => self.coerce_union(members, obj),
            Schema::List(item) => coerce_list(item, obj, self),
            Schema::Dict(key, value) => coerce_dict(key, value, obj, self),
            Schema::Set(item) => coerce_set(item, obj, self),
            Schema::Class(cls) => {
                if obj.is_instance(cls.bind(obj.py()))? {
                    Ok(Coerced::Object(obj.clone().unbind()))
                } else {
                    Err(type_error(&self.to_string(), obj))
                }
            }
        }
    }

    /// True when [`Schema::coerce`] provably runs no Python code and never blocks, so it may
    /// be applied to *borrowed* items inside a critical section of the source container:
    /// it only reads the internals of `int`/`float`/`bool`/`str` objects (subclasses
    /// included, without calling any of their methods; no `__index__`), compares types by
    /// pointer, and formats error messages natively (values from the extracted number,
    /// type names from the type object). Every other schema may call
    /// `__instancecheck__`, iterate or build nested containers.
    pub fn is_pure(&self) -> bool {
        matches!(self, Schema::Any | Schema::Int | Schema::Float | Schema::Bool | Schema::Str | Schema::Fixed(_))
    }

    fn coerce_union(&self, members: &[Arc<Schema>], obj: &Bound<'_, PyAny>) -> Result<Coerced, CoerceError> {
        let mut first_failure: Option<CoerceError> = None;
        for member in members {
            match member.coerce(obj) {
                Ok(c) => return Ok(c),
                Err(CoerceError::Type { .. }) => {}
                Err(CoerceError::Python(e))
                    if !(e.is_instance_of::<PyTypeError>(obj.py())
                        || e.is_instance_of::<PyValueError>(obj.py())
                        || e.is_instance_of::<PyOverflowError>(obj.py())) =>
                {
                    return Err(CoerceError::Python(e));
                }
                Err(other) => {
                    first_failure.get_or_insert(other);
                }
            }
        }
        Err(first_failure.unwrap_or_else(|| type_error(&self.to_string(), obj)))
    }
}

fn type_error(expected: &str, obj: &Bound<'_, PyAny>) -> CoerceError {
    CoerceError::Type { expected: expected.to_owned(), got: convert::type_name(obj) }
}

fn coerce_int(obj: &Bound<'_, PyAny>) -> Result<Coerced, CoerceError> {
    // SAFETY: `obj` is a valid object; the checks only read its type.
    if unsafe { ffi::PyLong_Check(obj.as_ptr()) } == 0 {
        return Err(type_error("int", obj));
    }
    Ok(match convert::long_as_i64(obj)? {
        Some(v) => Coerced::Native(Value::I64(v)),
        None => Coerced::Object(obj.clone().unbind()),
    })
}

/// `float` (incl. subclasses) or `int` converted like `float(x)`.
fn coerce_float(obj: &Bound<'_, PyAny>) -> Result<f64, CoerceError> {
    let p = obj.as_ptr();
    // SAFETY: type checks and value reads on a valid object of the checked type.
    unsafe {
        if ffi::PyFloat_CheckExact(p) != 0 {
            return Ok(ffi::PyFloat_AS_DOUBLE(p));
        }
        if ffi::PyFloat_Check(p) != 0 {
            let x = ffi::PyFloat_AsDouble(p);
            if x == -1.0
                && let Some(e) = PyErr::take(obj.py())
            {
                return Err(CoerceError::Python(e));
            }
            return Ok(x);
        }
        if ffi::PyLong_Check(p) != 0 {
            let x = ffi::PyLong_AsDouble(p);
            if x == -1.0
                && let Some(e) = PyErr::take(obj.py())
            {
                return Err(if e.is_instance_of::<PyOverflowError>(obj.py()) {
                    CoerceError::Overflow { message: "int too large to convert to float".to_owned() }
                } else {
                    CoerceError::Python(e)
                });
            }
            return Ok(x);
        }
    }
    Err(type_error("float", obj))
}

fn coerce_str(obj: &Bound<'_, PyAny>) -> Result<Coerced, CoerceError> {
    match convert::as_str(obj) {
        convert::StrValue::Native(s) => Ok(Coerced::Native(Value::Str(s))),
        // Lone surrogates have no UTF-8 form: keep the original object, which demotes the
        // container to object storage (still validated as `str`), like `Int` on overflow.
        convert::StrValue::Surrogates => Ok(Coerced::Object(obj.clone().unbind())),
        convert::StrValue::NotStr => Err(type_error("str", obj)),
    }
}

fn coerce_fixed(dtype: DType, obj: &Bound<'_, PyAny>) -> Result<Coerced, CoerceError> {
    match dtype {
        DType::Bool => return Schema::Bool.coerce(obj),
        DType::Str => return coerce_str(obj),
        _ => {}
    }
    let p = obj.as_ptr();
    // Builtin types first: they are the common case and need no scalar-class lookup.
    // SAFETY: type checks on a valid object.
    let (is_int, is_float) = unsafe { (ffi::PyLong_Check(p) != 0, ffi::PyFloat_Check(p) != 0) };
    // Error messages are formatted natively, never with `str()`/`repr()` (an int or float
    // subclass may override them): see `Schema::is_pure`.
    if is_int && dtype.is_integer() {
        let wide = convert::long_as_i128(obj)?;
        return match wide.and_then(|x| Value::int_of(dtype, x)) {
            Some(v) => Ok(Coerced::Native(v)),
            // Values beyond `i64::MIN..=u64::MAX` are not formatted: through Python that
            // could even import `_pylong`.
            None => Err(out_of_range(wide.map(|x| x.to_string()), dtype)),
        };
    }
    if (is_int || is_float) && dtype.is_float() {
        let x = coerce_float(obj)?;
        return match dtype {
            DType::F32 => convert::f64_to_f32(x)
                .map(|v| Coerced::Native(Value::F32(v)))
                .ok_or_else(|| out_of_range(Some(rustique_core::pyfmt::float_repr(x)), dtype)),
            _ => Ok(Coerced::Native(Value::F64(x))),
        };
    }
    if !is_int
        && !is_float
        && let Some(v) = crate::scalars::scalar_value(obj)
        && v.dtype() == dtype
    {
        return Ok(Coerced::Native(v));
    }
    Err(type_error(dtype.name(), obj))
}

fn out_of_range(value: Option<String>, dtype: DType) -> CoerceError {
    let message = match value {
        Some(value) => format!("{value} is out of range for {dtype}"),
        None => format!("int too large to convert to {dtype}"),
    };
    CoerceError::Overflow { message }
}

fn coerce_list(item: &Arc<Schema>, obj: &Bound<'_, PyAny>, schema: &Schema) -> Result<Coerced, CoerceError> {
    if let Ok(list) = obj.cast::<List>()
        && Arc::ptr_eq(list.get().schema(), item)
    {
        return Ok(Coerced::Object(obj.clone().unbind()));
    }
    if !convert::is_iterable(obj) {
        return Err(type_error(&schema.to_string(), obj));
    }
    let list = List::from_iterable(obj.py(), item.clone(), Some(obj))?;
    Ok(Coerced::Object(list.into_any().unbind()))
}

fn coerce_dict(
    key: &Arc<Schema>,
    value: &Arc<Schema>,
    obj: &Bound<'_, PyAny>,
    schema: &Schema,
) -> Result<Coerced, CoerceError> {
    if let Ok(dict) = obj.cast::<Dict>() {
        let d = dict.get();
        if Arc::ptr_eq(d.key_schema(), key) && Arc::ptr_eq(d.value_schema(), value) {
            return Ok(Coerced::Object(obj.clone().unbind()));
        }
    }
    // A mapping (anything with `keys`) or an iterable of pairs, like `dict(x)`.
    if !convert::is_iterable(obj) && !obj.hasattr(intern!(obj.py(), "keys"))? {
        return Err(type_error(&schema.to_string(), obj));
    }
    let dict = Dict::from_args(obj.py(), key.clone(), value.clone(), Some(obj), None)?;
    Ok(Coerced::Object(dict.into_any().unbind()))
}

fn coerce_set(item: &Arc<Schema>, obj: &Bound<'_, PyAny>, schema: &Schema) -> Result<Coerced, CoerceError> {
    if let Ok(set) = obj.cast::<Set>()
        && Arc::ptr_eq(set.get().schema(), item)
    {
        return Ok(Coerced::Object(obj.clone().unbind()));
    }
    if !convert::is_iterable(obj) {
        return Err(type_error(&schema.to_string(), obj));
    }
    let set = Set::from_iterable(obj.py(), item.clone(), Some(obj))?;
    Ok(Coerced::Object(set.into_any().unbind()))
}

/// INTERFACE. The shared `Any` schema.
pub fn any() -> Arc<Schema> {
    static ANY: OnceLock<Arc<Schema>> = OnceLock::new();
    ANY.get_or_init(|| intern(Schema::Any)).clone()
}

/// INTERFACE. Intern a schema: equal schemas share one `Arc` (so `Arc::ptr_eq` is a fast
/// equality check and aliases can be cached per schema). Interned schemas live for the
/// rest of the process, like the classes they describe.
pub fn intern(schema: Schema) -> Arc<Schema> {
    static INTERNER: Mutex<Option<HashSet<Arc<Schema>>>> = Mutex::new(None);
    let mut guard = INTERNER.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let set = guard.get_or_insert_with(HashSet::new);
    if let Some(existing) = set.get(&schema) {
        return existing.clone();
    }
    let interned = Arc::new(schema);
    set.insert(interned.clone());
    interned
}
