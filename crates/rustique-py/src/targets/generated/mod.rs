//! Group `generated`: methods CPython generates as Python source at class creation —
//! `collections.namedtuple`'s `__new__`/`_make`/`_replace` and the frozen `__init__` of
//! `@dataclass` — as vectorcall kernels installed on each generated function (dynamic
//! sites) by a hook on the generator. See docs/targets/datamodel.md.
//!
//! OWNER: datamodel workstream. `common` and `adapter` are shared with the groups `enum`,
//! `contextlib` and `functools`.

use pyo3::prelude::*;

use crate::accel::TargetSpec;

pub mod adapter;
pub mod common;
pub mod dataclass;
pub mod namedtuple;

pub static TARGETS: &[&TargetSpec] = &[&namedtuple::TARGET, &dataclass::TARGET];

/// `_accel.datamodel_dynamic_sites()`: live functions carrying a kernel, per target
/// (tests and diagnostics).
#[pyfunction]
fn datamodel_dynamic_sites(py: Python<'_>) -> PyResult<Bound<'_, pyo3::types::PyDict>> {
    let d = pyo3::types::PyDict::new(py);
    d.set_item(namedtuple::TARGET.name, namedtuple::dynamic_sites())?;
    d.set_item(dataclass::TARGET.name, dataclass::dynamic_sites())?;
    d.set_item(crate::targets::contextlib::TARGET.name, crate::targets::contextlib::dynamic_sites())?;
    Ok(d)
}

pub fn register(accel: &Bound<'_, PyModule>) -> PyResult<()> {
    accel.add_function(wrap_pyfunction!(datamodel_dynamic_sites, accel)?)?;
    Ok(())
}
