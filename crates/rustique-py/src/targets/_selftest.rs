//! Hidden group `_selftest`: targets that exercise every mechanism of the framework for the
//! test-suite (`tests/test_accel_*.py`). Never enabled unless named explicitly
//! (`rustique.enable(only=["_selftest"])`). Their kernels are deliberately *visible*
//! (return `("kernel", x)` where the original returns `("orig", x)`) so tests can tell
//! which one ran; real targets are indistinguishable from their originals.
//!
//! - `_selftest.meth`: `ml_meth` site on `_accel._probe` (a C function of ours).
//! - `_selftest.vectorcall`: vectorcall site on `_rustique_selftest.ident` (module target:
//!   lazy activation), with pins on a module global, an absent global, a builtin through
//!   `pin_global`, and a class attribute through `pin_attr`; tools guard.
//! - `_selftest.slot`: `tp_repr` of the heap type `_rustique_selftest.K`.
//! - `_selftest.badfp`: refused: fingerprint mismatch.
//! - `_selftest.refuse`: refused by its prepare.
//! - `_selftest.cas`: two `ml_meth` sites; the second is changed by "someone else" after
//!   validation, so install fails and the first must be rolled back.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicPtr, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyString, PyType};

use crate::accel::{self, Counters, DepFlag, Original, Prepare, Reason, Refusal, SlotName, TargetSpec, Tools};

type O = *mut ffi::PyObject;

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];
const MODULE: &str = "_rustique_selftest";

// ---------------------------------------------------------------------------------------
// Probe C functions with writable, static method tables.
// ---------------------------------------------------------------------------------------

struct Defs(UnsafeCell<[ffi::PyMethodDef; 4]>);
// SAFETY: written only through atomic pointer stores of `ml_meth` (sites) at install.
unsafe impl Sync for Defs {}

static KERNEL_STR: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static ORIG_STR: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());

/// `("orig"|"kernel", x)`.
unsafe fn tagged(tag: &AtomicPtr<ffi::PyObject>, x: O) -> O {
    // SAFETY: attached thread; the tag is a live interned str kept forever.
    unsafe { ffi::PyTuple_Pack(2, tag.load(Ordering::Relaxed), x) }
}

unsafe extern "C" fn probe_orig(_m: O, x: O) -> O {
    // SAFETY: METH_O call.
    unsafe { tagged(&ORIG_STR, x) }
}

unsafe extern "C" fn probe_other(_m: O, _x: O) -> O {
    // SAFETY: attached thread.
    unsafe { ffi::Py_NewRef(ffi::Py_None()) }
}

static DEFS: Defs = Defs(UnsafeCell::new([
    ffi::PyMethodDef {
        ml_name: c"_probe".as_ptr(),
        ml_meth: ffi::PyMethodDefPointer { PyCFunction: probe_orig },
        ml_flags: ffi::METH_O,
        ml_doc: c"Self-test probe.".as_ptr(),
    },
    ffi::PyMethodDef {
        ml_name: c"_probe_a".as_ptr(),
        ml_meth: ffi::PyMethodDefPointer { PyCFunction: probe_orig },
        ml_flags: ffi::METH_O,
        ml_doc: std::ptr::null(),
    },
    ffi::PyMethodDef {
        ml_name: c"_probe_b".as_ptr(),
        ml_meth: ffi::PyMethodDefPointer { PyCFunction: probe_orig },
        ml_flags: ffi::METH_O,
        ml_doc: std::ptr::null(),
    },
    ffi::PyMethodDef::zeroed(),
]));

fn def(i: usize) -> *mut ffi::PyMethodDef {
    // SAFETY: in-bounds element of the static table.
    unsafe { (*DEFS.0.get()).as_mut_ptr().add(i) }
}

// ---------------------------------------------------------------------------------------
// _selftest.meth
// ---------------------------------------------------------------------------------------

static C_METH: Counters = Counters::new();
static ORIG_METH: Original<ffi::PyCFunction> = Original::new();

