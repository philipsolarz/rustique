//! CPython's `textwrap` algorithms over code points, bit for bit: `TextWrapper`'s whitespace
//! munging, the chunking of `wordsep_re` / `wordsep_simple_re` (`re.split` on a pattern
//! that matches at every position, so the chunks are the consecutive matches), sentence
//! ending fixes, `_wrap_chunks` with `_handle_long_word`, `max_lines` and the placeholder,
//! plus `dedent` and `indent` (without predicate).
//!
//! Character classes come from the caller ([`Classes`]) so that the Python bindings use
//! CPython's own Unicode database (`\w`, `\d`, `str.isspace`); the regex semantics are
//! those of `sre` for `str` patterns without flags.

/// Unicode properties as CPython's `sre` and `str` methods see them.
pub trait Classes {
    /// `\w`: `str.isalnum()` or `_`.
    fn is_word(&self, c: u32) -> bool;
    /// `\d`: `str.isdecimal()`.
    fn is_decimal(&self, c: u32) -> bool;
    /// `str.isspace()` (what `strip()`/`rstrip()`/`lstrip()`/`split()` remove).
    fn is_space(&self, c: u32) -> bool;
}

/// `textwrap._whitespace`: the six ASCII whitespace characters of the regexes.
#[inline]
pub fn is_ws(c: u32) -> bool {
    matches!(c, 0x09..=0x0d | 0x20)
}

/// One chunk: a range of the munged text, or the literal two spaces that
/// `_fix_sentence_endings` substitutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chunk {
    Text(usize, usize),
    TwoSpaces,
}

impl Chunk {
    #[inline]
    pub fn len(self) -> usize {
        match self {
            Chunk::Text(a, b) => b - a,
            Chunk::TwoSpaces => 2,
        }
    }
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
    fn get(self, text: &[u32]) -> &[u32] {
        match self {
            Chunk::Text(a, b) => &text[a..b],
            Chunk::TwoSpaces => &[0x20, 0x20],
        }
    }
}

/// `str.expandtabs(tabsize)` then (if `replace`) `translate` of `_whitespace` to spaces.
pub fn munge(text: &[u32], expand_tabs: bool, tabsize: i64, replace: bool) -> Vec<u32> {
    let mut out = Vec::with_capacity(text.len());
    if expand_tabs {
        let mut col: i64 = 0;
        for &c in text {
            if c == 0x09 {
                if tabsize > 0 {
                    let incr = tabsize - col % tabsize;
                    col += incr;
                    out.extend(std::iter::repeat_n(0x20, incr as usize));
                }
            } else {
                col += 1;
                if c == 0x0a || c == 0x0d {
                    col = 0;
                }
                out.push(c);
            }
        }
    } else {
        out.extend_from_slice(text);
    }
    if replace {
        for c in &mut out {
            if is_ws(*c) {
                *c = 0x20;
            }
        }
    }
    out
}

/// `wordsep_simple_re.split(text)` without empty strings: maximal runs of `_whitespace`
/// and of everything else.
pub fn split_simple(text: &[u32]) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let ws = is_ws(text[i]);
        let mut j = i + 1;
        while j < text.len() && is_ws(text[j]) == ws {
            j += 1;
        }
        out.push(Chunk::Text(i, j));
        i = j;
    }
    out
}

