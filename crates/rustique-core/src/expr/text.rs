//! Text semantics: `int(str)` / `float(str)` for ASCII strings (anything else is left to
//! Python via [`Fault::Fallback`]) and the Python reprs used in error messages.

use crate::column::Value;

use super::kernels::Fault;

/// Python's whitespace for numeric parsing within ASCII (`Py_ISSPACE`).
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii() && is_space(c as u8))
}

/// Digits with single underscores between them (PEP 515), appended to `out`.
/// Returns whether at least one digit was consumed.
fn digits(b: &[u8], i: &mut usize, out: &mut String) -> bool {
    if !b.get(*i).is_some_and(u8::is_ascii_digit) {
        return false;
    }
    loop {
        out.push(b[*i] as char);
        *i += 1;
        match b.get(*i) {
            Some(d) if d.is_ascii_digit() => {}
            Some(b'_') if b.get(*i + 1).is_some_and(u8::is_ascii_digit) => *i += 1,
            _ => return true,
        }
    }
}

/// `int(s)` (base 10). Non-ASCII text and values beyond i64 need Python.
pub(super) fn parse_int(s: &str) -> Result<i64, Fault> {
    if !s.is_ascii() {
        return Err(Fault::Fallback);
    }
    let t = trim(s).as_bytes();
    let (negative, start) = match t.first() {
        Some(b'-') => (true, 1),
        Some(b'+') => (false, 1),
        _ => (false, 0),
    };
    let mut i = start;
    let mut clean = String::new();
    if !digits(t, &mut i, &mut clean) || i != t.len() {
        return Err(Fault::InvalidInt);
    }
    let mut v: u64 = 0;
    for d in clean.bytes() {
        v = v
            .checked_mul(10)
            .and_then(|v| v.checked_add(u64::from(d - b'0')))
            .ok_or(Fault::Fallback)?;
    }
    let v = if negative { -(v as i128) } else { v as i128 };
    i64::try_from(v).map_err(|_| Fault::Fallback)
}

/// `float(s)`. Non-ASCII text needs Python.
pub(super) fn parse_float(s: &str) -> Result<f64, Fault> {
    if !s.is_ascii() {
        return Err(Fault::Fallback);
    }
    let t = trim(s);
    let b = t.as_bytes();
    let mut clean = String::with_capacity(t.len());
    let mut i = 0;
    if let Some(&sign @ (b'+' | b'-')) = b.first() {
        clean.push(sign as char);
        i = 1;
    }
    let word = t[i..].to_ascii_lowercase();
    if matches!(word.as_str(), "inf" | "infinity" | "nan") {
        clean.push_str(&word);
    } else {
        let whole = digits(b, &mut i, &mut clean);
        let mut frac = false;
        if b.get(i) == Some(&b'.') {
            clean.push('.');
            i += 1;
            frac = digits(b, &mut i, &mut clean);
        }
        if !whole && !frac {
            return Err(Fault::InvalidFloat);
        }
        if let Some(b'e' | b'E') = b.get(i) {
            clean.push('e');
            i += 1;
            if let Some(&sign @ (b'+' | b'-')) = b.get(i) {
                clean.push(sign as char);
                i += 1;
            }
            if !digits(b, &mut i, &mut clean) {
                return Err(Fault::InvalidFloat);
            }
        }
        if i != b.len() {
            return Err(Fault::InvalidFloat);
        }
    }
    clean.parse().map_err(|_| Fault::InvalidFloat)
}

/// Python's `repr()` of an ASCII string (callers only pass ASCII).
pub(super) fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c == '\x7f' => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// The `ValueError` message of `int(s)` (CPython truncates the repr to 200 characters).
pub(super) fn invalid_int_message(s: &str) -> String {
    let repr: String = repr_str(s).chars().take(200).collect();
    format!("invalid literal for int() with base 10: {repr}")
}

pub(super) fn invalid_float_message(s: &str) -> String {
    format!("could not convert string to float: {}", repr_str(s))
}

/// Python-like rendering of a value for messages and `Display` (floats use Rust's
/// shortest round-trip digits; the bindings use CPython's repr instead).
pub(crate) fn format_value(v: &Value) -> String {
    match v {
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::F32(x) => format_float(f64::from(*x)),
        Value::F64(x) => format_float(*x),
        Value::Str(s) if s.is_ascii() => repr_str(s),
        Value::Str(s) => format!("{s:?}"),
        other => other.as_i128().map_or_else(String::new, |i| i.to_string()),
    }
}

fn format_float(x: f64) -> String {
    if x.is_nan() {
        "nan".into()
    } else if x.is_infinite() {
        if x > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        format!("{x:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints() {
        assert_eq!(parse_int(" 12 "), Ok(12));
        assert_eq!(parse_int("-1_000"), Ok(-1000));
        assert_eq!(parse_int("+007"), Ok(7));
        assert_eq!(parse_int("\x0b5\x0c"), Ok(5));
        assert_eq!(parse_int("-9223372036854775808"), Ok(i64::MIN));
        assert_eq!(parse_int("9223372036854775808"), Err(Fault::Fallback));
        assert_eq!(parse_int("99999999999999999999999"), Err(Fault::Fallback));
        assert_eq!(parse_int("١٢"), Err(Fault::Fallback));
        for bad in ["", " ", "_1", "1_", "1__0", "- 5", "1.0", "0x10", "\x1c5", "1\x00", "+"] {
            assert_eq!(parse_int(bad), Err(Fault::InvalidInt), "{bad:?}");
        }
    }

    #[test]
    fn floats() {
        assert_eq!(parse_float(" 1.5 "), Ok(1.5));
        assert_eq!(parse_float("1_0.0_1"), Ok(10.01));
        assert_eq!(parse_float("1e1_0"), Ok(1e10));
        assert_eq!(parse_float("+.5e-3"), Ok(0.0005));
        assert_eq!(parse_float("5."), Ok(5.0));
        assert_eq!(parse_float("1.e5"), Ok(1e5));
        assert_eq!(parse_float("-iNfInItY"), Ok(f64::NEG_INFINITY));
        assert!(parse_float("nan").unwrap().is_nan());
        assert_eq!(parse_float("1e400"), Ok(f64::INFINITY));
        assert_eq!(parse_float("0.1"), Ok(0.1));
        for bad in ["", ".", "e5", "1e", "1_.5", "1._5", "in_f", "0x10", "1e+", "--1", "1.5.5", "\x1c1"] {
            assert_eq!(parse_float(bad), Err(Fault::InvalidFloat), "{bad:?}");
        }
    }

    #[test]
    fn reprs() {
        assert_eq!(repr_str("abc"), "'abc'");
        assert_eq!(repr_str("a'b"), "\"a'b\"");
        assert_eq!(repr_str("a'\"b"), "'a\\'\"b'");
        assert_eq!(repr_str("\t\x7f\x1c\\"), "'\\t\\x7f\\x1c\\\\'");
        assert_eq!(invalid_int_message(&"x".repeat(300)).len(), "invalid literal for int() with base 10: ".len() + 200);
        assert_eq!(format_value(&Value::F64(1e300)), "1e300");
        assert_eq!(format_value(&Value::Bool(true)), "True");
    }
}
