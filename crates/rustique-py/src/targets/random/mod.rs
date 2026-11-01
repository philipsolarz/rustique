//! Group `random`: the pure-Python methods of `random.Random` that wrap the Mersenne
//! Twister — `randint`, `randrange`, `choice`, `shuffle`, `sample`, `choices` — as
//! vectorcall kernels on the function objects in `Random.__dict__` (the module-level
//! `random.randint` etc. are bound methods of `random._inst`, so they are covered, as are
//! user instances and subclasses). See docs/targets/random.md.
//!
//! OWNER: utils workstream.
//!
//! Exactness. CPython's methods reach randomness only through `self._randbelow(n)`
//! (= `_randbelow_with_getrandbits` unless a subclass changes it), `self.getrandbits(k)` and
//! `self.random()`. A kernel resolves those attributes exactly as `LOAD_ATTR` does
//! (`_PyType_Lookup` + `_PyObject_GetMethod`: class, then instance `__dict__`), without
//! running Python code, and then:
//! - when they are CPython's own (`_randbelow_with_getrandbits` and the C `getrandbits` /
//!   `random` of `_random.Random`), it runs the same algorithm on the object's
//!   `RandomObject` state with `rustique_core::random::Mt`, consuming the same words in the
//!   same order, so `getstate()` afterwards is identical;
//! - for `randint`/`randrange`/`choice`, when `getrandbits` or `_randbelow` is a Python
//!   function of a subclass (`SystemRandom`, user generators), it calls them exactly as the
//!   bytecode would (same arguments, same number of calls, re-looked-up each time) and
//!   combines the results with the generic number/sequence protocol;
//! - anything else (other argument types, instance attributes shadowing these methods,
//!   errors such as an empty sequence or range) runs CPython's original.
//! Free-threaded builds: the state is read and written under the object's critical section
//! (CPython's `getrandbits`/`random` are `@critical_section`), together with the list for
//! list arguments.

use std::ffi::c_int;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering::Relaxed};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};
use rustique_core::random::{Mt, N};

use crate::accel::stats::Reason;
use crate::targets::utils_kit::{
    self as kit, _PyObject_GetMethod, _PyType_Lookup, FnTarget, Guard, GroupState, O, call_original, int_value,
    interned, is_type, keep, long_from_i128, name_is, nargs, with_cs2,
};

// ---------------------------------------------------------------------------------------
// Statics filled by `prepare` (main interpreter objects; strong references kept).
// ---------------------------------------------------------------------------------------

static RND_BASE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static RANDOM_CLS: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static RB_WITH: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static C_GETRANDBITS: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static C_RANDOM: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static ONE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static S_RANDBELOW: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static S_GETRANDBITS: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static S_RANDOM: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static S_K: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static S_COUNTS: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static S_WEIGHTS: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static S_CUM_WEIGHTS: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
/// Offset of `RandomObject.index` (`state` follows at +4): `sizeof(PyObject)`, checked by
/// the self-test against `getstate()`.
static MT_OFF: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
fn ld(p: &AtomicPtr<ffi::PyObject>) -> O {
    p.load(Relaxed)
}

/// The Mersenne Twister state of a `RandomObject` (caller holds its critical section on
/// free-threaded builds and has checked the type is a subtype of `_random.Random`).
#[inline(always)]
unsafe fn mt<'a>(slf: O) -> Mt<'a> {
    // SAFETY: layout calibrated by the self-test; `slf` outlives the borrow (caller).
    unsafe {
        let base = slf.cast::<u8>().add(MT_OFF.load(Relaxed));
        Mt { index: &mut *base.cast::<i32>(), mt: &mut *base.add(4).cast::<[u32; N]>() }
    }
}

// ---------------------------------------------------------------------------------------
// How `self` produces randomness.
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Below {
    /// `_randbelow_with_getrandbits` with the C `getrandbits`: run natively.
    Native,
    /// `_randbelow_with_getrandbits` with a Python `getrandbits` (e.g. `SystemRandom`):
    /// inline the loop, call `self.getrandbits(k)` each time.
    GenericBits,
    /// Another Python `_randbelow` (e.g. `_randbelow_without_getrandbits`): call it.
    Call,
}

/// Does `self.<name>` resolve to the class attribute `expect` (not shadowed by the instance)?
/// Pure: `expect` is a function or method descriptor (no `__get__` call) and the type's
/// `tp_getattro` is the generic one (checked by the caller).
#[inline(always)]
unsafe fn unshadowed(slf: O, name: O, expect: O) -> bool {
    // SAFETY: attached thread; GetMethod returns a new reference or NULL with an error.
    unsafe {
        let mut m: O = std::ptr::null_mut();
        let found = _PyObject_GetMethod(slf, name, &mut m);
        if m.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        let ok = found == 1 && m == expect;
        ffi::Py_DECREF(m);
        ok
    }
}

#[inline(always)]
unsafe fn generic_getattro(tp: *mut ffi::PyTypeObject) -> bool {
    // SAFETY: live type.
    unsafe { (*tp).tp_getattro.map(|f| f as usize) == Some(ffi::PyObject_GenericGetAttr as *const () as usize) }
}

