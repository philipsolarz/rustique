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

struct Defs(UnsafeCell<[ffi::PyMethodDef; 5]>);
// SAFETY: written only through atomic pointer stores of `ml_meth` (sites) at install.
unsafe impl Sync for Defs {}

static KERNEL_STR: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static ORIG_STR: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());

/// `("orig"|"kernel", x)`.
unsafe fn tagged(tag: &AtomicPtr<ffi::PyObject>, x: O) -> O {
    // SAFETY: attached thread; the tag is a live interned str kept forever (created here
    // if the helpers were never registered: kernels must not depend on `_accel`'s extras).
    unsafe {
        let mut t = tag.load(Ordering::Acquire);
        if t.is_null() {
            let s: &std::ffi::CStr = if std::ptr::eq(tag, &KERNEL_STR) { c"kernel" } else { c"orig" };
            let new = ffi::PyUnicode_InternFromString(s.as_ptr());
            if new.is_null() {
                return std::ptr::null_mut();
            }
            t = match tag.compare_exchange(std::ptr::null_mut(), new, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => new,
                Err(cur) => {
                    ffi::Py_DECREF(new);
                    cur
                }
            };
        }
        ffi::PyTuple_Pack(2, t, x)
    }
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
    ffi::PyMethodDef {
        ml_name: c"_probe_t".as_ptr(),
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
            release: Some(release),
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
    release: Some(release),
    ..TargetSpec::DEFAULT
};
target!(VECTORCALL, "_selftest.vectorcall", Some(MODULE), C_VC, prepare_vectorcall);
target!(SLOT, "_selftest.slot", Some(MODULE), C_SLOT, prepare_slot);
target!(BADFP, "_selftest.badfp", Some(MODULE), C_NONE, prepare_badfp);
target!(REFUSE, "_selftest.refuse", None, C_NONE, prepare_refuse);
target!(CAS, "_selftest.cas", None, C_NONE, prepare_cas);
target!(SNAPSHOT, "_selftest.snapshot", Some(MODULE), C_SNAP, prepare_snapshot);
target!(SAME, "_selftest.same", Some(MODULE), C_SAME, prepare_same);

pub static TARGETS: &[&TargetSpec] = &[
    &METH,
    &VECTORCALL,
    &SLOT,
    &BADFP,
    &REFUSE,
    &CAS,
    &SNAPSHOT,
    &SAME,
    &SHARED_A,
    &SHARED_B,
    &TIER,
    &OPCODE,
    &HEAP,
    &DYN,
];

// ---------------------------------------------------------------------------------------
// _selftest.heap: `tp_descr_get` of the Python class `_rustique_selftest.Prop` (a heap-type
// slot holding CPython's `slot_tp_descr_get`), checked against `Prop2`'s.
// _selftest.dyn: dynamic vectorcall sites only; `on_install` adopts `made_before`.
// ---------------------------------------------------------------------------------------

static C_HEAP: Counters = Counters::new();
static ORIG_DESCR: Original<ffi::descrgetfunc> = Original::new();
static PROP_TYPE: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(std::ptr::null_mut());

unsafe extern "C" fn k_descr_get(d: O, obj: O, ty: O) -> O {
    // SAFETY: tp_descr_get call.
    unsafe {
        if ffi::Py_TYPE(d) == PROP_TYPE.load(Ordering::Relaxed) && !obj.is_null() {
            C_HEAP.hit();
            return tagged(&KERNEL_STR, obj);
        }
        C_HEAP.fallback(Reason::Args);
        ORIG_DESCR.get()(d, obj, ty)
    }
}

fn prepare_heap(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let prop = cx.module_attr("Prop")?.cast_into::<PyType>().map_err(PyErr::from)?;
    let prop2 = cx.module_attr("Prop2")?.cast_into::<PyType>().map_err(PyErr::from)?;
    // Kept alive by the site (and compared by address only).
    PROP_TYPE.store(prop.as_type_ptr(), Ordering::Relaxed);
    cx.heap_slot(&prop, SlotName::TpDescrGet, k_descr_get as ffi::descrgetfunc, &ORIG_DESCR, Some(&prop2))
}

pub static HEAP: TargetSpec = TargetSpec {
    name: "_selftest.heap",
    group: "_selftest",
    module: Some(MODULE),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_HEAP,
    prepare: prepare_heap,
    ..TargetSpec::DEFAULT
};

static C_DYN: Counters = Counters::new();
static DYN_TOOLS: Tools = Tools::new();

unsafe extern "C" fn k_dyn(f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments from CPython; attached thread.
    unsafe {
        if kw.is_null() && nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET == 1 && accel::tools_quiet(&DYN_TOOLS, f) {
            C_DYN.hit();
            return tagged(&KERNEL_STR, *args);
        }
        C_DYN.fallback(Reason::Args);
        accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw)
    }
}

