//! POSIX path algorithms: `posixpath.join/split/splitext/basename/dirname/isabs` and the C
//! `normpath` (`_Py_normpath_and_size`, Python/fileutils.c), on code-unit slices.

use super::Unit;

/// Index of the last `/`, as `p.rfind(sep)`.
#[inline]
pub fn rfind_slash<T: Unit>(s: &[T]) -> Option<usize> {
    s.iter().rposition(|&c| c == T::SLASH)
}

/// `p.rfind(sep) + 1`: where `basename` starts (`split`'s tail).
#[inline]
pub fn tail_start<T: Unit>(s: &[T]) -> usize {
    rfind_slash(s).map_or(0, |i| i + 1)
}

/// Length of `dirname(p)` (`split(p)[0]`), a prefix of `p`:
/// `head = p[:i]; if head and head != sep*len(head): head = head.rstrip(sep)`.
#[inline]
pub fn head_len<T: Unit>(s: &[T]) -> usize {
    let i = tail_start(s);
    match s[..i].iter().rposition(|&c| c != T::SLASH) {
        // Empty or only slashes: kept as is.
        None => i,
        Some(j) => j + 1,
    }
}

/// `genericpath._splitext(p, sep, None, extsep)`: the index of the extension's dot, or
/// `None` when the result is `(p, p[:0])`.
#[inline]
pub fn splitext<T: Unit>(s: &[T]) -> Option<usize> {
    let sep = rfind_slash(s).map_or(-1, |i| i as isize);
    let dot = s.iter().rposition(|&c| c == T::DOT)? as isize;
    if dot > sep {
        // Skip all leading dots of the file name.
        let mut f = (sep + 1) as usize;
        while (f as isize) < dot {
            if s[f] != T::DOT {
                return Some(dot as usize);
            }
            f += 1;
        }
    }
    None
}

/// `s.startswith(sep)`.
#[inline]
pub fn is_abs<T: Unit>(s: &[T]) -> bool {
    s.first() == Some(&T::SLASH)
}

/// What `posixpath.join` needs to know about one (already `fspath`'d) argument.
#[derive(Clone, Copy, Debug)]
pub struct JoinArg {
    pub len: usize,
    pub starts_sep: bool,
    pub ends_sep: bool,
}

impl JoinArg {
    pub fn of<T: Unit>(s: &[T]) -> Self {
        JoinArg { len: s.len(), starts_sep: s.first() == Some(&T::SLASH), ends_sep: s.last() == Some(&T::SLASH) }
    }
}

/// One piece of a join result: argument index, and whether `sep` precedes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Piece {
    pub arg: usize,
    pub sep_before: bool,
}

/// The result of `posixpath.join(*args)`: either one of the arguments itself (CPython's
/// concatenations returned an operand unchanged), or the concatenation of `pieces`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Joined {
    Arg(usize),
    Build { npieces: usize, len: usize },
}

/// CPython's loop, one argument at a time:
///
/// ```text
/// path = a
/// for b in p:
///     if b.startswith(sep) or not path: path = b
///     elif path.endswith(sep):           path += b
///     else:                              path += sep + b
/// ```
///
/// `path += b` with an empty `b` leaves `path` unchanged (the same object). `pieces` must
/// hold at least `args.len()` entries. `args` must not be empty.
pub fn join_plan(args: &[JoinArg], pieces: &mut [Piece]) -> Joined {
    let mut identity = Some(0usize);
    pieces[0] = Piece { arg: 0, sep_before: false };
    let mut n = 1;
    let mut len = args[0].len;
    let mut ends = args[0].ends_sep;
    for (k, b) in args.iter().enumerate().skip(1) {
        if b.starts_sep || len == 0 {
            identity = Some(k);
            pieces[0] = Piece { arg: k, sep_before: false };
            n = 1;
            len = b.len;
            ends = b.ends_sep;
        } else if ends {
            if b.len != 0 {
                pieces[n] = Piece { arg: k, sep_before: false };
                n += 1;
                identity = None;
                len += b.len;
                ends = b.ends_sep;
            }
        } else {
            pieces[n] = Piece { arg: k, sep_before: true };
            n += 1;
            identity = None;
            len += 1 + b.len;
            ends = b.len == 0 || b.ends_sep;
        }
    }
    match identity {
        Some(k) => Joined::Arg(k),
        None => Joined::Build { npieces: n, len },
    }
}

/// `_Py_normpath_and_size` for POSIX (no drive, no altsep), ported statement by statement
/// so that every corner (`..` at the root, leading `./`, exactly two leading slashes,
/// trailing slashes) behaves as CPython's `posix._path_normpath`. An empty result means
/// CPython returns `'.'`. Works on bytes too: under UTF-8 + surrogateescape (the only
/// filesystem encoding the bindings accept for bytes) a byte is `/` or `.` exactly when
/// the decoded code point is.
pub fn normpath<T: Unit>(path: &[T]) -> Vec<T> {
    let mut b = path.to_vec();
    let n = normpath_in_place(&mut b);
    b.truncate(n);
    b
}

