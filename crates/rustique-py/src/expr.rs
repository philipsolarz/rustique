//! Python-facing expression builder `X` and map/filter evaluation (ARCHITECTURE §4.5).
//!
//! OWNER: expr agent. Items marked `INTERFACE` are used by other modules.
//!
//! `Expr` wraps an immutable `rustique_core::expr::Expr`. Native storage is evaluated by the
//! core engine (detached from the interpreter for large columns). Object storage, results
//! the engine cannot represent natively (Python ints beyond i64, complex powers, …) and
//! plain Python callables are evaluated element by element with Python's own operators —
//! the same semantics, just slower.

use std::sync::Arc;

use pyo3::basic::CompareOp;
use pyo3::exceptions::{
    PyOverflowError, PyRecursionError, PySystemError, PyTypeError, PyValueError, PyZeroDivisionError,
};
use pyo3::prelude::*;
use pyo3::PyTypeInfo;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBool, PyFloat, PyInt, PyString, PyTuple, PyType};
use rustique_core::expr::{self as engine, BinaryOp, ErrorKind, ExprError, ExprType, IntMode, Node, UnaryOp};
use rustique_core::{Column, DType, Value};

use crate::convert;
use crate::schema::{self, Schema};
use crate::storage::Storage;

/// Columns at least this long are evaluated with the interpreter detached.
const DETACH_MIN: usize = 4096;

/// Native evaluations of at least this many node-elements also run detached.
const DETACH_WORK: u64 = 1 << 20;

/// An expression over the element placeholder `X`, e.g. `X * 2 + 1` or `(X % 3 == 0)`.
///
/// `List.map` / `List.filter` evaluate it natively over the whole list; calling it,
/// `expr(value)`, evaluates it for one value with Python's operators.
#[pyclass(module = "rustique", name = "Expr", frozen)]
pub struct Expr {
    inner: Arc<engine::Expr>,
}

impl Expr {
    /// INTERFACE. The core expression tree.
    pub fn inner(&self) -> &Arc<engine::Expr> {
        &self.inner
    }

    fn wrap(py: Python<'_>, inner: Arc<engine::Expr>) -> PyResult<Py<PyAny>> {
        if let Some(error) = inner.limit_error() {
            return Err(expr_error(py, error));
        }
        Ok(Bound::new(py, Expr { inner })?.into_any().unbind())
    }

    fn binary(&self, py: Python<'_>, other: &Bound<'_, PyAny>, op: BinaryOp, reflected: bool) -> PyResult<Py<PyAny>> {
        let Some(other) = operand(other)? else {
            return Ok(py.NotImplemented());
        };
        let (lhs, rhs) = if reflected { (other, self.inner.clone()) } else { (self.inner.clone(), other) };
        Expr::wrap(py, engine::Expr::binary(op, lhs, rhs))
    }

    fn unary(&self, py: Python<'_>, op: UnaryOp) -> PyResult<Py<PyAny>> {
        Expr::wrap(py, engine::Expr::unary(op, self.inner.clone()))
    }
}

/// An operand of an operator: another `Expr`, or a Python `bool`/`int`/`float`/`str`
/// constant. Anything else is `None` (→ `NotImplemented`).
fn operand(obj: &Bound<'_, PyAny>) -> PyResult<Option<Arc<engine::Expr>>> {
    if let Ok(e) = obj.cast::<Expr>() {
        return Ok(Some(e.get().inner.clone()));
    }
    let value = if let Ok(b) = obj.cast::<PyBool>() {
        Value::Bool(b.is_true())
    } else if obj.is_instance_of::<PyInt>() {
        if let Ok(v) = obj.extract::<i64>() {
            Value::I64(v)
        } else if let Ok(v) = obj.extract::<u64>() {
            Value::U64(v)
        } else {
            return Err(PyOverflowError::new_err("expression constants must fit in 64 bits"));
        }
    } else if let Ok(f) = obj.cast::<PyFloat>() {
        Value::F64(f.value())
    } else if let Ok(s) = obj.cast::<PyString>() {
        Value::from(s.to_str()?)
    } else {
        return Ok(None);
    };
    Ok(Some(engine::Expr::constant(value)))
}

