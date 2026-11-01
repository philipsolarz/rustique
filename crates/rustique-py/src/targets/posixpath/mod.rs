//! Group `posixpath`: Rust kernels behind `os.path` (= `posixpath`) functions, installed on
//! the existing function objects with `PyFunction_SetVectorcall` (contract kind C).
//!
//! Exact subset of every kernel: positional arguments that are all exact `str` (any PEP
//! 393 kind) or all exact `bytes`, no keywords (except `relpath(..., start=...)`), every
//! pinned dependency intact (`os.fspath`, `_get_sep`, ...), no tool listening. Results are
//! built with CPython's own slicing/concatenation identity rules: an argument comes back
//! as itself exactly where CPython returns it, constants are the code object's constants,
//! empty and one-character results are the shared singletons. Everything else (str/bytes
//! subclasses, `os.PathLike` objects, mixed str/bytes, errors) runs CPython's code.
//! See docs/targets/posixpath.md.

pub mod rt;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};

use rustique_core::paths::posix::{self, JoinArg, Joined, Piece};

use crate::accel::{self, DepFlag, Fingerprints, Prepare, Reason, Refusal, TargetSpec};
use crate::refuse;
use rt::{
    O, Rt, StrView, Text, Unit, bytes_view, exact_bytes, exact_str, nargs, new_str, str_from_units, str_view, subbytes,
    substr, write_text,
};

// ---------------------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------------------

/// Fingerprints of every original the kernels reproduce or call, recorded per minor version
/// with `rustique._rustique._accel.fingerprint(obj)` (identical on GIL and free-threaded
/// builds).
pub(crate) const FPS: &[(&str, Fingerprints)] = &[
    ("posixpath.join", &[(3, 14, "c5f7726e246711cc"), (3, 15, "1736eec0807db5fe")]),
    ("posixpath.split", &[(3, 14, "2a92a09f804e7f66"), (3, 15, "5b80ccd4010f9f07")]),
    ("posixpath.splitext", &[(3, 14, "57e4fff4ed5744e9"), (3, 15, "7b7f2fd89ce939cb")]),
    ("posixpath.basename", &[(3, 14, "9697ad6c31eb4ec6"), (3, 15, "14820d2e64ed72c1")]),
    ("posixpath.dirname", &[(3, 14, "0ed8402af932b639"), (3, 15, "9d669ca8789e07b1")]),
    ("posixpath.isabs", &[(3, 14, "eaad47df6dd6ed8b"), (3, 15, "135e5f019a3002ee")]),
    ("posixpath.relpath", &[(3, 14, "939e80226b486d55"), (3, 15, "7c09617a3d61fa23")]),
    ("posixpath.abspath", &[(3, 14, "5f8527ce7332a273"), (3, 15, "38577b59918fb8de")]),
    ("posixpath._get_sep", &[(3, 14, "6b0a736a3526f1ed"), (3, 15, "ec659b080301f8d1")]),
    ("genericpath._splitext", &[(3, 14, "2e130b2724074482"), (3, 15, "426bad07995314f0")]),
    ("genericpath.commonprefix", &[(3, 14, "60e08a2eb749cf79"), (3, 15, "48abda7c1b3bb7da")]),
    ("genericpath._commonprefix", &[(3, 15, "87fd0ffea61fdb85")]),
    ("os.fspath", &[(3, 14, "2dbb12933adb6fd8"), (3, 15, "2dbb12933adb6fd8")]),
    ("os.getcwd", &[(3, 14, "c9812d1c05053b8c"), (3, 15, "c9812d1c05053b8c")]),
    ("os.getcwdb", &[(3, 14, "2604f34e97fb5a20"), (3, 15, "2604f34e97fb5a20")]),
    ("posixpath.normpath", &[(3, 14, "9bc564ed2ef2fde6"), (3, 15, "9bc564ed2ef2fde6")]),
];

static RT_JOIN: Rt = Rt::new();
static RT_SPLIT: Rt = Rt::new();
static RT_SPLITEXT: Rt = Rt::new();
static RT_BASENAME: Rt = Rt::new();
static RT_DIRNAME: Rt = Rt::new();
static RT_ISABS: Rt = Rt::new();
static RT_RELPATH: Rt = Rt::new();
static RT_ABSPATH: Rt = Rt::new();

macro_rules! target {
    ($stat:ident, $name:literal, $rt:ident, $prep:ident) => {
        pub static $stat: TargetSpec = TargetSpec {
            name: $name,
            group: "posixpath",
            module: Some("posixpath"),
            versions: &[(3, 14), (3, 15)],
            free_threaded: true,
            counters: &$rt.counters,
            prepare: $prep,
            jit_safe: false,
            ..TargetSpec::DEFAULT
        };
    };
}

