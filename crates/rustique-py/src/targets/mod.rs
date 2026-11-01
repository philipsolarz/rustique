//! Accelerated targets, one directory per group (docs/ACCELERATION.md §9).

pub mod random;
pub mod utils_kit;

use pyo3::prelude::*;

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    random::register(m)?;
    Ok(())
}
