//! Takeover, safe tier (docs/TAKEOVER.md): exact-semantics Rust kernels behind the builtin
//! container methods and builtin functions, process-wide.
//!
//! OWNER: accel-core (group `containers` of docs/ACCELERATION.md: targets `list.sort`,
//! `list.index`, `list.count`, `tuple.index`, `tuple.count`, `list.__contains__`,
//! `tuple.__contains__`, registered as `TargetSpec`s in [`TARGETS`]).
//!
//! Mechanism. CPython reaches `list.sort`, `list.index`, `list.count`, `tuple.index` and
//! `tuple.count` through the `ml_meth` pointer of a static `PyMethodDef`; every method
//! descriptor, bound method and specialized call site (`CALL_METHOD_DESCRIPTOR_*`) reads
//! that pointer at call time (`sorted()` calls `sort` on its fresh list, so it is covered
//! too). The builtin *functions* (`sum`, `min`, `max`, `sorted`, `any`, `all`) belong to
//! `rustique._builtins` (workstream 5), which `rustique.enable()` installs alongside. `x in list` / `x in tuple` go through the `sq_contains` slot.
//! The registry (`crate::accel`) swaps those function pointers for the kernels below and
//! keeps the originals (`Original` statics), clearing the 3.15 JIT's traces afterwards.
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
//! lock-free `list_contains`. Installing patches process-global function pointers (atomic
//! stores: a concurrent call sees either function).

use std::cmp::Ordering;
use std::ffi::{CStr, c_int};
use std::panic::{self, AssertUnwindSafe};
use std::sync::OnceLock;
use std::sync::atomic::AtomicPtr;

use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::sync::critical_section::with_critical_section;
use pyo3::types::{PyDict, PyList, PySet, PyString, PyTuple};

use crate::accel::{self, Counters, Original, Prepare, Reason, Refusal, SlotName, TargetSpec};
use crate::refuse;

/// Free-threaded build: the object header carries thread id and shared refcount fields.
const FREE_THREADED: bool = size_of::<ffi::PyObject>() > 2 * size_of::<usize>();

type FastFn =
    unsafe extern "C" fn(*mut ffi::PyObject, *const *mut ffi::PyObject, ffi::Py_ssize_t) -> *mut ffi::PyObject;
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

/// How a scan walks its sequence. `fast` decides the comparison of one (non-identical)
/// element without running Python code where it can; `None` asks for CPython's
/// `PyObject_RichCompareBool(item, needle, Py_EQ)` (item on the left, like CPython).
trait Driver {
    /// `cont`: an element `fast` cannot decide is compared generically right away (the
    /// scan continues CPython's loop from there) instead of restarting in the original.
    ///
    /// # Safety
    /// `needle` is live; attached thread; `fast` runs no Python code.
    unsafe fn run(
        &self,
        needle: *mut ffi::PyObject,
        mode: Scan,
        cont: bool,
        fast: impl Fn(*mut ffi::PyObject) -> Option<bool>,
    ) -> Outcome;
}

/// What one element contributes to a scan.
#[inline(always)]
fn tally(hit: bool, i: isize, mode: Scan, count: &mut isize) -> Option<Outcome> {
    if hit {
        if mode == Scan::First {
            return Some(Outcome::Done(Ok(Some(i))));
        }
        *count += 1;
    }
    None
}

/// Generic comparison of one element with a strong reference held (Python code may run).
///
/// # Safety
/// `item` is a live borrowed reference; attached thread.
#[inline(never)]
unsafe fn generic_eq(item: *mut ffi::PyObject, needle: *mut ffi::PyObject) -> c_int {
    // SAFETY: per the caller.
    unsafe {
        ffi::Py_INCREF(item);
        let r = ffi::PyObject_RichCompareBool(item, needle, ffi::Py_EQ);
        ffi::Py_DECREF(item);
        r
    }
}

/// Borrowed items read directly: GIL builds (no other thread runs while no Python code
/// does), and tuples on every build (immutable). `get` returns NULL past the end; for lists
/// it re-reads the size and the item array every step (`__eq__` code may change them).
struct Direct<G>(G);

impl<G: Fn(isize) -> *mut ffi::PyObject> Driver for Direct<G> {
    #[inline(always)]
    unsafe fn run(
        &self,
        needle: *mut ffi::PyObject,
        mode: Scan,
        cont: bool,
        fast: impl Fn(*mut ffi::PyObject) -> Option<bool>,
    ) -> Outcome {
        let (mut i, mut count, mut ran_python) = (0isize, 0isize, cont);
        loop {
            let item = (self.0)(i);
            if item.is_null() {
                break;
            }
            let hit = if item == needle {
                1
            } else {
                match fast(item) {
                    Some(b) => c_int::from(b),
                    None if !ran_python && i < DELEGATE_BEFORE => return Outcome::Delegate,
                    None => {
                        ran_python = true;
                        // SAFETY: live item of the sequence, compared with a reference held.
                        unsafe { generic_eq(item, needle) }
                    }
                }
            };
            if hit < 0 {
                return Outcome::Done(Err(()));
            }
            if let Some(done) = tally(hit > 0, i, mode, &mut count) {
                return done;
            }
            i += 1;
        }
        Outcome::Done(Ok(if mode == Scan::First { None } else { Some(count) }))
    }
}

/// Free-threaded short lists: each element is taken as a strong reference
/// (`PyList_GetItemRef`, CPython's lock-free protocol), so no critical section is paid.
struct Owned(*mut ffi::PyObject);

impl Driver for Owned {
    #[inline(always)]
    unsafe fn run(
        &self,
        needle: *mut ffi::PyObject,
        mode: Scan,
        cont: bool,
        fast: impl Fn(*mut ffi::PyObject) -> Option<bool>,
    ) -> Outcome {
        // SAFETY: forwarded.
        unsafe { owned_scan(self.0, needle, mode, 0, 0, cont, fast) }
    }
}

/// The per-element-reference scan from index `i`.
///
/// # Safety
/// As for [`Driver::run`]; `list` is a list.
#[inline(always)]
unsafe fn owned_scan(
    list: *mut ffi::PyObject,
    needle: *mut ffi::PyObject,
    mode: Scan,
    mut i: isize,
    mut count: isize,
    mut ran_python: bool,
    fast: impl Fn(*mut ffi::PyObject) -> Option<bool>,
) -> Outcome {
    // SAFETY: every element is held through its own strong reference while used.
    unsafe {
        loop {
            if i >= ffi::PyList_GET_SIZE(list) {
                break;
            }
            let item = ffi::PyList_GetItemRef(list, i);
            if item.is_null() {
                ffi::PyErr_Clear(); // shrunk concurrently: out of bounds
                break;
            }
            let hit = if item == needle {
                1
            } else {
                match fast(item) {
                    Some(b) => c_int::from(b),
                    None if !ran_python && i < DELEGATE_BEFORE => {
                        ffi::Py_DECREF(item);
                        return Outcome::Delegate;
                    }
                    None => {
                        ran_python = true;
                        ffi::PyObject_RichCompareBool(item, needle, ffi::Py_EQ)
                    }
                }
            };
            ffi::Py_DECREF(item);
            if hit < 0 {
                return Outcome::Done(Err(()));
            }
            if let Some(done) = tally(hit > 0, i, mode, &mut count) {
                return done;
            }
            i += 1;
        }
    }
    Outcome::Done(Ok(if mode == Scan::First { None } else { Some(count) }))
}

/// Type flags of needles no fast path decides, rejected by one test: instances of classes
/// defined in Python (heap types; Python subclasses of `int`/`str` too, which the scans'
/// exact checks would reject anyway) and the builtin containers. These are the common
/// non-scalar needles (`objs.count(o)`, `(a, b) in pairs`).
const GENERIC_FLAGS: std::ffi::c_ulong = ffi::Py_TPFLAGS_HEAPTYPE
    | ffi::Py_TPFLAGS_TUPLE_SUBCLASS
    | ffi::Py_TPFLAGS_LIST_SUBCLASS
    | ffi::Py_TPFLAGS_DICT_SUBCLASS
    | ffi::Py_TPFLAGS_BYTES_SUBCLASS;

