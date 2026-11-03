//! Group `urllib`: `urllib.parse.quote`, `quote_plus`, `unquote`, `unquote_plus`,
//! `parse_qsl`, `parse_qs` and `_urlsplit` (the uncached work under `urlsplit` — whose
//! `functools.lru_cache` wrapper stays CPython's, so `cache_info()` is unchanged — and under
//! `urlparse`/`urljoin`), as vectorcall kernels on the module's function objects running
//! `rustique_core::urlparse`. See docs/targets/urllib.md.
//!
//! OWNER: utils workstream.
//!
//! Exact subset: exact `str` arguments (and exact `bytes` for `quote`), UTF-8 with the
//! default error handlers (`quote`: `strict`, text without lone surrogates; `unquote`:
//! `replace`, or `strict`/`ignore`/`surrogateescape` decoding through CPython's own UTF-8
//! decoder), `parse_qsl` without `strict_parsing` errors or field-limit errors, `_urlsplit`
//! of URLs whose netloc is ASCII without brackets. Results are built from slices of the
//! argument objects with `PyUnicode_Substring` (CPython's slicing), so identity matches
//! where CPython returns its argument (`unquote` without `%`, whole-string components).
//! Everything else runs CPython's original. All targets are side-effect free (`checkable`).
//! Private state skipped: `_byte_quoter_factory`'s cache and `_Quoter` dicts (`quote`),
//! the lazily built `_hextobyte` table (`unquote`).

use std::ffi::CStr;
use std::sync::atomic::{AtomicPtr, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyList, PyString};
use rustique_core::urlparse as up;

use crate::accel::{self, Prepare, Reason, Refusal, TargetSpec};
use crate::targets::textwrap::pep393::{self, Bufs, StrWriter, Unit, Units, with_bufs, with_units};
use crate::targets::utils_kit::{
    Guard, O, bind, call_original, int_value, interned, is_type, keep, key, nargs, pin_global_keys, pin_keys,
};

const R: Ordering = Ordering::Relaxed;

macro_rules! null {
    () => {
        AtomicPtr::new(std::ptr::null_mut())
    };
}

#[inline(always)]
fn ld(p: &AtomicPtr<ffi::PyObject>) -> O {
    p.load(R)
}

#[inline(always)]
unsafe fn exact_str(o: O) -> bool {
    // SAFETY: live object.
    unsafe { is_type(o, &raw mut ffi::PyUnicode_Type) }
}

/// `s == lit` for an exact str and an ASCII literal (no allocation, no Python code).
unsafe fn str_is(s: O, lit: &str) -> bool {
    // SAFETY: exact str.
    unsafe {
        exact_str(s)
            && ffi::PyUnicode_GET_LENGTH(s) as usize == lit.len()
            && lit.bytes().enumerate().all(|(i, b)| ffi::PyUnicode_ReadChar(s, i as ffi::Py_ssize_t) == u32::from(b))
    }
}

/// A new `str` (never a cached singleton: CPython's `''.join` of several parts and
/// `str.replace` build a fresh object even for one character) with the code points of `v`.
/// NULL with the error cleared on failure.
unsafe fn fresh_str<T: Unit>(v: &[T]) -> O {
    // SAFETY: attached thread; exactly `v.len()` points of class `max_cp(v)` written.
    unsafe {
        match StrWriter::fresh(v.len(), pep393::max_cp(v)) {
            Some(mut w) => {
                w.put(v);
                w.finish()
            }
            None => std::ptr::null_mut(),
        }
    }
}

/// The bytes of an exact `bytes` object.
#[inline]
unsafe fn bytes_of<'a>(b: O) -> &'a [u8] {
    // SAFETY: exact bytes, alive while the slice is used.
    unsafe { std::slice::from_raw_parts(ffi::PyBytes_AS_STRING(b).cast::<u8>(), ffi::PyBytes_Size(b) as usize) }
}

// ---------------------------------------------------------------------------------------
// Statics
// ---------------------------------------------------------------------------------------

static UNSAFE_LIST: AtomicPtr<ffi::PyObject> = null!();
static S_STRING: AtomicPtr<ffi::PyObject> = null!();
static S_SAFE: AtomicPtr<ffi::PyObject> = null!();
static S_ENCODING: AtomicPtr<ffi::PyObject> = null!();
static S_ERRORS: AtomicPtr<ffi::PyObject> = null!();
static S_QS: AtomicPtr<ffi::PyObject> = null!();
static S_KEEP: AtomicPtr<ffi::PyObject> = null!();
static S_STRICT: AtomicPtr<ffi::PyObject> = null!();
static S_MAXF: AtomicPtr<ffi::PyObject> = null!();
static S_SEP: AtomicPtr<ffi::PyObject> = null!();
static S_STACKLEVEL: AtomicPtr<ffi::PyObject> = null!();
static S_URL: AtomicPtr<ffi::PyObject> = null!();
static S_SCHEME: AtomicPtr<ffi::PyObject> = null!();
static S_ALLOW: AtomicPtr<ffi::PyObject> = null!();
static S_LOWER: AtomicPtr<ffi::PyObject> = null!();

static G_QUOTE: Guard = Guard::new(&URLLIB_DEPS);
static G_QUOTE_PLUS: Guard = Guard::new(&URLLIB_DEPS);
static G_UNQUOTE: Guard = Guard::new(&URLLIB_DEPS);
static G_UNQUOTE_PLUS: Guard = Guard::new(&URLLIB_DEPS);
static G_PARSE_QSL: Guard = Guard::new(&URLLIB_DEPS);
static G_PARSE_QS: Guard = Guard::new(&URLLIB_DEPS);
static G_URLSPLIT: Guard = Guard::new(&URLLIB_DEPS);

