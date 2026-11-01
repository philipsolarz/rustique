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

/// One piece of a `%`-format string (`PyUnicode_Format` grammar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    /// Literal text `fmt[start..end]`.
    Lit(usize, usize),
    /// `%%`.
    Percent,
    Dir(Dir),
}

/// A conversion `%[(key)][flags][width][.precision]conv`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dir {
    /// `fmt[start..end]` of a `%(key)`.
    pub key: Option<(usize, usize)>,
    /// `-`
    pub ljust: bool,
    /// `0`
    pub zero: bool,
    /// `+`, ` ` or `#` present (the native renderer declines these).
    pub other_flags: bool,
    pub width: Option<usize>,
    pub prec: Option<usize>,
    pub conv: u8,
}

/// Parse a format string into pieces. Accepts `%%` and
/// `%[(key)][-+ #0]*[digits][.digits](s|r|a|d|i|u|o|x|X|e|E|f|F|g|G|c)` with a key that has
/// no parentheses; returns `None` for `*`, length modifiers, nested parentheses, unknown
/// conversions or a trailing `%` (which `PyUnicode_Format` handles differently or rejects).
/// For accepted strings, `PyUnicode_Format` reads the same keys / positional arguments in
/// the same order.
pub fn parse_format(fmt: &[u8]) -> Option<Vec<Piece>> {
    let mut out = Vec::new();
    let n = fmt.len();
    let mut i = 0;
    let mut lit = 0;
    while i < n {
        if fmt[i] != b'%' {
            i += 1;
            continue;
        }
        if lit < i {
            out.push(Piece::Lit(lit, i));
        }
        i += 1;
        if *fmt.get(i)? == b'%' {
            out.push(Piece::Percent);
            i += 1;
            lit = i;
            continue;
        }
        let mut key = None;
        if fmt[i] == b'(' {
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
            key = Some((start, i));
            i += 1;
        }
        let (mut ljust, mut zero, mut other) = (false, false, false);
        while i < n {
            match fmt[i] {
                b'-' => ljust = true,
                b'0' => zero = true,
                b'+' | b' ' | b'#' => other = true,
                _ => break,
            }
            i += 1;
        }
        let digits = |i: &mut usize| -> Option<Option<usize>> {
            let s = *i;
            let mut v: usize = 0;
            while *i < n && fmt[*i].is_ascii_digit() {
                v = v.checked_mul(10)?.checked_add((fmt[*i] - b'0') as usize)?;
                *i += 1;
            }
            Some((*i > s).then_some(v))
        };
        let width = digits(&mut i)?;
        let mut prec = None;
        if i < n && fmt[i] == b'.' {
            i += 1;
            prec = Some(digits(&mut i)?.unwrap_or(0));
        }
        let c = *fmt.get(i)?;
        if !b"sradiuoxXeEfFgGc".contains(&c) {
            return None;
        }
        i += 1;
        out.push(Piece::Dir(Dir { key, ljust, zero, other_flags: other, width, prec, conv: c }));
        lit = i;
    }
    if lit < n {
        out.push(Piece::Lit(lit, n));
    }
    Some(out)
}

/// A converted argument, as the native renderer needs it.
#[derive(Clone, Copy, Debug)]
pub enum Val<'a> {
    /// `str(value)` as Latin-1 bytes (for `%s`).
    Text(&'a [u8]),
    /// The integer value (for `%d`/`%i`/`%u`: `int(value)`).
    Int(i64),
}

/// Widths and precisions the renderer accepts (larger ones go to `PyUnicode_Format`).
pub const MAX_WIDTH: usize = 1 << 16;

/// Whether the renderer handles this directive: `%s` (any flags: `+`, ` `, `#`, `0` have
/// no effect on strings) and `%d`/`%i`/`%u` with only `-`/`0` flags and no precision.
pub fn native_dir(d: &Dir) -> bool {
    if d.width.is_some_and(|w| w > MAX_WIDTH) || d.prec.is_some_and(|p| p > MAX_WIDTH) {
        return false;
    }
    match d.conv {
        b's' => true,
        b'd' | b'i' | b'u' => !d.other_flags && d.prec.is_none(),
        _ => false,
    }
}

