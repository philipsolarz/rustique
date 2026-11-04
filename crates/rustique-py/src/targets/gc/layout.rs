//! The main interpreter's collector state (`struct _gc_runtime_state`,
//! `Include/internal/pycore_interp_structs.h`), located through CPython's own
//! `_PyRuntime.debug_offsets` (the table remote debuggers read, `pycore_debug_offsets.h`) and
//! checked field by field against what the running interpreter reports, once per process.
//! Any mismatch refuses the tier (category `layout`, or `symbol` when `_PyRuntime` is not
//! exported).
//!
//! Fields used (offsets from the start of the struct; GIL and free-threaded builds of one
//! minor share them up to `long_lived_pending`):
//!
//! | field | 3.15 | 3.14 | read / written |
//! |---|---|---|---|
//! | `enabled`, `debug` (int) | 0, 4 | 12, 16 | read (program state at install) |
//! | `generations[i].threshold` / FT `young`, `old[i]` (int) | 24, 48, 72 | 40, 64, 88 | read |
//! | `generations[i].count` (int) | +4 | +4 | read (statistics) |
//! | `permanent_generation.head._gc_next` (GIL) | 80 | 96 | read (frozen objects?) |
//! | `collecting` (int) | 112 | 192 | CAS 0→1→0 around the swap of `callbacks` |
//! | `garbage` (list) | 128 | 200 | read (calibration) |
//! | `callbacks` (list) | 136 | 208 | swapped for the forwarder's private list |
//! | `heap_size` (GIL: tracked objects) | 144 | 216 | read (pause cap) |
//! | `long_lived_total` | 152 | 240 | read; written (deferral) |
//! | `long_lived_pending` (GIL) | 160 | 248 | read (statistics) |
//! | `freeze_active` (FT, int) | 168 | 256 | read (frozen objects?) |
//!
//! `_Py_DebugOffsets` (same prefix on 3.14 and 3.15): `cookie` "xdebugpy" at 0, `version`
//! (`PY_VERSION_HEX`) at 8, `free_threaded` at 16, `interpreter_state.size` at 48,
//! `interpreter_state.gc` at 88; `gc.size` / `gc.collecting` at 752/760 (3.15) and 648/656
//! (3.14).

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicUsize, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;

/// Per-(minor, build) offsets inside `struct _gc_runtime_state`.
#[derive(Debug)]
pub(super) struct Offsets {
    size: usize,
    pub enabled: usize,
    pub debug: usize,
    pub thresholds: [usize; 3],
    pub collecting: usize,
    pub garbage: usize,
    pub callbacks: usize,
    pub heap_size: usize,
    pub long_lived_total: usize,
    pub long_lived_pending: usize,
    /// GIL: `permanent_generation.head`; FT: `freeze_active`.
    pub frozen: usize,
    /// Byte offsets of `gc.size` and `gc.collecting` inside `_Py_DebugOffsets`.
    dbg_gc: (usize, usize),
}

const GC_315: Offsets = Offsets {
    size: 176,
    enabled: 0,
    debug: 4,
    thresholds: [24, 48, 72],
    collecting: 112,
    garbage: 128,
    callbacks: 136,
    heap_size: 144,
    long_lived_total: 152,
    long_lived_pending: 160,
    frozen: 80,
    dbg_gc: (752, 760),
};
const FT_315: Offsets = Offsets { frozen: 168, ..GC_315 };
const GC_314: Offsets = Offsets {
    size: 264,
    enabled: 12,
    debug: 16,
    thresholds: [40, 64, 88],
    collecting: 192,
    garbage: 200,
    callbacks: 208,
    heap_size: 216,
    long_lived_total: 240,
    long_lived_pending: 248,
    frozen: 96,
    dbg_gc: (648, 656),
};
const FT_314: Offsets = Offsets { size: 288, frozen: 256, ..GC_314 };

const FT: bool = cfg!(Py_GIL_DISABLED);

/// The calibrated state of the main interpreter's collector.
#[derive(Debug)]
pub(super) struct Layout {
    /// Address of `PyInterpreterState_Main()->gc`.
    base: usize,
    pub off: &'static Offsets,
    /// `_PyRuntime` (for the record).
    runtime: usize,
    gc_offset: usize,
}

static LAYOUT: OnceLock<Result<Layout, (u8, String)>> = OnceLock::new();
/// 0: symbol, 1: layout (refusal categories).
pub(super) const E_SYMBOL: u8 = 0;
pub(super) const E_LAYOUT: u8 = 1;

/// The calibrated layout, if [`calibrate`] succeeded.
/// The calibrated layout as a plain pointer (one load on the hooks' path).
static READY: std::sync::atomic::AtomicPtr<Layout> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

