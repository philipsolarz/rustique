//! `fnmatch` patterns: CPython's `fnmatch._translate` + `_join_translated_parts` turn a
//! shell pattern into a regular expression that `re` compiles and matches. This module
//! reproduces the *meaning* of that regular expression without building or running it:
//!
//! 1. [`Program::compile`] ports `_translate` statement by statement. For a bracket
//!    expression it builds the exact set text the regex would contain (escaping, range
//!    chunks, empty-range removal, set-operation escapes, `!` → `^`) and then reads that
//!    text the way `re._parser` reads a character set. Anything outside the grammar
//!    `_translate` can produce, and anything `re` would warn about, makes compilation
//!    return `None` (the caller runs CPython's code instead).
//! 2. [`Program::matches`] evaluates `(?s:PREFIX(?>.*?SEG)….*TAIL)\z` anchored at the
//!    start: every token matches exactly one code point, so the prefix is fixed, each
//!    middle segment commits to its leftmost match (an atomic lazy group), and the tail
//!    must end the string.

use super::Unit;

/// One code-point matcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    /// A literal code point (`re.escape(c)`).
    Lit(u32),
    /// `.` under DOTALL (`?`, and `[!]`-style negated empty ranges).
    Any,
    /// `(?!)`: an empty range never matches.
    Never,
    /// A character set: index into [`Program::sets`].
    Set(u32),
    /// `*`.
    Star,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharSet {
    pub negate: bool,
    /// Inclusive ranges; a literal is `(c, c)`.
    pub items: Vec<(u32, u32)>,
}

impl CharSet {
    #[inline]
    pub fn contains(&self, c: u32) -> bool {
        self.items.iter().any(|&(lo, hi)| lo <= c && c <= hi) != self.negate
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    pub toks: Vec<Tok>,
    pub sets: Vec<CharSet>,
}

const BACKSLASH: u32 = b'\\' as u32;
const DASH: u32 = b'-' as u32;
const LBRACKET: u32 = b'[' as u32;
const RBRACKET: u32 = b']' as u32;
const BANG: u32 = b'!' as u32;
const CARET: u32 = b'^' as u32;

impl Program {
    /// Port of `fnmatch._translate(pat, '*', '.')`; `None` when the pattern is outside
    /// what this module reproduces exactly.
    pub fn compile<T: Unit>(pat: &[T]) -> Option<Program> {
        let mut prog = Program { toks: Vec::with_capacity(pat.len()), sets: Vec::new() };
        prog.compile_into(pat)?;
        Some(prog)
    }

    /// Like [`Program::compile`], reusing this program's buffers.
    pub fn compile_into<T: Unit>(&mut self, pat: &[T]) -> Option<()> {
        self.toks.clear();
        self.sets.clear();
        let n = pat.len();
        let at = |i: usize| pat[i].cp();
        let mut i = 0usize;
        while i < n {
            let c = at(i);
            i += 1;
            if c == b'*' as u32 {
                self.toks.push(Tok::Star);
                while i < n && at(i) == b'*' as u32 {
                    i += 1;
                }
            } else if c == b'?' as u32 {
                self.toks.push(Tok::Any);
            } else if c == LBRACKET {
                let mut j = i;
                if j < n && at(j) == BANG {
                    j += 1;
                }
                if j < n && at(j) == RBRACKET {
                    j += 1;
                }
                while j < n && at(j) != RBRACKET {
                    j += 1;
                }
                if j >= n {
                    self.toks.push(Tok::Lit(LBRACKET));
                } else {
                    let stuff = bracket_text(pat, i, j)?;
                    i = j + 1;
                    if stuff.is_empty() {
                        self.toks.push(Tok::Never);
                    } else if stuff == [BANG] {
                        self.toks.push(Tok::Any);
                    } else {
                        let set = parse_set(&final_set_text(stuff))?;
                        self.toks.push(Tok::Set(self.sets.len() as u32));
                        self.sets.push(set);
                    }
                }
            } else {
                self.toks.push(Tok::Lit(c));
            }
        }
        Some(())
    }

    #[inline]
    fn tok_matches(&self, t: Tok, c: u32) -> bool {
        match t {
            Tok::Lit(l) => l == c,
            Tok::Any => true,
            Tok::Never | Tok::Star => false,
            Tok::Set(k) => self.sets[k as usize].contains(c),
        }
    }

    #[inline]
    fn seg_matches<T: Unit>(&self, seg: &[Tok], s: &[T]) -> bool {
        seg.iter().zip(s).all(|(&t, &c)| self.tok_matches(t, c.cp()))
    }

