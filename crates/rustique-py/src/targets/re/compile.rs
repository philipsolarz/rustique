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

/// Run `f` on the pattern's code units and type (`true`: str) for an exact `str` or `bytes`
/// (short patterns are converted on the stack: a declined pattern costs no allocation here).
///
/// # Safety
/// Attached thread; `p` live.
pub(crate) unsafe fn with_units<R>(p: O, f: impl FnOnce(&[u32], bool) -> R) -> Option<R> {
    const STACK: usize = 160;
    // SAFETY: exact type checks before the buffer reads.
    unsafe {
        let t = ffi::Py_TYPE(p);
        let mut buf = [0u32; STACK];
        let fill = |buf: &mut [u32; STACK], it: &mut dyn ExactSizeIterator<Item = u32>| -> Option<Vec<u32>> {
            if it.len() <= STACK {
                for (d, c) in buf.iter_mut().zip(it) {
                    *d = c;
                }
                None
            } else {
                Some(it.collect())
            }
        };
        let (n, heap, istext) = if t == &raw mut ffi::PyUnicode_Type {
            let n = ffi::PyUnicode_GET_LENGTH(p) as usize;
            let data = ffi::PyUnicode_DATA(p);
            let heap = match ffi::PyUnicode_KIND(p) {
                ffi::PyUnicode_1BYTE_KIND => {
                    fill(&mut buf, &mut std::slice::from_raw_parts(data.cast::<u8>(), n).iter().map(|&c| c as u32))
                }
                ffi::PyUnicode_2BYTE_KIND => {
                    fill(&mut buf, &mut std::slice::from_raw_parts(data.cast::<u16>(), n).iter().map(|&c| c as u32))
                }
                _ => fill(&mut buf, &mut std::slice::from_raw_parts(data.cast::<u32>(), n).iter().copied()),
            };
            (n, heap, true)
        } else if t == &raw mut ffi::PyBytes_Type {
            let n = ffi::Py_SIZE(p) as usize;
            let b = std::slice::from_raw_parts(ffi::PyBytes_AS_STRING(p).cast::<u8>(), n);
            (n, fill(&mut buf, &mut b.iter().map(|&c| c as u32)), false)
        } else {
            return None;
        };
        Some(match &heap {
            Some(v) => f(v, istext),
            None => f(&buf[..n], istext),
        })
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
        let table = st.extra_table.get().and_then(|r| r.as_ref().ok());
        let cases = state::Cases { extra: table, wanted: std::cell::Cell::new(false) };
        let r = with_units(pattern, |units, istext| core::compile(units, istext, fl, &st.env(&cases)).ok())?;
        if !cases.wanted.get() {
            return r;
        }
        // The first compile that needs `_EXTRA_CASES` (Unicode IGNORECASE): check and build
        // it, then compile again with it (the result without the table is discarded).
        let cases = state::Cases { extra: Some(super::extra_table(st)?), wanted: std::cell::Cell::new(false) };
        with_units(pattern, |units, istext| core::compile(units, istext, fl, &st.env(&cases)).ok())?
    }
}

/// The kernel: `compile(p, flags=0)`.
pub(crate) unsafe extern "C" fn k_compile(f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments as received; every guard runs before anything observable.
    unsafe {
        let n = ffi::PyVectorcall_NARGS(nargsf);
        let fallback = |why: Reason| {
            // Debits 1 against a credit of 255 per hit: only a stream of declined patterns
            // (64 in a row from the start, ~320 after a hit) suspends the target.
            COMPILE_COUNTERS.fallback_streak(why);
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
        if !super::compile_ready(st) {
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
        if COMPILE_COUNTERS.hit_check_streak() {
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
            crate::accel::check::RustiqueCheckError::new_err(
                "rustique check: re._compiler.compile: kernel != original",
            )
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
        if !super::compile_ready_now(st) {
            return Err(pyo3::exceptions::PyRuntimeError::new_err("the re group failed its validation"));
        }
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
