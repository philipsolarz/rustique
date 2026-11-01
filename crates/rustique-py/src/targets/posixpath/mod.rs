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

use pyo3::exceptions::PyValueError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyCFunction, PyDict, PyList, PyString, PyTuple};

use rustique_core::paths::posix::{self, JoinArg, Joined, Piece};

use crate::accel::stats::Reason;
use rt::{
    O, Rt, Spec, StrView, Text, Unit, bytes_view, exact_bytes, exact_str, nargs, new_str, str_from_units, str_view,
    substr, subbytes, write_text,
};

// ---------------------------------------------------------------------------------------
// Fingerprints (rt::fingerprint of `code`), per minor version. Identical on GIL and
// free-threaded builds. Regenerate with `_t_posixpath.fingerprints()`.
// ---------------------------------------------------------------------------------------

const FP_JOIN: &[(u8, u64)] = &[(14, 0xcc0eb7d00d3613ea), (15, 0x2003f3d325eefa6c)];
const FP_SPLIT: &[(u8, u64)] = &[(14, 0x540a18c3408c818d), (15, 0x7ae1a36ec06c322a)];
const FP_SPLITEXT: &[(u8, u64)] = &[(14, 0x778ac20d1b1ed7d9), (15, 0xa590050a418594fb)];
const FP_BASENAME: &[(u8, u64)] = &[(14, 0x6dfcae9b901fee66), (15, 0xb00bc3d9e18ffbcd)];
const FP_DIRNAME: &[(u8, u64)] = &[(14, 0xef8bc136ffc92fb8), (15, 0x89e042619898d0b6)];
const FP_ISABS: &[(u8, u64)] = &[(14, 0x7e334ef5f49f7308), (15, 0x057c59e7f1201cdb)];
const FP_RELPATH: &[(u8, u64)] = &[(14, 0x2bd7160528c0c177), (15, 0x1eb57c45e04d7f89)];
const FP_ABSPATH: &[(u8, u64)] = &[(14, 0x38eae2ba221fff7b), (15, 0x6c8cbb7e6289b750)];

const PINS_SEP: &[(&str, &str)] = &[
    ("posixpath", "os"),
    ("os", "fspath"),
    ("posixpath", "_get_sep"),
    ("builtins", "isinstance"),
    ("builtins", "bytes"),
];
const PINS_SPLITEXT: &[(&str, &str)] = &[
    ("posixpath", "os"),
    ("os", "fspath"),
    ("builtins", "isinstance"),
    ("builtins", "bytes"),
    ("posixpath", "genericpath"),
    ("genericpath", "_splitext"),
];
const PINS_ABSPATH: &[(&str, &str)] = &[
    ("posixpath", "os"),
    ("os", "fspath"),
    ("os", "getcwd"),
    ("os", "getcwdb"),
    ("posixpath", "join"),
    ("posixpath", "_get_sep"),
    ("posixpath", "normpath"),
    ("builtins", "isinstance"),
    ("builtins", "bytes"),
];
const PINS_RELPATH: &[(&str, &str)] = &[
    ("posixpath", "os"),
    ("os", "fspath"),
    ("os", "getcwd"),
    ("os", "getcwdb"),
    ("posixpath", "abspath"),
    ("posixpath", "join"),
    ("posixpath", "_get_sep"),
    ("posixpath", "normpath"),
    ("posixpath", "genericpath"),
    ("posixpath", "commonprefix"),
    ("builtins", "isinstance"),
    ("builtins", "bytes"),
    ("builtins", "len"),
    ("builtins", "min"),
    ("builtins", "max"),
    ("builtins", "enumerate"),
];

static RT_JOIN: Rt = Rt::new();
static RT_SPLIT: Rt = Rt::new();
static RT_SPLITEXT: Rt = Rt::new();
static RT_BASENAME: Rt = Rt::new();
static RT_DIRNAME: Rt = Rt::new();
static RT_ISABS: Rt = Rt::new();
static RT_RELPATH: Rt = Rt::new();
static RT_ABSPATH: Rt = Rt::new();

