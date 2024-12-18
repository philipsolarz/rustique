use pyo3::prelude::*;

#[pyclass(name="usize")]
pub struct Usize {
    value: usize,
}

#[pymethods]
impl Usize {
    #[new]
    pub fn new(value: usize) -> Self {
        Usize { value }
    }

}

pub fn register_usize(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Usize>()?;
    Ok(())
}
