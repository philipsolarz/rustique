//! Runtime shared by the `posixpath`, `fnmatch` and `pathlib` groups: inline access to
//! `str`/`bytes` buffers, result builders that keep CPython's identity rules, and the
//! per-target vectorcall plumbing (guards, counters, fallback) on top of `crate::accel`.

use std::ffi::c_void;
use std::sync::OnceLock;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple};

use crate::accel::{self, Counters, DepFlag, Fingerprints, Prepare, Reason, Refusal, Tools};
use crate::refuse;
pub use rustique_core::paths::Unit;

pub type O = *mut ffi::PyObject;

// ---------------------------------------------------------------------------------------
// str / bytes buffers
// ---------------------------------------------------------------------------------------

/// Where the kind/compact/ascii bits of `PyASCIIObject.state` live and where compact data
/// starts (measured at install against `PyUnicode_KIND`/`PyUnicode_DATA`, which are
/// out-of-line calls on 3.14+; same method as `takeover.rs`).
#[derive(Clone, Copy, Debug)]
struct StrLayout {
    kind_shift: u32,
    compact_mask: u32,
    ascii_mask: u32,
    ascii_data: usize,
    compact_data: usize,
}

static STR_LAYOUT: OnceLock<Option<StrLayout>> = OnceLock::new();

fn calibrate_str(py: Python<'_>) -> Option<StrLayout> {
    let samples: Vec<Bound<'_, PyString>> = ["ascii-sample", "lat\u{e9}n-1", "ucs-\u{101}-2", "ucs-\u{1F600}-4", "x"]
        .iter()
        .map(|s| PyString::new(py, &format!("{s}{}", py.version_info().minor)))
        .collect();
    // SAFETY: exact str objects kept alive by `samples`.
    let facts: Vec<(u32, u32, usize, bool)> = samples
        .iter()
        .map(|s| unsafe {
            let p = s.as_ptr();
            let data = ffi::PyUnicode_DATA(p) as usize - p as usize;
            (str_state(p), ffi::PyUnicode_KIND(p), data, s.to_str().is_ok_and(|t| t.is_ascii()))
        })
        .collect();
    let kind_shift = (0..29).find(|&sh| facts.iter().all(|&(st, kind, _, _)| (st >> sh) & 7 == kind))?;
    let layout = StrLayout {
        kind_shift,
        compact_mask: 1 << (kind_shift + 3),
        ascii_mask: 1 << (kind_shift + 4),
        ascii_data: size_of::<ffi::PyASCIIObject>(),
        compact_data: size_of::<ffi::PyCompactUnicodeObject>(),
    };
    let ok = facts.iter().all(|&(st, _, data, ascii)| {
        st & layout.compact_mask != 0
            && (st & layout.ascii_mask != 0) == ascii
            && data == if ascii { layout.ascii_data } else { layout.compact_data }
    });
    ok.then_some(layout)
}

/// Calibrate once; true if the inline layout is usable (otherwise the out-of-line API is).
pub fn init_str_layout(py: Python<'_>) -> bool {
    STR_LAYOUT.get_or_init(|| calibrate_str(py)).is_some()
}

/// # Safety
/// `op` is a `str` (exact or subclass).
#[inline(always)]
unsafe fn str_state(op: O) -> u32 {
    // SAFETY: the kind/compact/ascii bits never change after creation; volatile because
    // free-threaded builds update the `interned` bits of the same word atomically.
    unsafe { std::ptr::read_volatile(&raw const (*op.cast::<ffi::PyASCIIObject>()).state) }
}

