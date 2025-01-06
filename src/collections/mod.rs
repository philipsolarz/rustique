use pyo3::prelude::*;

// pub mod vector;
// pub mod hashmap;
pub mod list;

pub fn register_collections(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // vector::register_vector(m)?;
    list::register_list(m)?;
    // hashmap::register_hashmap(m)?;
    Ok(())
}