/// How `self._randbelow(n)` runs, decided without side effects; `None`: fall back.
#[inline]
unsafe fn below_mode(slf: O, allow_generic: bool) -> Option<Below> {
    // SAFETY: attached thread; lookups are pure (see `unshadowed`).
    unsafe {
        let tp = ffi::Py_TYPE(slf);
        if !generic_getattro(tp) {
            return None;
        }
        let rb = _PyType_Lookup(tp, ld(&S_RANDBELOW));
        if rb != ld(&RB_WITH) {
            if allow_generic && !rb.is_null() && is_type(rb, &raw mut ffi::PyFunction_Type) && unshadowed(slf, ld(&S_RANDBELOW), rb) {
                return Some(Below::Call);
            }
            return None;
        }
        if !unshadowed(slf, ld(&S_RANDBELOW), rb) {
            return None;
        }
        let gb = _PyType_Lookup(tp, ld(&S_GETRANDBITS));
        if gb == ld(&C_GETRANDBITS)
            && ffi::PyType_IsSubtype(tp, ld(&RND_BASE).cast()) != 0
            && unshadowed(slf, ld(&S_GETRANDBITS), gb)
        {
            return Some(Below::Native);
        }
        if allow_generic && !gb.is_null() && is_type(gb, &raw mut ffi::PyFunction_Type) && unshadowed(slf, ld(&S_GETRANDBITS), gb) {
            return Some(Below::GenericBits);
        }
        None
    }
}

/// Is `self.random` the C `random()` of `_random.Random`?
#[inline]
unsafe fn native_random(slf: O) -> bool {
    // SAFETY: as `below_mode`.
    unsafe {
        let tp = ffi::Py_TYPE(slf);
        generic_getattro(tp)
            && _PyType_Lookup(tp, ld(&S_RANDOM)) == ld(&C_RANDOM)
            && ffi::PyType_IsSubtype(tp, ld(&RND_BASE).cast()) != 0
            && unshadowed(slf, ld(&S_RANDOM), ld(&C_RANDOM))
    }
}

/// `self.<name>(arg)` exactly as the bytecode calls a method.
#[inline]
unsafe fn call_method1(slf: O, name: O, arg: O) -> O {
    // SAFETY: vectorcall with one writable slot before the arguments.
    unsafe {
        let mut buf = [std::ptr::null_mut(), slf, arg];
        ffi::PyObject_VectorcallMethod(name, buf.as_mut_ptr().add(1), 2 | ffi::PY_VECTORCALL_ARGUMENTS_OFFSET, std::ptr::null_mut())
    }
}

/// `self._randbelow(n)` for the generic modes; `n` is an exact positive int with
/// `n.bit_length() == k`. New reference or NULL with the error CPython raises.
unsafe fn randbelow_obj(slf: O, n: O, k: u32, mode: Below) -> O {
    // SAFETY: attached thread; Python code may run (generic calls), no borrowed state kept.
    unsafe {
        match mode {
            Below::Call => call_method1(slf, ld(&S_RANDBELOW), n),
            Below::Native => {
                // Not used by callers (they take the native path), kept total.
                let mut ov: c_int = 0;
                let v = ffi::PyLong_AsLongLongAndOverflow(n, &mut ov);
                debug_assert!(ov == 0 && v > 0);
                let r = with_cs2(slf, std::ptr::null_mut(), || mt(slf).randbelow(v as u64));
                ffi::PyLong_FromUnsignedLongLong(r)
            }
            Below::GenericBits => {
                // k = n.bit_length(); r = self.getrandbits(k); while r >= n: r = self.getrandbits(k)
                let kobj = ffi::PyLong_FromLong(k.into());
                if kobj.is_null() {
                    return kobj;
                }
                loop {
                    let r = call_method1(slf, ld(&S_GETRANDBITS), kobj);
                    if r.is_null() {
                        ffi::Py_DECREF(kobj);
                        return r;
                    }
                    let ge = ffi::PyObject_RichCompareBool(r, n, ffi::Py_GE);
                    if ge != 1 {
                        ffi::Py_DECREF(kobj);
                        if ge < 0 {
                            ffi::Py_DECREF(r);
                            return std::ptr::null_mut();
                        }
                        return r;
                    }
                    ffi::Py_DECREF(r);
                }
            }
        }
    }
}

/// `_index(x)` for an exact int (itself) or bool (the int 0/1).
#[inline]
unsafe fn index_obj(o: O, v: i64) -> O {
    // SAFETY: live object.
    unsafe {
        if is_type(o, &raw mut ffi::PyLong_Type) { ffi::Py_NewRef(o) } else { ffi::PyLong_FromLongLong(v) }
    }
}