fn prepare_dyn(_py: Python<'_>, _cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if !accel::dynamic::capture_function_vectorcall(&accel::FUNCTION_VECTORCALL) {
        return Err(Refusal::new("vectorcall"));
    }
    Ok(())
}

/// Adopt `made_before` (created before enable()).
fn dyn_on_install(py: Python<'_>) {
    let Ok(Some(m)) = PyModule::import(py, "sys")
        .and_then(|s| s.getattr("modules"))
        .and_then(|d| d.get_item(MODULE).map(Some).or_else(|_| Ok(None)))
    else {
        return;
    };
    if let Ok(f) = m.getattr("made_before") {
        accel::dynamic::install(py, &DYN, &f, k_dyn, &accel::FUNCTION_VECTORCALL);
    }
}

pub static DYN: TargetSpec = TargetSpec {
    name: "_selftest.dyn",
    group: "_selftest",
    module: Some(MODULE),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_DYN,
    prepare: prepare_dyn,
    on_install: Some(dyn_on_install),
    ..TargetSpec::DEFAULT
};

/// `accel::dynamic::install` of `_selftest.dyn`'s kernel on `func`.
#[pyfunction]
fn _selftest_dyn_install(func: &Bound<'_, PyAny>) -> bool {
    accel::dynamic::install(func.py(), &DYN, func, k_dyn, &accel::FUNCTION_VECTORCALL)
}

static CACHE: accel::ClassCache<16> = accel::ClassCache::new();

/// The self-test class cache: `get` (payload None) or `begin` + `put` (payload given).
#[pyfunction]
#[pyo3(signature = (cls, payload=None))]
fn _selftest_class_cache(cls: &Bound<'_, PyType>, payload: Option<u32>) -> Option<u32> {
    let tp = cls.as_type_ptr();
    // SAFETY: attached thread; live type.
    unsafe {
        match payload {
            None => CACHE.get(tp),
            Some(p) => {
                let (tag, epoch) = CACHE.begin(tp);
                CACHE.put(tp, tag, epoch, p);
                CACHE.get(tp)
            }
        }
    }
}

/// Changes of `d` invalidate `cls`'s entry of the self-test class cache.
#[pyfunction]
fn _selftest_class_cache_watch(d: &Bound<'_, pyo3::types::PyDict>, cls: &Bound<'_, PyType>) -> PyResult<()> {
    accel::guard::watch_dict_invalidates(d, &CACHE, cls.as_type_ptr() as usize)
}

// ---------------------------------------------------------------------------------------
// _selftest.tier: an opt-in target (tier `_selftest_tier`), `ml_meth` on `_accel._probe_t`.
// _selftest.opcode: an opcode-handler site on a private 4-entry "handler table".
// ---------------------------------------------------------------------------------------

static C_TIER: Counters = Counters::new();
static ORIG_T: Original<ffi::PyCFunction> = Original::new();

unsafe extern "C" fn k_tier(_m: O, x: O) -> O {
    // SAFETY: METH_O call.
    unsafe { tagged(&KERNEL_STR, x) }
}

fn prepare_tier(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    // SAFETY: a static, writable method table entry.
    unsafe { cx.ml_meth_def("_accel._probe_t", def(3), k_tier as ffi::PyCFunction, &ORIG_T) }
}

