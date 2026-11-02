//! Dynamic vectorcall sites: functions created after a target was installed (generated
//! `__init__`/`__new__` of dataclasses and namedtuples, `contextmanager` helpers), patched
//! at their first use or by an adoption scan (`TargetSpec::on_install`), held by weak
//! reference, counted in `status()["targets"][i]["dynamic_sites"]` and restored by
//! `disable()`, a suspension and the exit hook.

use pyo3::ffi;
use pyo3::prelude::*;

use super::spec::{Original, TargetSpec};

/// Put `kernel` behind `func`'s vectorcall for target `spec`, which must be installed (and
/// not suspended). `func` must be an exact pure-Python function whose vectorcall is
/// `original`'s (`_PyFunction_Vectorcall`: pass [`super::FUNCTION_VECTORCALL`] after the
/// target captured it with `Prepare::vectorcall`, or [`capture_function_vectorcall`]).
/// Returns whether the site was installed; `false` (nothing changed) when the registry is
/// busy (try again at a later call), the target is not installed, the function is already
/// patched or not a plain function. Uses `PyFunction_SetVectorcall` (stops the world on
/// free-threaded builds). Call on an attached thread, outside watchers and without holding
/// critical sections the caller cannot afford to have suspended; callable from a kernel.
/// The kernel's guards are the target's own (tools, pins): the registry adds none.
pub fn install(
    py: Python<'_>,
    spec: &'static TargetSpec,
    func: &Bound<'_, PyAny>,
    kernel: ffi::vectorcallfunc,
    original: &'static Original<ffi::vectorcallfunc>,
) -> bool {
    if !original.is_set() {
        return false;
    }
    // The tools guard reads the code object's local monitors: calibrated on first use.
    if !super::version::calibrate_monitoring(py) {
        return false;
    }
    super::registry::install_dynamic(py, spec, func, kernel as usize, original.addr())
}

/// Capture `_PyFunction_Vectorcall` into `cell` (for targets that only have dynamic sites).
pub fn capture_function_vectorcall(cell: &'static Original<ffi::vectorcallfunc>) -> bool {
    cell.capture(super::prepare::function_vectorcall()).is_ok()
}
