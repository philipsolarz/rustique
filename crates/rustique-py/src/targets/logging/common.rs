//! Machinery shared by the logging kernels: fallback, tool and type guards, attribute
//! helpers, purity checks and the interned names the kernels use.
//!
//! Every helper here is *pure* in the contract's sense: it runs no Python code and has no
//! observable effect, so a kernel may still restart in CPython's original after using it.

use std::cell::UnsafeCell;
use std::ffi::c_int;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};

use pyo3::prelude::*;

use pyo3::ffi;

use crate::accel::version::RawCode;

pub type O = *mut ffi::PyObject;
pub type Tp = *mut ffi::PyTypeObject;

// ---------------------------------------------------------------------------------------
// CPython symbols pyo3-ffi does not bind
// ---------------------------------------------------------------------------------------

/// `PyMethodObject` (cpython/classobject.h, public layout).
#[repr(C)]
pub struct RawMethod {
    pub ob_base: ffi::PyObject,
    pub im_func: O,
    pub im_self: O,
}

#[repr(C)]
struct MonState {
    active: u8,
    opaque: u8,
}

unsafe extern "C" {
    pub static mut PyMethod_Type: ffi::PyTypeObject;
    fn PyMonitoring_EnterScope(state: *mut MonState, version: *mut u64, events: *const u8, n: ffi::Py_ssize_t)
    -> c_int;
    /// Exported (cpython/object.h, 3.14 and 3.15): MRO lookup through the type cache.
    pub fn _PyType_LookupRef(t: Tp, name: O) -> O;
    pub fn PyUnstable_Type_AssignVersionTag(t: Tp) -> c_int;
    pub fn PyTime_Time(result: *mut i64) -> c_int;
}

// ---------------------------------------------------------------------------------------
// Fallback
// ---------------------------------------------------------------------------------------

/// Run CPython's original (`_PyFunction_Vectorcall`) with the unchanged arguments.
///
/// # Safety
/// Vectorcall arguments of a function patched through the registry; attached thread.
#[inline(always)]
pub unsafe fn original(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: set by the registry before any kernel is reachable.
    unsafe { crate::accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kw) }
}

#[inline(always)]
pub fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

// ---------------------------------------------------------------------------------------
// Tools guard
// ---------------------------------------------------------------------------------------

/// The ungrouped monitoring events (`_PY_MONITORING_UNGROUPED_EVENTS`, 16 in 3.14 and 3.15).
/// `C_RETURN`/`C_RAISE` follow `CALL`, `BRANCH` follows `BRANCH_LEFT/RIGHT`.
const EVENTS: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// "No `sys.monitoring` tool listens to any global event" (covers `sys.settrace`,
/// `sys.setprofile`, cProfile, coverage, debuggers). Stricter than the framework's
/// `PY_START`-only guard: the logging kernels replace whole call trees of Python code
/// (raises, handled exceptions, lines), so any global event a tool asked for must see
/// CPython's frames.
pub struct Tools {
    version: UnsafeCell<u64>,
    state: UnsafeCell<[MonState; 16]>,
    seen: UnsafeCell<u64>,
    quiet: UnsafeCell<bool>,
}

// SAFETY: written by CPython's API and by `quiet()` with values derived from the same
// interpreter state; concurrent writers store equal values (benign, as in Cython's use).
unsafe impl Sync for Tools {}

impl Tools {
    pub const fn new() -> Self {
        Tools {
            version: UnsafeCell::new(0),
            state: UnsafeCell::new([const { MonState { active: 0, opaque: 0 } }; 16]),
            seen: UnsafeCell::new(u64::MAX),
            quiet: UnsafeCell::new(false),
        }
    }

    /// # Safety
    /// Attached thread.
    #[inline(always)]
    pub unsafe fn quiet(&self) -> bool {
        // SAFETY: pointers into this static; see the type's Sync note.
        unsafe {
            PyMonitoring_EnterScope(self.state.get().cast(), self.version.get(), EVENTS.as_ptr(), 16);
            let v = *self.version.get();
            if v != *self.seen.get() {
                *self.quiet.get() = (*self.state.get()).iter().all(|s| s.active == 0);
                *self.seen.get() = v;
            }
            *self.quiet.get()
        }
    }
}

impl Default for Tools {
    fn default() -> Self {
        Self::new()
    }
}

pub static TOOLS: Tools = Tools::new();

