//! `urllib.parse` algorithms, bit for bit with CPython 3.14/3.15: `quote_from_bytes`'s
//! byte mapping, `_unquote_impl`'s percent decoding, the ASCII-run split of `unquote`,
//! the field splitting of `parse_qsl`, and `_urlsplit` (over code points, returning
//! component ranges so the bindings can slice the original `str` objects as CPython does).

/// `_ALWAYS_SAFE`: `A-Z a-z 0-9 _ . - ~`.
#[inline]
pub fn always_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~')
}

/// The safe set of `quote_from_bytes(bs, safe)` for a `str` `safe`:
/// `_ALWAYS_SAFE | safe.encode('ascii', 'ignore')`.
pub fn safe_table(safe: impl IntoIterator<Item = u32>) -> [bool; 256] {
    let mut t = [false; 256];
    for b in 0..=255u8 {
        t[b as usize] = always_safe(b);
    }
    for c in safe {
        if c < 128 {
            t[c as usize] = true;
        }
    }
    t
}

/// `quote_from_bytes(bs, safe)` (non-empty `bs`): safe bytes as themselves, others as
/// `%XX` (upper-case hex). The output is ASCII.
pub fn quote_bytes(bs: &[u8], safe: &[bool; 256], out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.reserve(bs.len());
    for &b in bs {
        if safe[b as usize] {
            out.push(b);
        } else {
            out.extend_from_slice(&[b'%', HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]);
        }
    }
}

#[inline]
fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `_unquote_impl(bytes)`: `bits = s.split(b'%')`, then each later bit starts with two hex
/// digits (`_hextobyte[item[:2]]`) → that byte + the rest, else `%` + the bit.
pub fn unquote_bytes(s: &[u8], out: &mut Vec<u8>) {
    let mut bits = s.split(|&b| b == b'%');
    out.extend_from_slice(bits.next().unwrap_or(&[]));
    for item in bits {
        if item.len() >= 2
            && let (Some(h), Some(l)) = (hexval(item[0]), hexval(item[1]))
        {
            out.push(h << 4 | l);
            out.extend_from_slice(&item[2..]);
        } else {
            out.push(b'%');
            out.extend_from_slice(item);
        }
    }
}

/// `_WHATWG_C0_CONTROL_OR_SPACE`: `\x00`-`\x20`.
#[inline]
pub fn is_c0_or_space(c: u32) -> bool {
    c <= 0x20
}

/// `_UNSAFE_URL_BYTES_TO_REMOVE`: tab, CR, LF.
#[inline]
pub fn is_unsafe_url_char(c: u32) -> bool {
    matches!(c, 0x09 | 0x0a | 0x0d)
}

/// `urllib.parse.scheme_chars`: ASCII letters, digits and `+-.`.
#[inline]
pub fn is_scheme_char(c: u32) -> bool {
    c < 128 && ((c as u8).is_ascii_alphanumeric() || matches!(c as u8, b'+' | b'-' | b'.'))
}

/// A component of `_urlsplit`'s result as a range of the cleaned `url` (`None`: absent).
pub type Range = Option<(usize, usize)>;

/// Result of [`urlsplit`]: ranges into the cleaned url; `scheme_from_url` tells whether
/// the scheme was taken from the url (then CPython lower-cases it) or is the argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub scheme_from_url: Range,
    pub netloc: Range,
    pub path: (usize, usize),
    pub query: Range,
    pub fragment: Range,
}

/// Why `_urlsplit` must run in CPython (it raises, or needs `ipaddress`/`unicodedata`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDefer {
    /// Brackets in the netloc (`ValueError` or `_check_bracketed_netloc`).
    Brackets,
    /// Non-ASCII netloc (`_checknetloc`'s NFKC check).
    NonAsciiNetloc,
}

/// `_urlsplit(url, scheme, allow_fragments)` on an already cleaned url (`url.lstrip(C0)`
/// with tab/CR/LF removed): the component ranges.
pub fn urlsplit(url: &[u32], allow_fragments: bool) -> Result<Split, SplitDefer> {
    let n = url.len();
    let mut start = 0;
    let mut scheme_from_url = None;
    // i = url.find(':'); if i > 0 and url[0].isascii() and url[0].isalpha(): all scheme_chars?
    if let Some(i) = url.iter().position(|&c| c == 0x3a)
        && i > 0
        && url[0] < 128
        && (url[0] as u8).is_ascii_alphabetic()
        && url[..i].iter().all(|&c| is_scheme_char(c))
    {
        scheme_from_url = Some((0, i));
        start = i + 1;
    }
    let mut netloc = None;
    if n >= start + 2 && url[start] == 0x2f && url[start + 1] == 0x2f {
        // _splitnetloc(url, 2): the earliest of '/', '?', '#' after the '//'.
        let s = start + 2;
        let delim = url[s..].iter().position(|&c| matches!(c, 0x2f | 0x3f | 0x23)).map_or(n, |p| s + p);
        let net = &url[s..delim];
        if net.iter().any(|&c| c == 0x5b || c == 0x5d) {
            return Err(SplitDefer::Brackets);
        }
        if net.iter().any(|&c| c >= 128) {
            return Err(SplitDefer::NonAsciiNetloc);
        }
        netloc = Some((s, delim));
        start = delim;
    }
    let mut end = n;
    let mut fragment = None;
    if allow_fragments && let Some(p) = url[start..].iter().position(|&c| c == 0x23) {
        fragment = Some((start + p + 1, n));
        end = start + p;
    }
    let mut query = None;
    if let Some(p) = url[start..end].iter().position(|&c| c == 0x3f) {
        query = Some((start + p + 1, end));
        end = start + p;
    }
    Ok(Split { scheme_from_url, netloc, path: (start, end), query, fragment })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u32> {
        s.chars().map(u32::from).collect()
    }
    fn part(url: &[u32], r: Range) -> Option<String> {
        r.map(|(a, b)| url[a..b].iter().map(|&c| char::from_u32(c).unwrap()).collect())
    }

    #[test]
    fn quote_matches_doc() {
        let t = safe_table(u("/"));
        let mut out = Vec::new();
        quote_bytes(b"abc def/x?y", &t, &mut out);
        assert_eq!(out, b"abc%20def/x%3Fy");
    }

    #[test]
    fn unquote_rules() {
        let mut out = Vec::new();
        unquote_bytes(b"a%20b%zz%4%%41%e2%82%AC", &mut out);
        assert_eq!(out, b"a b%zz%4%A\xe2\x82\xac");
    }

    #[test]
    fn split_basic() {
        let url = u("http://www.example.com:80/a/b?q=1&r=2#frag");
        let s = urlsplit(&url, true).unwrap();
        assert_eq!(part(&url, s.scheme_from_url).as_deref(), Some("http"));
        assert_eq!(part(&url, s.netloc).as_deref(), Some("www.example.com:80"));
        assert_eq!(part(&url, Some(s.path)).as_deref(), Some("/a/b"));
        assert_eq!(part(&url, s.query).as_deref(), Some("q=1&r=2"));
        assert_eq!(part(&url, s.fragment).as_deref(), Some("frag"));
        let url = u("path?x#y?z");
        let s = urlsplit(&url, false).unwrap();
        assert_eq!(s.fragment, None);
        assert_eq!(part(&url, s.query).as_deref(), Some("x#y?z"));
        assert_eq!(urlsplit(&u("http://[::1]/"), true), Err(SplitDefer::Brackets));
    }
}
