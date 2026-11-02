//! Reading a caller's `str` without side effects (no `PyUnicode_AsUTF8`: it would attach a
//! UTF-8 cache to non-ASCII strings). Compact ASCII strings are read inline through the
//! `state` bits, calibrated at install against `PyUnicode_KIND`/`PyUnicode_DATA`.

use std::sync::atomic::{AtomicU32, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;

/// `compact | ascii` bits of `PyASCIIObject.state`; 0 until calibrated.
static MASK: AtomicU32 = AtomicU32::new(0);

/// The `PyASCIIObject.state` bitfield, read the same way on every ABI (w2-dx §6.3).
/// CPython 3.14/3.15 declare `interned` as its own `unsigned char` on free-threaded builds,
/// then `kind:3 compact:1 ascii:1 ...`. GCC/Clang (SysV) pack the bitfields into the same
/// 32-bit unit as that byte (kind at bit 8 on free-threaded builds, bit 2 otherwise); MSVC
/// starts a new unit when the declared type changes, so on free-threaded Windows builds the
/// first word holds only `interned` and three never-written padding bytes, and the
/// bitfields live in the next 32-bit unit (kind at bit 0). Reading the right unit keeps
/// the fixed mask off uninitialised padding; [`calibrate`] still verifies the bits.
const UNIT_OFFSET: usize = if cfg!(all(windows, Py_GIL_DISABLED)) { 4 } else { 0 };

/// `compact | ascii` in that unit.
const CANDIDATE: u32 = if cfg!(all(windows, Py_GIL_DISABLED)) {
    (1 << 3) | (1 << 4)
} else if cfg!(Py_GIL_DISABLED) {
    (1 << 11) | (1 << 12)
} else {
    (1 << 5) | (1 << 6)
};

const _: () = assert!(
    std::mem::offset_of!(ffi::PyASCIIObject, state) + UNIT_OFFSET + 4 <= std::mem::size_of::<ffi::PyASCIIObject>()
);

/// The 32-bit unit of `op`'s state that holds kind/compact/ascii.
///
/// # Safety
/// `op` is a live `str`. Volatile: free-threaded builds update the `interned` byte
/// atomically (same unit on Unix); the other bits never change after creation.
#[inline(always)]
unsafe fn state_word(op: *mut ffi::PyObject) -> u32 {
    // SAFETY: per the caller; the unit lies inside the PyASCIIObject header on every ABI.
    unsafe {
        let p = (&raw const (*op.cast::<ffi::PyASCIIObject>()).state).cast::<u8>().add(UNIT_OFFSET);
        std::ptr::read_volatile(p.cast::<u32>())
    }
}

/// Check the candidate bits on sample strings (built at run time: compact, not static).
pub fn calibrate(py: Python<'_>) -> bool {
    if MASK.load(Ordering::Relaxed) != 0 {
        return true;
    }
    // Fresh (compact, not interned) strings: ASCII, Latin-1 and UCS-2.
    let ascii = pyo3::types::PyString::new(py, "rustique ascii probe");
    let latin = pyo3::types::PyString::new(py, "caf\u{e9} probe");
    let wide = pyo3::types::PyString::new(py, "\u{20ac} probe");
    // SAFETY: live str objects.
    let ok = unsafe {
        let (a, l, w) = (ascii.as_ptr(), latin.as_ptr(), wide.as_ptr());
        state_word(a) & CANDIDATE == CANDIDATE
            && state_word(l) & CANDIDATE != CANDIDATE
            && state_word(w) & CANDIDATE != CANDIDATE
            && ffi::PyUnicode_KIND(a) == 1
            && ffi::PyUnicode_DATA(a) == a.cast::<ffi::PyASCIIObject>().add(1).cast()
    };
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
            if state_word(s) & m == m {
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