/// [`normpath`] in place (as the C function works on its own buffer: the write position
/// never passes the read position): the normalized path is `b[..n]` for the returned `n`
/// (0: CPython returns `'.'`). No allocation.
pub fn normpath_in_place<T: Unit>(b: &mut [T]) -> usize {
    let size = b.len();
    if size == 0 {
        return 0;
    }
    // C reads one past the end (the NUL terminator): never a separator.
    let is_sep = |b: &[T], i: usize| i < size && b[i] == T::SLASH;
    let sep_or_end = |b: &[T], i: usize| i == size || (i < size && b[i] == T::SLASH);

    // _Py_skiproot
    let rootsize = if !is_sep(b, 0) {
        0
    } else if !is_sep(b, 1) || is_sep(b, 2) {
        1
    } else {
        2
    };
    let mut p1 = 0usize;
    let mut p2 = 0usize;
    let mut min_p2 = 0usize;
    // `lastC`: None stands for NUL.
    let mut last: Option<T> = None;
    if rootsize > 0 {
        p1 = rootsize;
        p2 = p1;
        min_p2 = p2 - 1;
        last = Some(b[min_p2]);
    }
    if p1 < size && b[p1] == T::DOT && sep_or_end(b, p1 + 1) {
        // Skip leading './'
        p1 += 1;
        last = if p1 < size { Some(b[p1]) } else { None };
        while is_sep(b, p1) {
            p1 += 1;
        }
    }
    while p1 != size {
        let c = b[p1];
        if last == Some(T::SLASH) {
            if c == T::DOT {
                let sep_at_1 = sep_or_end(b, p1 + 1);
                let sep_at_2 = !sep_at_1 && sep_or_end(b, p1 + 2);
                if sep_at_2 && b[p1 + 1] == T::DOT {
                    let mut p3 = p2;
                    while p3 != min_p2 {
                        p3 -= 1;
                        if b[p3] != T::SLASH {
                            break;
                        }
                    }
                    while p3 != min_p2 && b[p3 - 1] != T::SLASH {
                        p3 -= 1;
                    }
                    if p2 == min_p2 || (b[p3] == T::DOT && b[p3 + 1] == T::DOT && is_sep(b, p3 + 2)) {
                        // Previous segment is also ../, so append instead.
                        b[p2] = T::DOT;
                        b[p2 + 1] = T::DOT;
                        p2 += 2;
                        last = Some(T::DOT);
                    } else if b[p3] == T::SLASH {
                        // Absolute path, so absorb segment
                        p2 = p3 + 1;
                    } else {
                        p2 = p3;
                    }
                    p1 += 1;
                } else if sep_at_1 {
                } else {
                    b[p2] = c;
                    p2 += 1;
                    last = Some(c);
                }
            } else if c == T::SLASH {
            } else {
                b[p2] = c;
                p2 += 1;
                last = Some(c);
            }
        } else {
            b[p2] = c;
            p2 += 1;
            last = Some(c);
        }
        p1 += 1;
    }
    // `*p2 = '\0'`, then strip trailing separators down to min_p2.
    let mut q = p2 as isize;
    if p2 != min_p2 {
        loop {
            q -= 1;
            if q == min_p2 as isize || b[q as usize] != T::SLASH {
                break;
            }
        }
    } else {
        q -= 1;
    }
    (q + 1) as usize
}

/// `normpath(s) == s` for an absolute path, decided in one pass: one leading `/` (two
/// leading slashes are kept by `normpath`, three or more collapse: those take the full
/// algorithm), no empty, `.` or `..` component, no trailing `/` except the root itself.
/// Such a path passes through `_Py_normpath_and_size` unchanged (every character is copied
/// to its own position), so callers may use the input as the normalized path.
pub fn is_normal_abs<T: Unit>(s: &[T]) -> bool {
    if s.first() != Some(&T::SLASH) {
        return false;
    }
    if s.len() == 1 {
        return true;
    }
    // Scan components: each starts after a '/', must be non-empty and not '.'/'..'.
    let mut start = 1usize;
    let n = s.len();
    loop {
        let mut end = start;
        while end < n && s[end] != T::SLASH {
            end += 1;
        }
        let len = end - start;
        if len == 0 {
            return false; // '//' or a trailing '/'
        }
        if s[start] == T::DOT && (len == 1 || (len == 2 && s[start + 1] == T::DOT)) {
            return false;
        }
        if end == n {
            return true;
        }
        start = end + 1;
    }
}

