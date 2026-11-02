//! `Logger.findCaller(stack_info=False, stacklevel=1)` (target `logging.Logger.findCaller`).
//!
//! Walks the caller's frames with `_is_internal_frame` semantics (`_srcfile` and frozen
//! `importlib._bootstrap` frames are skipped) and returns
//! `(co_filename, f_lineno, co_name, None)`. The kernel has no frame of its own; it starts
//! where CPython's `sys._getframe(1).f_back` would, so the walk visits the same frames and
//! materialises the same frame objects. `stack_info` true (traceback formatting) and
//! `stacklevel < 1` (the result would be `findCaller`'s own frame) run CPython's original.

use pyo3::ffi;

use super::common::*;
use crate::accel::guard::DepFlag;
use crate::accel::stats::{Counters, Reason};
use crate::accel::version::RawCode;

pub static DEPS: DepFlag = DepFlag::new();
pub static STATS: Counters = Counters::new();
/// `logging._srcfile` (pinned; an exact str).
pub static SRCFILE: Cap = Cap::new();
/// `"importlib"`, `"_bootstrap"`.
pub static IMPORTLIB: Cap = Cap::new();
pub static BOOTSTRAP: Cap = Cap::new();
/// findCaller, _is_internal_frame, currentframe, posixpath.normcase.
pub static CODES: [FnGuard; 4] = [const { FnGuard::new() }; 4];

/// `_is_internal_frame(f)`: `Some(bool)`, or `None` when the filename is not an exact str.
#[inline(always)]
unsafe fn internal(f: *mut ffi::PyFrameObject) -> Option<bool> {
    // SAFETY: live frame; attached thread; code layout calibrated at install.
    unsafe {
        let code = ffi::PyFrame_GetCode(f).cast::<ffi::PyObject>();
        let filename = (*code.cast::<RawCode>()).co_filename;
        let r = if !exact_str(filename) {
            None
        } else if ffi::PyUnicode_Compare(filename, SRCFILE.get()) == 0 {
            Some(true)
        } else {
            Some(
                ffi::PyUnicode_Contains(filename, IMPORTLIB.get()) == 1
                    && ffi::PyUnicode_Contains(filename, BOOTSTRAP.get()) == 1,
            )
        };
        ffi::Py_DECREF(code);
        r
    }
}

/// `PyFrameObject` prefix (pycore_frame.h, identical in 3.14 and 3.15).
#[repr(C)]
struct RawFrame {
    ob_base: ffi::PyObject,
    f_back: O,
    f_frame: *mut u8,
    f_trace: O,
    f_lineno: std::ffi::c_int,
}

unsafe extern "C" {
    fn PyFrame_GetLasti(f: *mut ffi::PyFrameObject) -> std::ffi::c_int;
}

/// `f.f_lineno` (`PyFrame_GetLineNumber`). For a frame whose `f_lineno` field is unset (0:
/// not being traced) the line is `PyCode_Addr2Line(code, lasti)`, a pure function of the
/// code object and the instruction offset (CPython decodes the line table on every call:
/// 79% of findCaller's time). The kernels memoise it per `(code, co_version, lasti)` in a
/// per-thread table (`co_version` is unique per code object, so a reused address cannot
/// alias; per thread: no shared line written under threads, free-threaded builds too).
unsafe fn line_number(f: *mut ffi::PyFrameObject, code: O) -> std::ffi::c_int {
    // SAFETY: live frame of this thread and its code object.
    unsafe {
        if (*f.cast::<RawFrame>()).f_lineno == 0 {
            let version = (*code.cast::<RawCode>()).co_version;
            let lasti = PyFrame_GetLasti(f);
            if version != 0 && lasti >= 0 {
                let key = (code as u64) ^ ((lasti as u64) << 48) ^ ((version as u64) << 20);
                let h = (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - LINE_BITS)) as usize;
                return LINE_CACHE.with(|c| {
                    let e = &mut (*c.get())[h];
                    if e.0 == code as usize && e.1 == version && e.2 == lasti {
                        return e.3;
                    }
                    let line = ffi::PyFrame_GetLineNumber(f);
                    *e = (code as usize, version, lasti, line);
                    line
                });
            }
        }
        ffi::PyFrame_GetLineNumber(f)
    }
}

