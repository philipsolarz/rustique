//! Group `pathlib`: the pure-path core of `pathlib.PurePosixPath` / `PosixPath`.
//!
//! PurePath keeps its parsed state in private `__slots__` that its properties fill lazily
//! (`_raw_paths` from `__init__`; `_drv`, `_root`, `_tail_cached` from `_parse_path`;
//! `_str`, `_str_normcase_cached`, `_hash`). The kernels compute the same values natively
//! and store them in the same slots, so every non-accelerated method afterwards sees
//! exactly the state CPython would have built: same values, same object identities where
//! CPython shares objects (the tail items, `'/'`, the empty string, `_str` being the single
//! tail item of a one-component relative path, ...).
//!
//! Exact subset: the receiver (and, where relevant, the other operand) is exactly
//! `PurePosixPath` or `PosixPath`, its slots hold the values pathlib itself stores (exact
//! str/list), the raw paths are exact str; every attribute the original reaches is pinned
//! on both classes (`with_segments`, `__new__`, `__init__`, `parser`, the properties and
//! helpers) and fingerprinted. Subclasses, Windows flavours and anything unusual fall back.
//! See docs/targets/pathlib.md.

use std::sync::atomic::{AtomicIsize, AtomicPtr, AtomicUsize, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyType};

use rustique_core::paths::Unit;
use rustique_core::paths::posix::{self, JoinArg, Joined, Piece};

use crate::accel::{self, Fingerprints, Prepare, Refusal, TargetSpec};
use crate::refuse;
use crate::targets::posixpath::rt::{self, O, Rt, Text, exact_str, str_from_units, str_view};

// ---------------------------------------------------------------------------------------
// Fingerprints
// ---------------------------------------------------------------------------------------

pub(crate) const FPS: &[(&str, Fingerprints)] = &[
    ("PurePath.__new__", &[(3, 14, "8553742f3ce83247"), (3, 15, "08f481e9affb2b2e")]),
    ("PurePath.__init__", &[(3, 14, "9d0b36111268406f"), (3, 15, "3b91b5a8e2ffed94")]),
    ("PurePath.with_segments", &[(3, 14, "c8ea79facb1dde6c"), (3, 15, "cc1e289e7600ccd7")]),
    ("PurePath.__truediv__", &[(3, 14, "036ed49ccc9e1148"), (3, 15, "9cee1589673903dd")]),
    ("PurePath.__str__", &[(3, 14, "b7899590c36d2bda"), (3, 15, "60709840309e061c")]),
    ("PurePath._format_parsed_parts", &[(3, 14, "a0e16aabb3aa3bc8"), (3, 15, "bb4d4e5f78e49103")]),
    ("PurePath._from_parsed_parts", &[(3, 14, "3d047b67ea4910ed"), (3, 15, "30f38d9574d60d69")]),
    ("PurePath._from_parsed_string", &[(3, 14, "fd2087e34606168e"), (3, 15, "8f57c7679f1c3c52")]),
    ("PurePath._parse_path", &[(3, 14, "5b0c39ecad5f87be"), (3, 15, "365539562c66f6d7")]),
    ("PurePath._raw_path", &[(3, 14, "6853c2d3d7b53c22"), (3, 15, "21f2a7c505a3d340")]),
    ("PurePath.drive", &[(3, 14, "930245251769319a"), (3, 15, "a987dbbe71134c60")]),
    ("PurePath.root", &[(3, 14, "801a4df809140797"), (3, 15, "a96521e6c677c6dd")]),
    ("PurePath._tail", &[(3, 14, "21832fe6fee534f3"), (3, 15, "60bf96b7f6edb2a1")]),
    ("PurePath.parent", &[(3, 14, "e498a54608cb2f0f"), (3, 15, "a32ba739c4d2f032")]),
    ("PurePath.name", &[(3, 14, "35f8471d8e03bf0a"), (3, 15, "1b0f788df77fdf50")]),
    ("PurePath.suffix", &[(3, 14, "1f7760436204400c"), (3, 15, "68eda74b6c6aefae")]),
    ("PurePath.stem", &[(3, 14, "95f29eb9c9ddd9be"), (3, 15, "1bdbd4c395dec7fd")]),
    ("PurePath.parts", &[(3, 14, "f94d3784ccbb7c8d"), (3, 15, "5f6662003a116fef")]),
    ("PurePath.parents", &[(3, 14, "646a5a2c96d7aed9"), (3, 15, "3dc1abaff79a929f")]),
    ("PurePath.relative_to", &[(3, 14, "8f86ddf50a6c7e02"), (3, 15, "8452e3b0f55cd8f9")]),
    ("PurePath._str_normcase", &[(3, 14, "8da19417d97dd3bf"), (3, 15, "6e99083d05fd70a1")]),
    ("PurePath.__eq__", &[(3, 14, "abfb2a4ffcb8b062"), (3, 15, "f032fefd45724f3f")]),
    ("PurePath.__hash__", &[(3, 14, "dfb08cd5dea44bfc"), (3, 15, "6681c1e2b421d196")]),
    ("_PathParents.__init__", &[(3, 14, "104c9de7d3b519b2"), (3, 15, "43abba808573b4e1")]),
    ("_PathParents.__len__", &[(3, 14, "52094efa9c0f38b9"), (3, 15, "0ec2c915a87de6da")]),
    ("_PathParents.__getitem__", &[(3, 14, "3da0fc404ac1dc99"), (3, 15, "f5e7b1181c96746e")]),
    ("Path.__new__", &[(3, 14, "7b964357d3265c2c"), (3, 15, "5216e085641fb6bd")]),
    ("posixpath.splitdrive", &[(3, 14, "7ed944945a53a75d"), (3, 15, "66cb007f7a87d887")]),
    ("posixpath.splitroot", &[(3, 14, "2d41f32dde9746e1"), (3, 15, "6c3f34b1d74117df")]),
];

/// What parsing reaches through `self` / `cls`.
const PARSE: &[&str] = &["parser", "_parse_path", "_raw_path", "drive", "root", "_tail"];
/// What creating a derived path reaches (`_from_parsed_parts` → `with_segments` →
/// `type(self)(...)`).
const DERIVE: &[&str] =
    &["_format_parsed_parts", "_from_parsed_parts", "_from_parsed_string", "with_segments", "__new__", "__init__"];