#[inline]
pub(super) fn get() -> Option<&'static Layout> {
    let p = READY.load(Ordering::Acquire);
    // SAFETY: set once from the `OnceLock`'s value, which lives for the process.
    if p.is_null() { None } else { Some(unsafe { &*p }) }
}

#[cfg(unix)]
fn dlsym_default(name: &CStr) -> usize {
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
    }
    #[cfg(target_os = "macos")]
    let handle = -2isize as *mut c_void; // RTLD_DEFAULT
    #[cfg(not(target_os = "macos"))]
    let handle = std::ptr::null_mut(); // RTLD_DEFAULT
    // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name.
    unsafe { dlsym(handle, name.as_ptr()) as usize }
}

#[cfg(not(unix))]
fn dlsym_default(_name: &CStr) -> usize {
    0
}

fn rd_u64(addr: usize) -> u64 {
    // SAFETY: callers pass addresses inside `_PyRuntime` (a static of known minimum size).
    unsafe { std::ptr::read_unaligned(addr as *const u64) }
}

/// Locate and check the layout (once per process). `Err((category, reason))`.
pub(super) fn calibrate(py: Python<'_>) -> Result<&'static Layout, (u8, String)> {
    let r = LAYOUT.get_or_init(|| locate(py)).as_ref().map_err(|e| e.clone());
    if let Ok(l) = r {
        READY.store((l as *const Layout).cast_mut(), Ordering::Release);
    }
    r
}

fn locate(_py: Python<'_>) -> Result<Layout, (u8, String)> {
    let lay = |s: String| (E_LAYOUT, s);
    let (major, minor) = crate::accel::version::running();
    let off: &'static Offsets = match ((major, minor), FT) {
        ((3, 15), false) => &GC_315,
        ((3, 15), true) => &FT_315,
        ((3, 14), false) => &GC_314,
        ((3, 14), true) => &FT_314,
        _ => return Err(lay(format!("no collector layout recorded for CPython {major}.{minor}"))),
    };
    let runtime = dlsym_default(c"_PyRuntime");
    if runtime == 0 {
        return Err((E_SYMBOL, "symbol _PyRuntime not exported by this interpreter".into()));
    }
    // `_Py_DebugOffsets` is the first member of `_PyRuntimeState` (3.13+).
    // SAFETY: `_PyRuntime` is a large static struct; the reads stay inside its debug table.
    let cookie = unsafe { std::slice::from_raw_parts(runtime as *const u8, 8) };
    if cookie != b"xdebugpy" {
        return Err(lay("_PyRuntime.debug_offsets: cookie mismatch".into()));
    }
    #[allow(clippy::unnecessary_cast)] // c_ulong is u32 on Windows
    // SAFETY: exported data symbol of the running interpreter.
    let running_hex = unsafe { ffi::Py_Version } as u64;
    let version = rd_u64(runtime + 8);
    if version != running_hex {
        return Err(lay(format!("debug_offsets.version {version:#x} != Py_Version {running_hex:#x}")));
    }
    if rd_u64(runtime + 16) != FT as u64 {
        return Err(lay("debug_offsets.free_threaded differs from this build".into()));
    }
    let interp_size = rd_u64(runtime + 48) as usize;
    let gc_offset = rd_u64(runtime + 88) as usize;
    let (dsize, dcoll) = off.dbg_gc;
    let (gc_size, gc_collecting) = (rd_u64(runtime + dsize) as usize, rd_u64(runtime + dcoll) as usize);
    if gc_size != off.size || gc_collecting != off.collecting {
        return Err(lay(format!(
            "struct _gc_runtime_state: size {gc_size} / collecting at {gc_collecting}, recorded {} / {}",
            off.size, off.collecting
        )));
    }
    if gc_offset == 0 || gc_offset + off.size > interp_size || interp_size > (1 << 24) {
        return Err(lay(format!("PyInterpreterState.gc at {gc_offset} (size {interp_size})")));
    }
    // SAFETY: attached thread; the main interpreter outlives every use (process lifetime
    // until finalization, when the tier is released first).
    let interp = unsafe { ffi::PyInterpreterState_Main() } as usize;
    if interp == 0 {
        return Err(lay("no main interpreter".into()));
    }
    let l = Layout { base: interp + gc_offset, off, runtime, gc_offset };
    // Values every live collector state satisfies.
    let enabled = l.i32(off.enabled);
    let collecting = l.i32(off.collecting);
    if !(0..=1).contains(&enabled) || !(0..=1).contains(&collecting) || l.i32(off.debug) < 0 {
        return Err(lay("enabled/collecting/debug fields out of range".into()));
    }
    for &t in &off.thresholds {
        if l.i32(t) < 0 {
            return Err(lay("negative threshold".into()));
        }
    }
    // SAFETY: the two pointer fields of a live collector state (checked for exact lists).
    unsafe {
        for (what, o) in [("garbage", off.garbage), ("callbacks", off.callbacks)] {
            let p = l.ptr(o);
            if p.is_null() || ffi::PyList_CheckExact(p) == 0 {
                return Err(lay(format!("gc state field {what} is not a list")));
            }
        }
    }
    if l.isize(off.long_lived_total) < 0 || (!FT && (l.isize(off.heap_size) < 0 || l.isize(off.long_lived_pending) < 0))
    {
        return Err(lay("negative object count".into()));
    }
    Ok(l)
}

