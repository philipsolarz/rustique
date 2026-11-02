//! `Handler.handle` for a `StreamHandler` writing to an in-memory `io.StringIO` (target
//! `logging.Handler.handle`).
//!
//! CPython's path (identical in 3.14 and 3.15): `Handler.handle` → `Filterer.filter` (no
//! filters: the record) → `with self.lock:` → `StreamHandler.emit` → `Handler.format` →
//! `Formatter.format` → `stream.write(msg + self.terminator)` → `StreamHandler.flush`
//! (`with self.lock:` `if self.stream and hasattr(self.stream, "flush"): self.stream.flush()`)
//! → return the record. Every step is CPython code with nothing user-defined below it when:
//! - the handler's class resolves `filter`/`emit`/`format`/`flush` to CPython's functions
//!   (no instance shadows them) and `filters` is an exact empty list;
//! - `lock` is an exact `_thread.RLock` (C `acquire`/`release`), taken without waiting: a
//!   lock held by another thread falls back before anything happened (CPython then blocks
//!   in its own frames, handling signals there);
//! - `stream` is an exact `_io.StringIO` without an instance `__dict__` (so `write`/`flush`
//!   are its C methods, and `IOBase.flush` finds no `__IOBase_closed`): a write that fails
//!   (closed stream) changed nothing, so the kernel restarts in the original, whose
//!   `handleError` then reports it from CPython's frames;
//! - the formatter (or `_defaultFormatter`) is one `Formatter.format`'s kernel formats
//!   natively (`format::format`); anything it declines restarts in the original.
//!
//! File and console streams (`TextIOWrapper`) stay CPython's: an `OSError` in their `write`
//! or `flush` happens after bytes may have been handed to the OS, and `handleError` prints
//! a traceback through `emit`'s and `flush`'s frames, which a kernel cannot reproduce.

use pyo3::ffi;

use super::common::*;
use super::format::{self, FMT_RECORDS, FORMATTERS};
use crate::accel::guard::DepFlag;
use crate::accel::stats::{Counters, Reason};

pub static DEPS: DepFlag = DepFlag::new();
pub static STATS: Counters = Counters::new();

/// Handler.handle, Filterer.filter, StreamHandler.emit, Handler.format, StreamHandler.flush.
pub static CODES: [FnGuard; 5] = [const { FnGuard::new() }; 5];
pub static F_FILTER: Cap = Cap::new();
pub static F_EMIT: Cap = Cap::new();
pub static F_FORMAT: Cap = Cap::new();
pub static F_FLUSH: Cap = Cap::new();
/// `Formatter.format` (what `fmt.format(record)` must resolve to).
pub static FMT_FORMAT: Cap = Cap::new();
/// `logging.LogRecord` (pinned), `logging._defaultFormatter` (pinned).
pub static LOG_RECORD: Cap = Cap::new();
pub static DEFAULT_FORMATTER: Cap = Cap::new();
/// `_thread.RLock` and its `acquire`/`release` method descriptors.
pub static RLOCK: Cap = Cap::new();
pub static RLOCK_ACQUIRE: Cap = Cap::new();
pub static RLOCK_RELEASE: Cap = Cap::new();
/// `_io.StringIO` and its `write`/`flush` method descriptors.
pub static STRINGIO: Cap = Cap::new();
pub static SIO_WRITE: Cap = Cap::new();
pub static SIO_FLUSH: Cap = Cap::new();

pub const H_FILTERS: usize = 0;
pub const H_LOCK: usize = 1;
pub const H_FORMATTER: usize = 2;
pub const H_STREAM: usize = 3;
pub const H_TERMINATOR: usize = 4;
pub const H_FILTER_K: usize = 5;
pub const H_EMIT_K: usize = 6;
pub const H_FORMAT_K: usize = 7;
pub const H_FLUSH_K: usize = 8;

/// Reads filters, lock, formatter, stream, terminator; checks filter, emit, format, flush
/// are not shadowed.
pub static HANDLERS: TypeCache<9> = TypeCache::new(validate_handler);
/// Formatters whose `format` is `Formatter.format` (checked not shadowed).
pub static HFORMATTERS: TypeCache<1> = TypeCache::new(validate_hformatter);

unsafe fn validate_handler(t: Tp) -> bool {
    // SAFETY: attached thread; live type.
    unsafe {
        let n = n();
        generic_attrs(t, false)
            && resolves_to(t, o(n.filter), F_FILTER.get())
            && resolves_to(t, o(n.emit), F_EMIT.get())
            && resolves_to(t, o(n.format), F_FORMAT.get())
            && resolves_to(t, o(n.flush), F_FLUSH.get())
            && [n.filters, n.lock, n.formatter, n.stream, n.terminator].iter().all(|&a| plain_attr(t, o(a)))
    }
}

