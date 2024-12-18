use pyo3::prelude::*;

#[pyclass(name="i64")]
pub struct I64 {
    value: i64,
}

#[pymethods]
impl I64 {
    #[new]
    pub fn new(value: i64) -> Self {
        I64 { value }
    }

}

pub fn register_i64(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<I64>()?;
    Ok(())
}
