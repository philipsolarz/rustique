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

use crate::accel::{self, DepFlag, Fingerprints, GroupSpec, Prepare, Refusal, TargetSpec};
use crate::refuse;
use rt::{
    O, Rt, StrView, Text, Unit, bytes_view, exact_bytes, exact_str, new_str, str_from_units, str_view, subbytes,
    substr, write_text,
};

// ---------------------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------------------

/// Fingerprints of every original the kernels reproduce or call, recorded per minor version
/// with `rustique._rustique._accel.fingerprint(obj)` (identical on GIL and free-threaded
/// builds).
pub(crate) const FPS: &[(&str, Fingerprints)] = &[
    ("posixpath.join", &[(3, 14, "f9426bd851f5e1b8"), (3, 15, "f8eceb34d61d89f7")]),
    ("posixpath.split", &[(3, 14, "9982d1a934157004"), (3, 15, "96f8c3db5bc9fe52")]),
    ("posixpath.splitext", &[(3, 14, "73ba50ede7290757"), (3, 15, "8496dfae04f8b3f5")]),
    ("posixpath.basename", &[(3, 14, "bacc0b3f6c53df7d"), (3, 15, "a7e4e60fb5d2bc37")]),
    ("posixpath.dirname", &[(3, 14, "ce9cf6c4f5088db5"), (3, 15, "f707cb4e4bce98e9")]),
    ("posixpath.isabs", &[(3, 14, "9aa922bd104243c9"), (3, 15, "ed693fb046ef035d")]),
    ("posixpath.relpath", &[(3, 14, "ec917de6f2a960cd"), (3, 15, "1aaf374efc900813")]),
    ("posixpath.abspath", &[(3, 14, "e29e635f89a6c4e9"), (3, 15, "5fcee84df633241d")]),
    ("posixpath._get_sep", &[(3, 14, "54be3703c91faa4c"), (3, 15, "5bc506a017985796")]),
    ("genericpath._splitext", &[(3, 14, "41f984dfa9547d91"), (3, 15, "4779d796fd294850")]),
    ("genericpath.commonprefix", &[(3, 14, "21c1c29dadbfcf1f"), (3, 15, "1e8a11037b7658c6")]),
    ("genericpath._commonprefix", &[(3, 15, "1b3a68ee2d093e2f")]),
    ("os.fspath", &[(3, 14, "23569f1819f5a2a6"), (3, 15, "23569f1819f5a2a6")]),
    ("os.getcwd", &[(3, 14, "ed3473ee5d2671c7"), (3, 15, "ed3473ee5d2671c7")]),
    ("os.getcwdb", &[(3, 14, "64d5082e435b0ab7"), (3, 15, "64d5082e435b0ab7")]),
    ("posixpath.normpath", &[(3, 14, "62a5270f293cf375"), (3, 15, "62a5270f293cf375")]),
];

/// The group's dependency flag: everything any posixpath original reads at call time,
/// pinned once by [`SHARED`].
static DEPS: DepFlag = DepFlag::new();

static RT_JOIN: Rt = Rt::new(&DEPS);
static RT_SPLIT: Rt = Rt::new(&DEPS);
static RT_SPLITEXT: Rt = Rt::new(&DEPS);
static RT_BASENAME: Rt = Rt::new(&DEPS);
static RT_DIRNAME: Rt = Rt::new(&DEPS);
static RT_ISABS: Rt = Rt::new(&DEPS);
static RT_RELPATH: Rt = Rt::new(&DEPS);
static RT_ABSPATH: Rt = Rt::new(&DEPS);

/// Declarations shared by the eight targets (prepared once, kept while any is installed).
pub static SHARED: GroupSpec = GroupSpec { name: "posixpath", prepare: shared };

