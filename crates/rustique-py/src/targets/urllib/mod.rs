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

use std::ffi::{CStr, c_int};
use std::sync::atomic::{AtomicPtr, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyList, PyString};
use rustique_core::urlparse as up;

use crate::accel::{self, Prepare, Reason, Refusal, TargetSpec};
use crate::targets::utils_kit::{
    Guard, O, bind, call_original, int_value, interned, is_type, keep, nargs, pin_globals,
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

/// Code points of an exact `str`.
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

unsafe fn str_from_ucs4(v: &[u32]) -> O {
    // SAFETY: valid buffer.
    unsafe {
        ffi::PyUnicode_FromKindAndData(
            ffi::PyUnicode_4BYTE_KIND as c_int,
            v.as_ptr().cast(),
            v.len() as ffi::Py_ssize_t,
        )
    }
}

unsafe fn str_from_ascii(v: &[u8]) -> O {
    // SAFETY: ASCII bytes.
    unsafe {
        ffi::PyUnicode_FromKindAndData(
            ffi::PyUnicode_1BYTE_KIND as c_int,
            v.as_ptr().cast(),
            v.len() as ffi::Py_ssize_t,
        )
    }
}

/// UTF-8 of code points (`None`: a lone surrogate — `encode('utf-8', 'strict')` raises).
fn utf8(v: &[u32]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(v.len());
    let mut buf = [0u8; 4];
    for &c in v {
        if c < 128 {
            out.push(c as u8);
        } else {
            out.extend_from_slice(char::from_u32(c)?.encode_utf8(&mut buf).as_bytes());
        }
    }
    Some(out)
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

static G_QUOTE: Guard = Guard::new();
static G_QUOTE_PLUS: Guard = Guard::new();
static G_UNQUOTE: Guard = Guard::new();
static G_UNQUOTE_PLUS: Guard = Guard::new();
static G_PARSE_QSL: Guard = Guard::new();
static G_PARSE_QS: Guard = Guard::new();
static G_URLSPLIT: Guard = Guard::new();

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

/// The bytes `quote` encodes: `string.encode('utf-8', 'strict')` of a str, or bytes as is.
/// `None`: outside the subset (CPython raises or uses another codec).
unsafe fn quote_input(string: O, encoding: O, errors: O) -> Option<Vec<u8>> {
    // SAFETY: live objects.
    unsafe {
        let none = ffi::Py_None();
        if exact_str(string) {
            let enc_ok =
                encoding.is_null() || encoding == none || str_is(encoding, "utf-8") || str_is(encoding, "utf8");
            let err_ok = errors.is_null() || errors == none || str_is(errors, "strict");
            if !(enc_ok && err_ok) {
                return None;
            }
            return utf8(&ucs4(string)?);
        }
        if is_type(string, &raw mut ffi::PyBytes_Type)
            && (encoding.is_null() || encoding == none)
            && (errors.is_null() || errors == none)
        {
            let p = ffi::PyBytes_AS_STRING(string).cast::<u8>();
            return Some(std::slice::from_raw_parts(p, ffi::PyBytes_Size(string) as usize).to_vec());
        }
        None
    }
}

/// The safe set of `quote_from_bytes` from a `str` or `bytes` `safe` (plus `extra`).
unsafe fn safe_of(safe: O, extra: Option<u32>) -> Option<[bool; 256]> {
    // SAFETY: live object.
    unsafe {
        let mut cps: Vec<u32> = if exact_str(safe) {
            ucs4(safe)?
        } else if is_type(safe, &raw mut ffi::PyBytes_Type) {
            let p = ffi::PyBytes_AS_STRING(safe).cast::<u8>();
            std::slice::from_raw_parts(p, ffi::PyBytes_Size(safe) as usize).iter().map(|&b| u32::from(b)).collect()
        } else {
            return None;
        };
        cps.extend(extra);
        Some(up::safe_table(cps))
    }
}

/// `quote(string, safe, encoding, errors)`; `space_to_plus`: `quote_plus`'s
/// `quote(string, safe + ' ', ...).replace(' ', '+')` when the input has a space.
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
            // `quote('')` returns its argument; `quote(b'')` returns ''.
            if exact_str(string) && ffi::PyUnicode_GET_LENGTH(string) == 0 {
                if quote_input(string, encoding, errors).is_some() {
                    return checked(g, spec, ffi::Py_NewRef(string), func, args, nargsf, kw);
                }
            } else if let Some(bs) = quote_input(string, encoding, errors)
                && bs.len() < 200_000
            {
                let has_space = bs.contains(&b' ');
                let extra = (plus && has_space).then_some(0x20);
                let table = if safe.is_null() {
                    // defaults: quote: safe='/', quote_plus: safe=''
                    Some(up::safe_table(extra.into_iter().chain((!plus).then_some(u32::from(b'/')))))
                } else {
                    safe_of(safe, extra)
                };
                if let Some(table) = table {
                    let mut out = Vec::new();
                    up::quote_bytes(&bs, &table, &mut out);
                    if plus && has_space {
                        for b in &mut out {
                            if *b == b' ' {
                                *b = b'+';
                            }
                        }
                    }
                    let r = str_from_ascii(&out);
                    return checked(g, spec, r, func, args, nargsf, kw);
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

/// `unquote(text)` for code points containing `%`: ASCII runs percent-decoded and decoded
/// with CPython's UTF-8 decoder (`errors`), other characters kept. `None`: the decoder
/// raised (strict) — nothing observable happened.
unsafe fn unquote_cps(v: &[u32], errors: &CStr) -> Option<Vec<u32>> {
    // SAFETY: attached thread.
    unsafe {
        let mut out = Vec::with_capacity(v.len());
        let mut i = 0;
        let mut bytes = Vec::new();
        while i < v.len() {
            if v[i] >= 128 {
                out.push(v[i]);
                i += 1;
                continue;
            }
            let s = i;
            while i < v.len() && v[i] < 128 {
                i += 1;
            }
            let run: Vec<u8> = v[s..i].iter().map(|&c| c as u8).collect();
            bytes.clear();
            up::unquote_bytes(&run, &mut bytes);
            if bytes.is_ascii() {
                out.extend(bytes.iter().map(|&b| u32::from(b)));
            } else {
                let d =
                    ffi::PyUnicode_DecodeUTF8(bytes.as_ptr().cast(), bytes.len() as ffi::Py_ssize_t, errors.as_ptr());
                if d.is_null() {
                    ffi::PyErr_Clear();
                    return None;
                }
                let cps = ucs4(d);
                ffi::Py_DECREF(d);
                out.extend(cps?);
            }
        }
        Some(out)
    }
}

/// `unquote_plus(s)` / `unquote(s)` of an exact str: the result (new reference; the
/// argument itself when CPython returns it). `None`: fall back.
unsafe fn unquote_obj(s: O, plus: bool, errors: &CStr) -> Option<O> {
    // SAFETY: exact str.
    unsafe {
        let v = ucs4(s)?;
        let has_plus = plus && v.contains(&0x2b);
        if !v.contains(&0x25) {
            if !has_plus {
                return Some(ffi::Py_NewRef(s));
            }
            let w: Vec<u32> = v.iter().map(|&c| if c == 0x2b { 0x20 } else { c }).collect();
            return Some(str_from_ucs4(&w));
        }
        let v = if has_plus { v.iter().map(|&c| if c == 0x2b { 0x20 } else { c }).collect() } else { v };
        Some(str_from_ucs4(&unquote_cps(&v, errors)?))
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
            if let Some(r) = unquote_obj(string, plus, errs) {
                return checked(g, spec, r, func, args, nargsf, kw);
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

/// The decoded `(name, value)` pairs of `parse_qsl(qs, ...)` for an exact str; `None`:
/// fall back (CPython raises, or an unsupported argument).
unsafe fn parse_pairs(args: *const O, nargsf: usize, kw: O, with_stacklevel: bool) -> Option<Vec<(O, O)>> {
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
        let sep: Vec<u32> = if sep.is_null() {
            vec![0x26]
        } else if exact_str(sep) {
            ucs4(sep)?
        } else {
            return None;
        };
        if sep.is_empty() {
            return None; // ValueError
        }
        let v = ucs4(qs)?;
        // qs.split(separator)
        let mut fields: Vec<(usize, usize)> = Vec::new();
        let mut start = 0;
        let mut i = 0;
        while i + sep.len() <= v.len() {
            if v[i..i + sep.len()] == sep[..] {
                fields.push((start, i));
                i += sep.len();
                start = i;
            } else {
                i += 1;
            }
        }
        fields.push((start, v.len()));
        if !maxf.is_null() && maxf != ffi::Py_None() {
            if !is_type(maxf, &raw mut ffi::PyLong_Type) {
                return None;
            }
            let m = int_value(maxf)?;
            if v.is_empty() {
                // `if not qs: return []` comes first
            } else if m < fields.len() as i64 {
                return None; // ValueError: Max number of fields exceeded
            }
        }
        let mut out: Vec<(O, O)> = Vec::new();
        if v.is_empty() {
            return Some(out);
        }
        let fail = |out: &mut Vec<(O, O)>| {
            for (a, b) in out.drain(..) {
                ffi::Py_DECREF(a);
                ffi::Py_DECREF(b);
            }
        };
        for (a, b) in fields {
            if a == b {
                continue; // empty field (not strict)
            }
            // name, has_eq, value = name_value.partition('=')
            let eq = v[a..b].iter().position(|&c| c == 0x3d).map(|p| a + p);
            let (na, nb, va, vb) = match eq {
                Some(e) => (a, e, e + 1, b),
                None => (a, b, b, b),
            };
            if vb == va && !keep {
                continue;
            }
            let name_s = ffi::PyUnicode_Substring(qs, na as ffi::Py_ssize_t, nb as ffi::Py_ssize_t);
            let value_s = ffi::PyUnicode_Substring(qs, va as ffi::Py_ssize_t, vb as ffi::Py_ssize_t);
            if name_s.is_null() || value_s.is_null() {
                ffi::Py_XDECREF(name_s);
                ffi::Py_XDECREF(value_s);
                ffi::PyErr_Clear();
                fail(&mut out);
                return None;
            }
            let name = unquote_obj(name_s, true, errs);
            let value = unquote_obj(value_s, true, errs);
            ffi::Py_DECREF(name_s);
            ffi::Py_DECREF(value_s);
            match (name, value) {
                (Some(n), Some(v)) => out.push((n, v)),
                (n, v) => {
                    if let Some(n) = n {
                        ffi::Py_DECREF(n);
                    }
                    if let Some(v) = v {
                        ffi::Py_DECREF(v);
                    }
                    fail(&mut out);
                    return None;
                }
            }
        }
        Some(out)
    }
}

unsafe extern "C" fn vc_parse_qsl(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_PARSE_QSL;
        if g.ready(func)
            && let Some(pairs) = parse_pairs(args, nargsf, kw, true)
        {
            let out = ffi::PyList_New(pairs.len() as ffi::Py_ssize_t);
            if out.is_null() {
                for (a, b) in pairs {
                    ffi::Py_DECREF(a);
                    ffi::Py_DECREF(b);
                }
                return out;
            }
            for (i, (a, b)) in pairs.into_iter().enumerate() {
                let t = ffi::PyTuple_New(2);
                if t.is_null() {
                    ffi::Py_DECREF(a);
                    ffi::Py_DECREF(b);
                    ffi::Py_DECREF(out);
                    return t;
                }
                ffi::PyTuple_SET_ITEM(t, 0, a);
                ffi::PyTuple_SET_ITEM(t, 1, b);
                ffi::PyList_SET_ITEM(out, i as ffi::Py_ssize_t, t);
            }
            return checked(g, &PARSE_QSL, out, func, args, nargsf, kw);
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

unsafe extern "C" fn vc_parse_qs(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_PARSE_QS;
        if g.ready(func)
            && let Some(pairs) = parse_pairs(args, nargsf, kw, false)
        {
            // for name, value in pairs: parsed_result.setdefault-like append
            let d = ffi::PyDict_New();
            let mut ok = !d.is_null();
            for (a, b) in pairs {
                if ok {
                    let mut cur: O = std::ptr::null_mut();
                    let r = ffi::PyDict_GetItemRef(d, a, &mut cur);
                    ok = match r {
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
                }
                ffi::Py_DECREF(a);
                ffi::Py_DECREF(b);
            }
            if !ok {
                ffi::Py_XDECREF(d);
                return std::ptr::null_mut();
            }
            return checked(g, &PARSE_QS, d, func, args, nargsf, kw);
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

/// `x.lstrip(C0)` (or `strip` when `both`) then removal of tab/CR/LF: the cleaned code
/// points, and whether the result is the argument itself (nothing removed).
fn clean(v: &[u32], both: bool) -> (Vec<u32>, bool) {
    let mut a = 0;
    while a < v.len() && up::is_c0_or_space(v[a]) {
        a += 1;
    }
    let mut b = v.len();
    if both {
        while b > a && up::is_c0_or_space(v[b - 1]) {
            b -= 1;
        }
    }
    let s = &v[a..b];
    let out: Vec<u32> = s.iter().copied().filter(|&c| !up::is_unsafe_url_char(c)).collect();
    let same = a == 0 && b == v.len() && out.len() == v.len();
    (out, same)
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
        let (cu, url_same) = clean(&ucs4(url)?, false);
        let split = up::urlsplit(&cu, allow).ok()?;
        // The cleaned url object: the argument itself when nothing was removed.
        let u = if url_same { ffi::Py_NewRef(url) } else { str_from_ucs4(&cu) };
        if u.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let sub = |r: (usize, usize)| ffi::PyUnicode_Substring(u, r.0 as ffi::Py_ssize_t, r.1 as ffi::Py_ssize_t);
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
            None => {
                let (cs, same) = clean(&ucs4(scheme).unwrap_or_default(), true);
                if same { ffi::Py_NewRef(scheme) } else { str_from_ucs4(&cs) }
            }
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

#[allow(clippy::too_many_arguments)]
fn prepare_fn<'py>(
    py: Python<'py>,
    cx: &mut Prepare<'py>,
    attr: &str,
    fp: accel::Fingerprints,
    globals_pinned: &[&str],
    builtins: &[&str],
    g: &'static Guard,
    kernel: ffi::vectorcallfunc,
) -> Result<(), Refusal> {
    prepare_common(py, cx)?;
    let func = cx.module_attr(attr)?;
    cx.fingerprint_code(&func, fp)?;
    let globals = cx.module()?.dict();
    for n in globals_pinned {
        cx.pin(&globals, n, &g.deps)?;
    }
    pin_globals(cx, &globals, builtins, &g.deps)?;
    g.capture(&func);
    cx.vectorcall(&func, kernel, &accel::FUNCTION_VECTORCALL)
}

macro_rules! target {
    ($spec:ident, $prep:ident, $name:literal, $attr:literal, $kernel:ident, $guard:ident, $fp:expr,
     globals: [$($gl:literal),*], builtins: [$($bl:literal),*]) => {
        fn $prep<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
            prepare_fn(py, cx, $attr, $fp, &[$($gl),*], &[$($bl),*], &$guard, $kernel)
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
            ..TargetSpec::DEFAULT
        };
    };
}

target!(QUOTE, prep_quote, "urllib.parse.quote", "quote", vc_quote, G_QUOTE,
    &[(3, 14, "63e2a4049e9eaf92"), (3, 15, "94de6386774e0075")],
    globals: ["quote_from_bytes", "_ALWAYS_SAFE_BYTES", "_byte_quoter_factory"],
    builtins: ["isinstance", "str", "bytes", "bytearray", "len", "map"]);
target!(QUOTE_PLUS, prep_quote_plus, "urllib.parse.quote_plus", "quote_plus", vc_quote_plus, G_QUOTE_PLUS,
    &[(3, 14, "a94b1533a164b8f0"), (3, 15, "a048317d78bf0841")],
    globals: ["quote", "quote_from_bytes", "_ALWAYS_SAFE_BYTES", "_byte_quoter_factory"],
    builtins: ["isinstance", "str", "bytes", "bytearray", "len", "map"]);
target!(UNQUOTE, prep_unquote, "urllib.parse.unquote", "unquote", vc_unquote, G_UNQUOTE,
    &[(3, 14, "aa04e1662d0123ca"), (3, 15, "1e55635c96370788")],
    globals: ["_unquote_impl", "_generate_unquoted_parts", "_asciire"],
    builtins: ["isinstance", "bytes", "bytearray"]);
target!(UNQUOTE_PLUS, prep_unquote_plus, "urllib.parse.unquote_plus", "unquote_plus", vc_unquote_plus, G_UNQUOTE_PLUS,
    &[(3, 14, "acbbc4556e4ffe7a"), (3, 15, "66c61fa58bfdacc7")],
    globals: ["unquote", "_unquote_impl", "_generate_unquoted_parts", "_asciire"],
    builtins: ["isinstance", "bytes", "bytearray"]);
target!(PARSE_QSL, prep_parse_qsl, "urllib.parse.parse_qsl", "parse_qsl", vc_parse_qsl, G_PARSE_QSL,
    &[(3, 14, "c4385431f4670861"), (3, 15, "6cf72a7ada63572a")],
    globals: ["unquote_plus", "unquote", "_unquote_impl", "_generate_unquoted_parts", "_asciire"],
    builtins: ["isinstance", "str"]);
target!(PARSE_QS, prep_parse_qs, "urllib.parse.parse_qs", "parse_qs", vc_parse_qs, G_PARSE_QS,
    &[(3, 14, "d9510d05a0e09245"), (3, 15, "4228046b5716b612")],
    globals: ["parse_qsl", "unquote_plus", "unquote", "_unquote_impl", "_generate_unquoted_parts", "_asciire"],
    builtins: ["isinstance", "str"]);
target!(URLSPLIT, prep_urlsplit, "urllib.parse._urlsplit", "_urlsplit", vc_urlsplit, G_URLSPLIT,
    &[(3, 14, "8d4cdc781ae3dbf2"), (3, 15, "d7c3aef3e7e67e8d")],
    globals: ["_WHATWG_C0_CONTROL_OR_SPACE", "_UNSAFE_URL_BYTES_TO_REMOVE", "scheme_chars", "_splitnetloc",
              "_check_bracketed_netloc", "_checknetloc"],
    builtins: ["bool", "len", "min"]);

pub static TARGETS: &[&TargetSpec] = &[&QUOTE, &QUOTE_PLUS, &UNQUOTE, &UNQUOTE_PLUS, &PARSE_QSL, &PARSE_QS, &URLSPLIT];
