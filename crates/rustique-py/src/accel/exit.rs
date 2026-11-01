//! Interpreter exit: the registry's atexit hook.
//!
//! Installed targets hold strong references (patched functions, pinned values, watched
//! dicts, verified objects) and watch module dicts. Left to finalization, those references
//! keep module objects alive past the garbage collection that would reclaim them (CPython
//! then empties their dicts one key at a time) and every key cleared from a watched dict
//! is a watcher event: ~0.75M Ir per process for the paths groups. The hook runs with the
//! interpreter intact (Python `atexit` callbacks run before finalization starts) and
//! releases everything (`registry::teardown`).
//!
//! It is registered by the first `enable()` without importing `atexit` (~110k Ir on 3.15):
//! `atexit.register`'s C function is taken from the builtin module's definition and
//! appends to the interpreter's one callback list like `import atexit` would (fallback: a
//! private instance of the builtin module from `_imp.create_builtin`). Callbacks run last
//! in, first out: anything registered after the first `enable()` still sees the kernels.
//! If the hook cannot be registered (or `atexit._clear()` drops it), the dict watcher stops
//! at `Py_IsFinalizing` instead (flags cleared once, no further work), and the references
//! are only released with the interpreter.

use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;

static HOOKED: AtomicBool = AtomicBool::new(false);

/// Register the exit hook (once; again after it ran). A failure is not an error of the
/// caller: the next `enable()` retries.
pub(crate) fn ensure_hook(py: Python<'_>) {
    if HOOKED.swap(true, Ordering::AcqRel) {
        return;
    }
    if register(py).is_err() {
        HOOKED.store(false, Ordering::Release);
    }
}

/// Is the hook registered (and not yet run)?
pub fn hooked() -> bool {
    HOOKED.load(Ordering::Acquire)
}

fn register(py: Python<'_>) -> PyResult<()> {
    let hook = wrap_pyfunction!(exit_hook, py)?;
    if let Some(r) = register_direct(py, hook.as_any()) {
        return r;
    }
    atexit_module(py)?.call_method1(pyo3::intern!(py, "register"), (hook,))?;
    Ok(())
}

/// `atexit.register`'s C function, found in the builtin module's definition
/// (`PyImport_Inittab` → `PyInit_atexit` → `m_methods`), called without a module object:
/// it registers into the interpreter's state and ignores its module argument (3.14, 3.15:
/// `atexit_register` uses `get_atexit_state()` only). ~2k Ir. `None` when the definition
/// is not the expected shape (then a module instance is used).
fn register_direct(py: Python<'_>, hook: &Bound<'_, PyAny>) -> Option<PyResult<()>> {
    // SAFETY: attached thread; the inittab is a NULL-terminated static array, the init
    // function of a multi-phase builtin returns its static `PyModuleDef` (no module is
    // created), and the method table is NULL-terminated.
    unsafe {
        let mut entry = ffi::PyImport_Inittab;
        if entry.is_null() {
            return None;
        }
        while !(*entry).name.is_null() && std::ffi::CStr::from_ptr((*entry).name) != c"atexit" {
            entry = entry.add(1);
        }
        let init = if (*entry).name.is_null() { None } else { (*entry).initfunc }?;
        let def = init();
        if def.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        if ffi::PyObject_TypeCheck(def, &raw mut ffi::PyModuleDef_Type) == 0 {
            // A single-phase module object (a new reference): not the expected shape.
            ffi::Py_DECREF(def);
            return None;
        }
        let mut m = (*def.cast::<ffi::PyModuleDef>()).m_methods;
        while !m.is_null() && !(*m).ml_name.is_null() {
            if std::ffi::CStr::from_ptr((*m).ml_name) == c"register" {
                if (*m).ml_flags != ffi::METH_VARARGS | ffi::METH_KEYWORDS {
                    return None;
                }
                let f = (*m).ml_meth.PyCFunctionWithKeywords;
                let args = match pyo3::types::PyTuple::new(py, [hook]) {
                    Ok(a) => a,
                    Err(e) => return Some(Err(e)),
                };
                let r = f(std::ptr::null_mut(), args.as_ptr(), std::ptr::null_mut());
                return Some(Bound::from_owned_ptr_or_err(py, r).map(drop));
            }
            m = m.add(1);
        }
        None
    }
}

/// An `atexit` module: the imported one if any, else a private instance of the builtin
/// module (no import system: `_imp` is always loaded). Every instance registers into the
/// interpreter's callback list.
fn atexit_module(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    let modules = super::prepare::sys_modules(py)
        .map_err(|r| pyo3::exceptions::PyRuntimeError::new_err(r.reason().to_owned()))?;
    if let Some(m) = modules.get_item(pyo3::intern!(py, "atexit"))? {
        return Ok(m);
    }
    let imp = modules
        .get_item(pyo3::intern!(py, "_imp"))?
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("_imp is not loaded"))?;
    // A spec needs only `name`: a namespace of `sys.implementation`'s type.
    // SAFETY: a borrowed reference owned by the sys module, or NULL (no exception).
    let implementation = unsafe { ffi::PySys_GetObject(c"implementation".as_ptr()) };
    if implementation.is_null() {
        return Err(pyo3::exceptions::PyRuntimeError::new_err("sys.implementation is missing"));
    }
    // SAFETY: a live object (see above).
    let namespace = unsafe { Bound::from_borrowed_ptr(py, implementation) }.get_type();
    let kw = PyDict::new(py);
    kw.set_item(pyo3::intern!(py, "name"), pyo3::intern!(py, "atexit"))?;
    let spec = namespace.call((), Some(&kw))?;
    let module = imp.call_method1(pyo3::intern!(py, "create_builtin"), (&spec,))?;
    imp.call_method1(pyo3::intern!(py, "exec_builtin"), (&module,))?;
    Ok(module)
}

/// The atexit callback: write a pending `RUSTIQUE_REPORT` first (it describes the
/// installed state), then release everything.
#[pyfunction]
#[pyo3(name = "_exit_hook")]
pub fn exit_hook(py: Python<'_>) {
    HOOKED.store(false, Ordering::Release);
    if let Err(e) = super::switch::report_at_exit(py) {
        e.write_unraisable(py, None);
    }
    if let Err(e) = super::registry::teardown(py) {
        e.write_unraisable(py, None);
    }
}