macro_rules! target {
    ($stat:ident, $name:literal, $rt:ident, $prep:ident) => {
        // `..DEFAULT` keeps the spec additive when the registry grows fields.
        #[allow(clippy::needless_update)]
        pub static $stat: TargetSpec = TargetSpec {
            name: $name,
            group: "posixpath",
            module: Some("posixpath"),
            versions: &[(3, 14), (3, 15)],
            free_threaded: true,
            counters: &$rt.counters,
            prepare: $prep,
            // Steady-state JIT cells measured within 1.05 (fallbacks suspend the target).
            jit_safe: true,
            checkable: true,
            shared: Some(&SHARED),
            release: Some(release),
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

/// Layout calibrations every kernel of the group relies on (str buffers; plain str
/// subclasses, which fall back when unavailable).
fn init_layouts(cx: &Prepare<'_>) -> Result<(), Refusal> {
    if !rt::init_str_layout(cx.py()) {
        return Err(refuse!("str layout not calibrated"));
    }
    rt::init_plain(cx.py());
    Ok(())
}

/// The group's shared declarations: validations (fingerprints of the helpers the kernels
/// reproduce or call, identities) and the pins of everything the six string-only originals
/// (`join split splitext basename dirname isabs`) read at call time, once for the group.
/// These six are platform-independent code (`posixpath` is importable everywhere:
/// `zipfile._path`, `mimetypes`, `http.server` use it explicitly on Windows), so this part
/// does not depend on the OS; `abspath`/`relpath` add [`cwd_deps`].
fn shared(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    init_layouts(cx)?;
    let g = cx.module()?.dict();
    let osm = cx.loaded_module("os")?;
    let osd = osm.dict();
    if !cx.get(&g, "os")?.is(&osm) {
        return Err(refuse!("posixpath.os is not the os module"));
    }
    rt::check_c(cx, FPS, "os.fspath", &cx.get(&osd, "fspath")?)?;
    rt::check_fp(cx, FPS, "posixpath._get_sep", &cx.get(&g, "_get_sep")?)?;
    // splitext → genericpath._splitext
    let gp = cx.loaded_module("genericpath")?;
    let gd = gp.dict();
    if !cx.get(&g, "genericpath")?.is(&gp) {
        return Err(refuse!("posixpath.genericpath is not the genericpath module"));
    }
    rt::check_fp(cx, FPS, "genericpath._splitext", &cx.get(&gd, "_splitext")?)?;
    // Pins: posixpath globals, os.fspath, the builtins posixpath and genericpath code
    // resolves (`pin_global` also pins "not shadowed by a module global").
    for key in ["os", "_get_sep", "genericpath"] {
        cx.pin(&g, key, &DEPS)?;
    }
    cx.pin(&osd, "fspath", &DEPS)?;
    cx.pin(&gd, "_splitext", &DEPS)?;
    for name in ["isinstance", "bytes", "len"] {
        cx.pin_global(&g, name, &DEPS)?;
    }
    for name in ["isinstance", "list", "tuple", "min", "max", "enumerate"] {
        cx.pin_global(&gd, name, &DEPS)?;
    }
    Ok(())
}

/// What `abspath` and `relpath` add (POSIX only: they read the current directory and
/// normalize with C `posix._path_normpath`, which Windows builds of `posixpath` replace with
/// a Python `normpath` over `nt.getcwd()` results): `os.getcwd(b)` (C), `join`, the C
/// normpath, `abspath` itself, `(_)commonprefix`, `relpath`'s constants, all pinned on the
/// group's flag. Idempotent per process (the cells keep their first values).
fn cwd_deps(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if !cfg!(unix) {
        return Err(Refusal::with(
            crate::accel::spec::Category::Platform,
            "abspath/relpath: POSIX getcwd and C normpath only",
        ));
    }
    let g = cx.module()?.dict();
    let osd = cx.loaded_module("os")?.dict();
    for (key, cell) in [("getcwd", &GETCWD), ("getcwdb", &GETCWDB)] {
        let f = cx.get(&osd, key)?;
        rt::check_c(cx, FPS, &format!("os.{key}"), &f)?;
        if cell.load(Relaxed) != f.as_ptr() as usize {
            cell.store(rt::leak(&f), Relaxed);
        }
    }
    func(cx, &g, "join")?;
    func(cx, &g, "abspath")?;
    check_normpath(cx, &g)?;
    // relpath → `_commonprefix` (3.15: `genericpath._commonprefix`; 3.14: the module-level
    // `commonprefix`).
    let gd = cx.loaded_module("genericpath")?.dict();
    if cx.version() >= (3, 15) {
        rt::check_fp(cx, FPS, "genericpath._commonprefix", &cx.get(&gd, "_commonprefix")?)?;
        cx.pin(&gd, "_commonprefix", &DEPS)?;
    } else {
        rt::check_fp(cx, FPS, "genericpath.commonprefix", &cx.get(&g, "commonprefix")?)?;
        cx.pin(&g, "commonprefix", &DEPS)?;
    }
    // relpath's constants (returned by identity) and the interned `start` keyword.
    let relpath = cx.get(&g, "relpath")?;
    let values = [
        PyString::new(py, ".").into_any(),
        PyString::new(py, "..").into_any(),
        pyo3::types::PyBytes::new(py, b".").into_any(),
        pyo3::types::PyBytes::new(py, b"..").into_any(),
    ];
    for (k, v) in values.iter().enumerate() {
        let Some(c) = rt::code_const(&relpath, v)? else { return Err(refuse!("relpath constants differ")) };
        if REL_CONSTS[k].load(Relaxed) == 0 {
            REL_CONSTS[k].store(rt::leak(&c), Relaxed);
        } else if REL_CONSTS[k].load(Relaxed) != c.as_ptr() as usize {
            return Err(refuse!("relpath constants changed"));
        }
    }
    if KW_START.load(Relaxed) == 0 {
        KW_START.store(rt::leak(PyString::intern(py, "start").as_any()), Relaxed);
    }
    for key in ["join", "normpath", "abspath"] {
        cx.pin(&g, key, &DEPS)?;
    }
    for key in ["getcwd", "getcwdb"] {
        cx.pin(&osd, key, &DEPS)?;
    }
    Ok(())
}

/// A target: its own function's fingerprint and its vectorcall site.
fn prep_fn(cx: &mut Prepare<'_>, attr: &str, kernel: ffi::vectorcallfunc) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let f = func(cx, &g, attr)?;
    cx.vectorcall(&f, kernel, &accel::FUNCTION_VECTORCALL)
}

fn prep_join(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_fn(cx, "join", k_join)
}
fn prep_split(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_fn(cx, "split", k_split)
}
fn prep_basename(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_fn(cx, "basename", k_basename)
}
fn prep_dirname(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_fn(cx, "dirname", k_dirname)
}
fn prep_isabs(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_fn(cx, "isabs", k_isabs)
}
fn prep_splitext(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_fn(cx, "splitext", k_splitext)
}
fn prep_abspath(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    cwd_deps(py, cx)?;
    prep_fn(cx, "abspath", k_abspath)
}
fn prep_relpath(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    cwd_deps(py, cx)?;
    prep_fn(cx, "relpath", k_relpath)
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

// ---------------------------------------------------------------------------------------
// join
// ---------------------------------------------------------------------------------------

const MAXA: usize = 8;

crate::paths_kernel!(k_join, RT_JOIN, JOIN, |func, args, nargsf, n, kw| {
    if !kw.is_null() || n == 0 {
        None
    } else if n == 2 && exact_str(*args) && exact_str(*args.add(1)) {
        // The common `join(dir, name)`: no plan arrays.
        match rt::guards(&RT_JOIN, func) {
            Some(g) => Some(g),
            None => Some(join2_str(*args, *args.add(1))),
        }
    } else {
        match ty_of(*args) {
            Some(t) if (1..n).all(|k| ty_of(*args.add(k)) == Some(t)) => match rt::guards(&RT_JOIN, func) {
                Some(g) => Some(g),
                None => Some(if t == Ty::Str { join_str(args, n) } else { join_bytes(args, n) }),
            },
            _ => None,
        }
    }
});

/// `join(a, b)` for two exact `str`: CPython's loop for one step.
///
/// # Safety
/// Exact `str` arguments; attached thread.
#[inline(always)]
unsafe fn join2_str(a: O, b: O) -> O {
    // SAFETY: per the caller.
    unsafe {
        let (va, vb) = (str_view(a), str_view(b));
        let b_abs = crate::with_text!(vb.text, |s| posix::is_abs(s));
        // `if b.startswith(sep) or not path: path = b`
        if b_abs || va.is_empty() {
            return ffi::Py_NewRef(b);
        }
        let a_ends = crate::with_text!(va.text, |s| s.last() == Some(&Unit::SLASH));
        // `elif path.endswith(sep): path += b` (an empty `b` leaves `path` itself)
        if a_ends && vb.is_empty() {
            return ffi::Py_NewRef(a);
        }
        let sep = usize::from(!a_ends);
        let len = va.len() + sep + vb.len();
        let (o, data, kind) = new_str(len, va.maxchar.max(vb.maxchar));
        if o.is_null() {
            return o;
        }
        write_text(data, kind, 0, va.text);
        if sep == 1 {
            rt::write_units(data, kind, va.len(), b"/");
        }
        write_text(data, kind, va.len() + sep, vb.text);
        o
    }
}

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
                        rt::write_units(data, kind, at, b"/");
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
        for (k, j) in jargs.iter_mut().enumerate() {
            *j = JoinArg::of(bytes_view(*args.add(k)));
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
                Some($t) => match rt::guards(&$rt, $func) {
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

crate::paths_kernel!(k_split, RT_SPLIT, SPLIT, |func, args, nargsf, n, kw| one_arg!(
    RT_SPLIT,
    func,
    args,
    n,
    kw,
    |p, t| {
        let ((i, h), len) = units!(t, p, |s| (posix::tail_start(s), posix::head_len(s)));
        pair(slice(t, p, 0, h), slice(t, p, i, len))
    }
));

crate::paths_kernel!(k_splitext, RT_SPLITEXT, SPLITEXT, |func, args, nargsf, n, kw| one_arg!(
    RT_SPLITEXT,
    func,
    args,
    n,
    kw,
    |p, t| {
        let (dot, len) = units!(t, p, |s| posix::splitext(s));
        match dot {
            Some(d) => pair(slice(t, p, 0, d), slice(t, p, d, len)),
            None => pair(ffi::Py_NewRef(p), slice(t, p, 0, 0)),
        }
    }
));

crate::paths_kernel!(k_basename, RT_BASENAME, BASENAME, |func, args, nargsf, n, kw| one_arg!(
    RT_BASENAME,
    func,
    args,
    n,
    kw,
    |p, t| {
        let (i, len) = units!(t, p, |s| posix::tail_start(s));
        slice(t, p, i, len)
    }
));

crate::paths_kernel!(k_dirname, RT_DIRNAME, DIRNAME, |func, args, nargsf, n, kw| one_arg!(
    RT_DIRNAME,
    func,
    args,
    n,
    kw,
    |p, t| {
        let (h, _len) = units!(t, p, |s| posix::head_len(s));
        slice(t, p, 0, h)
    }
));

crate::paths_kernel!(k_isabs, RT_ISABS, ISABS, |func, args, nargsf, n, kw| one_arg!(
    RT_ISABS,
    func,
    args,
    n,
    kw,
    |p, t| {
        let (abs, _len) = units!(t, p, |s| posix::is_abs(s));
        ffi::Py_NewRef(if abs { ffi::Py_True() } else { ffi::Py_False() })
    }
));

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

/// `posixpath.normpath` must be the C `posix._path_normpath` (pinned, fingerprinted);
/// bytes are accepted only under UTF-8 + surrogateescape.
fn check_normpath(cx: &mut Prepare<'_>, g: &Bound<'_, PyDict>) -> Result<(), Refusal> {
    let np = cx.get(g, "normpath")?;
    let posix = cx.loaded_module("posix")?;
    let c_np = cx.get(&posix.dict(), "_path_normpath")?;
    if !np.is(&c_np) {
        return Err(refuse!("posixpath.normpath is not posix._path_normpath"));
    }
    rt::check_c(cx, FPS, "posixpath.normpath", &np)?;
    // Behaviour of the port is proved by the conformance and differential tests (and
    // `_paths_normpath` fuzzing); install only checks identity and the C fingerprint.
    let sys = cx.loaded_module("sys")?;
    let enc: String = sys.call_method0("getfilesystemencoding")?.extract()?;
    let err: String = sys.call_method0("getfilesystemencodeerrors")?.extract()?;
    BYTES_OK.store(enc == "utf-8" && err == "surrogateescape", Relaxed);
    Ok(())
}

/// Exit release: the verified objects and the cached `os.getcwd`/`os.getcwdb` (builtins
/// whose `__self__` is the `posix` module). The `relpath` constants and the `start` keyword
/// (str/bytes: they keep nothing alive) stay.
fn release(py: Python<'_>) {
    rt::release(py);
    rt::release_cell(&GETCWD);
    rt::release_cell(&GETCWDB);
}

/// The current directory through the pinned `os.getcwd`/`os.getcwdb` (a C function: no
/// Python code, no audit event): an owned exact `str`/`bytes`. `None` (error cleared) if it
/// fails: the caller declines and CPython's code raises.
unsafe fn cwd_obj(t: Ty) -> Option<Owned> {
    // SAFETY: attached thread; pinned builtins.
    unsafe {
        let f = if t == Ty::Str { GETCWD.load(Relaxed) } else { GETCWDB.load(Relaxed) } as O;
        if f.is_null() {
            return None; // released at exit (a call that passed the flag check before it)
        }
        let r = ffi::PyObject_CallNoArgs(f);
        if r.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let r = Owned(r);
        let ok = match t {
            Ty::Str => exact_str(r.0),
            Ty::Bytes => exact_bytes(r.0),
        };
        ok.then_some(r)
    }
}

/// One owned reference, released on drop.
struct Owned(O);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: an owned reference.
        unsafe { ffi::Py_DECREF(self.0) };
    }
}

/// The code units of a path argument or of the current directory, whatever their width.
#[derive(Clone, Copy)]
enum Src<'a> {
    T(Text<'a>),
    B(&'a [u8]),
}

impl Src<'_> {
    #[inline(always)]
    fn len(self) -> usize {
        match self {
            Src::T(t) => crate::with_text!(t, |s| s.len()),
            Src::B(b) => b.len(),
        }
    }
    #[inline(always)]
    fn is_abs(self) -> bool {
        match self {
            Src::T(t) => crate::with_text!(t, |s| posix::is_abs(s)),
            Src::B(b) => posix::is_abs(b),
        }
    }
    #[inline(always)]
    fn ends_sep(self) -> bool {
        match self {
            Src::T(t) => crate::with_text!(t, |s| s.last() == Some(&Unit::SLASH)),
            Src::B(b) => b.last() == Some(&b'/'),
        }
    }
    /// One byte per code unit (bytes, or a 1-byte-kind str).
    #[inline(always)]
    fn narrow(self) -> bool {
        matches!(self, Src::B(_) | Src::T(Text::U1(_)))
    }
    /// Copy into `dst` (room for `self.len()` units of `B`).
    ///
    /// # Safety
    /// `dst` valid for `self.len()` writes; every code point fits `B` (the caller picks
    /// `u8` only when every source is narrow).
    #[inline(always)]
    unsafe fn copy_to<B: Wide>(self, dst: *mut B) {
        // SAFETY: per the caller.
        unsafe {
            match self {
                Src::T(t) => crate::with_text!(t, |s| B::copy_from(dst, s)),
                Src::B(b) => B::copy_from(dst, b),
            }
        }
    }
}

/// Element type of the scratch buffers `abspath`/`relpath` normalize in: `u8` when every
/// source is one byte per unit, else `u32` (the code points C `normpath` sees).
trait Wide: Unit {
    /// # Safety
    /// `dst` valid for `src.len()` writes; every code point of `src` fits `Self`.
    unsafe fn copy_from<S: Unit>(dst: *mut Self, src: &[S]);
}

impl Wide for u8 {
    #[inline(always)]
    unsafe fn copy_from<S: Unit>(dst: *mut u8, src: &[S]) {
        // SAFETY: per the caller.
        unsafe {
            if size_of::<S>() == 1 {
                std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u8>(), dst, src.len());
            } else {
                for (k, &c) in src.iter().enumerate() {
                    *dst.add(k) = c.cp() as u8;
                }
            }
        }
    }
}

impl Wide for u32 {
    #[inline(always)]
    unsafe fn copy_from<S: Unit>(dst: *mut u32, src: &[S]) {
        // SAFETY: per the caller.
        unsafe {
            if size_of::<S>() == 4 {
                std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u32>(), dst, src.len());
            } else {
                for (k, &c) in src.iter().enumerate() {
                    *dst.add(k) = c.cp();
                }
            }
        }
    }
}

/// Scratch capacity (code units) on the stack; longer paths use one heap buffer.
const SCRATCH: usize = 512;

/// `normpath(p if p.startswith(sep) else join(cwd, p))` written into a scratch buffer of
/// `B` (no allocation for paths up to [`SCRATCH`] units), then `f(normalized)`. `cwd` is
/// required when `p` is relative.
///
/// # Safety
/// The views stay valid during the call; `B` is `u8` only if every source is narrow.
#[inline(always)]
unsafe fn with_abs<B: Wide, R>(p: Src<'_>, cwd: Option<Src<'_>>, f: impl FnOnce(&[B]) -> R) -> Option<R> {
    // join(cwd, p) for a relative p: `cwd + p` if cwd is empty or ends with '/', else
    // `cwd + '/' + p` (p never resets: it is relative).
    let (c, sep) = if p.is_abs() {
        (None, false)
    } else {
        let c = cwd?;
        (Some(c), c.len() != 0 && !c.ends_sep())
    };
    let need = c.map_or(0, |c| c.len()) + usize::from(sep) + p.len();
    let mut stack = std::mem::MaybeUninit::<[B; SCRATCH]>::uninit();
    let mut heap: Vec<B> = Vec::new();
    let base: *mut B = if need <= SCRATCH {
        stack.as_mut_ptr().cast::<B>()
    } else {
        heap.reserve_exact(need);
        heap.as_mut_ptr()
    };
    // SAFETY: `base` has room for `need` units; every unit is written before the slice is
    // formed.
    unsafe {
        let mut at = 0;
        if let Some(c) = c {
            c.copy_to(base);
            at = c.len();
            if sep {
                *base.add(at) = B::SLASH;
                at += 1;
            }
        }
        p.copy_to(base.add(at));
        let buf = std::slice::from_raw_parts_mut(base, need);
        let n = posix::normpath_in_place(buf);
        Some(f(&buf[..n]))
    }
}

/// The components of an absolute normalized path (`abspath(x).lstrip(sep).split(sep)`,
/// `[]` for the root).
#[inline(always)]
fn comps<B: Unit>(norm: &[B]) -> impl Iterator<Item = &[B]> + Clone {
    posix::abs_components(norm)
}

/// `abspath`'s str result: C `normpath` builds it with `PyUnicode_FromWideChar` (the
/// narrowest kind; `'.'` for an empty result).
///
/// # Safety
/// Attached thread.
unsafe fn abs_str<B: Unit>(norm: &[B]) -> O {
    // SAFETY: attached thread.
    unsafe { if norm.is_empty() { ffi::PyUnicode_FromOrdinal(b'.' as i32) } else { str_from_units(norm) } }
}

crate::paths_kernel!(k_abspath, RT_ABSPATH, ABSPATH, |func, args, nargsf, n, kw| one_arg!(
    RT_ABSPATH,
    func,
    args,
    n,
    kw,
    |p, t| {
        match t {
            Ty::Str => {
                let pv = Src::T(str_view(p).text);
                let cwd = if pv.is_abs() {
                    None
                } else {
                    match cwd_obj(Ty::Str) {
                        Some(c) => Some(c),
                        None => return RT_ABSPATH.fallback(Reason::Deopt, func, args, nargsf, kw),
                    }
                };
                let cv = cwd.as_ref().map(|c| Src::T(str_view(c.0).text));
                let r = if pv.narrow() && cv.is_none_or(|c| c.narrow()) {
                    with_abs::<u8, _>(pv, cv, |norm| abs_str(norm))
                } else {
                    with_abs::<u32, _>(pv, cv, |norm| abs_str(norm))
                };
                r.unwrap_or(std::ptr::null_mut())
            }
            Ty::Bytes => {
                if !BYTES_OK.load(Relaxed) {
                    return RT_ABSPATH.fallback(Reason::Args, func, args, nargsf, kw);
                }
                let pv = Src::B(bytes_view(p));
                let cwd = if pv.is_abs() {
                    None
                } else {
                    match cwd_obj(Ty::Bytes) {
                        Some(c) => Some(c),
                        None => return RT_ABSPATH.fallback(Reason::Deopt, func, args, nargsf, kw),
                    }
                };
                let cv = cwd.as_ref().map(|c| Src::B(bytes_view(c.0)));
                with_abs::<u8, _>(pv, cv, |norm| {
                    if norm.is_empty() {
                        ffi::PyBytes_FromStringAndSize(c".".as_ptr(), 1)
                    } else {
                        ffi::PyBytes_FromStringAndSize(norm.as_ptr().cast(), norm.len() as ffi::Py_ssize_t)
                    }
                })
                .unwrap_or(std::ptr::null_mut())
            }
        }
    }
));

crate::paths_kernel!(k_relpath, RT_RELPATH, RELPATH, |func, args, nargsf, n, kw| {
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
                    match rt::guards(&RT_RELPATH, func) {
                        Some(g) => Some(g),
                        None => Some(relpath(t, p, s)),
                    }
                }
            }
            _ => None,
        }
    }
});

