//! Fingerprints of originals (docs/ACCELERATION.md §3.2): a target records, per validated
//! minor version, the fingerprint of every original it replaces or depends on; a mismatch
//! at install refuses the target.
//!
//! - Python functions (format v2): a hash of the canonical code object: unspecialized
//!   bytecode (`PyCode_GetCode`), argument counts, flags, names, local names and kinds,
//!   constants (recursively; frozensets order-independent, so independent of
//!   `PYTHONHASHSEED`) and the exception table, read from the code object's fields;
//!   independent of the file path and line numbers. Word-at-a-time hash, a few thousand
//!   instructions per function; cached per registry pass.
//! - C functions and methods: `ml_name`, `ml_flags` and `ml_doc`.
//!
//! Both are printed as 16 hex digits. `python -m rustique._takeover._dev fingerprint <dotted.name>...` prints them.
//!
//! Everything here runs C code only (attribute getters of code objects, `repr` of
//! constants): safe inside the `sys.modules` watcher.

use std::ffi::CStr;

use pyo3::exceptions::PyTypeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyCode, PyFunction};

/// Expected fingerprints of one original: `(major, minor, hex)` per validated version.
pub type Fingerprints = &'static [(u8, u8, &'static str)];

python_extern! {
    fn PyCode_GetCode(co: *mut ffi::PyObject) -> *mut ffi::PyObject;
}

/// Word-at-a-time hasher for code fingerprints (v2): ~1 Ir per byte.
struct Fast(u64);

impl Fast {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    fn word(&mut self, w: u64) {
        self.0 = (self.0 ^ w).wrapping_mul(Self::K).rotate_left(29);
    }
    fn bytes(&mut self, b: &[u8]) {
        self.word(b.len() as u64);
        let (words, rest) = b.as_chunks::<8>();
        for c in words {
            self.word(u64::from_le_bytes(*c));
        }
        if !rest.is_empty() {
            let mut w = [0u8; 8];
            w[..rest.len()].copy_from_slice(rest);
            self.word(u64::from_le_bytes(w));
        }
    }
    fn finish_u64(&self) -> u64 {
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        x
    }

    fn finish(&self) -> String {
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        // 16 lowercase hex digits without `core::fmt` (~10x cheaper; installs compute a
        // few dozen of these at startup).
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let digits: Vec<u8> = (0..16).rev().map(|i| HEX[((x >> (4 * i)) & 15) as usize]).collect();
        String::from_utf8(digits).expect("ASCII hex digits")
    }
}

/// A str's PEP 393 data (kind and raw code units).
///
/// # Safety
/// `s` is a live exact str.
unsafe fn str_bytes<'a>(s: *mut ffi::PyObject) -> (u64, &'a [u8]) {
    // SAFETY: per the caller.
    unsafe {
        let kind = ffi::PyUnicode_KIND(s) as u64;
        let len = ffi::PyUnicode_GET_LENGTH(s) as usize * kind as usize;
        (kind, std::slice::from_raw_parts(ffi::PyUnicode_DATA(s).cast::<u8>(), len))
    }
}

/// # Safety
/// `b` is a live exact bytes object.
unsafe fn bytes_of<'a>(b: *mut ffi::PyObject) -> &'a [u8] {
    // SAFETY: per the caller.
    unsafe { std::slice::from_raw_parts(ffi::PyBytes_AS_STRING(b).cast::<u8>(), ffi::Py_SIZE(b) as usize) }
}

/// # Safety
/// Attached thread; `t` a live tuple of exact str.
unsafe fn str_tuple(h: &mut Fast, t: *mut ffi::PyObject) -> PyResult<()> {
    // SAFETY: per the caller; type-checked items.
    unsafe {
        let n = ffi::PyTuple_GET_SIZE(t);
        h.word(n as u64);
        for i in 0..n {
            let s = ffi::PyTuple_GET_ITEM(t, i);
            if ffi::PyUnicode_CheckExact(s) == 0 {
                return Err(PyTypeError::new_err("non-str name in a code object"));
            }
            let (kind, b) = str_bytes(s);
            h.word(kind);
            h.bytes(b);
        }
    }
    Ok(())
}

