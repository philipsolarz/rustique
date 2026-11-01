//! Group `logging`: the default record path of `logging`, piecewise (docs/targets/logging.md).
//!
//! | target | site(s) | kernel |
//! |---|---|---|
//! | `logging.Logger.level` | `Logger.isEnabledFor`, `.debug/.info/.warning/.error/.critical/.log` | disabled calls return without a frame |
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

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule, PyType};

use crate::accel::{self, DepFlag, Prepare, Refusal, TargetSpec};
use crate::refuse;
use common::{FnGuard, init_names};

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

/// Canonical code fingerprints (3.14.9 / 3.15.1; `_accel.fingerprint_report`).
mod fp {
    pub type F = &'static [(u8, u8, &'static str)];
    pub const IS_ENABLED_FOR: F = &[(3, 14, "83bf414059d64068"), (3, 15, "8a72e8093e7e446f")];
    pub const DEBUG: F = &[(3, 14, "980c2500c7c9e21c"), (3, 15, "3d1cb2b08e931295")];
    pub const INFO: F = &[(3, 14, "2814e23841bbf51c"), (3, 15, "78b2eee239ad0543")];
    pub const WARNING: F = &[(3, 14, "75301d32b2648fa8"), (3, 15, "345193e7a6dd3fbd")];
    pub const ERROR: F = &[(3, 14, "16ed5527967367a9"), (3, 15, "58ef0e8adea81ed6")];
    pub const CRITICAL: F = &[(3, 14, "8ba6fc57eb61d4b0"), (3, 15, "9aed1075e2120d35")];
    pub const LOG: F = &[(3, 14, "06b05c084c7d6925"), (3, 15, "76bbe82bc787ba4e")];
    pub const FIND_CALLER: F = &[(3, 14, "2e9992a457f30853"), (3, 15, "897161efc98826ee")];
    pub const IS_INTERNAL_FRAME: F = &[(3, 14, "adf353af595b259a"), (3, 15, "a47e46caadd055fb")];
    pub const CURRENTFRAME: F = &[(3, 14, "2dea0b383feafda7"), (3, 15, "aa935c576b264416")];
    pub const NORMCASE: F = &[(3, 14, "4430685c31fdf17c"), (3, 15, "b8ae21472a1996ce")];
    pub const RECORD_INIT: F = &[(3, 14, "2998442a9e27e16c"), (3, 15, "b5d7a51585221081")];
    pub const GET_LEVEL_NAME: F = &[(3, 14, "1cf9a61d4b0bf8da"), (3, 15, "336df648b5d33820")];
    pub const BASENAME: F = &[(3, 14, "9697ad6c31eb4ec6"), (3, 15, "14820d2e64ed72c1")];
    pub const SPLITEXT: F = &[(3, 14, "57e4fff4ed5744e9"), (3, 15, "7b7f2fd89ce939cb")];
    pub const SPLITEXT_: F = &[(3, 14, "2e130b2724074482"), (3, 15, "426bad07995314f0")];
    pub const GET_SEP: F = &[(3, 14, "6b0a736a3526f1ed"), (3, 15, "ec659b080301f8d1")];
    pub const CURRENT_THREAD: F = &[(3, 14, "e471a2b4760d5b18"), (3, 15, "7d11fae4c25d5278")];
    pub const THREAD_NAME: F = &[(3, 14, "b1968a197b7d56e7"), (3, 15, "f388e87b180c5d05")];
    pub const FORMAT_TIME: F = &[(3, 14, "3ac5eeaba13fbad3"), (3, 15, "7d15694a6571d638")];
    pub const FORMAT: F = &[(3, 14, "ae34d46f632032a5"), (3, 15, "a166ddd08c26efd5")];
    pub const USES_TIME: F = &[(3, 14, "905acf6461f87ddb"), (3, 15, "4d0778f480652df4")];
    pub const FORMAT_MESSAGE: F = &[(3, 14, "86fc549352a0112a"), (3, 15, "cf5833ad08a02618")];
    pub const STYLE_USES_TIME: F = &[(3, 14, "1efd55fb17981f37"), (3, 15, "73e7ab21d4af385f")];
    pub const STYLE_FORMAT: F = &[(3, 14, "43e1359203ae8454"), (3, 15, "dfc26494c08a2531")];
    pub const STYLE_FORMAT_: F = &[(3, 14, "519a62c3e0630a40"), (3, 15, "8bac2b3ab413bc6e")];
    pub const GET_MESSAGE: F = &[(3, 14, "b0f2c5f2fda238b7"), (3, 15, "9412d473eb787d3c")];
    pub const LOCALTIME: F = &[(3, 14, "9cb2569a8b899a9d"), (3, 15, "9cb2569a8b899a9d")];
    pub const GMTIME: F = &[(3, 14, "1a06bd510a06a7f4"), (3, 15, "1a06bd510a06a7f4")];
    pub const STRFTIME: F = &[(3, 14, "4aca0e0472e239e6"), (3, 15, "4aca0e0472e239e6")];
    pub const TIME_NS: F = &[(3, 14, "ecd5528fcb9e4ab3"), (3, 15, "ecd5528fcb9e4ab3")];
    pub const GET_IDENT: F = &[(3, 14, "a1ade4129b6dcef0"), (3, 15, "a1ade4129b6dcef0")];
    pub const GETPID: F = &[(3, 14, "2cd2b8d54a017870"), (3, 15, "2cd2b8d54a017870")];
}

