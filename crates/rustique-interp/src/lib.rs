//! The interp tier's platform layer: which handler sets this build contains, whether the
//! running CPython binary is a verified build, where its dispatch table is, and the
//! calling-convention self-test. The targets (`rustique-py/src/targets/interp`) declare the
//! table entries as sites; the registry swaps them.
//!
//! In a tail-calling CPython (python-build-standalone), every opcode handler is a
//! `preserve_none` function and dispatch is `musttail` through the writable static
//! `instruction_funcptr_handler_table`. The handler sets compiled from `gen/` (build.rs) are
//! generated from the running release's own `Python/bytecodes.c` with CPython's cases
//! generator plus `c/overrides.c`; a swapped entry runs one of them with the core's frame,
//! stack and thread state, and it dispatches onward through the core's table.
//!
//! Nothing here guesses: the release (exact `Py_Version`), the configuration and the ELF
//! build-id of the object holding the interpreter must match an entry of [`builds::VERIFIED`],
//! every entry the tier may swap must hold exactly the recorded core handler, and the ABI
//! self-test must pass; otherwise [`locate`] refuses with the reason.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub mod builds;

use builds::{VERIFIED, VerifiedBuild};

/// Mirror of `rq_init_args` (c/rq_api.h).
#[repr(C)]
struct RqInitArgs {
    core_handler_table: *mut c_void,
    core_tracing_table: *mut c_void,
    on_suspend: Option<unsafe extern "C" fn(c_int)>,
}

/// Mirror of `rq_api` (c/rq_api.h).
#[repr(C)]
pub struct RqApi {
    abi: u32,
    tag: *const c_char,
    gil_disabled: c_int,
    init: unsafe extern "C" fn(*const RqInitArgs) -> c_int,
    handler: unsafe extern "C" fn(c_int) -> *mut c_void,
    group_of: unsafe extern "C" fn(c_int) -> c_int,
    abi_selftest: unsafe extern "C" fn(*mut c_void) -> c_int,
    group_arm: unsafe extern "C" fn(c_int, *const *mut c_void, *const *mut c_void, *const *mut c_void, c_int) -> c_int,
    group_stats: unsafe extern "C" fn(c_int, *mut u64),
    opname: unsafe extern "C" fn(c_int) -> *const c_char,
}

const RQ_API_ABI: u32 = 1;

include!(concat!(env!("OUT_DIR"), "/sets.rs"));

/// Override groups (c/rq_common.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Group {
    /// The LOAD_ATTR family: megamorphic attribute cache.
    Attr = 0,
    /// CALL_NON_PY_GENERAL: `object.__setattr__`.
    SetAttr = 1,
    /// CALL_KW_NON_PY: keyword instantiation pushed inline.
    KwInit = 2,
    /// CALL_ISINSTANCE.
    IsInstance = 3,
}

pub const GROUPS: [Group; 4] = [Group::Attr, Group::SetAttr, Group::KwInit, Group::IsInstance];

/// Why the tier cannot run here (mapped to the registry's refusal categories by the caller).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refuse {
    Platform(String),
    Version(String),
    /// Not a verified binary (unknown build-id, not tail-calling, other distribution).
    Build(String),
    /// A dispatch-table entry is not the recorded core handler.
    Conflict(String),
    Selftest(String),
    /// This extension was built without a handler set for the running release.
    NoHandlers(String),
}

impl Refuse {
    pub fn reason(&self) -> &str {
        match self {
            Refuse::Platform(s)
            | Refuse::Version(s)
            | Refuse::Build(s)
            | Refuse::Conflict(s)
            | Refuse::Selftest(s)
            | Refuse::NoHandlers(s) => s,
        }
    }
}

/// The verified running interpreter and the handler set for it.
#[derive(Debug)]
pub struct Located {
    pub build: &'static VerifiedBuild,
    api: &'static RqApi,
    bias: usize,
    /// Path of the object holding the interpreter (for status).
    pub object: String,
}

