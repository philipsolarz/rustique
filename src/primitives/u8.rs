use pyo3::prelude::*;

#[pyclass(name="u8")]
pub struct U8 {
    value: u8,
}

#[pymethods]
impl U8 {
    #[new]
    pub fn new(value: u8) -> Self {
        U8 { value }
    }

}

pub fn register_u8(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<U8>()?;
    Ok(())
}
