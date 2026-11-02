//! Allocation-free access to `str` objects for the `textwrap` and `urllib` kernels: read the
//! PEP 393 buffer of an argument in place (no `Vec<u32>` copy), build a result `str`
//! directly in its final canonical form (`PyUnicode_New` with the exact maximum character,
//! then write), and per-thread scratch buffers that are reused from call to call.
//!
//! Why: in wave 1 these kernels spent 15–45% of their instructions in `malloc`/`free`/
//! `realloc` of per-call `Vec`s and UCS-4 round trips (docs/atlas/w2-floors.md §1), and on
//! free-threaded builds every per-call allocation went through glibc's arena lock (`quote`:
//! CPU ×19.6 per call at 8 threads, docs/atlas/w2-threads.md §3.1).

use std::cell::RefCell;

use pyo3::ffi;

pub type O = *mut ffi::PyObject;

/// The code units of a `str`, borrowed from its PEP 393 buffer.
#[derive(Clone, Copy)]
pub enum Units<'a> {
    L1(&'a [u8]),
    U2(&'a [u16]),
    U4(&'a [u32]),
}

/// A code unit of one of the three PEP 393 kinds.
pub trait Unit: Copy + Ord + Into<u32> {}
impl Unit for u8 {}
impl Unit for u16 {}
impl Unit for u32 {}

/// Run `$body` with `$t` bound to the unit slice of `$u`, monomorphised per kind.
macro_rules! with_units {
    ($u:expr, $t:ident => $body:expr) => {
        match $u {
            $crate::targets::textwrap::pep393::Units::L1($t) => $body,
            $crate::targets::textwrap::pep393::Units::U2($t) => $body,
            $crate::targets::textwrap::pep393::Units::U4($t) => $body,
        }
    };
}
pub(crate) use with_units;

impl Units<'_> {
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Units::L1(t) => t.len(),
            Units::U2(t) => t.len(),
            Units::U4(t) => t.len(),
        }
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Code point `i`.
    #[inline]
    pub fn at(&self, i: usize) -> u32 {
        match self {
            Units::L1(t) => u32::from(t[i]),
            Units::U2(t) => u32::from(t[i]),
            Units::U4(t) => t[i],
        }
    }
    /// Append the code points to `out`.
    #[inline]
    pub fn push_to(&self, out: &mut Vec<u32>) {
        with_units!(*self, t => out.extend(t.iter().map(|&c| u32::from(c))))
    }
}

/// The units of an exact (or subclass) `str`.
///
/// # Safety
/// `s` is a live `str`; the slice is used while `s` is alive.
#[inline]
pub unsafe fn units<'a>(s: O) -> Units<'a> {
    // SAFETY: per the caller (compact strings: PyUnicode_DATA is the inline buffer).
    unsafe {
        let n = ffi::PyUnicode_GET_LENGTH(s) as usize;
        let d = ffi::PyUnicode_DATA(s);
        match ffi::PyUnicode_KIND(s) {
            ffi::PyUnicode_1BYTE_KIND => Units::L1(std::slice::from_raw_parts(d.cast::<u8>(), n)),
            ffi::PyUnicode_2BYTE_KIND => Units::U2(std::slice::from_raw_parts(d.cast::<u16>(), n)),
            _ => Units::U4(std::slice::from_raw_parts(d.cast::<u32>(), n)),
        }
    }
}

/// Writes code points into a fresh `str` of a known length and maximum character.
///
/// A result of one Latin-1 character is CPython's cached singleton (`PyUnicode_FromOrdinal`),
/// as every CPython string builder returns it (`unicode_result`, `PyUnicode_Substring`,
/// `_PyUnicode_FromUCS*`): the identity of such results is observable (`is`), and the
/// empty string is `PyUnicode_New(0, ..)`'s singleton.
pub struct StrWriter {
    obj: O,
    data: *mut u8,
    /// PEP 393 kind, or 0: a single Latin-1 character (`single`).
    kind: u32,
    single: u32,
    pos: usize,
    #[cfg(debug_assertions)]
    len: usize,
}

impl StrWriter {
    /// `PyUnicode_New(len, maxchar)`; `None` (error cleared) on failure.
    ///
    /// # Safety
    /// Attached thread. `maxchar` must be the exact maximum of what will be written (the
    /// result must be canonical) and exactly `len` code points must be written before
    /// [`StrWriter::finish`].
    #[inline]
    pub unsafe fn new(len: usize, maxchar: u32) -> Option<StrWriter> {
        // SAFETY: per the caller.
        unsafe { Self::with(len, maxchar, true) }
    }

    /// As [`StrWriter::new`], but a one-character result is a new object, never the
    /// Latin-1 singleton: what CPython's `''.join` of several parts and `str.replace`
    /// return (`PyUnicode_New` without `unicode_result`).
    ///
    /// # Safety
    /// As [`StrWriter::new`].
    #[inline]
    pub unsafe fn fresh(len: usize, maxchar: u32) -> Option<StrWriter> {
        // SAFETY: per the caller.
        unsafe { Self::with(len, maxchar, false) }
    }

    #[inline]
    unsafe fn with(len: usize, maxchar: u32, singleton: bool) -> Option<StrWriter> {
        // SAFETY: per the caller.
        unsafe {
            if singleton && len == 1 && maxchar < 256 {
                return Some(StrWriter {
                    obj: std::ptr::null_mut(),
                    data: std::ptr::null_mut(),
                    kind: 0,
                    single: 0,
                    pos: 0,
                    #[cfg(debug_assertions)]
                    len,
                });
            }
            let obj = ffi::PyUnicode_New(len as ffi::Py_ssize_t, maxchar);
            if obj.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            Some(StrWriter {
                obj,
                data: ffi::PyUnicode_DATA(obj).cast(),
                kind: ffi::PyUnicode_KIND(obj),
                single: 0,
                pos: 0,
                #[cfg(debug_assertions)]
                len,
            })
        }
    }

