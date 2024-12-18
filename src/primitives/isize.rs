use pyo3::prelude::*;

#[pyclass(name="isize")]
pub struct ISize {
    value: isize,
}

#[pymethods]
impl ISize {
    #[new]
    pub fn new(value: isize) -> Self {
        ISize { value }
    }

}

pub fn register_isize(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ISize>()?;
    Ok(())
}