/// `Thread.name`'s getter compiled with `-O` (no `assert`), per minor version.
const THREAD_NAME_O: &[(u8, &str)] = &[(14, "3eea852412c3b4a0"), (15, "8b92be079aad8b80")];

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

// ---------------------------------------------------------------------------------------
// logging.Logger.level
// ---------------------------------------------------------------------------------------

fn prepare_level(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    init_names();
    cx.symbol(c"_PyObject_GetMethod", &common::GET_METHOD)?;
    let g = cx.module()?.dict();
    let logger = ty(cx.module_attr("Logger")?)?;
    let ief = cx.type_attr(&logger, "isEnabledFor")?;
    function(cx, &ief, fp::IS_ENABLED_FOR, &level::CODES[0])?;
    level::IS_ENABLED_FOR.set_obj(&ief);
    for (i, name) in ["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"].iter().enumerate() {
        let v = cx.get(&g, name)?;
        if !v.is_exact_instance_of::<pyo3::types::PyInt>() {
            return Err(refuse!("logging.{name} is not an int"));
        }
        cx.pin(&g, name, &level::DEPS)?;
        level::LEVELS[i].set_obj(&v);
    }
    cx.pin_global(&g, "isinstance", &level::DEPS)?;
    cx.pin_global(&g, "int", &level::DEPS)?;
    {
        let n = common::names();
        level::LOGGERS.set_names([n.disabled, n.cache]);
    }
    calibrate(cx)?;
    let methods: [(&str, fp::F, Vc); 6] = [
        ("debug", fp::DEBUG, level::vc_debug),
        ("info", fp::INFO, level::vc_info),
        ("warning", fp::WARNING, level::vc_warning),
        ("error", fp::ERROR, level::vc_error),
        ("critical", fp::CRITICAL, level::vc_critical),
        ("log", fp::LOG, level::vc_log),
    ];
    for (i, (name, f, _)) in methods.iter().enumerate() {
        let m = cx.type_attr(&logger, name)?;
        function(cx, &m, f, &level::CODES[i + 1])?;
    }
    cx.vectorcall(&ief, level::vc_is_enabled_for, &accel::FUNCTION_VECTORCALL)?;
    for (name, _, k) in methods {
        let m = cx.type_attr(&logger, name)?;
        cx.vectorcall(&m, k, &accel::FUNCTION_VECTORCALL)?;
    }
    Ok(())
}

type Vc = pyo3::ffi::vectorcallfunc;

// ---------------------------------------------------------------------------------------
// logging.Logger.findCaller
// ---------------------------------------------------------------------------------------

