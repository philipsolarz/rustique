use pyo3::prelude::*;

// mod primitives;
mod collections;

mod dict;
mod float;
mod int;
mod list;
mod tuple;

#[pymodule]
fn rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // primitives::register_primitives(m)?;
    // collections::register_collections(m)?;
    int::register_int(m)?;
    float::register_float(m)?;
    list::register_list(m)?;
    dict::register_dict(m)?;
    tuple::register_tuple(m)?;
    Ok(())
}
