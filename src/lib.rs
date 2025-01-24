use pyo3::prelude::*;

// mod dict;
mod engine;
mod list;
#[pymodule]
fn rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    list::register_list(m)?;
    engine::register_engine(m)?;
    // dict::register_dict(m)?;
    Ok(())
}
