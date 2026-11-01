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
//! `self.random()`. A kernel resolves those attributes as `LOAD_ATTR` does (class MRO, then
//! the instance `__dict__`) without running Python code, at entry, before anything
//! observable, and then:
//! - when they are CPython's own (`_randbelow_with_getrandbits` and the C `getrandbits` /
//!   `random` of `_random.Random`), it runs the same algorithm on the object's
//!   `RandomObject` state with `rustique_core::random::Mt`, consuming the same words in the
//!   same order, so `getstate()` afterwards is identical;
//! - when any of them is something else (a Python `getrandbits`/`_randbelow`/`random` of a
//!   subclass such as `SystemRandom`, or an instance attribute), it calls CPython's
//!   original: a kernel never calls user code, so frames and tracebacks are CPython's;
//! - anything else (other argument types, every error CPython would raise) also runs
//!   CPython's original.
//!
//! Lookups. The class side is classified once per (type, `tp_version_tag`) — the tag
//! changes whenever the type or a base is modified — and cached. The instance side uses
//! `utils_kit::INST` (inline values, calibrated) when the type's shared keys lack the three
//! names, else `_PyObject_GetMethod`.
//!
//! Free-threaded builds: the state is read and written under the object's critical section
//! (CPython's `getrandbits`/`random` are `@critical_section`), together with the list for
//! list arguments; nothing that can run Python code (or GC) happens inside it.

use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyTuple, PyType};
use rustique_core::random::{Mt, N};

use crate::accel::{self, Prepare, Reason, Refusal, TargetSpec};
use crate::targets::utils_kit::{
    _PyType_Lookup, Guard, INST, O, TypeCache, bind, call_original, generic_getattro, int_value, interned, is_type,
    keep, long_from_i128, nargs, pin_globals, prepare_symbols, unshadowed, version_tag, with_cs, with_cs2,
};

const R: Ordering = Ordering::Relaxed;

// ---------------------------------------------------------------------------------------
// Statics filled by `prepare` (main interpreter objects; strong references kept).
// ---------------------------------------------------------------------------------------

static RND_BASE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static RANDOM_CLS: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static RB_WITH: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static C_GETRANDBITS: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static C_RANDOM: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static ONE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static SLOT_SQ_LENGTH: AtomicUsize = AtomicUsize::new(0);
static SLOT_MP_LENGTH: AtomicUsize = AtomicUsize::new(0);
static SLOT_SQ_ITEM: AtomicUsize = AtomicUsize::new(0);
static SLOT_MP_SUBSCRIPT: AtomicUsize = AtomicUsize::new(0);

/// Interned names (process lifetime).
struct Names {
    randbelow: AtomicPtr<ffi::PyObject>,
    getrandbits: AtomicPtr<ffi::PyObject>,
    random: AtomicPtr<ffi::PyObject>,
    a: AtomicPtr<ffi::PyObject>,
    b: AtomicPtr<ffi::PyObject>,
    start: AtomicPtr<ffi::PyObject>,
    stop: AtomicPtr<ffi::PyObject>,
    step: AtomicPtr<ffi::PyObject>,
    seq: AtomicPtr<ffi::PyObject>,
    x: AtomicPtr<ffi::PyObject>,
    population: AtomicPtr<ffi::PyObject>,
    k: AtomicPtr<ffi::PyObject>,
    counts: AtomicPtr<ffi::PyObject>,
    weights: AtomicPtr<ffi::PyObject>,
    cum_weights: AtomicPtr<ffi::PyObject>,
}

macro_rules! null {
    () => {
        AtomicPtr::new(std::ptr::null_mut())
    };
}

static S: Names = Names {
    randbelow: null!(),
    getrandbits: null!(),
    random: null!(),
    a: null!(),
    b: null!(),
    start: null!(),
    stop: null!(),
    step: null!(),
    seq: null!(),
    x: null!(),
    population: null!(),
    k: null!(),
    counts: null!(),
    weights: null!(),
    cum_weights: null!(),
};

/// Offset of `RandomObject.index` (`state` follows at +4): `sizeof(PyObject)`, checked by
/// the self-test against `getstate()`.
static MT_OFF: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
fn ld(p: &AtomicPtr<ffi::PyObject>) -> O {
    p.load(R)
}

/// The Mersenne Twister state of a `RandomObject` (caller holds its critical section on
/// free-threaded builds and has checked the type is a subtype of `_random.Random`).
#[inline(always)]
unsafe fn mt<'a>(slf: O) -> Mt<'a> {
    // SAFETY: layout calibrated by the self-test; `slf` outlives the borrow (caller).
    unsafe {
        let base = slf.cast::<u8>().add(MT_OFF.load(R));
        Mt { index: &mut *base.cast::<i32>(), mt: &mut *base.add(4).cast::<[u32; N]>() }
    }
}

