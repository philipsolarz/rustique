//! Target group `json`: `json.dumps` and `json.loads` (docs/targets/json.md).
//!
//! Both are vectorcall sites on the pure-Python functions of the `json` package (installed
//! when `json` finishes importing). Their fast paths run no Python code and have no side
//! effects; every dependency CPython reads at call time is pinned or checked per call.

pub(crate) mod decode;
pub(crate) mod dumps;
pub(crate) mod encode;
pub(crate) mod loads;
pub(crate) mod pystr;
mod selftest;

use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};

use pyo3::ffi;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyType};

use crate::accel::{self, Fingerprints, Prepare, Refusal, TargetSpec};
use crate::refuse;

/// Canonical code fingerprints (`rustique._rustique._accel.fingerprint(f)`) of the Python
/// functions a target replaces or runs through, per validated minor version.
mod fp {
    use super::Fingerprints;
    pub const DUMPS: Fingerprints = &[(3, 14, "44f548cb97c25ed6"), (3, 15, "299b65261fd6e670")];
    pub const ENCODE: Fingerprints = &[(3, 14, "d684320438e5c058"), (3, 15, "2b56d3d22c5748f6")];
    pub const ITERENCODE: Fingerprints = &[(3, 14, "2480e0d236d7af8d"), (3, 15, "8991479886531852")];
    pub const ENCODER_INIT: Fingerprints = &[(3, 14, "b84cd944059c35b8"), (3, 15, "d85a7266d03bfce8")];
    pub const ENCODE_BASESTRING_ASCII: Fingerprints = &[(3, 14, "448b8d466edf28ad"), (3, 15, "6b1efe01cd0aabf3")];
    pub const LOADS: Fingerprints = &[(3, 14, "14eac0591dddcd6e"), (3, 15, "102b17fe130dee91")];
    pub const DECODE: Fingerprints = &[(3, 14, "9fd7e2cf9cb48497"), (3, 15, "6d49e790e3821789")];
    pub const RAW_DECODE: Fingerprints = &[(3, 14, "014b59b4541eabd3"), (3, 15, "947d9475a7d318ba")];
    pub const DETECT_ENCODING: Fingerprints = &[(3, 14, "c129d6cf5fd598e5"), (3, 15, "781a525e9a40c4f8")];
    pub const ENCODE_BASESTRING: Fingerprints = &[(3, 14, "9b3603c10dd95cd7"), (3, 15, "a14341830caa1909")];
}

/// A Python function the target runs through (`JSONEncoder.encode`, `JSONDecoder.decode`,
/// ...): its class dict entry is pinned, its code and defaults are checked per call
/// (assigning `__code__`/`__defaults__` notifies no watcher). Strong references for the
/// process lifetime.
pub(crate) struct FnCheck {
    func: AtomicPtr<ffi::PyObject>,
    code: AtomicPtr<ffi::PyObject>,
    defaults: AtomicPtr<ffi::PyObject>,
}

impl FnCheck {
    pub(crate) const fn new() -> Self {
        FnCheck {
            func: AtomicPtr::new(std::ptr::null_mut()),
            code: AtomicPtr::new(std::ptr::null_mut()),
            defaults: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    fn set(&self, func: &Bound<'_, PyAny>) {
        let f = func.as_ptr().cast::<ffi::PyFunctionObject>();
        // SAFETY: an exact function (fingerprinted by the caller); new strong references.
        unsafe {
            self.func.store(ffi::Py_NewRef(func.as_ptr()), Relaxed);
            self.code.store(ffi::Py_NewRef((*f).func_code), Relaxed);
            self.defaults.store(ffi::Py_XNewRef((*f).func_defaults), Relaxed);
        }
    }

    #[inline(always)]
    pub(crate) fn ok(&self) -> bool {
        let f = self.func.load(Relaxed).cast::<ffi::PyFunctionObject>();
        // SAFETY: set before the kernel is installed; kept alive.
        unsafe { (*f).func_code == self.code.load(Relaxed) && (*f).func_defaults == self.defaults.load(Relaxed) }
    }
}

/// `obj.__dict__` as a dict (materializes an instance's managed dict; C code only).
fn instance_dict<'py>(obj: &Bound<'py, PyAny>) -> Result<Bound<'py, PyDict>, Refusal> {
    obj.getattr("__dict__")?.cast_into::<PyDict>().map_err(|_| refuse!(Deps: "__dict__ is not a dict"))
}

