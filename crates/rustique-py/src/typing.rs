//! Compiling Python type expressions into schemas, and the `List[int]`-style alias
//! objects (ARCHITECTURE §3.4–3.5).
//!
//! OWNER: typing agent. Items marked `INTERFACE` are used by other modules and must keep
//! their signatures.
//!
//! Caching: `List[x]` first looks `x` up in a per-kind dict (no `typing` calls on a hit);
//! `compile(x)` has its own dict keyed by the expression. Both only store expressions whose
//! Python equality implies an identical schema. Python considers `int | str == str | int`
//! while member order is significant here, so a top-level union is keyed by its ordered
//! `__args__`, and expressions with nested unions are not stored at all. Aliases are
//! cached per schema, so `List[int] is List[int]`.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, LazyLock, OnceLock};

use parking_lot::{Mutex, MutexGuard};
use pyo3::exceptions::{PyRecursionError, PyRuntimeError, PyTypeError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PySet, PyString, PyTuple, PyType};
use rustique_core::DType;

use crate::dict::Dict;
use crate::list::List;
use crate::schema::{Schema, any, intern};
use crate::set::Set;

/// Nesting limit for type expressions (guards the recursive compiler and schema walks).
const MAX_DEPTH: usize = 128;

/// Class names of the fixed-width scalar types (`rs.i8` …); only these are offered to
/// `scalars::dtype_of_type`.
const SCALAR_NAMES: [&str; 10] = [
    "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64",
];

// ---------------------------------------------------------------------------------------
// Compilation
// ---------------------------------------------------------------------------------------

/// INTERFACE. Compile a Python type expression into an interned schema
/// (equal schemas share one `Arc`).
pub fn compile(ty: &Bound<'_, PyAny>) -> PyResult<Arc<Schema>> {
    compile_cached(ty).map(|(schema, _)| schema)
}

/// The objects from `typing`/`types` the compiler dispatches on.
struct Refs {
    any: Py<PyAny>,
    union: Py<PyAny>,
    union_type: Py<PyAny>,
    /// `type(typing.Union[int, str])`: `typing._UnionGenericAlias` on 3.13, else `typing.Union`.
    union_alias: Py<PyAny>,
    /// Private marker for union cache keys.
    union_key: Py<PyAny>,
    annotated: Py<PyAny>,
    literal: Py<PyAny>,
    get_origin: Py<PyAny>,
    get_args: Py<PyAny>,
    none_type: Py<PyType>,
    type_alias_type: Py<PyType>,
    type_var: Py<PyType>,
    param_spec: Py<PyType>,
    type_var_tuple: Py<PyType>,
    new_type: Py<PyType>,
    forward_ref: Py<PyType>,
    type_eq: Py<PyAny>,
    type_hash: Py<PyAny>,
}

fn refs(py: Python<'_>) -> PyResult<&'static Refs> {
    static REFS: PyOnceLock<Refs> = PyOnceLock::new();
    once(&REFS, py, || {
        let typing = py.import("typing")?;
        let types = py.import("types")?;
        let class = |name: &str| -> PyResult<Py<PyType>> {
            Ok(typing.getattr(name)?.cast_into::<PyType>()?.unbind())
        };
        let type_type = py.get_type::<PyType>();
        Ok(Refs {
            any: typing.getattr("Any")?.unbind(),
            union: typing.getattr("Union")?.unbind(),
            union_type: types.getattr("UnionType")?.unbind(),
            union_alias: typing
                .getattr("Union")?
                .get_item((py.get_type::<PyInt>(), py.get_type::<PyString>()))?
                .get_type()
                .into_any()
                .unbind(),
            union_key: py.get_type::<PyAny>().call0()?.unbind(),
            annotated: typing.getattr("Annotated")?.unbind(),
            literal: typing.getattr("Literal")?.unbind(),
            get_origin: typing.getattr("get_origin")?.unbind(),
            get_args: typing.getattr("get_args")?.unbind(),
            none_type: py.None().bind(py).get_type().unbind(),
            type_alias_type: class("TypeAliasType")?,
            type_var: class("TypeVar")?,
            param_spec: class("ParamSpec")?,
            type_var_tuple: class("TypeVarTuple")?,
            new_type: class("NewType")?,
            forward_ref: class("ForwardRef")?,
            type_eq: type_type.getattr("__eq__")?.unbind(),
            type_hash: type_type.getattr("__hash__")?.unbind(),
        })
    })
}

/// Initialise a `PyOnceLock` without holding any lock while `init` runs Python code
/// (a racing initialiser may win; both values are equivalent).
fn once<'a, T>(
    cell: &'a PyOnceLock<T>,
    py: Python<'_>,
    init: impl FnOnce() -> PyResult<T>,
) -> PyResult<&'a T> {
    if let Some(value) = cell.get(py) {
        return Ok(value);
    }
    let value = init()?;
    let _ = cell.set(py, value);
    cell.get(py)
        .ok_or_else(|| PyRuntimeError::new_err("rustique: failed to initialise a typing cache"))
}

/// `#[pyclass]` wrapper so compiled schemas can live in a Python dict.
#[pyclass(frozen, name = "_CompiledSchema", module = "rustique._rustique")]
struct CompiledSchema(Arc<Schema>);

