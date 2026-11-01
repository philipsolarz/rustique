//! Pure pieces of the `logging` record path (no Python): the numeric subset of C
//! `strftime` used for log timestamps, `logging.Formatter`'s default millisecond suffix,
//! a conservative parser for `%(key)s`-style format strings, and the path splitting of
//! `LogRecord.__init__` (`os.path.basename` + `os.path.splitext(...)[0]`).
//!
//! Every function either produces exactly what CPython (glibc `strftime`,
//! `PyUnicode_Format`, `posixpath`) produces for its input, or declines (`None`/`false`)
//! so that the caller runs CPython's own code.

/// Broken-down time as `localtime_r`/`gmtime_r` fill `struct tm`, in Python's
/// conventions for the fields the numeric directives read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tm {
    /// Full year (`tm_year + 1900`).
    pub year: i64,
    /// 1..=12 (`tm_mon + 1`).
    pub mon: i32,
    /// 1..=31.
    pub mday: i32,
    pub hour: i32,
    pub min: i32,
    /// 0..=61.
    pub sec: i32,
    /// 1..=366 (`tm_yday + 1`).
    pub yday: i32,
}

#[inline]
fn push2(out: &mut Vec<u8>, v: i32) {
    out.push(b'0' + (v / 10 % 10) as u8);
    out.push(b'0' + (v % 10) as u8);
}

/// Append `strftime(fmt, tm)` for an ASCII `fmt` made of literal characters and the
/// directives `%Y %m %d %H %M %S %y %j %%`, which glibc and BSD libc print as zero-padded
/// decimal numbers independent of the locale. Returns false (leaving `out` in an
/// unspecified state) for anything else: other directives (locale names, `%z`/`%Z`,
/// flags such as `%-d`), a trailing `%`, NUL or non-ASCII bytes, field values outside
/// `struct tm`'s ranges, or a year outside 1000..=9999 (libc padding rules differ there).
pub fn strftime_numeric(fmt: &[u8], tm: &Tm, out: &mut Vec<u8>) -> bool {
    if !(1000..=9999).contains(&tm.year)
        || !(1..=12).contains(&tm.mon)
        || !(1..=31).contains(&tm.mday)
        || !(0..=23).contains(&tm.hour)
        || !(0..=59).contains(&tm.min)
        || !(0..=61).contains(&tm.sec)
        || !(1..=366).contains(&tm.yday)
    {
        return false;
    }
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        if c == 0 || c > 127 {
            return false;
        }
        if c != b'%' {
            out.push(c);
            i += 1;
            continue;
        }
        let Some(&d) = fmt.get(i + 1) else { return false };
        match d {
            b'Y' => {
                let y = tm.year as i32;
                push2(out, y / 100);
                push2(out, y % 100);
            }
            b'm' => push2(out, tm.mon),
            b'd' => push2(out, tm.mday),
            b'H' => push2(out, tm.hour),
            b'M' => push2(out, tm.min),
            b'S' => push2(out, tm.sec),
            b'y' => push2(out, (tm.year % 100) as i32),
            b'j' => {
                out.push(b'0' + (tm.yday / 100) as u8);
                push2(out, tm.yday % 100);
            }
            b'%' => out.push(b'%'),
            _ => return false,
        }
        i += 2;
    }
    true
}

/// `'%s,%03d' % (s, msecs)` minus the `s` part: append `,` and `int(msecs)` zero-padded
/// to three digits, for a finite `msecs` in `0.0..1000.0` (what `LogRecord` stores).
/// Returns false otherwise.
pub fn msec_suffix(msecs: f64, out: &mut Vec<u8>) -> bool {
    if !(0.0..1000.0).contains(&msecs) {
        return false;
    }
    let v = msecs as i32; // truncation toward zero, as `%d` on a float
    out.push(b',');
    out.push(b'0' + (v / 100) as u8);
    push2(out, v % 100);
    true
}

