//! Reading a caller's `str` without side effects (no `PyUnicode_AsUTF8`: it would attach a
//! UTF-8 cache to non-ASCII strings). Compact ASCII strings are read inline through the
//! `state` bits, calibrated at install against `PyUnicode_KIND`/`PyUnicode_DATA`.

use std::sync::atomic::{AtomicU32, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;

/// `compact | ascii` bits of `PyASCIIObject.state`; 0 until calibrated.
static MASK: AtomicU32 = AtomicU32::new(0);

#[cfg(not(Py_GIL_DISABLED))]
const CANDIDATE: u32 = (1 << 5) | (1 << 6);
#[cfg(Py_GIL_DISABLED)]
const CANDIDATE: u32 = (1 << 11) | (1 << 12);

/// Check the candidate bits on sample strings (built at run time: compact, not static).
pub fn calibrate(py: Python<'_>) -> bool {
    let samples = [("rustique-ascii", true), ("caf\u{e9}-latin", false), ("\u{3b1}\u{3b2}-wide", false), ("\u{1F600}", false)];
    let ok = samples.iter().all(|&(text, ascii)| {
        let s = pyo3::types::PyString::new(py, &format!("{text}{}", py.version_info().minor));
        let p = s.as_ptr();
        // SAFETY: a live str.
        unsafe {
            let st = (*p.cast::<ffi::PyASCIIObject>()).state;
            let fast = st & CANDIDATE == CANDIDATE;
            fast == ascii
                && (!ascii
                    || (ffi::PyUnicode_KIND(p) == 1
                        && ffi::PyUnicode_DATA(p) == p.cast::<ffi::PyASCIIObject>().add(1).cast()))
        }
    });
    if ok {
        MASK.store(CANDIDATE, Ordering::Relaxed);
    }
    ok
}

/// The bytes of a string whose characters are all ASCII.
///
/// # Safety
/// `s` is a live `str`; the slice must not outlive it.
#[inline(always)]
pub unsafe fn ascii<'a>(s: *mut ffi::PyObject) -> Option<&'a [u8]> {
    // SAFETY: per the caller; the inline path only after calibration.
    unsafe {
        let m = MASK.load(Ordering::Relaxed);
        let n = ffi::PyUnicode_GET_LENGTH(s) as usize;
        if m != 0 {
            let st = std::ptr::read_volatile(&raw const (*s.cast::<ffi::PyASCIIObject>()).state);
            if st & m == m {
                return Some(std::slice::from_raw_parts(s.cast::<ffi::PyASCIIObject>().add(1).cast::<u8>(), n));
            }
        }
        slow(s, n)
    }
}

#[cold]
unsafe fn slow<'a>(s: *mut ffi::PyObject, n: usize) -> Option<&'a [u8]> {
    // SAFETY: per `ascii`.
    unsafe {
        if ffi::PyUnicode_KIND(s) != 1 {
            return None;
        }
        let b = std::slice::from_raw_parts(ffi::PyUnicode_DATA(s).cast::<u8>(), n);
        b.is_ascii().then_some(b)
    }
}