    /// `re.compile(translate(pat)).match(name) is not None`.
    pub fn matches<T: Unit>(&self, name: &[T]) -> bool {
        let toks = &self.toks[..];
        let n = name.len();
        let Some(first) = toks.iter().position(|&t| t == Tok::Star) else {
            return toks.len() == n && self.seg_matches(toks, name);
        };
        let prefix = &toks[..first];
        if prefix.len() > n || !self.seg_matches(prefix, name) {
            return false;
        }
        let mut pos = prefix.len();
        let mut rest = &toks[first + 1..];
        while let Some(next) = rest.iter().position(|&t| t == Tok::Star) {
            let seg = &rest[..next];
            // `(?>.*?SEG)`: the leftmost match commits.
            let mut found = None;
            let mut p = pos;
            while p + seg.len() <= n {
                if self.seg_matches(seg, &name[p..]) {
                    found = Some(p);
                    break;
                }
                p += 1;
            }
            let Some(p) = found else { return false };
            pos = p + seg.len();
            rest = &rest[next + 1..];
        }
        // `.*TAIL\z`
        n - pos >= rest.len() && self.seg_matches(rest, &name[n - rest.len()..])
    }
}

/// The set text `_translate` builds from `pat[i:j]` (the characters between the brackets),
/// before the set-operation escapes and the leading `!`/`^`/`[` handling.
fn bracket_text<T: Unit>(pat: &[T], i: usize, j: usize) -> Option<Vec<u32>> {
    let raw: Vec<u32> = pat[i..j].iter().map(|c| c.cp()).collect();
    if !raw.contains(&DASH) {
        // stuff.replace('\\', r'\\')
        return Some(escape_chars(&raw, false));
    }
    let at = |k: usize| pat[k].cp();
    // pat.find('-', k, j)
    let find_dash = |from: usize| -> Option<usize> { (from..j).find(|&k| at(k) == DASH) };
    let mut chunks: Vec<Vec<u32>> = Vec::new();
    let mut i = i;
    let mut k = if at(i) == BANG { i + 2 } else { i + 1 };
    loop {
        let Some(found) = (if k <= j { find_dash(k) } else { None }) else { break };
        chunks.push((i..found).map(at).collect());
        i = found + 1;
        k = found + 3;
    }
    let chunk: Vec<u32> = (i..j).map(at).collect();
    if !chunk.is_empty() {
        chunks.push(chunk);
    } else {
        chunks.last_mut()?.push(DASH);
    }
    // Remove empty ranges -- invalid in RE.
    for k in (1..chunks.len()).rev() {
        let last = *chunks[k - 1].last()?;
        let first = *chunks[k].first()?;
        if last > first {
            let tail: Vec<u32> = chunks[k][1..].to_vec();
            let prev = &mut chunks[k - 1];
            prev.pop();
            prev.extend_from_slice(&tail);
            chunks.remove(k);
        }
    }
    // '-'.join(s.replace('\\', r'\\').replace('-', r'\-') for s in chunks)
    let mut out = Vec::new();
    for (n, ch) in chunks.iter().enumerate() {
        if n > 0 {
            out.push(DASH);
        }
        out.extend(escape_chars(ch, true));
    }
    Some(out)
}

fn escape_chars(s: &[u32], dashes: bool) -> Vec<u32> {
    let mut out = Vec::with_capacity(s.len() + 2);
    for &c in s {
        if c == BACKSLASH || (dashes && c == DASH) {
            out.push(BACKSLASH);
        }
        out.push(c);
    }
    out
}

/// `_re_setops_sub(r'\\\1', stuff)`, then `!` → `^`, or a leading `^`/`[` escaped.
fn final_set_text(stuff: Vec<u32>) -> Vec<u32> {
    let mut s = Vec::with_capacity(stuff.len() + 2);
    for c in stuff {
        if c == b'&' as u32 || c == b'~' as u32 || c == b'|' as u32 {
            s.push(BACKSLASH);
        }
        s.push(c);
    }
    if s[0] == BANG {
        s[0] = CARET;
    } else if s[0] == CARET || s[0] == LBRACKET {
        s.insert(0, BACKSLASH);
    }
    s
}

/// Read `[` + `text` + `]` as `re._parser` reads a character set (escapes and ranges only:
/// the grammar `fnmatch` produces). `None` on anything else, on anything `re` would warn
/// about (FutureWarning for possible set operations or nested sets), and when the set
/// would end before the closing bracket we appended.
fn parse_set(text: &[u32]) -> Option<CharSet> {
    // Tokens as `re._parser.Tokenizer` yields them: a backslash takes the next character.
    #[derive(Clone, Copy, PartialEq)]
    enum T {
        Ch(u32),
        Esc(u32),
        End,
    }
    let mut full: Vec<u32> = Vec::with_capacity(text.len() + 1);
    full.extend_from_slice(text);
    full.push(RBRACKET);
    let mut pos = 0usize;
    let next = |pos: &mut usize| -> Option<T> {
        if *pos >= full.len() {
            return Some(T::End);
        }
        let c = full[*pos];
        if c == BACKSLASH {
            let e = *full.get(*pos + 1)?;
            *pos += 2;
            Some(T::Esc(e))
        } else {
            *pos += 1;
            Some(T::Ch(c))
        }
    };
    let peek = |pos: usize| -> Option<T> {
        let mut p = pos;
        next(&mut p)
    };
    // Literal value of an escape inside a set (`_class_escape`), for the escapes
    // `_translate` emits.
    let esc = |e: u32| -> Option<u32> {
        if e == BACKSLASH || [b'-', b'&', b'~', b'|', b'^', b'['].iter().any(|&x| e == x as u32) {
            Some(e)
        } else {
            None
        }
    };
    // `source.next == '['` right after the opening bracket: FutureWarning.
    if peek(pos)? == T::Ch(LBRACKET) {
        return None;
    }
    let mut set = CharSet::default();
    if peek(pos)? == T::Ch(CARET) {
        set.negate = true;
        next(&mut pos)?;
    }
    let mut any_item = false;
    loop {
        let this = next(&mut pos)?;
        let code1 = match this {
            T::End => return None,
            T::Ch(RBRACKET) if any_item => break,
            T::Esc(e) => esc(e)?,
            T::Ch(c) => {
                let setop = [b'-', b'&', b'~', b'|'].iter().any(|&x| c == x as u32);
                if any_item && setop && peek(pos)? == T::Ch(c) {
                    return None;
                }
                c
            }
        };
        if peek(pos)? == T::Ch(DASH) {
            next(&mut pos)?;
            let that = next(&mut pos)?;
            match that {
                T::End => return None,
                T::Ch(RBRACKET) => {
                    set.items.push((code1, code1));
                    set.items.push((DASH, DASH));
                    any_item = true;
                    break;
                }
                _ => {}
            }
            let code2 = match that {
                T::Esc(e) => esc(e)?,
                T::Ch(DASH) => return None,
                T::Ch(c) => c,
                T::End => return None,
            };
            if code2 < code1 {
                return None;
            }
            set.items.push((code1, code2));
        } else {
            set.items.push((code1, code1));
        }
        any_item = true;
    }
    // The set must end exactly at the bracket we appended.
    (pos == full.len()).then_some(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, pat: &str) -> Option<bool> {
        let p: Vec<u32> = pat.chars().map(|c| c as u32).collect();
        let n: Vec<u32> = name.chars().map(|c| c as u32).collect();
        Program::compile(&p).map(|prog| prog.matches(&n))
    }

    #[test]
    fn basics() {
        assert_eq!(m("abc", "abc"), Some(true));
        assert_eq!(m("abc", "a*"), Some(true));
        assert_eq!(m("abc", "*c"), Some(true));
        assert_eq!(m("abc", "*d"), Some(false));
        assert_eq!(m("abc", "a?c"), Some(true));
        assert_eq!(m("a\nc", "a?c"), Some(true));
        assert_eq!(m("abc\n", "abc"), Some(false));
        assert_eq!(m("abcabd", "*ab?*d"), Some(true));
        assert_eq!(m("main.py", "*.py"), Some(true));
        assert_eq!(m("main.pyc", "*.py"), Some(false));
        assert_eq!(m("a", "[abc]"), Some(true));
        assert_eq!(m("d", "[!abc]"), Some(true));
        assert_eq!(m("a", "[!abc]"), Some(false));
        assert_eq!(m("]", "[]]"), Some(true));
        assert_eq!(m("-", "[a-]"), Some(true));
        assert_eq!(m("b", "[a-c]"), Some(true));
        assert_eq!(m("b", "[c-a]"), Some(false));
        assert_eq!(m("[", "["), Some(true));
        assert_eq!(m("[a", "[a"), Some(true));
        assert_eq!(m("\\", "[\\]"), Some(true));
        assert_eq!(m("x", "[!]"), Some(false));
        assert_eq!(m("&", "[&]"), Some(true));
        assert_eq!(m("^", "[^]"), Some(true));
        assert_eq!(m("x", "[!b-a]"), Some(true));
        assert_eq!(m("-", "[a-b-c]"), Some(true));
        assert_eq!(m("c", "[a-b-c]"), Some(true));
        assert_eq!(m("d", "[a-b-c]"), Some(false));
        assert_eq!(m("", "*"), Some(true));
        assert_eq!(m("", ""), Some(true));
        assert_eq!(m("a", ""), Some(false));
    }
}