/// No tool listens locally (`sys.monitoring.set_local_events`: breakpoints) on `code`.
/// `_co_monitoring->local_monitors` holds one byte per local event: 11 on 3.14, 16 on 3.15.
///
/// # Safety
/// A live code object; layout validated by `version::code_layout_ok` at install.
#[inline(always)]
pub unsafe fn code_quiet(code: O) -> bool {
    // SAFETY: per the caller.
    unsafe {
        let mon = (*code.cast::<RawCode>())._co_monitoring;
        if mon.is_null() {
            return true;
        }
        let lo = std::ptr::read_unaligned(mon.cast::<u64>());
        #[cfg(Py_3_15)]
        let hi = std::ptr::read_unaligned(mon.add(8).cast::<u64>());
        #[cfg(not(Py_3_15))]
        let hi = std::ptr::read_unaligned(mon.add(8).cast::<u16>()) as u64 | (*mon.add(10) as u64) << 16;
        lo == 0 && hi == 0
    }
}

/// A Python function a kernel stands in for (the patched one or a helper it inlines):
/// its code and defaults are still the ones validated at install (`__code__`/`__defaults__`
/// assignment changes behaviour), and no tool listens locally on the code. Holds strong
/// references for the process lifetime.
pub struct FnGuard {
    func: AtomicUsize,
    code: AtomicUsize,
    defaults: AtomicUsize,
}

impl FnGuard {
    pub const fn new() -> Self {
        FnGuard { func: AtomicUsize::new(0), code: AtomicUsize::new(0), defaults: AtomicUsize::new(0) }
    }

    /// Record `func` (an exact function). Earlier references are leaked on purpose (a
    /// concurrent kernel may still read them).
    pub fn set(&self, func: &Bound<'_, PyAny>) {
        let f = func.as_ptr().cast::<ffi::PyFunctionObject>();
        // SAFETY: an exact function object (checked by the caller); attached thread.
        unsafe {
            let code = (*f).func_code;
            let defaults = (*f).func_defaults;
            ffi::Py_INCREF(func.as_ptr());
            ffi::Py_INCREF(code);
            ffi::Py_XINCREF(defaults);
            self.code.store(code as usize, Relaxed);
            self.defaults.store(defaults as usize, Relaxed);
            self.func.store(func.as_ptr() as usize, Relaxed);
        }
    }

    pub fn func(&self) -> O {
        self.func.load(Relaxed) as O
    }

    /// # Safety
    /// `set` ran; attached thread; code layout calibrated.
    #[inline(always)]
    pub unsafe fn ok(&self) -> bool {
        // SAFETY: per the caller (the function is kept alive by `set`).
        unsafe {
            let f = self.func.load(Relaxed) as *mut ffi::PyFunctionObject;
            let code = self.code.load(Relaxed);
            (*f).func_code as usize == code
                && (*f).func_defaults as usize == self.defaults.load(Relaxed)
                && code_quiet(code as O)
        }
    }
}

impl Default for FnGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// Every function in `g` passes [`FnGuard::ok`].
///
/// # Safety
/// See [`FnGuard::ok`].
#[inline(always)]
pub unsafe fn fns_ok(g: &[FnGuard]) -> bool {
    // SAFETY: per the caller.
    g.iter().all(|f| unsafe { f.ok() })
}

// ---------------------------------------------------------------------------------------
// Validated types
// ---------------------------------------------------------------------------------------

/// Small cache of `(type, tp_version_tag)` pairs a validator accepted. A type's version tag
/// changes whenever the type or a base is modified (attribute assignment, `__bases__`), and
/// tags are never reused within an interpreter, so a matching pair proves the class-level
/// lookups the validator checked still hold. Torn reads on free-threaded builds can only
/// produce false negatives (each tag belongs to one type).
pub struct TypeCache {
    ty: [AtomicUsize; 4],
    tag: [AtomicU32; 4],
    next: AtomicUsize,
    validate: unsafe fn(Tp) -> bool,
}

impl TypeCache {
    pub const fn new(validate: unsafe fn(Tp) -> bool) -> Self {
        TypeCache {
            ty: [const { AtomicUsize::new(0) }; 4],
            tag: [const { AtomicU32::new(0) }; 4],
            next: AtomicUsize::new(0),
            validate,
        }
    }

    /// # Safety
    /// `t` is a live type; attached thread.
    #[inline(always)]
    pub unsafe fn ok(&self, t: Tp) -> bool {
        // SAFETY: field read of a live type (atomic on free-threaded builds).
        let tag = unsafe { version_tag(t) };
        if tag != 0 {
            for i in 0..4 {
                if self.tag[i].load(Relaxed) == tag && self.ty[i].load(Relaxed) == t as usize {
                    return true;
                }
            }
        }
        // SAFETY: per the caller.
        unsafe { self.slow(t) }
    }