fn prepare_caller(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    init_names();
    let g = cx.module()?.dict();
    let logger = ty(cx.module_attr("Logger")?)?;
    let fc = cx.type_attr(&logger, "findCaller")?;
    function(cx, &fc, fp::FIND_CALLER, &caller::CODES[0])?;
    let iif = cx.get(&g, "_is_internal_frame")?;
    function(cx, &iif, fp::IS_INTERNAL_FRAME, &caller::CODES[1])?;
    let cf = cx.get(&g, "currentframe")?;
    function(cx, &cf, fp::CURRENTFRAME, &caller::CODES[2])?;
    let src = cx.get(&g, "_srcfile")?;
    if !src.is_exact_instance_of::<pyo3::types::PyString>() {
        return Err(Refusal::new("logging._srcfile is not a str"));
    }
    caller::SRCFILE.set_obj(&src);
    for name in ["_is_internal_frame", "currentframe", "_srcfile", "os", "sys"] {
        cx.pin(&g, name, &caller::DEPS)?;
    }
    let sys = module_dict(cx, "sys")?;
    if !cx.get(&g, "sys")?.is(cx.loaded_module("sys")?.as_any()) {
        return Err(Refusal::new("logging.sys is not sys"));
    }
    cx.pin(&sys, "_getframe", &caller::DEPS)?;
    let (os, pp) = os_path(cx, &g)?;
    let normcase = cx.get(&pp, "normcase")?;
    function(cx, &normcase, fp::NORMCASE, &caller::CODES[3])?;
    cx.pin(&os, "path", &caller::DEPS)?;
    cx.pin(&os, "fspath", &caller::DEPS)?;
    cx.pin(&pp, "normcase", &caller::DEPS)?;
    cx.pin(&pp, "os", &caller::DEPS)?;
    caller::IMPORTLIB.set_obj(pyo3::types::PyString::intern(py, "importlib").as_any());
    caller::BOOTSTRAP.set_obj(pyo3::types::PyString::intern(py, "_bootstrap").as_any());
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

/// `logging.os` is `os`, `os.path` is `posixpath` (and `posixpath.os` is `os`): the dicts
/// of `os` and `posixpath`.
fn os_path<'py>(
    cx: &Prepare<'py>,
    g: &Bound<'py, PyDict>,
) -> Result<(Bound<'py, PyDict>, Bound<'py, PyDict>), Refusal> {
    let os_mod = cx.loaded_module("os")?;
    if !cx.get(g, "os")?.is(os_mod.as_any()) {
        return Err(Refusal::new("logging.os is not os"));
    }
    let pp_mod = cx.loaded_module("posixpath")?;
    let os = os_mod.dict();
    if !cx.get(&os, "path")?.is(pp_mod.as_any()) {
        return Err(Refusal::new("os.path is not posixpath"));
    }
    let pp = pp_mod.dict();
    if !cx.get(&pp, "os")?.is(os_mod.as_any()) {
        return Err(Refusal::new("posixpath.os is not os"));
    }
    Ok((os, pp))
}

// ---------------------------------------------------------------------------------------
// logging.LogRecord.__init__
// ---------------------------------------------------------------------------------------

