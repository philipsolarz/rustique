//! Takeover, safe tier (docs/TAKEOVER.md): exact-semantics Rust kernels behind the builtin
//! container methods and builtin functions, process-wide.
//!
//! OWNER: takeover workstream.
//!
//! Mechanism. CPython reaches `list.sort`, `list.index`, `list.count`, `tuple.index` and
//! `tuple.count` through the `ml_meth` pointer of a static `PyMethodDef`; every method
//! descriptor, bound method and specialized call site (`CALL_METHOD_DESCRIPTOR_*`) reads
//! that pointer at call time (`sorted()` calls `sort` on its fresh list, so it is covered
//! too). The builtin *functions* (`sum`, `min`, `max`, `sorted`, `any`, `all`) belong to
//! `rustique._builtins` (workstream 5), which `rustique.enable()` installs alongside. `x in list` / `x in tuple` go through the `sq_contains` slot.
//! [`install`] swaps those function pointers for the kernels below and keeps the originals.
//! Nothing else changes: the objects stay exact builtins (every specializer and JIT fast
//! path keeps firing), the descriptors and builtin functions keep their identity, `repr`,
//! docs and pickling, and no type is modified (the fixed type version tags of `list`,
//! `tuple`, ... that the specializer keys on stay valid).
//!
//! Every kernel first decides, without running Python code and without changing anything,
//! whether its inputs admit a pure fast path whose result is provably the one CPython
//! computes: elements of exact type `int`/`bool`/`float`/`str`/`None`, no key function, no
//! extra arguments. Otherwise it calls the saved original C function with the original
//! arguments, so argument parsing, errors, messages and re-entrancy are CPython's own. A
//! scan that meets an element needing CPython's generic comparison while every comparison
//! so far was pure restarts in the original (unobservable: pure comparisons have no effects).
//!
//! Free-threading: `sort` holds the list's critical section like CPython's `list.sort`; per-element scans (`in`, `index`,
//! `count`) take a strong reference per element with `PyList_GetItemRef`, like CPython's
//! lock-free `list_contains`. Installing patches process-global function pointers: call
//! `install` before starting threads.

use std::cmp::Ordering;
use std::ffi::{CStr, c_int};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, OnceLock};

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::sync::critical_section::with_critical_section;
use pyo3::types::{PyDict, PyList, PySet, PyString, PyTuple};

/// Free-threaded build: the object header carries thread id and shared refcount fields.
const FREE_THREADED: bool = size_of::<ffi::PyObject>() > 2 * size_of::<usize>();

type FastFn = unsafe extern "C" fn(*mut ffi::PyObject, *const *mut ffi::PyObject, ffi::Py_ssize_t) -> *mut ffi::PyObject;
type FastKwFn = unsafe extern "C" fn(
    *mut ffi::PyObject,
    *const *mut ffi::PyObject,
    ffi::Py_ssize_t,
    *mut ffi::PyObject,
) -> *mut ffi::PyObject;
type OFn = unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject) -> *mut ffi::PyObject;
type ContainsFn = unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject) -> c_int;

// ---------------------------------------------------------------------------------------
// Raw layouts (CPython >= 3.12; this crate requires >= 3.14).
// ---------------------------------------------------------------------------------------

/// `PyLongObject`: header, `lv_tag` (sign in bits 0-1, digit count from bit 3), digits.
#[repr(C)]
struct RawLong {
    ob_base: ffi::PyObject,
    lv_tag: usize,
    ob_digit: [u32; 1],
}

const LONG_NON_SIZE_BITS: u32 = 3;

#[inline(always)]
fn long_type() -> *mut ffi::PyTypeObject {
    &raw mut ffi::PyLong_Type
}

#[inline(always)]
fn bool_type() -> *mut ffi::PyTypeObject {
    &raw mut ffi::PyBool_Type
}

#[inline(always)]
fn float_type() -> *mut ffi::PyTypeObject {
    &raw mut ffi::PyFloat_Type
}

#[inline(always)]
fn str_type() -> *mut ffi::PyTypeObject {
    &raw mut ffi::PyUnicode_Type
}

/// Value of a compact `int`/`bool` (|v| < 2**30), as `_PyLong_CompactValue` computes it.
///
/// # Safety
/// `op` must be a live object whose type is exactly `int` or `bool`.
#[inline(always)]
unsafe fn compact_value(op: *mut ffi::PyObject) -> Option<i64> {
    // SAFETY: caller guarantees an int/bool object, so the RawLong prefix is valid.
    let (tag, digit) = unsafe {
        let l = op.cast::<RawLong>();
        ((*l).lv_tag, (*l).ob_digit[0])
    };
    if tag < (2 << LONG_NON_SIZE_BITS) {
        let sign = 1 - (tag & 3) as i64;
        Some(sign * i64::from(digit))
    } else {
        None
    }
}

/// Value of an exact `int`/`bool` that fits `i64` (no Python code runs for exact ints).
///
/// # Safety
/// `op` must be a live object whose type is exactly `int` or `bool`.
#[inline(always)]
unsafe fn int_value(op: *mut ffi::PyObject) -> Option<i64> {
    // SAFETY: forwarded.
    if let Some(v) = unsafe { compact_value(op) } {
        return Some(v);
    }
    let mut overflow: c_int = 0;
    // SAFETY: exact int: converts without calling `__index__`; overflow is reported by flag.
    let v = unsafe { ffi::PyLong_AsLongLongAndOverflow(op, &mut overflow) };
    if overflow != 0 {
        return None;
    }
    if v == -1 {
        // SAFETY: attached thread; an error can only be a (theoretical) internal failure.
        if unsafe { !ffi::PyErr_Occurred().is_null() } {
            // SAFETY: attached thread with an error set.
            unsafe { ffi::PyErr_Clear() };
            return None;
        }
    }
    Some(v)
}

/// # Safety
/// `op` must be an exact `float`.
#[inline(always)]
unsafe fn float_value(op: *mut ffi::PyObject) -> f64 {
    // SAFETY: exact float.
    unsafe { (*op.cast::<ffi::PyFloatObject>()).ob_fval }
}

/// A `str` decomposed for comparisons: canonical kind (1, 2 or 4 bytes per code point),
/// data pointer, length in code points and cached hash (-1 if not computed yet).
#[derive(Clone, Copy)]
struct StrParts {
    kind: u32,
    data: *const u8,
    len: usize,
    hash: isize,
}

/// Where the kind/compact/ascii bits of `PyASCIIObject.state` live and where compact string
/// data starts, measured on the running interpreter at install time ([`calibrate_str`]).
/// The bitfield layout differs between 3.14 and 3.15 and between GIL and free-threaded
/// builds, so it is never hard-coded; without a verified layout `PyUnicode_KIND`/
/// `PyUnicode_DATA` (out-of-line calls on 3.14+) are used.
#[derive(Clone, Copy)]
struct StrLayout {
    kind_shift: u32,
    compact_mask: u32,
    ascii_mask: u32,
    ascii_data: usize,
    compact_data: usize,
}