/// `tp_flags` of a type, read once (an atomic on free-threaded builds).
///
/// # Safety
/// `t` must be a live type object.
#[inline(always)]
unsafe fn type_flags(t: *mut ffi::PyTypeObject) -> std::ffi::c_ulong {
    // SAFETY: a live type object.
    #[cfg(not(Py_GIL_DISABLED))]
    unsafe {
        (*t).tp_flags
    }
    // SAFETY: a live type object; relaxed, as `PyType_HasFeature`.
    #[cfg(Py_GIL_DISABLED)]
    unsafe {
        (*t).tp_flags.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// A needle no fast path can decide: [`GENERIC_FLAGS`], or anything [`maybe_scalar`]
/// rejects. Such a needle delegates at once (the fallback stays a few instructions: the
/// scoreboard's 1-element `objs.count(o)` costs CPython ~300 Ir).
///
/// # Safety
/// `op` must be a live object.
#[inline(always)]
unsafe fn needs_generic(op: *mut ffi::PyObject) -> bool {
    // SAFETY: type object reads of a live object; `Py_None` is a static object.
    unsafe {
        let t = ffi::Py_TYPE(op);
        let f = type_flags(t);
        if f & GENERIC_FLAGS != 0 {
            return true;
        }
        !(f & (ffi::Py_TPFLAGS_LONG_SUBCLASS | ffi::Py_TPFLAGS_UNICODE_SUBCLASS) != 0
            || t == float_type()
            || op == ffi::Py_None())
    }
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

#[inline(always)]
fn tuple_type() -> *mut ffi::PyTypeObject {
    &raw mut ffi::PyTuple_Type
}

/// `t`'s `==` is `object`'s (identity; `NotImplemented` against anything else): no
/// `tp_richcompare` of its own (classes with metatype `type`, plain instances, functions,
/// modules, `None`).
///
/// # Safety
/// `t` is a live type.
#[inline(always)]
unsafe fn identity_eq_type(t: *mut ffi::PyTypeObject) -> bool {
    // SAFETY: live type objects; `PyBaseObject_Type` is static.
    unsafe {
        match (*t).tp_richcompare {
            None => true,
            Some(f) => Some(f as usize) == ffi::PyBaseObject_Type.tp_richcompare.map(|o| o as usize),
        }
    }
}

/// An item that compares unequal to a needle of another kind without running code: an
/// identity-`==` object, or an exact `int`/`bool`/`float`/`str`/`None` (their slots return
/// `NotImplemented` for foreign types, and so do `object`'s and `tuple`'s for them).
///
/// # Safety
/// `item` is live.
#[inline(always)]
unsafe fn identity_safe_item(item: *mut ffi::PyObject) -> bool {
    // SAFETY: a live object.
    unsafe {
        let t = ffi::Py_TYPE(item);
        t == long_type() || t == str_type() || t == float_type() || t == bool_type() || identity_eq_type(t)
    }
}

/// A needle the identity or tuple scans decide (see [`scan_for`]).
///
/// # Safety
/// `op` is live.
#[inline(always)]
unsafe fn special_needle(op: *mut ffi::PyObject) -> bool {
    // SAFETY: a live object.
    unsafe {
        let t = ffi::Py_TYPE(op);
        t == tuple_type() || identity_eq_type(t)
    }
}

/// Nesting depth of tuples compared without running code.
const TUPLE_EQ_DEPTH: u32 = 8;

/// `a == b` for exact tuples while every element comparison runs no code, as
/// `tuplerichcompare(a, b, Py_EQ)` computes it (no early length test: the first unequal
/// element pair decides, else the lengths); `None` at the first element pair that could run
/// code.
///
/// # Safety
/// Live exact tuples.
unsafe fn tuple_eq_pure(a: *mut ffi::PyObject, b: *mut ffi::PyObject, depth: u32) -> Option<bool> {
    // SAFETY: tuple items are live while the tuples are.
    unsafe {
        let (na, nb) = (ffi::PyTuple_GET_SIZE(a), ffi::PyTuple_GET_SIZE(b));
        for i in 0..na.min(nb) {
            let (x, y) = (ffi::PyTuple_GET_ITEM(a, i), ffi::PyTuple_GET_ITEM(b, i));
            if x == y {
                continue;
            }
            if !elem_eq_pure(x, y, depth)? {
                return Some(false);
            }
        }
        Some(na == nb)
    }
}

/// `x == y` (`PyObject_RichCompareBool` after its identity test, `x` not `y`) for elements
/// whose comparison runs no code; `None` otherwise.
///
/// # Safety
/// Live objects.
#[inline(always)]
unsafe fn elem_eq_pure(x: *mut ffi::PyObject, y: *mut ffi::PyObject, depth: u32) -> Option<bool> {
    // SAFETY: exact-type reads; `PyObject_RichCompareBool` of exact builtin numbers runs
    // only C code.
    unsafe {
        let (tx, ty) = (ffi::Py_TYPE(x), ffi::Py_TYPE(y));
        let num = |t: *mut ffi::PyTypeObject| t == long_type() || t == bool_type() || t == float_type();
        if tx == str_type() && ty == str_type() {
            let (a, b) = (str_parts(x), str_parts(y));
            if a.len != b.len || (a.hash != -1 && b.hash != -1 && a.hash != b.hash) || a.kind != b.kind {
                return Some(false);
            }
            let n = a.len * a.kind as usize;
            return Some(std::slice::from_raw_parts(a.data, n) == std::slice::from_raw_parts(b.data, n));
        }
        if num(tx) && num(ty) {
            if tx != float_type()
                && ty != float_type()
                && let (Some(a), Some(b)) = (compact_value(x), compact_value(y))
            {
                return Some(a == b);
            }
            let r = ffi::PyObject_RichCompareBool(x, y, ffi::Py_EQ);
            if r < 0 {
                ffi::PyErr_Clear();
                return None;
            }
            return Some(r == 1);
        }
        if tx == tuple_type() && ty == tuple_type() {
            return if depth < TUPLE_EQ_DEPTH { tuple_eq_pure(x, y, depth + 1) } else { None };
        }
        // Any other pair of str / numbers / tuples / identity-`==` objects (not identical):
        // `NotImplemented` both ways, then identity: unequal.
        let plain = |t: *mut ffi::PyTypeObject| t == str_type() || num(t) || t == tuple_type() || identity_eq_type(t);
        (plain(tx) && plain(ty)).then_some(false)
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
/// As for [`Driver::run`].
#[inline(always)]
unsafe fn scan_for(driver: impl Driver, needle: *mut ffi::PyObject, mode: Scan) -> Outcome {
    // SAFETY: values are read according to the exact types checked first.
    unsafe {
        let t = ffi::Py_TYPE(needle);
        let none = ffi::Py_None();
        if t == long_type() || t == bool_type() {
            if let Some(v) = compact_value(needle) {
                return driver.run(needle, mode, false, |item| {
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
            return driver.run(needle, mode, false, |item| {
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
            return driver.run(needle, mode, false, |item| {
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
            return driver.run(needle, mode, false, |item| {
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
                    Some(
                        kind == np.kind
                            && std::slice::from_raw_parts(data, nbytes) == std::slice::from_raw_parts(np.data, nbytes),
                    )
                } else if t == long_type() || t == bool_type() || t == float_type() || item == none {
                    Some(false)
                } else {
                    None
                }
            });
        }
        if needle == none {
            return driver.run(needle, mode, false, |item| {
                let t = ffi::Py_TYPE(item);
                if t == long_type() || t == bool_type() || t == float_type() || t == str_type() {
                    Some(false)
                } else {
                    None
                }
            });
        }
        // Needles whose `==` is identity (classes, functions, plain instances): unequal to
        // any other item whose comparison runs no code; from the first other item on,
        // CPython's loop continues (no restart: nothing observable happened before it).
        if identity_eq_type(t) {
            return driver.run(needle, mode, true, |item| if identity_safe_item(item) { Some(false) } else { None });
        }
        // Exact tuples (`(a, b) in pairs`): `tuplerichcompare` element by element while the
        // elements compare without running code.
        if t == tuple_type() {
            return driver.run(needle, mode, true, |item| {
                if ffi::Py_TYPE(item) == tuple_type() {
                    tuple_eq_pure(item, needle, 0)
                } else if identity_safe_item(item) {
                    Some(false)
                } else {
                    None
                }
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
    // SAFETY: see the drivers.
    unsafe {
        if FREE_THREADED {
            // CPython's own lock-free protocol (a strong reference per element, taken with
            // `PyList_GetItemRef`): threads scanning one shared list never serialize on its
            // mutex (a whole-list critical section made `x in shared_list` 1.8-2.1x slower
            // than plain at 8 threads, `docs/atlas/w2-threads.md` §3.1), and immortal items
            // (small ints, interned strings) cost no reference-count traffic.
            scan_for(Owned(list), needle, mode)
        } else {
            scan_for(
                Direct(|i| {
                    if i >= ffi::PyList_GET_SIZE(list) {
                        return std::ptr::null_mut();
                    }
                    *(*list.cast::<ffi::PyListObject>()).ob_item.offset(i)
                }),
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
        scan_for(Direct(|i| if i < n { ffi::PyTuple_GET_ITEM(tuple, i) } else { std::ptr::null_mut() }), needle, mode)
    }
}

/// Free-threaded lists: what can be decided before scanning, lock-free: an empty list, the
/// needle being the first element, a needle or first element of a foreign type (delegate).
///
/// # Safety
/// `list` is a list; `needle` live; attached thread.
#[inline(always)]
unsafe fn ft_prelude(list: *mut ffi::PyObject, needle: *mut ffi::PyObject, mode: Scan) -> Option<Outcome> {
    // SAFETY: lock-free reads as in `ft_first_is`; the first element is examined through a
    // strong reference.
    unsafe {
        let n = ffi::PyList_GET_SIZE(list);
        if n <= 0 {
            return Some(Outcome::Done(Ok(if mode == Scan::First { None } else { Some(0) })));
        }
        let first = ft_item0(list);
        if first == needle {
            if mode == Scan::First {
                return Some(Outcome::Done(Ok(Some(0))));
            }
            if n == 1 {
                return Some(Outcome::Done(Ok(Some(1))));
            }
        }
        if needs_generic(needle) {
            return if special_needle(needle) { None } else { Some(Outcome::Delegate) };
        }
        // A heuristic peek at the first element's type, without a reference: it only picks
        // between two correct strategies (delegate, or scan), so a stale value is harmless.
        // The read itself is safe for the reason CPython's lock-free `list_get_item_ref`
        // is: QSBR keeps item arrays and object memory mapped while we are attached. The
        // type pointer is only compared, never dereferenced.
        if first.is_null() || first == needle {
            return None;
        }
        let t = std::ptr::read_volatile(&raw const (*first).ob_type);
        let scalar =
            t == long_type() || t == float_type() || t == str_type() || t == bool_type() || first == ffi::Py_None();
        if scalar { None } else { Some(Outcome::Delegate) }
    }
}

/// Free-threaded lists: the first element pointer of a list seen non-empty, lock-free
/// (NULL if the item array is gone). The result may be stale: it is compared, and at most
/// its header is read (see [`ft_prelude`]).
///
/// # Safety
/// `list` is a list; attached thread.
#[inline(always)]
unsafe fn ft_item0(list: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: acquire/relaxed atomic loads like `list_get_item_ref`; a non-NULL item array
    // has room for at least one item.
    unsafe {
        use std::sync::atomic::{AtomicPtr, Ordering};
        let slot = &raw mut (*list.cast::<ffi::PyListObject>()).ob_item;
        let items = (*slot.cast::<AtomicPtr<*mut ffi::PyObject>>()).load(Ordering::Acquire);
        if items.is_null() {
            return std::ptr::null_mut();
        }
        (*items.cast::<AtomicPtr<ffi::PyObject>>()).load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------------------
// Saved originals and patch sites.
// ---------------------------------------------------------------------------------------

static ORIG_LIST_SORT: Original<FastKwFn> = Original::new();
static ORIG_LIST_INDEX: Original<FastFn> = Original::new();
static ORIG_LIST_COUNT: Original<OFn> = Original::new();
static ORIG_TUPLE_INDEX: Original<FastFn> = Original::new();
static ORIG_TUPLE_COUNT: Original<OFn> = Original::new();
static ORIG_LIST_CONTAINS: Original<ContainsFn> = Original::new();
static ORIG_TUPLE_CONTAINS: Original<ContainsFn> = Original::new();

/// Exception messages of the original `index` methods, captured from the original's own
/// `ValueError` at the first miss (which the original handles). Kept as C strings, not
/// Python objects: the patched tables are process-global, so kernels also run in
/// subinterpreters, which must never share objects with the main interpreter.
static LIST_INDEX_MSG: OnceLock<std::ffi::CString> = OnceLock::new();
static TUPLE_INDEX_MSG: OnceLock<std::ffi::CString> = OnceLock::new();

/// Record the message of the `ValueError` the original just raised (left raised).
///
/// # Safety
/// Attached thread, an exception set by the original.
#[cold]
unsafe fn capture_index_message(cell: &'static OnceLock<std::ffi::CString>) {
    // SAFETY: per the caller; str() of an exact ValueError runs no Python code.
    unsafe {
        let exc = ffi::PyErr_GetRaisedException();
        if !exc.is_null() && ffi::Py_TYPE(exc) == ffi::PyExc_ValueError.cast() {
            let s = ffi::PyObject_Str(exc);
            if s.is_null() {
                ffi::PyErr_Clear();
            } else {
                let mut len: ffi::Py_ssize_t = 0;
                let p = ffi::PyUnicode_AsUTF8AndSize(s, &mut len);
                if p.is_null() {
                    ffi::PyErr_Clear();
                } else if let Ok(c) =
                    std::ffi::CString::new(std::slice::from_raw_parts(p.cast::<u8>(), len as usize).to_vec())
                {
                    let _ = cell.set(c);
                }
                ffi::Py_DECREF(s);
            }
        }
        ffi::PyErr_SetRaisedException(exc);
    }
}

static C_LIST_SORT: Counters = Counters::new();
static C_LIST_INDEX: Counters = Counters::new();
static C_LIST_COUNT: Counters = Counters::new();
static C_TUPLE_INDEX: Counters = Counters::new();
static C_TUPLE_COUNT: Counters = Counters::new();
static C_LIST_CONTAINS: Counters = Counters::new();
static C_TUPLE_CONTAINS: Counters = Counters::new();

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
unsafe fn prelude(
    items: *const *mut ffi::PyObject,
    n: isize,
    needle: *mut ffi::PyObject,
    mode: Scan,
) -> Option<Outcome> {
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
    }
    // The needle first: a needle that needs CPython's comparison makes any scan wasted
    // work (`objs.count(o)` would scan, then delegate). Identity and tuple needles scan
    // whatever the items are.
    // SAFETY: live objects.
    if unsafe { needs_generic(needle) } {
        // SAFETY: a live object.
        return if unsafe { special_needle(needle) } { None } else { Some(Outcome::Delegate) };
    }
    if first == needle {
        return None;
    }
    // SAFETY: live object.
    if unsafe { !maybe_scalar(first) } {
        return Some(Outcome::Delegate);
    }
    None
}

/// # Safety
/// `list` is a list; GIL build.
#[inline(always)]
unsafe fn list_prelude(list: *mut ffi::PyObject, needle: *mut ffi::PyObject, mode: Scan) -> Option<Outcome> {
    if FREE_THREADED {
        // SAFETY: list and live needle.
        return unsafe { ft_prelude(list, needle, mode) };
    }
    // SAFETY: GIL held; the item array is valid for the current size.
    unsafe { prelude((*list.cast::<ffi::PyListObject>()).ob_item, ffi::PyList_GET_SIZE(list), needle, mode) }
}

/// # Safety
/// `tuple` is a tuple.
#[inline(always)]
unsafe fn tuple_prelude(tuple: *mut ffi::PyObject, needle: *mut ffi::PyObject, mode: Scan) -> Option<Outcome> {
    // SAFETY: immutable tuple items.
    unsafe {
        prelude((*tuple.cast::<ffi::PyTupleObject>()).ob_item.as_ptr(), ffi::PyTuple_GET_SIZE(tuple), needle, mode)
    }
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
        if FREE_THREADED {
            match ft_prelude(seq, el, Scan::First) {
                Some(Outcome::Done(Ok(found))) => {
                    C_LIST_CONTAINS.hit();
                    return c_int::from(found.is_some());
                }
                Some(Outcome::Done(Err(()))) => return -1,
                Some(Outcome::Delegate) => {
                    C_LIST_CONTAINS.fallback(Reason::Args);
                    return ORIG_LIST_CONTAINS.get()(seq, el);
                }
                None => {}
            }
        } else {
            let n = ffi::PyList_GET_SIZE(seq);
            if n == 0 {
                C_LIST_CONTAINS.hit();
                return 0;
            }
            let first = *(*seq.cast::<ffi::PyListObject>()).ob_item;
            if first == el {
                C_LIST_CONTAINS.hit();
                return 1;
            }
            if needs_generic(el) || !maybe_scalar(first) {
                if special_needle(el) {
                    return list_contains_scan(seq, el);
                }
                C_LIST_CONTAINS.fallback(Reason::Args);
                return ORIG_LIST_CONTAINS.get()(seq, el);
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
            Outcome::Done(Ok(found)) => {
                C_LIST_CONTAINS.hit();
                c_int::from(found.is_some())
            }
            Outcome::Done(Err(())) => -1,
            Outcome::Delegate => {
                C_LIST_CONTAINS.fallback(Reason::Deopt);
                ORIG_LIST_CONTAINS.get()(seq, el)
            }
        }
    }
}

unsafe extern "C" fn k_tuple_contains(seq: *mut ffi::PyObject, el: *mut ffi::PyObject) -> c_int {
    // SAFETY: called by CPython through `sq_contains` with a tuple and a live object.
    unsafe {
        let n = ffi::PyTuple_GET_SIZE(seq);
        if n == 0 {
            C_TUPLE_CONTAINS.hit();
            return 0;
        }
        let first = ffi::PyTuple_GET_ITEM(seq, 0);
        if first == el {
            C_TUPLE_CONTAINS.hit();
            return 1;
        }
        if needs_generic(el) || !maybe_scalar(first) {
            if special_needle(el) {
                return tuple_contains_scan(seq, el);
            }
            C_TUPLE_CONTAINS.fallback(Reason::Args);
            return ORIG_TUPLE_CONTAINS.get()(seq, el);
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
            Outcome::Done(Ok(found)) => {
                C_TUPLE_CONTAINS.hit();
                c_int::from(found.is_some())
            }
            Outcome::Done(Err(())) => -1,
            Outcome::Delegate => {
                C_TUPLE_CONTAINS.fallback(Reason::Deopt);
                ORIG_TUPLE_CONTAINS.get()(seq, el)
            }
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

/// `index` after a decided scan: the result, or for a miss CPython's own `ValueError`
/// (the first miss runs the original, which raises it, and its message is recorded).
///
/// # Safety
/// Attached thread; the kernel's own METH_FASTCALL arguments.
#[inline(always)]
unsafe fn index_done(
    r: Result<Option<isize>, ()>,
    cell: &'static OnceLock<std::ffi::CString>,
    original: FastFn,
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
) -> *mut ffi::PyObject {
    // SAFETY: per the caller.
    unsafe {
        match cell.get() {
            Some(msg) => index_result(r, msg),
            None if matches!(r, Ok(None)) => {
                let res = original(slf, args, nargs);
                if res.is_null() {
                    capture_index_message(cell);
                }
                res
            }
            None => index_result(r, c""),
        }
    }
}

unsafe extern "C" fn k_list_index(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
) -> *mut ffi::PyObject {
    if nargs == 1 {
        // SAFETY: METH_FASTCALL guarantees `nargs` valid argument pointers.
        // SAFETY: as above.
        let outcome =
            unsafe { list_prelude(slf, *args, Scan::First).unwrap_or_else(|| scan_list_slow(slf, *args, Scan::First)) };
        if let Outcome::Done(r) = outcome {
            C_LIST_INDEX.hit();
            // SAFETY: attached thread; the kernel's own arguments.
            return unsafe { index_done(r, &LIST_INDEX_MSG, ORIG_LIST_INDEX.get(), slf, args, nargs) };
        }
    }
    C_LIST_INDEX.fallback(Reason::Args);
    // SAFETY: same arguments to the original METH_FASTCALL function.
    unsafe { ORIG_LIST_INDEX.get()(slf, args, nargs) }
}

unsafe extern "C" fn k_tuple_index(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
) -> *mut ffi::PyObject {
    if nargs == 1 {
        // SAFETY: METH_FASTCALL guarantees `nargs` valid argument pointers.
        // SAFETY: as above.
        let outcome = unsafe {
            tuple_prelude(slf, *args, Scan::First).unwrap_or_else(|| scan_tuple_slow(slf, *args, Scan::First))
        };
        if let Outcome::Done(r) = outcome {
            C_TUPLE_INDEX.hit();
            // SAFETY: attached thread; the kernel's own arguments.
            return unsafe { index_done(r, &TUPLE_INDEX_MSG, ORIG_TUPLE_INDEX.get(), slf, args, nargs) };
        }
    }
    C_TUPLE_INDEX.fallback(Reason::Args);
    // SAFETY: same arguments to the original METH_FASTCALL function.
    unsafe { ORIG_TUPLE_INDEX.get()(slf, args, nargs) }
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
    // A needle that needs CPython's comparison: no scan can decide, delegate at once (a
    // plain tail call: the fallback costs a few instructions).
    // SAFETY: METH_O: a live object.
    if unsafe { needs_generic(value) && !special_needle(value) } {
        C_LIST_COUNT.fallback(Reason::Args);
        // SAFETY: same arguments to the original METH_O function.
        return unsafe { ORIG_LIST_COUNT.get()(slf, value) };
    }
    // SAFETY: METH_O arguments, forwarded (a tail call: the entry above needs no frame).
    unsafe { list_count_scan(slf, value) }
}

/// The scan behind `list.count` (out of line: see [`k_list_count`]). Panic-free.
#[inline(never)]
unsafe extern "C" fn list_count_scan(slf: *mut ffi::PyObject, value: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: METH_O: `slf` is a list, `value` a live object.
    match unsafe { list_prelude(slf, value, Scan::Count).unwrap_or_else(|| scan_list_slow(slf, value, Scan::Count)) } {
        Outcome::Done(r) => {
            C_LIST_COUNT.hit();
            // SAFETY: attached thread.
            unsafe { count_result(r) }
        }
        Outcome::Delegate => {
            C_LIST_COUNT.fallback(Reason::Args);
            // SAFETY: same arguments to the original METH_O function.
            unsafe { ORIG_LIST_COUNT.get()(slf, value) }
        }
    }
}

unsafe extern "C" fn k_tuple_count(slf: *mut ffi::PyObject, value: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // A needle that needs CPython's comparison: no scan can decide, delegate at once (a
    // plain tail call: the fallback costs a few instructions).
    // SAFETY: METH_O: a live object.
    if unsafe { needs_generic(value) && !special_needle(value) } {
        C_TUPLE_COUNT.fallback(Reason::Args);
        // SAFETY: same arguments to the original METH_O function.
        return unsafe { ORIG_TUPLE_COUNT.get()(slf, value) };
    }
    // SAFETY: METH_O arguments, forwarded (a tail call: the entry above needs no frame).
    unsafe { tuple_count_scan(slf, value) }
}

/// The scan behind `tuple.count` (out of line: see [`k_tuple_count`]). Panic-free.
#[inline(never)]
unsafe extern "C" fn tuple_count_scan(slf: *mut ffi::PyObject, value: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: METH_O: `slf` is a tuple, `value` a live object.
    match unsafe { tuple_prelude(slf, value, Scan::Count).unwrap_or_else(|| scan_tuple_slow(slf, value, Scan::Count)) }
    {
        Outcome::Done(r) => {
            C_TUPLE_COUNT.hit();
            // SAFETY: attached thread.
            unsafe { count_result(r) }
        }
        Outcome::Delegate => {
            C_TUPLE_COUNT.fallback(Reason::Args);
            // SAFETY: same arguments to the original METH_O function.
            unsafe { ORIG_TUPLE_COUNT.get()(slf, value) }
        }
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
unsafe fn kw_is(name: *mut ffi::PyObject, s: &CStr, interned: &AtomicPtr<ffi::PyObject>) -> bool {
    // Keyword names compiled into code objects are interned: the same object as ours
    // (`key`/`reverse` are CPython's static strings, shared by every interpreter).
    // SAFETY: str object, NUL-terminated ASCII literal; this comparison never fails.
    name == interned.load(std::sync::atomic::Ordering::Relaxed)
        || unsafe { ffi::PyUnicode_CompareWithASCIIString(name, s.as_ptr()) == 0 }
}

/// Interned `"key"` and `"reverse"` (set by the first `prepare`).
static KW_KEY: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static KW_REVERSE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());

/// Parse `sort(*, key=None, reverse=<exact bool or int>)` without running Python code.
/// `None` means "let the original parse it". Also used by `builtins.sorted`, whose
/// keywords `list.sort` receives unchanged.
///
/// # Safety
/// Vectorcall arguments as received by a METH_FASTCALL|METH_KEYWORDS function.
#[inline(always)]
pub(crate) unsafe fn sort_args(
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> Option<bool> {
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
            if kw_is(name, c"key", &KW_KEY) {
                if value != ffi::Py_None() {
                    return None;
                }
            } else if kw_is(name, c"reverse", &KW_REVERSE) {
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

/// `list.sort` hands lists of fewer `str` than this to CPython's sort: there its binary
/// insertion with `unsafe_latin_compare` costs less than building keys (measured 1.09 for
/// 2 strings natively; `sorted` keeps them native, see [`sorted_candidate`]).
pub(crate) const STR_NATIVE_MIN: isize = 32;

/// Up to this many elements the native sort is timsort's small-list strategy: the first
/// natural run (`count_run`: a strictly descending run is reversed in place), then
/// insertion of the remaining elements. Above it, Rust's driftsort, which detects ascending
/// and strictly descending runs itself. (Rust's stable sort uses a plain insertion sort up
/// to 20 elements, quadratic on descending input: 1.23-1.32 for 20 descending ints/floats.)
const SMALL_SORT: usize = 20;

/// The type of a list's first element, read without a reference or a lock (NULL for an
/// empty list). Only a strategy choice depends on it: a stale value is harmless.
///
/// # Safety
/// `list` is a list; attached thread.
#[inline(always)]
pub(crate) unsafe fn first_type_racy(list: *mut ffi::PyObject) -> *mut ffi::PyTypeObject {
    // SAFETY: as `ft_prelude`: QSBR keeps the item array and the object mapped while we are
    // attached (free-threaded builds); the GIL protects both otherwise.
    unsafe {
        if ffi::PyList_GET_SIZE(list) <= 0 {
            return std::ptr::null_mut();
        }
        let first = if FREE_THREADED { ft_item0(list) } else { *(*list.cast::<ffi::PyListObject>()).ob_item };
        if first.is_null() {
            return std::ptr::null_mut();
        }
        std::ptr::read_volatile(&raw const (*first).ob_type)
    }
}

/// A list the native sort leaves to CPython at once: `str` lists shorter than
/// [`STR_NATIVE_MIN`].
///
/// # Safety
/// `list` is a list; attached thread.
#[inline(always)]
unsafe fn short_str_list(list: *mut ffi::PyObject) -> bool {
    // SAFETY: forwarded.
    unsafe { ffi::PyList_GET_SIZE(list) < STR_NATIVE_MIN && first_type_racy(list) == str_type() }
}

/// Whether `builtins.sorted` should copy `seq` (an exact list or tuple) and sort it
/// natively: its first item is an exact `int`/`bool`/`float`/`str` (`sorted` saves the
/// bound-method round trip of `list.sort`, so even short `str` lists win there). A
/// strategy choice only.
///
/// # Safety
/// `seq` is an exact list (`is_list`) or tuple; attached thread.
#[inline(always)]
pub(crate) unsafe fn sorted_candidate(seq: *mut ffi::PyObject, is_list: bool) -> bool {
    // SAFETY: forwarded; tuple items are immutable.
    unsafe {
        let (n, t) = if is_list {
            (ffi::PyList_GET_SIZE(seq), first_type_racy(seq))
        } else {
            let n = ffi::PyTuple_GET_SIZE(seq);
            (n, if n > 0 { ffi::Py_TYPE(ffi::PyTuple_GET_ITEM(seq, 0)) } else { std::ptr::null_mut() })
        };
        n < 2 || t == long_type() || t == float_type() || t == bool_type() || t == str_type()
    }
}

/// Sort a list that no other code can reach (the fresh copy `sorted` makes), natively.
/// `Some(true)`: sorted; `Some(false)`: not applicable, nothing changed; `None`: a panic,
/// `SystemError` set.
///
/// # Safety
/// `list` is a fresh list owned by the caller; attached thread.
pub(crate) unsafe fn sort_fresh_list(list: *mut ffi::PyObject, reverse: bool) -> Option<bool> {
    // SAFETY: nobody else holds `list`, so its items cannot change meanwhile.
    guard(None, || Some(unsafe { try_sort(list, reverse) }.is_some()))
}

/// The `list.sort` function `builtins.sorted` calls on its copy after the native sort
/// declined: CPython's own when `ml_meth` (`meth`) is this kernel (which would only repeat
/// the work that just declined), else `meth` itself.
///
/// # Safety
/// `meth` is the current `ml_meth` of `list.sort` (a METH_FASTCALL|METH_KEYWORDS function).
pub(crate) unsafe fn list_sort_after_decline(meth: usize) -> FastKwFn {
    if meth == k_list_sort as FastKwFn as usize && ORIG_LIST_SORT.is_set() {
        return ORIG_LIST_SORT.get();
    }
    // SAFETY: per the caller.
    unsafe { std::mem::transmute::<usize, FastKwFn>(meth) }
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

/// The first 8 bytes of 1-byte data of `len` bytes, big-endian, zero-padded: the order of
/// the 8-byte prefixes is the order of the strings up to ties. Reads only `data[..len]`
/// (overlapping loads instead of a byte loop).
///
/// # Safety
/// `data` is readable for `len` bytes.
#[inline(always)]
unsafe fn latin_prefix(data: *const u8, len: usize) -> u64 {
    // SAFETY: every load below stays within `data[..len]`.
    unsafe {
        let be32 = |p: *const u8| u64::from(u32::from_be_bytes(std::ptr::read_unaligned(p.cast::<[u8; 4]>())));
        if len >= 8 {
            u64::from_be_bytes(std::ptr::read_unaligned(data.cast::<[u8; 8]>()))
        } else if len >= 4 {
            // Bytes 0..4, then bytes 4..len (the low `len - 4` bytes of the last 4).
            let tail = len - 4;
            let lo = be32(data.add(len - 4)) & ((1u64 << (8 * tail)) - 1);
            (be32(data) << 32) | (lo << (8 * (8 - len)))
        } else if len >= 2 {
            let hi = u64::from(u16::from_be_bytes(std::ptr::read_unaligned(data.cast::<[u8; 2]>()))) << 48;
            if len == 3 { hi | (u64::from(*data.add(2)) << 40) } else { hi }
        } else if len == 1 {
            u64::from(*data) << 56
        } else {
            0
        }
    }
}

/// Whether two strings agree on their first `k` code points (both at least `k` long). Same
/// kind: their first `k * kind` bytes, compared 8 at a time (the last word overlapping);
/// different kinds: code point by code point.
///
/// # Safety
/// `a` and `b` describe live strings with `len >= k`.
/// `small_a` is `latin_prefix(a.data, k * a.kind)`, computed once by the caller.
#[inline(always)]
unsafe fn prefix_eq(a: &StrParts, b: &StrParts, k: usize, small_a: u64) -> bool {
    // SAFETY: every load stays within the first `k` code points of both strings.
    unsafe {
        if a.kind != b.kind {
            return (0..k).all(|i| code_point(a, i) == code_point(b, i));
        }
        let bytes = k * a.kind as usize;
        if bytes < 8 {
            return latin_prefix(b.data, bytes) == small_a;
        }
        let word = |p: *const u8, i: usize| std::ptr::read_unaligned(p.add(i).cast::<u64>());
        let mut i = 0;
        while i + 8 < bytes {
            if word(a.data, i) != word(b.data, i) {
                return false;
            }
            i += 8;
        }
        word(a.data, bytes - 8) == word(b.data, bytes - 8)
    }
}

/// Length of the common prefix, in code points, of two strings.
///
/// # Safety
/// `a` and `b` describe live strings.
#[inline(always)]
unsafe fn common_len(a: &StrParts, b: &StrParts) -> usize {
    let m = a.len.min(b.len);
    // SAFETY: reads within the first `m` code points of both strings.
    unsafe {
        let mut i = 0;
        if a.kind == b.kind && a.kind == ffi::PyUnicode_1BYTE_KIND {
            while i + 8 <= m {
                let x = std::ptr::read_unaligned(a.data.add(i).cast::<u64>())
                    ^ std::ptr::read_unaligned(b.data.add(i).cast::<u64>());
                if x != 0 {
                    return i + (u64::from_le(x).trailing_zeros() / 8) as usize;
                }
                i += 8;
            }
        }
        while i < m && code_point(a, i) == code_point(b, i) {
            i += 1;
        }
        i
    }
}

/// The parts of an exact `str` without its hash (cheaper than [`str_parts`]).
///
/// # Safety
/// `p` is a live exact `str`.
#[inline(always)]
unsafe fn str_view(p: *mut ffi::PyObject, layout: Option<&StrLayout>) -> StrParts {
    // SAFETY: per the caller.
    unsafe {
        let (kind, data) = str_kind_data(p, layout);
        StrParts { kind, data, len: ffi::PyUnicode_GET_LENGTH(p) as usize, hash: -1 }
    }
}

/// The sort key of a string from code point `skip` on: [`latin_prefix`] of the next 8
/// bytes when the whole list is 1-byte (`latin`), else the next 3 code points.
///
/// # Safety
/// `v` describes a live string of at least `skip` code points.
#[inline(always)]
unsafe fn str_key(v: &StrParts, latin: bool, skip: usize) -> u64 {
    // SAFETY: reads within the string's data.
    unsafe {
        if latin {
            return latin_prefix(v.data.add(skip), v.len - skip);
        }
        str_prefix(
            &StrParts { kind: v.kind, data: v.data.add(skip * v.kind as usize), len: v.len - skip, hash: -1 },
            false,
        )
    }
}

/// One keying pass of [`sort_strs`]: keys from code point `skip` on for strings sharing
/// the first `skip` code points of `items[0]` (see [`Pass`]).
///
/// # Safety
/// As [`sort_strs`]; `items[0]` is an exact `str`.
#[inline(always)]
unsafe fn key_pass(
    items: &[*mut ffi::PyObject],
    buf: &mut [std::mem::MaybeUninit<(u64, *mut ffi::PyObject)>],
    layout: Option<&StrLayout>,
    latin: bool,
    skip: usize,
) -> Pass {
    // SAFETY: each item's type is checked before its data is read.
    unsafe {
        let v0 = str_view(items[0], layout);
        if latin && v0.kind != ffi::PyUnicode_1BYTE_KIND {
            return Pass::NonLatin;
        }
        let k0 = str_key(&v0, latin, skip);
        let small0 = latin_prefix(v0.data, (skip * v0.kind as usize).min(7));
        let mut diff = 0u64;
        let mut mono = MonoTrack::new(k0);
        buf[0].write((k0, items[0]));
        for (slot, &p) in buf[1..].iter_mut().zip(items[1..].iter()) {
            if ffi::Py_TYPE(p) != str_type() {
                return Pass::NotStr;
            }
            let v = str_view(p, layout);
            if latin && v.kind != ffi::PyUnicode_1BYTE_KIND {
                return Pass::NonLatin;
            }
            if skip > 0 && (v.len < skip || !prefix_eq(&v0, &v, skip, small0)) {
                return Pass::Mismatch;
            }
            let k = str_key(&v, latin, skip);
            diff |= k ^ k0;
            mono.push(k);
            slot.write((k, p));
        }
        Pass::Done { all_equal: diff == 0, shape: mono.shape() }
    }
}

/// Outcome of a [`key_pass`].
enum Pass {
    /// Every key written.
    Done { all_equal: bool, shape: Shape },
    /// An item is not an exact `str`: not applicable.
    NotStr,
    /// A string wider than 1 byte in a pass keyed as 1-byte strings.
    NonLatin,
    /// A string without the shared prefix assumed.
    Mismatch,
}

/// Whether keys should start after a shared prefix of `guess` code points (strings of
/// `len`, keys holding `width` code points): when the prefix fills the key, or half of it
/// while the strings go on past the key (an offset-0 key would leave little to tell them
/// apart). Strings that fit in an offset-0 key never need it.
#[inline(always)]
fn keys_after_prefix(guess: usize, len: usize, width: usize) -> bool {
    guess >= width || (2 * guess >= width && len > width)
}

/// Lists shorter than this never key after a shared prefix (see [`sort_strs`]).
const SHARED_PREFIX_MIN_N: usize = 16;

/// The native sort of a list of exact `str`, on `(u64 key, item)` pairs; equal keys are
/// resolved by comparing the strings. Keys: the 8 bytes (1-byte lists) or 3 code points
/// that follow the prefix all strings share (`"https://..."`, `"key-0000..."`: keying on
/// the shared prefix would make every comparison a full one). The shared prefix is guessed
/// from the first, middle and last strings and verified on every string; a failed guess,
/// or a longer prefix (all keys equal), recomputes it exactly.
///
/// # Safety
/// As [`try_sort`]; `items.len() >= 2`, `items[0]` an exact `str`.
#[inline(never)]
unsafe fn sort_strs(items: &mut [*mut ffi::PyObject], reverse: bool) -> Option<()> {
    let layout = STR_LAYOUT.get();
    let n = items.len();
    let t = str_type();
    // SAFETY: every string is type-checked before its data is read; slots of `buf` are
    // read only after a completed pass wrote all of them.
    unsafe {
        // Keying after a shared prefix pays a check per string; worth it only when the
        // prefix would take half the key or more. Short lists: never (few comparisons).
        let (guess, len0) = if n >= SHARED_PREFIX_MIN_N
            && ffi::Py_TYPE(items[0]) == t
            && ffi::Py_TYPE(items[n / 2]) == t
            && ffi::Py_TYPE(items[n - 1]) == t
        {
            let v0 = str_view(items[0], layout);
            let (vm, vl) = (str_view(items[n / 2], layout), str_view(items[n - 1], layout));
            (common_len(&v0, &vm).min(common_len(&v0, &vl)), v0.len)
        } else {
            (0, 0)
        };
        with_pairs::<u64, ()>(n, |buf| {
            // The common case in one pass: 1-byte strings keyed on their first 8 bytes.
            let fast = if !keys_after_prefix(guess, len0, 8) { latin_pass(items, buf, layout) } else { Err(false) };
            let shape = match fast {
                Ok(shape) => shape,
                Err(true) => return None, // not an exact str
                Err(false) => general_str_keys(items, buf, layout, guess)?,
            };
            if finish_mono(items, shape.mono, reverse) {
                return Some(());
            }
            // SAFETY: all `n` slots written by the last completed pass.
            let pairs = std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<(u64, *mut ffi::PyObject)>(), n);
            adaptive_sort(
                pairs,
                reverse,
                shape.few_runs(n, reverse),
                |a: &(u64, *mut ffi::PyObject), b: &(u64, *mut ffi::PyObject)| {
                    a.0.cmp(&b.0).then_with(|| str_cmp(&str_view(a.1, layout), &str_view(b.1, layout)))
                },
            );
            for (dst, (_, p)) in items.iter_mut().zip(pairs.iter()) {
                *dst = *p;
            }
            Some(())
        })
    }
}

/// [`sort_strs`]' one-pass keying of 1-byte strings from their first byte. `Ok(mono)`: all
/// keys written; `Err(true)`: an item is not an exact `str`; `Err(false)`: use
/// [`general_str_keys`] (a wider string, or every key equal).
///
/// # Safety
/// As [`sort_strs`].
#[inline(always)]
unsafe fn latin_pass(
    items: &[*mut ffi::PyObject],
    buf: &mut [std::mem::MaybeUninit<(u64, *mut ffi::PyObject)>],
    layout: Option<&StrLayout>,
) -> Result<Shape, bool> {
    let t = str_type();
    // SAFETY: the type is checked before the data is read.
    unsafe {
        let key = |p: *mut ffi::PyObject| -> Result<u64, bool> {
            if ffi::Py_TYPE(p) != t {
                return Err(true);
            }
            let (kind, data) = str_kind_data(p, layout);
            if kind != ffi::PyUnicode_1BYTE_KIND {
                return Err(false);
            }
            Ok(latin_prefix(data, ffi::PyUnicode_GET_LENGTH(p) as usize))
        };
        let k0 = key(items[0])?;
        buf[0].write((k0, items[0]));
        let mut mono = MonoTrack::new(k0);
        let mut diff = 0u64;
        for (slot, &p) in buf[1..].iter_mut().zip(items[1..].iter()) {
            let k = key(p)?;
            diff |= k ^ k0;
            mono.push(k);
            slot.write((k, p));
        }
        if diff == 0 { Err(false) } else { Ok(mono.shape()) }
    }
}

/// [`sort_strs`]' general keying: any kinds, keys after the prefix every string shares
/// (`guess`, verified per string, or recomputed exactly when wrong or when all keys are
/// equal). The keys' [`Shape`]; `None`: an item is not an exact `str`.
///
/// # Safety
/// As [`sort_strs`].
unsafe fn general_str_keys(
    items: &[*mut ffi::PyObject],
    buf: &mut [std::mem::MaybeUninit<(u64, *mut ffi::PyObject)>],
    layout: Option<&StrLayout>,
    guess: usize,
) -> Option<Shape> {
    // SAFETY: every item is type-checked before its data is read.
    unsafe {
        let v0 = str_view(items[0], layout);
        // 1-byte keys first (the common case); a wider string switches to code-point keys.
        let mut latin = v0.kind == ffi::PyUnicode_1BYTE_KIND;
        let width = |latin: bool| if latin { 8 } else { 3 };
        let mut skip = if keys_after_prefix(guess, v0.len, width(latin)) { guess } else { 0 };
        loop {
            let all_equal = match key_pass(items, buf, layout, latin, skip) {
                Pass::NotStr => return None,
                Pass::Done { all_equal: false, shape } => return Some(shape),
                Pass::Done { all_equal: true, .. } => true,
                Pass::NonLatin => {
                    latin = false;
                    skip = if keys_after_prefix(guess, v0.len, width(latin)) { guess } else { 0 };
                    continue;
                }
                Pass::Mismatch => false,
            };
            // The exact prefix every string shares: shorter than the guess (a string did not
            // share it), or longer (every key equal). At most two more passes: the exact
            // prefix passes verification, and keys equal at it mean the strings are equal up
            // to their ends.
            let mut l = v0.len;
            for &p in items.iter() {
                if ffi::Py_TYPE(p) != str_type() {
                    return None;
                }
                l = l.min(common_len(&v0, &str_view(p, layout)));
            }
            if all_equal && l <= skip {
                return Some(Shape::UNKNOWN);
            }
            skip = l;
        }
    }
}

/// Try the native sort. `None`: not applicable (nothing was changed).
///
/// # Safety
/// `list` is a list whose critical section the caller holds (or that nobody else can
/// reach); no Python code runs here.
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
            sort_by_keys(
                items,
                reverse,
                |p| {
                    let t = ffi::Py_TYPE(p);
                    if t != long_type() && t != bool_type() {
                        return None;
                    }
                    int_value(p)
                },
                |a: &(i64, *mut ffi::PyObject), b: &(i64, *mut ffi::PyObject)| a.0.cmp(&b.0),
            )
        } else if t0 == float_type() {
            // No NaN: `<` is a strict weak order; -0.0 and 0.0 are equivalent and keep their
            // order, like CPython (one key for both). With NaNs CPython's order is
            // algorithm-specific: not handled.
            sort_by_keys(
                items,
                reverse,
                |p| {
                    if ffi::Py_TYPE(p) != float_type() {
                        return None;
                    }
                    let v = float_value(p);
                    if v.is_nan() { None } else { Some(v) }
                },
                |a: &(f64, *mut ffi::PyObject), b: &(f64, *mut ffi::PyObject)| {
                    a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal)
                },
            )
        } else if t0 == str_type() {
            sort_strs(items, reverse)
        } else {
            None
        }
    }
}

/// Stable sort of `v` by `cmp`; with `reverse`, descending with equal elements in their
/// original order (CPython's reverse-sort-reverse). Adaptive like timsort: see
/// [`SMALL_SORT`].
#[inline(always)]
fn adaptive_sort<T: Copy>(v: &mut [T], reverse: bool, few_runs: bool, cmp: impl Fn(&T, &T) -> Ordering) {
    let n = v.len();
    if n < 2 {
        return;
    }
    let less = |a: &T, b: &T| if reverse { cmp(b, a) == Ordering::Less } else { cmp(a, b) == Ordering::Less };
    if n > SMALL_SORT {
        if few_runs {
            // Nearly sorted, a few runs: timsort-like natural merging (driftsort is not
            // adaptive to runs shorter than about sqrt(n)).
            if reverse {
                natural_merge_sort(v, |a: &T, b: &T| cmp(b, a) == Ordering::Less);
            } else {
                natural_merge_sort(v, |a: &T, b: &T| cmp(a, b) == Ordering::Less);
            }
        } else if reverse {
            v.sort_by(|a, b| cmp(b, a));
        } else {
            v.sort_by(|a, b| cmp(a, b));
        }
        return;
    }
    // `count_run`: a non-descending run, or a strictly descending one (reversed: it has no
    // equal elements, so stability is kept).
    let mut run = 2;
    if less(&v[1], &v[0]) {
        while run < n && less(&v[run], &v[run - 1]) {
            run += 1;
        }
        v[..run].reverse();
    } else {
        while run < n && !less(&v[run], &v[run - 1]) {
            run += 1;
        }
    }
    // Insertion of the rest: an element moves left only past strictly greater ones.
    for i in run..n {
        let x = v[i];
        let mut j = i;
        while j > 0 && less(&x, &v[j - 1]) {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = x;
    }
}

/// Lists whose keys step against the requested order at most `n / NATURAL_DESCENTS` times
/// are sorted by [`natural_merge_sort`].
const NATURAL_DESCENTS: usize = 16;

/// Stable natural merge sort: timsort's run detection (`count_run`: strictly descending runs
/// reversed) and merge policy (`merge_collapse` invariants), without minrun extension or
/// galloping. Each merge first trims what is already in place (binary searches, as
/// `merge_at` does with `gallop_right`/`gallop_left`) and moves a run that precedes the other
/// as one block. Near-linear on input with few runs.
fn natural_merge_sort<T: Copy, F: Fn(&T, &T) -> bool>(v: &mut [T], less: F) {
    let n = v.len();
    let mut stack_buf: [std::mem::MaybeUninit<T>; STACK_SORT] = [const { std::mem::MaybeUninit::uninit() }; STACK_SORT];
    let mut heap_buf: Vec<std::mem::MaybeUninit<T>> = Vec::new();
    let buf: &mut [std::mem::MaybeUninit<T>] = if n / 2 < STACK_SORT {
        &mut stack_buf[..]
    } else {
        if heap_buf.try_reserve_exact(n).is_err() {
            v.sort_by(|a, b| {
                if less(a, b) {
                    Ordering::Less
                } else if less(b, a) {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            });
            return;
        }
        // SAFETY: `MaybeUninit` slots need no initialisation; the capacity is reserved.
        unsafe { heap_buf.set_len(n) };
        &mut heap_buf
    };
    let mut runs = [(0usize, 0usize); 96];
    let mut nr = 0;
    let mut i = 0;
    while i < n {
        let mut end = i + 1;
        if end < n {
            if less(&v[end], &v[i]) {
                while end + 1 < n && less(&v[end + 1], &v[end]) {
                    end += 1;
                }
                end += 1;
                v[i..end].reverse();
            } else {
                while end + 1 < n && !less(&v[end + 1], &v[end]) {
                    end += 1;
                }
                end += 1;
            }
        }
        runs[nr] = (i, end - i);
        nr += 1;
        i = end;
        while nr > 1 {
            let at = if (nr >= 3 && runs[nr - 3].1 <= runs[nr - 2].1 + runs[nr - 1].1)
                || (nr >= 4 && runs[nr - 4].1 <= runs[nr - 3].1 + runs[nr - 2].1)
            {
                if runs[nr - 3].1 < runs[nr - 1].1 { nr - 3 } else { nr - 2 }
            } else if runs[nr - 2].1 <= runs[nr - 1].1 {
                nr - 2
            } else {
                break;
            };
            merge_at(v, &mut runs, &mut nr, at, buf, &less);
        }
    }
    while nr > 1 {
        let at = if nr >= 3 && runs[nr - 3].1 < runs[nr - 1].1 { nr - 3 } else { nr - 2 };
        merge_at(v, &mut runs, &mut nr, at, buf, &less);
    }
}

/// Merge `runs[at]` and `runs[at + 1]` (adjacent in `v`) and update the run stack.
#[inline(always)]
fn merge_at<T: Copy, F: Fn(&T, &T) -> bool>(
    v: &mut [T],
    runs: &mut [(usize, usize)],
    nr: &mut usize,
    at: usize,
    buf: &mut [std::mem::MaybeUninit<T>],
    less: &F,
) {
    let (lo, l1) = runs[at];
    let l2 = runs[at + 1].1;
    merge_adjacent(&mut v[lo..lo + l1 + l2], l1, buf, less);
    runs[at] = (lo, l1 + l2);
    runs.copy_within(at + 2..*nr, at + 1);
    *nr -= 1;
}

/// Stable merge of the sorted halves `v[..mid]` and `v[mid..]`, `buf` holding at least the
/// smaller of the two after trimming.
#[inline(always)]
fn merge_adjacent<T: Copy, F: Fn(&T, &T) -> bool>(
    v: &mut [T],
    mid: usize,
    buf: &mut [std::mem::MaybeUninit<T>],
    less: &F,
) {
    if !less(&v[mid], &v[mid - 1]) {
        return; // already in order
    }
    let lo = v[..mid].partition_point(|x| !less(&v[mid], x));
    let hi = mid + v[mid..].partition_point(|x| less(x, &v[mid - 1]));
    if less(&v[hi - 1], &v[lo]) {
        // Every remaining right element precedes every remaining left one.
        v[lo..hi].rotate_left(mid - lo);
        return;
    }
    let (nl, nh) = (mid - lo, hi - mid);
    if nl <= nh {
        // Left part into the buffer, merge forwards.
        let b = buf.as_mut_ptr().cast::<T>();
        // SAFETY: `buf` holds at least `nl` slots (`nl <= n / 2`); indices stay in bounds:
        // `k < j` always, so the destination never overtakes the unread right part.
        unsafe {
            std::ptr::copy_nonoverlapping(v.as_ptr().add(lo), b, nl);
            let p = v.as_mut_ptr();
            let (mut i, mut j, mut k) = (0, mid, lo);
            while i < nl && j < hi {
                if less(&*p.add(j), &*b.add(i)) {
                    *p.add(k) = *p.add(j);
                    j += 1;
                } else {
                    *p.add(k) = *b.add(i);
                    i += 1;
                }
                k += 1;
            }
            std::ptr::copy_nonoverlapping(b.add(i), p.add(k), nl - i);
        }
    } else {
        // Right part into the buffer, merge backwards.
        let b = buf.as_mut_ptr().cast::<T>();
        // SAFETY: as above, mirrored (`nh < n / 2 + 1`).
        unsafe {
            std::ptr::copy_nonoverlapping(v.as_ptr().add(mid), b, nh);
            let p = v.as_mut_ptr();
            let (mut i, mut j, mut k) = (mid, nh, hi);
            while i > lo && j > 0 {
                if less(&*b.add(j - 1), &*p.add(i - 1)) {
                    *p.add(k - 1) = *p.add(i - 1);
                    i -= 1;
                } else {
                    *p.add(k - 1) = *b.add(j - 1);
                    j -= 1;
                }
                k -= 1;
            }
            std::ptr::copy_nonoverlapping(b, p.add(k - j), j);
        }
    }
}

/// Keys strictly increasing (`Up`) or strictly decreasing (`Down`) over the whole list:
/// the sorted order is the list itself or its reverse (no equal keys, so stability is
/// moot). Tracked while the keys are built, so presorted and reversed input costs one pass.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mono {
    Up,
    Down,
    Neither,
}

/// Running [`Mono`] state over keys.
struct MonoTrack<K> {
    prev: K,
    up: bool,
    down: bool,
    /// Steps where the key rose / fell (for the choice of [`adaptive_sort`]).
    rises: usize,
    falls: usize,
}

impl<K: Copy + PartialOrd> MonoTrack<K> {
    #[inline(always)]
    fn new(first: K) -> Self {
        MonoTrack { prev: first, up: true, down: true, rises: 0, falls: 0 }
    }

    #[inline(always)]
    fn push(&mut self, k: K) {
        let (r, f) = (self.prev < k, self.prev > k);
        self.up &= r;
        self.down &= f;
        self.rises += usize::from(r);
        self.falls += usize::from(f);
        self.prev = k;
    }

    #[inline(always)]
    fn shape(&self) -> Shape {
        Shape { mono: self.get(), rises: self.rises, falls: self.falls }
    }

    #[inline(always)]
    fn get(&self) -> Mono {
        if self.up {
            Mono::Up
        } else if self.down {
            Mono::Down
        } else {
            Mono::Neither
        }
    }
}

/// What a keying pass learned about the order of the keys.
#[derive(Clone, Copy)]
struct Shape {
    mono: Mono,
    rises: usize,
    falls: usize,
}

impl Shape {
    const UNKNOWN: Shape = Shape { mono: Mono::Neither, rises: usize::MAX / 2, falls: usize::MAX / 2 };

    /// Few steps against the requested order: natural runs are long.
    #[inline(always)]
    fn few_runs(&self, n: usize, reverse: bool) -> bool {
        (if reverse { self.rises } else { self.falls }).saturating_mul(NATURAL_DESCENTS) <= n
    }
}

/// Finish a sort whose keys turned out monotonic: nothing to do when they already run in
/// the requested direction, else reverse the items. `false`: not monotonic.
#[inline(always)]
fn finish_mono(items: &mut [*mut ffi::PyObject], mono: Mono, reverse: bool) -> bool {
    match (mono, reverse) {
        (Mono::Up, false) | (Mono::Down, true) => true,
        (Mono::Up, true) | (Mono::Down, false) => {
            items.reverse();
            true
        }
        (Mono::Neither, _) => false,
    }
}

/// Up to this many elements, keys live on the stack (no allocation: small sorts stay
/// cheap and allocator-noise free).
const STACK_SORT: usize = 128;

/// Run `f` on an uninitialised buffer of `n` `(key, item)` pairs: on the stack up to
/// [`STACK_SORT`] elements, else on the heap (`None` if the allocation fails).
#[inline(always)]
fn with_pairs<K: Copy, R>(
    n: usize,
    f: impl FnOnce(&mut [std::mem::MaybeUninit<(K, *mut ffi::PyObject)>]) -> Option<R>,
) -> Option<R> {
    if n <= STACK_SORT {
        let mut stack: [std::mem::MaybeUninit<(K, *mut ffi::PyObject)>; STACK_SORT] =
            [const { std::mem::MaybeUninit::uninit() }; STACK_SORT];
        f(&mut stack[..n])
    } else {
        let mut heap: Vec<std::mem::MaybeUninit<(K, *mut ffi::PyObject)>> = Vec::new();
        heap.try_reserve_exact(n).ok()?;
        // SAFETY: `MaybeUninit` slots need no initialisation; the capacity is reserved.
        unsafe { heap.set_len(n) };
        f(&mut heap)
    }
}

/// Extract a key per item (`None`: not applicable, nothing changed), stable-sort the
/// `(key, item)` pairs and write the items back in order.
///
/// # Safety
/// `items` is the list's item array (critical section held); `key` runs no Python code.
#[inline(always)]
unsafe fn sort_by_keys<K: Copy + PartialOrd>(
    items: &mut [*mut ffi::PyObject],
    reverse: bool,
    key: impl Fn(*mut ffi::PyObject) -> Option<K>,
    cmp: impl Fn(&(K, *mut ffi::PyObject), &(K, *mut ffi::PyObject)) -> Ordering,
) -> Option<()> {
    let n = items.len();
    with_pairs::<K, ()>(n, |buf| {
        let k0 = key(items[0])?;
        buf[0].write((k0, items[0]));
        let mut mono = MonoTrack::new(k0);
        for (slot, &p) in buf[1..].iter_mut().zip(items[1..].iter()) {
            let k = key(p)?;
            mono.push(k);
            slot.write((k, p));
        }
        let shape = mono.shape();
        if finish_mono(items, shape.mono, reverse) {
            return Some(());
        }
        // SAFETY: all `n` slots were written just above; `(K, ptr)` is Copy.
        let pairs = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<(K, *mut ffi::PyObject)>(), n) };
        adaptive_sort(pairs, reverse, shape.few_runs(n, reverse), cmp);
        for (dst, (_, p)) in items.iter_mut().zip(pairs.iter()) {
            *dst = *p;
        }
        Some(())
    })
}

unsafe extern "C" fn k_list_sort(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: vectorcall arguments from CPython; `slf` is a list. Short `str` lists are
    // decided before the critical section, so their hand-off is a tail call.
    // SAFETY: `slf` is a list (a racy peek at its first item only picks a strategy).
    let short_str = unsafe { short_str_list(slf) };
    // SAFETY: vectorcall arguments from CPython.
    let reverse = match unsafe { sort_args(args, nargs, kwnames) } {
        Some(reverse) if !short_str => reverse,
        _ => {
            C_LIST_SORT.fallback(Reason::Args);
            // SAFETY: same arguments to the original.
            return unsafe { ORIG_LIST_SORT.get()(slf, args, nargs, kwnames) };
        }
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
        Some(()) => {
            C_LIST_SORT.hit();
            // SAFETY: new reference to None.
            unsafe { ffi::Py_NewRef(ffi::Py_None()) }
        }
        None => {
            C_LIST_SORT.fallback(Reason::Args);
            // SAFETY: same arguments to the original.
            unsafe { ORIG_LIST_SORT.get()(slf, args, nargs, kwnames) }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Installation.
// ---------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------
// Targets (docs/ACCELERATION.md §9): group `containers`.
// ---------------------------------------------------------------------------------------

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

// Fingerprints of the C originals (`ml_name`, `ml_flags`, `ml_doc`), recorded with
// `python -m rustique._takeover._dev fingerprint list.sort ...` (format v2; identical on
// 3.14.9, 3.14.9t, 3.15.1 and 3.15.1t).
const FP_SORT: accel::Fingerprints = &[(3, 14, "e8c0ba48a89aa585"), (3, 15, "e8c0ba48a89aa585")];
const FP_INDEX: accel::Fingerprints = &[(3, 14, "e401ffbd097a7175"), (3, 15, "e401ffbd097a7175")];
const FP_COUNT: accel::Fingerprints = &[(3, 14, "e0cac92256fa864d"), (3, 15, "e0cac92256fa864d")];

/// The method descriptor `ty.name`, checked for the expected `ml_name` and `ml_flags`.
fn method<'py>(ty: &Bound<'py, pyo3::types::PyType>, name: &CStr, flags: c_int) -> Result<Bound<'py, PyAny>, Refusal> {
    let m = ty.getattr(name.to_str().unwrap_or_default())?;
    // SAFETY: attached thread; type-checked layout reads.
    let def = unsafe { accel::site::method_def(m.as_ptr()) }
        .ok_or_else(|| refuse!(Conflict: "{name:?}: not a method descriptor"))?;
    // SAFETY: a live method def.
    let (n, f) = unsafe { (CStr::from_ptr((*def).ml_name), (*def).ml_flags) };
    if n != name || f != flags {
        return Err(refuse!(Conflict: "{name:?}: unexpected method definition ({n:?}, {f:#x})"));
    }
    Ok(m)
}

/// Layouts the kernels read directly: int/float (required), str (calibrated, optional).
fn calibrate(py: Python<'_>) -> Result<(), Refusal> {
    static OK: OnceLock<bool> = OnceLock::new();
    if !*OK.get_or_init(|| layouts_ok(py).unwrap_or(false)) {
        return Err(Refusal::with(accel::Category::Layout, "unexpected int/float layout"));
    }
    if STR_LAYOUT.get().is_none()
        && let Some(layout) = calibrate_str(py)
    {
        let _ = STR_LAYOUT.set(layout);
    }
    Ok(())
}

/// What the native sort needs, for `list.sort` and `builtins.sorted`: calibrated layouts
/// and the interned keyword names.
pub(crate) fn prepare_sort_support(py: Python<'_>) -> Result<(), Refusal> {
    calibrate(py)?;
    for (cell, name) in [(&KW_KEY, "key"), (&KW_REVERSE, "reverse")] {
        if cell.load(std::sync::atomic::Ordering::Relaxed).is_null() {
            // Interned strings live as long as the interpreter; keep one reference forever.
            cell.store(PyString::intern(py, name).into_ptr(), std::sync::atomic::Ordering::Relaxed);
        }
    }
    Ok(())
}

/// The `PyMethodDef` of `list.sort` (its `ml_meth` is read at call time by
/// `builtins.sorted`, exactly as the bound method CPython's `sorted` builds would read it).
pub(crate) fn list_sort_def(py: Python<'_>) -> Result<*mut ffi::PyMethodDef, Refusal> {
    let m = method(&py.get_type::<PyList>(), c"sort", ffi::METH_FASTCALL | ffi::METH_KEYWORDS)?;
    // SAFETY: attached thread; checked to be a method descriptor by `method`.
    unsafe { accel::site::method_def(m.as_ptr()) }
        .ok_or_else(|| refuse!(Conflict: "list.sort: not a method descriptor"))
}

fn prepare_list_sort(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare_sort_support(py)?;
    let m = method(&py.get_type::<PyList>(), c"sort", ffi::METH_FASTCALL | ffi::METH_KEYWORDS)?;
    cx.fingerprint_c(&m, FP_SORT)?;
    cx.ml_meth(&m, k_list_sort as FastKwFn, &ORIG_LIST_SORT)
}

fn prepare_list_index(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    calibrate(py)?;
    let m = method(&py.get_type::<PyList>(), c"index", ffi::METH_FASTCALL)?;
    cx.fingerprint_c(&m, FP_INDEX)?;
    cx.ml_meth(&m, k_list_index as FastFn, &ORIG_LIST_INDEX)
}

fn prepare_list_count(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    calibrate(py)?;
    let m = method(&py.get_type::<PyList>(), c"count", ffi::METH_O)?;
    cx.fingerprint_c(&m, FP_COUNT)?;
    cx.ml_meth(&m, k_list_count as OFn, &ORIG_LIST_COUNT)
}

fn prepare_tuple_index(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    calibrate(py)?;
    let m = method(&py.get_type::<PyTuple>(), c"index", ffi::METH_FASTCALL)?;
    cx.fingerprint_c(&m, FP_INDEX)?;
    cx.ml_meth(&m, k_tuple_index as FastFn, &ORIG_TUPLE_INDEX)
}

fn prepare_tuple_count(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    calibrate(py)?;
    let m = method(&py.get_type::<PyTuple>(), c"count", ffi::METH_O)?;
    cx.fingerprint_c(&m, FP_COUNT)?;
    cx.ml_meth(&m, k_tuple_count as OFn, &ORIG_TUPLE_COUNT)
}

fn prepare_list_contains(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    calibrate(py)?;
    cx.slot(&py.get_type::<PyList>(), SlotName::SqContains, k_list_contains as ContainsFn, &ORIG_LIST_CONTAINS)
}

fn prepare_tuple_contains(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    calibrate(py)?;
    cx.slot(&py.get_type::<PyTuple>(), SlotName::SqContains, k_tuple_contains as ContainsFn, &ORIG_TUPLE_CONTAINS)
}

macro_rules! target {
    ($id:ident, $name:literal, $counters:ident, $prepare:ident) => {
        pub static $id: TargetSpec = TargetSpec {
            name: $name,
            group: "containers",
            module: None,
            versions: VERSIONS,
            free_threaded: true,
            counters: &$counters,
            prepare: $prepare,
            ..TargetSpec::DEFAULT
        };
    };
}

target!(LIST_SORT, "list.sort", C_LIST_SORT, prepare_list_sort);
target!(LIST_INDEX, "list.index", C_LIST_INDEX, prepare_list_index);
target!(LIST_COUNT, "list.count", C_LIST_COUNT, prepare_list_count);
target!(TUPLE_INDEX, "tuple.index", C_TUPLE_INDEX, prepare_tuple_index);
target!(TUPLE_COUNT, "tuple.count", C_TUPLE_COUNT, prepare_tuple_count);
target!(LIST_CONTAINS, "list.__contains__", C_LIST_CONTAINS, prepare_list_contains);
target!(TUPLE_CONTAINS, "tuple.__contains__", C_TUPLE_CONTAINS, prepare_tuple_contains);

/// The `containers` group.
pub static TARGETS: &[&TargetSpec] =
    &[&LIST_SORT, &LIST_INDEX, &LIST_COUNT, &TUPLE_INDEX, &TUPLE_COUNT, &LIST_CONTAINS, &TUPLE_CONTAINS];

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

fn group() -> Vec<String> {
    vec!["containers".to_owned()]
}

/// Install the `containers` group (kept for tests and benchmarks; `rustique.enable()` is
/// the API). Idempotent. Returns the names of the installed targets of the group.
#[pyfunction]
fn install(py: Python<'_>) -> PyResult<Vec<&'static str>> {
    accel::registry::enable(py, Some(group()), Vec::new())?;
    let names = accel::registry::installed(py, Some(&group()))?;
    if names.len() != TARGETS.len() {
        let st = accel::registry::status(py)?;
        return Err(PyRuntimeError::new_err(format!("rustique takeover: not every container target installed: {st}")));
    }
    Ok(names)
}

/// Restore the `containers` group. Idempotent.
#[pyfunction]
fn uninstall(py: Python<'_>) -> PyResult<()> {
    accel::registry::disable(py, Some(group()))
}

/// Whether `str` kind/data are read inline (a calibrated layout), for tests and reports.
#[pyfunction]
fn inline_str_layout() -> bool {
    STR_LAYOUT.get().is_some()
}

/// Names of the installed `containers` targets.
#[pyfunction]
fn installed(py: Python<'_>) -> PyResult<Vec<&'static str>> {
    accel::registry::installed(py, Some(&group()))
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
