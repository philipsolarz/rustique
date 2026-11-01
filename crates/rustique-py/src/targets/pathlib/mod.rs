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

use crate::accel::{self, DepFlag, Fingerprints, GroupSpec, Prepare, Refusal, TargetSpec};
use crate::refuse;
use crate::targets::posixpath::rt::{self, O, Rt, Text, exact_str, str_from_units, str_view};

// ---------------------------------------------------------------------------------------
// Fingerprints
// ---------------------------------------------------------------------------------------

pub(crate) const FPS: &[(&str, Fingerprints)] = &[
    ("PurePath.__new__", &[(3, 14, "90d53423c42cd9f2"), (3, 15, "a0545f276ef085b5")]),
    ("PurePath.__init__", &[(3, 14, "a35cafa2f4ebb06a"), (3, 15, "bd95237cb1780a25")]),
    ("PurePath.with_segments", &[(3, 14, "93fb271f0b71835e"), (3, 15, "185c894628a66be5")]),
    ("PurePath.__truediv__", &[(3, 14, "34032fe42e88f1f9"), (3, 15, "312ecc99f8ed4c4e")]),
    ("PurePath.__str__", &[(3, 14, "be38afbe0dd95720"), (3, 15, "435c10ada5537957")]),
    ("PurePath._format_parsed_parts", &[(3, 14, "7768f127fd92d97f"), (3, 15, "b4ba04165f5c4536")]),
    ("PurePath._from_parsed_parts", &[(3, 14, "4dafd1666d14a188"), (3, 15, "e58e5f84a4a7b4e0")]),
    ("PurePath._from_parsed_string", &[(3, 14, "8c6c016c800b9be2"), (3, 15, "47b96b9750737e75")]),
    ("PurePath._parse_path", &[(3, 14, "c49a41e3d1443ea0"), (3, 15, "91a2548967526f19")]),
    ("PurePath._raw_path", &[(3, 14, "8fecf3d60ce31cea"), (3, 15, "e045eecbfbf2dc6f")]),
    ("PurePath.drive", &[(3, 14, "7434b6710251141f"), (3, 15, "49fdd1be6e82942b")]),
    ("PurePath.root", &[(3, 14, "9137391bf4d9235b"), (3, 15, "cacffe548be351e9")]),
    ("PurePath._tail", &[(3, 14, "f54230d1aee5d02a"), (3, 15, "fca2b3b15057fbe5")]),
    ("PurePath.parent", &[(3, 14, "ccd50e73f288cb62"), (3, 15, "10afd99fa60c1767")]),
    ("PurePath.name", &[(3, 14, "04f63687b463ce51"), (3, 15, "6742d65169366db7")]),
    ("PurePath.suffix", &[(3, 14, "6db56329b81ec4c3"), (3, 15, "2743f98b22787ea0")]),
    ("PurePath.stem", &[(3, 14, "9130798afb03ca16"), (3, 15, "d90002f758fdcbf0")]),
    ("PurePath.parts", &[(3, 14, "dc587872dafaf39e"), (3, 15, "b0757588ced0408c")]),
    ("PurePath.parents", &[(3, 14, "7577d579015fb393"), (3, 15, "cfdcd46ebd00500a")]),
    ("PurePath.relative_to", &[(3, 14, "14c9dd961cb28ea9"), (3, 15, "330ea3433200c014")]),
    ("PurePath._str_normcase", &[(3, 14, "69e372eb8d93af60"), (3, 15, "6b40c2720cc18319")]),
    ("PurePath.__eq__", &[(3, 14, "682fc4ba88e06fe6"), (3, 15, "36689d128b81d2f2")]),
    ("PurePath.__hash__", &[(3, 14, "5183a881a15b15e8"), (3, 15, "bb899776aaffad3b")]),
    ("_PathParents.__init__", &[(3, 14, "96a115477db9da5e"), (3, 15, "acd964abdaf52b03")]),
    ("_PathParents.__len__", &[(3, 14, "015fec85711a1738"), (3, 15, "09822c3aec6c480b")]),
    ("_PathParents.__getitem__", &[(3, 14, "5ec086016dcf81d5"), (3, 15, "eb56850e04923f1a")]),
    ("Path.__new__", &[(3, 14, "78d2856d654304e2"), (3, 15, "060bb510a3f822a6")]),
    ("posixpath.splitdrive", &[(3, 14, "8514869d9c2a063f"), (3, 15, "3bb7f4243a107b0e")]),
    ("posixpath.splitroot", &[(3, 14, "207c2ea5f38e1e3d"), (3, 15, "886f7acf26378ad4")]),
];

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
    // SAFETY: the validated classes.
    unsafe {
        (t == PURE_POSIX.load(Relaxed) && class_intact(t, &TAG_PURE_POSIX, &ORIG_PURE_POSIX))
            || (t == POSIX.load(Relaxed) && class_intact(t, &TAG_POSIX, &ORIG_POSIX))
    }
}