static STR_LAYOUT: OnceLock<StrLayout> = OnceLock::new();

/// # Safety
/// `op` must be an exact `str`.
#[inline(always)]
unsafe fn str_state(op: *mut ffi::PyObject) -> u32 {
    // SAFETY: exact str. Volatile: free-threaded builds update the `interned` byte of the
    // same word atomically; the kind/compact/ascii bits never change after creation.
    unsafe { std::ptr::read_volatile(&raw const (*op.cast::<ffi::PyASCIIObject>()).state) }
}

/// Kind and data pointer of an exact `str`.
///
/// # Safety
/// `op` must be an exact `str`.
#[inline(always)]
unsafe fn str_kind_data(op: *mut ffi::PyObject, layout: Option<&StrLayout>) -> (u32, *const u8) {
    // SAFETY: exact str; offsets verified by `calibrate_str` against PyUnicode_DATA.
    unsafe {
        if let Some(l) = layout {
            let state = str_state(op);
            if state & l.compact_mask != 0 {
                let off = if state & l.ascii_mask != 0 { l.ascii_data } else { l.compact_data };
                return ((state >> l.kind_shift) & 7, op.cast::<u8>().add(off));
            }
        }
        (ffi::PyUnicode_KIND(op), ffi::PyUnicode_DATA(op).cast::<u8>())
    }
}

/// Measure [`StrLayout`] on sample strings of every kind; `None` if anything disagrees
/// with `PyUnicode_KIND`/`PyUnicode_DATA`.
fn calibrate_str(py: Python<'_>) -> Option<StrLayout> {
    // Built at run time: compact, not interned, not static.
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

/// # Safety
/// `op` must be an exact `str` that stays alive while the parts are used.
#[inline(always)]
unsafe fn str_parts(op: *mut ffi::PyObject) -> StrParts {
    // SAFETY: exact str; the hash field may be written concurrently on free-threaded
    // builds (relaxed atomic in CPython), hence the volatile read; any value read is either
    // -1 or the hash.
    unsafe {
        let ascii = op.cast::<ffi::PyASCIIObject>();
        let (kind, data) = str_kind_data(op, STR_LAYOUT.get());
        StrParts {
            kind,
            data,
            len: ffi::PyUnicode_GET_LENGTH(op) as usize,
            hash: std::ptr::read_volatile(&raw const (*ascii).hash),
        }
    }
}

/// The i-th code point of a string.
///
/// # Safety
/// `i < p.len`.
#[inline(always)]
unsafe fn code_point(p: &StrParts, i: usize) -> u32 {
    // SAFETY: in bounds per caller; data is aligned for its kind.
    unsafe {
        match p.kind {
            1 => u32::from(*p.data.add(i)),
            2 => u32::from(*p.data.cast::<u16>().add(i)),
            _ => *p.data.cast::<u32>().add(i),
        }
    }
}

/// Code-point lexicographic order, then length: `str.__lt__` for exact strings.
///
/// # Safety
/// Both parts must describe live strings.
unsafe fn str_cmp(a: &StrParts, b: &StrParts) -> Ordering {
    // SAFETY: lengths and kinds come from the strings themselves.
    unsafe {
        if a.kind == b.kind {
            return match a.kind {
                1 => std::slice::from_raw_parts(a.data, a.len).cmp(std::slice::from_raw_parts(b.data, b.len)),
                2 => std::slice::from_raw_parts(a.data.cast::<u16>(), a.len)
                    .cmp(std::slice::from_raw_parts(b.data.cast::<u16>(), b.len)),
                _ => std::slice::from_raw_parts(a.data.cast::<u32>(), a.len)
                    .cmp(std::slice::from_raw_parts(b.data.cast::<u32>(), b.len)),
            };
        }
        for i in 0..a.len.min(b.len) {
            match code_point(a, i).cmp(&code_point(b, i)) {
                Ordering::Equal => {}
                other => return other,
            }
        }
    }
    a.len.cmp(&b.len)
}

// ---------------------------------------------------------------------------------------
// Equality fast path shared by `in`, `index` and `count`.
// ---------------------------------------------------------------------------------------

/// Cached hash of an exact `str` (-1 if not computed yet).
///
/// # Safety
/// `op` must be an exact `str`.
#[inline(always)]
unsafe fn str_hash(op: *mut ffi::PyObject) -> isize {
    // SAFETY: exact str; volatile because free-threaded builds cache the hash concurrently
    // (any value read is either -1 or the hash).
    unsafe { std::ptr::read_volatile(&raw const (*op.cast::<ffi::PyASCIIObject>()).hash) }
}

/// How a scan treats matches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    /// Stop at the first match and report its index.
    First,
    /// Count every match.
    Count,
}

/// Outcome of a scan: `Ok(Some(i))`/`Ok(None)` for `First`, `Ok(Some(count))` for `Count`;
/// `Err(())`: a Python exception is set; `Delegate`: nothing observable happened yet and
/// the original C implementation must run the whole operation instead.
enum Outcome {
    Done(Result<Option<isize>, ()>),
    Delegate,
}

/// A scan that meets an element needing CPython's generic comparison before this index,
/// with no Python code run so far, restarts in the original implementation instead (all
/// comparisons made so far were pure, so redoing them is unobservable). CPython inlines
/// `PyObject_RichCompareBool` into its own loops; an extension pays an out-of-line call
/// (~23 instructions per element, measured), so long generic runs belong to the original.
const DELEGATE_BEFORE: isize = 16;

/// A scan in CPython's order: element `i` is fetched afresh every step (`get` returns NULL
/// past the end; for lists it re-reads the size and the item array, which `__eq__` code
/// may change), identity is tested first, then `fast` decides the comparison without
/// running Python code where it can (`None`: `PyObject_RichCompareBool(item, needle,
/// Py_EQ)`, item on the left like CPython). `OWNED`: `get` returns strong references.
///
/// # Safety
/// `get` yields live objects; `needle` is live; attached thread.
#[inline(always)]
unsafe fn scan<const OWNED: bool>(
    get: impl Fn(isize) -> *mut ffi::PyObject,
    needle: *mut ffi::PyObject,
    mode: Scan,
    fast: impl Fn(*mut ffi::PyObject) -> Option<bool>,
) -> Outcome {
    let mut count: isize = 0;
    let mut i: isize = 0;
    let mut ran_python = false;
    // SAFETY: per the function contract.
    unsafe {
        loop {
            let item = get(i);
            if item.is_null() {
                break;
            }
            let hit = if item == needle {
                1
            } else {
                match fast(item) {
                    Some(b) => c_int::from(b),
                    None if !ran_python && i < DELEGATE_BEFORE => {
                        if OWNED {
                            ffi::Py_DECREF(item);
                        }
                        return Outcome::Delegate;
                    }
                    None => {
                        ran_python = true;
                        if OWNED {
                            ffi::PyObject_RichCompareBool(item, needle, ffi::Py_EQ)
                        } else {
                            ffi::Py_INCREF(item);
                            let r = ffi::PyObject_RichCompareBool(item, needle, ffi::Py_EQ);
                            ffi::Py_DECREF(item);
                            r
                        }
                    }
                }
            };
            if OWNED {
                ffi::Py_DECREF(item);
            }
            if hit < 0 {
                return Outcome::Done(Err(()));
            }
            if hit > 0 {
                if mode == Scan::First {
                    return Outcome::Done(Ok(Some(i)));
                }
                count += 1;
            }
            i += 1;
        }
    }
    Outcome::Done(Ok(if mode == Scan::First { None } else { Some(count) }))
}

