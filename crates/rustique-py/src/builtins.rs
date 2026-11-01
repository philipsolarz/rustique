//! Replacement builtin functions: `sum`, `min`, `max`, `sorted`, `any`, `all`.
//!
//! OWNER: objects+builtins workstream (WS5).
//!
//! # Mechanism
//!
//! [`install`] puts a replacement for each function into the `builtins` module (and
//! [`uninstall`] restores the originals). A replacement is a `builtin_function_or_method`
//! built from a `PyMethodDef` that copies the original's name, docstring (hence
//! `__text_signature__`, `inspect.signature`, `help()`) and call flags, with the original's
//! `__self__` (the `builtins` module) and `__module__`. So it is indistinguishable from the
//! original except by identity: `__name__`/`__qualname__`/`__module__`/`repr` are equal,
//! `pickle.dumps(sum)` pickles by reference to `builtins.sum`, and the interpreter's
//! `CALL_BUILTIN_FAST_WITH_KEYWORDS` / `CALL_BUILTIN_O` specialisations fire exactly as for
//! the originals (same `ml_flags`).
//!
//! The entry points are raw `extern "C"` functions (no PyO3 argument extraction): they look
//! at the argument types and either take a fast path or call the *original* C function with
//! the original arguments, through the `ml_meth` pointer read from the saved original
//! function object at call time. The fallback therefore costs a few type compares and one
//! indirect call, and every error message, argument check and corner case of a fallback is
//! CPython's own.
//!
//! # Fast paths (results identical to CPython's, including which object is returned)
//!
//! - **Builtin `list`/`tuple`** (and the positional arguments of `min(a, b, ...)`): the item
//!   array is scanned directly (a list under its critical section on free-threaded builds)
//!   while the items have an exact builtin type whose handling runs no Python code
//!   (`int`/`bool`/`float`/`str`, plus `None`/`tuple`/`list`/`bytes` for truthiness). The
//!   first other item ends the scan; the computation then *continues* from that index
//!   exactly as CPython's loop would (items fetched one at a time from the live list, like
//!   `listiter`, Python code allowed), carrying the state over. Nothing observable happened
//!   during the scan, so the result, the order of Python-level calls and the reaction to
//!   mutation are CPython's.
//! - **Builtin `set`/`frozenset`/`dict`** (keys): the same scan over the hash table (under
//!   the critical section); if an item needs Python code, the original builtin is called
//!   from scratch (the scan had no side effects).
//! - **`rustique.List`**: native storage runs the `rustique-core` kernels (`sum` with
//!   CPython's compensated float summation, `min`/`max` with CPython's tie rules, `sort`) on
//!   the column under the list's read lock; object storage is scanned like a builtin list
//!   (on a snapshot); anything else calls the original builtin, which iterates the list.
//! - **`rustique.Set`**: its elements are scanned like a builtin set.
//! - `sorted(rustique.List / Set)` returns a new builtin `list`: native columns are sorted
//!   natively and boxed; otherwise (a `key`, NaNs, object storage) the elements are copied
//!   into a new builtin list and `list.sort` runs on it, as `sorted` does.
//!
//! `sum` follows `builtin_sum_impl` state by state (exact `int` accumulation in a C long,
//! then compensated float summation, then complex, then generic `+`), so float results are
//! bit-identical and integer overflow switches to exact big-int addition at the same point.
//!
//! # Concurrency
//!
//! Builtin lists, sets and dicts are read inside their per-object critical section; only
//! exact-type reads and the creation of `int`/`float` results happen inside it (no Python
//! code: no `__eq__`, no `__lt__`, no `__bool__`, no decref of objects with finalizers).
//! Rustique containers are read under their own read lock with the same rule (§6 of
//! ARCHITECTURE.md). Install and uninstall are thread-safe; a call racing with them sees
//! either function.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::ffi::{CStr, c_int};
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, Ordering as Atomic};

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::ffi;
use pyo3::impl_::trampoline::{self, MethodDef};
use pyo3::prelude::*;
use pyo3::sync::critical_section::with_critical_section;
use pyo3::types::PyString;
use rustique_core::kernels::{self, Sum};
use rustique_core::{Column, with_column};

use crate::accel::{self, Counters, Original, Prepare, Reason, Refusal, TargetSpec};
use crate::convert::{self, ToPython};
use crate::dict::{Dict, DictKeysRef};
use crate::frozenset::FrozenSet;
use crate::list::List;
use crate::objsort;
use crate::refuse;
use crate::set::{Set, SetElements};
use crate::storage::Storage;
use crate::tuple::Tuple;

/// Native kernels on at least this many elements run detached from the interpreter.
const DETACH_THRESHOLD: usize = 1 << 16;
/// Native sorts of at least this many elements run detached (they use the rayon pool).
const SORT_DETACH_THRESHOLD: usize = 1 << 14;
/// `any`/`all` snapshot a Rustique container's elements (no lazy element access exists
/// yet) only up to this size; larger ones are iterated lazily by the original builtin.
const SNAPSHOT_TRUTH_MAX: usize = 32;
/// `sorted` of a non-List Rustique container sorts a `str` column natively below this size
/// (above it, sorting the boxed objects with `list.sort` is cheaper).
const NATIVE_STR_SORT_MAX: usize = 32;

// =======================================================================================
// Raw object access (exact builtin types only)
// =======================================================================================

/// `PyLongObject` (CPython >= 3.12): header, `lv_tag` (sign in bits 0-1: 0 positive,
/// 1 zero, 2 negative; digit count from bit 3), 30-bit digits. Checked by
/// [`check_layouts`] before anything is installed.
#[repr(C)]
struct RawLong {
    ob_base: ffi::PyObject,
    lv_tag: usize,
    ob_digit: [u32; 1],
}

const LONG_NON_SIZE_BITS: u32 = 3;
const SIGN_ZERO: usize = 1;

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
#[inline(always)]
fn list_type() -> *mut ffi::PyTypeObject {
    &raw mut ffi::PyList_Type
}
#[inline(always)]
fn tuple_type() -> *mut ffi::PyTypeObject {
    &raw mut ffi::PyTuple_Type
}
#[inline(always)]
fn complex_type() -> *mut ffi::PyTypeObject {
    &raw mut ffi::PyComplex_Type
}

#[inline(always)]
unsafe fn type_of(op: *mut ffi::PyObject) -> *mut ffi::PyTypeObject {
    // SAFETY: caller passes a live object.
    unsafe { ffi::Py_TYPE(op) }
}

/// Value of an exact `int`/`bool` if it fits `i64` (CPython's `Py_ssize_t` on 64-bit).
///
/// # Safety
/// `op` must be a live object of type exactly `int` or `bool`.
#[inline(always)]
unsafe fn int_value(op: *mut ffi::PyObject) -> Option<i64> {
    // SAFETY: exact int/bool, so the `RawLong` prefix is valid (layout checked at install).
    let (tag, digit) = unsafe {
        let l = op.cast::<RawLong>();
        ((*l).lv_tag, (*l).ob_digit[0])
    };
    if tag < (2 << LONG_NON_SIZE_BITS) {
        // Compact (at most one digit), as `_PyLong_CompactValue`.
        return Some((1 - (tag & 3) as i64) * i64::from(digit));
    }
    let mut overflow: c_int = 0;
    // SAFETY: exact int: no `__index__` call; overflow reported by flag, no error possible.
    let v = unsafe { ffi::PyLong_AsLongLongAndOverflow(op, &mut overflow) };
    (overflow == 0).then_some(v)
}

/// # Safety
/// `op` must be a live `float` (or subclass).
#[inline(always)]
unsafe fn float_value(op: *mut ffi::PyObject) -> f64 {
    // SAFETY: float layout.
    unsafe { (*op.cast::<ffi::PyFloatObject>()).ob_fval }
}

/// Truthiness of objects whose `__bool__`/`__len__` is a builtin C slot that runs no
/// Python code; `None` for anything else.
///
/// # Safety
/// `op` must be a live object.
#[inline(always)]
unsafe fn pure_truth(op: *mut ffi::PyObject) -> Option<bool> {
    // SAFETY: each read matches the checked exact type.
    unsafe {
        if op == ffi::Py_True() {
            return Some(true);
        }
        if op == ffi::Py_False() || op == ffi::Py_None() {
            return Some(false);
        }
        let t = type_of(op);
        if t == long_type() {
            Some((*op.cast::<RawLong>()).lv_tag & 3 != SIGN_ZERO)
        } else if t == float_type() {
            Some(float_value(op) != 0.0)
        } else if t == str_type() {
            Some(ffi::PyUnicode_GET_LENGTH(op) != 0)
        } else if t == tuple_type() {
            Some(ffi::PyTuple_GET_SIZE(op) != 0)
        } else if t == list_type() {
            Some(ffi::PyList_GET_SIZE(op) != 0)
        } else if t == &raw mut ffi::PyBytes_Type {
            Some(ffi::Py_SIZE(op) != 0)
        } else {
            None
        }
    }
}

/// Exact comparison of an integer with a float (Python's `float_richcompare`); `None` when
/// the float is NaN.
#[inline]
fn cmp_int_float(i: i64, f: f64) -> Option<Ordering> {
    const TWO53: i64 = 1 << 53;
    const TWO63: f64 = 9_223_372_036_854_775_808.0;
    if f.is_nan() {
        return None;
    }
    if (-TWO53..=TWO53).contains(&i) {
        // Exactly representable.
        return (i as f64).partial_cmp(&f);
    }
    if f >= TWO63 {
        return Some(Ordering::Less);
    }
    if f < -TWO63 {
        return Some(Ordering::Greater);
    }
    // -2^63 <= f < 2^63: the integral part fits i64.
    let whole = f.trunc();
    Some(i.cmp(&(whole as i64)).then_with(|| {
        let frac = f - whole;
        if frac > 0.0 {
            Ordering::Less
        } else if frac < 0.0 {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }))
}

/// Code-point order of two exact `str`s (Python's `unicode_compare`).
///
/// One call into CPython: on 3.14+ the `PyUnicode_KIND`/`PyUnicode_DATA` accessors are
/// out-of-line functions, so reading the strings here would cost four calls.
///
/// # Safety
/// Both must be live exact `str` objects.
#[inline]
unsafe fn str_cmp(a: *mut ffi::PyObject, b: *mut ffi::PyObject) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    // SAFETY: two strings: the comparison cannot fail and runs no Python code.
    unsafe { ffi::PyUnicode_Compare(a, b) }.cmp(&0)
}

// =======================================================================================
// Sources
// =======================================================================================

/// A sequence read like CPython's `listiter`/`tupleiter`: by index, re-checking a list's
/// current length at every step.
#[derive(Clone, Copy)]
enum Seq {
    List(*mut ffi::PyObject),
    Tuple(*mut ffi::PyObject),
    /// The positional arguments of the call (`min(a, b, ...)`).
    Args(*const *mut ffi::PyObject, usize),
}

impl Seq {
    /// Run `f` on the items as borrowed pointers: a list's under its critical section
    /// (`f` must not run Python code nor block), a tuple's or the arguments directly.
    ///
    /// # Safety
    /// Attached thread; the sequence is alive for the call.
    #[inline(always)]
    unsafe fn with_items<R>(self, py: Python<'_>, f: impl FnOnce(&[*mut ffi::PyObject]) -> R) -> R {
        // SAFETY: exact list/tuple layouts; a list's `ob_item` is stable while its critical
        // section is held (no other thread can mutate it, and `f` runs no Python code).
        unsafe {
            match self {
                Seq::List(list) => {
                    let bound = Borrowed::from_ptr(py, list);
                    with_critical_section(&bound, || {
                        let n = ffi::PyList_GET_SIZE(list) as usize;
                        if n == 0 {
                            f(&[])
                        } else {
                            f(std::slice::from_raw_parts((*list.cast::<ffi::PyListObject>()).ob_item, n))
                        }
                    })
                }
                Seq::Tuple(tuple) => {
                    let n = ffi::PyTuple_GET_SIZE(tuple) as usize;
                    if n == 0 {
                        f(&[])
                    } else {
                        f(std::slice::from_raw_parts((*tuple.cast::<ffi::PyTupleObject>()).ob_item.as_ptr(), n))
                    }
                }
                Seq::Args(args, n) => f(if n == 0 { &[] } else { std::slice::from_raw_parts(args, n) }),
            }
        }
    }

    /// Item `i` as a new reference, or `None` past the current end.
    ///
    /// # Safety
    /// Attached thread; the sequence is alive.
    #[inline]
    unsafe fn get<'py>(self, py: Python<'py>, i: usize) -> Option<Bound<'py, PyAny>> {
        // SAFETY: bounds-checked reads; `PyList_GetItemRef` is the thread-safe accessor.
        unsafe {
            match self {
                Seq::List(list) => {
                    if i >= ffi::PyList_GET_SIZE(list) as usize {
                        return None;
                    }
                    let item = ffi::compat::PyList_GetItemRef(list, i as ffi::Py_ssize_t);
                    if item.is_null() {
                        // Shrunk concurrently (free-threaded): the end, as for `listiter`.
                        ffi::PyErr_Clear();
                        return None;
                    }
                    Some(Bound::from_owned_ptr(py, item))
                }
                Seq::Tuple(tuple) => (i < ffi::PyTuple_GET_SIZE(tuple) as usize).then(|| {
                    Bound::from_borrowed_ptr(py, *(*tuple.cast::<ffi::PyTupleObject>()).ob_item.as_ptr().add(i))
                }),
                Seq::Args(args, n) => (i < n).then(|| Bound::from_borrowed_ptr(py, *args.add(i))),
            }
        }
    }
}

unsafe extern "C" {
    /// The marker of deleted set-table entries (exported by CPython).
    static _PySet_Dummy: *mut ffi::PyObject;
}

