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
/// heap or anonymous memory, which CPython only allocates writable. Elsewhere: `/proc`
/// is not consulted and CPython's static method tables are writable data.
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

/// Forget the loaded objects' segments (the registry calls this around each pass; a pass
/// never sees an object unloaded, and objects loaded since show up in the next pass).
pub fn clear_snapshot() {
    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    phdr::clear();
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

    /// The leading fields of `struct dl_phdr_info` (stable since glibc 2.2.4 / musl).
    #[repr(C)]
    #[allow(dead_code)] // C layout
    struct Info {
        dlpi_addr: u64,
        dlpi_name: *const c_char,
        dlpi_phdr: *const Phdr,
        dlpi_phnum: u16,
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
    /// ranges marked read-only (from their page start, conservatively).
    static SNAPSHOT: std::sync::Mutex<Option<Vec<(u64, u64, bool)>>> = std::sync::Mutex::new(None);

    unsafe extern "C" fn collect(info: *mut Info, _size: usize, data: *mut c_void) -> c_int {
        // SAFETY: called by `dl_iterate_phdr` with a valid info and our vector.
        unsafe {
            let out = &mut *data.cast::<Vec<(u64, u64, bool)>>();
            let info = &*info;
            let phdrs = std::slice::from_raw_parts(info.dlpi_phdr, usize::from(info.dlpi_phnum));
            let base = info.dlpi_addr;
            for p in phdrs.iter().filter(|p| p.p_type == PT_GNU_RELRO) {
                let s = base.wrapping_add(p.p_vaddr) & !(PAGE - 1);
                out.push((s, base.wrapping_add(p.p_vaddr + p.p_memsz), false));
            }
            for p in phdrs.iter().filter(|p| p.p_type == PT_LOAD) {
                out.push((
                    base.wrapping_add(p.p_vaddr),
                    base.wrapping_add(p.p_vaddr + p.p_memsz),
                    p.p_flags & PF_W != 0,
                ));
            }
            0
        }
    }

    pub fn clear() {
        if let Ok(mut s) = SNAPSHOT.lock() {
            *s = None;
        }
    }

    pub fn writable(addr: usize, end: usize) -> bool {
        let (lo, hi) = (addr as u64, end as u64);
        let mut guard = SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner());
        let segs = guard.get_or_insert_with(|| {
            let mut v: Vec<(u64, u64, bool)> = Vec::new();
            // SAFETY: the callback only reads the program headers it is given.
            unsafe { dl_iterate_phdr(collect, (&raw mut v).cast()) };
            v
        });
        // RELRO entries come first per object: any overlap with one is read-only.
        let mut inside = None;
        for &(s, e, w) in segs.iter() {
            if lo < e && hi > s {
                if !w || !(lo >= s && hi <= e) {
                    return false;
                }
                inside = Some(true);
            }
        }
        inside.unwrap_or(true)
    }
}
