//! Group `functools`: the bookkeeping of single dispatch that ruling 15 leaves reachable —
//! the cache lookup of a `singledispatch` function's `dispatch(cls)` closure, and
//! `singledispatchmethod.__get__` (building the `_singledispatchmethod_get` object). The
//! `wrapper` and `_singledispatchmethod_get.__call__` call the user's implementation and
//! stay CPython's. See docs/targets/datamodel.md.
//!
//! OWNER: datamodel workstream.
//!
//! - `functools.singledispatch` (dynamic sites): a hook on `functools.singledispatch`
//!   gives each new `dispatch` closure (shared, fingerprinted code) a kernel that answers a
//!   cache hit: `cache_token` is `None` or equal to `abc.get_cache_token()` (an int, the C
//!   function pinned), `dispatch_cache` an exact `WeakKeyDictionary` (class unchanged, its
//!   `__getitem__` the fingerprinted function, `weakref.ref` unchanged), `cls` a class whose
//!   metaclass hashes and compares like `type`, and every key of the cache's `data` dict a
//!   weak reference to a dead object or one whose cached hash is its referent's pointer
//!   hash (checked when the dict changes): then the lookup compares only with `cls`'s own
//!   entry, in C. A miss (`_find_impl`, cache refill,
//!   token change) runs CPython's closure.
//! - `functools.singledispatchmethod.__get__` (vectorcall, reached from
//!   `slot_tp_descr_get`): for an exact `singledispatchmethod` whose `dispatcher` is a
//!   function and whose `func` is a plain function, allocate the
//!   `_singledispatchmethod_get` (class unchanged: `object.__new__`, the fingerprinted
//!   `__init__`, generic `__setattr__`) and set its attributes in CPython's order (3.15 adds
//!   `_dispatch_arg_index`).

use std::ptr::null_mut;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering::*};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyCode, PyTuple, PyType};

use crate::accel::{self, Counters, DepFlag, Prepare, Reason, Refusal, TargetSpec, Tools};
use crate::targets::generated::adapter::{ClassCache, Dynamic, watch_dict_invalidates};
use crate::targets::generated::common::*;

python_extern! {
    fn Py_HashPointer(ptr: *const std::ffi::c_void) -> ffi::Py_hash_t;
}

static C_DISPATCH: Counters = Counters::new();
static C_GET: Counters = Counters::new();
static DEPS: DepFlag = DepFlag::new();
static TOOLS: Tools = Tools::new();
static DYN: Dynamic = Dynamic::new(&DISPATCH);

/// Not on free-threaded builds this wave (lead's ruling): giving each generated function its
/// kernel calls `PyFunction_SetVectorcall`, which stops the world on 3.14t/3.15t; with busy
/// threads each stop waits for every thread to reach a safe point (measured:
/// `benches/datamodel/ft_create.py`). Static sites are installed at enable/import (§3.10).
pub static DISPATCH: TargetSpec = TargetSpec {
    name: "functools.singledispatch",
    group: "functools",
    module: Some("functools"),
    versions: &[(3, 14), (3, 15)],
    free_threaded: false,
    counters: &C_DISPATCH,
    prepare: prepare_dispatch,
    release: Some(release),
    ..TargetSpec::DEFAULT
};

pub static SDM_GET: TargetSpec = TargetSpec {
    name: "functools.singledispatchmethod.__get__",
    group: "functools",
    module: Some("functools"),
    versions: &[(3, 14), (3, 15)],
    free_threaded: true,
    counters: &C_GET,
    prepare: prepare_get,
    release: Some(release),
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&DISPATCH, &SDM_GET];

mod fp {
    use crate::accel::Fingerprints;
    pub const SINGLEDISPATCH: Fingerprints = &[(3, 14, "aa95491817929311"), (3, 15, "383312086a347235")];
    pub const SDM_GET: Fingerprints = &[(3, 14, "f01c1ce9900a5e32"), (3, 15, "d464a8451d6b2cc8")];
    pub const SDM_GET_INIT: Fingerprints = &[(3, 14, "3fa7ec349332ca55"), (3, 15, "04ae27436e9f582b")];
    pub const WKD_GETITEM: Fingerprints = &[(3, 14, "1c1e6e1045bc576a"), (3, 15, "00466e7cccfc2747")];
}

