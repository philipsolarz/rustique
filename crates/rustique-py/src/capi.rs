//! The C-API capsule `rustique._rustique._C_API` (ARCHITECTURE §5, "Rust interop").
//!
//! A separate extension module cannot call [`List::from_column`]: linking this crate as an
//! rlib gives it a *second* copy of the `List` type object, not `rustique.List`. Instead,
//! module initialisation publishes a versioned `#[repr(C)]` function table as a capsule
//! (the way NumPy publishes `_ARRAY_API`). Other extensions import it with
//! `PyCapsule_Import`; the `rustique-api` crate wraps it in a safe Rust API.
//!
//! ABI rules (also the contract for `rustique-api` and any C consumer):
//! - fields are only ever *appended*; each addition bumps [`API_VERSION`] and grows
//!   `abi_size`. Existing fields never move, change type or change meaning;
//! - [`DTypeCode`] values are never renumbered or reused;
//! - an incompatible change needs a new capsule name.
//!
//! Every entry point must be called by a thread attached to the interpreter (holding the
//! GIL on GIL builds). Errors return NULL or -1 with a Python exception set; Rust panics are
//! converted to `PanicException` and never unwind into the caller.

use std::any::Any;
use std::ffi::{CStr, c_int, c_void};
use std::mem::{ManuallyDrop, offset_of};
use std::panic::{self, AssertUnwindSafe};
use std::ptr::{self, NonNull};

use pyo3::exceptions::{PyMemoryError, PyTypeError, PyValueError};
use pyo3::ffi;
use pyo3::panic::PanicException;
use pyo3::prelude::*;
use pyo3::types::PyCapsule;
use rustique_core::{Column, CompactString, DType};

use crate::list::List;

/// Full name of the capsule: module path + attribute, as `PyCapsule_Import` requires.
pub const CAPSULE_NAME: &CStr = c"rustique._rustique._C_API";

/// Version of the function table. Version `n` contains every field of versions `< n`.
pub const API_VERSION: u32 = 1;

/// Stable element-type codes of the C API. They mirror [`DType`] but are an ABI promise:
/// a code is never renumbered or reused, and new types get new codes.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DTypeCode {
    Bool = 0,
    I8 = 1,
    I16 = 2,
    I32 = 3,
    I64 = 4,
    U8 = 5,
    U16 = 6,
    U32 = 7,
    U64 = 8,
    F32 = 9,
    F64 = 10,
    /// UTF-8 strings (`List[str]`); not a fixed-size type, so never borrowable.
    Str = 11,
}

impl DTypeCode {
    pub const fn from_dtype(dtype: DType) -> DTypeCode {
        match dtype {
            DType::Bool => DTypeCode::Bool,
            DType::I8 => DTypeCode::I8,
            DType::I16 => DTypeCode::I16,
            DType::I32 => DTypeCode::I32,
            DType::I64 => DTypeCode::I64,
            DType::U8 => DTypeCode::U8,
            DType::U16 => DTypeCode::U16,
            DType::U32 => DTypeCode::U32,
            DType::U64 => DTypeCode::U64,
            DType::F32 => DTypeCode::F32,
            DType::F64 => DTypeCode::F64,
            DType::Str => DTypeCode::Str,
        }
    }

    pub const fn dtype(self) -> DType {
        match self {
            DTypeCode::Bool => DType::Bool,
            DTypeCode::I8 => DType::I8,
            DTypeCode::I16 => DType::I16,
            DTypeCode::I32 => DType::I32,
            DTypeCode::I64 => DType::I64,
            DTypeCode::U8 => DType::U8,
            DTypeCode::U16 => DType::U16,
            DTypeCode::U32 => DType::U32,
            DTypeCode::U64 => DType::U64,
            DTypeCode::F32 => DType::F32,
            DTypeCode::F64 => DType::F64,
            DTypeCode::Str => DType::Str,
        }
    }

    pub fn from_raw(code: u8) -> Option<DTypeCode> {
        DType::ALL.into_iter().map(DTypeCode::from_dtype).find(|c| *c as u8 == code)
    }
}