target!(JOIN, "posixpath.join", RT_JOIN, prep_join);
target!(SPLIT, "posixpath.split", RT_SPLIT, prep_split);
target!(SPLITEXT, "posixpath.splitext", RT_SPLITEXT, prep_splitext);
target!(BASENAME, "posixpath.basename", RT_BASENAME, prep_basename);
target!(DIRNAME, "posixpath.dirname", RT_DIRNAME, prep_dirname);
target!(ISABS, "posixpath.isabs", RT_ISABS, prep_isabs);
target!(RELPATH, "posixpath.relpath", RT_RELPATH, prep_relpath);
target!(ABSPATH, "posixpath.abspath", RT_ABSPATH, prep_abspath);

/// The group's targets, in install order.
pub static TARGETS: &[&TargetSpec] = &[&JOIN, &SPLIT, &SPLITEXT, &BASENAME, &DIRNAME, &ISABS, &RELPATH, &ABSPATH];

/// `posixpath.<attr>`: fingerprint it and return it.
fn func<'py>(cx: &mut Prepare<'py>, g: &Bound<'py, PyDict>, attr: &str) -> Result<Bound<'py, PyAny>, Refusal> {
    let f = cx.get(g, attr)?;
    rt::check_fp(cx, FPS, &format!("posixpath.{attr}"), &f)?;
    Ok(f)
}

/// `os.fspath` as `posixpath` reads it: `posixpath.os` (the os module) and `os.fspath`
/// (the C function), both pinned.
fn pin_fspath(cx: &mut Prepare<'_>, g: &Bound<'_, PyDict>, deps: &'static DepFlag) -> Result<(), Refusal> {
    let os = cx.get(g, "os")?;
    let osm = cx.loaded_module("os")?;
    if !os.is(&osm) {
        return Err(refuse!("posixpath.os is not the os module"));
    }
    cx.pin(g, "os", deps)?;
    let osd = osm.dict();
    let fspath = cx.get(&osd, "fspath")?;
    rt::check_c(cx, FPS, "os.fspath", &fspath)?;
    cx.pin(&osd, "fspath", deps)?;
    Ok(())
}

/// Pins shared by the functions built on `os.fspath` and `_get_sep`.
fn pin_sep(cx: &mut Prepare<'_>, g: &Bound<'_, PyDict>, deps: &'static DepFlag) -> Result<(), Refusal> {
    pin_fspath(cx, g, deps)?;
    let get_sep = cx.get(g, "_get_sep")?;
    rt::check_fp(cx, FPS, "posixpath._get_sep", &get_sep)?;
    cx.pin(g, "_get_sep", deps)?;
    cx.pin_global(g, "isinstance", deps)?;
    cx.pin_global(g, "bytes", deps)?;
    Ok(())
}

/// Layout calibrations every kernel of the group relies on (str buffers; plain str
/// subclasses, which fall back when unavailable).
fn init_layouts(cx: &Prepare<'_>) -> Result<(), Refusal> {
    if !rt::init_str_layout(cx.py()) {
        return Err(refuse!("str layout not calibrated"));
    }
    rt::init_plain(cx.py());
    Ok(())
}

fn prep_sep_fn(cx: &mut Prepare<'_>, attr: &str, rt: &'static Rt, kernel: ffi::vectorcallfunc) -> Result<(), Refusal> {
    init_layouts(cx)?;
    let g = cx.module()?.dict();
    let f = func(cx, &g, attr)?;
    pin_sep(cx, &g, &rt.deps)?;
    cx.vectorcall(&f, kernel, &accel::FUNCTION_VECTORCALL)
}

fn prep_join(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_sep_fn(cx, "join", &RT_JOIN, k_join)
}
fn prep_split(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_sep_fn(cx, "split", &RT_SPLIT, k_split)
}
fn prep_basename(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_sep_fn(cx, "basename", &RT_BASENAME, k_basename)
}
fn prep_dirname(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_sep_fn(cx, "dirname", &RT_DIRNAME, k_dirname)
}
fn prep_isabs(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_sep_fn(cx, "isabs", &RT_ISABS, k_isabs)
}

fn prep_splitext(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    init_layouts(cx)?;
    let g = cx.module()?.dict();
    let f = func(cx, &g, "splitext")?;
    let deps = &RT_SPLITEXT.deps;
    pin_fspath(cx, &g, deps)?;
    cx.pin_global(&g, "isinstance", deps)?;
    cx.pin_global(&g, "bytes", deps)?;
    let gp = cx.get(&g, "genericpath")?;
    if !gp.is(&cx.loaded_module("genericpath")?) {
        return Err(refuse!("posixpath.genericpath is not the genericpath module"));
    }
    cx.pin(&g, "genericpath", deps)?;
    let gd = cx.loaded_module("genericpath")?.dict();
    let se = cx.get(&gd, "_splitext")?;
    rt::check_fp(cx, FPS, "genericpath._splitext", &se)?;
    cx.pin(&gd, "_splitext", deps)?;
    cx.vectorcall(&f, k_splitext, &accel::FUNCTION_VECTORCALL)
}