/// Live keys of an exact `set`/`frozenset`, in iteration order (table order).
///
/// # Safety
/// The set must stay unmodified while the iterator is used (critical section held for
/// `set`; frozensets are immutable).
unsafe fn set_keys(set: *mut ffi::PyObject) -> impl Iterator<Item = *mut ffi::PyObject> {
    // SAFETY: `PySetObject` layout from the CPython headers (checked at install).
    let (table, mask, dummy) = unsafe {
        let so = set.cast::<ffi::PySetObject>();
        ((*so).table, (*so).mask as usize, _PySet_Dummy)
    };
    (0..=mask).filter_map(move |i| {
        // SAFETY: `table` has `mask + 1` entries.
        let key = unsafe { (*table.add(i)).key };
        (!key.is_null() && key != dummy).then_some(key)
    })
}

/// Keys of an exact `dict`, in iteration order.
///
/// # Safety
/// The dict's critical section must be held while the iterator is used.
unsafe fn dict_keys(dict: *mut ffi::PyObject) -> impl Iterator<Item = *mut ffi::PyObject> {
    let mut pos: ffi::Py_ssize_t = 0;
    std::iter::from_fn(move || {
        let (mut key, mut value) = (ptr::null_mut(), ptr::null_mut());
        // SAFETY: borrowed references from a dict whose critical section is held.
        (unsafe { ffi::PyDict_Next(dict, &mut pos, &mut key, &mut value) } != 0).then_some(key)
    })
}

/// The kind of a builtin hashed container.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hashed {
    Set,
    FrozenSet,
    Dict,
}

impl Hashed {
    #[inline(always)]
    fn of(t: *mut ffi::PyTypeObject) -> Option<Hashed> {
        if t == &raw mut ffi::PySet_Type {
            Some(Hashed::Set)
        } else if t == &raw mut ffi::PyFrozenSet_Type {
            Some(Hashed::FrozenSet)
        } else if t == &raw mut ffi::PyDict_Type {
            Some(Hashed::Dict)
        } else {
            None
        }
    }

    /// Whether `sum`/`min`/`max` kernels pay off for this container (measured, ratio vs
    /// CPython's builtin): dict keys always do (0.38-0.89), and so does every set up to a
    /// thousand elements (0.31-0.92). A large `set`/`frozenset` walks its whole table in hash
    /// order, which only pays off for numbers (int/float 0.62-0.90 at 100k-1M, while str
    /// reaches 1.02 and tuples 1.17-1.20 at 1M), so above [`LARGE_SET`] elements sets go to
    /// the kernels only when their first element is an exact `int` or `float`.
    ///
    /// # Safety
    /// As [`Hashed::with_keys`].
    unsafe fn kernels_pay_off(self, py: Python<'_>, obj: *mut ffi::PyObject) -> bool {
        // SAFETY: forwarded; only the size (atomically on free-threaded builds) and the
        // first key's type are read.
        self == Hashed::Dict
            || unsafe { ffi::PySet_GET_SIZE(obj) as usize } < LARGE_SET
            || unsafe {
                self.with_keys(py, obj, |keys| {
                    keys.next().is_none_or(|k| {
                        let t = type_of(k);
                        t == long_type() || t == float_type()
                    })
                })
            }
    }

    /// Run `f` on the container's keys (see [`Seq::with_items`] for the rules on `f`).
    ///
    /// # Safety
    /// Attached thread; `obj` is a live container of this kind.
    unsafe fn with_keys<R>(
        self,
        py: Python<'_>,
        obj: *mut ffi::PyObject,
        f: impl FnOnce(&mut dyn Iterator<Item = *mut ffi::PyObject>) -> R,
    ) -> R {
        // SAFETY: the iterators' requirements are met by the critical section (or
        // immutability for frozensets).
        unsafe {
            match self {
                Hashed::FrozenSet => f(&mut set_keys(obj)),
                Hashed::Set => with_critical_section(&Borrowed::from_ptr(py, obj), || f(&mut set_keys(obj))),
                Hashed::Dict => with_critical_section(&Borrowed::from_ptr(py, obj), || f(&mut dict_keys(obj))),
            }
        }
    }
}

/// From this size on, a builtin set's table no longer stays cache-resident and walking it in
/// hash order loses to CPython's iteration for some element kinds (see
/// [`Hashed::kernels_pay_off`]): measured wins at 1k elements, losses from 100k.
const LARGE_SET: usize = 1 << 11;

/// How a scan without Python code ended.
enum Scan<T> {
    /// Finished with a result.
    Done(T),
    /// Item at this index needs Python code; nothing observable happened before it.
    Bail(usize),
    /// A Python exception is set (raised by exact-type operations, as CPython would).
    Error,
}

/// Result of a kernel: an object (NULL = exception set) or "use the original builtin".
enum Out {
    Obj(*mut ffi::PyObject),
    Fallback,
}

// =======================================================================================
// sum: a state machine following `builtin_sum_impl` (CPython >= 3.12)
// =======================================================================================

/// CPython's `CompensatedSum` (`cs_from_double` / `cs_add` / `cs_to_double`).
#[derive(Clone, Copy)]
struct Cs {
    hi: f64,
    lo: f64,
}

impl Cs {
    #[inline(always)]
    fn new(x: f64) -> Cs {
        Cs { hi: x, lo: 0.0 }
    }

    #[inline(always)]
    fn add(&mut self, x: f64) {
        let t = self.hi + x;
        if self.hi.abs() >= x.abs() {
            self.lo += (self.hi - t) + x;
        } else {
            self.lo += (x - t) + self.hi;
        }
        self.hi = t;
    }

    #[inline(always)]
    fn value(self) -> f64 {
        if self.lo != 0.0 && self.lo.is_finite() { self.hi + self.lo } else { self.hi }
    }
}

enum SumState<'py> {
    /// The C-long fast path (`i_result`).
    Int(i64),
    /// The compensated float path.
    Float(Cs),
    /// The compensated complex path (real, imaginary).
    Complex(Cs, Cs),
    /// The generic path: `result = result + item`.
    Obj(Bound<'py, PyAny>),
}

/// Outcome of feeding one item to a [`SumState`].
enum Step {
    Ok,
    /// The item needs Python code (only reported when that is not allowed).
    Impure,
    Error,
}

