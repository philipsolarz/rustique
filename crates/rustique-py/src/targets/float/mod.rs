//! Target group `float`: target `float.repr`.
//!
//! Site: the `tp_repr` slot of the static `PyFloat_Type` (process-global). `float` has no
//! `tp_str` of its own: `object_str` calls `tp_repr`, so `repr(x)`, `str(x)`, `f"{x}"`,
//! `f"{x!r}"`, `'%s' % x`, `'%r' % x`, `print(x)`, logging and `_json`'s float encoding
//! (`PyFloat_Type.tp_repr(obj)`) all reach the kernel. Not reached (and still exact):
//! `float.__repr__(x)` (the wrapper descriptor captured the original at type creation) and
//! subclasses created before install (they copied the slot).
//!
//! Second site: the `ml_meth` of `float.__format__` (static method table), for an empty
//! format spec only (`f"{x}"`, `format(x)`, `"{}".format(x)`), which CPython answers with
//! `str(x)` through a `_PyUnicodeWriter`; the kernel returns the repr directly.
//!
//! Exact subset: receivers of the exact type `float` with a finite value. Everything else
//! (subclasses, `inf`, `nan`) calls the original `float_repr`. The kernel builds the same
//! object `float_repr` does: a new compact ASCII `str` (`_PyUnicode_FromASCII`; a float
//! repr is never a single character, so no cached singleton is involved). It holds no
//! Python objects, so it is interpreter-agnostic, and it lives as long as the process
//! (subclasses created while installed keep its address after uninstall).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use rustique_core::float_repr;

use crate::accel::site::{Patch, Site, SlotName};
use crate::accel::stats::{self, Counters, Reason};
use crate::accel::version;

/// CPython minor versions `float.repr` is validated on (GIL and free-threaded builds).
pub const VERSIONS: &[u8] = &[14, 15];

pub static COUNTERS: Counters = Counters::new();

/// The original `float_repr` (the kernel's fallback), saved before the swap.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The original `float.__format__` (`METH_O`).
static ORIGINAL_FORMAT: AtomicUsize = AtomicUsize::new(0);
/// A fresh `PyUnicode_New(n, 127)` keeps its characters right after the `PyASCIIObject`
/// header (verified at install).
static ASCII_LAYOUT_OK: AtomicBool = AtomicBool::new(false);

struct State {
    patch: Option<Patch>,
    refused: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State { patch: None, refused: None });

/// A new compact ASCII `str` holding `bytes` (exactly `_PyUnicode_FromASCII` for
/// `len >= 2`; one character goes through CPython for its cached singleton).
///
/// # Safety
/// Attached thread; `bytes` is ASCII; [`ascii_layout_ok`] holds.
#[inline(always)]
pub(crate) unsafe fn new_ascii(bytes: &[u8]) -> *mut ffi::PyObject {
    // SAFETY: per the caller.
    unsafe {
        if bytes.len() < 2 {
            return ffi::PyUnicode_FromStringAndSize(bytes.as_ptr().cast(), bytes.len() as ffi::Py_ssize_t);
        }
        let s = ffi::PyUnicode_New(bytes.len() as ffi::Py_ssize_t, 127);
        if !s.is_null() {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                s.cast::<ffi::PyASCIIObject>().add(1).cast::<u8>(),
                bytes.len(),
            );
        }
        s
    }
}

/// Whether [`new_ascii`] may write a compact ASCII string's data directly.
#[allow(dead_code)] // used by the json group
#[inline(always)]
pub(crate) fn ascii_layout_ok() -> bool {
    ASCII_LAYOUT_OK.load(Relaxed)
}

/// Check (once) where `PyUnicode_New(n, 127)` puts its characters.
pub(crate) fn calibrate_ascii(py: Python<'_>) -> bool {
    if ASCII_LAYOUT_OK.load(Relaxed) {
        return true;
    }
    let _ = py;
    // SAFETY: attached thread (the `py` token); a fresh string, released below.
    let ok = unsafe {
        let s = ffi::PyUnicode_New(5, 127);
        if s.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        let ok = ffi::PyUnicode_DATA(s) == s.cast::<ffi::PyASCIIObject>().add(1).cast()
            && ffi::PyUnicode_KIND(s) == ffi::PyUnicode_1BYTE_KIND;
        ffi::Py_DECREF(s);
        ok
    };
    ASCII_LAYOUT_OK.store(ok, Relaxed);
    ok
}