// ---------------------------------------------------------------------------------------
// How `self` produces randomness: class classification (cached) + instance check.
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Class {
    /// `_randbelow` is `_randbelow_with_getrandbits` and `getrandbits` the C one of
    /// `_random.Random` (a subtype of it): `_randbelow(n)` runs natively.
    below_native: bool,
    /// `random` is the C `random()` of `_random.Random` (a subtype of it).
    random_native: bool,
    /// Shared keys of the type when they lacked our three names at `nentries` entries.
    keys: usize,
    nentries: usize,
}

static CACHE: TypeCache = TypeCache::new();

impl Class {
    fn pack(&self) -> u32 {
        u32::from(self.below_native) | u32::from(self.random_native) << 1
    }
    fn unpack(info: u32, keys: usize, nentries: usize) -> Class {
        Class { below_native: info & 1 != 0, random_native: info & 2 != 0, keys, nentries }
    }
}

/// Classify `tp` (pure: MRO dict lookups). `None` when attribute lookup is not the generic
/// one (a `__getattribute__`/`__getattr__` could run code): fall back.
#[inline]
unsafe fn class_of(tp: *mut ffi::PyTypeObject) -> Option<Class> {
    // SAFETY: attached thread; borrowed results are only compared.
    unsafe {
        let ver = version_tag(tp);
        if ver != 0
            && let Some((info, keys, nentries)) = CACHE.get(tp as usize, ver)
        {
            return Some(Class::unpack(info, keys, nentries));
        }
        if !generic_getattro(tp) {
            return None;
        }
        // Make sure a tag is assigned before reading it (lookups assign one).
        let _ = _PyType_Lookup(tp, ld(&S.randbelow));
        let v0 = version_tag(tp);
        let rb = _PyType_Lookup(tp, ld(&S.randbelow));
        let gb = _PyType_Lookup(tp, ld(&S.getrandbits));
        let rr = _PyType_Lookup(tp, ld(&S.random));
        let sub = ffi::PyType_IsSubtype(tp, ld(&RND_BASE).cast()) != 0;
        let below_native = sub && rb == ld(&RB_WITH) && gb == ld(&C_GETRANDBITS);
        let random_native = sub && rr == ld(&C_RANDOM);
        let mut c = Class { below_native, random_native, keys: 0, nentries: 0 };
        let keys = INST.shared_keys(tp);
        if !keys.is_null() {
            let n = INST.nentries(keys);
            if INST.absent(keys, ld(&S.randbelow))
                && INST.absent(keys, ld(&S.getrandbits))
                && INST.absent(keys, ld(&S.random))
            {
                c.keys = keys as usize;
                c.nentries = n;
            }
        }
        if v0 != 0 && version_tag(tp) == v0 {
            CACHE.put(tp as usize, v0, c.pack(), c.keys, c.nentries);
        }
        Some(c)
    }
}

/// Is the instance free of attributes named `_randbelow`/`getrandbits`/`random` (fast
/// check; `false` means "unknown", not "shadowed")?
#[inline(always)]
unsafe fn instance_clean(slf: O, c: &Class) -> bool {
    // SAFETY: live object; `c` classified its type.
    unsafe { INST.clean(slf, c.keys, c.nentries) }
}

/// Does the instance have `name` in its `__dict__` (where the class attribute is
/// `class_attr`, a function or method descriptor)? Exact and pure.
#[inline]
unsafe fn inst_has(slf: O, name: O, class_attr: O) -> bool {
    // SAFETY: attached thread.
    unsafe {
        match INST.has_attr(slf, name) {
            Some(b) => b,
            None => !unshadowed(slf, name, class_attr),
        }
    }
}

/// Does `self._randbelow(n)` run CPython's own `_randbelow_with_getrandbits` with the C
/// `getrandbits` (decided without side effects)? `false`: something else would run (a
/// Python override, an instance attribute, or attribute lookup that runs Python code):
/// fall back.
#[inline]
unsafe fn below_native(slf: O) -> bool {
    // SAFETY: attached thread; pure lookups.
    unsafe {
        let Some(c) = class_of(ffi::Py_TYPE(slf)) else { return false };
        c.below_native
            && (instance_clean(slf, &c)
                || !(inst_has(slf, ld(&S.randbelow), ld(&RB_WITH))
                    || inst_has(slf, ld(&S.getrandbits), ld(&C_GETRANDBITS))))
    }
}

/// Is `self.random` the C `random()` of `_random.Random` (decided as `below_native`)?
#[inline]
unsafe fn random_native(slf: O) -> bool {
    // SAFETY: as `below_native`.
    unsafe {
        let Some(c) = class_of(ffi::Py_TYPE(slf)) else { return false };
        c.random_native && (instance_clean(slf, &c) || !inst_has(slf, ld(&S.random), ld(&C_RANDOM)))
    }
}

