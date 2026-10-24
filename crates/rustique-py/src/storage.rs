//! Storage strategies (ARCHITECTURE §3.3): native columns with transparent demotion to
//! object storage.
//!
//! OWNER: list agent. Items marked `INTERFACE` are used by dict/set/expr code and must keep
//! their signatures.

use pyo3::prelude::*;
use rustique_core::{Column, DType};

use crate::schema::{Coerced, Schema};

/// INTERFACE. The physical representation of a sequence of elements.
#[derive(Debug)]
pub enum Storage {
    /// Untyped and empty: the first stored value picks the strategy.
    Empty,
    /// Unboxed native values.
    Native(Column),
    /// Python objects (heterogeneous data, unions, nested containers, big ints, …).
    Object(Vec<Py<PyAny>>),
}

/// INTERFACE. Python objects displaced by a mutation. Must be dropped *after* releasing
/// any Rustique lock (dropping decrefs and may run `__del__`).
pub type Displaced = Vec<Py<PyAny>>;

impl Storage {
    /// INTERFACE. Initial storage for a schema: `Empty` for `Any`, a native column for
    /// schemas with a preferred dtype, otherwise `Object`.
    pub fn for_schema(schema: &Schema) -> Storage {
        let _ = schema;
        todo!("list agent")
    }

    /// INTERFACE.
    pub fn len(&self) -> usize {
        todo!("list agent")
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// INTERFACE. Strategy name: `"empty"`, a dtype name (`"i64"`, `"str"`, …) or `"object"`.
    pub fn strategy(&self) -> &'static str {
        todo!("list agent")
    }

    /// INTERFACE. The native dtype, if stored natively.
    pub fn dtype(&self) -> Option<DType> {
        todo!("list agent")
    }

    /// INTERFACE. Append one coerced value, demoting to object storage if it does not fit.
    /// Creating Python objects for demotion is allowed under a lock (no user code runs).
    pub fn push(&mut self, py: Python<'_>, value: Coerced) {
        let _ = (py, value);
        todo!("list agent")
    }

    /// INTERFACE. The element at a normalised index as a Python object.
    pub fn get(&self, py: Python<'_>, index: usize) -> Option<Py<PyAny>> {
        let _ = (py, index);
        todo!("list agent")
    }

    /// INTERFACE. Replace the element at a normalised index; returns displaced objects.
    pub fn set(&mut self, py: Python<'_>, index: usize, value: Coerced) -> Displaced {
        let _ = (py, index, value);
        todo!("list agent")
    }

    /// INTERFACE. Remove the element at a normalised index, returning it as a Python object.
    pub fn remove(&mut self, py: Python<'_>, index: usize) -> Py<PyAny> {
        let _ = (py, index);
        todo!("list agent")
    }

    /// INTERFACE. Convert to object storage in place (no-op if already object storage).
    pub fn demote(&mut self, py: Python<'_>) {
        let _ = py;
        todo!("list agent")
    }

    /// INTERFACE. Clone the storage (native: copies the column; object: increfs).
    pub fn clone_ref(&self, py: Python<'_>) -> Storage {
        let _ = py;
        todo!("list agent")
    }
}