/// `base + mult * r` with the generic number protocol (`mult` NULL: `base + r`). Steals `r`.
unsafe fn add_mul(base: O, mult: O, r: O) -> O {
    // SAFETY: owned references handled explicitly.
    unsafe {
        if r.is_null() {
            return r;
        }
        let prod = if mult.is_null() {
            r
        } else {
            let p = ffi::PyNumber_Multiply(mult, r);
            ffi::Py_DECREF(r);
            if p.is_null() {
                return p;
            }
            p
        };
        let out = ffi::PyNumber_Add(base, prod);
        ffi::Py_DECREF(prod);
        out
    }
}

fn bit_length(n: u128) -> u32 {
    128 - n.leading_zeros()
}

// ---------------------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------------------

static G_RANDINT: Guard = Guard::new();
static G_RANDRANGE: Guard = Guard::new();
static G_CHOICE: Guard = Guard::new();
static G_SHUFFLE: Guard = Guard::new();
static G_SAMPLE: Guard = Guard::new();
static G_CHOICES: Guard = Guard::new();

/// `return [start +] [mult *] self._randbelow(n)` once the arguments are checked: native
/// when possible, else generic. `None`: fall back.
#[inline]
unsafe fn ranged(slf: O, start: Option<(O, i64)>, mult: Option<(O, i64)>, n: u128) -> Option<O> {
    // SAFETY: attached thread.
    unsafe {
        let mode = below_mode(slf, true)?;
        if mode == Below::Native {
            let n64 = u64::try_from(n).ok()?;
            let r = with_cs2(slf, std::ptr::null_mut(), || mt(slf).randbelow(n64));
            let s = start.map_or(0, |(_, v)| i128::from(v));
            let m = mult.map_or(1, |(_, v)| i128::from(v));
            return Some(long_from_i128(s + m * i128::from(r)));
        }
        let nobj = long_from_i128(i128::try_from(n).ok()?);
        if nobj.is_null() {
            return Some(nobj);
        }
        let r = randbelow_obj(slf, nobj, bit_length(n), mode);
        ffi::Py_DECREF(nobj);
        let Some((so, sv)) = start else { return Some(r) };
        let base = index_obj(so, sv);
        let m = mult.map(|(o, v)| index_obj(o, v));
        let out = add_mul(base, m.unwrap_or(std::ptr::null_mut()), r);
        ffi::Py_DECREF(base);
        if let Some(m) = m {
            ffi::Py_DECREF(m);
        }
        Some(out)
    }
}

