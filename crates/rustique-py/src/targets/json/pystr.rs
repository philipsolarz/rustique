//! Reading PEP 393 `str` buffers directly and building result strings the way CPython's
//! own constructors do (same kind, same cached singletons).
//!
//! Never calls `PyUnicode_AsUTF8` on a caller's string (it would attach a UTF-8 cache,
//! contract §5.4). The `PyASCIIObject.state` bit layout differs between 3.14/3.15 and
//! GIL/free-threaded builds, so it is measured at install ([`calibrate`]); until it is,
//! the out-of-line `PyUnicode_KIND`/`PyUnicode_DATA` are used.

use std::sync::OnceLock;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyString;

use rustique_core::json::escape::Text;

use crate::targets::float::new_ascii;

#[derive(Clone, Copy, Debug)]
struct Layout {
    kind_shift: u32,
    compact_mask: u32,
    ascii_mask: u32,
    ascii_data: usize,
    compact_data: usize,
}

static LAYOUT: OnceLock<Option<Layout>> = OnceLock::new();

/// A borrowed view of a `str`'s code units.
#[derive(Clone, Copy, Debug)]
pub(crate) struct View {
    pub kind: u32,
    pub data: *const u8,
    pub len: usize,
    pub ascii: bool,
}

impl View {
    /// Largest code point the string can hold, made exact for ASCII vs Latin-1 (canonical
    /// strings of kind 2/4 always contain a code point that needs that width).
    #[inline(always)]
    pub fn max(&self) -> u32 {
        match self.kind {
            1 if self.ascii => 0x7F,
            1 => 0xFF,
            2 => 0xFFFF,
            _ => 0x10_FFFF,
        }
    }

    /// # Safety
    /// The string is alive and `kind == 1`.
    #[inline(always)]
    pub unsafe fn u8s<'a>(&self) -> &'a [u8] {
        // SAFETY: per the caller.
        unsafe { std::slice::from_raw_parts(self.data, self.len) }
    }

    /// # Safety
    /// The string is alive and `kind == 2`.
    #[inline(always)]
    pub unsafe fn u16s<'a>(&self) -> &'a [u16] {
        // SAFETY: per the caller; data is aligned for its kind.
        unsafe { std::slice::from_raw_parts(self.data.cast::<u16>(), self.len) }
    }

    /// # Safety
    /// The string is alive and `kind == 4`.
    #[inline(always)]
    pub unsafe fn u32s<'a>(&self) -> &'a [u32] {
        // SAFETY: per the caller; data is aligned for its kind.
        unsafe { std::slice::from_raw_parts(self.data.cast::<u32>(), self.len) }
    }
}

#[inline(always)]
unsafe fn state(op: *mut ffi::PyObject) -> u32 {
    // SAFETY: a str; the kind/compact/ascii bits never change after creation (volatile:
    // free-threaded builds update the `interned` bits of the same word atomically).
    unsafe { std::ptr::read_volatile(&raw const (*op.cast::<ffi::PyASCIIObject>()).state) }
}

/// View of a `str` (exact or subclass).
///
/// # Safety
/// `op` is a live `str` instance; attached thread.
#[inline(always)]
pub(crate) unsafe fn view(op: *mut ffi::PyObject) -> View {
    // SAFETY: per the caller; offsets verified by `calibrate`.
    unsafe {
        let len = ffi::PyUnicode_GET_LENGTH(op) as usize;
        if let Some(Some(l)) = LAYOUT.get() {
            let st = state(op);
            if st & l.compact_mask != 0 {
                let ascii = st & l.ascii_mask != 0;
                let off = if ascii { l.ascii_data } else { l.compact_data };
                return View { kind: (st >> l.kind_shift) & 7, data: op.cast::<u8>().add(off), len, ascii };
            }
        }
        let kind = ffi::PyUnicode_KIND(op);
        let data = ffi::PyUnicode_DATA(op).cast::<u8>().cast_const();
        let ascii = kind == 1 && std::slice::from_raw_parts(data, len).is_ascii();
        View { kind, data, len, ascii }
    }
}

/// Measure the state-bit layout on sample strings of every kind (`false` if anything
/// disagrees with `PyUnicode_KIND`/`PyUnicode_DATA`; [`view`] then stays on the slow path).
pub(crate) fn calibrate(py: Python<'_>) -> bool {
    LAYOUT
        .get_or_init(|| {
            let samples: Vec<Bound<'_, PyString>> =
                ["ascii-sample", "lat\u{e9}n-1", "ucs-\u{101}-2", "ucs-\u{1F600}-4", "x"]
                    .iter()
                    .map(|s| PyString::new(py, &format!("{s}{}", py.version_info().minor)))
                    .collect();
            // SAFETY: live str objects.
            let facts: Vec<(u32, u32, usize, bool)> = samples
                .iter()
                .map(|s| unsafe {
                    let p = s.as_ptr();
                    let data = ffi::PyUnicode_DATA(p) as usize - p as usize;
                    (state(p), ffi::PyUnicode_KIND(p), data, s.to_str().is_ok_and(|t| t.is_ascii()))
                })
                .collect();
            let kind_shift = (0..29).find(|&sh| facts.iter().all(|&(st, kind, _, _)| (st >> sh) & 7 == kind))?;
            let l = Layout {
                kind_shift,
                compact_mask: 1 << (kind_shift + 3),
                ascii_mask: 1 << (kind_shift + 4),
                ascii_data: size_of::<ffi::PyASCIIObject>(),
                compact_data: size_of::<ffi::PyCompactUnicodeObject>(),
            };
            let ok = facts.iter().all(|&(st, _, data, ascii)| {
                st & l.compact_mask != 0
                    && (st & l.ascii_mask != 0) == ascii
                    && data == if ascii { l.ascii_data } else { l.compact_data }
            });
            ok.then_some(l)
        })
        .is_some()
}