/// Value of an `int` (any subclass) that fits an `i128` with room for arithmetic.
#[inline(always)]
unsafe fn int128(o: O) -> Option<i128> {
    // SAFETY: int objects only.
    unsafe {
        if let Some(v) = int_value(o) {
            return Some(v.into());
        }
        if ffi::PyLong_Check(o) == 0 {
            return None;
        }
        let mut buf = [0u8; 16];
        let need = ffi::PyLong_AsNativeBytes(o, buf.as_mut_ptr().cast(), 16, -1);
        if need < 0 {
            ffi::PyErr_Clear();
            return None;
        }
        if need > 16 {
            return None;
        }
        let v = i128::from_ne_bytes(buf);
        (v.unsigned_abs() < 1 << 125).then_some(v)
    }
}

/// `_index(x)` of an int (itself when exact, else an exact copy: `operator.index` returns
/// an exact `int` for subclasses without calling `__index__`).
#[inline]
unsafe fn index_obj(o: O) -> O {
    // SAFETY: an int object.
    unsafe { if is_type(o, &raw mut ffi::PyLong_Type) { ffi::Py_NewRef(o) } else { ffi::PyNumber_Index(o) } }
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

// ---------------------------------------------------------------------------------------
// Kernels
// ---------------------------------------------------------------------------------------

static G_RANDINT: Guard = Guard::new();
static G_RANDRANGE: Guard = Guard::new();
static G_CHOICE: Guard = Guard::new();
static G_SHUFFLE: Guard = Guard::new();
static G_SAMPLE: Guard = Guard::new();
static G_CHOICES: Guard = Guard::new();

/// `return [start +] [mult *] self._randbelow(n)` once the arguments are checked, natively.
/// `None`: fall back. `start`/`mult` are int objects with their values.
#[inline]
unsafe fn ranged(slf: O, start: Option<(O, i128)>, mult: Option<(O, i128)>, n: u128) -> Option<O> {
    // SAFETY: attached thread.
    unsafe {
        if !below_native(slf) {
            return None;
        }
        let r = with_cs(slf, || mt(slf).randbelow128(n));
        let s = start.map_or(0, |(_, v)| v);
        let m = mult.map_or(1, |(_, v)| v);
        Some(match m.checked_mul(r as i128).and_then(|p| p.checked_add(s)) {
            Some(v) => long_from_i128(v),
            // Out of i128 (huge step): finish with exact Python ints, same value.
            None => finish_big(start, mult, long_from_i128(r as i128)),
        })
    }
}

/// `[start +] [mult *] r` on objects (steals `r`): exact `int` arithmetic (`_index` of
/// the arguments), no user code.
unsafe fn finish_big(start: Option<(O, i128)>, mult: Option<(O, i128)>, r: O) -> O {
    // SAFETY: owned references handled explicitly.
    unsafe {
        let Some((so, _)) = start else { return r };
        if r.is_null() {
            return r;
        }
        let base = index_obj(so);
        let m = mult.map_or(std::ptr::null_mut(), |(o, _)| index_obj(o));
        let out = if base.is_null() || (mult.is_some() && m.is_null()) {
            ffi::Py_DECREF(r);
            std::ptr::null_mut()
        } else {
            add_mul(base, m, r)
        };
        ffi::Py_XDECREF(base);
        ffi::Py_XDECREF(m);
        out
    }
}

/// `Random.randint(self, a, b)`.
unsafe extern "C" fn vc_randint(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_RANDINT;
        if let Some([slf, ao, bo]) = bind(args, nargs(nargsf), kw, [std::ptr::null_mut(), ld(&S.a), ld(&S.b)])
            && !slf.is_null()
            && !ao.is_null()
            && !bo.is_null()
            && let (Some(a), Some(b)) = (int128(ao), int128(bo))
            && b >= a
        {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if let Some(r) = ranged(slf, Some((ao, a)), None, (b - a + 1) as u128) {
                g.counters.hit_streak();
                return r;
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

/// Python floor division of `a` by `b != 0`.
fn floordiv(a: i128, b: i128) -> i128 {
    let q = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) { q - 1 } else { q }
}

/// `Random.randrange(self, start, stop=None, step=_ONE)`.
unsafe extern "C" fn vc_randrange(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_RANDRANGE;
        let bound = bind(args, nargs(nargsf), kw, [std::ptr::null_mut(), ld(&S.start), ld(&S.stop), ld(&S.step)]);
        if let Some([slf, so, stop, step]) = bound
            && !slf.is_null()
            && !so.is_null()
            && let Some(start) = int128(so)
        {
            let none = ffi::Py_None();
            let stop = if stop.is_null() { none } else { stop };
            let step = if step.is_null() { ld(&ONE) } else { step };
            // (n, start, mult) of `[start +] [mult *] _randbelow(n)`; None: CPython raises.
            type Plan = (u128, Option<(O, i128)>, Option<(O, i128)>);
            let plan: Option<Plan> = if stop == none {
                // `step is not _ONE` -> TypeError; `istart <= 0` -> ValueError.
                (step == ld(&ONE) && start > 0).then_some((start as u128, None, None))
            } else if let (Some(stopv), Some(stepv)) = (int128(stop), int128(step)) {
                let width = stopv - start;
                if stepv == 1 {
                    (width > 0).then_some((width as u128, Some((so, start)), None))
                } else {
                    let cnt = match stepv.signum() {
                        1 => Some(floordiv(width + stepv - 1, stepv)),
                        -1 => Some(floordiv(width + stepv + 1, stepv)),
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
                    g.counters.hit_streak();
                    return r;
                }
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

/// A sequence argument.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seq {
    List,
    Tuple,
    /// `str`, `bytes` or `range`: items through `PySequence_GetItem` (pure, allocates
    /// only non-GC objects).
    Other,
    /// A C-level sequence (`choice` only): `len()` and `[]` through the generic protocol,
    /// called exactly as CPython's `choice` calls them.
    CLevel,
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
        } else if t == &raw mut ffi::PyUnicode_Type
            || t == &raw mut ffi::PyRange_Type
            || t == &raw mut ffi::PyBytes_Type
        {
            Some(Seq::Other)
        } else {
            None
        }
    }
}

/// A sequence type implemented in C (its length and item slots are not the `slot_*`
/// wrappers of Python-level `__len__`/`__getitem__`), and not a mapping.
#[inline]
unsafe fn c_sequence(o: O) -> bool {
    // SAFETY: slot reads of a live type.
    unsafe {
        let t = ffi::Py_TYPE(o);
        let sq = (*t).tp_as_sequence;
        if sq.is_null() || (*sq).sq_length.is_none() || (*sq).sq_item.is_none() {
            return false;
        }
        if ffi::PyType_HasFeature(t, ffi::Py_TPFLAGS_MAPPING) != 0 {
            return false;
        }
        let len = (*sq).sq_length.map(|f| f as usize);
        let item = (*sq).sq_item.map(|f| f as usize);
        let mp = (*t).tp_as_mapping;
        let (mlen, msub) = if mp.is_null() {
            (None, None)
        } else {
            ((*mp).mp_length.map(|f| f as usize), (*mp).mp_subscript.map(|f| f as usize))
        };
        len != Some(SLOT_SQ_LENGTH.load(R))
            && item != Some(SLOT_SQ_ITEM.load(R))
            && mlen != Some(SLOT_MP_LENGTH.load(R))
            && msub != Some(SLOT_MP_SUBSCRIPT.load(R))
    }
}

/// `len(seq)` (`None` if it raises: huge range, odd C sequence).
#[inline(always)]
unsafe fn seq_len(o: O, kind: Seq) -> Option<usize> {
    // SAFETY: exact types.
    unsafe {
        let n = match kind {
            Seq::List => ffi::PyList_GET_SIZE(o),
            Seq::Tuple => ffi::PyTuple_GET_SIZE(o),
            Seq::Other | Seq::CLevel => ffi::PyObject_Size(o),
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
            Seq::Other | Seq::CLevel => ffi::PySequence_GetItem(o, i as ffi::Py_ssize_t),
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
        if let Some(r) = choice(func, args, nargsf, kw) {
            return r;
        }
        call_original(func, args, nargsf, kw)
    }
}

unsafe fn choice(func: O, args: *const O, nargsf: usize, kw: O) -> Option<O> {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_CHOICE;
        let parsed = (|| {
            let [slf, seq] = bind(args, nargs(nargsf), kw, [std::ptr::null_mut(), ld(&S.seq)])?;
            if slf.is_null() || seq.is_null() {
                return None;
            }
            let kind = seq_kind(seq).or_else(|| c_sequence(seq).then_some(Seq::CLevel))?;
            Some((slf, seq, kind))
        })();
        let Some((slf, seq, kind)) = parsed else {
            g.counters.fallback_streak(Reason::Args);
            return None;
        };
        if !g.ready(func) {
            return None;
        }
        if !below_native(slf) {
            g.counters.fallback_streak(Reason::Args);
            return None;
        }
        if kind != Seq::CLevel {
            let r = with_cs2(slf, lock_of(seq, kind), || {
                let n = seq_len(seq, kind)?;
                if n == 0 {
                    return None; // IndexError
                }
                let i = mt(slf).randbelow(n as u64) as usize;
                Some(seq_item(seq, kind, i))
            });
            if r.is_none() {
                g.counters.fallback_streak(Reason::Args);
            } else {
                g.counters.hit_streak();
            }
            return r;
        }
        // A C-level sequence: `if not len(seq): raise IndexError(...)`;
        // `return seq[self._randbelow(len(seq))]` (both `len()` calls, as CPython).
        if seq_len(seq, kind).is_none_or(|n| n == 0) {
            g.counters.fallback_streak(Reason::Args);
            return None;
        }
        g.counters.hit_streak();
        let n2 = ffi::PyObject_Size(seq);
        if n2 < 0 {
            return Some(std::ptr::null_mut());
        }
        if n2 == 0 {
            // The length changed between the two calls: CPython's `_randbelow(0)` never
            // returns (it loops until interrupted); run that very function.
            let nobj = ffi::PyLong_FromSize_t(0);
            if nobj.is_null() {
                return Some(nobj);
            }
            let mut buf = [std::ptr::null_mut(), slf, nobj];
            let nf = 2 | ffi::PY_VECTORCALL_ARGUMENTS_OFFSET;
            let r = ffi::PyObject_VectorcallMethod(ld(&S.randbelow), buf.as_mut_ptr().add(1), nf, std::ptr::null_mut());
            ffi::Py_DECREF(nobj);
            return Some(r);
        }
        let i = with_cs(slf, || mt(slf).randbelow(n2 as u64));
        let r = ffi::PyLong_FromUnsignedLongLong(i);
        if r.is_null() {
            return Some(r);
        }
        let item = ffi::PyObject_GetItem(seq, r);
        ffi::Py_DECREF(r);
        Some(item)
    }
}

/// `Random.shuffle(self, x)` for an exact list.
unsafe extern "C" fn vc_shuffle(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_SHUFFLE;
        if let Some([slf, x]) = bind(args, nargs(nargsf), kw, [std::ptr::null_mut(), ld(&S.x)])
            && !slf.is_null()
            && !x.is_null()
            && is_type(x, &raw mut ffi::PyList_Type)
        {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if below_native(slf) {
                g.counters.hit_streak();
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
                return ffi::Py_NewRef(ffi::Py_None());
            }
        }
        g.counters.fallback_streak(Reason::Args);
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
        (*slot.cast::<AtomicPtr<ffi::PyObject>>()).store(v, Ordering::Release);
        #[cfg(not(Py_GIL_DISABLED))]
        {
            *slot = v;
        }
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

/// Fill `out` (a fresh list of `len(idx)` NULL items) with `pop[j]` for `j` in `idx`.
/// `false`: allocation failure (error set).
unsafe fn fill(out: O, pop: O, kind: Seq, idx: impl IntoIterator<Item = usize>) -> bool {
    // SAFETY: indices in range; `out` exclusively ours.
    unsafe {
        for (i, j) in idx.into_iter().enumerate() {
            let item = seq_item(pop, kind, j);
            if item.is_null() {
                return false;
            }
            ffi::PyList_SET_ITEM(out, i as ffi::Py_ssize_t, item);
        }
        true
    }
}

unsafe fn sample(func: O, args: *const O, nargsf: usize, kw: O) -> Option<O> {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_SAMPLE;
        let parsed = (|| {
            let names = [std::ptr::null_mut(), ld(&S.population), ld(&S.k), ld(&S.counts)];
            let npos = nargs(nargsf);
            if npos > 3 {
                return None; // `counts` is keyword-only
            }
            let [slf, pop, k_obj, counts] = bind(args, npos, kw, names)?;
            if slf.is_null() || pop.is_null() || k_obj.is_null() || !(counts.is_null() || counts == ffi::Py_None()) {
                return None;
            }
            let kind = seq_kind(pop)?;
            let k = int_value(k_obj)?;
            let n = seq_len(pop, kind)?; // pure; re-read under the lock below
            (0..=n as i64).contains(&k).then_some((slf, pop, kind, k as usize))
        })();
        let Some((slf, pop, kind, k)) = parsed else {
            g.counters.fallback_streak(Reason::Args);
            return None;
        };
        if !g.ready(func) {
            return None;
        }
        if !below_native(slf) {
            g.counters.fallback_streak(Reason::Args);
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
                return None; // ValueError (the list shrank on another thread)
            }
            let idx = mt(slf).sample_indices(n, k);
            Some(fill(out, pop, kind, idx))
        });
        finish_list(g, out, ok)
    }
}

/// Outcome of a list-building kernel: `None` → fall back (nothing observable happened),
/// `Some(false)` → error set.
unsafe fn finish_list(g: &Guard, out: O, ok: Option<bool>) -> Option<O> {
    // SAFETY: `out` is our new list.
    unsafe {
        match ok {
            None => {
                ffi::Py_DECREF(out);
                g.counters.fallback_streak(Reason::Args);
                None
            }
            Some(true) => {
                g.counters.hit_streak();
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

const EXACT_F64_INT: u128 = 1 << 53;

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
            // int + float: the int converts with correct rounding (as `i128 as f64` does).
            (Num::I(a), Num::F(b)) => Num::F(a as f64 + b),
            (Num::F(a), Num::I(b)) => Num::F(a + b as f64),
            (Num::F(a), Num::F(b)) => Num::F(a + b),
        })
    }

    /// The value as an `f64` for exact comparisons with a float (`None` when an int is too
    /// large to compare exactly through `f64`).
    fn exact_f64(self) -> Option<f64> {
        match self {
            Num::I(v) => (v.unsigned_abs() <= EXACT_F64_INT).then_some(v as f64),
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
        with_cs(o, || {
            let (n, items) = if list {
                (ffi::PyList_GET_SIZE(o) as usize, (*o.cast::<ffi::PyListObject>()).ob_item as *const O)
            } else {
                (ffi::PyTuple_GET_SIZE(o) as usize, (*o.cast::<ffi::PyTupleObject>()).ob_item.as_ptr() as *const O)
            };
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

/// `bisect.bisect_right(a, x, 0, hi)` over exact values (`x < a[mid]`, as `_bisect`).
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

/// Cumulative weights as exact `f64`s (CPython: `list(_accumulate(weights))` or
/// `cum_weights` itself), when every value compares exactly through `f64`.
unsafe fn cumulative(weights: O, cum_weights: O) -> Option<Option<Vec<f64>>> {
    // SAFETY: argument objects.
    unsafe {
        let none = ffi::Py_None();
        match (weights.is_null() || weights == none, cum_weights.is_null() || cum_weights == none) {
            (true, true) => Some(None),
            (false, true) => {
                let w = read_weights(weights)?;
                let mut acc: Vec<f64> = Vec::with_capacity(w.len());
                let mut cur: Option<Num> = None;
                for x in w {
                    let v = match cur {
                        None => x,
                        Some(c) => c.add(x)?,
                    };
                    cur = Some(v);
                    acc.push(v.exact_f64()?);
                }
                Some(Some(acc))
            }
            (true, false) => {
                let w = read_weights(cum_weights)?;
                Some(Some(w.into_iter().map(Num::exact_f64).collect::<Option<Vec<f64>>>()?))
            }
            (false, false) => None, // TypeError
        }
    }
}

unsafe fn choices(func: O, args: *const O, nargsf: usize, kw: O) -> Option<O> {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_CHOICES;
        let parsed = (|| {
            let names = [std::ptr::null_mut(), ld(&S.population), ld(&S.weights), ld(&S.cum_weights), ld(&S.k)];
            let npos = nargs(nargsf);
            if npos > 3 {
                return None; // `cum_weights` and `k` are keyword-only
            }
            let [slf, pop, weights, cum_weights, kk] = bind(args, npos, kw, names)?;
            if slf.is_null() || pop.is_null() {
                return None;
            }
            let k = if kk.is_null() { 1 } else { int_value(kk)? };
            if k >= 1 << 31 {
                return None;
            }
            let kind = seq_kind(pop)?;
            let n = seq_len(pop, kind)?;
            if n == 0 || n as u64 >= 1 << 53 {
                return None; // IndexError (or out of the exact range)
            }
            let cum = cumulative(weights, cum_weights)?;
            if cum.as_ref().is_some_and(|c| c.len() != n) {
                return None; // ValueError
            }
            if let Some(c) = &cum {
                // ValueError: total <= 0 or not finite (also: empty -> IndexError).
                let total = c.last().copied().unwrap_or(f64::NAN) + 0.0;
                if !(total > 0.0 && total.is_finite()) {
                    return None;
                }
            }
            Some((slf, pop, kind, k, cum))
        })();
        let Some((slf, pop, kind, k, cum)) = parsed else {
            g.counters.fallback_streak(Reason::Args);
            return None;
        };
        if !g.ready(func) {
            return None;
        }
        if !random_native(slf) {
            g.counters.fallback_streak(Reason::Args);
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
            let mut rng = mt(slf);
            match &cum {
                None => {
                    // `n += 0.0`; `population[floor(random() * n)]`
                    let nf = n as f64;
                    Some(fill(out, pop, kind, (0..kk).map(|_| ((rng.random() * nf).floor() as usize).min(n - 1))))
                }
                Some(c) => {
                    if c.len() != n {
                        return None; // ValueError
                    }
                    let total = c[n - 1] + 0.0;
                    Some(fill(out, pop, kind, (0..kk).map(|_| bisect_right(c, rng.random() * total, n - 1))))
                }
            }
        });
        finish_list(g, out, ok)
    }
}

// ---------------------------------------------------------------------------------------
// Preparation and self-test
// ---------------------------------------------------------------------------------------

/// Fill the group's statics from the `random` module (idempotent per `Random` class) and
/// run the self-test. C calls only (may run inside the `sys.modules` watcher).
fn prepare_common<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<Bound<'py, PyType>, Refusal> {
    prepare_symbols(cx)?;
    let cls = cx.module_attr("Random")?.cast_into::<PyType>().map_err(PyErr::from)?;
    if cls.as_ptr() == ld(&RANDOM_CLS) {
        return Ok(cls);
    }
    let cmod = cx.module_attr("_random")?.cast_into::<pyo3::types::PyModule>().map_err(PyErr::from)?;
    let base = cx.get(&cmod.dict(), "Random")?.cast_into::<PyType>().map_err(PyErr::from)?;
    // SAFETY: a ready type: tp_bases is a tuple.
    let bases = unsafe { Bound::from_borrowed_ptr(py, (*cls.as_type_ptr()).tp_bases) };
    let bases = bases.cast::<PyTuple>().map_err(PyErr::from)?;
    if bases.len() != 1 || !bases.get_item(0)?.is(&base) {
        return Err(Refusal::new("random.Random does not derive from _random.Random"));
    }
    let rb_with = cx.type_attr(&cls, "_randbelow_with_getrandbits")?;
    if !cx.type_attr(&cls, "_randbelow")?.is(&rb_with) {
        return Err(Refusal::new("Random._randbelow is not _randbelow_with_getrandbits"));
    }
    let c_getrandbits = cx.type_attr(&base, "getrandbits")?;
    let c_random = cx.type_attr(&base, "random")?;
    let c_getstate = cx.type_attr(&base, "getstate")?;
    let one = cx.module_attr("_ONE")?;
    if !is_small_one(&one) {
        return Err(Refusal::new("random._ONE is not 1"));
    }
    keep(&RND_BASE, &base);
    keep(&RB_WITH, &rb_with);
    keep(&C_GETRANDBITS, &c_getrandbits);
    keep(&C_RANDOM, &c_random);
    keep(&ONE, &one);
    for (slot, s) in [
        (&S.randbelow, "_randbelow"),
        (&S.getrandbits, "getrandbits"),
        (&S.random, "random"),
        (&S.a, "a"),
        (&S.b, "b"),
        (&S.start, "start"),
        (&S.stop, "stop"),
        (&S.step, "step"),
        (&S.seq, "seq"),
        (&S.x, "x"),
        (&S.population, "population"),
        (&S.k, "k"),
        (&S.counts, "counts"),
        (&S.weights, "weights"),
        (&S.cum_weights, "cum_weights"),
    ] {
        if slot.load(R).is_null() {
            slot.store(interned(py, s), R);
        }
    }
    MT_OFF.store(size_of::<ffi::PyObject>(), R);
    INST.calibrate(py)?;
    probe_slots(cx)?;
    cx.self_test("random generator layout and stream", |_py| self_test(&base, &c_getrandbits, &c_random, &c_getstate))?;
    keep(&RANDOM_CLS, cls.as_any());
    Ok(cls)
}

fn is_small_one(o: &Bound<'_, PyAny>) -> bool {
    // SAFETY: type check and value read of an int.
    unsafe { is_type(o.as_ptr(), &raw mut ffi::PyLong_Type) && int_value(o.as_ptr()) == Some(1) }
}

/// The `slot_*` functions CPython installs for Python-level `__len__`/`__getitem__`, read
/// from `os._Environ` (a Python class defining both; `os` is imported at startup), so no
/// probe class is created.
fn probe_slots(cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if SLOT_SQ_LENGTH.load(R) != 0 {
        return Ok(());
    }
    let os = cx.loaded_module("os")?;
    let env = cx.get(&os.dict(), "_Environ")?.cast_into::<PyType>().map_err(PyErr::from)?;
    // The class must define both in Python (its own `__dict__` entries are functions).
    for n in ["__len__", "__getitem__"] {
        let f = cx.type_attr(&env, n)?;
        // SAFETY: type check of a live object.
        if unsafe { !is_type(f.as_ptr(), &raw mut ffi::PyFunction_Type) } {
            return Err(Refusal::new("os._Environ does not define __len__/__getitem__ in Python"));
        }
    }
    // SAFETY: slot reads of a ready heap type.
    unsafe {
        let t = env.as_type_ptr();
        let sq = (*t).tp_as_sequence;
        let mp = (*t).tp_as_mapping;
        if sq.is_null() || mp.is_null() {
            return Err(Refusal::new("os._Environ has no sequence/mapping slots"));
        }
        SLOT_SQ_LENGTH.store((*sq).sq_length.map_or(1, |f| f as usize), R);
        SLOT_SQ_ITEM.store((*sq).sq_item.map_or(1, |f| f as usize), R);
        SLOT_MP_LENGTH.store((*mp).mp_length.map_or(1, |f| f as usize), R);
        SLOT_MP_SUBSCRIPT.store((*mp).mp_subscript.map_or(1, |f| f as usize), R);
    }
    Ok(())
}

/// The `RandomObject` layout and the generator, on one `_random.Random` probe (C calls
/// only): 40 words generated natively from a copy of its raw state (the first word twists
/// the freshly seeded state) equal its `getrandbits(32 * 40)`, then one `random()` value.
/// Kept small (install-cost budget); the algorithms are covered by `cargo test`
/// (reference outputs) and the conformance runs.
fn self_test(
    base: &Bound<'_, PyType>,
    getrandbits: &Bound<'_, PyAny>,
    random: &Bound<'_, PyAny>,
    _getstate: &Bound<'_, PyAny>,
) -> PyResult<bool> {
    const SEED: u64 = 20_261_101;
    const WORDS: usize = 40;
    let a = base.call1((SEED,))?;
    // SAFETY: probe object of the C class; nobody else knows it.
    let (mut idx, mut state) = unsafe {
        let g = mt(a.as_ptr());
        (*g.index, *g.mt)
    };
    let mut copy = Mt { index: &mut idx, mt: &mut state };
    let big = getrandbits.call1((&a, 32 * WORDS))?;
    let mut bytes = [0u8; 4 * WORDS];
    // SAFETY: an exact int; little-endian unsigned copy.
    let n = unsafe {
        ffi::PyLong_AsNativeBytes(
            big.as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len() as ffi::Py_ssize_t,
            ffi::Py_ASNATIVEBYTES_LITTLE_ENDIAN | ffi::Py_ASNATIVEBYTES_UNSIGNED_BUFFER,
        )
    };
    if n < 0 || n as usize > bytes.len() {
        return Ok(false);
    }
    if !(0..WORDS).all(|i| copy.next_u32().to_le_bytes() == bytes[4 * i..4 * i + 4]) {
        return Ok(false);
    }
    let want: f64 = random.call1((&a,))?.extract()?;
    let got = copy.random();
    // SAFETY: as above.
    let ia = unsafe { *mt(a.as_ptr()).index };
    Ok(want.to_bits() == got.to_bits() && ia == *copy.index)
}

/// Shared tail of every target's `prepare`: fingerprint, pins, guard identity, site.
#[allow(clippy::too_many_arguments)]
fn prepare_target<'py>(
    py: Python<'py>,
    cx: &mut Prepare<'py>,
    attr: &str,
    fp: accel::Fingerprints,
    module_globals: &[&str],
    builtins: &[&str],
    g: &'static Guard,
    kernel: ffi::vectorcallfunc,
) -> Result<(), Refusal> {
    let cls = prepare_common(py, cx)?;
    let func = cx.type_attr(&cls, attr)?;
    cx.fingerprint_code(&func, fp)?;
    let globals = cx.module()?.dict();
    for n in module_globals {
        cx.pin(&globals, n, &g.deps)?;
    }
    pin_globals(cx, &globals, builtins, &g.deps)?;
    g.capture(&func);
    cx.vectorcall(&func, kernel, &accel::FUNCTION_VECTORCALL)
}

macro_rules! target {
    ($spec:ident, $prep:ident, $name:literal, $attr:literal, $kernel:ident, $guard:ident, $fp:expr,
     globals: [$($gl:literal),*], builtins: [$($bl:literal),*]) => {
        fn $prep<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
            prepare_target(py, cx, $attr, $fp, &[$($gl),*], &[$($bl),*], &$guard, $kernel)
        }
        pub static $spec: TargetSpec = TargetSpec {
            name: $name,
            group: "random",
            module: Some("random"),
            versions: &[(3, 14), (3, 15)],
            free_threaded: true,
            counters: &$guard.counters,
            prepare: $prep,
            ..TargetSpec::DEFAULT
        };
    };
}

target!(RANDINT, prep_randint, "random.randint", "randint", vc_randint, G_RANDINT,
    &[(3, 14, "adea30b60f8787ba"), (3, 15, "35301d24f32825ff")],
    globals: ["_index"], builtins: []);
target!(RANDRANGE, prep_randrange, "random.randrange", "randrange", vc_randrange, G_RANDRANGE,
    &[(3, 14, "fdf9b5db8c0c8ef5"), (3, 15, "9d5fda7e7e5a709a")],
    globals: ["_index", "_ONE"], builtins: []);
target!(CHOICE, prep_choice, "random.choice", "choice", vc_choice, G_CHOICE,
    &[(3, 14, "30ab10d8aa189081"), (3, 15, "7f77eb62838674c9")],
    globals: [], builtins: ["len"]);
target!(SHUFFLE, prep_shuffle, "random.shuffle", "shuffle", vc_shuffle, G_SHUFFLE,
    &[(3, 14, "827e7189dab3b943"), (3, 15, "a37235ce15918735")],
    globals: [], builtins: ["len", "range", "reversed"]);
target!(SAMPLE, prep_sample, "random.sample", "sample", vc_sample, G_SAMPLE,
    &[(3, 14, "0405fd0e5439b39b"), (3, 15, "0a89161adf0e847e")],
    globals: ["_Sequence", "_ceil", "_log"],
    builtins: ["isinstance", "len", "list", "set", "range"]);
target!(CHOICES, prep_choices, "random.choices", "choices", vc_choices, G_CHOICES,
    &[(3, 14, "de1e524b9f5e3ca9"), (3, 15, "748775b6bd8b61e8")],
    globals: ["_floor", "_repeat", "_accumulate", "_bisect", "_isfinite"],
    builtins: ["len", "list"]);

pub static TARGETS: &[&TargetSpec] = &[&RANDINT, &RANDRANGE, &CHOICE, &SHUFFLE, &SAMPLE, &CHOICES];