fn prepare_dumps(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    use dumps::{CODE, DEFAULT_ENCODER, DEPS, ENCODER_TYPE, FUNC, KW, KW_NAMES, KWDEFAULTS};
    if !crate::targets::float::calibrate_ascii(py) || !pystr::calibrate(py) {
        return Err(refuse!(Layout: "str layout not calibrated"));
    }
    let json = cx.module()?.clone();
    let g = json.dict();
    let func = cx.module_attr("dumps")?;
    cx.fingerprint_code(&func, fp::DUMPS)?;

    // `json.encoder` and the C accelerator it must be using.
    let enc_mod = cx.loaded_module("json.encoder")?;
    let eg = enc_mod.dict();
    let c_json = cx.loaded_module("_json")?;
    for (name, c_name, fpr) in [
        ("encode_basestring_ascii", "encode_basestring_ascii", fp::ENCODE_BASESTRING_ASCII),
        ("encode_basestring", "encode_basestring", fp::ENCODE_BASESTRING),
    ] {
        let v = cx.get(&eg, name)?;
        if !v.is(&cx.get(&c_json.dict(), c_name)?) {
            return Err(refuse!(Deps: "json.encoder.{name} is not _json.{c_name}"));
        }
        cx.fingerprint_c(&v, fpr)?;
    }
    let make = cx.get(&eg, "c_make_encoder")?;
    if !make.is(&cx.get(&c_json.dict(), "make_encoder")?) {
        return Err(refuse!(Deps: "json.encoder.c_make_encoder is not _json.make_encoder"));
    }
    cx.pin_dict(&eg, &DEPS)?;
    // Globals `encode`/`iterencode` read (shadowing builtins included).
    // Globals `encode`/`iterencode` read: one snapshot pin of `json.encoder`'s namespace
    // (nothing adds to it at run time) plus the builtins they fall back to.
    let builtins = cx.builtins_of(&eg)?;
    for name in ["isinstance", "str", "list", "tuple", "float"] {
        cx.pin(&builtins, name, &DEPS)?;
    }

    // The encoder class and the cached default instance.
    let cls = cx.pin_global(&g, "JSONEncoder", &DEPS)?;
    let cls = cls.cast_into::<PyType>().map_err(|_| refuse!(Deps: "json.JSONEncoder is not a class"))?;
    if !cls.is(&cx.get(&eg, "JSONEncoder")?) {
        return Err(refuse!(Deps: "json.JSONEncoder is not json.encoder.JSONEncoder"));
    }
    let encode_fn = cx.type_attr(&cls, "encode")?;
    let iterencode_fn = cx.type_attr(&cls, "iterencode")?;
    cx.fingerprint_code(&encode_fn, fp::ENCODE)?;
    cx.fingerprint_code(&iterencode_fn, fp::ITERENCODE)?;
    cx.fingerprint_code(&cx.type_attr(&cls, "__init__")?, fp::ENCODER_INIT)?;
    for (name, want) in [("item_separator", ", "), ("key_separator", ": ")] {
        let v = cx.type_attr(&cls, name)?;
        if v.cast::<PyString>().ok().and_then(|s| s.to_str().ok().map(|s| s == want)) != Some(true) {
            return Err(refuse!(Deps: "JSONEncoder.{name} is not {want:?}"));
        }
    }
    // The class's own dict, whole (methods, separators, no shadowing descriptors), and
    // its MRO's other member (`object`, immutable).
    cx.pin_dict(&crate::accel::prepare::type_dict(&cls)?, &DEPS)?;
    let enc = cx.pin_global(&g, "_default_encoder", &DEPS)?;
    if !enc.get_type().is(&cls) {
        return Err(refuse!(Deps: "json._default_encoder is not a JSONEncoder"));
    }
    let ed = instance_dict(&enc)?;
    let expect = [
        ("skipkeys", false),
        ("ensure_ascii", true),
        ("check_circular", true),
        ("allow_nan", true),
        ("sort_keys", false),
    ];
    if ed.len() != expect.len() + 1 {
        return Err(refuse!(Deps: "json._default_encoder has unexpected attributes"));
    }
    for (name, want) in expect {
        if !cx.get(&ed, name)?.is(pyo3::types::PyBool::new(py, want)) {
            return Err(refuse!(Deps: "json._default_encoder.{name} is not {want}"));
        }
    }
    if !cx.get(&ed, "indent")?.is_none() {
        return Err(refuse!(Deps: "json._default_encoder.indent is not None"));
    }
    cx.pin_dict(&ed, &DEPS)?;

    // dumps' keyword defaults (a dict, pinned whole) and the function itself.
    let kwd = func.getattr("__kwdefaults__")?.cast_into::<PyDict>().map_err(|_| refuse!(Deps: "no __kwdefaults__"))?;
    for (name, want) in [
        ("skipkeys", Some(false)),
        ("ensure_ascii", Some(true)),
        ("check_circular", Some(true)),
        ("allow_nan", Some(true)),
        ("sort_keys", Some(false)),
        ("cls", None),
        ("indent", None),
        ("separators", None),
        ("default", None),
    ] {
        let v = cx.get(&kwd, name)?;
        let ok = match want {
            Some(b) => v.is(pyo3::types::PyBool::new(py, b)),
            None => v.is_none(),
        };
        if !ok {
            return Err(refuse!(Deps: "json.dumps default {name} differs"));
        }
    }
    if kwd.len() != KW_NAMES.len() {
        return Err(refuse!(Deps: "json.dumps keyword defaults differ"));
    }
    cx.pin_dict(&kwd, &DEPS)?;

    for (cell, name) in KW.iter().zip(KW_NAMES) {
        cell.store(PyString::intern(py, name).into_ptr(), Relaxed);
    }
    // Strong references for the process lifetime (the kernel compares identities).
    FUNC.store(func.clone().into_ptr(), Relaxed);
    CODE.store(func.getattr("__code__")?.into_ptr(), Relaxed);
    KWDEFAULTS.store(kwd.into_any().into_ptr(), Relaxed);
    DEFAULT_ENCODER.store(enc.into_ptr(), Relaxed);
    ENCODER_TYPE.store(cls.clone().into_any().into_ptr(), Relaxed);
    dumps::ENCODE.set(&encode_fn);
    dumps::ITERENCODE.set(&iterencode_fn);

    cx.vectorcall(&func, dumps::kernel, &accel::FUNCTION_VECTORCALL)?;

    // Self-test: the encoder's text for probe documents equals CPython's.
    cx.self_test("json.dumps probes", selftest::dumps_selftest)
}