/// A cheap pre-filter for the types the fast paths decide (exact `int`/`bool`/`float`/
/// `str`/`None`): `int`/`bool`/`str` and their subclasses by type flag, exact `float`,
/// `None`. Subclasses pass here and are rejected by the exact checks of the scan (which
/// then delegates).
///
/// # Safety
/// `op` must be a live object.
#[inline(always)]
unsafe fn maybe_scalar(op: *mut ffi::PyObject) -> bool {
    // SAFETY: type object reads of a live object; `Py_None` is a static object.
    unsafe {
        let t = ffi::Py_TYPE(op);
        ffi::PyType_HasFeature(t, ffi::Py_TPFLAGS_LONG_SUBCLASS | ffi::Py_TPFLAGS_UNICODE_SUBCLASS) != 0
            || t == float_type()
            || op == ffi::Py_None()
    }
}

/// Classify the needle and run `scan` with the matching pure comparison; needles of other
/// types delegate at once.
///
/// Exact `int`/`bool`/`float`/`str`/`None` compare by value; across those types only
/// numbers can be equal (`1 == 1.0 == True`), and `int`/`float` comparisons are exact for
/// compact ints (|v| < 2**30). A big int never equals a compact one (CPython normalises
/// ints); other big-int comparisons take the generic comparison.
///
/// # Safety
/// As for [`scan`].
#[inline(always)]
unsafe fn scan_for<const OWNED: bool>(
    get: impl Fn(isize) -> *mut ffi::PyObject,
    needle: *mut ffi::PyObject,
    mode: Scan,
) -> Outcome {
    // SAFETY: values are read according to the exact types checked first.
    unsafe {
        let t = ffi::Py_TYPE(needle);
        let none = ffi::Py_None();
        if t == long_type() || t == bool_type() {
            if let Some(v) = compact_value(needle) {
                return scan::<OWNED>(get, needle, mode, |item| {
                    let t = ffi::Py_TYPE(item);
                    if t == long_type() || t == bool_type() {
                        Some(compact_value(item) == Some(v))
                    } else if t == float_type() {
                        Some(v as f64 == float_value(item))
                    } else if t == str_type() || item == none {
                        Some(false)
                    } else {
                        None
                    }
                });
            }
            return scan::<OWNED>(get, needle, mode, |item| {
                let t = ffi::Py_TYPE(item);
                if t == long_type() || t == bool_type() {
                    if compact_value(item).is_some() { Some(false) } else { None }
                } else if t == str_type() || item == none {
                    Some(false)
                } else {
                    None
                }
            });
        }
        if t == float_type() {
            let f = float_value(needle);
            return scan::<OWNED>(get, needle, mode, |item| {
                let t = ffi::Py_TYPE(item);
                if t == float_type() {
                    Some(float_value(item) == f)
                } else if t == long_type() || t == bool_type() {
                    compact_value(item).map(|v| v as f64 == f)
                } else if t == str_type() || item == none {
                    Some(false)
                } else {
                    None
                }
            });
        }
        if t == str_type() {
            let layout = STR_LAYOUT.get();
            let np = str_parts(needle);
            let (len, hash) = (np.len as isize, np.hash);
            let nbytes = np.len * np.kind as usize;
            return scan::<OWNED>(get, needle, mode, |item| {
                let t = ffi::Py_TYPE(item);
                if t == str_type() {
                    if ffi::PyUnicode_GET_LENGTH(item) != len {
                        return Some(false);
                    }
                    let h = str_hash(item);
                    if h != -1 && hash != -1 && h != hash {
                        return Some(false);
                    }
                    // Canonical representation: equal strings have equal kinds and bytes.
                    let (kind, data) = str_kind_data(item, layout);
                    Some(kind == np.kind && std::slice::from_raw_parts(data, nbytes) == std::slice::from_raw_parts(np.data, nbytes))
                } else if t == long_type() || t == bool_type() || t == float_type() || item == none {
                    Some(false)
                } else {
                    None
                }
            });
        }
        if needle == none {
            return scan::<OWNED>(get, needle, mode, |item| {
                let t = ffi::Py_TYPE(item);
                if t == long_type() || t == bool_type() || t == float_type() || t == str_type() { Some(false) } else { None }
            });
        }
        Outcome::Delegate
    }
}

/// Scan a list like `list_contains` / `list.index` / `list.count`.
///
/// # Safety
/// `list` must be a `list` (exact or a subclass sharing the slot), `needle` a live object.
#[inline(always)]
unsafe fn scan_list(list: *mut ffi::PyObject, needle: *mut ffi::PyObject, mode: Scan) -> Outcome {
    // SAFETY: the size and the item array are re-read at every step; on free-threaded
    // builds each element is fetched as a strong reference (other threads may replace it).
    unsafe {
        if FREE_THREADED {
            scan_for::<true>(
                |i| {
                    if i >= ffi::PyList_GET_SIZE(list) {
                        return std::ptr::null_mut();
                    }
                    let item = ffi::PyList_GetItemRef(list, i);
                    if item.is_null() {
                        ffi::PyErr_Clear(); // shrunk concurrently: out of bounds
                    }
                    item
                },
                needle,
                mode,
            )
        } else {
            scan_for::<false>(
                |i| {
                    if i >= ffi::PyList_GET_SIZE(list) {
                        return std::ptr::null_mut();
                    }
                    *(*list.cast::<ffi::PyListObject>()).ob_item.offset(i)
                },
                needle,
                mode,
            )
        }
    }
}

/// Scan a tuple (immutable: the size is fixed).
///
/// # Safety
/// `tuple` must be a `tuple`, `needle` a live object.
#[inline(always)]
unsafe fn scan_tuple(tuple: *mut ffi::PyObject, needle: *mut ffi::PyObject, mode: Scan) -> Outcome {
    // SAFETY: tuple items are immutable and owned by the tuple, which the caller keeps alive.
    unsafe {
        let n = ffi::PyTuple_GET_SIZE(tuple);
        scan_for::<false>(|i| if i < n { ffi::PyTuple_GET_ITEM(tuple, i) } else { std::ptr::null_mut() }, needle, mode)
    }
}

