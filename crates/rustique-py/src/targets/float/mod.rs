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

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyList;

use rustique_core::float_repr;

use crate::accel::{Counters, Original, Prepare, Reason, Refusal, SlotName, TargetSpec, check};

pub static COUNTERS: Counters = Counters::new();

/// The original `float_repr` (the kernel's fallback).
static ORIG_REPR: Original<ffi::reprfunc> = Original::new();
/// The original `float.__format__` (`METH_O`).
static ORIG_FORMAT: Original<ffi::PyCFunction> = Original::new();
/// A fresh `PyUnicode_New(n, 127)` keeps its characters right after the `PyASCIIObject`
/// header (verified at install).
static ASCII_LAYOUT_OK: AtomicBool = AtomicBool::new(false);

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

/// The `tp_repr` kernel.
unsafe extern "C" fn repr_kernel(op: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: `op` is a live float (or float subclass) instance; attached thread.
    unsafe {
        if ffi::Py_TYPE(op) == &raw mut ffi::PyFloat_Type {
            let x = (*op.cast::<ffi::PyFloatObject>()).ob_fval;
            if x.is_finite() {
                let r = repr_finite(x);
                if COUNTERS.hit_check() {
                    return check::verify(&FLOAT_REPR, r, &[op], || ORIG_REPR.get()(op));
                }
                return r;
            }
        }
        COUNTERS.fallback(Reason::Args);
        ORIG_REPR.get()(op)
    }
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
                let r = repr_finite(x);
                if COUNTERS.hit_check() {
                    return check::verify(&FLOAT_REPR, r, &[slf, spec], || ORIG_FORMAT.get()(slf, spec));
                }
                return r;
            }
        }
        COUNTERS.fallback(Reason::Args);
        ORIG_FORMAT.get()(slf, spec)
    }
}

/// Probe values for the install-time self-test (layout cut-offs, a sign, an extreme).
const PROBES: &[f64] = &[-0.0, 0.1, 1e16, 1e-5, -1.2345e-5, 9999999999999998.0, 1e22, 5e-324];

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

/// Validate (layout, original pointers, fingerprint, self-test) and declare both sites.
fn prepare(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if !calibrate_ascii(py) {
        return Err(crate::refuse!(Layout: "str layout not calibrated"));
    }
    let float_type = py.get_type::<pyo3::types::PyFloat>();
    // The slot must hold what the `float.__repr__` wrapper captured (checked by `slot`).
    cx.slot(&float_type, SlotName::TpRepr, repr_kernel as ffi::reprfunc, &ORIG_REPR)?;
    let fmt = cx.type_attr(&float_type, "__format__")?;
    cx.fingerprint_c(&fmt, FORMAT_FINGERPRINT)?;
    cx.ml_meth(&fmt, format_kernel as ffi::PyCFunction, &ORIG_FORMAT)?;
    // Behavioural self-test: the core's text against `PyOS_double_to_string` (what
    // `float_repr` returns) and the original `__format__('')`, both signs of every probe.
    // Minimal by design (install budget; identity is proven offline on 464 M values): the
    // layout cases, against what `float_repr` and the original `__format__('')` return.
    cx.self_test("float repr probes", |py| {
        for &x in PROBES {
            let mut buf = [0u8; 32];
            let n = float_repr::write_finite(x, &mut buf);
            if cpython_repr(x).as_deref() != Some(&buf[..n]) {
                return Ok(false);
            }
        }
        let obj = pyo3::types::PyFloat::new(py, -1.5e-5);
        let empty = pyo3::types::PyString::new(py, "");
        // SAFETY: the original METH_O function with its receiver and argument.
        let r = unsafe { Bound::from_owned_ptr_or_err(py, ORIG_FORMAT.get()(obj.as_ptr(), empty.as_ptr()))? };
        Ok(r.extract::<String>()? == "-1.5e-05")
    })
}

/// `ml_name`/`ml_flags`/`ml_doc` of `float.__format__` per minor version
/// (`rustique._rustique._accel.fingerprint(float.__dict__['__format__'])`).
const FORMAT_FINGERPRINT: crate::accel::Fingerprints = &[(3, 14, FMT_314), (3, 15, FMT_315)];
const FMT_314: &str = "5626db3e609b8ffe";
const FMT_315: &str = "5626db3e609b8ffe";

/// `float.repr`: `PyFloat_Type.tp_repr` and `float.__format__('')`.
pub static FLOAT_REPR: TargetSpec = TargetSpec {
    name: "float.repr",
    group: "float",
    module: None,
    versions: &[(3, 14), (3, 15)],
    free_threaded: true,
    counters: &COUNTERS,
    prepare,
    checkable: true,
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&FLOAT_REPR];

// ---------------------------------------------------------------------------------------
// Test and benchmark helpers (`rustique._rustique._accel._t_float`).
// ---------------------------------------------------------------------------------------

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
    sub.add_function(wrap_pyfunction!(core_repr, &sub)?)?;
    sub.add_function(wrap_pyfunction!(core_reprs, &sub)?)?;
    sub.add_function(wrap_pyfunction!(selfcheck, &sub)?)?;
    m.add("_t_float", sub)
}
