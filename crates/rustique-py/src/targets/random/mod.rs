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
//! the instance `__dict__`) without running Python code, and then:
//! - when they are CPython's own (`_randbelow_with_getrandbits` and the C `getrandbits` /
//!   `random` of `_random.Random`), it runs the same algorithm on the object's
//!   `RandomObject` state with `rustique_core::random::Mt`, consuming the same words in the
//!   same order, so `getstate()` afterwards is identical;
//! - for `randint`/`randrange`/`choice`, when `getrandbits` or `_randbelow` is a Python
//!   function of a subclass (`SystemRandom`, user generators), it calls them as the
//!   bytecode would (same arguments, same number of calls, looked up again before each
//!   call) and combines the results with the generic number/sequence protocol;
//! - anything else (other argument types, instance attributes shadowing these methods,
//!   every error CPython would raise) runs CPython's original.
//!
//! Lookups. The class side is classified once per (type, `tp_version_tag`) — the tag
//! changes whenever the type or a base is modified — and cached. The instance side uses
//! `utils_kit::INST` (inline values, calibrated) when the type's shared keys lack the three
//! names, else `_PyObject_GetMethod`.
//!
//! Free-threaded builds: the state is read and written under the object's critical section
//! (CPython's `getrandbits`/`random` are `@critical_section`), together with the list for
//! list arguments; nothing that can run Python code (or GC) happens inside it.

use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple, PyType};
use rustique_core::random::{Mt, N};