/// `wordsep_re.split(text)` without empty strings.
///
/// The pattern (`TextWrapper.wordsep_re`) is
/// `( WS+ | (?<=WP) -{2,} (?=\w) | NWS+? (?: -(?: (?<=LT{2}-) | (?<=LT-LT-)) (?= LT -? LT)
///  | (?=WS|\z) | (?<=WP) (?=-{2,}\w) ) )` with `WP = [\w!"'&.,?]`, `LT = [^\d\W]`.
/// At every position one alternative matches (whitespace: the first; anything else: the
/// third, at the end of the non-whitespace run at the latest), so `split` yields exactly the
/// consecutive matches. Each is found as `sre` would: alternatives in order, the lazy
/// `NWS+?` tried with 1, 2, ... characters, lookarounds on the whole string.
pub fn split_words(text: &[u32], cls: &impl Classes) -> Vec<Chunk> {
    let n = text.len();
    let at = |i: usize| -> Option<u32> { text.get(i).copied() };
    let word = |c: Option<u32>| c.is_some_and(|c| cls.is_word(c));
    let letter = |c: Option<u32>| c.is_some_and(|c| cls.is_word(c) && !cls.is_decimal(c));
    let wp = |c: Option<u32>| {
        c.is_some_and(|c| cls.is_word(c) || matches!(c, 0x21 | 0x22 | 0x27 | 0x26 | 0x2e | 0x2c | 0x3f))
    };
    let hyph = |i: usize| at(i) == Some(0x2d);
    // Length of the run of '-' starting at i.
    let run = |i: usize| (i..n).take_while(|&k| text[k] == 0x2d).count();
    // `-{2,}\w` at i (greedy run, then a word character).
    let dashes_word = |i: usize| {
        let r = run(i);
        r >= 2 && word(at(i + r))
    };
    let mut out = Vec::new();
    let mut p = 0;
    while p < n {
        if is_ws(text[p]) {
            let mut q = p + 1;
            while q < n && is_ws(text[q]) {
                q += 1;
            }
            out.push(Chunk::Text(p, q));
            p = q;
            continue;
        }
        // Alternative 2: em-dash between words.
        if p > 0 && wp(at(p - 1)) && dashes_word(p) {
            let q = p + run(p);
            out.push(Chunk::Text(p, q));
            p = q;
            continue;
        }
        // Alternative 3: lazy non-whitespace, then one of three tails.
        let mut end = None;
        let mut q = p + 1;
        loop {
            // text[p..q] is non-whitespace (q - p >= 1).
            // (a) hyphenated word: '-' at q, lookbehinds at q + 1, lookahead after it.
            if hyph(q) {
                let behind = (q >= 2 && letter(at(q - 2)) && letter(at(q - 1)))
                    || (q >= 3 && letter(at(q - 3)) && hyph(q - 2) && letter(at(q - 1)));
                let ahead = letter(at(q + 1)) && (letter(at(q + 2)) || (hyph(q + 2) && letter(at(q + 3))));
                if behind && ahead {
                    end = Some(q + 1);
                    break;
                }
            }
            // (b) end of word.
            if q == n || is_ws(text[q]) {
                end = Some(q);
                break;
            }
            // (c) em-dash follows.
            if wp(at(q - 1)) && dashes_word(q) {
                end = Some(q);
                break;
            }
            q += 1;
            if is_ws(text[q - 1]) {
                break; // unreachable: (b) stops at whitespace
            }
        }
        let q = end.expect("the end-of-word tail always matches");
        out.push(Chunk::Text(p, q));
        p = q;
    }
    out
}

/// `TextWrapper._fix_sentence_endings`: a `" "` chunk after a chunk matching
/// `[a-z][.!?]["']?\z` becomes two spaces.
pub fn fix_sentence_endings(text: &[u32], chunks: &mut [Chunk]) {
    let ends_sentence = |c: &[u32]| {
        let lower = |x: u32| (0x61..=0x7a).contains(&x);
        let punct = |x: u32| matches!(x, 0x2e | 0x21 | 0x3f);
        let quote = |x: u32| matches!(x, 0x22 | 0x27);
        let n = c.len();
        (n >= 2 && lower(c[n - 2]) && punct(c[n - 1])) || (n >= 3 && lower(c[n - 3]) && punct(c[n - 2]) && quote(c[n - 1]))
    };
    let mut i = 0;
    while i + 1 < chunks.len() {
        if chunks[i + 1].get(text) == [0x20] && ends_sentence(chunks[i].get(text)) {
            chunks[i + 1] = Chunk::TwoSpaces;
            i += 2;
        } else {
            i += 1;
        }
    }
}

/// The options `_wrap_chunks` and `_handle_long_word` read.
#[derive(Debug, Clone)]
pub struct WrapOpts<'a> {
    pub width: i64,
    pub initial_indent: &'a [u32],
    pub subsequent_indent: &'a [u32],
    pub break_long_words: bool,
    pub drop_whitespace: bool,
    pub break_on_hyphens: bool,
    pub max_lines: Option<i64>,
    pub placeholder: &'a [u32],
}

/// Why `_wrap_chunks` raises (the caller lets CPython raise it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapError {
    InvalidWidth,
    PlaceholderTooLarge,
    /// The loop would never end (CPython's does not either: e.g. whitespace-only text
    /// with an indent wider than the width). The caller lets CPython run it, where the
    /// loop stays interruptible.
    NoProgress,
}