/// The cache key for a type expression: the expression itself, or `(marker, expr.__args__)`
/// for a union object (whose equality ignores member order; its ordered args determine it
/// completely). The flag tells which.
fn cache_key<'py>(ty: &Bound<'py, PyAny>, refs: &Refs) -> Option<(Bound<'py, PyAny>, bool)> {
    let py = ty.py();
    let class = ty.get_type();
    if !(class.is(&refs.union_type) || class.is(&refs.union_alias)) {
        return Some((ty.clone(), false));
    }
    let args = ty.getattr(intern!(py, "__args__")).ok()?;
    let key = PyTuple::new(py, [refs.union_key.bind(py).clone(), args]).ok()?;
    Some((key.into_any(), true))
}

/// Look `key` up in `cache`; on a miss run `build` and store its result if both the key
/// and `build` allow it (`build` reports whether its result may be cached under `key`).
fn cached<'py>(
    cache: &Bound<'py, PyDict>,
    key: Option<&Bound<'py, PyAny>>,
    build: impl FnOnce() -> PyResult<(Bound<'py, PyAny>, bool)>,
) -> PyResult<(Bound<'py, PyAny>, bool)> {
    let hashable = match key.map(|key| cache.get_item(key)) {
        Some(Ok(Some(hit))) => return Ok((hit, true)),
        Some(Ok(None)) => true,
        Some(Err(_)) | None => false,
    };
    let (value, storable) = build()?;
    let stored = hashable && storable && key.is_some_and(|key| cache.set_item(key, &value).is_ok());
    Ok((value, stored))
}

/// `compile` plus whether the result may be cached under `cache_key(ty)`.
fn compile_cached(ty: &Bound<'_, PyAny>) -> PyResult<(Arc<Schema>, bool)> {
    static CACHE: PyOnceLock<Py<PyDict>> = PyOnceLock::new();
    let py = ty.py();
    let refs = refs(py)?;
    if let Some(schema) = simple_schema(ty, refs)? {
        return Ok((schema, true));
    }
    let cache = once(&CACHE, py, || Ok(PyDict::new(py).unbind()))?.bind(py);
    let key = cache_key(ty, refs);
    let (compiled, storable) = cached(cache, key.as_ref().map(|(key, _)| key), || {
        let mut compiler = Compiler {
            py,
            refs,
            cacheable: true,
            top_union: false,
            depth: 0,
            expanding: Vec::new(),
            frames: Vec::new(),
        };
        let schema = compiler.compile(ty)?;
        let ordered_key = key.as_ref().is_some_and(|(_, ordered)| *ordered);
        let storable = compiler.cacheable && (!compiler.top_union || ordered_key);
        Ok((Bound::new(py, CompiledSchema(schema))?.into_any(), storable))
    })?;
    let schema = compiled.cast::<CompiledSchema>()?.get().0.clone();
    Ok((schema, storable))
}

macro_rules! static_schema {
    ($name:ident, $schema:expr) => {
        fn $name() -> Arc<Schema> {
            static CELL: OnceLock<Arc<Schema>> = OnceLock::new();
            CELL.get_or_init(|| intern($schema)).clone()
        }
    };
}

static_schema!(int_schema, Schema::Int);
static_schema!(float_schema, Schema::Float);
static_schema!(bool_schema, Schema::Bool);
static_schema!(str_schema, Schema::Str);

fn none_schema(py: Python<'_>, refs: &Refs) -> PyResult<Arc<Schema>> {
    static CELL: PyOnceLock<Arc<Schema>> = PyOnceLock::new();
    once(&CELL, py, || {
        Ok(intern(Schema::Class(refs.none_type.clone_ref(py))))
    })
    .cloned()
}

/// Schemas for the leaf expressions that need no `typing` machinery.
fn simple_schema(ty: &Bound<'_, PyAny>, refs: &Refs) -> PyResult<Option<Arc<Schema>>> {
    let py = ty.py();
    Ok(Some(if ty.is(py.get_type::<PyInt>()) {
        int_schema()
    } else if ty.is(py.get_type::<PyFloat>()) {
        float_schema()
    } else if ty.is(py.get_type::<PyBool>()) {
        bool_schema()
    } else if ty.is(py.get_type::<PyString>()) {
        str_schema()
    } else if ty.is(py.get_type::<PyAny>()) || ty.is(&refs.any) {
        any()
    } else if ty.is_none() || ty.is(&refs.none_type) {
        none_schema(py, refs)?
    } else {
        return Ok(None);
    }))
}

struct Compiler<'py> {
    py: Python<'py>,
    refs: &'static Refs,
    /// False once the expression contains something whose Python equality is coarser
    /// than schema equality (nested multi-member unions, classes with a custom metaclass
    /// `__eq__`). Unions inside PEP 695 alias values are fine: aliases compare by identity.
    cacheable: bool,
    /// The expression itself is a multi-member union (cacheable only under an ordered key).
    top_union: bool,
    depth: usize,
    /// PEP 695 aliases being expanded (recursion detection).
    expanding: Vec<Bound<'py, PyAny>>,
    /// Type-parameter bindings of the generic aliases being expanded (innermost last).
    frames: Vec<Vec<(Bound<'py, PyAny>, Arc<Schema>)>>,
}

/// Nesting depth of a schema. Every schema built by [`compile`] is at most `MAX_DEPTH` deep,
/// so this recursion is bounded.
fn schema_depth(schema: &Schema) -> usize {
    match schema {
        Schema::Optional(inner) | Schema::List(inner) | Schema::Set(inner) => 1 + schema_depth(inner),
        Schema::Dict(key, value) => 1 + schema_depth(key).max(schema_depth(value)),
        Schema::Union(members) => 1 + members.iter().map(|m| schema_depth(m)).max().unwrap_or(0),
        _ => 1,
    }
}