/// `repr(x)` of a finite float as a new `str` (`NULL` with an exception on allocation
/// failure, like `float_repr`).
///
/// # Safety
/// Attached thread; `x` finite; [`ascii_layout_ok`] holds.
#[inline(always)]
pub(crate) unsafe fn repr_finite(x: f64) -> *mut ffi::PyObject {
    let mut buf = [0u8; 32];
    let n = float_repr::write_finite(x, &mut buf);
    // SAFETY: ASCII bytes; per the caller.
    unsafe { new_ascii(&buf[..n]) }
}

#[inline(always)]
unsafe fn original(op: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: the saved `float_repr` (set before the kernel can be reached).
    unsafe { std::mem::transmute::<usize, ffi::reprfunc>(ORIGINAL.load(Relaxed))(op) }
}

/// The `tp_repr` kernel.
unsafe extern "C" fn repr_kernel(op: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: `op` is a live float (or float subclass) instance; attached thread.
    unsafe {
        if ffi::Py_TYPE(op) == &raw mut ffi::PyFloat_Type {
            let x = (*op.cast::<ffi::PyFloatObject>()).ob_fval;
            if x.is_finite() {
                COUNTERS.hit();
                return repr_finite(x);
            }
        }
        COUNTERS.fallback(Reason::Args);
        original(op)
    }
}

#[inline(always)]
unsafe fn original_format(slf: *mut ffi::PyObject, spec: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: the saved `float.__format__` (set before the kernel can be reached).
    unsafe { std::mem::transmute::<usize, ffi::PyCFunction>(ORIGINAL_FORMAT.load(Relaxed))(slf, spec) }
}

/// The `float.__format__` kernel (`METH_O`): an empty format spec is `str(x)`
/// (`_PyFloat_FormatAdvancedWriter` → `format_obj` → `PyObject_Str`), i.e. `float_repr` for an
/// exact float; the writer then returns that very string. Any other spec, receiver or
/// value runs the original.
unsafe extern "C" fn format_kernel(slf: *mut ffi::PyObject, spec: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: METH_O arguments of a float method; attached thread.
    unsafe {
        if ffi::Py_TYPE(slf) == &raw mut ffi::PyFloat_Type
            && ffi::Py_TYPE(spec) == &raw mut ffi::PyUnicode_Type
            && ffi::PyUnicode_GET_LENGTH(spec) == 0
        {
            let x = (*slf.cast::<ffi::PyFloatObject>()).ob_fval;
            if x.is_finite() {
                COUNTERS.hit();
                return repr_finite(x);
            }
        }
        COUNTERS.fallback(Reason::Args);
        original_format(slf, spec)
    }
}

/// `sys._clear_internal_caches()`: 3.15's tier-2 traces copy `ml_meth`; call after every
/// `ml_meth` install and uninstall (contract §3.8).
pub(crate) fn clear_jit_caches(py: Python<'_>) {
    if let Ok(f) = py.import("sys").and_then(|s| s.getattr("_clear_internal_caches")) {
        let _ = f.call0();
    }
}

/// Probe values for the install-time self-test (layout cut-offs, signs, extremes).
const PROBES: &[f64] = &[
    0.0,
    -0.0,
    1.0,
    -1.5,
    0.1,
    0.30000000000000004,
    123.456,
    1e15,
    1e16,
    9999999999999998.0,
    1e-4,
    9.999999999999999e-5,
    1e-5,
    -1.2345e-5,
    1e-7,
    1e22,
    1e100,
    1e-300,
    5e-324,
    2.2250738585072014e-308,
    1.7976931348623157e308,
    2.9802322387695312e-08,
];

/// CPython's own text for `x` (`PyOS_double_to_string(x, 'r', 0, Py_DTSF_ADD_DOT_0)`, what
/// `float_repr` returns), or `None` on allocation failure.
fn cpython_repr(x: f64) -> Option<Vec<u8>> {
    // SAFETY: CPython API; the buffer is NUL-terminated and released with PyMem_Free.
    unsafe {
        let p =
            ffi::PyOS_double_to_string(x, b'r' as std::ffi::c_char, 0, ffi::Py_DTSF_ADD_DOT_0, std::ptr::null_mut());
        if p.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let v = std::ffi::CStr::from_ptr(p).to_bytes().to_vec();
        ffi::PyMem_Free(p.cast());
        Some(v)
    }
}