/// Which exact type all arguments share.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ty {
    Str,
    Bytes,
}

/// # Safety
/// Live object.
#[inline(always)]
unsafe fn ty_of(o: O) -> Option<Ty> {
    // SAFETY: per the caller.
    unsafe {
        if exact_str(o) {
            Some(Ty::Str)
        } else if exact_bytes(o) {
            Some(Ty::Bytes)
        } else if ffi::PyUnicode_Check(o) != 0 && rt::plain_str_subclass(o) {
            Some(Ty::Str)
        } else {
            None
        }
    }
}

/// Common kernel skeleton: shape check, guards, body; a body returning `Ok(None)` declines
/// before anything observable, a null result clears the error and re-runs CPython.
macro_rules! kernel {
    ($name:ident, $rt:ident, |$func:ident, $args:ident, $nargsf:ident, $n:ident, $kw:ident| $body:expr) => {
        unsafe extern "C" fn $name($func: O, $args: *const O, $nargsf: usize, $kw: O) -> O {
            // SAFETY: vectorcall arguments from CPython; attached thread.
            unsafe {
                let $n = nargs($nargsf);
                let r: Option<O> = $body;
                match r {
                    None => $rt.fallback(Reason::Args, $func, $args, $nargsf, $kw),
                    Some(r) if r == GUARD_DEPS => $rt.fallback(Reason::Deps, $func, $args, $nargsf, $kw),
                    Some(r) if r == GUARD_TOOLS => $rt.fallback(Reason::Tools, $func, $args, $nargsf, $kw),
                    Some(r) if r.is_null() => {
                        ffi::PyErr_Clear();
                        $rt.fallback(Reason::Deopt, $func, $args, $nargsf, $kw)
                    }
                    Some(r) => {
                        $rt.counters.hit();
                        r
                    }
                }
            }
        }
    };
}

/// Sentinels a kernel body returns for "guard failed" (never valid object pointers).
const GUARD_DEPS: O = 1 as O;
const GUARD_TOOLS: O = 2 as O;

/// `Some(sentinel)` when a guard fails.
#[inline(always)]
unsafe fn guards(rt: &Rt, func: O) -> Option<O> {
    // SAFETY: attached thread.
    match unsafe { rt.ready(func) } {
        Ok(()) => None,
        Err(Reason::Deps) => Some(GUARD_DEPS),
        Err(_) => Some(GUARD_TOOLS),
    }
}

// ---------------------------------------------------------------------------------------
// join
// ---------------------------------------------------------------------------------------

const MAXA: usize = 8;

kernel!(k_join, RT_JOIN, |func, args, nargsf, n, kw| {
    if !kw.is_null() || n == 0 {
        None
    } else {
        match ty_of(*args) {
            Some(t) if (1..n).all(|k| ty_of(*args.add(k)) == Some(t)) => match guards(&RT_JOIN, func) {
                Some(g) => Some(g),
                None => Some(if t == Ty::Str { join_str(args, n) } else { join_bytes(args, n) }),
            },
            _ => None,
        }
    }
});

