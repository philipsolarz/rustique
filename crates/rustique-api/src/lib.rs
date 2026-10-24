//! # rustique-api
//!
//! Hand native data to Python as a [`rustique`](https://github.com/philipsolarz/rustique)
//! `List`, and read `List`s without converting them element by element, from **your own**
//! PyO3 extension module.
//!
//! Rustique publishes a versioned C function table as the capsule
//! `rustique._rustique._C_API` (like NumPy's `_ARRAY_API`). This crate imports it once
//! per process and wraps it in a safe API. It depends on PyO3 only: the lists it creates
//! are genuine `rustique.List` objects owned by the installed `rustique` package, not by a
//! second copy compiled into your extension.
//!
//! ```no_run
//! use pyo3::prelude::*;
//!
//! /// Returns `rustique.List[int]`: one memcpy, no per-element Python objects.
//! #[pyfunction]
//! fn squares(py: Python<'_>, n: i64) -> PyResult<Bound<'_, PyAny>> {
//!     let v: Vec<i64> = (0..n).map(|i| i * i).collect();
//!     rustique_api::list_from_slice(py, &v)
//! }
//!
//! /// Sums a `List[float]` in place (zero-copy; the list is frozen while borrowed).
//! #[pyfunction]
//! fn total(py: Python<'_>, xs: &Bound<'_, PyAny>) -> PyResult<f64> {
//!     let view = rustique_api::borrow_list::<f64>(xs)?;
//!     let data: &[f64] = &view;
//!     Ok(py.detach(|| data.iter().sum()))
//! }
//! ```
//!
//! Compatibility: this crate needs C API version [`REQUIRED_API_VERSION`] or newer and
//! raises `ImportError` on first use if the installed rustique is older (or missing).

use std::ffi::{CStr, c_void};
use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr::{self, NonNull};

use pyo3::exceptions::{PyImportError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyCapsule;

/// Name of the capsule (module path + attribute) that rustique publishes.
pub const CAPSULE_NAME: &CStr = c"rustique._rustique._C_API";

/// Oldest C API version this crate works with: version 1 is the table in [`ffi`].
pub const REQUIRED_API_VERSION: u32 = 1;

/// The raw C ABI, for code that needs it (e.g. to forward the table to C).
///
/// Mirrors `rustique_py::capi`; the field offsets are part of the ABI and asserted at
/// compile time on both sides. See ARCHITECTURE §5 "Rust interop" for the full contract.
pub mod ffi {
    use std::ffi::{c_int, c_void};
    use std::mem::offset_of;

    use pyo3::ffi::PyObject;

    /// Version 1 of the function table. Later versions only append fields.
    #[repr(C)]
    #[derive(Debug)]
    pub struct RustiqueCApi {
        pub api_version: u32,
        pub abi_size: usize,
        pub list_check: unsafe extern "C" fn(obj: *mut PyObject) -> c_int,
        pub list_dtype: unsafe extern "C" fn(obj: *mut PyObject) -> c_int,
        pub list_from_slice: unsafe extern "C" fn(dtype: u8, data: *const u8, len: usize) -> *mut PyObject,
        pub list_from_utf8:
            unsafe extern "C" fn(ptrs: *const *const u8, lens: *const usize, count: usize) -> *mut PyObject,
        pub list_borrow: unsafe extern "C" fn(
            obj: *mut PyObject,
            dtype_out: *mut u8,
            data_out: *mut *const u8,
            len_out: *mut usize,
            token_out: *mut *mut c_void,
        ) -> c_int,
        pub list_release: unsafe extern "C" fn(token: *mut c_void),
        pub list_copy_utf8: unsafe extern "C" fn(
            obj: *mut PyObject,
            data: *mut u8,
            data_cap: usize,
            offsets: *mut usize,
            offsets_cap: usize,
            data_len_out: *mut usize,
            count_out: *mut usize,
        ) -> c_int,
    }

    /// The two leading fields, present in every version.
    #[repr(C)]
    #[derive(Debug)]
    pub struct Header {
        pub api_version: u32,
        pub abi_size: usize,
    }

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
        assert!(offset_of!(Header, abi_size) == offset_of!(RustiqueCApi, abi_size));
    };
}