/// Validate (version gate, original pointer, self-test) and swap the slot.
fn install_inner(py: Python<'_>) -> Result<Patch, String> {
    version::supported(py, VERSIONS).map_err(|e| e.to_string())??;
    if !calibrate_ascii(py) {
        return Err("str layout not calibrated".into());
    }
    let float_type = py.get_type::<pyo3::types::PyFloat>();
    // The slot must still hold what CPython installed: the function the `float.__repr__`
    // wrapper descriptor captured when the type was readied.
    // (Static builtin types keep their dict in the interpreter, not in `tp_dict`.)
    let descr = float_type
        .getattr("__dict__")
        .and_then(|d| d.get_item("__repr__"))
        .map_err(|_| "float.__repr__ not found".to_string())?;
    // SAFETY: attached thread; type and descriptor objects are alive.
    let (current, wrapped) = unsafe {
        if ffi::Py_TYPE(descr.as_ptr()) != &raw mut ffi::PyWrapperDescr_Type {
            return Err("float.__repr__ is not a wrapper descriptor".into());
        }
        (
            (*float_type.as_type_ptr()).tp_repr.map_or(0, |f| f as usize),
            (*descr.as_ptr().cast::<ffi::PyWrapperDescrObject>()).d_wrapped as usize,
        )
    };
    if current == 0 || current != wrapped {
        return Err("float.tp_repr was replaced by someone else".into());
    }
    let site =
        Site::slot(&float_type, SlotName::TpRepr, repr_kernel as ffi::reprfunc as usize).map_err(|e| e.to_string())?;
    // `float.__format__`: fingerprint of the method table entry.
    let fmt = float_type.getattr("__dict__").and_then(|d| d.get_item("__format__")).map_err(|e| e.to_string())?;
    let (fmt_site, conv) =
        Site::ml_meth(&fmt, format_kernel as ffi::PyCFunction as usize).map_err(|e| e.to_string())?;
    if fmt_site.name != "__format__" || conv != ffi::METH_O {
        return Err(format!("float.__format__ fingerprint differs ({} {conv:#x})", fmt_site.name));
    }
    // Behavioural self-test of the kernel's text against the original's, before the swap.
    for &x in PROBES {
        for v in [x, -x] {
            let mut buf = [0u8; 32];
            let n = float_repr::write_finite(v, &mut buf);
            if cpython_repr(v).as_deref() != Some(&buf[..n]) {
                return Err(format!("self-test failed on {v:e}"));
            }
            // The original `__format__` with an empty spec gives the same text.
            let via_format = (|| -> PyResult<String> {
                pyo3::types::PyFloat::new(py, v).call_method1("__format__", ("",))?.extract()
            })();
            if via_format.ok().as_deref().map(str::as_bytes) != Some(&buf[..n]) {
                return Err(format!("format self-test failed on {v:e}"));
            }
        }
    }
    ORIGINAL.store(site.original, Relaxed);
    ORIGINAL_FORMAT.store(fmt_site.original, Relaxed);
    let mut patch = Patch { sites: vec![site, fmt_site] };
    patch.install().map_err(|e| e.to_string())?;
    clear_jit_caches(py);
    Ok(patch)
}

/// Install `float.repr`; returns `None` when installed (or already), the refusal reason
/// otherwise.
pub fn install(py: Python<'_>) -> Option<String> {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if st.patch.is_some() {
        return None;
    }
    match install_inner(py) {
        Ok(p) => {
            st.patch = Some(p);
            st.refused = None;
            None
        }
        Err(e) => {
            st.refused = Some(e.clone());
            Some(e)
        }
    }
}

/// Restore the original slot and method (the kernels stay callable: subclasses may have
/// copied the slot, traces may hold the method).
pub fn uninstall(py: Python<'_>) {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut p) = st.patch.take() {
        p.uninstall(false);
        clear_jit_caches(py);
    }
}

pub fn installed() -> bool {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).patch.is_some()
}

// ---------------------------------------------------------------------------------------
// Python interface (private module `rustique._rustique._t_float`, until the accel
// registry takes over installation).
// ---------------------------------------------------------------------------------------

#[pyfunction(name = "install")]
fn py_install(py: Python<'_>) -> Option<String> {
    install(py)
}

#[pyfunction(name = "uninstall")]
fn py_uninstall(py: Python<'_>) {
    uninstall(py)
}

#[pyfunction(name = "installed")]
fn py_installed() -> bool {
    installed()
}

#[pyfunction]
fn status(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let d = PyDict::new(py);
    d.set_item("name", "float.repr")?;
    d.set_item("group", "float")?;
    d.set_item(
        "state",
        if st.patch.is_some() {
            "installed"
        } else if st.refused.is_some() {
            "refused"
        } else {
            "disabled"
        },
    )?;
    d.set_item("reason", st.refused.clone())?;
    d.set_item("sites", vec!["float.tp_repr", "float.__format__"])?;
    d.set_item("versions", VERSIONS.iter().map(|m| format!("3.{m}")).collect::<Vec<_>>())?;
    Ok(d)
}

