use pyo3::prelude::*;

#[pyclass(name="u32")]
pub struct U32 {
    value: u32,
}

#[pymethods]
impl U32 {
    #[new]
    pub fn new(value: u32) -> Self {
        U32 { value }
    }

}

pub fn register_u32(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<U32>()?;
    Ok(())
}