/// Element types of rustique's native storage, with their stable C API codes.
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DType {
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
    Str = 11,
}

impl DType {
    const ALL: [DType; 12] = [
        DType::Bool,
        DType::I8,
        DType::I16,
        DType::I32,
        DType::I64,
        DType::U8,
        DType::U16,
        DType::U32,
        DType::U64,
        DType::F32,
        DType::F64,
        DType::Str,
    ];

    /// The dtype with C API code `code`, if this crate knows it.
    pub fn from_code(code: u8) -> Option<DType> {
        DType::ALL.into_iter().find(|d| *d as u8 == code)
    }

    /// The C API code.
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Name as reported by `List.strategy` (`"i64"`, `"f64"`, `"str"`, …).
    pub const fn name(self) -> &'static str {
        match self {
            DType::Bool => "bool",
            DType::I8 => "i8",
            DType::I16 => "i16",
            DType::I32 => "i32",
            DType::I64 => "i64",
            DType::U8 => "u8",
            DType::U16 => "u16",
            DType::U32 => "u32",
            DType::U64 => "u64",
            DType::F32 => "f32",
            DType::F64 => "f64",
            DType::Str => "str",
        }
    }

    /// The typed alias a list built from this dtype gets (`"List[int]"`, `"List[rs.u8]"`).
    pub const fn list_type(self) -> &'static str {
        match self {
            DType::Bool => "List[bool]",
            DType::I8 => "List[rs.i8]",
            DType::I16 => "List[rs.i16]",
            DType::I32 => "List[rs.i32]",
            DType::I64 => "List[int]",
            DType::U8 => "List[rs.u8]",
            DType::U16 => "List[rs.u16]",
            DType::U32 => "List[rs.u32]",
            DType::U64 => "List[rs.u64]",
            DType::F32 => "List[rs.f32]",
            DType::F64 => "List[float]",
            DType::Str => "List[str]",
        }
    }
}

impl fmt::Display for DType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

mod sealed {
    pub trait Sealed {}
}

/// A fixed-size element type that rustique stores natively. Sealed: implemented for
/// `bool`, `i8`…`i64`, `u8`…`u64`, `f32` and `f64`.
pub trait NativeElement: sealed::Sealed + Copy + Send + Sync + 'static {
    const DTYPE: DType;
}

macro_rules! native {
    ($($ty:ty => $dtype:ident),* $(,)?) => {$(
        impl sealed::Sealed for $ty {}
        impl NativeElement for $ty {
            const DTYPE: DType = DType::$dtype;
        }
    )*};
}

native! {
    bool => Bool, i8 => I8, i16 => I16, i32 => I32, i64 => I64,
    u8 => U8, u16 => U16, u32 => U32, u64 => U64, f32 => F32, f64 => F64,
}

/// Handle to the imported C API. Obtain it with [`RustiqueApi::get`]; the free functions
/// of this crate do that for you.
#[derive(Clone, Copy, Debug)]
pub struct RustiqueApi {
    raw: &'static ffi::RustiqueCApi,
}

static API: PyOnceLock<RustiqueApi> = PyOnceLock::new();

impl RustiqueApi {
    /// The C API, imported (`import rustique`) and version-checked on first use and cached
    /// for the life of the process.
    pub fn get(py: Python<'_>) -> PyResult<RustiqueApi> {
        API.get_or_try_init(py, || Self::import(py)).copied()
    }