static SD_CODE: Slot = Slot::new();
static DISPATCH_CODE: Slot = Slot::new();
static N_DISPATCH: Slot = Slot::new();
static N_DATA: Slot = Slot::new();
static N_REF: Slot = Slot::new();
static N_GETITEM: Slot = Slot::new();
static N_GET_CACHE_TOKEN: Slot = Slot::new();
/// Closure indices of `dispatch`: `cache_token`, `dispatch_cache`.
static CELL_TOKEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);
static CELL_CACHE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);
/// `functools.get_cache_token` (C: `_abc.get_cache_token`).
static GET_CACHE_TOKEN: Slot = Slot::new();

/// Lazily validated `weakref` state (the module is imported by `singledispatch()` itself,
/// after `functools`): `WeakKeyDictionary`, its `__getitem__` (function, code), the
/// module dict and `ref`. 0 unknown, 1 good, 2 bad (degraded).
static WR_STATE: AtomicU8 = AtomicU8::new(0);
static WKD: Slot = Slot::new();
static WKD_GETITEM: Slot = Slot::new();
static WKD_GETITEM_CODE: Slot = Slot::new();
static WR_DICT: Slot = Slot::new();
static WR_REF: Slot = Slot::new();
static WKD_TAG: AtomicU32 = AtomicU32::new(0);
/// Verdicts per `data` dict: keys compare in C only (watched: any change clears).
static DATA_OK: ClassCache<64> = ClassCache::new();

// singledispatchmethod.__get__
static SDM: Slot = Slot::new();
static SDM_GET_FN: Slot = Slot::new();
static SDM_GET_CODE: Slot = Slot::new();
static GET_T: Slot = Slot::new();
static GET_INIT: Slot = Slot::new();
static GET_INIT_CODE: Slot = Slot::new();
static N_DISPATCHER: Slot = Slot::new();
static N_FUNC: Slot = Slot::new();
static N_UNBOUND: Slot = Slot::new();
static N_UDISPATCH: Slot = Slot::new();
static N_OBJ: Slot = Slot::new();
static N_CLS: Slot = Slot::new();
static N_DAI: Slot = Slot::new();
static N_MODULE: Slot = Slot::new();
static N_DOC: Slot = Slot::new();
static N_INIT: Slot = Slot::new();
static SDM_TAG: AtomicU32 = AtomicU32::new(0);
static GET_TAG: AtomicU32 = AtomicU32::new(0);
static ZERO: Slot = Slot::new();
static ONE: Slot = Slot::new();

fn release(_py: Python<'_>) {
    DYN.release();
    for s in [
        &SD_CODE,
        &DISPATCH_CODE,
        &GET_CACHE_TOKEN,
        &WKD,
        &WKD_GETITEM,
        &WKD_GETITEM_CODE,
        &WR_DICT,
        &WR_REF,
        &SDM,
        &SDM_GET_FN,
        &SDM_GET_CODE,
        &GET_T,
        &GET_INIT,
        &GET_INIT_CODE,
    ] {
        s.release();
    }
    WR_STATE.store(0, Relaxed);
    WKD_TAG.store(0, Relaxed);
    SDM_TAG.store(0, Relaxed);
    GET_TAG.store(0, Relaxed);
    DATA_OK.clear_all();
}

// ---------------------------------------------------------------------------------------
// dispatch(cls)
// ---------------------------------------------------------------------------------------

