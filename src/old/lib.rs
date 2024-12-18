mod int;
mod I32;

use pyo3::prelude::*;
use pyo3::PyResult;

#[pymodule]
fn rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<int::int>()?;
    m.add_class::<I32::I32>()?;
    m.add_class::<I32::I64>()?;
    Ok(())
}