    fn import(py: Python<'_>) -> PyResult<RustiqueApi> {
        let not_available = |err: PyErr| {
            let new = PyImportError::new_err(format!(
                "rustique-api needs rustique with C API version >= {REQUIRED_API_VERSION} \
                 ({}), which could not be imported; install or upgrade rustique",
                CAPSULE_NAME.to_string_lossy()
            ));
            new.set_cause(py, Some(err));
            new
        };
        let table = PyCapsule::import_pointer(py, CAPSULE_NAME).map_err(not_available)?;
        let table = table.cast::<ffi::RustiqueCApi>();
        if !table.is_aligned() {
            return Err(PyImportError::new_err("rustique C API table is misaligned"));
        }
        // SAFETY: a capsule with this exact name is published only by rustique and points at
        // a `static` table in its extension module (never unloaded by CPython), which in
        // every version starts with `Header`.
        let header = unsafe { table.cast::<ffi::Header>().as_ref() };
        if header.api_version < REQUIRED_API_VERSION || header.abi_size < size_of::<ffi::RustiqueCApi>() {
            return Err(PyImportError::new_err(format!(
                "the installed rustique provides C API version {} ({} bytes), older than version \
                 {REQUIRED_API_VERSION} ({} bytes) required by rustique-api {}; upgrade rustique",
                header.api_version,
                header.abi_size,
                size_of::<ffi::RustiqueCApi>(),
                env!("CARGO_PKG_VERSION"),
            )));
        }
        // SAFETY: as above, and the table is at least `size_of::<RustiqueCApi>()` bytes
        // (checked); every version >= 1 starts with exactly these fields.
        let raw: &'static ffi::RustiqueCApi = unsafe { table.as_ref() };
        Ok(RustiqueApi { raw })
    }

