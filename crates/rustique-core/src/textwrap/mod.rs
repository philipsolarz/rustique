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
    c == 0x20 || c.wrapping_sub(0x09) < 5
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

/// `str.expandtabs(tabsize)` then (if `replace`) `translate` of `_whitespace` to spaces,
/// written into `out` (cleared first; reused between calls, so steady-state calls do not
/// allocate). `text` is any PEP 393 code-unit width.
pub fn munge_into<T: Copy + Into<u32>>(text: &[T], expand_tabs: bool, tabsize: i64, replace: bool, out: &mut Vec<u32>) {
    out.clear();
    out.reserve(text.len());
    // `_whitespace` minus the space itself (mapping ' ' to ' ' changes nothing).
    let map = |c: u32| if replace && c.wrapping_sub(0x09) < 5 { 0x20 } else { c };
    // Without a tab, `expandtabs` is the identity (its column counter matters for tabs only).
    if expand_tabs && text.iter().any(|&c| c.into() == 0x09) {
        let mut col: i64 = 0;
        for &c in text {
            let c: u32 = c.into();
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
                out.push(map(c));
            }
        }
    } else {
        out.extend(text.iter().map(|&c| map(c.into())));
    }
}

/// [`munge_into`] into a new `Vec`.
pub fn munge(text: &[u32], expand_tabs: bool, tabsize: i64, replace: bool) -> Vec<u32> {
    let mut out = Vec::new();
    munge_into(text, expand_tabs, tabsize, replace, &mut out);
    out
}

/// `wordsep_simple_re.split(text)` without empty strings, into `out` (cleared first):
/// maximal runs of `_whitespace` and of everything else.
pub fn split_simple_into(text: &[u32], out: &mut Vec<Chunk>) {
    out.clear();
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
}

/// [`split_simple_into`] into a new `Vec`.
pub fn split_simple(text: &[u32]) -> Vec<Chunk> {
    let mut out = Vec::new();
    split_simple_into(text, &mut out);
    out
}

/// `wordsep_re.split(text)` without empty strings, into `out` (cleared first).
///
/// The pattern (`TextWrapper.wordsep_re`) is
/// `( WS+ | (?<=WP) -{2,} (?=\w) | NWS+? (?: -(?: (?<=LT{2}-) | (?<=LT-LT-)) (?= LT -? LT)
///  | (?=WS|\z) | (?<=WP) (?=-{2,}\w) ) )` with `WP = [\w!"'&.,?]`, `LT = [^\d\W]`.
/// At every position one alternative matches (whitespace: the first; anything else: the
/// third, at the end of the non-whitespace run at the latest), so `split` yields exactly the
/// consecutive matches. Each is found as `sre` would: alternatives in order, the lazy
/// `NWS+?` tried with 1, 2, ... characters, lookarounds on the whole string. Tails (a) and
/// (c) need a `-` at the candidate end and (b) whitespace or the end, so the candidates
/// between are skipped in one scan (they cannot match).
pub fn split_words_into(text: &[u32], cls: &impl Classes, out: &mut Vec<Chunk>) {
    out.clear();
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
    let mut p = 0;
    while p < n {
        if is_ws(text[p]) {
            let q = p + 1 + text[p + 1..].iter().position(|&c| !is_ws(c)).unwrap_or(n - p - 1);
            out.push(Chunk::Text(p, q));
            p = q;
            continue;
        }
        // Alternative 2: em-dash between words (`-{2,}` starts here).
        if text[p] == 0x2d && p > 0 && wp(at(p - 1)) && dashes_word(p) {
            let q = p + run(p);
            out.push(Chunk::Text(p, q));
            p = q;
            continue;
        }
        // Alternative 3: lazy non-whitespace, then one of three tails.
        let mut q = p + 1;
        let end = loop {
            // text[p..q] is non-whitespace (q - p >= 1). No tail can match at a q whose
            // character is neither '-' nor whitespace: skip to the next candidate.
            q += text[q..].iter().position(|&c| c == 0x2d || is_ws(c)).unwrap_or(n - q);
            // (b) end of word.
            if q == n || is_ws(text[q]) {
                break q;
            }
            // text[q] == '-': (a) hyphenated word, lookbehinds at q + 1, lookahead after it.
            let behind = (q >= 2 && letter(at(q - 2)) && letter(at(q - 1)))
                || (q >= 3 && letter(at(q - 3)) && hyph(q - 2) && letter(at(q - 1)));
            let ahead = letter(at(q + 1)) && (letter(at(q + 2)) || (hyph(q + 2) && letter(at(q + 3))));
            if behind && ahead {
                break q + 1;
            }
            // (c) em-dash follows.
            if wp(at(q - 1)) && dashes_word(q) {
                break q;
            }
            q += 1;
        };
        out.push(Chunk::Text(p, end));
        p = end;
    }
}

