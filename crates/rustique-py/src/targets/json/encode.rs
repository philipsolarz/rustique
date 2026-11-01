//! The JSON encoder for the exact subset: what `_json.c`'s `encoder_call` writes for a tree
//! of builtin objects, without running any Python code.
//!
//! Covered: `None`/`True`/`False`, `str` (and subclasses: the C encoder reads their buffer
//! directly), `int` (and subclasses: `PyLong_Type.tp_repr`), `float` (and subclasses:
//! `PyFloat_Type.tp_repr`, `NaN`/`Infinity` with `allow_nan`), exact `list`/`tuple`, exact
//! `dict` with keys converted like `encoder_encode_key_value`, `indent`, separators,
//! `sort_keys` (all-`str` keys), `skipkeys`, `check_circular`, `ensure_ascii`.
//!
//! Anything else returns [`Stop`] before anything observable happened: an object that would
//! need `default()` (or `PyMapping_Items`/`PySequence_Fast` on a subclass), a circular
//! reference, `NaN` with `allow_nan=False`, an `int` beyond `sys.set_int_max_str_digits`,
//! mixed-type keys with `sort_keys`, the C recursion limit. The caller then runs CPython's
//! original, which produces the same output or raises the same error.

use pyo3::ffi;

use rustique_core::float_repr;
use rustique_core::json::escape::{Text, push_ascii_quoted};
use rustique_core::json::itoa;

use super::pystr::{View, view};

/// Not in the exact subset (nothing observable happened; run the original).
#[derive(Debug)]
pub(crate) struct Stop;

type R = Result<(), Stop>;

/// The indent of one encoding: `' ' * n` for an integer `indent`, or the given string.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Indent {
    Spaces(usize),
    Str(View),
}

/// Options of one encoding (the `c_make_encoder` arguments).
pub(crate) struct Opts {
    pub ensure_ascii: bool,
    pub allow_nan: bool,
    pub check_circular: bool,
    pub sort_keys: bool,
    pub skipkeys: bool,
    /// `None` for the compact form.
    pub indent: Option<Indent>,
    /// `None`: the defaults `", "` / `": "` (or `","` with an indent).
    pub item_sep: Option<View>,
    pub key_sep: Option<View>,
}

impl Opts {
    pub const DEFAULT: Opts = Opts {
        ensure_ascii: true,
        allow_nan: true,
        check_circular: true,
        sort_keys: false,
        skipkeys: false,
        indent: None,
        item_sep: None,
        key_sep: None,
    };
}

/// Where the text goes: ASCII bytes under `ensure_ascii`, any width otherwise.
pub(crate) trait Sink {
    fn ascii(&mut self, s: &[u8]);
    /// Quoted and escaped string value.
    ///
    /// # Safety
    /// The viewed string is alive.
    unsafe fn quoted(&mut self, v: &View);
    /// A separator or indent written as is.
    ///
    /// # Safety
    /// The viewed string is alive; under `ensure_ascii` it is ASCII (checked by the caller).
    unsafe fn raw(&mut self, v: &View);
}

impl Sink for Vec<u8> {
    #[inline(always)]
    fn ascii(&mut self, s: &[u8]) {
        self.extend_from_slice(s);
    }
    #[inline(always)]
    unsafe fn quoted(&mut self, v: &View) {
        // SAFETY: per the trait.
        unsafe {
            match v.kind {
                1 => push_ascii_quoted(self, v.u8s()),
                2 => push_ascii_quoted(self, v.u16s()),
                _ => push_ascii_quoted(self, v.u32s()),
            }
        }
    }
    #[inline(always)]
    unsafe fn raw(&mut self, v: &View) {
        // SAFETY: per the trait (ASCII, so kind 1).
        unsafe { self.extend_from_slice(v.u8s()) }
    }
}