impl<'py> SumState<'py> {
    /// Initial state for `start` (`NULL`: the default `0`). Only exact `int`/`float` starts
    /// are handled; `None` otherwise.
    ///
    /// # Safety
    /// `start` is NULL or a live object.
    unsafe fn new(py: Python<'py>, start: *mut ffi::PyObject) -> Option<SumState<'py>> {
        if start.is_null() {
            return Some(SumState::Int(0));
        }
        // SAFETY: exact-type reads of a live object.
        unsafe {
            let t = type_of(start);
            if t == long_type() {
                Some(match int_value(start) {
                    Some(v) => SumState::Int(v),
                    None => SumState::Obj(Bound::from_borrowed_ptr(py, start)),
                })
            } else if t == float_type() {
                Some(SumState::Float(Cs::new(float_value(start))))
            } else {
                None
            }
        }
    }

    /// The state after leaving the int path with `result` (CPython checks for an exact
    /// float, then an exact complex, before falling back to generic addition).
    ///
    /// # Safety
    /// `result` is a live object.
    unsafe fn after_int(result: Bound<'py, PyAny>) -> SumState<'py> {
        // SAFETY: exact-type checks and reads.
        unsafe {
            if type_of(result.as_ptr()) == float_type() {
                SumState::Float(Cs::new(float_value(result.as_ptr())))
            } else {
                SumState::after_float(result)
            }
        }
    }

    /// # Safety
    /// `result` is a live object.
    unsafe fn after_float(result: Bound<'py, PyAny>) -> SumState<'py> {
        // SAFETY: exact complex layout.
        unsafe {
            if type_of(result.as_ptr()) == complex_type() {
                let z = (*result.as_ptr().cast::<ffi::PyComplexObject>()).cval;
                SumState::Complex(Cs::new(z.real), Cs::new(z.imag))
            } else {
                SumState::Obj(result)
            }
        }
    }

    /// `PyNumber_Add(a, b)` as an owned result.
    ///
    /// # Safety
    /// Live objects; may run Python code unless both are exact numbers.
    #[inline]
    unsafe fn add(py: Python<'py>, a: *mut ffi::PyObject, b: *mut ffi::PyObject) -> Option<Bound<'py, PyAny>> {
        // SAFETY: new reference or NULL with an exception set.
        unsafe { Bound::from_owned_ptr_or_opt(py, ffi::PyNumber_Add(a, b)) }
    }

    /// Feed one item. With `python == false`, an item whose handling could run Python
    /// code is not consumed and `Step::Impure` is returned.
    ///
    /// # Safety
    /// `item` is a live object.
    #[inline(always)]
    unsafe fn step(&mut self, py: Python<'py>, item: *mut ffi::PyObject, python: bool) -> Step {
        // SAFETY: exact-type reads; `PyNumber_Add` on exact int/float operands runs no Python
        // code, other operands only when `python` allows it.
        unsafe {
            let t = type_of(item);
            match self {
                SumState::Int(acc) => {
                    if t == long_type() || t == bool_type() {
                        if let Some(b) = int_value(item)
                            && let Some(s) = acc.checked_add(b)
                        {
                            *acc = s;
                            return Step::Ok;
                        }
                        // C-long overflow: exact int addition (no Python code).
                    } else if t != float_type() && !python {
                        return Step::Impure;
                    }
                    let Some(boxed) = Bound::from_owned_ptr_or_opt(py, ffi::PyLong_FromLongLong(*acc)) else {
                        return Step::Error;
                    };
                    let Some(result) = Self::add(py, boxed.as_ptr(), item) else {
                        return Step::Error;
                    };
                    *self = SumState::after_int(result);
                    Step::Ok
                }
                SumState::Float(cs) => {
                    if t == float_type() {
                        cs.add(float_value(item));
                        return Step::Ok;
                    }
                    if ffi::PyLong_Check(item) != 0 {
                        // `PyLong_AsDouble` runs no Python code (also for int subclasses).
                        let v = if t == long_type() || t == bool_type() {
                            match int_value(item) {
                                Some(v) => v as f64,
                                None => ffi::PyLong_AsDouble(item),
                            }
                        } else {
                            ffi::PyLong_AsDouble(item)
                        };
                        if v == -1.0 && !ffi::PyErr_Occurred().is_null() {
                            return Step::Error;
                        }
                        cs.add(v);
                        return Step::Ok;
                    }
                    if !python {
                        return Step::Impure;
                    }
                    let Some(boxed) = Bound::from_owned_ptr_or_opt(py, ffi::PyFloat_FromDouble(cs.value())) else {
                        return Step::Error;
                    };
                    let Some(result) = Self::add(py, boxed.as_ptr(), item) else {
                        return Step::Error;
                    };
                    *self = SumState::after_float(result);
                    Step::Ok
                }
                SumState::Complex(re, im) => {
                    if t == complex_type() {
                        let z = (*item.cast::<ffi::PyComplexObject>()).cval;
                        re.add(z.real);
                        im.add(z.imag);
                        return Step::Ok;
                    }
                    if ffi::PyLong_Check(item) != 0 {
                        let v = ffi::PyLong_AsDouble(item);
                        if v == -1.0 && !ffi::PyErr_Occurred().is_null() {
                            return Step::Error;
                        }
                        re.add(v);
                        return Step::Ok;
                    }
                    if ffi::PyFloat_Check(item) != 0 {
                        re.add(float_value(item));
                        return Step::Ok;
                    }
                    if !python {
                        return Step::Impure;
                    }
                    let Some(boxed) =
                        Bound::from_owned_ptr_or_opt(py, ffi::PyComplex_FromDoubles(re.value(), im.value()))
                    else {
                        return Step::Error;
                    };
                    let Some(result) = Self::add(py, boxed.as_ptr(), item) else {
                        return Step::Error;
                    };
                    *self = SumState::Obj(result);
                    Step::Ok
                }
                SumState::Obj(result) => {
                    let rt = type_of(result.as_ptr());
                    let pure = (rt == long_type() || rt == float_type())
                        && (t == long_type() || t == bool_type() || t == float_type());
                    if !pure && !python {
                        return Step::Impure;
                    }
                    let Some(next) = Self::add(py, result.as_ptr(), item) else {
                        return Step::Error;
                    };
                    *result = next;
                    Step::Ok
                }
            }
        }
    }

    /// Feed items until one needs Python code.
    ///
    /// # Safety
    /// Live items; Python code must not run (critical sections may be held).
    #[inline]
    unsafe fn scan(&mut self, py: Python<'py>, items: impl Iterator<Item = *mut ffi::PyObject>) -> Scan<()> {
        for (k, item) in items.enumerate() {
            // SAFETY: forwarded.
            match unsafe { self.step(py, item, false) } {
                Step::Ok => {}
                Step::Impure => return Scan::Bail(k),
                Step::Error => return Scan::Error,
            }
        }
        Scan::Done(())
    }

    /// [`scan`](Self::scan) for a slice, with tight loops over runs of exact ints (while
    /// the C-long accumulator does not overflow) and exact floats.
    ///
    /// # Safety
    /// As [`scan`](Self::scan).
    #[inline]
    unsafe fn scan_slice(&mut self, py: Python<'py>, items: &[*mut ffi::PyObject]) -> Scan<()> {
        let n = items.len();
        let mut i = 0;
        while i < n {
            // SAFETY: live items; exact-type reads.
            unsafe {
                match self {
                    SumState::Int(acc) => {
                        let mut a = *acc;
                        while i < n {
                            let p = *items.get_unchecked(i);
                            let t = type_of(p);
                            if t != long_type() && t != bool_type() {
                                break;
                            }
                            let Some(s) = int_value(p).and_then(|v| a.checked_add(v)) else {
                                break;
                            };
                            a = s;
                            i += 1;
                        }
                        *acc = a;
                    }
                    SumState::Float(cs) => {
                        while i < n {
                            let p = *items.get_unchecked(i);
                            if type_of(p) != float_type() {
                                break;
                            }
                            cs.add(float_value(p));
                            i += 1;
                        }
                    }
                    _ => {}
                }
                if i == n {
                    break;
                }
                match self.step(py, *items.get_unchecked(i), false) {
                    Step::Ok => i += 1,
                    Step::Impure => return Scan::Bail(i),
                    Step::Error => return Scan::Error,
                }
            }
        }
        Scan::Done(())
    }

    /// The result object (NULL with an exception set on failure).
    fn finish(self) -> *mut ffi::PyObject {
        // SAFETY: creating result objects; NULL propagates the exception.
        unsafe {
            match self {
                SumState::Int(v) => ffi::PyLong_FromLongLong(v),
                SumState::Float(cs) => ffi::PyFloat_FromDouble(cs.value()),
                SumState::Complex(re, im) => ffi::PyComplex_FromDoubles(re.value(), im.value()),
                SumState::Obj(r) => r.into_ptr(),
            }
        }
    }
}

/// `sum(seq, start)` for a builtin list/tuple.
///
/// # Safety
/// Attached thread; `start` is NULL or an exact `int`/`float`.
unsafe fn sum_seq(py: Python<'_>, seq: Seq, start: *mut ffi::PyObject) -> Out {
    // SAFETY: forwarded requirements; the scan runs no Python code.
    unsafe {
        let Some(mut state) = SumState::new(py, start) else {
            return Out::Fallback;
        };
        match seq.with_items(py, |items| state.scan_slice(py, items)) {
            Scan::Done(()) => Out::Obj(state.finish()),
            Scan::Error => Out::Obj(ptr::null_mut()),
            Scan::Bail(mut i) => {
                // Continue exactly like CPython's loop over the live sequence.
                while let Some(item) = seq.get(py, i) {
                    match state.step(py, item.as_ptr(), true) {
                        Step::Ok => {}
                        Step::Error | Step::Impure => return Out::Obj(ptr::null_mut()),
                    }
                    i += 1;
                }
                Out::Obj(state.finish())
            }
        }
    }
}

/// `sum(container, start)` for a builtin set/frozenset/dict: a pure scan of the keys, or
/// the original builtin.
///
/// # Safety
/// Attached thread; `obj` a live container of kind `kind`; `start` NULL or exact int/float.
unsafe fn sum_hashed(py: Python<'_>, kind: Hashed, obj: *mut ffi::PyObject, start: *mut ffi::PyObject) -> Out {
    // SAFETY: forwarded requirements; the scan runs no Python code.
    unsafe {
        if !kind.kernels_pay_off(py, obj) {
            return Out::Fallback;
        }
        let Some(mut state) = SumState::new(py, start) else {
            return Out::Fallback;
        };
        match kind.with_keys(py, obj, |keys| state.scan(py, keys)) {
            Scan::Done(()) => Out::Obj(state.finish()),
            Scan::Error => Out::Obj(ptr::null_mut()),
            Scan::Bail(_) => Out::Fallback,
        }
    }
}

/// `sum` over borrowed items that the caller keeps alive (a snapshot): `None` if an item
/// needs Python code.
///
/// # Safety
/// Live items.
unsafe fn sum_items(
    py: Python<'_>,
    items: &[*mut ffi::PyObject],
    start: *mut ffi::PyObject,
) -> Option<*mut ffi::PyObject> {
    // SAFETY: forwarded.
    unsafe {
        let mut state = SumState::new(py, start)?;
        match state.scan_slice(py, items) {
            Scan::Done(()) => Some(state.finish()),
            Scan::Error => Some(ptr::null_mut()),
            Scan::Bail(_) => None,
        }
    }
}

// =======================================================================================
// min / max
// =======================================================================================

/// What `min`/`max` know about an item without running Python code.
#[derive(Clone, Copy)]
enum MKey {
    Int(i64),
    Float(f64),
    Str,
    /// An exact `int` beyond i64: compared with `PyObject_RichCompareBool` (pure).
    BigInt,
    /// An exact `tuple`: compared element-wise like `tuple_richcompare` while the elements
    /// compared are themselves pure.
    Tuple,
    /// `None` (any ordering comparison raises `TypeError`).
    None,
}

/// # Safety
/// `op` is a live object.
#[inline(always)]
unsafe fn mkey(op: *mut ffi::PyObject) -> Option<MKey> {
    // SAFETY: exact-type reads.
    unsafe {
        let t = type_of(op);
        if t == long_type() || t == bool_type() {
            Some(int_value(op).map_or(MKey::BigInt, MKey::Int))
        } else if t == float_type() {
            Some(MKey::Float(float_value(op)))
        } else if t == str_type() {
            Some(MKey::Str)
        } else if t == tuple_type() {
            Some(MKey::Tuple)
        } else if op == ffi::Py_None() {
            Some(MKey::None)
        } else {
            None
        }
    }
}

/// Outcome of a comparison attempted without running Python code.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmp {
    True,
    False,
    /// A Python exception is set (a builtin comparison raised, as CPython's would).
    Error,
    /// The comparison needs Python code (an element of a non-pure type).
    Impure,
}

impl Cmp {
    #[inline(always)]
    fn of(b: bool) -> Cmp {
        if b { Cmp::True } else { Cmp::False }
    }

    /// From a `PyObject_RichCompareBool` result.
    #[inline(always)]
    fn rich(r: c_int) -> Cmp {
        if r < 0 { Cmp::Error } else { Cmp::of(r > 0) }
    }
}

/// Nesting depth of tuples compared without Python code (deeper: handed to CPython).
const MAX_TUPLE_DEPTH: u32 = 16;

/// `a <op> b` for objects classified by [`mkey`] (`op`: `Py_LT`, `Py_GT` or `Py_EQ`),
/// following `PyObject_RichCompareBool` and `tuple_richcompare` exactly.
///
/// # Safety
/// Live objects; runs no Python code (returns `Impure` instead).
unsafe fn pure_compare(a: *mut ffi::PyObject, ak: MKey, b: *mut ffi::PyObject, bk: MKey, op: c_int, depth: u32) -> Cmp {
    if op == ffi::Py_EQ && a == b {
        return Cmp::True; // identity implies equality
    }
    let order = |o: Option<Ordering>| -> Cmp {
        match op {
            ffi::Py_LT => Cmp::of(o == Some(Ordering::Less)),
            ffi::Py_GT => Cmp::of(o == Some(Ordering::Greater)),
            _ => Cmp::of(o == Some(Ordering::Equal)),
        }
    };
    match (ak, bk) {
        (MKey::Int(x), MKey::Int(y)) => order(Some(x.cmp(&y))),
        (MKey::Float(x), MKey::Float(y)) => order(x.partial_cmp(&y)),
        (MKey::Int(x), MKey::Float(y)) => order(cmp_int_float(x, y)),
        (MKey::Float(x), MKey::Int(y)) => order(cmp_int_float(y, x).map(Ordering::reverse)),
        // SAFETY: both exact strings.
        (MKey::Str, MKey::Str) => order(Some(unsafe { str_cmp(a, b) })),
        (MKey::Tuple, MKey::Tuple) => {
            if depth >= MAX_TUPLE_DEPTH {
                return Cmp::Impure;
            }
            // SAFETY: exact tuples; their items are live while the tuples are.
            unsafe { tuple_compare(a, b, op, depth + 1) }
        }
        // Mixed kinds among int/float/str/tuple/None and big ints: CPython's own comparison
        // of exact builtin types (a `TypeError` for ordering mixed kinds, `False` for `==`).
        // SAFETY: live objects of exact builtin types: no Python code runs.
        _ => Cmp::rich(unsafe { ffi::PyObject_RichCompareBool(a, b, op) }),
    }
}

/// `tuple_richcompare(a, b, op)` for exact tuples, without Python code.
///
/// # Safety
/// Live exact tuples.
unsafe fn tuple_compare(a: *mut ffi::PyObject, b: *mut ffi::PyObject, op: c_int, depth: u32) -> Cmp {
    // SAFETY: tuple layout; items live as long as the tuples.
    unsafe {
        let (na, nb) = (ffi::PyTuple_GET_SIZE(a) as usize, ffi::PyTuple_GET_SIZE(b) as usize);
        let ia = (*a.cast::<ffi::PyTupleObject>()).ob_item.as_ptr();
        let ib = (*b.cast::<ffi::PyTupleObject>()).ob_item.as_ptr();
        // The first index where the items differ (`==` with the identity shortcut).
        let mut i = 0;
        while i < na && i < nb {
            let (x, y) = (*ia.add(i), *ib.add(i));
            if x != y {
                let (Some(xk), Some(yk)) = (mkey(x), mkey(y)) else {
                    return Cmp::Impure;
                };
                match pure_compare(x, xk, y, yk, ffi::Py_EQ, depth) {
                    Cmp::True => {}
                    Cmp::False => break,
                    other => return other,
                }
            }
            i += 1;
        }
        if i >= na || i >= nb {
            return Cmp::of(match op {
                ffi::Py_LT => na < nb,
                ffi::Py_GT => na > nb,
                _ => na == nb,
            });
        }
        if op == ffi::Py_EQ {
            return Cmp::False;
        }
        let (x, y) = (*ia.add(i), *ib.add(i));
        let (Some(xk), Some(yk)) = (mkey(x), mkey(y)) else {
            return Cmp::Impure;
        };
        pure_compare(x, xk, y, yk, op, depth)
    }
}

/// The current extreme of a scan.
#[derive(Clone, Copy)]
struct Best {
    obj: *mut ffi::PyObject,
    key: MKey,
}

/// Scan for the extreme while items need no Python code. On `Bail`, `best` holds the
/// extreme of the items before the bail index.
///
/// # Safety
/// Live items; runs no Python code.
#[inline]
unsafe fn minmax_scan(items: impl Iterator<Item = *mut ffi::PyObject>, op: c_int, best: &mut Option<Best>) -> Scan<()> {
    let max = op == ffi::Py_GT;
    for (k, item) in items.enumerate() {
        // SAFETY: live items; exact-type reads.
        let t = unsafe { type_of(item) };
        // Hot paths, inline: an item of the same kind as the current extreme.
        if let Some(b) = best {
            match b.key {
                MKey::Int(bv) if t == long_type() || t == bool_type() => {
                    // SAFETY: an exact int/bool.
                    if let Some(v) = unsafe { int_value(item) } {
                        if if max { v > bv } else { v < bv } {
                            *b = Best { obj: item, key: MKey::Int(v) };
                        }
                        continue;
                    }
                }
                MKey::Float(bv) if t == float_type() => {
                    // SAFETY: an exact float.
                    let v = unsafe { float_value(item) };
                    if if max { v > bv } else { v < bv } {
                        *b = Best { obj: item, key: MKey::Float(v) };
                    }
                    continue;
                }
                MKey::Str if t == str_type() => {
                    // SAFETY: two exact strings.
                    let o = unsafe { str_cmp(item, b.obj) };
                    if o == if max { Ordering::Greater } else { Ordering::Less } {
                        b.obj = item;
                    }
                    continue;
                }
                _ => {}
            }
        }
        // SAFETY: forwarded.
        match unsafe { minmax_step(item, op, best) } {
            Cmp::True | Cmp::False => {}
            Cmp::Error => return Scan::Error,
            Cmp::Impure => return Scan::Bail(k),
        }
    }
    Scan::Done(())
}

/// One item of a `min`/`max` scan, through the general path (`Impure`: not consumed).
///
/// # Safety
/// As [`minmax_scan`].
#[inline]
unsafe fn minmax_step(item: *mut ffi::PyObject, op: c_int, best: &mut Option<Best>) -> Cmp {
    // SAFETY: forwarded.
    let Some(ik) = (unsafe { mkey(item) }) else {
        return Cmp::Impure;
    };
    match best {
        None => {
            *best = Some(Best { obj: item, key: ik });
            Cmp::True
        }
        // `item <op> best`, as `min_max` asks.
        Some(b) => {
            // SAFETY: forwarded.
            let r = unsafe { pure_compare(item, ik, b.obj, b.key, op, 0) };
            if r == Cmp::True {
                *b = Best { obj: item, key: ik };
            }
            r
        }
    }
}

/// [`minmax_scan`] for a slice, with tight loops over runs of exact ints (fitting i64) and
/// exact floats once the current extreme is of that kind.
///
/// # Safety
/// As [`minmax_scan`].
#[inline]
unsafe fn minmax_scan_slice(items: &[*mut ffi::PyObject], op: c_int, best: &mut Option<Best>) -> Scan<()> {
    if op == ffi::Py_GT {
        // SAFETY: forwarded.
        unsafe { minmax_runs::<true>(items, best) }
    } else {
        // SAFETY: forwarded.
        unsafe { minmax_runs::<false>(items, best) }
    }
}

/// # Safety
/// As [`minmax_scan`].
#[inline(always)]
unsafe fn minmax_runs<const MAX: bool>(items: &[*mut ffi::PyObject], best: &mut Option<Best>) -> Scan<()> {
    let op = if MAX { ffi::Py_GT } else { ffi::Py_LT };
    let n = items.len();
    let mut i = 0;
    while i < n {
        // SAFETY: in-bounds reads of live items; exact-type reads.
        unsafe {
            match *best {
                Some(Best { obj, key: MKey::Int(bv) }) => {
                    let (mut bo, mut bv) = (obj, bv);
                    while i < n {
                        let p = *items.get_unchecked(i);
                        let t = type_of(p);
                        if t != long_type() && t != bool_type() {
                            break;
                        }
                        let Some(v) = int_value(p) else { break };
                        if if MAX { v > bv } else { v < bv } {
                            (bo, bv) = (p, v);
                        }
                        i += 1;
                    }
                    *best = Some(Best { obj: bo, key: MKey::Int(bv) });
                }
                Some(Best { obj, key: MKey::Float(bv) }) => {
                    let (mut bo, mut bv) = (obj, bv);
                    while i < n {
                        let p = *items.get_unchecked(i);
                        if type_of(p) != float_type() {
                            break;
                        }
                        let v = float_value(p);
                        if if MAX { v > bv } else { v < bv } {
                            (bo, bv) = (p, v);
                        }
                        i += 1;
                    }
                    *best = Some(Best { obj: bo, key: MKey::Float(bv) });
                }
                Some(Best { obj, key: MKey::Str }) => {
                    let want = if MAX { Ordering::Greater } else { Ordering::Less };
                    let mut bo = obj;
                    while i < n {
                        let p = *items.get_unchecked(i);
                        if type_of(p) != str_type() {
                            break;
                        }
                        if str_cmp(p, bo) == want {
                            bo = p;
                        }
                        i += 1;
                    }
                    *best = Some(Best { obj: bo, key: MKey::Str });
                }
                _ => {}
            }
            if i == n {
                break;
            }
            // One item of another kind, through the general path.
            match minmax_step(*items.get_unchecked(i), op, best) {
                Cmp::True | Cmp::False => i += 1,
                Cmp::Impure => return Scan::Bail(i),
                Cmp::Error => return Scan::Error,
            }
        }
    }
    Scan::Done(())
}

/// `min(a, b)` / `max(a, b)` for two exact `int`/`float`/`str` arguments: the extreme
/// (borrowed; `a` on ties, as CPython keeps the first), or `None` for other arguments.
///
/// # Safety
/// Live objects.
#[inline(always)]
unsafe fn minmax_pair(a: *mut ffi::PyObject, b: *mut ffi::PyObject, max: bool) -> Option<*mut ffi::PyObject> {
    // SAFETY: exact-type reads.
    unsafe {
        let (ta, tb) = (type_of(a), type_of(b));
        let want = if max { Ordering::Greater } else { Ordering::Less };
        // CPython asks `b <op> a`: `b` replaces `a` only when strictly better.
        let b_better = if (ta == long_type() || ta == bool_type()) && (tb == long_type() || tb == bool_type()) {
            let (x, y) = (int_value(a)?, int_value(b)?);
            y.cmp(&x) == want
        } else if ta == float_type() && tb == float_type() {
            float_value(b).partial_cmp(&float_value(a)) == Some(want)
        } else if ta == str_type() && tb == str_type() {
            str_cmp(b, a) == want
        } else if ta == float_type() && (tb == long_type() || tb == bool_type()) {
            cmp_int_float(int_value(b)?, float_value(a)) == Some(want)
        } else if (ta == long_type() || ta == bool_type()) && tb == float_type() {
            cmp_int_float(int_value(a)?, float_value(b)).map(Ordering::reverse) == Some(want)
        } else {
            return None;
        };
        Some(if b_better { b } else { a })
    }
}

/// The result of `min`/`max` for a finished scan: the extreme, the default, or ValueError.
///
/// # Safety
/// `best` (if any) and `default` (if non-NULL) are live.
unsafe fn minmax_result(
    best: Option<*mut ffi::PyObject>,
    default: *mut ffi::PyObject,
    op: c_int,
) -> *mut ffi::PyObject {
    // SAFETY: new references to live objects; error path sets an exception.
    unsafe {
        match best {
            Some(b) => ffi::Py_NewRef(b),
            None if !default.is_null() => ffi::Py_NewRef(default),
            None => {
                let msg = if op == ffi::Py_LT {
                    c"min() iterable argument is empty"
                } else {
                    c"max() iterable argument is empty"
                };
                ffi::PyErr_SetString(ffi::PyExc_ValueError, msg.as_ptr());
                ptr::null_mut()
            }
        }
    }
}

/// `min`/`max` (no key) over a builtin list/tuple or the positional arguments.
///
/// # Safety
/// Attached thread; live arguments.
unsafe fn minmax_seq(py: Python<'_>, seq: Seq, default: *mut ffi::PyObject, op: c_int) -> *mut ffi::PyObject {
    // SAFETY: forwarded requirements; the scan runs no Python code, and the extreme is
    // turned into a strong reference before the critical section ends.
    unsafe {
        let (scan, best) = seq.with_items(py, |items| {
            let mut best = None;
            let scan = minmax_scan_slice(items, op, &mut best);
            (scan, best.map(|b| Bound::from_borrowed_ptr(py, b.obj)))
        });
        match scan {
            Scan::Done(()) => minmax_result(best.as_ref().map(Bound::as_ptr), default, op),
            Scan::Error => ptr::null_mut(),
            Scan::Bail(mut i) => {
                // CPython's loop from here on, over the live sequence.
                let mut best = best;
                while let Some(item) = seq.get(py, i) {
                    match &best {
                        None => best = Some(item),
                        Some(b) => match ffi::PyObject_RichCompareBool(item.as_ptr(), b.as_ptr(), op) {
                            r if r < 0 => return ptr::null_mut(),
                            0 => {}
                            _ => best = Some(item),
                        },
                    }
                    i += 1;
                }
                minmax_result(best.as_ref().map(Bound::as_ptr), default, op)
            }
        }
    }
}

/// `min`/`max` over a builtin set/frozenset/dict (keys).
///
/// # Safety
/// Attached thread; live arguments.
unsafe fn minmax_hashed(
    py: Python<'_>,
    kind: Hashed,
    obj: *mut ffi::PyObject,
    default: *mut ffi::PyObject,
    op: c_int,
) -> Out {
    // SAFETY: as in `minmax_seq`.
    unsafe {
        if !kind.kernels_pay_off(py, obj) {
            return Out::Fallback;
        }
        let (scan, best) = kind.with_keys(py, obj, |keys| {
            let mut best = None;
            let scan = minmax_scan(keys, op, &mut best);
            (scan, best.map(|b| Bound::from_borrowed_ptr(py, b.obj)))
        });
        match scan {
            Scan::Done(()) => Out::Obj(minmax_result(best.as_ref().map(Bound::as_ptr), default, op)),
            Scan::Error => Out::Obj(ptr::null_mut()),
            Scan::Bail(_) => Out::Fallback,
        }
    }
}

/// `min`/`max` over items a snapshot keeps alive; `None` if an item needs Python code.
///
/// # Safety
/// Live items and default.
unsafe fn minmax_items(
    items: &[*mut ffi::PyObject],
    default: *mut ffi::PyObject,
    op: c_int,
) -> Option<*mut ffi::PyObject> {
    // SAFETY: forwarded.
    unsafe {
        let mut best = None;
        match minmax_scan_slice(items, op, &mut best) {
            Scan::Done(()) => Some(minmax_result(best.map(|b| b.obj), default, op)),
            Scan::Error => Some(ptr::null_mut()),
            Scan::Bail(_) => None,
        }
    }
}

// =======================================================================================
// any / all
// =======================================================================================

/// Scan for the first item whose truth is `stop` (`true` for `any`, `false` for `all`).
/// `Done(true)` if found.
///
/// # Safety
/// Live items; runs no Python code.
#[inline]
unsafe fn truth_scan(items: impl Iterator<Item = *mut ffi::PyObject>, stop: bool) -> Scan<bool> {
    for (k, item) in items.enumerate() {
        // SAFETY: forwarded.
        match unsafe { pure_truth(item) } {
            Some(t) if t == stop => return Scan::Done(true),
            Some(_) => {}
            None => return Scan::Bail(k),
        }
    }
    Scan::Done(false)
}

#[inline(always)]
fn bool_result(found: bool, stop: bool) -> *mut ffi::PyObject {
    // `any`: found a true item → True; `all`: found a false item → False.
    let value = if found { stop } else { !stop };
    // SAFETY: new reference to a singleton.
    unsafe { ffi::Py_NewRef(if value { ffi::Py_True() } else { ffi::Py_False() }) }
}

/// `any` (`stop = true`) / `all` (`stop = false`) over a builtin list/tuple.
///
/// # Safety
/// Attached thread; live sequence.
unsafe fn truth_seq(py: Python<'_>, seq: Seq, stop: bool) -> *mut ffi::PyObject {
    // SAFETY: forwarded; the scan runs no Python code.
    unsafe {
        match seq.with_items(py, |items| truth_scan(items.iter().copied(), stop)) {
            Scan::Done(found) => bool_result(found, stop),
            Scan::Error => ptr::null_mut(),
            Scan::Bail(mut i) => {
                while let Some(item) = seq.get(py, i) {
                    match ffi::PyObject_IsTrue(item.as_ptr()) {
                        r if r < 0 => return ptr::null_mut(),
                        r if (r > 0) == stop => return bool_result(true, stop),
                        _ => {}
                    }
                    i += 1;
                }
                bool_result(false, stop)
            }
        }
    }
}

/// # Safety
/// Attached thread; live container of kind `kind`.
unsafe fn truth_hashed(py: Python<'_>, kind: Hashed, obj: *mut ffi::PyObject, stop: bool) -> Out {
    // Measured against CPython's builtin: `all` over a large set walks the whole table in
    // hash order and loses from 100k elements (1.10-1.31; small sets win, 0.50-0.81), so
    // large sets use CPython. (`any` over a dict never gets here, see `rq_any`.)
    // SAFETY: a live set or frozenset (an atomic read of its size on free-threaded builds).
    let large_set = kind != Hashed::Dict && unsafe { ffi::PySet_GET_SIZE(obj) as usize } >= LARGE_SET;
    if !stop && large_set {
        return Out::Fallback;
    }
    // SAFETY: forwarded; the scan runs no Python code.
    unsafe {
        match kind.with_keys(py, obj, |keys| truth_scan(keys, stop)) {
            Scan::Done(found) => Out::Obj(bool_result(found, stop)),
            Scan::Error => Out::Obj(ptr::null_mut()),
            Scan::Bail(_) => Out::Fallback,
        }
    }
}

// =======================================================================================
// Native columns (rustique.List)
// =======================================================================================

/// How `start` enters a native sum.
#[derive(Clone, Copy)]
enum NativeStart {
    Default,
    Int(i64),
    Float(f64),
}

/// `sum(column, start)` with CPython's result; `None`: not handled natively (str data,
/// unusual starts), use the original builtin.
fn column_sum<'py>(py: Python<'py>, column: &Column, start: NativeStart) -> Option<PyResult<Bound<'py, PyAny>>> {
    let big = column.len() >= DETACH_THRESHOLD;
    if matches!(column, Column::Str(v) if !v.is_empty()) {
        return None; // CPython raises at the first `int + str`: let it.
    }
    let int_result = |x: i128| -> PyResult<Bound<'py, PyAny>> {
        match i64::try_from(x) {
            Ok(v) => Ok(v.to_python(py)),
            Err(_) => Ok(x.into_pyobject(py)?.into_any()),
        }
    };
    let is_float = matches!(column, Column::F32(_) | Column::F64(_));
    match start {
        NativeStart::Default => {
            let sum = if big { py.detach(|| kernels::sum(column)) } else { kernels::sum(column) };
            Some(match sum {
                Ok(Sum::Int(x)) => int_result(x),
                Ok(Sum::Float(x)) => Ok(x.to_python(py)),
                Err(_) => return None,
            })
        }
        NativeStart::Int(s) if !is_float => {
            let sum = if big { py.detach(|| kernels::sum(column)) } else { kernels::sum(column) };
            match sum {
                // |x| < 2^124, so adding an i64 cannot overflow i128.
                Ok(Sum::Int(x)) => Some(int_result(x + i128::from(s))),
                _ => None,
            }
        }
        NativeStart::Int(s) => {
            // Int path until the first float: `float(s) + x0`, then compensated summation.
            let run = || {
                let mut items = float_items(column);
                items.next().map(|x0| {
                    let mut cs = Cs::new(s as f64 + x0);
                    items.for_each(|x| cs.add(x));
                    cs.value()
                })
            };
            let total = if big { py.detach(run) } else { run() };
            Some(Ok(match total {
                Some(f) => f.to_python(py),
                None => s.to_python(py),
            }))
        }
        NativeStart::Float(f) => {
            // Compensated float path from the start: ints convert like `PyLong_AsDouble`.
            let run = || {
                let mut cs = Cs::new(f);
                with_column!(column, v => v.iter().for_each(|x| cs.add(element_as_f64(x))));
                cs.value()
            };
            let total = if big { py.detach(run) } else { run() };
            Some(Ok(total.to_python(py)))
        }
    }
}

