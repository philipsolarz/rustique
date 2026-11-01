//! Python bindings for Rustique: module `rustique._rustique`.
//!
//! This file only wires modules together; each component registers its own classes.

use pyo3::ffi;
use pyo3::prelude::*;

pub mod accel;
pub mod builtins;
pub mod capi;
pub mod convert;
pub mod dict;
pub mod expr;
pub mod frozenset;
pub mod list;
pub mod objsort;
pub mod patch;
pub mod scalars;
pub mod schema;
pub mod set;
pub mod storage;
pub mod sync;
pub mod takeover;
pub mod targets;
pub mod tuple;
pub mod typing;

static MODULE: pyo3::sync::PyOnceLock<Py<PyModule>> = pyo3::sync::PyOnceLock::new();

/// `rustique._rustique` itself (set at module exec; main interpreter only). Never
/// `py.import("rustique._rustique")`: `PyImport_Import` passes an empty fromlist, which
/// imports the parent package `rustique` too (the startup path loads only the extension).
pub fn module(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    MODULE
        .get(py)
        .map(|m| m.bind(py).clone())
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("rustique._rustique is not initialised"))
}

/// Scalar classes, each created on its first access.
const SCALARS: &[&str] = &["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64"];

fn add_scalar(m: &Bound<'_, PyModule>, name: &str) -> PyResult<bool> {
    use scalars::*;
    match name {
        "i8" => m.add_class::<I8>()?,
        "i16" => m.add_class::<I16>()?,
        "i32" => m.add_class::<I32>()?,
        "i64" => m.add_class::<I64>()?,
        "u8" => m.add_class::<U8>()?,
        "u16" => m.add_class::<U16>()?,
        "u32" => m.add_class::<U32>()?,
        "u64" => m.add_class::<U64>()?,
        "f32" => m.add_class::<F32>()?,
        "f64" => m.add_class::<F64>()?,
        _ => return Ok(false),
    }
    Ok(true)
}

static CORE: pyo3::sync::PyOnceLock<()> = pyo3::sync::PyOnceLock::new();

/// Create the opt-in containers and everything that can produce them (types, typed
/// constructors, `X`, `arange`, the C-API capsule), once, on first access to any of them
/// (startup: `import rustique` must cost almost nothing, docs/ACCELERATION.md §5.2). No
/// instance can exist before this ran: every way to make one starts at a module attribute.
fn ensure_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    CORE.get_or_try_init(py, || -> PyResult<()> {
        typing::register(m)?;
        list::register(m)?;
        tuple::register(m)?;
        dict::register(m)?;
        set::register(m)?;
        frozenset::register(m)?;
        expr::register(m)?;
        capi::register(m)?;
        // `isinstance(x, collections.abc.MutableSequence)` etc. hold, as for the builtins.
        // `_collections_abc` is imported by the interpreter at startup.
        let abc = py.import("_collections_abc")?;
        for (abc_name, cls) in [
            ("MutableSequence", py.get_type::<list::List>()),
            ("MutableMapping", py.get_type::<dict::Dict>()),
            ("MutableSet", py.get_type::<set::Set>()),
            ("Sequence", py.get_type::<tuple::Tuple>()),
            ("Set", py.get_type::<frozenset::FrozenSet>()),
        ] {
            abc.getattr(abc_name)?.call_method1("register", (cls,))?;
        }
        builtins::core_types_ready(py)?;
        Ok(())
    })?;
    Ok(())
}

/// PEP 562: names of the opt-in API are created on first access.
#[pyfunction]
#[pyo3(name = "__getattr__")]
fn module_getattr(py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
    let m = module(py)?;
    if name.starts_with("__") && name.ends_with("__") {
        return Err(pyo3::exceptions::PyAttributeError::new_err(format!(
            "module 'rustique._rustique' has no attribute '{name}'"
        )));
    }
    if SCALARS.contains(&name) {
        add_scalar(&m, name)?;
    } else if name == "_builtins" {
        builtins::register(&m)?;
    } else if name == "_takeover" {
        takeover::register(&m)?;
    } else {
        ensure_core(&m)?;
    }
    match m.dict().get_item(name)? {
        Some(v) => Ok(v.unbind()),
        None => Err(pyo3::exceptions::PyAttributeError::new_err(format!(
            "module 'rustique._rustique' has no attribute '{name}'"
        ))),
    }
}

/// `dir()` lists the lazy names too (creating them).
#[pyfunction]
#[pyo3(name = "__dir__")]
fn module_dir(py: Python<'_>) -> PyResult<Vec<String>> {
    let m = module(py)?;
    ensure_core(&m)?;
    for sub in ["_builtins", "_takeover"] {
        if !m.dict().contains(sub)? {
            module_getattr(py, sub)?;
        }
    }
    for name in SCALARS {
        if !m.dict().contains(*name)? {
            add_scalar(&m, name)?;
        }
    }
    let mut names: Vec<String> = m.dict().keys().iter().map(|k| k.extract()).collect::<PyResult<_>>()?;
    names.sort();
    Ok(names)
}

#[pymodule]
fn _rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Refuse every subinterpreter, isolated or legacy shared-GIL (docs/ACCELERATION.md
    // §3.11): PyO3 keeps type objects in process statics, and the accel registry's state
    // (watchers, pins, sites on per-interpreter objects) belongs to the main interpreter.
    // SAFETY: attached thread (module exec).
    if unsafe { ffi::PyInterpreterState_Get() != ffi::PyInterpreterState_Main() } {
        return Err(pyo3::exceptions::PyImportError::new_err(
            "rustique._rustique does not support loading in subinterpreters",
        ));
    }
    accel::guard::set_main_interpreter();
    let _ = MODULE.set(m.py(), m.clone().unbind());
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    // Eager: the switch (`_accel`: registry and its primitives). Everything else (the
    // opt-in API, the test/benchmark submodules `_builtins` and `_takeover`) is created on
    // first access (`__getattr__`).
    accel::py::register(m)?;
    m.add_function(wrap_pyfunction!(module_getattr, m)?)?;
    m.add_function(wrap_pyfunction!(module_dir, m)?)?;
    Ok(())
}