/// The `PurePath` attributes each original reaches through `self` / `cls` besides itself
/// (pinned through `PosixPath`'s MRO, which contains `PurePosixPath`'s: patching one on a
/// class suspends the target; replacing the target's own attribute just stops routing calls
/// to the kernel).
fn reached(attr: &str) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    match attr {
        "__new__" => {}
        "__init__" => v.push("parser"),
        "__truediv__" => v.extend(["with_segments", "__new__", "__init__", "parser"]),
        "__str__" | "name" | "parts" => {
            v.extend(PARSE);
            v.push("_format_parsed_parts");
        }
        "suffix" | "stem" => {
            v.extend(PARSE);
            v.push("name");
        }
        "parent" => {
            v.extend(PARSE);
            v.extend(DERIVE);
        }
        "__eq__" | "__hash__" => {
            v.extend(PARSE);
            v.extend(["_format_parsed_parts", "_str_normcase", "__str__"]);
        }
        _ => {
            // relative_to
            v.extend(PARSE);
            v.extend(DERIVE);
            v.extend(["parents", "__eq__", "_str_normcase", "__str__"]);
        }
    }
    v
}

// ---------------------------------------------------------------------------------------
// Validated layout: the two exact classes and the slot offsets
// ---------------------------------------------------------------------------------------

const RAW: usize = 0;
const DRV: usize = 1;
const ROOT: usize = 2;
const TAIL: usize = 3;
const STR: usize = 4;
const NORMCASE: usize = 5;
const HASH: usize = 6;
const SLOT_NAMES: [&str; 7] = ["_raw_paths", "_drv", "_root", "_tail_cached", "_str", "_str_normcase_cached", "_hash"];

static PURE_POSIX: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(std::ptr::null_mut());
/// `PurePath` and `Path` themselves (their `__new__` maps them to the POSIX classes).
static PURE_BASE: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(std::ptr::null_mut());
static PATH_BASE: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(std::ptr::null_mut());
static POSIX: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(std::ptr::null_mut());
static OFF: [AtomicIsize; 7] = [const { AtomicIsize::new(0) }; 7];
/// `'.'` (the `or '.'` constant of `__str__` / `_from_parsed_string`).
static DOT: AtomicUsize = AtomicUsize::new(0);

/// Exactly one of the two validated classes.
#[inline(always)]
unsafe fn exact_path(o: O) -> bool {
    // SAFETY: live object.
    let t = unsafe { ffi::Py_TYPE(o) };
    t == PURE_POSIX.load(Relaxed) || t == POSIX.load(Relaxed)
}

#[inline(always)]
unsafe fn cell(o: O, k: usize) -> &'static AtomicPtr<ffi::PyObject> {
    // SAFETY: `o` is an instance of a validated class: the offset is one of its slots.
    unsafe { &*(o.cast::<u8>().offset(OFF[k].load(Relaxed)) as *const AtomicPtr<ffi::PyObject>) }
}

/// The slot's value (borrowed; null when unset), read under the object's critical section.
#[inline(always)]
unsafe fn get(o: O, k: usize) -> O {
    // SAFETY: per `cell`.
    unsafe { cell(o, k).load(Acquire) }
}

/// Store an owned value into a slot that is unset (checked by the caller under the
/// object's critical section).
#[inline(always)]
unsafe fn put(o: O, k: usize, v: O) {
    // SAFETY: per `cell`; the previous value is null.
    unsafe { cell(o, k).store(v, Release) }
}

/// Run `f` holding the critical section of `o` (free-threaded builds; no-op with the GIL).
#[inline(always)]
unsafe fn locked<R>(o: O, f: impl FnOnce() -> R) -> R {
    // SAFETY: attached thread; live object.
    unsafe {
        let py = Python::assume_attached();
        let b = Bound::from_borrowed_ptr(py, o);
        pyo3::sync::critical_section::with_critical_section(&b, f)
    }
}

#[inline(always)]
unsafe fn locked2<R>(a: O, b: O, f: impl FnOnce() -> R) -> R {
    // SAFETY: attached thread; live objects.
    unsafe {
        let py = Python::assume_attached();
        let ba = Bound::from_borrowed_ptr(py, a);
        let bb = Bound::from_borrowed_ptr(py, b);
        pyo3::sync::critical_section::with_critical_section2(&ba, &bb, f)
    }
}

// ---------------------------------------------------------------------------------------
// Parsed state
// ---------------------------------------------------------------------------------------

/// Borrowed views of a path object's parsed slots (valid while the object lives and the
/// slots are not replaced: they only go from unset to set).
#[derive(Clone, Copy)]
struct Parsed {
    root: O,
    tail: O,
}

/// The parsed slots, filling them from `_raw_paths` when unset, as `drive`/`root`/`_tail`
/// do. `None` (nothing changed) when the object's state is not one pathlib itself builds.
///
/// # Safety
/// Attached thread; `o` an exact validated path object; critical section of `o` held.
unsafe fn parsed(o: O) -> Option<Parsed> {
    // SAFETY: per the caller.
    unsafe {
        let (d, r, t) = (get(o, DRV), get(o, ROOT), get(o, TAIL));
        if !d.is_null() && !r.is_null() && !t.is_null() {
            // `_drv` is always '' for POSIX paths built by pathlib.
            if !exact_str(d) || ffi::PyUnicode_GET_LENGTH(d) != 0 || !exact_str(r) || ffi::PyList_CheckExact(t) == 0 {
                return None;
            }
            return Some(Parsed { root: r, tail: t });
        }
        if !d.is_null() || !r.is_null() || !t.is_null() {
            return None;
        }
        let (root, tail) = parse_raw(get(o, RAW))?;
        put(o, DRV, str_from_units::<u8>(&[]));
        put(o, ROOT, root);
        put(o, TAIL, tail);
        Some(Parsed { root, tail })
    }
}

/// `_parse_path(_raw_path)`: (root, tail list) as new references; `None` when the raw paths
/// are not an exact list of exact str.
unsafe fn parse_raw(raw: O) -> Option<(O, O)> {
    // SAFETY: attached thread; live objects.
    unsafe {
        if raw.is_null() || ffi::PyList_CheckExact(raw) == 0 {
            return None;
        }
        let n = ffi::PyList_GET_SIZE(raw) as usize;
        let mut items: Vec<O> = Vec::with_capacity(n);
        for i in 0..n {
            let it = ffi::PyList_GET_ITEM(raw, i as ffi::Py_ssize_t);
            if !exact_str(it) {
                return None;
            }
            items.push(it);
        }
        let mut joined: Vec<u32> = Vec::new();
        let text: Text<'_> = match n {
            0 => Text::U1(&[]),
            1 => str_view(items[0]).text,
            _ => {
                // `self.parser.join(*paths)`: only its value matters here.
                let views: Vec<_> = items.iter().map(|&p| str_view(p)).collect();
                let jargs: Vec<JoinArg> = views.iter().map(|v| crate::with_text!(v.text, |s| JoinArg::of(s))).collect();
                let mut pieces = vec![Piece::default(); n];
                match posix::join_plan(&jargs, &mut pieces) {
                    Joined::Arg(k) => views[k].text,
                    Joined::Build { npieces, .. } => {
                        for p in &pieces[..npieces] {
                            if p.sep_before {
                                joined.push(b'/' as u32);
                            }
                            crate::with_text!(views[p.arg].text, |s| joined.extend(s.iter().map(|c| c.cp())));
                        }
                        Text::U4(&joined)
                    }
                }
            }
        };
        crate::with_text!(text, |s| parse_units(s))
    }
}

