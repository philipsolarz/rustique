//! FFI helpers shared by the data-model groups (`enum`, `generated`, `contextlib`,
//! `functools`): raw function/cell/type field access, MRO lookups that never run Python
//! code, interned names, argument binding of plain Python signatures and the plans hung on
//! code objects (`co_extra`).
//!
//! Everything here is called by kernels on an attached thread and runs no Python code
//! unless its doc says so.

use std::ffi::{c_int, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicIsize, AtomicPtr, AtomicU32, Ordering::Relaxed};

use pyo3::ffi;
use pyo3::prelude::*;

pub type P = *mut ffi::PyObject;
pub type Tp = *mut ffi::PyTypeObject;

python_extern! {
    pub fn _PyType_LookupRef(t: Tp, name: P) -> P;
    pub fn PyFunction_SetVectorcall(f: *mut ffi::PyFunctionObject, vc: ffi::vectorcallfunc);
    pub fn PyUnstable_Code_GetExtra(code: P, index: ffi::Py_ssize_t, extra: *mut *mut c_void) -> c_int;
    pub fn PyUnstable_Code_SetExtra(code: P, index: ffi::Py_ssize_t, extra: *mut c_void) -> c_int;
    pub fn PyUnstable_Eval_RequestCodeExtraIndex(f: ffi::freefunc) -> ffi::Py_ssize_t;
    pub fn PyUnstable_Type_AssignVersionTag(t: Tp) -> c_int;
    pub fn PyWeakref_GetRef(r: P, out: *mut P) -> c_int;
}

/// A slot or function pointer as an address (0 for an empty slot), for comparisons.
#[macro_export]
macro_rules! dm_addr {
    ($e:expr) => {
        $e.map_or(0usize, |f| f as usize)
    };
}
pub use dm_addr as addr;

/// `_PyFunction_Vectorcall`'s address (not an exported symbol on 3.14: read from a live
/// function object once).
#[inline(always)]
pub fn function_vectorcall() -> usize {
    crate::accel::prepare::function_vectorcall()
}

/// `_PyFunction_Vectorcall` itself.
#[inline(always)]
pub fn func_vectorcall() -> ffi::vectorcallfunc {
    // SAFETY: the address of CPython's `_PyFunction_Vectorcall` (never 0 once read).
    unsafe { std::mem::transmute::<usize, ffi::vectorcallfunc>(function_vectorcall()) }
}

/// Positional argument count of a vectorcall.
#[inline(always)]
pub fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

/// `_PyFunction_Vectorcall` (the original of every vectorcall site of these groups).
#[inline(always)]
pub unsafe fn orig(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: per the caller (a live function object and a valid vectorcall).
    unsafe { crate::accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw) }
}

/// The type's version tag (relaxed atomic read: CPython writes it atomically on
/// free-threaded builds). 0 = no valid tag.
#[inline(always)]
pub unsafe fn tag_of(t: Tp) -> u32 {
    // SAFETY: aligned u32 field of a live type.
    unsafe { (*(&raw const (*t).tp_version_tag).cast::<AtomicU32>()).load(Relaxed) }
}

/// Assign a version tag if the type has none; returns the tag (0 if exhausted).
pub unsafe fn ensure_tag(t: Tp) -> u32 {
    // SAFETY: attached thread, live type.
    unsafe {
        if PyUnstable_Type_AssignVersionTag(t) == 0 {
            if !ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_Clear();
            }
            return 0;
        }
        tag_of(t)
    }
}

/// `ty.__mro__` lookup of `name` (raw class attribute, descriptors not run): a new
/// reference or NULL (no error set).
#[inline(always)]
pub unsafe fn lookup(t: Tp, name: P) -> P {
    // SAFETY: attached thread; `_PyType_LookupRef` runs no Python code.
    unsafe { _PyType_LookupRef(t, name) }
}

