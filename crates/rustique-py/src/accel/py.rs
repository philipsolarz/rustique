//! `rustique._rustique._accel`: the switch primitives (docs/ACCELERATION.md §6, §8). The
//! public `rustique.enable/disable/status/stats/check` are thin wrappers around `enable`,
//! `disable`, `status`, `stats`, `check` here, so the startup path imports nothing but the
//! package and this extension.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use super::{fingerprint as fp, registry};

/// Names given as a list/tuple/iterable of str, or one comma-separated str.
fn names(v: Option<&Bound<'_, PyAny>>) -> PyResult<Option<Vec<String>>> {
    let Some(v) = v else { return Ok(None) };
    if v.is_none() {
        return Ok(None);
    }
    let raw: Vec<String> = if let Ok(s) = v.extract::<String>() {
        s.split(',').map(str::to_owned).collect()
    } else {
        v.try_iter()?.map(|x| x?.extract::<String>()).collect::<PyResult<_>>()?
    };
    Ok(Some(raw.into_iter().map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).collect()))
}

/// The switch: install the selected targets. `only`: groups or target names (a list or
/// `"a,b"`; `None`: every non-hidden target); `exclude`: the same; `builtins=False`
/// excludes the `builtins` group. Additive and idempotent; unknown names raise
/// `ValueError` before anything changes. Returns None, or `status()` with `quiet=False`
/// (building the status dict costs ~13k Ir per target: only on request). Reads no
/// environment variable and imports nothing. `tiers`: opt-in tiers to install too
/// (`["interp", "gc"]`; added to those already requested; unknown tiers raise).
#[pyfunction]
#[pyo3(signature = (only=None, exclude=None, builtins=true, quiet=true, tiers=None))]
fn enable<'py>(
    py: Python<'py>,
    only: Option<&Bound<'py, PyAny>>,
    exclude: Option<&Bound<'py, PyAny>>,
    builtins: bool,
    quiet: bool,
    tiers: Option<&Bound<'py, PyAny>>,
) -> PyResult<Option<Bound<'py, PyDict>>> {
    let mut excl = names(exclude)?.unwrap_or_default();
    if !builtins {
        excl.push("builtins".to_owned());
    }
    registry::enable_tiers(py, names(only)?, excl, names(tiers)?.unwrap_or_default())?;
    if quiet { Ok(None) } else { status(py).map(Some) }
}

/// Restore the matching targets (all when `names` is None). Returns None, or `status()`
/// with `quiet=False`.
#[pyfunction]
#[pyo3(signature = (names=None, quiet=true))]
fn disable<'py>(
    py: Python<'py>,
    names: Option<&Bound<'py, PyAny>>,
    quiet: bool,
) -> PyResult<Option<Bound<'py, PyDict>>> {
    let all = names.is_none_or(|n| n.is_none());
    registry::disable(py, self::names(names)?)?;
    if all {
        super::switch::reset_source();
    }
    if quiet { Ok(None) } else { status(py).map(Some) }
}

/// `{"targets": [...], "process": {...}}` (docs/ACCELERATION.md §6.2), including the
/// switch state (`process["switch"]`) and `process["free_threaded"]`.
#[pyfunction]
pub(crate) fn status(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let st = registry::status(py)?;
    let process = st
        .get_item("process")?
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("status without process"))?
        .cast_into::<PyDict>()?;
    super::switch::describe(py, &process)?;
    Ok(st)
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

