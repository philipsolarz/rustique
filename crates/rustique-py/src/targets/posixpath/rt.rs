//! Runtime shared by the `posixpath`, `fnmatch` and `pathlib` groups: inline access to
//! `str`/`bytes` buffers, result builders that keep CPython's identity rules, and the
//! per-target vectorcall plumbing (guards, counters, fallback) on top of `crate::accel`.

use std::ffi::c_void;
use std::sync::OnceLock;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple};

use crate::accel::{self, Counters, DepFlag, Fingerprints, Prepare, Reason, Refusal, Tools};
use crate::refuse;
pub use rustique_core::paths::Unit;

pub type O = *mut ffi::PyObject;

// ---------------------------------------------------------------------------------------
// str / bytes buffers
// ---------------------------------------------------------------------------------------

/// Where the kind/compact/ascii bits of `PyASCIIObject.state` live and where compact data
/// starts (measured at install against `PyUnicode_KIND`/`PyUnicode_DATA`, which are
/// out-of-line calls on 3.14+; same method as `takeover.rs`).
#[derive(Clone, Copy, Debug)]
struct StrLayout {
    kind_shift: u32,
    compact_mask: u32,
    ascii_mask: u32,
    ascii_data: usize,
    compact_data: usize,
}

static STR_LAYOUT: OnceLock<Option<StrLayout>> = OnceLock::new();

