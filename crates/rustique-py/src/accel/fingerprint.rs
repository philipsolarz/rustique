//! Fingerprints of originals (docs/ACCELERATION.md §3.2): a target records, per validated
//! minor version, the fingerprint of every original it replaces or depends on; a mismatch
//! at install refuses the target.
//!
//! - Python functions: a hash of the canonical code object: bytecode (`co_code`, which is
//!   the unspecialized form), names, varnames, freevars, cellvars, argument counts, flags,
//!   constants (recursively; frozensets sorted, so independent of `PYTHONHASHSEED`) and the
//!   exception table. Independent of the file path and line numbers.
//! - C functions and methods: `ml_name`, `ml_flags` and `ml_doc`.
//!
//! Both are FNV-1a 64 over a tagged, length-prefixed serialization, printed as 16 hex
//! digits. `python -m rustique._takeover._dev fingerprint <dotted.name>...` prints them.
//!
//! Everything here runs C code only (attribute getters of code objects, `repr` of
//! constants): safe inside the `sys.modules` watcher.

use std::ffi::CStr;

use pyo3::exceptions::PyTypeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyCode, PyFrozenSet, PyFunction, PyString, PyTuple};

/// Expected fingerprints of one original: `(major, minor, hex)` per validated version.
pub type Fingerprints = &'static [(u8, u8, &'static str)];

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn tag(&mut self, t: &str) {
        self.bytes(&(t.len() as u32).to_le_bytes());
        self.bytes(t.as_bytes());
    }
    fn field(&mut self, b: &[u8]) {
        self.bytes(&(b.len() as u64).to_le_bytes());
        self.bytes(b);
    }
    fn int(&mut self, v: i64) {
        self.bytes(&v.to_le_bytes());
    }
    fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

fn canon_strs(h: &mut Fnv, tag: &str, t: &Bound<'_, PyAny>) -> PyResult<()> {
    h.tag(tag);
    let t = t.cast::<PyTuple>()?;
    h.int(t.len() as i64);
    for s in t.iter() {
        h.field(s.cast::<PyString>()?.to_cow()?.as_bytes());
    }
    Ok(())
}

/// Serialize a constant into its own hasher state (so frozenset members can be sorted).
fn const_digest(c: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    let mut h = Fnv::new();
    canon_const(&mut h, c)?;
    Ok(h.0.to_le_bytes().to_vec())
}

fn canon_const(h: &mut Fnv, c: &Bound<'_, PyAny>) -> PyResult<()> {
    if let Ok(code) = c.cast::<PyCode>() {
        return canon_code(h, code.as_any());
    }
    if let Ok(t) = c.cast_exact::<PyTuple>() {
        h.tag("tuple");
        h.int(t.len() as i64);
        for x in t.iter() {
            canon_const(h, &x)?;
        }
        return Ok(());
    }
    if let Ok(fs) = c.cast_exact::<PyFrozenSet>() {
        h.tag("frozenset");
        let mut parts = fs.iter().map(|x| const_digest(&x)).collect::<PyResult<Vec<_>>>()?;
        parts.sort();
        h.int(parts.len() as i64);
        for p in parts {
            h.bytes(&p);
        }
        return Ok(());
    }
    if let Ok(b) = c.cast_exact::<PyBytes>() {
        h.tag("bytes");
        h.field(b.as_bytes());
        return Ok(());
    }
    // None, bool, int, float, complex, str, Ellipsis, slice, ...: type name and repr.
    let ty = c.get_type();
    h.tag(&ty.name()?.to_cow()?);
    h.field(c.repr()?.to_cow()?.as_bytes());
    Ok(())
}

fn canon_code(h: &mut Fnv, code: &Bound<'_, PyAny>) -> PyResult<()> {
    h.tag("code");
    h.field(code.getattr("co_code")?.cast::<PyBytes>()?.as_bytes());
    canon_strs(h, "names", &code.getattr("co_names")?)?;
    canon_strs(h, "varnames", &code.getattr("co_varnames")?)?;
    canon_strs(h, "freevars", &code.getattr("co_freevars")?)?;
    canon_strs(h, "cellvars", &code.getattr("co_cellvars")?)?;
    for a in ["co_argcount", "co_posonlyargcount", "co_kwonlyargcount", "co_flags"] {
        h.int(code.getattr(a)?.extract::<i64>()?);
    }
    let consts = code.getattr("co_consts")?;
    let consts = consts.cast::<PyTuple>()?;
    h.tag("consts");
    h.int(consts.len() as i64);
    for c in consts.iter() {
        canon_const(h, &c)?;
    }
    h.field(code.getattr("co_exceptiontable")?.cast::<PyBytes>()?.as_bytes());
    Ok(())
}

/// Canonical fingerprint of a code object.
pub fn code(code: &Bound<'_, PyAny>) -> PyResult<String> {
    if code.cast::<PyCode>().is_err() {
        return Err(PyTypeError::new_err("not a code object"));
    }
    let mut h = Fnv::new();
    canon_code(&mut h, code)?;
    Ok(h.hex())
}

/// Canonical fingerprint of a pure-Python function's code.
pub fn function(func: &Bound<'_, PyAny>) -> PyResult<String> {
    let f = func.cast_exact::<PyFunction>().map_err(|_| PyTypeError::new_err("not a pure-Python function"))?;
    // `__code__` of an exact function: the C getter, no Python code.
    code(&f.getattr("__code__")?)
}

/// Fingerprint of a `PyMethodDef`: `ml_name`, `ml_flags`, `ml_doc`.
///
/// # Safety
/// `def` is a live method table entry.
pub unsafe fn method_def(def: *const ffi::PyMethodDef) -> String {
    let mut h = Fnv::new();
    h.tag("cfunc");
    // SAFETY: per the caller; NUL-terminated strings or NULL.
    unsafe {
        let name = if (*def).ml_name.is_null() { &[][..] } else { CStr::from_ptr((*def).ml_name).to_bytes() };
        h.field(name);
        h.int(i64::from((*def).ml_flags));
        let doc = if (*def).ml_doc.is_null() { &[][..] } else { CStr::from_ptr((*def).ml_doc).to_bytes() };
        h.field(doc);
    }
    h.hex()
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

/// The expected fingerprint for the running `(major, minor)`, if recorded.
pub fn expected_for(expected: Fingerprints, major: u8, minor: u8) -> Option<&'static str> {
    expected.iter().find(|(a, b, _)| *a == major && *b == minor).map(|(_, _, h)| *h)
}