    /// The raw function table.
    pub fn raw(&self) -> &'static ffi::RustiqueCApi {
        self.raw
    }

    /// The installed rustique's C API version (≥ [`REQUIRED_API_VERSION`]).
    pub fn version(&self) -> u32 {
        self.raw.api_version
    }

    /// Whether `obj` is a `rustique.List`.
    pub fn is_list(&self, obj: &Bound<'_, PyAny>) -> bool {
        // SAFETY: `obj` is a valid object and we are attached.
        unsafe { (self.raw.list_check)(obj.as_ptr()) == 1 }
    }

    /// The dtype of `obj`'s native storage; `None` for non-lists, object storage and
    /// untyped empty lists.
    pub fn list_dtype(&self, obj: &Bound<'_, PyAny>) -> Option<DType> {
        // SAFETY: `obj` is a valid object and we are attached.
        let code = unsafe { (self.raw.list_dtype)(obj.as_ptr()) };
        u8::try_from(code).ok().and_then(DType::from_code)
    }

    /// A new typed `rustique.List` holding a copy of `data` (one memcpy). See
    /// [`list_from_slice`].
    pub fn list_from_slice<'py, T: NativeElement>(&self, py: Python<'py>, data: &[T]) -> PyResult<Bound<'py, PyAny>> {
        // SAFETY: `data` is readable for `data.len()` elements of `T`, whose code we pass;
        // we are attached (`py`).
        let list = unsafe { (self.raw.list_from_slice)(T::DTYPE.code(), data.as_ptr().cast(), data.len()) };
        // SAFETY: the function returns a new reference or NULL with an exception set.
        unsafe { Bound::from_owned_ptr_or_err(py, list) }
    }

    /// A new `rustique.List[str]` holding copies of `strings`. See [`list_from_strs`].
    pub fn list_from_strs<'py, S: AsRef<str>>(
        &self,
        py: Python<'py>,
        strings: impl IntoIterator<Item = S>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let strings: Vec<S> = strings.into_iter().collect();
        let ptrs: Vec<*const u8> = strings.iter().map(|s| s.as_ref().as_ptr()).collect();
        let lens: Vec<usize> = strings.iter().map(|s| s.as_ref().len()).collect();
        // SAFETY: `ptrs[i]`/`lens[i]` describe `strings[i]`, valid UTF-8 kept alive until
        // the call returns; we are attached (`py`).
        let list = unsafe { (self.raw.list_from_utf8)(ptrs.as_ptr(), lens.as_ptr(), strings.len()) };
        // SAFETY: the function returns a new reference or NULL with an exception set.
        unsafe { Bound::from_owned_ptr_or_err(py, list) }
    }

    /// Borrow a list's native storage as `&[T]`. See [`borrow_list`].
    pub fn borrow_list<'py, T: NativeElement>(&self, obj: &Bound<'py, PyAny>) -> PyResult<ListView<'py, T>> {
        let py = obj.py();
        let (mut code, mut data, mut len, mut token) = (0u8, ptr::null::<u8>(), 0usize, ptr::null_mut::<c_void>());
        // SAFETY: `obj` is valid, the out-pointers are writable locals; we are attached.
        let rc = unsafe { (self.raw.list_borrow)(obj.as_ptr(), &mut code, &mut data, &mut len, &mut token) };
        if rc != 0 {
            let err = PyErr::fetch(py);
            // An untyped empty `List()` has no storage to share, but no elements of a wrong
            // type either: it reads as an empty slice of any element type.
            if self.is_list(obj) && self.list_dtype(obj).is_none() && is_untyped_empty(obj) {
                return Ok(ListView::empty(self.raw));
            }
            return Err(err);
        }
        // Owns the export from here on: released on every path below.
        let view = ListView { data: data.cast::<T>(), len, token, api: self.raw, _py: PhantomData };
        if code != T::DTYPE.code() {
            let found = DType::from_code(code).map_or_else(|| format!("unknown dtype code {code}"), |d| d.to_string());
            return Err(PyTypeError::new_err(format!(
                "expected a rustique List with {} storage (e.g. {}), got one with {found} storage",
                T::DTYPE,
                T::DTYPE.list_type(),
            )));
        }
        if data.is_null() || !data.cast::<T>().is_aligned() {
            return Err(PyValueError::new_err("rustique C API returned a misaligned buffer"));
        }
        Ok(view)
    }

    /// Copy a `List[str]`'s strings out. See [`copy_str_list`].
    pub fn copy_str_list(&self, obj: &Bound<'_, PyAny>) -> PyResult<StrList> {
        let (mut data, mut offsets) = (Vec::<u8>::new(), Vec::<usize>::new());
        let (mut data_len, mut count) = (0usize, 0usize);
        loop {
            // SAFETY: `data`/`offsets` are writable for their capacities; `obj` is valid and
            // we are attached.
            let rc = unsafe {
                (self.raw.list_copy_utf8)(
                    obj.as_ptr(),
                    data.as_mut_ptr(),
                    data.capacity(),
                    offsets.as_mut_ptr(),
                    offsets.capacity(),
                    &mut data_len,
                    &mut count,
                )
            };
            match rc {
                0 => break,
                // Too small (the first call only measures, and the list may grow between
                // calls): retry with the reported sizes plus some slack.
                1 => {
                    data.reserve_exact(data_len + data_len / 8);
                    offsets.reserve_exact(count + 1 + count / 8);
                }
                _ => return Err(PyErr::fetch(obj.py())),
            }
        }
        // SAFETY: on success the provider initialised `data_len <= capacity` bytes and
        // `count + 1 <= capacity` offsets (C API contract).
        unsafe {
            data.set_len(data_len);
            offsets.set_len(count + 1);
        }
        StrList::new(data, offsets).ok_or_else(|| PyValueError::new_err("rustique C API returned malformed strings"))
    }
}

/// Whether a `rustique.List` has the `"empty"` strategy (untyped, no elements yet).
fn is_untyped_empty(list: &Bound<'_, PyAny>) -> bool {
    list.getattr(pyo3::intern!(list.py(), "strategy")).and_then(|s| s.extract::<String>()).is_ok_and(|s| s == "empty")
}