pub static SPECS: &[Spec] = &[
    Spec {
        name: "posixpath.join",
        func: "posixpath.join",
        kernel: k_join,
        rt: &RT_JOIN,
        code: &["posixpath.join", "posixpath._get_sep"],
        expected: FP_JOIN,
        pins: PINS_SEP,
        jit_safe: false,
        prepare: None,
    },
    Spec {
        name: "posixpath.split",
        func: "posixpath.split",
        kernel: k_split,
        rt: &RT_SPLIT,
        code: &["posixpath.split", "posixpath._get_sep"],
        expected: FP_SPLIT,
        pins: PINS_SEP,
        jit_safe: false,
        prepare: None,
    },
    Spec {
        name: "posixpath.splitext",
        func: "posixpath.splitext",
        kernel: k_splitext,
        rt: &RT_SPLITEXT,
        code: &["posixpath.splitext", "genericpath._splitext"],
        expected: FP_SPLITEXT,
        pins: PINS_SPLITEXT,
        jit_safe: false,
        prepare: None,
    },
    Spec {
        name: "posixpath.basename",
        func: "posixpath.basename",
        kernel: k_basename,
        rt: &RT_BASENAME,
        code: &["posixpath.basename", "posixpath._get_sep"],
        expected: FP_BASENAME,
        pins: PINS_SEP,
        jit_safe: false,
        prepare: None,
    },
    Spec {
        name: "posixpath.dirname",
        func: "posixpath.dirname",
        kernel: k_dirname,
        rt: &RT_DIRNAME,
        code: &["posixpath.dirname", "posixpath._get_sep"],
        expected: FP_DIRNAME,
        pins: PINS_SEP,
        jit_safe: false,
        prepare: None,
    },
    Spec {
        name: "posixpath.isabs",
        func: "posixpath.isabs",
        kernel: k_isabs,
        rt: &RT_ISABS,
        code: &["posixpath.isabs", "posixpath._get_sep"],
        expected: FP_ISABS,
        pins: PINS_SEP,
        jit_safe: false,
        prepare: None,
    },
    Spec {
        name: "posixpath.relpath",
        func: "posixpath.relpath",
        kernel: k_relpath,
        rt: &RT_RELPATH,
        code: &[
            "posixpath.relpath",
            "posixpath.abspath",
            "posixpath.join",
            "posixpath._get_sep",
            "genericpath.commonprefix",
            "genericpath._commonprefix",
        ],
        expected: FP_RELPATH,
        pins: PINS_RELPATH,
        jit_safe: false,
        prepare: Some(prepare_relpath),
    },
    Spec {
        name: "posixpath.abspath",
        func: "posixpath.abspath",
        kernel: k_abspath,
        rt: &RT_ABSPATH,
        code: &["posixpath.abspath", "posixpath.join", "posixpath._get_sep"],
        expected: FP_ABSPATH,
        pins: PINS_ABSPATH,
        jit_safe: false,
        prepare: Some(prepare_abspath),
    },
];

