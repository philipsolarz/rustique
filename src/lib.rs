use pyo3::prelude::*;

// mod dict;
// mod engine;
// mod list;

mod bool;
mod float;
mod int;
mod str;

// mod engine2;

// mod context_manager;
// mod memory;
// mod optimized_memory;
// mod rust_types;
// mod rust_types_advanced;
// mod rustique_wrapper;

// mod int2;

#[pymodule]
fn rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // list::register_list(m)?;
    // engine::register_engine(m)?;
    // dict::register_dict(m)?;

    int::register_int(m)?;
    float::register_float(m)?;
    str::register_str(m)?;

    // engine2::register_engine2(m)?;

    // memory::register_memory(m)?;
    // rust_types::register_rust_types(m)?;
    // rust_types_advanced::register_rust_types(m)?;
    // context_manager::register_context_manager(m)?;
    // rustique_wrapper::register_rustique_wrapper(m)?;
    // optimized_memory::register_optimized_memory(m)?;

    // int2::register_int2(m)?;
    Ok(())
}