/// The function table behind the capsule (layout version [`API_VERSION`]).
///
/// The `data`/`len` pairs below describe `len` *elements*; byte sizes are `len` times the
/// dtype's item size (bool is one byte holding 0 or 1).
#[repr(C)]
pub struct RustiqueCApi {
    /// [`API_VERSION`] of the providing rustique.
    pub api_version: u32,
    /// `size_of::<RustiqueCApi>()` of the providing rustique: a consumer may use a field
    /// only if it lies entirely within `abi_size` bytes.
    pub abi_size: usize,
    /// 1 if `obj` is a `rustique.List`, else 0. Never fails; NULL gives 0.
    pub list_check: unsafe extern "C" fn(obj: *mut ffi::PyObject) -> c_int,
    /// The [`DTypeCode`] of `obj`'s native storage, or -1 when `obj` is not a `List` or
    /// stores Python objects (or is an untyped empty list). Never sets an exception.
    pub list_dtype: unsafe extern "C" fn(obj: *mut ffi::PyObject) -> c_int,
    /// A new reference to a typed `List` holding a copy of `len` elements at `data`
    /// (`List[int]` for i64, `List[float]` for f64, `List[bool]`, otherwise
    /// `List[rs.<dtype>]`). `data` needs no alignment and may be NULL when `len == 0`; it
    /// must be readable for `len * itemsize` bytes and not be written during the call.
    /// `Str` is rejected (use `list_from_utf8`); bool bytes other than 0/1 raise
    /// `ValueError`.
    pub list_from_slice: unsafe extern "C" fn(dtype: u8, data: *const u8, len: usize) -> *mut ffi::PyObject,
    /// A new reference to a `List[str]` holding copies of `count` UTF-8 strings: string
    /// `i` is the `lens[i]` bytes at `ptrs[i]` (NULL allowed when `lens[i] == 0`). Invalid
    /// UTF-8 raises `UnicodeDecodeError`.
    pub list_from_utf8:
        unsafe extern "C" fn(ptrs: *const *const u8, lens: *const usize, count: usize) -> *mut ffi::PyObject,
    /// Zero-copy read access to fixed-size native storage. On success returns 0 and
    /// writes the dtype code, a pointer to the first element (non-NULL, aligned for the
    /// element type, possibly dangling when the length is 0), the element count and a
    /// token. Until the token is passed to `list_release` the list holds a buffer export:
    /// the data stays valid and unchanged and every mutation raises `BufferError` (any
    /// thread). The export also keeps the list alive. Raises `TypeError` for non-lists and
    /// for lists without fixed-size native storage.
    pub list_borrow: unsafe extern "C" fn(
        obj: *mut ffi::PyObject,
        dtype_out: *mut u8,
        data_out: *mut *const u8,
        len_out: *mut usize,
        token_out: *mut *mut c_void,
    ) -> c_int,
    /// Ends a borrow. Must be called exactly once per successful `list_borrow`, from an
    /// attached thread; NULL is ignored. Never fails, and leaves a pending exception as is.
    pub list_release: unsafe extern "C" fn(token: *mut c_void),
    /// Copies a `List[str]`'s strings out as UTF-8: the concatenated bytes into `data` and
    /// `count + 1` Arrow-style offsets (`offsets[0] == 0`, string `i` is
    /// `data[offsets[i]..offsets[i + 1]]`) into `offsets`. Always writes the required sizes
    /// to `data_len_out` (bytes) and `count_out` (strings). Returns 0 when everything was
    /// copied, 1 (no exception) when `data_cap < *data_len_out` or
    /// `offsets_cap < *count_out + 1` and nothing was copied, -1 with `TypeError` when `obj`
    /// is not a `List` with str storage. An untyped empty list copies as zero strings.
    pub list_copy_utf8: unsafe extern "C" fn(
        obj: *mut ffi::PyObject,
        data: *mut u8,
        data_cap: usize,
        offsets: *mut usize,
        offsets_cap: usize,
        data_len_out: *mut usize,
        count_out: *mut usize,
    ) -> c_int,
}

// The layout is the ABI: these offsets are documented in ARCHITECTURE §5 and asserted
// identically by `rustique-api`.
const WORD: usize = size_of::<usize>();
const _: () = {
    assert!(offset_of!(RustiqueCApi, api_version) == 0);
    assert!(offset_of!(RustiqueCApi, abi_size) == WORD);
    assert!(offset_of!(RustiqueCApi, list_check) == 2 * WORD);
    assert!(offset_of!(RustiqueCApi, list_dtype) == 3 * WORD);
    assert!(offset_of!(RustiqueCApi, list_from_slice) == 4 * WORD);
    assert!(offset_of!(RustiqueCApi, list_from_utf8) == 5 * WORD);
    assert!(offset_of!(RustiqueCApi, list_borrow) == 6 * WORD);
    assert!(offset_of!(RustiqueCApi, list_release) == 7 * WORD);
    assert!(offset_of!(RustiqueCApi, list_copy_utf8) == 8 * WORD);
    assert!(size_of::<RustiqueCApi>() == 9 * WORD);
};