#[pymethods]
impl Expr {
    fn __add__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::Add, false)
    }

    fn __radd__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::Add, true)
    }

    fn __sub__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::Sub, false)
    }

    fn __rsub__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::Sub, true)
    }

    fn __mul__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::Mul, false)
    }

    fn __rmul__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::Mul, true)
    }

    fn __truediv__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::TrueDiv, false)
    }

    fn __rtruediv__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::TrueDiv, true)
    }

    fn __floordiv__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::FloorDiv, false)
    }

    fn __rfloordiv__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::FloorDiv, true)
    }

    fn __mod__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::Mod, false)
    }

    fn __rmod__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::Mod, true)
    }

    fn __lshift__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::LShift, false)
    }

    fn __rlshift__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::LShift, true)
    }

    fn __rshift__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::RShift, false)
    }

    fn __rrshift__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::RShift, true)
    }

    fn __and__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::BitAnd, false)
    }

    fn __rand__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::BitAnd, true)
    }

    fn __or__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::BitOr, false)
    }

    fn __ror__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::BitOr, true)
    }

    fn __xor__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::BitXor, false)
    }

    fn __rxor__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, other, BinaryOp::BitXor, true)
    }

    fn __pow__(&self, py: Python<'_>, other: &Bound<'_, PyAny>, modulo: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if !modulo.is_none() {
            return Ok(py.NotImplemented());
        }
        self.binary(py, other, BinaryOp::Pow, false)
    }

    fn __rpow__(&self, py: Python<'_>, other: &Bound<'_, PyAny>, modulo: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if !modulo.is_none() {
            return Ok(py.NotImplemented());
        }
        self.binary(py, other, BinaryOp::Pow, true)
    }

    fn __richcmp__(&self, py: Python<'_>, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<Py<PyAny>> {
        let op = match op {
            CompareOp::Eq => BinaryOp::Eq,
            CompareOp::Ne => BinaryOp::Ne,
            CompareOp::Lt => BinaryOp::Lt,
            CompareOp::Le => BinaryOp::Le,
            CompareOp::Gt => BinaryOp::Gt,
            CompareOp::Ge => BinaryOp::Ge,
        };
        self.binary(py, other, op, false)
    }

    fn __neg__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.unary(py, UnaryOp::Neg)
    }

    fn __pos__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.unary(py, UnaryOp::Pos)
    }

    fn __abs__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.unary(py, UnaryOp::Abs)
    }

    /// `~x`: logical `not` for `bool` values, bitwise inversion for integers.
    fn __invert__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.unary(py, UnaryOp::Invert)
    }

    /// Whether the value is a float NaN (`False` for integers).
    fn is_nan(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.unary(py, UnaryOp::IsNan)
    }

    /// Convert to `int`, `float`, `bool` or a rustique scalar type (`rs.i8` … `rs.f64`):
    /// `int(x)` / `float(x)` / `bool(x)` semantics; fixed-width targets truncate floats
    /// and raise `OverflowError` when the value does not fit.
    fn cast(&self, py: Python<'_>, dtype: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let target = cast_target(dtype)?;
        Expr::wrap(py, engine::Expr::cast(target, self.inner.clone()))
    }

    /// Evaluate for one value with Python's operators.
    #[pyo3(signature = (value, /))]
    fn __call__<'py>(&self, value: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        call(&self.inner, value)
    }

    fn __bool__(&self) -> PyResult<bool> {
        Err(PyTypeError::new_err(
            "an Expr has no truth value: combine conditions with & | ~ instead of and/or/not",
        ))
    }

    fn __hash__(&self) -> PyResult<isize> {
        Err(PyTypeError::new_err("unhashable type: 'Expr' (== builds an expression)"))
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        self.inner.render(&mut |v| {
            convert::value_to_python(py, v).repr().map_or_else(|_| "?".to_owned(), |r| r.to_string())
        })
    }

    fn __copy__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    fn __deepcopy__<'py>(slf: Bound<'py, Self>, _memo: &Bound<'py, PyAny>) -> Bound<'py, Self> {
        slf
    }
}

fn cast_target(dtype: &Bound<'_, PyAny>) -> PyResult<ExprType> {
    let py = dtype.py();
    if let Ok(ty) = dtype.cast::<PyType>() {
        if ty.is(PyBool::type_object(py)) {
            return Ok(ExprType::Bool);
        }
        if ty.is(PyInt::type_object(py)) {
            return Ok(ExprType::Int);
        }
        if ty.is(PyFloat::type_object(py)) {
            return Ok(ExprType::Float);
        }
        if let Some(d) = crate::scalars::dtype_of_type(ty) {
            return Ok(ExprType::Fixed(d));
        }
    }
    Err(PyTypeError::new_err(format!(
        "cast() expects int, float, bool or a rustique scalar type, got {}",
        dtype.repr()?
    )))
}

