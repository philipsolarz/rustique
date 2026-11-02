//! Group `gc` (docs/targets/gc.md): the adaptive GC policy, an opt-in tier
//! (`RUSTIQUE_TIERS=gc`; DECISIONS §10.3).
//!
//! Full collections that keep finding nothing are deferred (the old generation may grow by
//! up to 2× instead of 25% before the next one), never past 1.1× the largest legitimate full
//! collection; as soon as full collections find garbage again CPython's rule is back. Young
//! collections and explicit `gc.collect()` are untouched. The program's own GC tuning
//! (`gc.set_threshold/enable/disable/freeze/unfreeze/set_debug`) hands control back for the
//! rest of the process.
//!
//! | piece | where | what |
//! |---|---|---|
//! | [`layout`] | `_PyRuntime.debug_offsets` + recorded struct offsets | the main interpreter's collector state, checked at install |
//! | [`observer`] | `gcstate->callbacks` → a private `[observer]` list | sees every collection; forwards to the program's `gc.callbacks` exactly as CPython does |
//! | [`policy`] | `gcstate->long_lived_total` | the rule; CPython's own 25% test then defers |
//! | hooks | `ml_meth` of `gc.set_threshold/enable/disable/freeze/unfreeze/set_debug` | call CPython's function, then step aside |
//!
//! Main interpreter only: the hooks sit in the `gc` module's static method table, shared by
//! every interpreter, and do nothing but call the original elsewhere.

use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

use crate::accel::{Category, Counters, Original, Prepare, Refusal, TargetSpec};

mod layout;
mod observer;
mod policy;

/// Hits: full collections the policy deferred (GIL; generation-1 collections run where
/// CPython's rule would have run a full one).
pub(crate) static COUNTERS: Counters = Counters::new();

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

// ---------------------------------------------------------------------------------------
// Opt-in adapter. Until the registry's opt-in tiers land (`TargetSpec.opt_in:
// Some("gc")`, `RUSTIQUE_TIERS`, `enable(tiers=...)`), the target refuses itself (category
// `build`: "not this configuration", expected by the test plugins) unless the tier was
// requested through `RUSTIQUE_TIERS` or `_accel._t_gc.request(True)`.
// ---------------------------------------------------------------------------------------

static REQUESTED: AtomicBool = AtomicBool::new(false);

fn ignore_environment(py: Python<'_>) -> bool {
    // SAFETY: borrowed lookup in the sys module dict.
    let flags = unsafe { ffi::PySys_GetObject(c"flags".as_ptr()) };
    if flags.is_null() {
        return false;
    }
    // SAFETY: a live borrowed object.
    let flags = unsafe { Bound::from_borrowed_ptr(py, flags) };
    flags.getattr("ignore_environment").and_then(|v| v.is_truthy()).unwrap_or(false)
}

fn tier_requested(py: Python<'_>) -> bool {
    if REQUESTED.load(Ordering::Relaxed) {
        return true;
    }
    if ignore_environment(py) {
        return false;
    }
    std::env::var_os("RUSTIQUE_TIERS")
        .is_some_and(|v| v.to_string_lossy().split([',', ';']).any(|t| t.trim().eq_ignore_ascii_case("gc")))
}

// ---------------------------------------------------------------------------------------
// Hooks: the program tuning the GC.
// ---------------------------------------------------------------------------------------

macro_rules! hook {
    ($kernel:ident, $orig:ident, $reason:expr) => {
        static $orig: Original<ffi::PyCFunction> = Original::new();

        /// CPython's function first (same validation, errors and effect), then step aside
        /// when it succeeded in the main interpreter.
        unsafe extern "C" fn $kernel(m: *mut ffi::PyObject, a: *mut ffi::PyObject) -> *mut ffi::PyObject {
            // SAFETY: the original of this site, with the caller's arguments.
            let r = unsafe { $orig.get()(m, a) };
            // SAFETY: attached thread (a C function called by CPython).
            if !r.is_null() && policy::watching() && unsafe { crate::accel::in_main_interpreter() } {
                policy::program_tuned($reason);
            }
            r
        }
    };
}

hook!(k_set_threshold, ORIG_SET_THRESHOLD, 1);
hook!(k_enable, ORIG_ENABLE, 2);
hook!(k_disable, ORIG_DISABLE, 3);
hook!(k_freeze, ORIG_FREEZE, 4);
hook!(k_unfreeze, ORIG_UNFREEZE, 5);
hook!(k_set_debug, ORIG_SET_DEBUG, 6);

type Hook = (&'static str, ffi::PyCFunction, &'static Original<ffi::PyCFunction>, crate::accel::Fingerprints);

