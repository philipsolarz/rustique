use pyo3::prelude::*;

#[pyclass(name="char")]
pub struct Char {
    value: char,
}

#[pymethods]
impl Char {
    #[new]
    pub fn new(value: char) -> Self {
        Char { value }
    }

}

pub fn register_char(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Char>()?;
    Ok(())
}
