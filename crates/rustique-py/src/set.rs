//! The `Set` class (ARCHITECTURE §4.3).
//!
//! OWNER: dict/set agent. Items marked `INTERFACE` are used by other modules.

use std::sync::Arc;

use pyo3::prelude::*;

use crate::schema::Schema;

#[pyclass(module = "rustique", frozen)]
pub struct Set {
    schema: Arc<Schema>,
}

impl Set {
    /// INTERFACE. Construct from an optional iterable.
    pub fn from_iterable<'py>(
        py: Python<'py>,
        schema: Arc<Schema>,
        iterable: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, Set>> {
        let _ = (py, schema, iterable);
        todo!("dict/set agent")
    }

    /// INTERFACE.
    pub fn schema(&self) -> &Arc<Schema> {
        &self.schema
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Set>()
}
