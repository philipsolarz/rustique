//! Process memory facts shared by the in-place patching code (`accel::site`,
//! `accel::prepare`).

/// Is `[addr, addr + len)` writable memory of this process?
///
/// What gets patched lives either in a loaded object's data (static `PyMethodDef` tables,
/// static type objects of libpython or an extension) or on the heap (heap types, function
/// objects). Linux (64-bit ELF): the loaded objects' program headers answer without a
/// system call: an address inside a `PT_LOAD` segment is writable when the segment is
/// `PF_W` and the address is outside `PT_GNU_RELRO` (made read-only after relocation;
/// treated from its page start, conservatively); an address inside no loaded object is
/// heap or anonymous memory, which CPython only allocates writable. The segments are kept
/// for the process and rebuilt when the loader's `dlpi_adds`/`dlpi_subs` counters say an
/// object was loaded or unloaded since (one `dl_iterate_phdr` step per registry pass, and
/// per query outside every known object).
/// Elsewhere: `/proc` is not consulted and CPython's static method tables are writable data.
pub fn writable(addr: usize, len: usize) -> bool {
    let Some(end) = addr.checked_add(len) else { return false };
    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    {
        phdr::writable(addr, end)
    }
    #[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
    {
        let _ = end;
        true
    }
}

/// A registry pass starts: the next query re-checks the loader's counters.
pub fn new_pass() {
    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    phdr::new_pass();
}

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
mod phdr {
    use std::ffi::{c_char, c_int, c_void};

    #[repr(C)]
    #[allow(dead_code)] // C layout
    struct Phdr {
        p_type: u32,
        p_flags: u32,
        p_offset: u64,
        p_vaddr: u64,
        p_paddr: u64,
        p_filesz: u64,
        p_memsz: u64,
        p_align: u64,
    }

    /// `struct dl_phdr_info` (glibc 2.4+ and musl have `dlpi_adds`/`dlpi_subs`, present
    /// when the callback's `size` covers them).
    #[repr(C)]
    #[allow(dead_code)] // C layout
    struct Info {
        dlpi_addr: u64,
        dlpi_name: *const c_char,
        dlpi_phdr: *const Phdr,
        dlpi_phnum: u16,
        dlpi_adds: u64,
        dlpi_subs: u64,
    }

    unsafe extern "C" {
        fn dl_iterate_phdr(
            cb: unsafe extern "C" fn(*mut Info, usize, *mut c_void) -> c_int,
            data: *mut c_void,
        ) -> c_int;
    }

    const PT_LOAD: u32 = 1;
    const PT_GNU_RELRO: u32 = 0x6474_e552;
    const PF_W: u32 = 2;
    const PAGE: u64 = 4096;

    /// Segments of the loaded objects: `(lo, hi, writable)` per PT_LOAD, with RELRO
    /// ranges marked read-only (from their page start, conservatively); and the loader's
    /// `(adds, subs)` counters when it was taken. Kept for the process; rebuilt when objects
    /// were loaded or unloaded since.
    #[allow(clippy::type_complexity)]
    static SNAPSHOT: std::sync::Mutex<Option<(Vec<(u64, u64, bool)>, Option<(u64, u64)>)>> =
        std::sync::Mutex::new(None);

    fn counters(info: &Info, size: usize) -> Option<(u64, u64)> {
        (size >= std::mem::size_of::<Info>()).then_some((info.dlpi_adds, info.dlpi_subs))
    }

    struct Collect {
        segs: Vec<(u64, u64, bool)>,
        counters: Option<(u64, u64)>,
    }

    unsafe extern "C" fn collect(info: *mut Info, size: usize, data: *mut c_void) -> c_int {
        // SAFETY: called by `dl_iterate_phdr` with a valid info and our state.
        unsafe {
            let out = &mut *data.cast::<Collect>();
            let info = &*info;
            out.counters = counters(info, size);
            let phdrs = std::slice::from_raw_parts(info.dlpi_phdr, usize::from(info.dlpi_phnum));
            let base = info.dlpi_addr;
            for p in phdrs.iter().filter(|p| p.p_type == PT_GNU_RELRO) {
                let s = base.wrapping_add(p.p_vaddr) & !(PAGE - 1);
                out.segs.push((s, base.wrapping_add(p.p_vaddr + p.p_memsz), false));
            }
            for p in phdrs.iter().filter(|p| p.p_type == PT_LOAD) {
                out.segs.push((
                    base.wrapping_add(p.p_vaddr),
                    base.wrapping_add(p.p_vaddr + p.p_memsz),
                    p.p_flags & PF_W != 0,
                ));
            }
            0
        }
    }

    unsafe extern "C" fn peek(info: *mut Info, size: usize, data: *mut c_void) -> c_int {
        // SAFETY: called by `dl_iterate_phdr`; reads the counters of the first object only.
        unsafe { *data.cast::<Option<(u64, u64)>>() = counters(&*info, size) };
        1
    }

    /// (segments as (start, end, writable), the loader's (adds, subs) counters).
    type Snapshot = (Vec<(u64, u64, bool)>, Option<(u64, u64)>);

    fn snapshot() -> Snapshot {
        let mut c = Collect { segs: Vec::new(), counters: None };
        // SAFETY: the callback only reads the program headers it is given.
        unsafe { dl_iterate_phdr(collect, (&raw mut c).cast()) };
        (c.segs, c.counters)
    }

    /// The loader's counters right now (`None` if unavailable).
    fn current_counters() -> Option<(u64, u64)> {
        let mut c: Option<(u64, u64)> = None;
        // SAFETY: the callback writes into `c` only.
        unsafe { dl_iterate_phdr(peek, (&raw mut c).cast()) };
        c
    }

    fn lookup(segs: &[(u64, u64, bool)], lo: u64, hi: u64) -> Option<bool> {
        // RELRO entries come first per object: any overlap with one is read-only.
        let mut inside = None;
        for &(s, e, w) in segs {
            if lo < e && hi > s {
                if !w || !(lo >= s && hi <= e) {
                    return Some(false);
                }
                inside = Some(true);
            }
        }
        inside
    }

    /// The snapshot's counters were compared with the loader's in this registry pass.
    static CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    pub fn new_pass() {
        CHECKED.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn writable(addr: usize, end: usize) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        let (lo, hi) = (addr as u64, end as u64);
        let mut guard = SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner());
        // Objects are not unloaded during a pass: compare the counters once per pass, and
        // again when the address is in no known object (one may have been loaded since).
        if CHECKED.load(Relaxed)
            && let Some((segs, _)) = guard.as_ref()
            && let Some(found) = lookup(segs, lo, hi)
        {
            return found;
        }
        let now = current_counters();
        if now.is_none() || guard.as_ref().is_none_or(|(_, then)| *then != now) {
            *guard = Some(snapshot());
        }
        CHECKED.store(true, Relaxed);
        let (segs, _) = guard.as_ref().expect("just set");
        // Inside no loaded object: heap or anonymous memory.
        lookup(segs, lo, hi).unwrap_or(true)
    }
}