/// The elements of a float column as `f64`.
fn float_items(column: &Column) -> Box<dyn Iterator<Item = f64> + '_> {
    match column {
        Column::F64(v) => Box::new(v.iter().copied()),
        Column::F32(v) => Box::new(v.iter().map(|&x| f64::from(x))),
        _ => Box::new(std::iter::empty()),
    }
}

/// A native element as Python's `float(x)` (correctly rounded for integers).
trait AsF64 {
    fn as_f64(&self) -> f64;
}
macro_rules! impl_as_f64 {
    ($($t:ty),*) => {$(
        impl AsF64 for $t {
            #[inline(always)]
            fn as_f64(&self) -> f64 {
                *self as f64
            }
        }
    )*};
}
impl_as_f64!(i8, i16, i32, i64, u8, u16, u32, u64, f32, f64);
impl AsF64 for bool {
    #[inline(always)]
    fn as_f64(&self) -> f64 {
        f64::from(u8::from(*self))
    }
}
impl AsF64 for compact_str::CompactString {
    fn as_f64(&self) -> f64 {
        f64::NAN // unreachable: str columns are rejected before summing
    }
}

#[inline(always)]
fn element_as_f64<T: AsF64>(x: &T) -> f64 {
    x.as_f64()
}

/// Truthiness of every element of a column (`any`: some true; `all`: all true).
fn column_truth(column: &Column, stop: bool) -> bool {
    macro_rules! scan {
        ($v:expr, $truth:expr) => {
            $v.iter().any(|x| $truth(x) == stop)
        };
    }
    match column {
        Column::Bool(v) => scan!(v, |x: &bool| *x),
        Column::I8(v) => scan!(v, |x: &i8| *x != 0),
        Column::I16(v) => scan!(v, |x: &i16| *x != 0),
        Column::I32(v) => scan!(v, |x: &i32| *x != 0),
        Column::I64(v) => scan!(v, |x: &i64| *x != 0),
        Column::U8(v) => scan!(v, |x: &u8| *x != 0),
        Column::U16(v) => scan!(v, |x: &u16| *x != 0),
        Column::U32(v) => scan!(v, |x: &u32| *x != 0),
        Column::U64(v) => scan!(v, |x: &u64| *x != 0),
        Column::F32(v) => scan!(v, |x: &f32| *x != 0.0),
        Column::F64(v) => scan!(v, |x: &f64| *x != 0.0),
        Column::Str(v) => scan!(v, |x: &compact_str::CompactString| !x.is_empty()),
    }
}

