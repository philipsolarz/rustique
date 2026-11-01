//! `rustique._rustique._accel`: the registry as seen from Python. `rustique._takeover`
//! wraps it into `rustique.enable/disable/status/stats`.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use super::{fingerprint as fp, registry};

/// Install the selected targets (`only`: target names or groups, `None` for every
/// non-hidden target; `exclude`: names or groups). Additive and idempotent.
#[pyfunction]
#[pyo3(signature = (only=None, exclude=None))]
fn enable(py: Python<'_>, only: Option<Vec<String>>, exclude: Option<Vec<String>>) -> PyResult<()> {
    registry::enable(py, only, exclude.unwrap_or_default())
}

/// Restore the matching targets (all when `names` is None).
#[pyfunction]
#[pyo3(signature = (names=None))]
fn disable(py: Python<'_>, names: Option<Vec<String>>) -> PyResult<()> {
    registry::disable(py, names)
}

#[pyfunction]
fn status(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    registry::status(py)
}

#[pyfunction]
#[pyo3(signature = (enabled=None, reset=false))]
fn stats(py: Python<'_>, enabled: Option<bool>, reset: bool) -> PyResult<Bound<'_, PyDict>> {
    registry::stats(py, enabled, reset)
}

/// Names of installed targets among `names` (all when None).
#[pyfunction]
#[pyo3(signature = (names=None))]
fn installed(py: Python<'_>, names: Option<Vec<String>>) -> PyResult<Vec<&'static str>> {
    registry::installed(py, names.as_deref())
}

/// `[(name, group, module)]` of every registered target (hidden ones included).
#[pyfunction]
fn targets(py: Python<'_>) -> PyResult<Vec<Bound<'_, PyTuple>>> {
    registry::all_specs().iter().map(|s| (s.name, s.group, s.module).into_pyobject(py)).collect::<PyResult<Vec<_>>>()
}

/// Fingerprint of a Python function / code object / C function or method descriptor.
#[pyfunction]
fn fingerprint(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    fp::any(obj)
}

/// Dev tool: fingerprints every selected target checks, observed vs recorded.
#[pyfunction]
#[pyo3(signature = (only=None))]
fn fingerprint_report(py: Python<'_>, only: Option<Vec<String>>) -> PyResult<Vec<registry::FingerprintRow>> {
    registry::fingerprint_report(py, only)
}

/// Pins of an installed target: `[(key, absent, intact, broken)]`.
#[pyfunction]
fn pins(py: Python<'_>, name: &str) -> PyResult<Vec<(String, bool, bool, bool)>> {
    registry::pins(py, name)
}

/// Check mode: `None`/`"off"` off, `"warn"` (count mismatches, return the original's
/// result), `"raise"` (raise `RustiqueCheckError`). Returns the previous mode.
#[pyfunction]
#[pyo3(signature = (mode=None))]
fn check(mode: Option<&str>) -> PyResult<Option<&'static str>> {
    let prev = mode_name(super::stats::check_mode());
    super::stats::set_check(match mode {
        None | Some("off") => None,
        Some("warn") => Some(false),
        Some("raise") => Some(true),
        Some(m) => {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "check mode must be None, 'off', 'warn' or 'raise', not {m:?}"
            )));
        }
    });
    Ok(prev)
}

/// The current check mode (`None`, `"warn"`, `"raise"`).
#[pyfunction]
fn check_mode() -> Option<&'static str> {
    mode_name(super::stats::check_mode())
}

fn mode_name(m: Option<bool>) -> Option<&'static str> {
    m.map(|raise| if raise { "raise" } else { "warn" })
}

/// `func_version` of a function (0 unset, 1 cleared, >= 2 valid): tests of uninstall.
#[pyfunction]
fn func_version(func: &Bound<'_, pyo3::types::PyFunction>) -> u32 {
    // SAFETY: an exact function object (type-checked by the signature).
    unsafe { (*func.as_ptr().cast::<pyo3::ffi::PyFunctionObject>()).func_version }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = PyModule::new(m.py(), "_accel")?;
    sub.add_function(wrap_pyfunction!(enable, &sub)?)?;
    sub.add_function(wrap_pyfunction!(disable, &sub)?)?;
    sub.add_function(wrap_pyfunction!(status, &sub)?)?;
    sub.add_function(wrap_pyfunction!(stats, &sub)?)?;
    sub.add_function(wrap_pyfunction!(installed, &sub)?)?;
    sub.add_function(wrap_pyfunction!(targets, &sub)?)?;
    sub.add_function(wrap_pyfunction!(fingerprint, &sub)?)?;
    sub.add_function(wrap_pyfunction!(fingerprint_report, &sub)?)?;
    sub.add_function(wrap_pyfunction!(pins, &sub)?)?;
    sub.add_function(wrap_pyfunction!(func_version, &sub)?)?;
    sub.add_function(wrap_pyfunction!(check, &sub)?)?;
    sub.add_function(wrap_pyfunction!(check_mode, &sub)?)?;
    sub.add("RustiqueCheckError", m.py().get_type::<super::check::RustiqueCheckError>())?;
    sub.add("FREE_THREADED", super::version::FREE_THREADED)?;
    crate::targets::register(&sub)?;
    m.add("_accel", sub)
}
