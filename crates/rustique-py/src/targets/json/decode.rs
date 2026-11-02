//! The JSON decoder for the exact subset: what `json.loads(s)` returns through
//! `JSONDecoder.decode` and `_json`'s `scan_once_unicode`, built with the same CPython
//! constructors in the same order (so lists and dicts grow, and strings are created,
//! exactly as CPython's do).
//!
//! Identical objects: `dict`s filled with `PyDict_SetItem` in document order (duplicate
//! keys: last value, first key object), `list`s grown with `PyList_Append`, key strings
//! memoized per call (repeated keys are one object, like the scanner's `memo`), string
//! values as `PyUnicode_Substring` (escape-free) or the writer's canonical string
//! (escaped), `int` through `PyLong_FromLongLong` / `PyLong_FromString`, `float`
//! correctly rounded (`float(s)`), `True`/`False`/`None` singletons.
//!
//! Anything CPython would raise for (syntax errors, control characters, a big int beyond
//! `int_max_str_digits`, the C recursion limit), and the `NaN`/`Infinity` literals (the
//! scanner's `parse_constant`), returns [`Stop`]: nothing observable happened, the
//! original re-runs the call and raises (or returns) CPython's own result.

use pyo3::ffi;

use rustique_core::json::Unit;
use rustique_core::json::scan::{self, Str};

use super::encode::Stop;
use super::pystr::{self, View, view};

type R<T> = Result<T, Stop>;

/// An owned reference released on drop unless taken.
struct Obj(*mut ffi::PyObject);

impl Obj {
    #[inline(always)]
    fn new(p: *mut ffi::PyObject) -> R<Obj> {
        if p.is_null() {
            // SAFETY: an exception is set (allocation failure): discard it, let CPython run.
            unsafe { ffi::PyErr_Clear() };
            return Err(Stop);
        }
        Ok(Obj(p))
    }
    #[inline(always)]
    fn take(self) -> *mut ffi::PyObject {
        let p = self.0;
        std::mem::forget(self);
        p
    }
}

impl Drop for Obj {
    #[inline(always)]
    fn drop(&mut self) {
        // SAFETY: an owned reference.
        unsafe { ffi::Py_DECREF(self.0) }
    }
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

#[inline(always)]
fn enter() -> R<Depth> {
    // SAFETY: attached thread.
    unsafe {
        if ffi::Py_EnterRecursiveCall(c" while decoding a JSON document".as_ptr()) != 0 {
            ffi::PyErr_Clear();
            return Err(Stop);
        }
    }
    Ok(Depth)
}

/// The per-call key memo: canonical key strings by content. Up to `SMALL` keys a linear
/// scan (length first); beyond, an open-addressing table on a content hash.
///
/// The memo owns a reference to every key it holds, like CPython's `memo` dict
/// (`PyDict_SetDefaultRef(memo, key, key)`): a key's only other owner can go away during
/// the parse — a repeated key in one object (`{"a": {"k": 1}, "a": 2}`) replaces the first
/// value, freeing its subtree and its keys. Released when the parse ends ([`Drop`]).
struct Memo {
    small: [(usize, *mut ffi::PyObject); SMALL],
    n: usize,
    /// (hash, key object); empty slots are null. Used once `n > SMALL`.
    slots: Vec<(u64, *mut ffi::PyObject)>,
}

const SMALL: usize = 24;

#[inline(always)]
fn mix(h: u64, c: u32) -> u64 {
    (h ^ u64::from(c)).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(23)
}

#[inline(always)]
fn hash_units<T: Unit>(units: &[T]) -> u64 {
    units.iter().fold(units.len() as u64, |h, u| mix(h, u.get()))
}

impl Memo {
    fn new() -> Self {
        Memo { small: [(0, std::ptr::null_mut()); SMALL], n: 0, slots: Vec::new() }
    }

    /// The memoized key equal to `units` (code points), if any.
    #[inline]
    fn find<T: Unit>(&self, units: &[T]) -> Option<*mut ffi::PyObject> {
        if self.n <= SMALL {
            for &(len, o) in &self.small[..self.n] {
                // SAFETY: memoized keys are live str objects (the memo owns a reference).
                if len == units.len() && unsafe { equal(&view(o), units) } {
                    return Some(o);
                }
            }
            return None;
        }
        let h = hash_units(units);
        let mask = self.slots.len() - 1;
        let mut i = h as usize & mask;
        loop {
            let (sh, o) = self.slots[i];
            if o.is_null() {
                return None;
            }
            // SAFETY: as above.
            if sh == h && unsafe { equal(&view(o), units) } {
                return Some(o);
            }
            i = (i + 1) & mask;
        }
    }