unsafe extern "C" fn k_meth(m: O, x: O) -> O {
    // SAFETY: METH_O call.
    unsafe {
        if ffi::PyLong_CheckExact(x) != 0 {
            let r = tagged(&KERNEL_STR, x);
            if C_METH.hit_check() {
                // Check mode sees the (deliberate) difference from the original.
                return accel::check::verify(&METH, r, &[x], || ORIG_METH.get()(m, x));
            }
            return r;
        }
        C_METH.fallback(Reason::Args);
        ORIG_METH.get()(m, x)
    }
}

fn prepare_meth(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    // SAFETY: a static, writable method table entry.
    unsafe { cx.ml_meth_def("_accel._probe", def(0), k_meth as ffi::PyCFunction, &ORIG_METH) }
}

// ---------------------------------------------------------------------------------------
// _selftest.vectorcall
// ---------------------------------------------------------------------------------------

static C_VC: Counters = Counters::new();
static DEPS: DepFlag = DepFlag::new();
static TOOLS: Tools = Tools::new();

unsafe extern "C" fn k_ident(f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments from CPython; attached thread.
    unsafe {
        if !(kw.is_null() && nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET == 1) {
            C_VC.fallback_streak(Reason::Args);
        } else if !DEPS.ok() {
            C_VC.fallback_streak(Reason::Deps);
        } else if !accel::tools_quiet(&TOOLS, f) {
            C_VC.fallback_streak(Reason::Tools);
        } else {
            C_VC.hit_streak();
            return tagged(&KERNEL_STR, *args);
        }
        accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw)
    }
}

fn prepare_vectorcall(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let ident = cx.module_attr("ident")?;
    let g = cx.module()?.dict();
    cx.pin(&g, "DEP", &DEPS)?;
    cx.pin_absent(&g, "SHADOW", &DEPS)?;
    cx.pin_global(&g, "len", &DEPS)?;
    let k = cx.module_attr("K")?.cast_into::<PyType>().map_err(PyErr::from)?;
    cx.pin_attr(&k, "attr", &DEPS)?;
    cx.vectorcall(&ident, k_ident, &accel::FUNCTION_VECTORCALL)
}

// ---------------------------------------------------------------------------------------
// _selftest.slot
// ---------------------------------------------------------------------------------------

static C_SLOT: Counters = Counters::new();
static ORIG_REPR: Original<ffi::reprfunc> = Original::new();
static K_TYPE: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(std::ptr::null_mut());

unsafe extern "C" fn k_repr(o: O) -> O {
    // SAFETY: tp_repr call.
    unsafe {
        if ffi::Py_TYPE(o) == K_TYPE.load(Ordering::Relaxed) {
            C_SLOT.hit();
            return ffi::Py_NewRef(KERNEL_STR.load(Ordering::Relaxed));
        }
        C_SLOT.fallback(Reason::Args);
        ORIG_REPR.get()(o)
    }
}

fn prepare_slot(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let k = cx.module_attr("K")?.cast_into::<PyType>().map_err(PyErr::from)?;
    // Keep the type alive for the process (the kernel compares its address).
    K_TYPE.store(k.clone().into_ptr().cast(), Ordering::Relaxed);
    cx.slot(&k, SlotName::TpRepr, k_repr as ffi::reprfunc, &ORIG_REPR)
}

// ---------------------------------------------------------------------------------------
// _selftest.badfp, _selftest.refuse, _selftest.cas
// ---------------------------------------------------------------------------------------

static C_NONE: Counters = Counters::new();

fn prepare_badfp(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let ident = cx.module_attr("ident")?;
    cx.fingerprint_code(&ident, &[(3, 14, "0000000000000000"), (3, 15, "0000000000000000")])?;
    cx.vectorcall(&ident, k_ident, &accel::FUNCTION_VECTORCALL)
}

fn prepare_refuse(_py: Python<'_>, _cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    Err(Refusal::new("selftest refusal"))
}

static ORIG_A: Original<ffi::PyCFunction> = Original::new();
static ORIG_B: Original<ffi::PyCFunction> = Original::new();

unsafe extern "C" fn k_a(m: O, x: O) -> O {
    // SAFETY: forwarded.
    unsafe { ORIG_A.get()(m, x) }
}

