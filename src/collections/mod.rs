use pyo3::prelude::*;

// pub mod vector;
// pub mod hashmap;
// pub mod collections;
// pub mod pydict;
// pub mod dict;
pub mod faster;
// pub mod list;
pub fn register_collections(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // vector::register_vector(m)?;
    // dict::register_dict(m)?;
    // list::register_list(m)?;
    // collections::register_collections(m)?;
    // pydict::register_pydict(m)?;
    // hashmap::register_hashmap(m)?;

    faster::register_faster(m)?;
    Ok(())
}
