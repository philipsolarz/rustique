//! Sorting Python object pointers with CPython's own `list.sort`.
//!
//! OWNER: objects+builtins workstream (WS5).
//!
//! Object storage (`Vec<Py<PyAny>>`, or a raw `PyObject*` array) is sorted by *moving* its
//! pointers into a temporary builtin `list` (the list takes over the references: no incref),
//! calling `list.sort` on it, and moving the pointers back (no decref). The sort itself is
//! CPython's: timsort with the type-specialised pre-sort compare selection
//! (`unsafe_long_compare`, `unsafe_latin_compare`, `unsafe_tuple_compare`, ...), `key`
//! called exactly once per element in order, stability, `reverse` keeping equal elements in
//! order, and its exception behaviour. The only costs on top of `list.sort` are one list
//! allocation and two `memcpy`s of the pointer array.
//!
//! `list.sort` is called through the `list.sort` method descriptor (vectorcall), so the
//! function pointer is read at call time: a process-wide acceleration of `list.sort` (the
//! takeover tier) applies here too.
//!
//! Semantics, for the caller:
//! - On success the items are a sorted permutation of the input.
//! - If `key` or a comparison raises, the error is returned and the items are left as *some
//!   permutation* of the input (CPython's `list.sort` guarantee; never lost or duplicated).
//!   If a `key` call fails, nothing has moved yet (keys are computed before sorting).
//! - The temporary list is private, so Python code run by `key`/`__lt__` cannot reach the
//!   items through it (except via `gc.get_referrers`-style introspection: then CPython's own
//!   "list modified during sort" detection applies to the temporary list and the items come
//!   back unchanged in number). Mutations of the *caller's* container while the sort runs
//!   are the caller's business: `List.sort` takes its elements out first, so the list
//!   appears empty meanwhile and changes made meanwhile are discarded and reported with
//!   `ValueError("list modified during sort")`, exactly like CPython.
//! - No Rustique lock may be held while calling these functions (they run Python code).

use std::sync::OnceLock;

use pyo3::exceptions::PySystemError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyList, PyString, PyTuple};

/// The `list.sort` method descriptor and the keyword-name tuples used to call it.
struct SortCallables {
    descriptor: Py<PyAny>,
    /// `("reverse",)`
    kw_reverse: Py<PyTuple>,
    /// `("key", "reverse")`
    kw_key_reverse: Py<PyTuple>,
}

static SORT: OnceLock<SortCallables> = OnceLock::new();