// ---------------------------------------------------------------------------------------
// Saved originals and patch sites.
// ---------------------------------------------------------------------------------------

struct Originals {
    list_sort: FastKwFn,
    list_index: FastFn,
    list_count: OFn,
    tuple_index: FastFn,
    tuple_count: OFn,
    list_contains: ContainsFn,
    tuple_contains: ContainsFn,
    /// Exception messages of the originals, captured at install time. Kept as C strings,
    /// not Python objects: the patched tables are process-global, so kernels also run in
    /// subinterpreters, which must never share objects with the main interpreter.
    list_index_msg: std::ffi::CString,
    tuple_index_msg: std::ffi::CString,
}

static ORIGINALS: OnceLock<Originals> = OnceLock::new();

/// The original `sq_contains` of `list` and `tuple`, for the cheapest possible delegation
/// (set once at first install, before any kernel can run).
static ORIG_LIST_CONTAINS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static ORIG_TUPLE_CONTAINS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// # Safety
/// `slot` holds a `ContainsFn` (set at install time, before the kernels are reachable).
#[inline(always)]
unsafe fn contains_fn(slot: &std::sync::atomic::AtomicUsize) -> ContainsFn {
    // SAFETY: per the caller; a non-zero value is a valid function pointer.
    unsafe { std::mem::transmute::<usize, ContainsFn>(slot.load(std::sync::atomic::Ordering::Relaxed)) }
}

/// A patched function pointer: its address, the original value and the kernel.
struct Site {
    name: &'static str,
    slot: usize,
    original: usize,
    kernel: usize,
}

static INSTALLED: Mutex<Vec<Site>> = Mutex::new(Vec::new());

#[inline(always)]
fn originals() -> &'static Originals {
    // SAFETY-free invariant: kernels are only reachable after ORIGINALS was set.
    match ORIGINALS.get() {
        Some(o) => o,
        None => unreachable_originals(),
    }
}

#[cold]
fn unreachable_originals() -> ! {
    // Kernels are installed strictly after ORIGINALS is initialised; abort rather than
    // unwind across the C boundary if that invariant were ever broken.
    std::process::abort()
}

/// Run a kernel body, converting a Rust panic into a Python `SystemError` (never unwinds
/// into CPython).
#[inline(always)]
fn guard<R>(on_panic: R, body: impl FnOnce() -> R) -> R {
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(r) => r,
        Err(_) => {
            // SAFETY: attached thread (we were called by CPython).
            unsafe { ffi::PyErr_SetString(ffi::PyExc_SystemError, c"rustique takeover kernel panicked".as_ptr()) };
            on_panic
        }
    }
}

// ---------------------------------------------------------------------------------------
// Kernels: `in`, `index`, `count`.
// ---------------------------------------------------------------------------------------

/// What the first element alone decides (GIL builds, list or tuple prefix `first`):
/// `Some(Done)` for an empty sequence or an identity hit that settles the operation,
/// `Some(Delegate)` when the first element or the needle is of a type the fast paths
/// cannot decide, `None` to run the scan.
///
/// # Safety
/// `items` points at `n` live items; `needle` is live.
#[inline(always)]
unsafe fn prelude(items: *const *mut ffi::PyObject, n: isize, needle: *mut ffi::PyObject, mode: Scan) -> Option<Outcome> {
    if n == 0 {
        return Some(Outcome::Done(Ok(if mode == Scan::First { None } else { Some(0) })));
    }
    // SAFETY: n > 0, so the first item exists.
    let first = unsafe { *items };
    if first == needle {
        if mode == Scan::First {
            return Some(Outcome::Done(Ok(Some(0))));
        }
        if n == 1 {
            return Some(Outcome::Done(Ok(Some(1))));
        }
        return None;
    }
    // SAFETY: live objects.
    if unsafe { !maybe_scalar(first) || !maybe_scalar(needle) } {
        return Some(Outcome::Delegate);
    }
    None
}

/// # Safety
/// `list` is a list; GIL build.
#[inline(always)]
unsafe fn list_prelude(list: *mut ffi::PyObject, needle: *mut ffi::PyObject, mode: Scan) -> Option<Outcome> {
    if FREE_THREADED {
        return None;
    }
    // SAFETY: GIL held; the item array is valid for the current size.
    unsafe { prelude((*list.cast::<ffi::PyListObject>()).ob_item, ffi::PyList_GET_SIZE(list), needle, mode) }
}

/// # Safety
/// `tuple` is a tuple.
#[inline(always)]
unsafe fn tuple_prelude(tuple: *mut ffi::PyObject, needle: *mut ffi::PyObject, mode: Scan) -> Option<Outcome> {
    // SAFETY: immutable tuple items.
    unsafe { prelude((*tuple.cast::<ffi::PyTupleObject>()).ob_item.as_ptr(), ffi::PyTuple_GET_SIZE(tuple), needle, mode) }
}

/// The scans proper, out of line so that the delegation path of the kernels stays small.
#[inline(never)]
unsafe fn scan_list_slow(list: *mut ffi::PyObject, needle: *mut ffi::PyObject, mode: Scan) -> Outcome {
    // SAFETY: forwarded.
    unsafe { scan_list(list, needle, mode) }
}

#[inline(never)]
unsafe fn scan_tuple_slow(tuple: *mut ffi::PyObject, needle: *mut ffi::PyObject, mode: Scan) -> Outcome {
    // SAFETY: forwarded.
    unsafe { scan_tuple(tuple, needle, mode) }
}

unsafe extern "C" fn k_list_contains(seq: *mut ffi::PyObject, el: *mut ffi::PyObject) -> c_int {
    // SAFETY: called by CPython through `sq_contains` with a list and a live object. The
    // checks below need no stack frame, so both exits are plain tail calls.
    unsafe {
        if !FREE_THREADED {
            let n = ffi::PyList_GET_SIZE(seq);
            if n == 0 {
                return 0;
            }
            let first = *(*seq.cast::<ffi::PyListObject>()).ob_item;
            if first == el {
                return 1;
            }
            if !maybe_scalar(first) || !maybe_scalar(el) {
                return contains_fn(&ORIG_LIST_CONTAINS)(seq, el);
            }
        }
        list_contains_scan(seq, el)
    }
}

/// The scan behind `x in list` (out of line: see [`k_list_contains`]). Panic-free; a panic
/// in an `extern "C"` function aborts, never unwinds.
#[inline(never)]
unsafe extern "C" fn list_contains_scan(seq: *mut ffi::PyObject, el: *mut ffi::PyObject) -> c_int {
    // SAFETY: forwarded from `k_list_contains`.
    unsafe {
        match scan_list(seq, el, Scan::First) {
            Outcome::Done(Ok(found)) => c_int::from(found.is_some()),
            Outcome::Done(Err(())) => -1,
            Outcome::Delegate => contains_fn(&ORIG_LIST_CONTAINS)(seq, el),
        }
    }
}

