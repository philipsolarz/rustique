//! Target `collections.namedtuple` (group `generated`): the methods `namedtuple()`
//! generates — `__new__` (reached from `slot_tp_new`, so a fallback costs a few
//! instructions), `_make` and `_replace`/`__replace__` (called from bytecode).
//!
//! Recognition, by construction. The hook is a vectorcall kernel on `collections.namedtuple`
//! itself: it calls CPython's original and then looks at the class it returned. With the
//! function's code fingerprinted (its nested `_make`/`_replace` code objects are constants
//! of that code, so they are covered) and every global the generated text depends on pinned
//! (`eval`, `map`, `tuple`, `type`, `_sys` and `sys.intern`), the class's `__new__` is
//! `eval(f'lambda _cls, {arg_list}: _tuple_new(_cls, ({arg_list}))')` for `arg_list` built
//! from the class's `_fields`, compiled by CPython moments before; nothing between that
//! `eval` and the hook's return can run user code (`type(...)` with metaclass `type`, a
//! `tuple` base without `__init_subclass__`, `type.__setattr__` for `__module__`). The kernel
//! is installed only if the code object also passes structural checks (argument counts,
//! flags, `co_varnames == ('_cls', *_fields)`, `co_names == ('_tuple_new',)`, no cells).
//! No reference is compiled: a compile would be an extra `compile` audit event (audit
//! hooks are user code). `_make` and `_replace` are the same code objects for every class
//! (identity with the fingerprinted constants), so classes created before `enable()` get
//! them too (scan of `tuple.__subclasses__()` at install, GIL builds).
//!
//! Exact subset (everything else runs CPython's function):
//! - `__new__(cls, *values)`: arguments bind to the parameters (positional, keywords by
//!   identity with the interned field names, `__defaults__`), the lambda's globals still map
//!   `_tuple_new` to `tuple.__new__`, and `tuple.__new__(cls, ...)` would pass its checks
//!   (`cls` a tuple subtype whose first base with a C `tp_new` is `tuple`): the instance is
//!   built as `tuple_subtype_new` does (`tp_alloc`, items, hash cache reset, GC tracking).
//! - `_make(cls, iterable)`: an exact `tuple`/`list` of exactly `len(_fields)` items, cells
//!   `tuple_new`/`_len`/`num_fields` unchanged.
//! - `_replace(self, /, **kwds)`: every keyword (identical interned name) is a field, `self`
//!   iterates as a tuple, has no instance dict, and `self._make` is the generated `_make`
//!   with unchanged cells.

use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering::*};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyCode, PyTuple, PyType};

use super::adapter::Dynamic;
use super::common::*;
use crate::accel::{self, Counters, DepFlag, Prepare, Reason, Refusal, TargetSpec, Tools};

pub(super) static COUNTERS: Counters = Counters::new();
static DEPS: DepFlag = DepFlag::new();
static TOOLS: Tools = Tools::new();
pub(super) static DYN: Dynamic = Dynamic::new();

/// Not on free-threaded builds this wave (lead's ruling): giving each generated function its
/// kernel calls `PyFunction_SetVectorcall`, which stops the world on 3.14t/3.15t; with busy
/// threads each stop waits for every thread to reach a safe point (measured:
/// `benches/datamodel/ft_create.py`). Static sites are installed at enable/import (§3.10).
pub static TARGET: TargetSpec = TargetSpec {
    name: "collections.namedtuple",
    group: "generated",
    module: Some("collections"),
    versions: &[(3, 14), (3, 15)],
    free_threaded: false,
    counters: &COUNTERS,
    prepare,
    release: Some(release),
    ..TargetSpec::DEFAULT
};

mod fp {
    use crate::accel::Fingerprints;
    pub const NAMEDTUPLE: Fingerprints = &[(3, 14, "9f9ab068d143d8e0"), (3, 15, "4a93df023ec11540")];
}

const NT_TAG: u32 = 0x4E54_0001;