/// A `rustique.List` (`List[int]` for `i64`, `List[float]` for `f64`, `List[bool]`,
/// otherwise `List[rs.<dtype>]`) holding a copy of `data`, made with a single memcpy and no
/// per-element Python objects. For a `Vec` you build only to return, this is the whole
/// conversion cost.
pub fn list_from_slice<'py, T: NativeElement>(py: Python<'py>, data: &[T]) -> PyResult<Bound<'py, PyAny>> {
    RustiqueApi::get(py)?.list_from_slice(py, data)
}

/// A `rustique.List[str]` holding copies of `strings`.
pub fn list_from_strs<'py, S: AsRef<str>>(
    py: Python<'py>,
    strings: impl IntoIterator<Item = S>,
) -> PyResult<Bound<'py, PyAny>> {
    RustiqueApi::get(py)?.list_from_strs(py, strings)
}

/// Borrow the native storage of a `rustique.List` as `&[T]`, without copying.
///
/// While the returned [`ListView`] lives, the list is frozen exactly as during a buffer
/// export: every mutation, from any thread, raises `BufferError`, so the slice can be read
/// freely, also from other threads or with the interpreter detached
/// (`let s: &[T] = &view; py.detach(|| ...)`). Dropping the view releases the list.
///
/// Raises `TypeError` if `obj` is not a `rustique.List` or its storage is not `T`
/// (`List[float]` stores `f64`, `List[int]` stores `i64` unless a value overflowed i64,
/// `List[rs.u8]` stores `u8`, an untyped list stores whatever its `.strategy` says). An
/// untyped empty `List()` borrows as an empty slice.
pub fn borrow_list<'py, T: NativeElement>(obj: &Bound<'py, PyAny>) -> PyResult<ListView<'py, T>> {
    RustiqueApi::get(obj.py())?.borrow_list(obj)
}

/// Copy the strings of a `rustique.List[str]` (or an untyped list with `str` strategy) into
/// one contiguous UTF-8 buffer, without creating Python `str` objects.
pub fn copy_str_list(obj: &Bound<'_, PyAny>) -> PyResult<StrList> {
    RustiqueApi::get(obj.py())?.copy_str_list(obj)
}

/// Whether `obj` is a `rustique.List`.
pub fn is_list(obj: &Bound<'_, PyAny>) -> PyResult<bool> {
    Ok(RustiqueApi::get(obj.py())?.is_list(obj))
}

/// The dtype of `obj`'s native storage (`None` for non-lists and object storage).
pub fn list_dtype(obj: &Bound<'_, PyAny>) -> PyResult<Option<DType>> {
    Ok(RustiqueApi::get(obj.py())?.list_dtype(obj))
}

/// Zero-copy, read-only view of a `rustique.List`'s elements; derefs to `[T]`.
///
/// The list cannot be mutated while the view exists (mutations raise `BufferError`); the
/// view releases it on drop. Bound to the `'py` attachment, so it is dropped while
/// attached; the slice it derefs to is `Send + Sync` and may cross [`Python::detach`].
pub struct ListView<'py, T: NativeElement> {
    data: *const T,
    len: usize,
    /// NULL for the empty view of an untyped list (nothing to release).
    token: *mut c_void,
    api: &'static ffi::RustiqueCApi,
    _py: PhantomData<(Python<'py>, &'py [T])>,
}

impl<'py, T: NativeElement> ListView<'py, T> {
    fn empty(api: &'static ffi::RustiqueCApi) -> Self {
        ListView { data: NonNull::dangling().as_ptr(), len: 0, token: ptr::null_mut(), api, _py: PhantomData }
    }