/// Check mode: `None`/`False`/`"off"` off, `"warn"` (count mismatches, return the
/// original's result), `"raise"` (raise `RustiqueCheckError`). Returns the previous mode.
#[pyfunction]
#[pyo3(signature = (mode=None))]
fn check(mode: Option<&Bound<'_, PyAny>>) -> PyResult<Option<&'static str>> {
    let prev = mode_name(super::stats::check_mode());
    let m: Option<String> = match mode {
        None => None,
        Some(v) if v.is_none() || v.is(pyo3::types::PyBool::new(v.py(), false)) => None,
        Some(v) => Some(v.extract::<String>()?),
    };
    super::stats::set_check(match m.as_deref() {
        None | Some("off") => None,
        Some("warn") => Some(false),
        Some("raise") => Some(true),
        Some(m) => {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "check mode must be None, False, 'off', 'warn' or 'raise', not {m:?}"
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

/// Whether a pure-Python function's vectorcall is CPython's own (`_PyFunction_Vectorcall`).
#[pyfunction]
fn vectorcall_is_original(func: &Bound<'_, pyo3::types::PyFunction>) -> bool {
    // SAFETY: an exact function object (type-checked by the signature).
    let vc = unsafe { (*func.as_ptr().cast::<pyo3::ffi::PyFunctionObject>()).vectorcall };
    vc.map_or(0, |f| f as usize) == super::prepare::function_vectorcall()
}

/// Diagnostics: run framework primitive `kind` on `obj` `n` times (for callgrind regions):
/// "fingerprint", "fingerprint_c", "layout", "describe".
#[pyfunction]
fn _bench(kind: &str, obj: &Bound<'_, PyAny>, n: usize) -> PyResult<()> {
    for _ in 0..n {
        match kind {
            "fingerprint" => {
                fp::clear_cache();
                fp::function(obj)?;
            }
            "fingerprint_c" => {
                fp::c_object(obj)?;
            }
            "layout" => {
                super::version::code_layout_ok_for(&obj.getattr("__code__")?)?;
            }
            _ => return Err(pyo3::exceptions::PyValueError::new_err("unknown kind")),
        }
    }
    Ok(())
}

/// `func_version` of a function (0 unset, 1 cleared, >= 2 valid): tests of uninstall.
#[pyfunction]
fn func_version(func: &Bound<'_, pyo3::types::PyFunction>) -> u32 {
    // SAFETY: an exact function object (type-checked by the signature).
    unsafe { (*func.as_ptr().cast::<pyo3::ffi::PyFunctionObject>()).func_version }
}

/// Lock-order violations of the pin tables' leaf rule seen in this process (tests assert 0).
#[pyfunction]
fn _lock_violations() -> u64 {
    super::guard::lock_violations()
}

/// Is the exit hook registered (tests)?
#[pyfunction]
fn exit_hook_registered() -> bool {
    super::exit::hooked()
}

static EXTRAS: pyo3::sync::PyOnceLock<()> = pyo3::sync::PyOnceLock::new();

/// Diagnostics, the self-test targets' helpers and `RustiqueCheckError`: created on first
/// access (startup: `import rustique; rustique.enable()` needs only the switch).
fn add_extras(sub: &Bound<'_, PyModule>) -> PyResult<()> {
    EXTRAS.get_or_try_init(sub.py(), || -> PyResult<()> {
        sub.add_function(wrap_pyfunction!(targets, sub)?)?;
        sub.add_function(wrap_pyfunction!(fingerprint, sub)?)?;
        sub.add_function(wrap_pyfunction!(fingerprint_report, sub)?)?;
        sub.add_function(wrap_pyfunction!(pins, sub)?)?;
        sub.add_function(wrap_pyfunction!(func_version, sub)?)?;
        sub.add_function(wrap_pyfunction!(_bench, sub)?)?;
        sub.add_function(wrap_pyfunction!(vectorcall_is_original, sub)?)?;
        sub.add_function(wrap_pyfunction!(super::exit::exit_hook, sub)?)?;
        sub.add_function(wrap_pyfunction!(exit_hook_registered, sub)?)?;
        sub.add_function(wrap_pyfunction!(_lock_violations, sub)?)?;
        sub.add("RustiqueCheckError", sub.py().get_type::<super::check::RustiqueCheckError>())?;
        crate::targets::register(sub)
    })?;
    Ok(())
}

/// PEP 562 for `_accel`: the extras on first access.
#[pyfunction]
#[pyo3(name = "__getattr__")]
fn accel_getattr(py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
    let sub = crate::module(py)?.getattr("_accel")?.cast_into::<PyModule>()?;
    if !(name.starts_with("__") && name.ends_with("__")) {
        add_extras(&sub)?;
        if let Some(v) = sub.dict().get_item(name)? {
            return Ok(v.unbind());
        }
    }
    Err(pyo3::exceptions::PyAttributeError::new_err(format!(
        "module 'rustique._rustique._accel' has no attribute '{name}'"
    )))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = PyModule::new(m.py(), "_accel")?;
    // Through the module dict, not `PyModule::add`: `add` maintains `__all__`, whose first
    // lookup raises AttributeError, and PyO3's `PyErr::take` then creates its
    // `PanicException` type (~40k instructions at startup; docs/atlas/w2-startup.md §1.5).
    let d = sub.dict();
    macro_rules! set {
        ($name:literal, $f:path) => {
            d.set_item(pyo3::intern!(m.py(), $name), wrap_pyfunction!($f, &sub)?)?
        };
    }
    set!("enable", enable);
    set!("disable", disable);
    set!("status", status);
    set!("stats", stats);
    set!("installed", installed);
    set!("check", check);
    set!("check_mode", check_mode);
    set!("__getattr__", accel_getattr);
    set!("startup", super::switch::startup);
    set!("switch_enable", super::switch::py_switch_enable);
    set!("killed", super::switch::killed);
    set!("report_to", super::switch::report_to);
    set!("_record", super::switch::record);
    set!("_report_at_exit", super::switch::report_at_exit);
    d.set_item("FREE_THREADED", super::version::FREE_THREADED)?;
    m.dict().set_item("_accel", sub)
}