/// `Random.randint(self, a, b)`.
unsafe extern "C" fn vc_randint(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_RANDINT;
        if kw.is_null() && nargs(nargsf) == 3 {
            let (slf, ao, bo) = (*args, *args.add(1), *args.add(2));
            if let (Some(a), Some(b)) = (int_value(ao), int_value(bo))
                && b >= a
            {
                if !g.ready(func) {
                    return call_original(func, args, nargsf, kw);
                }
                let width = (i128::from(b) - i128::from(a) + 1) as u128;
                if let Some(r) = ranged(slf, Some((ao, a)), None, width) {
                    g.counters.hit();
                    return r;
                }
            }
        }
        g.counters.fallback(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

/// Python floor division of `a` by `b != 0`.
fn floordiv(a: i128, b: i128) -> i128 {
    let q = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) { q - 1 } else { q }
}

/// `Random.randrange(self, start, stop=None, step=_ONE)` (positional arguments).
unsafe extern "C" fn vc_randrange(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_RANDRANGE;
        let n = nargs(nargsf);
        if kw.is_null() && (2..=4).contains(&n) {
            let (slf, so) = (*args, *args.add(1));
            let stop = if n >= 3 { *args.add(2) } else { ffi::Py_None() };
            let step = if n == 4 { *args.add(3) } else { ld(&ONE) };
            if let Some(start) = int_value(so) {
                // (n, start, mult) of `[start +] [mult *] _randbelow(n)`, or None: CPython raises.
                type Plan = (u128, Option<(O, i64)>, Option<(O, i64)>);
                let plan: Option<Plan> = if stop == ffi::Py_None() {
                    // `step is not _ONE` -> TypeError; `istart <= 0` -> ValueError.
                    (step == ld(&ONE) && start > 0).then_some((start as u128, None, None))
                } else if let (Some(stopv), Some(stepv)) = (int_value(stop), int_value(step)) {
                    let width = i128::from(stopv) - i128::from(start);
                    if stepv == 1 {
                        (width > 0).then_some((width as u128, Some((so, start)), None))
                    } else {
                        let st = i128::from(stepv);
                        let cnt = match stepv.signum() {
                            1 => Some(floordiv(width + st - 1, st)),
                            -1 => Some(floordiv(width + st + 1, st)),
                            _ => None,
                        };
                        cnt.filter(|c| *c > 0).map(|c| (c as u128, Some((so, start)), Some((step, stepv))))
                    }
                } else {
                    None
                };
                if let Some((cnt, base, mult)) = plan {
                    if !g.ready(func) {
                        return call_original(func, args, nargsf, kw);
                    }
                    if let Some(r) = ranged(slf, base, mult, cnt) {
                        g.counters.hit();
                        return r;
                    }
                }
            }
        }
        g.counters.fallback(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

/// A sequence argument the kernels index without side effects.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seq {
    List,
    Tuple,
    /// `str` or `range`: items through `PySequence_GetItem` (pure, allocates).
    Other,
}

#[inline(always)]
unsafe fn seq_kind(o: O) -> Option<Seq> {
    // SAFETY: type pointer reads.
    unsafe {
        let t = ffi::Py_TYPE(o);
        if t == &raw mut ffi::PyList_Type {
            Some(Seq::List)
        } else if t == &raw mut ffi::PyTuple_Type {
            Some(Seq::Tuple)
        } else if t == &raw mut ffi::PyUnicode_Type || t == &raw mut ffi::PyRange_Type {
            Some(Seq::Other)
        } else {
            None
        }
    }
}

/// `len(seq)` of a supported sequence (`None` if it raises: huge range).
#[inline(always)]
unsafe fn seq_len(o: O, kind: Seq) -> Option<usize> {
    // SAFETY: exact types.
    unsafe {
        let n = match kind {
            Seq::List => ffi::PyList_GET_SIZE(o),
            Seq::Tuple => ffi::PyTuple_GET_SIZE(o),
            Seq::Other => ffi::PyObject_Size(o),
        };
        if n < 0 {
            ffi::PyErr_Clear();
            return None;
        }
        Some(n as usize)
    }
}

/// `seq[i]` for `0 <= i < len(seq)` (new reference; NULL only on allocation failure).
#[inline(always)]
unsafe fn seq_item(o: O, kind: Seq, i: usize) -> O {
    // SAFETY: index checked by the caller; the list is held (critical section / GIL).
    unsafe {
        match kind {
            Seq::List => ffi::Py_NewRef(*(*o.cast::<ffi::PyListObject>()).ob_item.add(i)),
            Seq::Tuple => ffi::Py_NewRef(ffi::PyTuple_GET_ITEM(o, i as ffi::Py_ssize_t)),
            Seq::Other => ffi::PySequence_GetItem(o, i as ffi::Py_ssize_t),
        }
    }
}

/// Lock for the list argument on free-threaded builds (NULL: none needed).
#[inline(always)]
fn lock_of(o: O, kind: Seq) -> O {
    if kind == Seq::List { o } else { std::ptr::null_mut() }
}

/// `Random.choice(self, seq)`.
unsafe extern "C" fn vc_choice(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_CHOICE;
        if kw.is_null() && nargs(nargsf) == 2 {
            let (slf, seq) = (*args, *args.add(1));
            if let Some(kind) = seq_kind(seq) {
                if !g.ready(func) {
                    return call_original(func, args, nargsf, kw);
                }
                if let Some(mode) = below_mode(slf, true) {
                    if mode == Below::Native {
                        let r = with_cs2(slf, lock_of(seq, kind), || {
                            let n = seq_len(seq, kind)?;
                            if n == 0 {
                                return None;
                            }
                            let i = mt(slf).randbelow(n as u64) as usize;
                            Some(seq_item(seq, kind, i))
                        });
                        if let Some(r) = r {
                            g.counters.hit();
                            return r;
                        }
                    } else if let Some(n) = seq_len(seq, kind).filter(|n| *n > 0) {
                        // `seq[self._randbelow(len(seq))]`
                        g.counters.hit();
                        let nobj = ffi::PyLong_FromSize_t(n);
                        if nobj.is_null() {
                            return nobj;
                        }
                        let r = randbelow_obj(slf, nobj, bit_length(n as u128), mode);
                        ffi::Py_DECREF(nobj);
                        if r.is_null() {
                            return r;
                        }
                        let item = ffi::PyObject_GetItem(seq, r);
                        ffi::Py_DECREF(r);
                        return item;
                    }
                }
            }
        }
        g.counters.fallback(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

/// `Random.shuffle(self, x)` for an exact list and CPython's own generator.
unsafe extern "C" fn vc_shuffle(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_SHUFFLE;
        if kw.is_null() && nargs(nargsf) == 2 {
            let (slf, x) = (*args, *args.add(1));
            if is_type(x, &raw mut ffi::PyList_Type) {
                if !g.ready(func) {
                    return call_original(func, args, nargsf, kw);
                }
                if below_mode(slf, false) == Some(Below::Native) {
                    with_cs2(slf, x, || {
                        let n = ffi::PyList_GET_SIZE(x) as usize;
                        let items = (*x.cast::<ffi::PyListObject>()).ob_item;
                        mt(slf).shuffle_swaps(n, |i, j| {
                            let (pi, pj) = (items.add(i), items.add(j));
                            let (a, b) = (*pi, *pj);
                            store_item(pi, b);
                            store_item(pj, a);
                        });
                    });
                    g.counters.hit();
                    return ffi::Py_NewRef(ffi::Py_None());
                }
            }
        }
        g.counters.fallback(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

/// Store a list item pointer (release store on free-threaded builds, where readers load
/// items without the lock).
#[inline(always)]
unsafe fn store_item(slot: *mut O, v: O) {
    // SAFETY: valid aligned slot of a list held by the caller.
    unsafe {
        #[cfg(Py_GIL_DISABLED)]
        (*slot.cast::<AtomicPtr<ffi::PyObject>>()).store(v, std::sync::atomic::Ordering::Release);
        #[cfg(not(Py_GIL_DISABLED))]
        {
            *slot = v;
        }
    }
}

/// Values of the keyword arguments of a call whose kwnames are a subset of `names`
/// (`None`: an unknown keyword). Slot `i` holds the value for `names[i]` or NULL.
#[inline]
unsafe fn kwargs<const K: usize>(args: *const O, npos: usize, kw: O, names: [O; K]) -> Option<[O; K]> {
    // SAFETY: kwnames tuple of str and its values after the positional arguments.
    unsafe {
        let mut out = [std::ptr::null_mut(); K];
        if kw.is_null() {
            return Some(out);
        }
        for i in 0..ffi::PyTuple_GET_SIZE(kw) as usize {
            let k = ffi::PyTuple_GET_ITEM(kw, i as ffi::Py_ssize_t);
            let slot = (0..K).find(|&j| name_is(k, names[j]))?;
            out[slot] = *args.add(npos + i);
        }
        Some(out)
    }
}

/// `Random.sample(self, population, k, *, counts=None)` without counts.
unsafe extern "C" fn vc_sample(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        if let Some(r) = sample(func, args, nargsf, kw) {
            return r;
        }
        call_original(func, args, nargsf, kw)
    }
}

unsafe fn sample(func: O, args: *const O, nargsf: usize, kw: O) -> Option<O> {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_SAMPLE;
        let npos = nargs(nargsf);
        let args_ok = (|| {
            if !(2..=3).contains(&npos) {
                return None;
            }
            let [kk, counts] = kwargs(args, npos, kw, [ld(&S_K), ld(&S_COUNTS)])?;
            if !counts.is_null() && counts != ffi::Py_None() {
                return None;
            }
            let k_obj = match (npos == 3, kk.is_null()) {
                (true, true) => *args.add(2),
                (false, false) => kk,
                _ => return None,
            };
            let pop = *args.add(1);
            let kind = seq_kind(pop)?;
            let k = int_value(k_obj)?;
            (0..1 << 40).contains(&k).then_some((pop, kind, k as usize))
        })();
        let Some((pop, kind, k)) = args_ok else {
            g.counters.fallback(Reason::Args);
            return None;
        };
        if !g.ready(func) {
            return None;
        }
        let slf = *args;
        if below_mode(slf, false) != Some(Below::Native) {
            g.counters.fallback(Reason::Args);
            return None;
        }
        // Allocate first: nothing after this can run Python code (no GC-tracked allocation).
        let out = ffi::PyList_New(k as ffi::Py_ssize_t);
        if out.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let ok = with_cs2(slf, lock_of(pop, kind), || {
            let n = seq_len(pop, kind)?;
            if k > n {
                return None; // ValueError
            }
            let idx = mt(slf).sample_indices(n, k);
            for (i, j) in idx.into_iter().enumerate() {
                let item = seq_item(pop, kind, j);
                if item.is_null() {
                    return Some(false);
                }
                ffi::PyList_SET_ITEM(out, i as ffi::Py_ssize_t, item);
            }
            Some(true)
        });
        match ok {
            None => {
                ffi::Py_DECREF(out);
                g.counters.fallback(Reason::Args);
                None
            }
            Some(true) => {
                g.counters.hit();
                Some(out)
            }
            Some(false) => {
                ffi::Py_DECREF(out);
                Some(std::ptr::null_mut())
            }
        }
    }
}

/// A cumulative weight: CPython's `int` or `float` running sum.
#[derive(Clone, Copy, Debug)]
enum Num {
    I(i128),
    F(f64),
}

const EXACT_F64_INT: i128 = 1 << 53;

impl Num {
    /// `self + other` with Python's int/float semantics (`None`: outside the exact range).
    fn add(self, o: Num) -> Option<Num> {
        Some(match (self, o) {
            (Num::I(a), Num::I(b)) => {
                let s = a.checked_add(b)?;
                if s.unsigned_abs() >= 1 << 100 {
                    return None;
                }
                Num::I(s)
            }
            (Num::I(a), Num::F(b)) => Num::F(a as f64 + b),
            (Num::F(a), Num::I(b)) => Num::F(a + b as f64),
            (Num::F(a), Num::F(b)) => Num::F(a + b),
        })
    }

    /// The value as an `f64` for exact comparisons with a float (`None` when an int is too
    /// large to compare exactly through `f64`).
    fn exact_f64(self) -> Option<f64> {
        match self {
            Num::I(v) => (v.unsigned_abs() <= EXACT_F64_INT as u128).then_some(v as f64),
            Num::F(v) => Some(v),
        }
    }
}

/// Read a weight list/tuple: exact `int` (i64) or `float` elements. `None`: not supported.
unsafe fn read_weights(o: O) -> Option<Vec<Num>> {
    // SAFETY: exact list/tuple, read under the list's critical section.
    unsafe {
        let list = is_type(o, &raw mut ffi::PyList_Type);
        if !list && !is_type(o, &raw mut ffi::PyTuple_Type) {
            return None;
        }
        with_cs2(o, std::ptr::null_mut(), || {
            let n = ffi::PySequence_Fast_GET_SIZE(o) as usize;
            let items = ffi::PySequence_Fast_ITEMS(o);
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let x = *items.add(i);
                if is_type(x, &raw mut ffi::PyFloat_Type) {
                    out.push(Num::F(ffi::PyFloat_AS_DOUBLE(x)));
                } else if is_type(x, &raw mut ffi::PyLong_Type) {
                    out.push(Num::I(i128::from(int_value(x)?)));
                } else {
                    return None;
                }
            }
            Some(out)
        })
    }
}

/// `bisect.bisect_right(a, x, 0, hi)` over exact values.
fn bisect_right(a: &[f64], x: f64, hi: usize) -> usize {
    let (mut lo, mut hi) = (0, hi);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if x < a[mid] {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// `Random.choices(self, population, weights=None, *, cum_weights=None, k=1)`.
unsafe extern "C" fn vc_choices(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        if let Some(r) = choices(func, args, nargsf, kw) {
            return r;
        }
        call_original(func, args, nargsf, kw)
    }
}

unsafe fn choices(func: O, args: *const O, nargsf: usize, kw: O) -> Option<O> {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_CHOICES;
        let npos = nargs(nargsf);
        // (population, kind, k, cumulative weights as f64 or None)
        let parsed = (|| {
            if !(2..=3).contains(&npos) {
                return None;
            }
            let [w_kw, cw, kk] = kwargs(args, npos, kw, [ld(&S_WEIGHTS), ld(&S_CUM_WEIGHTS), ld(&S_K)])?;
            let weights = match (npos == 3, w_kw.is_null()) {
                (true, true) => *args.add(2),
                (false, false) => w_kw,
                (false, true) => ffi::Py_None(),
                (true, false) => return None,
            };
            let k = if kk.is_null() { 1 } else { int_value(kk)? };
            if k >= 1 << 31 {
                return None;
            }
            let pop = *args.add(1);
            let kind = seq_kind(pop)?;
            let none = ffi::Py_None();
            let cum: Option<Vec<f64>> = match (weights == none, cw.is_null() || cw == none) {
                (true, true) => None,
                (false, true) => {
                    let w = read_weights(weights)?;
                    let mut acc: Vec<f64> = Vec::with_capacity(w.len());
                    let mut cur: Option<Num> = None;
                    let mut last = Num::I(0);
                    for x in w {
                        let v = match cur {
                            None => x,
                            Some(c) => c.add(x)?,
                        };
                        cur = Some(v);
                        last = v;
                        acc.push(v.exact_f64()?);
                    }
                    // `total = cum_weights[-1] + 0.0` must be exact too (int -> float).
                    if !acc.is_empty() {
                        let _ = last.exact_f64()?;
                    }
                    Some(acc)
                }
                (true, false) => {
                    let w = read_weights(cw)?;
                    Some(w.into_iter().map(Num::exact_f64).collect::<Option<Vec<f64>>>()?)
                }
                (false, false) => return None, // TypeError
            };
            Some((pop, kind, k, cum))
        })();
        let Some((pop, kind, k, cum)) = parsed else {
            g.counters.fallback(Reason::Args);
            return None;
        };
        if let Some(c) = &cum {
            // ValueError: length mismatch, total <= 0, total not finite.
            let total = c.last().copied().unwrap_or(f64::NAN) + 0.0;
            if !(total > 0.0 && total.is_finite()) {
                g.counters.fallback(Reason::Args);
                return None;
            }
        }
        if !g.ready(func) {
            return None;
        }
        let slf = *args;
        if !native_random(slf) {
            g.counters.fallback(Reason::Args);
            return None;
        }
        let kk = k.max(0) as usize;
        let out = ffi::PyList_New(kk as ffi::Py_ssize_t);
        if out.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let ok = with_cs2(slf, lock_of(pop, kind), || {
            let n = seq_len(pop, kind)?;
            if n == 0 || n as u64 >= 1 << 53 {
                return None; // IndexError (or out of the exact range)
            }
            let mut g = mt(slf);
            match &cum {
                None => {
                    let nf = n as f64;
                    for i in 0..kk {
                        let j = (g.random() * nf).floor() as usize;
                        debug_assert!(j < n);
                        let item = seq_item(pop, kind, j.min(n - 1));
                        if item.is_null() {
                            return Some(false);
                        }
                        ffi::PyList_SET_ITEM(out, i as ffi::Py_ssize_t, item);
                    }
                }
                Some(c) => {
                    if c.len() != n {
                        return None; // ValueError (checked before consuming randomness)
                    }
                    let total = c[n - 1] + 0.0;
                    for i in 0..kk {
                        let j = bisect_right(c, g.random() * total, n - 1);
                        let item = seq_item(pop, kind, j);
                        if item.is_null() {
                            return Some(false);
                        }
                        ffi::PyList_SET_ITEM(out, i as ffi::Py_ssize_t, item);
                    }
                }
            }
            Some(true)
        });
        match ok {
            None => {
                ffi::Py_DECREF(out);
                g.counters.fallback(Reason::Args);
                None
            }
            Some(true) => {
                g.counters.hit();
                Some(out)
            }
            Some(false) => {
                ffi::Py_DECREF(out);
                Some(std::ptr::null_mut())
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Preparation and self-test
// ---------------------------------------------------------------------------------------

fn refuse(why: &str) -> PyResult<Result<(), String>> {
    Ok(Err(why.to_string()))
}

fn prepare(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<Result<(), String>> {
    let d = m.dict();
    let Some(cls) = d.get_item("Random")? else { return refuse("random.Random missing") };
    if cls.as_ptr() == ld(&RANDOM_CLS) {
        return Ok(Ok(()));
    }
    let Some(cmod) = d.get_item("_random")? else { return refuse("random._random missing") };
    let base = cmod.getattr("Random")?;
    let bases = cls.getattr("__bases__")?;
    let bases = bases.cast::<PyTuple>()?;
    if bases.len() != 1 || !bases.get_item(0)?.is(&base) {
        return refuse("random.Random does not derive from _random.Random");
    }
    let cdict = cls.getattr("__dict__")?;
    let rb_with = cdict.get_item("_randbelow_with_getrandbits")?;
    if !cdict.get_item("_randbelow")?.is(&rb_with) {
        return refuse("Random._randbelow is not _randbelow_with_getrandbits");
    }
    let bdict = base.getattr("__dict__")?;
    let c_getrandbits = bdict.get_item("getrandbits")?;
    let c_random = bdict.get_item("random")?;
    let one = d.get_item("_ONE")?.ok_or_else(|| pyo3::exceptions::PyKeyError::new_err("_ONE"))?;
    if one.extract::<i64>()? != 1 {
        return refuse("random._ONE is not 1");
    }
    keep(&RND_BASE, &base);
    keep(&RB_WITH, &rb_with);
    keep(&C_GETRANDBITS, &c_getrandbits);
    keep(&C_RANDOM, &c_random);
    keep(&ONE, &one);
    for (slot, s) in [
        (&S_RANDBELOW, "_randbelow"),
        (&S_GETRANDBITS, "getrandbits"),
        (&S_RANDOM, "random"),
        (&S_K, "k"),
        (&S_COUNTS, "counts"),
        (&S_WEIGHTS, "weights"),
        (&S_CUM_WEIGHTS, "cum_weights"),
    ] {
        if slot.load(Relaxed).is_null() {
            slot.store(interned(py, s), Relaxed);
        }
    }
    MT_OFF.store(size_of::<ffi::PyObject>(), Relaxed);
    if let Err(why) = self_test(&cls)? {
        return Ok(Err(why));
    }
    keep(&RANDOM_CLS, &cls);
    Ok(Ok(()))
}

/// The `RandomObject` layout and the generator: the raw state equals `getstate()`, and
/// 1400 words (two twists) and three `random()` values generated natively equal CPython's.
fn self_test(cls: &Bound<'_, PyAny>) -> PyResult<Result<(), String>> {
    const SEED: u64 = 20_261_101;
    const WORDS: usize = 1400;
    let a = cls.call1((SEED,))?;
    let b = cls.call1((SEED,))?;
    let st = a.call_method0("getstate")?.get_item(1)?;
    let st = st.cast::<PyTuple>()?;
    if st.len() != N + 1 {
        return refuse("unexpected state size");
    }
    // SAFETY: probe object of the exact class; GIL/attached, no other thread knows it.
    let raw_ok = unsafe {
        let g = mt(a.as_ptr());
        let mut ok = i64::from(*g.index) == st.get_item(N)?.extract::<i64>()?;
        for i in 0..N {
            ok &= u64::from(g.mt[i]) == st.get_item(i)?.extract::<u64>()?;
        }
        ok
    };
    if !raw_ok {
        return refuse("RandomObject layout differs");
    }
    let big = b.call_method1("getrandbits", (32 * WORDS,))?;
    let bytes = big.call_method1("to_bytes", (4 * WORDS, "little"))?;
    let bytes = bytes.cast::<PyBytes>()?.as_bytes();
    // SAFETY: as above.
    let words_ok = unsafe {
        let mut g = mt(a.as_ptr());
        (0..WORDS).all(|i| g.next_u32().to_le_bytes() == bytes[4 * i..4 * i + 4])
    };
    if !words_ok {
        return refuse("generator self-test failed (getrandbits)");
    }
    for _ in 0..3 {
        let want: f64 = b.call_method0("random")?.extract()?;
        // SAFETY: as above.
        let got = unsafe { mt(a.as_ptr()).random() };
        if want.to_bits() != got.to_bits() {
            return refuse("generator self-test failed (random)");
        }
    }
    if !a.call_method0("getstate")?.eq(b.call_method0("getstate")?)? {
        return refuse("generator self-test failed (state)");
    }
    Ok(Ok(()))
}

// ---------------------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------------------

/// Builtins and module globals each original reads at call time.
const DEPS_RANDINT: &[(&str, &str)] = &[("random", "_index")];
const DEPS_RANDRANGE: &[(&str, &str)] = &[("random", "_index"), ("random", "_ONE")];
const DEPS_CHOICE: &[(&str, &str)] = &[("builtins", "len")];
const DEPS_SHUFFLE: &[(&str, &str)] = &[("builtins", "len"), ("builtins", "range"), ("builtins", "reversed")];
const DEPS_SAMPLE: &[(&str, &str)] = &[
    ("random", "_Sequence"),
    ("random", "_ceil"),
    ("random", "_log"),
    ("builtins", "isinstance"),
    ("builtins", "len"),
    ("builtins", "list"),
    ("builtins", "set"),
    ("builtins", "range"),
];
const DEPS_CHOICES: &[(&str, &str)] = &[
    ("random", "_floor"),
    ("random", "_repeat"),
    ("random", "_accumulate"),
    ("random", "_bisect"),
    ("random", "_isfinite"),
    ("builtins", "len"),
    ("builtins", "list"),
];

/// Fingerprints of the originals' code (`utils_kit::fingerprint`), per minor version.
mod fp {
    pub const RANDINT: &[(u8, u64)] = &[(14, 0x25B753E4DB177088), (15, 0xD79D9465E558A6CB)];
    pub const RANDRANGE: &[(u8, u64)] = &[(14, 0x29B7A43B9D30DCDF), (15, 0x17D42BB653563BD3)];
    pub const CHOICE: &[(u8, u64)] = &[(14, 0xE5B5782D94140643), (15, 0x458191DE11D66785)];
    pub const SHUFFLE: &[(u8, u64)] = &[(14, 0x7881F529899CF2B8), (15, 0x0ECC20BA2C8BAF29)];
    pub const SAMPLE: &[(u8, u64)] = &[(14, 0x14E8993C35B243B0), (15, 0x345F213E33B07612)];
    pub const CHOICES: &[(u8, u64)] = &[(14, 0x169BDA8043380FF2), (15, 0x417FD5686C920458)];
}

macro_rules! target {
    ($name:literal, $attr:literal, $kernel:ident, $fp:expr, $deps:expr, $guard:ident) => {
        FnTarget {
            name: $name,
            group: "random",
            module: "random",
            path: &["Random", $attr],
            kernel: $kernel,
            fingerprints: $fp,
            free_threaded: true,
            deps: $deps,
            guard: &$guard,
            prepare: Some(prepare),
        }
    };
}

pub static TARGETS: [FnTarget; 6] = [
    target!("random.randint", "randint", vc_randint, fp::RANDINT, DEPS_RANDINT, G_RANDINT),
    target!("random.randrange", "randrange", vc_randrange, fp::RANDRANGE, DEPS_RANDRANGE, G_RANDRANGE),
    target!("random.choice", "choice", vc_choice, fp::CHOICE, DEPS_CHOICE, G_CHOICE),
    target!("random.shuffle", "shuffle", vc_shuffle, fp::SHUFFLE, DEPS_SHUFFLE, G_SHUFFLE),
    target!("random.sample", "sample", vc_sample, fp::SAMPLE, DEPS_SAMPLE, G_SAMPLE),
    target!("random.choices", "choices", vc_choices, fp::CHOICES, DEPS_CHOICES, G_CHOICES),
];

static STATE: GroupState = Mutex::new(Vec::new());

/// Install the group's targets (the `random` module must be imported); see `status()`.
#[pyfunction]
fn install(py: Python<'_>) -> PyResult<()> {
    kit::install_group(py, &TARGETS, &STATE)
}

#[pyfunction]
fn uninstall() -> PyResult<()> {
    kit::uninstall_group(&STATE)
}

#[pyfunction]
fn status(py: Python<'_>) -> PyResult<Bound<'_, pyo3::types::PyList>> {
    kit::status_list(py, &STATE)
}

#[pyfunction]
fn counters(py: Python<'_>) -> PyResult<Bound<'_, pyo3::types::PyDict>> {
    kit::counters_dict(py, &TARGETS)
}

#[pyfunction]
fn reset_counters() {
    kit::reset_counters(&TARGETS);
}

#[pyfunction]
fn stats(on: bool) {
    kit::set_stats(on);
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = PyModule::new(m.py(), "_t_random")?;
    sub.add_function(wrap_pyfunction!(install, &sub)?)?;
    sub.add_function(wrap_pyfunction!(uninstall, &sub)?)?;
    sub.add_function(wrap_pyfunction!(status, &sub)?)?;
    sub.add_function(wrap_pyfunction!(counters, &sub)?)?;
    sub.add_function(wrap_pyfunction!(reset_counters, &sub)?)?;
    sub.add_function(wrap_pyfunction!(stats, &sub)?)?;
    sub.add_function(wrap_pyfunction!(kit::fingerprint, &sub)?)?;
    m.add("_t_random", sub)
}