    /// Append code points (each ≤ the declared `maxchar`).
    #[inline]
    pub fn put<T: Unit>(&mut self, s: &[T]) {
        #[cfg(debug_assertions)]
        assert!(self.pos + s.len() <= self.len);
        // SAFETY: room for `s` (the caller's length contract); the kind fits every value.
        unsafe {
            match self.kind {
                0 => {
                    if let Some(&c) = s.first() {
                        self.single = c.into();
                    }
                }
                ffi::PyUnicode_1BYTE_KIND => {
                    let d = self.data.add(self.pos);
                    for (i, &c) in s.iter().enumerate() {
                        *d.add(i) = c.into() as u8;
                    }
                }
                ffi::PyUnicode_2BYTE_KIND => {
                    let d = self.data.cast::<u16>().add(self.pos);
                    for (i, &c) in s.iter().enumerate() {
                        *d.add(i) = c.into() as u16;
                    }
                }
                _ => {
                    let d = self.data.cast::<u32>().add(self.pos);
                    for (i, &c) in s.iter().enumerate() {
                        *d.add(i) = c.into();
                    }
                }
            }
        }
        self.pos += s.len();
    }

    /// Append one code point.
    #[inline]
    pub fn put1(&mut self, c: u32) {
        self.put(&[c]);
    }

    /// Append ASCII bytes (any kind).
    #[inline]
    pub fn put_ascii(&mut self, s: &[u8]) {
        if self.kind == ffi::PyUnicode_1BYTE_KIND {
            #[cfg(debug_assertions)]
            assert!(self.pos + s.len() <= self.len);
            // SAFETY: room for `s`.
            unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), self.data.add(self.pos), s.len()) };
            self.pos += s.len();
        } else {
            self.put(s);
        }
    }

    /// The new `str` (all `len` code points written); NULL with an error set on failure.
    ///
    /// # Safety
    /// Attached thread.
    pub unsafe fn finish(self) -> O {
        #[cfg(debug_assertions)]
        assert_eq!(self.pos, self.len);
        if self.kind == 0 {
            // SAFETY: attached thread; a Latin-1 code point: the cached singleton.
            return unsafe { ffi::PyUnicode_FromOrdinal(self.single as std::ffi::c_int) };
        }
        self.obj
    }
}

/// The largest code point of `s` (0 when empty).
#[inline]
pub fn max_cp<T: Unit>(s: &[T]) -> u32 {
    s.iter().fold(0u32, |m, &c| m.max(c.into()))
}

/// A new `str` with the code points of `v` (canonical kind); NULL with an error set on failure.
///
/// # Safety
/// Attached thread.
pub unsafe fn str_from_cps<T: Unit>(v: &[T]) -> O {
    // SAFETY: per the caller; exactly `v.len()` points of maximum `max_cp(v)` written.
    unsafe {
        let Some(mut w) = StrWriter::new(v.len(), max_cp(v)) else {
            return ffi::PyUnicode_New(v.len() as ffi::Py_ssize_t, max_cp(v)); // re-raise the error
        };
        w.put(v);
        w.finish()
    }
}

/// A new `str` from ASCII bytes; NULL with an error set on failure.
///
/// # Safety
/// Attached thread; `v` is ASCII.
pub unsafe fn str_from_ascii(v: &[u8]) -> O {
    // SAFETY: per the caller.
    unsafe {
        let Some(mut w) = StrWriter::new(v.len(), 127) else {
            return ffi::PyUnicode_New(v.len() as ffi::Py_ssize_t, 127); // re-raise
        };
        w.put_ascii(v);
        w.finish()
    }
}

/// Scratch buffers kept per thread (cleared, never shrunk below [`KEEP`] elements).
#[derive(Default)]
pub struct Bufs {
    pub u: [Vec<u32>; 4],
    pub b: [Vec<u8>; 2],
}

/// Capacity (elements) each scratch buffer keeps between calls; larger ones are released
/// after the call that grew them, so a huge input does not pin memory in a thread.
pub const KEEP: usize = 1 << 16;

impl Bufs {
    pub const fn new() -> Bufs {
        Bufs { u: [Vec::new(), Vec::new(), Vec::new(), Vec::new()], b: [Vec::new(), Vec::new()] }
    }
    fn trim(&mut self) {
        for v in &mut self.u {
            v.clear();
            if v.capacity() > KEEP {
                v.shrink_to(KEEP);
            }
        }
        for v in &mut self.b {
            v.clear();
            if v.capacity() > KEEP {
                v.shrink_to(KEEP);
            }
        }
    }
}

thread_local! {
    static BUFS: RefCell<Bufs> = const { RefCell::new(Bufs::new()) };
}

/// Run `f` with this thread's scratch buffers (cleared). Re-entrant: a nested use (no
/// kernel calls Python code while it holds them, but a finalizer could) gets fresh ones.
#[inline]
pub fn with_bufs<R>(f: impl FnOnce(&mut Bufs) -> R) -> R {
    let mut f = Some(f);
    let r = BUFS.try_with(|b| match b.try_borrow_mut() {
        Ok(mut g) => {
            let r = (f.take().expect("once"))(&mut g);
            g.trim();
            Some(r)
        }
        Err(_) => None,
    });
    match r {
        Ok(Some(r)) => r,
        // Nested use, or the thread is exiting: one-off buffers.
        _ => (f.take().expect("once"))(&mut Bufs::new()),
    }
}