fn pad(out: &mut Vec<u8>, c: u8, n: usize) {
    out.extend(std::iter::repeat_n(c, n));
}

/// Append the output of one `%s` / `%d` directive exactly as
/// `unicode_format_arg_output` writes it.
fn render_dir(d: &Dir, v: Val<'_>, out: &mut Vec<u8>) -> bool {
    let digits;
    let v = match (d.conv, v) {
        (b's', Val::Int(x)) => {
            digits = x.to_string();
            Val::Text(digits.as_bytes())
        }
        _ => v,
    };
    match (d.conv, v) {
        (b's', Val::Text(t)) => {
            let t = match d.prec {
                Some(p) if p < t.len() => &t[..p],
                _ => t,
            };
            let w = d.width.unwrap_or(0);
            let fillc = w.saturating_sub(t.len());
            if !d.ljust {
                pad(out, b' ', fillc);
            }
            out.extend_from_slice(t);
            if d.ljust {
                pad(out, b' ', fillc);
            }
            true
        }
        (b'd' | b'i' | b'u', Val::Int(x)) => {
            let digits = x.unsigned_abs().to_string();
            let neg = x < 0;
            let len = digits.len();
            let mut width = d.width.unwrap_or(0).max(len + neg as usize);
            let fill = if d.zero { b'0' } else { b' ' };
            if neg {
                if fill != b' ' {
                    out.push(b'-');
                }
                // `if width > len: width--` (len excludes the sign here)
                if width > len {
                    width -= 1;
                }
            }
            if width > len && !d.ljust {
                pad(out, fill, width - len);
                width = len;
            }
            if fill == b' ' && neg {
                out.push(b'-');
            }
            out.extend_from_slice(digits.as_bytes());
            if width > len {
                pad(out, b' ', width - len);
            }
            true
        }
        _ => false,
    }
}

