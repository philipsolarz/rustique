//! Path-string algorithms behind the `posixpath`, `fnmatch` and `pathlib` targets.
//!
//! Everything here works on slices of code units (`u8` for `bytes` and 1-byte `str`, `u16`
//! and `u32` for the wider PEP 393 kinds), so the Python layer can run them directly on a
//! string's buffer. Each function mirrors one piece of CPython's code; the doc comment
//! names it. No Python here: the bindings decide what object to return (identity rules).

pub mod glob;
pub mod posix;

/// A code unit of a path string: a byte of `bytes`, or a code point of a `str` stored in
/// one, two or four bytes.
pub trait Unit: Copy + Eq + Ord + 'static {
    const SLASH: Self;
    const DOT: Self;
    fn cp(self) -> u32;
}

impl Unit for u8 {
    const SLASH: Self = b'/';
    const DOT: Self = b'.';
    #[inline(always)]
    fn cp(self) -> u32 {
        u32::from(self)
    }
}

impl Unit for u16 {
    const SLASH: Self = b'/' as u16;
    const DOT: Self = b'.' as u16;
    #[inline(always)]
    fn cp(self) -> u32 {
        u32::from(self)
    }
}

impl Unit for u32 {
    const SLASH: Self = b'/' as u32;
    const DOT: Self = b'.' as u32;
    #[inline(always)]
    fn cp(self) -> u32 {
        self
    }
}