/// Turn the (global) counters on or off.
#[pyfunction]
fn stats_on(on: bool) {
    stats::ON.store(on, Relaxed);
}

#[pyfunction]
fn counters(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let (hits, fb) = COUNTERS.snapshot();
    let d = PyDict::new(py);
    d.set_item("hits", hits)?;
    let f = PyDict::new(py);
    for (k, v) in fb {
        f.set_item(k, v)?;
    }
    d.set_item("fallback", f)?;
    Ok(d)
}

#[pyfunction]
fn reset_counters() {
    COUNTERS.reset();
}

/// The core's text for `x` (any float), for tests.
#[pyfunction]
fn core_repr(x: f64) -> String {
    float_repr::repr(x)
}

/// [repr(x) for x in xs] through the core, for tests and benchmarks.
#[pyfunction]
fn core_reprs<'py>(py: Python<'py>, xs: Vec<f64>) -> PyResult<Bound<'py, PyList>> {
    PyList::new(py, xs.into_iter().map(float_repr::repr))
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Compare the core with CPython's `PyOS_double_to_string` (the function `float_repr`
/// calls) on `n` values of a family drawn from `seed`, in process. Families: `bits`
/// (random bit patterns), `subnormal`, `dyadic` (m * 2**e, exact short decimals and
/// halfway cases), `decimal` (short decimal strings parsed), `sweep` (n consecutive bit
/// patterns from `seed`). Returns `(checked, mismatches, first examples)`.
#[pyfunction]
#[pyo3(name = "_selfcheck")]
fn selfcheck(py: Python<'_>, family: &str, n: u64, seed: u64) -> PyResult<(u64, u64, Vec<String>)> {
    let mut r = Rng(seed | 1);
    let (mut checked, mut bad) = (0u64, 0u64);
    let mut examples = Vec::new();
    let mut buf = [0u8; 32];
    let mut i = 0u64;
    while i < n {
        i += 1;
        let x = match family {
            "bits" => f64::from_bits(r.next()),
            "subnormal" => f64::from_bits(r.next() & ((1 << 52) - 1) | (r.next() & (1 << 63))),
            "dyadic" => {
                let m = (r.next() >> 11) >> (r.next() % 53);
                let e = (r.next() % 200) as i32 - 100;
                m as f64 * 2f64.powi(e)
            }
            "decimal" => {
                let digits = 1 + r.next() % 17;
                let d = r.next() % 10u64.pow(digits as u32);
                let e = (r.next() % 660) as i64 - 340;
                format!("{d}e{e}").parse().unwrap_or(0.0)
            }
            "sweep" => f64::from_bits(seed.wrapping_add(i - 1)),
            _ => return Err(PyRuntimeError::new_err(format!("unknown family {family:?}"))),
        };
        if !x.is_finite() {
            continue;
        }
        checked += 1;
        let k = float_repr::write_finite(x, &mut buf);
        let want = cpython_repr(x).ok_or_else(|| PyRuntimeError::new_err("PyOS_double_to_string failed"))?;
        if want != buf[..k] {
            bad += 1;
            if examples.len() < 10 {
                examples.push(format!(
                    "{:016x}: cpython {} core {}",
                    x.to_bits(),
                    String::from_utf8_lossy(&want),
                    String::from_utf8_lossy(&buf[..k])
                ));
            }
        }
        if i.is_multiple_of(1_000_000) {
            py.check_signals()?;
        }
    }
    Ok((checked, bad, examples))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = PyModule::new(m.py(), "_t_float")?;
    sub.add_function(wrap_pyfunction!(py_install, &sub)?)?;
    sub.add_function(wrap_pyfunction!(py_uninstall, &sub)?)?;
    sub.add_function(wrap_pyfunction!(py_installed, &sub)?)?;
    sub.add_function(wrap_pyfunction!(status, &sub)?)?;
    sub.add_function(wrap_pyfunction!(stats_on, &sub)?)?;
    sub.add_function(wrap_pyfunction!(counters, &sub)?)?;
    sub.add_function(wrap_pyfunction!(reset_counters, &sub)?)?;
    sub.add_function(wrap_pyfunction!(core_repr, &sub)?)?;
    sub.add_function(wrap_pyfunction!(core_reprs, &sub)?)?;
    sub.add_function(wrap_pyfunction!(selfcheck, &sub)?)?;
    m.add("_t_float", sub)
}
