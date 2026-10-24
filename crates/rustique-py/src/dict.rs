//! The `Dict` class (ARCHITECTURE §4.2).
//!
//! OWNER: dict/set agent. Items marked `INTERFACE` are used by other modules.

use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::schema::Schema;

#[pyclass(module = "rustique", frozen)]
pub struct Dict {
    key_schema: Arc<Schema>,
    value_schema: Arc<Schema>,
}

impl Dict {
    /// INTERFACE. Construct like `dict(arg, **kwargs)` with the given key/value schemas.
    pub fn from_args<'py>(
        py: Python<'py>,
        key_schema: Arc<Schema>,
        value_schema: Arc<Schema>,
        arg: Option<&Bound<'py, PyAny>>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Bound<'py, Dict>> {
        let _ = (py, key_schema, value_schema, arg, kwargs);
        todo!("dict/set agent")
    }

    /// INTERFACE.
    pub fn key_schema(&self) -> &Arc<Schema> {
        &self.key_schema
    }

    /// INTERFACE.
    pub fn value_schema(&self) -> &Arc<Schema> {
        &self.value_schema
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Dict>()
}
