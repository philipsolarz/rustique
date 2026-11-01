//! `LogRecord.__init__` (target `logging.LogRecord.__init__`).
//!
//! Computes every attribute CPython's `__init__` computes, from the same sources
//! (`time.time_ns` → `PyTime_Time`, `getLevelName`, `os.path.basename`/`splitext`,
//! `threading.get_ident`/`current_thread().name`, `os.getpid`, asyncio's current task),
//! then stores them with the generic attribute protocol **in CPython's order**, so
//! `record.__dict__` has the same keys in the same order with equal values.
//!
//! Falls back (before storing anything) when: a dependency was replaced (module globals and
//! the logThreads/logProcesses/logMultiprocessing/logAsyncioTasks switches are pinned), a
//! tool listens, the record class overrides an attribute with a descriptor, `pathname` is
//! not a str, `level` is not an int, `args` is not a tuple, `multiprocessing` is imported
//! (its process name runs Python code), the current thread is not registered in
//! `threading._active`, or the task is not a plain `_asyncio.Task`.
//!
//! `isinstance(args[0], collections.abc.Mapping)` for a single argument: the kernel never
//! calls the ABC machinery (it can run user `__subclasshook__`s). It answers from a cache
//! of what CPython itself answered for that argument type at the current
//! `abc.get_cache_token()`; CPython's own answer is stable while the token is (positive
//! and negative ABC caches), so the cached answer is the one CPython would give. A miss
//! runs the original once and learns from the record it built.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

use pyo3::ffi;
use pyo3::prelude::*;

use super::common::*;
use crate::accel::guard::DepFlag;
use crate::accel::stats::{Counters, Reason};

pub static DEPS: DepFlag = DepFlag::new();
pub static STATS: Counters = Counters::new();

pub static LEVEL_TO_NAME: Cap = Cap::new();
/// Generation of `_levelToName` (bumped on any change by the registry's dict watcher) and
/// the generation + 1 at which every key was verified to be an exact int (0: never).
pub static L2N_GEN: AtomicU64 = AtomicU64::new(0);
static L2N_VERIFIED: AtomicU64 = AtomicU64::new(0);
pub static NAME_TO_LEVEL: Cap = Cap::new();
pub static SYS_MODULES: Cap = Cap::new();
pub static THREAD_ACTIVE: Cap = Cap::new();
/// `threading.Thread.name` (the property object).
pub static THREAD_NAME_PROP: Cap = Cap::new();
/// `_abc.get_cache_token` (NULL: the single-argument path always falls back).
pub static ABC_TOKEN: Cap = Cap::new();
/// `logging._startTime` (pinned), as an i64.
pub static START_TIME: AtomicU64 = AtomicU64::new(0);
/// logThreads, logMultiprocessing, logProcesses, logAsyncioTasks (pinned values) as bits.
pub static SWITCHES: AtomicUsize = AtomicUsize::new(0);
/// `_asyncio.current_task`, `_asyncio._get_running_loop`, `_asyncio.Task`; filled when
/// asyncio is first seen (NULL until then).
pub static AIO_CURRENT_TASK: Cap = Cap::new();
pub static AIO_RUNNING_LOOP: Cap = Cap::new();
pub static AIO_TASK: Cap = Cap::new();
/// __init__, getLevelName, basename, splitext, _splitext, current_thread, Thread.name.fget,
/// _get_sep.
pub static CODES: [FnGuard; 8] = [const { FnGuard::new() }; 8];

/// Reads `created`, `msecs` (formatTime).
pub static RECORDS: TypeCache<2> = TypeCache::new(validate_record);
/// Reads `_initialized`, `_name`.
pub static THREADS: TypeCache<2> = TypeCache::new(validate_thread);

/// The record attributes `__init__` stores, in CPython's order.
pub const ATTRS: usize = 21;