static C_API: RustiqueCApi = RustiqueCApi {
    api_version: API_VERSION,
    abi_size: size_of::<RustiqueCApi>(),
    list_check,
    list_dtype,
    list_from_slice,
    list_from_utf8,
    list_borrow,
    list_release,
    list_copy_utf8,
};

/// Publish the table as `_C_API` on the extension module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let table = NonNull::from(&C_API).cast::<c_void>();
    // SAFETY: the table is an immutable `static`, valid for the life of the process; the
    // capsule needs no destructor.
    let capsule = unsafe { PyCapsule::new_with_pointer(m.py(), table, CAPSULE_NAME)? };
    m.add("_C_API", capsule)
}

/// Run `body` attached to the interpreter, turning errors and panics into a raised Python
/// exception plus `on_error`.
fn boundary<R: Copy>(on_error: R, body: impl FnOnce(Python<'_>) -> PyResult<R>) -> R {
    let attached = panic::catch_unwind(AssertUnwindSafe(|| {
        Python::attach(|py| {
            panic::catch_unwind(AssertUnwindSafe(|| body(py)))
                .unwrap_or_else(|payload| Err(panic_error(payload)))
                .unwrap_or_else(|err| {
                    err.restore(py);
                    on_error
                })
        })
    }));
    // Only `Python::attach` itself can get here (e.g. during interpreter finalisation).
    attached.unwrap_or(on_error)
}

#[cold]
fn panic_error(payload: Box<dyn Any + Send>) -> PyErr {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("panic in the rustique C API");
    PanicException::new_err(message.to_owned())
}

/// `obj` as a `List`, or `TypeError`.
///
/// # Safety
/// `obj` is NULL or a valid object pointer.
unsafe fn as_list<'a, 'py>(py: Python<'py>, obj: *mut ffi::PyObject, what: &str) -> PyResult<Borrowed<'a, 'py, List>> {
    // SAFETY: caller contract; NULL is mapped to `None`.
    let obj = unsafe { Borrowed::from_ptr_or_opt(py, obj) }
        .ok_or_else(|| PyTypeError::new_err(format!("{what}: expected a rustique List, got NULL")))?;
    obj.cast::<List>().map_err(|_| {
        let ty = obj.get_type().name().map(|n| n.to_string()).unwrap_or_else(|_| "?".to_owned());
        PyTypeError::new_err(format!("{what}: expected a rustique List, got {ty}"))
    })
}

/// `List.strategy` (`"empty"`, `"object"`, a dtype name); only used off the fast paths.
fn strategy(list: &Bound<'_, List>) -> PyResult<String> {
    list.getattr(pyo3::intern!(list.py(), "strategy"))?.extract()
}

/// A fresh `Vec<T>` holding a bytewise copy of `len` elements at `data`.
///
/// # Safety
/// `data` is readable for `len * size_of::<T>()` bytes (or `len == 0`), and every bit
/// pattern of that size is a valid `T`.
unsafe fn copy_elements<T: Copy>(data: *const u8, len: usize) -> PyResult<Vec<T>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if data.is_null() {
        return Err(PyValueError::new_err("list_from_slice: data is NULL but len > 0"));
    }
    let bytes = len
        .checked_mul(size_of::<T>())
        .filter(|&b| b <= isize::MAX as usize)
        .ok_or_else(|| PyMemoryError::new_err(()))?;
    let mut out = Vec::<T>::new();
    out.try_reserve_exact(len).map_err(|_| PyMemoryError::new_err(()))?;
    // SAFETY: `data` is readable for `bytes` bytes (caller); `out` has capacity for `len`
    // elements = `bytes` bytes in a fresh, non-overlapping allocation; a byte copy needs no
    // source alignment, and every bit pattern is a valid `T` (caller), so all `len`
    // elements are initialised.
    unsafe {
        ptr::copy_nonoverlapping(data, out.as_mut_ptr().cast::<u8>(), bytes);
        out.set_len(len);
    }
    Ok(out)
}