fn calibrate_str(py: Python<'_>) -> Option<StrLayout> {
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

/// Calibrate once; true if the inline layout is usable (otherwise the out-of-line API is).
pub fn init_str_layout(py: Python<'_>) -> bool {
    STR_LAYOUT.get_or_init(|| calibrate_str(py)).is_some()
}

/// # Safety
/// `op` is a `str` (exact or subclass).
#[inline(always)]
unsafe fn str_state(op: O) -> u32 {
    // SAFETY: the kind/compact/ascii bits never change after creation; volatile because
    // free-threaded builds update the `interned` bits of the same word atomically.
    unsafe { std::ptr::read_volatile(&raw const (*op.cast::<ffi::PyASCIIObject>()).state) }
}

/// The code units of a `str`.
#[derive(Clone, Copy, Debug)]
pub enum Text<'a> {
    U1(&'a [u8]),
    U2(&'a [u16]),
    U4(&'a [u32]),
}

/// A `str` buffer and its `PyUnicode_MAX_CHAR_VALUE` class.
#[derive(Clone, Copy, Debug)]
pub struct StrView<'a> {
    pub text: Text<'a>,
    pub maxchar: u32,
}

impl StrView<'_> {
    #[inline(always)]
    pub fn len(&self) -> usize {
        match self.text {
            Text::U1(s) => s.len(),
            Text::U2(s) => s.len(),
            Text::U4(s) => s.len(),
        }
    }
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Run a generic `fn(&[T]) -> R` on a [`Text`].
#[macro_export]
macro_rules! with_text {
    ($t:expr, |$s:ident| $body:expr) => {
        match $t {
            $crate::targets::posixpath::rt::Text::U1($s) => $body,
            $crate::targets::posixpath::rt::Text::U2($s) => $body,
            $crate::targets::posixpath::rt::Text::U4($s) => $body,
        }
    };
}

/// View of a `str` (exact or subclass) buffer.
///
/// # Safety
/// `op` is a live `str`; the view must not outlive it.
#[inline(always)]
pub unsafe fn str_view<'a>(op: O) -> StrView<'a> {
    // SAFETY: per the caller; offsets verified by `calibrate_str`.
    unsafe {
        let len = (*op.cast::<ffi::PyASCIIObject>()).length as usize;
        let (kind, data, ascii) = match STR_LAYOUT.get() {
            Some(Some(l)) => {
                let st = str_state(op);
                if st & l.compact_mask != 0 {
                    let ascii = st & l.ascii_mask != 0;
                    let off = if ascii { l.ascii_data } else { l.compact_data };
                    ((st >> l.kind_shift) & 7, op.cast::<u8>().add(off) as *const c_void, ascii)
                } else {
                    let ascii = st & l.ascii_mask != 0;
                    ((st >> l.kind_shift) & 7, ffi::PyUnicode_DATA(op) as *const c_void, ascii)
                }
            }
            _ => {
                let kind = ffi::PyUnicode_KIND(op);
                let data = ffi::PyUnicode_DATA(op) as *const c_void;
                let ascii = kind == 1 && std::slice::from_raw_parts(data.cast::<u8>(), len).is_ascii();
                (kind, data, ascii)
            }
        };
        match kind {
            1 => StrView {
                text: Text::U1(std::slice::from_raw_parts(data.cast(), len)),
                maxchar: if ascii { 0x7f } else { 0xff },
            },
            2 => StrView { text: Text::U2(std::slice::from_raw_parts(data.cast(), len)), maxchar: 0xffff },
            _ => StrView { text: Text::U4(std::slice::from_raw_parts(data.cast(), len)), maxchar: 0x10ffff },
        }
    }
}

/// The bytes of a `bytes` (exact or subclass).
///
/// # Safety
/// `op` is a live `bytes`; the slice must not outlive it.
#[inline(always)]
pub unsafe fn bytes_view<'a>(op: O) -> &'a [u8] {
    // SAFETY: per the caller.
    unsafe {
        let n = ffi::Py_SIZE(op) as usize;
        std::slice::from_raw_parts(ffi::PyBytes_AS_STRING(op).cast::<u8>(), n)
    }
}

/// # Safety
/// Live object.
#[inline(always)]
pub unsafe fn exact_str(o: O) -> bool {
    // SAFETY: live object.
    unsafe { ffi::Py_TYPE(o) == &raw mut ffi::PyUnicode_Type }
}

/// # Safety
/// Live object.
#[inline(always)]
pub unsafe fn exact_bytes(o: O) -> bool {
    // SAFETY: live object.
    unsafe { ffi::Py_TYPE(o) == &raw mut ffi::PyBytes_Type }
}

/// `p[start:end]` of a `str`, exactly as slicing does it (`PyUnicode_Substring`: the
/// object itself for the whole of an exact str, the empty and latin-1 singletons).
///
/// # Safety
/// `op` is a live `str`, `start <= end <= len`.
#[inline(always)]
pub unsafe fn substr(op: O, start: usize, end: usize) -> O {
    // SAFETY: per the caller.
    unsafe { ffi::PyUnicode_Substring(op, start as ffi::Py_ssize_t, end as ffi::Py_ssize_t) }
}

/// `p[start:end]` of a `bytes`, as `bytes_subscript` does it (the object itself for the
/// whole of an exact bytes; `PyBytes_FromStringAndSize` otherwise, whose empty and
/// one-byte results are singletons).
///
/// # Safety
/// `op` is a live `bytes`, `start <= end <= len`.
#[inline(always)]
pub unsafe fn subbytes(op: O, start: usize, end: usize) -> O {
    // SAFETY: per the caller.
    unsafe {
        let s = bytes_view(op);
        if start == 0 && end == s.len() && exact_bytes(op) {
            return ffi::Py_NewRef(op);
        }
        ffi::PyBytes_FromStringAndSize(s.as_ptr().add(start).cast(), (end - start) as ffi::Py_ssize_t)
    }
}

/// Copy code units into the buffer of a fresh string of `kind` at `at`.
///
/// # Safety
/// `dst` points to a buffer of the right kind with room for `at + src.len()` units, and
/// every code point of `src` fits `kind`.
#[inline(always)]
pub unsafe fn write_units<S: Unit>(dst: *mut c_void, kind: u32, at: usize, src: &[S]) {
    // SAFETY: per the caller; same-size copies are plain memcpy.
    unsafe {
        let n = src.len();
        match kind {
            1 if size_of::<S>() == 1 => {
                std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u8>(), dst.cast::<u8>().add(at), n)
            }
            2 if size_of::<S>() == 2 => {
                std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u16>(), dst.cast::<u16>().add(at), n)
            }
            4 if size_of::<S>() == 4 => {
                std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u32>(), dst.cast::<u32>().add(at), n)
            }
            1 => {
                let d = dst.cast::<u8>().add(at);
                for (k, &c) in src.iter().enumerate() {
                    *d.add(k) = c.cp() as u8;
                }
            }
            2 => {
                let d = dst.cast::<u16>().add(at);
                for (k, &c) in src.iter().enumerate() {
                    *d.add(k) = c.cp() as u16;
                }
            }
            _ => {
                let d = dst.cast::<u32>().add(at);
                for (k, &c) in src.iter().enumerate() {
                    *d.add(k) = c.cp();
                }
            }
        }
    }
}

