use pyo3::prelude::*;

#[pyclass(name="bool")]
pub struct Bool {
    value: bool,
}

#[pymethods]
impl Bool {
    #[new]
    pub fn new(value: bool) -> Self {
        Bool { value }
    }

}

pub fn register_bool(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Bool>()?;
    Ok(())
}
