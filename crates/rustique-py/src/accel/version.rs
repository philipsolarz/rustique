//! Build fingerprint: what interpreter are we running in, and does it match a build the
//! kernels were validated on? `enable()` refuses per target when it does not.
//!
//! An extension built for `cpython-315-x86_64-linux-gnu` only loads into 3.15 release
//! builds with the same ABI flags (`t` for free-threaded; debug builds use a `d` suffix),
//! so the loader already rejects 3.16 and debug builds. What it does not reject: other
//! 3.15.x patch releases, a 3.15 built from source with different configure options
//! (`--with-pydebug` without `d` is impossible, but `--enable-optimizations`, tracerefs,
//! `--with-trace-refs`, assertions, `Py_STATS`), and a build of this crate against a
//! 3.16 alpha. Those are checked here at run time.

use pyo3::ffi;
use pyo3::prelude::*;

/// `PyCodeObject` prefix up to `co_version` (cpython/code.h, identical in 3.14 and 3.15;
/// calibrated at run time by [`code_layout_ok`]).
#[repr(C)]
pub struct RawCode {
    pub ob_base: ffi::PyVarObject,
    pub co_consts: *mut ffi::PyObject,
    pub co_names: *mut ffi::PyObject,
    pub co_exceptiontable: *mut ffi::PyObject,
    pub co_flags: i32,
    pub co_argcount: i32,
    pub co_posonlyargcount: i32,
    pub co_kwonlyargcount: i32,
    pub co_stacksize: i32,
    pub co_firstlineno: i32,
    pub co_nlocalsplus: i32,
    pub co_framesize: i32,
    pub co_nlocals: i32,
    pub co_ncellvars: i32,
    pub co_nfreevars: i32,
    pub co_version: u32,
    pub co_localsplusnames: *mut ffi::PyObject,
    pub co_localspluskinds: *mut ffi::PyObject,
    pub co_filename: *mut ffi::PyObject,
    pub co_name: *mut ffi::PyObject,
    pub co_qualname: *mut ffi::PyObject,
    pub co_linetable: *mut ffi::PyObject,
    pub co_weakreflist: *mut ffi::PyObject,
    pub co_executors: *mut std::ffi::c_void,
    pub _co_cached: *mut std::ffi::c_void,
    pub _co_instrumentation_version: usize,
    /// `_PyCoMonitoringData *`: NULL until a tool instruments this code object; its first
    /// member is `local_monitors` (one byte per local event: the tools listening locally).
    pub _co_monitoring: *mut u8,
}

/// `Py_Version` (`unsigned long`: 64-bit on Linux/macOS, 32-bit on Windows).
#[allow(clippy::useless_conversion)]
fn runtime_version() -> u64 {
    // SAFETY: exported data symbol, immutable after startup.
    u64::from(unsafe { ffi::Py_Version })
}

/// The minor version this extension was compiled for (pyo3 cfgs).
pub const fn compiled_minor() -> u8 {
    if cfg!(Py_3_16) {
        16
    } else if cfg!(Py_3_15) {
        15
    } else {
        14
    }
}

/// Check [`RawCode`] against the attributes CPython reports for a probe function.
pub fn code_layout_ok(py: Python<'_>) -> PyResult<bool> {
    let ns = pyo3::types::PyDict::new(py);
    py.run(c"def _probe(a, b, /, c, *, d, e=1):\n    x = a\n    return x\n", None, Some(&ns))?;
    let f = ns.get_item("_probe")?.expect("probe defined");
    let code = f.getattr("__code__")?;
    // SAFETY: a live code object; reads within the documented prefix.
    let raw = unsafe { &*code.as_ptr().cast::<RawCode>() };
    Ok(raw.co_argcount == code.getattr("co_argcount")?.extract::<i32>()?
        && raw.co_posonlyargcount == code.getattr("co_posonlyargcount")?.extract::<i32>()?
        && raw.co_kwonlyargcount == code.getattr("co_kwonlyargcount")?.extract::<i32>()?
        && raw.co_flags == code.getattr("co_flags")?.extract::<i32>()?
        && raw.co_firstlineno == code.getattr("co_firstlineno")?.extract::<i32>()?
        && raw.co_nlocals == code.getattr("co_nlocals")?.extract::<i32>()?
        && raw.co_consts == code.getattr("co_consts")?.as_ptr()
        && raw.co_filename == code.getattr("co_filename")?.as_ptr()
        && raw.co_name == code.getattr("co_name")?.as_ptr()
        && raw.co_qualname == code.getattr("co_qualname")?.as_ptr())
}

