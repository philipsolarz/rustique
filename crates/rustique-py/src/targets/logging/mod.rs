//! Group `logging`: the default record path of `logging`, piecewise (docs/targets/logging.md).
//!
//! | target | site(s) | kernel |
//! |---|---|---|
//! | `logging.Logger.level` | `Logger.debug/.info/.warning/.error/.critical/.log` | disabled calls return without a frame |
//! | `logging.Logger.findCaller` | `Logger.findCaller` | frame walk |
//! | `logging.LogRecord.__init__` | `LogRecord.__init__` | the record's attributes |
//! | `logging.Formatter.formatTime` | `Formatter.formatTime` | localtime + strftime + msecs |
//! | `logging.Formatter.format` | `Formatter.format` | getMessage + asctime + `PercentStyle` |
//!
//! Every site is the vectorcall of the function object in the class `__dict__`
//! (contract §1, kind C). Handlers, filters, locks, `emit`/`flush` and everything that
//! calls user code stay CPython's: a kernel only replaces code below which no Python code
//! runs, so tracebacks and stacks seen by user code are CPython's.

pub mod caller;
pub mod common;
pub mod format;
pub mod level;
pub mod record;

use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule, PyString, PyType};

use crate::accel::{self, GroupSpec, Prepare, Refusal, TargetSpec};
use crate::refuse;
use common::{FnGuard, init_names};

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

/// Canonical code fingerprints, format v2 (3.14.9 / 3.15.1;
/// `python -m rustique._takeover._dev report --only logging --import logging`).
mod fp {
    pub type F = &'static [(u8, u8, &'static str)];
    pub const IS_ENABLED_FOR: F = &[(3, 14, "4b19cbe6ef53dfdc"), (3, 15, "1feed9bbf2748015")];
    pub const DEBUG: F = &[(3, 14, "5b37f630ccbffc87"), (3, 15, "9359039f35a6508d")];
    pub const INFO: F = &[(3, 14, "14d34b74d3ce337d"), (3, 15, "f454321ee7257264")];
    pub const WARNING: F = &[(3, 14, "5dd1266167632a7a"), (3, 15, "9c6f8a74b17a65f6")];
    pub const ERROR: F = &[(3, 14, "36c727c5f18c0404"), (3, 15, "35b7db8f61d5cd21")];
    pub const CRITICAL: F = &[(3, 14, "fdff94c7ce04c180"), (3, 15, "9ea9f7b556c2f9ae")];
    pub const LOG: F = &[(3, 14, "03eb8d20fb00ca09"), (3, 15, "2f85c2a60c4e938b")];
    pub const FIND_CALLER: F = &[(3, 14, "051c7d1ade195d43"), (3, 15, "90bb56ff81078d83")];
    pub const IS_INTERNAL_FRAME: F = &[(3, 14, "74c07bd564317a01"), (3, 15, "f195365943801169")];
    pub const CURRENTFRAME: F = &[(3, 14, "373c0ac3cd9b72b9"), (3, 15, "cb9c590d8e8dc183")];
    pub const NORMCASE: F = &[(3, 14, "0c979efa01cde1ee"), (3, 15, "20a2c8c5e6360108")];
    pub const RECORD_INIT: F = &[(3, 14, "a0cc2bcf443c5336"), (3, 15, "a18f791489619827")];
    pub const GET_LEVEL_NAME: F = &[(3, 14, "d0c968d740ae41a3"), (3, 15, "9f633f561185776d")];
    pub const BASENAME: F = &[(3, 14, "bacc0b3f6c53df7d"), (3, 15, "a7e4e60fb5d2bc37")];
    pub const SPLITEXT: F = &[(3, 14, "73ba50ede7290757"), (3, 15, "8496dfae04f8b3f5")];
    pub const SPLITEXT_: F = &[(3, 14, "41f984dfa9547d91"), (3, 15, "4779d796fd294850")];
    pub const GET_SEP: F = &[(3, 14, "54be3703c91faa4c"), (3, 15, "5bc506a017985796")];
    pub const CURRENT_THREAD: F = &[(3, 14, "21539b44e3d4b637"), (3, 15, "dd60c13386a48011")];
    pub const THREAD_NAME: F = &[(3, 14, "0025e16a9abde810"), (3, 15, "d85e9eaeab19ac61")];
    pub const FORMAT_TIME: F = &[(3, 14, "900c2986e5c7edb2"), (3, 15, "874dc0c46915f13a")];
    pub const FORMAT: F = &[(3, 14, "7c604339eabb3201"), (3, 15, "55b379d32e49441d")];
    pub const USES_TIME: F = &[(3, 14, "ec504c20ccc4b437"), (3, 15, "3ab4b749587cfa25")];
    pub const FORMAT_MESSAGE: F = &[(3, 14, "1968b56bafd69edf"), (3, 15, "55c983ab6802468d")];
    pub const STYLE_USES_TIME: F = &[(3, 14, "a886d2be8f7b955b"), (3, 15, "86a4567930e0f2d5")];
    pub const STYLE_FORMAT: F = &[(3, 14, "9e9cb421d439dcef"), (3, 15, "159ae7f812f7f284")];
    pub const STYLE_FORMAT_: F = &[(3, 14, "9cb3dc55d4d49190"), (3, 15, "61a43ff8b7fd536f")];
    pub const GET_MESSAGE: F = &[(3, 14, "949189edb75c65ff"), (3, 15, "838cd95f22dc3a6f")];
    pub const LOCALTIME: F = &[(3, 14, "c880d99c78680a77"), (3, 15, "c880d99c78680a77")];
    pub const GMTIME: F = &[(3, 14, "bebb183e765c46a2"), (3, 15, "bebb183e765c46a2")];
    pub const STRFTIME: F = &[(3, 14, "d7799c4aa3b998d3"), (3, 15, "d7799c4aa3b998d3")];
    pub const TIME_NS: F = &[(3, 14, "cfd8cf62a2e81346"), (3, 15, "cfd8cf62a2e81346")];
    pub const GET_IDENT: F = &[(3, 14, "aefc494fea7ff696"), (3, 15, "aefc494fea7ff696")];
    pub const GETPID: F = &[(3, 14, "e7b51de1800a9fd6"), (3, 15, "e7b51de1800a9fd6")];
}

