//! Port of the checks of `re._parser.parse_template(repl, pattern)`: would CPython compile
//! the replacement template of `Pattern.sub` without raising? (Only the verdict is needed:
//! `re.sub` then lets CPython's own `_compile_template` build it.) Allocation-free: it runs on
//! every accelerated template `re.sub`.

use super::Decline;
use super::parser::int_of;

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

/// `Tokenizer(repl)` over code units: `(is_escape, char)` tokens.
struct Toks<'a> {
    s: &'a [u32],
    i: usize,
}

impl Toks<'_> {
    /// The next token without consuming it (`Err`: a trailing backslash, which the
    /// tokenizer reports as "bad escape (end of pattern)").
    #[inline]
    fn peek(&self) -> Result<Option<(bool, u32)>, Decline> {
        match self.s.get(self.i) {
            None => Ok(None),
            Some(&c) if c == BACKSLASH => match self.s.get(self.i + 1) {
                Some(&d) => Ok(Some((true, d))),
                None => Err(Decline::Error),
            },
            Some(&c) => Ok(Some((false, c))),
        }
    }
    #[inline]
    fn next(&mut self) -> Result<Option<(bool, u32)>, Decline> {
        let t = self.peek()?;
        if let Some((esc, _)) = t {
            self.i += if esc { 2 } else { 1 };
        }
        Ok(t)
    }
    /// `s.next` if it is a plain char.
    #[inline]
    fn next_char(&self) -> Result<Option<u32>, Decline> {
        Ok(match self.peek()? {
            Some((false, c)) => Some(c),
            _ => None,
        })
    }
}

/// `Ok` when `parse_template(repl, pattern)` returns normally for a pattern with `groups`
/// groups whose `groupindex` maps names with `index` (names given as their ASCII code units).
/// Names other than ASCII identifiers decline.
pub fn check(
    repl: &[u32],
    groups: u32,
    maxgroups: u32,
    index: &mut dyn FnMut(&[u32]) -> Option<u32>,
) -> Result<(), Decline> {
    let mut t = Toks { s: repl, i: 0 };
    // the Tokenizer constructor reads the first token: a lone trailing backslash raises
    t.peek()?;
    let addgroup = |index: u64| if index > groups as u64 { Err(Decline::Error) } else { Ok(()) };
    while let Some((esc, c)) = t.next()? {
        if !esc {
            continue;
        }
        if c == 'g' as u32 {
            if t.next_char()? != Some('<' as u32) {
                return Err(Decline::Error); // missing <
            }
            t.next()?;
            // getuntil(">"): a name is a run of plain chars (an escape inside makes it an
            // invalid identifier, which raises)
            let start = t.i;
            let mut escaped = false;
            loop {
                match t.next()? {
                    None => return Err(Decline::Error), // missing >, unterminated name
                    Some((false, x)) if x == '>' as u32 => break,
                    Some((e, _)) => escaped |= e,
                }
            }
            let name = &repl[start..t.i - 1];
            if name.is_empty() {
                return Err(Decline::Error); // missing group name
            }
            let idx = if !escaped && name.iter().all(|&c| is_digit(c)) {
                let v = int_of(name);
                if v >= maxgroups as u64 {
                    return Err(Decline::Error);
                }
                v
            } else {
                let ascii = name.iter().all(|&c| c < 128);
                let ok = !escaped
                    && ascii
                    && (is_asciiletter(name[0]) || name[0] == '_' as u32)
                    && name.iter().all(|&c| is_asciiletter(c) || is_digit(c) || c == '_' as u32);
                if !ok {
                    return Err(if ascii { Decline::Error } else { Decline::Unsupported });
                }
                index(name).ok_or(Decline::Error)? as u64 // IndexError: unknown group name
            };
            addgroup(idx)?;
        } else if c == '0' as u32 {
            if t.next_char()?.is_some_and(is_octdigit) {
                t.next()?;
                if t.next_char()?.is_some_and(is_octdigit) {
                    t.next()?;
                }
            }
        } else if is_digit(c) {
            let mut digits = [c, 0, 0];
            let mut n = 1;
            let mut isoctal = false;
            if let Some(d) = t.next_char()?.filter(|&d| is_digit(d)) {
                t.next()?;
                digits[1] = d;
                n = 2;
                if is_octdigit(c)
                    && is_octdigit(d)
                    && let Some(e) = t.next_char()?.filter(|&e| is_octdigit(e))
                {
                    t.next()?;
                    digits[2] = e;
                    isoctal = true;
                    let v = digits.iter().fold(0u32, |v, &d| v * 8 + (d - '0' as u32));
                    if v > 0o377 {
                        return Err(Decline::Error);
                    }
                }
            }
            if !isoctal {
                addgroup(int_of(&digits[..n]))?;
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