fn prepare_loads(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    use loads::{CODE, DECODER_TYPE, DEFAULT_DECODER, DEPS, FUNC, KW, KW_COUNT, KW_NAMES, KWDEFAULTS};
    if !crate::targets::float::calibrate_ascii(py) || !pystr::calibrate(py) {
        return Err(refuse!(Layout: "str layout not calibrated"));
    }
    let g = cx.module()?.dict();
    let func = cx.module_attr("loads")?;
    cx.fingerprint_code(&func, fp::LOADS)?;
    for name in ["isinstance", "str", "bytes", "bytearray", "len"] {
        cx.pin_global(&g, name, &DEPS)?;
    }
    // `detect_encoding` (bytes input) and the BOM constants it reads.
    let detect = cx.pin_global(&g, "detect_encoding", &DEPS)?;
    cx.fingerprint_code(&detect, fp::DETECT_ENCODING)?;
    let codecs = cx.pin_global(&g, "codecs", &DEPS)?;
    let cd = codecs.cast_into::<PyModule>().map_err(|_| refuse!(Deps: "json.codecs is not a module"))?.dict();
    for (name, want) in [
        ("BOM_UTF32_BE", &b"\x00\x00\xfe\xff"[..]),
        ("BOM_UTF32_LE", &b"\xff\xfe\x00\x00"[..]),
        ("BOM_UTF16_BE", &b"\xfe\xff"[..]),
        ("BOM_UTF16_LE", &b"\xff\xfe"[..]),
        ("BOM_UTF8", &b"\xef\xbb\xbf"[..]),
    ] {
        if cx.get(&cd, name)?.extract::<Vec<u8>>().ok().as_deref() != Some(want) {
            return Err(refuse!(Deps: "codecs.{name} differs"));
        }
        cx.pin(&cd, name, &DEPS)?;
    }

    // The decoder class, its methods and the cached default instance with its C scanner.
    let dec_mod = cx.loaded_module("json.decoder")?;
    let dg = dec_mod.dict();
    // `json.decoder`'s namespace (snapshot) and the builtin `len` decode reads.
    cx.pin_dict(&dg, &DEPS)?;
    cx.pin(&cx.builtins_of(&dg)?, "len", &DEPS)?;
    let cls = cx.get(&dg, "JSONDecoder")?;
    let cls = cls.cast_into::<PyType>().map_err(|_| refuse!(Deps: "JSONDecoder is not a class"))?;
    let decode_fn = cx.type_attr(&cls, "decode")?;
    let raw_decode_fn = cx.type_attr(&cls, "raw_decode")?;
    cx.fingerprint_code(&decode_fn, fp::DECODE)?;
    cx.fingerprint_code(&raw_decode_fn, fp::RAW_DECODE)?;
    // `decode(self, s, _w=WHITESPACE.match)`: `_w` must match `[ \t\n\r]*`.
    let w = decode_fn.getattr("__defaults__")?.get_item(0)?;
    let pat = w.getattr("__self__")?;
    let re_flags = 8 | 16 | 32 | 64; // MULTILINE | DOTALL | UNICODE | VERBOSE
    if w.getattr("__name__")?.extract::<String>()? != "match"
        || pat.getattr("pattern")?.extract::<String>()? != "[ \\t\\n\\r]*"
        || pat.getattr("flags")?.extract::<i64>()? != re_flags
    {
        return Err(refuse!(Deps: "JSONDecoder.decode's whitespace pattern differs"));
    }
    cx.pin_dict(&crate::accel::prepare::type_dict(&cls)?, &DEPS)?;
    let dec = cx.pin_global(&g, "_default_decoder", &DEPS)?;
    if !dec.get_type().is(&cls) {
        return Err(refuse!(Deps: "json._default_decoder is not a JSONDecoder"));
    }
    let dd = instance_dict(&dec)?;
    let scanner = cx.get(&dd, "scan_once")?;
    let c_json = cx.loaded_module("_json")?;
    if !scanner.get_type().is(&cx.get(&c_json.dict(), "make_scanner")?) {
        return Err(refuse!(Deps: "json._default_decoder.scan_once is not the C scanner"));
    }
    let none = |name: &str| -> Result<bool, Refusal> { Ok(scanner.getattr(name)?.is_none()) };
    let float_t = py.get_type::<pyo3::types::PyFloat>();
    let int_t = py.get_type::<pyo3::types::PyInt>();
    if !scanner.getattr("strict")?.is_truthy()?
        || !none("object_hook")?
        || !none("object_pairs_hook")?
        || (cx.version() >= (3, 15) && !none("array_hook")?)
        || !scanner.getattr("parse_float")?.is(&float_t)
        || !scanner.getattr("parse_int")?.is(&int_t)
    {
        return Err(refuse!(Deps: "json._default_decoder's scanner is not the default one"));
    }
    cx.pin_dict(&dd, &DEPS)?;

    // loads' keyword defaults: all None.
    let kwd = func.getattr("__kwdefaults__")?.cast_into::<PyDict>().map_err(|_| refuse!(Deps: "no __kwdefaults__"))?;
    let n_kw = if cx.version() >= (3, 15) { 7 } else { 6 };
    for name in &KW_NAMES[..n_kw] {
        if !cx.get(&kwd, name)?.is_none() {
            return Err(refuse!(Deps: "json.loads default {name} differs"));
        }
    }
    if kwd.len() != n_kw {
        return Err(refuse!(Deps: "json.loads keyword defaults differ"));
    }
    cx.pin_dict(&kwd, &DEPS)?;

    for (cell, name) in KW.iter().zip(KW_NAMES) {
        cell.store(PyString::intern(py, name).into_ptr(), Relaxed);
    }
    KW_COUNT.store(n_kw, Relaxed);
    FUNC.store(func.clone().into_ptr(), Relaxed);
    CODE.store(func.getattr("__code__")?.into_ptr(), Relaxed);
    KWDEFAULTS.store(kwd.into_any().into_ptr(), Relaxed);
    DEFAULT_DECODER.store(dec.into_ptr(), Relaxed);
    DECODER_TYPE.store(cls.into_any().into_ptr(), Relaxed);
    loads::DECODE.set(&decode_fn);
    loads::RAW_DECODE.set(&raw_decode_fn);

    cx.vectorcall(&func, loads::kernel, &accel::FUNCTION_VECTORCALL)?;
    cx.self_test("json.loads probes", selftest::loads_selftest)
}