/// `Thread.name`'s getter compiled with `-O` (no `assert`), per minor version.
const THREAD_NAME_O: &[(u8, &str)] = &[(14, "c8ab4ce5ca26be63"), (15, "bfd220e44371f430")];

fn ty<'py>(obj: Bound<'py, PyAny>) -> Result<Bound<'py, PyType>, Refusal> {
    obj.cast_into::<PyType>().map_err(|_| Refusal::new("not a class"))
}

fn module_dict<'py>(cx: &Prepare<'py>, name: &str) -> Result<Bound<'py, PyDict>, Refusal> {
    Ok(cx.loaded_module(name)?.dict())
}

/// An exact pure-Python function, fingerprinted, recorded in `guard`.
fn function<'py>(
    cx: &mut Prepare<'py>,
    f: &Bound<'py, PyAny>,
    expected: fp::F,
    guard: &FnGuard,
) -> Result<(), Refusal> {
    if f.cast_exact::<pyo3::types::PyFunction>().is_err() {
        return Err(refuse!("{f}: not a pure-Python function"));
    }
    // SAFETY: an exact function object.
    let code = unsafe { (*f.as_ptr().cast::<pyo3::ffi::PyFunctionObject>()).func_code } as usize;
    if !verified(code, expected) {
        cx.fingerprint_code(f, expected)?;
        // SAFETY: the function's code object (alive: borrowed, then a new reference).
        remember(code, expected, unsafe { Bound::from_borrowed_ptr(f.py(), code as *mut pyo3::ffi::PyObject) });
    }
    guard.set(f);
    Ok(())
}

/// A C function (builtin), fingerprinted.
fn c_function<'py>(cx: &mut Prepare<'py>, f: &Bound<'py, PyAny>, expected: fp::F) -> Result<(), Refusal> {
    if !verified(f.as_ptr() as usize, expected) {
        cx.fingerprint_c(f, expected)?;
        remember(f.as_ptr() as usize, expected, f.clone());
    }
    Ok(())
}