    #[cold]
    unsafe fn slow(&self, t: Tp) -> bool {
        // SAFETY: attached thread, live type.
        unsafe {
            let assigned = PyUnstable_Type_AssignVersionTag(t);
            let tag0 = version_tag(t);
            if !(self.validate)(t) {
                return false;
            }
            if assigned == 1 && tag0 != 0 && version_tag(t) == tag0 {
                let i = self.next.fetch_add(1, Relaxed) % 4;
                self.tag[i].store(0, Relaxed);
                self.ty[i].store(t as usize, Relaxed);
                self.tag[i].store(tag0, Relaxed);
            }
            true
        }
    }

    pub fn clear(&self) {
        for i in 0..4 {
            self.tag[i].store(0, Relaxed);
            self.ty[i].store(0, Relaxed);
        }
    }
}

/// # Safety
/// Live type.
#[inline(always)]
pub unsafe fn version_tag(t: Tp) -> u32 {
    // SAFETY: aligned u32 field; CPython writes it atomically on free-threaded builds.
    unsafe { (*(&raw const (*t).tp_version_tag).cast::<AtomicU32>()).load(Relaxed) }
}

/// The class attribute `name` resolves (MRO) to exactly `expected`.
///
/// # Safety
/// Attached thread, live type.
pub unsafe fn resolves_to(t: Tp, name: O, expected: O) -> bool {
    // SAFETY: per the caller.
    unsafe {
        let r = _PyType_LookupRef(t, name);
        if r.is_null() {
            return false;
        }
        ffi::Py_DECREF(r);
        r == expected
    }
}

/// The class attribute `name` is absent or a plain value (no `__get__`/`__set__`), so
/// `obj.name` reads the instance attribute or that value without running code.
///
/// # Safety
/// Attached thread, live type.
pub unsafe fn plain_attr(t: Tp, name: O) -> bool {
    // SAFETY: per the caller.
    unsafe {
        let r = _PyType_LookupRef(t, name);
        if r.is_null() {
            return true;
        }
        let rt = ffi::Py_TYPE(r);
        let ok = (*rt).tp_descr_get.is_none() && (*rt).tp_descr_set.is_none();
        ffi::Py_DECREF(r);
        ok
    }
}

/// Generic attribute protocol (no `__getattribute__`/`__getattr__`/`__setattr__`
/// overrides) and an instance `__dict__`.
///
/// # Safety
/// Live type.
pub unsafe fn generic_attrs(t: Tp, setattr_too: bool) -> bool {
    // SAFETY: field reads of a live type.
    unsafe {
        let ga = (*t).tp_getattro.map(|f| f as usize);
        let sa = (*t).tp_setattro.map(|f| f as usize);
        let has_dict = (*t).tp_dictoffset != 0 || (*t).tp_flags & ffi::Py_TPFLAGS_MANAGED_DICT != 0;
        has_dict
            && ga == Some(ffi::PyObject_GenericGetAttr as *const () as usize)
            && (!setattr_too || sa == Some(ffi::PyObject_GenericSetAttr as *const () as usize))
    }
}

// ---------------------------------------------------------------------------------------
// Attribute helpers
// ---------------------------------------------------------------------------------------

/// `obj.name`, or NULL with no error set. Only for objects of validated types, where the
/// lookup runs no Python code.
///
/// # Safety
/// Attached thread.
#[inline(always)]
pub unsafe fn get(obj: O, name: O) -> O {
    // SAFETY: per the caller.
    unsafe {
        let r = ffi::PyObject_GetAttr(obj, name);
        if r.is_null() {
            ffi::PyErr_Clear();
        }
        r
    }
}

/// `obj.name` is the bound method `func.__get__(obj)`: the instance does not shadow the
/// method its (validated) class provides.
///
/// # Safety
/// Attached thread; `obj` of a validated type.
#[inline(always)]
pub unsafe fn bound_to(obj: O, name: O, func: O) -> bool {
    // SAFETY: per the caller.
    unsafe {
        let m = get(obj, name);
        if m.is_null() {
            return false;
        }
        let ok = ffi::Py_TYPE(m) == &raw mut PyMethod_Type
            && (*m.cast::<RawMethod>()).im_func == func
            && (*m.cast::<RawMethod>()).im_self == obj;
        ffi::Py_DECREF(m);
        ok
    }
}

