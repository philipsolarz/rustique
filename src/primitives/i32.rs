use pyo3::prelude::*;

#[pyclass(name="i32")]
pub struct I32 {
    value: i32,
}

#[pymethods]
impl I32 {
    #[new]
    pub fn new(value: i32) -> Self {
        I32 { value }
    }

}

pub fn register_i32(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<I32>()?;
    Ok(())
}
