//! `rustique_demo`: a third-party extension module that hands Rust results to Python as
//! `rustique.List`s and reads `rustique.List`s in place, through `rustique-api`.
//!
//! Build it into the environment where rustique is installed:
//! `maturin develop --uv -m examples/rust_extension/Cargo.toml`.

use pyo3::prelude::*;

#[pymodule]
mod rustique_demo {
    use std::time::Duration;

    use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
    use pyo3::prelude::*;
    use rayon::prelude::*;
    use rustique_api::{DType, NativeElement, RustiqueApi};

    /// Inputs at least this long are processed in parallel with the interpreter detached.
    const PARALLEL: usize = 1 << 16;

    fn sieve(n: usize) -> Vec<i64> {
        if n < 3 {
            return Vec::new();
        }
        let mut composite = vec![false; n];
        let mut primes = Vec::with_capacity(n / 10 + 16);
        primes.push(2);
        let mut i = 3;
        while i < n {
            if !composite[i] {
                primes.push(i as i64);
                let mut j = i.saturating_mul(i);
                while j < n {
                    composite[j] = true;
                    j += 2 * i;
                }
            }
            i += 2;
        }
        primes
    }

    /// All primes below `n` as `rustique.List[int]`: one memcpy, no Python `int`s.
    #[pyfunction]
    fn primes(py: Python<'_>, n: usize) -> PyResult<Bound<'_, PyAny>> {
        let primes = py.detach(|| sieve(n));
        rustique_api::list_from_slice(py, &primes)
    }

    /// The same primes as a builtin `list[int]` through PyO3's `Vec<i64>` conversion
    /// (the baseline `primes` is measured against).
    #[pyfunction]
    fn primes_pylist(py: Python<'_>, n: usize) -> Vec<i64> {
        py.detach(|| sieve(n))
    }

    /// Number of primes below `n` (the sieve alone, for timing).
    #[pyfunction]
    fn count_primes(py: Python<'_>, n: usize) -> usize {
        py.detach(|| sieve(n).len())
    }

    /// `xs / ‖xs‖₂` for a list of floats, as a new `List[float]` (empty for an empty list).
    /// `xs` is read in place.
    #[pyfunction]
    fn normalize<'py>(py: Python<'py>, xs: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let view = rustique_api::borrow_list::<f64>(xs)?;
        let data: &[f64] = &view;
        let scaled = py.detach(|| {
            let norm = if data.len() >= PARALLEL {
                data.par_iter().map(|x| x * x).sum::<f64>().sqrt()
            } else {
                data.iter().map(|x| x * x).sum::<f64>().sqrt()
            };
            (data.is_empty() || (norm != 0.0 && norm.is_finite())).then(|| {
                if data.len() >= PARALLEL {
                    data.par_iter().map(|x| x / norm).collect::<Vec<f64>>()
                } else {
                    data.iter().map(|x| x / norm).collect()
                }
            })
        });
        drop(view);
        let scaled = scaled.ok_or_else(|| PyValueError::new_err("normalize() needs a finite, non-zero norm"))?;
        rustique_api::list_from_slice(py, &scaled)
    }

    /// Sum of a list of floats read in place (zero-copy borrow).
    #[pyfunction]
    fn fsum_borrowed(py: Python<'_>, xs: &Bound<'_, PyAny>) -> PyResult<f64> {
        let view = rustique_api::borrow_list::<f64>(xs)?;
        let data: &[f64] = &view;
        Ok(py.detach(|| data.iter().sum()))
    }

    /// Sum after PyO3's `Vec<f64>` extraction (the baseline `fsum_borrowed` is measured
    /// against; works for any sequence of floats).
    #[pyfunction]
    fn fsum_extracted(xs: Vec<f64>) -> f64 {
        xs.iter().sum()
    }