/// `name` resolves (MRO) to exactly `expected` (NULL: absent).
#[inline(always)]
pub unsafe fn resolves_to(t: Tp, name: P, expected: P) -> bool {
    // SAFETY: per `lookup`.
    unsafe {
        let r = lookup(t, name);
        if !r.is_null() {
            ffi::Py_DECREF(r);
        }
        r == expected
    }
}

/// The class attribute `name` is absent or a plain value: `obj.name` through the generic
/// protocol reads the instance attribute (or that value) without running code.
#[inline]
pub unsafe fn plain_attr(t: Tp, name: P) -> bool {
    // SAFETY: per `lookup`.
    unsafe {
        let r = lookup(t, name);
        if r.is_null() {
            return true;
        }
        let rt = ffi::Py_TYPE(r);
        let ok = (*rt).tp_descr_get.is_none() && (*rt).tp_descr_set.is_none();
        ffi::Py_DECREF(r);
        ok
    }
}

/// Instances use the generic attribute protocol (`object.__getattribute__`, no
/// `__getattr__`).
#[inline(always)]
pub unsafe fn generic_getattr(t: Tp) -> bool {
    // SAFETY: field read of a live type.
    unsafe { addr!((*t).tp_getattro) == ffi::PyObject_GenericGetAttr as *const () as usize }
}

/// `obj.name` through `PyObject_GenericGetAttr` (validated types only: generic protocol, no
/// descriptor for `name`), or NULL with no error set.
#[inline(always)]
pub unsafe fn get_plain(obj: P, name: P) -> P {
    // SAFETY: per the caller.
    unsafe {
        let r = ffi::PyObject_GenericGetAttr(obj, name);
        if r.is_null() {
            ffi::PyErr_Clear();
        }
        r
    }
}

/// The function's code object (borrowed).
#[inline(always)]
pub unsafe fn func_code(f: P) -> P {
    // SAFETY: a live function object.
    unsafe { (*f.cast::<ffi::PyFunctionObject>()).func_code }
}

/// Is `o` an exact pure-Python function?
#[inline(always)]
pub unsafe fn is_function(o: P) -> bool {
    // SAFETY: live object.
    unsafe { ffi::Py_TYPE(o) == &raw mut ffi::PyFunction_Type }
}

/// The current vectorcall of a function object.
#[inline(always)]
pub unsafe fn vectorcall_of(f: P) -> usize {
    // SAFETY: a live function object.
    unsafe { (*f.cast::<ffi::PyFunctionObject>()).vectorcall.map_or(0, |v| v as usize) }
}

/// Content of closure cell `i` of `func` (new reference) or NULL (empty cell, no closure,
/// out of range; no error set).
#[inline(always)]
pub unsafe fn cell(func: P, i: usize) -> P {
    // SAFETY: a live function object; `PyCell_Get` locks the cell on free-threaded builds.
    unsafe {
        let cl = (*func.cast::<ffi::PyFunctionObject>()).func_closure;
        if cl.is_null() || ffi::PyTuple_CheckExact(cl) == 0 || i >= ffi::PyTuple_GET_SIZE(cl) as usize {
            return null_mut();
        }
        let c = ffi::PyTuple_GET_ITEM(cl, i as ffi::Py_ssize_t);
        if ffi::Py_TYPE(c) != &raw mut ffi::PyCell_Type {
            return null_mut();
        }
        let v = ffi::PyCell_Get(c);
        if v.is_null() && !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
        }
        v
    }
}

/// Cell `i` of `func` holds exactly `expected`.
#[inline(always)]
pub unsafe fn cell_is(func: P, i: usize, expected: P) -> bool {
    // SAFETY: per `cell`.
    unsafe {
        let v = cell(func, i);
        if v.is_null() {
            return false;
        }
        ffi::Py_DECREF(v);
        v == expected
    }
}

/// A new strong reference.
#[inline(always)]
pub unsafe fn incref(o: P) -> P {
    // SAFETY: live object.
    unsafe {
        ffi::Py_INCREF(o);
        o
    }
}

/// An interned `str` kept for the process (immortal once interned on 3.14+).
pub fn intern(py: Python<'_>, s: &str) -> P {
    pyo3::types::PyString::intern(py, s).into_ptr()
}