/// A record class whose instances store every `__init__` attribute (and `message`,
/// `asctime`) as plain instance attributes: generic get/set, no descriptors.
unsafe fn validate_record(t: Tp) -> bool {
    // SAFETY: attached thread; live type.
    unsafe {
        let n = n();
        generic_attrs(t, true)
            && attr_names(n).iter().chain([n.message, n.asctime].iter()).all(|&a| plain_attr(t, o(a)))
    }
}

unsafe fn validate_thread(t: Tp) -> bool {
    // SAFETY: attached thread; live type.
    unsafe {
        let n = n();
        generic_attrs(t, false)
            && resolves_to(t, o(n.name), THREAD_NAME_PROP.get())
            && plain_attr(t, o(n.t_name))
            && plain_attr(t, o(n.t_initialized))
    }
}

pub fn attr_names(n: &Names) -> [usize; ATTRS] {
    [
        n.name,
        n.msg,
        n.args,
        n.levelname,
        n.levelno,
        n.pathname,
        n.filename,
        n.module,
        n.exc_info,
        n.exc_text,
        n.stack_info,
        n.lineno,
        n.func_name,
        n.created,
        n.msecs,
        n.relative_created,
        n.thread,
        n.thread_name,
        n.process_name,
        n.process,
        n.task_name,
    ]
}

// ---------------------------------------------------------------------------------------
// isinstance(args[0], Mapping) answers
// ---------------------------------------------------------------------------------------

/// Types whose `__class__`, truthiness and ABC answer involve no Python code.
fn mapping_slot(t: Tp) -> Option<usize> {
    let tys: [*mut ffi::PyTypeObject; 9] = [
        &raw mut ffi::PyLong_Type,
        &raw mut ffi::PyUnicode_Type,
        &raw mut ffi::PyFloat_Type,
        &raw mut ffi::PyBool_Type,
        // SAFETY: address of the None type object via the singleton.
        unsafe { ffi::Py_TYPE(ffi::Py_None()) },
        &raw mut ffi::PyBytes_Type,
        &raw mut ffi::PyTuple_Type,
        &raw mut ffi::PyList_Type,
        &raw mut ffi::PyDict_Type,
    ];
    tys.iter().position(|&x| x == t)
}

/// Per slot: `token << 2 | known << 1 | is_mapping`.
static MAPPING: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];

/// Truthiness of a whitelisted type (no Python code).
unsafe fn plain_truth(o: O) -> bool {
    // SAFETY: live object of a whitelisted type.
    unsafe { ffi::PyObject_IsTrue(o) == 1 }
}

unsafe fn abc_token() -> Option<u64> {
    // SAFETY: attached thread; `_abc.get_cache_token` is a C function with no arguments.
    unsafe {
        let f = ABC_TOKEN.get();
        if f.is_null() {
            return None;
        }
        let r = ffi::PyObject_CallNoArgs(f);
        if r.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let v = ffi::PyLong_AsUnsignedLongLong(r);
        ffi::Py_DECREF(r);
        if v == u64::MAX && !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        (v < 1 << 61).then_some(v)
    }
}

// ---------------------------------------------------------------------------------------
// Kernel
// ---------------------------------------------------------------------------------------

enum Outcome {
    Done,
    Fallback(Reason),
    /// Run the original, then learn `isinstance(arg0, Mapping)` for slot at token.
    Learn(usize, u64),
}

/// Owned references released on drop.
struct Owned<const N: usize>([O; N]);

impl<const N: usize> Drop for Owned<N> {
    fn drop(&mut self) {
        for &p in &self.0 {
            // SAFETY: owned or NULL.
            unsafe { ffi::Py_XDECREF(p) };
        }
    }
}