/// A piece of a line: part of a chunk or of the placeholder.
#[derive(Debug, Clone, Copy)]
enum Piece {
    Chunk(Chunk, usize, usize),
    Placeholder,
}

fn is_blank(s: &[u32], cls: &impl Classes) -> bool {
    s.iter().all(|&c| cls.is_space(c))
}

fn rstrip<'s>(s: &'s [u32], cls: &impl Classes) -> &'s [u32] {
    let mut e = s.len();
    while e > 0 && cls.is_space(s[e - 1]) {
        e -= 1;
    }
    &s[..e]
}

fn lstrip<'s>(s: &'s [u32], cls: &impl Classes) -> &'s [u32] {
    let mut b = 0;
    while b < s.len() && cls.is_space(s[b]) {
        b += 1;
    }
    &s[b..]
}

/// `TextWrapper._wrap_chunks(chunks)`: the lines as code points.
pub fn wrap_chunks(text: &[u32], chunks: Vec<Chunk>, o: &WrapOpts<'_>, cls: &impl Classes) -> Result<Vec<Vec<u32>>, WrapError> {
    let mut lines: Vec<Vec<u32>> = Vec::new();
    if o.width <= 0 {
        return Err(WrapError::InvalidWidth);
    }
    if let Some(m) = o.max_lines {
        let indent = if m > 1 { o.subsequent_indent } else { o.initial_indent };
        if (indent.len() + lstrip(o.placeholder, cls).len()) as i64 > o.width {
            return Err(WrapError::PlaceholderTooLarge);
        }
    }
    // Chunks as (chunk, start, end) slices so `_handle_long_word` can split them.
    let mut rev: Vec<(Chunk, usize, usize)> = chunks.into_iter().rev().map(|c| (c, 0, c.len())).collect();
    let piece_text = |p: &Piece| -> Vec<u32> {
        match *p {
            Piece::Chunk(c, a, b) => c.get(text)[a..b].to_vec(),
            Piece::Placeholder => o.placeholder.to_vec(),
        }
    };
    let slice_of = |(c, a, b): (Chunk, usize, usize)| -> &[u32] { &c.get(text)[a..b] };
    let plen = |p: &Piece| -> usize {
        match *p {
            Piece::Chunk(_, a, b) => b - a,
            Piece::Placeholder => o.placeholder.len(),
        }
    };
    let pblank = |p: &Piece| -> bool {
        match *p {
            Piece::Chunk(c, a, b) => is_blank(&c.get(text)[a..b], cls),
            Piece::Placeholder => is_blank(o.placeholder, cls),
        }
    };
    let build = |indent: &[u32], line: &[Piece]| -> Vec<u32> {
        let mut s = indent.to_vec();
        for p in line {
            s.extend(piece_text(p));
        }
        s
    };
    let mut stalls = 0;
    while !rev.is_empty() {
        let before = (rev.len(), rev.last().copied(), lines.len());
        let mut cur: Vec<Piece> = Vec::new();
        let mut cur_len: i64 = 0;
        let indent = if lines.is_empty() { o.initial_indent } else { o.subsequent_indent };
        let width = o.width - indent.len() as i64;
        if o.drop_whitespace
            && !lines.is_empty()
            && let Some(&last) = rev.last()
            && is_blank(slice_of(last), cls)
        {
            rev.pop();
        }
        while let Some(&(c, a, b)) = rev.last() {
            let l = (b - a) as i64;
            if cur_len + l <= width {
                cur.push(Piece::Chunk(c, a, b));
                rev.pop();
                cur_len += l;
            } else {
                break;
            }
        }
        if let Some(&(c, a, b)) = rev.last()
            && (b - a) as i64 > width
        {
            // _handle_long_word
            let space_left = if width < 1 { 1 } else { width - cur_len };
            if o.break_long_words && space_left > 0 {
                let mut end = space_left as usize;
                let chunk = &c.get(text)[a..b];
                if o.break_on_hyphens && chunk.len() > end {
                    // hyphen = chunk.rfind('-', 0, space_left)
                    if let Some(h) = chunk[..end].iter().rposition(|&x| x == 0x2d)
                        && h > 0
                        && chunk[..h].iter().any(|&x| x != 0x2d)
                    {
                        end = h + 1;
                    }
                }
                let end = end.min(b - a);
                cur.push(Piece::Chunk(c, a, a + end));
                let last = rev.len() - 1;
                rev[last] = (c, a + end, b);
            } else if cur.is_empty() {
                let (c, a, b) = rev.pop().expect("non-empty");
                cur.push(Piece::Chunk(c, a, b));
            }
            cur_len = cur.iter().map(|p| plen(p) as i64).sum();
        }
        if o.drop_whitespace
            && let Some(p) = cur.last()
            && pblank(p)
        {
            cur_len -= plen(p) as i64;
            cur.pop();
        }
        if !cur.is_empty() {
            let fits = match o.max_lines {
                None => true,
                Some(m) => {
                    (lines.len() as i64) + 1 < m
                        || ((rev.is_empty()
                            || (o.drop_whitespace && rev.len() == 1 && is_blank(slice_of(rev[0]), cls)))
                            && cur_len <= width)
                }
            };
            if fits {
                lines.push(build(indent, &cur));
            } else {
                let mut placed = false;
                while let Some(p) = cur.last() {
                    if !pblank(p) && cur_len + o.placeholder.len() as i64 <= width {
                        cur.push(Piece::Placeholder);
                        lines.push(build(indent, &cur));
                        placed = true;
                        break;
                    }
                    cur_len -= plen(p) as i64;
                    cur.pop();
                }
                if !placed {
                    if let Some(prev) = lines.last() {
                        let prev_line = rstrip(prev, cls).to_vec();
                        if (prev_line.len() + o.placeholder.len()) as i64 <= o.width {
                            let mut l = prev_line;
                            l.extend_from_slice(o.placeholder);
                            let last = lines.len() - 1;
                            lines[last] = l;
                            break;
                        }
                    }
                    let mut l = indent.to_vec();
                    l.extend_from_slice(lstrip(o.placeholder, cls));
                    lines.push(l);
                }
                break;
            }
        }
        if (rev.len(), rev.last().copied(), lines.len()).0 == before.0 && rev.last().copied() == before.1 {
            // Nothing consumed: the next iteration starts from the same chunks.
            if lines.len() == before.2 {
                return Err(WrapError::NoProgress);
            }
            stalls += 1;
            if o.max_lines.is_none() && stalls >= 2 {
                return Err(WrapError::NoProgress);
            }
        } else {
            stalls = 0;
        }
    }
    Ok(lines)
}