/// # Safety
/// As [`copy_elements`] with `T = u8`.
unsafe fn copy_bools(data: *const u8, len: usize) -> PyResult<Vec<bool>> {
    // Validate the private copy, not the caller's buffer, so a racing writer cannot slip
    // an invalid byte in after validation.
    // SAFETY: forwarded caller contract; every byte is a valid `u8`.
    let bytes = unsafe { copy_elements::<u8>(data, len)? };
    if let Some(i) = bytes.iter().position(|&b| b > 1) {
        return Err(PyValueError::new_err(format!(
            "list_from_slice: invalid bool byte {:#04x} at index {i} (expected 0 or 1)",
            bytes[i]
        )));
    }
    let mut bytes = ManuallyDrop::new(bytes);
    // SAFETY: `bool` has the size and alignment of `u8`, the allocation came from a
    // `Vec<u8>` with this capacity, and every byte is 0 or 1, a valid `bool`.
    Ok(unsafe { Vec::from_raw_parts(bytes.as_mut_ptr().cast::<bool>(), bytes.len(), bytes.capacity()) })
}

unsafe extern "C" fn list_check(obj: *mut ffi::PyObject) -> c_int {
    if obj.is_null() {
        return 0;
    }
    boundary(0, |py| {
        // SAFETY: `obj` is a valid object pointer (caller contract).
        let obj = unsafe { Borrowed::from_ptr(py, obj) };
        Ok(c_int::from(obj.is_instance_of::<List>()))
    })
}

unsafe extern "C" fn list_dtype(obj: *mut ffi::PyObject) -> c_int {
    if obj.is_null() {
        return -1;
    }
    boundary(-1, |py| {
        // SAFETY: `obj` is a valid object pointer (caller contract).
        let obj = unsafe { Borrowed::from_ptr(py, obj) };
        let Ok(list) = obj.cast::<List>() else { return Ok(-1) };
        Ok(list.get().with_column(py, |c| c_int::from(DTypeCode::from_dtype(c.dtype()) as u8)).unwrap_or(-1))
    })
}

unsafe extern "C" fn list_from_slice(dtype: u8, data: *const u8, len: usize) -> *mut ffi::PyObject {
    boundary(ptr::null_mut(), |py| {
        let code = DTypeCode::from_raw(dtype)
            .ok_or_else(|| PyValueError::new_err(format!("list_from_slice: unknown dtype code {dtype}")))?;
        // SAFETY: (every arm) `data` is readable for `len` elements of the dtype (caller
        // contract), and every bit pattern is valid for these integer and float types.
        let column = unsafe {
            match code {
                DTypeCode::Bool => Column::Bool(copy_bools(data, len)?),
                DTypeCode::I8 => Column::I8(copy_elements(data, len)?),
                DTypeCode::I16 => Column::I16(copy_elements(data, len)?),
                DTypeCode::I32 => Column::I32(copy_elements(data, len)?),
                DTypeCode::I64 => Column::I64(copy_elements(data, len)?),
                DTypeCode::U8 => Column::U8(copy_elements(data, len)?),
                DTypeCode::U16 => Column::U16(copy_elements(data, len)?),
                DTypeCode::U32 => Column::U32(copy_elements(data, len)?),
                DTypeCode::U64 => Column::U64(copy_elements(data, len)?),
                DTypeCode::F32 => Column::F32(copy_elements(data, len)?),
                DTypeCode::F64 => Column::F64(copy_elements(data, len)?),
                DTypeCode::Str => {
                    return Err(PyValueError::new_err(
                        "list_from_slice: str is not a fixed-size dtype; use list_from_utf8",
                    ));
                }
            }
        };
        Ok(List::from_column(py, column)?.into_ptr())
    })
}