/// A process-lifetime slot for an interned name or a kept object.
pub struct Slot(AtomicPtr<ffi::PyObject>);

impl Slot {
    pub const fn new() -> Self {
        Slot(AtomicPtr::new(null_mut()))
    }
    #[inline(always)]
    pub fn get(&self) -> P {
        self.0.load(Relaxed)
    }
    /// Store a name (interned once; later calls keep the first).
    pub fn name(&self, py: Python<'_>, s: &str) {
        if self.0.load(Relaxed).is_null() {
            self.0.store(intern(py, s), Relaxed);
        }
    }
    /// Keep a strong reference to `o` (replacing and releasing a previous one).
    pub fn keep(&self, o: &Bound<'_, PyAny>) {
        let old = self.0.swap(o.clone().into_ptr(), Relaxed);
        if !old.is_null() {
            // SAFETY: the reference this slot held.
            unsafe { ffi::Py_DECREF(old) };
        }
    }
    /// The kept object as a bound reference.
    pub fn get_bound<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        // SAFETY: a live object kept by the slot.
        unsafe { Bound::from_borrowed_ptr(py, self.get()) }
    }
    /// Drop the kept reference (exit).
    pub fn release(&self) {
        let old = self.0.swap(null_mut(), Relaxed);
        if !old.is_null() {
            // SAFETY: the reference this slot held.
            unsafe { ffi::Py_DECREF(old) };
        }
    }
}

impl Default for Slot {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------------------
// Argument binding of a plain signature (no *args/**kwargs, no positional-only).
// ---------------------------------------------------------------------------------------

/// Parameter facts of a function with a plain signature, parameters after the first
/// (`self` / `_cls`): interned names, how many are positional-or-keyword.
pub struct Sig {
    pub names: Vec<P>,
    pub npos: usize,
}

// SAFETY: interned (immortal) strings, read-only after construction.
unsafe impl Send for Sig {}
// SAFETY: as Send.
unsafe impl Sync for Sig {}

/// Most parameters a binding handles (larger signatures run the original).
pub const MAX_PARAMS: usize = 32;

/// Bind `args[1..nargs]` and the keywords to the parameters, missing ones from
/// `__defaults__`/`__kwdefaults__`. Fills `out[..names.len()]` with **new references**
/// (taken before anything can run: a finalizer replacing `__defaults__` cannot free them).
/// False (nothing held) when anything is unusual: too many arguments, an unknown, duplicate
/// or non-identical keyword, a missing argument without default: the original then binds,
/// and raises if it must.
#[inline]
pub unsafe fn bind(sig: &Sig, func: P, args: *const P, n: usize, kw: P, out: &mut [P; MAX_PARAMS]) -> bool {
    // SAFETY: a live function; `args` holds `n` positional values then one per keyword.
    unsafe {
        let np = sig.names.len();
        if np > MAX_PARAMS || n == 0 || n - 1 > sig.npos {
            return false;
        }
        for (i, o) in out.iter_mut().enumerate().take(np) {
            *o = if i < n - 1 { *args.add(i + 1) } else { null_mut() };
        }
        if !kw.is_null() {
            let nk = ffi::PyTuple_GET_SIZE(kw) as usize;
            'k: for j in 0..nk {
                let k = ffi::PyTuple_GET_ITEM(kw, j as ffi::Py_ssize_t);
                for (i, &nm) in sig.names.iter().enumerate() {
                    if nm == k {
                        if !out[i].is_null() {
                            return false;
                        }
                        out[i] = *args.add(n + j);
                        continue 'k;
                    }
                }
                return false;
            }
        }
        let fo = func.cast::<ffi::PyFunctionObject>();
        let defs = (*fo).func_defaults;
        let nd = if defs.is_null() { 0 } else { ffi::PyTuple_GET_SIZE(defs) as usize };
        let kd = (*fo).func_kwdefaults;
        // Every slot is either an argument or a default: decide before taking references.
        for i in 0..np {
            if !out[i].is_null() {
                continue;
            }
            if i < sig.npos {
                if i + nd < sig.npos {
                    return false;
                }
                out[i] = ffi::PyTuple_GET_ITEM(defs, (i + nd - sig.npos) as ffi::Py_ssize_t);
            } else {
                if kd.is_null() || ffi::PyDict_CheckExact(kd) == 0 {
                    return false;
                }
                // An exact dict whose keys are parameter names (str): no Python code.
                let v = ffi::PyDict_GetItemWithError(kd, sig.names[i]);
                if v.is_null() {
                    ffi::PyErr_Clear();
                    return false;
                }
                out[i] = v;
            }
        }
        for o in out.iter_mut().take(np) {
            ffi::Py_INCREF(*o);
        }
        true
    }
}

