//! Targets `re.match` (`re.prefixmatch` on 3.15), `re.fullmatch`, `re.search`, `re.sub`,
//! `re.subn`, `re.split`, `re.findall`, `re.finditer` and `re.compile`: the `_compile`
//! cache hit (CPython's own `re._cache2`, looked up with CPython's key, so `re.purge()` and
//! the caches behave exactly as CPython's), then the same `Pattern` method the original
//! calls. A miss (the pattern gets compiled), and every input outside the exact subset,
//! runs the original.

use pyo3::ffi;

use rustique_core::re::template;

use super::compile::{bytes_units, str_units};
use super::state::{self, O};
use super::{DEPS_FUNCS, FuncRt};
use crate::accel::{self, Reason};

/// The `Pattern` method a module function ends in (index into `Funcs::methods`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum Method {
    /// `prefixmatch` on 3.15 (`match` is the same function), `match` on 3.14
    Match = 0,
    Fullmatch = 1,
    Search = 2,
    Findall = 3,
    Finditer = 4,
    Sub = 5,
    Subn = 6,
    Split = 7,
    /// `re.compile`: no method, the Pattern itself
    Compile = 8,
}

/// Argument binding of the module functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// `(pattern, string, flags=0)`
    Simple,
    /// `(pattern, flags=0)`
    Compile,
    /// `(pattern, repl, string, *args, count=_zero_sentinel, flags=_zero_sentinel)`
    Sub,
    /// `(pattern, string, *args, maxsplit=_zero_sentinel, flags=_zero_sentinel)`
    Split,
}

impl Method {
    fn shape(self) -> Shape {
        match self {
            Method::Match | Method::Fullmatch | Method::Search | Method::Findall | Method::Finditer => Shape::Simple,
            Method::Compile => Shape::Compile,
            Method::Sub | Method::Subn => Shape::Sub,
            Method::Split => Shape::Split,
        }
    }
}

/// Keyword argument `name` of the call (interned-name identity: keyword names of call sites
/// in code objects are interned; others fall back).
///
/// # Safety
/// `kw` NULL or the call's kwnames tuple; `args` the call's arguments.
#[inline(always)]
unsafe fn kwarg(kw: O, args: *const O, nargs: usize, name: O) -> Option<O> {
    // SAFETY: per the caller.
    unsafe {
        if kw.is_null() {
            return None;
        }
        let nk = ffi::PyTuple_GET_SIZE(kw) as usize;
        (0..nk).find(|&j| ffi::PyTuple_GET_ITEM(kw, j as ffi::Py_ssize_t) == name).map(|j| *args.add(nargs + j))
    }
}

/// `__kwdefaults__[name]` of the function, read at call time like the original.
///
/// # Safety
/// `f` a live function object.
#[inline(always)]
unsafe fn kwdefault(f: O, name: O) -> Option<O> {
    // SAFETY: per the caller; exact dict with an interned str key: no Python code runs.
    unsafe {
        let d = (*f.cast::<ffi::PyFunctionObject>()).func_kwdefaults;
        if d.is_null() || ffi::PyDict_CheckExact(d) == 0 {
            return None;
        }
        let v = ffi::PyDict_GetItemWithError(d, name);
        if v.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(v)
    }
}

/// The flags value `_compile` keys its cache with: an exact `int` or `_zero_sentinel` as is,
/// a `RegexFlag` member's `_value_` (exact `int`, from the instance dict: `flags.value`).
/// Borrowed.
///
/// # Safety
/// Attached thread; `fl` live.
#[inline(always)]
unsafe fn key_flags(st: &state::Funcs, fl: O) -> Option<O> {
    // SAFETY: exact type checks before any read.
    unsafe {
        let t = ffi::Py_TYPE(fl);
        if t == &raw mut ffi::PyLong_Type || fl == st.zero {
            return Some(fl);
        }
        if t == st.regexflag_t {
            let d = ffi::PyObject_GenericGetDict(fl, std::ptr::null_mut());
            if d.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            let v = ffi::PyDict_GetItemWithError(d, st.n_value_);
            ffi::Py_DECREF(d);
            if v.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            // The member keeps its `_value_` alive (members are immortal-ish: held by the
            // class's member maps).
            if ffi::Py_TYPE(v) == &raw mut ffi::PyLong_Type {
                return Some(v);
            }
        }
        None
    }
}