/// `getLevelName(level)` for an exact int or `None`: `Some(new ref)`, `None` to fall back.
unsafe fn level_name(level: O) -> Option<O> {
    // SAFETY: attached thread; pinned dicts.
    unsafe {
        let l2n = LEVEL_TO_NAME.get();
        if !exact_dict(l2n) {
            return None;
        }
        // Every key an exact int: then a lookup with an int (or None) runs no `__eq__`.
        // Verified once per generation of the dict.
        let gen_ = L2N_GEN.load(Relaxed);
        if L2N_VERIFIED.load(Relaxed) != gen_ + 1 {
            if !(ffi::PyDict_Size(l2n) <= 256 && dict_all(l2n, |k, _| exact_int(k))) {
                return None;
            }
            L2N_VERIFIED.store(gen_ + 1, Relaxed);
        }
        let found: Result<Option<O>, ()> = if level == ffi::Py_None() {
            Ok(None) // equal to no int key
        } else {
            let mut v: O = std::ptr::null_mut();
            match ffi::PyDict_GetItemRef(l2n, level, &mut v) {
                1 => Ok(Some(v)),
                0 => Ok(None),
                _ => {
                    ffi::PyErr_Clear();
                    Err(())
                }
            }
        };
        match found {
            Ok(Some(v)) => {
                if v != ffi::Py_None() {
                    return Some(v);
                }
                ffi::Py_DECREF(v);
            }
            Ok(None) => {}
            Err(()) => return None,
        }
        // `_nameToLevel.get(level)`: an int never equals a str key.
        let n2l = NAME_TO_LEVEL.get();
        if !exact_dict(n2l) || !str_keyed(n2l, 64) {
            return None;
        }
        let s = ffi::PyUnicode_Format(o(n().level_fmt), level);
        if s.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(s)
    }
}

/// `(basename(p), splitext(basename(p))[0])` for an exact str (new references).
unsafe fn filename_module(p: O) -> Option<(O, O)> {
    // SAFETY: exact str.
    unsafe {
        let len = ffi::PyUnicode_GET_LENGTH(p) as usize;
        let kind = ffi::PyUnicode_KIND(p);
        let data = ffi::PyUnicode_DATA(p);
        let at = |i: usize| -> u32 {
            match kind {
                1 => *data.cast::<u8>().add(i) as u32,
                2 => *data.cast::<u16>().add(i) as u32,
                _ => *data.cast::<u32>().add(i),
            }
        };
        let rfind = |c: u32, a: usize, b: usize| (a..b).rev().find(|&i| at(i) == c);
        let (fs, me) = rustique_core::logging::split_path(len, rfind, at);
        let filename = ffi::PyUnicode_Substring(p, fs as _, len as _);
        if filename.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let module = ffi::PyUnicode_Substring(filename, 0, me as _);
        if module.is_null() {
            ffi::PyErr_Clear();
            ffi::Py_DECREF(filename);
            return None;
        }
        Some((filename, module))
    }
}

/// `threading.current_thread().name` when the current thread is registered: `Some(new ref)`.
unsafe fn thread_name(ident: O) -> Option<O> {
    // SAFETY: attached thread.
    unsafe {
        let active = THREAD_ACTIVE.get();
        let mut t: O = std::ptr::null_mut();
        if !exact_dict(active) || ffi::PyDict_GetItemRef(active, ident, &mut t) != 1 {
            ffi::PyErr_Clear();
            return None;
        }
        let e = THREADS.entry(ffi::Py_TYPE(t));
        let ok = e.is_some() && {
            let init = THREADS.read(e.unwrap_unchecked(), t, 0);
            let ok = init == ffi::Py_True();
            ffi::Py_XDECREF(init);
            ok
        };
        let r = if ok { THREADS.read(e.unwrap_unchecked(), t, 1) } else { std::ptr::null_mut() };
        ffi::Py_DECREF(t);
        (!r.is_null()).then_some(r)
    }
}

/// `sys.modules.get(name)`: Ok(NULL) absent, Ok(new ref), Err on a non-dict `sys.modules`.
unsafe fn sys_module(name: usize) -> Result<O, ()> {
    // SAFETY: attached thread; pinned sys.modules dict.
    unsafe {
        let m = SYS_MODULES.get();
        if !exact_dict(m) {
            return Err(());
        }
        let mut r: O = std::ptr::null_mut();
        if ffi::PyDict_GetItemRef(m, o(name), &mut r) < 0 {
            ffi::PyErr_Clear();
            return Err(());
        }
        Ok(r)
    }
}