/// Objects (code objects, C functions) whose fingerprint this process already checked, so
/// targets of the group sharing a helper check it once (install cost). Each entry keeps its
/// object alive: the address cannot be reused by another object.
static VERIFIED: std::sync::Mutex<Vec<(usize, usize)>> = std::sync::Mutex::new(Vec::new());

fn verified(addr: usize, expected: fp::F) -> bool {
    VERIFIED.lock().is_ok_and(|v| v.iter().any(|&(a, e)| a == addr && e == expected.as_ptr() as usize))
}

fn remember(addr: usize, expected: fp::F, keep: Bound<'_, PyAny>) {
    if let Ok(mut v) = VERIFIED.lock() {
        v.push((addr, expected.as_ptr() as usize));
        std::mem::forget(keep.unbind());
    }
}

/// `dict[key]` with a pre-interned key, refusing when absent.
fn get_s<'py>(
    cx: &Prepare<'py>,
    d: &Bound<'_, PyDict>,
    key: &Bound<'_, PyString>,
) -> Result<Bound<'py, PyAny>, Refusal> {
    cx.get_opt_s(d, key)?.ok_or_else(|| refuse!("missing {key}"))
}

/// `module.name is expected`, refusing otherwise.
fn same<'py>(
    cx: &Prepare<'py>,
    d: &Bound<'_, PyDict>,
    key: &Bound<'_, PyString>,
    expected: &Bound<'_, PyAny>,
) -> Result<(), Refusal> {
    if get_s(cx, d, key)?.is(expected) { Ok(()) } else { Err(refuse!("{key} is not the expected object")) }
}

// ---------------------------------------------------------------------------------------
// Shared declarations of the group (GroupSpec): module identities and dependencies more
// than one target reads, the time functions, the record classes, process-wide facts.
// ---------------------------------------------------------------------------------------

fn prepare_shared(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if !cfg!(unix) {
        // The kernels reproduce `os.path` as `posixpath` (findCaller's `normcase`, the
        // record's `basename`/`splitext`) and convert times with `localtime_r`; on Windows
        // `os.path` is `ntpath` (normcase via `_winapi.LCMapStringEx`, drive-aware
        // basename). An expected refusal there (`platform`), not a dependency surprise.
        return Err(Refusal::with(
            crate::accel::spec::Category::Platform,
            "os.path is ntpath: logging kernels validated with posixpath only",
        ));
    }
    init_names();
    cx.symbol(c"_PyObject_GetMethod", &common::GET_METHOD)?;
    calibrate(cx)?;
    let deps = &common::SHARED_DEPS;
    let g = cx.module()?.dict();
    let time = cx.loaded_module("time")?;
    let os_mod = cx.loaded_module("os")?;
    let pp_mod = cx.loaded_module("posixpath")?;
    let sys_mod = cx.loaded_module("sys")?;
    same(cx, &g, intern!(py, "time"), time.as_any())?;
    same(cx, &g, intern!(py, "os"), os_mod.as_any())?;
    same(cx, &g, intern!(py, "sys"), sys_mod.as_any())?;
    let (os, pp) = (os_mod.dict(), pp_mod.dict());
    same(cx, &os, intern!(py, "path"), pp_mod.as_any())?;
    same(cx, &pp, intern!(py, "os"), os_mod.as_any())?;
    for k in [intern!(py, "time"), intern!(py, "os"), intern!(py, "sys")] {
        cx.pin_s(&g, k, deps)?;
    }
    cx.pin_s(&os, intern!(py, "path"), deps)?;
    cx.pin_s(&os, intern!(py, "fspath"), deps)?;
    cx.pin_s(&pp, intern!(py, "os"), deps)?;
    cx.pin_global_s(&g, intern!(py, "isinstance"), deps)?;
    cx.pin_global_s(&g, intern!(py, "int"), deps)?;
    // time.localtime / gmtime / strftime (formatTime and format)
    let td = time.dict();
    let lt = get_s(cx, &td, intern!(py, "localtime"))?;
    let gt = get_s(cx, &td, intern!(py, "gmtime"))?;
    let sf = get_s(cx, &td, intern!(py, "strftime"))?;
    c_function(cx, &lt, fp::LOCALTIME)?;
    c_function(cx, &gt, fp::GMTIME)?;
    c_function(cx, &sf, fp::STRFTIME)?;
    format::LOCALTIME.set_obj(&lt);
    format::GMTIME.set_obj(&gt);
    format::STRFTIME.set_obj(&sf);
    cx.pin_s(&td, intern!(py, "strftime"), deps)?;
    // record classes read by the formatting kernels
    {
        let n = common::names();
        record::RECORDS.set_names([n.created, n.msecs]);
        record::THREADS.set_names([n.t_initialized, n.t_name]);
        record::MP_PROCS.set_names([n.t_name]);
        level::LOGGERS.set_names([n.disabled, n.cache]);
        format::TIME_FORMATTERS.set_names([n.converter, n.default_time_format, n.default_msec_format]);
        format::FORMATTERS.set_names([
            n.converter,
            n.default_time_format,
            n.default_msec_format,
            n.style,
            n.datefmt,
            n.uses_time,
            n.format_time,
            n.format_message,
        ]);
        format::STYLES.set_names([n.fmt, n.asctime_search, n.defaults, n.uses_time, n.format, n.format_]);
        format::FMT_RECORDS.set_names([
            n.msg,
            n.args,
            n.get_message,
            n.exc_info,
            n.exc_text,
            n.stack_info,
            n.created,
            n.msecs,
        ]);
    }
    let lr = ty(cx.module_attr("LogRecord")?)?;
    format::R_GET_MESSAGE.set_obj(&cx.type_attr(&lr, "getMessage")?);
    format::R_DICT.set_obj(&cx.type_attr(&lr, "__dict__")?);
    format::clear_type_caches();
    Ok(())
}