unsafe fn validate_hformatter(t: Tp) -> bool {
    // SAFETY: attached thread; live type.
    unsafe { FORMATTERS.ok(t) && resolves_to(t, o(n().format), FMT_FORMAT.get()) && always_true(t) }
}

/// `bool(x)` is True without running code: no `__bool__`/`__len__` slot.
unsafe fn always_true(t: Tp) -> bool {
    // SAFETY: live type.
    unsafe {
        let nb = (*t).tp_as_number;
        let mp = (*t).tp_as_mapping;
        let sq = (*t).tp_as_sequence;
        (nb.is_null() || (*nb).nb_bool.is_none())
            && (mp.is_null() || (*mp).mp_length.is_none())
            && (sq.is_null() || (*sq).sq_length.is_none())
    }
}

/// Install-time check of the stream type: StringIO objects are always true.
///
/// # Safety
/// `t` is a live type object.
pub unsafe fn stringio_always_true(t: Tp) -> bool {
    // SAFETY: per the caller.
    unsafe { always_true(t) }
}

/// Call a captured C method descriptor with `self` and up to one argument: new reference or
/// NULL (error left set).
#[inline(always)]
unsafe fn call(descr: O, me: O, arg: Option<O>) -> O {
    // SAFETY: attached thread; live objects.
    unsafe {
        let a = [me, arg.unwrap_or(std::ptr::null_mut())];
        ffi::PyObject_Vectorcall(descr, a.as_ptr(), if arg.is_some() { 2 } else { 1 }, std::ptr::null_mut())
    }
}

/// `lock.acquire(False)`: true when taken (or re-entered by this thread). Never waits, so
/// no signal handler runs below the kernel.
#[inline(always)]
unsafe fn try_acquire(lock: O) -> bool {
    // SAFETY: an exact RLock.
    unsafe {
        let r = call(RLOCK_ACQUIRE.get(), lock, Some(ffi::Py_False()));
        if r.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        let ok = r == ffi::Py_True();
        ffi::Py_DECREF(r);
        ok
    }
}

/// `lock.release()` of a lock this thread holds (cannot fail).
#[inline(always)]
unsafe fn release(lock: O) {
    // SAFETY: an exact RLock owned by this thread.
    unsafe {
        let r = call(RLOCK_RELEASE.get(), lock, None);
        if r.is_null() {
            ffi::PyErr_Clear();
        } else {
            ffi::Py_DECREF(r);
        }
    }
}

/// The instance `__dict__` slot of a StringIO (C type with `tp_dictoffset`): NULL when the
/// object never had attributes set.
#[inline(always)]
unsafe fn has_instance_dict(stream: O) -> bool {
    // SAFETY: an exact StringIO (positive dictoffset checked at install).
    unsafe {
        let off = (*ffi::Py_TYPE(stream)).tp_dictoffset;
        if off <= 0 {
            return true; // unexpected layout: treat as "may have one"
        }
        let d = *stream.cast::<u8>().add(off as usize).cast::<O>();
        !d.is_null() && ffi::PyDict_Size(d) != 0
    }
}

enum Outcome {
    Done(O),
    Fallback(Reason),
}