/// `module.name` for an exact module whose dict holds `name` (what `module.name` returns
/// then: `ModuleType` has no descriptor of these names). NULL when absent (CPython would
/// consult a module `__getattr__`: the caller falls back).
unsafe fn module_attr(m: O, name: O) -> O {
    // SAFETY: attached thread; exact module (checked by the caller); str key.
    unsafe {
        let d = ffi::PyModule_GetDict(m);
        let mut v: O = std::ptr::null_mut();
        if d.is_null() || ffi::PyDict_GetItemRef(d, name, &mut v) != 1 {
            ffi::PyErr_Clear();
            return std::ptr::null_mut();
        }
        v
    }
}

// ---------------------------------------------------------------------------------------
// multiprocessing: `mp.current_process().name`
// ---------------------------------------------------------------------------------------

/// `multiprocessing.process.current_process` (fingerprinted when first seen), its code,
/// its globals, and the `BaseProcess.name` property.
static MP_CURRENT_PROCESS: Cap = Cap::new();
static MP_CODE: Cap = Cap::new();
static MP_GLOBALS: Cap = Cap::new();
pub static MP_NAME_PROP: Cap = Cap::new();
/// 0 unknown, 1 validated, 2 refused.
static MP_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// Reads `_name`.
pub static MP_PROCS: TypeCache<1> = TypeCache::new(validate_mp_process);

const FP_CURRENT_PROCESS: &[(u8, u8, &str)] = &[(3, 14, "047839c36f3adea0"), (3, 15, "1b03a89d39d6f22d")];
const FP_PROCESS_NAME: &[(u8, u8, &str)] = &[(3, 14, "cc78b3d6131b11a9"), (3, 15, "40fb9ae6946bc8ad")];

unsafe fn validate_mp_process(t: Tp) -> bool {
    // SAFETY: attached thread; live type.
    unsafe {
        let n = n();
        generic_attrs(t, false) && resolves_to(t, o(n.name), MP_NAME_PROP.get()) && plain_attr(t, o(n.t_name))
    }
}

fn fingerprint_ok(obj: O, expected: &'static [(u8, u8, &'static str)]) -> bool {
    let (major, minor) = crate::accel::version::running();
    let Some(want) = crate::accel::fingerprint::expected_for(expected, major, minor) else { return false };
    // SAFETY: attached thread (kernel); a live object.
    let py = unsafe { Python::assume_attached() };
    // SAFETY: borrowed reference kept alive by the caller.
    let b = unsafe { Bound::from_borrowed_ptr(py, obj) };
    crate::accel::fingerprint::any(&b).is_ok_and(|f| f == want)
}