/// A string of `len` code points with largest code point `max`, filled by `fill` with
/// units of the resulting kind; one code point uses CPython's cached Latin-1 singletons
/// and zero the empty string, like `unicode_result`/`_PyUnicode_FromUCS*`.
///
/// # Safety
/// Attached thread; `max` is the exact largest code point (or its kind's bound when the
/// kind is already implied, see [`View::max`]).
#[inline]
unsafe fn build(
    len: usize,
    max: u32,
    first: impl FnOnce() -> u32,
    fill: impl FnOnce(*mut u8, u32),
) -> *mut ffi::PyObject {
    // SAFETY: per the caller.
    unsafe {
        if len == 1 {
            return ffi::PyUnicode_FromOrdinal(first() as std::ffi::c_int);
        }
        let s = ffi::PyUnicode_New(len as ffi::Py_ssize_t, max);
        if s.is_null() || len == 0 {
            return s;
        }
        let kind = ffi::PyUnicode_KIND(s);
        fill(ffi::PyUnicode_DATA(s).cast::<u8>(), kind);
        s
    }
}

/// The canonical `str` of `text` (`PyUnicodeWriter_Finish` of the same content).
///
/// # Safety
/// Attached thread; ASCII layout calibrated (`float::ascii_layout_ok`).
pub(crate) unsafe fn from_text(text: &Text) -> *mut ffi::PyObject {
    // SAFETY: per the caller; fills write exactly `len` units of the new string's kind.
    unsafe {
        match text {
            Text::K1(v, m) if *m <= 0x7F => new_ascii(v),
            Text::K1(v, m) => {
                build(v.len(), *m, || u32::from(v[0]), |d, _| std::ptr::copy_nonoverlapping(v.as_ptr(), d, v.len()))
            }
            Text::K2(v) => build(
                v.len(),
                0xFFFF,
                || u32::from(v[0]),
                |d, _| std::ptr::copy_nonoverlapping(v.as_ptr(), d.cast::<u16>(), v.len()),
            ),
            Text::K4(v) => build(
                v.len(),
                0x10_FFFF,
                || v[0],
                |d, _| std::ptr::copy_nonoverlapping(v.as_ptr(), d.cast::<u32>(), v.len()),
            ),
        }
    }
}

/// The `str` of decoded code points with exact maximum `max` (what `scanstring_unicode`'s
/// writer returns for an escaped string).
///
/// # Safety
/// Attached thread.
#[allow(dead_code)] // json.loads
pub(crate) unsafe fn from_code_points(cps: &[u32], max: u32) -> *mut ffi::PyObject {
    // SAFETY: per the caller; each code point fits the kind chosen for `max`.
    unsafe {
        build(
            cps.len(),
            max,
            || cps[0],
            |d, kind| match kind {
                1 => {
                    for (i, &c) in cps.iter().enumerate() {
                        *d.add(i) = c as u8;
                    }
                }
                2 => {
                    let d = d.cast::<u16>();
                    for (i, &c) in cps.iter().enumerate() {
                        *d.add(i) = c as u16;
                    }
                }
                _ => std::ptr::copy_nonoverlapping(cps.as_ptr(), d.cast::<u32>(), cps.len()),
            },
        )
    }
}

/// `PyUnicode_Substring(src, start, end)` for a source of the given view: an ASCII
/// source gives `_PyUnicode_FromASCII`, any other `PyUnicode_FromKindAndData` (which
/// narrows to the canonical kind); `""` and one Latin-1 character are singletons.
///
/// # Safety
/// Attached thread; `src` alive, `start <= end <= src.len`.
#[inline]
#[allow(dead_code)] // json.loads
pub(crate) unsafe fn substring(src: &View, start: usize, end: usize) -> *mut ffi::PyObject {
    // SAFETY: per the caller.
    unsafe {
        if src.ascii {
            return new_ascii(std::slice::from_raw_parts(src.data.add(start), end - start));
        }
        let k = src.kind as usize;
        ffi::PyUnicode_FromKindAndData(
            src.kind as std::ffi::c_int,
            src.data.add(start * k).cast(),
            (end - start) as ffi::Py_ssize_t,
        )
    }
}
