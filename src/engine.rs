use pyo3::prelude::*;

#[pyclass(name = "Rustique")]
pub struct Engine {}

#[pymethods]
impl Engine {
    #[new]
    pub fn new() -> Self {
        Engine {}
    }

    pub fn __enter__(&self) -> Self {
        println!("Entering Engine");
        Engine {}
    }

    pub fn __exit__(
        &self,
        _exc_type: PyObject,
        _exc_value: PyObject,
        _traceback: PyObject,
    ) -> bool {
        println!("Exiting Engine");
        true
    }
}

#[pymodule]
pub fn register_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Engine>()?;
    Ok(())
}