// SAFETY: `api` points at a static of the compiled set; addresses are plain integers.
unsafe impl Send for Located {}
// SAFETY: as above; the set's functions are thread-safe per their contract.
unsafe impl Sync for Located {}

impl std::fmt::Debug for RqApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RqApi(abi {})", self.abi)
    }
}

/// Group statistics of the compiled set (cumulative since the group was armed).
#[derive(Clone, Copy, Debug, Default)]
pub struct GroupStats {
    pub executions: u64,
    pub hits: u64,
    pub fills: u64,
    pub suspended: bool,
    pub windows: u64,
}

impl Located {
    /// The CPython release of the handler set.
    pub fn release(&self) -> &'static str {
        self.build.release
    }

    /// Address of the core's dispatch-table entry for `opcode`.
    pub fn cell(&self, opcode: u8) -> usize {
        self.bias + self.build.handler_table + 8 * usize::from(opcode)
    }

    /// The core's own handler for `opcode` (recorded), if the tier may swap it.
    pub fn core_handler(&self, opcode: u8) -> Option<usize> {
        self.build.handlers.iter().find(|h| h.0 == opcode).map(|h| self.bias + h.1)
    }

    /// Our handler for `opcode`, if swappable.
    pub fn handler(&self, opcode: u8) -> Option<usize> {
        // SAFETY: the set's pure lookup function.
        let p = unsafe { (self.api.handler)(c_int::from(opcode)) };
        (!p.is_null()).then_some(p as usize)
    }

    /// The name of a swappable opcode.
    pub fn opname(&self, opcode: u8) -> &'static str {
        // SAFETY: a static string of the set, or NULL.
        let p = unsafe { (self.api.opname)(c_int::from(opcode)) };
        if p.is_null() {
            return "?";
        }
        // SAFETY: a NUL-terminated static string of the set.
        unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("?")
    }

    /// The opcodes of a group.
    pub fn ops(&self, group: Group) -> Vec<u8> {
        (0..=255u8)
            // SAFETY: the set's pure lookup function.
            .filter(|&op| unsafe { (self.api.group_of)(c_int::from(op)) } == group as i32)
            .collect()
    }

    /// Current content of the table entry for `opcode`.
    pub fn current(&self, opcode: u8) -> usize {
        // SAFETY: a verified, aligned pointer-sized entry of the core's static table.
        unsafe { (*(self.cell(opcode) as *const AtomicUsize)).load(Ordering::Acquire) }
    }

    /// Give a group the table entries it may restore by itself (net-benefit suspension) and
    /// start its counts fresh. Call before the entries are swapped.
    pub fn arm(&self, group: Group, ops: &[u8]) -> bool {
        let mut cells = Vec::new();
        let mut orig = Vec::new();
        let mut mine = Vec::new();
        for &op in ops {
            let (Some(o), Some(m)) = (self.core_handler(op), self.handler(op)) else { return false };
            cells.push(self.cell(op) as *mut c_void);
            orig.push(o as *mut c_void);
            mine.push(m as *mut c_void);
        }
        SUSPENDED[group as usize].store(false, Ordering::Relaxed);
        // SAFETY: arrays of `ops.len()` entries; the set copies them.
        unsafe {
            (self.api.group_arm)(group as c_int, cells.as_ptr(), orig.as_ptr(), mine.as_ptr(), ops.len() as c_int) == 0
        }
    }

    pub fn stats(&self, group: Group) -> GroupStats {
        let mut out = [0u64; 5];
        // SAFETY: writes 5 integers.
        unsafe { (self.api.group_stats)(group as c_int, out.as_mut_ptr()) };
        GroupStats { executions: out[0], hits: out[1], fills: out[2], suspended: out[3] != 0, windows: out[4] }
    }
}

