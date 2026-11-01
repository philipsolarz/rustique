//! Lexical rules of `_json.c`'s scanner (`scan_once_unicode` and helpers), strict mode.
//!
//! Only *valid* input matters to callers: any error here means "let CPython's own decoder
//! run and raise", so the error ([`Invalid`]) carries no detail.

use super::Unit;

/// `IS_WHITESPACE(c)` (and `json.decoder.WHITESPACE`, `[ \t\n\r]*`).
#[inline(always)]
pub fn is_ws(c: u32) -> bool {
    c == 0x20 || c == 0x09 || c == 0x0A || c == 0x0D
}

/// Index of the first non-whitespace unit at or after `i`.
#[inline(always)]
pub fn skip_ws<T: Unit>(s: &[T], mut i: usize) -> usize {
    // Common case first: no whitespace at all.
    match s.get(i) {
        Some(c) if c.get() > 0x20 => return i,
        None => return i,
        _ => {}
    }
    while i < s.len() && is_ws(s[i].get()) {
        i += 1;
    }
    i
}

/// Index of the first byte at or after `i` that is `"`, `\\` or a C0 control (or `len`).
#[inline(always)]
fn find_special_u8(b: &[u8], mut i: usize) -> usize {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    while i + 8 <= b.len() {
        let mut w = [0u8; 8];
        w.copy_from_slice(&b[i..i + 8]);
        let x = u64::from_le_bytes(w);
        // Bytes < 0x20, == '"', == '\\' (exact SWAR tests; high-bit bytes never match).
        let lt = x.wrapping_sub(LO * 0x20) & !x & HI;
        let q = x ^ (LO * 0x22);
        let q = q.wrapping_sub(LO) & !q & HI;
        let bs = x ^ (LO * 0x5C);
        let bs = bs.wrapping_sub(LO) & !bs & HI;
        let m = lt | q | bs;
        if m != 0 {
            return i + (m.trailing_zeros() / 8) as usize;
        }
        i += 8;
    }
    while i < b.len() {
        let c = b[i];
        if c == b'"' || c == b'\\' || c < 0x20 {
            return i;
        }
        i += 1;
    }
    i
}

/// Input CPython's scanner rejects (it raises; callers let it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid;

/// A JSON string scanned from just after its opening quote.
#[derive(Debug, PartialEq, Eq)]
pub enum Str {
    /// No escapes: the content is `s[start..end]`; the closing quote is at `end`.
    Plain { end: usize },
    /// Escapes: the decoded code points were appended to the caller's buffer; `next` is the
    /// index after the closing quote and `max` the largest code point decoded.
    Escaped { next: usize, max: u32 },
}

#[inline(always)]
fn hex(c: u32) -> Option<u32> {
    match c {
        0x30..=0x39 => Some(c - 0x30),
        0x61..=0x66 => Some(c - 0x61 + 10),
        0x41..=0x46 => Some(c - 0x41 + 10),
        _ => None,
    }
}

#[inline(always)]
fn hex4<T: Unit>(s: &[T], at: usize) -> Option<u32> {
    let mut c = 0;
    for &u in &s[at..at + 4] {
        c = c << 4 | hex(u.get())?;
    }
    Some(c)
}

