//! Target `dataclasses.__init__` (group `generated`): the `__init__` that `@dataclass`
//! generates for **frozen** classes (each field set with `object.__setattr__`, 2.4–2.8× the
//! cost of a plain class; non-frozen `__init__`s are plain `STORE_ATTR`s already at the
//! floor and are left alone).
//!
//! Recognition, by construction, from CPython's own generator output. The hook is a
//! vectorcall kernel on `dataclasses._FuncBuilder.add_fns_to_class` (fingerprinted; it is
//! where the generated source is compiled and the functions set on the class). Before
//! calling it, the kernel reads the exact source text the generator produced for
//! `__init__` (`self.src`); after it returns, the class's own `__init__` is the function
//! compiled from that text: nothing in between can run user code (metaclass with `type`'s
//! `__setattr__`/`__getattribute__`, `__init__` not defined by the class itself, every
//! global `add_fns_to_class` uses pinned: `exec`, `zip`, `setattr`, `_set_new_attribute`,
//! `_make_annotate_function`, and `recursive_repr`, the only object `__create_fn__` calls).
//! The text is parsed by a strict parser that accepts only the frozen form `_init_fn`
//! emits without `__post_init__`; the plan (parameters, store steps, closure cells) is read
//! off the text, and the function's code object must agree with it (argument counts,
//! flags, `co_varnames`, `co_freevars`). No reference is compiled (a compile is an audit
//! event: user code).
//!
//! Exact subset of a call (everything else runs CPython's function):
//! - arguments bind (positional, keywords by identity with the interned parameter names,
//!   `__defaults__`/`__kwdefaults__` read at call time; new references held throughout);
//! - `self` is not a class, and for its type (cached per `tp_version_tag`) every field's
//!   store through `PyObject_GenericSetAttr` runs no Python code: the name resolves to
//!   nothing (instance dict present), to a writable member descriptor of a base (`__slots__`)
//!   or to a plain value whose type has no `__set__` (re-checked per call when that type is
//!   mutable);
//! - the closure cells hold `object` (`__dataclass_builtins_object__`), the
//!   `_HAS_DEFAULT_FACTORY` sentinel, and, for each factory the call would run, exactly
//!   `list`, `dict` or `set` (calling them runs no Python code); checked before the first
//!   store, so a fallback restarts from scratch.

use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::*};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyList, PyString, PyTuple, PyType};

use super::adapter::Dynamic;
use super::common::*;
use crate::accel::{self, Counters, DepFlag, Prepare, Reason, Refusal, TargetSpec, Tools};

pub(super) static COUNTERS: Counters = Counters::new();
static DEPS: DepFlag = DepFlag::new();
static TOOLS: Tools = Tools::new();
pub(super) static DYN: Dynamic = Dynamic::new();

pub static TARGET: TargetSpec = TargetSpec {
    name: "dataclasses.__init__",
    group: "generated",
    module: Some("dataclasses"),
    versions: &[(3, 14), (3, 15)],
    free_threaded: true,
    counters: &COUNTERS,
    prepare,
    release: Some(release),
    ..TargetSpec::DEFAULT
};

mod fp {
    use crate::accel::Fingerprints;
    pub const ADD_FNS_TO_CLASS: Fingerprints = &[(3, 14, "d8c9cfa386895a5d"), (3, 15, "efedeab6fd7c3f3d")];
    pub const SET_NEW_ATTRIBUTE: Fingerprints = &[(3, 14, "cd74a820d7aeba4f"), (3, 15, "1fd0ba6b39d0b48c")];
}

const DC_TAG: u32 = 0x4443_0001;

static ADD_CODE: Slot = Slot::new();
static FB_TYPE: Slot = Slot::new();
static HAS_FACTORY: Slot = Slot::new();
static N_SRC: Slot = Slot::new();
static N_NAMES: Slot = Slot::new();
static N_INIT: Slot = Slot::new();
static N_QUALNAME: Slot = Slot::new();

fn release(_py: Python<'_>) {
    DYN.release();
    for s in [&ADD_CODE, &FB_TYPE, &HAS_FACTORY] {
        s.release();
    }
}