/// Count the hit and, in check mode, compare with the original.
unsafe fn checked(g: &Guard, spec: &'static TargetSpec, r: O, func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: the original with the unchanged arguments.
    unsafe {
        if !g.counters.hit_check_streak() {
            return r;
        }
        let n = nargs(nargsf) + if kw.is_null() { 0 } else { ffi::PyTuple_GET_SIZE(kw) as usize };
        let argv: Vec<O> = (0..n).map(|i| *args.add(i)).collect();
        accel::check::verify(spec, r, &argv, || call_original(func, args, nargsf, kw))
    }
}

// ---------------------------------------------------------------------------------------
// quote / quote_plus
// ---------------------------------------------------------------------------------------

/// What `quote` encodes: the code units of a str (encoded as UTF-8 on the fly) or bytes.
#[derive(Clone, Copy)]
enum QIn<'a> {
    Str(Units<'a>),
    Bytes(&'a [u8]),
}

/// The input of `quote(string, encoding, errors)`: a str with UTF-8/`strict` (the defaults
/// or given explicitly), or exact bytes without codec arguments. `None`: outside the subset
/// (CPython raises or uses another codec).
unsafe fn quote_input<'a>(string: O, encoding: O, errors: O) -> Option<QIn<'a>> {
    // SAFETY: live objects.
    unsafe {
        let none = ffi::Py_None();
        if exact_str(string) {
            let enc_ok =
                encoding.is_null() || encoding == none || str_is(encoding, "utf-8") || str_is(encoding, "utf8");
            let err_ok = errors.is_null() || errors == none || str_is(errors, "strict");
            return (enc_ok && err_ok).then(|| QIn::Str(pep393::units(string)));
        }
        if is_type(string, &raw mut ffi::PyBytes_Type)
            && (encoding.is_null() || encoding == none)
            && (errors.is_null() || errors == none)
        {
            return Some(QIn::Bytes(bytes_of(string)));
        }
        None
    }
}

/// The safe set of `quote_from_bytes` from a `str` (`safe.encode('ascii', 'ignore')`) or
/// `bytes` (bytes < 128) `safe`, plus `extra`.
unsafe fn safe_of(safe: O, extra: Option<u32>) -> Option<[bool; 256]> {
    // SAFETY: live object.
    unsafe {
        let mut t = if exact_str(safe) {
            with_units!(pep393::units(safe), s => up::safe_table(s.iter().map(|&c| c.cp())))
        } else if is_type(safe, &raw mut ffi::PyBytes_Type) {
            up::safe_table(bytes_of(safe).iter().map(|&b| u32::from(b)))
        } else {
            return None;
        };
        if let Some(c) = extra {
            t[c as usize] = true;
        }
        Some(t)
    }
}

/// The UTF-8 bytes of code point `c` (`None`: a lone surrogate — `strict` raises).
#[inline(always)]
fn utf8(c: u32, buf: &mut [u8; 4]) -> Option<&[u8]> {
    if c < 128 {
        buf[0] = c as u8;
        return Some(&buf[..1]);
    }
    Some(char::from_u32(c)?.encode_utf8(buf).as_bytes())
}

/// Visit the bytes `quote` sees (`string.encode('utf-8')` for a str); `false` if the str
/// has a lone surrogate (the encoder raises).
#[inline]
fn each_byte(input: QIn<'_>, mut f: impl FnMut(u8)) -> bool {
    match input {
        QIn::Bytes(b) => {
            b.iter().for_each(|&x| f(x));
            true
        }
        QIn::Str(u) => with_units!(u, t => {
            let mut buf = [0u8; 4];
            for &c in t {
                let c: u32 = c.cp();
                if c < 128 {
                    f(c as u8);
                } else {
                    let Some(b) = utf8(c, &mut buf) else { return false };
                    b.iter().for_each(|&x| f(x));
                }
            }
            true
        }),
    }
}

/// `quote(string, safe, encoding, errors)`; `plus`: `quote_plus`, i.e. when the input has
/// a space, `quote(string, safe + ' ', ...).replace(' ', '+')`.
///
/// Results (identity as CPython's): `quote('')` is the argument; all bytes safe:
/// `bs.decode()` (a new str; the singleton for one character); a single unsafe byte: the
/// `_Quoter` cache's shared `'%XX'` object (fall back: CPython's cache owns it); otherwise
/// `''.join(...)` of several parts (a new str). `quote_plus` with a space: `str.replace`'s
/// new object (never a singleton).
unsafe fn quote_kernel(
    g: &Guard,
    spec: &'static TargetSpec,
    plus: bool,
    func: O,
    args: *const O,
    nargsf: usize,
    kw: O,
) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let names = [ld(&S_STRING), ld(&S_SAFE), ld(&S_ENCODING), ld(&S_ERRORS)];
        if let Some([string, safe, encoding, errors]) = bind(args, nargs(nargsf), kw, names)
            && !string.is_null()
        {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if let Some(input) = quote_input(string, encoding, errors) {
                // `quote('')` returns its argument.
                if matches!(input, QIn::Str(u) if u.is_empty()) {
                    return checked(g, spec, ffi::Py_NewRef(string), func, args, nargsf, kw);
                }
                // A one-byte input that needs quoting: CPython returns the `_Quoter` cache's
                // shared object; decide that before any other work.
                let one = match input {
                    QIn::Bytes(b) if b.len() == 1 => Some(u32::from(b[0])),
                    QIn::Str(u) if u.len() == 1 && u.at(0) < 128 => Some(u.at(0)),
                    _ => None,
                };
                if let Some(c) = one
                    && !up::ALWAYS_SAFE_TABLE[c as usize]
                    && !(c == u32::from(b'/') && !plus && safe.is_null())
                    && !(c == 0x20 && plus)
                    && (safe.is_null() || !safe_of(safe, None).is_some_and(|t| t[c as usize]))
                {
                    g.counters.fallback_streak(Reason::Args);
                    return call_original(func, args, nargsf, kw);
                }
                let has_space = match input {
                    QIn::Bytes(b) => b.contains(&b' '),
                    QIn::Str(u) => with_units!(u, t => t.iter().any(|&c| c.cp() == 0x20)),
                };
                let extra = (plus && has_space).then_some(0x20);
                let table = if safe.is_null() {
                    // defaults: quote: safe='/', quote_plus: safe=''
                    let mut t = up::ALWAYS_SAFE_TABLE;
                    if !plus {
                        t[b'/' as usize] = true;
                    }
                    if let Some(c) = extra {
                        t[c as usize] = true;
                    }
                    Some(t)
                } else {
                    safe_of(safe, extra)
                };
                // Pass 1: input bytes and output length (UTF-8 encode errors fall back).
                let (mut nbytes, mut out_len, mut unsafe_bytes) = (0usize, 0usize, 0usize);
                if let Some(table) = table
                    && each_byte(input, |b| {
                        nbytes += 1;
                        if table[b as usize] {
                            out_len += 1;
                        } else {
                            out_len += 3;
                            unsafe_bytes += 1;
                        }
                    })
                    && nbytes < 200_000
                    && !(nbytes == 1 && unsafe_bytes == 1)
                {
                    let swap = plus && has_space;
                    let w = if swap { StrWriter::fresh(out_len, 127) } else { StrWriter::new(out_len, 127) };
                    if let Some(mut w) = w {
                        // Pass 2: write (ASCII output).
                        const HEX: &[u8; 16] = b"0123456789ABCDEF";
                        each_byte(input, |b| {
                            if table[b as usize] {
                                w.put_ascii(&[if swap && b == b' ' { b'+' } else { b }]);
                            } else {
                                w.put_ascii(&[b'%', HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]);
                            }
                        });
                        return checked(g, spec, w.finish(), func, args, nargsf, kw);
                    }
                }
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

unsafe extern "C" fn vc_quote(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { quote_kernel(&G_QUOTE, &QUOTE, false, func, args, nargsf, kw) }
}

unsafe extern "C" fn vc_quote_plus(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { quote_kernel(&G_QUOTE_PLUS, &QUOTE_PLUS, true, func, args, nargsf, kw) }
}

// ---------------------------------------------------------------------------------------
// unquote / unquote_plus
// ---------------------------------------------------------------------------------------

/// The UTF-8 error handler of an `unquote` call, as a C string (`None`: unsupported).
unsafe fn decode_errors(encoding: O, errors: O) -> Option<&'static CStr> {
    // SAFETY: live objects.
    unsafe {
        let none = ffi::Py_None();
        let enc_ok = encoding.is_null() || encoding == none || str_is(encoding, "utf-8") || str_is(encoding, "utf8");
        if !enc_ok {
            return None;
        }
        if errors.is_null() || errors == none || str_is(errors, "replace") {
            Some(c"replace")
        } else if str_is(errors, "strict") {
            Some(c"strict")
        } else if str_is(errors, "ignore") {
            Some(c"ignore")
        } else if str_is(errors, "surrogateescape") {
            Some(c"surrogateescape")
        } else {
            None
        }
    }
}

/// `unquote_plus(s)` / `unquote(s)` of the code units `t` of a str (or of a slice of one):
/// `Ok(None)` when CPython returns its argument unchanged (no `%`, no `+` to replace),
/// `Ok(Some(new str))`, or `Err` to fall back (a strict decode error or an allocation
/// failure: nothing observable happened).
///
/// CPython: `string.replace('+', ' ')` (a new object), then with a `%`:
/// `''.join(_generate_unquoted_parts(...))` — non-ASCII runs kept, ASCII runs percent-
/// decoded (`_unquote_impl`) and decoded by CPython's UTF-8 decoder with `errors`; a join
/// of at least three parts, so always a new object.
unsafe fn unquote_t<T: Unit>(t: &[T], plus: bool, errors: &CStr, b: &mut Bufs) -> Result<Option<O>, ()> {
    // SAFETY: attached thread.
    unsafe {
        let has_pct = t.iter().any(|&c| c.into() == 0x25);
        let has_plus = plus && t.iter().any(|&c| c.into() == 0x2b);
        if !has_pct {
            if !has_plus {
                return Ok(None);
            }
            // '+' and ' ' are both ASCII: the class of `t` is the result's.
            let mut w = StrWriter::fresh(t.len(), pep393::max_cp(t)).ok_or(())?;
            for &c in t {
                let c: u32 = c.into();
                w.put1(if c == 0x2b { 0x20 } else { c });
            }
            return Ok(Some(w.finish()));
        }
        let map = |c: u32| if has_plus && c == 0x2b { 0x20u8 } else { c as u8 };
        let [b0, b1] = &mut b.b;
        if t.iter().all(|&c| c.into() < 128) {
            // One ASCII run (the common case).
            b1.clear();
            b1.extend(t.iter().map(|&c| map(c.into())));
            b0.clear();
            up::unquote_bytes(b1, b0);
            if b0.is_ascii() {
                let r = fresh_str(b0.as_slice());
                return if r.is_null() { Err(()) } else { Ok(Some(r)) };
            }
            let d = ffi::PyUnicode_DecodeUTF8(b0.as_ptr().cast(), b0.len() as ffi::Py_ssize_t, errors.as_ptr());
            if d.is_null() {
                ffi::PyErr_Clear();
                return Err(());
            }
            // The decoder's new string is the join's value; a one-character result may be
            // the cached singleton, which the join would have copied.
            if ffi::PyUnicode_GET_LENGTH(d) >= 2 && ffi::Py_REFCNT(d) == 1 {
                return Ok(Some(d));
            }
            let r = with_units!(pep393::units(d), s => fresh_str(s));
            ffi::Py_DECREF(d);
            return if r.is_null() { Err(()) } else { Ok(Some(r)) };
        }
        let out = &mut b.u[0];
        out.clear();
        out.reserve(t.len());
        let mut i = 0;
        while i < t.len() {
            let c: u32 = t[i].into();
            if c >= 128 {
                out.push(c);
                i += 1;
                continue;
            }
            let s = i;
            while i < t.len() && t[i].into() < 128 {
                i += 1;
            }
            b1.clear();
            b1.extend(t[s..i].iter().map(|&c| map(c.into())));
            b0.clear();
            up::unquote_bytes(b1, b0);
            if b0.is_ascii() {
                out.extend(b0.iter().map(|&x| u32::from(x)));
            } else {
                let d = ffi::PyUnicode_DecodeUTF8(b0.as_ptr().cast(), b0.len() as ffi::Py_ssize_t, errors.as_ptr());
                if d.is_null() {
                    ffi::PyErr_Clear();
                    return Err(());
                }
                pep393::units(d).push_to(out);
                ffi::Py_DECREF(d);
            }
        }
        let r = fresh_str(out.as_slice());
        if r.is_null() { Err(()) } else { Ok(Some(r)) }
    }
}

unsafe fn unquote_kernel(
    g: &Guard,
    spec: &'static TargetSpec,
    plus: bool,
    func: O,
    args: *const O,
    nargsf: usize,
    kw: O,
) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let names = [ld(&S_STRING), ld(&S_ENCODING), ld(&S_ERRORS)];
        if let Some([string, encoding, errors]) = bind(args, nargs(nargsf), kw, names)
            && !string.is_null()
            && exact_str(string)
            && let Some(errs) = decode_errors(encoding, errors)
        {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            let r = with_bufs(|b| with_units!(pep393::units(string), t => unquote_t(t, plus, errs, b)));
            match r {
                Ok(None) => return checked(g, spec, ffi::Py_NewRef(string), func, args, nargsf, kw),
                Ok(Some(o)) => return checked(g, spec, o, func, args, nargsf, kw),
                Err(()) => {}
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

unsafe extern "C" fn vc_unquote(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { unquote_kernel(&G_UNQUOTE, &UNQUOTE, false, func, args, nargsf, kw) }
}

unsafe extern "C" fn vc_unquote_plus(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { unquote_kernel(&G_UNQUOTE_PLUS, &UNQUOTE_PLUS, true, func, args, nargsf, kw) }
}

// ---------------------------------------------------------------------------------------
// parse_qsl / parse_qs
// ---------------------------------------------------------------------------------------

/// `qs.split(separator)` field ranges, in order (non-overlapping occurrences, left to right).
fn for_fields<T: Unit>(t: &[T], sep: &[u32], mut f: impl FnMut(usize, usize) -> bool) -> bool {
    let (n, m) = (t.len(), sep.len());
    let mut start = 0;
    let mut i = 0;
    if m == 1 {
        let s = sep[0];
        while i < n {
            if t[i].into() == s {
                if !f(start, i) {
                    return false;
                }
                start = i + 1;
            }
            i += 1;
        }
    } else {
        while i + m <= n {
            if t[i..i + m].iter().zip(sep).all(|(&a, &b)| a.into() == b) {
                if !f(start, i) {
                    return false;
                }
                i += m;
                start = i;
            } else {
                i += 1;
            }
        }
    }
    f(start, n)
}

/// `parse_qsl(qs, ...)` for an exact str, each decoded `(name, value)` handed to `emit`
/// (new references; `emit` returns false on an allocation failure). `None`: fall back
/// (CPython raises, or an unsupported argument) — nothing observable happened.
unsafe fn parse_pairs(
    args: *const O,
    nargsf: usize,
    kw: O,
    with_stacklevel: bool,
    emit: &mut dyn FnMut(O, O) -> bool,
) -> Option<()> {
    // SAFETY: vectorcall arguments.
    unsafe {
        let names = [
            ld(&S_QS),
            ld(&S_KEEP),
            ld(&S_STRICT),
            ld(&S_ENCODING),
            ld(&S_ERRORS),
            ld(&S_MAXF),
            ld(&S_SEP),
            ld(&S_STACKLEVEL),
        ];
        let npos = nargs(nargsf);
        if npos > 7 {
            return None;
        }
        let [qs, keep, strict, encoding, errors, maxf, sep, stacklevel] = bind(args, npos, kw, names)?;
        if qs.is_null() || !exact_str(qs) || (!stacklevel.is_null() && !with_stacklevel) {
            return None;
        }
        let truth = |o: O| -> Option<bool> {
            if o.is_null() || o == ffi::Py_False() || o == ffi::Py_None() {
                Some(false)
            } else if o == ffi::Py_True() {
                Some(true)
            } else if is_type(o, &raw mut ffi::PyLong_Type) {
                Some(ffi::PyObject_IsTrue(o) != 0)
            } else {
                None
            }
        };
        let keep = truth(keep)?;
        if truth(strict)? {
            return None; // strict_parsing: let CPython validate and raise
        }
        let errs = decode_errors(encoding, errors)?;
        let mut sep_buf = [0u32; 8];
        let sep: &[u32] = if sep.is_null() {
            sep_buf[0] = 0x26;
            &sep_buf[..1]
        } else if exact_str(sep) {
            let u = pep393::units(sep);
            if u.is_empty() || u.len() > sep_buf.len() {
                return None; // ValueError / an unusual separator: CPython
            }
            for (i, slot) in sep_buf.iter_mut().enumerate().take(u.len()) {
                *slot = u.at(i);
            }
            &sep_buf[..u.len()]
        } else {
            return None;
        };
        let u = pep393::units(qs);
        if !maxf.is_null() && maxf != ffi::Py_None() {
            if !is_type(maxf, &raw mut ffi::PyLong_Type) {
                return None;
            }
            let m = int_value(maxf)?;
            if !u.is_empty() {
                let nfields = with_units!(u, t => {
                    let mut k = 0usize;
                    for_fields(t, sep, |_, _| {
                        k += 1;
                        true
                    });
                    k
                });
                if m < nfields as i64 {
                    return None; // ValueError: Max number of fields exceeded
                }
            }
        }
        if u.is_empty() {
            return Some(()); // `if not qs: return []`
        }
        with_bufs(|b| {
            with_units!(u, t => {
                // name or value: `_unquote(part)`; the slice of `qs` itself when unchanged
                // (`PyUnicode_Substring`: CPython's slicing, `qs` for the whole string).
                let part = |a: usize, z: usize, b: &mut Bufs| -> Option<O> {
                    match unquote_t(&t[a..z], true, errs, b) {
                        Ok(Some(o)) => Some(o),
                        Ok(None) => {
                            let s = ffi::PyUnicode_Substring(qs, a as ffi::Py_ssize_t, z as ffi::Py_ssize_t);
                            if s.is_null() {
                                ffi::PyErr_Clear();
                                None
                            } else {
                                Some(s)
                            }
                        }
                        Err(()) => None,
                    }
                };
                for_fields(t, sep, |a, z| {
                    if a == z {
                        return true; // empty field (not strict)
                    }
                    // name, has_eq, value = name_value.partition('=')
                    let eq = t[a..z].iter().position(|&c| c.cp() == 0x3d).map(|p| a + p);
                    let (na, nb, va, vb) = match eq {
                        Some(e) => (a, e, e + 1, z),
                        None => (a, z, z, z),
                    };
                    if vb == va && !keep {
                        return true;
                    }
                    let Some(name) = part(na, nb, b) else { return false };
                    let Some(value) = part(va, vb, b) else {
                        ffi::Py_DECREF(name);
                        return false;
                    };
                    emit(name, value)
                })
            })
        })
        .then_some(())
    }
}

unsafe extern "C" fn vc_parse_qsl(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_PARSE_QSL;
        if g.ready(func) {
            let out = ffi::PyList_New(0);
            if out.is_null() {
                ffi::PyErr_Clear();
            } else {
                let mut emit = |a: O, b: O| -> bool {
                    let t = ffi::PyTuple_New(2);
                    if t.is_null() {
                        ffi::PyErr_Clear();
                        ffi::Py_DECREF(a);
                        ffi::Py_DECREF(b);
                        return false;
                    }
                    ffi::PyTuple_SET_ITEM(t, 0, a);
                    ffi::PyTuple_SET_ITEM(t, 1, b);
                    let ok = ffi::PyList_Append(out, t) == 0;
                    ffi::Py_DECREF(t);
                    if !ok {
                        ffi::PyErr_Clear();
                    }
                    ok
                };
                if parse_pairs(args, nargsf, kw, true, &mut emit).is_some() {
                    return checked(g, &PARSE_QSL, out, func, args, nargsf, kw);
                }
                ffi::Py_DECREF(out);
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

unsafe extern "C" fn vc_parse_qs(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_PARSE_QS;
        if g.ready(func) {
            let d = ffi::PyDict_New();
            if d.is_null() {
                ffi::PyErr_Clear();
            } else {
                // for name, value in pairs: append to parsed_result[name] (or a new list)
                let mut emit = |a: O, b: O| -> bool {
                    let mut cur: O = std::ptr::null_mut();
                    let ok = match ffi::PyDict_GetItemRef(d, a, &mut cur) {
                        1 => {
                            let x = ffi::PyList_Append(cur, b) == 0;
                            ffi::Py_DECREF(cur);
                            x
                        }
                        0 => {
                            let l = ffi::PyList_New(1);
                            if l.is_null() {
                                false
                            } else {
                                ffi::PyList_SET_ITEM(l, 0, ffi::Py_NewRef(b));
                                let x = ffi::PyDict_SetItem(d, a, l) == 0;
                                ffi::Py_DECREF(l);
                                x
                            }
                        }
                        _ => false,
                    };
                    ffi::Py_DECREF(a);
                    ffi::Py_DECREF(b);
                    if !ok {
                        ffi::PyErr_Clear();
                    }
                    ok
                };
                if parse_pairs(args, nargsf, kw, false, &mut emit).is_some() {
                    return checked(g, &PARSE_QS, d, func, args, nargsf, kw);
                }
                ffi::Py_DECREF(d);
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

// ---------------------------------------------------------------------------------------
// _urlsplit
// ---------------------------------------------------------------------------------------

/// `_UNSAFE_URL_BYTES_TO_REMOVE` is still the list of `'\t'`, `'\r'`, `'\n'`.
#[inline]
unsafe fn unsafe_list_ok() -> bool {
    // SAFETY: kept reference to the (pinned) list.
    unsafe {
        let l = ld(&UNSAFE_LIST);
        if ffi::PyList_GET_SIZE(l) != 3 {
            return false;
        }
        ["\t", "\r", "\n"].iter().enumerate().all(|(i, s)| str_is(ffi::PyList_GET_ITEM(l, i as ffi::Py_ssize_t), s))
    }
}

/// `x.lstrip(C0)` (or `strip` when `both`) then the removal of tab/CR/LF, for an exact str
/// `x`: `(object, offset)` such that the cleaned text is `object[offset..]`'s prefix of the
/// cleaned length — either `x` itself with the strip offset (nothing removed: CPython's
/// `lstrip` gives a slice of `x`, `replace` returns its argument) or a new object (`replace`
/// removed something: a new str, never a singleton). New reference.
unsafe fn cleaned(x: O, both: bool, b: &mut Bufs) -> Option<(O, usize, usize)> {
    // SAFETY: attached thread; exact str.
    unsafe {
        with_units!(pep393::units(x), t => {
            let mut a = 0;
            while a < t.len() && up::is_c0_or_space(t[a].cp()) {
                a += 1;
            }
            let mut z = t.len();
            if both {
                while z > a && up::is_c0_or_space(t[z - 1].cp()) {
                    z -= 1;
                }
            }
            let s = &t[a..z];
            if !s.iter().any(|&c| up::is_unsafe_url_char(c.cp())) {
                return Some((ffi::Py_NewRef(x), a, z));
            }
            let v = &mut b.u[1];
            v.clear();
            v.extend(s.iter().map(|&c| c.cp()).filter(|&c| !up::is_unsafe_url_char(c)));
            let o = fresh_str(v.as_slice());
            (!o.is_null()).then_some((o, 0, v.len()))
        })
    }
}

unsafe extern "C" fn vc_urlsplit(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        if let Some(r) = urlsplit(func, args, nargsf, kw) {
            return r;
        }
        G_URLSPLIT.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

unsafe fn urlsplit(func: O, args: *const O, nargsf: usize, kw: O) -> Option<O> {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_URLSPLIT;
        let [url, scheme, allow] = bind(args, nargs(nargsf), kw, [ld(&S_URL), ld(&S_SCHEME), ld(&S_ALLOW)])?;
        if url.is_null() || !exact_str(url) {
            return None;
        }
        let none = ffi::Py_None();
        let scheme = if scheme.is_null() { none } else { scheme };
        if scheme != none && !exact_str(scheme) {
            return None;
        }
        let allow = if allow.is_null() || allow == ffi::Py_True() {
            true
        } else if allow == ffi::Py_False() || allow == none {
            false
        } else if is_type(allow, &raw mut ffi::PyLong_Type) {
            ffi::PyObject_IsTrue(allow) != 0
        } else {
            return None;
        };
        if !g.ready(func) {
            return Some(call_original(func, args, nargsf, kw));
        }
        if !unsafe_list_ok() {
            return None;
        }
        with_bufs(|b| {
            // The cleaned url is `u[off..end]`; components are its slices (CPython's own
            // slicing: `u` itself for the whole string, singletons for one character).
            let (u, off, end) = cleaned(url, false, b)?;
            let split = with_units!(pep393::units(u), t => up::urlsplit(&t[off..end], allow));
            let Ok(split) = split else {
                ffi::Py_DECREF(u);
                return None;
            };
            let sub = |r: (usize, usize)| {
                ffi::PyUnicode_Substring(u, (off + r.0) as ffi::Py_ssize_t, (off + r.1) as ffi::Py_ssize_t)
            };
            let opt = |r: up::Range| r.map_or_else(|| ffi::Py_NewRef(none), sub);
            let scheme_obj = match split.scheme_from_url {
                Some(r) => {
                    let s = sub(r);
                    if s.is_null() {
                        s
                    } else {
                        // `url[:i].lower()` (CPython's own method: a new str)
                        let l = ffi::PyObject_CallMethodNoArgs(s, ld(&S_LOWER));
                        ffi::Py_DECREF(s);
                        l
                    }
                }
                None if scheme == none => ffi::Py_NewRef(none),
                None => match cleaned(scheme, true, b) {
                    Some((s, a, z)) => {
                        let r = ffi::PyUnicode_Substring(s, a as ffi::Py_ssize_t, z as ffi::Py_ssize_t);
                        ffi::Py_DECREF(s);
                        r
                    }
                    None => std::ptr::null_mut(),
                },
            };
            let parts = [scheme_obj, opt(split.netloc), sub(split.path), opt(split.query), opt(split.fragment)];
            ffi::Py_DECREF(u);
            if parts.iter().any(|p| p.is_null()) {
                for p in parts {
                    ffi::Py_XDECREF(p);
                }
                return Some(std::ptr::null_mut());
            }
            let t = ffi::PyTuple_New(5);
            if t.is_null() {
                for p in parts {
                    ffi::Py_DECREF(p);
                }
                return Some(t);
            }
            for (i, p) in parts.into_iter().enumerate() {
                ffi::PyTuple_SET_ITEM(t, i as ffi::Py_ssize_t, p);
            }
            Some(checked(g, &URLSPLIT, t, func, args, nargsf, kw))
        })
    }
}

// ---------------------------------------------------------------------------------------
// Preparation
// ---------------------------------------------------------------------------------------

fn prepare_common(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if S_STRING.load(R).is_null() {
        for (slot, s) in [
            (&S_STRING, "string"),
            (&S_SAFE, "safe"),
            (&S_ENCODING, "encoding"),
            (&S_ERRORS, "errors"),
            (&S_QS, "qs"),
            (&S_KEEP, "keep_blank_values"),
            (&S_STRICT, "strict_parsing"),
            (&S_MAXF, "max_num_fields"),
            (&S_SEP, "separator"),
            (&S_STACKLEVEL, "_stacklevel"),
            (&S_URL, "url"),
            (&S_SCHEME, "scheme"),
            (&S_ALLOW, "allow_fragments"),
            (&S_LOWER, "lower"),
        ] {
            slot.store(interned(py, s), R);
        }
    }
    // Module constants the originals read, checked once (and pinned by the targets).
    let c0 = cx.module_attr("_WHATWG_C0_CONTROL_OR_SPACE")?;
    let want: String = (0u8..=0x20).map(char::from).collect();
    if c0.cast::<PyString>().map_err(PyErr::from)?.to_cow()? != want {
        return Err(Refusal::new("_WHATWG_C0_CONTROL_OR_SPACE differs"));
    }
    let sc = cx.module_attr("scheme_chars")?;
    let want_sc = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-.";
    if sc.cast::<PyString>().map_err(PyErr::from)?.to_cow()? != want_sc {
        return Err(Refusal::new("scheme_chars differs"));
    }
    let l = cx.module_attr("_UNSAFE_URL_BYTES_TO_REMOVE")?;
    l.cast::<PyList>().map_err(PyErr::from)?;
    keep(&UNSAFE_LIST, &l);
    // SAFETY: the list just kept.
    if unsafe { !unsafe_list_ok() } {
        return Err(Refusal::new("_UNSAFE_URL_BYTES_TO_REMOVE differs"));
    }
    let safe = cx.module_attr("_ALWAYS_SAFE_BYTES")?;
    let want_safe: Vec<u8> = (0..=255u8).filter(|&b| up::always_safe(b)).collect();
    let mut have: Vec<u8> = safe.extract()?;
    have.sort_unstable();
    if have != want_safe {
        return Err(Refusal::new("_ALWAYS_SAFE_BYTES differs"));
    }
    Ok(())
}

/// Pins of the group (`SHARED`): the module globals and builtins the seven originals read
/// on the accelerated paths.
static URLLIB_DEPS: accel::DepFlag = accel::DepFlag::new();

/// The group's shared declarations (prepared once before the first target).
/// The module helpers the kernels reimplement on their accepted paths (quoting through
/// `_byte_quoter_factory`'s `_Quoter`, unquoting, the netloc checks of `_urlsplit` for an
/// ASCII netloc without brackets): fingerprinted so that a patch release changing one
/// refuses instead of differing. `_check_bracketed_host` is not: bracketed netlocs fall
/// back. Inline v2 values (3.14.9 / 3.15.1); the recorded v3 sets are in
/// `accel/fingerprints.txt`.
const URL_HELPERS: [(&str, accel::Fingerprints); 8] = [
    ("quote_from_bytes", &[(3, 14, "b5cd6f61ae36077e"), (3, 15, "d4d31e1f59fa3248")]),
    ("_byte_quoter_factory", &[(3, 14, "6c3350c664036dbe"), (3, 15, "6ec68c585534130f")]),
    ("_unquote_impl", &[(3, 14, "e2c6486fd86c9e05"), (3, 15, "1cf55bc4ba119164")]),
    ("_generate_unquoted_parts", &[(3, 14, "2145397716b3a43f"), (3, 15, "dde052b38b1b8c8b")]),
    ("unquote_to_bytes", &[(3, 14, "a07614d01156af76"), (3, 15, "2361d8a6390f809f")]),
    ("_splitnetloc", &[(3, 14, "184be9387f292dc5"), (3, 15, "249451b4aa2c7b7a")]),
    ("_checknetloc", &[(3, 14, "41b0aceb5e297d77"), (3, 15, "305ebd0ef18ff6ba")]),
    ("_check_bracketed_netloc", &[(3, 14, "cc53d21ea63a0e4a"), (3, 15, "a607276c54c49a1a")]),
];
const QUOTER_HELPERS: [(&str, accel::Fingerprints); 2] = [
    ("__init__", &[(3, 14, "7a057e1ce085ce97"), (3, 15, "9042e804a5b097fb")]),
    ("__missing__", &[(3, 14, "83cf665569323911"), (3, 15, "4cab190d58a497f4")]),
];

fn prepare_group<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
    prepare_common(py, cx)?;
    let globals = cx.module()?.dict();
    for (name, fps) in URL_HELPERS {
        let f = cx.get(&globals, name)?;
        // `_byte_quoter_factory` is an `lru_cache` wrapper: its function is `__wrapped__`.
        let f = if name == "_byte_quoter_factory" { f.getattr("__wrapped__")? } else { f };
        cx.fingerprint_code(&f, fps)?;
    }
    let quoter = cx.get(&globals, "_Quoter")?.cast_into::<pyo3::types::PyType>().map_err(PyErr::from)?;
    let qd = crate::accel::prepare::type_dict(&quoter)?;
    for (name, fps) in QUOTER_HELPERS {
        cx.fingerprint_code(&cx.get(&qd, name)?, fps)?;
    }
    pin_keys!(
        cx,
        &globals,
        &URLLIB_DEPS,
        [
            "quote",
            "quote_from_bytes",
            "_ALWAYS_SAFE_BYTES",
            "_byte_quoter_factory",
            "unquote",
            "unquote_plus",
            "_unquote_impl",
            "_generate_unquoted_parts",
            "_asciire",
            "parse_qsl",
            "_WHATWG_C0_CONTROL_OR_SPACE",
            "_UNSAFE_URL_BYTES_TO_REMOVE",
            "scheme_chars",
            "_splitnetloc",
            "_check_bracketed_netloc",
            "_checknetloc",
        ]
    );
    pin_global_keys!(
        cx,
        &globals,
        &URLLIB_DEPS,
        ["isinstance", "str", "bytes", "bytearray", "len", "map", "bool", "min"]
    );
    Ok(())
}

/// Shared declarations of the seven targets.
pub static SHARED: accel::GroupSpec = accel::GroupSpec { name: "urllib", prepare: prepare_group };

/// A target's own declarations: fingerprint, guard identity, site.
fn prepare_fn<'py>(
    cx: &mut Prepare<'py>,
    attr: &Bound<'py, PyString>,
    fp: accel::Fingerprints,
    g: &'static Guard,
    kernel: ffi::vectorcallfunc,
) -> Result<(), Refusal> {
    let func = cx
        .get_opt_s(&cx.module()?.dict(), attr)?
        .ok_or_else(|| Refusal::new(format!("urllib.parse.{attr} missing")))?;
    cx.fingerprint_code(&func, fp)?;
    g.capture(&func);
    cx.vectorcall(&func, kernel, &accel::FUNCTION_VECTORCALL)
}

macro_rules! target {
    ($spec:ident, $prep:ident, $name:literal, $attr:literal, $kernel:ident, $guard:ident, $fp:expr) => {
        fn $prep<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
            prepare_fn(cx, &key(py, $attr), $fp, &$guard, $kernel)
        }
        pub static $spec: TargetSpec = TargetSpec {
            name: $name,
            group: "urllib",
            module: Some("urllib.parse"),
            versions: &[(3, 14), (3, 15)],
            free_threaded: true,
            counters: &$guard.counters,
            prepare: $prep,
            checkable: true,
            shared: Some(&SHARED),
            ..TargetSpec::DEFAULT
        };
    };
}

target!(
    QUOTE,
    prep_quote,
    "urllib.parse.quote",
    "quote",
    vc_quote,
    G_QUOTE,
    &[(3, 14, "63e2a4049e9eaf92"), (3, 15, "94de6386774e0075")]
);
target!(
    QUOTE_PLUS,
    prep_quote_plus,
    "urllib.parse.quote_plus",
    "quote_plus",
    vc_quote_plus,
    G_QUOTE_PLUS,
    &[(3, 14, "a94b1533a164b8f0"), (3, 15, "a048317d78bf0841")]
);
target!(
    UNQUOTE,
    prep_unquote,
    "urllib.parse.unquote",
    "unquote",
    vc_unquote,
    G_UNQUOTE,
    &[(3, 14, "aa04e1662d0123ca"), (3, 15, "1e55635c96370788")]
);
target!(
    UNQUOTE_PLUS,
    prep_unquote_plus,
    "urllib.parse.unquote_plus",
    "unquote_plus",
    vc_unquote_plus,
    G_UNQUOTE_PLUS,
    &[(3, 14, "acbbc4556e4ffe7a"), (3, 15, "66c61fa58bfdacc7")]
);
target!(
    PARSE_QSL,
    prep_parse_qsl,
    "urllib.parse.parse_qsl",
    "parse_qsl",
    vc_parse_qsl,
    G_PARSE_QSL,
    &[(3, 14, "c4385431f4670861"), (3, 15, "6cf72a7ada63572a")]
);
target!(
    PARSE_QS,
    prep_parse_qs,
    "urllib.parse.parse_qs",
    "parse_qs",
    vc_parse_qs,
    G_PARSE_QS,
    &[(3, 14, "d9510d05a0e09245"), (3, 15, "4228046b5716b612")]
);
target!(
    URLSPLIT,
    prep_urlsplit,
    "urllib.parse._urlsplit",
    "_urlsplit",
    vc_urlsplit,
    G_URLSPLIT,
    &[(3, 14, "8d4cdc781ae3dbf2"), (3, 15, "d7c3aef3e7e67e8d")]
);

pub static TARGETS: &[&TargetSpec] = &[&QUOTE, &QUOTE_PLUS, &UNQUOTE, &UNQUOTE_PLUS, &PARSE_QSL, &PARSE_QS, &URLSPLIT];