/// [`split_words_into`] into a new `Vec`.
pub fn split_words(text: &[u32], cls: &impl Classes) -> Vec<Chunk> {
    let mut out = Vec::new();
    split_words_into(text, cls, &mut out);
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
        (n >= 2 && lower(c[n - 2]) && punct(c[n - 1]))
            || (n >= 3 && lower(c[n - 3]) && punct(c[n - 2]) && quote(c[n - 1]))
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

/// Where a [`Seg`] points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    /// The munged text.
    Text,
    /// The two spaces `_fix_sentence_endings` substitutes.
    Two,
    Initial,
    Subsequent,
    Placeholder,
}

/// A piece of an output line: `src[a..b]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seg {
    pub src: Src,
    pub a: usize,
    pub b: usize,
}

impl Seg {
    #[inline]
    pub fn len(self) -> usize {
        self.b - self.a
    }
    #[inline]
    pub fn is_empty(self) -> bool {
        self.a == self.b
    }
    /// The code points of this piece.
    #[inline]
    pub fn get<'a>(self, text: &'a [u32], o: &WrapOpts<'a>) -> &'a [u32] {
        const TWO: &[u32] = &[0x20, 0x20];
        let s: &'a [u32] = match self.src {
            Src::Text => text,
            Src::Two => TWO,
            Src::Initial => o.initial_indent,
            Src::Subsequent => o.subsequent_indent,
            Src::Placeholder => o.placeholder,
        };
        &s[self.a..self.b]
    }
}

impl From<Chunk> for Seg {
    #[inline]
    fn from(c: Chunk) -> Seg {
        match c {
            Chunk::Text(a, b) => Seg { src: Src::Text, a, b },
            Chunk::TwoSpaces => Seg { src: Src::Two, a: 0, b: 2 },
        }
    }
}

/// The wrapped lines, as pieces: line `i` is `segs[lines[i - 1]..lines[i]]` (`lines[-1]`
/// = 0). The buffers are reused between calls.
#[derive(Debug, Default)]
pub struct Wrapped {
    pub segs: Vec<Seg>,
    pub lines: Vec<usize>,
    /// Set when a line is CPython's placeholder object itself or a one-character `str` that
    /// CPython builds without its Latin-1 singleton (a `max_lines` cut with an empty
    /// placeholder, `'' + placeholder`, `'' + placeholder.lstrip()` with nothing to strip):
    /// identity a piece list cannot express; the caller lets CPython run.
    pub placeholder_identity: bool,
    rev: Vec<Seg>,
    cur: Vec<Seg>,
}

impl Wrapped {
    pub const fn new() -> Wrapped {
        Wrapped { segs: Vec::new(), lines: Vec::new(), placeholder_identity: false, rev: Vec::new(), cur: Vec::new() }
    }
    /// The pieces of line `i`.
    #[inline]
    pub fn line(&self, i: usize) -> &[Seg] {
        let a = if i == 0 { 0 } else { self.lines[i - 1] };
        &self.segs[a..self.lines[i]]
    }
    /// Number of lines.
    #[inline]
    pub fn len(&self) -> usize {
        self.lines.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
    /// Release capacity above `keep` elements per buffer.
    pub fn trim(&mut self, keep: usize) {
        for v in [&mut self.segs, &mut self.rev, &mut self.cur] {
            v.clear();
            if v.capacity() > keep {
                v.shrink_to(keep);
            }
        }
        self.lines.clear();
        if self.lines.capacity() > keep {
            self.lines.shrink_to(keep);
        }
    }
    /// Line `i` as code points (tests, diagnostics).
    pub fn line_cps(&self, i: usize, text: &[u32], o: &WrapOpts<'_>) -> Vec<u32> {
        self.line(i).iter().flat_map(|s| s.get(text, o).iter().copied()).collect()
    }
}

fn is_blank(s: &[u32], cls: &impl Classes) -> bool {
    s.iter().all(|&c| cls.is_space(c))
}

fn lstrip<'s>(s: &'s [u32], cls: &impl Classes) -> &'s [u32] {
    let mut b = 0;
    while b < s.len() && cls.is_space(s[b]) {
        b += 1;
    }
    &s[b..]
}