/// The `%(key)` directives of a format string applied to a mapping, as `(start, end)`
/// character ranges of each key, in order. Accepts only the plain subset
/// `%%` and `%(key)[flags][width][.precision]conv` with flags in `-+ #0`, decimal width
/// and precision, a key without parentheses and `conv` in `sradiuoxXeEfFgGc`; returns
/// `None` for anything else (positional directives, `*`, length modifiers, nested
/// parentheses, unknown conversions, a trailing `%`), which `PyUnicode_Format` may treat
/// differently. For accepted strings, `PyUnicode_Format(fmt, mapping)` looks up exactly
/// these keys, left to right, and nothing else.
pub fn percent_keys(fmt: &[u8]) -> Option<Vec<(usize, usize)>> {
    let mut keys = Vec::new();
    let mut i = 0;
    let n = fmt.len();
    while i < n {
        if fmt[i] != b'%' {
            i += 1;
            continue;
        }
        i += 1;
        match fmt.get(i)? {
            b'%' => {
                i += 1;
                continue;
            }
            b'(' => {}
            _ => return None,
        }
        i += 1;
        let start = i;
        while i < n && fmt[i] != b')' {
            if fmt[i] == b'(' {
                return None;
            }
            i += 1;
        }
        if i >= n {
            return None;
        }
        keys.push((start, i));
        i += 1;
        while i < n && matches!(fmt[i], b'-' | b'+' | b' ' | b'#' | b'0') {
            i += 1;
        }
        while i < n && fmt[i].is_ascii_digit() {
            i += 1;
        }
        if i < n && fmt[i] == b'.' {
            i += 1;
            while i < n && fmt[i].is_ascii_digit() {
                i += 1;
            }
        }
        let c = *fmt.get(i)?;
        if !b"sradiuoxXeEfFgGc".contains(&c) {
            return None;
        }
        i += 1;
    }
    Some(keys)
}