/// Whether a float column holds a NaN (CPython's sort order with NaNs differs from the
/// native kernel's, so such columns are sorted by `list.sort`).
fn has_nan(column: &Column) -> bool {
    match column {
        Column::F64(v) => v.iter().any(|x| x.is_nan()),
        Column::F32(v) => v.iter().any(|x| x.is_nan()),
        _ => false,
    }
}

/// A new builtin list holding the column's elements as Python objects.
fn column_to_pylist<'py>(py: Python<'py>, column: &Column) -> PyResult<Bound<'py, PyAny>> {
    let n = column.len();
    // SAFETY: a fresh list of `n` NULL slots, each filled exactly once with a new reference
    // before the list is visible to anyone.
    unsafe {
        let list = Bound::from_owned_ptr_or_err(py, ffi::PyList_New(n as ffi::Py_ssize_t))?;
        let slots = (*list.as_ptr().cast::<ffi::PyListObject>()).ob_item;
        with_column!(column, v => {
            for (i, x) in v.iter().enumerate() {
                *slots.add(i) = x.to_python(py).into_ptr();
            }
        });
        Ok(list)
    }
}

/// A new builtin list taking over owned references.
fn objects_to_pylist(py: Python<'_>, objects: Vec<Py<PyAny>>) -> PyResult<Bound<'_, PyAny>> {
    // SAFETY: as in `column_to_pylist`; `into_ptr` transfers each reference to the list.
    unsafe {
        let list = Bound::from_owned_ptr_or_err(py, ffi::PyList_New(objects.len() as ffi::Py_ssize_t))?;
        let slots = (*list.as_ptr().cast::<ffi::PyListObject>()).ob_item;
        for (i, obj) in objects.into_iter().enumerate() {
            *slots.add(i) = obj.into_ptr();
        }
        Ok(list)
    }
}

/// Raw pointers of owned objects (borrowed for the lifetime of `objects`).
fn raw(objects: &[Py<PyAny>]) -> &[*mut ffi::PyObject] {
    // SAFETY: `Py<PyAny>` is `repr(transparent)` over a non-null `PyObject*`.
    unsafe { std::slice::from_raw_parts(objects.as_ptr().cast::<*mut ffi::PyObject>(), objects.len()) }
}

// =======================================================================================
// Saved originals, keyword names, Rustique types
// =======================================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
enum Which {
    Sum = 0,
    Min = 1,
    Max = 2,
    Sorted = 3,
    Any = 4,
    All = 5,
}

const NAMES: [&CStr; 6] = [c"sum", c"min", c"max", c"sorted", c"any", c"all"];
const FASTCALL_KW: c_int = ffi::METH_FASTCALL | ffi::METH_KEYWORDS;
const FLAGS: [c_int; 6] = [FASTCALL_KW, FASTCALL_KW, FASTCALL_KW, FASTCALL_KW, ffi::METH_O, ffi::METH_O];

/// The original C entry points (`ml_meth`) of the six builtins, captured by the registry.
/// The patched `PyMethodDef`s point to the kernels, so fallbacks call these directly.
static ORIG_FASTKW: [Original<ffi::PyCFunctionFastWithKeywords>; 4] = [const { Original::new() }; 4];
static ORIG_O: [Original<ffi::PyCFunction>; 2] = [const { Original::new() }; 2];

/// Counters per builtin (`hits`: calls that entered a fast path; `deopt`: of those, the
/// ones that restarted in or continued with the original; `args`: direct fallbacks).
static C_SUM: Counters = Counters::new();
static C_MIN: Counters = Counters::new();
static C_MAX: Counters = Counters::new();
static C_SORTED: Counters = Counters::new();
static C_ANY: Counters = Counters::new();
static C_ALL: Counters = Counters::new();

#[inline(always)]
fn counters(which: Which) -> &'static Counters {
    match which {
        Which::Sum => &C_SUM,
        Which::Min => &C_MIN,
        Which::Max => &C_MAX,
        Which::Sorted => &C_SORTED,
        Which::Any => &C_ANY,
        Which::All => &C_ALL,
    }
}
/// `self` of the builtins: the `builtins` module.
static BUILTINS_MODULE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(ptr::null_mut());
/// Interned keyword names: `key`, `default`, `start`, `reverse`.
static KW: [AtomicPtr<ffi::PyObject>; 4] = [const { AtomicPtr::new(ptr::null_mut()) }; 4];
const KW_KEY: usize = 0;
const KW_DEFAULT: usize = 1;
const KW_START: usize = 2;
const KW_REVERSE: usize = 3;
/// Type objects of the Rustique containers.
static RS_LIST: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static RS_SET: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static RS_FROZENSET: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static RS_DICT: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static RS_TUPLE: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(ptr::null_mut());

/// Call the original METH_FASTCALL|METH_KEYWORDS builtin.
///
/// # Safety
/// The originals were captured (true whenever a kernel is reachable); vectorcall arguments.
#[inline(always)]
unsafe fn original_fastkw(
    which: Which,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    counters(which).fallback(Reason::Deopt);
    // SAFETY: the saved pointer is the original C function of a METH_FASTCALL|METH_KEYWORDS
    // builtin (flags checked at capture), called as CPython would.
    unsafe { ORIG_FASTKW[which as usize].get()(BUILTINS_MODULE.load(Atomic::Relaxed), args, nargs, kwnames) }
}

/// Call the original METH_O builtin.
///
/// # Safety
/// As [`original_fastkw`].
#[inline(always)]
unsafe fn original_o(which: Which, arg: *mut ffi::PyObject) -> *mut ffi::PyObject {
    counters(which).fallback(Reason::Deopt);
    // SAFETY: as above, for a METH_O builtin.
    unsafe { ORIG_O[which as usize - 4].get()(BUILTINS_MODULE.load(Atomic::Relaxed), arg) }
}

/// Direct fallback of a kernel (argument shape outside every fast path).
///
/// # Safety
/// As [`original_fastkw`].
#[inline(always)]
unsafe fn fallback_fastkw(
    which: Which,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    counters(which).fallback(Reason::Args);
    // SAFETY: per the caller.
    unsafe { ORIG_FASTKW[which as usize].get()(BUILTINS_MODULE.load(Atomic::Relaxed), args, nargs, kwnames) }
}

/// Direct fallback of a METH_O kernel.
///
/// # Safety
/// As [`original_o`].
#[inline(always)]
unsafe fn fallback_o(which: Which, arg: *mut ffi::PyObject) -> *mut ffi::PyObject {
    counters(which).fallback(Reason::Args);
    // SAFETY: per the caller.
    unsafe { ORIG_O[which as usize - 4].get()(BUILTINS_MODULE.load(Atomic::Relaxed), arg) }
}

/// Whether a keyword name equals one of the interned names.
///
/// # Safety
/// `name` is a live `str`.
#[inline(always)]
unsafe fn kw_is(name: *mut ffi::PyObject, which: usize) -> bool {
    let kw = KW[which].load(Atomic::Relaxed);
    // SAFETY: both are strings (keyword names always are), so the comparison cannot fail.
    name == kw || unsafe { ffi::PyUnicode_Compare(name, kw) } == 0
}

/// Keyword arguments of a vectorcall: `(name, value)` pairs.
///
/// # Safety
/// Vectorcall arguments.
#[inline(always)]
unsafe fn kwargs(
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> impl Iterator<Item = (*mut ffi::PyObject, *mut ffi::PyObject)> {
    // SAFETY: `kwnames` is NULL or a tuple whose length is the number of keyword values
    // following the positional arguments.
    let n = if kwnames.is_null() { 0 } else { unsafe { ffi::PyTuple_GET_SIZE(kwnames) } };
    (0..n).map(move |i| {
        // SAFETY: in bounds, see above.
        unsafe { (ffi::PyTuple_GET_ITEM(kwnames, i), *args.offset(nargs + i)) }
    })
}

/// Run a raw fast path, turning a Rust panic into `SystemError` (never unwinding into C).
#[inline(always)]
fn guarded(f: impl FnOnce() -> *mut ffi::PyObject) -> *mut ffi::PyObject {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => {
            // SAFETY: attached thread (called by CPython).
            unsafe { ffi::PyErr_SetString(ffi::PyExc_SystemError, c"rustique builtin kernel panicked".as_ptr()) };
            ptr::null_mut()
        }
    }
}

/// The `rustique.List` behind `obj`, if that is its type.
///
/// # Safety
/// `obj` is a live object.
#[inline(always)]
unsafe fn as_rs_list<'a>(py: Python<'_>, obj: *mut ffi::PyObject) -> Option<&'a List> {
    // SAFETY: exact type check; `List` is a frozen pyclass, so a shared reference to its
    // Rust value is valid for as long as the (caller-owned) object lives.
    unsafe {
        (type_of(obj) == RS_LIST.load(Atomic::Relaxed))
            .then(|| &*(Borrowed::from_ptr(py, obj).cast_unchecked::<List>().get() as *const List))
    }
}

#[inline(always)]
fn is_rustique(t: *mut ffi::PyTypeObject) -> bool {
    t == RS_LIST.load(Atomic::Relaxed)
        || t == RS_SET.load(Atomic::Relaxed)
        || t == RS_TUPLE.load(Atomic::Relaxed)
        || t == RS_DICT.load(Atomic::Relaxed)
        || t == RS_FROZENSET.load(Atomic::Relaxed)
}

/// Rustique containers whose elements are only reachable by a full snapshot, which `any`
/// (nearly always stopping at the first element) should not pay for.
#[inline(always)]
fn is_rustique_hashed(t: *mut ffi::PyTypeObject) -> bool {
    t == RS_SET.load(Atomic::Relaxed) || t == RS_DICT.load(Atomic::Relaxed) || t == RS_FROZENSET.load(Atomic::Relaxed)
}

/// The native column of the `rustique.Tuple` behind `obj`, if its storage is native.
/// Tuples are immutable, so the column is read without a lock.
///
/// # Safety
/// `obj` is a live object that outlives the returned reference.
#[inline(always)]
unsafe fn rs_tuple_column<'a>(py: Python<'_>, obj: *mut ffi::PyObject) -> Option<&'a Column> {
    // SAFETY: exact type check; `Tuple` is a frozen pyclass whose storage never changes.
    unsafe {
        if type_of(obj) != RS_TUPLE.load(Atomic::Relaxed) {
            return None;
        }
        let tuple = &*(Borrowed::from_ptr(py, obj).cast_unchecked::<Tuple>().get() as *const Tuple);
        match tuple.storage() {
            Storage::Native(column) => Some(column),
            _ => None,
        }
    }
}

/// `len()` of a Rustique container (raw slots: no Python code runs).
///
/// # Safety
/// `obj` is a live Rustique container.
#[inline(always)]
unsafe fn rs_len(obj: *mut ffi::PyObject) -> usize {
    // SAFETY: forwarded.
    unsafe { ffi::PyObject_Size(obj).max(0) as usize }
}

/// The elements of a Rustique container other than `List` (the keys, for a `Dict`), read in
/// one lock-scoped pass. Only `int`/`str` objects are created and references taken under the
/// lock, so no Python code runs while it is held.
fn rs_objects(py: Python<'_>, it: &Borrowed<'_, '_, PyAny>) -> PyResult<Vec<Py<PyAny>>> {
    if let Ok(set) = it.cast::<Set>() {
        return Ok(set.get().element_objects(py));
    }
    if let Ok(frozen) = it.cast::<FrozenSet>() {
        return Ok(frozen.get().table().key_objects(py));
    }
    if let Ok(tuple) = it.cast::<Tuple>() {
        return Ok(match tuple.get().storage() {
            Storage::Empty => Vec::new(),
            Storage::Native(column) => convert::column_objects(py, column, 0..column.len()),
            Storage::Object(items) => items.iter().map(|o| o.clone_ref(py)).collect(),
        });
    }
    let dict = it.cast::<Dict>()?;
    Ok(dict.get().with_keys(py, |keys| match keys {
        DictKeysRef::Empty => Vec::new(),
        DictKeysRef::Int(ki) => ki.iter().map(|(_, k)| k.to_python(py).unbind()).collect(),
        DictKeysRef::NativeStr(ki) => {
            ki.iter().map(|(_, k)| PyString::new(py, k.as_str()).into_any().unbind()).collect()
        }
        DictKeysRef::Object(ki) => ki.iter().map(|(_, k)| k.clone_ref(py)).collect(),
    }))
}