use crate::accel::{self, Prepare, Reason, Refusal, TargetSpec};
use crate::targets::utils_kit::{
    _PyType_Lookup, Guard, INST, O, bind, call_original, generic_getattro, int_value, interned, is_type, keep,
    long_from_i128, nargs, pin_globals, prepare_symbols, unshadowed, with_cs, with_cs2,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Below {
    /// `_randbelow_with_getrandbits` with the C `getrandbits`: run natively.
    Native = 1,
    /// `_randbelow_with_getrandbits` with a Python `getrandbits` (e.g. `SystemRandom`):
    /// inline the loop, call `self.getrandbits(k)` each time.
    GenericBits = 2,
    /// Another Python `_randbelow` (e.g. `_randbelow_without_getrandbits`): call it.
    Call = 3,
}

#[derive(Clone, Copy, Debug)]
struct Class {
    below: Option<Below>,
    random_native: bool,
    /// Shared keys of the type when they lacked our three names at `nentries` entries.
    keys: usize,
    nentries: usize,
}

/// One-entry cache of [`Class`] by (type, `tp_version_tag`), a seqlock (readers never
/// block; a writer that loses a race just does not cache).
struct TypeCache {
    seq: AtomicU64,
    tp: AtomicUsize,
    ver: AtomicU32,
    info: AtomicU32,
    keys: AtomicUsize,
    nentries: AtomicUsize,
}

static CACHE: TypeCache = TypeCache {
    seq: AtomicU64::new(0),
    tp: AtomicUsize::new(0),
    ver: AtomicU32::new(0),
    info: AtomicU32::new(0),
    keys: AtomicUsize::new(0),
    nentries: AtomicUsize::new(0),
};

impl TypeCache {
    #[inline(always)]
    fn get(&self, tp: usize, ver: u32) -> Option<Class> {
        let s1 = self.seq.load(Ordering::Acquire);
        if s1 & 1 != 0 {
            return None;
        }
        let (t, v, info, keys, nentries) =
            (self.tp.load(R), self.ver.load(R), self.info.load(R), self.keys.load(R), self.nentries.load(R));
        std::sync::atomic::fence(Ordering::Acquire);
        if self.seq.load(R) != s1 || t != tp || v != ver {
            return None;
        }
        let below = match info & 0xff {
            1 => Some(Below::Native),
            2 => Some(Below::GenericBits),
            3 => Some(Below::Call),
            _ => None,
        };
        Some(Class { below, random_native: info & 0x100 != 0, keys, nentries })
    }

    fn put(&self, tp: usize, ver: u32, c: &Class) {
        let s = self.seq.load(R);
        if s & 1 != 0 || self.seq.compare_exchange(s, s + 1, Ordering::Acquire, R).is_err() {
            return;
        }
        self.tp.store(tp, R);
        self.ver.store(ver, R);
        self.info.store(c.below.map_or(0, |b| b as u32) | if c.random_native { 0x100 } else { 0 }, R);
        self.keys.store(c.keys, R);
        self.nentries.store(c.nentries, R);
        self.seq.store(s + 2, Ordering::Release);
    }
}

#[inline(always)]
unsafe fn version_tag(tp: *mut ffi::PyTypeObject) -> u32 {
    // SAFETY: live type; a plain u32 field CPython updates atomically.
    unsafe { std::ptr::read_volatile(&raw const (*tp).tp_version_tag) }
}

/// Classify `tp` (pure: MRO dict lookups). `None` when attribute lookup is not the generic
/// one (a `__getattribute__`/`__getattr__` could run code): fall back.
#[inline]
unsafe fn class_of(tp: *mut ffi::PyTypeObject) -> Option<Class> {
    // SAFETY: attached thread; borrowed results are only compared.
    unsafe {
        let ver = version_tag(tp);
        if ver != 0
            && let Some(c) = CACHE.get(tp as usize, ver)
        {
            return Some(c);
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
        let is_fn = |o: O| !o.is_null() && is_type(o, &raw mut ffi::PyFunction_Type);
        let below = if rb == ld(&RB_WITH) {
            if gb == ld(&C_GETRANDBITS) && sub {
                Some(Below::Native)
            } else if is_fn(gb) {
                Some(Below::GenericBits)
            } else {
                None
            }
        } else if is_fn(rb) {
            Some(Below::Call)
        } else {
            None
        };
        let random_native = rr == ld(&C_RANDOM) && sub;
        let mut c = Class { below, random_native, keys: 0, nentries: 0 };
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
            CACHE.put(tp as usize, v0, &c);
        }
        Some(c)
    }
}

/// Is the instance free of attributes named `_randbelow`/`getrandbits`/`random` (fast
/// check; `false` means "unknown", not "shadowed")?
#[inline(always)]
unsafe fn instance_clean(slf: O, c: &Class) -> bool {
    // SAFETY: live object; `c` classified its type.
    unsafe {
        c.keys != 0
            && (*ffi::Py_TYPE(slf).cast::<ffi::PyHeapTypeObject>()).ht_cached_keys as usize == c.keys
            && INST.nentries(c.keys as *mut u8) == c.nentries
            && INST.inline_only(slf)
    }
}

/// How `self._randbelow(n)` runs, decided without side effects; `None`: fall back.
#[inline]
unsafe fn below_mode(slf: O, allow_generic: bool) -> Option<Below> {
    // SAFETY: attached thread; pure lookups.
    unsafe {
        let tp = ffi::Py_TYPE(slf);
        let c = class_of(tp)?;
        let b = c.below?;
        if !allow_generic && b != Below::Native {
            return None;
        }
        if instance_clean(slf, &c) {
            return Some(b);
        }
        let ok = match b {
            Below::Native => {
                unshadowed(slf, ld(&S.randbelow), ld(&RB_WITH))
                    && unshadowed(slf, ld(&S.getrandbits), ld(&C_GETRANDBITS))
            }
            Below::GenericBits => {
                unshadowed(slf, ld(&S.randbelow), ld(&RB_WITH))
                    && unshadowed(slf, ld(&S.getrandbits), _PyType_Lookup(tp, ld(&S.getrandbits)))
            }
            Below::Call => unshadowed(slf, ld(&S.randbelow), _PyType_Lookup(tp, ld(&S.randbelow))),
        };
        ok.then_some(b)
    }
}

/// Is `self.random` the C `random()` of `_random.Random`?
#[inline]
unsafe fn native_random(slf: O) -> bool {
    // SAFETY: as `below_mode`.
    unsafe {
        let Some(c) = class_of(ffi::Py_TYPE(slf)) else { return false };
        c.random_native && (instance_clean(slf, &c) || unshadowed(slf, ld(&S.random), ld(&C_RANDOM)))
    }
}

/// `self.<name>(arg)` exactly as the bytecode calls a method: when the class attribute is
/// a plain function the instance does not shadow, call it directly; otherwise let
/// `PyObject_VectorcallMethod` do the full lookup.
unsafe fn call_method1(slf: O, name: O, arg: O) -> O {
    // SAFETY: vectorcall with one writable slot before the arguments.
    unsafe {
        let mut buf = [std::ptr::null_mut(), slf, arg];
        let argv = buf.as_mut_ptr().add(1);
        let nf = 2 | ffi::PY_VECTORCALL_ARGUMENTS_OFFSET;
        #[cfg(not(Py_GIL_DISABLED))]
        {
            let tp = ffi::Py_TYPE(slf);
            if let Some(c) = class_of(tp)
                && instance_clean(slf, &c)
            {
                let f = _PyType_Lookup(tp, name);
                if !f.is_null() && is_type(f, &raw mut ffi::PyFunction_Type) {
                    // GIL held: the type keeps `f` alive until we own a reference.
                    ffi::Py_INCREF(f);
                    let r = ffi::PyObject_Vectorcall(f, argv, nf, std::ptr::null_mut());
                    ffi::Py_DECREF(f);
                    return r;
                }
            }
        }
        ffi::PyObject_VectorcallMethod(name, argv, nf, std::ptr::null_mut())
    }
}

/// `self._randbelow(n)` for the generic modes; `n` is an exact positive int with
/// `n.bit_length() == k`. New reference or NULL with the error CPython raises.
unsafe fn randbelow_obj(slf: O, n: O, k: u32, mode: Below) -> O {
    // SAFETY: attached thread; Python code may run (generic calls), no borrowed state kept.
    unsafe {
        match mode {
            Below::Call | Below::Native => call_method1(slf, ld(&S.randbelow), n),
            Below::GenericBits => {
                // k = n.bit_length(); r = self.getrandbits(k); while r >= n: r = self.getrandbits(k)
                let kobj = ffi::PyLong_FromLong(k.into());
                if kobj.is_null() {
                    return kobj;
                }
                loop {
                    let r = call_method1(slf, ld(&S.getrandbits), kobj);
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

fn bit_length(n: u128) -> u32 {
    128 - n.leading_zeros()
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

/// `return [start +] [mult *] self._randbelow(n)` once the arguments are checked: native
/// when possible, else generic. `None`: fall back. `start`/`mult` are int objects with
/// their values.
#[inline]
unsafe fn ranged(slf: O, start: Option<(O, i128)>, mult: Option<(O, i128)>, n: u128) -> Option<O> {
    // SAFETY: attached thread.
    unsafe {
        let mode = below_mode(slf, true)?;
        if mode == Below::Native {
            let r = with_cs(slf, || mt(slf).randbelow128(n));
            let s = start.map_or(0, |(_, v)| v);
            let m = mult.map_or(1, |(_, v)| v);
            return match m.checked_mul(r as i128).and_then(|p| p.checked_add(s)) {
                Some(v) => Some(long_from_i128(v)),
                // Out of i128 (huge step): finish with Python ints, same value.
                None => Some(finish_generic(start, mult, long_from_i128(r as i128))),
            };
        }
        let nobj = long_from_i128(i128::try_from(n).ok()?);
        if nobj.is_null() {
            return Some(nobj);
        }
        let r = randbelow_obj(slf, nobj, bit_length(n), mode);
        ffi::Py_DECREF(nobj);
        Some(finish_generic(start, mult, r))
    }
}

/// `[start +] [mult *] r` on objects (steals `r`).
unsafe fn finish_generic(start: Option<(O, i128)>, mult: Option<(O, i128)>, r: O) -> O {
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
                g.counters.hit();
                return r;
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
                    g.counters.hit();
                    return r;
                }
            }
        }
        g.counters.fallback(Reason::Args);
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
    CSeq,
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
        } else if t == &raw mut ffi::PyUnicode_Type || t == &raw mut ffi::PyRange_Type || t == &raw mut ffi::PyBytes_Type
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
            Seq::Other | Seq::CSeq => ffi::PyObject_Size(o),
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
            Seq::Other | Seq::CSeq => ffi::PySequence_GetItem(o, i as ffi::Py_ssize_t),
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
            let kind = seq_kind(seq).or_else(|| c_sequence(seq).then_some(Seq::CSeq))?;
            Some((slf, seq, kind))
        })();
        let Some((slf, seq, kind)) = parsed else {
            g.counters.fallback(Reason::Args);
            return None;
        };
        if !g.ready(func) {
            return None;
        }
        let Some(mode) = below_mode(slf, true) else {
            g.counters.fallback(Reason::Args);
            return None;
        };
        if mode == Below::Native && kind != Seq::CSeq {
            let r = with_cs2(slf, lock_of(seq, kind), || {
                let n = seq_len(seq, kind)?;
                if n == 0 {
                    return None; // IndexError
                }
                let i = mt(slf).randbelow(n as u64) as usize;
                Some(seq_item(seq, kind, i))
            });
            if r.is_none() {
                g.counters.fallback(Reason::Args);
            } else {
                g.counters.hit();
            }
            return r;
        }
        // `if not len(seq): raise IndexError(...)`; `return seq[self._randbelow(len(seq))]`
        let Some(n) = seq_len(seq, kind).filter(|n| *n > 0) else {
            g.counters.fallback(Reason::Args);
            return None;
        };
        g.counters.hit();
        let n2 = if kind == Seq::CSeq {
            // The second `len(seq)`: CPython calls it, so do we.
            let m = ffi::PyObject_Size(seq);
            if m < 0 {
                return Some(std::ptr::null_mut());
            }
            m as usize
        } else {
            n
        };
        if n2 == 0 {
            // `_randbelow(0)` never returns in CPython either; only reachable with a C
            // sequence whose length changed between the two calls.
            let nobj = ffi::PyLong_FromSize_t(0);
            let r = call_method1(slf, ld(&S.randbelow), nobj);
            ffi::Py_XDECREF(nobj);
            return Some(r);
        }
        let r = if mode == Below::Native {
            let i = with_cs(slf, || mt(slf).randbelow(n2 as u64));
            ffi::PyLong_FromUnsignedLongLong(i)
        } else {
            let nobj = ffi::PyLong_FromSize_t(n2);
            if nobj.is_null() {
                return Some(nobj);
            }
            let r = randbelow_obj(slf, nobj, bit_length(n2 as u128), mode);
            ffi::Py_DECREF(nobj);
            r
        };
        if r.is_null() {
            return Some(r);
        }
        let item = ffi::PyObject_GetItem(seq, r);
        ffi::Py_DECREF(r);
        Some(item)
    }
}