/// Release the references [`bind`] took.
#[inline]
pub unsafe fn unbind(out: &[P; MAX_PARAMS], np: usize) {
    // SAFETY: references taken by `bind`.
    unsafe {
        for &o in out.iter().take(np) {
            ffi::Py_DECREF(o);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Plans on code objects (co_extra)
// ---------------------------------------------------------------------------------------

/// Header of every plan hung on a code object: its kind and how to free it.
#[repr(C)]
pub struct PlanHeader {
    pub tag: u32,
    pub free: unsafe fn(*mut PlanHeader),
}

static EXTRA_INDEX: AtomicIsize = AtomicIsize::new(-1);

unsafe extern "C" fn free_plan(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    let h = p.cast::<PlanHeader>();
    // SAFETY: every extra under our index is a boxed plan starting with a PlanHeader.
    unsafe { ((*h).free)(h) }
}

/// The code-extra index of the data-model plans (requested once per process).
pub fn extra_index() -> Option<ffi::Py_ssize_t> {
    let i = EXTRA_INDEX.load(Relaxed);
    if i >= 0 {
        return Some(i);
    }
    // SAFETY: attached thread (callers are attached).
    let i = unsafe { PyUnstable_Eval_RequestCodeExtraIndex(free_plan) };
    if i < 0 {
        // SAFETY: attached thread.
        unsafe { ffi::PyErr_Clear() };
        return None;
    }
    match EXTRA_INDEX.compare_exchange(-1, i, Relaxed, Relaxed) {
        Ok(_) => Some(i),
        Err(prev) => Some(prev),
    }
}

/// The plan on `code` (NULL if none or another kind).
#[inline(always)]
pub unsafe fn plan_of(code: P, tag: u32) -> *const PlanHeader {
    let i = EXTRA_INDEX.load(Relaxed);
    if i < 0 {
        return std::ptr::null();
    }
    let mut p: *mut c_void = null_mut();
    // SAFETY: a live code object; our index.
    unsafe {
        if PyUnstable_Code_GetExtra(code, i, &mut p) != 0 {
            ffi::PyErr_Clear();
            return std::ptr::null();
        }
        let h = p.cast::<PlanHeader>();
        if h.is_null() || (*h).tag != tag {
            return std::ptr::null();
        }
        h
    }
}

/// Hang `plan` on `code`, once: a code object that already carries a plan of ours keeps it
/// (a plan is never freed while a kernel might read it; CPython frees it with the code
/// object). False when not set (the plan is freed).
pub unsafe fn set_plan(code: P, plan: *mut PlanHeader) -> bool {
    let Some(i) = extra_index() else {
        // SAFETY: a plan we own.
        unsafe { ((*plan).free)(plan) };
        return false;
    };
    // SAFETY: a live code object; `plan` is ours until stored.
    unsafe {
        let mut old: *mut c_void = null_mut();
        if PyUnstable_Code_GetExtra(code, i, &mut old) != 0 {
            ffi::PyErr_Clear();
            ((*plan).free)(plan);
            return false;
        }
        if !old.is_null() || PyUnstable_Code_SetExtra(code, i, plan.cast()) != 0 {
            if !ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_Clear();
            }
            ((*plan).free)(plan);
            return false;
        }
        true
    }
}