// ---------------------------------------------------------------------------------------
// The generated text and its plan
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Value {
    /// `N` (a parameter).
    Param,
    /// `__dataclass_dflt_N__() if N is __dataclass_HAS_DEFAULT_FACTORY__ else N`.
    FactoryOrParam,
    /// `__dataclass_dflt_N__()` (`init=False` with a factory).
    Factory,
    /// `__dataclass_dflt_N__` (`init=False` with a default, slots classes).
    CellValue,
}

#[derive(Debug)]
struct Parsed {
    self_name: String,
    /// Parameters after `self`, in order; `kwonly_from` is the index of the first
    /// keyword-only one (`params.len()` if none).
    params: Vec<String>,
    kwonly_from: usize,
    /// Store steps in body order.
    steps: Vec<(String, Value)>,
}

fn ident(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c == '_' || c.is_alphanumeric())
}

/// Parse the source `_init_fn` produced for a frozen class without `__post_init__`
/// (`add_fn`'s format: `" def __init__(" + ",".join(params) + "):\n" + "\n".join(body)`).
/// `None` for anything else.
fn parse(text: &str) -> Option<Parsed> {
    let rest = text.strip_prefix(" def __init__(")?;
    let close = rest.find("):\n")?;
    let (head, body) = (&rest[..close], &rest[close + 3..]);
    let mut toks = head.split(',');
    let self_name = toks.next()?.to_owned();
    if self_name != "self" && self_name != "__dataclass_self__" {
        return None;
    }
    let mut params: Vec<String> = Vec::new();
    let mut kwonly_from = None;
    for t in toks {
        if t == "*" {
            if kwonly_from.is_some() {
                return None;
            }
            kwonly_from = Some(params.len());
            continue;
        }
        let name = match t.split_once('=') {
            None => t,
            Some((n, d)) => {
                if d != "__dataclass_HAS_DEFAULT_FACTORY__" && d != format!("__dataclass_dflt_{n}__") {
                    return None;
                }
                n
            }
        };
        if !ident(name) || name == self_name || params.iter().any(|p| p == name) {
            return None;
        }
        params.push(name.to_owned());
    }
    let kwonly_from = kwonly_from.unwrap_or(params.len());
    if kwonly_from == params.len() && head.ends_with(",*") {
        return None;
    }
    let mut steps: Vec<(String, Value)> = Vec::new();
    let lines: Vec<&str> = body.split('\n').collect();
    if lines == ["  pass"] {
        return Some(Parsed { self_name, params, kwonly_from, steps });
    }
    let pre = format!("  __dataclass_builtins_object__.__setattr__({self_name},'");
    for line in lines {
        let l = line.strip_prefix(pre.as_str())?;
        let (n, v) = l.split_once("',")?;
        let v = v.strip_suffix(')')?;
        if !ident(n) || steps.iter().any(|(s, _)| s == n) {
            return None;
        }
        let is_param = params.iter().any(|p| p == n);
        let dflt = format!("__dataclass_dflt_{n}__");
        let kind = if v == n && is_param {
            Value::Param
        } else if is_param && v == format!("{dflt}() if {n} is __dataclass_HAS_DEFAULT_FACTORY__ else {n}") {
            Value::FactoryOrParam
        } else if !is_param && v == format!("{dflt}()") {
            Value::Factory
        } else if !is_param && v == dflt {
            Value::CellValue
        } else {
            return None;
        };
        steps.push((n.to_owned(), kind));
    }
    Some(Parsed { self_name, params, kwonly_from, steps })
}

struct Step {
    name: P,
    /// Index into the bound parameters (`Param`/`FactoryOrParam`).
    param: usize,
    kind: Value,
    /// Closure index of `__dataclass_dflt_N__` (factory / value).
    cell: usize,
}

#[repr(C)]
struct DcPlan {
    h: PlanHeader,
    sig: Sig,
    steps: Vec<Step>,
    obj_cell: usize,
    sentinel_cell: usize,
    /// Version tags of `self` types validated for the stores (never reused, 0 = empty).
    ok_tags: [AtomicU32; 4],
    next: AtomicUsize,
}