/// `textwrap.dedent(text)`: `None` when the result is `text` itself (CPython returns the
/// same object: one line, nothing removed).
pub fn dedent(text: &[u32], cls: &impl Classes) -> Option<Vec<u32>> {
    let lines: Vec<&[u32]> = text.split(|&c| c == 0x0a).collect();
    let blank = |l: &[u32]| !l.is_empty() && l.iter().all(|&c| cls.is_space(c));
    let mut l1: Option<&[u32]> = None;
    let mut l2: Option<&[u32]> = None;
    for &l in &lines {
        if l.is_empty() || blank(l) {
            continue;
        }
        if l1.is_none_or(|m| l < m) {
            l1 = Some(l);
        }
        if l2.is_none_or(|m| l > m) {
            l2 = Some(l);
        }
    }
    let mut margin = 0;
    if let (Some(l1), Some(l2)) = (l1, l2) {
        for (i, &c) in l1.iter().enumerate() {
            margin = i;
            if l2.get(i) != Some(&c) || !(c == 0x20 || c == 0x09) {
                break;
            }
        }
    }
    if lines.len() == 1 && margin == 0 && !blank(lines[0]) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len());
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.push(0x0a);
        }
        if !blank(l) {
            out.extend_from_slice(&l[margin.min(l.len())..]);
        }
    }
    Some(out)
}

/// `str.splitlines(True)` boundaries: `\n \r \r\n \x0b \x0c \x1c \x1d \x1e \x85    `.
pub fn is_line_break(c: u32) -> bool {
    matches!(c, 0x0a | 0x0b | 0x0c | 0x0d | 0x1c | 0x1d | 0x1e | 0x85 | 0x2028 | 0x2029)
}

/// The lines of `str.splitlines(True)` as ranges.
pub fn splitlines_keepends(text: &[u32]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut i, n) = (0, text.len());
    while i < n {
        let start = i;
        while i < n && !is_line_break(text[i]) {
            i += 1;
        }
        if i < n {
            if text[i] == 0x0d && i + 1 < n && text[i + 1] == 0x0a {
                i += 2;
            } else {
                i += 1;
            }
        }
        out.push((start, i));
    }
    out
}

