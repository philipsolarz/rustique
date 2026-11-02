//! Per-process counters that survive the process (docs/atlas/w2-dx.md §3.1): forked workers
//! ending in `os._exit` and killed processes lose nothing, and a forked child starts from
//! zero instead of inheriting its parent's counts.
//!
//! **Fork.** A `pthread_atfork` child handler (atomics only) zeroes every target's counters
//! and detaches them from the parent's file, before any Python code runs in the child; the
//! kernel state (scores, suspensions) is inherited. `status()["process"]["ppid"]` is the
//! parent's pid in a child forked after the first `enable()`. A `posix.register_at_fork`
//! `after_in_child` hook (no import: the builtin module from `sys.modules`) then gives the
//! child its own live file. Children of `fork()`+`exec` (subprocess) create nothing.
//!
//! **Live counter file** (Unix; Windows gets the exit-hook JSON only): when counters are on
//! and `RUSTIQUE_REPORT` is a `*.json` path `P`, each process keeps its counters in a
//! `MAP_SHARED` mapping of `P.<pid>.cnt`, created at the first `enable()`, and each forked
//! child in its own. Layout (native-endian `u64` words; version 2):
//!
//! | offset | word |
//! |---|---|
//! | 0 | magic `b"RQCNT\0\0\x02"` |
//! | 8 | pid |
//! | 16 | ppid (the parent's pid in a forked child, else 0) |
//! | 24 | number of targets `n` |
//! | 32 | byte offset of the names: `n` NUL-terminated target names, registry order |
//! | 40 | byte offset of the counters: `n` rows of `row_words` words |
//! | 48 | flags: bit 0 = the exit-hook JSON record was written |
//! | 56 | `row_words` (11) |
//!
//! Row: `[hits, args, deps, tools, interp, deopt, error, check, suspensions, continue,
//! abandon]` (version 1 had the first 9). Targets sharing one `Counters` count in the row of
//! the first of them. The hot path is unchanged: one byte load when counting is off; when on,
//! increments go to the row through one pointer load.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use pyo3::prelude::*;

use super::registry::all_specs;
use super::stats::ROW_WORDS;

pub const MAGIC: [u8; 8] = *b"RQCNT\0\0\x02";
const HEADER: usize = 64;

/// The parent's pid in a child forked after the first `enable()` (0: not forked).
static FORKED_FROM: AtomicU64 = AtomicU64::new(0);
static HOOKS: AtomicBool = AtomicBool::new(false);
/// `P` of the live file (`RUSTIQUE_REPORT`), when the live file is on.
static PATH: Mutex<Option<String>> = Mutex::new(None);
/// The current mapping (address, length), for the exit flag.
static MAP: AtomicUsize = AtomicUsize::new(0);

/// The parent's pid in a forked child (status `process["ppid"]`).
pub fn forked_from() -> Option<u64> {
    match FORKED_FROM.load(Ordering::Relaxed) {
        0 => None,
        p => Some(p),
    }
}

#[cfg(unix)]
mod sys {
    use std::ffi::{c_int, c_void};
    unsafe extern "C" {
        pub fn pthread_atfork(
            prepare: Option<unsafe extern "C" fn()>,
            parent: Option<unsafe extern "C" fn()>,
            child: Option<unsafe extern "C" fn()>,
        ) -> c_int;
        pub fn getppid() -> c_int;
        pub fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut c_void;
    }
    pub const PROT_READ: c_int = 1;
    pub const PROT_WRITE: c_int = 2;
    pub const MAP_SHARED: c_int = 1;
}

/// The `pthread_atfork` child handler: atomics only (async-signal-safe).
#[cfg(unix)]
unsafe extern "C" fn after_fork_child() {
    // SAFETY: plain syscall.
    let ppid = unsafe { sys::getppid() };
    FORKED_FROM.store(ppid as u64, Ordering::Relaxed);
    MAP.store(0, Ordering::Relaxed);
    for spec in all_specs() {
        spec.counters.reset_after_fork();
    }
}

