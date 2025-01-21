use pyo3::prelude::*;

// mod dict;
mod list;

#[pymodule]
fn rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    list::register_list(m)?;
    // dict::register_dict(m)?;
    Ok(())
}
