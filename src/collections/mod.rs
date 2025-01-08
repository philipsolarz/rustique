use pyo3::prelude::*;

// pub mod vector;
// pub mod hashmap;
pub mod collections;
// pub mod dict;
pub mod list;
pub fn register_collections(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // vector::register_vector(m)?;
    // dict::register_dict(m)?;
    list::register_list(m)?;
    collections::register_collections(m)?;
    // hashmap::register_hashmap(m)?;
    Ok(())
}