unsafe extern "C" fn k_tuple_contains(seq: *mut ffi::PyObject, el: *mut ffi::PyObject) -> c_int {
    // SAFETY: called by CPython through `sq_contains` with a tuple and a live object.
    unsafe {
        let n = ffi::PyTuple_GET_SIZE(seq);
        if n == 0 {
            return 0;
        }
        let first = ffi::PyTuple_GET_ITEM(seq, 0);
        if first == el {
            return 1;
        }
        if !maybe_scalar(first) || !maybe_scalar(el) {
            return contains_fn(&ORIG_TUPLE_CONTAINS)(seq, el);
        }
        tuple_contains_scan(seq, el)
    }
}

/// The scan behind `x in tuple` (out of line: see [`k_tuple_contains`]).
#[inline(never)]
unsafe extern "C" fn tuple_contains_scan(seq: *mut ffi::PyObject, el: *mut ffi::PyObject) -> c_int {
    // SAFETY: forwarded from `k_tuple_contains`.
    unsafe {
        match scan_tuple(seq, el, Scan::First) {
            Outcome::Done(Ok(found)) => c_int::from(found.is_some()),
            Outcome::Done(Err(())) => -1,
            Outcome::Delegate => contains_fn(&ORIG_TUPLE_CONTAINS)(seq, el),
        }
    }
}

/// `index` result: the position, or CPython's own `ValueError` message.
///
/// # Safety
/// Attached thread.
unsafe fn index_result(r: Result<Option<isize>, ()>, msg: &CStr) -> *mut ffi::PyObject {
    // SAFETY: attached thread; `msg` is NUL-terminated UTF-8.
    unsafe {
        match r {
            Ok(Some(i)) => ffi::PyLong_FromSsize_t(i),
            Ok(None) => {
                ffi::PyErr_SetString(ffi::PyExc_ValueError, msg.as_ptr());
                std::ptr::null_mut()
            }
            Err(()) => std::ptr::null_mut(),
        }
    }
}

unsafe extern "C" fn k_list_index(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
) -> *mut ffi::PyObject {
    let o = originals();
    if nargs == 1 {
        // SAFETY: METH_FASTCALL guarantees `nargs` valid argument pointers.
        // SAFETY: as above.
        let outcome = unsafe { list_prelude(slf, *args, Scan::First).unwrap_or_else(|| scan_list_slow(slf, *args, Scan::First)) };
        if let Outcome::Done(r) = outcome {
            // SAFETY: attached thread.
            return unsafe { index_result(r, &o.list_index_msg) };
        }
    }
    // SAFETY: same arguments to the original METH_FASTCALL function.
    unsafe { (o.list_index)(slf, args, nargs) }
}

unsafe extern "C" fn k_tuple_index(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
) -> *mut ffi::PyObject {
    let o = originals();
    if nargs == 1 {
        // SAFETY: METH_FASTCALL guarantees `nargs` valid argument pointers.
        // SAFETY: as above.
        let outcome = unsafe { tuple_prelude(slf, *args, Scan::First).unwrap_or_else(|| scan_tuple_slow(slf, *args, Scan::First)) };
        if let Outcome::Done(r) = outcome {
            // SAFETY: attached thread.
            return unsafe { index_result(r, &o.tuple_index_msg) };
        }
    }
    // SAFETY: same arguments to the original METH_FASTCALL function.
    unsafe { (o.tuple_index)(slf, args, nargs) }
}

/// `count` result.
///
/// # Safety
/// Attached thread.
unsafe fn count_result(r: Result<Option<isize>, ()>) -> *mut ffi::PyObject {
    match r {
        // SAFETY: attached thread.
        Ok(n) => unsafe { ffi::PyLong_FromSsize_t(n.unwrap_or(0)) },
        Err(()) => std::ptr::null_mut(),
    }
}

unsafe extern "C" fn k_list_count(slf: *mut ffi::PyObject, value: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: METH_O: `slf` is a list, `value` a live object.
    match unsafe { list_prelude(slf, value, Scan::Count).unwrap_or_else(|| scan_list_slow(slf, value, Scan::Count)) } {
        // SAFETY: attached thread.
        Outcome::Done(r) => unsafe { count_result(r) },
        // SAFETY: same arguments to the original METH_O function.
        Outcome::Delegate => unsafe { (originals().list_count)(slf, value) },
    }
}

unsafe extern "C" fn k_tuple_count(slf: *mut ffi::PyObject, value: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: METH_O: `slf` is a tuple, `value` a live object.
    match unsafe { tuple_prelude(slf, value, Scan::Count).unwrap_or_else(|| scan_tuple_slow(slf, value, Scan::Count)) } {
        // SAFETY: attached thread.
        Outcome::Done(r) => unsafe { count_result(r) },
        // SAFETY: same arguments to the original METH_O function.
        Outcome::Delegate => unsafe { (originals().tuple_count)(slf, value) },
    }
}

// ---------------------------------------------------------------------------------------
// Kernel: `list.sort` (and `sorted`, which calls `sort` on its fresh list).
// ---------------------------------------------------------------------------------------

/// `true` when `name` (a keyword name) equals the ASCII string `s`.
///
/// # Safety
/// `name` must be a `str`.
#[inline]
unsafe fn kw_is(name: *mut ffi::PyObject, s: &CStr) -> bool {
    // SAFETY: str object, NUL-terminated ASCII literal; this comparison never fails.
    unsafe { ffi::PyUnicode_CompareWithASCIIString(name, s.as_ptr()) == 0 }
}

