//! Helpers shared by the utils groups (`random`, `textwrap`, `urllib`, `statistics`,
//! `ipaddress`): pure-Python function targets on top of the accel registry.
//!
//! OWNER: utils workstream.
//!
//! - [`Guard`]: per-target counters, dependency flag, tools scope, plus the identity of the
//!   patched function's code/defaults/kwdefaults (assigning `__code__` or `__defaults__`
//!   keeps the patched vectorcall, so the kernel checks it still runs the validated code).
//! - [`InstLayout`]: a calibrated, side-effect-free "may this instance have attribute
//!   `name` in its `__dict__`?" for objects with inline values (CPython 3.13+ layout),
//!   used to decide `self.method` lookups without `_PyObject_GetMethod`'s cost.
//! - small exact-type helpers (ints, critical sections, keyword binding).

use std::ffi::{CStr, c_int};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering, Ordering::Relaxed};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyString;

use crate::accel::{self, Counters, DepFlag, Original, Prepare, Reason, Refusal, Tools};

pub type O = *mut ffi::PyObject;

/// `_PyObject_GetMethod(obj, name, &method)` (`PyAPI_FUNC`, internal headers, 3.14/3.15):
/// the lookup behind `LOAD_ATTR` for a method call. 1 + the unbound descriptor when the
/// type provides a method descriptor that the instance does not shadow; else 0 and the
/// attribute (or NULL with an error).
pub type GetMethodFn = unsafe extern "C" fn(O, O, *mut O) -> c_int;
pub static GET_METHOD: Original<GetMethodFn> = Original::new();

unsafe extern "C" {
    /// `PyAPI_FUNC` in `Include/cpython/object.h`: MRO lookup, borrowed (compare only).
    pub fn _PyType_Lookup(tp: *mut ffi::PyTypeObject, name: O) -> O;
}

/// Resolve the internal-but-exported symbols the utils kernels call.
pub fn prepare_symbols(cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    cx.symbol(c"_PyObject_GetMethod", &GET_METHOD)?;
    Ok(())
}

#[inline(always)]
pub fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

/// # Safety
/// Live object.
#[inline(always)]
pub unsafe fn is_type(o: O, t: *mut ffi::PyTypeObject) -> bool {
    // SAFETY: per the caller.
    unsafe { ffi::Py_TYPE(o) == t }
}

/// Fall back: CPython's `_PyFunction_Vectorcall` with the unchanged arguments.
///
/// # Safety
/// Vectorcall arguments as received by a kernel installed through
/// `cx.vectorcall(.., &accel::FUNCTION_VECTORCALL)`.
#[inline(always)]
pub unsafe fn call_original(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: per the caller.
    unsafe { accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kw) }
}

/// Per-target guard state.
pub struct Guard {
    code: AtomicPtr<ffi::PyObject>,
    defaults: AtomicPtr<ffi::PyObject>,
    kwdefaults: AtomicPtr<ffi::PyObject>,
    pub tools: Tools,
    pub deps: DepFlag,
    pub counters: Counters,
}

impl Guard {
    pub const fn new() -> Self {
        Guard {
            code: AtomicPtr::new(std::ptr::null_mut()),
            defaults: AtomicPtr::new(std::ptr::null_mut()),
            kwdefaults: AtomicPtr::new(std::ptr::null_mut()),
            tools: Tools::new(),
            deps: DepFlag::new(),
            counters: Counters::new(),
        }
    }

    /// Everything but the argument subset: the function still has the validated
    /// code/defaults/kwdefaults, the pinned dependencies are intact, no tool listens.
    /// Counts the reason when it says no.
    ///
    /// # Safety
    /// `func` is the patched function object (attached thread).
    #[inline(always)]
    pub unsafe fn ready(&self, func: O) -> bool {
        // SAFETY: `func` is a function object (the kernel is installed on it only).
        unsafe {
            let f = func.cast::<ffi::PyFunctionObject>();
            if (*f).func_code != self.code.load(Relaxed)
                || (*f).func_defaults != self.defaults.load(Relaxed)
                || (*f).func_kwdefaults != self.kwdefaults.load(Relaxed)
                || !self.deps.ok()
            {
                self.counters.fallback_streak(Reason::Deps);
                return false;
            }
            if !accel::tools_quiet(&self.tools, func) {
                self.counters.fallback_streak(Reason::Tools);
                return false;
            }
            true
        }
    }