/// The code units of a `str`.
#[derive(Clone, Copy, Debug)]
pub enum Text<'a> {
    U1(&'a [u8]),
    U2(&'a [u16]),
    U4(&'a [u32]),
}

/// A `str` buffer and its `PyUnicode_MAX_CHAR_VALUE` class.
#[derive(Clone, Copy, Debug)]
pub struct StrView<'a> {
    pub text: Text<'a>,
    pub maxchar: u32,
}

impl StrView<'_> {
    #[inline(always)]
    pub fn len(&self) -> usize {
        match self.text {
            Text::U1(s) => s.len(),
            Text::U2(s) => s.len(),
            Text::U4(s) => s.len(),
        }
    }
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Run a generic `fn(&[T]) -> R` on a [`Text`].
#[macro_export]
macro_rules! with_text {
    ($t:expr, |$s:ident| $body:expr) => {
        match $t {
            $crate::targets::posixpath::rt::Text::U1($s) => $body,
            $crate::targets::posixpath::rt::Text::U2($s) => $body,
            $crate::targets::posixpath::rt::Text::U4($s) => $body,
        }
    };
}

/// View of a `str` (exact or subclass) buffer.
///
/// # Safety
/// `op` is a live `str`; the view must not outlive it.
#[inline(always)]
pub unsafe fn str_view<'a>(op: O) -> StrView<'a> {
    // SAFETY: per the caller; offsets verified by `calibrate_str`.
    unsafe {
        let len = (*op.cast::<ffi::PyASCIIObject>()).length as usize;
        let (kind, data, ascii) = match STR_LAYOUT.get() {
            Some(Some(l)) => {
                let st = str_state(op);
                if st & l.compact_mask != 0 {
                    let ascii = st & l.ascii_mask != 0;
                    let off = if ascii { l.ascii_data } else { l.compact_data };
                    ((st >> l.kind_shift) & 7, op.cast::<u8>().add(off) as *const c_void, ascii)
                } else {
                    let ascii = st & l.ascii_mask != 0;
                    ((st >> l.kind_shift) & 7, ffi::PyUnicode_DATA(op) as *const c_void, ascii)
                }
            }
            _ => {
                let kind = ffi::PyUnicode_KIND(op);
                let data = ffi::PyUnicode_DATA(op) as *const c_void;
                let ascii = kind == 1 && std::slice::from_raw_parts(data.cast::<u8>(), len).is_ascii();
                (kind, data, ascii)
            }
        };
        match kind {
            1 => StrView {
                text: Text::U1(std::slice::from_raw_parts(data.cast(), len)),
                maxchar: if ascii { 0x7f } else { 0xff },
            },
            2 => StrView { text: Text::U2(std::slice::from_raw_parts(data.cast(), len)), maxchar: 0xffff },
            _ => StrView { text: Text::U4(std::slice::from_raw_parts(data.cast(), len)), maxchar: 0x10ffff },
        }
    }
}

/// The bytes of a `bytes` (exact or subclass).
///
/// # Safety
/// `op` is a live `bytes`; the slice must not outlive it.
#[inline(always)]
pub unsafe fn bytes_view<'a>(op: O) -> &'a [u8] {
    // SAFETY: per the caller.
    unsafe {
        let n = ffi::Py_SIZE(op) as usize;
        std::slice::from_raw_parts(ffi::PyBytes_AS_STRING(op).cast::<u8>(), n)
    }
}

#[inline(always)]
pub unsafe fn exact_str(o: O) -> bool {
    // SAFETY: live object.
    unsafe { ffi::Py_TYPE(o) == &raw mut ffi::PyUnicode_Type }
}

#[inline(always)]
pub unsafe fn exact_bytes(o: O) -> bool {
    // SAFETY: live object.
    unsafe { ffi::Py_TYPE(o) == &raw mut ffi::PyBytes_Type }
}

/// `p[start:end]` of a `str`, exactly as slicing does it (`PyUnicode_Substring`: the
/// object itself for the whole of an exact str, the empty and latin-1 singletons).
///
/// # Safety
/// `op` is a live `str`, `start <= end <= len`.
#[inline(always)]
pub unsafe fn substr(op: O, start: usize, end: usize) -> O {
    // SAFETY: per the caller.
    unsafe { ffi::PyUnicode_Substring(op, start as ffi::Py_ssize_t, end as ffi::Py_ssize_t) }
}

/// `p[start:end]` of a `bytes`, as `bytes_subscript` does it (the object itself for the
/// whole of an exact bytes; `PyBytes_FromStringAndSize` otherwise, whose empty and
/// one-byte results are singletons).
///
/// # Safety
/// `op` is a live `bytes`, `start <= end <= len`.
#[inline(always)]
pub unsafe fn subbytes(op: O, start: usize, end: usize) -> O {
    // SAFETY: per the caller.
    unsafe {
        let s = bytes_view(op);
        if start == 0 && end == s.len() && exact_bytes(op) {
            return ffi::Py_NewRef(op);
        }
        ffi::PyBytes_FromStringAndSize(s.as_ptr().add(start).cast(), (end - start) as ffi::Py_ssize_t)
    }
}

/// Copy code units into the buffer of a fresh string of `kind` at `at`.
///
/// # Safety
/// `dst` points to a buffer of the right kind with room for `at + src.len()` units, and
/// every code point of `src` fits `kind`.
#[inline(always)]
pub unsafe fn write_units<S: Unit>(dst: *mut c_void, kind: u32, at: usize, src: &[S]) {
    // SAFETY: per the caller; same-size copies are plain memcpy.
    unsafe {
        let n = src.len();
        match kind {
            1 if size_of::<S>() == 1 => std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u8>(), dst.cast::<u8>().add(at), n),
            2 if size_of::<S>() == 2 => std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u16>(), dst.cast::<u16>().add(at), n),
            4 if size_of::<S>() == 4 => std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u32>(), dst.cast::<u32>().add(at), n),
            1 => {
                let d = dst.cast::<u8>().add(at);
                for (k, &c) in src.iter().enumerate() {
                    *d.add(k) = c.cp() as u8;
                }
            }
            2 => {
                let d = dst.cast::<u16>().add(at);
                for (k, &c) in src.iter().enumerate() {
                    *d.add(k) = c.cp() as u16;
                }
            }
            _ => {
                let d = dst.cast::<u32>().add(at);
                for (k, &c) in src.iter().enumerate() {
                    *d.add(k) = c.cp();
                }
            }
        }
    }
}

