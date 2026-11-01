//! Accelerated targets, one module per group (docs/ACCELERATION.md §9).
//!
//! To register a group: add `pub mod <group>;` below and its `TARGETS` slice to [`ALL`].
//! Each group's `TARGETS` is a `&'static [&'static TargetSpec]`.

use pyo3::prelude::*;

use crate::accel::TargetSpec;

pub mod _selftest;
pub mod builtins;
pub mod containers;
pub mod copy;
pub mod datetime;
pub mod float;
pub mod fnmatch;
pub mod json;
pub mod logging;
pub mod pathlib;
pub mod posixpath;
pub mod random;
pub mod statistics;
pub mod textwrap;
pub mod urllib;
pub mod utils_kit;

/// Every group's targets, in install order.
pub static ALL: &[&[&TargetSpec]] = &[
    containers::TARGETS,
    builtins::TARGETS,
    float::TARGETS,
    json::TARGETS,
    logging::TARGETS,
    random::TARGETS,
    statistics::TARGETS,
    textwrap::TARGETS,
    urllib::TARGETS,
    posixpath::TARGETS,
    fnmatch::TARGETS,
    pathlib::TARGETS,
    datetime::TARGETS,
    copy::TARGETS,
    copy::dc::TARGETS,
    // Hidden (group `_selftest`): exercise the framework in tests; never enabled unless
    // named in `enable(only=...)`.
    _selftest::TARGETS,
];

/// Python-visible helpers of targets (registered on `rustique._rustique._accel`).
pub fn register(accel: &Bound<'_, PyModule>) -> PyResult<()> {
    float::register(accel)?;
    json::register(accel)?;
    _selftest::register(accel)?;
    logging::register(accel)?;
    posixpath::register(accel)?;
    fnmatch::register(accel)?;
    datetime::register(accel)?;
    copy::register(accel)?;
    Ok(())
}