unsafe extern "C" fn k_b(m: O, x: O) -> O {
    // SAFETY: forwarded.
    unsafe { ORIG_B.get()(m, x) }
}

fn prepare_cas(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    // SAFETY: static, writable method table entries.
    unsafe {
        cx.ml_meth_def("_accel._probe_a", def(1), k_a as ffi::PyCFunction, &ORIG_A)?;
        cx.ml_meth_def("_accel._probe_b", def(2), k_b as ffi::PyCFunction, &ORIG_B)?;
        // "Someone else" patches `_probe_b` after validation: install must fail and roll
        // `_probe_a` back. (`_accel._selftest_untamper()` restores it.)
        set_meth(2, probe_other as ffi::PyCFunction as usize);
    }
    Ok(())
}

/// # Safety
/// Index of a static probe entry; `f` a METH_O function.
unsafe fn set_meth(i: usize, f: usize) {
    // SAFETY: aligned pointer-sized field of a static entry.
    unsafe { (*(&raw mut (*def(i)).ml_meth).cast::<std::sync::atomic::AtomicUsize>()).store(f, Ordering::Release) };
}

macro_rules! target {
    ($id:ident, $name:literal, $module:expr, $counters:ident, $prepare:ident) => {
        pub static $id: TargetSpec = TargetSpec {
            name: $name,
            group: "_selftest",
            module: $module,
            versions: VERSIONS,
            free_threaded: true,
            counters: &$counters,
            prepare: $prepare,
            ..TargetSpec::DEFAULT
        };
    };
}

pub static METH: TargetSpec = TargetSpec {
    name: "_selftest.meth",
    group: "_selftest",
    module: None,
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_METH,
    prepare: prepare_meth,
    checkable: true,
    ..TargetSpec::DEFAULT
};
target!(VECTORCALL, "_selftest.vectorcall", Some(MODULE), C_VC, prepare_vectorcall);
target!(SLOT, "_selftest.slot", Some(MODULE), C_SLOT, prepare_slot);
target!(BADFP, "_selftest.badfp", Some(MODULE), C_NONE, prepare_badfp);
target!(REFUSE, "_selftest.refuse", None, C_NONE, prepare_refuse);
target!(CAS, "_selftest.cas", None, C_NONE, prepare_cas);
target!(SNAPSHOT, "_selftest.snapshot", Some(MODULE), C_SNAP, prepare_snapshot);
target!(SAME, "_selftest.same", Some(MODULE), C_SAME, prepare_same);

pub static TARGETS: &[&TargetSpec] =
    &[&METH, &VECTORCALL, &SLOT, &BADFP, &REFUSE, &CAS, &SNAPSHOT, &SAME, &SHARED_A, &SHARED_B];

// ---------------------------------------------------------------------------------------
// _selftest.shared_a / shared_b: two targets with shared declarations (`GroupSpec`).
// ---------------------------------------------------------------------------------------

static SHARED_DEPS: DepFlag = DepFlag::new();
static C_SHARED_A: Counters = Counters::new();
static C_SHARED_B: Counters = Counters::new();
static SHARED_TOOLS: Tools = Tools::new();

fn prepare_shared_group(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    if cx.get_opt(&g, "REFUSE_SHARED")?.is_some() {
        return Err(Refusal::new("shared refusal"));
    }
    cx.pin(&g, "SEP", &SHARED_DEPS)?;
    Ok(())
}

pub static SHARED: accel::GroupSpec = accel::GroupSpec { name: "_selftest.shared", prepare: prepare_shared_group };

/// # Safety
/// Vectorcall arguments from CPython; attached thread.
unsafe fn shared_kernel(c: &Counters, f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: per the caller.
    unsafe {
        if !(kw.is_null() && nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET == 1) {
            c.fallback_streak(Reason::Args);
        } else if !SHARED_DEPS.ok() {
            c.fallback_streak(Reason::Deps);
        } else if !accel::tools_quiet(&SHARED_TOOLS, f) {
            c.fallback_streak(Reason::Tools);
        } else {
            c.hit_streak();
            return ffi::Py_NewRef(*args);
        }
        accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw)
    }
}

