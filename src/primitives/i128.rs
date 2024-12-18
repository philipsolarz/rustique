use pyo3::prelude::*;

#[pyclass(name="i128")]
pub struct I128 {
    value: i128,
}

#[pymethods]
impl I128 {
    #[new]
    pub fn new(value: i128) -> Self {
        I128 { value }
    }

}

pub fn register_i128(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<I128>()?;
    Ok(())
}