pub static SHARED: GroupSpec = GroupSpec { name: "logging", prepare: prepare_shared };

// ---------------------------------------------------------------------------------------
// logging.Logger.level
// ---------------------------------------------------------------------------------------

fn prepare_level(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let logger = ty(cx.module_attr("Logger")?)?;
    let ief = cx.type_attr(&logger, "isEnabledFor")?;
    function(cx, &ief, fp::IS_ENABLED_FOR, &level::CODES[0])?;
    level::IS_ENABLED_FOR.set_obj(&ief);
    let consts = [
        intern!(py, "DEBUG"),
        intern!(py, "INFO"),
        intern!(py, "WARNING"),
        intern!(py, "ERROR"),
        intern!(py, "CRITICAL"),
    ];
    for (i, k) in consts.into_iter().enumerate() {
        let v = get_s(cx, &g, k)?;
        if !v.is_exact_instance_of::<pyo3::types::PyInt>() {
            return Err(refuse!("logging.{k} is not an int"));
        }
        cx.pin_s(&g, k, &level::DEPS)?;
        level::LEVELS[i].set_obj(&v);
    }
    level::LOGGERS.clear();
    let methods: [(&str, fp::F, Vc); 6] = [
        ("debug", fp::DEBUG, level::vc_debug),
        ("info", fp::INFO, level::vc_info),
        ("warning", fp::WARNING, level::vc_warning),
        ("error", fp::ERROR, level::vc_error),
        ("critical", fp::CRITICAL, level::vc_critical),
        ("log", fp::LOG, level::vc_log),
    ];
    for (i, (name, f, k)) in methods.into_iter().enumerate() {
        let m = cx.type_attr(&logger, name)?;
        function(cx, &m, f, &level::CODES[i + 1])?;
        cx.vectorcall(&m, k, &accel::FUNCTION_VECTORCALL)?;
    }
    Ok(())
}

type Vc = pyo3::ffi::vectorcallfunc;

// ---------------------------------------------------------------------------------------
// logging.Logger.findCaller
// ---------------------------------------------------------------------------------------