/// `textwrap.indent(text, prefix)` (no predicate): `None` when CPython returns `text`
/// itself (a single line that is all whitespace: nothing to prefix, `''.join([text])`).
pub fn indent(text: &[u32], prefix: &[u32], cls: &impl Classes) -> Option<Vec<u32>> {
    let lines = splitlines_keepends(text);
    let space = |l: &[u32]| !l.is_empty() && l.iter().all(|&c| cls.is_space(c));
    if lines.len() == 1 && space(text) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() + prefix.len() * lines.len());
    for (a, b) in lines {
        let l = &text[a..b];
        if !space(l) {
            out.extend_from_slice(prefix);
        }
        out.extend_from_slice(l);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ASCII-only classes for the logic tests (the bindings use CPython's database).
    struct Ascii;
    impl Classes for Ascii {
        fn is_word(&self, c: u32) -> bool {
            char::from_u32(c).is_some_and(|c| c.is_alphanumeric() || c == '_')
        }
        fn is_decimal(&self, c: u32) -> bool {
            char::from_u32(c).is_some_and(|c| c.is_ascii_digit())
        }
        fn is_space(&self, c: u32) -> bool {
            matches!(c, 0x09..=0x0d | 0x1c..=0x20 | 0x85 | 0xa0)
        }
    }

    fn u(s: &str) -> Vec<u32> {
        s.chars().map(u32::from).collect()
    }
    fn s(v: &[u32]) -> String {
        v.iter().map(|&c| char::from_u32(c).unwrap()).collect()
    }
    fn chunks(t: &str) -> Vec<String> {
        let v = u(t);
        split_words(&v, &Ascii).into_iter().map(|c| s(c.get(&v))).collect()
    }

    #[test]
    fn doc_example() {
        assert_eq!(
            chunks("Hello there -- you goof-ball, use the -b option!"),
            ["Hello", " ", "there", " ", "--", " ", "you", " ", "goof-", "ball,", " ", "use", " ", "the", " ", "-b", " ", "option!"]
        );
        assert_eq!(chunks("Look, goof-ball -- use the -b option!")[2..4], ["goof-".to_string(), "ball".to_string()]);
    }

    #[test]
    fn em_dashes_and_hyphens() {
        assert_eq!(chunks("this--that"), ["this", "--", "that"]);
        assert_eq!(chunks("a-b-c"), ["a-b-c"]);
        assert_eq!(chunks("ab-cd"), ["ab-", "cd"]);
        assert_eq!(chunks("a-b-cd-ef"), ["a-b-", "cd-", "ef"]);
        assert_eq!(chunks("--x"), ["--x"]);
        assert_eq!(chunks("x--"), ["x--"]);
        assert_eq!(chunks("well-known"), ["well-", "known"]);
        assert_eq!(chunks("12-34"), ["12-34"]);
    }

    #[test]
    fn wrap_basics() {
        let t = u("The quick brown fox jumps over the lazy dog");
        let ch = split_words(&t, &Ascii);
        let o = WrapOpts {
            width: 10,
            initial_indent: &[],
            subsequent_indent: &[],
            break_long_words: true,
            drop_whitespace: true,
            break_on_hyphens: true,
            max_lines: None,
            placeholder: &u(" [...]"),
        };
        let lines: Vec<String> = wrap_chunks(&t, ch, &o, &Ascii).unwrap().iter().map(|l| s(l)).collect();
        assert_eq!(lines, ["The quick", "brown fox", "jumps over", "the lazy", "dog"]);
    }

    #[test]
    fn dedent_and_indent() {
        assert_eq!(dedent(&u("  a\n  b"), &Ascii).map(|v| s(&v)), Some("a\nb".into()));
        assert_eq!(dedent(&u("abc"), &Ascii), None);
        assert_eq!(dedent(&u("  abc"), &Ascii).map(|v| s(&v)), Some("abc".into()));
        assert_eq!(dedent(&u("   "), &Ascii).map(|v| s(&v)), Some("".into()));
        assert_eq!(indent(&u("a\n\nb"), &u("> "), &Ascii).map(|v| s(&v)), Some("> a\n\n> b".into()));
        assert_eq!(indent(&u("  "), &u("> "), &Ascii), None);
    }
}
