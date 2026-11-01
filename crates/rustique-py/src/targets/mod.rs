//! Accelerated targets, one module per group (docs/ACCELERATION.md §9).
//!
//! To register a group: add `pub mod <group>;` below and its `TARGETS` slice to [`ALL`].
//! Each group's `TARGETS` is a `&'static [&'static TargetSpec]`.

use pyo3::prelude::*;

use crate::accel::TargetSpec;

pub mod _selftest;
pub mod builtins;
pub mod containers;
pub mod logging;

/// Every group's targets, in install order.
pub static ALL: &[&[&TargetSpec]] = &[
    containers::TARGETS,
    builtins::TARGETS,
    logging::TARGETS,
    // Hidden (group `_selftest`): exercise the framework in tests; never enabled unless
    // named in `enable(only=...)`.
    _selftest::TARGETS,
];

/// Python-visible helpers of targets (registered on `rustique._rustique._accel`).
pub fn register(accel: &Bound<'_, PyModule>) -> PyResult<()> {
    _selftest::register(accel)?;
    logging::register(accel)
}