/// Validate `multiprocessing.current_process` the first time multiprocessing is seen.
#[cold]
unsafe fn capture_mp(cp: O) -> bool {
    // SAFETY: attached thread.
    unsafe {
        let st = MP_STATE.load(Relaxed);
        if st != 0 {
            return st == 1;
        }
        let ok = (|| {
            if ffi::Py_TYPE(cp) != &raw mut ffi::PyFunction_Type || !fingerprint_ok(cp, FP_CURRENT_PROCESS) {
                return false;
            }
            let f = cp.cast::<ffi::PyFunctionObject>();
            let globals = (*f).func_globals;
            // the module that defines it, as imported
            let mut pm: O = std::ptr::null_mut();
            let key = ffi::PyUnicode_InternFromString(c"multiprocessing.process".as_ptr());
            let found = !key.is_null() && ffi::PyDict_GetItemRef(SYS_MODULES.get(), key, &mut pm) == 1;
            ffi::Py_XDECREF(key);
            if !found {
                ffi::PyErr_Clear();
                return false;
            }
            let same = ffi::Py_TYPE(pm) == &raw mut ffi::PyModule_Type && ffi::PyModule_GetDict(pm) == globals;
            let bp = if same {
                let k = ffi::PyUnicode_InternFromString(c"BaseProcess".as_ptr());
                let bp = if k.is_null() { std::ptr::null_mut() } else { module_attr(pm, k) };
                ffi::Py_XDECREF(k);
                bp
            } else {
                std::ptr::null_mut()
            };
            ffi::Py_DECREF(pm);
            if bp.is_null() || ffi::PyType_Check(bp) == 0 {
                ffi::Py_XDECREF(bp);
                return false;
            }
            let prop = _PyType_LookupRef(bp.cast(), o(n().name));
            ffi::Py_DECREF(bp);
            if prop.is_null() {
                return false;
            }
            let ok = ffi::Py_TYPE(prop) == &raw mut ffi::PyProperty_Type && {
                let fget = ffi::PyObject_GetAttrString(prop, c"fget".as_ptr());
                if fget.is_null() {
                    ffi::PyErr_Clear();
                }
                let ok = !fget.is_null() && fingerprint_ok(fget, FP_PROCESS_NAME);
                ffi::Py_XDECREF(fget);
                ok
            };
            if ok {
                MP_NAME_PROP.set(prop);
                MP_GLOBALS.set(globals);
                MP_CODE.set((*f).func_code);
                MP_CURRENT_PROCESS.set(cp);
            }
            ffi::Py_DECREF(prop);
            ok
        })();
        ffi::PyErr_Clear();
        MP_STATE.store(if ok { 1 } else { 2 }, Relaxed);
        ok
    }
}

/// `mp.current_process().name` for the multiprocessing module `mp`: `Some(new ref)`, or
/// `None` to fall back. `current_process()` returns the global `_current_process` of
/// `multiprocessing.process` and `name` is `BaseProcess.name` (`return self._name`): read
/// directly once both are validated (fingerprints, identities), so no Python code runs.
unsafe fn mp_process_name(mp: O) -> Option<O> {
    // SAFETY: attached thread.
    unsafe {
        if ffi::Py_TYPE(mp) != &raw mut ffi::PyModule_Type {
            return None;
        }
        let n = n();
        let cp = module_attr(mp, o(n.current_process));
        if cp.is_null() {
            return None;
        }
        ffi::Py_DECREF(cp); // identity only (validated: held by MP_CURRENT_PROCESS)
        if cp != MP_CURRENT_PROCESS.get()
            && (MP_STATE.load(Relaxed) != 0 || !capture_mp(cp) || cp != MP_CURRENT_PROCESS.get())
        {
            return None;
        }
        if (*cp.cast::<ffi::PyFunctionObject>()).func_code != MP_CODE.get() {
            return None;
        }
        let mut proc: O = std::ptr::null_mut();
        if ffi::PyDict_GetItemRef(MP_GLOBALS.get(), o(n.current_process_g), &mut proc) != 1 {
            ffi::PyErr_Clear();
            return None;
        }
        let e = MP_PROCS.entry(ffi::Py_TYPE(proc));
        let name = match e {
            Some(e) => MP_PROCS.read(e, proc, 0),
            None => std::ptr::null_mut(),
        };
        ffi::Py_DECREF(proc);
        (!name.is_null()).then_some(name)
    }
}