/// `scanstring_unicode(pystr, start, strict=1)`: `Err` wherever CPython raises.
#[inline]
pub fn string<T: Unit>(s: &[T], start: usize, out: &mut Vec<u32>) -> Result<Str, Invalid> {
    let len = s.len();
    let mut end = start;
    let mut escaped = false;
    let mut max = 0u32;
    loop {
        // Find the end of the string or the next escape.
        let mut next = end;
        let mut c = 0u32;
        if T::MAX == 0xFF {
            // SAFETY: `T` is `u8` (the only unit with MAX 0xFF).
            let b = unsafe { std::slice::from_raw_parts(s.as_ptr().cast::<u8>(), s.len()) };
            next = find_special_u8(b, next);
            if next < len {
                c = u32::from(b[next]);
            }
        } else {
            while next < len {
                c = s[next].get();
                if c == u32::from(b'"') || c == u32::from(b'\\') || c <= 0x1F {
                    break;
                }
                next += 1;
            }
        }
        if next >= len {
            return Err(Invalid); // Unterminated string
        }
        if c <= 0x1F {
            return Err(Invalid); // Invalid control character
        }
        if c == u32::from(b'"') && !escaped {
            return Ok(Str::Plain { end: next });
        }
        escaped = true;
        for &u in &s[end..next] {
            let v = u.get();
            max = max.max(v);
            out.push(v);
        }
        next += 1;
        if c == u32::from(b'"') {
            return Ok(Str::Escaped { next, max });
        }
        if next == len {
            return Err(Invalid); // Unterminated string
        }
        let e = s[next].get();
        let ch;
        if e != u32::from(b'u') {
            end = next + 1;
            ch = match e {
                0x22 | 0x5C | 0x2F => e,
                0x62 => 0x08,
                0x66 => 0x0C,
                0x6E => 0x0A,
                0x72 => 0x0D,
                0x74 => 0x09,
                _ => return Err(Invalid), // Invalid \escape
            };
        } else {
            next += 1;
            end = next + 4;
            if end > len {
                return Err(Invalid); // Invalid \uXXXX escape
            }
            let mut c1 = hex4(s, next).ok_or(Invalid)?;
            // Surrogate pair: `end + 6 < len` and `\u` follow (CPython's exact condition).
            if (0xD800..=0xDBFF).contains(&c1)
                && end + 6 < len
                && s[end].get() == u32::from(b'\\')
                && s[end + 1].get() == u32::from(b'u')
            {
                let c2 = hex4(s, end + 2).ok_or(Invalid)?;
                if (0xDC00..=0xDFFF).contains(&c2) {
                    c1 = 0x1_0000 + ((c1 - 0xD800) << 10 | (c2 - 0xDC00));
                    end += 6;
                }
            }
            ch = c1;
        }
        max = max.max(ch);
        out.push(ch);
    }
}

/// A number token: `s[start..end]`, a float when it has a fraction or an exponent.
#[derive(Debug, PartialEq, Eq)]
pub struct Num {
    pub end: usize,
    pub is_float: bool,
}

#[inline(always)]
fn digit(c: u32) -> bool {
    (0x30..=0x39).contains(&c)
}