/// Objects and names kept for the process.
static NT_CODE: Slot = Slot::new();
static MAKE_CODE: Slot = Slot::new();
static REPLACE_CODE: Slot = Slot::new();
static TUPLE_NEW: Slot = Slot::new();
static BUILTIN_LEN: Slot = Slot::new();
static N_TUPLE_NEW: Slot = Slot::new();
static N_NEW: Slot = Slot::new();
static N_MAKE: Slot = Slot::new();
static N_REPLACE: Slot = Slot::new();
static N_FIELDS: Slot = Slot::new();
static N_CLS: Slot = Slot::new();
/// `slot_tp_new` (read from a namedtuple class: its `__new__` is in its dict).
static SLOT_TP_NEW: AtomicUsize = AtomicUsize::new(0);
/// Closure-cell indices of `_make` (`_len`, `num_fields`, `tuple_new`) and `_replace`
/// (`_map`, `field_names`), from their code objects' `co_freevars`.
static MAKE_CELLS: [AtomicUsize; 3] = [const { AtomicUsize::new(usize::MAX) }; 3];
static REPLACE_CELLS: [AtomicUsize; 2] = [const { AtomicUsize::new(usize::MAX) }; 2];

#[repr(C)]
struct NtPlan {
    h: PlanHeader,
    sig: Sig,
}

unsafe fn free_nt_plan(p: *mut PlanHeader) {
    // SAFETY: created by Box::into_raw of an NtPlan.
    drop(unsafe { Box::from_raw(p.cast::<NtPlan>()) });
}

fn release(_py: Python<'_>) {
    DYN.release();
    for s in [&NT_CODE, &MAKE_CODE, &REPLACE_CODE, &TUPLE_NEW, &BUILTIN_LEN] {
        s.release();
    }
}

// ---------------------------------------------------------------------------------------
// Construction: tuple.__new__(cls, items)
// ---------------------------------------------------------------------------------------

/// `tuple.__new__(cls, items)` for exact items: `tp_new_wrapper`'s checks, then
/// `tuple_subtype_new`. `None` when a check would fail (the original raises); `Some(NULL)`
/// with an exception set on allocation failure.
#[inline]
unsafe fn construct(cls: P, items: &[P]) -> Option<P> {
    // SAFETY: attached thread; live objects; borrowed items (references taken here).
    unsafe {
        if ffi::PyType_Check(cls) == 0 {
            return None;
        }
        let ct = cls.cast::<ffi::PyTypeObject>();
        let tuple_t = &raw mut ffi::PyTuple_Type;
        if ct == tuple_t || ffi::PyType_IsSubtype(ct, tuple_t) == 0 {
            return None;
        }
        let slot_new = SLOT_TP_NEW.load(Relaxed);
        if slot_new == 0 {
            return None;
        }
        let mut sb = ct;
        while !sb.is_null() && (*sb).tp_new.map_or(0, |f| f as usize) == slot_new {
            sb = (*sb).tp_base;
        }
        if !sb.is_null() && addr!((*sb).tp_new) != addr!((*tuple_t).tp_new) {
            return None;
        }
        let alloc = (*ct).tp_alloc?;
        let obj = alloc(ct, items.len() as ffi::Py_ssize_t);
        if obj.is_null() {
            return Some(null_mut());
        }
        for (i, &v) in items.iter().enumerate() {
            ffi::PyTuple_SET_ITEM(obj, i as ffi::Py_ssize_t, incref(v));
        }
        #[cfg(Py_3_14)]
        {
            (*obj.cast::<ffi::PyTupleObject>()).ob_hash = -1;
        }
        if ffi::PyObject_GC_IsTracked(obj) == 0 {
            ffi::PyObject_GC_Track(obj.cast());
        }
        Some(obj)
    }
}

// ---------------------------------------------------------------------------------------
// Kernels
// ---------------------------------------------------------------------------------------