pub static TIER: TargetSpec = TargetSpec {
    name: "_selftest.tier",
    group: "_selftest",
    module: None,
    versions: VERSIONS,
    free_threaded: true,
    opt_in: Some("_selftest_tier"),
    counters: &C_TIER,
    prepare: prepare_tier,
    ..TargetSpec::DEFAULT
};

/// A fake handler table: entry 1 is the swapped cell.
static TABLE: [std::sync::atomic::AtomicUsize; 4] = [const { std::sync::atomic::AtomicUsize::new(0) }; 4];
const HANDLER_CORE: usize = 0x1000;
const HANDLER_OURS: usize = 0x2000;

fn prepare_opcode(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let _ = TABLE[1].compare_exchange(0, HANDLER_CORE, Ordering::AcqRel, Ordering::Acquire);
    cx.opcode_handler("_selftest.TABLE[1]", (&raw const TABLE[1]) as usize, HANDLER_CORE, HANDLER_OURS)
}

pub static OPCODE: TargetSpec = TargetSpec {
    name: "_selftest.opcode",
    group: "_selftest",
    module: None,
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_NONE,
    prepare: prepare_opcode,
    ..TargetSpec::DEFAULT
};

/// The fake handler table entry: "core", "ours", or the raw value.
#[pyfunction]
fn _selftest_handler() -> String {
    match TABLE[1].load(Ordering::Acquire) {
        HANDLER_CORE => "core".into(),
        HANDLER_OURS => "ours".into(),
        v => format!("{v:#x}"),
    }
}

/// Request a net-benefit suspension of a self-test target (any site kind), as a kernel's
/// fallback would.
#[pyfunction]
fn _selftest_request_suspend(name: &str) -> PyResult<()> {
    let spec = TARGETS
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("unknown self-test target"))?;
    spec.counters.suspend_requested.store(true, Ordering::Release);
    crate::accel::registry::suspend_from_kernel();
    Ok(())
}

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

/// `accel::degrade` on the named self-test target.
#[pyfunction]
fn _selftest_degrade(name: &str, reason: &str) -> PyResult<()> {
    let spec = TARGETS
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("unknown self-test target"))?;
    accel::degrade(spec, reason);
    Ok(())
}

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

static RELEASED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The self-test targets' exit release (one function shared by all: called once).
fn release(_py: Python<'_>) {
    RELEASED.fetch_add(1, Ordering::Relaxed);
}

/// How many times the self-test release ran.
#[pyfunction]
fn _selftest_released() -> u32 {
    RELEASED.load(Ordering::Relaxed)
}

/// Call `_selftest.vectorcall`'s kernel directly on `func` (a stale path: whatever the
/// function's current vectorcall is).
#[pyfunction]
fn _selftest_call_kernel<'py>(func: &Bound<'py, PyAny>, x: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let args = [x.as_ptr()];
    // SAFETY: a vectorcall of one positional argument on a live object; attached thread.
    unsafe { Bound::from_owned_ptr_or_err(func.py(), k_ident(func.as_ptr(), args.as_ptr(), 1, std::ptr::null_mut())) }
}

/// The vectorcall self-test's dependency flag.
#[pyfunction]
fn _selftest_deps_ok() -> bool {
    DEPS.ok()
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    for (tag, text) in [(&KERNEL_STR, "kernel"), (&ORIG_STR, "orig")] {
        let new = PyString::intern(py, text).into_ptr();
        if tag.compare_exchange(std::ptr::null_mut(), new, Ordering::AcqRel, Ordering::Acquire).is_err() {
            // SAFETY: our own new reference (a kernel created the tag first).
            unsafe { ffi::Py_DECREF(new) };
        }
    }
    for (i, name) in [(0, "_probe"), (1, "_probe_a"), (2, "_probe_b"), (3, "_probe_t")] {
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
    m.add_function(wrap_pyfunction!(_selftest_degrade, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_released, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_call_kernel, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_handler, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_dyn_install, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_class_cache, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_class_cache_watch, m)?)?;
    m.add_function(wrap_pyfunction!(_selftest_request_suspend, m)?)?;
    Ok(())
}