/// Is `string` of a type the pattern's method takes without calling user code or raising
/// (str for a str pattern; exact bytes or bytearray for a bytes pattern)?
///
/// # Safety
/// Attached thread; `s` live.
#[inline(always)]
unsafe fn string_ok(s: O, istext: bool) -> bool {
    // SAFETY: type checks only.
    unsafe {
        if istext {
            ffi::PyUnicode_Check(s) != 0
        } else {
            let t = ffi::Py_TYPE(s);
            t == &raw mut ffi::PyBytes_Type || t == &raw mut ffi::PyByteArray_Type
        }
    }
}

/// A `count`/`maxsplit` value `Pattern.sub`/`split` convert without error or user code:
/// `_zero_sentinel`, or an exact int that fits `Py_ssize_t`.
///
/// # Safety
/// Attached thread; `v` live.
#[inline(always)]
unsafe fn ssize_ok(st: &state::Funcs, v: O) -> bool {
    // SAFETY: exact type check first.
    unsafe {
        if v == st.zero {
            return true;
        }
        if ffi::Py_TYPE(v) != &raw mut ffi::PyLong_Type {
            return false;
        }
        let x = ffi::PyLong_AsSsize_t(v);
        if x == -1 && !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        true
    }
}

/// `_compile(pattern, flags)` when it is a cache hit, as a new reference to the cached
/// `Pattern`, with the pattern's type (`true`: str); `None` (no error set) otherwise.
///
/// # Safety
/// Attached thread; live arguments.
#[inline(always)]
unsafe fn cached(st: &state::Funcs, pattern: O, flags: O) -> Option<(O, bool)> {
    // SAFETY: per the caller; the key's members are exact str/bytes/int (C hashes and
    // comparisons, no Python code).
    unsafe {
        let fl = key_flags(st, flags)?;
        let pt = ffi::Py_TYPE(pattern);
        let istext = if pt == &raw mut ffi::PyUnicode_Type {
            true
        } else if pt == &raw mut ffi::PyBytes_Type {
            false
        } else if pt == st.pattern_t {
            // `_compile` returns a compiled pattern as is when `flags` is falsy (an empty
            // `_cache2` lookup and `_cache.pop` miss first: no observable effect).
            let falsy = fl == st.zero || (ffi::Py_TYPE(fl) == &raw mut ffi::PyLong_Type && long_is_zero(fl));
            if !falsy {
                return None;
            }
            let src = ffi::PyObject_GetAttr(pattern, st.n_pattern);
            if src.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            let t = ffi::Py_TYPE(src);
            ffi::Py_DECREF(src);
            let istext = if t == &raw mut ffi::PyUnicode_Type {
                true
            } else if t == &raw mut ffi::PyBytes_Type {
                false
            } else {
                return None;
            };
            return Some((ffi::Py_NewRef(pattern), istext));
        } else {
            return None;
        };
        let key = ffi::PyTuple_New(3);
        if key.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        ffi::PyTuple_SET_ITEM(key, 0, ffi::Py_NewRef(pt.cast()));
        ffi::PyTuple_SET_ITEM(key, 1, ffi::Py_NewRef(pattern));
        ffi::PyTuple_SET_ITEM(key, 2, ffi::Py_NewRef(fl));
        let mut p: O = std::ptr::null_mut();
        let rc = ffi::PyDict_GetItemRef(st.cache2, key, &mut p);
        ffi::Py_DECREF(key);
        if rc < 0 {
            ffi::PyErr_Clear();
            return None;
        }
        if rc == 0 {
            return None;
        }
        if ffi::Py_TYPE(p) != st.pattern_t {
            ffi::Py_DECREF(p);
            return None;
        }
        Some((p, istext))
    }
}