const LINE_BITS: u32 = 8;
const LINES: usize = 1 << LINE_BITS;

type LineEntry = (usize, u32, std::ffi::c_int, std::ffi::c_int);

thread_local! {
    /// `(code address, co_version, lasti) -> line`, per thread (no Drop: a plain TLS slot).
    static LINE_CACHE: std::cell::UnsafeCell<[LineEntry; LINES]> =
        const { std::cell::UnsafeCell::new([(0, 0, -1, 0); LINES]) };
}

unsafe fn find(mut level: i64) -> Option<O> {
    // SAFETY: attached thread; frames of this thread.
    unsafe {
        let top = ffi::PyEval_GetFrame();
        if top.is_null() {
            return None; // CPython would answer with findCaller's own frame
        }
        let mut f = top;
        ffi::Py_INCREF(f.cast());
        loop {
            match internal(f) {
                Some(true) => {}
                Some(false) => level -= 1,
                None => {
                    ffi::Py_DECREF(f.cast());
                    return None;
                }
            }
            if level <= 0 {
                break;
            }
            let back = ffi::PyFrame_GetBack(f);
            if back.is_null() {
                break;
            }
            ffi::Py_DECREF(f.cast());
            f = back;
        }
        let code = ffi::PyFrame_GetCode(f).cast::<ffi::PyObject>();
        let lineno = line_number(f, code);
        ffi::Py_DECREF(f.cast());
        if lineno < 0 {
            ffi::Py_DECREF(code);
            return None;
        }
        let raw = &*code.cast::<RawCode>();
        let t = ffi::PyTuple_New(4);
        let ln = ffi::PyLong_FromLong(lineno as _);
        if t.is_null() || ln.is_null() {
            ffi::PyErr_Clear();
            ffi::Py_XDECREF(t);
            ffi::Py_XDECREF(ln);
            ffi::Py_DECREF(code);
            return None;
        }
        ffi::PyTuple_SET_ITEM(t, 0, ffi::Py_NewRef(raw.co_filename));
        ffi::PyTuple_SET_ITEM(t, 1, ln);
        ffi::PyTuple_SET_ITEM(t, 2, ffi::Py_NewRef(raw.co_name));
        ffi::PyTuple_SET_ITEM(t, 3, ffi::Py_NewRef(ffi::Py_None()));
        ffi::Py_DECREF(code);
        Some(t)
    }
}

/// `Logger.findCaller(self, stack_info=False, stacklevel=1)`.
///
/// # Safety
/// Installed as the function's vectorcall: CPython calls it with vectorcall arguments.
pub unsafe extern "C" fn vc_find_caller(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let n = nargs(nargsf);
        let why = 'fb: {
            if !kw.is_null() || !(1..=3).contains(&n) {
                break 'fb Reason::Args;
            }
            if n >= 2 {
                let si = *args.add(1);
                if si != ffi::Py_False() && si != ffi::Py_None() {
                    break 'fb Reason::Args;
                }
            }
            let level = if n == 3 {
                let sl = *args.add(2);
                if !exact_int(sl) {
                    break 'fb Reason::Args;
                }
                let mut ov = 0;
                let v = ffi::PyLong_AsLongLongAndOverflow(sl, &mut ov);
                if ov != 0 || !(1..=1 << 30).contains(&v) {
                    break 'fb Reason::Args;
                }
                v
            } else {
                1
            };
            if !DEPS.ok() || !SHARED_DEPS.ok() {
                break 'fb Reason::Deps;
            }
            if !TOOLS.quiet() || !fns_ok(&CODES) {
                break 'fb Reason::Tools;
            }
            if let Some(r) = find(level) {
                if STATS.hit_check_streak() {
                    let a = std::slice::from_raw_parts(args, n);
                    return crate::accel::check::verify(&super::FIND_CALLER, r, a, || original(func, args, nargsf, kw));
                }
                return r;
            }
            Reason::Deopt
        };
        STATS.fallback_streak(why);
        original(func, args, nargsf, kw)
    }
}