fn sort_callables(py: Python<'_>) -> PyResult<&'static SortCallables> {
    if let Some(s) = SORT.get() {
        return Ok(s);
    }
    let descriptor = py.get_type::<PyList>().getattr("sort")?.unbind();
    let kw_reverse = PyTuple::new(py, [PyString::intern(py, "reverse")])?.unbind();
    let kw_key_reverse = PyTuple::new(py, [PyString::intern(py, "key"), PyString::intern(py, "reverse")])?.unbind();
    // A racing initialiser builds equal objects; the loser's are dropped (attached: fine).
    let _ = SORT.set(SortCallables { descriptor, kw_reverse, kw_key_reverse });
    Ok(SORT.get().expect("initialised above"))
}

/// The `list.sort` method descriptor (for callers that sort a builtin list they own).
pub fn list_sort_descriptor(py: Python<'_>) -> PyResult<&'static Py<PyAny>> {
    Ok(&sort_callables(py)?.descriptor)
}

/// Sort owned object references in place with CPython's `list.sort` semantics (see the
/// module documentation for the behaviour on errors and mutation).
pub fn sort_objects(
    py: Python<'_>,
    items: &mut Vec<Py<PyAny>>,
    key: Option<&Bound<'_, PyAny>>,
    reverse: bool,
) -> PyResult<()> {
    sort_slice(py, items.as_mut_slice(), key, reverse)
}

/// [`sort_objects`] for a slice of owned references.
pub fn sort_slice(
    py: Python<'_>,
    items: &mut [Py<PyAny>],
    key: Option<&Bound<'_, PyAny>>,
    reverse: bool,
) -> PyResult<()> {
    let len = items.len();
    // SAFETY: `Py<PyAny>` is `repr(transparent)` over a non-null `PyObject*`, and every
    // element owns one strong reference, which is what `sort_raw` requires.
    unsafe { sort_raw(py, items.as_mut_ptr().cast::<*mut ffi::PyObject>(), len, key, reverse) }
}

/// [`sort_objects`] for a raw array of owned references (e.g. a `PyObject*` array that a
/// container keeps behind its own lock).
///
/// # Safety
/// `items` must point to `len` initialised, non-null pointers, each owning one strong
/// reference, and nothing else may read or write the array during the call (the caller's
/// container must not expose it: take the elements out, or keep them private). The thread
/// must be attached and hold no Rustique lock (Python code runs). On return (success or
/// error) the array again holds `len` owned, non-null references, a permutation of the input.
pub unsafe fn sort_raw(
    py: Python<'_>,
    items: *mut *mut ffi::PyObject,
    len: usize,
    key: Option<&Bound<'_, PyAny>>,
    reverse: bool,
) -> PyResult<()> {
    let key = key.filter(|k| !k.is_none());
    // CPython calls `key` once per element even for a single element; with no key, fewer
    // than two elements involve no Python code at all.
    if len == 0 || (len == 1 && key.is_none()) {
        return Ok(());
    }
    let callables = sort_callables(py)?;
    let n = isize::try_from(len).map_err(|_| PySystemError::new_err("sort: too many elements"))?;
    // SAFETY: creating a list; NULL means MemoryError (nothing moved yet).
    let tmp = unsafe { ffi::PyList_New(n) };
    if tmp.is_null() {
        return Err(PyErr::fetch(py));
    }
    // SAFETY: `tmp` is a fresh list with `len` NULL slots; the references move into it, so
    // from here on the list owns them and `items` only holds stale copies.
    unsafe {
        let slots = (*tmp.cast::<ffi::PyListObject>()).ob_item;
        std::ptr::copy_nonoverlapping(items, slots, len);
    }
    // SAFETY: `tmp` is a live list; the descriptor call only reads its arguments.
    let result = unsafe { call_sort(py, callables, tmp, key, reverse) };
    // SAFETY: `list.sort` always restores the list's original length and item array (a
    // permutation of the references we moved in), even on errors or if the list was
    // modified meanwhile. Move the references back and empty the list without decrefs.
    unsafe {
        let back = ffi::PyList_GET_SIZE(tmp);
        let slots = (*tmp.cast::<ffi::PyListObject>()).ob_item;
        if back != n || slots.is_null() {
            // Unreachable with CPython's `list.sort`; never hand back a broken array.
            std::process::abort();
        }
        std::ptr::copy_nonoverlapping(slots, items, len);
        (*tmp.cast::<ffi::PyVarObject>()).ob_size = 0;
        ffi::Py_DECREF(tmp);
    }
    result
}

/// `list.sort(tmp, key=key, reverse=reverse)` through the method descriptor.
///
/// # Safety
/// `tmp` must be a live builtin list.
unsafe fn call_sort(
    py: Python<'_>,
    callables: &SortCallables,
    tmp: *mut ffi::PyObject,
    key: Option<&Bound<'_, PyAny>>,
    reverse: bool,
) -> PyResult<()> {
    // SAFETY: the bool singletons.
    let rev = unsafe { if reverse { ffi::Py_True() } else { ffi::Py_False() } };
    let descriptor = callables.descriptor.as_ptr();
    // SAFETY: borrowed argument arrays live on the stack for the duration of the call.
    let out = unsafe {
        match key {
            None if !reverse => ffi::PyObject_Vectorcall(descriptor, [tmp].as_ptr(), 1, std::ptr::null_mut()),
            None => ffi::PyObject_Vectorcall(descriptor, [tmp, rev].as_ptr(), 1, callables.kw_reverse.as_ptr()),
            Some(k) => ffi::PyObject_Vectorcall(
                descriptor,
                [tmp, k.as_ptr(), rev].as_ptr(),
                1,
                callables.kw_key_reverse.as_ptr(),
            ),
        }
    };
    if out.is_null() {
        return Err(PyErr::fetch(py));
    }
    // SAFETY: `out` is a new reference (None).
    unsafe { ffi::Py_DECREF(out) };
    Ok(())
}