fn prepare_record(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    use std::sync::atomic::Ordering::Relaxed;
    init_names();
    let g = cx.module()?.dict();
    let lr = ty(cx.module_attr("LogRecord")?)?;
    let init = cx.type_attr(&lr, "__init__")?;
    function(cx, &init, fp::RECORD_INIT, &record::CODES[0])?;
    let gln = cx.get(&g, "getLevelName")?;
    function(cx, &gln, fp::GET_LEVEL_NAME, &record::CODES[1])?;
    let deps = &record::DEPS;
    for name in [
        "time",
        "getLevelName",
        "_levelToName",
        "_nameToLevel",
        "os",
        "threading",
        "sys",
        "collections",
        "logThreads",
        "logMultiprocessing",
        "logProcesses",
        "logAsyncioTasks",
        "_startTime",
    ] {
        cx.pin(&g, name, deps)?;
    }
    for name in ["isinstance", "len", "hasattr", "int"] {
        cx.pin_global(&g, name, deps)?;
    }
    let l2n = cx.get(&g, "_levelToName")?;
    let n2l = cx.get(&g, "_nameToLevel")?;
    if !l2n.is_exact_instance_of::<PyDict>() || !n2l.is_exact_instance_of::<PyDict>() {
        return Err(Refusal::new("level name tables are not dicts"));
    }
    record::LEVEL_TO_NAME.set_obj(&l2n);
    accel::guard::watch_dict_generation(l2n.cast::<PyDict>().map_err(PyErr::from)?, &record::L2N_GEN)?;
    record::NAME_TO_LEVEL.set_obj(&n2l);
    let start: i64 = cx.get(&g, "_startTime")?.extract().map_err(|_| Refusal::new("_startTime"))?;
    record::START_TIME.store(start as u64, Relaxed);
    let mut sw = 0usize;
    for (bit, name) in ["logThreads", "logMultiprocessing", "logProcesses", "logAsyncioTasks"].iter().enumerate() {
        let v = cx.get(&g, name)?;
        if v.is(pyo3::types::PyBool::new(cx.py(), true).as_any()) {
            sw |= 1 << bit;
        } else if !v.is(pyo3::types::PyBool::new(cx.py(), false).as_any()) {
            return Err(refuse!("logging.{name} is not a bool"));
        }
    }
    record::SWITCHES.store(sw, Relaxed);
    // time.time_ns
    let time = cx.loaded_module("time")?;
    if !cx.get(&g, "time")?.is(time.as_any()) {
        return Err(Refusal::new("logging.time is not time"));
    }
    let td = time.dict();
    c_function(cx, &cx.get(&td, "time_ns")?, fp::TIME_NS)?;
    cx.pin(&td, "time_ns", deps)?;
    // os.path.basename / splitext, os.getpid
    let (os, pp) = os_path(cx, &g)?;
    c_function(cx, &cx.get(&os, "getpid")?, fp::GETPID)?;
    for name in ["path", "getpid", "fspath"] {
        cx.pin(&os, name, deps)?;
    }
    let bn = cx.get(&pp, "basename")?;
    function(cx, &bn, fp::BASENAME, &record::CODES[2])?;
    let se = cx.get(&pp, "splitext")?;
    function(cx, &se, fp::SPLITEXT, &record::CODES[3])?;
    let gs = cx.get(&pp, "_get_sep")?;
    function(cx, &gs, fp::GET_SEP, &record::CODES[7])?;
    for name in ["basename", "splitext", "os", "_get_sep", "genericpath"] {
        cx.pin(&pp, name, deps)?;
    }
    cx.pin_global(&pp, "isinstance", deps)?;
    let gp_mod = cx.loaded_module("genericpath")?;
    if !cx.get(&pp, "genericpath")?.is(gp_mod.as_any()) {
        return Err(Refusal::new("posixpath.genericpath is not genericpath"));
    }
    let gp = gp_mod.dict();
    let sx = cx.get(&gp, "_splitext")?;
    function(cx, &sx, fp::SPLITEXT_, &record::CODES[4])?;
    cx.pin(&gp, "_splitext", deps)?;
    // threading
    let th = cx.loaded_module("threading")?;
    if !cx.get(&g, "threading")?.is(th.as_any()) {
        return Err(Refusal::new("logging.threading is not threading"));
    }
    let thd = th.dict();
    let thread_mod = cx.loaded_module("_thread")?.dict();
    let gi = cx.get(&thd, "get_ident")?;
    if !gi.is(&cx.get(&thread_mod, "get_ident")?) {
        return Err(Refusal::new("threading.get_ident is not _thread.get_ident"));
    }
    c_function(cx, &gi, fp::GET_IDENT)?;
    let ct = cx.get(&thd, "current_thread")?;
    function(cx, &ct, fp::CURRENT_THREAD, &record::CODES[5])?;
    for name in ["get_ident", "current_thread", "_active"] {
        cx.pin(&thd, name, deps)?;
    }
    let active = cx.get(&thd, "_active")?;
    if !active.is_exact_instance_of::<PyDict>() {
        return Err(Refusal::new("threading._active is not a dict"));
    }
    record::THREAD_ACTIVE.set_obj(&active);
    let thread = ty(cx.get(&thd, "Thread")?)?;
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
    let sys_mod = cx.loaded_module("sys")?;
    if !cx.get(&g, "sys")?.is(sys_mod.as_any()) {
        return Err(Refusal::new("logging.sys is not sys"));
    }
    cx.pin(&sys_mod.dict(), "modules", deps)?;
    record::SYS_MODULES.set_obj(&cx.get(&sys_mod.dict(), "modules")?);
    let coll = cx.loaded_module("collections")?;
    if !cx.get(&g, "collections")?.is(coll.as_any()) {
        return Err(Refusal::new("logging.collections is not collections"));
    }
    cx.pin(&coll.dict(), "abc", deps)?;
    let cabc = cx.get(&coll.dict(), "abc")?.cast_into::<PyModule>().map_err(|_| Refusal::new("collections.abc"))?;
    cx.pin(&cabc.dict(), "Mapping", deps)?;
    match cx.loaded_module("_abc").and_then(|m| cx.get(&m.dict(), "get_cache_token")) {
        Ok(tok) => record::ABC_TOKEN.set_obj(&tok),
        Err(_) => record::ABC_TOKEN.clear_ptr(),
    }
    {
        let n = common::names();
        record::RECORDS.set_names([n.created, n.msecs]);
        record::THREADS.set_names([n.t_initialized, n.t_name]);
        record::MP_PROCS.set_names([n.t_name]);
    }
    calibrate(cx)?;
    record::reset_mapping_cache();
    cx.vectorcall(&init, record::vc_record_init, &accel::FUNCTION_VECTORCALL)
}

