use pyo3::prelude::*;

#[pyclass(name="i16")]
pub struct I16 {
    value: i16,
}

#[pymethods]
impl I16 {
    #[new]
    pub fn new(value: i16) -> Self {
        I16 { value }
    }

}

pub fn register_i16(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<I16>()?;
    Ok(())
}
