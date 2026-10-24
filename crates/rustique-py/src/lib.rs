//! Python bindings for Rustique: module `rustique._rustique`.
//!
//! This file only wires modules together; each component registers its own classes.

use pyo3::prelude::*;

pub mod convert;
pub mod dict;
pub mod expr;
pub mod list;
pub mod scalars;
pub mod schema;
pub mod set;
pub mod storage;
pub mod sync;
pub mod typing;

#[pymodule]
fn _rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    scalars::register(m)?;
    typing::register(m)?;
    list::register(m)?;
    dict::register(m)?;
    set::register(m)?;
    expr::register(m)?;
    Ok(())
}
