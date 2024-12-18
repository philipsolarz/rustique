use pyo3::prelude::*;

/// A simple Rust-backed f64 type exposed to Python
#[pyclass(name="f64")]
pub struct F64 {
    value: f64,
}

#[pymethods]
impl F64 {
    #[new]
    pub fn new(value: f64) -> Self {
        F64 { value }
    }

}

/// Register f64 with the root module
pub fn register_f64(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<F64>()?;
    Ok(())
}