/// # Safety
/// `v` an exact int.
#[inline(always)]
unsafe fn long_is_zero(v: O) -> bool {
    // SAFETY: per the caller.
    unsafe { ffi::PyObject_IsTrue(v) == 0 }
}

/// Is `repl` a replacement `Pattern.sub` handles without user code or errors: an exact
/// str/bytes of the pattern's type that is a literal (no backslash) or a template CPython's
/// `parse_template` accepts for this pattern (validated by the port)?
///
/// # Safety
/// Attached thread; `p` an exact Pattern; `repl` live.
unsafe fn repl_ok(st: &state::Funcs, p: O, repl: O, istext: bool) -> bool {
    // SAFETY: exact type checks before buffer reads.
    unsafe {
        let t = ffi::Py_TYPE(repl);
        let literal = if istext {
            if t != &raw mut ffi::PyUnicode_Type {
                return false;
            }
            ffi::PyUnicode_FindChar(repl, '\\' as u32, 0, ffi::PyUnicode_GET_LENGTH(repl), 1) == -1
        } else {
            if t != &raw mut ffi::PyBytes_Type {
                return false;
            }
            let b = std::slice::from_raw_parts(
                ffi::PyBytes_AS_STRING(repl).cast::<u8>(),
                ffi::PyBytes_GET_SIZE(repl) as usize,
            );
            !b.contains(&b'\\')
        };
        literal || template_checked(st, p, repl, istext)
    }
}

/// The port's `parse_template` verdict for an exact str/bytes template of pattern `p`.
///
/// # Safety
/// Attached thread; `p` an exact Pattern; `repl` an exact str or bytes.
unsafe fn template_checked(st: &state::Funcs, p: O, repl: O, istext: bool) -> bool {
    // SAFETY: C getters of an exact Pattern (member/getset descriptors).
    unsafe {
        let g = ffi::PyObject_GetAttr(p, st.n_groups);
        if g.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        let groups = ffi::PyLong_AsLong(g);
        ffi::Py_DECREF(g);
        if groups < 0 {
            ffi::PyErr_Clear();
            return false;
        }
        let units = if istext { str_units(repl) } else { bytes_units(repl) };
        let maxgroups = state::compile_state().map_or(i32::MAX as u32 / 2, |c| c.maxgroups);
        let index = |name: &str| -> Option<u32> {
            let gi = ffi::PyObject_GetAttr(p, st.n_groupindex);
            if gi.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            let k = ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name.len() as ffi::Py_ssize_t);
            let v = if k.is_null() { std::ptr::null_mut() } else { ffi::PyObject_GetItem(gi, k) };
            ffi::Py_XDECREF(k);
            ffi::Py_DECREF(gi);
            if v.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            let x = ffi::PyLong_AsLong(v);
            ffi::Py_DECREF(v);
            if x < 0 {
                ffi::PyErr_Clear();
                return None;
            }
            Some(x as u32)
        };
        template::check(&units, groups as u32, maxgroups, &index).is_ok()
    }
}

/// Test helper backend: the template verdict for any compiled pattern.
///
/// # Safety
/// Attached thread; live arguments.
pub(crate) unsafe fn template_ok(p: O, repl: O) -> bool {
    // SAFETY: per the caller.
    unsafe {
        let Some(st) = state::funcs_state() else { return false };
        if ffi::Py_TYPE(p) != st.pattern_t {
            return false;
        }
        let src = ffi::PyObject_GetAttr(p, st.n_pattern);
        if src.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        let istext = ffi::Py_TYPE(src) == &raw mut ffi::PyUnicode_Type;
        ffi::Py_DECREF(src);
        let t = ffi::Py_TYPE(repl);
        if t != (if istext { &raw mut ffi::PyUnicode_Type } else { &raw mut ffi::PyBytes_Type }) {
            return false;
        }
        template_checked(st, p, repl, istext)
    }
}