    /// Memoize a new key whose content is `units` (takes a new reference to `o`).
    fn insert<T: Unit>(&mut self, units: &[T], o: *mut ffi::PyObject) {
        // SAFETY: a live str object; attached thread.
        unsafe { ffi::Py_INCREF(o) };
        if self.n < SMALL {
            self.small[self.n] = (units.len(), o);
            self.n += 1;
            return;
        }
        if self.n == SMALL {
            // Switch to the table: re-hash the small entries from their objects.
            self.slots = vec![(0, std::ptr::null_mut()); 4 * SMALL];
            for k in 0..SMALL {
                let o = self.small[k].1;
                // SAFETY: a live key string.
                let v = unsafe { view(o) };
                // SAFETY: kind-matched slices of a live string.
                let h = unsafe {
                    match v.kind {
                        1 => hash_units(v.u8s()),
                        2 => hash_units(v.u16s()),
                        _ => hash_units(v.u32s()),
                    }
                };
                self.put(h, o);
            }
        }
        self.n += 1;
        if self.n * 2 > self.slots.len() {
            let old = std::mem::take(&mut self.slots);
            self.slots = vec![(0, std::ptr::null_mut()); old.len() * 2];
            for (h, o) in old {
                if !o.is_null() {
                    self.put(h, o);
                }
            }
        }
        self.put(hash_units(units), o);
    }

    fn put(&mut self, h: u64, o: *mut ffi::PyObject) {
        let mask = self.slots.len() - 1;
        let mut i = h as usize & mask;
        while !self.slots[i].1.is_null() {
            i = (i + 1) & mask;
        }
        self.slots[i] = (h, o);
    }
}

impl Drop for Memo {
    fn drop(&mut self) {
        // Every key once: up to `SMALL` keys they live in `small`; beyond, `slots` holds
        // all of them (the small entries were re-hashed into it).
        // SAFETY: references taken in `insert`; the parser runs attached (the kernel
        // drops the parser before returning to CPython).
        unsafe {
            if self.n <= SMALL {
                for &(_, o) in &self.small[..self.n] {
                    ffi::Py_DECREF(o);
                }
            } else {
                for &(_, o) in &self.slots {
                    if !o.is_null() {
                        ffi::Py_DECREF(o);
                    }
                }
            }
        }
    }
}

/// Does the string `v` hold exactly the code points `units`?
///
/// # Safety
/// `v` views a live string.
#[inline]
unsafe fn equal<T: Unit>(v: &View, units: &[T]) -> bool {
    if v.len != units.len() {
        return false;
    }
    // SAFETY: per the caller.
    unsafe {
        if T::MAX == 0xFF && v.kind == 1 {
            // SAFETY: `T` is `u8` (the only unit with MAX 0xFF): a byte comparison.
            let u = std::slice::from_raw_parts(units.as_ptr().cast::<u8>(), units.len());
            return v.u8s() == u;
        }
        match v.kind {
            1 => v.u8s().iter().zip(units).all(|(&a, b)| u32::from(a) == b.get()),
            2 => v.u16s().iter().zip(units).all(|(&a, b)| u32::from(a) == b.get()),
            _ => v.u32s().iter().zip(units).all(|(&a, b)| a == b.get()),
        }
    }
}

/// One `json.loads` of a decoded document.
pub(crate) struct Parser<'a, T: Unit> {
    s: &'a [T],
    /// The source string (for substrings of escape-free strings).
    src: View,
    memo: Memo,
    /// Decoded code points of an escaped string.
    buf: Vec<u32>,
}

impl<'a, T: Unit> Parser<'a, T> {
    /// # Safety
    /// `src` views a live string whose units are `s`.
    pub unsafe fn new(s: &'a [T], src: View) -> Self {
        Parser { s, src, memo: Memo::new(), buf: Vec::new() }
    }