/// `splitroot` (C `_Py_skiproot`) then `[x for x in rel.split(sep) if x and x != '.']`.
unsafe fn parse_units<T: Unit>(s: &[T]) -> Option<(O, O)> {
    // SAFETY: attached thread.
    unsafe {
        let is_sep = |i: usize| i < s.len() && s[i] == T::SLASH;
        let rootsize = if s.is_empty() || !is_sep(0) {
            0
        } else if !is_sep(1) || is_sep(2) {
            1
        } else {
            2
        };
        let root = str_from_units(&s[..rootsize]);
        if root.is_null() {
            return None;
        }
        let tail = ffi::PyList_New(0);
        if tail.is_null() {
            ffi::Py_DECREF(root);
            return None;
        }
        for part in s[rootsize..].split(|&c| c == T::SLASH) {
            if part.is_empty() || (part.len() == 1 && part[0] == T::DOT) {
                continue;
            }
            let o = str_from_units(part);
            if o.is_null() || ffi::PyList_Append(tail, o) != 0 {
                ffi::Py_XDECREF(o);
                ffi::Py_DECREF(root);
                ffi::Py_DECREF(tail);
                return None;
            }
            ffi::Py_DECREF(o);
        }
        Some((root, tail))
    }
}

/// `_format_parsed_parts('', root, tail)` (a new reference): `root + sep.join(tail)` when
/// there is a root, `sep.join(tail)` otherwise, with CPython's identity results (the root
/// itself for an empty tail, the single item of a one-item relative tail, '' when empty).
unsafe fn format(root: O, tail: O) -> O {
    // SAFETY: attached thread; exact str root, exact list of exact str tail.
    unsafe {
        let n = ffi::PyList_GET_SIZE(tail) as usize;
        let has_root = ffi::PyUnicode_GET_LENGTH(root) != 0;
        if n == 0 {
            return if has_root { ffi::Py_NewRef(root) } else { str_from_units::<u8>(&[]) };
        }
        if n == 1 && !has_root {
            return ffi::Py_NewRef(ffi::PyList_GET_ITEM(tail, 0));
        }
        let mut views = Vec::with_capacity(n + 1);
        let rv = str_view(root);
        let mut len = rv.len();
        let mut maxchar = rv.maxchar;
        for i in 0..n {
            let it = ffi::PyList_GET_ITEM(tail, i as ffi::Py_ssize_t);
            if !exact_str(it) {
                return std::ptr::null_mut();
            }
            let v = str_view(it);
            len += v.len() + usize::from(i > 0);
            maxchar = maxchar.max(v.maxchar);
            views.push(v);
        }
        let (o, data, kind) = rt::new_str(len, maxchar.max(0x7f));
        if o.is_null() {
            return o;
        }
        let mut at = 0;
        rt::write_text(data, kind, at, rv.text);
        at += rv.len();
        for (i, v) in views.iter().enumerate() {
            if i > 0 {
                rt::write_units(data, kind, at, b"/");
                at += 1;
            }
            rt::write_text(data, kind, at, v.text);
            at += v.len();
        }
        o
    }
}

/// `str(self)`: the `_str` slot, filled (with the parsed slots) when unset.
///
/// # Safety
/// As [`parsed`].
unsafe fn ensure_str(o: O) -> Option<O> {
    // SAFETY: per the caller.
    unsafe {
        let s = get(o, STR);
        if !s.is_null() {
            return exact_str(s).then_some(s);
        }
        let p = parsed(o)?;
        let f = format(p.root, p.tail);
        if f.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let s = if ffi::PyUnicode_GET_LENGTH(f) == 0 {
            ffi::Py_DECREF(f);
            ffi::Py_NewRef(DOT.load(Relaxed) as O)
        } else {
            f
        };
        put(o, STR, s);
        Some(s)
    }
}

/// `self._str_normcase` (POSIX: `str(self)` itself), filled when unset.
unsafe fn ensure_normcase(o: O) -> Option<O> {
    // SAFETY: as `parsed`.
    unsafe {
        let c = get(o, NORMCASE);
        if !c.is_null() {
            return exact_str(c).then_some(c);
        }
        let s = ensure_str(o)?;
        put(o, NORMCASE, ffi::Py_NewRef(s));
        Some(s)
    }
}

/// A new instance of `ty` as `type(self)(path_str)` builds it: `object.__new__(ty)`, then
/// `_raw_paths = raw` (an owned list).
unsafe fn new_path(ty: *mut ffi::PyTypeObject, raw: O) -> O {
    // SAFETY: attached thread; `ty` a validated class (its `__new__`/`__init__` pinned).
    unsafe {
        let object = &raw const ffi::PyBaseObject_Type;
        let new = (*object).tp_new.expect("object.tp_new");
        let empty = ffi::PyTuple_New(0);
        let o = new(ty, empty, std::ptr::null_mut());
        ffi::Py_DECREF(empty);
        if o.is_null() {
            ffi::Py_DECREF(raw);
            return o;
        }
        put(o, RAW, raw);
        o
    }
}

/// `self._from_parsed_parts('', root, tail)` (`tail` owned): a new object with `_raw_paths
/// = [path_str]`, `_str = path_str or '.'`, `_drv = ''`, `_root = root`, `_tail_cached =
/// tail`.
unsafe fn from_parsed_parts(ty: *mut ffi::PyTypeObject, root: O, tail: O) -> O {
    // SAFETY: attached thread.
    unsafe {
        let path_str = format(root, tail);
        if path_str.is_null() {
            ffi::Py_DECREF(tail);
            return path_str;
        }
        let raw = ffi::PyList_New(1);
        if raw.is_null() {
            ffi::Py_DECREF(path_str);
            ffi::Py_DECREF(tail);
            return raw;
        }
        ffi::PyList_SET_ITEM(raw, 0, ffi::Py_NewRef(path_str));
        let o = new_path(ty, raw);
        if o.is_null() {
            ffi::Py_DECREF(path_str);
            ffi::Py_DECREF(tail);
            return o;
        }
        let s = if ffi::PyUnicode_GET_LENGTH(path_str) == 0 {
            ffi::Py_DECREF(path_str);
            ffi::Py_NewRef(DOT.load(Relaxed) as O)
        } else {
            path_str
        };
        put(o, STR, s);
        put(o, DRV, str_from_units::<u8>(&[]));
        put(o, ROOT, ffi::Py_NewRef(root));
        put(o, TAIL, tail);
        o
    }
}