unsafe fn handle(me: O, record: O) -> Outcome {
    // SAFETY: attached thread; validated handler type.
    unsafe {
        macro_rules! fb {
            ($r:expr) => {
                return Outcome::Fallback($r)
            };
        }
        let Some(he) = HANDLERS.entry(ffi::Py_TYPE(me)) else { fb!(Reason::Args) };
        let Some(re) = FMT_RECORDS.entry(ffi::Py_TYPE(record)) else { fb!(Reason::Args) };
        // `isinstance(rv, LogRecord)` and `if rv:` for the record Filterer.filter returns.
        if ffi::PyObject_TypeCheck(record, LOG_RECORD.get().cast()) == 0 || !always_true(ffi::Py_TYPE(record)) {
            fb!(Reason::Args);
        }
        if !HANDLERS.bound(he, me, H_FILTER_K, F_FILTER.get())
            || !HANDLERS.bound(he, me, H_EMIT_K, F_EMIT.get())
            || !HANDLERS.bound(he, me, H_FORMAT_K, F_FORMAT.get())
            || !HANDLERS.bound(he, me, H_FLUSH_K, F_FLUSH.get())
        {
            fb!(Reason::Args);
        }
        let filters = Owned(HANDLERS.read(he, me, H_FILTERS));
        if filters.0.is_null()
            || ffi::Py_TYPE(filters.0) != &raw mut ffi::PyList_Type
            || ffi::PyList_GET_SIZE(filters.0) != 0
        {
            fb!(Reason::Args);
        }
        let lock = Owned(HANDLERS.read(he, me, H_LOCK));
        if lock.0.is_null() || ffi::Py_TYPE(lock.0).cast() != RLOCK.get() {
            fb!(Reason::Args);
        }
        let stream = Owned(HANDLERS.read(he, me, H_STREAM));
        if stream.0.is_null() || ffi::Py_TYPE(stream.0).cast() != STRINGIO.get() || has_instance_dict(stream.0) {
            fb!(Reason::Args);
        }
        let term = Owned(HANDLERS.read(he, me, H_TERMINATOR));
        if term.0.is_null() || !exact_str(term.0) {
            fb!(Reason::Args);
        }
        // `fmt = self.formatter if self.formatter else _defaultFormatter`
        let fmt = Owned(HANDLERS.read(he, me, H_FORMATTER));
        if fmt.0.is_null() {
            fb!(Reason::Args);
        }
        let fmt = if fmt.0 == ffi::Py_None() { Owned(ffi::Py_NewRef(DEFAULT_FORMATTER.get())) } else { fmt };
        let Some(hfe) = HFORMATTERS.entry(ffi::Py_TYPE(fmt.0)) else { fb!(Reason::Args) };
        if !HFORMATTERS.bound(hfe, fmt.0, 0, FMT_FORMAT.get()) {
            fb!(Reason::Args);
        }
        let Some(fe) = FORMATTERS.entry(ffi::Py_TYPE(fmt.0)) else { fb!(Reason::Args) };
        if !format::plain_message(re, record) {
            fb!(Reason::Args);
        }
        // `with self.lock:` (observable from here: the lock, then the record's stores).
        if !try_acquire(lock.0) {
            fb!(Reason::Deopt);
        }
        let msg = match format::format(fe, fmt.0, re, record) {
            format::Fmt::Done(s) => Owned(s),
            format::Fmt::Fallback(r) => {
                release(lock.0);
                fb!(r);
            }
        };
        // stream.write(msg + self.terminator)
        let line = Owned(ffi::PyUnicode_Concat(msg.0, term.0));
        if line.0.is_null() {
            ffi::PyErr_Clear();
            release(lock.0);
            fb!(Reason::Deopt);
        }
        let w = call(SIO_WRITE.get(), stream.0, Some(line.0));
        if w.is_null() {
            // StringIO checks before it writes: nothing was written. CPython's emit reports
            // this through handleError from its own frames: restart there.
            ffi::PyErr_Clear();
            release(lock.0);
            fb!(Reason::Deopt);
        }
        ffi::Py_DECREF(w);
        // self.flush(): `with self.lock:` (re-entered), `self.stream.flush()`
        if try_acquire(lock.0) {
            let f = call(SIO_FLUSH.get(), stream.0, None);
            if f.is_null() {
                // Only a concurrent `IOBase.close(stream)` from another thread can make it
                // fail (no instance dict was there): that close is ordered after the flush.
                ffi::PyErr_Clear();
            } else {
                ffi::Py_DECREF(f);
            }
            release(lock.0);
        }
        release(lock.0);
        Outcome::Done(ffi::Py_NewRef(record))
    }
}

/// One owned reference (or NULL), released on drop.
struct Owned(O);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: owned or NULL.
        unsafe { ffi::Py_XDECREF(self.0) };
    }
}

/// `Handler.handle(self, record)`.
///
/// # Safety
/// Installed as the function's vectorcall: CPython calls it with vectorcall arguments.
pub unsafe extern "C" fn vc_handle(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let why = 'fb: {
            if !kw.is_null() || nargs(nargsf) != 2 {
                break 'fb Reason::Args;
            }
            if !DEPS.ok() || !SHARED_DEPS.ok() {
                break 'fb Reason::Deps;
            }
            if !TOOLS.quiet() || !fns_ok(&CODES) || !fns_ok(&format::FORMAT_CODES) {
                break 'fb Reason::Tools;
            }
            match handle(*args, *args.add(1)) {
                Outcome::Done(r) => {
                    STATS.hit_streak();
                    return r;
                }
                Outcome::Fallback(r) => r,
            }
        };
        STATS.fallback_streak(why);
        original(func, args, nargsf, kw)
    }
}
