//! Target `re._compiler.compile`: CPython's pure-Python regular-expression compiler,
//! reached on `re._compile` cache misses (`re.compile`, `re.match`, ...). The kernel parses
//! and compiles the pattern in Rust (`rustique_core::re`) and calls CPython's own
//! `_sre.compile` with exactly the arguments `re._compiler.compile` would pass.

use pyo3::ffi;
use pyo3::prelude::*;

use rustique_core::re::{self as core, Compiled};

use super::state::{self, O};
use super::{COMPILE_COUNTERS, COMPILE_TOOLS, DEPS_COMPILE, TARGET_COMPILE};
use crate::accel::{self, Reason};

/// The code units of an exact `str` (PEP 393 buffer read directly, never the UTF-8 cache).
///
/// # Safety
/// `s` a live exact str.
pub(crate) unsafe fn str_units(s: O) -> Vec<u32> {
    // SAFETY: per the caller.
    unsafe {
        let n = ffi::PyUnicode_GET_LENGTH(s) as usize;
        let data = ffi::PyUnicode_DATA(s);
        match ffi::PyUnicode_KIND(s) {
            ffi::PyUnicode_1BYTE_KIND => {
                std::slice::from_raw_parts(data.cast::<u8>(), n).iter().map(|&c| c as u32).collect()
            }
            ffi::PyUnicode_2BYTE_KIND => {
                std::slice::from_raw_parts(data.cast::<u16>(), n).iter().map(|&c| c as u32).collect()
            }
            _ => std::slice::from_raw_parts(data.cast::<u32>(), n).to_vec(),
        }
    }
}

/// The bytes of an exact `bytes`, as code units (the parser's latin-1 decoding).
///
/// # Safety
/// `b` a live exact bytes object.
pub(crate) unsafe fn bytes_units(b: O) -> Vec<u32> {
    // SAFETY: per the caller.
    unsafe {
        let n = ffi::Py_SIZE(b) as usize;
        std::slice::from_raw_parts(ffi::PyBytes_AS_STRING(b).cast::<u8>(), n).iter().map(|&c| c as u32).collect()
    }
}

/// The pattern's code units and type, for an exact `str` or `bytes`.
///
/// # Safety
/// Attached thread; `p` live.
pub(crate) unsafe fn pattern_units(p: O) -> Option<(Vec<u32>, bool)> {
    // SAFETY: exact type checks before the buffer reads.
    unsafe {
        let t = ffi::Py_TYPE(p);
        if t == &raw mut ffi::PyUnicode_Type {
            Some((str_units(p), true))
        } else if t == &raw mut ffi::PyBytes_Type {
            Some((bytes_units(p), false))
        } else {
            None
        }
    }
}

/// The flags argument as the port takes it: an exact `int` in `[0, 2**31)`.
///
/// # Safety
/// Attached thread; `f` live.
unsafe fn flags_of(f: O) -> Option<u32> {
    // SAFETY: exact int check first.
    unsafe {
        if ffi::PyLong_CheckExact(f) == 0 {
            return None;
        }
        let mut overflow = 0;
        let v = ffi::PyLong_AsLongAndOverflow(f, &mut overflow);
        if overflow != 0 || !(0..=i32::MAX as std::ffi::c_long).contains(&v) {
            if v == -1 {
                ffi::PyErr_Clear();
            }
            return None;
        }
        Some(v as u32)
    }
}

/// `_sre.compile`'s arguments as Python objects, and the call. NULL (no error set) if any
/// step fails: the caller falls back.
///
/// # Safety
/// Attached thread; `pattern` the live pattern object.
unsafe fn call_sre_compile(sre_compile: O, pattern: O, c: &Compiled) -> O {
    // SAFETY: new references checked and released once; the list is filled completely
    // before anyone sees it.
    unsafe {
        let args = build_args(pattern, c);
        let Some(args) = args else {
            ffi::PyErr_Clear();
            return std::ptr::null_mut();
        };
        let r = ffi::PyObject_Vectorcall(sre_compile, args.as_ptr(), 6, std::ptr::null_mut());
        for &a in &args[1..] {
            ffi::Py_DECREF(a);
        }
        if r.is_null() {
            // `_sre.compile` rejected it (cannot happen for code CPython's compiler would
            // emit; nothing observable happened): let CPython's compiler run and raise.
            ffi::PyErr_Clear();
        }
        r
    }
}

