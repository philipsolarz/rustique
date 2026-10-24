//! Compiling Python type expressions into schemas, and the `List[int]`-style alias
//! objects (ARCHITECTURE §3.4–3.5).
//!
//! OWNER: typing agent. Items marked `INTERFACE` are used by other modules and must keep
//! their signatures.

use std::sync::Arc;

use pyo3::prelude::*;

use crate::schema::Schema;

/// INTERFACE. Compile a Python type expression into an interned schema
/// (equal schemas share one `Arc`).
pub fn compile(ty: &Bound<'_, PyAny>) -> PyResult<Arc<Schema>> {
    let _ = ty;
    todo!("typing agent")
}

/// INTERFACE. `List[item]`: the cached alias object for the element type expression `item`.
pub fn list_alias<'py>(py: Python<'py>, item: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, item);
    todo!("typing agent")
}

/// INTERFACE. `Dict[key, value]` alias (`params` is the subscript, normally a 2-tuple).
pub fn dict_alias<'py>(py: Python<'py>, params: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, params);
    todo!("typing agent")
}

/// INTERFACE. `Set[item]` alias.
pub fn set_alias<'py>(py: Python<'py>, item: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, item);
    todo!("typing agent")
}

/// INTERFACE. The alias object describing an existing schema (used by `schema_type`).
/// For `Any` returns the bare class (`List`, `Dict`, `Set`).
pub fn list_alias_for<'py>(py: Python<'py>, schema: &Arc<Schema>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, schema);
    todo!("typing agent")
}

pub fn dict_alias_for<'py>(py: Python<'py>, key: &Arc<Schema>, value: &Arc<Schema>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, key, value);
    todo!("typing agent")
}

pub fn set_alias_for<'py>(py: Python<'py>, schema: &Arc<Schema>) -> PyResult<Bound<'py, PyAny>> {
    let _ = (py, schema);
    todo!("typing agent")
}

pub fn register(_m: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}