/// Validate the `weakref` objects the original reads (first use; `weakref` is imported by
/// then). On failure the target is degraded: its kernels fall back.
#[cold]
unsafe fn wr_validate() -> bool {
    // SAFETY: attached thread; dict reads, C getters, fingerprint of a code object.
    unsafe {
        let py = Python::assume_attached();
        let ok = (|| -> Option<()> {
            let modules = Bound::from_borrowed_ptr(py, ffi::PyImport_GetModuleDict());
            let m = modules.get_item("weakref").ok()?;
            let m = m.cast_into::<pyo3::types::PyModule>().ok()?;
            let d = m.dict();
            let wkd = d.get_item("WeakKeyDictionary").ok()??.cast_into::<PyType>().ok()?;
            let r = d.get_item("ref").ok()??;
            if r.as_ptr() != (&raw mut ffi::_PyWeakref_RefType).cast() {
                return None;
            }
            let gi = crate::accel::prepare::type_dict(&wkd).ok()?.get_item("__getitem__").ok()??;
            if !gi.is_exact_instance_of::<pyo3::types::PyFunction>() {
                return None;
            }
            let observed = crate::accel::fingerprint::function(&gi).ok()?;
            let (major, minor) = crate::accel::version::running();
            if crate::accel::fingerprint::expected_for(fp::WKD_GETITEM, major, minor) != Some(observed.as_str()) {
                return None;
            }
            WKD.keep(wkd.as_any());
            WKD_GETITEM_CODE.keep(&Bound::from_borrowed_ptr(py, func_code(gi.as_ptr())));
            WKD_GETITEM.keep(&gi);
            WR_DICT.keep(d.as_any());
            WR_REF.keep(&r);
            Some(())
        })()
        .is_some();
        if !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
        }
        WR_STATE.store(if ok { 1 } else { 2 }, Relaxed);
        if !ok {
            crate::accel::degrade(&DISPATCH, "weakref.WeakKeyDictionary differs from the validated one");
        }
        ok
    }
}

/// `WeakKeyDictionary` unchanged (tag) and `weakref.ref` still `_weakref.ref`.
#[inline]
unsafe fn wkd_ok(t: Tp) -> bool {
    // SAFETY: attached thread; live objects kept by the slots.
    unsafe {
        let tag = tag_of(t);
        if tag == 0 || tag != WKD_TAG.load(Relaxed) {
            let tag = ensure_tag(t);
            let ok = generic_getattr(t)
                && resolves_to(t, N_GETITEM.get(), WKD_GETITEM.get())
                && plain_attr(t, N_DATA.get())
                && tag != 0
                && tag_of(t) == tag;
            if !ok {
                return false;
            }
            WKD_TAG.store(tag, Relaxed);
        }
        let mut r: P = null_mut();
        if ffi::PyDict_GetItemRef(WR_DICT.get(), N_REF.get(), &mut r) <= 0 {
            ffi::PyErr_Clear();
            return false;
        }
        ffi::Py_DECREF(r);
        r == WR_REF.get() && func_code(WKD_GETITEM.get()) == WKD_GETITEM_CODE.get()
    }
}

/// `type`'s hash and comparison (the lookup of a class hashes it and compares it with
/// itself).
#[inline(always)]
unsafe fn type_like(t: Tp) -> bool {
    let tt = &raw mut ffi::PyType_Type;
    // SAFETY: live types.
    unsafe { addr!((*t).tp_hash) == addr!((*tt).tp_hash) && addr!((*t).tp_richcompare) == addr!((*tt).tp_richcompare) }
}

/// No key of `data` can compare equal-by-hash with a live class other than itself: every
/// key is an exact weak reference whose referent is dead (compared by identity) or whose
/// cached hash is the referent's pointer hash (distinct live objects never share one). A
/// lookup then compares only with the looked-up class's own entry, in C (its metaclass is
/// checked per call). Cached per dict (watched: any change clears the verdict).
#[inline]
unsafe fn data_safe(data: P) -> bool {
    // SAFETY: attached thread; the dict is iterated under its critical section.
    unsafe {
        // The dict is not a type: index the cache by its address with tag 1 (the watcher
        // clears the slot on any change; DEALLOCATED forgets it).
        if DATA_OK.get_addr(data as usize) {
            return true;
        }
        let epoch = DATA_OK.epoch();
        if !watch_dict_invalidates(data, &DATA_OK, data.cast()) {
            return false;
        }
        let ok = crate::targets::r#enum::with_cs(data, || {
            let mut pos: ffi::Py_ssize_t = 0;
            let mut k: P = null_mut();
            let mut v: P = null_mut();
            while ffi::PyDict_Next(data, &mut pos, &mut k, &mut v) != 0 {
                if ffi::Py_TYPE(k) != &raw mut ffi::_PyWeakref_RefType {
                    return false;
                }
                let mut o: P = null_mut();
                let rc = PyWeakref_GetRef(k, &mut o);
                if rc < 0 {
                    ffi::PyErr_Clear();
                    return false;
                }
                if rc == 1 {
                    // The weak reference's cached hash (a dict key always has one).
                    let h = ffi::PyObject_Hash(k);
                    let ok = h != -1 && h == Py_HashPointer(o.cast());
                    ffi::Py_DECREF(o);
                    if !ok {
                        ffi::PyErr_Clear();
                        return false;
                    }
                }
            }
            true
        });
        if ok {
            DATA_OK.put_addr(data as usize, epoch);
        }
        ok
    }
}