// ---------------------------------------------------------------------------------------
// Targets and kernels
// ---------------------------------------------------------------------------------------

static RT_STR: Rt = Rt::new();
static RT_TRUEDIV: Rt = Rt::new();
static RT_PARENT: Rt = Rt::new();
static RT_NAME: Rt = Rt::new();
static RT_SUFFIX: Rt = Rt::new();
static RT_STEM: Rt = Rt::new();
static RT_PARTS: Rt = Rt::new();
static RT_RELATIVE_TO: Rt = Rt::new();
static RT_EQ: Rt = Rt::new();
static RT_HASH: Rt = Rt::new();
static RT_NEW_PURE: Rt = Rt::new();
static RT_NEW_PATH: Rt = Rt::new();
static RT_INIT: Rt = Rt::new();

macro_rules! target {
    ($stat:ident, $prep:ident, $name:literal, $rt:ident, $attr:literal, $kernel:ident) => {
        target!($stat, $prep, $name, $rt, "PurePath", $attr, $kernel, &[(3, 14), (3, 15)]);
    };
    ($stat:ident, $prep:ident, $name:literal, $rt:ident, $owner:literal, $attr:literal, $kernel:ident) => {
        target!($stat, $prep, $name, $rt, $owner, $attr, $kernel, &[(3, 14), (3, 15)]);
    };
    ($stat:ident, $prep:ident, $name:literal, $rt:ident, $owner:literal, $attr:literal, $kernel:ident, $versions:expr) => {
        fn $prep(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
            prep(cx, $owner, $attr, &$rt, $kernel)
        }
        // `..DEFAULT` keeps the spec additive when the registry grows fields.
        #[allow(clippy::needless_update)]
        pub static $stat: TargetSpec = TargetSpec {
            name: $name,
            group: "pathlib",
            module: Some("pathlib"),
            versions: $versions,
            free_threaded: true,
            counters: &$rt.counters,
            prepare: $prep,
            // Steady-state JIT cells measured within 1.05 (fallbacks suspend the target).
            jit_safe: true,
            checkable: false,
            ..TargetSpec::DEFAULT
        };
    };
}

target!(STR_T, prep_str, "pathlib.PurePath.__str__", RT_STR, "__str__", k_str);
target!(TRUEDIV_T, prep_truediv, "pathlib.PurePath.__truediv__", RT_TRUEDIV, "__truediv__", k_truediv);
target!(PARENT_T, prep_parent, "pathlib.PurePath.parent", RT_PARENT, "PurePath", "parent", k_parent, PROPERTY_VERSIONS);
target!(NAME_T, prep_name, "pathlib.PurePath.name", RT_NAME, "PurePath", "name", k_name, PROPERTY_VERSIONS);
target!(SUFFIX_T, prep_suffix, "pathlib.PurePath.suffix", RT_SUFFIX, "PurePath", "suffix", k_suffix, PROPERTY_VERSIONS);
target!(STEM_T, prep_stem, "pathlib.PurePath.stem", RT_STEM, "PurePath", "stem", k_stem, PROPERTY_VERSIONS);
target!(PARTS_T, prep_parts, "pathlib.PurePath.parts", RT_PARTS, "PurePath", "parts", k_parts, PROPERTY_VERSIONS);
target!(RELATIVE_TO_T, prep_relative_to, "pathlib.PurePath.relative_to", RT_RELATIVE_TO, "relative_to", k_relative_to);
target!(EQ_T, prep_eq, "pathlib.PurePath.__eq__", RT_EQ, "__eq__", k_eq);
target!(HASH_T, prep_hash, "pathlib.PurePath.__hash__", RT_HASH, "__hash__", k_hash);

target!(NEW_PURE_T, prep_new_pure, "pathlib.PurePath.__new__", RT_NEW_PURE, "PurePath", "__new__", k_new_pure);
target!(NEW_PATH_T, prep_new_path, "pathlib.Path.__new__", RT_NEW_PATH, "Path", "__new__", k_new_path);
target!(INIT_T, prep_init, "pathlib.PurePath.__init__", RT_INIT, "__init__", k_init);

/// Properties: 3.15 only. On 3.14 `LOAD_ATTR_PROPERTY` caches the fget object and pushes
/// its frame inline without checking `func_version` (3.15 checks it), so a property access
/// site specializes to the original and never reaches the kernel: no gain to have.
const PROPERTY_VERSIONS: &[(u8, u8)] = &[(3, 15)];

pub static TARGETS: &[&TargetSpec] = &[
    &NEW_PURE_T,
    &NEW_PATH_T,
    &INIT_T,
    &STR_T,
    &TRUEDIV_T,
    &PARENT_T,
    &NAME_T,
    &SUFFIX_T,
    &STEM_T,
    &PARTS_T,
    &RELATIVE_TO_T,
    &EQ_T,
    &HASH_T,
];

/// The function behind a class attribute: functions as is, properties' `fget`,
/// classmethods' `__func__`.
fn underlying<'py>(v: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    if v.hasattr("fget")? {
        return v.getattr("fget");
    }
    if v.hasattr("__func__")? {
        return v.getattr("__func__");
    }
    Ok(v.clone())
}

fn type_dict<'py>(ty: &Bound<'py, PyType>) -> Result<Bound<'py, PyDict>, Refusal> {
    // SAFETY: a ready heap type: tp_dict is its real dict.
    let d = unsafe { (*ty.as_type_ptr()).tp_dict };
    if d.is_null() {
        return Err(refuse!("type without dict"));
    }
    // SAFETY: borrowed dict kept alive by the type.
    Ok(unsafe { Bound::from_borrowed_ptr(ty.py(), d) }.cast_into::<PyDict>().map_err(PyErr::from)?)
}