/// Set by the handler set when a group restored the core's handlers (net benefit).
pub static SUSPENDED: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];
static SUSPEND_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Called by a group (any attached thread) after it restored the core's handlers.
unsafe extern "C" fn on_suspend(group: c_int) {
    if let Some(s) = usize::try_from(group).ok().and_then(|g| SUSPENDED.get(g)) {
        s.store(true, Ordering::Release);
    }
    let h = SUSPEND_HOOK.load(Ordering::Acquire);
    if h != 0 {
        // SAFETY: set by `set_suspend_hook` to a `fn(Group)`.
        let f: fn(Group) = unsafe { std::mem::transmute::<usize, fn(Group)>(h) };
        if let Some(g) = GROUPS.get(group as usize) {
            f(*g);
        }
    }
}

/// Register a function called when a group suspends itself (must not block or run Python).
pub fn set_suspend_hook(f: fn(Group)) {
    SUSPEND_HOOK.store(f as usize, Ordering::Release);
}

/// Releases with a handler set compiled into this build.
pub fn compiled_releases() -> Vec<&'static str> {
    SETS.iter().map(|s| s.0).collect()
}

/// Verify the running interpreter (once per process) and initialize its handler set.
/// Needs an attached thread state of the main interpreter.
pub fn locate() -> Result<&'static Located, Refuse> {
    static LOCATED: OnceLock<Result<Located, Refuse>> = OnceLock::new();
    LOCATED.get_or_init(locate_uncached).as_ref().map_err(Clone::clone)
}

fn locate_uncached() -> Result<Located, Refuse> {
    if !cfg!(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu")) {
        return Err(Refuse::Platform("the interp tier is validated on x86_64 Linux (glibc) only".into()));
    }
    // SAFETY: a constant exported by every CPython >= 3.11.
    let hex = unsafe { pyo3::ffi::Py_Version } as u32;
    let release = format!("{}.{}.{}", hex >> 24, (hex >> 16) & 0xff, (hex >> 8) & 0xff);
    if hex & 0xf0 != 0xf0 {
        return Err(Refuse::Version(format!("CPython {release} is a pre-release")));
    }
    let candidates: Vec<&VerifiedBuild> = VERIFIED.iter().filter(|b| b.hexversion == hex).collect();
    if candidates.is_empty() {
        let mut known: Vec<&str> = VERIFIED.iter().map(|b| b.release).collect();
        known.dedup();
        return Err(Refuse::Version(format!(
            "CPython {release}: no verified interp handlers (verified releases: {})",
            known.join(", ")
        )));
    }
    let Some(set) = SETS.iter().find(|s| s.0 == release) else {
        return Err(Refuse::NoHandlers(format!(
            "built without handlers for CPython {release} (compiled: {})",
            if SETS.is_empty() { "none".to_owned() } else { compiled_releases().join(", ") }
        )));
    };
    // SAFETY: the set's static descriptor.
    let api: &'static RqApi = unsafe { &*(set.2)() };
    if api.abi != RQ_API_ABI {
        return Err(Refuse::NoHandlers("handler set interface mismatch".into()));
    }
    let ft = api.gil_disabled != 0;
    let obj =
        interpreter_object().ok_or_else(|| Refuse::Build("cannot find the object holding the interpreter".into()))?;
    let Some(build) = candidates.iter().find(|b| b.free_threaded == ft && b.build_id == obj.build_id) else {
        return Err(Refuse::Build(format!(
            "unverified CPython binary (build-id {}, {}): the interp tier runs only on verified python-build-standalone builds",
            if obj.build_id.is_empty() { "none" } else { &obj.build_id },
            obj.path
        )));
    };
    let located = Located { build, api, bias: obj.bias, object: obj.path };
    // Every entry the tier may swap, and NOP (self-test), holds the recorded core handler.
    for &(op, _) in build.handlers {
        let want = located.core_handler(op).unwrap();
        if located.current(op) != want {
            return Err(Refuse::Conflict(format!(
                "dispatch-table entry {op} is not CPython's handler (patched by someone else?)"
            )));
        }
    }
    let nop = located.core_handler(NOP).ok_or_else(|| Refuse::Build("no NOP handler recorded".into()))?;
    // SAFETY: NOP's verified core handler; the self-test touches only its own scratch data.
    if unsafe { (api.abi_selftest)(nop as *mut c_void) } != 1 {
        return Err(Refuse::Selftest(
            "calling-convention self-test failed: the handlers were compiled with an incompatible ABI".into(),
        ));
    }
    let args = RqInitArgs {
        core_handler_table: (obj.bias + build.handler_table) as *mut c_void,
        core_tracing_table: if build.tracing_table == 0 {
            std::ptr::null_mut()
        } else {
            (obj.bias + build.tracing_table) as *mut c_void
        },
        on_suspend: Some(on_suspend),
    };
    // SAFETY: attached thread (caller); args outlive the call.
    if unsafe { (api.init)(&args) } != 0 {
        return Err(Refuse::Selftest("handler set initialization failed".into()));
    }
    Ok(located)
}