/// `ml_name`/`ml_flags`/`ml_doc` fingerprints per minor
/// (`rustique._rustique._accel.fingerprint(gc.<name>)`).
const HOOKS: [Hook; 6] = [
    (
        "set_threshold",
        k_set_threshold,
        &ORIG_SET_THRESHOLD,
        &[(3, 14, "18f236c6e46a1bf7"), (3, 15, "18f236c6e46a1bf7")],
    ),
    ("enable", k_enable, &ORIG_ENABLE, &[(3, 14, "0185fe5e4ebffa52"), (3, 15, "0185fe5e4ebffa52")]),
    ("disable", k_disable, &ORIG_DISABLE, &[(3, 14, "8ff6fe940889e8d7"), (3, 15, "8ff6fe940889e8d7")]),
    ("freeze", k_freeze, &ORIG_FREEZE, &[(3, 14, "57579536ed27c2f0"), (3, 15, "57579536ed27c2f0")]),
    ("unfreeze", k_unfreeze, &ORIG_UNFREEZE, &[(3, 14, "078f3fad5f9cbeb5"), (3, 15, "078f3fad5f9cbeb5")]),
    ("set_debug", k_set_debug, &ORIG_SET_DEBUG, &[(3, 14, "323eacf2612e78eb"), (3, 15, "323eacf2612e78eb")]),
];

// ---------------------------------------------------------------------------------------
// Install.
// ---------------------------------------------------------------------------------------

/// A private instance of the builtin `gc` module (never imports: `sys.modules` is not
/// touched). Executing it binds `garbage` and `callbacks` to the collector state's own lists,
/// which validates the layout's pointer fields; its functions use the module's static method
/// table, the same entries every `gc` module object uses.
fn private_gc_module(py: Python<'_>) -> Result<Bound<'_, PyModule>, Refusal> {
    let modules = crate::accel::prepare::sys_modules(py)?;
    let imp = modules
        .get_item(pyo3::intern!(py, "_imp"))?
        .ok_or_else(|| Refusal::with(Category::Deps, "_imp is not loaded"))?;
    // A spec needs only `name`: a namespace of `sys.implementation`'s type.
    // SAFETY: a borrowed reference owned by the sys module, or NULL (no exception).
    let implementation = unsafe { ffi::PySys_GetObject(c"implementation".as_ptr()) };
    if implementation.is_null() {
        return Err(Refusal::with(Category::Deps, "sys.implementation is missing"));
    }
    // SAFETY: a live object (see above).
    let namespace = unsafe { Bound::from_borrowed_ptr(py, implementation) }.get_type();
    let kw = PyDict::new(py);
    kw.set_item(pyo3::intern!(py, "name"), pyo3::intern!(py, "gc"))?;
    let spec = namespace.call((), Some(&kw))?;
    let module = imp.call_method1(pyo3::intern!(py, "create_builtin"), (&spec,))?;
    imp.call_method1(pyo3::intern!(py, "exec_builtin"), (&module,))?;
    module
        .cast_into::<PyModule>()
        .map_err(|_| Refusal::with(Category::Other, "_imp.create_builtin('gc') is not a module"))
}

/// Drops the private module's namespace when done: its functions reference the module
/// (`m_self`), a cycle that would otherwise keep a second reference to `gc.garbage` and
/// `gc.callbacks` alive until the next full collection (visible to `sys.getrefcount` and
/// `gc.get_referrers`).
struct Private<'py>(Bound<'py, PyModule>);

impl Drop for Private<'_> {
    fn drop(&mut self) {
        self.0.dict().clear();
    }
}

fn layout_refusal((cat, why): (u8, String)) -> Refusal {
    Refusal::with(if cat == layout::E_SYMBOL { Category::Symbol } else { Category::Layout }, why)
}

/// Check the calibrated fields against a freshly executed `gc` module's view.
fn validate(py: Python<'_>, l: &layout::Layout, gc: &Bound<'_, PyModule>, cx: &Prepare<'_>) -> Result<(), Refusal> {
    let d = gc.dict();
    let bad =
        |what: &str| Refusal::with(Category::Layout, format!("collector state: {what} differs from the gc module's"));
    if cx.get(&d, "garbage")?.as_ptr() != l.ptr(l.off.garbage) {
        return Err(bad("garbage"));
    }
    if cx.get(&d, "callbacks")?.as_ptr() != l.ptr(l.off.callbacks) {
        return Err(bad("callbacks"));
    }
    let (enabled, thr, debug) = l.config();
    let call = |name: &str| -> Result<Bound<'_, PyAny>, Refusal> { Ok(cx.get(&d, name)?.call0()?) };
    if call("isenabled")?.extract::<bool>()? != (enabled != 0) {
        return Err(bad("enabled"));
    }
    if call("get_threshold")?.extract::<(i32, i32, i32)>()? != (thr[0], thr[1], thr[2]) {
        return Err(bad("thresholds"));
    }
    if call("get_debug")?.extract::<i32>()? != debug {
        return Err(bad("debug"));
    }
    let _ = py;
    Ok(())
}