impl Sink for Text {
    #[inline(always)]
    fn ascii(&mut self, s: &[u8]) {
        self.push_ascii(s);
    }
    #[inline(always)]
    unsafe fn quoted(&mut self, v: &View) {
        // SAFETY: per the trait.
        unsafe {
            match v.kind {
                1 => self.push_unicode_quoted(v.u8s(), v.max()),
                2 => self.push_unicode_quoted(v.u16s(), v.max()),
                _ => self.push_unicode_quoted(v.u32s(), v.max()),
            }
        }
    }
    #[inline(always)]
    unsafe fn raw(&mut self, v: &View) {
        // SAFETY: per the trait.
        unsafe {
            match v.kind {
                1 => self.push_units(v.u8s(), v.max()),
                2 => self.push_units(v.u16s(), v.max()),
                _ => self.push_units(v.u32s(), v.max()),
            }
        }
    }
}

/// One encoding in progress.
pub(crate) struct Enc<'a, S: Sink> {
    pub out: &'a mut S,
    pub opts: &'a Opts,
    /// Containers being encoded (the `markers` of the C encoder, for the circular check).
    stack: Markers,
}

/// A stack of container pointers: inline for the usual nesting depths.
struct Markers {
    inline: [*mut ffi::PyObject; 16],
    n: usize,
    spill: Vec<*mut ffi::PyObject>,
}

impl Markers {
    #[inline(always)]
    fn contains(&self, o: *mut ffi::PyObject) -> bool {
        self.inline[..self.n.min(16)].contains(&o) || (self.n > 16 && self.spill.contains(&o))
    }
    #[inline(always)]
    fn push(&mut self, o: *mut ffi::PyObject) {
        if self.n < 16 {
            self.inline[self.n] = o;
        } else {
            self.spill.push(o);
        }
        self.n += 1;
    }
    #[inline(always)]
    fn pop(&mut self) {
        self.n -= 1;
        if self.n >= 16 {
            self.spill.pop();
        }
    }
}

#[inline(always)]
fn ty(o: *mut ffi::PyObject) -> *mut ffi::PyTypeObject {
    // SAFETY: live object.
    unsafe { ffi::Py_TYPE(o) }
}

/// Leaves `Py_EnterRecursiveCall` on drop.
struct Depth;

impl Drop for Depth {
    #[inline(always)]
    fn drop(&mut self) {
        // SAFETY: paired with a successful `Py_EnterRecursiveCall`.
        unsafe { ffi::Py_LeaveRecursiveCall() }
    }
}

/// `_Py_EnterRecursiveCall(" while encoding a JSON object")`: where CPython would raise
/// RecursionError, stop (the original then raises it).
#[inline(always)]
fn enter() -> Result<Depth, Stop> {
    // SAFETY: attached thread.
    unsafe {
        if ffi::Py_EnterRecursiveCall(c" while encoding a JSON object".as_ptr()) != 0 {
            ffi::PyErr_Clear();
            return Err(Stop);
        }
    }
    Ok(Depth)
}

/// Holds an object's critical section on free-threaded builds (what the C encoder's
/// `Py_BEGIN_CRITICAL_SECTION(_SEQUENCE_FAST)` does).
struct Locked {
    #[cfg(Py_GIL_DISABLED)]
    cs: ffi::PyCriticalSection,
}

impl Locked {
    #[inline(always)]
    #[allow(unused_variables)]
    fn new(o: *mut ffi::PyObject) -> Box<Self> {
        #[cfg(Py_GIL_DISABLED)]
        {
            // SAFETY: zeroed storage is what CPython's macro starts from; Begin initializes it.
            let mut b = Box::new(Locked { cs: unsafe { std::mem::zeroed() } });
            // SAFETY: live object; the section is ended on drop, in LIFO order.
            unsafe { ffi::PyCriticalSection_Begin(&mut b.cs, o) };
            b
        }
        #[cfg(not(Py_GIL_DISABLED))]
        {
            Box::new(Locked {})
        }
    }
}

#[cfg(Py_GIL_DISABLED)]
impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: begun in `new`.
        unsafe { ffi::PyCriticalSection_End(&mut self.cs) }
    }
}