/// # Safety
/// Attached thread; `c` a live constant.
unsafe fn const2(h: &mut Fast, c: *mut ffi::PyObject, depth: u32) -> PyResult<()> {
    if depth > 64 {
        return Err(PyTypeError::new_err("constants nested too deeply"));
    }
    // SAFETY: per the caller; exact type checks before layout reads.
    unsafe {
        let t = ffi::Py_TYPE(c);
        if c == ffi::Py_None() {
            h.word(1);
        } else if c == ffi::Py_True() {
            h.word(2);
        } else if c == ffi::Py_False() {
            h.word(3);
        } else if c == ffi::Py_Ellipsis() {
            h.word(4);
        } else if t == &raw mut ffi::PyUnicode_Type {
            let (kind, b) = str_bytes(c);
            h.word(5 | kind << 8);
            h.bytes(b);
        } else if t == &raw mut ffi::PyLong_Type {
            let mut overflow = 0;
            let v = ffi::PyLong_AsLongLongAndOverflow(c, &mut overflow);
            if overflow == 0 && !(v == -1 && !ffi::PyErr_Occurred().is_null()) {
                h.word(6);
                h.word(v as u64);
            } else {
                ffi::PyErr_Clear();
                h.word(7);
                let r = Bound::from_owned_ptr_or_err(Python::assume_attached(), ffi::PyObject_Repr(c))?;
                let (_, b) = str_bytes(r.as_ptr());
                h.bytes(b);
            }
        } else if t == &raw mut ffi::PyFloat_Type {
            h.word(8);
            h.word(ffi::PyFloat_AS_DOUBLE(c).to_bits());
        } else if t == &raw mut ffi::PyComplex_Type {
            let z = ffi::PyComplex_AsCComplex(c);
            h.word(9);
            h.word(z.real.to_bits());
            h.word(z.imag.to_bits());
        } else if t == &raw mut ffi::PyBytes_Type {
            h.word(10);
            h.bytes(bytes_of(c));
        } else if t == &raw mut ffi::PyTuple_Type {
            let n = ffi::PyTuple_GET_SIZE(c);
            h.word(11 | (n as u64) << 8);
            for i in 0..n {
                const2(h, ffi::PyTuple_GET_ITEM(c, i), depth + 1)?;
            }
        } else if t == &raw mut ffi::PyFrozenSet_Type {
            // Iteration order depends on string hashes (PYTHONHASHSEED): sort member digests.
            let mut parts = Vec::new();
            let it = Bound::from_owned_ptr_or_err(Python::assume_attached(), ffi::PyObject_GetIter(c))?;
            loop {
                let x = ffi::PyIter_Next(it.as_ptr());
                if x.is_null() {
                    if !ffi::PyErr_Occurred().is_null() {
                        return Err(PyErr::fetch(Python::assume_attached()));
                    }
                    break;
                }
                let mut sub = Fast(0);
                let r = const2(&mut sub, x, depth + 1);
                ffi::Py_DECREF(x);
                r?;
                parts.push(sub.0);
            }
            parts.sort_unstable();
            h.word(12 | (parts.len() as u64) << 8);
            for p in parts {
                h.word(p);
            }
        } else if t == &raw mut ffi::PyCode_Type {
            h.word(13);
            code2(h, c, depth + 1)?;
        } else {
            // Anything else (slices, ...): type name and repr.
            let py = Python::assume_attached();
            let obj = Bound::from_borrowed_ptr(py, c);
            h.word(14);
            h.bytes(obj.get_type().name()?.to_cow()?.as_bytes());
            h.bytes(obj.repr()?.to_cow()?.as_bytes());
        }
    }
    Ok(())
}

/// # Safety
/// Attached thread; `code` a live code object; `RawCode` layout calibrated.
unsafe fn code2(h: &mut Fast, code: *mut ffi::PyObject, depth: u32) -> PyResult<()> {
    // SAFETY: per the caller.
    unsafe {
        let raw = &*code.cast::<super::version::RawCode>();
        let co = Bound::from_owned_ptr_or_err(Python::assume_attached(), PyCode_GetCode(code))?;
        h.bytes(bytes_of(co.as_ptr()));
        h.word(raw.co_argcount as u32 as u64 | (raw.co_posonlyargcount as u32 as u64) << 32);
        h.word(raw.co_kwonlyargcount as u32 as u64 | (raw.co_flags as u32 as u64) << 32);
        str_tuple(h, raw.co_names)?;
        str_tuple(h, raw.co_localsplusnames)?;
        h.bytes(bytes_of(raw.co_localspluskinds));
        let consts = raw.co_consts;
        let n = ffi::PyTuple_GET_SIZE(consts);
        h.word(n as u64);
        for i in 0..n {
            const2(h, ffi::PyTuple_GET_ITEM(consts, i), depth)?;
        }
        h.bytes(bytes_of(raw.co_exceptiontable));
    }
    Ok(())
}

