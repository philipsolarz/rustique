use pyo3::prelude::*;

#[pyclass(name="f32")]
pub struct F32 {
    value: f32,
}

#[pymethods]
impl F32 {
    #[new]
    pub fn new(value: f32) -> Self {
        F32 { value }
    }

}

pub fn register_f32(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<F32>()?;
    Ok(())
}