unsafe extern "C" fn list_from_utf8(ptrs: *const *const u8, lens: *const usize, count: usize) -> *mut ffi::PyObject {
    boundary(ptr::null_mut(), |py| {
        let mut strings = Vec::new();
        if count > 0 {
            if ptrs.is_null() || lens.is_null() {
                return Err(PyValueError::new_err("list_from_utf8: ptrs or lens is NULL but count > 0"));
            }
            if count > isize::MAX as usize / size_of::<usize>() {
                return Err(PyMemoryError::new_err(()));
            }
            // SAFETY: both arrays hold `count` readable elements (caller contract), and
            // `count * size_of::<usize>()` fits in `isize` (checked above).
            let (ptrs, lens) =
                unsafe { (std::slice::from_raw_parts(ptrs, count), std::slice::from_raw_parts(lens, count)) };
            strings.try_reserve_exact(count).map_err(|_| PyMemoryError::new_err(()))?;
            for (i, (&p, &n)) in ptrs.iter().zip(lens).enumerate() {
                let bytes: &[u8] = match n {
                    0 => &[],
                    _ if p.is_null() => {
                        return Err(PyValueError::new_err(format!(
                            "list_from_utf8: string {i} is NULL but has length {n}"
                        )));
                    }
                    _ if n > isize::MAX as usize => return Err(PyMemoryError::new_err(())),
                    // SAFETY: `p` is readable for `n` bytes (caller contract); `n` fits
                    // in `isize`.
                    _ => unsafe { std::slice::from_raw_parts(p, n) },
                };
                let s = std::str::from_utf8(bytes).map_err(|e| {
                    let err = pyo3::exceptions::PyUnicodeDecodeError::new_err_from_utf8(py, bytes, e);
                    // Best effort: the note only adds context.
                    let _ = err.add_note(py, format!("in string {i} passed to list_from_utf8"));
                    err
                })?;
                strings.push(CompactString::from(s));
            }
        }
        Ok(List::from_column(py, Column::Str(strings))?.into_ptr())
    })
}

unsafe extern "C" fn list_borrow(
    obj: *mut ffi::PyObject,
    dtype_out: *mut u8,
    data_out: *mut *const u8,
    len_out: *mut usize,
    token_out: *mut *mut c_void,
) -> c_int {
    boundary(-1, |py| {
        if dtype_out.is_null() || data_out.is_null() || len_out.is_null() || token_out.is_null() {
            return Err(PyValueError::new_err("list_borrow: an output pointer is NULL"));
        }
        // SAFETY: `obj` is NULL or a valid object pointer (caller contract).
        let list = unsafe { as_list(py, obj, "list_borrow") }?;
        if !list.get().with_column(py, |c| c.dtype().item_size().is_some()).unwrap_or(false) {
            return Err(PyTypeError::new_err(format!(
                "list_borrow: List with '{}' strategy has no fixed-size native storage",
                strategy(&list)?
            )));
        }
        // The list's own buffer export does the freezing: under its lock it bumps the
        // export count that makes every mutation raise `BufferError`, and the view holds a
        // strong reference to the list.
        let mut view = Box::new(ffi::Py_buffer::new());
        // SAFETY: `list` is a valid object and `view` a writable `Py_buffer`.
        if unsafe { ffi::PyObject_GetBuffer(list.as_ptr(), &mut *view, ffi::PyBUF_FORMAT) } != 0 {
            return Err(PyErr::fetch(py));
        }
        let format = (!view.format.is_null())
            // SAFETY: a non-NULL format is the List's static, NUL-terminated format string.
            .then(|| unsafe { CStr::from_ptr(view.format) })
            .and_then(|f| f.to_str().ok())
            .and_then(DType::from_buffer_format);
        let (Some(dtype), Ok(itemsize)) = (format, usize::try_from(view.itemsize)) else {
            // SAFETY: releases the export obtained above, exactly once.
            unsafe { ffi::PyBuffer_Release(&mut *view) };
            return Err(PyTypeError::new_err("list_borrow: unexpected buffer format"));
        };
        let len = usize::try_from(view.len).unwrap_or(0) / itemsize.max(1);
        // SAFETY: the output pointers are non-NULL and writable (checked / caller contract).
        unsafe {
            *dtype_out = DTypeCode::from_dtype(dtype) as u8;
            *data_out = view.buf.cast_const().cast::<u8>();
            *len_out = len;
            *token_out = Box::into_raw(view).cast::<c_void>();
        }
        Ok(0)
    })
}

unsafe extern "C" fn list_release(token: *mut c_void) {
    if token.is_null() {
        return;
    }
    boundary((), |_py| {
        // SAFETY: a non-NULL token is the leaked `Py_buffer` box of one successful
        // `list_borrow`, released exactly once (caller contract).
        let mut view = unsafe { Box::from_raw(token.cast::<ffi::Py_buffer>()) };
        // SAFETY: we are attached. A caller may release on its own error path: the pending
        // exception (or NULL) is set aside around the release and then restored (stolen).
        unsafe {
            let pending = ffi::PyErr_GetRaisedException();
            ffi::PyBuffer_Release(&mut *view);
            ffi::PyErr_SetRaisedException(pending);
        }
        Ok(())
    })
}