/// Canonical fingerprint of a code object (format v2): the unspecialized bytecode, argument
/// counts, flags, names, local names and kinds, constants (recursively; frozensets order-
/// independent) and the exception table, read from the code object's own fields; path,
/// line and position independent. A few thousand instructions per function.
pub fn code(code: &Bound<'_, PyAny>) -> PyResult<String> {
    if code.cast::<PyCode>().is_err() {
        return Err(PyTypeError::new_err("not a code object"));
    }
    if !super::version::code_layout_cached(code)? {
        return Err(PyTypeError::new_err("PyCodeObject layout differs"));
    }
    if let Some(hit) = cache_get(code.as_ptr() as usize) {
        return Ok(hit);
    }
    let mut h = Fast(0x2c0d_e2c0_de2c_0de2);
    // SAFETY: attached thread; a live code object; layout calibrated just above.
    unsafe { code2(&mut h, code.as_ptr(), 0)? };
    let out = h.finish();
    cache_put(code, &out);
    Ok(out)
}

/// Fingerprints computed in this process, keyed by code object address. Each entry holds a
/// strong reference to its code object, so an address is never reused while cached.
#[allow(clippy::type_complexity)]
static CACHE: std::sync::Mutex<
    Option<std::collections::HashMap<usize, (Py<PyAny>, String), foldhash::fast::FixedState>>,
> = std::sync::Mutex::new(None);

fn cache_get(code: usize) -> Option<String> {
    CACHE.lock().ok()?.as_ref()?.get(&code).map(|(_, f)| f.clone())
}

fn cache_put(code: &Bound<'_, PyAny>, fp: &str) {
    if let Ok(mut c) = CACHE.lock() {
        c.get_or_insert_with(Default::default).insert(code.as_ptr() as usize, (code.clone().unbind(), fp.to_owned()));
    }
}

/// Forget cached fingerprints (diagnostics only; the exit hook).
pub(crate) fn clear_cache() {
    let old = CACHE.lock().ok().and_then(|mut c| c.take());
    drop(old);
    let old3 = CACHE3.lock().ok().and_then(|mut c| c.take());
    drop(old3);
}

/// Canonical fingerprint of a pure-Python function's code.
pub fn function(func: &Bound<'_, PyAny>) -> PyResult<String> {
    let f = func.cast_exact::<PyFunction>().map_err(|_| PyTypeError::new_err("not a pure-Python function"))?;
    // SAFETY: an exact function object: its code is a live code object.
    let code = unsafe { Bound::from_borrowed_ptr(f.py(), (*f.as_ptr().cast::<ffi::PyFunctionObject>()).func_code) };
    self::code(&code)
}

/// Fingerprint of a `PyMethodDef` (format v2): `ml_name`, `ml_flags`, `ml_doc`.
///
/// # Safety
/// `def` is a live method table entry.
pub unsafe fn method_def(def: *const ffi::PyMethodDef) -> String {
    let mut h = Fast(0x0c0f_f1e5_c0ff_1e50);
    // SAFETY: per the caller; NUL-terminated strings or NULL.
    unsafe {
        let name = if (*def).ml_name.is_null() { &[][..] } else { CStr::from_ptr((*def).ml_name).to_bytes() };
        h.bytes(name);
        h.word((*def).ml_flags as u32 as u64);
        let doc = if (*def).ml_doc.is_null() { &[][..] } else { CStr::from_ptr((*def).ml_doc).to_bytes() };
        h.bytes(doc);
    }
    h.finish()
}