impl Layout {
    #[inline]
    fn at(&self, off: usize) -> usize {
        self.base + off
    }

    #[inline]
    pub fn i32(&self, off: usize) -> i32 {
        // SAFETY: an aligned int field of the live collector state (calibrated).
        unsafe { (*(self.at(off) as *const AtomicI32)).load(Ordering::Relaxed) }
    }

    #[inline]
    pub fn isize(&self, off: usize) -> isize {
        // SAFETY: an aligned Py_ssize_t field of the live collector state (calibrated).
        unsafe { (*(self.at(off) as *const AtomicIsize)).load(Ordering::SeqCst) }
    }

    #[inline]
    pub fn set_isize(&self, off: usize, v: isize) {
        // SAFETY: an aligned Py_ssize_t field of the live collector state (calibrated);
        // CPython reads it with plain loads (aligned word stores are atomic on every
        // supported platform).
        unsafe { (*(self.at(off) as *const AtomicIsize)).store(v, Ordering::SeqCst) }
    }

    #[inline]
    pub fn ptr(&self, off: usize) -> *mut ffi::PyObject {
        // SAFETY: an aligned pointer field of the live collector state (calibrated).
        unsafe { (*(self.at(off) as *const AtomicUsize)).load(Ordering::Acquire) as *mut ffi::PyObject }
    }

    #[inline]
    pub fn set_ptr(&self, off: usize, p: *mut ffi::PyObject) {
        // SAFETY: as `ptr`; callers hold the collector's `collecting` flag.
        unsafe { (*(self.at(off) as *const AtomicUsize)).store(p as usize, Ordering::Release) }
    }

    /// Take the collector's own "a collection is running" flag (CAS 0 → 1), as
    /// `gc_collect_main` does: while held, no collection runs and no GC callback is being
    /// invoked (callbacks run inside `gc_collect_main`, before the flag is cleared).
    pub fn try_hold_collecting(&self) -> bool {
        // SAFETY: the aligned int field `collecting` (calibrated).
        let a = unsafe { &*(self.at(self.off.collecting) as *const AtomicI32) };
        a.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    pub fn release_collecting(&self) {
        // SAFETY: as `try_hold_collecting`.
        let a = unsafe { &*(self.at(self.off.collecting) as *const AtomicI32) };
        a.store(0, Ordering::Release);
    }

    /// The program's GC configuration as CPython holds it: `(enabled, (t0, t1, t2), debug)`.
    pub fn config(&self) -> (i32, [i32; 3], i32) {
        let o = self.off;
        (self.i32(o.enabled), o.thresholds.map(|t| self.i32(t)), self.i32(o.debug))
    }

    /// `generations[i].count` (FT: `young.count`, `old[i].count`).
    pub fn counts(&self) -> [i32; 3] {
        self.off.thresholds.map(|t| self.i32(t + 4))
    }

    /// Objects in the permanent generation (`gc.freeze()` was used and not undone).
    pub fn frozen(&self) -> bool {
        if FT {
            self.i32(self.off.frozen) != 0
        } else {
            // An empty list head points at itself.
            let head = self.at(self.off.frozen);
            // SAFETY: `_gc_next` of the permanent generation's list head.
            let next = unsafe { (*(head as *const AtomicUsize)).load(Ordering::Relaxed) };
            next != head
        }
    }

    pub fn describe(&self) -> Vec<(&'static str, usize)> {
        let o = self.off;
        vec![
            ("_PyRuntime", self.runtime),
            ("interp.gc", self.gc_offset),
            ("gc.size", o.size),
            ("collecting", o.collecting),
            ("callbacks", o.callbacks),
            ("long_lived_total", o.long_lived_total),
            ("heap_size", o.heap_size),
        ]
    }
}
