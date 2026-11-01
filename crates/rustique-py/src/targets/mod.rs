//! Accelerated targets (docs/ACCELERATION.md §9): one directory per group.

pub mod float;

/// Register the targets' private Python modules (`rustique._rustique._t_<group>`).
pub fn register(m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
    float::register(m)?;
    Ok(())
}