    /// `JSONDecoder.decode`: whitespace, one value, whitespace, end of input.
    pub fn document(&mut self) -> R<*mut ffi::PyObject> {
        let i = scan::skip_ws(self.s, 0);
        let (v, end) = self.value(i)?;
        if scan::skip_ws(self.s, end) != self.s.len() {
            return Err(Stop); // Extra data
        }
        Ok(v.take())
    }

    /// The unit at `i` (callers check `i < len` first).
    #[inline(always)]
    fn at(&self, i: usize) -> u32 {
        debug_assert!(i < self.s.len());
        // SAFETY: every caller compares `i` with the length first.
        unsafe { self.s.get_unchecked(i).get() }
    }

    /// `scan_once_unicode` at `i`: the value and the index after it.
    fn value(&mut self, i: usize) -> R<(Obj, usize)> {
        let s = self.s;
        if i >= s.len() {
            return Err(Stop); // Expecting value
        }
        // SAFETY: attached thread; CPython constructors, checked for NULL.
        unsafe {
            match self.at(i) {
                0x22 => self.string(i + 1),
                0x7B => {
                    let _d = enter()?;
                    self.object(i + 1)
                }
                0x5B => {
                    let _d = enter()?;
                    self.array(i + 1)
                }
                0x6E if self.word(i, b"null") => Ok((Obj(ffi::Py_NewRef(ffi::Py_None())), i + 4)),
                0x74 if self.word(i, b"true") => Ok((Obj(ffi::Py_NewRef(ffi::Py_True())), i + 4)),
                0x66 if self.word(i, b"false") => Ok((Obj(ffi::Py_NewRef(ffi::Py_False())), i + 5)),
                // NaN / Infinity / -Infinity: the scanner's `parse_constant` (not covered).
                0x4E | 0x49 => Err(Stop),
                0x2D if self.word(i, b"-Infinity") => Err(Stop),
                _ => self.number(i),
            }
        }
    }

    /// `s[i..]` starts with `w` (CPython's bounds: the whole word must fit).
    #[inline(always)]
    fn word(&self, i: usize, w: &[u8]) -> bool {
        i + w.len() <= self.s.len() && w.iter().enumerate().all(|(k, &c)| self.at(i + k) == u32::from(c))
    }

    /// A string value starting after its quote.
    #[inline]
    fn string(&mut self, start: usize) -> R<(Obj, usize)> {
        self.buf.clear();
        // SAFETY: attached thread; the source string is alive for the parse.
        unsafe {
            match scan::string(self.s, start, &mut self.buf).map_err(|_| Stop)? {
                Str::Plain { end } => Ok((Obj::new(pystr::substring(&self.src, start, end))?, end + 1)),
                Str::Escaped { next, max } => Ok((Obj::new(pystr::from_code_points(&self.buf, max))?, next)),
            }
        }
    }

    /// A key string (memoized) starting after its quote.
    #[inline]
    fn key(&mut self, start: usize) -> R<(*mut ffi::PyObject, Obj, usize)> {
        self.buf.clear();
        // SAFETY: attached thread; the source string is alive for the parse.
        unsafe {
            match scan::string(self.s, start, &mut self.buf).map_err(|_| Stop)? {
                Str::Plain { end } => {
                    let units = &self.s[start..end];
                    if let Some(k) = self.memo.find(units) {
                        return Ok((k, Obj(ffi::Py_NewRef(k)), end + 1));
                    }
                    let k = Obj::new(pystr::substring(&self.src, start, end))?;
                    self.memo.insert(units, k.0);
                    Ok((k.0, k, end + 1))
                }
                Str::Escaped { next, max } => {
                    if let Some(k) = self.memo.find(&self.buf[..]) {
                        return Ok((k, Obj(ffi::Py_NewRef(k)), next));
                    }
                    let k = Obj::new(pystr::from_code_points(&self.buf, max))?;
                    self.memo.insert(&self.buf[..], k.0);
                    Ok((k.0, k, next))
                }
            }
        }
    }