/// [`write_units`] for a [`Text`].
///
/// # Safety
/// As [`write_units`].
#[inline(always)]
pub unsafe fn write_text(dst: *mut c_void, kind: u32, at: usize, src: Text<'_>) {
    // SAFETY: forwarded.
    unsafe {
        match src {
            Text::U1(s) => write_units(dst, kind, at, s),
            Text::U2(s) => write_units(dst, kind, at, s),
            Text::U4(s) => write_units(dst, kind, at, s),
        }
    }
}

/// A fresh, uninitialised `str` of `len` code points whose kind and ascii flag follow
/// `maxchar` (as `PyUnicode_New`): `(object, data, kind)`; null object on MemoryError.
///
/// # Safety
/// Attached thread. The caller fills every code point before exposing the object.
#[inline(always)]
pub unsafe fn new_str(len: usize, maxchar: u32) -> (O, *mut c_void, u32) {
    // SAFETY: attached thread.
    unsafe {
        let o = ffi::PyUnicode_New(len as ffi::Py_ssize_t, maxchar);
        if o.is_null() {
            return (o, std::ptr::null_mut(), 0);
        }
        let v = str_view(o);
        let (data, kind) = match v.text {
            Text::U1(s) => (s.as_ptr() as *mut c_void, 1),
            Text::U2(s) => (s.as_ptr() as *mut c_void, 2),
            Text::U4(s) => (s.as_ptr() as *mut c_void, 4),
        };
        (o, data, kind)
    }
}

/// A `str` from code points that are not all ASCII-known in advance: the kind follows the
/// actual maximum (as `PyUnicode_FromKindAndData`/`_PyUnicode_FromUCS*`), so one-character
/// latin-1 results are the shared singletons.
///
/// # Safety
/// Attached thread.
pub unsafe fn str_from_units<T: Unit>(units: &[T]) -> O {
    // SAFETY: attached thread; the buffer has the size passed.
    unsafe {
        let kind = match size_of::<T>() {
            1 => ffi::PyUnicode_1BYTE_KIND,
            2 => ffi::PyUnicode_2BYTE_KIND,
            _ => ffi::PyUnicode_4BYTE_KIND,
        };
        ffi::PyUnicode_FromKindAndData(kind as std::ffi::c_int, units.as_ptr().cast(), units.len() as ffi::Py_ssize_t)
    }
}

// ---------------------------------------------------------------------------------------
// Per-target run-time state, guards and fallback
// ---------------------------------------------------------------------------------------

/// Run-time state of one vectorcall target, read by its kernel.
pub struct Rt {
    pub counters: Counters,
    pub tools: Tools,
    pub deps: DepFlag,
}

impl Rt {
    pub const fn new() -> Self {
        Rt { counters: Counters::new(), tools: Tools::new(), deps: DepFlag::new() }
    }

    /// Call the original (`_PyFunction_Vectorcall`) with the unchanged arguments.
    ///
    /// # Safety
    /// Vectorcall arguments as received by the kernel.
    #[inline(always)]
    pub unsafe fn fallback(&self, why: Reason, func: O, args: *const O, nargsf: usize, kw: O) -> O {
        // Consecutive fallbacks (except Deps) suspend the target (adaptive suspension).
        self.counters.fallback_streak(why);
        // SAFETY: set by the registry before any kernel is reachable.
        unsafe { accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kw) }
    }

    /// Dependencies intact and no tool listening (globally or on this function's code).
    ///
    /// # Safety
    /// Attached thread; `func` is the patched function.
    #[inline(always)]
    pub unsafe fn ready(&self, func: O) -> Result<(), Reason> {
        if !self.deps.ok() {
            return Err(Reason::Deps);
        }
        // SAFETY: attached thread; `func` is a function object.
        if unsafe { !accel::tools_quiet(&self.tools, func) } {
            return Err(Reason::Tools);
        }
        Ok(())
    }
}

impl Default for Rt {
    fn default() -> Self {
        Self::new()
    }
}

#[inline(always)]
pub fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

/// Strong reference kept for the life of the process (constants returned by identity).
pub fn leak(o: &Bound<'_, PyAny>) -> usize {
    o.clone().into_ptr() as usize
}