/// The components of an absolute normalized path after `lstrip(sep)`, as `split(sep)`
/// would return them (`[]` for the root itself).
pub fn abs_components<T: Unit>(norm: &[T]) -> impl Iterator<Item = &[T]> + Clone {
    let start = norm.iter().position(|&c| c != T::SLASH).unwrap_or(norm.len());
    let rest = &norm[start..];
    let empty = rest.is_empty();
    rest.split(|&c| c == T::SLASH).filter(move |_| !empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn np(s: &str) -> String {
        let v: Vec<u32> = s.chars().map(|c| c as u32).collect();
        let r = normpath(&v);
        if r.is_empty() { ".".into() } else { r.into_iter().map(|c| char::from_u32(c).unwrap()).collect() }
    }

    #[test]
    fn normpath_cases() {
        // Expected values from CPython 3.15.1 posixpath.normpath.
        let cases = [
            ("", "."),
            ("/", "/"),
            ("//", "//"),
            ("///", "/"),
            ("///foo/.//bar//", "/foo/bar"),
            ("///foo/.//bar//.//..//.//baz///", "/foo/baz"),
            ("///..//./foo/.//bar", "/foo/bar"),
            ("//foo/..", "//"),
            (".", "."),
            ("./", "."),
            ("./.", "."),
            ("..", ".."),
            ("../..", "../.."),
            ("a/..", "."),
            ("a/../..", ".."),
            ("a/b/../../..", ".."),
            ("../a/../b", "../b"),
            ("/../a", "/a"),
            ("foo/./bar", "foo/bar"),
            ("foo//bar//", "foo/bar"),
            ("./a/./b/.", "a/b"),
            ("/a/b/c/../../d", "/a/d"),
            ("..a/b", "..a/b"),
            ("a/.b/..c", "a/.b/..c"),
            ("a/b/...", "a/b/..."),
        ];
        for (inp, want) in cases {
            assert_eq!(np(inp), want, "normpath({inp:?})");
        }
    }

    #[test]
    fn normal_abs() {
        let yes = ["/", "/a", "/a/b", "/a/.b/..c/...", "/x y/z"];
        let no = ["", "a", "//", "//a", "///a", "/a/", "/a//b", "/a/./b", "/a/../b", "/.", "/..", "/a/.", "/a/.."];
        for s in yes {
            assert!(is_normal_abs(s.as_bytes()), "{s:?}");
            assert_eq!(np(s), s, "{s:?}");
        }
        for s in no {
            assert!(!is_normal_abs(s.as_bytes()), "{s:?}");
        }
        // Exhaustive over short strings of '/', '.', 'a': the check implies normpath(s) == s.
        let alphabet = [b'/', b'.', b'a'];
        for len in 1..=7u32 {
            for mut k in 0..3usize.pow(len) {
                let mut v = Vec::new();
                for _ in 0..len {
                    v.push(alphabet[k % 3]);
                    k /= 3;
                }
                if is_normal_abs(&v) {
                    assert_eq!(normpath(&v), v, "{:?}", String::from_utf8_lossy(&v));
                }
            }
        }
    }

    #[test]
    fn split_and_ext() {
        let b = |s: &str| s.as_bytes().to_vec();
        assert_eq!(head_len(&b("/a/b//c")), 4);
        assert_eq!(head_len(&b("//c")), 2);
        assert_eq!(head_len(&b("c")), 0);
        assert_eq!(tail_start(&b("/a/b")), 3);
        assert_eq!(splitext(&b("a/b.py")), Some(3));
        assert_eq!(splitext(&b(".bashrc")), None);
        assert_eq!(splitext(&b("..a.b")), Some(3));
        assert_eq!(splitext(&b("a.b/c")), None);
        assert_eq!(splitext(&b("x...")), Some(3));
    }

    #[test]
    fn join_cases() {
        let plan = |parts: &[&str]| {
            let a: Vec<JoinArg> = parts.iter().map(|s| JoinArg::of(s.as_bytes())).collect();
            let mut p = vec![Piece::default(); a.len()];
            let j = join_plan(&a, &mut p);
            match j {
                Joined::Arg(k) => format!("arg{k}"),
                Joined::Build { npieces, len } => {
                    let mut s = String::new();
                    for pc in &p[..npieces] {
                        if pc.sep_before {
                            s.push('/');
                        }
                        s.push_str(parts[pc.arg]);
                    }
                    assert_eq!(s.len(), len);
                    s
                }
            }
        };
        assert_eq!(plan(&["/a", "b"]), "/a/b");
        assert_eq!(plan(&["/a/", "b"]), "/a/b");
        assert_eq!(plan(&["/a", "/b"]), "arg1");
        assert_eq!(plan(&["", "b"]), "arg1");
        assert_eq!(plan(&["a", ""]), "a/");
        assert_eq!(plan(&["a/", ""]), "arg0");
        assert_eq!(plan(&["", ""]), "arg1");
        assert_eq!(plan(&["a"]), "arg0");
        assert_eq!(plan(&["a", "b", "/c", "d"]), "/c/d");
        assert_eq!(plan(&["a", "", "", "b"]), "a/b");
    }

    #[test]
    fn components() {
        let c: Vec<&[u8]> = abs_components(b"/a/b").collect();
        assert_eq!(c, vec![&b"a"[..], &b"b"[..]]);
        assert_eq!(abs_components(b"/").count(), 0);
        assert_eq!(abs_components(b"//x").count(), 1);
    }
}