    /// Record the identity of `func`'s code and defaults (compared by address only).
    pub fn capture(&self, func: &Bound<'_, PyAny>) {
        let f = func.as_ptr().cast::<ffi::PyFunctionObject>();
        // SAFETY: exact function object (the caller declares a vectorcall site on it).
        unsafe {
            self.code.store((*f).func_code, Relaxed);
            self.defaults.store((*f).func_defaults, Relaxed);
            self.kwdefaults.store((*f).func_kwdefaults, Relaxed);
        }
    }
}

impl Default for Guard {
    fn default() -> Self {
        Self::new()
    }
}

/// Pin what `LOAD_GLOBAL name` resolves to in `globals`, for each name.
pub fn pin_globals(
    cx: &mut Prepare<'_>,
    globals: &Bound<'_, pyo3::types::PyDict>,
    names: &[&str],
    flag: &'static DepFlag,
) -> Result<(), Refusal> {
    for n in names {
        cx.pin_global(globals, n, flag)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Instance attribute check (inline values).
// ---------------------------------------------------------------------------------------

/// Calibrated layout of objects whose type has `Py_TPFLAGS_INLINE_VALUES` (3.13+): the
/// managed `__dict__` pointer before the object and the inline values after
/// `tp_basicsize` (`struct _dictvalues { u8 capacity, size, embedded, valid; values[] }`),
/// plus `dk_nentries` of the type's shared keys (`ht_cached_keys`).
///
/// Invariant used (CPython 3.13–3.15, `Objects/dictobject.c`): while an object's inline
/// values are `valid` and no dict was materialized for it, its attributes are exactly the
/// shared keys with a non-NULL value; shared keys only ever grow (`dk_nentries` counts
/// insertions). So if `name` was absent from the shared keys when they had `n` entries
/// and they still have `n`, no such object has `name` in its `__dict__`.
pub struct InstLayout {
    ok: AtomicBool,
    dict_off: AtomicIsize,
    lookup: AtomicUsize,
}

pub static INST: InstLayout =
    InstLayout { ok: AtomicBool::new(false), dict_off: AtomicIsize::new(0), lookup: AtomicUsize::new(0) };

/// `Py_TPFLAGS_INLINE_VALUES` (object.h, 3.13+).
const TPFLAGS_INLINE_VALUES: std::ffi::c_ulong = 1 << 2;
const DK_NENTRIES_OFF: usize = 24;

type StringLookupFn = unsafe extern "C" fn(*mut u8, O) -> ffi::Py_ssize_t;

#[cfg(unix)]
fn dlsym(name: &CStr) -> usize {
    unsafe extern "C" {
        fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    }
    #[cfg(target_os = "macos")]
    let handle = -2isize as *mut std::ffi::c_void;
    #[cfg(not(target_os = "macos"))]
    let handle = std::ptr::null_mut();
    // SAFETY: NUL-terminated name.
    unsafe { dlsym(handle, name.as_ptr()) as usize }
}

#[cfg(not(unix))]
fn dlsym(_name: &CStr) -> usize {
    0
}

impl InstLayout {
    /// Calibrate once (no Python code runs: a class is created by calling `type`, an
    /// instance's attributes set and its dict materialized through the generic C paths).
    /// On any mismatch the fast check stays off and kernels use `_PyObject_GetMethod`
    /// (exact, slower).
    pub fn calibrate(&self, py: Python<'_>) -> PyResult<bool> {
        if self.ok.load(Relaxed) {
            return Ok(true);
        }
        let lookup = dlsym(c"_PyDictKeys_StringLookup");
        if lookup == 0 {
            return Ok(false);
        }
        let dict_off: isize = if cfg!(Py_GIL_DISABLED) { -8 } else { -24 };
        // SAFETY: C-level object creation and layout reads on probe objects we own.
        let ok = unsafe {
            let ty = ffi::PyObject_CallFunction(
                (&raw mut ffi::PyType_Type).cast(),
                c"s()N".as_ptr(),
                c"_rustique_inline_probe".as_ptr(),
                ffi::PyDict_New(),
            );
            if ty.is_null() {
                return Err(PyErr::fetch(py));
            }
            let ty = Bound::from_owned_ptr(py, ty);
            let tp = ty.as_ptr().cast::<ffi::PyTypeObject>();
            if (*tp).tp_flags & TPFLAGS_INLINE_VALUES == 0 {
                return Ok(false);
            }
            let obj = Bound::from_owned_ptr_or_err(py, ffi::PyObject_CallNoArgs(ty.as_ptr()))?;
            let o = obj.as_ptr();
            let sentinel = PyString::new(py, "sentinel");
            let name_a = PyString::intern(py, "a");
            let name_b = PyString::intern(py, "b");
            if ffi::PyObject_SetAttr(o, name_a.as_ptr(), sentinel.as_ptr()) < 0 {
                return Err(PyErr::fetch(py));
            }
            let values = o.cast::<u8>().add((*tp).tp_basicsize as usize);
            let keys = (*tp.cast::<ffi::PyHeapTypeObject>()).ht_cached_keys.cast::<u8>();
            let dictp = o.cast::<u8>().offset(dict_off).cast::<O>();
            let f = std::mem::transmute::<usize, StringLookupFn>(lookup);
            let mut ok = !keys.is_null()
                && *values >= 2
                && *values.add(3) == 1
                && *values.add(8).cast::<O>() == sentinel.as_ptr()
                && (*dictp).is_null()
                && *keys.add(DK_NENTRIES_OFF).cast::<usize>() == 1
                && f(keys, name_a.as_ptr()) == 0
                && f(keys, name_b.as_ptr()) < 0;
            if ok {
                if ffi::PyObject_SetAttr(o, name_b.as_ptr(), sentinel.as_ptr()) < 0 {
                    return Err(PyErr::fetch(py));
                }
                ok = *keys.add(DK_NENTRIES_OFF).cast::<usize>() == 2 && f(keys, name_b.as_ptr()) == 1;
            }
            if ok {
                // Materialize the dict: the pointer must appear where we read it.
                let d = ffi::PyObject_GenericGetDict(o, std::ptr::null_mut());
                if d.is_null() {
                    return Err(PyErr::fetch(py));
                }
                ok = *dictp == d;
                ffi::Py_DECREF(d);
            }
            ok
        };
        if ok {
            self.dict_off.store(dict_off, Relaxed);
            self.lookup.store(lookup, Relaxed);
            self.ok.store(true, Relaxed);
        }
        Ok(ok)
    }

    #[inline(always)]
    pub fn enabled(&self) -> bool {
        self.ok.load(Relaxed)
    }

    /// The shared keys of `tp` (NULL when it has none, does not use inline values, or the
    /// layout is not calibrated).
    ///
    /// # Safety
    /// Live type.
    #[inline(always)]
    pub unsafe fn shared_keys(&self, tp: *mut ffi::PyTypeObject) -> *mut u8 {
        // SAFETY: heap type field read after the flag checks.
        unsafe {
            if !self.enabled()
                || (*tp).tp_flags & TPFLAGS_INLINE_VALUES == 0
                || (*tp).tp_flags & ffi::Py_TPFLAGS_HEAPTYPE == 0
            {
                return std::ptr::null_mut();
            }
            (*tp.cast::<ffi::PyHeapTypeObject>()).ht_cached_keys.cast()
        }
    }

    /// Entries of shared keys.
    ///
    /// # Safety
    /// Keys from [`Self::shared_keys`].
    #[inline(always)]
    pub unsafe fn nentries(&self, keys: *mut u8) -> usize {
        // SAFETY: calibrated offset.
        unsafe { std::ptr::read_volatile(keys.add(DK_NENTRIES_OFF).cast::<usize>()) }
    }

    /// Is `name` absent from these shared keys?
    ///
    /// # Safety
    /// Keys from [`Self::shared_keys`]; `name` an exact str.
    pub unsafe fn absent(&self, keys: *mut u8, name: O) -> bool {
        // SAFETY: calibrated symbol.
        unsafe { std::mem::transmute::<usize, StringLookupFn>(self.lookup.load(Relaxed))(keys, name) < 0 }
    }

    /// Does `obj` have `name` in its instance `__dict__`? Exact when it answers; `None`
    /// when it cannot tell cheaply (no inline values, a materialized dict, no calibration).
    ///
    /// # Safety
    /// Live object; `name` an exact str.
    #[inline]
    pub unsafe fn has_attr(&self, obj: O, name: O) -> Option<bool> {
        // SAFETY: calibrated layout; index bounded by the values' capacity.
        unsafe {
            let tp = ffi::Py_TYPE(obj);
            let keys = self.shared_keys(tp);
            if keys.is_null() || !self.inline_only(obj) {
                return None;
            }
            let ix = std::mem::transmute::<usize, StringLookupFn>(self.lookup.load(Relaxed))(keys, name);
            if ix < 0 {
                return Some(false);
            }
            let values = obj.cast::<u8>().add((*tp).tp_basicsize as usize);
            if ix as usize >= usize::from(std::ptr::read_volatile(values)) {
                return Some(false);
            }
            Some(!std::ptr::read_volatile(values.add(8).cast::<O>().add(ix as usize)).is_null())
        }
    }

    /// Does `obj` (of an inline-values type) use only its inline values (valid, no
    /// materialized dict)?
    ///
    /// # Safety
    /// Live object of an inline-values type; layout calibrated.
    #[inline(always)]
    pub unsafe fn inline_only(&self, obj: O) -> bool {
        // SAFETY: calibrated offsets.
        unsafe {
            let tp = ffi::Py_TYPE(obj);
            let values = obj.cast::<u8>().add((*tp).tp_basicsize as usize);
            let dictp = obj.cast::<u8>().offset(self.dict_off.load(Relaxed)).cast::<O>();
            std::ptr::read_volatile(values.add(3)) == 1 && std::ptr::read_volatile(dictp).is_null()
        }
    }
}

impl InstLayout {
    /// Is `obj` free of instance attributes named like the names that were absent from
    /// its type's shared keys `keys` when they had `nentries` entries (`keys == 0`: never)?
    ///
    /// # Safety
    /// Live object whose type gave `keys` (via [`InstLayout::shared_keys`]).
    #[inline(always)]
    pub unsafe fn clean(&self, obj: O, keys: usize, nentries: usize) -> bool {
        // SAFETY: per the caller.
        unsafe {
            keys != 0
                && (*ffi::Py_TYPE(obj).cast::<ffi::PyHeapTypeObject>()).ht_cached_keys as usize == keys
                && self.nentries(keys as *mut u8) == nentries
                && self.inline_only(obj)
        }
    }
}

/// `tp_version_tag` (0: none assigned; changes whenever the type or a base is modified).
///
/// # Safety
/// Live type.
#[inline(always)]
pub unsafe fn version_tag(tp: *mut ffi::PyTypeObject) -> u32 {
    // SAFETY: a u32 field CPython updates atomically.
    unsafe { std::ptr::read_volatile(&raw const (*tp).tp_version_tag) }
}

/// One-entry cache of a per-type classification `(info, shared keys, nentries)` keyed by
/// (type, `tp_version_tag`): a seqlock (readers never block; a writer that loses a race
/// does not cache).
pub struct TypeCache {
    seq: AtomicU64,
    tp: AtomicUsize,
    ver: AtomicU32,
    info: AtomicU32,
    keys: AtomicUsize,
    nentries: AtomicUsize,
}

impl TypeCache {
    pub const fn new() -> Self {
        TypeCache {
            seq: AtomicU64::new(0),
            tp: AtomicUsize::new(0),
            ver: AtomicU32::new(0),
            info: AtomicU32::new(0),
            keys: AtomicUsize::new(0),
            nentries: AtomicUsize::new(0),
        }
    }

    #[inline(always)]
    pub fn get(&self, tp: usize, ver: u32) -> Option<(u32, usize, usize)> {
        let s1 = self.seq.load(Ordering::Acquire);
        if s1 & 1 != 0 {
            return None;
        }
        let (t, v, info, keys, nentries) = (
            self.tp.load(Relaxed),
            self.ver.load(Relaxed),
            self.info.load(Relaxed),
            self.keys.load(Relaxed),
            self.nentries.load(Relaxed),
        );
        std::sync::atomic::fence(Ordering::Acquire);
        if self.seq.load(Relaxed) != s1 || t != tp || v != ver {
            return None;
        }
        Some((info, keys, nentries))
    }

    pub fn put(&self, tp: usize, ver: u32, info: u32, keys: usize, nentries: usize) {
        let s = self.seq.load(Relaxed);
        if s & 1 != 0 || self.seq.compare_exchange(s, s + 1, Ordering::Acquire, Relaxed).is_err() {
            return;
        }
        self.tp.store(tp, Relaxed);
        self.ver.store(ver, Relaxed);
        self.info.store(info, Relaxed);
        self.keys.store(keys, Relaxed);
        self.nentries.store(nentries, Relaxed);
        self.seq.store(s + 2, Ordering::Release);
    }
}

impl Default for TypeCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Does `self.<name>` resolve to the class attribute `expect` (not shadowed by the
/// instance)? Exact `LOAD_ATTR` semantics through `_PyObject_GetMethod`; pure when
/// `expect` is a function or method descriptor (no `__get__` runs) and the type's
/// `tp_getattro` is the generic one (callers check).
///
/// # Safety
/// Attached thread; symbols prepared.
#[inline]
pub unsafe fn unshadowed(slf: O, name: O, expect: O) -> bool {
    // SAFETY: GetMethod returns a new reference or NULL with an error.
    unsafe {
        let mut m: O = std::ptr::null_mut();
        let found = GET_METHOD.get()(slf, name, &mut m);
        if m.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        let ok = found == 1 && m == expect;
        ffi::Py_DECREF(m);
        ok
    }
}

/// # Safety
/// Live type.
#[inline(always)]
pub unsafe fn generic_getattro(tp: *mut ffi::PyTypeObject) -> bool {
    // SAFETY: per the caller.
    unsafe { (*tp).tp_getattro.map(|f| f as usize) == Some(ffi::PyObject_GenericGetAttr as *const () as usize) }
}

// ---------------------------------------------------------------------------------------
// Small helpers for kernels.
// ---------------------------------------------------------------------------------------

/// A new reference to an interned str kept for the life of the process.
pub fn interned(py: Python<'_>, s: &str) -> O {
    PyString::intern(py, s).into_ptr()
}

/// Store a strong reference in a static slot (the previous one, if any, is leaked: kernels
/// may still read it on another thread).
pub fn keep(slot: &AtomicPtr<ffi::PyObject>, obj: &Bound<'_, PyAny>) {
    slot.store(obj.clone().into_ptr(), Relaxed);
}

/// Value of an `int` (any subclass, e.g. `bool`, `IntEnum`: read without calling Python
/// code, as `operator.index` does) that fits an `i64`.
///
/// # Safety
/// Live object; attached thread.
#[inline(always)]
pub unsafe fn int_value(o: O) -> Option<i64> {
    // SAFETY: int objects only; overflow reported through the flag (no error set).
    unsafe {
        if ffi::PyLong_Check(o) == 0 {
            return None;
        }
        let mut overflow: c_int = 0;
        let v = ffi::PyLong_AsLongLongAndOverflow(o, &mut overflow);
        if overflow != 0 {
            return None;
        }
        if v == -1 && !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(v)
    }
}

/// An `int` object for an `i128` (small ints come from CPython's cache).
///
/// # Safety
/// Attached thread.
#[inline]
pub unsafe fn long_from_i128(v: i128) -> O {
    // SAFETY: plain constructors.
    unsafe {
        if let Ok(x) = i64::try_from(v) {
            ffi::PyLong_FromLongLong(x)
        } else {
            let bytes = v.to_ne_bytes();
            ffi::PyLong_FromNativeBytes(bytes.as_ptr().cast(), 16, -1)
        }
    }
}

/// Run `f` holding `o`'s critical section on free-threaded builds (nothing on GIL builds,
/// where no other thread runs while the kernel executes native code).
///
/// # Safety
/// Attached thread; `f` must not run Python code.
#[inline(always)]
pub unsafe fn with_cs<R>(o: O, f: impl FnOnce() -> R) -> R {
    #[cfg(Py_GIL_DISABLED)]
    {
        let mut cs = std::mem::MaybeUninit::<ffi::PyCriticalSection>::uninit();
        // SAFETY: the section lives on this frame until `End`.
        unsafe {
            ffi::PyCriticalSection_Begin(cs.as_mut_ptr(), o);
            let r = f();
            ffi::PyCriticalSection_End(cs.as_mut_ptr());
            r
        }
    }
    #[cfg(not(Py_GIL_DISABLED))]
    {
        let _ = o;
        f()
    }
}

/// [`with_cs`] for two objects (`b` may be NULL or equal to `a`).
///
/// # Safety
/// As [`with_cs`].
#[inline(always)]
pub unsafe fn with_cs2<R>(a: O, b: O, f: impl FnOnce() -> R) -> R {
    #[cfg(Py_GIL_DISABLED)]
    {
        // SAFETY: as above.
        unsafe {
            if b.is_null() || a == b {
                return with_cs(a, f);
            }
            let mut cs = std::mem::MaybeUninit::<ffi::PyCriticalSection2>::uninit();
            ffi::PyCriticalSection2_Begin(cs.as_mut_ptr(), a, b);
            let r = f();
            ffi::PyCriticalSection2_End(cs.as_mut_ptr());
            r
        }
    }
    #[cfg(not(Py_GIL_DISABLED))]
    {
        let _ = (a, b);
        f()
    }
}

/// Bind a vectorcall's arguments to parameters: positional ones first, then keywords by
/// name (a NULL name: positional-only, e.g. `self`). `None`: an unknown keyword, a duplicate, or too many positionals (the original
/// raises). Slots of unbound parameters are NULL.
///
/// # Safety
/// Vectorcall arguments; `names` are interned str.
#[inline]
pub unsafe fn bind<const K: usize>(args: *const O, npos: usize, kw: O, names: [O; K]) -> Option<[O; K]> {
    // SAFETY: kwnames tuple of str and its values after the positional arguments.
    unsafe {
        if npos > K {
            return None;
        }
        let mut out = [std::ptr::null_mut(); K];
        for (i, slot) in out.iter_mut().enumerate().take(npos) {
            *slot = *args.add(i);
        }
        if kw.is_null() {
            return Some(out);
        }
        for i in 0..ffi::PyTuple_GET_SIZE(kw) as usize {
            let k = ffi::PyTuple_GET_ITEM(kw, i as ffi::Py_ssize_t);
            let slot = (0..K).find(|&j| {
                let n = names[j];
                !n.is_null() && (k == n || ffi::PyUnicode_Compare(k, n) == 0)
            })?;
            if !out[slot].is_null() {
                return None;
            }
            out[slot] = *args.add(npos + i);
        }
        Some(out)
    }
}