/// Render `fmt` with one value per directive (in order), appending Latin-1 bytes to `out`.
/// Every directive must satisfy [`native_dir`]. Returns false (output unspecified) if a
/// value does not fit its directive.
pub fn render(fmt: &[u8], pieces: &[Piece], vals: &[Val<'_>], out: &mut Vec<u8>) -> bool {
    let mut k = 0;
    for p in pieces {
        match *p {
            Piece::Lit(a, b) => out.extend_from_slice(&fmt[a..b]),
            Piece::Percent => out.push(b'%'),
            Piece::Dir(ref d) => {
                let Some(&v) = vals.get(k) else { return false };
                k += 1;
                if !native_dir(d) || !render_dir(d, v, out) {
                    return false;
                }
            }
        }
    }
    k == vals.len()
}

/// The directives of `pieces`, in order.
pub fn dirs(pieces: &[Piece]) -> impl Iterator<Item = &Dir> {
    pieces.iter().filter_map(|p| if let Piece::Dir(d) = p { Some(d) } else { None })
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

    fn keys(f: &str) -> Option<Vec<String>> {
        parse_format(f.as_bytes())
            .map(|v| dirs(&v).map(|d| d.key.map_or("-".to_string(), |(a, b)| f[a..b].to_string())).collect())
    }

    #[test]
    fn parse() {
        assert_eq!(keys("%(asctime)s %(levelname)-8s [%(name)s] %(message)s").unwrap(), ["asctime", "levelname", "name", "message"]);
        assert_eq!(keys("%(msecs)03d %(created).3f %(x)+ #0-10.2e 100%%").unwrap(), ["msecs", "created", "x"]);
        assert_eq!(keys("no directives").unwrap(), Vec::<String>::new());
        assert_eq!(keys("%()s").unwrap(), [""]);
        assert_eq!(keys("x=%s y=%5.2f %%").unwrap(), ["-", "-"]);
        for f in ["%(a)", "%(a", "%(a(b))s", "%(a)*d", "%(a).*f", "%(a)ld", "%(a)k", "%", "abc%", "%*d", "%.*s", "%hd"] {
            assert!(keys(f).is_none(), "{f:?}");
        }
        let p = parse_format(b"a%-08.3sb").unwrap();
        assert_eq!(p[0], Piece::Lit(0, 1));
        assert_eq!(
            p[1],
            Piece::Dir(Dir { key: None, ljust: true, zero: true, other_flags: false, width: Some(8), prec: Some(3), conv: b's' })
        );
        assert_eq!(p[2], Piece::Lit(8, 9));
    }

    fn r(f: &str, vals: &[Val<'_>]) -> Option<String> {
        let p = parse_format(f.as_bytes())?;
        let mut o = Vec::new();
        render(f.as_bytes(), &p, vals, &mut o).then(|| String::from_utf8(o).unwrap())
    }

    #[test]
    fn render_matches_python() {
        // Expected strings are Python's `fmt % args`.
        let t = |s: &'static str| Val::Text(s.as_bytes());
        assert_eq!(r("%s", &[t("abc")]).unwrap(), "abc");
        assert_eq!(r("[%-8s]", &[t("INFO")]).unwrap(), "[INFO    ]");
        assert_eq!(r("[%8s]", &[t("INFO")]).unwrap(), "[    INFO]");
        assert_eq!(r("[%.2s]", &[t("INFO")]).unwrap(), "[IN]");
        assert_eq!(r("[%-6.2s]", &[t("INFO")]).unwrap(), "[IN    ]");
        assert_eq!(r("[%05s]", &[t("ab")]).unwrap(), "[   ab]");
        assert_eq!(r("[%+s]", &[t("ab")]).unwrap(), "[ab]");
        assert_eq!(r("%d", &[Val::Int(-42)]).unwrap(), "-42");
        assert_eq!(r("%05d", &[Val::Int(-42)]).unwrap(), "-0042");
        assert_eq!(r("%05d", &[Val::Int(42)]).unwrap(), "00042");
        assert_eq!(r("%-5d|", &[Val::Int(-42)]).unwrap(), "-42  |");
        assert_eq!(r("%-05d|", &[Val::Int(42)]).unwrap(), "42   |");
        assert_eq!(r("%5d|", &[Val::Int(-42)]).unwrap(), "  -42|");
        assert_eq!(r("%03d", &[Val::Int(7)]).unwrap(), "007");
        assert_eq!(r("%03d", &[Val::Int(1234)]).unwrap(), "1234");
        assert_eq!(r("%1d", &[Val::Int(-5)]).unwrap(), "-5");
        assert_eq!(r("%02d", &[Val::Int(-5)]).unwrap(), "-5");
        assert_eq!(r("%03d", &[Val::Int(-5)]).unwrap(), "-05");
        assert_eq!(r("%i %u", &[Val::Int(i64::MIN), Val::Int(0)]).unwrap(), "-9223372036854775808 0");
        assert_eq!(r("100%% %s", &[t("x")]).unwrap(), "100% x");
        assert_eq!(r("[%5s|%-4s|%.1s]", &[Val::Int(-42), Val::Int(7), Val::Int(123)]).unwrap(), "[  -42|7   |1]");
        // declined
        assert!(r("%+d", &[Val::Int(1)]).is_none());
        assert!(r("%.3d", &[Val::Int(1)]).is_none());
        assert!(r("%r", &[t("x")]).is_none());
        assert!(r("%f", &[Val::Int(1)]).is_none());
        assert!(r("%d", &[t("x")]).is_none());
        assert!(r("%s %s", &[t("x")]).is_none());
        assert!(r("%s", &[t("x"), t("y")]).is_none());
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