/// Register the fork hooks (once per process, at the first `enable()`).
pub(crate) fn ensure_hooks(py: Python<'_>) {
    if HOOKS.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = all_specs(); // initialised before any fork (the child handler reads it)
    #[cfg(unix)]
    // SAFETY: registering a handler that only touches atomics.
    unsafe {
        sys::pthread_atfork(None, None, Some(after_fork_child));
    }
    // `posix.register_at_fork(after_in_child=...)`: the child's own live file.
    if let Ok(Some(posix)) = super::prepare::sys_modules(py).map(|m| m.get_item("posix").ok().flatten())
        && let Ok(f) = wrap_pyfunction!(after_in_child, py)
    {
        let kw = pyo3::types::PyDict::new(py);
        if kw.set_item("after_in_child", f).is_ok() {
            let _ = posix.call_method("register_at_fork", (), Some(&kw));
        }
    }
}

#[pyfunction]
#[pyo3(name = "_after_fork_child")]
fn after_in_child() {
    let path = PATH.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(p) = path {
        open(&p);
    }
}

/// Turn the live file on for report path `p` (a `*.json` path) and create this process's.
pub(crate) fn start(p: &str) {
    *PATH.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.to_owned());
    if MAP.load(Ordering::Relaxed) == 0 {
        open(p);
    }
}

/// The live file's name for this process.
pub fn file_name(p: &str) -> String {
    format!("{p}.{}.cnt", std::process::id())
}

#[cfg(unix)]
fn open(p: &str) {
    use std::os::unix::io::AsRawFd;
    let specs = all_specs();
    let mut names: Vec<u8> = Vec::new();
    for s in specs {
        names.extend_from_slice(s.name.as_bytes());
        names.push(0);
    }
    while !names.len().is_multiple_of(8) {
        names.push(0);
    }
    let counters_off = HEADER + names.len();
    let len = counters_off + specs.len() * ROW_WORDS * 8;
    let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(file_name(p))
    else {
        return;
    };
    if f.set_len(len as u64).is_err() {
        return;
    }
    // SAFETY: a fresh shared mapping of the file we just sized; the fd may be closed after.
    let base = unsafe {
        sys::mmap(std::ptr::null_mut(), len, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_SHARED, f.as_raw_fd(), 0)
    };
    if base as isize == -1 || base.is_null() {
        return;
    }
    let base = base.cast::<u8>();
    // SAFETY: within the mapping; the header is written before any counter points into it.
    unsafe {
        std::ptr::copy_nonoverlapping(MAGIC.as_ptr(), base, 8);
        let w = base.cast::<u64>();
        *w.add(1) = u64::from(std::process::id());
        *w.add(2) = FORKED_FROM.load(Ordering::Relaxed);
        *w.add(3) = specs.len() as u64;
        *w.add(4) = HEADER as u64;
        *w.add(5) = counters_off as u64;
        *w.add(6) = 0;
        *w.add(7) = ROW_WORDS as u64;
        std::ptr::copy_nonoverlapping(names.as_ptr(), base.add(HEADER), names.len());
    }
    // Rows: the first target of each distinct `Counters` gets it; the current counts are
    // carried over.
    let mut seen: Vec<usize> = Vec::new();
    for (i, s) in specs.iter().enumerate() {
        let c = s.counters as *const _ as usize;
        if seen.contains(&c) {
            continue;
        }
        seen.push(c);
        // SAFETY: row `i` within the mapping; 8-aligned (page-aligned base, 8-multiple offsets).
        let row = unsafe { base.add(counters_off + i * ROW_WORDS * 8) }.cast::<AtomicU64>();
        let (hits, fb) = s.counters.snapshot();
        let susp = s.counters.suspension_count();
        // SAFETY: ROW_WORDS words of the row.
        unsafe {
            (*row).store(hits, Ordering::Relaxed);
            for (j, (_, n)) in fb.iter().enumerate() {
                let w = if j <= 6 { j + 1 } else { j + 2 };
                (*row.add(w)).store(*n, Ordering::Relaxed);
            }
            (*row.add(8)).store(susp, Ordering::Relaxed);
        }
        s.counters.set_slot(row);
    }
    MAP.store(base as usize, Ordering::Release);
    // The mapping lives as long as the process (kernels may count until the end).
}

#[cfg(not(unix))]
fn open(_p: &str) {}

/// Mark the exit-hook JSON record as written (header flags bit 0).
pub(crate) fn mark_json_written() {
    let base = MAP.load(Ordering::Acquire);
    if base != 0 {
        // SAFETY: the live mapping's header (word 6).
        unsafe { (*(base as *const AtomicU64).add(6)).fetch_or(1, Ordering::Release) };
    }
}