/// `relpath(path, start)` for exact str (or plain str subclass) / exact bytes arguments.
unsafe fn relpath(t: Ty, p: O, s: Option<O>) -> O {
    // SAFETY: arguments of type `t`; attached thread.
    unsafe {
        let view = |o: O| match t {
            Ty::Str => Src::T(str_view(o).text),
            Ty::Bytes => Src::B(bytes_view(o)),
        };
        let pv = view(p);
        // `start = curdir` when None.
        let sv = match s {
            Some(s) => view(s),
            None => match t {
                Ty::Str => Src::T(Text::U1(b".")),
                Ty::Bytes => Src::B(b"."),
            },
        };
        let cwd = if pv.is_abs() && sv.is_abs() {
            None
        } else {
            match cwd_obj(t) {
                Some(c) => Some(c),
                None => return std::ptr::null_mut(),
            }
        };
        let cv = cwd.as_ref().map(|c| view(c.0));
        let narrow = pv.narrow() && sv.narrow() && cv.is_none_or(|c| c.narrow());
        let r = if narrow { rel_build::<u8>(t, pv, sv, cv) } else { rel_build::<u32>(t, pv, sv, cv) };
        r.unwrap_or(std::ptr::null_mut())
    }
}

/// abspath both, compare components, build `sep.join([pardir] * ups + rest)`.
///
/// # Safety
/// Attached thread; live views.
unsafe fn rel_build<B: Wide>(t: Ty, pv: Src<'_>, sv: Src<'_>, cv: Option<Src<'_>>) -> Option<O> {
    // SAFETY: forwarded.
    unsafe {
        with_abs::<B, _>(sv, cv, |sa| {
            with_abs::<B, _>(pv, cv, |pa| {
                let mut sc = comps(sa);
                let mut pc = comps(pa);
                // common prefix of the component lists
                let mut ups = 0usize;
                let mut rest = pc.clone();
                loop {
                    match (sc.next(), pc.next()) {
                        (Some(a), Some(b)) if a == b => {
                            rest = pc.clone();
                        }
                        (Some(_), _) => {
                            ups = 1 + sc.count();
                            break;
                        }
                        (None, _) => break,
                    }
                }
                rel_object(t, ups, rest)
            })
        })?
    }
}

