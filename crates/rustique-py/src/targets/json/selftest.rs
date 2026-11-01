//! Install-time self-test of the `json` encoder: probe documents built without running
//! Python code (module targets prepare inside the `sys.modules` watcher), expected texts
//! recorded from CPython 3.15.1 (identical on 3.14.9).

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyFloat, PyList, PyString, PyTuple};

use super::dumps;
use super::encode::{Indent, Opts};
use super::pystr::view;

fn int(py: Python<'_>, v: i64) -> Bound<'_, PyAny> {
    pyo3::types::PyInt::new(py, v).into_any()
}

/// `[None, True, False, 0, -7, 2**64, 1.5, -0.0, 1e16, 1e-7, nan, inf, -inf, "",
/// "a\"b\\\n\x00\x7f\xe9\u20ac\U0001F600", [], {}, [1, [2, [3, {}]]],
/// {"k": [1, 2.5, None], "\xe9": {"x": "y"}, "": 0}, (1, "t")]`
fn probe(py: Python<'_>) -> PyResult<Bound<'_, PyList>> {
    // SAFETY: CPython's own constructor for 2**64; a new reference or NULL.
    let big = unsafe {
        Bound::from_owned_ptr_or_err(
            py,
            ffi::PyLong_FromString(c"18446744073709551616".as_ptr(), std::ptr::null_mut(), 10),
        )?
    };
    let nested = PyList::new(
        py,
        [
            int(py, 1),
            PyList::new(py, [int(py, 2), PyList::new(py, [int(py, 3), PyDict::new(py).into_any()])?.into_any()])?
                .into_any(),
        ],
    )?;
    let d = PyDict::new(py);
    d.set_item("k", PyList::new(py, [int(py, 1), PyFloat::new(py, 2.5).into_any(), py.None().into_bound(py)])?)?;
    let inner = PyDict::new(py);
    inner.set_item("x", "y")?;
    d.set_item("\u{e9}", inner)?;
    d.set_item("", 0)?;
    let items: Vec<Bound<'_, PyAny>> = vec![
        py.None().into_bound(py),
        true.into_pyobject(py)?.to_owned().into_any(),
        false.into_pyobject(py)?.to_owned().into_any(),
        int(py, 0),
        int(py, -7),
        big,
        PyFloat::new(py, 1.5).into_any(),
        PyFloat::new(py, -0.0).into_any(),
        PyFloat::new(py, 1e16).into_any(),
        PyFloat::new(py, 1e-7).into_any(),
        PyFloat::new(py, f64::NAN).into_any(),
        PyFloat::new(py, f64::INFINITY).into_any(),
        PyFloat::new(py, f64::NEG_INFINITY).into_any(),
        PyString::new(py, "").into_any(),
        PyString::new(py, "a\"b\\\n\u{0}\u{7f}\u{e9}\u{20ac}\u{1f600}").into_any(),
        PyList::empty(py).into_any(),
        PyDict::new(py).into_any(),
        nested.into_any(),
        d.into_any(),
        PyTuple::new(py, [int(py, 1), PyString::new(py, "t").into_any()])?.into_any(),
    ];
    PyList::new(py, items)
}

const DEFAULT: &str = "[null, true, false, 0, -7, 18446744073709551616, 1.5, -0.0, 1e+16, 1e-07, NaN, Infinity, -Infinity, \"\", \"a\\\"b\\\\\\n\\u0000\\u007f\\u00e9\\u20ac\\ud83d\\ude00\", [], {}, [1, [2, [3, {}]]], {\"k\": [1, 2.5, null], \"\\u00e9\": {\"x\": \"y\"}, \"\": 0}, [1, \"t\"]]";
const UNICODE_INDENT2_SORTED: &str = "[\u{a}  null,\u{a}  true,\u{a}  false,\u{a}  0,\u{a}  -7,\u{a}  18446744073709551616,\u{a}  1.5,\u{a}  -0.0,\u{a}  1e+16,\u{a}  1e-07,\u{a}  NaN,\u{a}  Infinity,\u{a}  -Infinity,\u{a}  \"\",\u{a}  \"a\\\"b\\\\\\n\\u0000\u{7f}\u{e9}\u{20ac}\u{1f600}\",\u{a}  [],\u{a}  {},\u{a}  [\u{a}    1,\u{a}    [\u{a}      2,\u{a}      [\u{a}        3,\u{a}        {}\u{a}      ]\u{a}    ]\u{a}  ],\u{a}  {\u{a}    \"\": 0,\u{a}    \"k\": [\u{a}      1,\u{a}      2.5,\u{a}      null\u{a}    ],\u{a}    \"\u{e9}\": {\u{a}      \"x\": \"y\"\u{a}    }\u{a}  },\u{a}  [\u{a}    1,\u{a}    \"t\"\u{a}  ]\u{a}]";
const INDENT0_COMPACT: &str = "[\u{a}null,\u{a}true,\u{a}false,\u{a}0,\u{a}-7,\u{a}18446744073709551616,\u{a}1.5,\u{a}-0.0,\u{a}1e+16,\u{a}1e-07,\u{a}NaN,\u{a}Infinity,\u{a}-Infinity,\u{a}\"\",\u{a}\"a\\\"b\\\\\\n\\u0000\\u007f\\u00e9\\u20ac\\ud83d\\ude00\",\u{a}[],\u{a}{},\u{a}[\u{a}1,\u{a}[\u{a}2,\u{a}[\u{a}3,\u{a}{}\u{a}]\u{a}]\u{a}],\u{a}{\u{a}\"k\":[\u{a}1,\u{a}2.5,\u{a}null\u{a}],\u{a}\"\\u00e9\":{\u{a}\"x\":\"y\"\u{a}},\u{a}\"\":0\u{a}},\u{a}[\u{a}1,\u{a}\"t\"\u{a}]\u{a}]";

/// The encoder's text for the probe equals CPython's (two option sets at install, all
/// three with `all`).
pub(super) fn dumps_selftest(py: Python<'_>) -> PyResult<bool> {
    dumps_selftest_n(py, 2)
}

pub(super) fn dumps_selftest_n(py: Python<'_>, n: usize) -> PyResult<bool> {
    let p = probe(py)?;
    let comma = PyString::new(py, ",");
    let colon = PyString::new(py, ":");
    let cases: [(Opts, &str); 3] = [
        (Opts::DEFAULT, DEFAULT),
        (
            Opts { ensure_ascii: false, indent: Some(Indent::Spaces(2)), sort_keys: true, ..Opts::DEFAULT },
            UNICODE_INDENT2_SORTED,
        ),
        // SAFETY: live str objects kept by `comma`/`colon`.
        (
            Opts {
                indent: Some(Indent::Spaces(0)),
                // SAFETY: live str objects kept by `comma`/`colon`.
                item_sep: Some(unsafe { view(comma.as_ptr()) }),
                // SAFETY: as above.
                key_sep: Some(unsafe { view(colon.as_ptr()) }),
                ..Opts::DEFAULT
            },
            INDENT0_COMPACT,
        ),
    ];
    for (o, want) in cases.into_iter().take(n) {
        // SAFETY: attached thread; live probe.
        let got = match unsafe { dumps::dumps(p.as_ptr(), &o) } {
            // SAFETY: a new reference.
            Ok(s) => unsafe { Bound::from_owned_ptr(py, s) }.extract::<String>()?,
            Err(_) => return Ok(false),
        };
        if got != want {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The decoder's objects for a probe document equal CPython's (values, types, key memo).
pub(super) fn loads_selftest(py: Python<'_>) -> PyResult<bool> {
    let doc = PyString::new(
        py,
        "{\"a\": [1, -2.5e-3, \"x\\u00e9\\ud83d\\ude00\", true, false, null, {}, [], 0, -0, 1E+2], \"k\": {\"k\": \"v\"}, \"big\": 123456789012345678901234567890, \"nl\": \"\\n\", \"\\u00e9\": \"\"}",
    );
    // SAFETY: attached thread; live string.
    let Ok(o) = (unsafe { super::loads::loads(doc.as_ptr()) }) else {
        return Ok(false);
    };
    // SAFETY: a new reference.
    let o = unsafe { Bound::from_owned_ptr(py, o) };
    let d = o.cast::<PyDict>()?;
    let a = d.get_item("a")?.ok_or_else(|| pyo3::exceptions::PyKeyError::new_err("a"))?;
    let a = a.cast::<PyList>()?;
    let f = a.get_item(1)?;
    let ok = d.len() == 5
        && a.len() == 11
        && a.get_item(0)?.extract::<i64>()? == 1
        && f.is_exact_instance_of::<PyFloat>()
        && f.extract::<f64>()?.to_bits() == (-2.5e-3f64).to_bits()
        && a.get_item(2)?.cast::<PyString>()?.to_cow()? == "x\u{e9}\u{1f600}"
        && a.get_item(3)?.is(pyo3::types::PyBool::new(py, true))
        && a.get_item(4)?.is(pyo3::types::PyBool::new(py, false))
        && a.get_item(5)?.is_none()
        && a.get_item(6)?.cast::<PyDict>()?.is_empty()
        && a.get_item(7)?.cast::<PyList>()?.is_empty()
        && a.get_item(8)?.is(&a.get_item(9)?) // small ints: the cached 0
        && a.get_item(10)?.extract::<f64>()? == 100.0
        && d.get_item("big")?.map(|b| b.to_string()).as_deref() == Some("123456789012345678901234567890")
        && d.get_item("nl")?.map(|b| b.to_string()).as_deref() == Some("\n")
        && d.get_item("\u{e9}")?.map(|b| b.to_string()).as_deref() == Some("");
    // Repeated keys are one object (the scanner's memo).
    let outer_k = d.keys().iter().find(|k| k.to_string() == "k");
    let inner = d.get_item("k")?.ok_or_else(|| pyo3::exceptions::PyKeyError::new_err("k"))?;
    let inner_k = inner.cast::<PyDict>()?.keys().get_item(0)?;
    Ok(ok && outer_k.is_some_and(|k| k.is(&inner_k)))
}
