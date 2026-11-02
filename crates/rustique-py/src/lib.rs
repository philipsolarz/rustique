//! The Rustique engine: the extension module `_rustique` (also `rustique._rustique`).
//!
//! Wheels ship it at the top level of site-packages, so that the `.pth` startup line
//! (`python/_rustique_startup.pth`) loads it with one plain import and without the `rustique`
//! package (docs/DECISIONS.md §10.7). Builds that keep maturin's in-package layout (wheels
//! built from the sdist, editable installs) reach the same file through the top-level shim
//! `python/_rustique.py`. Either way the module registers itself under both names.
//!
//! This file only wires modules together; each component registers its own functions.

use pyo3::ffi;
use pyo3::prelude::*;

/// An `extern "C"` block of CPython symbols that pyo3-ffi does not declare. On Windows they
/// are imported from the Python DLL with `raw-dylib`, as PyO3 imports its own (the DLL name
/// comes from `build.rs`); elsewhere the interpreter provides them at load time. Every
/// symbol must be exported by the DLL (`PyAPI_FUNC`/`PyAPI_DATA`) on every supported
/// version, or the extension does not load on Windows. Symbols pyo3-ffi already declares
/// are used through `pyo3::ffi`, not redeclared here.
macro_rules! python_extern {
    ($($body:tt)*) => {
        #[cfg_attr(all(windows, rq_pydll = "python314"), link(name = "python314", kind = "raw-dylib"))]
        #[cfg_attr(all(windows, rq_pydll = "python314t"), link(name = "python314t", kind = "raw-dylib"))]
        #[cfg_attr(all(windows, rq_pydll = "python315"), link(name = "python315", kind = "raw-dylib"))]
        #[cfg_attr(all(windows, rq_pydll = "python315t"), link(name = "python315t", kind = "raw-dylib"))]
        #[cfg_attr(all(windows, rq_pydll = "python316"), link(name = "python316", kind = "raw-dylib"))]
        #[cfg_attr(all(windows, rq_pydll = "python316t"), link(name = "python316t", kind = "raw-dylib"))]
        unsafe extern "C" {
            $($body)*
        }
    };
}

pub mod accel;
pub mod builtins;
pub mod convert;
pub mod patch;
pub mod takeover;
pub mod targets;

/// The names this module is registered under in `sys.modules`, whichever it was loaded as.
const NAMES: [&str; 2] = ["_rustique", "rustique._rustique"];

static MODULE: pyo3::sync::PyOnceLock<Py<PyModule>> = pyo3::sync::PyOnceLock::new();

/// The extension module itself (set at the first module exec; main interpreter only). Never
/// `py.import("rustique._rustique")`: `PyImport_Import` passes an empty fromlist, which
/// imports the parent package `rustique` too (the startup path loads only the extension).
pub fn module(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    MODULE
        .get(py)
        .map(|m| m.bind(py).clone())
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("_rustique is not initialised"))
}

/// PEP 562: the submodules are created on first access. `_accel` (the registry and the
/// switch) is first used by `import rustique` and by the startup path's enable;
/// `_builtins` and `_takeover` serve tests and benchmarks only.
#[pyfunction]
#[pyo3(name = "__getattr__")]
fn module_getattr(py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
    let m = module(py)?;
    match name {
        "_accel" => accel::py::register(&m)?,
        "_builtins" => builtins::register(&m)?,
        "_takeover" => takeover::register(&m)?,
        _ => {}
    }
    match m.dict().get_item(name)? {
        Some(v) => Ok(v.unbind()),
        None => {
            Err(pyo3::exceptions::PyAttributeError::new_err(format!("module '_rustique' has no attribute '{name}'")))
        }
    }
}

/// `dir()` lists the lazy submodules too (creating them).
#[pyfunction]
#[pyo3(name = "__dir__")]
fn module_dir(py: Python<'_>) -> PyResult<Vec<String>> {
    let m = module(py)?;
    for sub in ["_accel", "_builtins", "_takeover"] {
        if !m.dict().contains(sub)? {
            module_getattr(py, sub)?;
        }
    }
    let mut names: Vec<String> = m.dict().keys().iter().map(|k| k.extract()).collect::<PyResult<_>>()?;
    names.sort();
    Ok(names)
}

