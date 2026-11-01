//! Recorded fingerprints of the originals per CPython minor version (contract §3.2), in the
//! registry's canonical format (v2), on 3.14.9 / 3.14.9t and 3.15.1 / 3.15.1t (free-threaded
//! builds give the same values); regenerate with
//! `python -m rustique._takeover._dev report --only datetime --import datetime,_strptime,time`
//! (or `benches/targets/_dt_fp_report.py`).

use crate::accel::Fingerprints;

/// Python code the strptime kernels reimplement (`_strptime`, `locale`), checked when the
/// kernels first see a `_strptime` module object.
pub fn code(name: &str) -> Fingerprints {
    match name {
        "_strptime_datetime_datetime" => &[(3, 14, "4beb911d71dd87bc"), (3, 15, "78d16a7ed4ddeba0")],
        "_strptime_datetime_date" => &[(3, 14, "51c5573cad121d01"), (3, 15, "d9a664d66476b7bb")],
        "_strptime_datetime_time" => &[(3, 14, "8069b45f5bcf7dec"), (3, 15, "f8ff45c4dd63b26a")],
        "_strptime_time" => &[(3, 14, "93e790072c913105"), (3, 15, "50b125778a5dca4d")],
        "_strptime" => &[(3, 14, "89fd394e42461045"), (3, 15, "c143f8352a0b01b5")],
        "_getlang" => &[(3, 14, "27e4adb5de175de2"), (3, 15, "30c4390b610fd90d")],
        "_parse_tz" => &[(3, 14, "ecf697b9df34cb2d"), (3, 15, "a2e51f6d6174666e")],
        "_fixmonths" => &[(3, 14, "3d54d2a033bfdcf1"), (3, 15, "9b4da6485ec893b9")],
        "getlocale" => &[(3, 14, "f968b3c9457f6c43"), (3, 15, "17624659bd5e6d41")],
        "_parse_localename" => &[(3, 14, "d09bd0daee17ff36"), (3, 15, "b34fc7cbc4c26c6a")],
        "normalize" => &[(3, 14, "933dce848be24a0e"), (3, 15, "d77ca0412769d24a")],
        "TimeRE.__init__" => &[(3, 14, "06c46c3dd5a832d4"), (3, 15, "e6a6564b3c80a3c1")],
        "TimeRE._TimeRE__seqToRE" => &[(3, 14, "608b2c8cae55772e"), (3, 15, "934dc4130b2bb350")],
        "TimeRE.pattern" => &[(3, 14, "782023e9caec96e2"), (3, 15, "c03e9460964c2c13")],
        "TimeRE.compile" => &[(3, 14, "7b6c294c6ffc3dd7"), (3, 15, "82cdef59d33c7e10")],
        _ => &[],
    }
}

/// C entry points (method definitions) the kernels replace.
pub fn c(name: &str) -> Fingerprints {
    match name {
        "datetime.datetime.strptime" => &[(3, 14, "e584c3169ffc40f2"), (3, 15, "b93548f6d74165af")],
        "datetime.date.strptime" => &[(3, 14, "429050ff630d11fd"), (3, 15, "c85c2f84d89df72a")],
        "datetime.time.strptime" => &[(3, 14, "e91e35d557e75aa2"), (3, 15, "0572e013a22aa9e2")],
        "time.strptime" => &[(3, 14, "633878062ff6886c"), (3, 15, "633878062ff6886c")],
        "datetime.date.strftime" => &[(3, 14, "1daa815724b5b913"), (3, 15, "1e7c65fada5a5ba5")],
        "datetime.time.strftime" => &[(3, 14, "1daa815724b5b913"), (3, 15, "0996633a011a5569")],
        "time.strftime" => &[(3, 14, "d7799c4aa3b998d3"), (3, 15, "d7799c4aa3b998d3")],
        "datetime.datetime.isoformat" => &[(3, 14, "9ef6efc1cf6a87b2"), (3, 15, "c0779ff2e71ee16f")],
        "datetime.date.isoformat" => &[(3, 14, "69c033b648a84eda"), (3, 15, "69c033b648a84eda")],
        "datetime.time.isoformat" => &[(3, 14, "a4cd19d89666c202"), (3, 15, "6ad063497f363aec")],
        _ => &[],
    }
}