/// The constant of `func.__code__.co_consts` equal to `value` with exactly `value`'s type.
pub fn code_const<'py>(func: &Bound<'py, PyAny>, value: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyAny>>> {
    let consts = func.getattr("__code__")?.getattr("co_consts")?;
    for c in consts.cast_into::<PyTuple>()?.iter() {
        if c.get_type().is(value.get_type()) && c.eq(value)? {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// Require the recorded fingerprint of `obj` (named `name` in `table`).
pub fn check_fp(
    cx: &mut Prepare<'_>,
    table: &[(&str, Fingerprints)],
    name: &str,
    obj: &Bound<'_, PyAny>,
) -> Result<(), Refusal> {
    let fps = table
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, f)| *f)
        .ok_or_else(|| refuse!("{name}: no fingerprint table"))?;
    cx.fingerprint_code(obj, fps)
}

/// The module dict of an already imported module (never imports).
pub fn module_dict<'py>(cx: &Prepare<'py>, name: &str) -> Result<Bound<'py, PyDict>, Refusal> {
    Ok(cx.loaded_module(name)?.dict())
}

/// Require the recorded `ml_name`/`ml_flags`/`ml_doc` fingerprint of a C function.
pub fn check_c(
    cx: &mut Prepare<'_>,
    table: &[(&str, Fingerprints)],
    name: &str,
    obj: &Bound<'_, PyAny>,
) -> Result<(), Refusal> {
    let fps = table
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, f)| *f)
        .ok_or_else(|| refuse!("{name}: no fingerprint table"))?;
    cx.fingerprint_c(obj, fps)
}

// ---------------------------------------------------------------------------------------
// "Plain" str subclasses: the operations CPython's path code applies to a str subclass
// instance (method calls `startswith`/`endswith`/`rfind`, slicing, concatenation,
// truthiness, `isinstance` reading `__class__`) run no Python code and give str's results
// when the subclass overrides none of them and the instance's `__dict__` is empty (an
// instance attribute would shadow a method).
// ---------------------------------------------------------------------------------------

unsafe extern "C" {
    fn _PyType_Lookup(ty: *mut ffi::PyTypeObject, name: *mut ffi::PyObject) -> *mut ffi::PyObject;
    /// Public since 3.13: visits a managed `__dict__`, or the inline values (attributes
    /// stored without a dict object) when there is none.
    fn PyObject_VisitManagedDict(
        obj: *mut ffi::PyObject,
        visit: ffi::visitproc,
        arg: *mut std::ffi::c_void,
    ) -> std::ffi::c_int;
}

unsafe extern "C" fn count_visit(_o: *mut ffi::PyObject, arg: *mut std::ffi::c_void) -> std::ffi::c_int {
    // SAFETY: `arg` is the `usize` counter passed by `has_inline_attrs`.
    unsafe { *arg.cast::<usize>() += 1 };
    0
}

/// Whether a managed-dict instance without a dict object has attributes in its inline
/// values.
///
/// # Safety
/// Attached thread; live instance of a managed-dict type.
unsafe fn has_inline_attrs(op: O) -> bool {
    let mut n = 0usize;
    // SAFETY: per the caller; the visitor only counts.
    unsafe { PyObject_VisitManagedDict(op, count_visit, (&raw mut n).cast()) };
    n != 0
}

/// Methods looked up on str subclass instances by the path functions (and `__class__`, read
/// by `isinstance`), with the descriptors `str` / `object` resolve them to.
const PLAIN_NAMES: [&str; 4] = ["startswith", "endswith", "rfind", "__class__"];

struct PlainInfo {
    names: [(usize, usize); PLAIN_NAMES.len()],
    /// Where a managed `__dict__` pointer lives relative to the object.
    managed_dict_offset: isize,
}

static PLAIN: OnceLock<Option<PlainInfo>> = OnceLock::new();

const MANAGED_DICT: std::ffi::c_ulong = 1 << 4;