unsafe fn free_dc_plan(p: *mut PlanHeader) {
    // SAFETY: created by Box::into_raw of a DcPlan.
    drop(unsafe { Box::from_raw(p.cast::<DcPlan>()) });
}

/// The plan for the function compiled from `text`, after checking its code agrees.
fn build_plan(py: Python<'_>, func: &Bound<'_, PyAny>, parsed: &Parsed) -> PyResult<Option<DcPlan>> {
    let code = func.getattr("__code__")?;
    let int = |n: &str| -> PyResult<i64> { code.getattr(n)?.extract() };
    const CO_VARARGS: i64 = 0x04;
    const CO_VARKEYWORDS: i64 = 0x08;
    const CO_GEN_ANY: i64 = 0x20 | 0x80 | 0x100 | 0x200;
    let np = parsed.params.len();
    if np > MAX_PARAMS
        || int("co_argcount")? != 1 + parsed.kwonly_from as i64
        || int("co_kwonlyargcount")? != (np - parsed.kwonly_from) as i64
        || int("co_posonlyargcount")? != 0
        || int("co_flags")? & (CO_VARARGS | CO_VARKEYWORDS | CO_GEN_ANY) != 0
    {
        return Ok(None);
    }
    let varnames = code.getattr("co_varnames")?.cast_into_exact::<PyTuple>()?;
    if varnames.len() < np + 1 || varnames.get_item(0)?.ne(parsed.self_name.as_str())? {
        return Ok(None);
    }
    for (i, p) in parsed.params.iter().enumerate() {
        if varnames.get_item(i + 1)?.ne(p.as_str())? {
            return Ok(None);
        }
    }
    // The free variables the body uses, as a set.
    let mut want: Vec<String> = vec!["__dataclass_builtins_object__".into()];
    let mut factory_param = false;
    for (n, k) in &parsed.steps {
        if *k != Value::Param {
            want.push(format!("__dataclass_dflt_{n}__"));
        }
        factory_param |= *k == Value::FactoryOrParam;
    }
    if factory_param {
        want.push("__dataclass_HAS_DEFAULT_FACTORY__".into());
    }
    if parsed.steps.is_empty() {
        want.clear();
    }
    let fv = code.getattr("co_freevars")?.cast_into_exact::<PyTuple>()?;
    let fv: Vec<String> = fv.iter().map(|v| v.extract::<String>()).collect::<PyResult<_>>()?;
    let mut a = fv.clone();
    let mut b = want.clone();
    a.sort();
    b.sort();
    if a != b || !code.getattr("co_cellvars")?.cast_into_exact::<PyTuple>()?.is_empty() {
        return Ok(None);
    }
    let ix = |n: &str| fv.iter().position(|v| v == n).unwrap_or(usize::MAX);
    let names: Vec<P> = parsed.params.iter().map(|p| intern(py, p)).collect();
    let steps = parsed
        .steps
        .iter()
        .map(|(n, k)| Step {
            name: intern(py, n),
            param: parsed.params.iter().position(|p| p == n).unwrap_or(usize::MAX),
            kind: *k,
            cell: ix(&format!("__dataclass_dflt_{n}__")),
        })
        .collect();
    Ok(Some(DcPlan {
        h: PlanHeader { tag: DC_TAG, free: free_dc_plan },
        sig: Sig { names, npos: parsed.kwonly_from },
        steps,
        obj_cell: ix("__dataclass_builtins_object__"),
        sentinel_cell: ix("__dataclass_HAS_DEFAULT_FACTORY__"),
        ok_tags: [const { AtomicU32::new(0) }; 4],
        next: AtomicUsize::new(0),
    }))
}

// SAFETY: interned names and atomics; immutable after construction otherwise.
unsafe impl Send for DcPlan {}
// SAFETY: as Send.
unsafe impl Sync for DcPlan {}

// ---------------------------------------------------------------------------------------
// The store check for `self`'s type
// ---------------------------------------------------------------------------------------

#[repr(C)]
struct RawDescr {
    ob_base: ffi::PyObject,
    d_type: Tp,
    d_name: P,
    d_qualname: P,
}