/// Parse `sort(*, key=None, reverse=<exact bool or int>)` without running Python code.
/// `None` means "let the original parse it".
///
/// # Safety
/// Vectorcall arguments as received by a METH_FASTCALL|METH_KEYWORDS function.
unsafe fn sort_args(args: *const *mut ffi::PyObject, nargs: ffi::Py_ssize_t, kwnames: *mut ffi::PyObject) -> Option<bool> {
    if nargs != 0 {
        return None;
    }
    if kwnames.is_null() {
        return Some(false);
    }
    let mut reverse = false;
    // SAFETY: kwnames is a tuple of str; values follow the positional arguments.
    unsafe {
        for j in 0..ffi::PyTuple_GET_SIZE(kwnames) {
            let name = ffi::PyTuple_GET_ITEM(kwnames, j);
            let value = *args.offset(nargs + j);
            if kw_is(name, c"key") {
                if value != ffi::Py_None() {
                    return None;
                }
            } else if kw_is(name, c"reverse") {
                let t = ffi::Py_TYPE(value);
                if t == bool_type() {
                    reverse = value == ffi::Py_True();
                } else if t == long_type() {
                    reverse = ffi::PyObject_IsTrue(value) == 1; // exact int: pure
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }
    }
    Some(reverse)
}

/// Sort `items` in place by `keys` (stable; descending keeps equal elements in their
/// original order, as CPython's reverse-sort-reverse does).
fn sort_pairs<K>(pairs: &mut [(K, *mut ffi::PyObject)], reverse: bool, cmp: impl Fn(&K, &K) -> Ordering) {
    if reverse {
        pairs.sort_by(|a, b| cmp(&b.0, &a.0));
    } else {
        pairs.sort_by(|a, b| cmp(&a.0, &b.0));
    }
}

/// Order-preserving 64-bit prefix of a string: 8 bytes for 1-byte-kind strings (`latin`),
/// else 3 code points of 21 bits. Equal prefixes are resolved by a full comparison.
///
/// # Safety
/// `p` describes a live string.
#[inline]
unsafe fn str_prefix(p: &StrParts, latin: bool) -> u64 {
    let mut out = 0u64;
    if latin {
        for i in 0..8 {
            // SAFETY: i < len checked.
            let c = if i < p.len { u64::from(unsafe { *p.data.add(i) }) } else { 0 };
            out = (out << 8) | c;
        }
    } else {
        for i in 0..3 {
            // SAFETY: i < len checked.
            let c = if i < p.len { u64::from(unsafe { code_point(p, i) }) } else { 0 };
            out = (out << 21) | c;
        }
    }
    out
}

/// Try the native sort. `None`: not applicable (nothing was changed).
///
/// # Safety
/// `list` is a list whose critical section the caller holds; no Python code runs here.
unsafe fn try_sort(list: *mut ffi::PyObject, reverse: bool) -> Option<()> {
    // SAFETY: list layout reads/writes under the list's critical section (or the GIL);
    // only pointers are permuted, no reference counts change.
    unsafe {
        let n = ffi::PyList_GET_SIZE(list) as usize;
        if n < 2 {
            return Some(());
        }
        let items = std::slice::from_raw_parts_mut((*list.cast::<ffi::PyListObject>()).ob_item, n);
        let t0 = ffi::Py_TYPE(items[0]);
        if t0 == long_type() || t0 == bool_type() {
            sort_by_keys(items, reverse, |p| {
                let t = ffi::Py_TYPE(p);
                if t != long_type() && t != bool_type() {
                    return None;
                }
                int_value(p)
            }, i64::cmp)
        } else if t0 == float_type() {
            // No NaN: `<` is a strict weak order (-0.0 and 0.0 are equivalent and keep
            // their order, like CPython; `total_cmp` would separate them). With NaNs
            // CPython's order is algorithm-specific: not handled.
            sort_by_keys(items, reverse, |p| {
                if ffi::Py_TYPE(p) != float_type() {
                    return None;
                }
                let v = float_value(p);
                if v.is_nan() { None } else { Some(v) }
            }, |a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal))
        } else if t0 == str_type() {
            let mut latin = true;
            for &p in items.iter() {
                if ffi::Py_TYPE(p) != str_type() {
                    return None;
                }
                if str_kind_data(p, STR_LAYOUT.get()).0 != ffi::PyUnicode_1BYTE_KIND {
                    latin = false;
                }
            }
            sort_by_keys(items, reverse, |p| {
                let parts = str_parts(p);
                Some((str_prefix(&parts, latin), parts))
            }, |a, b| a.0.cmp(&b.0).then_with(|| str_cmp(&a.1, &b.1)))
        } else {
            None
        }
    }
}

/// Up to this many elements, keys live on the stack (no allocation: small sorts stay
/// cheap and allocator-noise free; Rust's stable sort needs no heap scratch at this size).
const STACK_SORT: usize = 128;

/// Extract a key per item (`None`: not applicable, nothing changed), stable-sort the
/// `(key, item)` pairs and write the items back in order.
///
/// # Safety
/// `items` is the list's item array (critical section held); `key` runs no Python code.
#[inline(always)]
unsafe fn sort_by_keys<K: Copy>(
    items: &mut [*mut ffi::PyObject],
    reverse: bool,
    key: impl Fn(*mut ffi::PyObject) -> Option<K>,
    cmp: impl Fn(&K, &K) -> Ordering,
) -> Option<()> {
    let n = items.len();
    let mut heap: Vec<(K, *mut ffi::PyObject)> = Vec::new();
    let mut stack: [std::mem::MaybeUninit<(K, *mut ffi::PyObject)>; STACK_SORT] = [const { std::mem::MaybeUninit::uninit() }; STACK_SORT];
    let pairs: &mut [(K, *mut ffi::PyObject)] = if n <= STACK_SORT {
        for (slot, &p) in stack.iter_mut().zip(items.iter()) {
            slot.write((key(p)?, p));
        }
        // SAFETY: the first `n` slots were initialised just above; `(K, ptr)` is Copy.
        unsafe { std::slice::from_raw_parts_mut(stack.as_mut_ptr().cast::<(K, *mut ffi::PyObject)>(), n) }
    } else {
        heap.try_reserve_exact(n).ok()?;
        for &p in items.iter() {
            heap.push((key(p)?, p));
        }
        &mut heap
    };
    sort_pairs(pairs, reverse, cmp);
    for (dst, (_, p)) in items.iter_mut().zip(pairs.iter()) {
        *dst = *p;
    }
    Some(())
}

unsafe extern "C" fn k_list_sort(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let o = originals();
    // SAFETY: vectorcall arguments from CPython.
    let Some(reverse) = (unsafe { sort_args(args, nargs, kwnames) }) else {
        // SAFETY: same arguments to the original.
        return unsafe { (o.list_sort)(slf, args, nargs, kwnames) };
    };
    let done = guard(None, || {
        // SAFETY: attached thread, `slf` is a live list.
        let py = unsafe { Python::assume_attached() };
        // SAFETY: `slf` is a live list borrowed for the duration of the call.
        let list = unsafe { Borrowed::from_ptr(py, slf) };
        // SAFETY: the list's critical section is held; no Python code runs in `try_sort`.
        with_critical_section(&list, || unsafe { try_sort(slf, reverse) })
    });
    // SAFETY: attached thread.
    if unsafe { !ffi::PyErr_Occurred().is_null() } {
        return std::ptr::null_mut();
    }
    match done {
        // SAFETY: new reference to None.
        Some(()) => unsafe { ffi::Py_NewRef(ffi::Py_None()) },
        // SAFETY: same arguments to the original.
        None => unsafe { (o.list_sort)(slf, args, nargs, kwnames) },
    }
}

// ---------------------------------------------------------------------------------------
// Installation.
// ---------------------------------------------------------------------------------------

/// Is `[addr, addr + len)` inside a writable mapping of this process? (Linux: from
/// `/proc/self/maps`; elsewhere the static method tables of CPython are writable data.)
fn writable(addr: usize, len: usize) -> bool {
    if cfg!(target_os = "linux") {
        let Ok(maps) = std::fs::read_to_string("/proc/self/maps") else { return false };
        for line in maps.lines() {
            let mut parts = line.split_whitespace();
            let (Some(range), Some(perm)) = (parts.next(), parts.next()) else { continue };
            let Some((lo, hi)) = range.split_once('-') else { continue };
            let (Ok(lo), Ok(hi)) = (usize::from_str_radix(lo, 16), usize::from_str_radix(hi, 16)) else { continue };
            if lo <= addr && addr + len <= hi {
                return perm.as_bytes().get(1) == Some(&b'w');
            }
        }
        false
    } else {
        true
    }
}

/// The static `PyMethodDef` behind a method descriptor or builtin function, checked for the
/// expected name and calling convention.
///
/// # Safety
/// Attached thread.
unsafe fn method_def(obj: &Bound<'_, PyAny>, name: &CStr, flags: c_int) -> Option<*mut ffi::PyMethodDef> {
    // SAFETY: type-checked layout reads.
    unsafe {
        let p = obj.as_ptr();
        let def = if ffi::Py_TYPE(p) == &raw mut ffi::PyMethodDescr_Type {
            (*p.cast::<ffi::PyMethodDescrObject>()).d_method
        } else if ffi::PyCFunction_Check(p) != 0 {
            (*p.cast::<ffi::PyCFunctionObject>()).m_ml
        } else {
            return None;
        };
        if def.is_null() || (*def).ml_meth.is_null() || (*def).ml_name.is_null() {
            return None;
        }
        if CStr::from_ptr((*def).ml_name) != name || (*def).ml_flags != flags {
            return None;
        }
        Some(def)
    }
}

/// Address of the `ml_meth` field of a method def.
fn ml_meth_slot(def: *mut ffi::PyMethodDef) -> usize {
    // SAFETY: address computation only.
    unsafe { (&raw mut (*def).ml_meth) as usize }
}

/// Address of `tp_as_sequence->sq_contains` of a static type.
///
/// # Safety
/// `t` is a ready type object with a sequence table.
unsafe fn sq_contains_slot(t: *mut ffi::PyTypeObject) -> Option<usize> {
    // SAFETY: reads of a ready static type.
    unsafe {
        let seq = (*t).tp_as_sequence;
        if seq.is_null() || (*seq).sq_contains.is_none() {
            return None;
        }
        Some((&raw mut (*seq).sq_contains) as usize)
    }
}

/// Atomically store a function pointer into a patch site.
///
/// # Safety
/// `slot` is a valid, writable, pointer-aligned function-pointer field.
unsafe fn store(slot: usize, value: usize) {
    // SAFETY: per the caller; an aligned pointer-sized atomic store, so concurrent readers
    // see either the old or the new function.
    unsafe { (*(slot as *const std::sync::atomic::AtomicUsize)).store(value, std::sync::atomic::Ordering::Release) };
}

/// Capture the `ValueError` message the original `index` raises for a missing value.
fn index_message(py: Python<'_>, original: FastFn, seq: &Bound<'_, PyAny>) -> PyResult<std::ffi::CString> {
    let missing = PyTuple::empty(py); // never equal to an element of an empty sequence
    let argv = [missing.as_ptr()];
    // SAFETY: calling the original METH_FASTCALL function with one argument.
    let r = unsafe { original(seq.as_ptr(), argv.as_ptr(), 1) };
    if !r.is_null() {
        // SAFETY: owned reference returned by the call.
        unsafe { ffi::Py_DECREF(r) };
        return Err(PyRuntimeError::new_err("index() of an empty sequence did not raise"));
    }
    let err = PyErr::fetch(py);
    if !err.is_instance_of::<PyValueError>(py) {
        return Err(err);
    }
    let text = err.value(py).str()?.to_string();
    std::ffi::CString::new(text).map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

/// Sanity-check the raw int/float layouts against the running interpreter.
fn layouts_ok(py: Python<'_>) -> PyResult<bool> {
    for v in [0i64, 1, -1, 12345, -(1 << 29), (1 << 30) - 1] {
        let obj = v.into_pyobject(py)?;
        // SAFETY: exact int objects.
        if unsafe { compact_value(obj.as_ptr()) } != Some(v) {
            return Ok(false);
        }
    }
    for v in [1i64 << 30, -(1 << 40), i64::MAX, i64::MIN] {
        let obj = v.into_pyobject(py)?;
        // SAFETY: exact int objects.
        if unsafe { compact_value(obj.as_ptr()) }.is_some() || unsafe { int_value(obj.as_ptr()) } != Some(v) {
            return Ok(false);
        }
    }
    let f = 2.5f64.into_pyobject(py)?;
    // SAFETY: exact float.
    Ok(unsafe { float_value(f.as_ptr()) } == 2.5)
}

/// Install the safe-tier kernels. Idempotent. Returns the names of the active kernels.
#[pyfunction]
fn install(py: Python<'_>) -> PyResult<Vec<&'static str>> {
    let mut installed = INSTALLED.lock().map_err(|_| PyRuntimeError::new_err("takeover state poisoned"))?;
    if !installed.is_empty() {
        return Ok(installed.iter().map(|s| s.name).collect());
    }
    if !layouts_ok(py)? {
        return Err(PyRuntimeError::new_err("rustique takeover: unexpected int/float layout"));
    }
    if STR_LAYOUT.get().is_none()
        && let Some(layout) = calibrate_str(py)
    {
        let _ = STR_LAYOUT.set(layout);
    }
    let list_t = py.get_type::<PyList>();
    let tuple_t = py.get_type::<PyTuple>();
    let fast_kw = ffi::METH_FASTCALL | ffi::METH_KEYWORDS;
    // SAFETY: attached thread; descriptors/functions are kept alive by their owners.
    let defs = unsafe {
        [
            ("list.sort", method_def(&list_t.getattr("sort")?, c"sort", fast_kw), k_list_sort as FastKwFn as usize),
            ("list.index", method_def(&list_t.getattr("index")?, c"index", ffi::METH_FASTCALL), k_list_index as FastFn as usize),
            ("list.count", method_def(&list_t.getattr("count")?, c"count", ffi::METH_O), k_list_count as OFn as usize),
            ("tuple.index", method_def(&tuple_t.getattr("index")?, c"index", ffi::METH_FASTCALL), k_tuple_index as FastFn as usize),
            ("tuple.count", method_def(&tuple_t.getattr("count")?, c"count", ffi::METH_O), k_tuple_count as OFn as usize),
        ]
    };
    // SAFETY: ready static types.
    let list_contains = unsafe { sq_contains_slot(&raw mut ffi::PyList_Type) };
    // SAFETY: ready static type.
    let tuple_contains = unsafe { sq_contains_slot(&raw mut ffi::PyTuple_Type) };

    let mut sites = Vec::new();
    for (name, def, kernel) in defs {
        let Some(def) = def else { continue };
        let slot = ml_meth_slot(def);
        if writable(slot, size_of::<usize>()) {
            // SAFETY: valid pointer-sized field.
            let original = unsafe { *(slot as *const usize) };
            sites.push(Site { name, slot, original, kernel });
        }
    }
    for (name, slot, kernel) in [
        ("x in list", list_contains, k_list_contains as ContainsFn as usize),
        ("x in tuple", tuple_contains, k_tuple_contains as ContainsFn as usize),
    ] {
        let Some(slot) = slot else { continue };
        if writable(slot, size_of::<usize>()) {
            // SAFETY: valid pointer-sized field.
            let original = unsafe { *(slot as *const usize) };
            sites.push(Site { name, slot, original, kernel });
        }
    }
    // Every kernel needs its original; install all or nothing.
    let expected = ["list.sort", "list.index", "list.count", "tuple.index", "tuple.count", "x in list", "x in tuple"];
    if sites.len() != expected.len() {
        let found: Vec<_> = sites.iter().map(|s| s.name).collect();
        let missing: Vec<_> = expected.iter().filter(|n| !found.contains(n)).collect();
        return Err(PyRuntimeError::new_err(format!("rustique takeover: cannot patch {missing:?} on this interpreter")));
    }
    if ORIGINALS.get().is_none() {
        let orig = |name: &str| sites.iter().find(|s| s.name == name).map(|s| s.original).unwrap_or(0);
        // SAFETY: each original was read from a method def / slot with the matching
        // calling convention, verified above.
        let originals = unsafe {
            Originals {
                list_sort: std::mem::transmute::<usize, FastKwFn>(orig("list.sort")),
                list_index: std::mem::transmute::<usize, FastFn>(orig("list.index")),
                list_count: std::mem::transmute::<usize, OFn>(orig("list.count")),
                tuple_index: std::mem::transmute::<usize, FastFn>(orig("tuple.index")),
                tuple_count: std::mem::transmute::<usize, OFn>(orig("tuple.count")),
                list_contains: std::mem::transmute::<usize, ContainsFn>(orig("x in list")),
                tuple_contains: std::mem::transmute::<usize, ContainsFn>(orig("x in tuple")),
                list_index_msg: index_message(
                    py,
                    std::mem::transmute::<usize, FastFn>(orig("list.index")),
                    PyList::empty(py).as_any(),
                )?,
                tuple_index_msg: index_message(
                    py,
                    std::mem::transmute::<usize, FastFn>(orig("tuple.index")),
                    PyTuple::empty(py).as_any(),
                )?,
            }
        };
        ORIG_LIST_CONTAINS.store(originals.list_contains as usize, std::sync::atomic::Ordering::Relaxed);
        ORIG_TUPLE_CONTAINS.store(originals.tuple_contains as usize, std::sync::atomic::Ordering::Relaxed);
        let _ = ORIGINALS.set(originals);
    }
    let o = originals();
    // A second install after uninstall must find the same originals (nobody else patched).
    for s in &sites {
        let saved = match s.name {
            "list.sort" => o.list_sort as usize,
            "list.index" => o.list_index as usize,
            "list.count" => o.list_count as usize,
            "tuple.index" => o.tuple_index as usize,
            "tuple.count" => o.tuple_count as usize,
            "x in list" => o.list_contains as usize,
            _ => o.tuple_contains as usize,
        };
        if saved != s.original {
            return Err(PyRuntimeError::new_err(format!("rustique takeover: {} was patched by someone else", s.name)));
        }
    }
    for s in &sites {
        // SAFETY: writable, aligned function-pointer field verified above.
        unsafe { store(s.slot, s.kernel) };
    }
    *installed = sites;
    Ok(installed.iter().map(|s| s.name).collect())
}

/// Restore every original function pointer. Idempotent.
#[pyfunction]
fn uninstall() -> PyResult<()> {
    let mut installed = INSTALLED.lock().map_err(|_| PyRuntimeError::new_err("takeover state poisoned"))?;
    for s in installed.drain(..) {
        // SAFETY: the site was verified writable at install time.
        unsafe { store(s.slot, s.original) };
    }
    Ok(())
}

/// Whether `str` kind/data are read inline (a calibrated layout), for tests and reports.
#[pyfunction]
fn inline_str_layout() -> bool {
    STR_LAYOUT.get().is_some()
}

/// Names of the active kernels (empty when not installed).
#[pyfunction]
fn installed() -> PyResult<Vec<&'static str>> {
    let installed = INSTALLED.lock().map_err(|_| PyRuntimeError::new_err("takeover state poisoned"))?;
    Ok(installed.iter().map(|s| s.name).collect())
}

// ---------------------------------------------------------------------------------------
// Probes emulating typical PyO3 extension code (boundary-compatibility tests).
// ---------------------------------------------------------------------------------------

/// `obj.cast::<PyList>()` (a non-exact `PyList_Check`) followed by item reads.
#[pyfunction]
fn _pyo3_probe_list(obj: &Bound<'_, PyAny>) -> PyResult<Vec<Py<PyAny>>> {
    let list = obj.cast::<PyList>()?;
    (0..list.len()).map(|i| list.get_item(i).map(Bound::unbind)).collect()
}

/// `Vec<i64>` extraction (sequence protocol).
#[pyfunction]
fn _pyo3_probe_vec(obj: &Bound<'_, PyAny>) -> PyResult<Vec<i64>> {
    obj.extract()
}

/// `obj.cast::<PyDict>()` followed by iteration over the dict's own table.
#[pyfunction]
fn _pyo3_probe_dict(obj: &Bound<'_, PyAny>) -> PyResult<Vec<(Py<PyAny>, Py<PyAny>)>> {
    let dict = obj.cast::<PyDict>()?;
    Ok(dict.iter().map(|(k, v)| (k.unbind(), v.unbind())).collect())
}

/// `obj.cast::<PySet>()` followed by iteration over the set's own table (sorted by repr
/// for a deterministic comparison).
#[pyfunction]
fn _pyo3_probe_set(obj: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let set = obj.cast::<PySet>()?;
    let mut out = set.iter().map(|v| v.repr().map(|r| r.to_string())).collect::<PyResult<Vec<_>>>()?;
    out.sort();
    Ok(out)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = PyModule::new(m.py(), "_takeover")?;
    sub.add_function(wrap_pyfunction!(install, &sub)?)?;
    sub.add_function(wrap_pyfunction!(uninstall, &sub)?)?;
    sub.add_function(wrap_pyfunction!(installed, &sub)?)?;
    sub.add_function(wrap_pyfunction!(_pyo3_probe_list, &sub)?)?;
    sub.add_function(wrap_pyfunction!(_pyo3_probe_vec, &sub)?)?;
    sub.add_function(wrap_pyfunction!(_pyo3_probe_dict, &sub)?)?;
    sub.add_function(wrap_pyfunction!(_pyo3_probe_set, &sub)?)?;
    sub.add("FREE_THREADED", FREE_THREADED)?;
    sub.add_function(wrap_pyfunction!(inline_str_layout, &sub)?)?;
    m.add("_takeover", sub)
}