fn calibrate_plain(py: Python<'_>) -> PyResult<Option<PlainInfo>> {
    let mut names = [(0usize, 0usize); PLAIN_NAMES.len()];
    for (k, n) in PLAIN_NAMES.iter().enumerate() {
        let name = PyString::intern(py, n);
        // SAFETY: static type; interned name; borrowed result kept alive by the type.
        let d = unsafe { _PyType_Lookup(&raw mut ffi::PyUnicode_Type, name.as_ptr()) };
        if !d.is_null() {
            // SAFETY: live descriptor: keep a reference for the process lifetime.
            unsafe { ffi::Py_IncRef(d) };
        }
        names[k] = (name.clone().into_ptr() as usize, d as usize);
    }
    // Probe: a str subclass instance with one attribute; its managed dict must be found at
    // MANAGED_DICT_OFFSET (pycore_object.h: -3 words on GIL builds, -1 free-threaded).
    let off: isize =
        if cfg!(Py_GIL_DISABLED) { -(size_of::<usize>() as isize) } else { -3 * size_of::<usize>() as isize };
    let ty = py.get_type::<pyo3::types::PyType>().call1((
        "_RustiquePlainProbe",
        (py.get_type::<PyString>(),),
        PyDict::new(py),
    ))?;
    // SAFETY: a heap type object.
    let flags = unsafe { ffi::PyType_GetFlags(ty.as_ptr().cast::<ffi::PyTypeObject>()) };
    if flags & MANAGED_DICT == 0 {
        return Ok(None);
    }
    let inst = ty.call1(("x",))?;
    // SAFETY: a live instance of a managed-dict type.
    let read = |o: &Bound<'_, PyAny>| unsafe { *(o.as_ptr().cast::<u8>().offset(off) as *const usize) };
    // SAFETY: live instances of a managed-dict type.
    let (empty_inline, set_inline) = unsafe {
        let e = has_inline_attrs(inst.as_ptr());
        inst.setattr("a", 1)?;
        (e, has_inline_attrs(inst.as_ptr()))
    };
    let before = read(&inst);
    let d = inst.getattr("__dict__")?;
    let after = read(&inst);
    // No attribute → nothing visited; an attribute is seen either in the inline values
    // (no dict object yet) or in the dict; touching `__dict__` materializes it at `off`.
    if empty_inline || !(set_inline || before != 0) || after != d.as_ptr() as usize {
        return Ok(None);
    }
    Ok(Some(PlainInfo { names, managed_dict_offset: off }))
}

/// Calibrate once; false if plain-subclass support is unavailable (subclasses fall back).
pub fn init_plain(py: Python<'_>) -> bool {
    PLAIN.get_or_init(|| calibrate_plain(py).ok().flatten()).is_some()
}

macro_rules! same_sub_slot {
    ($tp:expr, $base:expr, $table:ident . $slot:ident) => {{
        let a = (*$tp).$table;
        let b = (*$base).$table;
        if a.is_null() || b.is_null() {
            a.is_null() && b.is_null()
        } else {
            (*a).$slot.map(|f| f as usize) == (*b).$slot.map(|f| f as usize)
        }
    }};
}

/// A `str` subclass instance on which the path functions' operations are str's own.
///
/// # Safety
/// Attached thread; `op` is a live `str` subclass instance (`PyUnicode_Check`, not exact).
#[inline(never)]
pub unsafe fn plain_str_subclass(op: O) -> bool {
    let Some(Some(info)) = PLAIN.get() else { return false };
    // SAFETY: per the caller; type objects are live; descriptors only compared.
    unsafe {
        let tp = ffi::Py_TYPE(op);
        let base = &raw mut ffi::PyUnicode_Type;
        if (*tp).tp_getattro.map(|f| f as usize) != (*base).tp_getattro.map(|f| f as usize)
            || (*tp).tp_richcompare.map(|f| f as usize) != (*base).tp_richcompare.map(|f| f as usize)
            || !same_sub_slot!(tp, base, tp_as_number.nb_bool)
            || !same_sub_slot!(tp, base, tp_as_number.nb_add)
            || !same_sub_slot!(tp, base, tp_as_number.nb_inplace_add)
            || !same_sub_slot!(tp, base, tp_as_sequence.sq_concat)
            || !same_sub_slot!(tp, base, tp_as_sequence.sq_inplace_concat)
            || !same_sub_slot!(tp, base, tp_as_sequence.sq_length)
            || !same_sub_slot!(tp, base, tp_as_mapping.mp_length)
            || !same_sub_slot!(tp, base, tp_as_mapping.mp_subscript)
        {
            return false;
        }
        for &(name, descr) in &info.names {
            if _PyType_Lookup(tp, name as O) as usize != descr {
                return false;
            }
        }
        // No instance attribute may shadow a method: the instance dict is absent or empty.
        let flags = ffi::PyType_GetFlags(tp);
        let dict = if flags & MANAGED_DICT != 0 {
            let cell =
                op.cast::<u8>().offset(info.managed_dict_offset) as *const std::sync::atomic::AtomicPtr<ffi::PyObject>;
            (*cell).load(std::sync::atomic::Ordering::Acquire)
        } else if (*tp).tp_dictoffset == 0 {
            std::ptr::null_mut()
        } else if (*tp).tp_dictoffset > 0 {
            let cell =
                op.cast::<u8>().offset((*tp).tp_dictoffset) as *const std::sync::atomic::AtomicPtr<ffi::PyObject>;
            (*cell).load(std::sync::atomic::Ordering::Acquire)
        } else {
            return false;
        };
        if dict.is_null() {
            // Attributes may live in the inline values without a dict object.
            flags & MANAGED_DICT == 0 || !has_inline_attrs(op)
        } else {
            ffi::PyDict_Size(dict) == 0
        }
    }
}