    /// Lengths (in code points, like `len(str)`) of the strings of a `List[str]`, as
    /// `List[int]`. The strings are copied out in one call, without Python `str` objects.
    #[pyfunction]
    fn word_lengths<'py>(py: Python<'py>, words: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let words = rustique_api::copy_str_list(words)?;
        let lengths: Vec<i64> = words.iter().map(|w| w.chars().count() as i64).collect();
        rustique_api::list_from_slice(py, &lengths)
    }

    /// Upper-cased copies of a `List[str]` as a new `List[str]`.
    #[pyfunction]
    fn upper<'py>(py: Python<'py>, words: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let words = rustique_api::copy_str_list(words)?;
        rustique_api::list_from_strs(py, words.iter().map(str::to_uppercase))
    }

    /// Run `$body` with `$t` bound to the Rust type of `$dtype` (fixed-size dtypes only).
    macro_rules! with_native_type {
        ($dtype:expr, $t:ident => $body:expr, _ => $other:expr) => {
            match $dtype {
                Some(DType::Bool) => {
                    type $t = bool;
                    $body
                }
                Some(DType::I8) => {
                    type $t = i8;
                    $body
                }
                Some(DType::I16) => {
                    type $t = i16;
                    $body
                }
                Some(DType::I32) => {
                    type $t = i32;
                    $body
                }
                Some(DType::I64) => {
                    type $t = i64;
                    $body
                }
                Some(DType::U8) => {
                    type $t = u8;
                    $body
                }
                Some(DType::U16) => {
                    type $t = u16;
                    $body
                }
                Some(DType::U32) => {
                    type $t = u32;
                    $body
                }
                Some(DType::U64) => {
                    type $t = u64;
                    $body
                }
                Some(DType::F32) => {
                    type $t = f32;
                    $body
                }
                Some(DType::F64) => {
                    type $t = f64;
                    $body
                }
                _ => $other,
            }
        };
    }

    fn copy_as<'py, T: NativeElement>(xs: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let view = rustique_api::borrow_list::<T>(xs)?;
        rustique_api::list_from_slice(xs.py(), &view)
    }

    /// A new list with the same storage dtype and values, made by borrowing `xs` and
    /// building from the borrowed slice (`list_from_strs` for strings).
    #[pyfunction]
    fn roundtrip<'py>(py: Python<'py>, xs: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let dtype = rustique_api::list_dtype(xs)?;
        with_native_type!(dtype, T => copy_as::<T>(xs), _ => match dtype {
            Some(DType::Str) => rustique_api::list_from_strs(py, rustique_api::copy_str_list(xs)?.iter()),
            _ => Err(PyTypeError::new_err("roundtrip() needs a rustique List with native storage")),
        })
    }

    fn call_while_borrowed<'py, T: NativeElement>(
        xs: &Bound<'py, PyAny>,
        callback: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let _view = rustique_api::borrow_list::<T>(xs)?;
        callback.call0()
    }

    /// Calls `callback()` while `xs` (any fixed-size storage) is borrowed and returns its
    /// result. Mutating `xs` inside the callback, from any thread, raises `BufferError`.
    #[pyfunction]
    fn while_borrowed<'py>(xs: &Bound<'py, PyAny>, callback: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let dtype = rustique_api::list_dtype(xs)?;
        with_native_type!(dtype, T => call_while_borrowed::<T>(xs, callback), _ => {
            Err(PyTypeError::new_err("while_borrowed() needs a rustique List with fixed-size storage"))
        })
    }

    /// Borrows a list of floats, sleeps `seconds` with the interpreter detached (so other
    /// threads run against the borrowed list), then returns the sum read from the view.
    #[pyfunction]
    fn hold_and_sum(py: Python<'_>, xs: &Bound<'_, PyAny>, seconds: f64) -> PyResult<f64> {
        let view = rustique_api::borrow_list::<f64>(xs)?;
        let data: &[f64] = &view;
        let pause = Duration::try_from_secs_f64(seconds).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(py.detach(|| {
            std::thread::sleep(pause);
            data.iter().sum()
        }))
    }

    /// Name of the native storage dtype (`None` for object storage and non-lists).
    #[pyfunction]
    fn storage_dtype(obj: &Bound<'_, PyAny>) -> PyResult<Option<&'static str>> {
        Ok(rustique_api::list_dtype(obj)?.map(DType::name))
    }

    #[pyfunction]
    fn is_rustique_list(obj: &Bound<'_, PyAny>) -> PyResult<bool> {
        rustique_api::is_list(obj)
    }

    /// `(api_version, abi_size)` of the installed rustique's C API.
    #[pyfunction]
    fn capi_version(py: Python<'_>) -> PyResult<(u32, usize)> {
        let api = RustiqueApi::get(py)?;
        Ok((api.version(), api.raw().abi_size))
    }

    /// Raw `list_from_slice` on bytes (for testing the C-level checks): the elements of
    /// dtype `code` in `data[offset:]` (`offset` lets tests pass a misaligned pointer).
    #[pyfunction]
    #[pyo3(signature = (code, data, itemsize, offset = 0))]
    fn raw_from_slice<'py>(
        py: Python<'py>,
        code: u8,
        data: &[u8],
        itemsize: usize,
        offset: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let api = RustiqueApi::get(py)?.raw();
        let data = data.get(offset..).unwrap_or_default();
        let len = data.len() / itemsize.max(1);
        // SAFETY: `data` is readable for `len * itemsize` bytes; the C API copies without
        // alignment requirements and validates the dtype code; we are attached.
        let list = unsafe { (api.list_from_slice)(code, data.as_ptr(), len) };
        // SAFETY: a new reference or NULL with an exception set.
        unsafe { Bound::from_owned_ptr_or_err(py, list) }
    }

    /// Raw borrow, then `list_release` while a Python exception is pending (as a C caller
    /// on its error path would): true if the exception survived and the list was released.
    #[pyfunction]
    fn raw_release_keeps_pending_error(py: Python<'_>, xs: &Bound<'_, PyAny>) -> PyResult<bool> {
        let api = RustiqueApi::get(py)?.raw();
        let (mut code, mut data, mut len, mut token) = (0u8, std::ptr::null(), 0usize, std::ptr::null_mut());
        // SAFETY: valid object and writable out-pointers; attached.
        if unsafe { (api.list_borrow)(xs.as_ptr(), &mut code, &mut data, &mut len, &mut token) } != 0 {
            return Err(PyErr::fetch(py));
        }
        PyKeyError::new_err("pending").restore(py);
        // SAFETY: the token of the successful borrow above, released once; attached.
        unsafe { (api.list_release)(token) };
        let survived = PyErr::take(py).is_some_and(|e| e.is_instance_of::<PyKeyError>(py));
        Ok(survived && rustique_api::list_dtype(xs)?.is_some())
    }

    /// Raw `list_from_utf8` on byte strings (invalid UTF-8 must raise).
    #[pyfunction]
    fn raw_from_utf8<'py>(py: Python<'py>, items: Vec<Vec<u8>>) -> PyResult<Bound<'py, PyAny>> {
        let api = RustiqueApi::get(py)?.raw();
        let ptrs: Vec<*const u8> = items.iter().map(|b| b.as_ptr()).collect();
        let lens: Vec<usize> = items.iter().map(Vec::len).collect();
        // SAFETY: `ptrs[i]`/`lens[i]` describe `items[i]`, alive for the call; attached.
        let list = unsafe { (api.list_from_utf8)(ptrs.as_ptr(), lens.as_ptr(), items.len()) };
        // SAFETY: a new reference or NULL with an exception set.
        unsafe { Bound::from_owned_ptr_or_err(py, list) }
    }
}