/// A strong reference released on drop (items are held while encoded, like the C encoder,
/// so a concurrent mutation on a free-threaded build cannot free them).
struct Held(*mut ffi::PyObject);

impl Held {
    #[inline(always)]
    fn new(o: *mut ffi::PyObject) -> Self {
        // SAFETY: live object.
        unsafe { ffi::Py_INCREF(o) };
        Held(o)
    }
}

impl Drop for Held {
    #[inline(always)]
    fn drop(&mut self) {
        // SAFETY: the reference taken in `new`.
        unsafe { ffi::Py_DECREF(self.0) }
    }
}

impl<'a, S: Sink> Enc<'a, S> {
    pub fn new(out: &'a mut S, opts: &'a Opts) -> Self {
        Enc { out, opts, stack: Markers { inline: [std::ptr::null_mut(); 16], n: 0, spill: Vec::new() } }
    }

    #[inline(always)]
    fn item_sep(&mut self) {
        match self.opts.item_sep {
            // SAFETY: separators are kept alive by the caller.
            Some(v) => unsafe { self.out.raw(&v) },
            None if self.opts.indent.is_some() => self.out.ascii(b","),
            None => self.out.ascii(b", "),
        }
    }

    #[inline(always)]
    fn key_sep(&mut self) {
        match self.opts.key_sep {
            // SAFETY: separators are kept alive by the caller.
            Some(v) => unsafe { self.out.raw(&v) },
            None => self.out.ascii(b": "),
        }
    }

    #[inline(always)]
    fn newline_indent(&mut self, level: usize) {
        match self.opts.indent {
            None => {}
            Some(Indent::Spaces(n)) => {
                self.out.ascii(b"\n");
                const SPACES: &[u8; 64] = &[b' '; 64];
                let mut k = n * level;
                while k > 0 {
                    let c = k.min(64);
                    self.out.ascii(&SPACES[..c]);
                    k -= c;
                }
            }
            Some(Indent::Str(ind)) => {
                self.out.ascii(b"\n");
                for _ in 0..level {
                    // SAFETY: the indent string is kept alive by the caller.
                    unsafe { self.out.raw(&ind) };
                }
            }
        }
    }

    /// Circular-reference check (`markers`): a container already being encoded stops.
    #[inline(always)]
    fn push_marker(&mut self, o: *mut ffi::PyObject) -> R {
        if self.opts.check_circular {
            if self.stack.contains(o) {
                return Err(Stop);
            }
            self.stack.push(o);
        }
        Ok(())
    }

    #[inline(always)]
    fn pop_marker(&mut self) {
        if self.opts.check_circular {
            self.stack.pop();
        }
    }

    /// `encoder_encode_float` for a finite or non-finite value.
    #[inline(always)]
    fn float(&mut self, x: f64) -> R {
        if x.is_finite() {
            let mut buf = [0u8; 32];
            let n = float_repr::write_finite(x, &mut buf);
            self.out.ascii(&buf[..n]);
        } else if !self.opts.allow_nan {
            return Err(Stop);
        } else if x > 0.0 {
            self.out.ascii(b"Infinity");
        } else if x < 0.0 {
            self.out.ascii(b"-Infinity");
        } else {
            self.out.ascii(b"NaN");
        }
        Ok(())
    }

    /// `int.__repr__` of an int (or subclass), written to the sink.
    #[inline(always)]
    fn int(&mut self, o: *mut ffi::PyObject) -> R {
        // SAFETY: `o` is an int instance; attached thread.
        unsafe {
            let mut overflow = 0;
            let v = ffi::PyLong_AsLongLongAndOverflow(o, &mut overflow);
            if overflow == 0 {
                if v == -1 && !ffi::PyErr_Occurred().is_null() {
                    ffi::PyErr_Clear();
                    return Err(Stop);
                }
                let mut b = [0u8; 20];
                self.out.ascii(itoa::format_i64(&mut b, v));
                return Ok(());
            }
            // Beyond 64 bits: CPython's own conversion (`int_max_str_digits` may raise).
            let s = (ffi::PyLong_Type.tp_repr.ok_or(Stop)?)(o);
            if s.is_null() {
                ffi::PyErr_Clear();
                return Err(Stop);
            }
            self.out.ascii(view(s).u8s());
            ffi::Py_DECREF(s);
            Ok(())
        }
    }