/// `[pattern, flags, code, groups, groupindex, indexgroup]` (new references except
/// `pattern`), or `None` with an error set.
///
/// # Safety
/// Attached thread.
pub(crate) unsafe fn build_args(pattern: O, c: &Compiled) -> Option<[O; 6]> {
    // SAFETY: per the caller; every reference is owned until returned or released.
    unsafe {
        let code = ffi::PyList_New(c.code.len() as ffi::Py_ssize_t);
        if code.is_null() {
            return None;
        }
        for (i, &w) in c.code.iter().enumerate() {
            let v = ffi::PyLong_FromUnsignedLong(w as std::ffi::c_ulong);
            if v.is_null() {
                ffi::Py_DECREF(code);
                return None;
            }
            ffi::PyList_SET_ITEM(code, i as ffi::Py_ssize_t, v);
        }
        let flags = ffi::PyLong_FromUnsignedLong(c.flags as std::ffi::c_ulong);
        let groups = ffi::PyLong_FromUnsignedLong(c.groups as std::ffi::c_ulong);
        let groupindex = ffi::PyDict_New();
        let indexgroup = ffi::PyTuple_New(c.groups as ffi::Py_ssize_t + 1);
        let release = |objs: &[O]| {
            for &o in objs {
                ffi::Py_XDECREF(o);
            }
        };
        if flags.is_null() || groups.is_null() || groupindex.is_null() || indexgroup.is_null() {
            release(&[code, flags, groups, groupindex, indexgroup]);
            return None;
        }
        for i in 0..=c.groups as ffi::Py_ssize_t {
            ffi::PyTuple_SET_ITEM(indexgroup, i, ffi::Py_NewRef(ffi::Py_None()));
        }
        for (name, gid) in &c.groupindex {
            let k = ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name.len() as ffi::Py_ssize_t);
            let v = ffi::PyLong_FromUnsignedLong(*gid as std::ffi::c_ulong);
            if k.is_null() || v.is_null() || ffi::PyDict_SetItem(groupindex, k, v) < 0 {
                ffi::Py_XDECREF(k);
                ffi::Py_XDECREF(v);
                release(&[code, flags, groups, groupindex, indexgroup]);
                return None;
            }
            ffi::Py_DECREF(v);
            // `indexgroup[i] = k`: the same name object as the dict key
            let slot = ffi::PyTuple_GET_ITEM(indexgroup, *gid as ffi::Py_ssize_t);
            ffi::PyTuple_SET_ITEM(indexgroup, *gid as ffi::Py_ssize_t, k);
            ffi::Py_DECREF(slot);
        }
        Some([pattern, flags, code, groups, groupindex, indexgroup])
    }
}

/// The port's result for `(pattern, flags)`, or `None` (decline).
///
/// # Safety
/// Attached thread; live arguments.
pub(crate) unsafe fn port(st: &state::Compile, pattern: O, flags: O) -> Option<Compiled> {
    // SAFETY: per the caller.
    unsafe {
        let fl = flags_of(flags)?;
        let (units, istext) = pattern_units(pattern)?;
        core::compile(&units, istext, fl, &st.env()).ok()
    }
}

/// The kernel: `compile(p, flags=0)`.
pub(crate) unsafe extern "C" fn k_compile(f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments as received; every guard runs before anything observable.
    unsafe {
        let n = ffi::PyVectorcall_NARGS(nargsf);
        let fallback = |why: Reason| {
            // The fallback penalty (+~250 Ir) is <2% of the cheapest CPython compile: no debit
            // (the net-benefit suspension is for targets whose fallbacks cost a share of a
            // cheap call; see docs/targets/re.md).
            COMPILE_COUNTERS.fallback(why);
            accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw)
        };
        if !DEPS_COMPILE.ok() {
            return fallback(Reason::Deps);
        }
        let Some(st) = state::compile_state() else {
            return fallback(Reason::Deps);
        };
        if !st.tags_intact() {
            return fallback(Reason::Deps);
        }
        if !accel::tools_quiet(&COMPILE_TOOLS, f) {
            return fallback(Reason::Tools);
        }
        if !kw.is_null() || !(1..=2).contains(&n) {
            return fallback(Reason::Args);
        }
        let pattern = *args;
        // `compile(p, flags=0)`: the default is read from the function at call time
        let flags = if n == 2 {
            *args.add(1)
        } else {
            let d = (*f.cast::<ffi::PyFunctionObject>()).func_defaults;
            if d.is_null() || ffi::PyTuple_GET_SIZE(d) != 1 {
                return fallback(Reason::Args);
            }
            ffi::PyTuple_GET_ITEM(d, 0)
        };
        let Some(c) = port(st, pattern, flags) else {
            return fallback(Reason::Args);
        };
        let r = call_sre_compile(st.sre_compile, pattern, &c);
        if r.is_null() {
            return fallback(Reason::Error);
        }
        if COMPILE_COUNTERS.hit_check() {
            return check(r, f, args, nargsf, kw);
        }
        r
    }
}

