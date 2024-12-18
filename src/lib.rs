use pyo3::prelude::*;

mod primitives;
mod collections;

#[pymodule]
fn rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    primitives::register_primitives(m)?;
    collections::register_collections(m)?;
    Ok(())
}