/// # Safety
/// `n >= 1` exact `str` arguments.
unsafe fn join_str(args: *const O, n: usize) -> O {
    // SAFETY: per the caller.
    unsafe {
        let mut views_s = [StrView { text: Text::U1(&[]), maxchar: 0x7f }; MAXA];
        let mut jargs_s = [JoinArg { len: 0, starts_sep: false, ends_sep: false }; MAXA];
        let mut pieces_s = [Piece::default(); MAXA];
        let (mut views_h, mut jargs_h, mut pieces_h);
        let (views, jargs, pieces): (&mut [StrView], &mut [JoinArg], &mut [Piece]) = if n <= MAXA {
            (&mut views_s[..n], &mut jargs_s[..n], &mut pieces_s[..n])
        } else {
            views_h = vec![StrView { text: Text::U1(&[]), maxchar: 0x7f }; n];
            jargs_h = vec![JoinArg { len: 0, starts_sep: false, ends_sep: false }; n];
            pieces_h = vec![Piece::default(); n];
            (&mut views_h[..], &mut jargs_h[..], &mut pieces_h[..])
        };
        for k in 0..n {
            let v = str_view(*args.add(k));
            views[k] = v;
            jargs[k] = crate::with_text!(v.text, |s| JoinArg::of(s));
        }
        let plan = match posix::join_plan(jargs, pieces) {
            // `path += b` with an empty `b` keeps `path` only on the exact-str fast path
            // (`PyUnicode_Append` with the empty singleton). With a str subclass on either
            // side CPython goes through `PyUnicode_Concat`, which returns a fresh exact copy.
            Joined::Arg(k) if k + 1 < n && (k..n).any(|j| !exact_str(*args.add(j))) => {
                pieces[0] = Piece { arg: k, sep_before: false };
                Joined::Build { npieces: 1, len: views[k].len() }
            }
            p => p,
        };
        match plan {
            Joined::Arg(k) => ffi::Py_NewRef(*args.add(k)),
            Joined::Build { npieces, len } => {
                let pieces = &pieces[..npieces];
                let maxchar = pieces.iter().map(|p| views[p.arg].maxchar).max().unwrap_or(0x7f).max(0x7f);
                let (o, data, kind) = new_str(len, maxchar);
                if o.is_null() {
                    return o;
                }
                let mut at = 0;
                for p in pieces {
                    if p.sep_before {
                        rt::write_units(data, kind, at, &[b'/']);
                        at += 1;
                    }
                    let v = views[p.arg];
                    write_text(data, kind, at, v.text);
                    at += v.len();
                }
                o
            }
        }
    }
}