/// Why the program's GC configuration at install already counts as tuned (the policy then
/// installs passive), or `None`.
fn tuned_at_install(l: &layout::Layout) -> Option<u8> {
    let (enabled, thr, debug) = l.config();
    if enabled == 0 {
        Some(policy::R_DISABLED_AT_ENABLE)
    } else if thr != [2000, 10, 10] {
        Some(policy::R_THRESHOLDS_AT_ENABLE)
    } else if debug != 0 {
        Some(policy::R_DEBUG_AT_ENABLE)
    } else if l.frozen() {
        Some(policy::R_FROZEN_AT_ENABLE)
    } else {
        None
    }
}

fn prepare(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if !tier_requested(py) {
        return Err(Refusal::with(Category::Build, "opt-in tier: RUSTIQUE_TIERS=gc"));
    }
    let l = layout::calibrate(py).map_err(layout_refusal)?;
    let gc = Private(private_gc_module(py)?);
    validate(py, l, &gc.0, cx)?;
    let d = gc.0.dict();
    let mut set_threshold_def: *mut ffi::PyMethodDef = std::ptr::null_mut();
    for (name, kernel, orig, fp) in HOOKS {
        let f = cx.get(&d, name)?;
        cx.fingerprint_c(&f, fp)?;
        cx.ml_meth(&f, kernel, orig)?;
        if name == "set_threshold" {
            // SAFETY: attached thread; a builtin function of the gc module.
            set_threshold_def = unsafe { crate::accel::site::method_def(f.as_ptr()) }.unwrap_or(std::ptr::null_mut());
        }
    }
    if set_threshold_def.is_null() {
        return Err(Refusal::with(Category::Conflict, "gc.set_threshold has no method def"));
    }
    if cx.record {
        // The dev tool's fingerprint report: declare, change nothing.
        return Ok(());
    }
    // Last: the only step with an effect outside the declared sites. The observer is
    // passive until the registry has installed the hooks (`policy` checks them).
    observer::install(py, l).map_err(|e| Refusal::with(Category::Other, e))?;
    policy::arm(l, tuned_at_install(l), set_threshold_def, k_set_threshold as *const () as usize);
    Ok(())
}

/// `disable()` and the exit hook (`on_disable`), and the exit hook again (`release`, also
/// reached when the hooks were never installed): CPython's value back in
/// `long_lived_total` at once, the program's list back in `gcstate->callbacks` (unless a
/// collection is running right now, e.g. `disable()` called from a GC callback: the observer
/// then stays and only forwards until the next restore).
fn restore(py: Python<'_>) {
    policy::disarm();
    if let Some(l) = layout::get() {
        observer::uninstall(py, l);
    }
}

/// `gc.adaptive`: the policy, its observer and the six stepping-aside hooks.
pub static ADAPTIVE: TargetSpec = TargetSpec {
    name: "gc.adaptive",
    group: "gc",
    module: None,
    versions: VERSIONS,
    free_threaded: true,
    counters: &COUNTERS,
    prepare,
    on_disable: Some(restore),
    release: Some(restore),
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&ADAPTIVE];

// ---------------------------------------------------------------------------------------
// Test and benchmark helpers (`rustique._rustique._accel._t_gc`).
// ---------------------------------------------------------------------------------------

/// The policy's state and the collector fields it reads.
#[pyfunction]
fn report(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    policy::report(py)
}

/// Request the tier in this process (the adapter's stand-in for `enable(tiers=["gc"])`).
#[pyfunction]
fn request(on: bool) {
    REQUESTED.store(on, Ordering::Relaxed);
}

/// Research only: the rule's parameters (applied at the next install, and to the running
/// rule).
#[pyfunction]
#[pyo3(signature = (y_low=0.01, y_high=0.05, kmax=3, cap=policy::DEFAULT_CAP, step_aside=true))]
fn set_params(y_low: f64, y_high: f64, kmax: u32, cap: f64, step_aside: bool) -> PyResult<()> {
    if !(0.0..=1.0).contains(&y_low) || !(0.0..=1.0).contains(&y_high) || kmax > 8 || cap < 0.0 {
        return Err(pyo3::exceptions::PyValueError::new_err("parameter out of range"));
    }
    policy::set_params(policy::Params { y_low, y_high, kmax, cap, step_aside });
    Ok(())
}

/// The calibration result: field offsets, or the refusal reason.
#[pyfunction]
#[pyo3(name = "layout")]
fn layout_info(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let d = PyDict::new(py);
    match layout::calibrate(py) {
        Ok(l) => {
            for (k, v) in l.describe() {
                d.set_item(k, v)?;
            }
        }
        Err((_, why)) => d.set_item("error", why)?,
    }
    Ok(d)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = PyModule::new(m.py(), "_t_gc")?;
    sub.add_function(wrap_pyfunction!(report, &sub)?)?;
    sub.add_function(wrap_pyfunction!(request, &sub)?)?;
    sub.add_function(wrap_pyfunction!(set_params, &sub)?)?;
    sub.add_function(wrap_pyfunction!(layout_info, &sub)?)?;
    m.add("_t_gc", sub)
}
