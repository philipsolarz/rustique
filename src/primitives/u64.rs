use pyo3::prelude::*;

#[pyclass(name="u64")]
pub struct U64 {
    value: u64,
}

#[pymethods]
impl U64 {
    #[new]
    pub fn new(value: u64) -> Self {
        U64 { value }
    }

}

pub fn register_u64(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<U64>()?;
    Ok(())
}