/// The `dispatch(cls)` closure of a `singledispatch` function.
unsafe extern "C" fn k_dispatch(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol; `f` is a function this target patched.
    unsafe {
        if func_code(f) != DISPATCH_CODE.get() || nargs(nargsf) != 1 || !kw.is_null() {
            C_DISPATCH.fallback_streak(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DYN.alive() {
            DYN.retire(f, k_dispatch);
            return orig(f, args, nargsf, kw);
        }
        let ready = match WR_STATE.load(Relaxed) {
            1 => true,
            0 => wr_validate(),
            _ => false,
        };
        if !ready || !DEPS.ok() || !accel::tools_quiet(&TOOLS, f) || !accel::code_locally_quiet(WKD_GETITEM_CODE.get())
        {
            C_DISPATCH.fallback_streak(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        match lookup_impl(f, *args) {
            Some(r) => {
                C_DISPATCH.hit_streak();
                r
            }
            None => {
                C_DISPATCH.fallback_streak(Reason::Args);
                orig(f, args, nargsf, kw)
            }
        }
    }
}

#[inline]
unsafe fn lookup_impl(f: P, cls: P) -> Option<P> {
    // SAFETY: per `k_dispatch`.
    unsafe {
        if ffi::PyType_Check(cls) == 0 || !type_like(ffi::Py_TYPE(cls)) {
            return None;
        }
        let tok = cell(f, CELL_TOKEN.load(Relaxed));
        if tok.is_null() {
            return None;
        }
        let same = if tok == ffi::Py_None() {
            true
        } else {
            let cur = if ffi::PyLong_CheckExact(tok) != 0 {
                ffi::PyObject_CallNoArgs(GET_CACHE_TOKEN.get())
            } else {
                null_mut()
            };
            let same = !cur.is_null()
                && ffi::PyLong_CheckExact(cur) != 0
                && ffi::PyObject_RichCompareBool(tok, cur, ffi::Py_EQ) == 1;
            if !cur.is_null() {
                ffi::Py_DECREF(cur);
            }
            if !ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_Clear();
            }
            same
        };
        ffi::Py_DECREF(tok);
        if !same {
            return None;
        }
        let dc = cell(f, CELL_CACHE.load(Relaxed));
        if dc.is_null() {
            return None;
        }
        let t = ffi::Py_TYPE(dc);
        let r = if t.cast::<ffi::PyObject>() == WKD.get() && wkd_ok(t) {
            let data = get_plain(dc, N_DATA.get());
            let r = if !data.is_null() && ffi::PyDict_CheckExact(data) != 0 && data_safe(data) {
                let wr = ffi::PyWeakref_NewRef(cls, null_mut());
                if wr.is_null() {
                    ffi::PyErr_Clear();
                    None
                } else {
                    let mut out: P = null_mut();
                    let rc = ffi::PyDict_GetItemRef(data, wr, &mut out);
                    ffi::Py_DECREF(wr);
                    if rc < 0 {
                        ffi::PyErr_Clear();
                    }
                    (rc == 1).then_some(out)
                }
            } else {
                None
            };
            if !data.is_null() {
                ffi::Py_DECREF(data);
            }
            r
        } else {
            None
        };
        ffi::Py_DECREF(dc);
        r
    }
}

/// `functools.singledispatch(func)`: CPython's, then the new `dispatch` gets the kernel.
unsafe extern "C" fn k_singledispatch(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol.
    unsafe {
        let w = orig(f, args, nargsf, kw);
        if !w.is_null() && DEPS.ok() && func_code(f) == SD_CODE.get() && is_function(w) {
            // `wrapper.dispatch = dispatch`: the wrapper function's own __dict__.
            let d = (*w.cast::<ffi::PyFunctionObject>()).func_dict;
            if !d.is_null() && ffi::PyDict_CheckExact(d) != 0 {
                let mut dp: P = null_mut();
                if ffi::PyDict_GetItemRef(d, N_DISPATCH.get(), &mut dp) > 0 {
                    if is_function(dp) && func_code(dp) == DISPATCH_CODE.get() {
                        DYN.install(dp, k_dispatch);
                    }
                    ffi::Py_DECREF(dp);
                } else {
                    ffi::PyErr_Clear();
                }
            }
        }
        w
    }
}

// ---------------------------------------------------------------------------------------
// singledispatchmethod.__get__
// ---------------------------------------------------------------------------------------

/// `singledispatchmethod` and `_singledispatchmethod_get` as validated.
#[inline]
unsafe fn get_classes_ok(sdm: Tp, gt: Tp) -> bool {
    // SAFETY: attached thread; live types kept by the slots.
    unsafe {
        let (a, b) = (tag_of(sdm), tag_of(gt));
        if a != 0 && b != 0 && a == SDM_TAG.load(Relaxed) && b == GET_TAG.load(Relaxed) {
            return true;
        }
        validate_get_classes(sdm, gt)
    }
}

#[cold]
unsafe fn validate_get_classes(sdm: Tp, gt: Tp) -> bool {
    // SAFETY: attached thread; MRO lookups and slot reads.
    unsafe {
        let (a, b) = (ensure_tag(sdm), ensure_tag(gt));
        let ob = &raw mut ffi::PyBaseObject_Type;
        let tt = &raw mut ffi::PyType_Type;
        let meta = ffi::Py_TYPE(gt.cast());
        let mut ok = generic_getattr(sdm)
            && plain_attr(sdm, N_DISPATCHER.get())
            && plain_attr(sdm, N_FUNC.get())
            && addr!((*meta).tp_call) == addr!((*tt).tp_call)
            && addr!((*gt).tp_new) == addr!((*ob).tp_new)
            && addr!((*gt).tp_setattro) == ffi::PyObject_GenericSetAttr as *const () as usize
            && addr!((*gt).tp_alloc) == ffi::PyType_GenericAlloc as *const () as usize
            && ffi::PyType_GetFlags(gt) & (ffi::Py_TPFLAGS_MANAGED_DICT | ffi::Py_TPFLAGS_IS_ABSTRACT)
                == ffi::Py_TPFLAGS_MANAGED_DICT
            && resolves_to(gt, N_INIT.get(), GET_INIT.get());
        for n in [&N_UNBOUND, &N_UDISPATCH, &N_OBJ, &N_CLS, &N_DAI, &N_MODULE, &N_DOC] {
            ok = ok && plain_attr(gt, n.get());
        }
        if ok && a != 0 && b != 0 && tag_of(sdm) == a && tag_of(gt) == b {
            SDM_TAG.store(a, Relaxed);
            GET_TAG.store(b, Relaxed);
        }
        ok
    }
}

/// `singledispatchmethod.__get__(self, obj, cls=None)`.
unsafe extern "C" fn k_sdm_get(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol.
    unsafe {
        let n = nargs(nargsf);
        if !(n == 2 || n == 3) || !kw.is_null() || f != SDM_GET_FN.get() || func_code(f) != SDM_GET_CODE.get() {
            C_GET.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DEPS.ok()
            || !accel::tools_quiet(&TOOLS, f)
            || func_code(GET_INIT.get()) != GET_INIT_CODE.get()
            || !accel::code_locally_quiet(GET_INIT_CODE.get())
        {
            C_GET.fallback(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        let this = *args;
        let gt = GET_T.get().cast::<ffi::PyTypeObject>();
        if ffi::Py_TYPE(this).cast::<ffi::PyObject>() != SDM.get() || !get_classes_ok(ffi::Py_TYPE(this), gt) {
            C_GET.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        let obj = *args.add(1);
        let cls = if n == 3 { *args.add(2) } else { ffi::Py_None() };
        match build_get(this, obj, cls, gt) {
            Some(r) => {
                C_GET.hit();
                r
            }
            None => {
                C_GET.fallback(Reason::Args);
                orig(f, args, nargsf, kw)
            }
        }
    }
}

#[inline]
unsafe fn build_get(this: P, obj: P, cls: P, gt: Tp) -> Option<P> {
    // SAFETY: per `k_sdm_get`.
    unsafe {
        let disp = get_plain(this, N_DISPATCHER.get());
        let func = get_plain(this, N_FUNC.get());
        let mut dd: P = null_mut();
        let pre = !disp.is_null() && is_function(disp) && !func.is_null() && is_function(func) && {
            // `dispatcher.dispatch`: the function's own __dict__ (functions define no
            // `dispatch` descriptor).
            let d = (*disp.cast::<ffi::PyFunctionObject>()).func_dict;
            !d.is_null() && ffi::PyDict_CheckExact(d) != 0 && ffi::PyDict_GetItemRef(d, N_DISPATCH.get(), &mut dd) > 0
        };
        if !pre {
            for o in [disp, func, dd] {
                if !o.is_null() {
                    ffi::Py_DECREF(o);
                }
            }
            if !ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_Clear();
            }
            return None;
        }
        // From here on everything is what CPython does, in its order.
        let o = ffi::PyType_GenericAlloc(gt, 0);
        let module = ffi::PyObject_GetAttr(func, N_MODULE.get());
        let doc = ffi::PyObject_GetAttr(func, N_DOC.get());
        let dai = if obj == ffi::Py_None() { ONE.get() } else { ZERO.get() };
        let with_dai = crate::accel::version::running().1 >= 15;
        let ok = !o.is_null()
            && !module.is_null()
            && !doc.is_null()
            && ffi::PyObject_GenericSetAttr(o, N_UNBOUND.get(), this) == 0
            && ffi::PyObject_GenericSetAttr(o, N_UDISPATCH.get(), dd) == 0
            && ffi::PyObject_GenericSetAttr(o, N_OBJ.get(), obj) == 0
            && ffi::PyObject_GenericSetAttr(o, N_CLS.get(), cls) == 0
            && (!with_dai || ffi::PyObject_GenericSetAttr(o, N_DAI.get(), dai) == 0)
            && ffi::PyObject_GenericSetAttr(o, N_MODULE.get(), module) == 0
            && ffi::PyObject_GenericSetAttr(o, N_DOC.get(), doc) == 0;
        for x in [disp, func, dd, module, doc] {
            if !x.is_null() {
                ffi::Py_DECREF(x);
            }
        }
        if !ok {
            if !o.is_null() {
                ffi::Py_DECREF(o);
            }
            // Allocation failures only (CPython raises them too).
            return Some(null_mut());
        }
        Some(o)
    }
}

// ---------------------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------------------

fn names(py: Python<'_>) {
    for (s, n) in [
        (&N_DISPATCH, "dispatch"),
        (&N_DATA, "data"),
        (&N_REF, "ref"),
        (&N_GETITEM, "__getitem__"),
        (&N_GET_CACHE_TOKEN, "get_cache_token"),
        (&N_DISPATCHER, "dispatcher"),
        (&N_FUNC, "func"),
        (&N_UNBOUND, "_unbound"),
        (&N_UDISPATCH, "_dispatch"),
        (&N_OBJ, "_obj"),
        (&N_CLS, "_cls"),
        (&N_DAI, "_dispatch_arg_index"),
        (&N_MODULE, "__module__"),
        (&N_DOC, "__doc__"),
        (&N_INIT, "__init__"),
    ] {
        s.name(py, n);
    }
    if ZERO.get().is_null() {
        ZERO.keep(&0i64.into_pyobject(py).expect("int").into_any());
        ONE.keep(&1i64.into_pyobject(py).expect("int").into_any());
    }
}

fn prepare_dispatch(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let sd = cx.module_attr("singledispatch")?;
    if !sd.is_exact_instance_of::<pyo3::types::PyFunction>() {
        return Err(Refusal::new("functools.singledispatch is not a function"));
    }
    cx.fingerprint_code(&sd, fp::SINGLEDISPATCH)?;
    // SAFETY: an exact function.
    let code = unsafe { Bound::from_borrowed_ptr(py, func_code(sd.as_ptr())) };
    let consts = code.getattr("co_consts")?.cast_into_exact::<PyTuple>().map_err(PyErr::from)?;
    let mut dispatch = None;
    for c in consts.iter() {
        if c.is_exact_instance_of::<PyCode>() && c.getattr("co_name")?.eq("dispatch")? {
            dispatch = Some(c);
        }
    }
    let dispatch = dispatch.ok_or_else(|| Refusal::new("singledispatch: no dispatch code"))?;
    let fv = dispatch.getattr("co_freevars")?.cast_into_exact::<PyTuple>().map_err(PyErr::from)?;
    let ix = |n: &str| -> Result<usize, Refusal> {
        for (i, v) in fv.iter().enumerate() {
            if v.eq(n)? {
                return Ok(i);
            }
        }
        Err(Refusal::new(format!("singledispatch: no free variable {n}")))
    };
    CELL_TOKEN.store(ix("cache_token")?, Relaxed);
    CELL_CACHE.store(ix("dispatch_cache")?, Relaxed);
    let gct = cx.pin_global(&g, "get_cache_token", &DEPS)?;
    if !gct.is_exact_instance_of::<pyo3::types::PyCFunction>() {
        return Err(Refusal::new("functools.get_cache_token is not the C function"));
    }
    // `weakref` is imported by singledispatch() itself; when it is loaded already, check
    // the WeakKeyDictionary lookup now (else at the first dispatch).
    if let Ok(wr) = cx.loaded_module("weakref") {
        let wkd = cx.get(&wr.dict(), "WeakKeyDictionary")?.cast_into::<PyType>().map_err(PyErr::from)?;
        let gi = cx.type_attr(&wkd, "__getitem__")?;
        cx.fingerprint_code(&gi, fp::WKD_GETITEM)?;
    }
    names(py);
    SD_CODE.keep(&code);
    DISPATCH_CODE.keep(&dispatch);
    GET_CACHE_TOKEN.keep(&gct);
    WR_STATE.store(0, Relaxed);
    WKD_TAG.store(0, Relaxed);
    DATA_OK.clear_all();
    cx.vectorcall(&sd, k_singledispatch, &accel::FUNCTION_VECTORCALL)?;
    Ok(())
}

fn prepare_get(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let sdm = cx.module_attr("singledispatchmethod")?.cast_into::<PyType>().map_err(PyErr::from)?;
    let get = cx.type_attr(&sdm, "__get__")?;
    if !get.is_exact_instance_of::<pyo3::types::PyFunction>() {
        return Err(Refusal::new("singledispatchmethod.__get__ is not a function"));
    }
    cx.fingerprint_code(&get, fp::SDM_GET)?;
    let gt = cx.get(&g, "_singledispatchmethod_get")?.cast_into::<PyType>().map_err(PyErr::from)?;
    cx.pin(&g, "_singledispatchmethod_get", &DEPS)?;
    if cx.version().1 >= 15 {
        // `_dispatch_arg_index = 1 if obj is None and isinstance(func, FunctionType) else 0`
        cx.pin_global(&g, "FunctionType", &DEPS)?;
        cx.pin_global(&g, "isinstance", &DEPS)?;
    }
    let init = cx.type_attr(&gt, "__init__")?;
    if !init.is_exact_instance_of::<pyo3::types::PyFunction>() {
        return Err(Refusal::new("_singledispatchmethod_get.__init__ is not a function"));
    }
    cx.fingerprint_code(&init, fp::SDM_GET_INIT)?;
    names(py);
    SDM.keep(sdm.as_any());
    SDM_GET_FN.keep(&get);
    // SAFETY: exact functions.
    unsafe {
        SDM_GET_CODE.keep(&Bound::from_borrowed_ptr(py, func_code(get.as_ptr())));
        GET_INIT_CODE.keep(&Bound::from_borrowed_ptr(py, func_code(init.as_ptr())));
    }
    GET_T.keep(gt.as_any());
    GET_INIT.keep(&init);
    SDM_TAG.store(0, Relaxed);
    GET_TAG.store(0, Relaxed);
    cx.vectorcall(&get, k_sdm_get, &accel::FUNCTION_VECTORCALL)
}

/// Number of `dispatch` closures given the kernel (tests).
pub fn dynamic_sites() -> usize {
    DYN.live()
}