/// The kernel of one module function.
///
/// # Safety
/// Vectorcall arguments as received by the kernel of `rt`'s target.
#[inline(always)]
pub(crate) unsafe fn kernel(rt: &FuncRt, m: Method, f: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: per the caller; guards run before anything observable.
    unsafe {
        let fallback = |why: Reason| {
            rt.counters.fallback_streak(why);
            accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw)
        };
        if !DEPS_FUNCS.ok() {
            return fallback(Reason::Deps);
        }
        let Some(st) = state::funcs_state() else {
            return fallback(Reason::Deps);
        };
        if !st.tags_intact() {
            return fallback(Reason::Deps);
        }
        if !accel::tools_quiet(&rt.tools, f) {
            return fallback(Reason::Tools);
        }
        let n = ffi::PyVectorcall_NARGS(nargsf) as usize;
        let nk = if kw.is_null() { 0 } else { ffi::PyTuple_GET_SIZE(kw) as usize };
        let defaults = || {
            let d = (*f.cast::<ffi::PyFunctionObject>()).func_defaults;
            if d.is_null() || ffi::PyTuple_GET_SIZE(d) != 1 { None } else { Some(ffi::PyTuple_GET_ITEM(d, 0)) }
        };
        // Bind (pattern, [repl,] string, aux, flags) as the original's signature does.
        let (npos, aux_name) = match m.shape() {
            Shape::Simple => (2, None),
            Shape::Compile => (1, None),
            Shape::Sub => (3, Some(st.n_count)),
            Shape::Split => (2, Some(st.n_maxsplit)),
        };
        let mut used = 0;
        let (flags, aux) = match m.shape() {
            Shape::Simple | Shape::Compile => {
                let fl = if n == npos + 1 && nk == 0 {
                    Some(*args.add(npos))
                } else if n == npos && nk == 0 {
                    defaults()
                } else if n == npos && nk == 1 {
                    used = 1;
                    kwarg(kw, args, n, st.n_flags)
                } else {
                    None
                };
                (fl, None)
            }
            Shape::Sub | Shape::Split => {
                if n != npos {
                    // positional count/maxsplit: DeprecationWarning path
                    return fallback(Reason::Args);
                }
                let aux_name = aux_name.expect("set for these shapes");
                let aux = match kwarg(kw, args, n, aux_name) {
                    Some(v) => {
                        used += 1;
                        Some(v)
                    }
                    None => kwdefault(f, aux_name),
                };
                let fl = match kwarg(kw, args, n, st.n_flags) {
                    Some(v) => {
                        used += 1;
                        Some(v)
                    }
                    None => kwdefault(f, st.n_flags),
                };
                (fl, aux)
            }
        };
        let Some(flags) = flags else {
            return fallback(Reason::Args);
        };
        if used != nk {
            return fallback(Reason::Args);
        }
        let pattern = *args;
        let Some((p, istext)) = cached(st, pattern, flags) else {
            return fallback(Reason::Args);
        };
        if m == Method::Compile {
            rt.counters.hit_streak();
            return p;
        }
        let string = *args.add(npos - 1);
        let ok = string_ok(string, istext)
            && match m.shape() {
                Shape::Sub => {
                    ssize_ok(st, aux.expect("bound above")) && repl_ok(st, p, *args.add(1), istext)
                }
                Shape::Split => ssize_ok(st, aux.expect("bound above")),
                _ => true,
            };
        if !ok {
            ffi::Py_DECREF(p);
            return fallback(Reason::Args);
        }
        rt.counters.hit_streak();
        let meth = st.methods[m as usize];
        let r = match m.shape() {
            Shape::Sub => {
                let call = [p, *args.add(1), string, aux.expect("bound above")];
                ffi::PyObject_Vectorcall(meth, call.as_ptr(), 4, std::ptr::null_mut())
            }
            Shape::Split => {
                let call = [p, string, aux.expect("bound above")];
                ffi::PyObject_Vectorcall(meth, call.as_ptr(), 3, std::ptr::null_mut())
            }
            _ => {
                let call = [p, string];
                ffi::PyObject_Vectorcall(meth, call.as_ptr(), 2, std::ptr::null_mut())
            }
        };
        ffi::Py_DECREF(p);
        r
    }
}