#[repr(C)]
struct RawMemberDescr {
    common: RawDescr,
    d_member: *const ffi::PyMemberDef,
}

const READONLY: std::ffi::c_int = 1;

/// Verdict for `self`'s type: `Some(true)` every store is plain and the verdict may be
/// cached; `Some(false)` plain now but a field's class value has a mutable type (re-check
/// next call); `None` some store could run code or fail.
unsafe fn stores_plain(plan: &DcPlan, t: Tp) -> Option<bool> {
    // SAFETY: attached thread, live type; MRO lookups run no Python code.
    unsafe {
        let has_dict = (*t).tp_dictoffset != 0 || ffi::PyType_GetFlags(t) & ffi::Py_TPFLAGS_MANAGED_DICT != 0;
        let mut cacheable = true;
        for s in &plan.steps {
            let d = lookup(t, s.name);
            if d.is_null() {
                if !has_dict {
                    return None;
                }
                continue;
            }
            let dt = ffi::Py_TYPE(d);
            let ok = if dt == &raw mut ffi::PyMemberDescr_Type {
                let m = d.cast::<RawMemberDescr>();
                let owner = (*m).common.d_type;
                !owner.is_null()
                    && ffi::PyType_IsSubtype(t, owner) != 0
                    && !(*m).d_member.is_null()
                    && (*(*m).d_member).flags & READONLY == 0
            } else if (*dt).tp_descr_set.is_none() {
                if ffi::PyType_GetFlags(dt) & ffi::Py_TPFLAGS_IMMUTABLETYPE == 0 {
                    cacheable = false;
                }
                has_dict
            } else {
                false
            };
            ffi::Py_DECREF(d);
            if !ok {
                return None;
            }
        }
        Some(cacheable)
    }
}

#[inline]
unsafe fn type_ok(plan: &DcPlan, t: Tp) -> bool {
    // SAFETY: per `stores_plain`.
    unsafe {
        let tag = tag_of(t);
        if tag != 0 && plan.ok_tags.iter().any(|s| s.load(Relaxed) == tag) {
            return true;
        }
        check_type(plan, t)
    }
}

