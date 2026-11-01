//! Group `textwrap`: `wrap`, `fill`, `shorten`, `dedent`, `indent` and
//! `TextWrapper.wrap`/`fill`, as vectorcall kernels on the module's function objects and
//! the class's methods, running `rustique_core::textwrap` (CPython's algorithms on code
//! points, the regex chunking replicated exactly, Unicode classes from CPython's own
//! database). See docs/targets/textwrap.md.
//!
//! OWNER: utils workstream.
//!
//! Exact subset: `str` text (exact type); options of exact types (`width`, `tabsize`,
//! `max_lines`: int; indents and `placeholder`: str; flags: bool, int or None — the values
//! CPython only tests for truth, or `is True` for `break_on_hyphens`); `TextWrapper`
//! instances whose class resolves every method and class attribute the algorithm uses to
//! CPython's own (subclasses that override nothing qualify) and that hold the options as
//! plain instance attributes. Everything else, and every error (`width <= 0`, placeholder
//! too large, non-str text), runs CPython's original. All targets are side-effect free:
//! `checkable`.

use std::ffi::c_int;
use std::sync::atomic::{AtomicPtr, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyType};
use rustique_core::textwrap::{self as tw, Chunk, Classes, WrapError, WrapOpts};

use crate::accel::{self, Original, Prepare, Reason, Refusal, TargetSpec};
use crate::targets::utils_kit::{
    _PyType_Lookup, Guard, INST, O, TypeCache, call_original, generic_getattro, int_value, interned, is_type, keep,
    nargs, pin_globals, unshadowed, version_tag,
};

const R: Ordering = Ordering::Relaxed;

macro_rules! null {
    () => {
        AtomicPtr::new(std::ptr::null_mut())
    };
}

// ---------------------------------------------------------------------------------------
// Unicode classes: CPython's database (exported `_PyUnicode_Is*`), ASCII inline.
// ---------------------------------------------------------------------------------------

type IsFn = unsafe extern "C" fn(u32) -> c_int;
static IS_ALPHA: Original<IsFn> = Original::new();
static IS_DECIMAL: Original<IsFn> = Original::new();
static IS_DIGIT: Original<IsFn> = Original::new();
static IS_NUMERIC: Original<IsFn> = Original::new();
static IS_SPACE: Original<IsFn> = Original::new();

struct Cls;

impl Classes for Cls {
    #[inline]
    fn is_word(&self, c: u32) -> bool {
        if c < 128 {
            return (c as u8).is_ascii_alphanumeric() || c == 0x5f;
        }
        // SAFETY: CPython's classification functions (resolved in `prepare`).
        unsafe {
            IS_ALPHA.get()(c) != 0 || IS_DECIMAL.get()(c) != 0 || IS_DIGIT.get()(c) != 0 || IS_NUMERIC.get()(c) != 0
        }
    }
    #[inline]
    fn is_decimal(&self, c: u32) -> bool {
        if c < 128 {
            return (c as u8).is_ascii_digit();
        }
        // SAFETY: as above.
        unsafe { IS_DECIMAL.get()(c) != 0 }
    }
    #[inline]
    fn is_space(&self, c: u32) -> bool {
        if c < 128 {
            return matches!(c, 0x09..=0x0d | 0x1c..=0x20);
        }
        // SAFETY: as above.
        unsafe { IS_SPACE.get()(c) != 0 }
    }
}

// ---------------------------------------------------------------------------------------
// str <-> code points
// ---------------------------------------------------------------------------------------

/// The code points of an exact `str` (copied out of the PEP 393 buffer).
unsafe fn ucs4(s: O) -> Option<Vec<u32>> {
    // SAFETY: exact str.
    unsafe {
        let n = ffi::PyUnicode_GET_LENGTH(s) as usize;
        let mut v = vec![0u32; n + 1];
        if ffi::PyUnicode_AsUCS4(s, v.as_mut_ptr(), (n + 1) as ffi::Py_ssize_t, 1).is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        v.truncate(n);
        Some(v)
    }
}

/// A new `str` from code points (canonical PEP 393 kind).
unsafe fn new_str(v: &[u32]) -> O {
    // SAFETY: valid buffer.
    unsafe { ffi::PyUnicode_FromKindAndData(ffi::PyUnicode_4BYTE_KIND as c_int, v.as_ptr().cast(), v.len() as ffi::Py_ssize_t) }
}