pub static GROUP: rt::Group = rt::Group::new("posixpath", SPECS);

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
        match posix::join_plan(jargs, pieces) {
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

fn check_normpath(py: Python<'_>) -> PyResult<Result<(), String>> {
    let Some(np) = rt::resolve(py, "posixpath.normpath")? else { return Ok(Err("posixpath.normpath missing".into())) };
    let posix = py.import("sys")?.getattr("modules")?.get_item("posix")?;
    let c_np = posix.getattr("_path_normpath")?;
    if !np.is(&c_np) {
        return Ok(Err("posixpath.normpath is not posix._path_normpath".into()));
    }
    // Behavioural self-test of the port against the C function.
    let probes = [
        "", "/", "//", "///", ".", "..", "./", "a/..", "a/../..", "/../a", "//x/../..", "a//b/./c/../d/", "\u{e9}/\u{4f60}/..",
        "./../a", "../../a/b/../..", "/a/b/c/../../../../d", "a/./.b/..c/...", "\u{1F600}//./\u{1F600}",
    ];
    for p in probes {
        let want: String = c_np.call1((p,))?.extract()?;
        let cps: Vec<u32> = p.chars().map(|c| c as u32).collect();
        let got = posix::normpath(&cps);
        let got: String = if got.is_empty() { ".".into() } else { got.iter().map(|&c| char::from_u32(c).unwrap_or('?')).collect() };
        if got != want {
            return Ok(Err(format!("normpath self-test failed on {p:?}")));
        }
    }
    let sys = py.import("sys")?;
    let enc: String = sys.call_method0("getfilesystemencoding")?.extract()?;
    let err: String = sys.call_method0("getfilesystemencodeerrors")?.extract()?;
    BYTES_OK.store(enc == "utf-8" && err == "surrogateescape", Relaxed);
    Ok(Ok(()))
}

fn prepare_cwd(py: Python<'_>) -> PyResult<Result<(), String>> {
    let os = py.import("sys")?.getattr("modules")?.get_item("os")?;
    let d = os.getattr("__dict__")?.cast_into::<PyDict>()?;
    for (key, cell) in [("getcwd", &GETCWD), ("getcwdb", &GETCWDB)] {
        let Some(f) = d.get_item(key)? else { return Ok(Err(format!("os.{key} missing"))) };
        if !f.is_instance_of::<PyCFunction>() {
            return Ok(Err(format!("os.{key} is not the builtin")));
        }
        if cell.load(Relaxed) != f.as_ptr() as usize {
            cell.store(rt::leak(&f), Relaxed);
        }
    }
    check_normpath(py)
}

fn prepare_abspath(py: Python<'_>) -> PyResult<Result<(), String>> {
    prepare_cwd(py)
}

fn prepare_relpath(py: Python<'_>) -> PyResult<Result<(), String>> {
    if let Err(why) = prepare_cwd(py)? {
        return Ok(Err(why));
    }
    let Some(relpath) = rt::resolve(py, "posixpath.relpath")? else { return Ok(Err("relpath missing".into())) };
    let values = [
        PyString::new(py, ".").into_any(),
        PyString::new(py, "..").into_any(),
        pyo3::types::PyBytes::new(py, b".").into_any(),
        pyo3::types::PyBytes::new(py, b"..").into_any(),
    ];
    for (k, v) in values.iter().enumerate() {
        let Some(c) = rt::code_const(&relpath, v)? else { return Ok(Err("relpath constants differ".into())) };
        REL_CONSTS[k].store(rt::leak(&c), Relaxed);
    }
    KW_START.store(rt::leak(PyString::intern(py, "start").as_any()), Relaxed);
    // 3.15 reads genericpath._commonprefix, 3.14 the module-level commonprefix (pinned).
    if py.version_info().minor >= 15 {
        let g = rt::resolve(py, "genericpath")?.ok_or_else(|| PyValueError::new_err("genericpath"))?;
        let gd = g.getattr("__dict__")?.cast_into::<PyDict>()?;
        if gd.get_item("_commonprefix")?.is_none() {
            return Ok(Err("genericpath._commonprefix missing".into()));
        }
        crate::accel::guard::pin(&gd, "_commonprefix", &RT_RELPATH.deps)?;
    }
    Ok(Ok(()))
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
// Python module `rustique._rustique._t_posixpath`
// ---------------------------------------------------------------------------------------

/// The C normpath port, for differential tests.
#[pyfunction]
fn _normpath(py: Python<'_>, p: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    if let Ok(s) = p.cast::<PyString>() {
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

pub fn register_group<'py>(m: &Bound<'py, PyModule>, group: &'static rt::Group, modname: &str) -> PyResult<Bound<'py, PyModule>> {
    let py = m.py();
    let sub = PyModule::new(py, modname)?;
    sub.add_function(PyCFunction::new_closure(
        py,
        Some(c"install"),
        Some(c"install(names=None, force_jit=False): install the group's targets; returns [(name, state, reason)]"),
        move |a: &Bound<'_, PyTuple>, k: Option<&Bound<'_, PyDict>>| -> PyResult<Py<PyAny>> {
            let py = a.py();
            let mut names: Option<Vec<String>> = None;
            let mut force_jit = false;
            if let Some(x) = a.get_item(0).ok().filter(|x| !x.is_none()) {
                names = Some(x.extract()?);
            }
            if let Some(k) = k {
                if let Some(x) = k.get_item("names")?.filter(|x| !x.is_none()) {
                    names = Some(x.extract()?);
                }
                if let Some(x) = k.get_item("force_jit")? {
                    force_jit = x.extract()?;
                }
            }
            Ok(group.install(py, names, force_jit)?.into_pyobject(py)?.into_any().unbind())
        },
    )?)?;
    sub.add_function(PyCFunction::new_closure(
        py,
        Some(c"uninstall"),
        None,
        move |_a: &Bound<'_, PyTuple>, _k: Option<&Bound<'_, PyDict>>| -> PyResult<()> { group.uninstall() },
    )?)?;
    sub.add_function(PyCFunction::new_closure(
        py,
        Some(c"status"),
        None,
        move |a: &Bound<'_, PyTuple>, _k: Option<&Bound<'_, PyDict>>| -> PyResult<Py<PyList>> {
            Ok(group.status(a.py())?.unbind())
        },
    )?)?;
    sub.add_function(PyCFunction::new_closure(
        py,
        Some(c"counters"),
        None,
        move |a: &Bound<'_, PyTuple>, _k: Option<&Bound<'_, PyDict>>| -> PyResult<Py<PyDict>> {
            Ok(group.counters(a.py())?.unbind())
        },
    )?)?;
    sub.add_function(PyCFunction::new_closure(
        py,
        Some(c"reset_counters"),
        None,
        move |_a: &Bound<'_, PyTuple>, _k: Option<&Bound<'_, PyDict>>| -> PyResult<()> {
            group.reset_counters();
            Ok(())
        },
    )?)?;
    sub.add_function(PyCFunction::new_closure(
        py,
        Some(c"fingerprints"),
        None,
        move |a: &Bound<'_, PyTuple>, _k: Option<&Bound<'_, PyDict>>| -> PyResult<Py<PyDict>> {
            Ok(group.fingerprints(a.py())?.unbind())
        },
    )?)?;
    sub.add_function(PyCFunction::new_closure(
        py,
        Some(c"stats"),
        None,
        move |a: &Bound<'_, PyTuple>, _k: Option<&Bound<'_, PyDict>>| -> PyResult<()> {
            rt::set_stats(a.get_item(0)?.extract()?);
            Ok(())
        },
    )?)?;
    m.add(modname, &sub)?;
    Ok(sub)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = register_group(m, &GROUP, "_t_posixpath")?;
    sub.add_function(wrap_pyfunction!(_normpath, &sub)?)?;
    Ok(())
}