    /// `_parse_object_unicode` (no hooks) after `{`.
    fn object(&mut self, mut i: usize) -> R<(Obj, usize)> {
        // SAFETY: attached thread; a fresh dict nobody else sees.
        unsafe {
            let d = Obj::new(ffi::PyDict_New())?;
            i = scan::skip_ws(self.s, i);
            if i < self.s.len() && self.at(i) == 0x7D {
                return Ok((d, i + 1));
            }
            loop {
                if i >= self.s.len() || self.at(i) != 0x22 {
                    return Err(Stop); // Expecting property name enclosed in double quotes
                }
                // The memo keeps the key alive (the dict holds it); `kref` is our reference.
                let (_k, kref, next) = self.key(i + 1)?;
                i = scan::skip_ws(self.s, next);
                if i >= self.s.len() || self.at(i) != 0x3A {
                    return Err(Stop); // Expecting ':' delimiter
                }
                i = scan::skip_ws(self.s, i + 1);
                let (v, next) = self.value(i)?;
                if ffi::PyDict_SetItem(d.0, kref.0, v.0) < 0 {
                    ffi::PyErr_Clear();
                    return Err(Stop);
                }
                drop(v);
                drop(kref);
                i = scan::skip_ws(self.s, next);
                if i < self.s.len() && self.at(i) == 0x7D {
                    return Ok((d, i + 1));
                }
                if i >= self.s.len() || self.at(i) != 0x2C {
                    return Err(Stop); // Expecting ',' delimiter
                }
                i = scan::skip_ws(self.s, i + 1);
                if i < self.s.len() && self.at(i) == 0x7D {
                    return Err(Stop); // Illegal trailing comma before end of object
                }
            }
        }
    }

    /// `_parse_array_unicode` (no hook) after `[`.
    fn array(&mut self, mut i: usize) -> R<(Obj, usize)> {
        // SAFETY: attached thread; a fresh list nobody else sees.
        unsafe {
            let l = Obj::new(ffi::PyList_New(0))?;
            i = scan::skip_ws(self.s, i);
            if i < self.s.len() && self.at(i) == 0x5D {
                return Ok((l, i + 1));
            }
            loop {
                let (v, next) = self.value(i)?;
                if ffi::PyList_Append(l.0, v.0) < 0 {
                    ffi::PyErr_Clear();
                    return Err(Stop);
                }
                drop(v);
                i = scan::skip_ws(self.s, next);
                if i < self.s.len() && self.at(i) == 0x5D {
                    return Ok((l, i + 1));
                }
                if i >= self.s.len() || self.at(i) != 0x2C {
                    return Err(Stop); // Expecting ',' delimiter
                }
                i = scan::skip_ws(self.s, i + 1);
                if i < self.s.len() && self.at(i) == 0x5D {
                    return Err(Stop); // Illegal trailing comma before end of array
                }
            }
        }
    }

    /// `_match_number_unicode` with the default `parse_int`/`parse_float`.
    #[inline]
    fn number(&mut self, i: usize) -> R<(Obj, usize)> {
        let n = scan::number(self.s, i).ok_or(Stop)?;
        let tok = &self.s[i..n.end];
        // SAFETY: attached thread; CPython constructors checked for NULL.
        unsafe {
            if !n.is_float {
                let digits = tok.len() - usize::from(tok[0].get() == u32::from(b'-'));
                if digits <= 18 {
                    let mut v: i64 = 0;
                    for u in &tok[tok.len() - digits..] {
                        v = v * 10 + i64::from(u.get() - 0x30);
                    }
                    if tok.len() != digits {
                        v = -v;
                    }
                    return Ok((Obj::new(ffi::PyLong_FromLongLong(v))?, n.end));
                }
                // CPython's conversion (`int_max_str_digits` applies; an error falls back).
                let mut b: Vec<u8> = tok.iter().map(|u| u.get() as u8).collect();
                b.push(0);
                let o = ffi::PyLong_FromString(b.as_ptr().cast(), std::ptr::null_mut(), 10);
                return Ok((Obj::new(o)?, n.end));
            }
            // ASCII digits, sign, '.', 'e'/'E': correctly rounded like `float(s)`.
            let mut small = [0u8; 64];
            let x = if tok.len() <= small.len() {
                for (d, u) in small.iter_mut().zip(tok) {
                    *d = u.get() as u8;
                }
                std::str::from_utf8_unchecked(&small[..tok.len()]).parse::<f64>()
            } else {
                let b: Vec<u8> = tok.iter().map(|u| u.get() as u8).collect();
                std::str::from_utf8_unchecked(&b).parse::<f64>()
            }
            .map_err(|_| Stop)?;
            Ok((Obj::new(ffi::PyFloat_FromDouble(x))?, n.end))
        }
    }
}