/// # Safety
/// `n >= 1` exact `bytes` arguments.
unsafe fn join_bytes(args: *const O, n: usize) -> O {
    // SAFETY: per the caller.
    unsafe {
        let mut jargs_s = [JoinArg { len: 0, starts_sep: false, ends_sep: false }; MAXA];
        let mut pieces_s = [Piece::default(); MAXA];
        let (mut jargs_h, mut pieces_h);
        let (jargs, pieces): (&mut [JoinArg], &mut [Piece]) = if n <= MAXA {
            (&mut jargs_s[..n], &mut pieces_s[..n])
        } else {
            jargs_h = vec![JoinArg { len: 0, starts_sep: false, ends_sep: false }; n];
            pieces_h = vec![Piece::default(); n];
            (&mut jargs_h[..], &mut pieces_h[..])
        };
        for k in 0..n {
            jargs[k] = JoinArg::of(bytes_view(*args.add(k)));
        }
        match posix::join_plan(jargs, pieces) {
            Joined::Arg(k) => ffi::Py_NewRef(*args.add(k)),
            Joined::Build { npieces, len } => {
                let o = ffi::PyBytes_FromStringAndSize(std::ptr::null(), len as ffi::Py_ssize_t);
                if o.is_null() {
                    return o;
                }
                let d = ffi::PyBytes_AS_STRING(o) as *mut u8;
                let mut at = 0;
                for p in &pieces[..npieces] {
                    if p.sep_before {
                        *d.add(at) = b'/';
                        at += 1;
                    }
                    let s = bytes_view(*args.add(p.arg));
                    std::ptr::copy_nonoverlapping(s.as_ptr(), d.add(at), s.len());
                    at += s.len();
                }
                o
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// split, splitext, basename, dirname, isabs
// ---------------------------------------------------------------------------------------

/// One exact str/bytes positional argument, guards passed.
macro_rules! one_arg {
    ($rt:ident, $func:ident, $args:ident, $n:ident, $kw:ident, |$p:ident, $t:ident| $body:expr) => {
        if !$kw.is_null() || $n != 1 {
            None
        } else {
            let $p = *$args;
            match ty_of($p) {
                None => None,
                Some($t) => match guards(&$rt, $func) {
                    Some(g) => Some(g),
                    None => Some($body),
                },
            }
        }
    };
}

/// A 2-tuple taking ownership of `a` and `b` (null if either is null).
unsafe fn pair(a: O, b: O) -> O {
    // SAFETY: owned references or null.
    unsafe {
        if a.is_null() || b.is_null() {
            ffi::Py_XDECREF(a);
            ffi::Py_XDECREF(b);
            return std::ptr::null_mut();
        }
        let t = ffi::PyTuple_New(2);
        if t.is_null() {
            ffi::Py_DECREF(a);
            ffi::Py_DECREF(b);
            return t;
        }
        ffi::PyTuple_SET_ITEM(t, 0, a);
        ffi::PyTuple_SET_ITEM(t, 1, b);
        t
    }
}

/// `p[start:end]` for the argument's type.
#[inline(always)]
unsafe fn slice(t: Ty, p: O, start: usize, end: usize) -> O {
    // SAFETY: forwarded.
    unsafe {
        match t {
            Ty::Str => substr(p, start, end),
            Ty::Bytes => subbytes(p, start, end),
        }
    }
}

/// Run a generic algorithm on the argument's code units; returns (value, len).
macro_rules! units {
    ($t:expr, $p:expr, |$s:ident| $body:expr) => {
        match $t {
            Ty::Str => {
                let v = str_view($p);
                (crate::with_text!(v.text, |$s| $body), v.len())
            }
            Ty::Bytes => {
                let $s = bytes_view($p);
                ($body, $s.len())
            }
        }
    };
}

kernel!(k_split, RT_SPLIT, |func, args, nargsf, n, kw| one_arg!(RT_SPLIT, func, args, n, kw, |p, t| {
    let ((i, h), len) = units!(t, p, |s| (posix::tail_start(s), posix::head_len(s)));
    pair(slice(t, p, 0, h), slice(t, p, i, len))
}));

kernel!(k_splitext, RT_SPLITEXT, |func, args, nargsf, n, kw| one_arg!(RT_SPLITEXT, func, args, n, kw, |p, t| {
    let (dot, len) = units!(t, p, |s| posix::splitext(s));
    match dot {
        Some(d) => pair(slice(t, p, 0, d), slice(t, p, d, len)),
        None => pair(ffi::Py_NewRef(p), slice(t, p, 0, 0)),
    }
}));

kernel!(k_basename, RT_BASENAME, |func, args, nargsf, n, kw| one_arg!(RT_BASENAME, func, args, n, kw, |p, t| {
    let (i, len) = units!(t, p, |s| posix::tail_start(s));
    slice(t, p, i, len)
}));

kernel!(k_dirname, RT_DIRNAME, |func, args, nargsf, n, kw| one_arg!(RT_DIRNAME, func, args, n, kw, |p, t| {
    let (h, _len) = units!(t, p, |s| posix::head_len(s));
    slice(t, p, 0, h)
}));

kernel!(k_isabs, RT_ISABS, |func, args, nargsf, n, kw| one_arg!(RT_ISABS, func, args, n, kw, |p, t| {
    let (abs, _len) = units!(t, p, |s| posix::is_abs(s));
    ffi::Py_NewRef(if abs { ffi::Py_True() } else { ffi::Py_False() })
}));

// ---------------------------------------------------------------------------------------
// abspath, relpath (normpath ported from C, os.getcwd through the pinned function)
// ---------------------------------------------------------------------------------------

/// Pinned `os.getcwd` / `os.getcwdb` objects (strong references, main interpreter).
static GETCWD: AtomicUsize = AtomicUsize::new(0);
static GETCWDB: AtomicUsize = AtomicUsize::new(0);
/// Constants of `relpath`'s code: `'.'`, `'..'`, `b'.'`, `b'..'`.
static REL_CONSTS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
/// Bytes paths round-trip through the filesystem encoding in C `normpath`: only
/// UTF-8 + surrogateescape is byte-transparent for `/` and `.`.
static BYTES_OK: AtomicBool = AtomicBool::new(false);
/// Interned `"start"` (keyword of relpath).
static KW_START: AtomicUsize = AtomicUsize::new(0);

/// `posixpath.normpath` must be the C `posix._path_normpath` (pinned), behaving like the
/// port on probe inputs; bytes are accepted only under UTF-8 + surrogateescape.
fn check_normpath(cx: &mut Prepare<'_>, g: &Bound<'_, PyDict>, deps: &'static DepFlag) -> Result<(), Refusal> {
    let np = cx.get(g, "normpath")?;
    let posix = cx.loaded_module("posix")?;
    let c_np = cx.get(&posix.dict(), "_path_normpath")?;
    if !np.is(&c_np) {
        return Err(refuse!("posixpath.normpath is not posix._path_normpath"));
    }
    rt::check_c(cx, FPS, "posixpath.normpath", &np)?;
    cx.pin(g, "normpath", deps)?;
    cx.self_test("normpath port", |_py| {
        let probes = [
            "", "/", "//", "///", ".", "..", "./", "a/..", "a/../..", "/../a", "//x/../..", "a//b/./c/../d/",
            "\u{e9}/\u{4f60}/..", "./../a", "../../a/b/../..", "/a/b/c/../../../../d", "a/./.b/..c/...",
            "\u{1F600}//./\u{1F600}", "\0/x/../\0",
        ];
        for p in probes {
            let want: String = c_np.call1((p,))?.extract()?;
            let cps: Vec<u32> = p.chars().map(|c| c as u32).collect();
            let got = posix::normpath(&cps);
            let got: String =
                if got.is_empty() { ".".into() } else { got.iter().filter_map(|&c| char::from_u32(c)).collect() };
            if got != want {
                return Ok(false);
            }
        }
        Ok(true)
    })?;
    let sys = cx.loaded_module("sys")?;
    let enc: String = sys.call_method0("getfilesystemencoding")?.extract()?;
    let err: String = sys.call_method0("getfilesystemencodeerrors")?.extract()?;
    BYTES_OK.store(enc == "utf-8" && err == "surrogateescape", Relaxed);
    Ok(())
}

/// What `abspath` reads: `os.fspath`, `os.getcwd(b)`, `join`, `_get_sep`, `normpath`.
fn prep_abs_common<'py>(cx: &mut Prepare<'py>, g: &Bound<'py, PyDict>, deps: &'static DepFlag) -> Result<(), Refusal> {
    init_layouts(cx)?;
    pin_sep(cx, g, deps)?;
    let osd = cx.loaded_module("os")?.dict();
    for (key, cell) in [("getcwd", &GETCWD), ("getcwdb", &GETCWDB)] {
        let f = cx.get(&osd, key)?;
        rt::check_c(cx, FPS, &format!("os.{key}"), &f)?;
        cx.pin(&osd, key, deps)?;
        if cell.load(Relaxed) != f.as_ptr() as usize {
            cell.store(rt::leak(&f), Relaxed);
        }
    }
    func(cx, g, "join")?;
    cx.pin(g, "join", deps)?;
    check_normpath(cx, g, deps)
}

fn prep_abspath(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let f = func(cx, &g, "abspath")?;
    prep_abs_common(cx, &g, &RT_ABSPATH.deps)?;
    cx.vectorcall(&f, k_abspath, &accel::FUNCTION_VECTORCALL)
}

fn prep_relpath(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let f = func(cx, &g, "relpath")?;
    let deps = &RT_RELPATH.deps;
    prep_abs_common(cx, &g, deps)?;
    func(cx, &g, "abspath")?;
    cx.pin(&g, "abspath", deps)?;
    cx.pin_global(&g, "len", deps)?;
    // `_commonprefix([start_list, path_list])` (3.15: `genericpath._commonprefix`; 3.14:
    // the module-level `commonprefix` imported from genericpath).
    let gp = cx.loaded_module("genericpath")?;
    let gd = gp.dict();
    let cp = if cx.version() >= (3, 15) {
        if !cx.get(&g, "genericpath")?.is(&gp) {
            return Err(refuse!("posixpath.genericpath is not the genericpath module"));
        }
        cx.pin(&g, "genericpath", deps)?;
        let cp = cx.get(&gd, "_commonprefix")?;
        rt::check_fp(cx, FPS, "genericpath._commonprefix", &cp)?;
        cx.pin(&gd, "_commonprefix", deps)?;
        cp
    } else {
        let cp = cx.get(&g, "commonprefix")?;
        rt::check_fp(cx, FPS, "genericpath.commonprefix", &cp)?;
        cx.pin(&g, "commonprefix", deps)?;
        cp
    };
    let _ = cp;
    for name in ["isinstance", "list", "tuple", "min", "max", "enumerate"] {
        cx.pin_global(&gd, name, deps)?;
    }
    let values = [
        PyString::new(py, ".").into_any(),
        PyString::new(py, "..").into_any(),
        pyo3::types::PyBytes::new(py, b".").into_any(),
        pyo3::types::PyBytes::new(py, b"..").into_any(),
    ];
    for (k, v) in values.iter().enumerate() {
        let Some(c) = rt::code_const(&f, v)? else { return Err(refuse!("relpath constants differ")) };
        if REL_CONSTS[k].load(Relaxed) == 0 {
            REL_CONSTS[k].store(rt::leak(&c), Relaxed);
        } else if REL_CONSTS[k].load(Relaxed) != c.as_ptr() as usize {
            return Err(refuse!("relpath constants changed"));
        }
    }
    if KW_START.load(Relaxed) == 0 {
        KW_START.store(rt::leak(PyString::intern(py, "start").as_any()), Relaxed);
    }
    cx.vectorcall(&f, k_relpath, &accel::FUNCTION_VECTORCALL)
}

/// The current directory through the pinned `os.getcwd`/`os.getcwdb` (a C function: no
/// Python code, no audit event), as code units. `None` (error cleared) if it fails: the
/// caller declines and CPython's code raises.
unsafe fn cwd_units(t: Ty) -> Option<CwdBuf> {
    // SAFETY: attached thread; pinned builtins.
    unsafe {
        let f = if t == Ty::Str { GETCWD.load(Relaxed) } else { GETCWDB.load(Relaxed) } as O;
        let r = ffi::PyObject_CallNoArgs(f);
        if r.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let out = match t {
            Ty::Str if exact_str(r) => {
                Some(CwdBuf::Str(crate::with_text!(str_view(r).text, |s| s.iter().map(|c| c.cp()).collect())))
            }
            Ty::Bytes if exact_bytes(r) => Some(CwdBuf::Bytes(bytes_view(r).to_vec())),
            _ => None,
        };
        ffi::Py_DECREF(r);
        out
    }
}

enum CwdBuf {
    Str(Vec<u32>),
    Bytes(Vec<u8>),
}

/// `normpath(p if p.startswith(sep) else join(cwd(), p))` as code units.
fn abs_units<T: Unit>(p: &[T], cwd: &mut Option<Vec<T>>, getcwd: &mut dyn FnMut() -> Option<Vec<T>>) -> Option<Vec<T>> {
    if posix::is_abs(p) {
        return Some(posix::normpath(p));
    }
    if cwd.is_none() {
        *cwd = Some(getcwd()?);
    }
    let c = cwd.as_ref()?;
    // join(cwd, p): p is relative.
    let mut joined: Vec<T> = Vec::with_capacity(c.len() + 1 + p.len());
    if c.is_empty() {
        joined.extend_from_slice(p);
    } else {
        joined.extend_from_slice(c);
        if *c.last()? != T::SLASH {
            joined.push(T::SLASH);
        }
        joined.extend_from_slice(p);
    }
    Some(posix::normpath(&joined))
}

fn to_u32(t: Text<'_>) -> Vec<u32> {
    crate::with_text!(t, |s| s.iter().map(|c| c.cp()).collect())
}

kernel!(k_abspath, RT_ABSPATH, |func, args, nargsf, n, kw| one_arg!(RT_ABSPATH, func, args, n, kw, |p, t| {
    match t {
        Ty::Str => {
            let v = to_u32(str_view(p).text);
            let mut cwd = None;
            let mut get = || match cwd_units(Ty::Str) {
                Some(CwdBuf::Str(c)) => Some(c),
                _ => None,
            };
            match abs_units(&v, &mut cwd, &mut get) {
                None => std::ptr::null_mut(),
                Some(norm) if norm.is_empty() => ffi::PyUnicode_FromOrdinal(b'.' as i32),
                Some(norm) => str_from_units(&norm),
            }
        }
        Ty::Bytes => {
            if !BYTES_OK.load(Relaxed) {
                return RT_ABSPATH.fallback(Reason::Args, func, args, nargsf, kw);
            }
            let v = bytes_view(p);
            let mut cwd = None;
            let mut get = || match cwd_units(Ty::Bytes) {
                Some(CwdBuf::Bytes(c)) => Some(c),
                _ => None,
            };
            match abs_units(v, &mut cwd, &mut get) {
                None => std::ptr::null_mut(),
                Some(norm) if norm.is_empty() => ffi::PyBytes_FromStringAndSize(c".".as_ptr(), 1),
                Some(norm) => ffi::PyBytes_FromStringAndSize(norm.as_ptr().cast(), norm.len() as ffi::Py_ssize_t),
            }
        }
    }
}));

/// Components of `relpath(path, start)` relative to `start` (already absolute and
/// normalized), CPython's list `[pardir] * (len(start_list)-i) + path_list[i:]`.
fn rel_parts<'a, T: Unit>(path_abs: &'a [T], start_abs: &'a [T]) -> (usize, Vec<&'a [T]>) {
    let start_list: Vec<&[T]> = posix::abs_components(start_abs).collect();
    let path_list: Vec<&[T]> = posix::abs_components(path_abs).collect();
    let i = start_list.iter().zip(&path_list).take_while(|(a, b)| a == b).count();
    (start_list.len() - i, path_list[i..].to_vec())
}

kernel!(k_relpath, RT_RELPATH, |func, args, nargsf, n, kw| {
    // relpath(path), relpath(path, start), relpath(path, start=...)
    let start_kw = !kw.is_null()
        && n == 1
        && ffi::PyTuple_GET_SIZE(kw) == 1
        && ffi::PyTuple_GET_ITEM(kw, 0) as usize == KW_START.load(Relaxed);
    if !(kw.is_null() && (n == 1 || n == 2)) && !start_kw {
        None
    } else {
        let p = *args;
        let s = if n == 2 || start_kw { Some(*args.add(1)) } else { None };
        let s = s.filter(|&s| s != ffi::Py_None());
        match (ty_of(p), s.map(|s| ty_of(s))) {
            (Some(t), None) | (Some(t), Some(Some(_))) if s.is_none_or(|s| ty_of(s) == Some(t)) => {
                let empty = match t {
                    Ty::Str => str_view(p).is_empty(),
                    Ty::Bytes => bytes_view(p).is_empty(),
                };
                if empty || (t == Ty::Bytes && !BYTES_OK.load(Relaxed)) {
                    None
                } else {
                    match guards(&RT_RELPATH, func) {
                        Some(g) => Some(g),
                        None => Some(match t {
                            Ty::Str => relpath_str(p, s),
                            Ty::Bytes => relpath_bytes(p, s),
                        }),
                    }
                }
            }
            _ => None,
        }
    }
});

unsafe fn relpath_str(p: O, s: Option<O>) -> O {
    // SAFETY: exact str arguments; attached thread.
    unsafe {
        let pv = to_u32(str_view(p).text);
        let sv = match s {
            Some(s) => to_u32(str_view(s).text),
            None => vec![b'.' as u32],
        };
        let mut cwd = None;
        let mut get = || match cwd_units(Ty::Str) {
            Some(CwdBuf::Str(c)) => Some(c),
            _ => None,
        };
        let Some(sa) = abs_units(&sv, &mut cwd, &mut get) else { return std::ptr::null_mut() };
        let Some(pa) = abs_units(&pv, &mut cwd, &mut get) else { return std::ptr::null_mut() };
        let (ups, rest) = rel_parts(&pa, &sa);
        match (ups, rest.len()) {
            (0, 0) => ffi::Py_NewRef(REL_CONSTS[0].load(Relaxed) as O),
            (1, 0) => ffi::Py_NewRef(REL_CONSTS[1].load(Relaxed) as O),
            (0, 1) => str_from_units(rest[0]),
            _ => {
                let mut out: Vec<u32> = Vec::new();
                for k in 0..ups {
                    if k > 0 {
                        out.push(b'/' as u32);
                    }
                    out.extend_from_slice(&[b'.' as u32, b'.' as u32]);
                }
                for (k, c) in rest.iter().enumerate() {
                    if ups > 0 || k > 0 {
                        out.push(b'/' as u32);
                    }
                    out.extend_from_slice(c);
                }
                str_from_units(&out)
            }
        }
    }
}

unsafe fn relpath_bytes(p: O, s: Option<O>) -> O {
    // SAFETY: exact bytes arguments; attached thread.
    unsafe {
        let pv = bytes_view(p);
        let sv: &[u8] = match s {
            Some(s) => bytes_view(s),
            None => b".",
        };
        let mut cwd = None;
        let mut get = || match cwd_units(Ty::Bytes) {
            Some(CwdBuf::Bytes(c)) => Some(c),
            _ => None,
        };
        let Some(sa) = abs_units(sv, &mut cwd, &mut get) else { return std::ptr::null_mut() };
        let Some(pa) = abs_units(pv, &mut cwd, &mut get) else { return std::ptr::null_mut() };
        let (ups, rest) = rel_parts(&pa, &sa);
        match (ups, rest.len()) {
            (0, 0) => ffi::Py_NewRef(REL_CONSTS[2].load(Relaxed) as O),
            (1, 0) => ffi::Py_NewRef(REL_CONSTS[3].load(Relaxed) as O),
            _ => {
                let mut out: Vec<u8> = Vec::new();
                for k in 0..ups {
                    if k > 0 {
                        out.push(b'/');
                    }
                    out.extend_from_slice(b"..");
                }
                for (k, c) in rest.iter().enumerate() {
                    if ups > 0 || k > 0 {
                        out.push(b'/');
                    }
                    out.extend_from_slice(c);
                }
                ffi::PyBytes_FromStringAndSize(out.as_ptr().cast(), out.len() as ffi::Py_ssize_t)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Test helpers on `rustique._rustique._accel`
// ---------------------------------------------------------------------------------------

/// The C normpath port, for differential tests.
#[pyfunction]
fn _paths_normpath(py: Python<'_>, p: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    if let Ok(s) = p.cast::<PyString>() {
        rt::init_str_layout(py);
        // SAFETY: a live str.
        let v = to_u32(unsafe { str_view(s.as_ptr()) }.text);
        let r = posix::normpath(&v);
        if r.is_empty() {
            return Ok(PyString::new(py, ".").into_any().unbind());
        }
        // SAFETY: attached thread.
        return unsafe { Ok(Bound::from_owned_ptr_or_err(py, str_from_units(&r))?.unbind()) };
    }
    let b = p.cast::<pyo3::types::PyBytes>()?;
    let r = posix::normpath(b.as_bytes());
    Ok(pyo3::types::PyBytes::new(py, if r.is_empty() { b"." } else { &r }).into_any().unbind())
}

pub fn register(accel: &Bound<'_, PyModule>) -> PyResult<()> {
    accel.add_function(wrap_pyfunction!(_paths_normpath, accel)?)?;
    Ok(())
}