    /// The borrowed elements.
    pub fn as_slice(&self) -> &[T] {
        // SAFETY: `data` is non-NULL and aligned (checked when borrowed, or dangling for
        // len 0), points at `len` initialised `T`s that the export keeps alive and
        // unmodified until `self` is dropped.
        unsafe { std::slice::from_raw_parts(self.data, self.len) }
    }
}

impl<T: NativeElement> Deref for ListView<'_, T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T: NativeElement> AsRef<[T]> for ListView<'_, T> {
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T: NativeElement + fmt::Debug> fmt::Debug for ListView<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ListView").field(&self.as_slice()).finish()
    }
}

impl<T: NativeElement> Drop for ListView<'_, T> {
    fn drop(&mut self) {
        // SAFETY: the token came from one successful `list_borrow` (or is NULL, which is
        // ignored) and is released exactly once; `'py` proves we are attached.
        unsafe { (self.api.list_release)(self.token) };
    }
}

/// Strings copied out of a `List[str]`: one UTF-8 buffer plus `len() + 1` offsets.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StrList {
    data: String,
    offsets: Vec<usize>,
}

impl StrList {
    /// Validates the parts (UTF-8, monotonic in-bounds offsets on char boundaries).
    fn new(data: Vec<u8>, offsets: Vec<usize>) -> Option<StrList> {
        let data = String::from_utf8(data).ok()?;
        let valid = offsets.first() == Some(&0)
            && offsets.last() == Some(&data.len())
            && offsets.windows(2).all(|w| w[0] <= w[1])
            && offsets.iter().all(|&o| data.is_char_boundary(o));
        valid.then_some(StrList { data, offsets })
    }

    pub fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        let (&start, &end) = (self.offsets.get(index)?, self.offsets.get(index + 1)?);
        self.data.get(start..end)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &str> + DoubleEndedIterator + '_ {
        self.offsets.windows(2).map(|w| &self.data[w[0]..w[1]])
    }

    /// All strings concatenated.
    pub fn as_str(&self) -> &str {
        &self.data
    }

    /// `len() + 1` byte offsets into [`StrList::as_str`] (Arrow layout).
    pub fn offsets(&self) -> &[usize] {
        &self.offsets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtype_codes_match_the_abi() {
        let names = ["bool", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "str"];
        for (code, name) in names.iter().enumerate() {
            let d = DType::from_code(code as u8).unwrap();
            assert_eq!(d.code() as usize, code);
            assert_eq!(d.name(), *name);
        }
        assert_eq!(DType::from_code(12), None);
        assert_eq!(<i64 as NativeElement>::DTYPE, DType::I64);
        assert_eq!(<f64 as NativeElement>::DTYPE, DType::F64);
        assert_eq!(<bool as NativeElement>::DTYPE, DType::Bool);
        assert_eq!(<u8 as NativeElement>::DTYPE.list_type(), "List[rs.u8]");
    }

    #[test]
    fn str_list_parts_are_validated() {
        let s = StrList::new(b"aboc\xc3\xa9".to_vec(), vec![0, 2, 2, 6]).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s.iter().collect::<Vec<_>>(), ["ab", "", "oc\u{e9}"]);
        assert_eq!(s.get(2), Some("oc\u{e9}"));
        assert_eq!(s.get(3), None);
        assert_eq!(s.iter().next_back(), Some("oc\u{e9}"));

        let empty = StrList::new(Vec::new(), vec![0]).unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty.iter().count(), 0);

        assert!(StrList::new(b"ab".to_vec(), vec![0, 3]).is_none(), "offset past the end");
        assert!(StrList::new(b"ab".to_vec(), vec![0, 2, 1]).is_none(), "not monotonic");
        assert!(StrList::new(b"\xc3\xa9".to_vec(), vec![0, 1, 2]).is_none(), "inside a char");
        assert!(StrList::new(b"\xff".to_vec(), vec![0, 1]).is_none(), "invalid UTF-8");
        assert!(StrList::new(Vec::new(), Vec::new()).is_none(), "missing leading offset");
    }
}
