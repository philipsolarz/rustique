//! Python-facing expression builder `X` and map/filter evaluation (ARCHITECTURE §4.5).
//!
//! OWNER: expr agent. Items marked `INTERFACE` are used by other modules.

use std::sync::Arc;

use pyo3::prelude::*;

use crate::schema::Schema;
use crate::storage::Storage;

/// INTERFACE. `lst.map(f)`: evaluate `f` (an `X` expression or any Python callable) over
/// an owned snapshot of the list's storage. Returns the new storage and its schema.
/// May run Python code: callers must not hold a Rustique lock.
pub fn map_storage(
    py: Python<'_>,
    storage: Storage,
    schema: &Arc<Schema>,
    f: &Bound<'_, PyAny>,
) -> PyResult<(Storage, Arc<Schema>)> {
    let _ = (py, storage, schema, f);
    todo!("expr agent")
}

/// INTERFACE. `lst.filter(f)`: keep elements for which `f` is truthy. Same rules as
/// [`map_storage`]; the schema is unchanged.
pub fn filter_storage(
    py: Python<'_>,
    storage: Storage,
    schema: &Arc<Schema>,
    f: &Bound<'_, PyAny>,
) -> PyResult<Storage> {
    let _ = (py, storage, schema, f);
    todo!("expr agent")
}

pub fn register(_m: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}