/// Check mode: the original's Pattern must equal ours (`==` compares flags, code and
/// pattern) and have the same `groups` and `groupindex` (items in order).
///
/// # Safety
/// Attached thread; `ours` a new reference; vectorcall arguments as received.
unsafe fn check(ours: O, f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: per the caller; references released once.
    unsafe {
        let theirs = accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw);
        if theirs.is_null() {
            ffi::Py_DECREF(ours);
            return theirs;
        }
        let py = Python::assume_attached();
        let same = (|| -> PyResult<bool> {
            let a = Bound::from_borrowed_ptr(py, ours);
            let b = Bound::from_borrowed_ptr(py, theirs);
            let items = |p: &Bound<'_, PyAny>| -> PyResult<Vec<(String, usize)>> {
                p.getattr("groupindex")?.call_method0("items")?.try_iter()?.map(|kv| kv?.extract()).collect()
            };
            Ok(a.eq(&b)?
                && a.getattr("groups")?.eq(b.getattr("groups")?)?
                && items(&a)? == items(&b)?
                && a.getattr("pattern")?.is(&b.getattr("pattern")?))
        })()
        .unwrap_or(false);
        if same {
            ffi::Py_DECREF(theirs);
            return ours;
        }
        TARGET_COMPILE.counters.mismatch();
        ffi::Py_DECREF(ours);
        if crate::accel::stats::check_mode() == Some(true) {
            ffi::Py_DECREF(theirs);
            crate::accel::check::RustiqueCheckError::new_err("rustique check: re._compiler.compile: kernel != original")
                .restore(py);
            return std::ptr::null_mut();
        }
        theirs
    }
}

/// Test helper `_accel._re_compile_args(pattern, flags)`: the arguments the kernel passes to
/// `_sre.compile` (`(pattern, flags, code, groups, groupindex, indexgroup)`), or `None` when
/// the port declines. Needs the group prepared (`enable(only="re")` after `import re`).
#[pyfunction]
pub(crate) fn _re_compile_args<'py>(
    py: Python<'py>,
    pattern: &Bound<'py, PyAny>,
    flags: &Bound<'py, PyAny>,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let Some(st) = state::compile_state() else {
        return Err(pyo3::exceptions::PyRuntimeError::new_err("the re group is not prepared"));
    };
    // SAFETY: attached thread; live arguments.
    unsafe {
        let Some(c) = port(st, pattern.as_ptr(), flags.as_ptr()) else {
            return Ok(None);
        };
        let Some(a) = build_args(pattern.as_ptr(), &c) else {
            return Err(PyErr::fetch(py));
        };
        let t = ffi::PyTuple_New(6);
        if t.is_null() {
            for &o in &a[1..] {
                ffi::Py_DECREF(o);
            }
            return Err(PyErr::fetch(py));
        }
        ffi::PyTuple_SET_ITEM(t, 0, ffi::Py_NewRef(a[0]));
        for (i, &o) in a.iter().enumerate().skip(1) {
            ffi::PyTuple_SET_ITEM(t, i as ffi::Py_ssize_t, o);
        }
        Ok(Some(Bound::from_owned_ptr(py, t)))
    }
}

/// Test helper `_accel._re_template_ok(pattern_obj, repl)`: the port's verdict on a `sub`
/// template for a compiled pattern (`True`: CPython's `parse_template` succeeds).
#[pyfunction]
pub(crate) fn _re_template_ok(pattern: &Bound<'_, PyAny>, repl: &Bound<'_, PyAny>) -> PyResult<bool> {
    // SAFETY: attached thread; live arguments.
    Ok(unsafe { super::funcs::template_ok(pattern.as_ptr(), repl.as_ptr()) })
}