/// NOP's opcode (25 in every 3.15 release; Include/opcode_ids.h).
const NOP: u8 = 25;

// ---- The object holding the interpreter -------------------------------------------------

struct ObjInfo {
    bias: usize,
    build_id: String,
    path: String,
}

#[repr(C)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

#[repr(C)]
struct DlPhdrInfo {
    dlpi_addr: usize,
    dlpi_name: *const c_char,
    dlpi_phdr: *const Elf64Phdr,
    dlpi_phnum: u16,
}

unsafe extern "C" {
    fn dl_iterate_phdr(
        cb: unsafe extern "C" fn(*mut DlPhdrInfo, usize, *mut c_void) -> c_int,
        data: *mut c_void,
    ) -> c_int;
}

const PT_LOAD: u32 = 1;
const PT_NOTE: u32 = 4;
const NT_GNU_BUILD_ID: u32 = 3;

struct Search {
    anchor: usize,
    found: Option<ObjInfo>,
}

unsafe extern "C" fn visit(info: *mut DlPhdrInfo, _size: usize, data: *mut c_void) -> c_int {
    // SAFETY: called by dl_iterate_phdr with a valid info; data is our Search.
    unsafe {
        let s = &mut *(data as *mut Search);
        let info = &*info;
        let phdrs = std::slice::from_raw_parts(info.dlpi_phdr, usize::from(info.dlpi_phnum));
        let holds = phdrs.iter().any(|p| {
            p.p_type == PT_LOAD && {
                let start = info.dlpi_addr + p.p_vaddr as usize;
                (start..start + p.p_memsz as usize).contains(&s.anchor)
            }
        });
        if !holds {
            return 0;
        }
        let mut build_id = String::new();
        for p in phdrs.iter().filter(|p| p.p_type == PT_NOTE) {
            let mut at = info.dlpi_addr + p.p_vaddr as usize;
            let end = at + p.p_memsz as usize;
            while at + 12 <= end {
                let namesz = *(at as *const u32) as usize;
                let descsz = *((at + 4) as *const u32) as usize;
                let ntype = *((at + 8) as *const u32);
                let name = at + 12;
                let desc = name + namesz.div_ceil(4) * 4;
                if ntype == NT_GNU_BUILD_ID && namesz == 4 && *(name as *const [u8; 4]) == *b"GNU\0" {
                    let bytes = std::slice::from_raw_parts(desc as *const u8, descsz);
                    build_id = bytes.iter().map(|b| format!("{b:02x}")).collect();
                }
                at = desc + descsz.div_ceil(4) * 4;
            }
        }
        let name = if info.dlpi_name.is_null() { "" } else { CStr::from_ptr(info.dlpi_name).to_str().unwrap_or("") };
        let path = if name.is_empty() {
            std::fs::read_link("/proc/self/exe").map(|p| p.display().to_string()).unwrap_or_default()
        } else {
            name.to_owned()
        };
        s.found = Some(ObjInfo { bias: info.dlpi_addr, build_id, path });
        1
    }
}

fn interpreter_object() -> Option<ObjInfo> {
    let mut s = Search { anchor: pyo3::ffi::PyEval_EvalCode as *const () as usize, found: None };
    // SAFETY: the callback only reads loader-provided program headers and notes.
    unsafe { dl_iterate_phdr(visit, (&raw mut s).cast()) };
    s.found
}
