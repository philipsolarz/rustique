//! JSON string escaping exactly as `_json.c` does it.
//!
//! - `ensure_ascii=True` (`ascii_escape_unichar`): printable ASCII except `\` and `"` is
//!   copied; `\\ \" \b \f \n \r \t` use their short forms; everything else becomes
//!   `\uXXXX` with lowercase hex, code points above U+FFFF as a UTF-16 surrogate pair (DEL
//!   U+007F and lone surrogates included).
//! - `ensure_ascii=False` (`escape_unicode`): only `\\ \"`, the short forms and the other
//!   C0 controls (`\u00XX`) are escaped; every other code point is copied unchanged.

use super::Unit;

const HEX: &[u8; 16] = b"0123456789abcdef";

/// `S_CHAR(c)` of `_json.c`: written as is under `ensure_ascii`.
#[inline(always)]
pub const fn s_char(c: u32) -> bool {
    c >= 0x20 && c <= 0x7E && c != b'\\' as u32 && c != b'"' as u32
}

/// For each byte: 0 = copy, 1 = needs escaping under `ensure_ascii`.
const ASCII_ESCAPE: [u8; 256] = {
    let mut t = [1u8; 256];
    let mut c = 0x20;
    while c <= 0x7E {
        t[c] = 0;
        c += 1;
    }
    t[b'\\' as usize] = 1;
    t[b'"' as usize] = 1;
    t
};

/// For each byte: 0 = copy, 1 = needs escaping without `ensure_ascii`.
const UNICODE_ESCAPE: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut c = 0;
    while c < 0x20 {
        t[c] = 1;
        c += 1;
    }
    t[b'\\' as usize] = 1;
    t[b'"' as usize] = 1;
    t
};

#[inline(always)]
fn push_u4(out: &mut Vec<u8>, c: u32) {
    out.extend_from_slice(&[
        b'\\',
        b'u',
        HEX[(c >> 12 & 0xF) as usize],
        HEX[(c >> 8 & 0xF) as usize],
        HEX[(c >> 4 & 0xF) as usize],
        HEX[(c & 0xF) as usize],
    ]);
}

/// The escape of one code point that is not an `S_CHAR` (`ascii_escape_unichar`).
#[inline]
pub fn push_ascii_escape(out: &mut Vec<u8>, c: u32) {
    match c {
        0x5C => out.extend_from_slice(b"\\\\"),
        0x22 => out.extend_from_slice(b"\\\""),
        0x08 => out.extend_from_slice(b"\\b"),
        0x0C => out.extend_from_slice(b"\\f"),
        0x0A => out.extend_from_slice(b"\\n"),
        0x0D => out.extend_from_slice(b"\\r"),
        0x09 => out.extend_from_slice(b"\\t"),
        _ if c >= 0x1_0000 => {
            // Py_UNICODE_HIGH_SURROGATE / Py_UNICODE_LOW_SURROGATE
            push_u4(out, 0xD800 - (0x1_0000 >> 10) + (c >> 10));
            push_u4(out, 0xDC00 + (c & 0x3FF));
        }
        _ => push_u4(out, c),
    }
}

/// `"` + `s` escaped for `ensure_ascii=True` + `"` (`write_escaped_ascii`).
#[inline]
pub fn push_ascii_quoted<T: Unit>(out: &mut Vec<u8>, s: &[T]) {
    out.reserve(s.len() + 2);
    out.push(b'"');
    if T::MAX == 0xFF {
        // SAFETY: `T` is `u8` (the only unit with MAX 0xFF).
        let b = unsafe { std::slice::from_raw_parts(s.as_ptr().cast::<u8>(), s.len()) };
        let mut start = 0;
        for (i, &c) in b.iter().enumerate() {
            if ASCII_ESCAPE[c as usize] != 0 {
                out.extend_from_slice(&b[start..i]);
                push_ascii_escape(out, u32::from(c));
                start = i + 1;
            }
        }
        out.extend_from_slice(&b[start..]);
    } else {
        for &u in s {
            let c = u.get();
            if s_char(c) {
                out.push(c as u8);
            } else {
                push_ascii_escape(out, c);
            }
        }
    }
    out.push(b'"');
}

