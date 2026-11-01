//! Recorded fingerprints of the originals per CPython minor version (contract §3.2),
//! produced with `rustique._rustique._accel.fingerprint_report(only=["datetime"])` on
//! 3.14.9 / 3.14.9t and 3.15.1 / 3.15.1t.

use crate::accel::Fingerprints;

/// Python code the strptime kernels reimplement (`_strptime`, `locale`).
pub fn code(name: &str) -> Fingerprints {
    match name {
        _ => &[],
    }
}

/// C entry points (method definitions) the kernels replace.
pub fn c(name: &str) -> Fingerprints {
    match name {
        _ => &[],
    }
}