/// The keys of a Rustique `Dict`/`Set`/`FrozenSet` with native `int`/`str` keys, copied
/// unboxed into a column under the container's lock (no Python objects are created), so
/// the column kernels apply. `None` for any other key storage.
fn rs_key_column(py: Python<'_>, it: &Borrowed<'_, '_, PyAny>) -> Option<Column> {
    fn ints(ki: &rustique_core::index::KeyIndex<i64>) -> Column {
        Column::I64(ki.iter().map(|(_, &k)| k).collect())
    }
    fn strs(ki: &rustique_core::index::KeyIndex<rustique_core::CompactString>) -> Column {
        Column::Str(ki.iter().map(|(_, k)| k.clone()).collect())
    }
    fn of_elements(elements: SetElements<'_>) -> Option<Column> {
        match elements {
            SetElements::Int(ki) => Some(ints(ki)),
            SetElements::Str(ki) => Some(strs(ki)),
            SetElements::Empty | SetElements::Object(_) => None,
        }
    }
    if let Ok(set) = it.cast::<Set>() {
        return set.get().with_elements(py, of_elements);
    }
    if let Ok(frozen) = it.cast::<FrozenSet>() {
        return of_elements(frozen.get().elements());
    }
    let dict = it.cast::<Dict>().ok()?;
    dict.get().with_keys(py, |keys| match keys {
        DictKeysRef::Int(ki) => Some(ints(ki)),
        DictKeysRef::NativeStr(ki) => Some(strs(ki)),
        DictKeysRef::Empty | DictKeysRef::Object(_) => None,
    })
}

/// Sort a column copied out of a container and return it as a builtin list, parsing
/// `reverse` (whose truthiness may run Python code) after the copy, like `list.sort`.
///
/// # Safety
/// `reverse` is NULL or a live object.
unsafe fn sorted_column(
    py: Python<'_>,
    mut column: Column,
    reverse: *mut ffi::PyObject,
) -> PyResult<*mut ffi::PyObject> {
    let reverse = !reverse.is_null() && {
        // SAFETY: a live object, per the caller.
        let r = unsafe { ffi::PyObject_IsTrue(reverse) };
        if r < 0 {
            return Err(PyErr::fetch(py));
        }
        r == 1
    };
    if column.len() >= SORT_DETACH_THRESHOLD {
        py.detach(|| kernels::sort(&mut column, reverse));
    } else {
        kernels::sort(&mut column, reverse);
    }
    column_to_pylist(py, &column).map(Bound::into_ptr)
}

// =======================================================================================
// sum
// =======================================================================================

/// Whether a fast path may apply to an iterable of this type (anything else goes straight
/// to the original, from a thin entry point that adds only these compares).
#[inline(always)]
fn candidate(t: *mut ffi::PyTypeObject) -> bool {
    t == list_type()
        || t == tuple_type()
        || t == &raw mut ffi::PySet_Type
        || t == &raw mut ffi::PyFrozenSet_Type
        || t == &raw mut ffi::PyDict_Type
        || is_rustique(t)
}

/// `sum(iterable, /, start=0)`.
unsafe extern "C" fn rq_sum(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: called by CPython with vectorcall arguments on an attached thread.
    unsafe {
        if nargs >= 1 && candidate(type_of(*args)) {
            C_SUM.hit();
            sum_entry(slf, args, nargs, kwnames)
        } else {
            fallback_fastkw(Which::Sum, args, nargs, kwnames)
        }
    }
}

/// # Safety
/// Vectorcall arguments from CPython.
#[inline(never)]
unsafe fn sum_entry(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: called by CPython with vectorcall arguments on an attached thread.
    unsafe {
        let start = match (nargs, kwnames.is_null()) {
            (1, true) => ptr::null_mut(),
            (2, true) => *args.add(1),
            (1, false) if ffi::PyTuple_GET_SIZE(kwnames) == 1 && kw_is(ffi::PyTuple_GET_ITEM(kwnames, 0), KW_START) => {
                *args.add(1)
            }
            _ => return original_fastkw(Which::Sum, args, nargs, kwnames),
        };
        if !start.is_null() {
            let t = type_of(start);
            if t != long_type() && t != float_type() {
                return original_fastkw(Which::Sum, args, nargs, kwnames);
            }
        }
        let it = *args;
        let t = type_of(it);
        let py = Python::assume_attached();
        let out = if t == list_type() {
            Out::Obj(guarded(|| match sum_seq(py, Seq::List(it), start) {
                Out::Obj(r) => r,
                Out::Fallback => original_fastkw(Which::Sum, args, nargs, kwnames),
            }))
        } else if t == tuple_type() {
            Out::Obj(guarded(|| match sum_seq(py, Seq::Tuple(it), start) {
                Out::Obj(r) => r,
                Out::Fallback => original_fastkw(Which::Sum, args, nargs, kwnames),
            }))
        } else if is_rustique(t) {
            // Native storage: the column kernel, without PyO3's trampoline (nothing here
            // drops a `Py<T>`); everything else inside it.
            let native = as_rs_list(py, it).and_then(|list| {
                let ns = if start.is_null() {
                    NativeStart::Default
                } else if type_of(start) == float_type() {
                    NativeStart::Float(float_value(start))
                } else {
                    NativeStart::Int(int_value(start)?)
                };
                list.with_column(py, |column| column_sum(py, column, ns))?
            });
            match native {
                Some(Ok(result)) => Out::Obj(result.into_ptr()),
                Some(Err(e)) => {
                    e.restore(py);
                    Out::Obj(ptr::null_mut())
                }
                None => Out::Obj(trampoline::fastcall_cfunction_with_keywords::<RsSum>(slf, args, nargs, kwnames)),
            }
        } else if let Some(kind) = Hashed::of(t) {
            match guarded(|| match sum_hashed(py, kind, it, start) {
                Out::Obj(r) => r,
                Out::Fallback => FALLBACK_MARK,
            }) {
                r if r == FALLBACK_MARK => Out::Fallback,
                r => Out::Obj(r),
            }
        } else {
            Out::Fallback
        };
        match out {
            Out::Obj(r) => r,
            Out::Fallback => original_fastkw(Which::Sum, args, nargs, kwnames),
        }
    }
}

/// A non-null marker returned through [`guarded`] meaning "call the original" (never a
/// real object pointer: nothing lives at the lowest page).
const FALLBACK_MARK: *mut ffi::PyObject = ptr::dangling_mut();

struct RsSum;
impl MethodDef<trampoline::fastcall_cfunction_with_keywords::Func> for RsSum {
    const METH: trampoline::fastcall_cfunction_with_keywords::Func = rs_sum;
}

