//! Pure pieces of CPython's `json` (`Lib/json`, `Modules/_json.c`), reproduced exactly:
//! string escaping for `json.dumps` ([`escape`]), integer text ([`itoa`]), and the
//! scanner's lexical rules for `json.loads` ([`scan`]: whitespace, strings with their
//! escapes, numbers with `_match_number_unicode`'s quirks).
//!
//! Everything works on PEP 393 code-unit slices (`u8` for ASCII/Latin-1 strings, `u16`,
//! `u32`) through [`Unit`], so callers read CPython's string buffers directly.

pub mod escape;
pub mod itoa;
pub mod scan;

/// A code unit of a PEP 393 string buffer (`Py_UCS1`, `Py_UCS2`, `Py_UCS4`).
pub trait Unit: Copy + Eq + Ord + Default + 'static {
    /// Largest code point this unit width can hold (the string kind's `maxchar`).
    const MAX: u32;
    fn get(self) -> u32;
    /// Truncating conversion; callers only store code points `<= Self::MAX`.
    fn from_u32(c: u32) -> Self;
}

impl Unit for u8 {
    const MAX: u32 = 0xFF;
    #[inline(always)]
    fn get(self) -> u32 {
        u32::from(self)
    }
    #[inline(always)]
    fn from_u32(c: u32) -> Self {
        c as u8
    }
}

impl Unit for u16 {
    const MAX: u32 = 0xFFFF;
    #[inline(always)]
    fn get(self) -> u32 {
        u32::from(self)
    }
    #[inline(always)]
    fn from_u32(c: u32) -> Self {
        c as u16
    }
}

impl Unit for u32 {
    const MAX: u32 = 0x10_FFFF;
    #[inline(always)]
    fn get(self) -> u32 {
        self
    }
    #[inline(always)]
    fn from_u32(c: u32) -> Self {
        c
    }
}