/// `sys.modules` (a borrowed reference).
fn sys_modules(py: Python<'_>) -> Bound<'_, PyAny> {
    // SAFETY: attached thread; the interpreter's `sys.modules` dict, borrowed.
    unsafe { Bound::from_borrowed_ptr(py, ffi::PyImport_GetModuleDict()) }
}

/// Put `m` into `sys.modules` under both of its names.
fn register_names(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    let modules = sys_modules(py);
    for name in NAMES {
        modules.set_item(name, m)?;
    }
    Ok(())
}

/// `-E`/`-I`: `RUSTIQUE` is ignored, as every `PYTHON*` variable is.
fn ignore_environment(py: Python<'_>) -> bool {
    // SAFETY: borrowed lookup in the sys module dict (NULL if missing).
    let flags = unsafe { ffi::PySys_GetObject(c"flags".as_ptr()) };
    if flags.is_null() {
        return false;
    }
    // SAFETY: a live borrowed object.
    let flags = unsafe { Bound::from_borrowed_ptr(py, flags) };
    flags.getattr("ignore_environment").and_then(|v| v.is_truthy()).unwrap_or(false)
}

/// The `RUSTIQUE=1` startup path: the `.pth` line imports this module when `RUSTIQUE` is set
/// to anything but `0`, and calls this. Never raises.
///
/// The OFF values other than `0` (`off`/`false`/`no`, empty) and `-E`/`-I` leave the process
/// as if nothing had been loaded: the module takes itself out of `sys.modules` again (a later
/// import loads the file again and gets this same module back). Any other value goes to the
/// switch (`accel::switch::startup`): ON values enable, anything else warns.
#[pyfunction]
fn startup(py: Python<'_>) {
    if ignore_environment(py) || accel::switch::killed(py) {
        let modules = sys_modules(py);
        for name in NAMES {
            let _ = modules.del_item(name);
        }
        return;
    }
    accel::switch::startup(py);
}

#[pymodule]
fn _rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    // Refuse every subinterpreter, isolated or legacy shared-GIL (docs/ACCELERATION.md
    // §3.11): PyO3 keeps type objects in process statics, and the accel registry's state
    // (watchers, pins, sites on per-interpreter objects) belongs to the main interpreter.
    // SAFETY: attached thread (module exec).
    if unsafe { ffi::PyInterpreterState_Get() != ffi::PyInterpreterState_Main() } {
        return Err(pyo3::exceptions::PyImportError::new_err("_rustique does not support loading in subinterpreters"));
    }
    accel::guard::set_main_interpreter();
    if let Some(first) = MODULE.get(py) {
        // A second load of the same file (under its other name, or after the startup path
        // took the module out of `sys.modules`). importlib returns what `sys.modules` holds
        // once exec is done, so both names keep meaning the first module: one registry, one
        // `_accel`.
        return register_names(py, first.bind(py));
    }
    let _ = MODULE.set(py, m.clone().unbind());
    // Only what the startup line and `import rustique` need, through the module dict:
    // `PyModule::add` maintains `__all__`, whose first lookup fails, and taking that error
    // makes PyO3 create its `PanicException` type (~40k instructions; atlas w2-startup §1.5).
    let d = m.dict();
    d.set_item(pyo3::intern!(py, "__version__"), env!("CARGO_PKG_VERSION"))?;
    d.set_item(pyo3::intern!(py, "startup"), wrap_pyfunction!(startup, m)?)?;
    d.set_item(pyo3::intern!(py, "__getattr__"), wrap_pyfunction!(module_getattr, m)?)?;
    d.set_item(pyo3::intern!(py, "__dir__"), wrap_pyfunction!(module_dir, m)?)?;
    register_names(py, m)
}
