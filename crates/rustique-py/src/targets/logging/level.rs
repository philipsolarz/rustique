//! The level methods `debug`/`info`/`warning`/`error`/`critical`/`log` (target
//! `logging.Logger.level`). `Logger.isEnabledFor` itself is not a site: called directly it
//! is ~600 Ir and a kernel gains nothing there (1.36 on free-threaded builds, where
//! attribute reads go through `PyObject_GenericGetAttr`); its logic is inlined here.
//!
//! The kernels decide `self.isEnabledFor(level)` from `self.disabled` and the logger's
//! level cache exactly as CPython does, and return `None` for a **disabled** call: the most
//! frequent logging call in real code. An enabled call (or a cache miss, which takes
//! `logging._lock`) runs CPython's original, so every frame below it (`_log`, handlers,
//! filters, user code) is CPython's.

use pyo3::ffi;

use super::common::*;
use crate::accel::guard::DepFlag;
use crate::accel::stats::{Counters, Reason};

pub static DEPS: DepFlag = DepFlag::new();
pub static STATS: Counters = Counters::new();
/// `Logger.isEnabledFor`.
pub static IS_ENABLED_FOR: Cap = Cap::new();
/// `logging.DEBUG`, `INFO`, `WARNING`, `ERROR`, `CRITICAL` (pinned).
pub static LEVELS: [Cap; 5] = [const { Cap::new() }; 5];
/// Code objects the kernels stand in for: isEnabledFor, debug, info, warning, error,
/// critical, log.
pub static CODES: [FnGuard; 7] = [const { FnGuard::new() }; 7];

/// Reads `disabled`, `_cache`.
pub static LOGGERS: TypeCache<2> = TypeCache::new(validate_logger);

/// A logger class whose `isEnabledFor` is CPython's and whose `disabled`/`_cache` are plain
/// instance attributes (no descriptors), with the generic attribute protocol.
unsafe fn validate_logger(t: Tp) -> bool {
    // SAFETY: attached thread; live type (TypeCache contract).
    unsafe {
        let n = n();
        generic_attrs(t, false)
            && resolves_to(t, o(n.is_enabled_for), IS_ENABLED_FOR.get())
            && plain_attr(t, o(n.disabled))
            && plain_attr(t, o(n.cache))
    }
}

/// `self.isEnabledFor(level)` (`level` an exact int) without running Python code:
/// `Some(value)` (new reference: what CPython returns) or `None` when CPython would take
/// the cache-miss path (lock, `getEffectiveLevel`) or the operands are not plain.
///
/// # Safety
/// Attached thread; `me` of a validated logger type.
#[inline(always)]
unsafe fn decide(e: usize, me: O, level: O) -> Option<O> {
    // SAFETY: per the caller.
    unsafe {
        let d = LOGGERS.read(e, me, 0);
        if d.is_null() {
            return None;
        }
        let t = pure_truth(d);
        ffi::Py_DECREF(d);
        match t {
            Some(true) => return Some(ffi::Py_NewRef(ffi::Py_False())),
            Some(false) => {}
            None => return None,
        }
        let c = LOGGERS.read(e, me, 1);
        if c.is_null() {
            return None;
        }
        let r = if exact_dict(c) { int_keyed_get(c, level) } else { Err(()) };
        ffi::Py_DECREF(c);
        match r {
            Ok(Some(v)) => Some(v),
            _ => None,
        }
    }
}