impl<'py> Compiler<'py> {
    fn compile(&mut self, ty: &Bound<'py, PyAny>) -> PyResult<Arc<Schema>> {
        if let Some(schema) = simple_schema(ty, self.refs)? {
            return Ok(schema);
        }
        if self.depth >= MAX_DEPTH {
            return Err(PyRecursionError::new_err(format!(
                "type expression nested more than {MAX_DEPTH} levels deep"
            )));
        }
        self.depth += 1;
        let result = self.compile_node(ty);
        self.depth -= 1;
        let schema = result?;
        // Aliases carry already-built schemas, so syntactic depth alone does not bound the
        // result (`T = List[T]` repeated); every schema stays within MAX_DEPTH so the
        // recursive walks over it (render, hash, display) cannot overflow the stack.
        if schema_depth(&schema) > MAX_DEPTH {
            return Err(PyRecursionError::new_err(format!(
                "type expression nested more than {MAX_DEPTH} levels deep"
            )));
        }
        Ok(schema)
    }

    fn compile_node(&mut self, ty: &Bound<'py, PyAny>) -> PyResult<Arc<Schema>> {
        let py = self.py;
        let refs = self.refs;
        if let Ok(alias) = ty.cast::<ListType>() {
            return Ok(intern(Schema::List(alias.get().item.clone())));
        }
        if let Ok(alias) = ty.cast::<DictType>() {
            let alias = alias.get();
            return Ok(intern(Schema::Dict(alias.key.clone(), alias.value.clone())));
        }
        if let Ok(alias) = ty.cast::<SetType>() {
            return Ok(intern(Schema::Set(alias.get().item.clone())));
        }
        if let Ok(class) = ty.cast::<PyType>() {
            return self.compile_class(class);
        }
        if ty.is_instance(refs.type_alias_type.bind(py))? {
            return self.expand_alias(ty, None);
        }
        if ty.is_instance(refs.type_var.bind(py))? {
            // Bound or not, a type variable is erased at runtime (like `list[T]`): binding
            // it to its bound would coerce values (e.g. `bool` → `int`) that `T` admits.
            let bound = self
                .frames
                .last()
                .and_then(|frame| frame.iter().find(|(param, _)| param.is(ty)))
                .map(|(_, schema)| schema.clone());
            return Ok(bound.unwrap_or_else(any));
        }
        if ty.is_instance(refs.param_spec.bind(py))?
            || ty.is_instance(refs.type_var_tuple.bind(py))?
        {
            return Err(PyTypeError::new_err(format!(
                "{} is not a valid element type (ParamSpec and TypeVarTuple describe argument lists)",
                type_repr(ty)
            )));
        }
        if ty.is_instance(refs.new_type.bind(py))? {
            return self.compile(&ty.getattr(intern!(py, "__supertype__"))?);
        }
        if ty.is_instance_of::<PyString>() || ty.is_instance(refs.forward_ref.bind(py))? {
            return Err(PyTypeError::new_err(format!(
                "forward references are not supported as element types (got {}); pass the \
                 class itself once it is defined",
                type_repr(ty)
            )));
        }
        let origin = refs.get_origin.bind(py).call1((ty,))?;
        if !origin.is_none() {
            return self.compile_generic(ty, &origin);
        }
        Err(unsupported(ty))
    }

    fn compile_class(&mut self, class: &Bound<'py, PyType>) -> PyResult<Arc<Schema>> {
        let py = self.py;
        if class.is(py.get_type::<PyList>()) || class.is(py.get_type::<List>()) {
            return Ok(intern(Schema::List(any())));
        }
        if class.is(py.get_type::<PyDict>()) || class.is(py.get_type::<Dict>()) {
            return Ok(intern(Schema::Dict(any(), any())));
        }
        if class.is(py.get_type::<PySet>()) || class.is(py.get_type::<Set>()) {
            return Ok(intern(Schema::Set(any())));
        }
        if SCALAR_NAMES.contains(&class.name()?.to_cow()?.as_ref())
            && let Some(dtype) = crate::scalars::dtype_of_type(class)
        {
            return Ok(intern(Schema::Fixed(dtype)));
        }
        if !self.has_identity_eq(class)? {
            self.cacheable = false;
        }
        Ok(intern(Schema::Class(class.clone().unbind())))
    }

    /// Whether `class` compares and hashes by identity (its metaclass keeps `type`'s
    /// `__eq__`/`__hash__`), making it a safe cache key.
    fn has_identity_eq(&self, class: &Bound<'py, PyType>) -> PyResult<bool> {
        let py = self.py;
        let meta = class.get_type();
        if meta.is(py.get_type::<PyType>()) {
            return Ok(true);
        }
        Ok(meta.getattr(intern!(py, "__eq__"))?.is(&self.refs.type_eq)
            && meta
                .getattr(intern!(py, "__hash__"))?
                .is(&self.refs.type_hash))
    }