/// Capture `_asyncio`'s functions the first time asyncio is seen.
#[cold]
unsafe fn capture_asyncio() -> bool {
    // SAFETY: attached thread.
    unsafe {
        let mut aio: O = std::ptr::null_mut();
        let key = ffi::PyUnicode_InternFromString(c"_asyncio".as_ptr());
        let found = !key.is_null() && ffi::PyDict_GetItemRef(SYS_MODULES.get(), key, &mut aio) == 1;
        ffi::Py_XDECREF(key);
        if !found || ffi::Py_TYPE(aio) != &raw mut ffi::PyModule_Type {
            ffi::PyErr_Clear();
            ffi::Py_XDECREF(aio);
            return false;
        }
        let ct = module_attr(aio, o(n().current_task));
        let rl = module_attr(aio, o(n().get_running_loop));
        let task = ffi::PyObject_GetAttrString(aio, c"Task".as_ptr());
        if task.is_null() {
            ffi::PyErr_Clear();
        }
        ffi::Py_DECREF(aio);
        let ok = !ct.is_null()
            && !rl.is_null()
            && !task.is_null()
            && ffi::PyCFunction_Check(ct) != 0
            && ffi::PyCFunction_Check(rl) != 0
            && ffi::PyType_Check(task) != 0;
        if ok {
            AIO_RUNNING_LOOP.set(rl);
            AIO_TASK.set(task);
            AIO_CURRENT_TASK.set(ct); // published last: the kernel tests this one
        }
        ffi::Py_XDECREF(ct);
        ffi::Py_XDECREF(rl);
        ffi::Py_XDECREF(task);
        ok
    }
}

/// `taskName`: `Ok(new ref)` (None or the name) or `Err` to fall back.
unsafe fn task_name() -> Result<O, ()> {
    // SAFETY: attached thread.
    unsafe {
        let none = || ffi::Py_NewRef(ffi::Py_None());
        let aio = sys_module(n().asyncio)?;
        if aio.is_null() || aio == ffi::Py_None() {
            ffi::Py_XDECREF(aio);
            return Ok(none()); // `if asyncio:` is false
        }
        let is_mod = ffi::Py_TYPE(aio) == &raw mut ffi::PyModule_Type;
        if !is_mod || (AIO_CURRENT_TASK.get().is_null() && !capture_asyncio()) {
            ffi::Py_DECREF(aio);
            return Err(());
        }
        // `asyncio.current_task` must be the C function.
        let ct = module_attr(aio, o(n().current_task));
        ffi::Py_DECREF(aio);
        let same = ct == AIO_CURRENT_TASK.get();
        ffi::Py_XDECREF(ct);
        if !same {
            return Err(());
        }
        // No running loop: `current_task()` raises RuntimeError, which `__init__` swallows.
        let lp = ffi::PyObject_CallNoArgs(AIO_RUNNING_LOOP.get());
        if lp.is_null() {
            ffi::PyErr_Clear();
            return Err(());
        }
        let running = lp != ffi::Py_None();
        ffi::Py_DECREF(lp);
        if !running {
            return Ok(none());
        }
        let task = ffi::PyObject_CallNoArgs(AIO_CURRENT_TASK.get());
        if task.is_null() {
            ffi::PyErr_Clear();
            return Err(());
        }
        if task == ffi::Py_None() {
            ffi::Py_DECREF(task);
            return Ok(none()); // `None.get_name()` raises AttributeError: swallowed
        }
        if ffi::Py_TYPE(task) != AIO_TASK.get().cast() {
            ffi::Py_DECREF(task);
            return Err(());
        }
        let name = ffi::PyObject_CallMethodNoArgs(task, o(n().get_name));
        ffi::Py_DECREF(task);
        if name.is_null() {
            ffi::PyErr_Clear();
            return Err(());
        }
        Ok(name)
    }
}