/// `Random.shuffle(self, x)` for an exact list and CPython's own generator.
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
            (0..1 << 40).contains(&k).then_some((slf, pop, kind, k as usize))
        })();
        let Some((slf, pop, kind, k)) = parsed else {
            g.counters.fallback(Reason::Args);
            return None;
        };
        if !g.ready(func) {
            return None;
        }
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
            let cum = cumulative(weights, cum_weights)?;
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
            g.counters.fallback(Reason::Args);
            return None;
        };
        if !g.ready(func) {
            return None;
        }
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
            let mut gen = mt(slf);
            match &cum {
                None => {
                    // `n += 0.0`; `population[floor(random() * n)]`
                    let nf = n as f64;
                    Some(fill(out, pop, kind, (0..kk).map(|_| ((gen.random() * nf).floor() as usize).min(n - 1))))
                }
                Some(c) => {
                    if c.len() != n {
                        return None; // ValueError
                    }
                    let total = c[n - 1] + 0.0;
                    Some(fill(out, pop, kind, (0..kk).map(|_| bisect_right(c, gen.random() * total, n - 1))))
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
    probe_slots(py)?;
    cx.self_test("random generator layout and stream", |_py| self_test(&base, &c_getrandbits, &c_random, &c_getstate))?;
    keep(&RANDOM_CLS, cls.as_any());
    Ok(cls)
}

fn is_small_one(o: &Bound<'_, PyAny>) -> bool {
    // SAFETY: type check and value read of an int.
    unsafe { is_type(o.as_ptr(), &raw mut ffi::PyLong_Type) && int_value(o.as_ptr()) == Some(1) }
}

/// The `slot_*` functions CPython installs for Python-level `__len__`/`__getitem__`
/// (read from a probe class created by calling `type`; no Python code runs).
fn probe_slots(py: Python<'_>) -> PyResult<()> {
    if SLOT_SQ_LENGTH.load(R) != 0 {
        return Ok(());
    }
    // SAFETY: C-level class creation; slot reads of the new heap type.
    unsafe {
        let ns = Bound::from_owned_ptr_or_err(py, ffi::PyDict_New())?;
        // Any non-wrapper value makes `type` install the `slot_*` functions.
        let v = Bound::from_owned_ptr_or_err(py, ffi::PyLong_FromLong(0))?;
        let ns = ns.cast_into::<pyo3::types::PyDict>()?;
        ns.set_item("__len__", &v)?;
        ns.set_item("__getitem__", &v)?;
        let ty = ffi::PyObject_CallFunction(
            (&raw mut ffi::PyType_Type).cast(),
            c"s()O".as_ptr(),
            c"_rustique_seq_probe".as_ptr(),
            ns.as_ptr(),
        );
        let ty = Bound::from_owned_ptr_or_err(py, ty)?;
        let t = ty.as_ptr().cast::<ffi::PyTypeObject>();
        let sq = (*t).tp_as_sequence;
        let mp = (*t).tp_as_mapping;
        SLOT_SQ_LENGTH.store((*sq).sq_length.map_or(1, |f| f as usize), R);
        SLOT_SQ_ITEM.store((*sq).sq_item.map_or(1, |f| f as usize), R);
        SLOT_MP_LENGTH.store((*mp).mp_length.map_or(1, |f| f as usize), R);
        SLOT_MP_SUBSCRIPT.store((*mp).mp_subscript.map_or(1, |f| f as usize), R);
    }
    Ok(())
}

/// The `RandomObject` layout and the generator, on `_random.Random` probes (C only): the
/// raw state equals `getstate()`, and 1400 words (two twists) and three `random()` values
/// generated natively equal CPython's, leaving the same state.
fn self_test(
    base: &Bound<'_, PyType>,
    getrandbits: &Bound<'_, PyAny>,
    random: &Bound<'_, PyAny>,
    getstate: &Bound<'_, PyAny>,
) -> PyResult<bool> {
    const SEED: u64 = 20_261_101;
    const WORDS: usize = 1400;
    let a = base.call1((SEED,))?;
    let b = base.call1((SEED,))?;
    let st = getstate.call1((&a,))?;
    let st = st.cast::<PyTuple>()?;
    if st.len() != N + 1 {
        return Ok(false);
    }
    // SAFETY: probe object of the C class; nobody else knows it.
    let raw_ok = unsafe {
        let g = mt(a.as_ptr());
        let mut ok = i64::from(*g.index) == st.get_item(N)?.extract::<i64>()?;
        for i in 0..N {
            ok &= u64::from(g.mt[i]) == st.get_item(i)?.extract::<u64>()?;
        }
        ok
    };
    if !raw_ok {
        return Ok(false);
    }
    let big = getrandbits.call1((&b, 32 * WORDS))?;
    let bytes = big.call_method1("to_bytes", (4 * WORDS, "little"))?;
    let bytes = bytes.cast::<PyBytes>()?.as_bytes();
    // SAFETY: as above.
    let words_ok = unsafe {
        let mut g = mt(a.as_ptr());
        (0..WORDS).all(|i| g.next_u32().to_le_bytes() == bytes[4 * i..4 * i + 4])
    };
    if !words_ok {
        return Ok(false);
    }
    for _ in 0..3 {
        let want: f64 = random.call1((&b,))?.extract()?;
        // SAFETY: as above.
        let got = unsafe { mt(a.as_ptr()).random() };
        if want.to_bits() != got.to_bits() {
            return Ok(false);
        }
    }
    getstate.call1((&a,))?.eq(getstate.call1((&b,))?)
}

/// Shared tail of every target's `prepare`: fingerprint, pins, guard identity, site.
#[allow(clippy::too_many_arguments)]
fn prepare_target(
    py: Python<'_>,
    cx: &mut Prepare<'_>,
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
        fn $prep(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
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
    &[(3, 14, "PENDING"), (3, 15, "PENDING")],
    globals: ["_index"], builtins: ["ValueError"]);
target!(RANDRANGE, prep_randrange, "random.randrange", "randrange", vc_randrange, G_RANDRANGE,
    &[(3, 14, "PENDING"), (3, 15, "PENDING")],
    globals: ["_index", "_ONE"], builtins: ["TypeError", "ValueError"]);
target!(CHOICE, prep_choice, "random.choice", "choice", vc_choice, G_CHOICE,
    &[(3, 14, "PENDING"), (3, 15, "PENDING")],
    globals: [], builtins: ["len", "IndexError"]);
target!(SHUFFLE, prep_shuffle, "random.shuffle", "shuffle", vc_shuffle, G_SHUFFLE,
    &[(3, 14, "PENDING"), (3, 15, "PENDING")],
    globals: [], builtins: ["len", "range", "reversed"]);
target!(SAMPLE, prep_sample, "random.sample", "sample", vc_sample, G_SAMPLE,
    &[(3, 14, "PENDING"), (3, 15, "PENDING")],
    globals: ["_Sequence", "_ceil", "_log", "_accumulate", "_bisect"],
    builtins: ["isinstance", "len", "list", "set", "range", "int", "TypeError", "ValueError"]);
target!(CHOICES, prep_choices, "random.choices", "choices", vc_choices, G_CHOICES,
    &[(3, 14, "PENDING"), (3, 15, "PENDING")],
    globals: ["_floor", "_repeat", "_accumulate", "_bisect", "_isfinite"],
    builtins: ["len", "list", "isinstance", "int", "TypeError", "ValueError"]);

pub static TARGETS: &[&TargetSpec] = &[&RANDINT, &RANDRANGE, &CHOICE, &SHUFFLE, &SAMPLE, &CHOICES];
