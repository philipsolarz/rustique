//! Reading exact `int`s without a call (used by the datetime kernels).

use pyo3::ffi;

/// The value of an `int` whose magnitude fits one 30-bit digit, read from its digits
/// (CPython 3.12+ layout: `lv_tag` holds the sign in bits 0-1 and the digit count from
/// bit 3); `None` for larger ints.
///
/// # Safety
/// `p` must point to a live `int` (exact or subclass).
#[inline(always)]
pub unsafe fn compact_int_value(p: *mut ffi::PyObject) -> Option<i64> {
    // SAFETY: see above.
    unsafe {
        let value = p.cast::<u8>().add(size_of::<ffi::PyObject>()).cast::<usize>();
        let tag = *value;
        if tag >= (2 << 3) {
            return None;
        }
        let sign = 1 - (tag & 3) as i64;
        Some(sign * i64::from(*value.add(1).cast::<u32>()))
    }
}

/// An exact `int` as `i64` (compact ints read inline), `None` on overflow.
///
/// # Safety
/// `p` must point to a live exact `int`.
#[inline(always)]
pub unsafe fn exact_int_i64(p: *mut ffi::PyObject) -> Option<i64> {
    // SAFETY: see above.
    unsafe {
        compact_int_value(p).or_else(|| {
            let mut overflow = 0;
            let v = ffi::PyLong_AsLongLongAndOverflow(p, &mut overflow);
            (overflow == 0).then_some(v)
        })
    }
}