/// The rustique scalar class of a fixed-width dtype (`rs.i8`, …).
fn scalar_class<'py>(py: Python<'py>, dtype: DType) -> PyResult<Bound<'py, PyAny>> {
    py.import("rustique._rustique")?.getattr(dtype.name())
}

fn math_isnan(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static ISNAN: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    ISNAN.import(py, "math", "isnan")
}

fn is_nan<'py>(value: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let py = value.py();
    let nan = if let Ok(f) = value.cast::<PyFloat>() {
        f.value().is_nan()
    } else if value.is_instance_of::<PyInt>() {
        false
    } else if value.is_instance_of::<PyString>() {
        return Err(PyTypeError::new_err("must be real number, not str"));
    } else {
        math_isnan(py)?.call1((value,))?.is_truthy()?
    };
    Ok(PyBool::new(py, nan).to_owned().into_any())
}

fn cast_value<'py>(to: ExprType, value: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let py = value.py();
    match to {
        ExprType::Bool => Ok(PyBool::new(py, value.is_truthy()?).to_owned().into_any()),
        ExprType::Int => PyInt::type_object(py).call1((value,)),
        ExprType::Float => PyFloat::type_object(py).call1((value,)),
        ExprType::Str => Err(PyTypeError::new_err("cannot cast to str")),
        ExprType::Fixed(d) => {
            if value.is_instance_of::<PyString>() {
                return Err(PyTypeError::new_err(format!("cannot cast str to {d}")));
            }
            let number = if d.is_float() {
                PyFloat::type_object(py).call1((value,))?
            } else {
                PyInt::type_object(py).call1((value,))?
            };
            scalar_class(py, d)?.call1((number,))
        }
    }
}

/// Evaluate `expr` for one value with Python's operators (left operand first). `~` on a
/// `bool` is `not`, matching the native engine.
fn call<'py>(expr: &engine::Expr, x: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let py = x.py();
    match expr.node() {
        Node::Element => Ok(x.clone()),
        Node::Const(v) => Ok(convert::value_to_python(py, v)),
        Node::Unary(op, a) => {
            let a = call(a, x)?;
            match op {
                UnaryOp::Neg => a.neg(),
                UnaryOp::Pos => a.pos(),
                UnaryOp::Abs => a.abs(),
                UnaryOp::Invert if a.is_exact_instance_of::<PyBool>() => {
                    Ok(PyBool::new(py, !a.is_truthy()?).to_owned().into_any())
                }
                UnaryOp::Invert => a.bitnot(),
                UnaryOp::IsNan => is_nan(&a),
            }
        }
        Node::Binary(op, l, r) => {
            let a = call(l, x)?;
            let b = call(r, x)?;
            match op {
                BinaryOp::Add => a.add(b),
                BinaryOp::Sub => a.sub(b),
                BinaryOp::Mul => a.mul(b),
                BinaryOp::TrueDiv => a.div(b),
                BinaryOp::FloorDiv => a.floor_div(b),
                BinaryOp::Mod => a.rem(b),
                BinaryOp::Pow => a.pow(b, py.None()),
                BinaryOp::LShift => a.lshift(b),
                BinaryOp::RShift => a.rshift(b),
                BinaryOp::BitAnd => a.bitand(b),
                BinaryOp::BitOr => a.bitor(b),
                BinaryOp::BitXor => a.bitxor(b),
                BinaryOp::Eq => a.rich_compare(b, CompareOp::Eq),
                BinaryOp::Ne => a.rich_compare(b, CompareOp::Ne),
                BinaryOp::Lt => a.rich_compare(b, CompareOp::Lt),
                BinaryOp::Le => a.rich_compare(b, CompareOp::Le),
                BinaryOp::Gt => a.rich_compare(b, CompareOp::Gt),
                BinaryOp::Ge => a.rich_compare(b, CompareOp::Ge),
            }
        }
        Node::Cast(to, a) => cast_value(*to, &call(a, x)?),
    }
}

fn expr_error(py: Python<'_>, error: ExprError) -> PyErr {
    let ExprError::Raise { kind, message } = error else {
        return PySystemError::new_err("expression fallback was not handled");
    };
    match kind {
        ErrorKind::TypeError => PyTypeError::new_err(message),
        ErrorKind::ValueError => PyValueError::new_err(message),
        ErrorKind::ZeroDivisionError => PyZeroDivisionError::new_err(message),
        ErrorKind::OverflowError => PyOverflowError::new_err(message),
        ErrorKind::RangeError => range_error(py).unwrap_or_else(|e| e),
        ErrorKind::RecursionError => PyRecursionError::new_err(message),
        ErrorKind::SystemError => PySystemError::new_err(message),
    }
}