unsafe extern "C" fn list_copy_utf8(
    obj: *mut ffi::PyObject,
    data: *mut u8,
    data_cap: usize,
    offsets: *mut usize,
    offsets_cap: usize,
    data_len_out: *mut usize,
    count_out: *mut usize,
) -> c_int {
    boundary(-1, |py| {
        if data_len_out.is_null() || count_out.is_null() {
            return Err(PyValueError::new_err("list_copy_utf8: an output pointer is NULL"));
        }
        // SAFETY: `obj` is NULL or a valid object pointer (caller contract).
        let list = unsafe { as_list(py, obj, "list_copy_utf8") }?;
        let copied = list.get().with_column(py, |column| {
            let Column::Str(strings) = column else { return Err(column.dtype().name()) };
            let total: usize = strings.iter().map(|s| s.len()).sum();
            // SAFETY: output pointers are non-NULL (checked above) and writable (caller).
            unsafe {
                *data_len_out = total;
                *count_out = strings.len();
            }
            if data_cap < total || offsets_cap <= strings.len() || (total > 0 && data.is_null()) || offsets.is_null() {
                return Ok(1);
            }
            // SAFETY: `data` has room for `total` bytes and `offsets` for `len + 1` entries
            // (capacities checked above, writability per caller contract); the sources are
            // our own strings, which cannot overlap caller memory.
            unsafe {
                let mut at = 0;
                *offsets = 0;
                for (i, s) in strings.iter().enumerate() {
                    // `data` may be NULL when every string is empty.
                    if !s.is_empty() {
                        ptr::copy_nonoverlapping(s.as_ptr(), data.add(at), s.len());
                    }
                    at += s.len();
                    *offsets.add(i + 1) = at;
                }
            }
            Ok(0)
        });
        match copied {
            Some(Ok(rc)) => Ok(rc),
            Some(Err(name)) => {
                Err(PyTypeError::new_err(format!("list_copy_utf8: expected a List with 'str' strategy, got '{name}'")))
            }
            // Untyped and empty: zero strings.
            None if strategy(&list)? == "empty" => {
                // SAFETY: output pointers are non-NULL (checked above) and writable (caller).
                unsafe {
                    *data_len_out = 0;
                    *count_out = 0;
                    if offsets_cap == 0 || offsets.is_null() {
                        return Ok(1);
                    }
                    *offsets = 0;
                }
                Ok(0)
            }
            None => Err(PyTypeError::new_err(format!(
                "list_copy_utf8: expected a List with 'str' strategy, got '{}'",
                strategy(&list)?
            ))),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtype_codes_are_stable() {
        let expected = [
            (DType::Bool, 0),
            (DType::I8, 1),
            (DType::I16, 2),
            (DType::I32, 3),
            (DType::I64, 4),
            (DType::U8, 5),
            (DType::U16, 6),
            (DType::U32, 7),
            (DType::U64, 8),
            (DType::F32, 9),
            (DType::F64, 10),
            (DType::Str, 11),
        ];
        assert_eq!(expected.len(), DType::ALL.len());
        for (dtype, code) in expected {
            let c = DTypeCode::from_dtype(dtype);
            assert_eq!(c as u8, code);
            assert_eq!(c.dtype(), dtype);
            assert_eq!(DTypeCode::from_raw(code), Some(c));
        }
        assert_eq!(DTypeCode::from_raw(12), None);
        assert_eq!(DTypeCode::from_raw(255), None);
    }

    #[test]
    fn table_describes_itself() {
        assert_eq!(C_API.api_version, API_VERSION);
        assert_eq!(C_API.abi_size, size_of::<RustiqueCApi>());
        assert_eq!(CAPSULE_NAME.to_str().unwrap(), "rustique._rustique._C_API");
    }

    #[test]
    fn null_objects_need_no_interpreter() {
        // SAFETY: NULL is explicitly allowed and handled before attaching.
        unsafe {
            assert_eq!(list_check(ptr::null_mut()), 0);
            assert_eq!(list_dtype(ptr::null_mut()), -1);
            list_release(ptr::null_mut());
        }
    }
}