/// No keyword in `kw` names one of the positional parameters (`self`, `msg`, and `level`
/// for `log`): CPython would raise "got multiple values". Every keyword is an exact str.
#[inline(always)]
unsafe fn kw_ok(kw: O, with_level: bool) -> bool {
    if kw.is_null() {
        return true;
    }
    // SAFETY: kwnames is a tuple of str (vectorcall protocol).
    unsafe {
        let n = n();
        for i in 0..ffi::PyTuple_GET_SIZE(kw) {
            let k = ffi::PyTuple_GET_ITEM(kw, i);
            if !exact_str(k) {
                return false;
            }
            let len = ffi::PyUnicode_GET_LENGTH(k);
            let clash = match len {
                3 => ffi::PyUnicode_Compare(k, o(n.msg)) == 0,
                4 => ffi::PyUnicode_Compare(k, o(n.self_)) == 0,
                5 => with_level && ffi::PyUnicode_Compare(k, o(n.level)) == 0,
                _ => false,
            };
            if clash {
                return false;
            }
        }
        true
    }
}

/// Shared body of the level methods: `if self.isEnabledFor(level): self._log(...)`.
#[inline(always)]
unsafe fn level_call(func: O, args: *const O, nargsf: usize, kw: O, level: O, code: usize, first: usize) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let why = 'fb: {
            if nargs(nargsf) < first + 1 || !kw_ok(kw, first == 2) {
                break 'fb Reason::Args;
            }
            let me = *args;
            if !exact_int(level) {
                break 'fb Reason::Args;
            }
            let Some(e) = LOGGERS.entry(ffi::Py_TYPE(me)) else { break 'fb Reason::Args };
            if !DEPS.ok() || !SHARED_DEPS.ok() {
                break 'fb Reason::Deps;
            }
            if !TOOLS.quiet() || !CODES[0].ok() || !CODES[code].ok() {
                break 'fb Reason::Tools;
            }
            if !bound_to(me, o(n().is_enabled_for), IS_ENABLED_FOR.get()) {
                break 'fb Reason::Args;
            }
            if let Some(v) = decide(e, me, level) {
                let t = pure_truth(v);
                ffi::Py_DECREF(v);
                if t == Some(false) {
                    let r = ffi::Py_NewRef(ffi::Py_None());
                    if STATS.hit_check_streak() {
                        let a = std::slice::from_raw_parts(args, nargs(nargsf));
                        return crate::accel::check::verify(&super::LEVEL, r, a, || original(func, args, nargsf, kw));
                    }
                    return r;
                }
            }
            // Enabled (or undecided): CPython's own frames run the record path. The call
            // costs the kernel's checks plus the vectorcall fallback (~+800 Ir), a disabled
            // call saves ~500 Ir: the score (credit 5 per hit, debit 7 per enabled call,
            // `LEVEL`'s spec) suspends the target in programs where enabled calls dominate
            // (services logging every request), so they pay nothing.
            STATS.fallback_streak(Reason::Deopt);
            return original(func, args, nargsf, kw);
        };
        STATS.fallback_streak(why);
        original(func, args, nargsf, kw)
    }
}

macro_rules! level_kernel {
    ($name:ident, $idx:expr, $code:expr) => {
        /// A level method (`debug`, `info`, ...).
        ///
        /// # Safety
        /// Installed as the function's vectorcall: CPython calls it with vectorcall arguments.
        pub unsafe extern "C" fn $name(func: O, args: *const O, nargsf: usize, kw: O) -> O {
            // SAFETY: vectorcall arguments.
            unsafe { level_call(func, args, nargsf, kw, LEVELS[$idx].get(), $code, 1) }
        }
    };
}

level_kernel!(vc_debug, 0, 1);
level_kernel!(vc_info, 1, 2);
level_kernel!(vc_warning, 2, 3);
level_kernel!(vc_error, 3, 4);
level_kernel!(vc_critical, 4, 5);

/// `Logger.log(self, level, msg, *args, **kwargs)` with an exact int level.
///
/// # Safety
/// Installed as the function's vectorcall: CPython calls it with vectorcall arguments.
pub unsafe extern "C" fn vc_log(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        if nargs(nargsf) >= 3 {
            return level_call(func, args, nargsf, kw, *args.add(1), 6, 2);
        }
        STATS.fallback_streak(Reason::Args);
        original(func, args, nargsf, kw)
    }
}
