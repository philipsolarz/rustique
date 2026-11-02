//! Target `json.dumps`: the vectorcall of the pure-Python `json.dumps`.
//!
//! Exact subset (decided before anything observable happens): one positional argument,
//! keyword arguments among `skipkeys`, `ensure_ascii`, `check_circular`, `allow_nan`,
//! `sort_keys` (exact `bool`/`int`/`None`: their truthiness runs no code), `indent` (`None`,
//! an exact `int` in `-4096..=4096` or an exact `str`), `separators` (`None` or an exact
//! 2-tuple of exact `str`), `cls=None` or `cls=JSONEncoder`, and any `default` (lazy: the
//! hook is needed only for an object outside the subset, where the kernel restarts in the
//! original before calling anything); under `ensure_ascii` the
//! separators and indent must be ASCII. Then the object tree must be in the encoder's
//! subset ([`super::encode`]); otherwise the original runs from the start (nothing
//! observable happened: no Python code ran, no object was mutated).
//!
//! What CPython runs for these calls — `dumps` → `JSONEncoder.encode` →
//! `JSONEncoder.iterencode` → `c_make_encoder(...)(o, 0)` → `''.join` — has no other
//! observable effect than its result; the dependencies it reads at call time are pinned in
//! [`super::prepare_dumps`].

use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};

use pyo3::ffi;

use rustique_core::json::escape::Text;

use super::encode::{Enc, Indent, Opts, Stop};
use super::pystr::{self, View, view};
use crate::accel::{self, Reason, check};
use crate::targets::float::new_ascii;

pub(super) static COUNTERS: accel::Counters = accel::Counters::new();
pub(super) static DEPS: accel::DepFlag = accel::DepFlag::new();
pub(super) static TOOLS: accel::Tools = accel::Tools::new();

/// Objects the kernel was installed for (identity checks per call; their contents are
/// pinned): the `json.dumps` function, its code and keyword defaults, `json._default_encoder`
/// and the `JSONEncoder` class.
pub(super) static FUNC: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
pub(super) static CODE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
pub(super) static KWDEFAULTS: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
pub(super) static DEFAULT_ENCODER: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
pub(super) static ENCODER_TYPE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());

/// `JSONEncoder.encode` / `iterencode`: code and defaults unchanged.
pub(super) static ENCODE: super::FnCheck = super::FnCheck::new();
pub(super) static ITERENCODE: super::FnCheck = super::FnCheck::new();

/// Interned keyword names, in `KW_*` order.
pub(super) static KW: [AtomicPtr<ffi::PyObject>; 9] = [const { AtomicPtr::new(std::ptr::null_mut()) }; 9];
pub(super) const KW_NAMES: [&str; 9] =
    ["skipkeys", "ensure_ascii", "check_circular", "allow_nan", "cls", "indent", "separators", "default", "sort_keys"];
const KW_SKIPKEYS: usize = 0;
const KW_ENSURE_ASCII: usize = 1;
const KW_CHECK_CIRCULAR: usize = 2;
const KW_ALLOW_NAN: usize = 3;
const KW_CLS: usize = 4;
const KW_INDENT: usize = 5;
const KW_SEPARATORS: usize = 6;
const KW_DEFAULT: usize = 7;
const KW_SORT_KEYS: usize = 8;

/// Which keyword `name` is (interned pointer first, then by value).
#[inline(always)]
unsafe fn kw_index(name: *mut ffi::PyObject) -> Option<usize> {
    for (i, k) in KW.iter().enumerate() {
        if k.load(Relaxed) == name {
            return Some(i);
        }
    }
    // SAFETY: keyword names are str; comparing two exact str runs no Python code.
    unsafe {
        if ffi::Py_TYPE(name) != &raw mut ffi::PyUnicode_Type {
            return None;
        }
        KW.iter().position(|k| ffi::PyUnicode_Compare(name, k.load(Relaxed)) == 0)
    }
}