    /// `JSONEncoder.encode(o)`: a `str` goes through `encode_basestring(_ascii)`, anything
    /// else through the C encoder at indent level 0 (same text for a `str`).
    pub fn top(&mut self, o: *mut ffi::PyObject) -> R {
        self.obj(o, 0)
    }

    /// `encoder_listencode_obj`.
    pub fn obj(&mut self, o: *mut ffi::PyObject, level: usize) -> R {
        // SAFETY: `o` is a live object; type checks before every layout read.
        unsafe {
            let t = ty(o);
            if t == &raw mut ffi::PyUnicode_Type {
                self.out.quoted(&view(o));
            } else if t == &raw mut ffi::PyLong_Type {
                self.int(o)?;
            } else if t == &raw mut ffi::PyFloat_Type {
                self.float((*o.cast::<ffi::PyFloatObject>()).ob_fval)?;
            } else if t == &raw mut ffi::PyList_Type {
                self.list(o, level, true)?;
            } else if t == &raw mut ffi::PyDict_Type {
                self.dict(o, level)?;
            } else if o == ffi::Py_None() {
                self.out.ascii(b"null");
            } else if o == ffi::Py_True() {
                self.out.ascii(b"true");
            } else if o == ffi::Py_False() {
                self.out.ascii(b"false");
            } else if t == &raw mut ffi::PyTuple_Type {
                self.list(o, level, false)?;
            } else if ffi::PyUnicode_Check(o) != 0 {
                self.out.quoted(&view(o));
            } else if ffi::PyLong_Check(o) != 0 {
                self.int(o)?;
            } else if ffi::PyFloat_Check(o) != 0 {
                self.float((*o.cast::<ffi::PyFloatObject>()).ob_fval)?;
            } else {
                return Err(Stop);
            }
        }
        Ok(())
    }

    /// `encoder_listencode_list` for an exact list or tuple.
    fn list(&mut self, o: *mut ffi::PyObject, level: usize, is_list: bool) -> R {
        // SAFETY: exact list/tuple; sizes re-read on every step (items are held while
        // encoded, the list's critical section is held on free-threaded builds).
        unsafe {
            if ffi::Py_SIZE(o) == 0 {
                self.out.ascii(b"[]");
                return Ok(());
            }
            let _depth = enter()?;
            self.push_marker(o)?;
            let _lock = if is_list { Some(Locked::new(o)) } else { None };
            self.out.ascii(b"[");
            let level = level + 1;
            let indented = self.opts.indent.is_some();
            if indented {
                self.newline_indent(level);
            }
            let mut i = 0;
            while i < ffi::Py_SIZE(o) {
                let item = if is_list { ffi::PyList_GET_ITEM(o, i) } else { ffi::PyTuple_GET_ITEM(o, i) };
                if i > 0 {
                    self.item_sep();
                    if indented {
                        self.newline_indent(level);
                    }
                }
                let held = Held::new(item);
                self.obj(held.0, level)?;
                i += 1;
            }
            self.pop_marker();
            if indented {
                self.newline_indent(level - 1);
            }
            self.out.ascii(b"]");
        }
        Ok(())
    }

