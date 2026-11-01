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
            Some(ffi::PyUnicode_Contains(filename, IMPORTLIB.get()) == 1 && ffi::PyUnicode_Contains(filename, BOOTSTRAP.get()) == 1)
        };
        ffi::Py_DECREF(code);
        r
    }
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
        let lineno = ffi::PyFrame_GetLineNumber(f);
        let code = ffi::PyFrame_GetCode(f).cast::<ffi::PyObject>();
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
            if !DEPS.ok() {
                break 'fb Reason::Deps;
            }
            if !TOOLS.quiet() || !fns_ok(&CODES) {
                break 'fb Reason::Tools;
            }
            if let Some(r) = find(level) {
                STATS.hit();
                return r;
            }
            Reason::Deopt
        };
        STATS.fallback(why);
        original(func, args, nargsf, kw)
    }
}
