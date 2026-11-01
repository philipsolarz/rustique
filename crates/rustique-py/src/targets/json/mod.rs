//! Target group `json`: `json.dumps` and `json.loads` (docs/targets/json.md).
//!
//! Both are vectorcall sites on the pure-Python functions of the `json` package (installed
//! when `json` finishes importing). Their fast paths run no Python code and have no side
//! effects; every dependency CPython reads at call time is pinned or checked per call.

pub(crate) mod dumps;
pub(crate) mod encode;
pub(crate) mod pystr;
mod selftest;

use std::sync::atomic::Ordering::Relaxed;

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
    pub const ENCODE_BASESTRING: Fingerprints = &[(3, 14, "9b3603c10dd95cd7"), (3, 15, "a14341830caa1909")];
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
        cx.pin(&eg, name, &DEPS)?;
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
    cx.pin(&eg, "c_make_encoder", &DEPS)?;
    // Globals `encode`/`iterencode` read (shadowing builtins included).
    for name in ["isinstance", "str", "list", "tuple", "float", "INFINITY"] {
        cx.pin_global(&eg, name, &DEPS)?;
    }

    // The encoder class and the cached default instance.
    let cls = cx.pin_global(&g, "JSONEncoder", &DEPS)?;
    let cls = cls.cast_into::<PyType>().map_err(|_| refuse!(Deps: "json.JSONEncoder is not a class"))?;
    if !cls.is(&cx.get(&eg, "JSONEncoder")?) {
        return Err(refuse!(Deps: "json.JSONEncoder is not json.encoder.JSONEncoder"));
    }
    cx.fingerprint_code(&cx.type_attr(&cls, "encode")?, fp::ENCODE)?;
    cx.fingerprint_code(&cx.type_attr(&cls, "iterencode")?, fp::ITERENCODE)?;
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

    cx.vectorcall(&func, dumps::kernel, &accel::FUNCTION_VECTORCALL)?;

    // Self-test: the encoder's text for probe documents equals CPython's.
    cx.self_test("json.dumps probes", selftest::dumps_selftest)
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

pub static TARGETS: &[&TargetSpec] = &[&DUMPS];

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

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = PyModule::new(m.py(), "_t_json")?;
    sub.add_function(wrap_pyfunction!(encoder_dumps, &sub)?)?;
    m.add("_t_json", sub)
}
