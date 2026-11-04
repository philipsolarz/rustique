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

/// Byte offset, from `&PyASCIIObject.state`, of the 32-bit unit that holds the
/// kind/compact/ascii bits. CPython 3.14/3.15 declare `state` as an `unsigned char interned`
/// (free-threaded builds) or `unsigned int interned:2` followed by `unsigned int kind:3`,
/// `compact:1`, `ascii:1`, ... GCC and Clang (SysV/Itanium rules) pack `kind` into the
/// first 32-bit unit on every build; MSVC starts a new storage unit when the declared type
/// changes, so on free-threaded Windows builds the first unit holds only `interned` and
/// three padding bytes CPython never writes, and the bitfields live in the next unit
/// (docs/atlas/w2-dx.md §6). Reading that unit keeps the calibration below searching
/// initialised bits on every ABI; the calibration still verifies the positions.
const STATE_UNIT: usize = if cfg!(all(windows, Py_GIL_DISABLED)) { 4 } else { 0 };

const _: () =
    assert!(std::mem::offset_of!(ffi::PyASCIIObject, state) + STATE_UNIT + 4 <= size_of::<ffi::PyASCIIObject>());

/// # Safety
/// `op` is a `str` (exact or subclass).
#[inline(always)]
unsafe fn str_state(op: O) -> u32 {
    // SAFETY: the unit lies inside the PyASCIIObject header on every ABI (assert above);
    // the kind/compact/ascii bits never change after creation; volatile because
    // free-threaded builds update the `interned` byte of the same word atomically (Unix).
    unsafe {
        let p = (&raw const (*op.cast::<ffi::PyASCIIObject>()).state).cast::<u8>().add(STATE_UNIT);
        std::ptr::read_volatile(p.cast::<u32>())
    }
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
        // `PyUnicode_New` makes a compact string whose kind follows `maxchar`.
        if let Some(Some(l)) = STR_LAYOUT.get() {
            let (off, kind) = match maxchar {
                0..0x80 => (l.ascii_data, 1),
                0x80..0x100 => (l.compact_data, 1),
                0x100..0x10000 => (l.compact_data, 2),
                _ => (l.compact_data, 4),
            };
            return (o, o.cast::<u8>().add(off).cast(), kind);
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
    /// The group's dependency flag (its `GroupSpec` pins everything the group's originals
    /// read at call time).
    pub deps: &'static DepFlag,
}

impl Rt {
    pub const fn new(deps: &'static DepFlag) -> Self {
        Rt { counters: Counters::new(), tools: Tools::new(), deps }
    }

    /// Call the original (`_PyFunction_Vectorcall`) with the unchanged arguments.
    ///
    /// # Safety
    /// Vectorcall arguments as received by the kernel.
    #[inline(always)]
    pub unsafe fn fallback(&self, why: Reason, func: O, args: *const O, nargsf: usize, kw: O) -> O {
        // Fallbacks (except Deps) debit the net-benefit balance (adaptive suspension).
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

/// Objects whose fingerprint matched, by identity (strong references kept, so a pointer
/// is never reused): targets sharing helpers (`_get_sep`, `os.fspath`, ...) hash them once.
static VERIFIED: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

/// Exit release of the paths groups (`TargetSpec::release`, run by the registry's atexit
/// hook after every site was restored and every flag cleared): drop the references taken
/// on verified objects. Functions keep their module's globals alive, so without this the
/// `posixpath`/`genericpath`/`pathlib` dicts would survive into finalization. Idempotent; a
/// later prepare verifies again.
pub fn release(_py: Python<'_>) {
    let v = std::mem::take(&mut *VERIFIED.lock().unwrap_or_else(|p| p.into_inner()));
    for o in v {
        // SAFETY: attached thread; the strong references taken by `mark_verified`.
        unsafe { ffi::Py_DECREF(o as O) };
    }
}

/// Clear a cell holding a strong reference (from [`leak`]) and drop the reference. Only for
/// cells a kernel reads past its dependency flag and checks for null.
pub fn release_cell(cell: &std::sync::atomic::AtomicUsize) {
    let p = cell.swap(0, std::sync::atomic::Ordering::Relaxed);
    if p != 0 {
        // SAFETY: attached thread; the reference stored with the cell.
        unsafe { ffi::Py_DECREF(p as O) };
    }
}

fn verified(obj: &Bound<'_, PyAny>) -> bool {
    VERIFIED.lock().map(|v| v.contains(&(obj.as_ptr() as usize))).unwrap_or(false)
}

fn mark_verified(obj: &Bound<'_, PyAny>) {
    if let Ok(mut v) = VERIFIED.lock()
        && !v.contains(&(obj.as_ptr() as usize))
    {
        v.push(leak(obj));
    }
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
    if verified(obj) {
        return Ok(());
    }
    cx.fingerprint_code(obj, fps)?;
    mark_verified(obj);
    Ok(())
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
    if verified(obj) {
        return Ok(());
    }
    cx.fingerprint_c(obj, fps)?;
    mark_verified(obj);
    Ok(())
}

// ---------------------------------------------------------------------------------------
// "Plain" str subclasses: the operations CPython's path code applies to a str subclass
// instance (method calls `startswith`/`endswith`/`rfind`, slicing, concatenation,
// truthiness, `isinstance` reading `__class__`) run no Python code and give str's results
// when the subclass overrides none of them and the instance's `__dict__` is empty (an
// instance attribute would shadow a method).
// ---------------------------------------------------------------------------------------

python_extern! {
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

/// Whether a managed-dict instance has a dict object or attributes in its inline values
/// (`PyObject_VisitManagedDict` visits the dict if there is one, else the inline values).
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
}

static PLAIN: OnceLock<Option<PlainInfo>> = OnceLock::new();

const MANAGED_DICT: std::ffi::c_ulong = 1 << 4;

/// The descriptors `str` (and `object`) resolve the looked-up names to. Creates no
/// objects visible to Python (no probe class: it would show in `str.__subclasses__()`).
fn calibrate_plain(py: Python<'_>) -> PyResult<Option<PlainInfo>> {
    let mut names = [(0usize, 0usize); PLAIN_NAMES.len()];
    for (k, n) in PLAIN_NAMES.iter().enumerate() {
        let name = PyString::intern(py, n);
        // SAFETY: static type; interned name; borrowed result kept alive by the type.
        let d = unsafe { _PyType_Lookup(&raw mut ffi::PyUnicode_Type, name.as_ptr()) };
        if d.is_null() {
            return Ok(None);
        }
        // SAFETY: live descriptor: keep a reference for the process lifetime.
        unsafe { ffi::Py_IncRef(d) };
        names[k] = (name.clone().into_ptr() as usize, d as usize);
    }
    Ok(Some(PlainInfo { names }))
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
        // No instance attribute may shadow a method. Managed dicts: nothing to visit (no
        // dict object and no inline values; an instance whose `__dict__` was materialized
        // falls back even when empty). Classic dicts: absent or empty.
        let flags = ffi::PyType_GetFlags(tp);
        if flags & MANAGED_DICT != 0 {
            return !has_inline_attrs(op);
        }
        match (*tp).tp_dictoffset {
            0 => true,
            off if off > 0 => {
                let cell = op.cast::<u8>().offset(off) as *const std::sync::atomic::AtomicPtr<ffi::PyObject>;
                let dict = (*cell).load(std::sync::atomic::Ordering::Acquire);
                dict.is_null() || ffi::PyDict_Size(dict) == 0
            }
            _ => false,
        }
    }
}

/// Run `f` holding `o`'s critical section (free-threaded builds; with the GIL just `f()`),
/// without touching `o`'s reference count: on a path object shared by threads, a
/// `Bound` (incref + decref) per call bounces its refcount line (8-thread rule).
///
/// # Safety
/// Attached thread; live object.
#[inline(always)]
pub unsafe fn with_cs<R>(o: O, f: impl FnOnce() -> R) -> R {
    #[cfg(Py_GIL_DISABLED)]
    // SAFETY: per the caller; the guard ends the section on every exit path.
    unsafe {
        struct End(ffi::PyCriticalSection);
        impl Drop for End {
            fn drop(&mut self) {
                // SAFETY: begun below, on this thread.
                unsafe { ffi::PyCriticalSection_End(&mut self.0) };
            }
        }
        let mut g = End(std::mem::zeroed());
        ffi::PyCriticalSection_Begin(&mut g.0, o);
        f()
    }
    #[cfg(not(Py_GIL_DISABLED))]
    {
        let _ = o;
        f()
    }
}

/// [`with_cs`] for two objects (`Py_BEGIN_CRITICAL_SECTION2`).
///
/// # Safety
/// Attached thread; live objects.
#[inline(always)]
pub unsafe fn with_cs2<R>(a: O, b: O, f: impl FnOnce() -> R) -> R {
    #[cfg(Py_GIL_DISABLED)]
    // SAFETY: per the caller; the guard ends the section on every exit path.
    unsafe {
        struct End(ffi::PyCriticalSection2);
        impl Drop for End {
            fn drop(&mut self) {
                // SAFETY: begun below, on this thread.
                unsafe { ffi::PyCriticalSection2_End(&mut self.0) };
            }
        }
        let mut g = End(std::mem::zeroed());
        ffi::PyCriticalSection2_Begin(&mut g.0, a, b);
        f()
    }
    #[cfg(not(Py_GIL_DISABLED))]
    {
        let _ = (a, b);
        f()
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
/// Successful results go through check mode when it is on and the target is checkable.
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
                        // Check mode re-runs the original only for side-effect-free
                        // targets (`checkable`); pathlib's results are fresh objects.
                        if $rt.counters.hit_check_streak() && $spec.checkable {
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
