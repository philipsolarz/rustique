use pyo3::prelude::*;

// mod dict;
// mod engine;
// mod list;

// mod float;
// mod int;
// mod str;

mod engine2;

#[pymodule]
fn rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // list::register_list(m)?;
    // engine::register_engine(m)?;
    // dict::register_dict(m)?;

    // int::register_int(m)?;
    // float::register_float(m)?;
    // str::register_str(m)?;

    engine2::register_engine2(m)?;
    Ok(())
}