// ---------------------------------------------------------------------------------------
// logging.Formatter.formatTime / .format
// ---------------------------------------------------------------------------------------

/// `logging.time` is `time`; capture `time.localtime/gmtime/strftime`; pin
/// `logging.time` and `time.strftime` on `deps`.
fn time_functions(cx: &mut Prepare<'_>, deps: &'static DepFlag) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let time = cx.loaded_module("time")?;
    if !cx.get(&g, "time")?.is(time.as_any()) {
        return Err(Refusal::new("logging.time is not time"));
    }
    let td = time.dict();
    let lt = cx.get(&td, "localtime")?;
    let gt = cx.get(&td, "gmtime")?;
    let sf = cx.get(&td, "strftime")?;
    c_function(cx, &lt, fp::LOCALTIME)?;
    c_function(cx, &gt, fp::GMTIME)?;
    c_function(cx, &sf, fp::STRFTIME)?;
    format::LOCALTIME.set_obj(&lt);
    format::GMTIME.set_obj(&gt);
    format::STRFTIME.set_obj(&sf);
    cx.pin(&g, "time", deps)?;
    cx.pin(&td, "strftime", deps)?;
    Ok(())
}

fn prepare_format_time(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    init_names();
    time_functions(cx, &format::TIME_DEPS)?;
    let fmt = ty(cx.module_attr("Formatter")?)?;
    let ft = cx.type_attr(&fmt, "formatTime")?;
    function(cx, &ft, fp::FORMAT_TIME, &format::TIME_CODES[0])?;
    prepare_record_types(cx)?;
    format::clear_type_caches();
    cx.vectorcall(&ft, format::vc_format_time, &accel::FUNCTION_VECTORCALL)
}

/// The record classes the formatting kernels read (`RECORDS` validation needs the names).
fn prepare_record_types(cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    {
        let n = common::names();
        record::RECORDS.set_names([n.created, n.msecs]);
        format::TIME_FORMATTERS.set_names([n.converter, n.default_time_format, n.default_msec_format]);
        format::FORMATTERS.set_names([n.converter, n.default_time_format, n.default_msec_format, n.style, n.datefmt]);
        format::STYLES.set_names([n.fmt, n.asctime_search, n.defaults]);
        format::FMT_RECORDS.set_names([n.msg, n.args]);
    }
    calibrate(cx)?;
    let lr = ty(cx.module_attr("LogRecord")?)?;
    let gm = cx.type_attr(&lr, "getMessage")?;
    format::R_GET_MESSAGE.set_obj(&gm);
    format::R_DICT.set_obj(&cx.type_attr(&lr, "__dict__")?);
    Ok(())
}

fn prepare_format(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    init_names();
    cx.symbol(c"_PyObject_GetMethod", &common::GET_METHOD)?;
    format::clear_fmt_cache();
    time_functions(cx, &format::FORMAT_DEPS)?;
    let g = cx.module()?.dict();
    cx.pin_global(&g, "str", &format::FORMAT_DEPS)?;
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
    prepare_record_types(cx)?;
    format::clear_type_caches();
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
            ..TargetSpec::DEFAULT
        };
    };
}

// Checkable: the kernel's exact subset has no side effects (a disabled level call, a
// cache hit of isEnabledFor, the frame walk, formatTime). `format` and `__init__` store
// into the record.
target!(LEVEL, "logging.Logger.level", &level::STATS, prepare_level, true);
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
