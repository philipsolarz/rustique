//! Process memory facts shared by the in-place patching code (`takeover.rs`, `builtins.rs`).

use std::sync::OnceLock;

/// Writable mappings of this process, read once from `/proc/self/maps` (Linux). What gets
/// patched (static `PyMethodDef` tables, static type slot tables) lives in CPython's data
/// segments, which are mapped before any extension loads and never move, so one snapshot
/// answers every check (measured: reading the maps once per patch site cost ~0.6 ms of
/// `rustique.enable()`).
static WRITABLE: OnceLock<Vec<(usize, usize)>> = OnceLock::new();

/// Is `[addr, addr + len)` inside a writable mapping of this process? (Linux: from
/// `/proc/self/maps`; elsewhere CPython's static method tables are writable data.)
pub fn writable(addr: usize, len: usize) -> bool {
    if !cfg!(target_os = "linux") {
        return true;
    }
    let inside = |ranges: &[(usize, usize)]| {
        ranges.iter().any(|&(lo, hi)| lo <= addr && addr.checked_add(len).is_some_and(|end| end <= hi))
    };
    // Not in the snapshot: read again (a mapping created after the snapshot was taken).
    inside(WRITABLE.get_or_init(writable_mappings)) || inside(&writable_mappings())
}

fn writable_mappings() -> Vec<(usize, usize)> {
    let Ok(maps) = std::fs::read_to_string("/proc/self/maps") else {
        return Vec::new();
    };
    maps.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let (range, perm) = (parts.next()?, parts.next()?);
            if perm.as_bytes().get(1) != Some(&b'w') {
                return None;
            }
            let (lo, hi) = range.split_once('-')?;
            Some((usize::from_str_radix(lo, 16).ok()?, usize::from_str_radix(hi, 16).ok()?))
        })
        .collect()
}
