//! Reading a caller's `str` without side effects (no `PyUnicode_AsUTF8`: it would attach a
//! UTF-8 cache to non-ASCII strings).

use pyo3::ffi;

/// The bytes of a 1-byte-kind string whose characters are all ASCII.
///
/// # Safety
/// `s` is a live `str`; the slice must not outlive it.
#[inline]
pub unsafe fn ascii<'a>(s: *mut ffi::PyObject) -> Option<&'a [u8]> {
    // SAFETY: per the caller.
    unsafe {
        if ffi::PyUnicode_KIND(s) != 1 {
            return None;
        }
        let n = ffi::PyUnicode_GET_LENGTH(s) as usize;
        let b = std::slice::from_raw_parts(ffi::PyUnicode_DATA(s).cast::<u8>(), n);
        b.is_ascii().then_some(b)
    }
}