/// [`write_units`] for a [`Text`].
///
/// # Safety
/// As [`write_units`].
#[inline(always)]
pub unsafe fn write_text(dst: *mut c_void, kind: u32, at: usize, src: Text<'_>) {
    // SAFETY: forwarded.
    unsafe {
        match src {
            Text::U1(s) => write_units(dst, kind, at, s),
            Text::U2(s) => write_units(dst, kind, at, s),
            Text::U4(s) => write_units(dst, kind, at, s),
        }
    }
}

/// A fresh, uninitialised `str` of `len` code points whose kind and ascii flag follow
/// `maxchar` (as `PyUnicode_New`): `(object, data, kind)`; null object on MemoryError.
///
/// # Safety
/// Attached thread. The caller fills every code point before exposing the object.
#[inline(always)]
pub unsafe fn new_str(len: usize, maxchar: u32) -> (O, *mut c_void, u32) {
    // SAFETY: attached thread.
    unsafe {
        let o = ffi::PyUnicode_New(len as ffi::Py_ssize_t, maxchar);
        if o.is_null() {
            return (o, std::ptr::null_mut(), 0);
        }
        let v = str_view(o);
        let (data, kind) = match v.text {
            Text::U1(s) => (s.as_ptr() as *mut c_void, 1),
            Text::U2(s) => (s.as_ptr() as *mut c_void, 2),
            Text::U4(s) => (s.as_ptr() as *mut c_void, 4),
        };
        (o, data, kind)
    }
}