#[cold]
unsafe fn check_type(plan: &DcPlan, t: Tp) -> bool {
    // SAFETY: per `stores_plain`.
    unsafe {
        let tag = ensure_tag(t);
        match stores_plain(plan, t) {
            None => false,
            Some(cacheable) => {
                if cacheable && tag != 0 && tag_of(t) == tag {
                    let i = plan.next.fetch_add(1, Relaxed) % plan.ok_tags.len();
                    plan.ok_tags[i].store(tag, Relaxed);
                }
                true
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Kernel
// ---------------------------------------------------------------------------------------

#[inline]
fn is_builtin_factory(f: P) -> bool {
    f == (&raw mut ffi::PyList_Type).cast() || f == (&raw mut ffi::PyDict_Type).cast() || f == (&raw mut ffi::PySet_Type).cast()
}

/// The generated frozen `__init__` (reached from `slot_tp_init`, or `super().__init__`).
unsafe extern "C" fn k_dc_init(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol; `f` is a function this target patched.
    unsafe {
        let plan = plan_of(func_code(f), DC_TAG).cast::<DcPlan>();
        let n = nargs(nargsf);
        if plan.is_null() || n == 0 {
            COUNTERS.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DYN.alive() {
            DYN.retire(f, k_dc_init);
            COUNTERS.fallback(Reason::Deps);
            return orig(f, args, nargsf, kw);
        }
        if !accel::tools_quiet(&TOOLS, f) {
            COUNTERS.fallback(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        let plan = &*plan;
        let this = *args;
        if ffi::PyType_Check(this) != 0 || !type_ok(plan, ffi::Py_TYPE(this)) {
            COUNTERS.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        let mut vals = [null_mut(); MAX_PARAMS];
        if !bind(&plan.sig, f, args, n, kw, &mut vals) {
            COUNTERS.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        let np = plan.sig.names.len();
        let r = run_steps(plan, f, this, &vals);
        unbind(&vals, np);
        match r {
            Some(o) => {
                COUNTERS.hit();
                o
            }
            None => {
                COUNTERS.fallback(Reason::Deps);
                orig(f, args, nargsf, kw)
            }
        }
    }
}

/// Validate the cells, then store every field. `None`: nothing was done (fall back);
/// `Some(NULL)`: a store failed after earlier ones (CPython's error, raised).
#[inline]
unsafe fn run_steps(plan: &DcPlan, f: P, this: P, vals: &[P; MAX_PARAMS]) -> Option<P> {
    // SAFETY: per `k_dc_init`; references to cell contents are held across the stores.
    unsafe {
        if plan.steps.is_empty() {
            return Some(incref(ffi::Py_None()));
        }
        if !cell_is(f, plan.obj_cell, (&raw mut ffi::PyBaseObject_Type).cast()) {
            return None;
        }
        // Values to store (new references), decided before the first store.
        let mut out: [P; MAX_PARAMS] = [null_mut(); MAX_PARAMS];
        let mut held: [P; MAX_PARAMS] = [null_mut(); MAX_PARAMS];
        let mut sentinel: P = null_mut();
        let mut ok = true;
        let ns = plan.steps.len();
        if ns > MAX_PARAMS {
            return None;
        }
        for (i, s) in plan.steps.iter().enumerate() {
            match s.kind {
                Value::Param => out[i] = vals[s.param],
                Value::FactoryOrParam | Value::Factory => {
                    let use_factory = if s.kind == Value::Factory {
                        true
                    } else {
                        if sentinel.is_null() {
                            sentinel = cell(f, plan.sentinel_cell);
                            if sentinel.is_null() || sentinel != HAS_FACTORY.get() {
                                ok = false;
                                break;
                            }
                        }
                        vals[s.param] == sentinel
                    };
                    if use_factory {
                        let fac = cell(f, s.cell);
                        if fac.is_null() || !is_builtin_factory(fac) {
                            if !fac.is_null() {
                                ffi::Py_DECREF(fac);
                            }
                            ok = false;
                            break;
                        }
                        held[i] = fac;
                    } else {
                        out[i] = vals[s.param];
                    }
                }
                Value::CellValue => {
                    let v = cell(f, s.cell);
                    if v.is_null() {
                        ok = false;
                        break;
                    }
                    held[i] = v;
                    out[i] = v;
                }
            }
        }
        if !sentinel.is_null() {
            ffi::Py_DECREF(sentinel);
        }
        let release = |held: &[P; MAX_PARAMS]| {
            for &h in held.iter().take(ns) {
                if !h.is_null() {
                    ffi::Py_DECREF(h);
                }
            }
        };
        if !ok {
            release(&held);
            return None;
        }
        let mut result = incref(ffi::Py_None());
        for (i, s) in plan.steps.iter().enumerate() {
            let mut v = out[i];
            let mut made = false;
            if v.is_null() {
                // A builtin factory: list(), dict(), set().
                v = ffi::PyObject_CallNoArgs(held[i]);
                if v.is_null() {
                    ffi::Py_DECREF(result);
                    result = null_mut();
                    break;
                }
                made = true;
            }
            let rc = ffi::PyObject_GenericSetAttr(this, s.name, v);
            if made {
                ffi::Py_DECREF(v);
            }
            if rc < 0 {
                ffi::Py_DECREF(result);
                result = null_mut();
                break;
            }
        }
        release(&held);
        Some(result)
    }
}

// ---------------------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------------------

/// What the hook captured before `add_fns_to_class` ran.
struct Captured {
    text: String,
}

/// `self` is a `_FuncBuilder` about to add an `__init__` to `cls`, a class whose
/// attribute protocol is `type`'s and which does not define `__init__` itself.
unsafe fn capture(builder: P, cls: P) -> Option<Captured> {
    // SAFETY: attached thread; only C getters and exact-type container reads.
    unsafe {
        if ffi::Py_TYPE(builder).cast::<ffi::PyObject>() != FB_TYPE.get() || ffi::PyType_Check(cls) == 0 {
            return None;
        }
        let meta = ffi::Py_TYPE(cls);
        let tt = &raw mut ffi::PyType_Type;
        if addr!((*meta).tp_setattro) != addr!((*tt).tp_setattro) || addr!((*meta).tp_getattro) != addr!((*tt).tp_getattro) {
            return None;
        }
        // `__qualname__` read by add_fns_to_class: `type`'s own descriptor.
        let q = lookup(meta, N_QUALNAME.get());
        let qt = lookup(tt, N_QUALNAME.get());
        let same = q == qt;
        for o in [q, qt] {
            if !o.is_null() {
                ffi::Py_DECREF(o);
            }
        }
        if !same {
            return None;
        }
        let py = Python::assume_attached();
        let cls_b = Bound::from_borrowed_ptr(py, cls).cast_into::<PyType>().ok()?;
        let td = crate::accel::prepare::type_dict(&cls_b).ok()?;
        if td.contains(N_INIT.get_bound(py)).ok()? {
            return None;
        }
        // The builder's own attributes (instance dict of a plain class).
        let src = get_plain(builder, N_SRC.get());
        let names = get_plain(builder, N_NAMES.get());
        let r = (|| {
            let src = Bound::from_owned_ptr_or_opt(py, src)?.cast_into_exact::<PyList>().ok()?;
            let names = Bound::from_owned_ptr_or_opt(py, names)?.cast_into_exact::<PyList>().ok()?;
            if src.len() != names.len() {
                return None;
            }
            let mut at = None;
            for (i, n) in names.iter().enumerate() {
                if n.is_exact_instance_of::<PyString>() && n.eq("__init__").ok()? {
                    if at.is_some() {
                        return None;
                    }
                    at = Some(i);
                }
            }
            let t = src.get_item(at?).ok()?;
            let t = t.cast_into_exact::<PyString>().ok()?;
            Some(Captured { text: t.to_str().ok()?.to_owned() })
        })();
        if r.is_none() && !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
        }
        r
    }
}

/// After `add_fns_to_class` returned: the class's `__init__` is compiled from `text`.
unsafe fn adopt(cls: P, cap: Captured) {
    // SAFETY: attached thread; C getters, dict reads, our own plan.
    unsafe {
        let py = Python::assume_attached();
        let Some(parsed) = parse(&cap.text) else { return };
        if parsed.steps.is_empty() && parsed.params.is_empty() {
            // `def __init__(self): pass`: nothing to gain.
            return;
        }
        let r = (|| -> PyResult<Option<P>> {
            let cls = Bound::from_borrowed_ptr(py, cls).cast_into::<PyType>()?;
            let td = crate::accel::prepare::type_dict(&cls)
                .map_err(|r| pyo3::exceptions::PyRuntimeError::new_err(r.reason().to_owned()))?;
            let Some(func) = td.get_item(N_INIT.get_bound(py))? else { return Ok(None) };
            if !func.is_exact_instance_of::<pyo3::types::PyFunction>() {
                return Ok(None);
            }
            let code = func.getattr("__code__")?;
            if code.getattr("co_qualname")?.ne("__create_fn__.<locals>.__init__")?
                || code.getattr("co_filename")?.ne("<string>")?
            {
                return Ok(None);
            }
            let Some(plan) = build_plan(py, &func, &parsed)? else { return Ok(None) };
            let plan = Box::into_raw(Box::new(plan));
            if !set_plan(func_code(func.as_ptr()), plan.cast()) {
                return Ok(None);
            }
            Ok(Some(func.as_ptr()))
        })();
        match r {
            Ok(Some(f)) => {
                DYN.install(f, k_dc_init);
            }
            Ok(None) => {}
            Err(_) => ffi::PyErr_Clear(),
        }
    }
}

/// `_FuncBuilder.add_fns_to_class(self, cls)`.
unsafe extern "C" fn k_add_fns(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol.
    unsafe {
        let cap = if nargs(nargsf) == 2 && kw.is_null() && DEPS.ok() && func_code(f) == ADD_CODE.get() && DYN.alive() {
            capture(*args, *args.add(1))
        } else {
            None
        };
        let cls = if cap.is_some() { incref(*args.add(1)) } else { null_mut() };
        let r = orig(f, args, nargsf, kw);
        if let Some(cap) = cap {
            if !r.is_null() && DEPS.ok() {
                adopt(cls, cap);
            }
            ffi::Py_DECREF(cls);
        }
        r
    }
}

// ---------------------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------------------

fn prepare(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let fb = cx.module_attr("_FuncBuilder")?.cast_into::<PyType>().map_err(PyErr::from)?;
    let add = cx.type_attr(&fb, "add_fns_to_class")?;
    cx.fingerprint_code(&add, fp::ADD_FNS_TO_CLASS)?;
    let sna = cx.module_attr("_set_new_attribute")?;
    cx.fingerprint_code(&sna, fp::SET_NEW_ATTRIBUTE)?;
    for name in ["_set_new_attribute", "_make_annotate_function", "recursive_repr", "_HAS_DEFAULT_FACTORY"] {
        cx.pin(&g, name, &DEPS)?;
    }
    for name in ["exec", "zip", "setattr", "TypeError"] {
        cx.pin_global(&g, name, &DEPS)?;
    }
    if !add.is_exact_instance_of::<pyo3::types::PyFunction>() {
        return Err(Refusal::new("_FuncBuilder.add_fns_to_class is not a function"));
    }
    // SAFETY: an exact function.
    let code = unsafe { Bound::from_borrowed_ptr(py, func_code(add.as_ptr())) };
    ADD_CODE.keep(&code);
    FB_TYPE.keep(fb.as_any());
    HAS_FACTORY.keep(&cx.module_attr("_HAS_DEFAULT_FACTORY")?);
    N_SRC.name(py, "src");
    N_NAMES.name(py, "names");
    N_INIT.name(py, "__init__");
    N_QUALNAME.name(py, "__qualname__");
    extra_index().ok_or_else(|| Refusal::new("no code-extra index available"))?;
    cx.vectorcall(&add, k_add_fns, &accel::FUNCTION_VECTORCALL)?;
    DYN.set_hook(add.as_ptr(), k_add_fns);
    Ok(())
}

/// Number of live `__init__` functions with the kernel (tests).
pub fn dynamic_sites() -> usize {
    DYN.live()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frozen_forms() {
        let t = " def __init__(self,x,y=__dataclass_dflt_y__,z=__dataclass_HAS_DEFAULT_FACTORY__,*,w):\n  __dataclass_builtins_object__.__setattr__(self,'x',x)\n  __dataclass_builtins_object__.__setattr__(self,'y',y)\n  __dataclass_builtins_object__.__setattr__(self,'z',__dataclass_dflt_z__() if z is __dataclass_HAS_DEFAULT_FACTORY__ else z)\n  __dataclass_builtins_object__.__setattr__(self,'w',w)\n  __dataclass_builtins_object__.__setattr__(self,'v',__dataclass_dflt_v__())";
        let p = parse(t).unwrap();
        assert_eq!(p.params, ["x", "y", "z", "w"]);
        assert_eq!(p.kwonly_from, 3);
        assert_eq!(
            p.steps.iter().map(|s| s.1).collect::<Vec<_>>(),
            [Value::Param, Value::Param, Value::FactoryOrParam, Value::Param, Value::Factory]
        );
    }

    #[test]
    fn rejects_other_forms() {
        // non-frozen
        assert!(parse(" def __init__(self,x):\n  self.x=x").is_none());
        // __post_init__
        assert!(
            parse(" def __init__(self,x):\n  __dataclass_builtins_object__.__setattr__(self,'x',x)\n  self.__post_init__()")
                .is_none()
        );
        // value of another parameter
        assert!(parse(" def __init__(self,x,y):\n  __dataclass_builtins_object__.__setattr__(self,'x',y)").is_none());
        // unknown default
        assert!(parse(" def __init__(self,x=1):\n  __dataclass_builtins_object__.__setattr__(self,'x',x)").is_none());
        assert_eq!(parse(" def __init__(self):\n  pass").unwrap().steps.len(), 0);
    }
}