/// Output text of any PEP 393 width, widened on demand: what `ensure_ascii=False` and
/// non-ASCII separators produce. `max` is the largest code point written so far (exactly
/// for ASCII vs Latin-1; wider widths are only entered for code points that need them),
/// so `PyUnicode_New(len, max)` gives CPython's canonical string.
#[derive(Debug)]
pub enum Text {
    K1(Vec<u8>, u32),
    K2(Vec<u16>),
    K4(Vec<u32>),
}

impl Default for Text {
    fn default() -> Self {
        Text::K1(Vec::new(), 0x7F)
    }
}

impl Text {
    pub fn with_capacity(n: usize) -> Self {
        Text::K1(Vec::with_capacity(n), 0x7F)
    }

    pub fn clear(&mut self) {
        match self {
            Text::K1(v, m) => {
                v.clear();
                *m = 0x7F;
            }
            _ => *self = Text::default(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Text::K1(v, _) => v.len(),
            Text::K2(v) => v.len(),
            Text::K4(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Largest code point written (at least 0x7F).
    pub fn max_char(&self) -> u32 {
        match self {
            Text::K1(_, m) => *m,
            // Only entered for a code point that needs the width (canonical inputs).
            Text::K2(_) => 0xFFFF,
            Text::K4(_) => 0x10_FFFF,
        }
    }

    /// Make room for code points up to `max`.
    #[inline]
    fn widen(&mut self, max: u32) {
        match self {
            Text::K1(v, m) if max <= 0xFF => {
                if max > *m {
                    *m = max;
                }
                let _ = v;
            }
            Text::K1(v, _) if max <= 0xFFFF => *self = Text::K2(v.iter().map(|&b| u16::from(b)).collect()),
            Text::K1(v, _) => *self = Text::K4(v.iter().map(|&b| u32::from(b)).collect()),
            Text::K2(v) if max > 0xFFFF => *self = Text::K4(v.iter().map(|&b| u32::from(b)).collect()),
            _ => {}
        }
    }

    /// Append ASCII bytes.
    #[inline]
    pub fn push_ascii(&mut self, s: &[u8]) {
        match self {
            Text::K1(v, _) => v.extend_from_slice(s),
            Text::K2(v) => v.extend(s.iter().map(|&b| u16::from(b))),
            Text::K4(v) => v.extend(s.iter().map(|&b| u32::from(b))),
        }
    }

    /// Append code units whose largest code point is `max` (see the type docs).
    #[inline]
    pub fn push_units<T: Unit>(&mut self, s: &[T], max: u32) {
        self.widen(max);
        match self {
            Text::K1(v, _) => v.extend(s.iter().map(|&u| u.get() as u8)),
            Text::K2(v) => v.extend(s.iter().map(|&u| u.get() as u16)),
            Text::K4(v) => v.extend(s.iter().map(|&u| u.get())),
        }
    }

    /// `"` + `s` escaped for `ensure_ascii=False` + `"` (`write_escaped_unicode`); `max` as
    /// for [`Text::push_units`].
    #[inline]
    pub fn push_unicode_quoted<T: Unit>(&mut self, s: &[T], max: u32) {
        self.widen(max);
        match self {
            Text::K1(v, _) => push_unicode_quoted_into(v, s),
            Text::K2(v) => push_unicode_quoted_into(v, s),
            Text::K4(v) => push_unicode_quoted_into(v, s),
        }
    }
}

#[inline(always)]
fn esc_byte(c: u32) -> bool {
    c < 0x100 && UNICODE_ESCAPE[c as usize] != 0
}

fn push_unicode_quoted_into<T: Unit, O: Unit>(out: &mut Vec<O>, s: &[T]) {
    out.reserve(s.len() + 2);
    out.push(O::from_u32(u32::from(b'"')));
    let mut start = 0;
    for (i, &u) in s.iter().enumerate() {
        let c = u.get();
        if esc_byte(c) {
            out.extend(s[start..i].iter().map(|&x| O::from_u32(x.get())));
            let short = match c {
                0x5C => b'\\',
                0x22 => b'"',
                0x08 => b'b',
                0x0C => b'f',
                0x0A => b'n',
                0x0D => b'r',
                0x09 => b't',
                _ => 0,
            };
            let tmp: [u8; 6];
            let esc: &[u8] = if short != 0 {
                tmp = [b'\\', short, 0, 0, 0, 0];
                &tmp[..2]
            } else {
                tmp = [b'\\', b'u', b'0', b'0', HEX[(c >> 4) as usize], HEX[(c & 0xF) as usize]];
                &tmp
            };
            out.extend(esc.iter().map(|&b| O::from_u32(u32::from(b))));
            start = i + 1;
        }
    }
    out.extend(s[start..].iter().map(|&x| O::from_u32(x.get())));
    out.push(O::from_u32(u32::from(b'"')));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascii(s: &str) -> String {
        let units: Vec<u32> = s.chars().map(u32::from).collect();
        let mut out = Vec::new();
        push_ascii_quoted(&mut out, &units);
        let mut out8 = Vec::new();
        if s.chars().all(|c| (c as u32) < 256) {
            let b: Vec<u8> = s.chars().map(|c| c as u8).collect();
            push_ascii_quoted(&mut out8, &b);
            assert_eq!(out, out8);
        }
        String::from_utf8(out).unwrap()
    }

    fn unicode(s: &str) -> String {
        let units: Vec<u32> = s.chars().map(u32::from).collect();
        let mut t = Text::default();
        t.push_unicode_quoted(&units, units.iter().copied().max().unwrap_or(0).max(0x7F));
        match t {
            Text::K1(v, _) => v.iter().map(|&b| char::from(b)).collect(),
            Text::K2(v) => v.iter().map(|&u| char::from_u32(u32::from(u)).unwrap()).collect(),
            Text::K4(v) => v.iter().map(|&u| char::from_u32(u).unwrap()).collect(),
        }
    }

    /// Expected values printed by CPython 3.15.1 (`json.dumps(s)` / `ensure_ascii=False`).
    #[test]
    fn matches_cpython() {
        assert_eq!(ascii("abc"), r#""abc""#);
        assert_eq!(ascii("a\"b\\c/d"), r#""a\"b\\c/d""#);
        assert_eq!(ascii("\x08\x0c\n\r\t"), r#""\b\f\n\r\t""#);
        assert_eq!(ascii("\x00\x01\x1f\x7f"), r#""\u0000\u0001\u001f\u007f""#);
        assert_eq!(ascii("é\u{100}\u{FFFF}"), r#""\u00e9\u0100\uffff""#);
        assert_eq!(ascii("\u{1F600}"), r#""\ud83d\ude00""#);
        assert_eq!(ascii("\u{10FFFF}"), r#""\udbff\udfff""#);
        assert_eq!(unicode("abc"), r#""abc""#);
        assert_eq!(unicode("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(unicode("\x08\x0c\n\r\t\x00\x1f\x7f"), "\"\\b\\f\\n\\r\\t\\u0000\\u001f\x7f\"");
        assert_eq!(unicode("é€\u{1F600}"), "\"é€\u{1F600}\"");
    }

    #[test]
    fn text_widens() {
        let mut t = Text::default();
        t.push_ascii(b"ab");
        assert_eq!(t.max_char(), 0x7F);
        t.push_units(&[0xE9u8], 0xFF);
        assert!(matches!(t, Text::K1(_, 0xFF)));
        t.push_units(&[0x20ACu16], 0xFFFF);
        assert!(matches!(t, Text::K2(_)));
        t.push_units(&[0x1F600u32], 0x10_FFFF);
        match &t {
            Text::K4(v) => assert_eq!(v, &[0x61, 0x62, 0xE9, 0x20AC, 0x1F600]),
            _ => panic!(),
        }
        assert_eq!(t.len(), 5);
    }
}