/// Values whose `str`, `repr`, `%`-conversions and `__format__` are CPython C code with no
/// hooks: exact `str`, `int`, `bool`, `float`, `None`.
///
/// # Safety
/// Live object.
#[inline(always)]
pub unsafe fn pure_scalar(o: O) -> bool {
    // SAFETY: header reads.
    unsafe {
        let t = ffi::Py_TYPE(o);
        t == &raw mut ffi::PyUnicode_Type
            || t == &raw mut ffi::PyLong_Type
            || t == &raw mut ffi::PyFloat_Type
            || o == ffi::Py_None()
            || t == &raw mut ffi::PyBool_Type
    }
}

#[inline(always)]
pub unsafe fn is_type(o: O, t: *mut ffi::PyTypeObject) -> bool {
    // SAFETY: header read of a live object.
    unsafe { ffi::Py_TYPE(o) == t }
}

#[inline(always)]
pub unsafe fn exact_str(o: O) -> bool {
    // SAFETY: header read.
    unsafe { is_type(o, &raw mut ffi::PyUnicode_Type) }
}

#[inline(always)]
pub unsafe fn exact_int(o: O) -> bool {
    // SAFETY: header read.
    unsafe { is_type(o, &raw mut ffi::PyLong_Type) }
}

#[inline(always)]
pub unsafe fn exact_dict(o: O) -> bool {
    // SAFETY: header read.
    unsafe { is_type(o, &raw mut ffi::PyDict_Type) }
}

#[inline(always)]
pub unsafe fn exact_tuple(o: O) -> bool {
    // SAFETY: header read.
    unsafe { is_type(o, &raw mut ffi::PyTuple_Type) }
}

/// Truthiness of an object whose `bool()` runs no Python code: `Some(b)` for exact
/// `bool`/`None`/`int`/`str`/`tuple`/`dict`, `None` otherwise.
///
/// # Safety
/// Live object.
#[inline(always)]
pub unsafe fn pure_truth(o: O) -> Option<bool> {
    // SAFETY: header reads; the size reads are lock-free in CPython too.
    unsafe {
        if o == ffi::Py_None() || o == ffi::Py_False() {
            Some(false)
        } else if o == ffi::Py_True() {
            Some(true)
        } else if exact_str(o) {
            Some(ffi::PyUnicode_GET_LENGTH(o) != 0)
        } else if exact_tuple(o) {
            Some(ffi::PyTuple_GET_SIZE(o) != 0)
        } else if exact_dict(o) {
            Some(ffi::PyDict_Size(o) != 0)
        } else if exact_int(o) {
            let r = ffi::PyObject_IsTrue(o);
            Some(r == 1)
        } else {
            None
        }
    }
}

/// Visit every `(key, value)` of an exact dict under its critical section
/// (free-threaded builds), stopping when `f` returns false. Returns false if stopped.
///
/// # Safety
/// Attached thread; `d` an exact dict; `f` must not run Python code.
pub unsafe fn dict_all(d: O, mut f: impl FnMut(O, O) -> bool) -> bool {
    // SAFETY: per the caller; borrowed key/value references are used under the section.
    unsafe {
        #[cfg(Py_GIL_DISABLED)]
        let mut cs: ffi::PyCriticalSection = std::mem::zeroed();
        #[cfg(Py_GIL_DISABLED)]
        ffi::PyCriticalSection_Begin(&mut cs, d);
        let mut pos: ffi::Py_ssize_t = 0;
        let mut k: O = std::ptr::null_mut();
        let mut v: O = std::ptr::null_mut();
        let mut all = true;
        while ffi::PyDict_Next(d, &mut pos, &mut k, &mut v) != 0 {
            if !f(k, v) {
                all = false;
                break;
            }
        }
        #[cfg(Py_GIL_DISABLED)]
        ffi::PyCriticalSection_End(&mut cs);
        all
    }
}

/// `d.get(key)` for an exact int `key` on an exact dict whose keys are all exact ints:
/// `Ok(Some(v))` (new reference) / `Ok(None)` (absent), `Err(())` if some key is not an
/// exact int (the lookup could run a user `__eq__`) or the dict is large.
///
/// # Safety
/// Attached thread; `d` exact dict; `key` exact int.
pub unsafe fn int_keyed_get(d: O, key: O) -> Result<Option<O>, ()> {
    // SAFETY: per the caller.
    unsafe {
        if ffi::PyDict_Size(d) > 32 {
            return Err(());
        }
        let mut ov: c_int = 0;
        let kv = ffi::PyLong_AsLongLongAndOverflow(key, &mut ov);
        if ov != 0 {
            return Err(());
        }
        let mut found: O = std::ptr::null_mut();
        let all_int = dict_all(d, |k, v| {
            if !exact_int(k) {
                return false;
            }
            if found.is_null() && (k == key || {
                let mut o2: c_int = 0;
                let x = ffi::PyLong_AsLongLongAndOverflow(k, &mut o2);
                o2 == 0 && x == kv
            }) {
                found = ffi::Py_NewRef(v);
            }
            true
        });
        if !all_int {
            if !found.is_null() {
                ffi::Py_DECREF(found);
            }
            return Err(());
        }
        Ok((!found.is_null()).then_some(found))
    }
}