/// Truthiness of a flag argument when it runs no Python code: exact `bool`, `None`, or an
/// exact `int`.
#[inline(always)]
unsafe fn flag(v: *mut ffi::PyObject) -> Option<bool> {
    // SAFETY: live object; exact types only.
    unsafe {
        if v == ffi::Py_True() {
            Some(true)
        } else if v == ffi::Py_False() || v == ffi::Py_None() {
            Some(false)
        } else if ffi::Py_TYPE(v) == &raw mut ffi::PyLong_Type {
            Some(ffi::PyObject_IsTrue(v) == 1)
        } else {
            None
        }
    }
}

/// The options of a call, or `None` when the call is outside the exact subset.
#[inline(always)]
unsafe fn parse_opts(args: *const *mut ffi::PyObject, nargs: usize, kwnames: *mut ffi::PyObject) -> Option<Opts> {
    let mut o = Opts::DEFAULT;
    if kwnames.is_null() {
        return Some(o);
    }
    // SAFETY: vectorcall convention: `kwnames` is a tuple of str, values follow the
    // positional arguments.
    unsafe {
        let n = ffi::PyTuple_GET_SIZE(kwnames) as usize;
        for j in 0..n {
            let v = *args.add(nargs + j);
            match kw_index(ffi::PyTuple_GET_ITEM(kwnames, j as ffi::Py_ssize_t))? {
                KW_SKIPKEYS => o.skipkeys = flag(v)?,
                KW_ENSURE_ASCII => o.ensure_ascii = flag(v)?,
                KW_CHECK_CIRCULAR => o.check_circular = flag(v)?,
                KW_ALLOW_NAN => o.allow_nan = flag(v)?,
                KW_SORT_KEYS => o.sort_keys = flag(v)?,
                // Lazy `default=`: the hook is called only for an object outside the
                // encoder's subset, and there the kernel restarts in the original before
                // anything observable happened (ruling 15: the original is Python), so any
                // `default` gives the result of `default=None` whenever the kernel completes.
                KW_DEFAULT => {}
                // `cls=JSONEncoder` itself (pinned, its `__dict__` snapshot-pinned) builds the
                // encoder `cls=None` builds; subclasses fall back.
                KW_CLS => {
                    if v != ffi::Py_None() && v != ENCODER_TYPE.load(Relaxed) {
                        return None;
                    }
                }
                KW_INDENT => {
                    if v == ffi::Py_None() {
                        o.indent = None;
                    } else if ffi::Py_TYPE(v) == &raw mut ffi::PyUnicode_Type {
                        o.indent = Some(Indent::Str(view(v)));
                    } else if ffi::Py_TYPE(v) == &raw mut ffi::PyLong_Type
                        || ffi::Py_TYPE(v) == &raw mut ffi::PyBool_Type
                    {
                        let mut overflow = 0;
                        let k = ffi::PyLong_AsLongLongAndOverflow(v, &mut overflow);
                        if overflow != 0 || !(-4096..=4096).contains(&k) {
                            return None;
                        }
                        // `' ' * n`: empty for n <= 0, still an indent (newlines).
                        o.indent = Some(Indent::Spaces(k.max(0) as usize));
                    } else {
                        return None;
                    }
                }
                KW_SEPARATORS => {
                    if v == ffi::Py_None() {
                        o.item_sep = None;
                        o.key_sep = None;
                    } else {
                        let t = ffi::Py_TYPE(v);
                        // A tuple keeps its items alive for the call (a list could drop
                        // them from another thread on free-threaded builds): lists fall back.
                        let (a, b) = if t == &raw mut ffi::PyTuple_Type && ffi::PyTuple_GET_SIZE(v) == 2 {
                            (ffi::PyTuple_GET_ITEM(v, 0), ffi::PyTuple_GET_ITEM(v, 1))
                        } else {
                            return None;
                        };
                        if ffi::Py_TYPE(a) != &raw mut ffi::PyUnicode_Type
                            || ffi::Py_TYPE(b) != &raw mut ffi::PyUnicode_Type
                        {
                            return None;
                        }
                        o.item_sep = Some(view(a));
                        o.key_sep = Some(view(b));
                    }
                }
                _ => return None,
            }
        }
        if o.ensure_ascii {
            let ascii = |v: &Option<View>| v.as_ref().is_none_or(|v| v.ascii);
            if !ascii(&o.item_sep) || !ascii(&o.key_sep) {
                return None;
            }
            if let Some(Indent::Str(v)) = &o.indent
                && !v.ascii
            {
                return None;
            }
        }
    }
    Some(o)
}

