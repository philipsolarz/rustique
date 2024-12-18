use pyo3::prelude::*;

pub mod vector;
pub mod hashmap;

pub fn register_collections(m: &Bound<'_, PyModule>) -> PyResult<()> {
    vector::register_vector(m)?;
    hashmap::register_hashmap(m)?;
    Ok(())
}