/// Facts about the running interpreter, compared with the build the extension was compiled
/// for. All checks are cheap (< 50 µs total, measured by `proto.fingerprint`).
pub fn fingerprint(py: Python<'_>) -> PyResult<Vec<(&'static str, String)>> {
    let sys = py.import("sys")?;
    let mut out = Vec::new();
    let runtime = runtime_version();
    out.push(("runtime_hexversion", format!("{runtime:#x}")));
    out.push(("compiled_for_minor", compiled_minor().to_string()));
    out.push(("abiflags", sys.getattr("abiflags")?.extract::<String>()?));
    out.push(("debug_build", sys.hasattr("gettotalrefcount")?.to_string()));
    out.push(("free_threaded_compiled", cfg!(Py_GIL_DISABLED).to_string()));
    let gil = match sys.getattr("_is_gil_enabled") {
        Ok(f) => f.call0()?.extract::<bool>()?.to_string(),
        Err(_) => "n/a".into(),
    };
    out.push(("gil_enabled_runtime", gil));
    out.push(("cache_tag", sys.getattr("implementation")?.getattr("cache_tag")?.extract::<String>()?));
    out.push(("releaselevel", sys.getattr("version_info")?.getattr("releaselevel")?.extract::<String>()?));
    // Layout facts the kernels rely on.
    let ty = |name: &str| -> PyResult<isize> {
        py.eval(&std::ffi::CString::new(format!("{name}.__basicsize__"))?, None, None)?.extract()
    };
    out.push(("object.__basicsize__", ty("object")?.to_string()));
    out.push(("str.__basicsize__", ty("str")?.to_string()));
    out.push(("int.__basicsize__", ty("int")?.to_string()));
    out.push(("type.__basicsize__", ty("type")?.to_string()));
    let func_basic: isize = py.eval(c"(lambda: 0).__class__.__basicsize__", None, None)?.extract()?;
    out.push(("function.__basicsize__", func_basic.to_string()));
    out.push(("rust PyFunctionObject size", size_of::<ffi::PyFunctionObject>().to_string()));
    // SAFETY: static type object read.
    let vc_off = unsafe { ffi::PyFunction_Type.tp_vectorcall_offset };
    out.push((
        "function vectorcall offset (runtime/compiled)",
        format!("{vc_off}/{}", std::mem::offset_of!(ffi::PyFunctionObject, vectorcall)),
    ));
    out.push(("code layout ok", code_layout_ok(py)?.to_string()));
    Ok(out)
}

/// The per-target decision: may a kernel validated for `validated` (minor versions) run on
/// this interpreter? Rejects unknown minors, pre-releases, debug builds and layout drift.
pub fn supported(py: Python<'_>, validated: &[u8]) -> PyResult<Result<(), String>> {
    let v = runtime_version();
    let (major, minor, level) = ((v >> 24) & 0xff, (v >> 16) & 0xff, (v >> 4) & 0xf);
    if major != 3 || !validated.contains(&(minor as u8)) {
        return Ok(Err(format!("CPython {major}.{minor} not validated")));
    }
    if level != 0xf {
        return Ok(Err("pre-release interpreter".into()));
    }
    if py.import("sys")?.hasattr("gettotalrefcount")? {
        return Ok(Err("debug build".into()));
    }
    // SAFETY: static type object read.
    let vc_off = unsafe { ffi::PyFunction_Type.tp_vectorcall_offset } as usize;
    if vc_off != std::mem::offset_of!(ffi::PyFunctionObject, vectorcall) {
        return Ok(Err("PyFunctionObject layout differs".into()));
    }
    if !code_layout_ok(py)? {
        return Ok(Err("PyCodeObject layout differs".into()));
    }
    Ok(Ok(()))
}

// ---------------------------------------------------------------------------------------
// The registry's gate (docs/ACCELERATION.md §3.2).
// ---------------------------------------------------------------------------------------

/// Running `(major, minor)`.
pub fn running() -> (u8, u8) {
    let v = runtime_version();
    (((v >> 24) & 0xff) as u8, ((v >> 16) & 0xff) as u8)
}

/// Running on a free-threaded build (this extension is compiled per ABI, so compile time
/// tells).
pub const FREE_THREADED: bool = cfg!(Py_GIL_DISABLED);

use super::spec::{Category, Refusal};

static PROCESS: std::sync::OnceLock<Result<(), Refusal>> = std::sync::OnceLock::new();

/// Facts every target needs: a final release, not a debug build, the minor this extension
/// was compiled for. Computed once, without importing anything.
pub fn process_gate() -> Result<(), Refusal> {
    PROCESS
        .get_or_init(|| {
            let v = runtime_version();
            let (major, minor, level) = ((v >> 24) & 0xff, (v >> 16) & 0xff, (v >> 4) & 0xf);
            if major != 3 || minor as u8 != compiled_minor() {
                return Err(Refusal::with(
                    Category::Version,
                    format!("CPython {major}.{minor}: extension compiled for 3.{}", compiled_minor()),
                ));
            }
            if level != 0xf {
                return Err(Refusal::with(Category::Version, format!("CPython {major}.{minor} pre-release ({v:#x})")));
            }
            // SAFETY: attached thread (callers hold the registry lock); borrowed lookup in
            // the sys module dict, no import.
            if !unsafe { ffi::PySys_GetObject(c"gettotalrefcount".as_ptr()) }.is_null() {
                return Err(Refusal::with(Category::Build, "debug build"));
            }
            // SAFETY: static type object read.
            let vc_off = unsafe { ffi::PyFunction_Type.tp_vectorcall_offset } as usize;
            if vc_off != std::mem::offset_of!(ffi::PyFunctionObject, vectorcall) {
                return Err(Refusal::with(Category::Layout, "PyFunctionObject layout differs"));
            }
            Ok(())
        })
        .clone()
}