    /// Expressions with a `typing.get_origin`: unions, `Annotated`, generic aliases.
    fn compile_generic(
        &mut self,
        ty: &Bound<'py, PyAny>,
        origin: &Bound<'py, PyAny>,
    ) -> PyResult<Arc<Schema>> {
        let py = self.py;
        let refs = self.refs;
        let args = refs
            .get_args
            .bind(py)
            .call1((ty,))?
            .cast_into::<PyTuple>()?;
        if origin.is(&refs.annotated) {
            return self.compile(&args.get_item(0)?);
        }
        if origin.is(&refs.union) || origin.is(&refs.union_type) {
            return self.compile_union(&args);
        }
        if origin.is(&refs.literal) {
            return Err(PyTypeError::new_err(format!(
                "Literal types are not supported as element types (got {}); use the type of \
                 the literal values instead",
                type_repr(ty)
            )));
        }
        if origin.is(py.get_type::<PyList>()) {
            let [item] = self.container_args(ty, &args)?;
            return Ok(intern(Schema::List(item)));
        }
        if origin.is(py.get_type::<PyDict>()) {
            let [key, value] = self.container_args(ty, &args)?;
            return Ok(intern(Schema::Dict(key, value)));
        }
        if origin.is(py.get_type::<PySet>()) {
            let [item] = self.container_args(ty, &args)?;
            return Ok(intern(Schema::Set(item)));
        }
        if origin.is_instance(refs.type_alias_type.bind(py))? {
            return self.expand_alias(origin, Some(&args));
        }
        if let Ok(class) = origin.cast::<PyType>() {
            // A bare deprecated alias such as `typing.Sequence` means the class itself.
            if args.is_empty() && !ty.hasattr(intern!(py, "__args__"))? {
                return self.compile_class(class);
            }
            return Err(PyTypeError::new_err(format!(
                "{} cannot be checked at runtime: only list[...], dict[...] and set[...] are \
                 validated element-wise; use the bare class {} to accept any instance",
                type_repr(ty),
                type_repr(class)
            )));
        }
        Err(unsupported(ty))
    }

    /// Element schemas of `list[T]`/`dict[K, V]`/`set[T]`; no arguments means `Any`.
    fn container_args<const N: usize>(
        &mut self,
        ty: &Bound<'py, PyAny>,
        args: &Bound<'py, PyTuple>,
    ) -> PyResult<[Arc<Schema>; N]> {
        if args.is_empty() {
            return Ok(std::array::from_fn(|_| any()));
        }
        if args.len() != N {
            return Err(PyTypeError::new_err(format!(
                "{} expects {N} type argument{}, got {}",
                type_repr(ty),
                if N == 1 { "" } else { "s" },
                args.len()
            )));
        }
        let mut schemas = Vec::with_capacity(N);
        for arg in args.iter() {
            schemas.push(self.compile(&arg)?);
        }
        schemas
            .try_into()
            .map_err(|_| PyRuntimeError::new_err("rustique: container arity mismatch"))
    }

    /// `A | B | None`: flatten nested unions/optionals, dedupe preserving order, and
    /// collapse to `Optional`/a single member/`Any` where possible.
    fn compile_union(&mut self, args: &Bound<'py, PyTuple>) -> PyResult<Arc<Schema>> {
        fn absorb(
            schema: &Arc<Schema>,
            none: &Arc<Schema>,
            members: &mut Vec<Arc<Schema>>,
            has_none: &mut bool,
        ) {
            match &**schema {
                _ if schema == none => *has_none = true,
                Schema::Optional(inner) => {
                    *has_none = true;
                    absorb(inner, none, members, has_none);
                }
                Schema::Union(inner) => inner
                    .iter()
                    .for_each(|m| absorb(m, none, members, has_none)),
                _ if members.contains(schema) => {}
                _ => members.push(schema.clone()),
            }
        }

        let none = none_schema(self.py, self.refs)?;
        let mut members = Vec::new();
        let mut has_none = false;
        for arg in args.iter() {
            let schema = self.compile(&arg)?;
            absorb(&schema, &none, &mut members, &mut has_none);
        }
        if members.iter().any(|m| m.is_any()) {
            return Ok(any());
        }
        let core = match members.len() {
            0 => return Ok(none),
            1 => members.pop().unwrap_or_else(any),
            _ => {
                if self.expanding.is_empty() {
                    if self.depth == 1 {
                        self.top_union = true;
                    } else {
                        self.cacheable = false;
                    }
                }
                intern(Schema::Union(members))
            }
        };
        Ok(if has_none {
            intern(Schema::Optional(core))
        } else {
            core
        })
    }

    /// PEP 695 `type Alias[T] = ...`: compile `__value__` with the type parameters bound
    /// to `args` (unbound parameters erase to `Any`).
    fn expand_alias(
        &mut self,
        alias: &Bound<'py, PyAny>,
        args: Option<&Bound<'py, PyTuple>>,
    ) -> PyResult<Arc<Schema>> {
        let py = self.py;
        let name = alias.getattr(intern!(py, "__name__"))?.to_string();
        if self.expanding.iter().any(|a| a.is(alias)) {
            return Err(PyTypeError::new_err(format!(
                "recursive type alias {name} is not supported as an element type"
            )));
        }
        let params = alias
            .getattr(intern!(py, "__type_params__"))?
            .cast_into::<PyTuple>()?;
        let mut frame = Vec::new();
        if let Some(args) = args {
            if args.len() > params.len() {
                return Err(alias_arity(&name, params.len(), args.len()));
            }
            for (param, arg) in params.iter().zip(args.iter()) {
                if !param.is_instance(self.refs.type_var.bind(py))? {
                    return Err(PyTypeError::new_err(format!(
                        "type alias {name} has a ParamSpec or TypeVarTuple parameter, which \
                         element types do not support"
                    )));
                }
                frame.push((param, self.compile(&arg)?));
            }
        }
        let value = alias.getattr(intern!(py, "__value__")).map_err(|err| {
            let wrapped =
                PyTypeError::new_err(format!("could not resolve type alias {name}: {err}"));
            wrapped.set_cause(py, Some(err));
            wrapped
        })?;
        self.expanding.push(alias.clone());
        self.frames.push(frame);
        let result = self.expand_alias_body(
            &name,
            &params,
            args.map_or(0, |a| a.len()),
            args.is_some(),
            &value,
        );
        self.frames.pop();
        self.expanding.pop();
        result
    }