/// `LogRecord.__init__`'s `filename = os.path.basename(pathname)` and
/// `module = os.path.splitext(filename)[0]` (posixpath, `str` input) as character
/// ranges: `(filename_start, module_end)`; `filename` is `pathname[filename_start..]` and
/// `module` is `filename[..module_end]` (relative to `filename`).
///
/// `rfind` is a reverse search for a character: `None` when absent.
pub fn split_path(len: usize, rfind: impl Fn(u32, usize, usize) -> Option<usize>, at: impl Fn(usize) -> u32) -> (usize, usize) {
    // basename: p[p.rfind('/') + 1:]
    let fstart = rfind('/' as u32, 0, len).map_or(0, |i| i + 1);
    let flen = len - fstart;
    // genericpath._splitext(filename, '/', None, '.'): there is no '/' in filename.
    let module_end = match rfind('.' as u32, fstart, len) {
        Some(dot) => {
            let dot = dot - fstart;
            // skip all leading dots
            if (0..dot).any(|k| at(fstart + k) != '.' as u32) { dot } else { flen }
        }
        None => flen,
    };
    (fstart, module_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tm() -> Tm {
        Tm { year: 2026, mon: 3, mday: 4, hour: 5, min: 6, sec: 7, yday: 63 }
    }

    fn fmt(f: &str, t: &Tm) -> Option<String> {
        let mut out = Vec::new();
        strftime_numeric(f.as_bytes(), t, &mut out).then(|| String::from_utf8(out).unwrap())
    }

    #[test]
    fn numeric_directives() {
        assert_eq!(fmt("%Y-%m-%d %H:%M:%S", &tm()).unwrap(), "2026-03-04 05:06:07");
        assert_eq!(fmt("%Y-%m-%dT%H:%M:%S", &tm()).unwrap(), "2026-03-04T05:06:07");
        assert_eq!(fmt("%y/%j %%", &tm()).unwrap(), "26/063 %");
        assert_eq!(fmt("", &tm()).unwrap(), "");
        assert_eq!(fmt("plain", &tm()).unwrap(), "plain");
        let t = Tm { year: 1000, mon: 12, mday: 31, hour: 23, min: 59, sec: 60, yday: 366 };
        assert_eq!(fmt("%Y %m %d %H %M %S %j %y", &t).unwrap(), "1000 12 31 23 59 60 366 00");
    }

    #[test]
    fn declines() {
        for f in ["%a", "%b", "%z", "%Z", "%-d", "%", "x%", "%f", "%c", "\u{e9}%Y", "%E", "a\0b"] {
            assert!(fmt(f, &tm()).is_none(), "{f:?}");
        }
        assert!(fmt("%Y", &Tm { year: 999, ..tm() }).is_none());
        assert!(fmt("%Y", &Tm { year: 10000, ..tm() }).is_none());
        assert!(fmt("%Y", &Tm { mon: 0, ..tm() }).is_none());
    }

    #[test]
    fn msecs() {
        let s = |m: f64| {
            let mut o = Vec::new();
            msec_suffix(m, &mut o).then(|| String::from_utf8(o).unwrap())
        };
        assert_eq!(s(0.0).unwrap(), ",000");
        assert_eq!(s(7.9).unwrap(), ",007");
        assert_eq!(s(42.0).unwrap(), ",042");
        assert_eq!(s(999.0).unwrap(), ",999");
        assert_eq!(s(999.999).unwrap(), ",999");
        assert!(s(1000.0).is_none());
        assert!(s(-0.5).is_none());
        assert!(s(f64::NAN).is_none());
    }

    #[test]
    fn percent() {
        let k = |f: &str| percent_keys(f.as_bytes()).map(|v| v.iter().map(|&(a, b)| f[a..b].to_string()).collect::<Vec<_>>());
        assert_eq!(k("%(asctime)s %(levelname)-8s [%(name)s] %(message)s").unwrap(), ["asctime", "levelname", "name", "message"]);
        assert_eq!(k("%(msecs)03d %(created).3f %(x)+ #0-10.2e 100%%").unwrap(), ["msecs", "created", "x"]);
        assert_eq!(k("no directives").unwrap(), Vec::<String>::new());
        assert_eq!(k("%()s").unwrap(), [""]);
        for f in ["%s", "%(a)", "%(a", "%(a(b))s", "%(a)*d", "%(a).*f", "%(a)ld", "%(a)k", "%", "abc%", "%(a)s %d"] {
            assert!(k(f).is_none(), "{f:?}");
        }
    }

    #[test]
    fn paths() {
        let sp = |p: &str| {
            let c: Vec<char> = p.chars().collect();
            let (fs, me) = split_path(
                c.len(),
                |ch, a, b| (a..b).rev().find(|&i| c[i] as u32 == ch),
                |i| c[i] as u32,
            );
            let filename: String = c[fs..].iter().collect();
            let module: String = c[fs..fs + me].iter().collect();
            (filename, module)
        };
        assert_eq!(sp("/srv/app/handlers.py"), ("handlers.py".into(), "handlers".into()));
        assert_eq!(sp("handlers.py"), ("handlers.py".into(), "handlers".into()));
        assert_eq!(sp("/a/b/"), ("".into(), "".into()));
        assert_eq!(sp("/a/.bashrc"), (".bashrc".into(), ".bashrc".into()));
        assert_eq!(sp("/a/..x.py"), ("..x.py".into(), "..x".into()));
        assert_eq!(sp("/a/b.c/d"), ("d".into(), "d".into()));
        assert_eq!(sp("<stdin>"), ("<stdin>".into(), "<stdin>".into()));
        assert_eq!(sp("x.tar.gz"), ("x.tar.gz".into(), "x.tar".into()));
        assert_eq!(sp("..."), ("...".into(), "...".into()));
        assert_eq!(sp(""), ("".into(), "".into()));
    }
}