fn prepare_caller(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let logger = ty(cx.module_attr("Logger")?)?;
    let fc = cx.type_attr(&logger, "findCaller")?;
    function(cx, &fc, fp::FIND_CALLER, &caller::CODES[0])?;
    let iif = get_s(cx, &g, intern!(py, "_is_internal_frame"))?;
    function(cx, &iif, fp::IS_INTERNAL_FRAME, &caller::CODES[1])?;
    let cf = get_s(cx, &g, intern!(py, "currentframe"))?;
    function(cx, &cf, fp::CURRENTFRAME, &caller::CODES[2])?;
    let src = get_s(cx, &g, intern!(py, "_srcfile"))?;
    if !src.is_exact_instance_of::<PyString>() {
        return Err(Refusal::new("logging._srcfile is not a str"));
    }
    caller::SRCFILE.set_obj(&src);
    for k in [intern!(py, "_is_internal_frame"), intern!(py, "currentframe"), intern!(py, "_srcfile")] {
        cx.pin_s(&g, k, &caller::DEPS)?;
    }
    cx.pin_s(&module_dict(cx, "sys")?, intern!(py, "_getframe"), &caller::DEPS)?;
    let pp = module_dict(cx, "posixpath")?;
    let normcase = get_s(cx, &pp, intern!(py, "normcase"))?;
    function(cx, &normcase, fp::NORMCASE, &caller::CODES[3])?;
    cx.pin_s(&pp, intern!(py, "normcase"), &caller::DEPS)?;
    caller::IMPORTLIB.set_obj(intern!(py, "importlib").as_any());
    caller::BOOTSTRAP.set_obj(intern!(py, "_bootstrap").as_any());
    cx.vectorcall(&fc, caller::vc_find_caller, &accel::FUNCTION_VECTORCALL)
}