    fn expand_alias_body(
        &mut self,
        name: &str,
        params: &Bound<'py, PyTuple>,
        given: usize,
        applied: bool,
        value: &Bound<'py, PyAny>,
    ) -> PyResult<Arc<Schema>> {
        if applied {
            // PEP 696 defaults for trailing parameters; they may refer to earlier ones.
            for param in params.iter().skip(given) {
                if !param
                    .call_method0(intern!(self.py, "has_default"))?
                    .is_truthy()?
                {
                    return Err(alias_arity(name, params.len(), given));
                }
                let schema = self.compile(&param.getattr(intern!(self.py, "__default__"))?)?;
                if let Some(frame) = self.frames.last_mut() {
                    frame.push((param, schema));
                }
            }
        }
        self.compile(value)
    }
}

fn alias_arity(name: &str, expected: usize, got: usize) -> PyErr {
    PyTypeError::new_err(format!(
        "type alias {name} expects {expected} type arguments, got {got}"
    ))
}

fn unsupported(ty: &Bound<'_, PyAny>) -> PyErr {
    PyTypeError::new_err(format!(
        "{} is not a supported element type; expected a class, X | Y, Optional[X], \
         Annotated[X, ...], list[X], dict[K, V], set[X], a NewType or a PEP 695 type alias",
        type_repr(ty)
    ))
}

/// `int`, `collections.OrderedDict`, or the object's repr for non-classes.
fn type_repr(ty: &Bound<'_, PyAny>) -> String {
    if let Ok(class) = ty.cast::<PyType>()
        && let (Ok(module), Ok(qualname)) = (class.module(), class.qualname())
    {
        return if module.to_cow().is_ok_and(|m| m == "builtins") {
            qualname.to_string()
        } else {
            format!("{module}.{qualname}")
        };
    }
    ty.repr().map_or_else(
        |_| "<unprintable type expression>".to_owned(),
        |r| r.to_string(),
    )
}

// ---------------------------------------------------------------------------------------
// Rendering and canonical type objects
// ---------------------------------------------------------------------------------------

/// Python-flavoured rendering: `int`, `i8`, `int | None`, `List[str]`, `Dict[str, float]`;
/// untyped nested containers render as the bare class name.
fn render(py: Python<'_>, schema: &Schema, out: &mut String) {
    match schema {
        Schema::Any => out.push_str("Any"),
        Schema::Int => out.push_str("int"),
        Schema::Float => out.push_str("float"),
        Schema::Bool => out.push_str("bool"),
        Schema::Str => out.push_str("str"),
        Schema::Fixed(dtype) => out.push_str(dtype.name()),
        Schema::Optional(inner) => {
            render(py, inner, out);
            out.push_str(" | None");
        }
        Schema::Union(members) => render_joined(py, members, " | ", out),
        Schema::List(item) => render_container(py, "List", &[item], out),
        Schema::Dict(key, value) => render_container(py, "Dict", &[key, value], out),
        Schema::Set(item) => render_container(py, "Set", &[item], out),
        Schema::Class(class) => {
            let class = class.bind(py);
            if class.is(py.None().bind(py).get_type()) {
                out.push_str("None");
            } else {
                match class.qualname() {
                    Ok(name) => out.push_str(&name.to_string()),
                    Err(_) => out.push('?'),
                }
            }
        }
    }
}

fn render_joined<'a>(
    py: Python<'_>,
    schemas: impl IntoIterator<Item = &'a Arc<Schema>>,
    sep: &str,
    out: &mut String,
) {
    for (i, schema) in schemas.into_iter().enumerate() {
        if i > 0 {
            out.push_str(sep);
        }
        render(py, schema, out);
    }
}

fn render_container(py: Python<'_>, name: &str, params: &[&Arc<Schema>], out: &mut String) {
    out.push_str(name);
    if params.iter().any(|p| !p.is_any()) {
        out.push('[');
        render_joined(py, params.iter().copied(), ", ", out);
        out.push(']');
    }
}

fn render_string(py: Python<'_>, schema: &Schema) -> String {
    let mut out = String::new();
    render(py, schema, &mut out);
    out
}

/// A type expression that compiles back to `schema` (used for `__args__` and pickling).
fn canonical<'py>(py: Python<'py>, schema: &Arc<Schema>) -> PyResult<Bound<'py, PyAny>> {
    let refs = refs(py)?;
    Ok(match &**schema {
        Schema::Any => refs.any.bind(py).clone(),
        Schema::Int => py.get_type::<PyInt>().into_any(),
        Schema::Float => py.get_type::<PyFloat>().into_any(),
        Schema::Bool | Schema::Fixed(DType::Bool) => py.get_type::<PyBool>().into_any(),
        Schema::Str | Schema::Fixed(DType::Str) => py.get_type::<PyString>().into_any(),
        Schema::Fixed(dtype) => py.import("rustique._rustique")?.getattr(dtype.name())?,
        Schema::Optional(inner) => {
            let mut members = match &**inner {
                Schema::Union(members) => canonical_all(py, members)?,
                _ => vec![canonical(py, inner)?],
            };
            members.push(refs.none_type.bind(py).clone().into_any());
            refs.union.bind(py).get_item(PyTuple::new(py, members)?)?
        }
        Schema::Union(members) => refs
            .union
            .bind(py)
            .get_item(PyTuple::new(py, canonical_all(py, members)?)?)?,
        Schema::List(item) => list_alias_for(py, item)?,
        Schema::Dict(key, value) => dict_alias_for(py, key, value)?,
        Schema::Set(item) => set_alias_for(py, item)?,
        Schema::Class(class) => class.bind(py).clone().into_any(),
    })
}

