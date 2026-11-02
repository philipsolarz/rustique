//! Port of the checks of `re._parser.parse_template(repl, pattern)`: would CPython compile
//! the replacement template of `Pattern.sub` without raising? (Only the verdict is needed:
//! `re.sub` then lets CPython's own `_compile_template` build it.)

use super::Decline;
use super::parser::{ascii_decimal, int_of};

const BACKSLASH: u32 = '\\' as u32;

#[inline]
fn is_digit(c: u32) -> bool {
    (b'0' as u32..=b'9' as u32).contains(&c)
}
#[inline]
fn is_octdigit(c: u32) -> bool {
    (b'0' as u32..=b'7' as u32).contains(&c)
}
#[inline]
fn is_asciiletter(c: u32) -> bool {
    (b'a' as u32..=b'z' as u32).contains(&c) || (b'A' as u32..=b'Z' as u32).contains(&c)
}

/// The tokens of `Tokenizer(repl)`: `(is_escape, char)`.
fn tokens(s: &[u32]) -> Result<Vec<(bool, u32)>, Decline> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == BACKSLASH {
            let Some(&c) = s.get(i + 1) else {
                return Err(Decline::Error); // bad escape (end of pattern)
            };
            out.push((true, c));
            i += 2;
        } else {
            out.push((false, s[i]));
            i += 1;
        }
    }
    Ok(out)
}

/// `Ok` when `parse_template(repl, pattern)` returns normally for a pattern with `groups`
/// groups whose `groupindex` is `index`. Names other than ASCII identifiers decline.
pub fn check(repl: &[u32], groups: u32, maxgroups: u32, index: &dyn Fn(&str) -> Option<u32>) -> Result<(), Decline> {
    let toks = tokens(repl)?;
    let mut k = 0;
    // `s.next`: the token at `k` if it is a plain char
    let next_char = |k: usize| -> Option<u32> {
        match toks.get(k) {
            Some(&(false, c)) => Some(c),
            _ => None,
        }
    };
    let addgroup = |index: u64| if index > groups as u64 { Err(Decline::Error) } else { Ok(()) };
    while k < toks.len() {
        let (esc, c) = toks[k];
        k += 1;
        if !esc {
            continue;
        }
        if c == 'g' as u32 {
            if next_char(k) != Some('<' as u32) {
                return Err(Decline::Error); // missing <
            }
            k += 1;
            // getuntil(">")
            let mut name = Vec::new();
            loop {
                let Some(&(e, x)) = toks.get(k) else {
                    return Err(Decline::Error); // missing >, unterminated name
                };
                k += 1;
                if !e && x == '>' as u32 {
                    if name.is_empty() {
                        return Err(Decline::Error); // missing group name
                    }
                    break;
                }
                if e {
                    name.push(BACKSLASH);
                }
                name.push(x);
            }
            let idx = if !ascii_decimal(&name) {
                let ok = name.iter().all(|&c| c < 128)
                    && (is_asciiletter(name[0]) || name[0] == '_' as u32)
                    && name.iter().all(|&c| is_asciiletter(c) || is_digit(c) || c == '_' as u32);
                if !ok {
                    return Err(if name.iter().all(|&c| c < 128) { Decline::Error } else { Decline::Unsupported });
                }
                let n: String = name.iter().map(|&c| c as u8 as char).collect();
                index(&n).ok_or(Decline::Error)? as u64 // IndexError: unknown group name
            } else {
                let v = int_of(&name);
                if v >= maxgroups as u64 {
                    return Err(Decline::Error);
                }
                v
            };
            addgroup(idx)?;
        } else if c == '0' as u32 {
            if next_char(k).is_some_and(is_octdigit) {
                k += 1;
                if next_char(k).is_some_and(is_octdigit) {
                    k += 1;
                }
            }
        } else if is_digit(c) {
            let mut digits = vec![c];
            let mut isoctal = false;
            if let Some(d) = next_char(k).filter(|&d| is_digit(d)) {
                k += 1;
                digits.push(d);
                if is_octdigit(c)
                    && is_octdigit(d)
                    && let Some(e) = next_char(k).filter(|&e| is_octdigit(e))
                {
                    k += 1;
                    digits.push(e);
                    isoctal = true;
                    let v = digits.iter().fold(0u32, |v, &d| v * 8 + (d - '0' as u32));
                    if v > 0o377 {
                        return Err(Decline::Error);
                    }
                }
            }
            if !isoctal {
                addgroup(int_of(&digits))?;
            }
        } else {
            let known = matches!(char::from_u32(c), Some('a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' | '\\'));
            if !known && is_asciiletter(c) {
                return Err(Decline::Error); // bad escape
            }
        }
    }
    Ok(())
}