/// Fingerprint of a builtin function / method descriptor (C original).
pub fn c_object(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    // SAFETY: attached thread.
    let def = unsafe { super::site::method_def(obj.as_ptr()) }
        .ok_or_else(|| PyTypeError::new_err("not a builtin function or method descriptor"))?;
    // SAFETY: a live method def (owned by the object's type or module).
    Ok(unsafe { method_def(def) })
}

/// Fingerprint of any original: Python function → code hash; C function/method → def hash.
pub fn any(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    if obj.cast_exact::<PyFunction>().is_ok() {
        function(obj)
    } else if obj.cast::<PyCode>().is_ok() {
        code(obj)
    } else {
        c_object(obj)
    }
}

/// The expected fingerprint for the running `(major, minor)`, if recorded (format v2,
/// inline in a target's code; superseded by the v3 data, [`accepted`]).
pub fn expected_for(expected: Fingerprints, major: u8, minor: u8) -> Option<&'static str> {
    expected.iter().find(|(a, b, _)| *a == major && *b == minor).map(|(_, _, h)| *h)
}

// ---------------------------------------------------------------------------------------
// Format v3 (DECISIONS §10.5, docs/atlas/w2-floors.md §5.3)
// ---------------------------------------------------------------------------------------
//
// v2 minus docstrings (`co_consts[0]` when `CO_HAS_DOCSTRING`; `ml_doc` of C originals), and
// with the exception table canonicalized: the handler (target, depth, lasti) of each
// instruction that can raise a *synchronous* exception (opcode metadata `HAS_ERROR` /
// `HAS_ERROR_NO_POP`), instead of the table's byte encoding. Two code objects with equal v3
// send every synchronous exception to the same handler; asynchronous ones (signals,
// `PyThreadState_SetAsyncExc` at `JUMP_BACKWARD`'s eval breaker) are outside the contract
// (ACCELERATION §5.8). The accepted v3 hashes of every original live in `fingerprints.txt`
// (per target, original and minor; a set: a patch release whose code changed in a way the
// kernel still covers adds its hash after review), with the list of recorded patch releases
// (`benches/accel/record_patch.py` runs the conformance suites first).

const CO_HAS_DOCSTRING: i32 = 0x0400_0000;

/// Raising opcodes, cache sizes and `EXTENDED_ARG` of the running minor.
fn opcode_tables() -> Option<(&'static [u64; 4], &'static [u8; 256])> {
    use super::opcodes::*;
    match super::version::running() {
        (3, 14) => Some((&RAISES_3_14, &CACHES_3_14)),
        (3, 15) => Some((&RAISES_3_15, &CACHES_3_15)),
        _ => None,
    }
}

/// One exception-table varint (6 bits per byte, bit 6: more).
fn varint(t: &[u8], i: &mut usize) -> Option<u32> {
    let mut b = *t.get(*i)?;
    *i += 1;
    let mut v = u32::from(b & 63);
    while b & 64 != 0 {
        b = *t.get(*i)?;
        *i += 1;
        v = (v << 6) | u32::from(b & 63);
    }
    Some(v)
}

/// The canonical exception table: for each raising instruction (code-unit index), its
/// handler or none.
fn canonical_exceptions(h: &mut Fast, code: &[u8], table: &[u8]) -> PyResult<()> {
    let (raises, caches) =
        opcode_tables().ok_or_else(|| PyTypeError::new_err("no opcode tables for this CPython minor"))?;
    let mut entries: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut i = 0;
    while i < table.len() {
        let parsed = (|| {
            Some((varint(table, &mut i)?, varint(table, &mut i)?, varint(table, &mut i)?, varint(table, &mut i)?))
        })();
        let Some((start, len, target, dl)) = parsed else {
            return Err(PyTypeError::new_err("malformed exception table"));
        };
        entries.push((start, start + len, target, dl));
    }
    let units = code.len() / 2;
    let mut n = 0u64;
    let mut u = 0usize;
    while u < units {
        let op = code[2 * u];
        if raises[usize::from(op) / 64] >> (op % 64) & 1 != 0 {
            let at = u as u32;
            match entries.iter().find(|e| e.0 <= at && at < e.1) {
                Some(&(_, _, target, dl)) => {
                    h.word(u64::from(at) | 1 << 32);
                    h.word(u64::from(target) | u64::from(dl) << 32);
                }
                None => h.word(u64::from(at)),
            }
            n += 1;
        }
        u += 1 + usize::from(caches[usize::from(op)]);
    }
    h.word(n);
    Ok(())
}