fn canonical_all<'py>(
    py: Python<'py>,
    schemas: &[Arc<Schema>],
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    schemas.iter().map(|s| canonical(py, s)).collect()
}

fn same_schema(a: &Arc<Schema>, b: &Arc<Schema>) -> bool {
    Arc::ptr_eq(a, b) || a == b
}

// ---------------------------------------------------------------------------------------
// Alias objects
// ---------------------------------------------------------------------------------------

/// `List[T]`: a typed view of the `List` class (`List[int](...)`, `isinstance(x, List[int])`).
#[pyclass(module = "rustique", frozen)]
pub struct ListType {
    item: Arc<Schema>,
    args: PyOnceLock<Py<PyTuple>>,
}

/// `Dict[K, V]`.
#[pyclass(module = "rustique", frozen)]
pub struct DictType {
    key: Arc<Schema>,
    value: Arc<Schema>,
    args: PyOnceLock<Py<PyTuple>>,
}

/// `Set[T]`.
#[pyclass(module = "rustique", frozen)]
pub struct SetType {
    item: Arc<Schema>,
    args: PyOnceLock<Py<PyTuple>>,
}

impl ListType {
    /// The element schema.
    pub fn item_schema(&self) -> &Arc<Schema> {
        &self.item
    }

    fn params(&self) -> [&Arc<Schema>; 1] {
        [&self.item]
    }
}

impl DictType {
    pub fn key_schema(&self) -> &Arc<Schema> {
        &self.key
    }

    pub fn value_schema(&self) -> &Arc<Schema> {
        &self.value
    }

    fn params(&self) -> [&Arc<Schema>; 2] {
        [&self.key, &self.value]
    }
}

impl SetType {
    /// The element schema.
    pub fn item_schema(&self) -> &Arc<Schema> {
        &self.item
    }

    fn params(&self) -> [&Arc<Schema>; 1] {
        [&self.item]
    }
}

/// `__reduce__` result: `(operator.getitem, (origin, subscript))`.
type Reduced<'py> = (Bound<'py, PyAny>, (Bound<'py, PyType>, Bound<'py, PyAny>));

/// The `#[pymethods]` shared by the alias classes; `$extra` holds the per-class methods
/// (`__call__`, `__instancecheck__`, `schema`).
macro_rules! alias_methods {
    ($alias:ty, $origin:ty, $name:literal, { $($extra:tt)* }) => {
        impl $alias {
            fn repr(&self, py: Python<'_>) -> String {
                let mut out = String::new();
                render_container(py, $name, &self.params(), &mut out);
                out
            }
        }

        #[pymethods]
        impl $alias {
            $($extra)*

            fn __repr__(&self, py: Python<'_>) -> String {
                self.repr(py)
            }

            fn __eq__(&self, other: &Self) -> bool {
                self.params().iter().zip(other.params()).all(|(a, b)| same_schema(a, b))
            }

            fn __hash__(&self) -> u64 {
                let mut hasher = DefaultHasher::new();
                $name.hash(&mut hasher);
                self.params().hash(&mut hasher);
                hasher.finish()
            }

            /// `alias | other`, like `typing` aliases.
            fn __or__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                refs(slf.py())?.union.bind(slf.py()).get_item((slf, other))
            }

            fn __ror__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                refs(slf.py())?.union.bind(slf.py()).get_item((other, slf))
            }

            fn __getitem__(&self, py: Python<'_>, _params: &Bound<'_, PyAny>) -> PyResult<()> {
                Err(PyTypeError::new_err(format!("{} is already parameterised", self.repr(py))))
            }

            fn __mro_entries__(&self, py: Python<'_>, _bases: &Bound<'_, PyAny>) -> PyResult<()> {
                Err(PyTypeError::new_err(format!(
                    "cannot subclass {}: rustique containers do not support subclassing; \
                     wrap one in your own class instead",
                    self.repr(py)
                )))
            }

            fn __reduce__<'py>(&self, py: Python<'py>) -> PyResult<Reduced<'py>> {
                let args = self.type_args(py)?;
                let subscript = if args.len() == 1 { args.get_item(0)? } else { args.into_any() };
                let getitem = py.import("operator")?.getattr("getitem")?;
                Ok((getitem, (py.get_type::<$origin>(), subscript)))
            }

            fn __copy__(slf: &Bound<'_, Self>) -> Py<Self> {
                slf.clone().unbind()
            }

            fn __deepcopy__(slf: &Bound<'_, Self>, _memo: &Bound<'_, PyAny>) -> Py<Self> {
                slf.clone().unbind()
            }

            /// The rustique container class.
            #[getter(__origin__)]
            fn origin<'py>(&self, py: Python<'py>) -> Bound<'py, PyType> {
                py.get_type::<$origin>()
            }

            /// The parameters as canonical type expressions, e.g. `(int | None,)`.
            #[getter(__args__)]
            fn type_args<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
                once(&self.args, py, || {
                    let items = self.params().map(|s| canonical(py, s));
                    Ok(PyTuple::new(py, items.into_iter().collect::<PyResult<Vec<_>>>()?)?.unbind())
                })
                .map(|args| args.bind(py).clone())
            }

            #[getter(__parameters__)]
            fn parameters<'py>(&self, py: Python<'py>) -> Bound<'py, PyTuple> {
                PyTuple::empty(py)
            }

            #[getter(__name__)]
            fn name(&self) -> &'static str {
                $name
            }

            #[getter(__qualname__)]
            fn qualname(&self) -> &'static str {
                $name
            }
        }
    };
}

