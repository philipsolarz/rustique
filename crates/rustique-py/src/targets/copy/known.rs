//! Standard-library C types whose copy protocol is C code that calls back into nothing:
//! `_datetime`'s `date`, `datetime`, `time`, `timedelta`, `timezone`, and
//! `_decimal.Decimal` (whose `__copy__`/`__deepcopy__` return `self`).
//!
//! Resolved lazily, never by importing: a type is recognised once its `tp_name` matches and
//! it is the very object `sys.modules["_datetime" | "_decimal"].<name>`, and it is immutable
//! (static, or a heap type with `Py_TPFLAGS_IMMUTABLETYPE`), so its methods cannot be
//! replaced afterwards. Exact types only: a subclass may be user code.

use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::Relaxed;

use pyo3::ffi;

use super::rt::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Known {
    Date,
    DateTime,
    Time,
    TimeDelta,
    TimeZone,
    Decimal,
}

const KINDS: [(Known, &[u8], &std::ffi::CStr, &std::ffi::CStr); 6] = [
    (Known::Date, b"datetime.date", c"_datetime", c"date"),
    (Known::DateTime, b"datetime.datetime", c"_datetime", c"datetime"),
    (Known::Time, b"datetime.time", c"_datetime", c"time"),
    (Known::TimeDelta, b"datetime.timedelta", c"_datetime", c"timedelta"),
    (Known::TimeZone, b"datetime.timezone", c"_datetime", c"timezone"),
    (Known::Decimal, b"decimal.Decimal", c"_decimal", c"Decimal"),
];

static TYPES: [AtomicPtr<ffi::PyTypeObject>; 6] = [const { AtomicPtr::new(std::ptr::null_mut()) }; 6];

/// The kind of `cls` if it is one of the recognised types.
#[inline(always)]
pub unsafe fn known(cls: *mut ffi::PyTypeObject) -> Option<Known> {
    for (i, t) in TYPES.iter().enumerate() {
        if t.load(Relaxed) == cls {
            return Some(KINDS[i].0);
        }
    }
    // SAFETY: a live type.
    unsafe { resolve(cls) }
}

#[cold]
#[inline(never)]
unsafe fn resolve(cls: *mut ffi::PyTypeObject) -> Option<Known> {
    // SAFETY: a live type object; pure dict lookups (no import, no attribute hooks).
    unsafe {
        let name = std::ffi::CStr::from_ptr((*cls).tp_name).to_bytes();
        let i = KINDS.iter().position(|k| k.1 == name)?;
        let flags = ffi::PyType_GetFlags(cls);
        if flags & ffi::Py_TPFLAGS_HEAPTYPE != 0 && flags & ffi::Py_TPFLAGS_IMMUTABLETYPE == 0 {
            return None;
        }
        let modules = ffi::PyImport_GetModuleDict();
        if modules.is_null() || ty(modules) != &raw mut ffi::PyDict_Type {
            return None;
        }
        let key = Own::new(ffi::PyUnicode_FromString(KINDS[i].2.as_ptr())).ok()?;
        let m = Own(dict_get(modules, key.get()).ok()?);
        if m.get().is_null() || ty(m.get()) != &raw mut ffi::PyModule_Type {
            ffi::PyErr_Clear();
            return None;
        }
        let md = ffi::PyModule_GetDict(m.get());
        let attr = Own::new(ffi::PyUnicode_FromString(KINDS[i].3.as_ptr())).ok()?;
        let a = Own(dict_get(md, attr.get()).ok()?);
        if a.get() != cls.cast() {
            ffi::PyErr_Clear();
            return None;
        }
        if TYPES[i].compare_exchange(std::ptr::null_mut(), cls, Relaxed, Relaxed).is_ok() {
            // Kept for the process (a heap type must outlive the cached pointer).
            incref(cls.cast());
            Some(KINDS[i].0)
        } else if TYPES[i].load(Relaxed) == cls {
            Some(KINDS[i].0)
        } else {
            None // another interpreter's module instance
        }
    }
}

/// `x.__reduce_ex__(4)` of a recognised `_datetime` object (`cls` exact): CPython's own C
/// method for `date`/`datetime`/`time`/`timedelta`; for `timezone`, `tzinfo.__reduce__`
/// emulated — `(cls, x.__getinitargs__(), None)` — because CPython's runs
/// `copyreg._slotnames(timezone)` (a Python frame, failing to cache on the static type):
/// `slot_frame` says that frame is within the recursion limit and `copyreg._slotnames` is
/// intact. Returns the reduce value, validated to be `(cls, args_tuple[, None])`; `Ok(None)`
/// when it cannot be produced purely.
pub unsafe fn datetime_reduce(st: &St, x: O, cls: *mut ffi::PyTypeObject, k: Known, slot_frame: bool) -> R<Option<O>> {
    // SAFETY: live objects; C methods of immutable `_datetime` types.
    unsafe {
        let rv = match k {
            Known::Date | Known::DateTime | Known::Time | Known::TimeDelta => {
                let four = Own::new(ffi::PyLong_FromLong(4))?;
                Own::new(ffi::PyObject_CallMethodOneArg(x, st.s_reduce_ex, four.get()))?
            }
            Known::TimeZone => {
                if !slot_frame {
                    return Ok(None);
                }
                let args = Own::new(ffi::PyObject_CallMethodNoArgs(x, st.s_getinitargs))?;
                Own::new(ffi::PyTuple_Pack(3, cls.cast::<ffi::PyObject>(), args.get(), ffi::Py_None()))?
            }
            Known::Decimal => return Ok(None),
        };
        let r = rv.get();
        if ty(r) != &raw mut ffi::PyTuple_Type {
            return Ok(None);
        }
        let n = ffi::PyTuple_GET_SIZE(r);
        if !(2..=3).contains(&n)
            || ffi::PyTuple_GET_ITEM(r, 0) != cls.cast()
            || ty(ffi::PyTuple_GET_ITEM(r, 1)) != &raw mut ffi::PyTuple_Type
            || (n == 3 && ffi::PyTuple_GET_ITEM(r, 2) != ffi::Py_None())
        {
            return Ok(None);
        }
        Ok(Some(rv.into_raw()))
    }
}
