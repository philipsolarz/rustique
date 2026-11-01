//! Group `copy`: `copy.deepcopy` and `copy.copy`; and (in [`dc`]) group `dataclasses`:
//! `dataclasses.asdict`, `astuple`, `replace`. All are vectorcall kernels on the existing
//! function objects (contract §3.7). See `docs/targets/copy.md`.

pub mod dc;
pub mod deep;
pub mod memo;
pub mod rt;
pub mod shallow;

use std::ffi::c_int;
use std::sync::atomic::Ordering::Relaxed;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

use crate::accel::{self, Counters, DepFlag, Prepare, Reason, Refusal, TargetSpec, Tools};
use rt::*;

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

pub static C_DEEPCOPY: Counters = Counters::new();
pub static C_COPY: Counters = Counters::new();
/// The module globals and builtins `copy.py` reads at call time are the objects validated.
pub static DEPS_COPY: DepFlag = DepFlag::new();
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

#[inline(always)]
pub unsafe fn original(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: the saved `_PyFunction_Vectorcall`.
    unsafe { accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kw) }
}

#[inline(always)]
fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

/// Is the set `deepcopy` checks atomic types against intact (3.14's is a mutable set)?
#[inline(always)]
pub unsafe fn atomic_fast(st: &St) -> bool {
    match &st.atomic_snap {
        None => true,
        // SAFETY: live set.
        Some(s) => unsafe { s.matches(st.atomic) },
    }
}

/// Is the `copy.deepcopy` kernel installed and its guards satisfied, so that callers inside
/// Rustique (dataclasses) may run the engine directly instead of calling the function?
#[inline(always)]
pub unsafe fn deepcopy_direct(st: &St, f: O) -> bool {
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
pub unsafe extern "C" fn vc_deepcopy(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let st = st();
        let n = nargs(nargsf);
        let memo = if kw.is_null() {
            match n {
                1 => std::ptr::null_mut(),
                2 => *args.add(1),
                _ => {
                    C_DEEPCOPY.fallback(Reason::Args);
                    return original(func, args, nargsf, kw);
                }
            }
        } else if n == 1 && ffi::PyTuple_GET_SIZE(kw) == 1 && ffi::PyTuple_GET_ITEM(kw, 0) == st.s_memo {
            *args.add(1)
        } else {
            C_DEEPCOPY.fallback(Reason::Args);
            return original(func, args, nargsf, kw);
        };
        let memo = if memo == ffi::Py_None() { std::ptr::null_mut() } else { memo };
        if !memo.is_null() && ty(memo) != &raw mut ffi::PyDict_Type {
            C_DEEPCOPY.fallback(Reason::Args);
            return original(func, args, nargsf, kw);
        }
        let fo = func.cast::<ffi::PyFunctionObject>();
        if !DEPS_COPY.ok() || (*fo).func_code != st.deepcopy_code || (*fo).func_defaults != st.deepcopy_defaults {
            C_DEEPCOPY.fallback(Reason::Deps);
            return original(func, args, nargsf, kw);
        }
        if !accel::tools_quiet(&TOOLS_DEEPCOPY, func) {
            C_DEEPCOPY.fallback(Reason::Tools);
            return original(func, args, nargsf, kw);
        }
        C_DEEPCOPY.hit();
        let mut e = deep::Engine::new(st, memo, atomic_fast(st));
        e.run(*args).unwrap_or(std::ptr::null_mut())
    }
}

/// Kernel for `copy.copy(x)`.
unsafe extern "C" fn vc_copy(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let st = st();
        if !kw.is_null() || nargs(nargsf) != 1 {
            C_COPY.fallback(Reason::Args);
            return original(func, args, nargsf, kw);
        }
        let fo = func.cast::<ffi::PyFunctionObject>();
        if !DEPS_COPY.ok() || (*fo).func_code != st.copy_code || !(*fo).func_defaults.is_null() {
            C_COPY.fallback(Reason::Deps);
            return original(func, args, nargsf, kw);
        }
        if !accel::tools_quiet(&TOOLS_COPY, func) {
            C_COPY.fallback(Reason::Tools);
            return original(func, args, nargsf, kw);
        }
        C_COPY.hit();
        shallow::copy(st, *args, st.minor >= 15).unwrap_or(std::ptr::null_mut())
    }
}