alias_methods!(ListType, List, "List", {
    /// `List[T](iterable=(), /)`.
    #[pyo3(signature = (*args, **kwargs))]
    fn __call__<'py>(
        &self,
        py: Python<'py>,
        args: &Bound<'py, PyTuple>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        no_keywords(kwargs, || self.repr(py))?;
        let iterable = single_positional(args, || self.repr(py))?;
        Ok(List::from_iterable(py, self.item.clone(), iterable.as_ref())?.into_any())
    }

    fn __instancecheck__(&self, obj: &Bound<'_, PyAny>) -> bool {
        obj.cast::<List>()
            .is_ok_and(|list| same_schema(list.get().schema(), &self.item))
    }

    /// The element schema, e.g. `'int | None'`.
    #[getter]
    fn schema(&self, py: Python<'_>) -> String {
        render_string(py, &self.item)
    }
});

alias_methods!(DictType, Dict, "Dict", {
    /// `Dict[K, V](arg=(), /, **kwargs)`.
    #[pyo3(signature = (*args, **kwargs))]
    fn __call__<'py>(
        &self,
        py: Python<'py>,
        args: &Bound<'py, PyTuple>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let arg = single_positional(args, || self.repr(py))?;
        Ok(Dict::from_args(
            py,
            self.key.clone(),
            self.value.clone(),
            arg.as_ref(),
            kwargs,
        )?
        .into_any())
    }

    fn __instancecheck__(&self, obj: &Bound<'_, PyAny>) -> bool {
        obj.cast::<Dict>().is_ok_and(|dict| {
            let dict = dict.get();
            same_schema(dict.key_schema(), &self.key)
                && same_schema(dict.value_schema(), &self.value)
        })
    }

    /// The key and value schemas, e.g. `'str, List[float]'`.
    #[getter]
    fn schema(&self, py: Python<'_>) -> String {
        let mut out = String::new();
        render_joined(py, self.params(), ", ", &mut out);
        out
    }
});

alias_methods!(SetType, Set, "Set", {
    /// `Set[T](iterable=(), /)`.
    #[pyo3(signature = (*args, **kwargs))]
    fn __call__<'py>(
        &self,
        py: Python<'py>,
        args: &Bound<'py, PyTuple>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        no_keywords(kwargs, || self.repr(py))?;
        let iterable = single_positional(args, || self.repr(py))?;
        Ok(Set::from_iterable(py, self.item.clone(), iterable.as_ref())?.into_any())
    }

    fn __instancecheck__(&self, obj: &Bound<'_, PyAny>) -> bool {
        obj.cast::<Set>()
            .is_ok_and(|set| same_schema(set.get().schema(), &self.item))
    }

    /// The element schema, e.g. `'str'`.
    #[getter]
    fn schema(&self, py: Python<'_>) -> String {
        render_string(py, &self.item)
    }
});

/// `list()`/`set()` take no keyword arguments.
fn no_keywords(kwargs: Option<&Bound<'_, PyDict>>, repr: impl FnOnce() -> String) -> PyResult<()> {
    match kwargs {
        Some(kwargs) if !kwargs.is_empty() => Err(PyTypeError::new_err(format!(
            "{}() takes no keyword arguments",
            repr()
        ))),
        _ => Ok(()),
    }
}

/// The optional single positional argument of a constructor call.
fn single_positional<'py>(
    args: &Bound<'py, PyTuple>,
    repr: impl FnOnce() -> String,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    match args.len() {
        0 => Ok(None),
        1 => Ok(Some(args.get_item(0)?)),
        n => Err(PyTypeError::new_err(format!(
            "{} expected at most 1 argument, got {n}",
            repr()
        ))),
    }
}

// ---------------------------------------------------------------------------------------
// Alias caches
// ---------------------------------------------------------------------------------------

/// Aliases per schema. Keys are structural, so even a non-interned schema finds its alias.
#[derive(Default)]
struct AliasMaps {
    list: HashMap<Arc<Schema>, Py<ListType>>,
    dict: HashMap<(Arc<Schema>, Arc<Schema>), Py<DictType>>,
    set: HashMap<Arc<Schema>, Py<SetType>>,
}

static ALIASES: LazyLock<Mutex<AliasMaps>> = LazyLock::new(Mutex::default);

/// Lock without blocking the interpreter: when contended, wait detached, then retry.
/// Critical sections only touch Rust data (hash lookups and increfs).
fn lock_aliases(py: Python<'_>) -> MutexGuard<'static, AliasMaps> {
    loop {
        if let Some(guard) = ALIASES.try_lock() {
            return guard;
        }
        py.detach(|| drop(ALIASES.lock()));
    }
}

/// Look up or insert an alias; `make` runs (and a losing duplicate is dropped) outside the lock.
fn cached_alias<'py, K, T>(
    py: Python<'py>,
    key: K,
    map: fn(&mut AliasMaps) -> &mut HashMap<K, Py<T>>,
    make: impl FnOnce() -> T,
) -> PyResult<Bound<'py, PyAny>>
where
    K: Eq + std::hash::Hash,
    T: pyo3::PyClass + Into<pyo3::PyClassInitializer<T>>,
{
    let hit = map(&mut lock_aliases(py))
        .get(&key)
        .map(|alias| alias.clone_ref(py));
    if let Some(alias) = hit {
        return Ok(alias.into_bound(py).into_any());
    }
    let fresh = Py::new(py, make())?;
    let winner = map(&mut lock_aliases(py))
        .entry(key)
        .or_insert_with(|| fresh.clone_ref(py))
        .clone_ref(py);
    drop(fresh);
    Ok(winner.into_bound(py).into_any())
}