/// `OverflowError(errno.ERANGE, os.strerror(errno.ERANGE))`, exactly as float `**` raises it.
fn range_error(py: Python<'_>) -> PyResult<PyErr> {
    let erange = py.import("errno")?.getattr("ERANGE")?;
    let text = py.import("os")?.getattr("strerror")?.call1((&erange,))?;
    Ok(PyOverflowError::new_err(PyTuple::new(py, [erange, text])?.unbind()))
}

/// How the engine types a native column of a container with `schema`.
fn int_mode(schema: &Schema) -> IntMode {
    if let Schema::Fixed(_) = schema { IntMode::Fixed } else { IntMode::Python }
}

/// The element type of a typed schema, if the engine knows it.
fn schema_type(schema: &Schema) -> Option<ExprType> {
    Some(match schema {
        Schema::Int => ExprType::Int,
        Schema::Float => ExprType::Float,
        Schema::Bool => ExprType::Bool,
        Schema::Str => ExprType::Str,
        Schema::Fixed(d) => ExprType::Fixed(*d),
        _ => return None,
    })
}

/// The schema of a map result of type `ty`: typed inputs give typed results, untyped
/// inputs stay untyped (so strategies adapt) unless the result is fixed-width.
fn result_schema(input: &Schema, ty: ExprType) -> Arc<Schema> {
    match ty {
        ExprType::Fixed(d) => schema::intern(Schema::Fixed(d)),
        _ if input.is_any() => schema::any(),
        ExprType::Int => schema::intern(Schema::Int),
        ExprType::Float => schema::intern(Schema::Float),
        ExprType::Bool => schema::intern(Schema::Bool),
        ExprType::Str => schema::intern(Schema::Str),
    }
}

/// Whether `obj` is exactly a value of the Python type `schema` stands for.
fn fits_exactly(schema: &Schema, obj: &Bound<'_, PyAny>) -> bool {
    match schema {
        Schema::Int => obj.is_exact_instance_of::<PyInt>(),
        Schema::Float => obj.is_exact_instance_of::<PyFloat>(),
        Schema::Bool => obj.is_exact_instance_of::<PyBool>(),
        Schema::Str => obj.is_exact_instance_of::<PyString>(),
        Schema::Fixed(d) => crate::scalars::scalar_value(obj).is_some_and(|v| v.dtype() == *d),
        _ => false,
    }
}

/// Storage holding `values`: with `preferred` when every value has exactly its type,
/// otherwise untyped (adaptive strategies).
fn collect(py: Python<'_>, values: Vec<Py<PyAny>>, preferred: Option<Arc<Schema>>) -> PyResult<(Storage, Arc<Schema>)> {
    let schema = preferred
        .filter(|s| values.iter().all(|v| fits_exactly(s, v.bind(py))))
        .unwrap_or_else(schema::any);
    let mut storage = Storage::for_schema(&schema);
    for (i, value) in values.iter().enumerate() {
        let value = value.bind(py);
        if schema.is_any() {
            storage.push_any(py, value);
        } else {
            let coerced = schema
                .coerce(value)
                .map_err(|e| e.into_pyerr(&convert::container_name(&schema), Some(i)))?;
            storage.push(py, coerced);
        }
    }
    Ok((storage, schema))
}

/// The elements as Python objects. With `scalars`, the native numbers of a fixed-width
/// schema become rustique scalars (so expression fallback keeps fixed-width semantics);
/// otherwise they are what element reads return.
fn elements<'py>(py: Python<'py>, storage: &Storage, schema: &Schema, scalars: bool) -> PyResult<Vec<Bound<'py, PyAny>>> {
    let scalars = scalars && matches!(schema, Schema::Fixed(_));
    match storage {
        Storage::Empty => Ok(Vec::new()),
        Storage::Object(objects) => Ok(objects.iter().map(|o| o.bind(py).clone()).collect()),
        Storage::Native(column) => {
            let class = match column.dtype() {
                d if scalars && d.is_numeric() => Some(scalar_class(py, d)?),
                _ => None,
            };
            column
                .iter()
                .map(|v| {
                    let obj = convert::value_to_python(py, &v);
                    match &class {
                        Some(class) => class.call1((obj,)),
                        None => Ok(obj),
                    }
                })
                .collect()
        }
    }
}

fn not_callable(f: &Bound<'_, PyAny>, method: &str) -> PyErr {
    let name = f.get_type().name().map(|n| n.to_string()).unwrap_or_else(|_| "?".into());
    PyTypeError::new_err(format!("{method}() expects an Expr or a callable, got '{name}'"))
}