// ---------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------

/// Canonical code fingerprints (`rustique._rustique._accel.fingerprint`) of every Python
/// function the kernels replace or emulate.
pub mod fp {
    use crate::accel::Fingerprints;
    pub const DEEPCOPY: Fingerprints = &[(3, 14, "a287496a0d76e2fa"), (3, 15, "d19ec63249fba4bc")];
    pub const COPY: Fingerprints = &[(3, 14, "e72c3343e74a17a5"), (3, 15, "08cc44a96c8b6e95")];
    pub const DEEPCOPY_LIST: Fingerprints = &[(3, 14, "bd3cf7284db8584f"), (3, 15, "cf55f78857788db7")];
    pub const DEEPCOPY_TUPLE: Fingerprints = &[(3, 14, "45a481d94ebaabaa"), (3, 15, "5d98059dc4051884")];
    pub const DEEPCOPY_DICT: Fingerprints = &[(3, 14, "637e9e6fc42f392b"), (3, 15, "9769f589217192e9")];
    pub const DEEPCOPY_FROZENDICT: Fingerprints = &[(3, 15, "cc960aba60739226")];
    pub const DEEPCOPY_METHOD: Fingerprints = &[(3, 14, "f8003d8e1e6a1386"), (3, 15, "a85c9c3946dab55a")];
    pub const KEEP_ALIVE: Fingerprints = &[(3, 14, "421fc8f79248786d"), (3, 15, "968491e533d8ab2b")];
    pub const RECONSTRUCT: Fingerprints = &[(3, 14, "389ff8af1279cb19"), (3, 15, "1365ac6436d0273d")];
    pub const NEWOBJ: Fingerprints = &[(3, 14, "59d84e3426e38a01"), (3, 15, "f3dc1dde05b828a4")];
}

/// The CPython 3.14 type sets in copy.py are mutable sets: a snapshot of the hash table.
pub struct SetSnap {
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
    pub unsafe fn matches(&self, set: O) -> bool {
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

/// `(co_code, co_flags)` of `src`'s first function, compiled (no code runs).
fn ret_self_code(py: Python<'_>, src: &std::ffi::CStr) -> PyResult<(O, c_int)> {
    {
        // SAFETY: compiles only.
        let m = unsafe { ffi::Py_CompileString(src.as_ptr(), c"<rustique>".as_ptr(), ffi::Py_file_input) };
        // SAFETY: new reference or error.
        let m = unsafe { Bound::from_owned_ptr_or_err(py, m)? };
        for c in m.getattr("co_consts")?.try_iter()? {
            let c = c?;
            if c.get_type().name()? == "code" {
                return Ok((c.getattr("co_code")?.into_ptr(), c.getattr("co_flags")?.extract()?));
            }
        }
        Err(pyo3::exceptions::PyRuntimeError::new_err("no code object"))
    }
}

fn intern(py: Python<'_>, s: &str) -> O {
    pyo3::types::PyString::intern(py, s).into_ptr()
}

/// The offset of `py_recursion_remaining` in `PyThreadState` (public cpython/pystate.h
/// layout: 3.15 added `gil_requested`), checked against `sys.getrecursionlimit()`.
fn tstate_offset(cx: &Prepare<'_>, minor: u8) -> Result<usize, Refusal> {
    let off = if minor >= 15 { 52 } else { 48 };
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
    Ok(off)
}

/// `d[k]` of an exact dict with a str key (no Python code runs).
pub fn dget<'py>(d: &Bound<'py, PyDict>, k: &str) -> Result<Bound<'py, PyAny>, Refusal> {
    d.get_item(k)?.ok_or_else(|| crate::refuse!("{k} missing"))
}

fn func3(o: &Bound<'_, PyAny>) -> PyResult<(O, O, O)> {
    let code = o.getattr("__code__")?;
    let defaults = o.getattr("__defaults__")?;
    Ok((
        o.clone().into_ptr(),
        code.into_ptr(),
        if defaults.is_none() { std::ptr::null_mut() } else { defaults.into_ptr() },
    ))
}