/// The per-kind dict from subscript expression to alias: the `List[int]` fast path.
fn expr_cache<'py>(py: Python<'py>, cell: &PyOnceLock<Py<PyDict>>) -> PyResult<Bound<'py, PyDict>> {
    Ok(once(cell, py, || Ok(PyDict::new(py).unbind()))?
        .bind(py)
        .clone())
}

/// The subscript of a one-parameter alias: `T` or the 1-tuple `(T,)`.
fn single_param<'py>(name: &str, params: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    match params.cast::<PyTuple>() {
        Ok(tuple) if tuple.len() == 1 => tuple.get_item(0),
        Ok(tuple) => Err(PyTypeError::new_err(format!(
            "{name}[...] expects 1 type argument, got {}",
            tuple.len()
        ))),
        Err(_) => Ok(params.clone()),
    }
}

/// INTERFACE. `List[item]`: the cached alias object for the element type expression `item`.
pub fn list_alias<'py>(py: Python<'py>, item: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    static BY_EXPR: PyOnceLock<Py<PyDict>> = PyOnceLock::new();
    let item = single_param("List", item)?;
    let key = cache_key(&item, refs(py)?).map(|(key, _)| key);
    let (alias, _) = cached(&expr_cache(py, &BY_EXPR)?, key.as_ref(), || {
        let (schema, storable) = compile_cached(&item)?;
        Ok((list_alias_for(py, &schema)?, storable))
    })?;
    Ok(alias)
}

/// INTERFACE. `Dict[key, value]` alias (`params` is the subscript, normally a 2-tuple).
pub fn dict_alias<'py>(py: Python<'py>, params: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    static BY_EXPR: PyOnceLock<Py<PyDict>> = PyOnceLock::new();
    let (key, value) = match params.cast::<PyTuple>() {
        Ok(tuple) if tuple.len() == 2 => (tuple.get_item(0)?, tuple.get_item(1)?),
        other => {
            let got = other.map_or(1, |t| t.len());
            return Err(PyTypeError::new_err(format!(
                "Dict[...] expects 2 type arguments (key, value), got {got}"
            )));
        }
    };
    let refs = refs(py)?;
    let cache_key = match (cache_key(&key, refs), cache_key(&value, refs)) {
        (Some((_, false)), Some((_, false))) => Some(params.clone()),
        (Some((k, _)), Some((v, _))) => Some(PyTuple::new(py, [k, v])?.into_any()),
        _ => None,
    };
    let (alias, _) = cached(&expr_cache(py, &BY_EXPR)?, cache_key.as_ref(), || {
        let (key, key_storable) = compile_cached(&key)?;
        let (value, value_storable) = compile_cached(&value)?;
        Ok((
            dict_alias_for(py, &key, &value)?,
            key_storable && value_storable,
        ))
    })?;
    Ok(alias)
}

/// INTERFACE. `Set[item]` alias.
pub fn set_alias<'py>(py: Python<'py>, item: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    static BY_EXPR: PyOnceLock<Py<PyDict>> = PyOnceLock::new();
    let item = single_param("Set", item)?;
    let key = cache_key(&item, refs(py)?).map(|(key, _)| key);
    let (alias, _) = cached(&expr_cache(py, &BY_EXPR)?, key.as_ref(), || {
        let (schema, storable) = compile_cached(&item)?;
        Ok((set_alias_for(py, &schema)?, storable))
    })?;
    Ok(alias)
}

/// INTERFACE. The alias object describing an existing schema (used by `schema_type`).
/// For `Any` returns the bare class (`List`, `Dict`, `Set`).
pub fn list_alias_for<'py>(py: Python<'py>, schema: &Arc<Schema>) -> PyResult<Bound<'py, PyAny>> {
    if schema.is_any() {
        return Ok(py.get_type::<List>().into_any());
    }
    cached_alias(
        py,
        schema.clone(),
        |maps| &mut maps.list,
        || ListType {
            item: schema.clone(),
            args: PyOnceLock::new(),
        },
    )
}

pub fn dict_alias_for<'py>(
    py: Python<'py>,
    key: &Arc<Schema>,
    value: &Arc<Schema>,
) -> PyResult<Bound<'py, PyAny>> {
    if key.is_any() && value.is_any() {
        return Ok(py.get_type::<Dict>().into_any());
    }
    cached_alias(
        py,
        (key.clone(), value.clone()),
        |maps| &mut maps.dict,
        || DictType {
            key: key.clone(),
            value: value.clone(),
            args: PyOnceLock::new(),
        },
    )
}

pub fn set_alias_for<'py>(py: Python<'py>, schema: &Arc<Schema>) -> PyResult<Bound<'py, PyAny>> {
    if schema.is_any() {
        return Ok(py.get_type::<Set>().into_any());
    }
    cached_alias(
        py,
        schema.clone(),
        |maps| &mut maps.set,
        || SetType {
            item: schema.clone(),
            args: PyOnceLock::new(),
        },
    )
}

/// `_schema_repr(tp)`: the compiled schema of a type expression, rendered (debug helper).
#[pyfunction]
#[pyo3(name = "_schema_repr")]
fn schema_repr(ty: &Bound<'_, PyAny>) -> PyResult<String> {
    let schema = compile(ty)?;
    Ok(render_string(ty.py(), &schema))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ListType>()?;
    m.add_class::<DictType>()?;
    m.add_class::<SetType>()?;
    m.add_function(wrap_pyfunction!(schema_repr, m)?)?;
    Ok(())
}