#[inline(always)]
unsafe fn exact_str(o: O) -> bool {
    // SAFETY: live object.
    unsafe { is_type(o, &raw mut ffi::PyUnicode_Type) }
}

/// The truth value CPython computes for a flag, when that is pure (bool, int, None).
#[inline]
unsafe fn truth(o: O) -> Option<bool> {
    // SAFETY: live object.
    unsafe {
        if o == ffi::Py_True() {
            Some(true)
        } else if o == ffi::Py_False() || o == ffi::Py_None() {
            Some(false)
        } else if is_type(o, &raw mut ffi::PyLong_Type) {
            Some(ffi::PyObject_IsTrue(o) != 0)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------------------

/// `TextWrapper.__init__` parameter order.
const OPT_NAMES: [&str; 12] = [
    "width",
    "initial_indent",
    "subsequent_indent",
    "expand_tabs",
    "replace_whitespace",
    "fix_sentence_endings",
    "break_long_words",
    "drop_whitespace",
    "break_on_hyphens",
    "tabsize",
    "max_lines",
    "placeholder",
];

struct Opts {
    width: i64,
    initial_indent: Vec<u32>,
    subsequent_indent: Vec<u32>,
    expand_tabs: bool,
    replace_whitespace: bool,
    fix_sentence_endings: bool,
    break_long_words: bool,
    drop_whitespace: bool,
    /// `break_on_hyphens is True` (choice of regex)
    boh_is_true: bool,
    /// `bool(break_on_hyphens)` (`_handle_long_word`)
    boh_truthy: bool,
    tabsize: i64,
    max_lines: Option<i64>,
    placeholder: Vec<u32>,
}

impl Opts {
    /// `TextWrapper.__init__`'s defaults (checked against the function at install).
    fn defaults() -> Opts {
        Opts {
            width: 70,
            initial_indent: Vec::new(),
            subsequent_indent: Vec::new(),
            expand_tabs: true,
            replace_whitespace: true,
            fix_sentence_endings: false,
            break_long_words: true,
            drop_whitespace: true,
            boh_is_true: true,
            boh_truthy: true,
            tabsize: 8,
            max_lines: None,
            placeholder: " [...]".chars().map(u32::from).collect(),
        }
    }

    /// Set option `i` (index in [`OPT_NAMES`]) from a value; `None`: outside the subset.
    unsafe fn set(&mut self, i: usize, v: O) -> Option<()> {
        // SAFETY: live objects.
        unsafe {
            match i {
                0 => self.width = exact_int(v)?,
                1 => self.initial_indent = str_arg(v)?,
                2 => self.subsequent_indent = str_arg(v)?,
                3 => self.expand_tabs = truth(v)?,
                4 => self.replace_whitespace = truth(v)?,
                5 => self.fix_sentence_endings = truth(v)?,
                6 => self.break_long_words = truth(v)?,
                7 => self.drop_whitespace = truth(v)?,
                8 => {
                    self.boh_truthy = truth(v)?;
                    self.boh_is_true = v == ffi::Py_True();
                }
                9 => self.tabsize = exact_int(v)?,
                10 => self.max_lines = if v == ffi::Py_None() { None } else { Some(exact_int(v)?) },
                11 => self.placeholder = str_arg(v)?,
                _ => return None,
            }
            Some(())
        }
    }
}

#[inline]
unsafe fn exact_int(o: O) -> Option<i64> {
    // SAFETY: live object.
    unsafe {
        if !is_type(o, &raw mut ffi::PyLong_Type) {
            return None;
        }
        int_value(o).filter(|v| v.unsigned_abs() < 1 << 40)
    }
}

#[inline]
unsafe fn str_arg(o: O) -> Option<Vec<u32>> {
    // SAFETY: live object.
    unsafe { if exact_str(o) { ucs4(o) } else { None } }
}

// ---------------------------------------------------------------------------------------
// The algorithm
// ---------------------------------------------------------------------------------------

/// The wrapped lines of `text` (code points), or `None` when CPython raises (the caller
/// falls back). `whole` is set when the only line is the whole text, built from one chunk
/// with an empty indent, from munged text that is the input object itself: CPython then
/// returns the input `str` (`''.join([chunk])`, `'' + line`, `PyUnicode_Substring` of the
/// whole string, `expandtabs` without tabs: all return their argument).
fn wrap_lines(text: &[u32], o: &Opts, whole: &mut bool) -> Option<Vec<Vec<u32>>> {
    let cls = Cls;
    let munged_is_input = !o.replace_whitespace && (!o.expand_tabs || !text.contains(&0x09));
    let munged = tw::munge(text, o.expand_tabs, o.tabsize, o.replace_whitespace);
    let mut chunks = if o.boh_is_true { tw::split_words(&munged, &cls) } else { tw::split_simple(&munged) };
    if o.fix_sentence_endings {
        tw::fix_sentence_endings(&munged, &mut chunks);
    }
    let one_chunk = chunks.len() == 1 && chunks[0] == Chunk::Text(0, munged.len());
    let opts = WrapOpts {
        width: o.width,
        initial_indent: &o.initial_indent,
        subsequent_indent: &o.subsequent_indent,
        break_long_words: o.break_long_words,
        drop_whitespace: o.drop_whitespace,
        break_on_hyphens: o.boh_truthy,
        max_lines: o.max_lines,
        placeholder: &o.placeholder,
    };
    let lines = match tw::wrap_chunks(&munged, chunks, &opts, &cls) {
        Ok(l) => l,
        Err(WrapError::InvalidWidth | WrapError::PlaceholderTooLarge | WrapError::NoProgress) => return None,
    };
    *whole = munged_is_input
        && one_chunk
        && o.initial_indent.is_empty()
        && lines.len() == 1
        && lines[0].as_slice() == munged.as_slice();
    Some(lines)
}

/// `list` of `str` lines (`whole`: `[text]`).
unsafe fn lines_list(lines: &[Vec<u32>], text: O, whole: bool) -> O {
    // SAFETY: new list filled completely.
    unsafe {
        let out = ffi::PyList_New(lines.len() as ffi::Py_ssize_t);
        if out.is_null() {
            return out;
        }
        for (i, l) in lines.iter().enumerate() {
            let s = if whole { ffi::Py_NewRef(text) } else { new_str(l) };
            if s.is_null() {
                ffi::Py_DECREF(out);
                return s;
            }
            ffi::PyList_SET_ITEM(out, i as ffi::Py_ssize_t, s);
        }
        out
    }
}

/// `"\n".join(lines)` (`whole`: `text`).
unsafe fn lines_joined(lines: &[Vec<u32>], text: O, whole: bool) -> O {
    // SAFETY: plain constructors.
    unsafe {
        if whole {
            return ffi::Py_NewRef(text);
        }
        let mut buf: Vec<u32> = Vec::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                buf.push(0x0a);
            }
            buf.extend_from_slice(l);
        }
        new_str(&buf)
    }
}

// ---------------------------------------------------------------------------------------
// Statics (main-interpreter objects, strong references kept)
// ---------------------------------------------------------------------------------------

static TW_CLS: AtomicPtr<ffi::PyObject> = null!();
static INIT_DEFAULTS: AtomicPtr<ffi::PyObject> = null!();
static INIT_KWDEFAULTS: AtomicPtr<ffi::PyObject> = null!();
static INIT_FUNC: AtomicPtr<ffi::PyObject> = null!();
static TRANS: AtomicPtr<ffi::PyObject> = null!();
static S_TEXT: AtomicPtr<ffi::PyObject> = null!();
static S_PREFIX: AtomicPtr<ffi::PyObject> = null!();
static S_PREDICATE: AtomicPtr<ffi::PyObject> = null!();
static S_OPTS: [AtomicPtr<ffi::PyObject>; 12] = [const { null!() }; 12];
/// Method / class attribute names the algorithm resolves on `self`, and their originals.
const METHOD_NAMES: [&str; 11] = [
    "wrap",
    "_munge_whitespace",
    "_split",
    "_split_chunks",
    "_fix_sentence_endings",
    "_handle_long_word",
    "_wrap_chunks",
    "wordsep_re",
    "wordsep_simple_re",
    "sentence_end_re",
    "unicode_whitespace_trans",
];
static S_METHODS: [AtomicPtr<ffi::PyObject>; 11] = [const { null!() }; 11];
static METHODS: [AtomicPtr<ffi::PyObject>; 11] = [const { null!() }; 11];

#[inline(always)]
fn ld(p: &AtomicPtr<ffi::PyObject>) -> O {
    p.load(R)
}

/// `TextWrapper.__init__` still has the defaults validated at install, and the whitespace
/// translation table still maps exactly `_whitespace` to spaces.
#[inline]
unsafe fn class_state_ok() -> bool {
    // SAFETY: kept references.
    unsafe {
        let init = ld(&INIT_FUNC).cast::<ffi::PyFunctionObject>();
        if (*init).func_defaults != ld(&INIT_DEFAULTS) || (*init).func_kwdefaults != ld(&INIT_KWDEFAULTS) {
            return false;
        }
        let t = ld(&TRANS);
        if ffi::PyDict_Size(t) != 6 {
            return false;
        }
        for k in [9i64, 10, 11, 12, 13, 32] {
            let key = ffi::PyLong_FromLongLong(k); // small ints: cached, no allocation
            let mut v: O = std::ptr::null_mut();
            let r = ffi::PyDict_GetItemRef(t, key, &mut v);
            ffi::Py_DECREF(key);
            if r != 1 {
                ffi::PyErr_Clear();
                return false;
            }
            let ok = is_type(v, &raw mut ffi::PyLong_Type) && int_value(v) == Some(32);
            ffi::Py_DECREF(v);
            if !ok {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------------------
// Module functions: wrap / fill / shorten
// ---------------------------------------------------------------------------------------

static G_WRAP: Guard = Guard::new();
static G_FILL: Guard = Guard::new();
static G_SHORTEN: Guard = Guard::new();
static G_DEDENT: Guard = Guard::new();
static G_INDENT: Guard = Guard::new();
static G_TW_WRAP: Guard = Guard::new();
static G_TW_FILL: Guard = Guard::new();

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Wrap,
    Fill,
    Shorten,
}

/// Parse `(text, width, **kwargs)`; `None`: outside the subset (or CPython raises).
unsafe fn parse_module_call(args: *const O, nargsf: usize, kw: O, kind: Kind) -> Option<(O, Opts)> {
    // SAFETY: vectorcall arguments.
    unsafe {
        let npos = nargs(nargsf);
        if npos > 2 {
            return None;
        }
        let mut text = if npos >= 1 { *args } else { std::ptr::null_mut() };
        let mut width = if npos >= 2 { *args.add(1) } else { std::ptr::null_mut() };
        let mut o = Opts::defaults();
        let mut seen = [false; 12];
        if !kw.is_null() {
            for i in 0..ffi::PyTuple_GET_SIZE(kw) as usize {
                let k = ffi::PyTuple_GET_ITEM(kw, i as ffi::Py_ssize_t);
                let v = *args.add(npos + i);
                if k == ld(&S_TEXT) || ffi::PyUnicode_Compare(k, ld(&S_TEXT)) == 0 {
                    if !text.is_null() {
                        return None;
                    }
                    text = v;
                    continue;
                }
                let j = (0..12).find(|&j| k == ld(&S_OPTS[j]) || ffi::PyUnicode_Compare(k, ld(&S_OPTS[j])) == 0)?;
                if j == 0 {
                    if !width.is_null() {
                        return None;
                    }
                    width = v;
                    continue;
                }
                if seen[j] || (kind == Kind::Shorten && j == 10) {
                    return None; // duplicate / `max_lines` given twice to TextWrapper
                }
                seen[j] = true;
                o.set(j, v)?;
            }
        }
        if text.is_null() || !exact_str(text) {
            return None;
        }
        match (width.is_null(), kind) {
            (true, Kind::Shorten) => return None, // missing argument
            (true, _) => {}
            (false, _) => o.width = exact_int(width)?,
        }
        if kind == Kind::Shorten {
            o.max_lines = Some(1);
        }
        Some((text, o))
    }
}

/// `' '.join(text.strip().split())` (`None`: it is `text` itself — no whitespace at all).
fn collapse(text: &[u32]) -> Option<Vec<u32>> {
    let cls = Cls;
    if !text.iter().any(|&c| cls.is_space(c)) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len());
    for w in text.split(|&c| cls.is_space(c)).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(0x20);
        }
        out.extend_from_slice(w);
    }
    Some(out)
}

unsafe fn module_kernel(g: &Guard, kind: Kind, func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        if let Some((text, o)) = parse_module_call(args, nargsf, kw, kind) {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if class_state_ok()
                && let Some(t) = ucs4(text)
            {
                let (src, is_input) = match kind {
                    Kind::Shorten => match collapse(&t) {
                        Some(c) => (c, false),
                        None => (t, true),
                    },
                    _ => (t, true),
                };
                let mut whole = false;
                if let Some(lines) = wrap_lines(&src, &o, &mut whole) {
                    let whole = whole && is_input;
                    let r = match kind {
                        Kind::Wrap => lines_list(&lines, text, whole),
                        _ => lines_joined(&lines, text, whole),
                    };
                    if g.counters.hit_check_streak() {
                        return verify(g, kind_spec(kind), r, func, args, nargsf, kw);
                    }
                    return r;
                }
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

fn kind_spec(kind: Kind) -> &'static TargetSpec {
    match kind {
        Kind::Wrap => &WRAP,
        Kind::Fill => &FILL,
        Kind::Shorten => &SHORTEN,
    }
}

/// Check mode: compare with the original (`accel::check`).
unsafe fn verify(_g: &Guard, spec: &'static TargetSpec, r: O, func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: the original with the unchanged arguments.
    unsafe {
        let n = nargs(nargsf) + if kw.is_null() { 0 } else { ffi::PyTuple_GET_SIZE(kw) as usize };
        let argv: Vec<O> = (0..n).map(|i| *args.add(i)).collect();
        accel::check::verify(spec, r, &argv, || call_original(func, args, nargsf, kw))
    }
}

unsafe extern "C" fn vc_wrap(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { module_kernel(&G_WRAP, Kind::Wrap, func, args, nargsf, kw) }
}

unsafe extern "C" fn vc_fill(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { module_kernel(&G_FILL, Kind::Fill, func, args, nargsf, kw) }
}

unsafe extern "C" fn vc_shorten(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { module_kernel(&G_SHORTEN, Kind::Shorten, func, args, nargsf, kw) }
}

// ---------------------------------------------------------------------------------------
// TextWrapper.wrap / fill
// ---------------------------------------------------------------------------------------

static CACHE: TypeCache = TypeCache::new();

/// Is `tp`'s attribute resolution the one the algorithm assumes: generic `getattr`, every
/// method and class attribute CPython's own, and no class attribute named like an option
/// (so reading the options is a plain instance-dict read)? Cached per type version.
/// Returns the shared keys (and their entries) when they lack the method names.
unsafe fn tw_class(tp: *mut ffi::PyTypeObject) -> Option<(usize, usize)> {
    // SAFETY: attached thread; borrowed results are only compared.
    unsafe {
        let ver = version_tag(tp);
        if ver != 0
            && let Some((info, keys, n)) = CACHE.get(tp as usize, ver)
        {
            return (info == 1).then_some((keys, n));
        }
        if !generic_getattro(tp) {
            return None;
        }
        let _ = _PyType_Lookup(tp, ld(&S_METHODS[0]));
        let v0 = version_tag(tp);
        let mut ok = (0..METHOD_NAMES.len()).all(|i| _PyType_Lookup(tp, ld(&S_METHODS[i])) == ld(&METHODS[i]));
        ok &= (0..12).all(|i| _PyType_Lookup(tp, ld(&S_OPTS[i])).is_null());
        let (mut keys, mut n) = (0usize, 0usize);
        let k = INST.shared_keys(tp);
        if ok && !k.is_null() {
            let n0 = INST.nentries(k);
            if (0..METHOD_NAMES.len()).all(|i| INST.absent(k, ld(&S_METHODS[i]))) {
                keys = k as usize;
                n = n0;
            }
        }
        if v0 != 0 && version_tag(tp) == v0 {
            CACHE.put(tp as usize, v0, u32::from(ok), keys, n);
        }
        ok.then_some((keys, n))
    }
}

/// The options of a `TextWrapper` instance (`None`: outside the subset).
unsafe fn instance_opts(slf: O) -> Option<Opts> {
    // SAFETY: attached thread; the class has no attribute of these names (checked), so
    // generic getattr reads the instance dict only.
    unsafe {
        let (keys, n) = tw_class(ffi::Py_TYPE(slf))?;
        if !INST.clean(slf, keys, n) {
            for i in 0..METHOD_NAMES.len() {
                let m = ld(&METHODS[i]);
                let shadowed = match INST.has_attr(slf, ld(&S_METHODS[i])) {
                    Some(b) => b,
                    None if is_type(m, &raw mut ffi::PyFunction_Type) => !unshadowed(slf, ld(&S_METHODS[i]), m),
                    None => {
                        // A class attribute that is not a method (patterns, the table):
                        // look in the instance dict directly.
                        let d = ffi::PyObject_GenericGetDict(slf, std::ptr::null_mut());
                        if d.is_null() {
                            ffi::PyErr_Clear();
                            return None;
                        }
                        let r = ffi::PyDict_Contains(d, ld(&S_METHODS[i]));
                        ffi::Py_DECREF(d);
                        r != 0
                    }
                };
                if shadowed {
                    return None;
                }
            }
        }
        let mut o = Opts::defaults();
        for i in 0..12 {
            let v = ffi::PyObject_GenericGetAttr(slf, ld(&S_OPTS[i]));
            if v.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            let ok = o.set(i, v);
            ffi::Py_DECREF(v);
            ok?;
        }
        Some(o)
    }
}

unsafe fn method_kernel(g: &Guard, fill: bool, func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        if kw.is_null() && nargs(nargsf) == 2 {
            let (slf, text) = (*args, *args.add(1));
            if exact_str(text) {
                if !g.ready(func) {
                    return call_original(func, args, nargsf, kw);
                }
                if class_state_ok()
                    && let Some(o) = instance_opts(slf)
                    && let Some(t) = ucs4(text)
                {
                    let mut whole = false;
                    if let Some(lines) = wrap_lines(&t, &o, &mut whole) {
                        let r = if fill { lines_joined(&lines, text, whole) } else { lines_list(&lines, text, whole) };
                        if g.counters.hit_check_streak() {
                            return verify(g, if fill { &TW_FILL } else { &TW_WRAP }, r, func, args, nargsf, kw);
                        }
                        return r;
                    }
                }
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

unsafe extern "C" fn vc_tw_wrap(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { method_kernel(&G_TW_WRAP, false, func, args, nargsf, kw) }
}

unsafe extern "C" fn vc_tw_fill(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { method_kernel(&G_TW_FILL, true, func, args, nargsf, kw) }
}

// ---------------------------------------------------------------------------------------
// dedent / indent
// ---------------------------------------------------------------------------------------

unsafe extern "C" fn vc_dedent(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_DEDENT;
        let text = match (nargs(nargsf), kw.is_null()) {
            (1, true) => *args,
            (0, false) if ffi::PyTuple_GET_SIZE(kw) == 1 && ffi::PyUnicode_Compare(ffi::PyTuple_GET_ITEM(kw, 0), ld(&S_TEXT)) == 0 => *args,
            _ => std::ptr::null_mut(),
        };
        if !text.is_null() && exact_str(text) {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if let Some(t) = ucs4(text) {
                let r = match tw::dedent(&t, &Cls) {
                    None => ffi::Py_NewRef(text),
                    Some(v) => new_str(&v),
                };
                if g.counters.hit_check_streak() {
                    return verify(g, &DEDENT, r, func, args, nargsf, kw);
                }
                return r;
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

unsafe extern "C" fn vc_indent(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_INDENT;
        let names = [ld(&S_TEXT), ld(&S_PREFIX), ld(&S_PREDICATE)];
        if let Some([text, prefix, pred]) = crate::targets::utils_kit::bind(args, nargs(nargsf), kw, names)
            && !text.is_null()
            && !prefix.is_null()
            && (pred.is_null() || pred == ffi::Py_None())
            && exact_str(text)
            && exact_str(prefix)
        {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if let (Some(t), Some(p)) = (ucs4(text), ucs4(prefix)) {
                let r = match tw::indent(&t, &p, &Cls) {
                    None => ffi::Py_NewRef(text),
                    Some(v) => new_str(&v),
                };
                if g.counters.hit_check_streak() {
                    return verify(g, &INDENT, r, func, args, nargsf, kw);
                }
                return r;
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

// ---------------------------------------------------------------------------------------
// Preparation
// ---------------------------------------------------------------------------------------

/// Statics shared by the group (idempotent per `TextWrapper` class).
fn prepare_common<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<Bound<'py, PyType>, Refusal> {
    for (cell, name) in [
        (&IS_ALPHA, c"_PyUnicode_IsAlpha"),
        (&IS_DECIMAL, c"_PyUnicode_IsDecimalDigit"),
        (&IS_DIGIT, c"_PyUnicode_IsDigit"),
        (&IS_NUMERIC, c"_PyUnicode_IsNumeric"),
        (&IS_SPACE, c"_PyUnicode_IsWhitespace"),
    ] {
        cx.symbol(name, cell)?;
    }
    crate::targets::utils_kit::prepare_symbols(cx)?;
    INST.calibrate(py)?;
    let cls = cx.module_attr("TextWrapper")?.cast_into::<PyType>().map_err(PyErr::from)?;
    if cls.as_ptr() == ld(&TW_CLS) {
        return Ok(cls);
    }
    if S_TEXT.load(R).is_null() {
        S_TEXT.store(interned(py, "text"), R);
        S_PREFIX.store(interned(py, "prefix"), R);
        S_PREDICATE.store(interned(py, "predicate"), R);
        for (i, n) in OPT_NAMES.iter().enumerate() {
            S_OPTS[i].store(interned(py, n), R);
        }
        for (i, n) in METHOD_NAMES.iter().enumerate() {
            S_METHODS[i].store(interned(py, n), R);
        }
    }
    for (i, n) in METHOD_NAMES.iter().enumerate() {
        keep(&METHODS[i], &cx.type_attr(&cls, n)?);
    }
    let init = cx.type_attr(&cls, "__init__")?;
    let defaults = init.getattr("__defaults__")?;
    let kwdefaults = init.getattr("__kwdefaults__")?;
    let want = py.eval(c"(70, '', '', True, True, False, True, True, True, 8)", None, None)?;
    let want_kw = py.eval(c"{'max_lines': None, 'placeholder': ' [...]'}", None, None)?;
    if !defaults.eq(&want)? || !kwdefaults.eq(&want_kw)? {
        return Err(Refusal::new("TextWrapper.__init__ defaults differ"));
    }
    // Every default must be of the exact type `Opts::defaults` assumes.
    let trans = cx.type_attr(&cls, "unicode_whitespace_trans")?;
    if !trans.is_exact_instance_of::<PyDict>() {
        return Err(Refusal::new("unicode_whitespace_trans is not a dict"));
    }
    keep(&INIT_FUNC, &init);
    keep(&INIT_DEFAULTS, &defaults);
    keep(&INIT_KWDEFAULTS, &kwdefaults);
    keep(&TRANS, &trans);
    // SAFETY: probe of the table installed above (no Python code runs).
    if unsafe { !class_state_ok() } {
        return Err(Refusal::new("unicode_whitespace_trans differs"));
    }
    cx.self_test("textwrap chunking", |py| self_test(py, &cls))?;
    keep(&TW_CLS, cls.as_any());
    Ok(cls)
}

/// The chunking of the two patterns on probe strings equals `re.split` (C regex engine;
/// no Python code runs: `Pattern.split` and `str` methods are C).
fn self_test(py: Python<'_>, cls: &Bound<'_, PyType>) -> PyResult<bool> {
    let wordsep = cls.getattr("wordsep_re")?;
    let simple = cls.getattr("wordsep_simple_re")?;
    for probe in [
        "Hello there -- you goof-ball, use the -b option!",
        "a-b-cd-ef --x x-- well--known ab-c 12-34 \u{e9}t\u{e9}-l\u{e0} x\u{2014}y \u{0663}\u{0664}-ab",
        " \t lead and  trail \u{a0}nbsp\u{a0} ",
    ] {
        let v: Vec<u32> = probe.chars().map(u32::from).collect();
        for (pat, chunks) in [(&wordsep, tw::split_words(&v, &Cls)), (&simple, tw::split_simple(&v))] {
            let want: Vec<String> = pat.call_method1("split", (probe,))?.extract()?;
            let want: Vec<String> = want.into_iter().filter(|s| !s.is_empty()).collect();
            let got: Vec<String> = chunks
                .iter()
                .map(|c| match *c {
                    Chunk::Text(a, b) => v[a..b].iter().filter_map(|&x| char::from_u32(x)).collect(),
                    Chunk::TwoSpaces => "  ".to_string(),
                })
                .collect();
            if got != want {
                return Ok(false);
            }
        }
    }
    let _ = py;
    Ok(true)
}

fn pin_class(cx: &mut Prepare<'_>, cls: &Bound<'_, PyType>, g: &'static Guard) -> Result<(), Refusal> {
    for n in METHOD_NAMES.iter().copied().chain(["fill", "__init__", "__new__"]) {
        cx.pin_attr(cls, n, &g.deps)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_fn<'py>(
    py: Python<'py>,
    cx: &mut Prepare<'py>,
    path: &[&str],
    fp: accel::Fingerprints,
    builtins: &[&str],
    class_pins: bool,
    g: &'static Guard,
    kernel: ffi::vectorcallfunc,
) -> Result<(), Refusal> {
    let cls = prepare_common(py, cx)?;
    let func = if path.len() == 2 { cx.type_attr(&cls, path[1])? } else { cx.module_attr(path[0])? };
    cx.fingerprint_code(&func, fp)?;
    let globals = cx.module()?.dict();
    pin_globals(cx, &globals, builtins, &g.deps)?;
    if class_pins {
        cx.pin(&globals, "TextWrapper", &g.deps)?;
        pin_class(cx, &cls, g)?;
    }
    g.capture(&func);
    cx.vectorcall(&func, kernel, &accel::FUNCTION_VECTORCALL)
}

macro_rules! target {
    ($spec:ident, $prep:ident, $name:literal, [$($path:literal),+], $kernel:ident, $guard:ident, $fp:expr,
     builtins: [$($bl:literal),*], class_pins: $cp:expr) => {
        fn $prep<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
            prepare_fn(py, cx, &[$($path),+], $fp, &[$($bl),*], $cp, &$guard, $kernel)
        }
        pub static $spec: TargetSpec = TargetSpec {
            name: $name,
            group: "textwrap",
            module: Some("textwrap"),
            versions: &[(3, 14), (3, 15)],
            free_threaded: true,
            counters: &$guard.counters,
            prepare: $prep,
            checkable: true,
            ..TargetSpec::DEFAULT
        };
    };
}

target!(WRAP, prep_wrap, "textwrap.wrap", ["wrap"], vc_wrap, G_WRAP,
    &[(3, 14, "3693405a8fb53e5c"), (3, 15, "a2632ba2df2a23a3")], builtins: [], class_pins: true);
target!(FILL, prep_fill, "textwrap.fill", ["fill"], vc_fill, G_FILL,
    &[(3, 14, "2a8243c0a41ca2ad"), (3, 15, "0236093725ea15dc")], builtins: [], class_pins: true);
target!(SHORTEN, prep_shorten, "textwrap.shorten", ["shorten"], vc_shorten, G_SHORTEN,
    &[(3, 14, "1e331e723a975c7a"), (3, 15, "37134789c5932809")], builtins: [], class_pins: true);
target!(DEDENT, prep_dedent, "textwrap.dedent", ["dedent"], vc_dedent, G_DEDENT,
    &[(3, 14, "893f855ff5f348e9"), (3, 15, "e7dee1126e679cdc")],
    builtins: ["min", "max", "enumerate", "AttributeError", "TypeError", "type"], class_pins: false);
target!(INDENT, prep_indent, "textwrap.indent", ["indent"], vc_indent, G_INDENT,
    &[(3, 14, "f900977c89bc767a"), (3, 15, "5b8ede3946f79b26")], builtins: [], class_pins: false);
target!(TW_WRAP, prep_tw_wrap, "textwrap.TextWrapper.wrap", ["TextWrapper", "wrap"], vc_tw_wrap, G_TW_WRAP,
    &[(3, 14, "306c46f71fa226ab"), (3, 15, "5da373846fc66b9a")], builtins: [], class_pins: true);
target!(TW_FILL, prep_tw_fill, "textwrap.TextWrapper.fill", ["TextWrapper", "fill"], vc_tw_fill, G_TW_FILL,
    &[(3, 14, "f4a79c3b7712c1b4"), (3, 15, "0fcd68ac19533157")], builtins: [], class_pins: true);

pub static TARGETS: &[&TargetSpec] = &[&WRAP, &FILL, &SHORTEN, &DEDENT, &INDENT, &TW_WRAP, &TW_FILL];