/// Validate the module (fingerprints, classes, slots, pins) once per target.
fn prep(
    cx: &mut Prepare<'_>,
    owner: &str,
    attr: &str,
    rt: &'static Rt,
    kernel: ffi::vectorcallfunc,
) -> Result<(), Refusal> {
    let py = cx.py();
    if !rt::init_str_layout(py) {
        return Err(refuse!("str layout not calibrated"));
    }
    let g = cx.module()?.dict();
    let mut types = Vec::new();
    for name in ["PurePath", "PurePosixPath", "PosixPath", "Path", "_PathParents"] {
        types.push(cx.get(&g, name)?.cast_into::<PyType>().map_err(PyErr::from)?);
    }
    let [pure, pure_posix, posix, path, parents] = <[Bound<'_, PyType>; 5]>::try_from(types).expect("five");
    let pd = type_dict(&pure)?;
    // Fingerprints of every function the kernels reproduce. The ten targets share them:
    // checked once per set of function objects (kept alive, so identity is stable).
    let mut funcs = Vec::with_capacity(FPS.len());
    for (name, _) in FPS {
        let (owner, a) = name.split_once('.').expect("dotted");
        let f = match owner {
            "PurePath" => underlying(&cx.get(&pd, a)?)?,
            "_PathParents" => underlying(&cx.get(&type_dict(&parents)?, a)?)?,
            "Path" => underlying(&cx.get(&type_dict(&path)?, a)?)?,
            _ => cx.get(&cx.loaded_module(owner)?.dict(), a)?,
        };
        funcs.push(f);
    }
    let ptrs: Vec<usize> = funcs.iter().map(|f| f.as_ptr() as usize).collect();
    let known = VALIDATED.lock().map(|v| *v == ptrs).unwrap_or(false);
    if !known {
        for ((name, _), f) in FPS.iter().zip(&funcs) {
            if *name == "posixpath.splitroot" {
                rt::check_c(cx, FPS, name, f)?;
            } else {
                rt::check_fp(cx, FPS, name, f)?;
            }
        }
    }
    // Both exact classes: default metaclass, posixpath parser; the attributes this original
    // reaches pinned along PosixPath's MRO (PosixPath, Path, PurePosixPath, PurePath, object:
    // it contains PurePosixPath's).
    let pp = cx.loaded_module("posixpath")?;
    for t in [&pure_posix, &posix] {
        if !t.get_type().is(py.get_type::<PyType>()) {
            return Err(refuse!("custom metaclass"));
        }
    }
    if !cx.type_attr(&pure_posix, "parser")?.is(&pp) {
        return Err(refuse!("PurePosixPath.parser is not posixpath"));
    }
    // `parser` through PosixPath's MRO (also registers the type watcher that breaks the pin on
    // an MRO change); the other attributes: few → one MRO pin each; many → one snapshot pin
    // per class dict of the MRO (any added, replaced or deleted attribute breaks it).
    let v = cx.pin_attr(&posix, "parser", &rt.deps)?;
    if !v.is(&pp) {
        return Err(refuse!("PosixPath.parser is not posixpath"));
    }
    let names = reached(attr);
    if names.len() <= 4 {
        for name in names.iter().filter(|n| **n != "parser") {
            cx.pin_attr(&posix, name, &rt.deps)?;
        }
    } else {
        for t in [&posix, &path, &pure_posix, &pure] {
            cx.pin_dict(&type_dict(t)?, &rt.deps)?;
        }
    }
    if attr == "relative_to" {
        // `path in self.parents`: _PathParents.__getitem__/__len__ (its own dict) and the
        // Sequence mixin's __contains__ / __iter__.
        cx.pin_dict(&type_dict(&parents)?, &rt.deps)?;
        for name in ["__contains__", "__iter__"] {
            cx.pin_attr(&parents, name, &rt.deps)?;
        }
    }
    let ppd = pp.dict();
    if !cx.get(&ppd, "sep")?.eq("/")? || !cx.get(&ppd, "altsep")?.is_none() {
        return Err(refuse!("posixpath.sep/altsep differ"));
    }
    let parses = reached(attr).contains(&"_parse_path");
    if parses {
        // `_raw_path` → `parser.join`; `_parse_path` → `sep`, `altsep`, `splitroot`;
        // `_format_parsed_parts` → `splitdrive`.
        for key in ["sep", "altsep", "join", "splitroot", "splitdrive", "_get_sep"] {
            cx.pin(&ppd, key, &rt.deps)?;
        }
        let join = cx.get(&ppd, "join")?;
        rt::check_fp(cx, crate::targets::posixpath::FPS, "posixpath.join", &join)?;
        rt::check_fp(cx, crate::targets::posixpath::FPS, "posixpath._get_sep", &cx.get(&ppd, "_get_sep")?)?;
        cx.pin_global(&g, "len", &rt.deps)?;
        // `posixpath.join` and `posixpath.splitdrive` call `os.fspath` (through
        // `posixpath.os`) and `_get_sep` resolves `isinstance` / `bytes`.
        if !cx.get(&ppd, "os")?.is(&cx.loaded_module("os")?) {
            return Err(refuse!("posixpath.os is not the os module"));
        }
        cx.pin(&ppd, "os", &rt.deps)?;
        for name in ["isinstance", "bytes"] {
            cx.pin_global(&ppd, name, &rt.deps)?;
        }
    }
    // The module globals and builtins this original's code (and the helpers it reaches)
    // resolves at call time.
    let (globals, builtins): (&[&str], &[&str]) = match attr {
        "__new__" => {
            (&["os", "PurePath", "PurePosixPath", "PosixPath", "Path", "PureWindowsPath", "WindowsPath"], &["object"])
        }
        "__init__" => (&["os", "PurePath"], &["isinstance", "str", "TypeError"]),
        "__truediv__" => (&["os", "PurePath", "Path"], &["type", "object", "isinstance", "str", "TypeError"]),
        "parts" => (&[], &["tuple"]),
        "parent" => (&["os", "PurePath", "Path"], &["type", "object", "isinstance", "str", "TypeError"]),
        "__eq__" => (&["PurePath", "posixpath"], &["isinstance", "str"]),
        "__hash__" => (&["posixpath"], &["hash", "str"]),
        "relative_to" => (
            &["os", "PurePath", "Path", "posixpath", "chain", "_PathParents"],
            &["type", "object", "isinstance", "str", "hasattr", "ValueError", "TypeError"],
        ),
        _ => (&[], &[]),
    };
    for key in globals {
        cx.pin(&g, key, &rt.deps)?;
    }
    for name in builtins {
        cx.pin_global(&g, name, &rt.deps)?;
    }
    // Slot offsets: object-typed, writable member descriptors of PurePath.
    for (k, name) in SLOT_NAMES.iter().enumerate() {
        let d = cx.get(&pd, name)?;
        // SAFETY: type check, then the member definition of a member descriptor.
        let off = unsafe {
            if ffi::Py_TYPE(d.as_ptr()) != &raw mut ffi::PyMemberDescr_Type {
                return Err(refuse!("{name} is not a slot"));
            }
            let m = (*d.as_ptr().cast::<ffi::PyMemberDescrObject>()).d_member;
            if (*m).type_code != ffi::Py_T_OBJECT_EX || (*m).flags & ffi::Py_READONLY != 0 {
                return Err(refuse!("{name} is not a writable object slot"));
            }
            (*m).offset
        };
        let prev = OFF[k].swap(off, Relaxed);
        if prev != 0 && prev != off {
            return Err(refuse!("slot layout changed"));
        }
    }
    // Same layout for both classes (no extra slots before PurePath's).
    for t in [&pure_posix, &posix] {
        // SAFETY: ready types.
        let ok = unsafe { ffi::PyType_IsSubtype(t.as_type_ptr(), pure.as_type_ptr()) != 0 };
        if !ok {
            return Err(refuse!("class hierarchy differs"));
        }
    }
    store_type(&PURE_POSIX, &pure_posix);
    store_type(&POSIX, &posix);
    if DOT.load(Relaxed) == 0 {
        DOT.store(rt::leak(PyString::new(py, ".").as_any()), Relaxed);
    }
    if !known && let Ok(mut v) = VALIDATED.lock() {
        for f in &funcs {
            rt::leak(f);
        }
        *v = ptrs;
    }
    let osd = cx.loaded_module("os")?.dict();
    if !cx.get(&osd, "name")?.eq("posix")? {
        return Err(refuse!("os.name is not 'posix'"));
    }
    if globals.contains(&"os") {
        // `__new__` picks the flavour from `os.name` (when called on `PurePath`/`Path`).
        cx.pin(&osd, "name", &rt.deps)?;
    }
    if globals.contains(&"os") || parses {
        // `__init__` calls `os.fspath(arg)` on every non-path argument; parsing reaches it
        // through `posixpath.join` / `posixpath.splitdrive`.
        let fspath = cx.get(&osd, "fspath")?;
        rt::check_c(cx, crate::targets::posixpath::FPS, "os.fspath", &fspath)?;
        cx.pin(&osd, "fspath", &rt.deps)?;
    }
    store_type(&PURE_BASE, &pure);
    store_type(&PATH_BASE, &path);
    let od = if owner == "Path" { type_dict(&path)? } else { pd };
    let f = underlying(&cx.get(&od, attr)?)?;
    cx.vectorcall(&f, kernel, &accel::FUNCTION_VECTORCALL)
}

/// The function objects whose fingerprints all matched (strong references leaked).
static VALIDATED: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

fn store_type(cell: &AtomicPtr<ffi::PyTypeObject>, t: &Bound<'_, PyType>) {
    let p = t.as_type_ptr();
    if cell.load(Relaxed) != p {
        // The class lives as long as its module; keep a reference for the process.
        let _ = t.clone().into_ptr();
        cell.store(p, Relaxed);
    }
}

/// One exact path receiver, no keywords, `n` positional arguments.
macro_rules! receiver {
    ($rt:ident, $func:ident, $args:ident, $n:ident, $kw:ident, $want:expr, |$s:ident| $body:expr) => {
        if !$kw.is_null() || $n != $want || !exact_path(*$args) {
            None
        } else {
            match rt::guards(&$rt, $func) {
                Some(g) => Some(g),
                None => {
                    let $s = *$args;
                    $body
                }
            }
        }
    };
}

crate::paths_kernel!(k_str, RT_STR, STR_T, |func, args, nargsf, n, kw| {
    receiver!(RT_STR, func, args, n, kw, 1, |s| locked(s, || ensure_str(s).map(|o| ffi::Py_NewRef(o))))
});

crate::paths_kernel!(k_name, RT_NAME, NAME_T, |func, args, nargsf, n, kw| {
    receiver!(RT_NAME, func, args, n, kw, 1, |s| locked(s, || name_of(s).map(|o| ffi::Py_NewRef(o))))
});

/// `self.name` (borrowed): the last tail item, or `''`.
unsafe fn name_of(s: O) -> Option<O> {
    // SAFETY: as `parsed`.
    unsafe {
        let p = parsed(s)?;
        let n = ffi::PyList_GET_SIZE(p.tail);
        if n == 0 {
            static EMPTY: AtomicUsize = AtomicUsize::new(0);
            if EMPTY.load(Relaxed) == 0 {
                EMPTY.store(str_from_units::<u8>(&[]) as usize, Relaxed);
            }
            return Some(EMPTY.load(Relaxed) as O);
        }
        let it = ffi::PyList_GET_ITEM(p.tail, n - 1);
        exact_str(it).then_some(it)
    }
}

/// `name.lstrip('.')` then the index of its last `.` (in `name`).
fn suffix_start<T: Unit>(t: &[T]) -> Option<usize> {
    let start = t.iter().position(|&c| c != T::DOT).unwrap_or(t.len());
    t[start..].iter().rposition(|&c| c == T::DOT).map(|i| start + i)
}

/// `i = name.rfind('.')`, kept when `name[:i]` has a non-dot character.
fn stem_end<T: Unit>(t: &[T]) -> Option<usize> {
    t.iter().rposition(|&c| c == T::DOT).filter(|&i| t[..i].iter().any(|&c| c != T::DOT))
}

crate::paths_kernel!(k_suffix, RT_SUFFIX, SUFFIX_T, |func, args, nargsf, n, kw| {
    receiver!(RT_SUFFIX, func, args, n, kw, 1, |s| locked(s, || {
        // name.lstrip('.') then name[i:] from its last '.', else ''.
        let name = name_of(s)?;
        let v = str_view(name);
        let r = crate::with_text!(v.text, |t| suffix_start(t));
        Some(match r {
            Some(i) => rt::substr(name, i, v.len()),
            None => str_from_units::<u8>(&[]),
        })
    }))
});

crate::paths_kernel!(k_stem, RT_STEM, STEM_T, |func, args, nargsf, n, kw| {
    receiver!(RT_STEM, func, args, n, kw, 1, |s| locked(s, || {
        // stem = name[:i] if it has a non-dot character, else name.
        let name = name_of(s)?;
        let v = str_view(name);
        let r = crate::with_text!(v.text, |t| stem_end(t));
        Some(match r {
            Some(i) => rt::substr(name, 0, i),
            None => ffi::Py_NewRef(name),
        })
    }))
});

crate::paths_kernel!(k_parts, RT_PARTS, PARTS_T, |func, args, nargsf, n, kw| {
    receiver!(RT_PARTS, func, args, n, kw, 1, |s| locked(s, || {
        let p = parsed(s)?;
        let n = ffi::PyList_GET_SIZE(p.tail);
        let has_root = ffi::PyUnicode_GET_LENGTH(p.root) != 0;
        let t = ffi::PyTuple_New(n + isize::from(has_root));
        if t.is_null() {
            return Some(t);
        }
        let mut k = 0;
        if has_root {
            // drive + root: '' + root is root itself.
            ffi::PyTuple_SET_ITEM(t, 0, ffi::Py_NewRef(p.root));
            k = 1;
        }
        for i in 0..n {
            ffi::PyTuple_SET_ITEM(t, k + i, ffi::Py_NewRef(ffi::PyList_GET_ITEM(p.tail, i)));
        }
        Some(t)
    }))
});

crate::paths_kernel!(k_parent, RT_PARENT, PARENT_T, |func, args, nargsf, n, kw| {
    receiver!(RT_PARENT, func, args, n, kw, 1, |s| locked(s, || {
        let p = parsed(s)?;
        let n = ffi::PyList_GET_SIZE(p.tail);
        if n == 0 {
            return Some(ffi::Py_NewRef(s));
        }
        let tail = ffi::PyList_GetSlice(p.tail, 0, n - 1);
        if tail.is_null() {
            return Some(tail);
        }
        Some(from_parsed_parts(ffi::Py_TYPE(s), p.root, tail))
    }))
});

crate::paths_kernel!(k_truediv, RT_TRUEDIV, TRUEDIV_T, |func, args, nargsf, n, kw| {
    receiver!(RT_TRUEDIV, func, args, n, kw, 2, |s| {
        // `type(self)(self, key)`: `_raw_paths` = self's raw paths, then `key` (an exact
        // str) or key's raw paths (an exact path of the same flavour).
        let key = *args.add(1);
        let raw = if exact_str(key) {
            locked(s, || concat_raw(s, None, key))
        } else if !exact_path(key) {
            None
        } else if key == s {
            locked(s, || concat_raw(s, Some(s), key))
        } else {
            locked2(s, key, || concat_raw(s, Some(key), key))
        };
        match raw {
            Some(r) if !r.is_null() => Some(new_path(ffi::Py_TYPE(s), r)),
            other => other,
        }
    })
});

/// The exact list in `_raw_paths`.
#[inline(always)]
unsafe fn raw_list(o: O) -> Option<O> {
    // SAFETY: critical section of `o` held.
    unsafe {
        let r = get(o, RAW);
        (!r.is_null() && ffi::PyList_CheckExact(r) != 0).then_some(r)
    }
}

/// A new list: `s._raw_paths` + (`kp._raw_paths` or `[key]`); `Some(null)` on error.
unsafe fn concat_raw(s: O, kp: Option<O>, key: O) -> Option<O> {
    // SAFETY: critical sections held by the caller.
    unsafe {
        let a = raw_list(s)?;
        let b = match kp {
            Some(k) => Some(raw_list(k)?),
            None => None,
        };
        let n = ffi::PyList_GET_SIZE(a);
        let l = ffi::PyList_GetSlice(a, 0, n);
        if l.is_null() {
            return Some(l);
        }
        let rc = match b {
            Some(b) => ffi::PyList_SetSlice(l, n, n, b),
            None => ffi::PyList_Append(l, key),
        };
        if rc != 0 {
            ffi::Py_DECREF(l);
            return Some(std::ptr::null_mut());
        }
        Some(l)
    }
}

crate::paths_kernel!(k_relative_to, RT_RELATIVE_TO, RELATIVE_TO_T, |func, args, nargsf, n, kw| {
    receiver!(RT_RELATIVE_TO, func, args, n, kw, 2, |s| relative_to(s, *args.add(1)))
});

/// `self.relative_to(other)` without `walk_up`: `other` (an exact str, or an exact path of
/// the same flavour) must be `self` or one of its parents, compared as POSIX strings; the
/// result is `self._from_parsed_parts('', '', self._tail[len(other._tail):])`. Fills
/// `self`'s parsed slots, `_str` and `_str_normcase_cached` (and `other`'s, when a path)
/// as the original's comparisons do. Declines when the original raises (it may already have
/// filled `self`'s parsed slots, with the values the original fills them with).
unsafe fn relative_to(s: O, other: O) -> Option<O> {
    // SAFETY: attached thread; live objects.
    unsafe {
        if exact_str(other) {
            let raw = ffi::PyList_New(1);
            if raw.is_null() {
                return Some(raw);
            }
            ffi::PyList_SET_ITEM(raw, 0, ffi::Py_NewRef(other));
            let (oroot, otail) = match parse_raw(raw) {
                Some(x) => x,
                None => {
                    ffi::Py_DECREF(raw);
                    return None;
                }
            };
            ffi::Py_DECREF(raw);
            let r = locked(s, || rel_from(s, oroot, otail, None));
            ffi::Py_DECREF(oroot);
            ffi::Py_DECREF(otail);
            return r;
        }
        if !exact_path(other) {
            return None;
        }
        if other == s {
            return locked(s, || {
                let p = parsed(s)?;
                rel_from(s, p.root, p.tail, Some(s))
            });
        }
        locked2(s, other, || {
            // Validate both before writing anything into either.
            let ok = |o: O| get(o, RAW).is_null() || ffi::PyList_CheckExact(get(o, RAW)) != 0;
            if !ok(s) || !ok(other) {
                return None;
            }
            let po = parsed(other)?;
            rel_from(s, po.root, po.tail, Some(other))
        })
    }
}

/// The relative path of `s` under (`oroot`, `otail`), or `None` when it is not under it.
unsafe fn rel_from(s: O, oroot: O, otail: O, other_obj: Option<O>) -> Option<O> {
    // SAFETY: critical sections held by the caller; exact str/list values.
    unsafe {
        let p = parsed(s)?;
        if ffi::PyUnicode_Compare(p.root, oroot) != 0 {
            return None;
        }
        let n = ffi::PyList_GET_SIZE(p.tail);
        let k = ffi::PyList_GET_SIZE(otail);
        if k > n {
            return None;
        }
        for i in 0..k {
            let a = ffi::PyList_GET_ITEM(p.tail, i);
            let b = ffi::PyList_GET_ITEM(otail, i);
            if !exact_str(a) || !exact_str(b) || ffi::PyUnicode_Compare(a, b) != 0 {
                return None;
            }
        }
        // The comparisons `path == self` fill both objects' `_str_normcase`.
        ensure_normcase(s)?;
        if let Some(o) = other_obj {
            ensure_normcase(o)?;
        }
        let tail = ffi::PyList_GetSlice(p.tail, k, n);
        if tail.is_null() {
            return Some(tail);
        }
        let empty = str_from_units::<u8>(&[]);
        let r = from_parsed_parts(ffi::Py_TYPE(s), empty, tail);
        ffi::Py_DECREF(empty);
        Some(r)
    }
}

crate::paths_kernel!(k_eq, RT_EQ, EQ_T, |func, args, nargsf, n, kw| {
    receiver!(RT_EQ, func, args, n, kw, 2, |s| {
        let other = *args.add(1);
        if !exact_path(other) {
            None
        } else {
            let both = |o: O| -> Option<O> { ensure_normcase(o) };
            let r = if other == s {
                locked(s, || both(s).map(|_| true))
            } else {
                locked2(s, other, || {
                    let a = both(s)?;
                    let b = both(other)?;
                    // Same parser: both classes use posixpath (pinned).
                    Some(ffi::PyUnicode_Compare(a, b) == 0)
                })
            };
            r.map(|eq| ffi::Py_NewRef(if eq { ffi::Py_True() } else { ffi::Py_False() }))
        }
    })
});

crate::paths_kernel!(k_hash, RT_HASH, HASH_T, |func, args, nargsf, n, kw| {
    receiver!(RT_HASH, func, args, n, kw, 1, |s| locked(s, || {
        let h = get(s, HASH);
        if !h.is_null() {
            return Some(ffi::Py_NewRef(h));
        }
        let c = ensure_normcase(s)?;
        let v = ffi::PyObject_Hash(c);
        if v == -1 {
            return Some(std::ptr::null_mut());
        }
        let o = ffi::PyLong_FromSsize_t(v);
        if o.is_null() {
            return Some(o);
        }
        put(s, HASH, ffi::Py_NewRef(o));
        Some(o)
    }))
});

/// `PurePath.__new__(cls, *args, **kwargs)` / `Path.__new__`: `object.__new__(cls)`, with
/// `PurePath` → `PurePosixPath` and `Path` → `PosixPath` (`os.name` is 'posix', pinned).
unsafe fn new_for(cls: O, base: &AtomicPtr<ffi::PyTypeObject>, posix: &AtomicPtr<ffi::PyTypeObject>) -> Option<O> {
    // SAFETY: attached thread.
    unsafe {
        let c = cls.cast::<ffi::PyTypeObject>();
        let target = if c == base.load(Relaxed) {
            posix.load(Relaxed)
        } else if c == PURE_POSIX.load(Relaxed) || c == POSIX.load(Relaxed) {
            c
        } else {
            return None;
        };
        let object = &raw const ffi::PyBaseObject_Type;
        let new = (*object).tp_new.expect("object.tp_new");
        let empty = ffi::PyTuple_New(0);
        let o = new(target, empty, std::ptr::null_mut());
        ffi::Py_DECREF(empty);
        Some(o)
    }
}

crate::paths_kernel!(k_new_pure, RT_NEW_PURE, NEW_PURE_T, |func, args, nargsf, n, kw| {
    if !kw.is_null() || n == 0 {
        None
    } else {
        let cls = *args;
        // PosixPath's `__new__` is Path's: only the pure classes come here.
        if cls.cast::<ffi::PyTypeObject>() == POSIX.load(Relaxed) {
            None
        } else {
            match rt::guards(&RT_NEW_PURE, func) {
                Some(g) => Some(g),
                None => new_for(cls, &PURE_BASE, &PURE_POSIX),
            }
        }
    }
});

crate::paths_kernel!(k_new_path, RT_NEW_PATH, NEW_PATH_T, |func, args, nargsf, n, kw| {
    if !kw.is_null() || n == 0 || (*args).cast::<ffi::PyTypeObject>() == PURE_POSIX.load(Relaxed) {
        None
    } else {
        match rt::guards(&RT_NEW_PATH, func) {
            Some(g) => Some(g),
            None => new_for(*args, &PATH_BASE, &POSIX),
        }
    }
});

crate::paths_kernel!(k_init, RT_INIT, INIT_T, |func, args, nargsf, n, kw| {
    // `__init__(self, *args)`: `_raw_paths` = the exact str arguments, and the raw paths of
    // exact POSIX path arguments (same parser), in order.
    if !kw.is_null() || n == 0 || !exact_path(*args) {
        None
    } else {
        let s = *args;
        let rest = std::slice::from_raw_parts(args.add(1), n - 1);
        if !rest.iter().all(|&a| exact_str(a) || exact_path(a)) {
            None
        } else {
            match rt::guards(&RT_INIT, func) {
                Some(g) => Some(g),
                None => init_raw(s, rest),
            }
        }
    }
});

/// Build the new `_raw_paths` and store it (replacing an old value, as `self._raw_paths =
/// paths` does); returns `None` (Python's result) or declines.
unsafe fn init_raw(s: O, rest: &[O]) -> Option<O> {
    // SAFETY: attached thread; live objects.
    unsafe {
        let l = ffi::PyList_New(0);
        if l.is_null() {
            return Some(l);
        }
        for &a in rest {
            let rc = if exact_str(a) {
                ffi::PyList_Append(l, a)
            } else {
                // `paths.extend(arg._raw_paths)` (read under the argument's lock).
                let r = locked(a, || {
                    let r = raw_list(a)?;
                    Some(ffi::PyList_GetSlice(r, 0, ffi::PyList_GET_SIZE(r)))
                });
                match r {
                    None => {
                        ffi::Py_DECREF(l);
                        return None;
                    }
                    Some(r) if r.is_null() => -1,
                    Some(r) => {
                        let n = ffi::PyList_GET_SIZE(l);
                        let rc = ffi::PyList_SetSlice(l, n, n, r);
                        ffi::Py_DECREF(r);
                        rc
                    }
                }
            };
            if rc != 0 {
                ffi::Py_DECREF(l);
                return Some(std::ptr::null_mut());
            }
        }
        let old = locked(s, || cell(s, RAW).swap(l, Release));
        ffi::Py_XDECREF(old);
        Some(ffi::Py_NewRef(ffi::Py_None()))
    }
}

pub fn register(_accel: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}