/// # Safety
/// Attached thread; `code` a live code object; `RawCode` layout calibrated.
unsafe fn code3(h: &mut Fast, code: *mut ffi::PyObject, depth: u32) -> PyResult<()> {
    // SAFETY: per the caller.
    unsafe {
        let raw = &*code.cast::<super::version::RawCode>();
        let co = Bound::from_owned_ptr_or_err(Python::assume_attached(), PyCode_GetCode(code))?;
        let bytecode = bytes_of(co.as_ptr());
        h.bytes(bytecode);
        h.word(raw.co_argcount as u32 as u64 | (raw.co_posonlyargcount as u32 as u64) << 32);
        h.word(raw.co_kwonlyargcount as u32 as u64 | (raw.co_flags as u32 as u64) << 32);
        str_tuple(h, raw.co_names)?;
        str_tuple(h, raw.co_localsplusnames)?;
        h.bytes(bytes_of(raw.co_localspluskinds));
        let consts = raw.co_consts;
        let n = ffi::PyTuple_GET_SIZE(consts);
        h.word(n as u64);
        let skip_doc = raw.co_flags & CO_HAS_DOCSTRING != 0;
        for i in 0..n {
            if i == 0 && skip_doc {
                h.word(15); // a docstring: its text is not part of the behaviour
                continue;
            }
            let c = ffi::PyTuple_GET_ITEM(consts, i);
            if ffi::Py_TYPE(c) == &raw mut ffi::PyCode_Type {
                h.word(13);
                code3(h, c, depth + 1)?;
            } else {
                const2(h, c, depth)?;
            }
        }
        canonical_exceptions(h, bytecode, bytes_of(raw.co_exceptiontable))?;
    }
    Ok(())
}

/// Canonical fingerprint of a code object, format v3 (cached per code object).
pub fn code_v3(code: &Bound<'_, PyAny>) -> PyResult<u64> {
    if code.cast::<PyCode>().is_err() {
        return Err(PyTypeError::new_err("not a code object"));
    }
    if !super::version::code_layout_cached(code)? {
        return Err(PyTypeError::new_err("PyCodeObject layout differs"));
    }
    if let Some(hit) = cache3_get(code.as_ptr() as usize) {
        return Ok(hit);
    }
    let mut h = Fast(0x3c0d_e3c0_de3c_0de3);
    // SAFETY: attached thread; a live code object; layout calibrated just above.
    unsafe { code3(&mut h, code.as_ptr(), 0)? };
    let out = h.finish_u64();
    cache3_put(code, out);
    Ok(out)
}

/// Format v3 of a C original: `ml_name` and `ml_flags` (no `ml_doc`).
///
/// # Safety
/// `def` is a live method table entry.
pub unsafe fn method_def_v3(def: *const ffi::PyMethodDef) -> u64 {
    let mut h = Fast(0x3c0f_f3e5_c3ff_3e53);
    // SAFETY: per the caller; NUL-terminated strings or NULL.
    unsafe {
        let name = if (*def).ml_name.is_null() { &[][..] } else { CStr::from_ptr((*def).ml_name).to_bytes() };
        h.bytes(name);
        h.word((*def).ml_flags as u32 as u64);
    }
    h.finish_u64()
}

/// v3 of any original (function, code object, C function or method descriptor) and its
/// stable id (`module.qualname` of a function, `qualname` of a code object, `Type.name` /
/// `module.name` of a C original).
pub fn any_v3(obj: &Bound<'_, PyAny>) -> PyResult<(String, u64)> {
    let py = obj.py();
    if obj.cast_exact::<PyFunction>().is_ok() {
        // SAFETY: an exact function object: live code and module fields.
        let (code, module) = unsafe {
            let f = obj.as_ptr().cast::<ffi::PyFunctionObject>();
            (Bound::from_borrowed_ptr(py, (*f).func_code), (*f).func_module)
        };
        let q: String = code.getattr(pyo3::intern!(py, "co_qualname"))?.extract()?;
        let m = if module.is_null() {
            None
        } else {
            // SAFETY: a live object (the function's module name).
            unsafe { Bound::from_borrowed_ptr(py, module) }.extract::<String>().ok()
        };
        let id = match m {
            Some(m) => format!("{m}.{q}"),
            None => q,
        };
        return Ok((id, code_v3(&code)?));
    }
    if obj.cast::<PyCode>().is_ok() {
        let q: String = obj.getattr(pyo3::intern!(py, "co_qualname"))?.extract()?;
        return Ok((q, code_v3(obj)?));
    }
    // SAFETY: attached thread.
    let def = unsafe { super::site::method_def(obj.as_ptr()) }
        .ok_or_else(|| PyTypeError::new_err("not a function, code object or C function"))?;
    // SAFETY: a live method def.
    Ok((super::prepare::c_name_of(obj, def), unsafe { method_def_v3(def) }))
}

