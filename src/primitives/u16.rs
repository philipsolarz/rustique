use pyo3::prelude::*;

#[pyclass(name="u16")]
pub struct U16 {
    value: u16,
}

#[pymethods]
impl U16 {
    #[new]
    pub fn new(value: u16) -> Self {
        U16 { value }
    }

}

pub fn register_u16(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<U16>()?;
    Ok(())
}