/// `sum` over a `rustique.List`/`Set` (inside PyO3's trampoline: attached, panic-safe).
unsafe fn rs_sum(
    py: Python<'_>,
    _slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> PyResult<*mut ffi::PyObject> {
    // SAFETY: vectorcall arguments already validated by `rq_sum` (one iterable, optional
    // exact int/float start).
    unsafe {
        let it = Borrowed::from_ptr(py, *args);
        let start = if nargs == 2 || !kwnames.is_null() { *args.add(1) } else { ptr::null_mut() };
        let native_start = if start.is_null() {
            Some(NativeStart::Default)
        } else if type_of(start) == float_type() {
            Some(NativeStart::Float(float_value(start)))
        } else {
            int_value(start).map(NativeStart::Int)
        };
        let fallback = || Ok(original_fastkw(Which::Sum, args, nargs, kwnames));
        let objects = if let Ok(list) = it.cast::<List>() {
            let list = list.get();
            if let Some(ns) = native_start
                && let Some(done) = list.with_column(py, |column| column_sum(py, column, ns))
            {
                return match done {
                    Some(result) => result.map(Bound::into_ptr),
                    None => fallback(),
                };
            }
            match list.snapshot(py) {
                Storage::Native(column) => {
                    return match native_start.and_then(|ns| column_sum(py, &column, ns)) {
                        Some(result) => result.map(Bound::into_ptr),
                        None => fallback(),
                    };
                }
                storage => storage.into_objects(py),
            }
        } else if let Some(column) = rs_tuple_column(py, it.as_ptr()) {
            return match native_start.and_then(|ns| column_sum(py, column, ns)) {
                Some(result) => result.map(Bound::into_ptr),
                None => fallback(),
            };
        } else if let Some(column) = rs_key_column(py, &it) {
            return match native_start.and_then(|ns| column_sum(py, &column, ns)) {
                Some(result) => result.map(Bound::into_ptr),
                None => fallback(),
            };
        } else {
            rs_objects(py, &it)?
        };
        match sum_items(py, raw(&objects), start) {
            Some(r) if r.is_null() => Err(PyErr::fetch(py)),
            Some(r) => Ok(r),
            None => fallback(),
        }
    }
}

// =======================================================================================
// min / max
// =======================================================================================

unsafe extern "C" fn rq_min(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: vectorcall arguments from CPython.
    unsafe {
        if nargs > 1 || (nargs == 1 && candidate(type_of(*args))) {
            C_MIN.hit();
            minmax_entry(slf, args, nargs, kwnames, Which::Min)
        } else {
            fallback_fastkw(Which::Min, args, nargs, kwnames)
        }
    }
}

unsafe extern "C" fn rq_max(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: vectorcall arguments from CPython.
    unsafe {
        if nargs > 1 || (nargs == 1 && candidate(type_of(*args))) {
            C_MAX.hit();
            minmax_entry(slf, args, nargs, kwnames, Which::Max)
        } else {
            fallback_fastkw(Which::Max, args, nargs, kwnames)
        }
    }
}

/// The `default` argument of a `min`/`max` call that a fast path can serve (no key
/// function): `Some(default or NULL)`; `None` to call the original.
///
/// # Safety
/// Vectorcall arguments.
#[inline(always)]
unsafe fn minmax_args(
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> Option<*mut ffi::PyObject> {
    if nargs == 0 {
        return None;
    }
    let mut default = ptr::null_mut();
    // SAFETY: forwarded.
    for (name, value) in unsafe { kwargs(args, nargs, kwnames) } {
        // SAFETY: keyword names are strings.
        let is = |which| unsafe { kw_is(name, which) };
        if is(KW_KEY) {
            // SAFETY: comparing with the None singleton.
            if value != unsafe { ffi::Py_None() } {
                return None;
            }
        } else if is(KW_DEFAULT) {
            default = value;
        } else {
            return None;
        }
    }
    // A default with several positional arguments is an error: the original reports it.
    (nargs == 1 || default.is_null()).then_some(default)
}

/// # Safety
/// Vectorcall arguments from CPython.
#[inline(never)]
unsafe fn minmax_entry(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
    which: Which,
) -> *mut ffi::PyObject {
    let op = if which == Which::Min { ffi::Py_LT } else { ffi::Py_GT };
    // SAFETY: vectorcall arguments; the kernels' requirements hold for live arguments.
    unsafe {
        let Some(default) = minmax_args(args, nargs, kwnames) else {
            return original_fastkw(which, args, nargs, kwnames);
        };
        if nargs == 2
            && let Some(r) = minmax_pair(*args, *args.add(1), which == Which::Max)
        {
            return ffi::Py_NewRef(r);
        }
        let py = Python::assume_attached();
        if nargs > 1 {
            return guarded(|| minmax_seq(py, Seq::Args(args, nargs as usize), default, op));
        }
        let it = *args;
        let t = type_of(it);
        if t == list_type() {
            guarded(|| minmax_seq(py, Seq::List(it), default, op))
        } else if t == tuple_type() {
            guarded(|| minmax_seq(py, Seq::Tuple(it), default, op))
        } else if is_rustique(t) {
            // Native storage: the column kernel, without PyO3's trampoline.
            let native = as_rs_list(py, it).and_then(|list| {
                list.with_column(py, |column| {
                    let run = || if op == ffi::Py_GT { kernels::max(column) } else { kernels::min(column) };
                    if column.len() >= DETACH_THRESHOLD { py.detach(run) } else { run() }
                })
            });
            match native {
                Some(Some(value)) => convert::value_to_python(py, &value).into_ptr(),
                Some(None) => minmax_result(None, default, op),
                None if which == Which::Min => {
                    trampoline::fastcall_cfunction_with_keywords::<RsMin>(slf, args, nargs, kwnames)
                }
                None => trampoline::fastcall_cfunction_with_keywords::<RsMax>(slf, args, nargs, kwnames),
            }
        } else if let Some(kind) = Hashed::of(t) {
            match guarded(|| match minmax_hashed(py, kind, it, default, op) {
                Out::Obj(r) => r,
                Out::Fallback => FALLBACK_MARK,
            }) {
                r if r == FALLBACK_MARK => original_fastkw(which, args, nargs, kwnames),
                r => r,
            }
        } else {
            original_fastkw(which, args, nargs, kwnames)
        }
    }
}

struct RsMin;
impl MethodDef<trampoline::fastcall_cfunction_with_keywords::Func> for RsMin {
    const METH: trampoline::fastcall_cfunction_with_keywords::Func = rs_min;
}
struct RsMax;
impl MethodDef<trampoline::fastcall_cfunction_with_keywords::Func> for RsMax {
    const METH: trampoline::fastcall_cfunction_with_keywords::Func = rs_max;
}

unsafe fn rs_min(
    py: Python<'_>,
    _slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> PyResult<*mut ffi::PyObject> {
    // SAFETY: forwarded.
    unsafe { rs_minmax(py, args, nargs, kwnames, Which::Min) }
}

unsafe fn rs_max(
    py: Python<'_>,
    _slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> PyResult<*mut ffi::PyObject> {
    // SAFETY: forwarded.
    unsafe { rs_minmax(py, args, nargs, kwnames, Which::Max) }
}

/// `min`/`max` of a `rustique.List`/`Set` (one positional argument, no key function).
///
/// # Safety
/// Vectorcall arguments validated by [`minmax_entry`].
unsafe fn rs_minmax(
    py: Python<'_>,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
    which: Which,
) -> PyResult<*mut ffi::PyObject> {
    let op = if which == Which::Min { ffi::Py_LT } else { ffi::Py_GT };
    let max = which == Which::Max;
    // SAFETY: validated arguments; kernels on live objects.
    unsafe {
        let default = minmax_args(args, nargs, kwnames).unwrap_or(ptr::null_mut());
        let it = Borrowed::from_ptr(py, *args);
        let native = |column: &Column| -> Option<rustique_core::Value> {
            let run = || if max { kernels::max(column) } else { kernels::min(column) };
            if column.len() >= DETACH_THRESHOLD { py.detach(run) } else { run() }
        };
        let objects = if let Ok(list) = it.cast::<List>() {
            let list = list.get();
            let found = match list.with_column(py, |column| native(column)) {
                Some(found) => Some(found),
                None => match list.snapshot(py) {
                    Storage::Native(column) => Some(native(&column)),
                    storage => {
                        let objects = storage.into_objects(py);
                        if objects.is_empty() {
                            Some(None)
                        } else {
                            return finish_items(py, &objects, default, op, args, nargs, kwnames, which);
                        }
                    }
                },
            };
            return match found {
                Some(Some(value)) => Ok(convert::value_to_python(py, &value).into_ptr()),
                // Empty.
                _ => {
                    let r = minmax_result(None, default, op);
                    if r.is_null() { Err(PyErr::fetch(py)) } else { Ok(r) }
                }
            };
        } else if let Some(column) =
            rs_tuple_column(py, it.as_ptr()).map(Cow::Borrowed).or_else(|| rs_key_column(py, &it).map(Cow::Owned))
        {
            return match native(&column) {
                Some(value) => Ok(convert::value_to_python(py, &value).into_ptr()),
                None => {
                    let r = minmax_result(None, default, op);
                    if r.is_null() { Err(PyErr::fetch(py)) } else { Ok(r) }
                }
            };
        } else {
            rs_objects(py, &it)?
        };
        finish_items(py, &objects, default, op, args, nargs, kwnames, which)
    }
}

/// `min`/`max` over a snapshot of objects, or the original builtin if one needs Python code.
///
/// # Safety
/// Vectorcall arguments; live default.
#[allow(clippy::too_many_arguments)]
unsafe fn finish_items(
    py: Python<'_>,
    objects: &[Py<PyAny>],
    default: *mut ffi::PyObject,
    op: c_int,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
    which: Which,
) -> PyResult<*mut ffi::PyObject> {
    // SAFETY: the snapshot keeps the items alive.
    unsafe {
        match minmax_items(raw(objects), default, op) {
            Some(r) if r.is_null() => Err(PyErr::fetch(py)),
            Some(r) => Ok(r),
            None => Ok(original_fastkw(which, args, nargs, kwnames)),
        }
    }
}

// =======================================================================================
// sorted
// =======================================================================================

unsafe extern "C" fn rq_sorted(
    slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: vectorcall arguments from CPython.
    unsafe {
        if nargs == 1 && is_rustique(type_of(*args)) {
            C_SORTED.hit();
            return trampoline::fastcall_cfunction_with_keywords::<RsSorted>(slf, args, nargs, kwnames);
        }
        // Builtin inputs: `sorted` copies and calls `list.sort` (itself accelerated by the
        // `containers` group when enabled).
        fallback_fastkw(Which::Sorted, args, nargs, kwnames)
    }
}

struct RsSorted;
impl MethodDef<trampoline::fastcall_cfunction_with_keywords::Func> for RsSorted {
    const METH: trampoline::fastcall_cfunction_with_keywords::Func = rs_sorted;
}

/// `sorted(rustique.List / Set, *, key=None, reverse=False)`.
unsafe fn rs_sorted(
    py: Python<'_>,
    _slf: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> PyResult<*mut ffi::PyObject> {
    // SAFETY: vectorcall arguments (one positional, validated by `rq_sorted`).
    unsafe {
        let it = Borrowed::from_ptr(py, *args);
        // Keyword arguments as `list.sort` will parse them; anything else goes to it as is.
        let (mut key_none, mut reverse, mut plain) = (true, ptr::null_mut(), true);
        for (name, value) in kwargs(args, nargs, kwnames) {
            if kw_is(name, KW_KEY) {
                key_none = value == ffi::Py_None();
            } else if kw_is(name, KW_REVERSE) {
                reverse = value;
            } else {
                plain = false;
            }
        }
        let list = it.cast::<List>().ok();
        if plain
            && key_none
            && let Some(list) = &list
            // Copy the column under the read lock (like `list(x)`); NaNs: CPython's order.
            && let Some(Some(mut column)) = list.get().with_column(py, |c| (!has_nan(c)).then(|| c.clone()))
        {
            // `list.sort` parses `reverse` (truthiness may run Python code) after the copy.
            let reverse = !reverse.is_null() && {
                let r = ffi::PyObject_IsTrue(reverse);
                if r < 0 {
                    return Err(PyErr::fetch(py));
                }
                r == 1
            };
            if column.len() >= SORT_DETACH_THRESHOLD {
                py.detach(|| kernels::sort(&mut column, reverse));
            } else {
                kernels::sort(&mut column, reverse);
            }
            return column_to_pylist(py, &column).map(Bound::into_ptr);
        }
        // Native numeric keys of a Dict/Set/FrozenSet, or a native numeric Tuple: sorted
        // natively (no NaNs in key columns; a Tuple column never holds one). Larger `str`
        // columns take the object path below: boxing every string afresh costs more than
        // sorting the existing objects with `list.sort` (measured, sorted ratio vs builtin:
        // native 1.44 / 1.35, objects 1.78 / 1.12 at 8 / 100 elements).
        if plain
            && key_none
            && list.is_none()
            && let Some(column) = rs_tuple_column(py, it.as_ptr()).cloned().or_else(|| rs_key_column(py, &it))
            && (!matches!(column, Column::Str(_)) || column.len() < NATIVE_STR_SORT_MAX)
        {
            return sorted_column(py, column, reverse);
        }
        // `list(x)` then `list.sort(**kwargs)`, like CPython's `sorted`, with the copy made
        // in one read of the container instead of an iteration.
        let objects = match list {
            Some(list) => list.get().snapshot(py).into_objects(py),
            None => rs_objects(py, &it)?,
        };
        let newlist = objects_to_pylist(py, objects)?;
        let descriptor = objsort::list_sort_descriptor(py)?;
        let nkw = if kwnames.is_null() { 0 } else { ffi::PyTuple_GET_SIZE(kwnames) as usize };
        let mut call_args = Vec::with_capacity(1 + nkw);
        call_args.push(newlist.as_ptr());
        call_args.extend((0..nkw).map(|i| *args.add(nargs as usize + i)));
        let r = ffi::PyObject_Vectorcall(descriptor.as_ptr(), call_args.as_ptr(), 1, kwnames);
        if r.is_null() {
            return Err(PyErr::fetch(py));
        }
        ffi::Py_DECREF(r);
        Ok(newlist.into_ptr())
    }
}

// =======================================================================================
// any / all
// =======================================================================================

unsafe extern "C" fn rq_any(slf: *mut ffi::PyObject, arg: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: METH_O call from CPython.
    // `any` over a dict or a Rustique Set/Dict/FrozenSet nearly always stops at its first
    // key: nothing to win, and any extra step before CPython's loop shows (measured: an
    // in-kernel fallback cost 1.07-1.12 on `any(dict)`). Those go straight to CPython.
    unsafe {
        let t = type_of(arg);
        if candidate(t) && !is_rustique_hashed(t) && t != &raw mut ffi::PyDict_Type {
            C_ANY.hit();
            truth_entry(slf, arg, Which::Any)
        } else {
            fallback_o(Which::Any, arg)
        }
    }
}

unsafe extern "C" fn rq_all(slf: *mut ffi::PyObject, arg: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: METH_O call from CPython.
    unsafe {
        if candidate(type_of(arg)) {
            C_ALL.hit();
            truth_entry(slf, arg, Which::All)
        } else {
            fallback_o(Which::All, arg)
        }
    }
}

/// # Safety
/// METH_O arguments from CPython.
#[inline(never)]
unsafe fn truth_entry(slf: *mut ffi::PyObject, it: *mut ffi::PyObject, which: Which) -> *mut ffi::PyObject {
    let stop = which == Which::Any;
    // SAFETY: live argument; kernels' requirements hold.
    unsafe {
        let t = type_of(it);
        let py = Python::assume_attached();
        if t == list_type() {
            guarded(|| truth_seq(py, Seq::List(it), stop))
        } else if t == tuple_type() {
            guarded(|| truth_seq(py, Seq::Tuple(it), stop))
        } else if is_rustique(t) {
            match as_rs_list(py, it) {
                Some(list) => {
                    // Native storage: the column scan, without PyO3's trampoline.
                    if let Some(found) = list.with_column(py, |c| column_truth(c, stop)) {
                        return bool_result(found, stop);
                    }
                    // `any`/`all` usually stop early, but a snapshot costs every element.
                    if list.len() > SNAPSHOT_TRUTH_MAX {
                        return original_o(which, it);
                    }
                }
                // Another Rustique container (`any` reaches here only for a Tuple): a native
                // Tuple column is scanned in place, anything else snapshotted only when small.
                None => {
                    if let Some(column) = rs_tuple_column(py, it) {
                        return bool_result(column_truth(column, stop), stop);
                    }
                    // Hashed containers may have native keys (copied unboxed in `rs_truth`).
                    if rs_len(it) > SNAPSHOT_TRUTH_MAX && !is_rustique_hashed(t) {
                        return original_o(which, it);
                    }
                }
            }
            if stop { trampoline::binaryfunc::<RsAny>(slf, it) } else { trampoline::binaryfunc::<RsAll>(slf, it) }
        } else if let Some(kind) = Hashed::of(t) {
            match guarded(|| match truth_hashed(py, kind, it, stop) {
                Out::Obj(r) => r,
                Out::Fallback => FALLBACK_MARK,
            }) {
                r if r == FALLBACK_MARK => original_o(which, it),
                r => r,
            }
        } else {
            original_o(which, it)
        }
    }
}

struct RsAny;
impl MethodDef<trampoline::binaryfunc::Func> for RsAny {
    const METH: trampoline::binaryfunc::Func = rs_any;
}
struct RsAll;
impl MethodDef<trampoline::binaryfunc::Func> for RsAll {
    const METH: trampoline::binaryfunc::Func = rs_all;
}

unsafe fn rs_any(py: Python<'_>, _slf: *mut ffi::PyObject, it: *mut ffi::PyObject) -> PyResult<*mut ffi::PyObject> {
    // SAFETY: forwarded.
    unsafe { rs_truth(py, it, Which::Any) }
}

unsafe fn rs_all(py: Python<'_>, _slf: *mut ffi::PyObject, it: *mut ffi::PyObject) -> PyResult<*mut ffi::PyObject> {
    // SAFETY: forwarded.
    unsafe { rs_truth(py, it, Which::All) }
}

/// `any`/`all` of a `rustique.List`/`Set`.
///
/// # Safety
/// `it` is a live `rustique.List` or `Set`.
unsafe fn rs_truth(py: Python<'_>, it: *mut ffi::PyObject, which: Which) -> PyResult<*mut ffi::PyObject> {
    let stop = which == Which::Any;
    // SAFETY: live object; kernels on owned snapshots.
    unsafe {
        let obj = Borrowed::from_ptr(py, it);
        // `any`/`all` usually stop early, but a snapshot costs every element: above
        // `SNAPSHOT_TRUTH_MAX` elements, iterate lazily with the original instead.
        let objects = if let Ok(list) = obj.cast::<List>() {
            let list = list.get();
            if let Some(found) = list.with_column(py, |c| column_truth(c, stop)) {
                return Ok(bool_result(found, stop));
            }
            if list.len() > SNAPSHOT_TRUTH_MAX {
                return Ok(original_o(which, it));
            }
            match list.snapshot(py) {
                Storage::Native(column) => return Ok(bool_result(column_truth(&column, stop), stop)),
                storage => storage.into_objects(py),
            }
        } else {
            if let Some(column) = rs_tuple_column(py, it) {
                return Ok(bool_result(column_truth(column, stop), stop));
            }
            if let Some(column) = rs_key_column(py, &obj) {
                return Ok(bool_result(column_truth(&column, stop), stop));
            }
            if rs_len(it) > SNAPSHOT_TRUTH_MAX {
                return Ok(original_o(which, it));
            }
            rs_objects(py, &obj)?
        };
        match truth_scan(raw(&objects).iter().copied(), stop) {
            Scan::Done(found) => Ok(bool_result(found, stop)),
            Scan::Error => Err(PyErr::fetch(py)),
            Scan::Bail(_) => Ok(original_o(which, it)),
        }
    }
}

// =======================================================================================
// Installation: patching the builtins' method table in place
// =======================================================================================
//
// `install` swaps the `ml_meth` pointer of the six entries of CPython's static `builtins`
// method table (`PyModule_GetDef(builtins)->m_methods`, non-const static data) for the
// kernels, and `uninstall` swaps them back. The builtin function objects keep their
// identity: CPython 3.14+ compiles `any(<genexpr>)` / `all(<genexpr>)` into an *identity*
// check against the original `any`/`all` followed by an inlined loop, which a different
// object in `builtins` would defeat (measured: `any(genexpr)` 1.19x the instructions);
// references taken before `install()` (`from builtins import sum`, C extensions) are
// accelerated too and keep pickling. Every caller reads `ml_meth` at call time (the
// `CALL_BUILTIN_*` specialisations, `cfunction_vectorcall_*`), so the swap takes effect
// everywhere at once; the store is one aligned pointer write, so a concurrent call sees
// either function.

/// The kernels, per [`Which`].
fn kernel(which: Which) -> usize {
    match which {
        Which::Sum => rq_sum as ffi::PyCFunctionFastWithKeywords as usize,
        Which::Min => rq_min as ffi::PyCFunctionFastWithKeywords as usize,
        Which::Max => rq_max as ffi::PyCFunctionFastWithKeywords as usize,
        Which::Sorted => rq_sorted as ffi::PyCFunctionFastWithKeywords as usize,
        Which::Any => rq_any as ffi::PyCFunction as usize,
        Which::All => rq_all as ffi::PyCFunction as usize,
    }
}

const ALL: [Which; 6] = [Which::Sum, Which::Min, Which::Max, Which::Sorted, Which::Any, Which::All];

/// The builtins' method table entries, captured once: `None` per entry not recognised.
struct Captured {
    defs: [Option<usize>; 6],
    /// Standalone function objects running the kernels (see [`make_replacement`]).
    replacements: [Option<Py<PyAny>>; 6],
}

static STATE: Mutex<Option<Captured>> = Mutex::new(None);

/// Verify the object layouts the kernels read directly.
fn check_layouts(py: Python<'_>) -> PyResult<()> {
    let bad = |what: &str| PyRuntimeError::new_err(format!("rustique builtins: unexpected {what} layout"));
    for v in [0i64, 1, -1, 7, -12345, (1 << 30) - 1, -((1 << 30) - 1), 1 << 30, i64::MAX, i64::MIN] {
        let obj = v.into_pyobject(py)?;
        // SAFETY: an exact int.
        if unsafe { int_value(obj.as_ptr()) } != Some(v) {
            return Err(bad("int"));
        }
        // SAFETY: an exact int.
        if unsafe { pure_truth(obj.as_ptr()) } != Some(v != 0) {
            return Err(bad("int"));
        }
    }
    let big = (1u128 << 64).into_pyobject(py)?;
    // SAFETY: an exact int.
    if unsafe { int_value(big.as_ptr()) }.is_some() {
        return Err(bad("int"));
    }
    let set = pyo3::types::PySet::empty(py)?;
    for v in [1, 2, 3] {
        set.add(v)?;
    }
    set.add("x")?;
    set.discard(2)?;
    // SAFETY: an exact set, not shared with other threads.
    let mut keys: Vec<String> = unsafe { set_keys(set.as_ptr()) }
        .map(|k| {
            // SAFETY: live keys of the set.
            unsafe { Bound::from_borrowed_ptr(py, k) }.repr().map(|r| r.to_string())
        })
        .collect::<PyResult<_>>()?;
    keys.sort();
    if keys != ["'x'", "1", "3"] {
        return Err(bad("set"));
    }
    Ok(())
}

/// Find the six entries in the `builtins` method table (once per process).
fn capture<'a>(py: Python<'_>, state: &'a mut Option<Captured>) -> PyResult<&'a mut Captured> {
    if let Some(captured) = state {
        return Ok(captured);
    }
    check_layouts(py)?;
    init_statics(py)?;
    let builtins = py.import("builtins")?;
    let mut defs = [None; 6];
    // SAFETY: `PyModule_GetDef` of the builtins module returns its static definition; the
    // method table is NULL-terminated (`ml_name == NULL`).
    unsafe {
        let module_def = ffi::PyModule_GetDef(builtins.as_ptr());
        let mut def = if module_def.is_null() { ptr::null_mut() } else { (*module_def).m_methods };
        while !def.is_null() && !(*def).ml_name.is_null() {
            let name = CStr::from_ptr((*def).ml_name);
            if let Some(which) = ALL.into_iter().find(|&w| NAMES[w as usize] == name)
                && (*def).ml_flags == FLAGS[which as usize]
            {
                defs[which as usize] = Some(def as usize);
            }
            def = def.add(1);
        }
    }
    BUILTINS_MODULE.store(builtins.clone().into_ptr(), Atomic::Release); // kept forever
    Ok(state.insert(Captured { defs, replacements: Default::default() }))
}

/// A standalone builtin function running the kernel of `which` (same name, docstring,
/// flags and `__self__` as the original), for tests and benchmarks that compare both
/// without patching anything.
fn make_replacement(py: Python<'_>, which: Which, def: usize, builtins: &Bound<'_, PyModule>) -> PyResult<Py<PyAny>> {
    let original = def as *mut ffi::PyMethodDef;
    // SAFETY: a valid method table entry; the new function object keeps a pointer to its
    // own `PyMethodDef`, which therefore gets static storage.
    unsafe {
        let new_def: &'static mut ffi::PyMethodDef = Box::leak(Box::new(ffi::PyMethodDef {
            ml_name: (*original).ml_name,
            ml_meth: std::mem::transmute::<usize, ffi::PyMethodDefPointer>(kernel(which)),
            ml_flags: (*original).ml_flags,
            ml_doc: (*original).ml_doc,
        }));
        let module_name = PyString::new(py, "builtins");
        let func = ffi::PyCFunction_NewEx(new_def, builtins.as_ptr(), module_name.as_ptr());
        Ok(Bound::from_owned_ptr_or_err(py, func)?.unbind())
    }
}

/// One-time initialisation of the statics the kernels read.
fn init_statics(py: Python<'_>) -> PyResult<()> {
    for (slot, name) in KW.iter().zip(["key", "default", "start", "reverse"]) {
        if slot.load(Atomic::Relaxed).is_null() {
            // Interned strings live as long as the interpreter; keep one reference forever.
            slot.store(PyString::intern(py, name).into_ptr(), Atomic::Relaxed);
        }
    }
    Ok(())
}

/// The Rustique container types exist now (created lazily on first access to the opt-in
/// API, `crate::ensure_core`): let the kernels recognise them. Until then no Rustique
/// container can exist, so the null pointers the kernels compare against are correct.
pub fn core_types_ready(py: Python<'_>) -> PyResult<()> {
    objsort::list_sort_descriptor(py)?;
    RS_LIST.store(py.get_type::<List>().as_type_ptr(), Atomic::Relaxed);
    RS_SET.store(py.get_type::<Set>().as_type_ptr(), Atomic::Relaxed);
    RS_FROZENSET.store(py.get_type::<FrozenSet>().as_type_ptr(), Atomic::Relaxed);
    RS_DICT.store(py.get_type::<Dict>().as_type_ptr(), Atomic::Relaxed);
    RS_TUPLE.store(py.get_type::<Tuple>().as_type_ptr(), Atomic::Relaxed);
    Ok(())
}

fn lock_state() -> PyResult<std::sync::MutexGuard<'static, Option<Captured>>> {
    STATE.lock().map_err(|_| PyRuntimeError::new_err("rustique builtins: state poisoned"))
}

// ---------------------------------------------------------------------------------------
// Targets (docs/ACCELERATION.md §9): group `builtins`.
// ---------------------------------------------------------------------------------------

/// Fingerprints of the six C originals (`ml_name`, `ml_flags`, `ml_doc`), in [`Which`]
/// order, recorded with `python -m rustique._takeover._dev fingerprint builtins.sum ...`
/// (identical on 3.14.9, 3.14.9t, 3.15.1 and 3.15.1t).
const FINGERPRINTS: [accel::Fingerprints; 6] = [
    &[(3, 14, "6361e6d2d1b2c0b5"), (3, 15, "6361e6d2d1b2c0b5")],
    &[(3, 14, "cb287d8f2c8a83bd"), (3, 15, "cb287d8f2c8a83bd")],
    &[(3, 14, "c98ff6f6ef98751c"), (3, 15, "c98ff6f6ef98751c")],
    &[(3, 14, "d2839c81e5ecc243"), (3, 15, "d2839c81e5ecc243")],
    &[(3, 14, "52c0a11cbdfca869"), (3, 15, "52c0a11cbdfca869")],
    &[(3, 14, "7839de65dbc5a549"), (3, 15, "7839de65dbc5a549")],
];

fn prepare(py: Python<'_>, cx: &mut Prepare<'_>, which: Which) -> Result<(), Refusal> {
    let def = {
        let mut state = lock_state()?;
        capture(py, &mut state).map_err(|e| Refusal::from(e).or_category(accel::Category::Layout))?.defs[which as usize]
    };
    let name = NAMES[which as usize].to_str().unwrap_or_default();
    let def = def.ok_or_else(|| refuse!(Conflict: "builtins.{name}: method table entry not found"))?;
    let what = format!("builtins.{name}");
    let def = def as *mut ffi::PyMethodDef;
    // SAFETY: a live entry of the static `builtins` method table.
    unsafe {
        cx.fingerprint_def(&what, def, FINGERPRINTS[which as usize])?;
        match which {
            Which::Any | Which::All => {
                let k = kernel(which);
                let k = std::mem::transmute::<usize, ffi::PyCFunction>(k);
                cx.ml_meth_def(&what, def, k, &ORIG_O[which as usize - 4])
            }
            _ => {
                let k = std::mem::transmute::<usize, ffi::PyCFunctionFastWithKeywords>(kernel(which));
                cx.ml_meth_def(&what, def, k, &ORIG_FASTKW[which as usize])
            }
        }
    }
}

fn prepare_sum(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare(py, cx, Which::Sum)
}
fn prepare_min(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare(py, cx, Which::Min)
}
fn prepare_max(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare(py, cx, Which::Max)
}
fn prepare_sorted(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare(py, cx, Which::Sorted)
}
fn prepare_any(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare(py, cx, Which::Any)
}
fn prepare_all(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare(py, cx, Which::All)
}

macro_rules! target {
    ($id:ident, $name:literal, $counters:ident, $prepare:ident) => {
        pub static $id: TargetSpec = TargetSpec {
            name: $name,
            group: "builtins",
            module: None,
            versions: &[(3, 14), (3, 15)],
            free_threaded: true,
            counters: &$counters,
            prepare: $prepare,
            ..TargetSpec::DEFAULT
        };
    };
}

target!(SUM, "builtins.sum", C_SUM, prepare_sum);
target!(MIN, "builtins.min", C_MIN, prepare_min);
target!(MAX, "builtins.max", C_MAX, prepare_max);
target!(SORTED, "builtins.sorted", C_SORTED, prepare_sorted);
target!(ANY, "builtins.any", C_ANY, prepare_any);
target!(ALL_, "builtins.all", C_ALL, prepare_all);

/// The `builtins` group.
pub static TARGETS: &[&TargetSpec] = &[&SUM, &MIN, &MAX, &SORTED, &ANY, &ALL_];

fn group() -> Vec<String> {
    vec!["builtins".to_owned()]
}

fn short(names: Vec<&'static str>) -> Vec<&'static str> {
    // Keep `REPLACED` order.
    ALL.into_iter()
        .map(|w| NAMES[w as usize].to_str().expect("ASCII names"))
        .filter(|n| names.iter().any(|m| m.strip_prefix("builtins.") == Some(*n)))
        .collect()
}

/// Accelerate `sum`, `min`, `max`, `sorted`, `any` and `all` in place (the `builtins` group;
/// `rustique.enable()` is the API). Idempotent; returns the names now accelerated.
#[pyfunction]
fn install(py: Python<'_>) -> PyResult<Vec<&'static str>> {
    accel::registry::enable(py, Some(group()), Vec::new())?;
    Ok(short(accel::registry::installed(py, Some(&group()))?))
}

/// Restore the original entry points.
#[pyfunction]
fn uninstall(py: Python<'_>) -> PyResult<()> {
    accel::registry::disable(py, Some(group()))
}

/// Names of the builtins currently accelerated.
#[pyfunction]
fn installed(py: Python<'_>) -> PyResult<Vec<&'static str>> {
    Ok(short(accel::registry::installed(py, Some(&group()))?))
}

/// A standalone function running the kernel of builtin `name` (installed or not). For tests
/// and benchmarks. Captures the originals (through the registry's `prepare` in record
/// mode) if nothing did yet.
#[pyfunction]
fn replacement(py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
    let i = NAMES
        .iter()
        .position(|n| n.to_str() == Ok(name))
        .ok_or_else(|| PyValueError::new_err(format!("not an accelerated builtin: {name:?}")))?;
    if !(ORIG_FASTKW.iter().all(Original::is_set) && ORIG_O.iter().all(Original::is_set)) {
        // The kernels need their originals: capture them without installing.
        let _ = accel::registry::fingerprint_report(py, Some(group()))?;
    }
    let mut state = lock_state()?;
    let captured = capture(py, &mut state)?;
    if captured.replacements[i].is_none()
        && let Some(def) = captured.defs[i]
    {
        let builtins = py.import("builtins")?;
        captured.replacements[i] = Some(make_replacement(py, ALL[i], def, &builtins)?);
    }
    captured.replacements[i]
        .as_ref()
        .map(|r| r.clone_ref(py))
        .ok_or_else(|| PyValueError::new_err(format!("builtin {name} was not recognised")))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = PyModule::new(m.py(), "_builtins")?;
    sub.add_function(wrap_pyfunction!(install, &sub)?)?;
    sub.add_function(wrap_pyfunction!(uninstall, &sub)?)?;
    sub.add_function(wrap_pyfunction!(installed, &sub)?)?;
    sub.add_function(wrap_pyfunction!(replacement, &sub)?)?;
    m.add("_builtins", sub)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_float_comparison_is_exact() {
        use Ordering::*;
        assert_eq!(cmp_int_float(1, 1.0), Some(Equal));
        assert_eq!(cmp_int_float(1, 1.5), Some(Less));
        assert_eq!(cmp_int_float(2, 1.5), Some(Greater));
        assert_eq!(cmp_int_float(-1, -1.5), Some(Greater));
        assert_eq!(cmp_int_float(0, f64::NAN), None);
        assert_eq!(cmp_int_float(i64::MAX, 9_223_372_036_854_775_808.0), Some(Less));
        assert_eq!(cmp_int_float(i64::MIN, -9_223_372_036_854_775_808.0), Some(Equal));
        assert_eq!(cmp_int_float((1 << 53) + 1, (1u64 << 53) as f64), Some(Greater));
        assert_eq!(cmp_int_float(i64::MAX, f64::INFINITY), Some(Less));
        assert_eq!(cmp_int_float(i64::MIN, f64::NEG_INFINITY), Some(Greater));
        assert_eq!(cmp_int_float(-(1 << 60) - 1, -((1u64 << 60) as f64)), Some(Less));
    }

    #[test]
    fn compensated_sum_matches_cpython_rules() {
        let mut cs = Cs::new(0.0 + 1e16);
        cs.add(1.0);
        cs.add(-1e16);
        assert_eq!(cs.value(), 1.0);
        assert!(Cs::new(-0.0).value().is_sign_negative());
    }
}