// ---------------------------------------------------------------------------------------
// Class guard: instead of pinning every class attribute the originals reach (dozens of
// dict pins per target at install), the kernels compare the class's `tp_version_tag`
// (CPython assigns a new tag whenever the class or any base is modified, `__bases__`
// included). On a new tag, the attributes are looked up again (`_PyType_Lookup`, no Python
// code) and compared with the validated originals; if all are the same objects (a patch was
// undone), the new tag is accepted, otherwise the call falls back.
// ---------------------------------------------------------------------------------------

python_extern! {
    fn _PyType_Lookup(ty: *mut ffi::PyTypeObject, name: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyUnstable_Type_AssignVersionTag(ty: *mut ffi::PyTypeObject) -> std::ffi::c_int;
}

/// Every PurePath attribute any kernel's original reaches through `self`/`cls`.
const CLASS_ATTRS: &[&str] = &[
    "parser",
    "_parse_path",
    "_raw_path",
    "drive",
    "root",
    "_tail",
    "_format_parsed_parts",
    "_from_parsed_parts",
    "_from_parsed_string",
    "with_segments",
    "__new__",
    "__init__",
    "name",
    "parents",
    "__eq__",
    "_str_normcase",
    "__str__",
    "__hash__",
];
/// `_PathParents` attributes `path in self.parents` reaches (relative_to).
const PARENTS_ATTRS: &[&str] = &["__init__", "__len__", "__getitem__", "__contains__", "__iter__"];

/// (interned name, validated attribute) per class: strong references for the process.
static ORIG_POSIX: std::sync::Mutex<Vec<(usize, usize)>> = std::sync::Mutex::new(Vec::new());
static ORIG_PURE_POSIX: std::sync::Mutex<Vec<(usize, usize)>> = std::sync::Mutex::new(Vec::new());
static ORIG_PARENTS: std::sync::Mutex<Vec<(usize, usize)>> = std::sync::Mutex::new(Vec::new());
static TAG_PURE_POSIX: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static TAG_POSIX: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static PARENTS_TYPE: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(std::ptr::null_mut());
static TAG_PARENTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[inline(always)]
unsafe fn version_tag(t: *mut ffi::PyTypeObject) -> u32 {
    // SAFETY: a live type; the tag is written atomically by CPython (relaxed read).
    unsafe { (*(&raw const (*t).tp_version_tag).cast::<std::sync::atomic::AtomicU32>()).load(Relaxed) }
}

/// The class's attributes are the validated ones (fast: its version tag is the accepted one).
#[inline(always)]
unsafe fn class_intact(
    t: *mut ffi::PyTypeObject,
    accepted: &std::sync::atomic::AtomicU32,
    attrs: &std::sync::Mutex<Vec<(usize, usize)>>,
) -> bool {
    // SAFETY: live type; attached thread for the slow path.
    unsafe {
        let tag = version_tag(t);
        (tag != 0 && tag == accepted.load(Relaxed)) || reverify(t, accepted, attrs)
    }
}

/// Look the attributes up again; accept the class's (new) version tag if they are all the
/// validated objects.
#[cold]
unsafe fn reverify(
    t: *mut ffi::PyTypeObject,
    accepted: &std::sync::atomic::AtomicU32,
    attrs: &std::sync::Mutex<Vec<(usize, usize)>>,
) -> bool {
    let Ok(attrs) = attrs.lock() else { return false };
    // SAFETY: attached thread; live type; interned names; results only compared.
    unsafe {
        if attrs.is_empty() || attrs.iter().any(|&(n, v)| _PyType_Lookup(t, n as O) as usize != v) {
            return false;
        }
        if PyUnstable_Type_AssignVersionTag(t) == 0 {
            return false;
        }
        accepted.store(version_tag(t), Relaxed);
    }
    true
}

/// `_PathParents` unchanged (relative_to's original iterates `self.parents`).
#[inline(always)]
unsafe fn parents_intact() -> bool {
    let t = PARENTS_TYPE.load(Relaxed);
    if t.is_null() {
        return false;
    }
    // SAFETY: the validated, live class.
    unsafe {
        let tag = version_tag(t);
        (tag != 0 && tag == TAG_PARENTS.load(Relaxed)) || reverify(t, &TAG_PARENTS, &ORIG_PARENTS)
    }
}

/// Record the validated attributes and accept the classes' current tags.
fn record_class(
    py: Python<'_>,
    posix: &Bound<'_, PyType>,
    pure_posix: &Bound<'_, PyType>,
    parents: &Bound<'_, PyType>,
) -> Result<(), Refusal> {
    let lookup = |t: &Bound<'_, PyType>, names: &[&str]| -> Result<Vec<(usize, usize)>, Refusal> {
        let mut v = Vec::with_capacity(names.len());
        for n in names {
            let name = PyString::intern(py, n);
            // SAFETY: a ready type; interned name; the result is kept with a new reference.
            let d = unsafe { _PyType_Lookup(t.as_type_ptr(), name.as_ptr()) };
            if d.is_null() {
                return Err(refuse!("{n} not found on the class"));
            }
            // SAFETY: a live object found in the class dicts.
            unsafe { ffi::Py_IncRef(d) };
            v.push((name.into_ptr() as usize, d as usize));
        }
        Ok(v)
    };
    let a = lookup(posix, CLASS_ATTRS)?;
    let b = lookup(pure_posix, CLASS_ATTRS)?;
    let pa = lookup(parents, PARENTS_ATTRS)?;
    if let Ok(mut v) = ORIG_POSIX.lock() {
        *v = a;
    }
    if let Ok(mut v) = ORIG_PURE_POSIX.lock() {
        *v = b;
    }
    if let Ok(mut v) = ORIG_PARENTS.lock() {
        *v = pa;
    }
    PARENTS_TYPE.store(parents.as_type_ptr(), Relaxed);
    // SAFETY: ready types.
    unsafe {
        for (t, tag) in [(posix, &TAG_POSIX), (pure_posix, &TAG_PURE_POSIX), (parents, &TAG_PARENTS)] {
            if PyUnstable_Type_AssignVersionTag(t.as_type_ptr()) == 0 {
                return Err(refuse!("no type version tag"));
            }
            tag.store(version_tag(t.as_type_ptr()), Relaxed);
        }
    }
    Ok(())
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

/// The group's dependency flag, pinned once by [`SHARED`] (class attributes are guarded by
/// the classes' version tags instead).
static DEPS: DepFlag = DepFlag::new();

/// Declarations shared by the thirteen targets (prepared once, kept while any is installed).
pub static SHARED: GroupSpec = GroupSpec { name: "pathlib", prepare: shared };

static RT_STR: Rt = Rt::new(&DEPS);
static RT_TRUEDIV: Rt = Rt::new(&DEPS);
static RT_PARENT: Rt = Rt::new(&DEPS);
static RT_NAME: Rt = Rt::new(&DEPS);
static RT_SUFFIX: Rt = Rt::new(&DEPS);
static RT_STEM: Rt = Rt::new(&DEPS);
static RT_PARTS: Rt = Rt::new(&DEPS);
static RT_RELATIVE_TO: Rt = Rt::new(&DEPS);
static RT_EQ: Rt = Rt::new(&DEPS);
static RT_HASH: Rt = Rt::new(&DEPS);
static RT_NEW_PURE: Rt = Rt::new(&DEPS);
static RT_NEW_PATH: Rt = Rt::new(&DEPS);
static RT_INIT: Rt = Rt::new(&DEPS);

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
            shared: Some(&SHARED),
            release: Some(release),
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
/// The group's shared declarations: fingerprints of every function the kernels reproduce,
/// the two exact classes (default metaclass, `posixpath` parser, PurePath's slot layout,
/// their attributes recorded for the version-tag guard), `posixpath.sep/altsep`, `os.name`,
/// and the pins of every module global and builtin any pathlib original resolves at call
/// time, once for the thirteen targets.
fn shared(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
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
    for (name, _) in FPS {
        let (owner, a) = name.split_once('.').expect("dotted");
        let f = match owner {
            "PurePath" => underlying(&cx.get(&pd, a)?)?,
            "_PathParents" => underlying(&cx.get(&type_dict(&parents)?, a)?)?,
            "Path" => underlying(&cx.get(&type_dict(&path)?, a)?)?,
            _ => cx.get(&cx.loaded_module(owner)?.dict(), a)?,
        };
        if *name == "posixpath.splitroot" {
            rt::check_c(cx, FPS, name, &f)?;
        } else {
            rt::check_fp(cx, FPS, name, &f)?;
        }
    }
    let pp = cx.loaded_module("posixpath")?;
    for t in [&pure_posix, &posix] {
        if !t.get_type().is(py.get_type::<PyType>()) {
            return Err(refuse!("custom metaclass"));
        }
        // SAFETY: ready types.
        if unsafe { ffi::PyType_IsSubtype(t.as_type_ptr(), pure.as_type_ptr()) } == 0 {
            return Err(refuse!("class hierarchy differs"));
        }
    }
    if !cx.type_attr(&pure_posix, "parser")?.is(&pp) {
        return Err(refuse!("PurePosixPath.parser is not posixpath"));
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
    let ppd = pp.dict();
    if !cx.get(&ppd, "sep")?.eq("/")? || !cx.get(&ppd, "altsep")?.is_none() {
        return Err(refuse!("posixpath.sep/altsep differ"));
    }
    let osm = cx.loaded_module("os")?;
    if !cx.get(&ppd, "os")?.is(&osm) {
        return Err(refuse!("posixpath.os is not the os module"));
    }
    let osd = osm.dict();
    if !cx.get(&osd, "name")?.eq("posix")? {
        return Err(refuse!("os.name is not 'posix'"));
    }
    let join = cx.get(&ppd, "join")?;
    rt::check_fp(cx, crate::targets::posixpath::FPS, "posixpath.join", &join)?;
    rt::check_fp(cx, crate::targets::posixpath::FPS, "posixpath._get_sep", &cx.get(&ppd, "_get_sep")?)?;
    rt::check_c(cx, crate::targets::posixpath::FPS, "os.fspath", &cx.get(&osd, "fspath")?)?;
    // The attributes the originals reach are the ones just fingerprinted (looked up
    // through each class's MRO); the kernels then guard them by version tag.
    record_class(py, &posix, &pure_posix, &parents)?;
    store_type(&PURE_POSIX, &pure_posix);
    store_type(&POSIX, &posix);
    store_type(&PURE_BASE, &pure);
    store_type(&PATH_BASE, &path);
    if DOT.load(Relaxed) == 0 {
        DOT.store(rt::leak(PyString::new(py, ".").as_any()), Relaxed);
    }
    let ppd = pp.dict();
    for key in ["sep", "altsep", "join", "splitroot", "splitdrive", "_get_sep", "os"] {
        cx.pin(&ppd, key, &DEPS)?;
    }
    for name in ["isinstance", "bytes"] {
        cx.pin_global(&ppd, name, &DEPS)?;
    }
    for key in [
        "os",
        "PurePath",
        "PurePosixPath",
        "PosixPath",
        "Path",
        "PureWindowsPath",
        "WindowsPath",
        "posixpath",
        "chain",
        "_PathParents",
    ] {
        cx.pin(&g, key, &DEPS)?;
    }
    for name in ["object", "isinstance", "str", "TypeError", "type", "tuple", "hash", "hasattr", "ValueError", "len"] {
        cx.pin_global(&g, name, &DEPS)?;
    }
    // `__new__` reads `os.name`; `__init__`, `posixpath.join` and `splitdrive` call `os.fspath`.
    for key in ["name", "fspath"] {
        cx.pin(&osd, key, &DEPS)?;
    }
    Ok(())
}

/// A target: its vectorcall site (everything else is shared).
fn prep(
    cx: &mut Prepare<'_>,
    owner: &str,
    attr: &str,
    _rt: &'static Rt,
    kernel: ffi::vectorcallfunc,
) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let cls =
        cx.get(&g, if owner == "Path" { "Path" } else { "PurePath" })?.cast_into::<PyType>().map_err(PyErr::from)?;
    let f = underlying(&cx.get(&type_dict(&cls)?, attr)?)?;
    cx.vectorcall(&f, kernel, &accel::FUNCTION_VECTORCALL)
}

/// Exit release: the verified functions, the classes and their recorded attributes (the
/// methods' globals are the pathlib module's). Kernels compare a receiver's type with the
/// class cells before anything else (null: decline) and reach the attribute lists only
/// for a matching type; `_PathParents` is checked for null; the accepted tags are reset.
/// `DOT`/the empty str stay (they keep nothing alive).
fn release(py: Python<'_>) {
    rt::release(py);
    let types: Vec<_> = [&PURE_POSIX, &POSIX, &PURE_BASE, &PATH_BASE]
        .into_iter()
        .map(|c| c.swap(std::ptr::null_mut(), Relaxed))
        .filter(|p| !p.is_null())
        .collect();
    PARENTS_TYPE.store(std::ptr::null_mut(), Relaxed);
    for tag in [&TAG_PURE_POSIX, &TAG_POSIX, &TAG_PARENTS] {
        tag.store(0, Relaxed);
    }
    let mut refs = Vec::new();
    for attrs in [&ORIG_POSIX, &ORIG_PURE_POSIX, &ORIG_PARENTS] {
        refs.extend(std::mem::take(&mut *attrs.lock().unwrap_or_else(|p| p.into_inner())));
    }
    // SAFETY: attached thread; the references taken by `record_class` and `store_type`.
    unsafe {
        for (name, value) in refs {
            ffi::Py_DECREF(name as O);
            ffi::Py_DECREF(value as O);
        }
        for t in types {
            ffi::Py_DECREF(t.cast());
        }
    }
}

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
        if !parents_intact() {
            return None;
        }
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