/// Per-target part of the gate: validated minor and free-threading.
pub fn target_gate(versions: &[(u8, u8)], free_threaded: bool) -> Result<(), Refusal> {
    process_gate()?;
    let (major, minor) = running();
    if !versions.contains(&(major, minor)) {
        return Err(Refusal::with(Category::Version, format!("CPython {major}.{minor} not validated for this target")));
    }
    if FREE_THREADED && !free_threaded {
        return Err(Refusal::with(Category::Build, "not validated on free-threaded builds"));
    }
    Ok(())
}

/// [`code_layout_ok_for`] once per process (the layout is the same for every code object):
/// the first code object seen calibrates it.
pub fn code_layout_cached(code: &Bound<'_, PyAny>) -> PyResult<bool> {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if let Some(ok) = OK.get() {
        return Ok(*ok);
    }
    let ok = code_layout_ok_for(code)?;
    let _ = OK.set(ok);
    Ok(ok)
}

/// Check [`RawCode`] against the attributes CPython reports for `code` itself (no probe
/// compilation, no Python code: safe inside the `sys.modules` watcher). The pointer fields
/// make this a strong check even when the argument counts are all zero.
pub fn code_layout_ok_for(code: &Bound<'_, PyAny>) -> PyResult<bool> {
    if code.cast::<pyo3::types::PyCode>().is_err() {
        return Ok(false);
    }
    // SAFETY: a live code object; reads within the documented prefix.
    let raw = unsafe { &*code.as_ptr().cast::<RawCode>() };
    Ok(raw.co_argcount == code.getattr("co_argcount")?.extract::<i32>()?
        && raw.co_posonlyargcount == code.getattr("co_posonlyargcount")?.extract::<i32>()?
        && raw.co_kwonlyargcount == code.getattr("co_kwonlyargcount")?.extract::<i32>()?
        && raw.co_flags == code.getattr("co_flags")?.extract::<i32>()?
        && raw.co_firstlineno == code.getattr("co_firstlineno")?.extract::<i32>()?
        && raw.co_nlocals == code.getattr("co_nlocals")?.extract::<i32>()?
        && raw.co_consts == code.getattr("co_consts")?.as_ptr()
        && raw.co_names == code.getattr("co_names")?.as_ptr()
        && raw.co_filename == code.getattr("co_filename")?.as_ptr()
        && raw.co_name == code.getattr("co_name")?.as_ptr()
        && raw.co_qualname == code.getattr("co_qualname")?.as_ptr())
}

unsafe extern "C" {
    fn PyCode_NewEmpty(
        filename: *const std::ffi::c_char,
        funcname: *const std::ffi::c_char,
        firstlineno: std::ffi::c_int,
    ) -> *mut ffi::PyObject;
}

static MONITORING_LAYOUT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Check the local-monitoring layout the tools guard reads (`_co_monitoring` →
/// `local_monitors.tools[guard::LOCAL_EVENTS]`), without touching `sys.monitoring`: on a
/// fresh empty code object (never instrumented), the calibrated `PyCodeObject` prefix must
/// hold (pointer fields equal the attributes CPython reports, up to `co_linetable`), and
/// the fields that follow, up to `_co_monitoring`, must be NULL as for any code object no
/// tool ever instrumented. The field order after `co_linetable` and the number of local
/// events are those of the minor's header (`cpython/code.h`, identical on GIL and
/// free-threaded builds of 3.14 and 3.15). Runs when the first vectorcall site is prepared,
/// once per process; C calls only (safe in the `sys.modules` watcher).
///
/// (Wave 1 probed the layout by setting a local event with a free tool id: freeing the id
/// bumps the interpreter's monitoring version for good, so every code object created
/// afterwards paid `_Py_Instrument` at its first `RESUME`, ~930 instructions per new code
/// object: a never-slower leak on every later import.)
pub fn calibrate_monitoring(py: Python<'_>) -> bool {
    *MONITORING_LAYOUT.get_or_init(|| check_monitoring_layout(py).unwrap_or(false))
}

fn check_monitoring_layout(py: Python<'_>) -> PyResult<bool> {
    // SAFETY: static C strings; returns a new reference or NULL with an exception.
    let code = unsafe { PyCode_NewEmpty(c"<rustique-probe>".as_ptr(), c"probe".as_ptr(), 1) };
    // SAFETY: new reference or NULL.
    let code = unsafe { Bound::from_owned_ptr_or_err(py, code) }?;
    if !code_layout_ok_for(&code)? {
        return Ok(false);
    }
    // SAFETY: a live code object; reads within the documented prefix (calibrated above).
    let raw = unsafe { &*code.as_ptr().cast::<RawCode>() };
    Ok(raw.co_linetable == code.getattr("co_linetable")?.as_ptr()
        && raw.co_weakreflist.is_null()
        && raw.co_executors.is_null()
        && raw._co_cached.is_null()
        && raw._co_monitoring.is_null())
}