/// Evaluate natively; `None` when the column is not native (or empty).
fn native(py: Python<'_>, expr: &engine::Expr, storage: &Storage, schema: &Schema) -> Option<Result<Column, ExprError>> {
    let Storage::Native(column) = storage else { return None };
    if column.is_empty() {
        return None;
    }
    let mode = int_mode(schema);
    let work = (column.len() as u64).saturating_mul(expr.size());
    Some(if column.len() >= DETACH_MIN || work >= DETACH_WORK {
        py.detach(|| engine::eval(expr, column, mode))
    } else {
        engine::eval(expr, column, mode)
    })
}

/// INTERFACE. `lst.map(f)`: evaluate `f` (an `X` expression or any Python callable) over
/// an owned snapshot of the list's storage. Returns the new storage and its schema.
/// May run Python code: callers must not hold a Rustique lock.
pub fn map_storage(
    py: Python<'_>,
    storage: Storage,
    schema: &Arc<Schema>,
    f: &Bound<'_, PyAny>,
) -> PyResult<(Storage, Arc<Schema>)> {
    let Ok(expr) = f.cast::<Expr>() else {
        if !f.is_callable() {
            return Err(not_callable(f, "map"));
        }
        let mut results = Vec::with_capacity(storage.len());
        for x in elements(py, &storage, schema, false)? {
            results.push(f.call1((x,))?.unbind());
        }
        return collect(py, results, None);
    };
    let expr = expr.get().inner.clone();
    let input = match &storage {
        Storage::Native(column) => Some(ExprType::of_column(column.dtype(), int_mode(schema))),
        _ => schema_type(schema),
    };
    let preferred = input.and_then(|t| expr.result_type(t).ok()).map(|t| result_schema(schema, t));
    match native(py, &expr, &storage, schema) {
        Some(Ok(column)) => {
            let ty = ExprType::of_column(column.dtype(), int_mode(schema));
            let schema = preferred.unwrap_or_else(|| result_schema(schema, ty));
            return Ok((Storage::Native(column), schema));
        }
        Some(Err(ExprError::Overflow)) | None => {}
        Some(Err(error)) => return Err(expr_error(py, error)),
    }
    let mut results = Vec::with_capacity(storage.len());
    for x in elements(py, &storage, schema, true)? {
        // Builtin operators never check for signals: keep long evaluations interruptible.
        py.check_signals()?;
        results.push(call(&expr, &x)?.unbind());
    }
    if results.is_empty() {
        let schema = preferred.unwrap_or_else(schema::any);
        return Ok((Storage::for_schema(&schema), schema));
    }
    collect(py, results, preferred)
}

/// INTERFACE. `lst.filter(f)`: keep elements for which `f` is truthy. Same rules as
/// [`map_storage`]; the schema is unchanged.
pub fn filter_storage(
    py: Python<'_>,
    storage: Storage,
    schema: &Arc<Schema>,
    f: &Bound<'_, PyAny>,
) -> PyResult<Storage> {
    let mask = match f.cast::<Expr>() {
        Ok(expr) => {
            let expr = expr.get().inner.clone();
            match native(py, &expr, &storage, schema) {
                Some(Ok(column)) => engine::into_truthy(column),
                Some(Err(error @ ExprError::Raise { .. })) => return Err(expr_error(py, error)),
                Some(Err(ExprError::Overflow)) | None => elements(py, &storage, schema, true)?
                    .iter()
                    .map(|x| {
                        py.check_signals()?;
                        call(&expr, x)?.is_truthy()
                    })
                    .collect::<PyResult<Vec<bool>>>()?,
            }
        }
        Err(_) if f.is_callable() => elements(py, &storage, schema, false)?
            .into_iter()
            .map(|x| f.call1((x,))?.is_truthy())
            .collect::<PyResult<Vec<bool>>>()?,
        Err(_) => return Err(not_callable(f, "filter")),
    };
    let kept = match storage {
        Storage::Empty => Storage::Empty,
        Storage::Native(column) => Storage::Native(engine::select(&column, &mask)),
        Storage::Object(objects) => {
            Storage::Object(objects.into_iter().zip(&mask).filter(|(_, keep)| **keep).map(|(o, _)| o).collect())
        }
    };
    Ok(if kept.is_empty() && schema.is_any() { Storage::Empty } else { kept })
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Expr>()?;
    m.add("X", Bound::new(m.py(), Expr { inner: engine::Expr::element() })?)
}