/// Append `cur` to `segs`, merging pieces of the text that are contiguous (consecutive
/// chunks are adjacent ranges of the munged text), so a line is usually two pieces.
#[inline]
fn push_merged(segs: &mut Vec<Seg>, cur: &[Seg]) {
    let mut prev: Option<Seg> = None;
    for &s in cur {
        match prev {
            Some(p) if p.src == Src::Text && s.src == Src::Text && p.b == s.a => prev = Some(Seg { b: s.b, ..p }),
            Some(p) => {
                segs.push(p);
                prev = Some(s);
            }
            None => prev = Some(s),
        }
    }
    segs.extend(prev);
}

/// `TextWrapper._wrap_chunks(chunks)`: the lines, as pieces, in `w` (cleared first).
pub fn wrap_chunks_into(
    text: &[u32],
    chunks: &[Chunk],
    o: &WrapOpts<'_>,
    cls: &impl Classes,
    w: &mut Wrapped,
) -> Result<(), WrapError> {
    w.segs.clear();
    w.lines.clear();
    w.cur.clear();
    w.rev.clear();
    w.placeholder_identity = false;
    if o.width <= 0 {
        return Err(WrapError::InvalidWidth);
    }
    if let Some(m) = o.max_lines {
        let indent = if m > 1 { o.subsequent_indent } else { o.initial_indent };
        if (indent.len() + lstrip(o.placeholder, cls).len()) as i64 > o.width {
            return Err(WrapError::PlaceholderTooLarge);
        }
    }
    let ph_len = o.placeholder.len();
    let ph_lstrip = ph_len - lstrip(o.placeholder, cls).len();
    // chunks.reverse(): the next chunk is the last element.
    w.rev.extend(chunks.iter().rev().map(|&c| Seg::from(c)));
    let blank = |s: Seg| is_blank(s.get(text, o), cls);
    let mut stalls = 0;
    while !w.rev.is_empty() {
        let before = (w.rev.len(), w.rev.last().copied(), w.lines.len());
        w.cur.clear();
        let mut cur_len: i64 = 0;
        let first = w.lines.is_empty();
        let indent = if first {
            Seg { src: Src::Initial, a: 0, b: o.initial_indent.len() }
        } else {
            Seg { src: Src::Subsequent, a: 0, b: o.subsequent_indent.len() }
        };
        let width = o.width - indent.len() as i64;
        if o.drop_whitespace
            && !first
            && let Some(&last) = w.rev.last()
            && blank(last)
        {
            w.rev.pop();
        }
        while let Some(&s) = w.rev.last() {
            let l = s.len() as i64;
            if cur_len + l <= width {
                w.cur.push(s);
                w.rev.pop();
                cur_len += l;
            } else {
                break;
            }
        }
        if let Some(&s) = w.rev.last()
            && s.len() as i64 > width
        {
            // _handle_long_word
            let space_left = if width < 1 { 1 } else { width - cur_len };
            if o.break_long_words && space_left > 0 {
                let mut end = space_left as usize;
                let chunk = s.get(text, o);
                if o.break_on_hyphens && chunk.len() > end {
                    // hyphen = chunk.rfind('-', 0, space_left)
                    if let Some(h) = chunk[..end].iter().rposition(|&x| x == 0x2d)
                        && h > 0
                        && chunk[..h].iter().any(|&x| x != 0x2d)
                    {
                        end = h + 1;
                    }
                }
                let end = end.min(s.len());
                w.cur.push(Seg { b: s.a + end, ..s });
                let last = w.rev.len() - 1;
                w.rev[last].a = s.a + end;
            } else if w.cur.is_empty() {
                let s = w.rev.pop().expect("non-empty");
                w.cur.push(s);
            }
            cur_len = w.cur.iter().map(|p| p.len() as i64).sum();
        }
        if o.drop_whitespace
            && let Some(&p) = w.cur.last()
            && blank(p)
        {
            cur_len -= p.len() as i64;
            w.cur.pop();
        }
        if !w.cur.is_empty() {
            let fits = match o.max_lines {
                None => true,
                Some(m) => {
                    (w.lines.len() as i64) + 1 < m
                        || ((w.rev.is_empty() || (o.drop_whitespace && w.rev.len() == 1 && blank(w.rev[0])))
                            && cur_len <= width)
                }
            };
            if fits {
                w.segs.push(indent);
                push_merged(&mut w.segs, &w.cur);
                w.lines.push(w.segs.len());
            } else {
                // `''.join(cur_line + [placeholder])` with an empty placeholder can be one
                // fresh character; `prev_line + ''` is `prev_line` (an object of its own).
                w.placeholder_identity |= ph_len == 0;
                let mut placed = false;
                while let Some(&p) = w.cur.last() {
                    if !blank(p) && cur_len + ph_len as i64 <= width {
                        w.segs.push(indent);
                        push_merged(&mut w.segs, &w.cur);
                        w.segs.push(Seg { src: Src::Placeholder, a: 0, b: ph_len });
                        w.lines.push(w.segs.len());
                        placed = true;
                        break;
                    }
                    cur_len -= p.len() as i64;
                    w.cur.pop();
                }
                if !placed {
                    if let Some(&end) = w.lines.last() {
                        // prev_line = lines[-1].rstrip()
                        let start = if w.lines.len() >= 2 { w.lines[w.lines.len() - 2] } else { 0 };
                        let mut keep = end; // segs[start..keep] survive, the last trimmed to `cut`
                        let mut cut = 0;
                        while keep > start {
                            let s = w.segs[keep - 1];
                            let t = s.get(text, o);
                            let mut e = t.len();
                            while e > 0 && cls.is_space(t[e - 1]) {
                                e -= 1;
                            }
                            if e > 0 {
                                cut = s.a + e;
                                break;
                            }
                            keep -= 1;
                        }
                        let prev_len: usize =
                            w.segs[start..keep].iter().map(|s| s.len()).sum::<usize>() - (if keep > start {
                                w.segs[keep - 1].b - cut
                            } else {
                                0
                            });
                        if (prev_len + ph_len) as i64 <= o.width {
                            // `'' + placeholder` is the placeholder object
                            w.placeholder_identity |= prev_len == 0;
                            w.segs.truncate(keep);
                            if keep > start {
                                w.segs[keep - 1].b = cut;
                            }
                            w.segs.push(Seg { src: Src::Placeholder, a: 0, b: ph_len });
                            let last = w.lines.len() - 1;
                            w.lines[last] = w.segs.len();
                            break;
                        }
                    }
                    // `'' + placeholder.lstrip()` with nothing stripped: the placeholder object
                    w.placeholder_identity |= indent.is_empty() && ph_lstrip == 0;
                    w.segs.push(indent);
                    w.segs.push(Seg { src: Src::Placeholder, a: ph_lstrip, b: ph_len });
                    w.lines.push(w.segs.len());
                }
                break;
            }
        }
        if w.rev.len() == before.0 && w.rev.last().copied() == before.1 {
            // Nothing consumed: the next iteration starts from the same chunks.
            if w.lines.len() == before.2 {
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
    Ok(())
}

/// `TextWrapper._wrap_chunks(chunks)`: the lines as code points (allocating; tests and
/// callers that want owned lines).
pub fn wrap_chunks(
    text: &[u32],
    chunks: Vec<Chunk>,
    o: &WrapOpts<'_>,
    cls: &impl Classes,
) -> Result<Vec<Vec<u32>>, WrapError> {
    let mut w = Wrapped::new();
    wrap_chunks_into(text, &chunks, o, cls, &mut w)?;
    Ok((0..w.len()).map(|i| w.line_cps(i, text, o)).collect())
}

/// `textwrap.dedent(text)` as ranges of `text`: the kept part of each line, in order (to be
/// joined with `\n`; blank lines give empty ranges), in `out` (cleared first). `false` when
/// the result is `text` itself (CPython returns the same object: one line, nothing
/// removed) and `out` is unused.
pub fn dedent_ranges<T: Copy + Ord + Into<u32>>(text: &[T], cls: &impl Classes, out: &mut Vec<(usize, usize)>) -> bool {
    out.clear();
    let nl = |c: &T| (*c).into() == 0x0a;
    let blank = |l: &[T]| !l.is_empty() && l.iter().all(|&c| cls.is_space(c.into()));
    let mut l1: Option<&[T]> = None;
    let mut l2: Option<&[T]> = None;
    // One pass: line ranges (with a blank flag) and the min/max non-blank lines.
    let mut start = 0;
    for l in text.split(nl) {
        let b = blank(l);
        out.push((start, if b { start } else { start + l.len() }));
        start += l.len() + 1;
        if l.is_empty() || b {
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
            let cu: u32 = c.into();
            if l2.get(i) != Some(&c) || !(cu == 0x20 || cu == 0x09) {
                break;
            }
        }
    }
    if out.len() == 1 && margin == 0 && !blank(text) {
        return false;
    }
    for r in out.iter_mut() {
        if r.1 > r.0 {
            r.0 += margin.min(r.1 - r.0);
        }
    }
    true
}

/// `textwrap.dedent(text)`: `None` when the result is `text` itself.
pub fn dedent(text: &[u32], cls: &impl Classes) -> Option<Vec<u32>> {
    let mut r = Vec::new();
    if !dedent_ranges(text, cls, &mut r) {
        return None;
    }
    let mut out = Vec::new();
    for (i, &(a, b)) in r.iter().enumerate() {
        if i > 0 {
            out.push(0x0a);
        }
        out.extend_from_slice(&text[a..b]);
    }
    Some(out)
}

/// `str.splitlines(True)` boundaries: `\n \r \r\n \x0b \x0c \x1c \x1d \x1e \x85    `.
#[inline]
pub fn is_line_break(c: u32) -> bool {
    if c > 0x1e { c == 0x85 || c == 0x2028 || c == 0x2029 } else { c >= 0x0a && (c <= 0x0d || c >= 0x1c) }
}

/// The lines of `str.splitlines(True)` as ranges, one at a time.
pub fn lines_keepends<T: Copy + Into<u32>>(text: &[T]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let n = text.len();
    let mut i = 0;
    std::iter::from_fn(move || {
        if i >= n {
            return None;
        }
        let start = i;
        while i < n && !is_line_break(text[i].into()) {
            i += 1;
        }
        if i < n {
            if text[i].into() == 0x0d && i + 1 < n && text[i + 1].into() == 0x0a {
                i += 2;
            } else {
                i += 1;
            }
        }
        Some((start, i))
    })
}

/// The lines of `str.splitlines(True)` as ranges.
pub fn splitlines_keepends(text: &[u32]) -> Vec<(usize, usize)> {
    lines_keepends(text).collect()
}

/// `textwrap.indent(text, prefix)` (no predicate) as the lines of `str.splitlines(True)`
/// with a flag "prefixed" (`not line.isspace()`), in `out` (cleared first). `false` when
/// CPython returns `text` itself (a single line that is all whitespace: nothing to prefix,
/// `''.join([text])`) and `out` is unused.
pub fn indent_ranges<T: Copy + Into<u32>>(text: &[T], cls: &impl Classes, out: &mut Vec<(usize, usize, bool)>) -> bool {
    out.clear();
    let space = |l: &[T]| !l.is_empty() && l.iter().all(|&c| cls.is_space(c.into()));
    for (a, b) in lines_keepends(text) {
        out.push((a, b, !space(&text[a..b])));
    }
    !(out.len() == 1 && !out[0].2)
}

/// `textwrap.indent(text, prefix)` (no predicate): `None` when CPython returns `text`.
pub fn indent(text: &[u32], prefix: &[u32], cls: &impl Classes) -> Option<Vec<u32>> {
    let mut r = Vec::new();
    if !indent_ranges(text, cls, &mut r) {
        return None;
    }
    let mut out = Vec::new();
    for (a, b, p) in r {
        if p {
            out.extend_from_slice(prefix);
        }
        out.extend_from_slice(&text[a..b]);
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
            [
                "Hello", " ", "there", " ", "--", " ", "you", " ", "goof-", "ball,", " ", "use", " ", "the", " ", "-b",
                " ", "option!"
            ]
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