    /// One `key: value` member (`encoder_encode_key_value`); `first` as in the C encoder.
    #[inline(always)]
    fn member(&mut self, k: *mut ffi::PyObject, v: *mut ffi::PyObject, level: usize, first: &mut bool) -> R {
        // SAFETY: live key and value (held by the caller); type checks before reads.
        unsafe {
            // Key kinds, in the C encoder's order of checks.
            enum K {
                Str,
                Float(f64),
                Const(&'static [u8]),
                Int,
            }
            let kind = if ffi::PyUnicode_Check(k) != 0 {
                K::Str
            } else if ffi::PyFloat_Check(k) != 0 {
                K::Float((*k.cast::<ffi::PyFloatObject>()).ob_fval)
            } else if k == ffi::Py_True() {
                K::Const(b"true")
            } else if k == ffi::Py_False() {
                K::Const(b"false")
            } else if k == ffi::Py_None() {
                K::Const(b"null")
            } else if ffi::PyLong_Check(k) != 0 {
                K::Int
            } else if self.opts.skipkeys {
                return Ok(());
            } else {
                return Err(Stop);
            };
            if let K::Float(x) = kind
                && !x.is_finite()
                && !self.opts.allow_nan
            {
                return Err(Stop);
            }
            if *first {
                *first = false;
            } else {
                self.item_sep();
            }
            self.newline_indent(level);
            match kind {
                K::Str => self.out.quoted(&view(k)),
                // Converted keys are ASCII and need no escaping: just quotes.
                K::Float(x) => {
                    self.out.ascii(b"\"");
                    self.float(x)?;
                    self.out.ascii(b"\"");
                }
                K::Const(c) => {
                    self.out.ascii(b"\"");
                    self.out.ascii(c);
                    self.out.ascii(b"\"");
                }
                K::Int => {
                    self.out.ascii(b"\"");
                    self.int(k)?;
                    self.out.ascii(b"\"");
                }
            }
            self.key_sep();
            self.obj(v, level)
        }
    }

    /// `encoder_listencode_dict` for an exact dict.
    fn dict(&mut self, o: *mut ffi::PyObject, level: usize) -> R {
        // SAFETY: exact dict; iterated under its critical section (free-threaded builds)
        // with keys and values held while encoded.
        unsafe {
            if ffi::PyDict_Size(o) == 0 {
                self.out.ascii(b"{}");
                return Ok(());
            }
            let _depth = enter()?;
            self.push_marker(o)?;
            self.out.ascii(b"{");
            let level = level + 1;
            let mut first = true;
            if self.opts.sort_keys {
                let mut items: Vec<(Held, Held)> = Vec::new();
                {
                    let _lock = Locked::new(o);
                    let (mut pos, mut k, mut v) = (0, std::ptr::null_mut(), std::ptr::null_mut());
                    while ffi::PyDict_Next(o, &mut pos, &mut k, &mut v) != 0 {
                        if ffi::Py_TYPE(k) != &raw mut ffi::PyUnicode_Type {
                            return Err(Stop);
                        }
                        items.push((Held::new(k), Held::new(v)));
                    }
                }
                items.sort_by(|a, b| str_cmp(&view(a.0.0), &view(b.0.0)));
                for (k, v) in &items {
                    self.member(k.0, v.0, level, &mut first)?;
                }
            } else {
                let _lock = Locked::new(o);
                let (mut pos, mut k, mut v) = (0, std::ptr::null_mut(), std::ptr::null_mut());
                while ffi::PyDict_Next(o, &mut pos, &mut k, &mut v) != 0 {
                    let (k, v) = (Held::new(k), Held::new(v));
                    self.member(k.0, v.0, level, &mut first)?;
                }
            }
            self.pop_marker();
            if !first {
                self.newline_indent(level - 1);
            }
            self.out.ascii(b"}");
        }
        Ok(())
    }
}

/// Code-point order of two strings (`str.__lt__`).
fn str_cmp(a: &View, b: &View) -> std::cmp::Ordering {
    // SAFETY: views of live strings (held by the caller).
    unsafe {
        if a.kind == b.kind && a.kind == 1 {
            return a.u8s().cmp(b.u8s());
        }
        let get = |v: &View, i: usize| -> u32 {
            match v.kind {
                1 => u32::from(*v.data.add(i)),
                2 => u32::from(*v.data.cast::<u16>().add(i)),
                _ => *v.data.cast::<u32>().add(i),
            }
        };
        for i in 0..a.len.min(b.len) {
            match get(a, i).cmp(&get(b, i)) {
                std::cmp::Ordering::Equal => {}
                o => return o,
            }
        }
        a.len.cmp(&b.len)
    }
}