unsafe fn init(me: O, a: *const O, n_args: usize) -> Outcome {
    // SAFETY: vectorcall arguments (n_args >= 8); attached thread.
    unsafe {
        let nm = n();
        let (name, level, pathname, lineno, msg, args, exc_info) =
            (*a.add(1), *a.add(2), *a.add(3), *a.add(4), *a.add(5), *a.add(6), *a.add(7));
        let func = if n_args >= 9 { *a.add(8) } else { ffi::Py_None() };
        let sinfo = if n_args >= 10 { *a.add(9) } else { ffi::Py_None() };
        if !exact_tuple(args) || !(exact_int(level) || level == ffi::Py_None()) || !exact_str(pathname) {
            return Outcome::Fallback(Reason::Args);
        }
        let mut rargs = args;
        if ffi::PyTuple_GET_SIZE(args) == 1 {
            let arg0 = ffi::PyTuple_GET_ITEM(args, 0);
            let Some(slot) = mapping_slot(ffi::Py_TYPE(arg0)) else { return Outcome::Fallback(Reason::Args) };
            let Some(tok) = abc_token() else { return Outcome::Fallback(Reason::Args) };
            let e = MAPPING[slot].load(Relaxed);
            if e & 2 == 0 || e >> 2 != tok {
                return Outcome::Learn(slot, tok);
            }
            if e & 1 == 1 && plain_truth(arg0) {
                rargs = arg0;
            }
        }
        let sw = SWITCHES.load(Relaxed);
        let Some(levelname) = level_name(level) else { return Outcome::Fallback(Reason::Args) };
        let mut own = Owned([
            levelname,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ]);
        let Some((filename, module)) = filename_module(pathname) else { return Outcome::Fallback(Reason::Args) };
        own.0[1] = filename;
        own.0[2] = module;
        // ct = time.time_ns()
        let mut ct: i64 = 0;
        if PyTime_Time(&mut ct) < 0 {
            ffi::PyErr_Clear();
            return Outcome::Fallback(Reason::Deopt);
        }
        let Some((created, msecs, relative)) = record_times(ct, START_TIME.load(Relaxed) as i64) else {
            return Outcome::Fallback(Reason::Deopt);
        };
        own.0[3] = ffi::PyFloat_FromDouble(created);
        own.0[4] = ffi::PyFloat_FromDouble(msecs);
        own.0[5] = ffi::PyFloat_FromDouble(relative);
        // thread, threadName
        let none = ffi::Py_None();
        let (thread, thread_name_v) = if sw & 1 != 0 {
            let id = ffi::PyThread_get_thread_ident();
            let ident = ffi::PyLong_FromUnsignedLongLong(id as u64);
            own.0[6] = ident;
            if ident.is_null() {
                ffi::PyErr_Clear();
                return Outcome::Fallback(Reason::Deopt);
            }
            let Some(tn) = thread_name(ident) else { return Outcome::Fallback(Reason::Args) };
            own.0[7] = tn;
            (ident, tn)
        } else {
            (none, none)
        };
        // processName
        let process_name = if sw & 2 != 0 {
            match sys_module(nm.multiprocessing) {
                Ok(mp) if mp.is_null() || mp == none => {
                    ffi::Py_XDECREF(mp);
                    o(nm.main_process)
                }
                Ok(mp) => {
                    let name = mp_process_name(mp);
                    ffi::Py_DECREF(mp);
                    let Some(name) = name else { return Outcome::Fallback(Reason::Args) };
                    own.0[10] = name;
                    name
                }
                Err(()) => return Outcome::Fallback(Reason::Args),
            }
        } else {
            none
        };
        // process
        let process = if sw & 4 != 0 {
            let p = ffi::PyLong_FromLong(getpid() as _);
            own.0[8] = p;
            if p.is_null() {
                ffi::PyErr_Clear();
                return Outcome::Fallback(Reason::Deopt);
            }
            p
        } else {
            none
        };
        // taskName
        let task = if sw & 8 != 0 {
            match task_name() {
                Ok(t) => {
                    own.0[9] = t;
                    t
                }
                Err(()) => return Outcome::Fallback(Reason::Args),
            }
        } else {
            none
        };
        if own.0[3..6].iter().any(|p| p.is_null()) {
            ffi::PyErr_Clear();
            return Outcome::Fallback(Reason::Deopt);
        }
        // Store, in CPython's order.
        let values: [O; ATTRS] = [
            name,
            msg,
            rargs,
            levelname,
            level,
            pathname,
            filename,
            module,
            exc_info,
            none,
            sinfo,
            lineno,
            func,
            own.0[3],
            own.0[4],
            own.0[5],
            thread,
            thread_name_v,
            process_name,
            process,
            task,
        ];
        let names = attr_names(nm);
        for i in 0..ATTRS {
            if ffi::PyObject_GenericSetAttr(me, o(names[i]), values[i]) < 0 {
                // Nothing observable yet except stores into `self`, which the original
                // repeats (same keys, same order).
                ffi::PyErr_Clear();
                return Outcome::Fallback(Reason::Deopt);
            }
        }
        Outcome::Done
    }
}

