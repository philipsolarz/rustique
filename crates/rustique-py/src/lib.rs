//! Python bindings for Rustique: module `rustique._rustique`.
//!
//! This file only wires modules together; each component registers its own classes.

use pyo3::ffi;
use pyo3::prelude::*;

pub mod accel;
pub mod builtins;
pub mod capi;
pub mod convert;
pub mod dict;
pub mod expr;
pub mod frozenset;
pub mod list;
pub mod objsort;
pub mod patch;
pub mod scalars;
pub mod schema;
pub mod set;
pub mod storage;
pub mod sync;
pub mod takeover;
pub mod targets;
pub mod tuple;
pub mod typing;

#[pymodule]
fn _rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Refuse every subinterpreter, isolated or legacy shared-GIL (docs/ACCELERATION.md
    // §3.11): PyO3 keeps type objects in process statics, and the accel registry's state
    // (watchers, pins, sites on per-interpreter objects) belongs to the main interpreter.
    // SAFETY: attached thread (module exec).
    if unsafe { ffi::PyInterpreterState_Get() != ffi::PyInterpreterState_Main() } {
        return Err(pyo3::exceptions::PyImportError::new_err(
            "rustique._rustique does not support loading in subinterpreters",
        ));
    }
    accel::guard::set_main_interpreter();
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    scalars::register(m)?;
    typing::register(m)?;
    list::register(m)?;
    tuple::register(m)?;
    dict::register(m)?;
    set::register(m)?;
    frozenset::register(m)?;
    expr::register(m)?;
    capi::register(m)?;
    builtins::register(m)?;
    takeover::register(m)?;
    accel::py::register(m)?;
    Ok(())
}