unsafe extern "C" fn k_shared_a(f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: forwarded.
    unsafe { shared_kernel(&C_SHARED_A, f, args, nargsf, kw) }
}

unsafe extern "C" fn k_shared_b(f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: forwarded.
    unsafe { shared_kernel(&C_SHARED_B, f, args, nargsf, kw) }
}

fn prepare_shared_a(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let f = cx.module_attr("same_a")?;
    cx.vectorcall(&f, k_shared_a, &accel::FUNCTION_VECTORCALL)
}

fn prepare_shared_b(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let f = cx.module_attr("same_b")?;
    cx.vectorcall(&f, k_shared_b, &accel::FUNCTION_VECTORCALL)
}

pub static SHARED_A: TargetSpec = TargetSpec {
    name: "_selftest.shared_a",
    group: "_selftest",
    module: Some(MODULE),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_SHARED_A,
    prepare: prepare_shared_a,
    shared: Some(&SHARED),
    ..TargetSpec::DEFAULT
};

pub static SHARED_B: TargetSpec = TargetSpec {
    name: "_selftest.shared_b",
    group: "_selftest",
    module: Some(MODULE),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_SHARED_B,
    prepare: prepare_shared_b,
    shared: Some(&SHARED),
    ..TargetSpec::DEFAULT
};

/// The shared self-test group's dependency flag.
#[pyfunction]
fn _selftest_shared_ok() -> bool {
    SHARED_DEPS.ok()
}

// ---------------------------------------------------------------------------------------
// _selftest.same: an *exact* kernel (`def same(x): return x`) for the shared mechanics
// helpers (tests/targets/_mechanics.py), with a pinned module global `SEP`.
// ---------------------------------------------------------------------------------------

static C_SAME: Counters = Counters::new();
static SAME_DEPS: DepFlag = DepFlag::new();
static SAME_TOOLS: Tools = Tools::new();

unsafe extern "C" fn k_same(f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments from CPython; attached thread.
    unsafe {
        if !(kw.is_null() && nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET == 1) {
            C_SAME.fallback_streak(Reason::Args);
        } else if !SAME_DEPS.ok() {
            C_SAME.fallback_streak(Reason::Deps);
        } else if !accel::tools_quiet(&SAME_TOOLS, f) {
            C_SAME.fallback_streak(Reason::Tools);
        } else {
            C_SAME.hit_streak();
            return ffi::Py_NewRef(*args);
        }
        accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw)
    }
}

fn prepare_same(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let f = cx.module_attr("same")?;
    let g = cx.module()?.dict();
    cx.pin(&g, "SEP", &SAME_DEPS)?;
    cx.vectorcall(&f, k_same, &accel::FUNCTION_VECTORCALL)
}

// ---------------------------------------------------------------------------------------
// _selftest.snapshot: a snapshot pin (`pin_dict`) on an instance dict.
// ---------------------------------------------------------------------------------------

static C_SNAP: Counters = Counters::new();
static SNAP_DEPS: DepFlag = DepFlag::new();

unsafe extern "C" fn k_ident2(f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments from CPython; attached thread.
    unsafe {
        if kw.is_null() && nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET == 1 && SNAP_DEPS.ok() {
            C_SNAP.hit();
            return tagged(&KERNEL_STR, *args);
        }
        C_SNAP.fallback(Reason::Deps);
        accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw)
    }
}

fn prepare_snapshot(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let f = cx.module_attr("ident2")?;
    let obj = cx.module_attr("OBJ")?;
    let d = obj.getattr("__dict__")?.cast_into::<pyo3::types::PyDict>().map_err(PyErr::from)?;
    cx.pin_dict(&d, &SNAP_DEPS)?;
    cx.vectorcall(&f, k_ident2, &accel::FUNCTION_VECTORCALL)
}

/// The snapshot self-test's dependency flag.
#[pyfunction]
fn _selftest_snapshot_ok() -> bool {
    SNAP_DEPS.ok()
}