/// Every key of the exact dict `d` is an exact `str` (so lookups with `str` keys run no
/// Python code). Bounded size.
///
/// # Safety
/// Attached thread; exact dict.
pub unsafe fn str_keyed(d: O, max: isize) -> bool {
    // SAFETY: per the caller.
    unsafe { ffi::PyDict_Size(d) <= max && dict_all(d, |k, _| exact_str(k)) }
}

// ---------------------------------------------------------------------------------------
// Interned names
// ---------------------------------------------------------------------------------------

macro_rules! names {
    ($($field:ident = $s:literal),* $(,)?) => {
        /// Interned attribute names and constants (main interpreter; held forever).
        pub struct Names { $(pub $field: usize,)* }
        // SAFETY: plain addresses of objects kept alive for the process lifetime.
        unsafe impl Sync for Names {}
        impl Names {
            pub fn new() -> Self {
                // SAFETY: attached thread (called from install); NUL-terminated literals.
                unsafe {
                    Names { $($field: ffi::PyUnicode_InternFromString(concat!($s, "\0").as_ptr().cast()) as usize,)* }
                }
            }
        }
    };
}

names! {
    is_enabled_for = "isEnabledFor", disabled = "disabled", cache = "_cache", msg = "msg", self_ = "self",
    level = "level", name = "name", args = "args", levelname = "levelname", levelno = "levelno",
    pathname = "pathname", filename = "filename", module = "module", exc_info = "exc_info",
    exc_text = "exc_text", stack_info = "stack_info", lineno = "lineno", func_name = "funcName",
    created = "created", msecs = "msecs", relative_created = "relativeCreated", thread = "thread",
    thread_name = "threadName", process_name = "processName", process = "process", task_name = "taskName",
    message = "message", asctime = "asctime", style = "_style", datefmt = "datefmt", converter = "converter",
    default_time_format = "default_time_format", default_msec_format = "default_msec_format",
    uses_time = "usesTime", format_time = "formatTime", format_message = "formatMessage",
    format_exception = "formatException", format_stack = "formatStack", format = "format", fmt = "_fmt",
    defaults = "_defaults", asctime_search = "asctime_search", format_ = "_format", get_message = "getMessage",
    t_name = "_name", t_initialized = "_initialized", current_process = "current_process",
    current_task = "current_task", get_name = "get_name", multiprocessing = "multiprocessing",
    asyncio = "asyncio", main_process = "MainProcess", level_fmt = "Level %s", modules = "modules",
    dunder_init = "__init__", find_caller = "findCaller", msec_default = "%s,%03d",
    get_running_loop = "_get_running_loop", current_process_g = "_current_process",
}

impl Default for Names {
    fn default() -> Self {
        Self::new()
    }
}

static NAMES: OnceLock<Names> = OnceLock::new();

pub fn init_names() {
    NAMES.get_or_init(Names::new);
}

/// # Safety
/// `init_names` ran (every kernel is installed after it).
#[inline(always)]
pub unsafe fn n() -> &'static Names {
    // SAFETY: per the caller.
    unsafe { NAMES.get().unwrap_unchecked() }
}

/// A name as an object pointer.
#[inline(always)]
pub fn o(addr: usize) -> O {
    addr as O
}

/// An object pointer captured at install (strong reference held by the target).
#[derive(Debug)]
pub struct Cap(AtomicUsize);

impl Cap {
    pub const fn new() -> Self {
        Cap(AtomicUsize::new(0))
    }
    #[inline(always)]
    pub fn get(&self) -> O {
        self.0.load(Relaxed) as O
    }
    /// Store a strong reference (the previous one, if any, is leaked on purpose: a kernel
    /// running concurrently may still be using it).
    pub fn set(&self, obj: O) {
        // SAFETY: attached thread (install).
        unsafe { ffi::Py_XINCREF(obj) };
        self.0.store(obj as usize, Relaxed);
    }
}

impl Default for Cap {
    fn default() -> Self {
        Self::new()
    }
}
