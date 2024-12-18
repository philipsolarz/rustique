use pyo3::prelude::*;

#[pyclass(name="u128")]
pub struct U128 {
    value: u128,
}

#[pymethods]
impl U128 {
    #[new]
    pub fn new(value: u128) -> Self {
        U128 { value }
    }

}

pub fn register_u128(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<U128>()?;
    Ok(())
}
