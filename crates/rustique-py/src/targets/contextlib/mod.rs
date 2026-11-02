//! Group `contextlib`: constructing a `@contextmanager` context manager, `cm(*args)` — the
//! `helper` closure `contextmanager()` returns and `_GeneratorContextManagerBase.__init__`
//! — as a vectorcall kernel on each `helper` (dynamic sites; a hook on
//! `contextlib.contextmanager` installs it). `__enter__`/`__exit__` stay CPython's: they run
//! the user's generator (ruling 15). See docs/targets/datamodel.md.
//!
//! OWNER: datamodel workstream.
//!
//! Exact subset (everything else runs CPython's `helper`): the helper still runs the
//! fingerprinted code, its `func` cell holds an exact pure-Python *generator* function with
//! CPython's vectorcall (calling it only binds the arguments and creates the generator: no
//! frame of user code runs), no tool listens (globally, or locally on `helper`, `__init__`
//! or `func`), the global `_GeneratorContextManager` is the validated class, unchanged
//! (`tp_version_tag`, its metaclass's too): `object.__new__`, the validated `__init__`,
//! generic `__setattr__`, no descriptor for the five attributes. Binding errors of
//! `func(*args, **kwds)` make the kernel fall back (CPython raises them from its frames).
//!
//! The kernel does what the two frames do, in CPython's order: `args` tuple and `kwds`
//! dict, allocation (`tp_alloc`, as `object.__new__`), `self.gen = func(*args, **kwds)`,
//! `self.func`, `self.args`, `self.kwds`, then `self.__doc__ = func.__doc__` (or the
//! class's when `None`).

use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering::*};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyCode, PyTuple, PyType};

use crate::accel::{self, Counters, DepFlag, Prepare, Reason, Refusal, TargetSpec, Tools};
use crate::targets::generated::adapter::Dynamic;
use crate::targets::generated::common::*;

static COUNTERS: Counters = Counters::new();
static DEPS: DepFlag = DepFlag::new();
static TOOLS: Tools = Tools::new();
static DYN: Dynamic = Dynamic::new();

pub static TARGET: TargetSpec = TargetSpec {
    name: "contextlib.contextmanager",
    group: "contextlib",
    module: Some("contextlib"),
    versions: &[(3, 14), (3, 15)],
    free_threaded: true,
    counters: &COUNTERS,
    prepare,
    release: Some(release),
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&TARGET];

mod fp {
    use crate::accel::Fingerprints;
    pub const CONTEXTMANAGER: Fingerprints = &[(3, 14, "e4c751871a4a69f6"), (3, 15, "f68ebc56c587d927")];
    pub const INIT: Fingerprints = &[(3, 14, "9a0c1fe778252123"), (3, 15, "39e83af693ad44ad")];
}

static CM_CODE: Slot = Slot::new();
static HELPER_CODE: Slot = Slot::new();
static GCM: Slot = Slot::new();
static INIT_FN: Slot = Slot::new();
static INIT_CODE: Slot = Slot::new();
static GCM_DOC: Slot = Slot::new();
static N_GEN: Slot = Slot::new();
static N_FUNC: Slot = Slot::new();
static N_ARGS: Slot = Slot::new();
static N_KWDS: Slot = Slot::new();
static N_DOC: Slot = Slot::new();
static N_INIT: Slot = Slot::new();
/// Tags of `_GeneratorContextManager` and its metaclass when validated (0: not yet).
static GCM_TAG: AtomicU32 = AtomicU32::new(0);
static META_TAG: AtomicU32 = AtomicU32::new(0);

fn release(_py: Python<'_>) {
    DYN.release();
    for s in [&CM_CODE, &HELPER_CODE, &GCM, &INIT_FN, &INIT_CODE, &GCM_DOC] {
        s.release();
    }
    GCM_TAG.store(0, Relaxed);
    META_TAG.store(0, Relaxed);
}

const CO_GENERATOR: i32 = 0x20;
const CO_COROUTINE: i32 = 0x80;
const CO_ITERABLE_COROUTINE: i32 = 0x100;
const CO_ASYNC_GENERATOR: i32 = 0x200;

/// `_GeneratorContextManager` (and its metaclass) still as validated.
#[inline(always)]
unsafe fn gcm_ok(gcm: Tp) -> bool {
    // SAFETY: a live type kept by `GCM`.
    unsafe {
        let meta = ffi::Py_TYPE(gcm.cast());
        let (t, m) = (tag_of(gcm), tag_of(meta));
        (t != 0 && t == GCM_TAG.load(Relaxed) && m != 0 && m == META_TAG.load(Relaxed)) || validate_gcm(gcm)
    }
}