/// The str-family view of a path argument: exact str, or a plain str subclass.
///
/// # Safety
/// Attached thread; live object.
#[inline(always)]
pub unsafe fn str_like(op: O) -> bool {
    // SAFETY: per the caller.
    unsafe { exact_str(op) || (ffi::PyUnicode_Check(op) != 0 && plain_str_subclass(op)) }
}

// ---------------------------------------------------------------------------------------
// Kernel skeleton
// ---------------------------------------------------------------------------------------

/// Sentinels a kernel body returns for "guard failed" (never valid object pointers).
pub const GUARD_DEPS: O = 1 as O;
pub const GUARD_TOOLS: O = 2 as O;

/// `Some(sentinel)` when a guard fails.
///
/// # Safety
/// Attached thread; `func` is the patched function.
#[inline(always)]
pub unsafe fn guards(rt: &Rt, func: O) -> Option<O> {
    // SAFETY: attached thread.
    match unsafe { rt.ready(func) } {
        Ok(()) => None,
        Err(Reason::Deps) => Some(GUARD_DEPS),
        Err(_) => Some(GUARD_TOOLS),
    }
}

/// A vectorcall kernel: the body evaluates to `None` (outside the exact subset: decline
/// before anything observable), `Some(GUARD_*)` (a guard failed), `Some(null)` (an error
/// before anything observable: cleared, CPython re-runs and raises), or `Some(result)`.
/// Successful results go through check mode when it is on.
#[macro_export]
macro_rules! paths_kernel {
    ($name:ident, $rt:ident, $spec:ident, |$func:ident, $args:ident, $nargsf:ident, $n:ident, $kw:ident| $body:expr) => {
        // The body is the kernel's own code (one unsafe region per kernel).
        #[allow(clippy::macro_metavars_in_unsafe)]
        unsafe extern "C" fn $name(
            $func: *mut pyo3::ffi::PyObject,
            $args: *const *mut pyo3::ffi::PyObject,
            $nargsf: usize,
            $kw: *mut pyo3::ffi::PyObject,
        ) -> *mut pyo3::ffi::PyObject {
            use $crate::accel::Reason;
            use $crate::targets::posixpath::rt::{GUARD_DEPS, GUARD_TOOLS};
            // SAFETY: vectorcall arguments from CPython; attached thread.
            unsafe {
                let $n = $crate::targets::posixpath::rt::nargs($nargsf);
                let r: Option<*mut pyo3::ffi::PyObject> = $body;
                match r {
                    None => $rt.fallback(Reason::Args, $func, $args, $nargsf, $kw),
                    Some(r) if r == GUARD_DEPS => $rt.fallback(Reason::Deps, $func, $args, $nargsf, $kw),
                    Some(r) if r == GUARD_TOOLS => $rt.fallback(Reason::Tools, $func, $args, $nargsf, $kw),
                    Some(r) if r.is_null() => {
                        pyo3::ffi::PyErr_Clear();
                        $rt.fallback(Reason::Deopt, $func, $args, $nargsf, $kw)
                    }
                    Some(r) => {
                        if $rt.counters.hit_check_streak() {
                            let nkw = if $kw.is_null() { 0 } else { pyo3::ffi::PyTuple_GET_SIZE($kw) as usize };
                            let all = std::slice::from_raw_parts($args, $n + nkw);
                            return $crate::accel::check::verify(&$spec, r, all, || {
                                $crate::accel::FUNCTION_VECTORCALL.get()($func, $args, $nargsf, $kw)
                            });
                        }
                        r
                    }
                }
            }
        }
    };
}
