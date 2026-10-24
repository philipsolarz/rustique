//! The `List` class (ARCHITECTURE §4.1).
//!
//! OWNER: list agent. Items marked `INTERFACE` are used by other modules.

use std::sync::Arc;

use pyo3::prelude::*;
use rustique_core::Column;

use crate::schema::Schema;

#[pyclass(module = "rustique", frozen)]
pub struct List {
    schema: Arc<Schema>,
}

impl List {
    /// INTERFACE. Construct a `List` with `schema` from an optional iterable.
    pub fn from_iterable<'py>(
        py: Python<'py>,
        schema: Arc<Schema>,
        iterable: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, List>> {
        let _ = (py, schema, iterable);
        todo!("list agent")
    }

    /// INTERFACE. Wrap a native column without per-element conversion (O(1)).
    pub fn from_column(py: Python<'_>, column: Column) -> PyResult<Bound<'_, List>> {
        let _ = (py, column);
        todo!("list agent")
    }

    /// INTERFACE. The element schema.
    pub fn schema(&self) -> &Arc<Schema> {
        &self.schema
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<List>()
}