/// The result object of `relpath` from `ups` parent steps and the remaining components.
///
/// # Safety
/// Attached thread.
unsafe fn rel_object<'a, B: Wide>(t: Ty, ups: usize, rest: impl Iterator<Item = &'a [B]> + Clone) -> O {
    // SAFETY: attached thread; the object is filled completely before it is returned.
    unsafe {
        let mut nrest = 0usize;
        let mut len = if ups > 0 { 3 * ups - 1 } else { 0 };
        let mut maxcp = 0x2eu32;
        let mut first: &[B] = &[];
        for c in rest.clone() {
            if nrest == 0 {
                first = c;
            }
            len += c.len() + usize::from(ups > 0 || nrest > 0);
            if size_of::<B>() > 1 || t == Ty::Str {
                maxcp = c.iter().fold(maxcp, |m, u| m.max(u.cp()));
            }
            nrest += 1;
        }
        let k = if t == Ty::Str { 0 } else { 2 };
        match (ups, nrest) {
            (0, 0) => return ffi::Py_NewRef(REL_CONSTS[k].load(Relaxed) as O),
            (1, 0) => return ffi::Py_NewRef(REL_CONSTS[k + 1].load(Relaxed) as O),
            (0, 1) => {
                return match t {
                    Ty::Str => str_from_units(first),
                    Ty::Bytes => ffi::PyBytes_FromStringAndSize(first.as_ptr().cast(), first.len() as ffi::Py_ssize_t),
                };
            }
            _ => {}
        }
        let (o, data, kind) = match t {
            Ty::Str => new_str(len, maxcp.max(0x7f)),
            Ty::Bytes => {
                let o = ffi::PyBytes_FromStringAndSize(std::ptr::null(), len as ffi::Py_ssize_t);
                let d: *mut std::ffi::c_void =
                    if o.is_null() { std::ptr::null_mut() } else { ffi::PyBytes_AS_STRING(o) as *mut std::ffi::c_void };
                (o, d, 1)
            }
        };
        if o.is_null() {
            return o;
        }
        let mut at = 0usize;
        for k in 0..ups {
            if k > 0 {
                rt::write_units(data, kind, at, b"/");
                at += 1;
            }
            rt::write_units(data, kind, at, b"..");
            at += 2;
        }
        for (k, c) in rest.enumerate() {
            if ups > 0 || k > 0 {
                rt::write_units(data, kind, at, b"/");
                at += 1;
            }
            rt::write_units(data, kind, at, c);
            at += c.len();
        }
        debug_assert_eq!(at, len);
        o
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
        let v: Vec<u32> =
            crate::with_text!(unsafe { str_view(s.as_ptr()) }.text, |s| s.iter().map(|c| c.cp()).collect());
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
