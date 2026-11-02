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
    d.set_item(crate::targets::functools::DISPATCH.name, crate::targets::functools::dynamic_sites())?;
    Ok(d)
}

/// `_accel.datamodel_generic_alloc(cls)`: the class allocates with `PyType_GenericAlloc`
/// (not routed to the dataclass `__init__` kernel) — tests.
#[pyfunction]
fn datamodel_generic_alloc(cls: &Bound<'_, pyo3::types::PyType>) -> bool {
    // SAFETY: a live type object.
    unsafe { (*cls.as_type_ptr()).tp_alloc.map_or(0, |f| f as usize) == pyo3::ffi::PyType_GenericAlloc as *const () as usize }
}

pub fn register(accel: &Bound<'_, PyModule>) -> PyResult<()> {
    accel.add_function(wrap_pyfunction!(datamodel_dynamic_sites, accel)?)?;
    accel.add_function(wrap_pyfunction!(datamodel_generic_alloc, accel)?)?;
    Ok(())
}