pub static DUMPS: TargetSpec = TargetSpec {
    name: "json.dumps",
    group: "json",
    module: Some("json"),
    versions: &[(3, 14), (3, 15)],
    free_threaded: true,
    counters: &dumps::COUNTERS,
    prepare: prepare_dumps,
    checkable: true,
    ..TargetSpec::DEFAULT
};

pub static LOADS: TargetSpec = TargetSpec {
    name: "json.loads",
    group: "json",
    module: Some("json"),
    versions: &[(3, 14), (3, 15)],
    free_threaded: true,
    counters: &loads::COUNTERS,
    prepare: prepare_loads,
    checkable: true,
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&DUMPS, &LOADS];

// ---------------------------------------------------------------------------------------
// Test helpers (`rustique._rustique._accel._t_json`).
// ---------------------------------------------------------------------------------------

/// The encoder alone (no guards), for differential tests: `None` when outside the subset.
#[pyfunction]
#[pyo3(signature = (obj, ensure_ascii=true, indent=None, sort_keys=false, skipkeys=false, allow_nan=true, check_circular=true))]
#[allow(clippy::too_many_arguments)]
fn encoder_dumps(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    ensure_ascii: bool,
    indent: Option<usize>,
    sort_keys: bool,
    skipkeys: bool,
    allow_nan: bool,
    check_circular: bool,
) -> PyResult<Option<Py<PyAny>>> {
    let o = encode::Opts {
        ensure_ascii,
        indent: indent.map(encode::Indent::Spaces),
        sort_keys,
        skipkeys,
        allow_nan,
        check_circular,
        ..encode::Opts::DEFAULT
    };
    // SAFETY: attached thread; live object.
    match unsafe { dumps::dumps(obj.as_ptr(), &o) } {
        // SAFETY: a new reference.
        Ok(s) => Ok(Some(unsafe { Bound::from_owned_ptr(py, s) }.unbind())),
        Err(_) => Ok(None),
    }
}

/// The decoder alone (no guards): `(True, obj)` or `(False, None)` outside the subset.
#[pyfunction]
fn decoder_loads(py: Python<'_>, s: &Bound<'_, PyAny>) -> (bool, Py<PyAny>) {
    // SAFETY: attached thread; live object.
    match unsafe { loads::loads(s.as_ptr()) } {
        // SAFETY: a new reference.
        Ok(o) => (true, unsafe { Bound::from_owned_ptr(py, o) }.unbind()),
        Err(_) => (false, py.None()),
    }
}

/// All install-time self-test texts (the install runs a subset, within its cost budget).
#[pyfunction]
fn selftest_all(py: Python<'_>) -> PyResult<bool> {
    Ok(selftest::dumps_selftest_n(py, 3)? && selftest::loads_selftest(py)?)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = PyModule::new(m.py(), "_t_json")?;
    sub.add_function(wrap_pyfunction!(selftest_all, &sub)?)?;
    sub.add_function(wrap_pyfunction!(encoder_dumps, &sub)?)?;
    sub.add_function(wrap_pyfunction!(decoder_loads, &sub)?)?;
    m.add("_t_json", sub)
}
