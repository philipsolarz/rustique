//! Accelerated targets (docs/ACCELERATION.md §9): one directory per group.

pub mod posixpath;

use pyo3::prelude::*;

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    posixpath::register(m)?;
    Ok(())
}