/// The class creates and initializes its instances exactly as the kernel does.
#[cold]
unsafe fn validate_gcm(gcm: Tp) -> bool {
    // SAFETY: attached thread; MRO lookups and slot reads only.
    unsafe {
        let meta = ffi::Py_TYPE(gcm.cast());
        let (t, m) = (ensure_tag(gcm), ensure_tag(meta));
        let tt = &raw mut ffi::PyType_Type;
        let ob = &raw mut ffi::PyBaseObject_Type;
        let mut ok = addr!((*meta).tp_call) == addr!((*tt).tp_call)
            && addr!((*meta).tp_getattro) == addr!((*tt).tp_getattro)
            && addr!((*gcm).tp_new) == addr!((*ob).tp_new)
            && addr!((*gcm).tp_init) != addr!((*ob).tp_init)
            && addr!((*gcm).tp_setattro) == ffi::PyObject_GenericSetAttr as *const () as usize
            && addr!((*gcm).tp_alloc) == ffi::PyType_GenericAlloc as *const () as usize
            && ffi::PyType_GetFlags(gcm) & (ffi::Py_TPFLAGS_MANAGED_DICT | ffi::Py_TPFLAGS_IS_ABSTRACT)
                == ffi::Py_TPFLAGS_MANAGED_DICT
            && resolves_to(gcm, N_INIT.get(), INIT_FN.get());
        for n in [&N_GEN, &N_FUNC, &N_ARGS, &N_KWDS] {
            ok = ok && resolves_to(gcm, n.get(), null_mut());
        }
        // `__doc__`: the class's own (a plain value, read by `type(self).__doc__`).
        let doc = lookup(gcm, N_DOC.get());
        if doc.is_null() {
            ok = false;
        } else {
            let dt = ffi::Py_TYPE(doc);
            ok = ok && (*dt).tp_descr_get.is_none() && (*dt).tp_descr_set.is_none();
            if ok {
                GCM_DOC.keep(&Bound::from_borrowed_ptr(Python::assume_attached(), doc));
            }
            ffi::Py_DECREF(doc);
        }
        // The metaclass leaves `__doc__` to the class (no data descriptor before it).
        let md = lookup(meta, N_DOC.get());
        if !md.is_null() {
            ok = ok && (*ffi::Py_TYPE(md)).tp_descr_set.is_none();
            ffi::Py_DECREF(md);
        }
        if ok && t != 0 && m != 0 && tag_of(gcm) == t && tag_of(meta) == m {
            GCM_TAG.store(t, Relaxed);
            META_TAG.store(m, Relaxed);
        }
        ok
    }
}

