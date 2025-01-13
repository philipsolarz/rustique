use pyo3::{prelude::*, types::PyFloat};

#[pyclass]
pub struct Float {
    float: Py<PyFloat>,
}

#[pymethods]
impl Float {
    #[new]
    pub fn new(float: Py<PyFloat>) -> Self {
        Self { float }
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("Float({})", self.float.to_string()))
    }
}

#[pymodule]
pub fn register_float(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Float>()?;
    Ok(())
}