/// 16 lowercase hex digits.
pub fn hex(x: u64) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digits: Vec<u8> = (0..16).rev().map(|i| HEX[((x >> (4 * i)) & 15) as usize]).collect();
    String::from_utf8(digits).expect("ASCII hex digits")
}

#[allow(clippy::type_complexity)]
static CACHE3: std::sync::Mutex<
    Option<std::collections::HashMap<usize, (Py<PyAny>, u64), foldhash::fast::FixedState>>,
> = std::sync::Mutex::new(None);

fn cache3_get(code: usize) -> Option<u64> {
    CACHE3.lock().ok()?.as_ref()?.get(&code).map(|(_, f)| *f)
}

fn cache3_put(code: &Bound<'_, PyAny>, fp: u64) {
    let old = CACHE3.lock().ok().and_then(|mut c| {
        c.get_or_insert_with(Default::default).insert(code.as_ptr() as usize, (code.clone().unbind(), fp))
    });
    drop(old);
}

/// The recorded fingerprint data (`fingerprints.txt`).
pub struct Data {
    /// Recorded patch releases (`"3.15.1"`, `"3.15.1t"`).
    pub patches: Vec<String>,
    /// `(target, original id, minor)` → accepted v3 hashes.
    pub accepted: std::collections::HashMap<(String, String, u8), Vec<u64>, foldhash::fast::FixedState>,
}

/// The recorded data, parsed once from the file compiled into the extension.
pub fn data() -> &'static Data {
    static DATA: std::sync::OnceLock<Data> = std::sync::OnceLock::new();
    DATA.get_or_init(|| parse(include_str!("fingerprints.txt")))
}

/// Parse the data file: `patch <version>...` lines and `<major.minor> <target> <id> <hex>...`
/// lines; `#` starts a comment.
pub fn parse(text: &str) -> Data {
    let mut d = Data { patches: Vec::new(), accepted: Default::default() };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut it = line.split_whitespace();
        match it.next() {
            None => {}
            Some("patch") => d.patches.extend(it.map(str::to_owned)),
            Some(minor) => {
                let Some(m) = minor.strip_prefix("3.").and_then(|m| m.parse::<u8>().ok()) else { continue };
                let (Some(target), Some(id)) = (it.next(), it.next()) else { continue };
                let hashes = it.filter_map(|h| u64::from_str_radix(h, 16).ok());
                d.accepted.entry((target.to_owned(), id.to_owned(), m)).or_default().extend(hashes);
            }
        }
    }
    d
}

/// The accepted v3 hashes of `target`'s original `id` on the running minor, if recorded.
pub fn accepted(target: &str, id: &str) -> Option<&'static [u64]> {
    let (_, minor) = super::version::running();
    data().accepted.get(&(target.to_owned(), id.to_owned(), minor)).map(Vec::as_slice)
}

/// For targets that check an original themselves (outside `Prepare`): is `obj` an original
/// recorded for `target` (v3 data), or, when the data has no entry for it, does its v2
/// fingerprint equal the inline `expected` one?
pub fn check(target: &str, obj: &Bound<'_, PyAny>, expected: Fingerprints) -> bool {
    match any_v3(obj) {
        Ok((id, v3)) => match accepted(target, &id) {
            Some(set) => set.contains(&v3),
            None => {
                let (major, minor) = super::version::running();
                expected_for(expected, major, minor).is_some_and(|w| any(obj).is_ok_and(|f| f == w))
            }
        },
        Err(_) => false,
    }
}