/// The `helper(*args, **kwds)` closure of `contextmanager`.
unsafe extern "C" fn k_helper(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol; `f` is a function this target patched.
    unsafe {
        if func_code(f) != HELPER_CODE.get() {
            COUNTERS.fallback_streak(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DYN.alive() {
            DYN.retire(f, k_helper);
            return orig(f, args, nargsf, kw);
        }
        if !DEPS.ok()
            || !accel::tools_quiet(&TOOLS, f)
            || func_code(INIT_FN.get()) != INIT_CODE.get()
            || !accel::code_locally_quiet(INIT_CODE.get())
        {
            COUNTERS.fallback_streak(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        let g = cell(f, 0);
        if g.is_null() {
            COUNTERS.fallback_streak(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        let r = construct(g, args, nargs(nargsf), kw);
        ffi::Py_DECREF(g);
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

/// `_GeneratorContextManager(func, args, kwds)`; `None` before anything observable.
#[inline]
unsafe fn construct(g: P, args: *const P, n: usize, kw: P) -> Option<P> {
    // SAFETY: per `k_helper`.
    unsafe {
        if !is_function(g) || vectorcall_of(g) != function_vectorcall() {
            return None;
        }
        let gcode = func_code(g);
        let flags = (*gcode.cast::<crate::accel::version::RawCode>()).co_flags;
        if flags & CO_GENERATOR == 0
            || flags & (CO_COROUTINE | CO_ITERABLE_COROUTINE | CO_ASYNC_GENERATOR) != 0
            || !accel::code_locally_quiet(gcode)
        {
            return None;
        }
        let gcm = GCM.get().cast::<ffi::PyTypeObject>();
        if gcm.is_null() || !gcm_ok(gcm) {
            return None;
        }
        // The generator first: binding errors fall back (nothing observable yet).
        let genr = func_vectorcall()(g, args, n, kw);
        if genr.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        // From here on, CPython's order: helper's `*args`/`**kwds`, the instance.
        let a = ffi::PyTuple_New(n as ffi::Py_ssize_t);
        let d = ffi::PyDict_New();
        let mut ok = !a.is_null() && !d.is_null();
        if ok {
            for i in 0..n {
                ffi::PyTuple_SET_ITEM(a, i as ffi::Py_ssize_t, incref(*args.add(i)));
            }
            if !kw.is_null() {
                let nk = ffi::PyTuple_GET_SIZE(kw) as usize;
                for j in 0..nk {
                    if ffi::PyDict_SetItem(d, ffi::PyTuple_GET_ITEM(kw, j as ffi::Py_ssize_t), *args.add(n + j)) < 0 {
                        ok = false;
                        break;
                    }
                }
            }
        }
        let obj = if ok { ffi::PyType_GenericAlloc(gcm, 0) } else { null_mut() };
        ok = ok && !obj.is_null();
        // func.__doc__ (a C getter of an exact function), or the class's.
        let mut doc = if ok { ffi::PyObject_GetAttr(g, N_DOC.get()) } else { null_mut() };
        if ok && doc.is_null() {
            ok = false;
        }
        if ok && doc == ffi::Py_None() {
            ffi::Py_DECREF(doc);
            doc = incref(GCM_DOC.get());
        }
        ok = ok
            && ffi::PyObject_GenericSetAttr(obj, N_GEN.get(), genr) == 0
            && ffi::PyObject_GenericSetAttr(obj, N_FUNC.get(), g) == 0
            && ffi::PyObject_GenericSetAttr(obj, N_ARGS.get(), a) == 0
            && ffi::PyObject_GenericSetAttr(obj, N_KWDS.get(), d) == 0
            && ffi::PyObject_GenericSetAttr(obj, N_DOC.get(), doc) == 0;
        for o in [genr, a, d, doc] {
            if !o.is_null() {
                ffi::Py_DECREF(o);
            }
        }
        if !ok {
            if !obj.is_null() {
                ffi::Py_DECREF(obj);
            }
            // Only allocation failures get here: CPython would raise them too.
            return Some(null_mut());
        }
        Some(obj)
    }
}

/// `contextlib.contextmanager(func)`: CPython's, then the new `helper` gets the kernel.
unsafe extern "C" fn k_contextmanager(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol.
    unsafe {
        let h = orig(f, args, nargsf, kw);
        if !h.is_null() && DEPS.ok() && func_code(f) == CM_CODE.get() && is_function(h) && func_code(h) == HELPER_CODE.get()
        {
            DYN.install(h, k_helper);
        }
        h
    }
}

fn prepare(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let cm = cx.module_attr("contextmanager")?;
    if !cm.is_exact_instance_of::<pyo3::types::PyFunction>() {
        return Err(Refusal::new("contextlib.contextmanager is not a function"));
    }
    cx.fingerprint_code(&cm, fp::CONTEXTMANAGER)?;
    // SAFETY: an exact function.
    let code = unsafe { Bound::from_borrowed_ptr(py, func_code(cm.as_ptr())) };
    let consts = code.getattr("co_consts")?.cast_into_exact::<PyTuple>().map_err(PyErr::from)?;
    let mut helper = None;
    for c in consts.iter() {
        if c.is_exact_instance_of::<PyCode>() && c.getattr("co_name")?.eq("helper")? {
            helper = Some(c);
        }
    }
    let helper = helper.ok_or_else(|| Refusal::new("contextmanager: no helper code"))?;
    let fv = helper.getattr("co_freevars")?.cast_into_exact::<PyTuple>().map_err(PyErr::from)?;
    if fv.len() != 1 || fv.get_item(0)?.ne("func")? {
        return Err(Refusal::new("contextmanager: unexpected helper closure"));
    }
    let gcm = cx.get(&g, "_GeneratorContextManager")?.cast_into::<PyType>().map_err(PyErr::from)?;
    cx.pin(&g, "_GeneratorContextManager", &DEPS)?;
    let base = cx.get(&g, "_GeneratorContextManagerBase")?.cast_into::<PyType>().map_err(PyErr::from)?;
    let init = cx.type_attr(&base, "__init__")?;
    if !init.is_exact_instance_of::<pyo3::types::PyFunction>() {
        return Err(Refusal::new("_GeneratorContextManagerBase.__init__ is not a function"));
    }
    cx.fingerprint_code(&init, fp::INIT)?;
    // `getattr(func, "__doc__", None)` in __init__.
    cx.pin_global(&g, "getattr", &DEPS)?;
    cx.pin_global(&g, "type", &DEPS)?;
    CM_CODE.keep(&code);
    HELPER_CODE.keep(&helper);
    GCM.keep(gcm.as_any());
    INIT_FN.keep(&init);
    // SAFETY: an exact function.
    INIT_CODE.keep(&unsafe { Bound::from_borrowed_ptr(py, func_code(init.as_ptr())) });
    for (s, n) in [(&N_GEN, "gen"), (&N_FUNC, "func"), (&N_ARGS, "args"), (&N_KWDS, "kwds"), (&N_DOC, "__doc__"), (&N_INIT, "__init__")]
    {
        s.name(py, n);
    }
    GCM_TAG.store(0, Relaxed);
    META_TAG.store(0, Relaxed);
    cx.vectorcall(&cm, k_contextmanager, &accel::FUNCTION_VECTORCALL)?;
    DYN.set_hook(cm.as_ptr(), k_contextmanager);
    Ok(())
}

/// Number of live helpers with the kernel (tests).
pub fn dynamic_sites() -> usize {
    DYN.live()
}
