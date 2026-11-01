//! Recorded fingerprints of the originals per CPython minor version (contract §3.2), on
//! 3.14.9 / 3.14.9t and 3.15.1 / 3.15.1t (free-threaded builds give the same values);
//! regenerate with `benches/targets/_dt_fp_report.py`.

use crate::accel::Fingerprints;

/// Python code the strptime kernels reimplement (`_strptime`, `locale`), with the group's
/// cheap fingerprint (`fastfp`; `_accel._datetime_fingerprints(_strptime)`).
pub fn code(name: &str) -> Fingerprints {
    match name {
        "_strptime_datetime_datetime" => &[(3, 14, "75758937167911c2"), (3, 15, "594f12d05a3b7eac")],
        "_strptime_datetime_date" => &[(3, 14, "43e3ced6c7621672"), (3, 15, "005aa63778bff25e")],
        "_strptime_datetime_time" => &[(3, 14, "e1a61f240af33779"), (3, 15, "ea3b0e5b7f1419f8")],
        "_strptime_time" => &[(3, 14, "a07883562e4bd221"), (3, 15, "e0467dd7db84fb10")],
        "_strptime" => &[(3, 14, "5f43d74e422c65ac"), (3, 15, "8c49f07e7219b400")],
        "_getlang" => &[(3, 14, "8234af0123bf2302"), (3, 15, "d040dda165365f38")],
        "_parse_tz" => &[(3, 14, "5e6b4a159588db5d"), (3, 15, "574da3f094e543ea")],
        "_fixmonths" => &[(3, 14, "a754f0e332ffe2b8"), (3, 15, "6978de62c431c738")],
        "getlocale" => &[(3, 14, "1ae23ba5ff95dfc2"), (3, 15, "b17640d2f4f6b6c2")],
        "_parse_localename" => &[(3, 14, "40c27a311aaccd02"), (3, 15, "6a3e6a53e6e9c08f")],
        "normalize" => &[(3, 14, "8f5395ecb624269f"), (3, 15, "b04d4ec2bd73f17c")],
        "TimeRE.__init__" => &[(3, 14, "894e5840de882a68"), (3, 15, "5638d3e380d6e5e2")],
        "TimeRE._TimeRE__seqToRE" => &[(3, 14, "8a980141b1c1dcd8"), (3, 15, "61ebda3e40f3db1d")],
        "TimeRE.pattern" => &[(3, 14, "fbfe157c97a52afb"), (3, 15, "a1e5c8c16afc8efc")],
        "TimeRE.compile" => &[(3, 14, "b341ef86c4daa78a"), (3, 15, "c29d22ef39fc6ca0")],
        _ => &[],
    }
}

/// C entry points (method definitions) the kernels replace (`fastfp::method_def`).
pub fn c(name: &str) -> Fingerprints {
    match name {
        "datetime.datetime.strptime" => &[(3, 14, "aaa4a5d22304732c"), (3, 15, "5bad6d9c269a6d60")],
        "datetime.date.strptime" => &[(3, 14, "7b5bd010217a0886"), (3, 15, "f581d6076ea90509")],
        "datetime.time.strptime" => &[(3, 14, "10a289233e46a62e"), (3, 15, "e208f47e4665a8df")],
        "datetime.date.strftime" => &[(3, 14, "b405caeeca036f24"), (3, 15, "072502a3d515ab0d")],
        "datetime.time.strftime" => &[(3, 14, "b405caeeca036f24"), (3, 15, "145408c88b879769")],
        "datetime.datetime.isoformat" => &[(3, 14, "c19c8cb4f7434c01"), (3, 15, "c92efb43bb948f42")],
        "datetime.date.isoformat" => &[(3, 14, "258bb13574d1d304"), (3, 15, "258bb13574d1d304")],
        "datetime.time.isoformat" => &[(3, 14, "7310902bdfa8f987"), (3, 15, "4384bd1ef4dfd1ff")],
        "time.strptime" => &[(3, 14, "2a21679255e7dc75"), (3, 15, "2a21679255e7dc75")],
        "time.strftime" => &[(3, 14, "712f5007a4c92f8f"), (3, 15, "712f5007a4c92f8f")],
        _ => &[],
    }
}