unsafe extern "C" {
    fn getpid() -> i32;
}

/// `created`, `msecs`, `relativeCreated` from `ct = time.time_ns()` as `__init__` computes
/// them: `ct / 1e9` (int → double, correctly rounded, as `PyLong_AsDouble`),
/// `(ct % 10**9) // 10**6 + 0.0` (floor semantics) reset to 0.0 when the seconds rounded up
/// (gh-89047), `(ct - _startTime) / 1e6`.
pub fn record_times(ct: i64, start: i64) -> Option<(f64, f64, f64)> {
    let created = ct as f64 / 1e9;
    let mut msecs = (ct.rem_euclid(1_000_000_000) / 1_000_000) as f64;
    if msecs == 999.0 && created.trunc() as i64 != ct.div_euclid(1_000_000_000) {
        msecs = 0.0;
    }
    let diff = ct.checked_sub(start)?;
    Some((created, msecs, diff as f64 / 1e6))
}

/// `LogRecord.__init__(self, name, level, pathname, lineno, msg, args, exc_info,
/// func=None, sinfo=None, **kwargs)`.
///
/// # Safety
/// Installed as the function's vectorcall: CPython calls it with vectorcall arguments.
pub unsafe extern "C" fn vc_record_init(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let n = nargs(nargsf);
        let why = 'fb: {
            if !kw.is_null() || !(8..=10).contains(&n) {
                break 'fb Reason::Args;
            }
            if !DEPS.ok() || !SHARED_DEPS.ok() {
                break 'fb Reason::Deps;
            }
            let me = *args;
            if !RECORDS.ok(ffi::Py_TYPE(me)) {
                break 'fb Reason::Args;
            }
            if !TOOLS.quiet() || !fns_ok(&CODES) {
                break 'fb Reason::Tools;
            }
            match init(me, args, n) {
                Outcome::Done => {
                    STATS.hit_streak();
                    return ffi::Py_NewRef(ffi::Py_None());
                }
                Outcome::Fallback(r) => r,
                Outcome::Learn(slot, tok) => {
                    STATS.fallback_streak(Reason::Deopt);
                    let r = original(func, args, nargsf, kw);
                    if !r.is_null() {
                        learn(me, *args.add(6), slot, tok);
                    }
                    return r;
                }
            }
        };
        STATS.fallback_streak(why);
        original(func, args, nargsf, kw)
    }
}

/// After the original built the record: `self.args is args[0]` means
/// `isinstance(args[0], Mapping) and args[0]` held; `self.args is args` with a true
/// `args[0]` means it was not a Mapping. Recorded only if the ABC token did not move.
#[cold]
unsafe fn learn(me: O, args: O, slot: usize, tok: u64) {
    // SAFETY: attached thread; `me` of a validated record type.
    unsafe {
        let arg0 = ffi::PyTuple_GET_ITEM(args, 0);
        if !plain_truth(arg0) {
            return;
        }
        let got = get(me, o(n().args));
        if got.is_null() {
            return;
        }
        let verdict = if got == arg0 {
            Some(true)
        } else if got == args {
            Some(false)
        } else {
            None
        };
        ffi::Py_DECREF(got);
        if let Some(v) = verdict
            && abc_token() == Some(tok)
        {
            MAPPING[slot].store(tok << 2 | 2 | v as u64, Relaxed);
        }
    }
}

pub fn reset_mapping_cache() {
    for m in &MAPPING {
        m.store(0, Relaxed);
    }
}
