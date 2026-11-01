//! Group `copy`: `copy.deepcopy` and `copy.copy`; and (in [`dc`]) group `dataclasses`:
//! `dataclasses.asdict`, `astuple`, `replace`. All are vectorcall kernels on the existing
//! function objects (contract §3.7). See `docs/targets/copy.md`.

pub mod dc;
mod deep;
mod memo;
mod reduce;
mod rt;
mod shallow;

use std::ffi::c_int;
use std::sync::atomic::Ordering::Relaxed;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

use crate::accel::{self, Counters, DepFlag, Prepare, Reason, Refusal, TargetSpec, Tools};
use rt::*;

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

static C_DEEPCOPY: Counters = Counters::new();
static C_COPY: Counters = Counters::new();
/// The module globals and builtins `copy.py` reads at call time are the objects validated.
static DEPS_COPY: DepFlag = DepFlag::new();
static TOOLS_DEEPCOPY: Tools = Tools::new();
static TOOLS_COPY: Tools = Tools::new();

pub static DEEPCOPY: TargetSpec = TargetSpec {
    name: "copy.deepcopy",
    group: "copy",
    module: Some("copy"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_DEEPCOPY,
    prepare: prepare_deepcopy,
    ..TargetSpec::DEFAULT
};

pub static COPY: TargetSpec = TargetSpec {
    name: "copy.copy",
    group: "copy",
    module: Some("copy"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_COPY,
    prepare: prepare_copy,
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&DEEPCOPY, &COPY];

/// `_PyObject_GetMethod` of the running interpreter.
static GET_METHOD: accel::Original<reduce::GetMethodFn> = accel::Original::new();

#[inline(always)]
unsafe fn original(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: the saved `_PyFunction_Vectorcall`.
    unsafe { accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kw) }
}

#[inline(always)]
fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

/// Is the set `deepcopy` checks atomic types against intact (3.14's is a mutable set)?
#[inline(always)]
unsafe fn atomic_fast(st: &St) -> bool {
    match &st.atomic_snap {
        None => true,
        // SAFETY: live set.
        Some(s) => unsafe { s.matches(st.atomic) || same_types(st.atomic, &st.atomic_types) },
    }
}

/// Does the (mutable, 3.14) set hold exactly `known` (after an add/discard, its table can
/// differ from the snapshot while its content is the validated one)?
#[cold]
unsafe fn same_types(set: O, known: &[*mut ffi::PyTypeObject]) -> bool {
    // SAFETY: a live set; the known entries are builtin types (pointer hashes).
    unsafe {
        ffi::PySet_Size(set) == known.len() as isize && known.iter().all(|&t| ffi::PySet_Contains(set, t.cast()) == 1)
    }
}

/// Are `copy.copy`'s two type sets intact (3.14's are mutable sets)?
#[inline(always)]
unsafe fn copy_sets_fast(st: &St) -> bool {
    // SAFETY: live sets.
    unsafe {
        match (&st.copy_atomic_snap, &st.copy_containers_snap) {
            (Some(a), Some(b)) => {
                (a.matches(st.copy_atomic) || same_types(st.copy_atomic, &st.copy_atomic_types))
                    && (b.matches(st.copy_containers) || same_types(st.copy_containers, &st.copy_container_types))
            }
            _ => true,
        }
    }
}

/// Is the `copy.deepcopy` kernel installed and its guards satisfied, so that callers inside
/// Rustique (dataclasses) may run the engine directly instead of calling the function?
#[inline(always)]
unsafe fn deepcopy_direct(st: &St, f: O) -> bool {
    // SAFETY: live function object.
    unsafe {
        f == st.deepcopy_fn && {
            let fo = f.cast::<ffi::PyFunctionObject>();
            (*fo).vectorcall.map(|v| v as usize) == Some(vc_deepcopy as ffi::vectorcallfunc as usize)
                && DEPS_COPY.ok()
                && (*fo).func_code == st.deepcopy_code
                && (*fo).func_defaults == st.deepcopy_defaults
                && accel::code_locally_quiet(st.deepcopy_code)
        }
    }
}

/// Kernel for `copy.deepcopy(x, memo=None)`.
unsafe extern "C" fn vc_deepcopy(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let st = st();
        let n = nargs(nargsf);
        let memo = if kw.is_null() {
            match n {
                1 => std::ptr::null_mut(),
                2 => *args.add(1),
                _ => {
                    C_DEEPCOPY.fallback_streak(Reason::Args);
                    return original(func, args, nargsf, kw);
                }
            }
        } else if n == 1 && ffi::PyTuple_GET_SIZE(kw) == 1 && ffi::PyTuple_GET_ITEM(kw, 0) == st.s_memo {
            *args.add(1)
        } else {
            C_DEEPCOPY.fallback_streak(Reason::Args);
            return original(func, args, nargsf, kw);
        };
        let memo = if memo == ffi::Py_None() { std::ptr::null_mut() } else { memo };
        // Any other memo's `__contains__`/`__getitem__` could be user code.
        if !memo.is_null() && ty(memo) != &raw mut ffi::PyDict_Type {
            C_DEEPCOPY.fallback_streak(Reason::Args);
            return original(func, args, nargsf, kw);
        }
        let fo = func.cast::<ffi::PyFunctionObject>();
        if !DEPS_COPY.ok() || (*fo).func_code != st.deepcopy_code || (*fo).func_defaults != st.deepcopy_defaults {
            C_DEEPCOPY.fallback_streak(Reason::Deps);
            return original(func, args, nargsf, kw);
        }
        if !accel::tools_quiet(&TOOLS_DEEPCOPY, func) {
            C_DEEPCOPY.fallback_streak(Reason::Tools);
            return original(func, args, nargsf, kw);
        }
        let e = deep::Engine::new(st, memo, atomic_fast(st), recursion_remaining());
        match e.run(*args) {
            Some(y) => {
                C_DEEPCOPY.hit_streak();
                y
            }
            None => {
                // User code (or an error, or the recursion limit) ahead: CPython's own call,
                // from scratch, with the unchanged arguments.
                C_DEEPCOPY.fallback_streak(Reason::Deopt);
                original(func, args, nargsf, kw)
            }
        }
    }
}