/// Which vectorcall a function has: "kernel" (a self-test kernel), "original"
/// (`_PyFunction_Vectorcall`) or "other".
#[pyfunction]
fn _selftest_vectorcall(func: &Bound<'_, pyo3::types::PyFunction>) -> &'static str {
    // SAFETY: an exact function object.
    let vc = unsafe { (*func.as_ptr().cast::<ffi::PyFunctionObject>()).vectorcall }.map_or(0, |f| f as usize);
    if vc == k_ident as ffi::vectorcallfunc as usize || vc == k_ident2 as ffi::vectorcallfunc as usize {
        "kernel"
    } else if vc == crate::accel::prepare::function_vectorcall() {
        "original"
    } else {
        "other"
    }
}

static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Generation watch on `dict[key]` with the self-test counter (idempotent).
#[pyfunction]
fn _selftest_watch(dict: &Bound<'_, pyo3::types::PyDict>, key: &str) -> PyResult<()> {
    accel::guard::watch(dict, key, &GEN)
}

static OVERRIDES: accel::guard::OverrideCache = accel::guard::OverrideCache::new(&["__fspath__", "join"]);

/// Override verdict of `cls` against `base` (sets the base when given).
#[pyfunction]
#[pyo3(signature = (cls, base=None))]
fn _selftest_overrides(cls: &Bound<'_, PyType>, base: Option<&Bound<'_, PyType>>) -> Option<bool> {
    if let Some(b) = base {
        OVERRIDES.set_base(b);
    }
    // SAFETY: attached thread; live type.
    unsafe { OVERRIDES.overrides(cls.as_type_ptr()) }
}

static GEN_DICT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whole-dict generation watch on `dict` (idempotent); returns the counter.
#[pyfunction]
fn _selftest_watch_dict(dict: &Bound<'_, pyo3::types::PyDict>) -> PyResult<u64> {
    accel::guard::watch_dict_generation(dict, &GEN_DICT)?;
    Ok(GEN_DICT.load(Ordering::Relaxed))
}

/// The self-test generation counter.
#[pyfunction]
fn _selftest_gen() -> u64 {
    GEN.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------------------
// Python helpers
// ---------------------------------------------------------------------------------------

/// Undo `_selftest.cas`'s simulated foreign patch.
#[pyfunction]
fn _selftest_untamper() {
    // SAFETY: static probe entry, METH_O function.
    unsafe { set_meth(2, probe_orig as ffi::PyCFunction as usize) };
}

/// Whether the probe entries hold their original functions: `[_probe, _probe_a, _probe_b]`.
#[pyfunction]
fn _selftest_probe_state() -> Vec<bool> {
    (0..3)
        .map(|i| {
            // SAFETY: static probe entry.
            let cur = unsafe {
                (*(&raw const (*def(i)).ml_meth).cast::<std::sync::atomic::AtomicUsize>()).load(Ordering::Acquire)
            };
            cur == probe_orig as ffi::PyCFunction as usize
        })
        .collect()
}

/// The vectorcall self-test's dependency flag.
#[pyfunction]
fn _selftest_deps_ok() -> bool {
    DEPS.ok()
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    KERNEL_STR.store(PyString::intern(py, "kernel").into_ptr(), Ordering::Relaxed);
    ORIG_STR.store(PyString::intern(py, "orig").into_ptr(), Ordering::Relaxed);
    for (i, name) in [(0, "_probe"), (1, "_probe_a"), (2, "_probe_b")] {
        // SAFETY: a static method def; module name string.
        let f = unsafe {
            Bound::from_owned_ptr_or_err(
                py,
                ffi::PyCFunction_NewEx(def(i), std::ptr::null_mut(), std::ptr::null_mut()),
            )?
        };
        m.add(name, f)?;
    }
    m.add_function(wrap_pyfunction!(_selftest_untamper, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_probe_state, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_deps_ok, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_snapshot_ok, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_vectorcall, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_watch, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_gen, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_overrides, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_watch_dict, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_shared_ok, m)?)?;
    Ok(())
}
