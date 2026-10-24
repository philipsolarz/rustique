//! Fixed-width scalar types `i8 … u64`, `f32`, `f64` (ARCHITECTURE §4.4).
//!
//! OWNER: scalars agent. Items marked `INTERFACE` are used by other modules.

use pyo3::prelude::*;
use pyo3::types::PyType;
use rustique_core::DType;

/// INTERFACE. If `ty` is one of the scalar classes, its dtype.
pub fn dtype_of_type(ty: &Bound<'_, PyType>) -> Option<DType> {
    let _ = ty;
    todo!("scalars agent")
}

/// INTERFACE. If `obj` is an instance of a scalar class, its native value.
pub fn scalar_value(obj: &Bound<'_, PyAny>) -> Option<rustique_core::Value> {
    let _ = obj;
    todo!("scalars agent")
}

pub fn register(_m: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}