/// Calibrate the inline-values reads on `logging.root` (a RootLogger: generic attributes,
/// no descriptors for these names). A failed calibration only disables the fast reads.
fn calibrate(cx: &Prepare<'_>) -> Result<(), Refusal> {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return Ok(()); // layout facts: once per process
    }
    common::calibrate_dict_kind(cx.py());
    let root = cx.module_attr("root")?;
    common::calibrate_inline(
        cx.py(),
        &root,
        &["name", "level", "parent", "propagate", "handlers", "disabled", "_cache"],
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------
// logging.LogRecord.__init__
// ---------------------------------------------------------------------------------------

fn prepare_record(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    use std::sync::atomic::Ordering::Relaxed;
    let g = cx.module()?.dict();
    let lr = ty(cx.module_attr("LogRecord")?)?;
    let init = cx.type_attr(&lr, "__init__")?;
    function(cx, &init, fp::RECORD_INIT, &record::CODES[0])?;
    let gln = get_s(cx, &g, intern!(py, "getLevelName"))?;
    function(cx, &gln, fp::GET_LEVEL_NAME, &record::CODES[1])?;
    let deps = &record::DEPS;
    let switches = [
        intern!(py, "logThreads"),
        intern!(py, "logMultiprocessing"),
        intern!(py, "logProcesses"),
        intern!(py, "logAsyncioTasks"),
    ];
    for k in [
        intern!(py, "getLevelName"),
        intern!(py, "_levelToName"),
        intern!(py, "_nameToLevel"),
        intern!(py, "threading"),
        intern!(py, "collections"),
        intern!(py, "_startTime"),
    ]
    .into_iter()
    .chain(switches)
    {
        cx.pin_s(&g, k, deps)?;
    }
    for k in [intern!(py, "len"), intern!(py, "hasattr")] {
        cx.pin_global_s(&g, k, deps)?;
    }
    let l2n = get_s(cx, &g, intern!(py, "_levelToName"))?;
    let n2l = get_s(cx, &g, intern!(py, "_nameToLevel"))?;
    if !l2n.is_exact_instance_of::<PyDict>() || !n2l.is_exact_instance_of::<PyDict>() {
        return Err(Refusal::new("level name tables are not dicts"));
    }
    record::LEVEL_TO_NAME.set_obj(&l2n);
    accel::guard::watch_dict_generation(l2n.cast::<PyDict>().map_err(PyErr::from)?, &record::L2N_GEN)?;
    record::NAME_TO_LEVEL.set_obj(&n2l);
    let start: i64 = get_s(cx, &g, intern!(py, "_startTime"))?.extract().map_err(|_| Refusal::new("_startTime"))?;
    record::START_TIME.store(start as u64, Relaxed);
    let mut sw = 0usize;
    for (bit, k) in switches.into_iter().enumerate() {
        let v = get_s(cx, &g, k)?;
        if v.is(pyo3::types::PyBool::new(py, true).as_any()) {
            sw |= 1 << bit;
        } else if !v.is(pyo3::types::PyBool::new(py, false).as_any()) {
            return Err(refuse!("logging.{k} is not a bool"));
        }
    }
    record::SWITCHES.store(sw, Relaxed);
    // time.time_ns, os.getpid
    let td = module_dict(cx, "time")?;
    c_function(cx, &get_s(cx, &td, intern!(py, "time_ns"))?, fp::TIME_NS)?;
    cx.pin_s(&td, intern!(py, "time_ns"), deps)?;
    let os = module_dict(cx, "os")?;
    c_function(cx, &get_s(cx, &os, intern!(py, "getpid"))?, fp::GETPID)?;
    cx.pin_s(&os, intern!(py, "getpid"), deps)?;
    // os.path.basename / splitext
    let pp = module_dict(cx, "posixpath")?;
    let bn = get_s(cx, &pp, intern!(py, "basename"))?;
    function(cx, &bn, fp::BASENAME, &record::CODES[2])?;
    let se = get_s(cx, &pp, intern!(py, "splitext"))?;
    function(cx, &se, fp::SPLITEXT, &record::CODES[3])?;
    let gs = get_s(cx, &pp, intern!(py, "_get_sep"))?;
    function(cx, &gs, fp::GET_SEP, &record::CODES[7])?;
    for k in [intern!(py, "basename"), intern!(py, "splitext"), intern!(py, "_get_sep"), intern!(py, "genericpath")] {
        cx.pin_s(&pp, k, deps)?;
    }
    cx.pin_global_s(&pp, intern!(py, "isinstance"), deps)?;
    let gp_mod = cx.loaded_module("genericpath")?;
    same(cx, &pp, intern!(py, "genericpath"), gp_mod.as_any())?;
    let gp = gp_mod.dict();
    let sx = get_s(cx, &gp, intern!(py, "_splitext"))?;
    function(cx, &sx, fp::SPLITEXT_, &record::CODES[4])?;
    cx.pin_s(&gp, intern!(py, "_splitext"), deps)?;
    // threading
    let th = cx.loaded_module("threading")?;
    same(cx, &g, intern!(py, "threading"), th.as_any())?;
    let thd = th.dict();
    let gi = get_s(cx, &thd, intern!(py, "get_ident"))?;
    if !gi.is(&get_s(cx, &module_dict(cx, "_thread")?, intern!(py, "get_ident"))?) {
        return Err(Refusal::new("threading.get_ident is not _thread.get_ident"));
    }
    c_function(cx, &gi, fp::GET_IDENT)?;
    let ct = get_s(cx, &thd, intern!(py, "current_thread"))?;
    function(cx, &ct, fp::CURRENT_THREAD, &record::CODES[5])?;
    for k in [intern!(py, "get_ident"), intern!(py, "current_thread"), intern!(py, "_active")] {
        cx.pin_s(&thd, k, deps)?;
    }
    let active = get_s(cx, &thd, intern!(py, "_active"))?;
    if !active.is_exact_instance_of::<PyDict>() {
        return Err(Refusal::new("threading._active is not a dict"));
    }
    record::THREAD_ACTIVE.set_obj(&active);
    let thread = ty(get_s(cx, &thd, intern!(py, "Thread"))?)?;
    let prop = cx.type_attr(&thread, "name")?;
    // SAFETY: type check of a live object.
    if unsafe { pyo3::ffi::Py_TYPE(prop.as_ptr()) } != &raw mut pyo3::ffi::PyProperty_Type {
        return Err(Refusal::new("Thread.name is not a property"));
    }
    let fget = prop.getattr("fget")?;
    // `assert self._initialized` is stripped under `python -O`: accept either bytecode
    // (the kernel reads `_initialized` either way and falls back unless it is True).
    let fget_fp = cx.fingerprint_of(&fget)?;
    let (_, minor) = cx.version();
    if THREAD_NAME_O.iter().any(|&(m, f)| m == minor && f == fget_fp) {
        if fget.cast_exact::<pyo3::types::PyFunction>().is_err() {
            return Err(Refusal::new("Thread.name getter is not a function"));
        }
        record::CODES[6].set(&fget);
    } else {
        function(cx, &fget, fp::THREAD_NAME, &record::CODES[6])?;
    }
    record::THREAD_NAME_PROP.set_obj(&prop);
    // sys.modules, collections.abc.Mapping, _abc.get_cache_token
    let sysd = module_dict(cx, "sys")?;
    cx.pin_s(&sysd, intern!(py, "modules"), deps)?;
    record::SYS_MODULES.set_obj(&get_s(cx, &sysd, intern!(py, "modules"))?);
    let coll = cx.loaded_module("collections")?;
    same(cx, &g, intern!(py, "collections"), coll.as_any())?;
    cx.pin_s(&coll.dict(), intern!(py, "abc"), deps)?;
    let cabc = get_s(cx, &coll.dict(), intern!(py, "abc"))?
        .cast_into::<PyModule>()
        .map_err(|_| Refusal::new("collections.abc"))?;
    cx.pin_s(&cabc.dict(), intern!(py, "Mapping"), deps)?;
    match cx.loaded_module("_abc").and_then(|m| get_s(cx, &m.dict(), intern!(py, "get_cache_token"))) {
        Ok(tok) => record::ABC_TOKEN.set_obj(&tok),
        Err(_) => record::ABC_TOKEN.clear_ptr(),
    }
    record::RECORDS.clear();
    record::THREADS.clear();
    record::MP_PROCS.clear();
    record::reset_mapping_cache();
    cx.vectorcall(&init, record::vc_record_init, &accel::FUNCTION_VECTORCALL)
}

// ---------------------------------------------------------------------------------------
// logging.Formatter.formatTime / .format
// ---------------------------------------------------------------------------------------

fn prepare_format_time(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if !cfg!(unix) {
        // The kernel converts with `localtime_r`/`gmtime_r` (Unix only): it would always fall back.
        return Err(Refusal::with(
            crate::accel::spec::Category::Platform,
            "localtime_r/gmtime_r: only validated on Unix",
        ));
    }
    let fmt = ty(cx.module_attr("Formatter")?)?;
    let ft = cx.type_attr(&fmt, "formatTime")?;
    function(cx, &ft, fp::FORMAT_TIME, &format::TIME_CODES[0])?;
    cx.vectorcall(&ft, format::vc_format_time, &accel::FUNCTION_VECTORCALL)
}

fn prepare_format(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    cx.pin_global_s(&g, intern!(py, "str"), &format::FORMAT_DEPS)?;
    let fmt = ty(cx.module_attr("Formatter")?)?;
    let style = ty(cx.module_attr("PercentStyle")?)?;
    let lr = ty(cx.module_attr("LogRecord")?)?;
    let f = cx.type_attr(&fmt, "format")?;
    let codes = &format::FORMAT_CODES;
    function(cx, &f, fp::FORMAT, &codes[0])?;
    let ft = cx.type_attr(&fmt, "formatTime")?;
    function(cx, &ft, fp::FORMAT_TIME, &codes[1])?;
    let ut = cx.type_attr(&fmt, "usesTime")?;
    function(cx, &ut, fp::USES_TIME, &codes[2])?;
    let fm = cx.type_attr(&fmt, "formatMessage")?;
    function(cx, &fm, fp::FORMAT_MESSAGE, &codes[3])?;
    let sut = cx.type_attr(&style, "usesTime")?;
    function(cx, &sut, fp::STYLE_USES_TIME, &codes[4])?;
    let sf = cx.type_attr(&style, "format")?;
    function(cx, &sf, fp::STYLE_FORMAT, &codes[5])?;
    let sf_ = cx.type_attr(&style, "_format")?;
    function(cx, &sf_, fp::STYLE_FORMAT_, &codes[6])?;
    let gm = cx.type_attr(&lr, "getMessage")?;
    function(cx, &gm, fp::GET_MESSAGE, &codes[7])?;
    format::F_USES_TIME.set_obj(&ut);
    format::F_FORMAT_TIME.set_obj(&ft);
    format::F_FORMAT_MESSAGE.set_obj(&fm);
    format::S_USES_TIME.set_obj(&sut);
    format::S_FORMAT.set_obj(&sf);
    format::S_FORMAT_.set_obj(&sf_);
    let search = cx.type_attr(&style, "asctime_search")?;
    if search.is_exact_instance_of::<PyString>() {
        format::ASCTIME_SEARCH.set_obj(&search);
    }
    cx.vectorcall(&f, format::vc_format, &accel::FUNCTION_VECTORCALL)
}

// ---------------------------------------------------------------------------------------
// Specs
// ---------------------------------------------------------------------------------------

macro_rules! target {
    ($id:ident, $name:literal, $counters:expr, $prepare:ident, $checkable:expr) => {
        pub static $id: TargetSpec = TargetSpec {
            name: $name,
            group: "logging",
            module: Some("logging"),
            versions: VERSIONS,
            free_threaded: true,
            counters: $counters,
            prepare: $prepare,
            checkable: $checkable,
            shared: Some(&SHARED),
            ..TargetSpec::DEFAULT
        };
    };
}

// Checkable: the kernel's exact subset has no side effects (a disabled level call, the
// frame walk, formatTime). `format` and `__init__` store
// into the record.
/// Net benefit (ACCELERATION §5.6, measured on `L.info("x %s", 5)` / `L.debug(...)`,
/// 3.15.1): a disabled call saves ~500 Ir, an enabled one costs ~+800 Ir (kernel checks +
/// vectorcall fallback): credit 5 / debit 7 keeps the target while ≥ ~58% of the calls
/// are disabled ones.
pub static LEVEL: TargetSpec = TargetSpec {
    name: "logging.Logger.level",
    group: "logging",
    module: Some("logging"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &level::STATS,
    prepare: prepare_level,
    checkable: true,
    shared: Some(&SHARED),
    hit_credit: 5,
    fallback_debit: 7,
    ..TargetSpec::DEFAULT
};
target!(FIND_CALLER, "logging.Logger.findCaller", &caller::STATS, prepare_caller, true);
target!(RECORD_INIT, "logging.LogRecord.__init__", &record::STATS, prepare_record, false);
target!(FORMAT_TIME, "logging.Formatter.formatTime", &format::TIME_STATS, prepare_format_time, true);
target!(FORMAT, "logging.Formatter.format", &format::FORMAT_STATS, prepare_format, false);

pub static TARGETS: &[&TargetSpec] = &[&LEVEL, &FIND_CALLER, &RECORD_INIT, &FORMAT_TIME, &FORMAT];

/// Test helpers on `rustique._rustique._accel`: forget the logging kernels' learned state
/// (validated types, ABC answers).
#[pyfunction]
fn _logging_reset_caches() {
    level::LOGGERS.clear();
    record::RECORDS.clear();
    record::THREADS.clear();
    record::MP_PROCS.clear();
    record::reset_mapping_cache();
    format::clear_type_caches();
}

/// Test helper: the kernel's `(created, msecs, relativeCreated)` for `ct` nanoseconds.
#[pyfunction]
fn _logging_times(ct: i64) -> Option<(f64, f64, f64)> {
    record::record_times(ct, record::START_TIME.load(std::sync::atomic::Ordering::Relaxed) as i64)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(_logging_reset_caches, m)?)?;
    m.add_function(wrap_pyfunction!(_logging_times, m)?)
}
