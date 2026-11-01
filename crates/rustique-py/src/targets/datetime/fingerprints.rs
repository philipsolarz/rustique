//! Recorded fingerprints of the originals per CPython minor version (contract §3.2),
//! produced with `rustique._rustique._accel.fingerprint_report(only=["datetime"])` on
//! 3.14.9 / 3.14.9t and 3.15.1 / 3.15.1t (free-threaded builds give the same values).

use crate::accel::Fingerprints;

/// Python code the strptime kernels reimplement (`_strptime`, `locale`).
pub fn code(name: &str) -> Fingerprints {
    match name {
        "_strptime_datetime_datetime" => &[(3, 14, "d1a9bb6c6a3d83a0"), (3, 15, "1efd10fe7e679814")],
        "_strptime_datetime_date" => &[(3, 14, "15c51a19cf06b4bf"), (3, 15, "7d381b7786b5cc37")],
        "_strptime_datetime_time" => &[(3, 14, "ee12af58c61ba5dc"), (3, 15, "d55df6e1b0a8fb80")],
        "_strptime_time" => &[(3, 14, "cc336f4a3bf509d9"), (3, 15, "3c8a3cc4e1dc6bdb")],
        "_strptime" => &[(3, 14, "3e19cf9d0fa3ce0b"), (3, 15, "9d348ea5606ba0b2")],
        "_getlang" => &[(3, 14, "75b82ba52e5929d7"), (3, 15, "a818a6da4c11582f")],
        "_parse_tz" => &[(3, 14, "4b15ba68acd6f552"), (3, 15, "5ace6c3a2be1fbf2")],
        "getlocale" => &[(3, 14, "cae838445304c68f"), (3, 15, "3bfb7bd64c23e9b5")],
        "_parse_localename" => &[(3, 14, "4097c31dca4af639"), (3, 15, "76f5fe7ee3feeedc")],
        "normalize" => &[(3, 14, "552acf26939fa21c"), (3, 15, "d39f26184c4c49a9")],
        "_fixmonths" => &[(3, 14, "405eab31985a9453"), (3, 15, "cc75eed017ef957b")],
        "TimeRE.__init__" => &[(3, 14, "4b3a13d6c066a9d6"), (3, 15, "30f3b36048aafc89")],
        "TimeRE._TimeRE__seqToRE" => &[(3, 14, "5962bea718d6043a"), (3, 15, "63aa2f2efbae1eac")],
        "TimeRE.pattern" => &[(3, 14, "83f00931f0a1af0e"), (3, 15, "7c42d1cda0ee3ac1")],
        "TimeRE.compile" => &[(3, 14, "02be480b9d9f2449"), (3, 15, "020f976c4a1bc144")],
        "LocaleTime.__init__" => &[(3, 14, "29077d825f581fb4"), (3, 15, "78664601ae5ac24a")],
        "LocaleTime._LocaleTime__calc_weekday" => &[(3, 14, "52209fe12b22a0e4"), (3, 15, "672aae39b5225b6a")],
        "LocaleTime._LocaleTime__calc_month" => &[(3, 14, "0dc3b68f3dcc149c"), (3, 15, "5dbf7b8b24a15b0c")],
        "LocaleTime._LocaleTime__calc_am_pm" => &[(3, 14, "8c9f92d891717a85"), (3, 15, "e0509febad1595cf")],
        "LocaleTime._LocaleTime__calc_alt_digits" => &[(3, 14, "4cc763c461ce3a1a"), (3, 15, "5b94b3a796ed3062")],
        _ => &[],
    }
}

/// C entry points (method definitions) the kernels replace.
pub fn c(name: &str) -> Fingerprints {
    match name {
        "datetime.datetime.strptime" => &[(3, 14, "50b5638df6967a40"), (3, 15, "634f087a6bd2ebf5")],
        "datetime.date.strptime" => &[(3, 14, "e387084987a5b66f"), (3, 15, "9747b2f8ee6d59be")],
        "datetime.time.strptime" => &[(3, 14, "fa30ac2d44ba8738"), (3, 15, "1fa71ebb530df15f")],
        "time.strptime" => &[(3, 14, "c2b064f05dca09c8"), (3, 15, "c2b064f05dca09c8")],
        "datetime.date.strftime" => &[(3, 14, "b6058113cdbbf30f"), (3, 15, "705b6fd647a2a764")],
        "datetime.time.strftime" => &[(3, 14, "b6058113cdbbf30f"), (3, 15, "a51ebd6066934d99")],
        "time.strftime" => &[(3, 14, "4aca0e0472e239e6"), (3, 15, "4aca0e0472e239e6")],
        _ => &[],
    }
}