/// A `str` from code points that are not all ASCII-known in advance: the kind follows the
/// actual maximum (as `PyUnicode_FromKindAndData`/`_PyUnicode_FromUCS*`), so one-character
/// latin-1 results are the shared singletons.
///
/// # Safety
/// Attached thread.
pub unsafe fn str_from_units<T: Unit>(units: &[T]) -> O {
    // SAFETY: attached thread; the buffer has the size passed.
    unsafe {
        let kind = match size_of::<T>() {
            1 => ffi::PyUnicode_1BYTE_KIND,
            2 => ffi::PyUnicode_2BYTE_KIND,
            _ => ffi::PyUnicode_4BYTE_KIND,
        };
        ffi::PyUnicode_FromKindAndData(kind as std::ffi::c_int, units.as_ptr().cast(), units.len() as ffi::Py_ssize_t)
    }
}

// ---------------------------------------------------------------------------------------
// Per-target run-time state, guards and fallback
// ---------------------------------------------------------------------------------------

/// Run-time state of one vectorcall target, read by its kernel.
pub struct Rt {
    pub counters: Counters,
    pub tools: Tools,
    pub deps: DepFlag,
}

impl Rt {
    pub const fn new() -> Self {
        Rt { counters: Counters::new(), tools: Tools::new(), deps: DepFlag::new() }
    }

    /// Call the original (`_PyFunction_Vectorcall`) with the unchanged arguments.
    ///
    /// # Safety
    /// Vectorcall arguments as received by the kernel.
    #[inline(always)]
    pub unsafe fn fallback(&self, why: Reason, func: O, args: *const O, nargsf: usize, kw: O) -> O {
        self.counters.fallback(why);
        // SAFETY: set by the registry before any kernel is reachable.
        unsafe { accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kw) }
    }

    /// Dependencies intact and no tool listening (globally or on this function's code).
    ///
    /// # Safety
    /// Attached thread; `func` is the patched function.
    #[inline(always)]
    pub unsafe fn ready(&self, func: O) -> Result<(), Reason> {
        if !self.deps.ok() {
            return Err(Reason::Deps);
        }
        // SAFETY: attached thread; `func` is a function object.
        if unsafe { !accel::tools_quiet(&self.tools, func) } {
            return Err(Reason::Tools);
        }
        Ok(())
    }
}

impl Default for Rt {
    fn default() -> Self {
        Self::new()
    }
}

#[inline(always)]
pub fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

/// Strong reference kept for the life of the process (constants returned by identity).
pub fn leak(o: &Bound<'_, PyAny>) -> usize {
    o.clone().into_ptr() as usize
}

/// The constant of `func.__code__.co_consts` equal to `value` with exactly `value`'s type.
pub fn code_const<'py>(func: &Bound<'py, PyAny>, value: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyAny>>> {
    let consts = func.getattr("__code__")?.getattr("co_consts")?;
    for c in consts.cast_into::<PyTuple>()?.iter() {
        if c.get_type().is(&value.get_type()) && c.eq(value)? {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// Require the recorded fingerprint of `obj` (named `name` in `table`).
pub fn check_fp(
    cx: &mut Prepare<'_>,
    table: &[(&str, Fingerprints)],
    name: &str,
    obj: &Bound<'_, PyAny>,
) -> Result<(), Refusal> {
    let fps = table.iter().find(|(n, _)| *n == name).map(|(_, f)| *f).ok_or_else(|| refuse!("{name}: no fingerprint table"))?;
    cx.fingerprint_code(obj, fps)
}

/// The module dict of an already imported module (never imports).
pub fn module_dict<'py>(cx: &Prepare<'py>, name: &str) -> Result<Bound<'py, PyDict>, Refusal> {
    Ok(cx.loaded_module(name)?.dict())
}

/// Require the recorded `ml_name`/`ml_flags`/`ml_doc` fingerprint of a C function.
pub fn check_c(
    cx: &mut Prepare<'_>,
    table: &[(&str, Fingerprints)],
    name: &str,
    obj: &Bound<'_, PyAny>,
) -> Result<(), Refusal> {
    let fps = table.iter().find(|(n, _)| *n == name).map(|(_, f)| *f).ok_or_else(|| refuse!("{name}: no fingerprint table"))?;
    cx.fingerprint_c(obj, fps)
}
