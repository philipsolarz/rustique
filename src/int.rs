use pyo3::{prelude::*, types::PyInt};

#[pyclass]
pub struct Int {
    int: Py<PyInt>,
}

#[pymethods]
impl Int {
    #[new]
    pub fn new(int: Py<PyInt>) -> Self {
        Self { int }
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("Int({})", self.int.to_string()))
    }
}

#[pymodule]
pub fn register_int(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Int>()?;
    Ok(())
}