/// The generated `__new__` (reached from `slot_tp_new`).
unsafe extern "C" fn k_nt_new(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol; `f` is a function this target patched.
    unsafe {
        let plan = plan_of(func_code(f), NT_TAG).cast::<NtPlan>();
        let n = nargs(nargsf);
        if plan.is_null() || n == 0 {
            COUNTERS.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DYN.alive() {
            DYN.retire(f, k_nt_new);
            COUNTERS.fallback(Reason::Deps);
            return orig(f, args, nargsf, kw);
        }
        if !accel::tools_quiet(&TOOLS, f) {
            COUNTERS.fallback(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        let g = (*f.cast::<ffi::PyFunctionObject>()).func_globals;
        if ffi::PyDict_CheckExact(g) == 0 || !dict_item_is(g, N_TUPLE_NEW.get(), TUPLE_NEW.get()) {
            COUNTERS.fallback(Reason::Deps);
            return orig(f, args, nargsf, kw);
        }
        let sig = &(*plan).sig;
        let mut vals = [null_mut(); MAX_PARAMS];
        if !bind(sig, f, args, n, kw, &mut vals) {
            COUNTERS.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        let np = sig.names.len();
        let r = construct(*args, &vals[..np]);
        unbind(&vals, np);
        match r {
            Some(o) => {
                COUNTERS.hit();
                o
            }
            None => {
                COUNTERS.fallback(Reason::Args);
                orig(f, args, nargsf, kw)
            }
        }
    }
}

/// Cells of a generated `_make` are the validated ones; returns `num_fields`.
#[inline]
unsafe fn make_cells_ok(mk: P) -> Option<isize> {
    // SAFETY: a live function with `MAKE_CODE`.
    unsafe {
        if !cell_is(mk, MAKE_CELLS[0].load(Relaxed), BUILTIN_LEN.get())
            || !cell_is(mk, MAKE_CELLS[2].load(Relaxed), TUPLE_NEW.get())
        {
            return None;
        }
        let nf = cell(mk, MAKE_CELLS[1].load(Relaxed));
        if nf.is_null() {
            return None;
        }
        let v = if ffi::PyLong_CheckExact(nf) != 0 { ffi::PyLong_AsSsize_t(nf) } else { -1 };
        ffi::Py_DECREF(nf);
        if v < 0 {
            ffi::PyErr_Clear();
            return None;
        }
        Some(v)
    }
}

/// `namedtuple._make(cls, iterable)` (a classmethod's function, called from bytecode).
unsafe extern "C" fn k_nt_make(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol; `f` is a function this target patched.
    unsafe {
        if func_code(f) != MAKE_CODE.get() || nargs(nargsf) != 2 || !kw.is_null() {
            COUNTERS.fallback_streak(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DYN.alive() {
            DYN.retire(f, k_nt_make);
            return orig(f, args, nargsf, kw);
        }
        if !accel::tools_quiet(&TOOLS, f) {
            COUNTERS.fallback_streak(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        let Some(nf) = make_cells_ok(f) else {
            COUNTERS.fallback_streak(Reason::Deps);
            return orig(f, args, nargsf, kw);
        };
        let (cls, it) = (*args, *args.add(1));
        let t = if ffi::PyTuple_CheckExact(it) != 0 {
            incref(it)
        } else if ffi::PyList_CheckExact(it) != 0 {
            let t = ffi::PyList_AsTuple(it);
            if t.is_null() {
                ffi::PyErr_Clear();
                COUNTERS.fallback_streak(Reason::Error);
                return orig(f, args, nargsf, kw);
            }
            t
        } else {
            COUNTERS.fallback_streak(Reason::Args);
            return orig(f, args, nargsf, kw);
        };
        let n = ffi::PyTuple_GET_SIZE(t);
        if n != nf {
            ffi::Py_DECREF(t);
            COUNTERS.fallback_streak(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        let items = std::slice::from_raw_parts((*t.cast::<ffi::PyTupleObject>()).ob_item.as_ptr(), n as usize);
        let r = construct(cls, items);
        ffi::Py_DECREF(t);
        match r {
            Some(o) => {
                COUNTERS.hit_streak();
                o
            }
            None => {
                COUNTERS.fallback_streak(Reason::Args);
                orig(f, args, nargsf, kw)
            }
        }
    }
}

/// `namedtuple._replace(self, /, **kwds)` (also `__replace__`), called from bytecode.
unsafe extern "C" fn k_nt_replace(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol; `f` is a function this target patched.
    unsafe {
        if func_code(f) != REPLACE_CODE.get() || nargs(nargsf) != 1 {
            COUNTERS.fallback_streak(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DYN.alive() {
            DYN.retire(f, k_nt_replace);
            return orig(f, args, nargsf, kw);
        }
        if !accel::tools_quiet(&TOOLS, f) {
            COUNTERS.fallback_streak(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        let this = *args;
        let tp = ffi::Py_TYPE(this);
        let tuple_t = &raw mut ffi::PyTuple_Type;
        // `self` iterates as a tuple, has no instance dict (so `self._make` is the class's).
        if ffi::PyTuple_Check(this) == 0
            || addr!((*tp).tp_iter) != addr!((*tuple_t).tp_iter)
            || !generic_getattr(tp)
            || (*tp).tp_dictoffset != 0
            || ffi::PyType_GetFlags(tp) & ffi::Py_TPFLAGS_MANAGED_DICT != 0
        {
            COUNTERS.fallback_streak(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !cell_is(f, REPLACE_CELLS[0].load(Relaxed), (&raw mut ffi::PyMap_Type).cast()) {
            COUNTERS.fallback_streak(Reason::Deps);
            return orig(f, args, nargsf, kw);
        }
        let fields = cell(f, REPLACE_CELLS[1].load(Relaxed));
        if fields.is_null() {
            COUNTERS.fallback_streak(Reason::Deps);
            return orig(f, args, nargsf, kw);
        }
        let r = replace_native(f, this, tp, fields, args, kw);
        ffi::Py_DECREF(fields);
        match r {
            Some(o) => {
                COUNTERS.hit_streak();
                o
            }
            None => {
                COUNTERS.fallback_streak(Reason::Args);
                orig(f, args, nargsf, kw)
            }
        }
    }
}

const MAX_FIELDS: usize = 64;

#[inline]
unsafe fn replace_native(f: P, this: P, tp: Tp, fields: P, args: *const P, kw: P) -> Option<P> {
    // SAFETY: per `k_nt_replace`.
    unsafe {
        let _ = f;
        if ffi::PyTuple_CheckExact(fields) == 0 {
            return None;
        }
        let nf = ffi::PyTuple_GET_SIZE(fields) as usize;
        if nf > MAX_FIELDS || ffi::PyTuple_GET_SIZE(this) as usize != nf {
            return None;
        }
        // `self._make`: the class attribute (no instance dict) is the generated classmethod.
        let cm = lookup(tp, N_MAKE.get());
        if cm.is_null() {
            return None;
        }
        let mk = if ffi::Py_TYPE(cm) == &raw mut ffi::PyClassMethod_Type {
            (*cm.cast::<RawWrapper>()).callable
        } else {
            null_mut()
        };
        let ok = !mk.is_null() && is_function(mk) && func_code(mk) == MAKE_CODE.get() && accel::tools_quiet(&TOOLS, mk);
        let nfm = if ok { make_cells_ok(mk) } else { None };
        ffi::Py_DECREF(cm);
        if nfm != Some(nf as isize) {
            return None;
        }
        let mut items: [P; MAX_FIELDS] = [null_mut(); MAX_FIELDS];
        for (i, it) in items.iter_mut().enumerate().take(nf) {
            *it = ffi::PyTuple_GET_ITEM(this, i as ffi::Py_ssize_t);
        }
        if !kw.is_null() {
            let nk = ffi::PyTuple_GET_SIZE(kw) as usize;
            'k: for j in 0..nk {
                let k = ffi::PyTuple_GET_ITEM(kw, j as ffi::Py_ssize_t);
                for (i, it) in items.iter_mut().enumerate().take(nf) {
                    if ffi::PyTuple_GET_ITEM(fields, i as ffi::Py_ssize_t) == k {
                        *it = *args.add(1 + j);
                        continue 'k;
                    }
                }
                return None;
            }
        }
        construct(tp.cast(), &items[..nf])
    }
}

/// `classmethod`/`staticmethod` layout: the wrapped callable follows the header.
#[repr(C)]
pub(super) struct RawWrapper {
    pub ob_base: ffi::PyObject,
    pub callable: P,
}

// ---------------------------------------------------------------------------------------
// Hook and adoption
// ---------------------------------------------------------------------------------------

/// `collections.namedtuple`: CPython's function, then the new class's methods get kernels.
unsafe extern "C" fn k_namedtuple(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol.
    unsafe {
        let cls = orig(f, args, nargsf, kw);
        if !cls.is_null() && DEPS.ok() && func_code(f) == NT_CODE.get() {
            adopt(cls, true, false);
        }
        cls
    }
}

/// The function inside a `staticmethod`/`classmethod` in `cls`'s own dict (borrowed), or
/// NULL.
unsafe fn own_wrapped(cls: P, name: P, wrapper: *mut ffi::PyTypeObject) -> P {
    // SAFETY: a live type; dict reads with an interned str key.
    unsafe {
        let d = ffi::PyType_GetDict(cls.cast());
        if d.is_null() {
            ffi::PyErr_Clear();
            return null_mut();
        }
        let o = ffi::PyDict_GetItemWithError(d, name);
        ffi::Py_DECREF(d);
        if o.is_null() {
            ffi::PyErr_Clear();
            return null_mut();
        }
        let f = if wrapper.is_null() {
            o
        } else if ffi::Py_TYPE(o) == wrapper {
            (*o.cast::<RawWrapper>()).callable
        } else {
            return null_mut();
        };
        if f.is_null() || !is_function(f) { null_mut() } else { f }
    }
}

/// Give a namedtuple class's methods their kernels. `fresh`: the class was just returned
/// by the hooked `namedtuple()` (its `__new__` is known by construction); otherwise (a class
/// found at install) only the shared-code `_make`/`_replace`. `force`: install although the
/// hook is not swapped yet (adoption inside `prepare`).
unsafe fn adopt(cls: P, fresh: bool, force: bool) {
    // SAFETY: attached thread, live class; errors are cleared (the class stays CPython's).
    unsafe {
        if ffi::PyType_Check(cls) == 0 || ffi::PyType_IsSubtype(cls.cast(), &raw mut ffi::PyTuple_Type) == 0 {
            return;
        }
        let install = |func: P, k: ffi::vectorcallfunc| {
            if force { DYN.install_forced(func, k) } else { DYN.install(func, k) }
        };
        if fresh {
            let new = own_wrapped(cls, N_NEW.get(), &raw mut ffi::PyStaticMethod_Type);
            if !new.is_null() && new_plan(cls, new) {
                SLOT_TP_NEW
                    .compare_exchange(
                        0,
                        (*cls.cast::<ffi::PyTypeObject>()).tp_new.map_or(0, |f| f as usize),
                        Relaxed,
                        Relaxed,
                    )
                    .ok();
                install(new, k_nt_new);
            }
        }
        let mk = own_wrapped(cls, N_MAKE.get(), &raw mut ffi::PyClassMethod_Type);
        if !mk.is_null() && func_code(mk) == MAKE_CODE.get() {
            install(mk, k_nt_make);
            if SLOT_TP_NEW.load(Relaxed) == 0
                && !own_wrapped(cls, N_NEW.get(), &raw mut ffi::PyStaticMethod_Type).is_null()
            {
                SLOT_TP_NEW
                    .compare_exchange(
                        0,
                        (*cls.cast::<ffi::PyTypeObject>()).tp_new.map_or(0, |f| f as usize),
                        Relaxed,
                        Relaxed,
                    )
                    .ok();
            }
        }
        let rp = own_wrapped(cls, N_REPLACE.get(), null_mut());
        if !rp.is_null() && func_code(rp) == REPLACE_CODE.get() {
            install(rp, k_nt_replace);
        }
    }
}

/// Structural checks of a just-generated `__new__` and its plan (parameter names). The
/// class's `_fields` must be what the parameters are. Raw reads of the code object
/// (layout calibrated by the target's vectorcall site): this runs at every class creation.
unsafe fn new_plan(cls: P, new: P) -> bool {
    // SAFETY: attached thread; dict reads with interned keys, field reads of live objects.
    unsafe {
        let code = func_code(new).cast::<crate::accel::version::RawCode>();
        let d = ffi::PyType_GetDict(cls.cast());
        if d.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        let fields = ffi::PyDict_GetItemWithError(d, N_FIELDS.get());
        let fields = if fields.is_null() || ffi::PyTuple_CheckExact(fields) == 0 { null_mut() } else { incref(fields) };
        ffi::Py_DECREF(d);
        if fields.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        let nf = ffi::PyTuple_GET_SIZE(fields) as usize;
        const CO_VARARGS: i32 = 0x04;
        const CO_VARKEYWORDS: i32 = 0x08;
        const CO_GEN_ANY: i32 = 0x20 | 0x80 | 0x100 | 0x200;
        let names = (*code).co_names;
        let lp = (*code).co_localsplusnames;
        let ok = nf > 0
            && nf <= MAX_PARAMS
            && (*code).co_argcount as usize == nf + 1
            && (*code).co_posonlyargcount == 0
            && (*code).co_kwonlyargcount == 0
            && (*code).co_flags & (CO_VARARGS | CO_VARKEYWORDS | CO_GEN_ANY) == 0
            && (*code).co_ncellvars == 0
            && (*code).co_nfreevars == 0
            && (*code).co_nlocalsplus as usize == nf + 1
            && ffi::PyTuple_CheckExact(names) != 0
            && ffi::PyTuple_GET_SIZE(names) == 1
            && same_str(ffi::PyTuple_GET_ITEM(names, 0), N_TUPLE_NEW.get())
            && ffi::PyTuple_CheckExact(lp) != 0
            && ffi::PyTuple_GET_SIZE(lp) as usize == nf + 1
            && same_str(ffi::PyTuple_GET_ITEM(lp, 0), N_CLS.get())
            && (0..nf).all(|i| {
                same_str(
                    ffi::PyTuple_GET_ITEM(lp, (i + 1) as ffi::Py_ssize_t),
                    ffi::PyTuple_GET_ITEM(fields, i as ffi::Py_ssize_t),
                )
            });
        // The globals the lambda reads.
        let g = (*new.cast::<ffi::PyFunctionObject>()).func_globals;
        let mut tn: P = null_mut();
        let ok = ok && ffi::PyDict_CheckExact(g) != 0 && ffi::PyDict_GetItemRef(g, N_TUPLE_NEW.get(), &mut tn) > 0;
        if !tn.is_null() {
            ffi::Py_DECREF(tn);
        }
        if !ok || tn != TUPLE_NEW.get() {
            ffi::Py_DECREF(fields);
            if !ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_Clear();
            }
            return false;
        }
        // Parameter names: the code's own (keywords from call sites are compared by
        // identity; anything else falls back).
        let sig_names = (0..nf).map(|i| incref(ffi::PyTuple_GET_ITEM(lp, (i + 1) as ffi::Py_ssize_t))).collect();
        ffi::Py_DECREF(fields);
        let plan = Box::into_raw(Box::new(NtPlan {
            h: PlanHeader { tag: NT_TAG, free: free_nt_plan },
            sig: Sig { names: sig_names, npos: nf },
        }));
        set_plan(func_code(new), plan.cast())
    }
}

/// Exact `str` objects with equal contents (identity first: names are interned).
#[inline]
unsafe fn same_str(a: P, b: P) -> bool {
    // SAFETY: live objects; exact str comparison runs no Python code.
    unsafe {
        a == b
            || (ffi::PyUnicode_CheckExact(a) != 0
                && ffi::PyUnicode_CheckExact(b) != 0
                && ffi::PyUnicode_Compare(a, b) == 0)
    }
}

// ---------------------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------------------

fn code_const<'py>(code: &Bound<'py, PyAny>, name: &str) -> Result<Bound<'py, PyAny>, Refusal> {
    let consts = code.getattr("co_consts")?.cast_into_exact::<PyTuple>().map_err(PyErr::from)?;
    let mut found = None;
    for c in consts.iter() {
        if c.is_exact_instance_of::<PyCode>() && c.getattr("co_name")?.eq(name)? {
            if found.is_some() {
                return Err(Refusal::new(format!("namedtuple: two code constants named {name}")));
            }
            found = Some(c);
        }
    }
    found.ok_or_else(|| Refusal::new(format!("namedtuple: no code constant {name}")))
}

fn cell_indices(code: &Bound<'_, PyAny>, names: &[&str], out: &[AtomicUsize]) -> Result<(), Refusal> {
    let fv = code.getattr("co_freevars")?.cast_into_exact::<PyTuple>().map_err(PyErr::from)?;
    if fv.len() != names.len() {
        return Err(Refusal::new("namedtuple: unexpected closure of a generated method"));
    }
    for (i, n) in names.iter().enumerate() {
        let mut ix = None;
        for (j, v) in fv.iter().enumerate() {
            if v.eq(*n)? {
                ix = Some(j);
            }
        }
        out[i].store(ix.ok_or_else(|| Refusal::new(format!("namedtuple: no free variable {n}")))?, Relaxed);
    }
    Ok(())
}

fn prepare(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let nt = cx.module_attr("namedtuple")?;
    cx.fingerprint_code(&nt, fp::NAMEDTUPLE)?;
    // SAFETY: an exact function (checked by fingerprint_code's function path below).
    if !nt.is_exact_instance_of::<pyo3::types::PyFunction>() {
        return Err(Refusal::new("collections.namedtuple is not a function"));
    }
    // SAFETY: an exact function object.
    let code = unsafe { Bound::from_borrowed_ptr(py, func_code(nt.as_ptr())) };
    let make = code_const(&code, "_make")?;
    let replace = code_const(&code, "_replace")?;
    cell_indices(&make, &["_len", "num_fields", "tuple_new"], &MAKE_CELLS)?;
    cell_indices(&replace, &["_map", "field_names"], &REPLACE_CELLS)?;
    // Everything the generated text and the class creation depend on.
    for name in ["eval", "map", "tuple", "type", "len"] {
        cx.pin_global(&g, name, &DEPS)?;
    }
    let sys_mod = cx.get(&g, "_sys")?;
    let sys = cx.loaded_module("sys")?;
    if !sys_mod.is(&sys) {
        return Err(Refusal::with(accel::Category::Deps, "collections._sys is not sys"));
    }
    cx.pin(&g, "_sys", &DEPS)?;
    cx.pin(&sys.dict(), "intern", &DEPS)?;
    let builtins = cx.builtins_of(&g)?;
    let len = cx.get(&builtins, "len")?;
    let tuple_t = py.get_type::<PyTuple>();
    let tn = tuple_t.getattr("__new__")?;
    NT_CODE.keep(&code);
    MAKE_CODE.keep(&make);
    REPLACE_CODE.keep(&replace);
    TUPLE_NEW.keep(&tn);
    BUILTIN_LEN.keep(&len);
    N_TUPLE_NEW.name(py, "_tuple_new");
    N_NEW.name(py, "__new__");
    N_MAKE.name(py, "_make");
    N_REPLACE.name(py, "_replace");
    N_FIELDS.name(py, "_fields");
    N_CLS.name(py, "_cls");
    extra_index().ok_or_else(|| Refusal::new("no code-extra index available"))?;
    cx.vectorcall(&nt, k_namedtuple, &accel::FUNCTION_VECTORCALL)?;
    // SAFETY: a live function.
    DYN.set_hook(nt.as_ptr(), k_namedtuple);
    if !accel::version::FREE_THREADED {
        adopt_existing(py, &tuple_t);
    }
    Ok(())
}

/// Classes made by `namedtuple()` before install: their `_make`/`_replace` (shared code).
fn adopt_existing(py: Python<'_>, tuple_t: &Bound<'_, PyType>) {
    let Ok(subs) = tuple_t.call_method0("__subclasses__") else {
        // SAFETY: attached thread.
        unsafe { ffi::PyErr_Clear() };
        return;
    };
    let Ok(subs) = subs.cast_into_exact::<pyo3::types::PyList>() else { return };
    for c in subs.iter() {
        // SAFETY: live class objects.
        unsafe { adopt(c.as_ptr(), false, true) };
    }
    let _ = py;
}

/// Number of live functions with a namedtuple kernel (tests).
pub fn dynamic_sites() -> usize {
    DYN.live()
}