/// Kernel for `copy.copy(x)`.
unsafe extern "C" fn vc_copy(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let st = st();
        if !kw.is_null() || nargs(nargsf) != 1 {
            C_COPY.fallback_streak(Reason::Args);
            return original(func, args, nargsf, kw);
        }
        let fo = func.cast::<ffi::PyFunctionObject>();
        if !DEPS_COPY.ok() || (*fo).func_code != st.copy_code || !(*fo).func_defaults.is_null() {
            C_COPY.fallback_streak(Reason::Deps);
            return original(func, args, nargsf, kw);
        }
        if !accel::tools_quiet(&TOOLS_COPY, func) {
            C_COPY.fallback_streak(Reason::Tools);
            return original(func, args, nargsf, kw);
        }
        match shallow::copy(st, *args, copy_sets_fast(st), recursion_remaining()) {
            Some(y) => {
                C_COPY.hit_streak();
                y
            }
            None => {
                C_COPY.fallback_streak(Reason::Deopt);
                original(func, args, nargsf, kw)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------

/// Canonical code fingerprints (`rustique._rustique._accel.fingerprint`) of every Python
/// function the kernels replace or emulate.
mod fp {
    use crate::accel::Fingerprints;
    pub const DEEPCOPY: Fingerprints = &[(3, 14, "a8e15d1610cfc3b7"), (3, 15, "307dc8f19f9b5db4")];
    pub const COPY: Fingerprints = &[(3, 14, "14c52708c4954d3f"), (3, 15, "2eea511e3b3c11d5")];
    pub const DEEPCOPY_LIST: Fingerprints = &[(3, 14, "b7639b0a182a4223"), (3, 15, "0ebca81111a34cee")];
    pub const DEEPCOPY_TUPLE: Fingerprints = &[(3, 14, "21bef80950ffc7aa"), (3, 15, "7334dda0989c80a9")];
    pub const DEEPCOPY_DICT: Fingerprints = &[(3, 14, "8ae3d261d6c844e1"), (3, 15, "355063d5f74669c2")];
    pub const DEEPCOPY_FROZENDICT: Fingerprints = &[(3, 15, "7e2fe4b5411f95f3")];
    pub const DEEPCOPY_METHOD: Fingerprints = &[(3, 14, "512dee8b68330b17"), (3, 15, "5a261da2c95b4fce")];
    pub const KEEP_ALIVE: Fingerprints = &[(3, 14, "e3a4068b84c7e5ac"), (3, 15, "51a2113d512c2be0")];
    pub const RECONSTRUCT: Fingerprints = &[(3, 14, "eef7983ee166e632"), (3, 15, "cfebe2bd44245931")];
    pub const SLOTNAMES: Fingerprints = &[(3, 14, "fa693b043c4d7ca4"), (3, 15, "d2fbfedc0f8bcb0d")];
    pub const NEWOBJ: Fingerprints = &[(3, 14, "e9a487187e3c3309"), (3, 15, "dc09633e5824ebb5")];
}

/// The CPython 3.14 type sets in copy.py are mutable sets: a snapshot of the hash table.
struct SetSnap {
    fill: isize,
    used: isize,
    mask: isize,
    table: usize,
    entries: Vec<[usize; 2]>,
}

#[repr(C)]
struct RawSet {
    ob_base: ffi::PyObject,
    fill: isize,
    used: isize,
    mask: isize,
    table: *mut [usize; 2],
}

impl SetSnap {
    unsafe fn take(set: O) -> SetSnap {
        // SAFETY: a live set (public PySetObject layout).
        unsafe {
            with_cs(set, || {
                let s = &*set.cast::<RawSet>();
                SetSnap {
                    fill: s.fill,
                    used: s.used,
                    mask: s.mask,
                    table: s.table as usize,
                    entries: std::slice::from_raw_parts(s.table, (s.mask + 1) as usize).to_vec(),
                }
            })
        }
    }

    #[inline(always)]
    unsafe fn matches(&self, set: O) -> bool {
        // SAFETY: as take.
        unsafe {
            with_cs(set, || {
                let s = &*set.cast::<RawSet>();
                s.fill == self.fill
                    && s.used == self.used
                    && s.mask == self.mask
                    && s.table as usize == self.table
                    && std::slice::from_raw_parts(s.table, (s.mask + 1) as usize) == &self.entries[..]
            })
        }
    }
}

fn types(list: &[*mut ffi::PyTypeObject]) -> Vec<*mut ffi::PyTypeObject> {
    list.to_vec()
}

/// `copy._atomic_types` in the kernels' order (most frequent first).
fn atomic_types() -> Vec<*mut ffi::PyTypeObject> {
    // SAFETY: static type objects and singletons.
    unsafe {
        types(&[
            &raw mut ffi::PyUnicode_Type,
            &raw mut ffi::PyLong_Type,
            ty(ffi::Py_None()),
            &raw mut ffi::PyFloat_Type,
            &raw mut ffi::PyBool_Type,
            &raw mut ffi::PyComplex_Type,
            &raw mut ffi::PyBytes_Type,
            &raw mut ffi::PyType_Type,
            &raw mut ffi::PyRange_Type,
            &raw mut ffi::PyCode_Type,
            &raw mut ffi::PyCFunction_Type,
            &raw mut ffi::PyFunction_Type,
            &raw mut ffi::_PyWeakref_RefType,
            &raw mut ffi::PyProperty_Type,
            &raw mut ffi::PyEllipsis_Type,
            ty(ffi::Py_NotImplemented()),
        ])
    }
}

fn copy_atomic_types(minor: u8) -> Vec<*mut ffi::PyTypeObject> {
    // SAFETY: static type objects and singletons.
    unsafe {
        let mut v = types(&[
            ty(ffi::Py_None()),
            &raw mut ffi::PyLong_Type,
            &raw mut ffi::PyFloat_Type,
            &raw mut ffi::PyBool_Type,
            &raw mut ffi::PyComplex_Type,
            &raw mut ffi::PyUnicode_Type,
            &raw mut ffi::PyTuple_Type,
            &raw mut ffi::PyBytes_Type,
            &raw mut ffi::PyFrozenSet_Type,
            &raw mut ffi::PyType_Type,
            &raw mut ffi::PyRange_Type,
            &raw mut ffi::PySlice_Type,
            &raw mut ffi::PyProperty_Type,
            &raw mut ffi::PyCFunction_Type,
            &raw mut ffi::PyEllipsis_Type,
            ty(ffi::Py_NotImplemented()),
            &raw mut ffi::PyFunction_Type,
            &raw mut ffi::PyCode_Type,
            &raw mut ffi::_PyWeakref_RefType,
            &raw mut ffi::PySuper_Type,
        ]);
        if minor >= 15 {
            v.push(frozendict_type());
        }
        v
    }
}

#[cfg(Py_3_15)]
fn frozendict_type() -> *mut ffi::PyTypeObject {
    &raw mut ffi::PyFrozenDict_Type
}

#[cfg(not(Py_3_15))]
fn frozendict_type() -> *mut ffi::PyTypeObject {
    std::ptr::null_mut()
}

/// Does `set` hold exactly `want` (types: hashing and comparing them runs no Python code)?
fn same_type_set(set: &Bound<'_, PyAny>, want: &[*mut ffi::PyTypeObject]) -> PyResult<bool> {
    let n = set.len()?;
    if n != want.len() {
        return Ok(false);
    }
    for &t in want {
        // SAFETY: a live set and a static type.
        if unsafe { ffi::PySet_Contains(set.as_ptr(), t.cast()) } != 1 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn intern(py: Python<'_>, s: &str) -> O {
    pyo3::types::PyString::intern(py, s).into_ptr()
}

/// The offset of `py_recursion_remaining` in `PyThreadState` (public cpython/pystate.h
/// layout: 3.15 added `gil_requested`), checked against `sys.getrecursionlimit()`.
pub(crate) fn tstate_offset(cx: &Prepare<'_>, minor: u8) -> Result<usize, Refusal> {
    let off = if minor >= 15 { 52 } else { 48 };
    if REM_OFF.load(Relaxed) == off {
        return Ok(off);
    }
    let sys = cx.loaded_module("sys")?;
    let limit: c_int = sys.getattr("getrecursionlimit")?.call0()?.extract()?;
    // SAFETY: attached thread; reads within PyThreadState.
    let (rem, lim) = unsafe {
        let ts = ffi::PyThreadState_Get().cast::<u8>();
        (*ts.add(off).cast::<c_int>(), *ts.add(off + 4).cast::<c_int>())
    };
    if lim != limit || rem <= 0 || rem > limit {
        return Err(crate::refuse!("PyThreadState layout differs (limit {lim} vs {limit}, remaining {rem})"));
    }
    REM_OFF.store(off, Relaxed);
    Ok(off)
}

/// `d[k]` of an exact dict with a str key (no Python code runs).
fn dget<'py>(d: &Bound<'py, PyDict>, k: &str) -> Result<Bound<'py, PyAny>, Refusal> {
    d.get_item(k)?.ok_or_else(|| crate::refuse!("{k} missing"))
}

/// (function, code, defaults) of a pure-Python function, read from the struct (strong refs).
fn func3(o: &Bound<'_, PyAny>) -> PyResult<(O, O, O)> {
    if o.cast::<pyo3::types::PyFunction>().is_err() {
        return Err(pyo3::exceptions::PyTypeError::new_err("not a function"));
    }
    // SAFETY: an exact function object; its code/defaults are kept alive by the strong refs.
    unsafe {
        let f = o.as_ptr().cast::<ffi::PyFunctionObject>();
        let d = (*f).func_defaults;
        Ok((incref(o.as_ptr()), incref((*f).func_code), if d.is_null() { d } else { incref(d) }))
    }
}

/// [`St`] if it was built from this very copy module and nothing it validated changed.
fn current_state(cd: &Bound<'_, PyDict>, rd: &Bound<'_, PyDict>) -> Result<Option<&'static St>, Refusal> {
    let p = ST.load(Relaxed);
    if p.is_null() {
        return Ok(None);
    }
    // SAFETY: never freed.
    let st: &'static St = unsafe { &*p };
    if st.copy_dict != cd.as_ptr() {
        return Ok(None);
    }
    let names = [
        "_deepcopy_list",
        "_deepcopy_tuple",
        "_deepcopy_dict",
        "_deepcopy_frozendict",
        "_deepcopy_method",
        "_keep_alive",
        "_reconstruct",
    ];
    for (i, n) in names.iter().enumerate() {
        let (f, code, _) = st.helpers[i];
        if f.is_null() {
            continue;
        }
        let cur = dget(cd, n)?;
        // SAFETY: a function object (identity-checked).
        if cur.as_ptr() != f || unsafe { (*f.cast::<ffi::PyFunctionObject>()).func_code } != code {
            return Ok(None);
        }
    }
    for (i, n) in [(H_NEWOBJ, "__newobj__"), (H_SLOTNAMES, "_slotnames")] {
        let (f, code, _) = st.helpers[i];
        let cur = dget(rd, n)?;
        // SAFETY: as above.
        if cur.as_ptr() != f || unsafe { (*f.cast::<ffi::PyFunctionObject>()).func_code } != code {
            return Ok(None);
        }
    }
    let same = |k: &str, want: O| -> Result<bool, Refusal> { Ok(dget(cd, k)?.as_ptr() == want) };
    // SAFETY: function objects.
    let code_of = |f: &Bound<'_, PyAny>| unsafe { (*f.as_ptr().cast::<ffi::PyFunctionObject>()).func_code };
    let ok = same("deepcopy", st.deepcopy_fn)?
        && code_of(&dget(cd, "deepcopy")?) == st.deepcopy_code
        && code_of(&dget(cd, "copy")?) == st.copy_code
        && same("_atomic_types", st.atomic)?
        && same("_copy_atomic_types", st.copy_atomic)?
        && same("_copy_builtin_containers", st.copy_containers)?
        && same("_deepcopy_dispatch", st.dispatch)?
        && same("dispatch_table", st.dispatch_table)?;
    Ok(if ok { Some(st) } else { None })
}

/// Validate the copy module and build [`St`] (shared by both targets and by the
/// dataclasses group); refuses on any difference from the validated sources.
fn ensure_state(py: Python<'_>, cx: &mut Prepare<'_>, copy: &Bound<'_, PyModule>) -> Result<&'static St, Refusal> {
    let (_, minor) = cx.version();
    tstate_offset(cx, minor)?;
    let copyreg = cx.loaded_module("copyreg")?;
    let cd = copy.dict();
    let rd = copyreg.dict();
    let get = |k: &str| dget(&cd, k);
    // The group's other target already validated this module: same functions and code
    // objects (the fingerprinted ones), same sets.
    if !cx.record
        && let Some(st) = current_state(&cd, &rd)?
    {
        return Ok(st);
    }
    let deepcopy = get("deepcopy")?;
    let copy_fn = get("copy")?;
    cx.fingerprint_code(&deepcopy, fp::DEEPCOPY)?;
    cx.fingerprint_code(&copy_fn, fp::COPY)?;
    let h_list = get("_deepcopy_list")?;
    let h_tuple = get("_deepcopy_tuple")?;
    let h_dict = get("_deepcopy_dict")?;
    let h_method = get("_deepcopy_method")?;
    let keep_alive = get("_keep_alive")?;
    let reconstruct = get("_reconstruct")?;
    let newobj = dget(&rd, "__newobj__")?;
    let slotnames = dget(&rd, "_slotnames")?;
    cx.fingerprint_code(&slotnames, fp::SLOTNAMES)?;
    cx.fingerprint_code(&h_list, fp::DEEPCOPY_LIST)?;
    cx.fingerprint_code(&h_tuple, fp::DEEPCOPY_TUPLE)?;
    cx.fingerprint_code(&h_dict, fp::DEEPCOPY_DICT)?;
    cx.fingerprint_code(&h_method, fp::DEEPCOPY_METHOD)?;
    cx.fingerprint_code(&keep_alive, fp::KEEP_ALIVE)?;
    cx.fingerprint_code(&reconstruct, fp::RECONSTRUCT)?;
    cx.fingerprint_code(&newobj, fp::NEWOBJ)?;
    let h_frozendict = if minor >= 15 {
        let f = get("_deepcopy_frozendict")?;
        cx.fingerprint_code(&f, fp::DEEPCOPY_FROZENDICT)?;
        Some(f)
    } else {
        None
    };
    // The type sets copy.py consults must hold exactly the types the kernels assume.
    let atomic = atomic_types();
    let copy_atomic = copy_atomic_types(minor);
    let containers = vec![
        &raw mut ffi::PyList_Type,
        &raw mut ffi::PyDict_Type,
        &raw mut ffi::PySet_Type,
        &raw mut ffi::PyByteArray_Type,
    ];
    if !same_type_set(&get("_atomic_types")?, &atomic)?
        || !same_type_set(&get("_copy_atomic_types")?, &copy_atomic)?
        || !same_type_set(&get("_copy_builtin_containers")?, &containers)?
    {
        return Err("copy's type sets were modified".into());
    }
    if !get("dispatch_table")?.is(&dget(&rd, "dispatch_table")?) {
        return Err("copy.dispatch_table is not copyreg.dispatch_table".into());
    }
    let nul = std::ptr::null_mut();
    let helpers = [
        func3(&h_list)?,
        func3(&h_tuple)?,
        func3(&h_dict)?,
        match &h_frozendict {
            Some(f) => func3(f)?,
            None => (nul, nul, nul),
        },
        func3(&h_method)?,
        func3(&keep_alive)?,
        func3(&reconstruct)?,
        func3(&newobj)?,
        func3(&slotnames)?,
    ];
    let (_, deepcopy_code, deepcopy_defaults) = func3(&deepcopy)?;
    let (_, copy_code, _) = func3(&copy_fn)?;
    let atomic_set = get("_atomic_types")?;
    let get_method = cx.symbol(c"_PyObject_GetMethod", &GET_METHOD)?;
    let object_attr = |name: &str| -> O {
        let n = pyo3::types::PyString::intern(py, name);
        // SAFETY: a ready static type; borrowed result kept alive by `object` (made strong).
        unsafe {
            let o = _PyType_Lookup(&raw mut ffi::PyBaseObject_Type, n.as_ptr());
            if o.is_null() { o } else { incref(o) }
        }
    };
    let mut st = St {
        minor,
        copy_dict: cd.clone().into_ptr(),
        deepcopy_fn: deepcopy.into_ptr(),
        deepcopy_code,
        deepcopy_defaults,
        copy_code,
        atomic: atomic_set.into_ptr(),
        atomic_snap: None,
        copy_atomic_snap: None,
        copy_containers_snap: None,
        copy_atomic: get("_copy_atomic_types")?.into_ptr(),
        copy_containers: get("_copy_builtin_containers")?.into_ptr(),
        copy_atomic_types: copy_atomic,
        copy_container_types: containers,
        dispatch: get("_deepcopy_dispatch")?.into_ptr(),
        dispatch_table: get("dispatch_table")?.into_ptr(),
        h_list: helpers[H_LIST].0,
        h_tuple: helpers[H_TUPLE].0,
        h_dict: helpers[H_DICT].0,
        h_frozendict: helpers[H_FROZENDICT].0,
        h_method: helpers[H_METHOD].0,
        helpers,
        newobj: helpers[H_NEWOBJ].0,
        object_new: object_attr("__new__"),
        obj_reduce_ex: object_attr("__reduce_ex__"),
        obj_reduce: object_attr("__reduce__"),
        obj_getstate: object_attr("__getstate__"),
        get_method,
        get_method_addr: get_method as usize,
        s_getstate: intern(py, "__getstate__"),
        s_getnewargs: intern(py, "__getnewargs__"),
        s_getnewargs_ex: intern(py, "__getnewargs_ex__"),
        s_slotnames: intern(py, "__slotnames__"),
        s_del: intern(py, "__del__"),
        atomic_types: atomic.try_into().map_err(|_| Refusal::new("atomic types"))?,
        // SAFETY: static type objects.
        method_type: &raw mut PyMethod_Type,
        frozendict_type: frozendict_type(),
        // SAFETY: static type object read.
        s_deepcopy: intern(py, "__deepcopy__"),
        s_copy: intern(py, "__copy__"),
        s_copy_method: intern(py, "copy"),
        s_memo: intern(py, "memo"),
        s_reduce_ex: intern(py, "__reduce_ex__"),
        s_reduce: intern(py, "__reduce__"),
        s_setstate: intern(py, "__setstate__"),
        s_dict: intern(py, "__dict__"),
        s_new: intern(py, "__new__"),
    };
    if minor < 15 {
        // SAFETY: a live set.
        st.atomic_snap = Some(unsafe { SetSnap::take(st.atomic) });
        // SAFETY: live sets.
        st.copy_atomic_snap = Some(unsafe { SetSnap::take(st.copy_atomic) });
        // SAFETY: live sets.
        st.copy_containers_snap = Some(unsafe { SetSnap::take(st.copy_containers) });
    }
    let st: &'static St = Box::leak(Box::new(st));
    ST.store((st as *const St).cast_mut(), Relaxed);
    Ok(st)
}

/// Pin what copy.py reads at call time (shared flag of the group): copy's globals as one
/// snapshot (every binding the kernels validated, and the absence of any global that would
/// shadow a builtin), the builtins it uses, `copyreg.__newobj__`/`_slotnames`, and
/// `sys.modules['copyreg']` (what `object.__reduce_ex__` imports).
fn pin_deps(cx: &mut Prepare<'_>, copy: &Bound<'_, PyModule>) -> Result<(), Refusal> {
    let (_, minor) = cx.version();
    let g = copy.dict();
    cx.pin_dict(&g, &DEPS_COPY)?;
    let builtins = cx.builtins_of(&g)?;
    let mut names =
        vec!["type", "id", "issubclass", "getattr", "isinstance", "hasattr", "len", "setattr", "zip", "tuple"];
    if minor >= 15 {
        names.push("frozendict");
    }
    for k in names {
        cx.pin(&builtins, k, &DEPS_COPY)?;
    }
    let copyreg = cx.loaded_module("copyreg")?;
    cx.pin(&copyreg.dict(), "__newobj__", &DEPS_COPY)?;
    cx.pin(&copyreg.dict(), "_slotnames", &DEPS_COPY)?;
    let modules = cx.loaded_module("sys")?.dict();
    let modules = cx.get(&modules, "modules")?.cast_into::<PyDict>().map_err(PyErr::from)?;
    cx.pin(&modules, "copyreg", &DEPS_COPY)?;
    Ok(())
}

fn prepare_deepcopy(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let copy = cx.module()?.clone();
    ensure_state(py, cx, &copy)?;
    pin_deps(cx, &copy)?;
    let f = cx.module_attr("deepcopy")?;
    cx.vectorcall(&f, vc_deepcopy, &accel::FUNCTION_VECTORCALL)
}

fn prepare_copy(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let copy = cx.module()?.clone();
    ensure_state(py, cx, &copy)?;
    pin_deps(cx, &copy)?;
    let f = cx.module_attr("copy")?;
    cx.vectorcall(&f, vc_copy, &accel::FUNCTION_VECTORCALL)
}

/// `tstate->py_recursion_remaining` as the kernels see it (tests).
#[pyfunction]
fn copy_recursion_remaining() -> PyResult<c_int> {
    let p = ST.load(Relaxed);
    if p.is_null() {
        return Err(pyo3::exceptions::PyRuntimeError::new_err("copy group never prepared"));
    }
    // SAFETY: attached thread.
    Ok(unsafe { recursion_remaining() })
}

/// Python-visible helpers (on `rustique._rustique._accel`).
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(copy_recursion_remaining, m)?)
}