/// `_match_number_unicode` without the conversion: `None` where it raises StopIteration
/// (no number at `start`). Reproduces its look-ahead rules (`idx < end_idx` for the
/// fraction and exponent, backtracking over an exponent without digits).
#[inline]
pub fn number<T: Unit>(s: &[T], start: usize) -> Option<Num> {
    let len = s.len();
    let end_idx = len as isize - 1;
    let at = |i: usize| s[i].get();
    let mut idx = start;
    if at(idx) == u32::from(b'-') {
        idx += 1;
        if idx as isize > end_idx {
            return None;
        }
    }
    let c = at(idx);
    if (0x31..=0x39).contains(&c) {
        idx += 1;
        while (idx as isize) <= end_idx && digit(at(idx)) {
            idx += 1;
        }
    } else if c == u32::from(b'0') {
        idx += 1;
    } else {
        return None;
    }
    let mut is_float = false;
    if (idx as isize) < end_idx && at(idx) == u32::from(b'.') && digit(at(idx + 1)) {
        is_float = true;
        idx += 2;
        while (idx as isize) <= end_idx && digit(at(idx)) {
            idx += 1;
        }
    }
    if (idx as isize) < end_idx && (at(idx) == u32::from(b'e') || at(idx) == u32::from(b'E')) {
        let e_start = idx;
        idx += 1;
        if (idx as isize) < end_idx && (at(idx) == u32::from(b'-') || at(idx) == u32::from(b'+')) {
            idx += 1;
        }
        while (idx as isize) <= end_idx && digit(at(idx)) {
            idx += 1;
        }
        if digit(at(idx - 1)) {
            is_float = true;
        } else {
            idx = e_start;
        }
    }
    Some(Num { end: idx, is_float })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> Vec<u32> {
        s.chars().map(u32::from).collect()
    }

    fn num(s: &str) -> Option<(usize, bool)> {
        number(&units(s), 0).map(|n| (n.end, n.is_float))
    }

    #[test]
    fn numbers_like_cpython() {
        assert_eq!(num("0"), Some((1, false)));
        assert_eq!(num("-0"), Some((2, false)));
        assert_eq!(num("01"), Some((1, false)));
        assert_eq!(num("123,"), Some((3, false)));
        assert_eq!(num("1.5"), Some((3, true)));
        assert_eq!(num("1."), Some((1, false)));
        assert_eq!(num("1.]"), Some((1, false)));
        assert_eq!(num("1e5"), Some((3, true)));
        assert_eq!(num("1E+5"), Some((4, true)));
        assert_eq!(num("1e-05]"), Some((5, true)));
        assert_eq!(num("1e"), Some((1, false)));
        assert_eq!(num("1e+"), Some((1, false)));
        assert_eq!(num("2e-"), Some((1, false)));
        assert_eq!(num("1.5e"), Some((3, true)));
        assert_eq!(num("-"), None);
        assert_eq!(num("-a"), None);
        assert_eq!(num("+1"), None);
        assert_eq!(num(".5"), None);
        assert_eq!(num("-1.25e+10x"), Some((9, true)));
    }

    fn st(s: &str) -> Result<(Str, String), Invalid> {
        let u = units(s);
        let mut out = Vec::new();
        let r = string(&u, 1, &mut out)?;
        let text = match r {
            Str::Plain { end } => u[1..end].iter().map(|&c| char::from_u32(c).unwrap_or('?')).collect(),
            Str::Escaped { .. } => out.iter().map(|&c| char::from_u32(c).unwrap_or('?')).collect(),
        };
        Ok((r, text))
    }

    #[test]
    fn strings_like_cpython() {
        assert_eq!(st("\"abc\"").unwrap(), (Str::Plain { end: 4 }, "abc".into()));
        assert_eq!(st("\"\"").unwrap(), (Str::Plain { end: 1 }, "".into()));
        assert_eq!(st(r#""a\nb""#).unwrap().1, "a\nb");
        assert_eq!(st(r#""\"\\\/\b\f\n\r\t""#).unwrap().1, "\"\\/\x08\x0c\n\r\t");
        assert_eq!(st(r#""\u00e9\u20AC""#).unwrap().1, "é€");
        assert_eq!(st(r#""\ud83d\ude00""#).unwrap().1, "\u{1F600}");
        assert!(matches!(st(r#""\ud83d\ude00""#).unwrap().0, Str::Escaped { next: 14, max: 0x1F600 }));
        assert!(st("\"a").is_err());
        assert!(st("\"a\\").is_err());
        assert!(st("\"\\x\"").is_err());
        assert!(st("\"\\u12\"").is_err());
        assert!(st("\"\\u12g4\"").is_err());
        assert!(st("\"a\nb\"").is_err());
        assert!(st("\"a\x7fb\"").is_ok());
        // High surrogate followed by a non-low escape: kept alone, then the next escape.
        let u = units(r#""\ud83d\u0041""#);
        let mut out = Vec::new();
        assert!(string(&u, 1, &mut out).is_ok());
        assert_eq!(out, vec![0xD83D, 0x41]);
        // Lone surrogates are kept.
        let u = units(r#""\udc00x""#);
        let mut out = Vec::new();
        assert!(string(&u, 1, &mut out).is_ok());
        assert_eq!(out, vec![0xDC00, 0x78]);
        // A bad second escape after a high surrogate is an error (CPython raises).
        assert!(st(r#""\ud83d\uZZZZ""#).is_err());
        // The pair needs `end + 6 < len`: at the very end there is no room for the quote.
        let u = units(r#""\ud83d\ude00"#);
        assert!(string(&u, 1, &mut Vec::new()).is_err());
    }

    #[test]
    fn special_scan_matches_naive() {
        let mut b: Vec<u8> = (0u8..=255).collect();
        b.extend_from_slice(b"abcdefghijklmnopqrstuvwxyz\"0123456789\\");
        for start in 0..b.len() {
            let naive = (start..b.len()).find(|&i| b[i] == b'"' || b[i] == b'\\' || b[i] < 0x20).unwrap_or(b.len());
            let mut c = b.clone();
            // Also with every byte before `start` special and none after up to the hit.
            c[..start].fill(b'"');
            assert_eq!(find_special_u8(&b, start), naive, "start {start}");
            let naive_c = (start..c.len()).find(|&i| c[i] == b'"' || c[i] == b'\\' || c[i] < 0x20).unwrap_or(c.len());
            assert_eq!(find_special_u8(&c, start), naive_c);
        }
        for n in 0..40 {
            let v = vec![b'x'; n];
            assert_eq!(find_special_u8(&v, 0), n);
            let mut w = vec![0xE9u8; n];
            w.push(b'"');
            assert_eq!(find_special_u8(&w, 0), n);
        }
    }

    #[test]
    fn whitespace() {
        let u = units(" \t\n\r x");
        assert_eq!(skip_ws(&u, 0), 5);
        assert!(!is_ws(0x0B) && !is_ws(0x0C) && !is_ws(0xA0));
    }
}