thread_local! {
    static ASCII_BUF: std::cell::Cell<Vec<u8>> = const { std::cell::Cell::new(Vec::new()) };
}

/// Encode `obj` with `opts` into a new `str`; `Err` when outside the subset.
///
/// # Safety
/// Attached thread; live object; string views in `opts` alive for the call.
pub(super) unsafe fn dumps(obj: *mut ffi::PyObject, opts: &Opts) -> Result<*mut ffi::PyObject, Stop> {
    // SAFETY: per the caller.
    unsafe {
        if opts.ensure_ascii {
            // A per-thread buffer: no allocation per call (taken, so a nested use cannot
            // alias it).
            let mut out = ASCII_BUF.take();
            out.clear();
            let r = Enc::new(&mut out, opts).top(obj);
            let s = if r.is_ok() { new_ascii(&out) } else { std::ptr::null_mut() };
            if out.capacity() <= 1 << 20 {
                ASCII_BUF.set(out);
            }
            r?;
            if s.is_null() {
                ffi::PyErr_Clear();
                return Err(Stop);
            }
            Ok(s)
        } else {
            let mut out = Text::with_capacity(256);
            Enc::new(&mut out, opts).top(obj)?;
            let s = pystr::from_text(&out);
            if s.is_null() {
                ffi::PyErr_Clear();
                return Err(Stop);
            }
            Ok(s)
        }
    }
}

/// The `json.dumps` kernel.
pub(super) unsafe extern "C" fn kernel(
    func: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: vectorcall arguments of `json.dumps`; attached thread.
    unsafe {
        let nargs = nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET;
        let why = 'fallback: {
            if nargs != 1 {
                break 'fallback Reason::Args;
            }
            let Some(opts) = parse_opts(args, nargs, kwnames) else {
                break 'fallback Reason::Args;
            };
            let f = func.cast::<ffi::PyFunctionObject>();
            if !DEPS.ok()
                || !super::SHARED_DEPS.ok()
                || func != FUNC.load(Relaxed)
                || (*f).func_code != CODE.load(Relaxed)
                || (*f).func_kwdefaults != KWDEFAULTS.load(Relaxed)
                || ffi::Py_TYPE(DEFAULT_ENCODER.load(Relaxed)).cast() != ENCODER_TYPE.load(Relaxed)
                || !ENCODE.ok()
                || !ITERENCODE.ok()
            {
                break 'fallback Reason::Deps;
            }
            if !accel::tools_quiet(&TOOLS, func) {
                break 'fallback Reason::Tools;
            }
            match dumps(*args, &opts) {
                Ok(r) => {
                    if COUNTERS.hit_check_streak() {
                        let all = std::slice::from_raw_parts(
                            args,
                            nargs + if kwnames.is_null() { 0 } else { ffi::PyTuple_GET_SIZE(kwnames) as usize },
                        );
                        return check::verify(&super::DUMPS, r, all, || {
                            accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kwnames)
                        });
                    }
                    return r;
                }
                Err(Stop) => break 'fallback Reason::Deopt,
            }
        };
        COUNTERS.fallback_streak(why);
        accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kwnames)
    }
}