/// Validate the copy module and build [`St`] (shared by both targets and by the
/// dataclasses group); refuses on any difference from the validated sources.
pub fn ensure_state(py: Python<'_>, cx: &mut Prepare<'_>, copy: &Bound<'_, PyModule>) -> Result<&'static St, Refusal> {
    let (_, minor) = cx.version();
    let rem_off = tstate_offset(cx, minor)?;
    let copyreg = cx.loaded_module("copyreg")?;
    let cd = copy.dict();
    let rd = copyreg.dict();
    let get = |k: &str| dget(&cd, k);
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
    ];
    let (_, deepcopy_code, deepcopy_defaults) = func3(&deepcopy)?;
    let (_, copy_code, _) = func3(&copy_fn)?;
    let atomic_set = get("_atomic_types")?;
    let mut st = St {
        minor,
        copy_dict: cd.clone().into_ptr(),
        deepcopy_fn: deepcopy.into_ptr(),
        deepcopy_code,
        deepcopy_defaults,
        copy_fn: copy_fn.into_ptr(),
        copy_code,
        atomic: atomic_set.into_ptr(),
        atomic_snap: None,
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
        keep_alive: helpers[H_KEEP_ALIVE].0,
        reconstruct: helpers[H_RECONSTRUCT].0,
        helpers,
        newobj: helpers[H_NEWOBJ].0,
        newobj_code: helpers[H_NEWOBJ].1,
        object_new: nul,
        atomic_types: atomic.try_into().map_err(|_| Refusal::new("atomic types"))?,
        // SAFETY: static type objects.
        method_type: &raw mut PyMethod_Type,
        frozendict_type: frozendict_type(),
        // SAFETY: static type object read.
        default_type_hash: unsafe { (*(&raw mut ffi::PyType_Type)).tp_hash.map_or(0, |f| f as usize) },
        s_deepcopy: intern(py, "__deepcopy__"),
        s_copy: intern(py, "__copy__"),
        s_copy_method: intern(py, "copy"),
        s_memo: intern(py, "memo"),
        s_reduce_ex: intern(py, "__reduce_ex__"),
        s_reduce: intern(py, "__reduce__"),
        s_setstate: intern(py, "__setstate__"),
        s_dict: intern(py, "__dict__"),
        s_update: intern(py, "update"),
        s_items: intern(py, "items"),
        s_append: intern(py, "append"),
        s_new: intern(py, "__new__"),
        s_error: intern(py, "Error"),
        // SAFETY: attached thread.
        int4: unsafe { ffi::PyLong_FromLong(4) },
        ret_self2: ret_self_code(py, c"def f(self, memo):\n    return self\n")?,
        ret_self1: ret_self_code(py, c"def f(self):\n    return self\n")?,
        rem_off,
    };
    if minor < 15 {
        // SAFETY: a live set.
        st.atomic_snap = Some(unsafe { SetSnap::take(st.atomic) });
    }
    let st: &'static St = Box::leak(Box::new(st));
    ST.store((st as *const St).cast_mut(), Relaxed);
    Ok(st)
}

/// Pin the globals and builtins copy.py reads at call time (shared flag of the group).
fn pin_deps(cx: &mut Prepare<'_>, copy: &Bound<'_, PyModule>) -> Result<(), Refusal> {
    let (_, minor) = cx.version();
    let g = copy.dict();
    for k in [
        "deepcopy",
        "_reconstruct",
        "_keep_alive",
        "_atomic_types",
        "_deepcopy_dispatch",
        "dispatch_table",
        "_copy_atomic_types",
        "_copy_builtin_containers",
    ] {
        cx.pin(&g, k, &DEPS_COPY)?;
    }
    let mut names =
        vec!["type", "id", "issubclass", "getattr", "isinstance", "hasattr", "len", "setattr", "zip", "tuple"];
    if minor >= 15 {
        names.push("frozendict");
    }
    for k in names {
        cx.pin_global(&g, k, &DEPS_COPY)?;
    }
    let copyreg = cx.loaded_module("copyreg")?;
    cx.pin(&copyreg.dict(), "__newobj__", &DEPS_COPY)?;
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
    cx.pin(&copy.dict(), "copy", &DEPS_COPY)?;
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
    Ok(unsafe { Depth::new(&*p) }.remaining())
}

/// Python-visible helpers (on `rustique._rustique._accel`).
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(copy_recursion_remaining, m)?)
}
