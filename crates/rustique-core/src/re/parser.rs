//! Port of `re._parser.parse` (CPython 3.14 / 3.15): the pattern to a tree of
//! sub-patterns, exactly as CPython builds it, for every pattern that CPython parses without
//! raising, warning or importing (`\N{...}` imports `unicodedata`). Anything else returns
//! [`Decline`] and CPython's parser runs.
//!
//! The tree mirrors CPython's objects: a [`Sub`] is a `SubPattern` (arena-allocated, so that
//! "same object" stays expressible), a [`Node`] is one `(op, av)` item, a [`SetItem`] one
//! member of an `IN` set.

use super::consts::*;
use super::{Decline, Env};

/// Index of a [`Sub`] in [`Tree::subs`].
pub(crate) type SubId = u32;

/// One `(op, av)` item of a `SubPattern`.
#[derive(Clone, Debug)]
pub(crate) enum Node {
    Literal(u32),
    NotLiteral(u32),
    Any,
    At(u32),
    In(Vec<SetItem>),
    /// `(BRANCH, (None, items))`
    Branch(Vec<SubId>),
    /// `(SUBPATTERN, (group, add_flags, del_flags, p))`
    Subpattern {
        group: Option<u32>,
        add: u32,
        del: u32,
        p: SubId,
    },
    /// `(MIN_REPEAT|MAX_REPEAT|POSSESSIVE_REPEAT, (min, max, item))`
    Repeat {
        op: u32,
        min: u32,
        max: u32,
        item: SubId,
    },
    GroupRef(u32),
    /// `(GROUPREF_EXISTS, (condgroup, item_yes, item_no))`
    GroupRefExists {
        cond: u32,
        yes: SubId,
        no: Option<SubId>,
    },
    /// `(ASSERT, (dir, p))`
    Assert {
        dir: i8,
        p: SubId,
    },
    /// `(ASSERT_NOT, (dir, p))`
    AssertNot {
        dir: i8,
        p: SubId,
    },
    AtomicGroup(SubId),
    /// `(FAILURE, ())`
    Failure,
}

impl Node {
    /// `a == b` on CPython's tuples: ints compare by value, `SubPattern`s by identity (they
    /// define no `__eq__`). Two items taken from different alternatives never share a
    /// `SubPattern`, so any item holding one compares unequal.
    fn same(&self, other: &Node) -> bool {
        match (self, other) {
            (Node::Literal(a), Node::Literal(b))
            | (Node::NotLiteral(a), Node::NotLiteral(b))
            | (Node::At(a), Node::At(b))
            | (Node::GroupRef(a), Node::GroupRef(b)) => a == b,
            (Node::Any, Node::Any) | (Node::Failure, Node::Failure) => true,
            (Node::In(a), Node::In(b)) => a == b,
            _ => false,
        }
    }
}

/// One `(op, av)` member of an `IN` set (parser output and optimizer output).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SetItem {
    Negate,
    Literal(u32),
    Range(u32, u32),
    /// A raw `CATEGORY_*` code (mapped to its UNICODE/LOCALE variant when emitted).
    Category(u32),
    /// Optimizer output only.
    RangeUniIgnore(u32, u32),
    Charset(Vec<u32>),
    BigCharset(Vec<u32>),
}

/// A `SubPattern`: its items and the cached `getwidth()`.
#[derive(Debug, Default)]
pub(crate) struct Sub {
    pub data: Vec<Node>,
    width: Option<(u128, u128)>,
}

/// `_parser.State` (minus the fields only errors read).
#[derive(Debug)]
pub(crate) struct State {
    pub flags: u32,
    /// `groupdict`, in insertion order: (name, gid). Names are ASCII identifiers.
    pub groupdict: Vec<(String, u32)>,
    /// `groupwidths` (index 0 is the whole pattern)
    pub groupwidths: Vec<Option<(u128, u128)>>,
    lookbehindgroups: Option<u32>,
    /// Keys of `grouprefpos` (numeric conditional references).
    grouprefpos: Vec<u32>,
}

impl State {
    /// `State.groups`
    #[inline]
    pub fn groups(&self) -> u32 {
        self.groupwidths.len() as u32
    }
    fn checkgroup(&self, gid: u32) -> bool {
        gid < self.groups() && self.groupwidths[gid as usize].is_some()
    }
    fn checklookbehindgroup(&self, gid: u32) -> Result<(), Decline> {
        if let Some(lb) = self.lookbehindgroups {
            if !self.checkgroup(gid) {
                return Err(Decline::Error);
            }
            if gid >= lb {
                return Err(Decline::Error);
            }
        }
        Ok(())
    }
}

/// The parse result: `p` (sub-pattern 0 is never used as `p`; see [`Tree::root`]).
#[derive(Debug)]
pub(crate) struct Tree {
    pub subs: Vec<Sub>,
    pub state: State,
    pub root: SubId,
    /// `_sre.MAXREPEAT` (`getwidth` compares `av[1] == MAXREPEAT`)
    maxrepeat: u32,
}

/// `MAXWIDTH` (`1 << 64`): `getwidth()` caps at it.
pub(crate) const MAXWIDTH: u128 = 1 << 64;

/// Deepest `nested` level the port accepts (two per group level: `_parse_sub` and `_parse`).
/// CPython's parser and compiler recurse with Python frames; deeper patterns are left to it
/// so that its recursion limit applies (and the port's own recursion stays shallow).
const MAX_NESTING: u32 = 64;

impl Tree {
    fn new_sub(&mut self) -> SubId {
        self.subs.push(Sub::default());
        (self.subs.len() - 1) as SubId
    }

    pub fn sub(&self, id: SubId) -> &Sub {
        &self.subs[id as usize]
    }

    /// `SubPattern.getwidth()`, cached like CPython's (`self.width`).
    pub fn getwidth(&mut self, id: SubId) -> (u128, u128) {
        if let Some(w) = self.subs[id as usize].width {
            return w;
        }
        let mut lo: u128 = 0;
        let mut hi: u128 = 0;
        let n = self.subs[id as usize].data.len();
        for k in 0..n {
            // Items are cloned out (ids only) so nested `getwidth` can borrow the arena.
            let node = self.subs[id as usize].data[k].clone();
            match node {
                Node::Branch(items) => {
                    let mut i = MAXWIDTH;
                    let mut j = 0;
                    for av in items {
                        let (l, h) = self.getwidth(av);
                        i = i.min(l);
                        j = j.max(h);
                    }
                    lo = lo.saturating_add(i);
                    hi = hi.saturating_add(j);
                }
                Node::AtomicGroup(p) => {
                    let (i, j) = self.getwidth(p);
                    lo = lo.saturating_add(i);
                    hi = hi.saturating_add(j);
                }
                Node::Subpattern { p, .. } => {
                    let (i, j) = self.getwidth(p);
                    lo = lo.saturating_add(i);
                    hi = hi.saturating_add(j);
                }
                Node::Repeat { min, max, item, .. } => {
                    let (i, j) = self.getwidth(item);
                    lo = lo.saturating_add(i.saturating_mul(min as u128));
                    if max == self.maxrepeat && j != 0 {
                        hi = MAXWIDTH;
                    } else {
                        hi = hi.saturating_add(j.saturating_mul(max as u128));
                    }
                }
                Node::Any | Node::In(_) | Node::Literal(_) | Node::NotLiteral(_) => {
                    lo = lo.saturating_add(1);
                    hi = hi.saturating_add(1);
                }
                Node::GroupRef(g) => {
                    // A reference is only parsed once its group is closed.
                    let (i, j) = self.state.groupwidths[g as usize].unwrap_or((0, 0));
                    lo = lo.saturating_add(i);
                    hi = hi.saturating_add(j);
                }
                Node::GroupRefExists { yes, no, .. } => {
                    let (mut i, mut j) = self.getwidth(yes);
                    if let Some(no) = no {
                        let (l, h) = self.getwidth(no);
                        i = i.min(l);
                        j = j.max(h);
                    } else {
                        i = 0;
                    }
                    lo = lo.saturating_add(i);
                    hi = hi.saturating_add(j);
                }
                Node::At(_) | Node::Assert { .. } | Node::AssertNot { .. } | Node::Failure => {}
            }
        }
        let w = (lo.min(MAXWIDTH), hi.min(MAXWIDTH));
        self.subs[id as usize].width = Some(w);
        w
    }
}

/// A token: `Tokenizer.next` (`None`, one char, or a backslash escape of two chars).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    End,
    Ch(u32),
    Esc(u32),
}

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
fn is_hexdigit(c: u32) -> bool {
    is_digit(c) || (b'a' as u32..=b'f' as u32).contains(&c) || (b'A' as u32..=b'F' as u32).contains(&c)
}
#[inline]
fn is_asciiletter(c: u32) -> bool {
    (b'a' as u32..=b'z' as u32).contains(&c) || (b'A' as u32..=b'Z' as u32).contains(&c)
}
/// `_parser.WHITESPACE`
#[inline]
fn is_whitespace(c: u32) -> bool {
    matches!(c, 0x20 | 0x09 | 0x0a | 0x0d | 0x0b | 0x0c)
}
#[inline]
fn ch(c: char) -> u32 {
    c as u32
}

/// `_parser.Tokenizer` over the code units of a `str` (or the latin-1 decoding of `bytes`).
struct Tokenizer<'a> {
    s: &'a [u32],
    istext: bool,
    index: usize,
    next: Tok,
}

impl<'a> Tokenizer<'a> {
    fn new(s: &'a [u32], istext: bool) -> Result<Self, Decline> {
        let mut t = Tokenizer { s, istext, index: 0, next: Tok::End };
        t.advance()?;
        Ok(t)
    }

    /// `__next`
    #[inline]
    fn advance(&mut self) -> Result<(), Decline> {
        let mut index = self.index;
        let Some(&c) = self.s.get(index) else {
            self.next = Tok::End;
            return Ok(());
        };
        if c == BACKSLASH {
            index += 1;
            let Some(&d) = self.s.get(index) else {
                return Err(Decline::Error); // bad escape (end of pattern)
            };
            self.next = Tok::Esc(d);
        } else {
            self.next = Tok::Ch(c);
        }
        self.index = index + 1;
        Ok(())
    }

    /// `match(char)` for a one-char string.
    #[inline]
    fn matches(&mut self, c: char) -> Result<bool, Decline> {
        if self.next == Tok::Ch(c as u32) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// `get()`
    #[inline]
    fn get(&mut self) -> Result<Tok, Decline> {
        let this = self.next;
        self.advance()?;
        Ok(this)
    }

    /// The single char of `next` if it is one (`source.next in <charset>` tests).
    #[inline]
    fn next_char(&self) -> Option<u32> {
        match self.next {
            Tok::Ch(c) => Some(c),
            _ => None,
        }
    }

    /// `getwhile(n, charset)`: the chars appended to a growing escape.
    fn getwhile(&mut self, n: usize, pred: fn(u32) -> bool, out: &mut Vec<u32>) -> Result<(), Decline> {
        for _ in 0..n {
            match self.next_char() {
                Some(c) if pred(c) => {
                    out.push(c);
                    self.advance()?;
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// `getuntil(terminator, name)`: the text up to `terminator` (escapes keep both chars).
    fn getuntil(&mut self, terminator: char) -> Result<Vec<u32>, Decline> {
        let mut result = Vec::new();
        loop {
            let c = self.next;
            self.advance()?;
            match c {
                Tok::End => return Err(Decline::Error),
                Tok::Ch(x) if x == terminator as u32 => {
                    if result.is_empty() {
                        return Err(Decline::Error);
                    }
                    break;
                }
                Tok::Ch(x) => result.push(x),
                Tok::Esc(x) => {
                    result.push(BACKSLASH);
                    result.push(x);
                }
            }
        }
        Ok(result)
    }

    /// `tell()`
    #[inline]
    fn tell(&self) -> usize {
        self.index
            - match self.next {
                Tok::End => 0,
                Tok::Ch(_) => 1,
                Tok::Esc(_) => 2,
            }
    }

    /// `seek(index)`
    fn seek(&mut self, index: usize) -> Result<(), Decline> {
        self.index = index;
        self.advance()
    }
}

/// `Tokenizer.checkgroupname`, restricted to ASCII identifiers (non-ASCII names, valid or
/// not, are left to CPython: `str.isidentifier` follows the running Unicode database).
fn group_name(name: &[u32]) -> Result<String, Decline> {
    let ok = !name.is_empty()
        && name.iter().all(|&c| c < 128)
        && (is_asciiletter(name[0]) || name[0] == '_' as u32)
        && name.iter().all(|&c| is_asciiletter(c) || is_digit(c) || c == '_' as u32);
    if ok {
        Ok(name.iter().map(|&c| c as u8 as char).collect())
    } else if name.iter().all(|&c| c < 128) {
        Err(Decline::Error)
    } else {
        Err(Decline::Unsupported)
    }
}

/// `str.isdecimal() and str.isascii()` on a group name (`(?(1)...)`, `\g<1>`).
pub(crate) fn ascii_decimal(name: &[u32]) -> bool {
    !name.is_empty() && name.iter().all(|&c| is_digit(c))
}

/// `int(digits)` saturating at `u64::MAX` (only compared against limits below 2**32).
pub(crate) fn int_of(digits: &[u32]) -> u64 {
    let mut v: u64 = 0;
    for &d in digits {
        v = v.saturating_mul(10).saturating_add((d - '0' as u32) as u64);
    }
    v
}

fn hex_value(digits: &[u32]) -> u32 {
    digits.iter().fold(0u32, |v, &d| {
        let x = char::from_u32(d).and_then(|c| c.to_digit(16)).unwrap_or(0);
        v.wrapping_mul(16).wrapping_add(x)
    })
}

fn oct_value(digits: &[u32]) -> u32 {
    digits.iter().fold(0u32, |v, &d| v * 8 + (d - '0' as u32))
}

/// What an escape stands for (`_escape` / `_class_escape` return values).
enum Esc {
    Lit(u32),
    /// `(IN, [(CATEGORY, code)])`
    Cat(u32),
    /// `(AT, code)`
    At(u32),
    GroupRef(u32),
}

/// `CATEGORIES.get(escape)`
fn categories(c: u32) -> Option<Esc> {
    Some(match char::from_u32(c)? {
        'A' => Esc::At(AT_BEGINNING_STRING),
        'b' => Esc::At(AT_BOUNDARY),
        'B' => Esc::At(AT_NON_BOUNDARY),
        'd' => Esc::Cat(CATEGORY_DIGIT),
        'D' => Esc::Cat(CATEGORY_NOT_DIGIT),
        's' => Esc::Cat(CATEGORY_SPACE),
        'S' => Esc::Cat(CATEGORY_NOT_SPACE),
        'w' => Esc::Cat(CATEGORY_WORD),
        'W' => Esc::Cat(CATEGORY_NOT_WORD),
        'z' | 'Z' => Esc::At(AT_END_STRING),
        _ => return None,
    })
}

/// `ESCAPES.get(escape)`
fn escapes(c: u32) -> Option<u32> {
    Some(match char::from_u32(c)? {
        'a' => 0x07,
        'b' => 0x08,
        'f' => 0x0c,
        'n' => 0x0a,
        'r' => 0x0d,
        't' => 0x09,
        'v' => 0x0b,
        '\\' => 0x5c,
        _ => return None,
    })
}

struct Parser<'a, 'e> {
    src: Tokenizer<'a>,
    tree: Tree,
    env: &'e Env<'e>,
}

impl<'a, 'e> Parser<'a, 'e> {
    /// `\x`, `\u`, `\U` (shared by both escape functions): the value, or `None` if `c` is not
    /// one of them for this pattern type.
    fn hex_escape(&mut self, c: u32) -> Result<Option<u32>, Decline> {
        let n = if c == ch('x') {
            2
        } else if c == ch('u') && self.src.istext {
            4
        } else if c == ch('U') && self.src.istext {
            8
        } else {
            return Ok(None);
        };
        let mut digits = Vec::with_capacity(n);
        self.src.getwhile(n, is_hexdigit, &mut digits)?;
        if digits.len() != n {
            return Err(Decline::Error); // incomplete escape
        }
        let v = hex_value(&digits);
        if n == 8 && v > 0x10ffff {
            return Err(Decline::Error); // chr() raises: bad escape
        }
        Ok(Some(v))
    }

    /// `_class_escape(source, escape)` for `escape = "\\" + c`.
    fn class_escape(&mut self, c: u32) -> Result<Esc, Decline> {
        if let Some(v) = escapes(c) {
            return Ok(Esc::Lit(v));
        }
        if let Some(Esc::Cat(k)) = categories(c) {
            return Ok(Esc::Cat(k));
        }
        if let Some(v) = self.hex_escape(c)? {
            return Ok(Esc::Lit(v));
        }
        if c == ch('N') && self.src.istext {
            return Err(Decline::Unsupported); // imports unicodedata
        }
        if is_octdigit(c) {
            let mut digits = vec![c];
            self.src.getwhile(2, is_octdigit, &mut digits)?;
            let v = oct_value(&digits);
            if v > 0o377 {
                return Err(Decline::Error);
            }
            return Ok(Esc::Lit(v));
        }
        if is_digit(c) {
            return Err(Decline::Error);
        }
        if is_asciiletter(c) {
            return Err(Decline::Error);
        }
        Ok(Esc::Lit(c))
    }

    /// `_escape(source, escape, state)` for `escape = "\\" + c`.
    fn escape(&mut self, c: u32) -> Result<Esc, Decline> {
        if let Some(e) = categories(c) {
            return Ok(e);
        }
        if let Some(v) = escapes(c) {
            return Ok(Esc::Lit(v));
        }
        if let Some(v) = self.hex_escape(c)? {
            return Ok(Esc::Lit(v));
        }
        if c == ch('N') && self.src.istext {
            return Err(Decline::Unsupported); // imports unicodedata
        }
        if c == ch('0') {
            let mut digits = vec![c];
            self.src.getwhile(2, is_octdigit, &mut digits)?;
            return Ok(Esc::Lit(oct_value(&digits)));
        }
        if is_digit(c) {
            let mut digits = vec![c];
            if let Some(d) = self.src.next_char()
                && is_digit(d)
            {
                self.src.get()?;
                digits.push(d);
                if is_octdigit(digits[0])
                    && is_octdigit(digits[1])
                    && let Some(e) = self.src.next_char()
                    && is_octdigit(e)
                {
                    self.src.get()?;
                    digits.push(e);
                    let v = oct_value(&digits);
                    if v > 0o377 {
                        return Err(Decline::Error);
                    }
                    return Ok(Esc::Lit(v));
                }
            }
            let group = int_of(&digits);
            let state = &self.tree.state;
            if group < state.groups() as u64 {
                let group = group as u32;
                if !state.checkgroup(group) {
                    return Err(Decline::Error);
                }
                state.checklookbehindgroup(group)?;
                return Ok(Esc::GroupRef(group));
            }
            return Err(Decline::Error); // invalid group reference
        }
        if is_asciiletter(c) {
            return Err(Decline::Error);
        }
        Ok(Esc::Lit(c))
    }

    /// `_parse_sub(source, state, verbose, nested)`
    fn parse_sub(&mut self, mut verbose: bool, nested: u32) -> Result<SubId, Decline> {
        if nested > MAX_NESTING {
            return Err(Decline::Unsupported);
        }
        let mut items: Vec<SubId> = Vec::new();
        loop {
            let first = nested == 0 && items.is_empty();
            let item = self.parse(verbose, nested + 1, first)?;
            items.push(item);
            if !self.src.matches('|')? {
                break;
            }
            if nested == 0 {
                verbose = self.tree.state.flags & SRE_FLAG_VERBOSE != 0;
            }
        }
        if items.len() == 1 {
            return Ok(items[0]);
        }
        let sub = self.tree.new_sub();
        // check if all items share a common prefix
        loop {
            let mut prefix: Option<Node> = None;
            let mut all = true;
            for &item in &items {
                let data = &self.tree.subs[item as usize].data;
                let Some(first) = data.first() else {
                    all = false;
                    break;
                };
                match &prefix {
                    None => prefix = Some(first.clone()),
                    Some(p) => {
                        if !first.same(p) {
                            all = false;
                            break;
                        }
                    }
                }
            }
            if !all {
                break;
            }
            for &item in &items {
                self.tree.subs[item as usize].data.remove(0);
            }
            self.tree.subs[sub as usize].data.push(prefix.expect("items is not empty"));
        }
        // check if the branch can be replaced by a character set
        let mut set: Vec<SetItem> = Vec::new();
        let mut all = true;
        for &item in &items {
            let data = &self.tree.subs[item as usize].data;
            if data.len() != 1 {
                all = false;
                break;
            }
            match &data[0] {
                Node::Literal(v) => set.push(SetItem::Literal(*v)),
                Node::In(av) if av[0] != SetItem::Negate => set.extend(av.iter().cloned()),
                _ => {
                    all = false;
                    break;
                }
            }
        }
        if all {
            self.tree.subs[sub as usize].data.push(Node::In(uniq(set)));
            return Ok(sub);
        }
        self.tree.subs[sub as usize].data.push(Node::Branch(items));
        Ok(sub)
    }

    /// `_parse(source, state, verbose, nested, first)`
    fn parse(&mut self, mut verbose: bool, nested: u32, first: bool) -> Result<SubId, Decline> {
        let sp = self.tree.new_sub();
        loop {
            let this = self.src.next;
            match this {
                Tok::End => break,
                Tok::Ch(c) if c == ch('|') || c == ch(')') => break,
                _ => {}
            }
            self.src.get()?;
            if verbose && let Tok::Ch(c) = this {
                if is_whitespace(c) {
                    continue;
                }
                if c == ch('#') {
                    loop {
                        let t = self.src.get()?;
                        if t == Tok::End || t == Tok::Ch(ch('\n')) {
                            break;
                        }
                    }
                    continue;
                }
            }
            let c = match this {
                Tok::Esc(e) => {
                    let node = match self.escape(e)? {
                        Esc::Lit(v) => Node::Literal(v),
                        Esc::Cat(k) => Node::In(vec![SetItem::Category(k)]),
                        Esc::At(a) => Node::At(a),
                        Esc::GroupRef(g) => Node::GroupRef(g),
                    };
                    self.push(sp, node);
                    continue;
                }
                Tok::Ch(c) => c,
                Tok::End => unreachable!(),
            };
            match char::from_u32(c) {
                Some('[') => self.parse_set(sp)?,
                Some('*' | '+' | '?' | '{') => self.parse_repeat(sp, c)?,
                Some('.') => self.push(sp, Node::Any),
                Some('(') => {
                    if let Some(v) = self.parse_group(sp, verbose, nested, first)? {
                        verbose = v;
                    }
                }
                Some('^') => self.push(sp, Node::At(AT_BEGINNING)),
                Some('$') => self.push(sp, Node::At(AT_END)),
                // SPECIAL_CHARS also holds '\\' (always an escape token) and '|', ')' (end)
                _ => self.push(sp, Node::Literal(c)),
            }
        }
        // unpack non-capturing groups
        let unpack = self.tree.subs[sp as usize]
            .data
            .iter()
            .any(|n| matches!(n, Node::Subpattern { group: None, add: 0, del: 0, .. }));
        if !unpack {
            return Ok(sp);
        }
        let data = std::mem::take(&mut self.tree.subs[sp as usize].data);
        let mut out = Vec::with_capacity(data.len());
        for node in data {
            if let Node::Subpattern { group: None, add: 0, del: 0, p } = node {
                out.extend(self.tree.subs[p as usize].data.iter().cloned());
            } else {
                out.push(node);
            }
        }
        self.tree.subs[sp as usize].data = out;
        Ok(sp)
    }

    #[inline]
    fn push(&mut self, sp: SubId, node: Node) {
        self.tree.subs[sp as usize].data.push(node);
    }

    /// The `[` branch of `_parse` (after the `[`).
    fn parse_set(&mut self, sp: SubId) -> Result<(), Decline> {
        let mut set: Vec<SetItem> = Vec::new();
        if self.src.next == Tok::Ch(ch('[')) {
            return Err(Decline::Warning); // Possible nested set
        }
        let negate = self.src.matches('^')?;
        // A set member: (LITERAL, v) or (IN, [(CATEGORY, k)]) before unwrapping.
        enum Member {
            Lit(u32),
            Cat(u32),
        }
        loop {
            let this = self.src.get()?;
            let code1 = match this {
                Tok::End => return Err(Decline::Error), // unterminated character set
                Tok::Ch(c) if c == ch(']') && !set.is_empty() => break,
                Tok::Esc(e) => match self.class_escape(e)? {
                    Esc::Lit(v) => Member::Lit(v),
                    Esc::Cat(k) => Member::Cat(k),
                    _ => unreachable!("class escapes are literals or categories"),
                },
                Tok::Ch(c) => {
                    if !set.is_empty()
                        && matches!(char::from_u32(c), Some('-' | '&' | '~' | '|'))
                        && self.src.next == Tok::Ch(c)
                    {
                        return Err(Decline::Warning); // Possible set operation
                    }
                    Member::Lit(c)
                }
            };
            if self.src.matches('-')? {
                // potential range
                let that = self.src.get()?;
                let code2 = match that {
                    Tok::End => return Err(Decline::Error),
                    Tok::Ch(c) if c == ch(']') => {
                        set.push(match code1 {
                            Member::Lit(v) => SetItem::Literal(v),
                            Member::Cat(k) => SetItem::Category(k),
                        });
                        set.push(SetItem::Literal(ch('-')));
                        break;
                    }
                    Tok::Esc(e) => match self.class_escape(e)? {
                        Esc::Lit(v) => Member::Lit(v),
                        Esc::Cat(k) => Member::Cat(k),
                        _ => unreachable!("class escapes are literals or categories"),
                    },
                    Tok::Ch(c) => {
                        if c == ch('-') {
                            return Err(Decline::Warning); // Possible set difference
                        }
                        Member::Lit(c)
                    }
                };
                let (Member::Lit(lo), Member::Lit(hi)) = (code1, code2) else {
                    return Err(Decline::Error); // bad character range
                };
                if hi < lo {
                    return Err(Decline::Error);
                }
                set.push(SetItem::Range(lo, hi));
            } else {
                set.push(match code1 {
                    Member::Lit(v) => SetItem::Literal(v),
                    Member::Cat(k) => SetItem::Category(k),
                });
            }
        }
        let mut set = uniq(set);
        if set.len() == 1
            && let SetItem::Literal(v) = set[0]
        {
            self.push(sp, if negate { Node::NotLiteral(v) } else { Node::Literal(v) });
        } else {
            if negate {
                set.insert(0, SetItem::Negate);
            }
            self.push(sp, Node::In(set));
        }
        Ok(())
    }

    /// The repeat branch of `_parse` (after `*`, `+`, `?` or `{`).
    fn parse_repeat(&mut self, sp: SubId, this: u32) -> Result<(), Decline> {
        let maxrepeat = self.env.maxrepeat;
        let here = self.src.tell();
        let (min, max) = match char::from_u32(this) {
            Some('?') => (0, 1),
            Some('*') => (0, maxrepeat),
            Some('+') => (1, maxrepeat),
            _ => {
                // '{'
                if self.src.next == Tok::Ch(ch('}')) {
                    self.push(sp, Node::Literal(this));
                    return Ok(());
                }
                let (mut min, mut max) = (0, maxrepeat);
                let mut lo = Vec::new();
                let mut hi = Vec::new();
                while let Some(d) = self.src.next_char()
                    && is_digit(d)
                {
                    lo.push(d);
                    self.src.get()?;
                }
                if self.src.matches(',')? {
                    while let Some(d) = self.src.next_char()
                        && is_digit(d)
                    {
                        hi.push(d);
                        self.src.get()?;
                    }
                } else {
                    hi = lo.clone();
                }
                if !self.src.matches('}')? {
                    self.push(sp, Node::Literal(this));
                    self.src.seek(here)?;
                    return Ok(());
                }
                if !lo.is_empty() {
                    let v = int_of(&lo);
                    if v >= maxrepeat as u64 {
                        return Err(Decline::Error); // OverflowError
                    }
                    min = v as u32;
                }
                if !hi.is_empty() {
                    let v = int_of(&hi);
                    if v >= maxrepeat as u64 {
                        return Err(Decline::Error); // OverflowError
                    }
                    max = v as u32;
                    if max < min {
                        return Err(Decline::Error);
                    }
                }
                (min, max)
            }
        };
        // figure out which item to repeat
        let last = match self.tree.subs[sp as usize].data.last() {
            None => return Err(Decline::Error), // nothing to repeat
            Some(n) => n.clone(),
        };
        let item = match last {
            Node::At(_) => return Err(Decline::Error),         // nothing to repeat
            Node::Repeat { .. } => return Err(Decline::Error), // multiple repeat
            Node::Subpattern { group: None, add: 0, del: 0, p } => p,
            other => {
                // `subpattern[-1:]`: a new SubPattern holding the last item
                let id = self.tree.new_sub();
                self.tree.subs[id as usize].data.push(other);
                id
            }
        };
        let op = if self.src.matches('?')? {
            MIN_REPEAT
        } else if self.src.matches('+')? {
            POSSESSIVE_REPEAT
        } else {
            MAX_REPEAT
        };
        let data = &mut self.tree.subs[sp as usize].data;
        *data.last_mut().expect("checked above") = Node::Repeat { op, min, max, item };
        Ok(())
    }

    /// The `(` branch of `_parse` (after the `(`). Returns the new `verbose` after global
    /// flags (`Some`), or `None`.
    fn parse_group(&mut self, sp: SubId, verbose: bool, nested: u32, first: bool) -> Result<Option<bool>, Decline> {
        let mut capture = true;
        let mut atomic = false;
        let mut name: Option<String> = None;
        let mut add_flags = 0;
        let mut del_flags = 0;
        if self.src.matches('?')? {
            let c = match self.src.get()? {
                Tok::End => return Err(Decline::Error),    // unexpected end of pattern
                Tok::Esc(_) => return Err(Decline::Error), // unknown extension
                Tok::Ch(c) => c,
            };
            match char::from_u32(c) {
                Some('P') => {
                    if self.src.matches('<')? {
                        let n = self.src.getuntil('>')?;
                        name = Some(group_name(&n)?);
                    } else if self.src.matches('=')? {
                        let n = self.src.getuntil(')')?;
                        let n = group_name(&n)?;
                        let state = &self.tree.state;
                        let Some(&(_, gid)) = state.groupdict.iter().find(|(k, _)| *k == n) else {
                            return Err(Decline::Error); // unknown group name
                        };
                        if !state.checkgroup(gid) {
                            return Err(Decline::Error);
                        }
                        state.checklookbehindgroup(gid)?;
                        self.push(sp, Node::GroupRef(gid));
                        return Ok(None);
                    } else {
                        return Err(Decline::Error); // unknown extension ?P
                    }
                }
                Some(':') => capture = false,
                Some('#') => {
                    // comment
                    loop {
                        if self.src.next == Tok::End {
                            return Err(Decline::Error);
                        }
                        if self.src.get()? == Tok::Ch(ch(')')) {
                            break;
                        }
                    }
                    return Ok(None);
                }
                Some(k @ ('=' | '!' | '<')) => {
                    // lookahead / lookbehind assertions
                    let mut dir: i8 = 1;
                    let mut kind = k;
                    let mut lookbehindgroups = None;
                    if k == '<' {
                        kind = match self.src.get()? {
                            Tok::Ch(c) if c == ch('=') => '=',
                            Tok::Ch(c) if c == ch('!') => '!',
                            _ => return Err(Decline::Error),
                        };
                        dir = -1;
                        lookbehindgroups = Some(self.tree.state.lookbehindgroups);
                        if self.tree.state.lookbehindgroups.is_none() {
                            self.tree.state.lookbehindgroups = Some(self.tree.state.groups());
                        }
                    }
                    let p = self.parse_sub(verbose, nested + 1)?;
                    if dir < 0 && lookbehindgroups == Some(None) {
                        self.tree.state.lookbehindgroups = None;
                    }
                    if !self.src.matches(')')? {
                        return Err(Decline::Error); // missing ), unterminated subpattern
                    }
                    let node = if kind == '=' {
                        Node::Assert { dir, p }
                    } else if !self.tree.subs[p as usize].data.is_empty() {
                        Node::AssertNot { dir, p }
                    } else {
                        Node::Failure
                    };
                    self.push(sp, node);
                    return Ok(None);
                }
                Some('(') => {
                    // conditional backreference group
                    let condname = self.src.getuntil(')')?;
                    let condgroup;
                    if !ascii_decimal(&condname) {
                        let n = group_name(&condname)?;
                        let Some(&(_, gid)) = self.tree.state.groupdict.iter().find(|(k, _)| *k == n) else {
                            return Err(Decline::Error); // unknown group name
                        };
                        condgroup = gid;
                    } else {
                        let v = int_of(&condname);
                        if v == 0 {
                            return Err(Decline::Error); // bad group number
                        }
                        if v >= self.env.maxgroups as u64 {
                            return Err(Decline::Error); // invalid group reference
                        }
                        condgroup = v as u32;
                        if !self.tree.state.grouprefpos.contains(&condgroup) {
                            self.tree.state.grouprefpos.push(condgroup);
                        }
                    }
                    self.tree.state.checklookbehindgroup(condgroup)?;
                    let yes = self.parse(verbose, nested + 1, false)?;
                    let no = if self.src.matches('|')? {
                        let no = self.parse(verbose, nested + 1, false)?;
                        if self.src.next == Tok::Ch(ch('|')) {
                            return Err(Decline::Error); // more than two branches
                        }
                        Some(no)
                    } else {
                        None
                    };
                    if !self.src.matches(')')? {
                        return Err(Decline::Error);
                    }
                    self.push(sp, Node::GroupRefExists { cond: condgroup, yes, no });
                    return Ok(None);
                }
                Some('>') => {
                    capture = false;
                    atomic = true;
                }
                Some('i' | 'L' | 'm' | 's' | 'x' | 'a' | 'u' | '-') => {
                    match self.parse_flags(c)? {
                        None => {
                            // global flags
                            if !first || !self.tree.subs[sp as usize].data.is_empty() {
                                return Err(Decline::Error); // not at the start
                            }
                            return Ok(Some(self.tree.state.flags & SRE_FLAG_VERBOSE != 0));
                        }
                        Some((a, d)) => {
                            add_flags = a;
                            del_flags = d;
                            capture = false;
                        }
                    }
                }
                _ => return Err(Decline::Error), // unknown extension
            }
        }
        // parse group contents
        let group = if capture {
            let state = &mut self.tree.state;
            let gid = state.groups();
            state.groupwidths.push(None);
            if state.groups() > self.env.maxgroups {
                return Err(Decline::Error); // too many groups
            }
            if let Some(n) = name {
                if state.groupdict.iter().any(|(k, _)| *k == n) {
                    return Err(Decline::Error); // redefinition of group name
                }
                state.groupdict.push((n, gid));
            }
            Some(gid)
        } else {
            None
        };
        let sub_verbose = (verbose || add_flags & SRE_FLAG_VERBOSE != 0) && del_flags & SRE_FLAG_VERBOSE == 0;
        let p = self.parse_sub(sub_verbose, nested + 1)?;
        if !self.src.matches(')')? {
            return Err(Decline::Error); // missing ), unterminated subpattern
        }
        if let Some(g) = group {
            let w = self.tree.getwidth(p);
            self.tree.state.groupwidths[g as usize] = Some(w);
        }
        if atomic {
            self.push(sp, Node::AtomicGroup(p));
        } else {
            self.push(sp, Node::Subpattern { group, add: add_flags, del: del_flags, p });
        }
        Ok(None)
    }

    /// `_parse_flags(source, state, char)`: `None` for global flags (applied to the state),
    /// else `(add_flags, del_flags)`.
    fn parse_flags(&mut self, first: u32) -> Result<Option<(u32, u32)>, Decline> {
        fn flag(c: u32) -> Option<u32> {
            Some(match char::from_u32(c)? {
                'i' => SRE_FLAG_IGNORECASE,
                'L' => SRE_FLAG_LOCALE,
                'm' => SRE_FLAG_MULTILINE,
                's' => SRE_FLAG_DOTALL,
                'x' => SRE_FLAG_VERBOSE,
                'a' => SRE_FLAG_ASCII,
                'u' => SRE_FLAG_UNICODE,
                _ => return None,
            })
        }
        let mut add_flags = 0;
        let mut del_flags = 0;
        let mut c = first;
        if c != ch('-') {
            loop {
                let f = flag(c).ok_or(Decline::Error)?;
                if self.src.istext {
                    if c == ch('L') {
                        return Err(Decline::Error);
                    }
                } else if c == ch('u') {
                    return Err(Decline::Error);
                } else if c == ch('L') {
                    return Err(Decline::Unsupported); // LOCALE: left to CPython
                }
                add_flags |= f;
                if f & TYPE_FLAGS != 0 && add_flags & TYPE_FLAGS != f {
                    return Err(Decline::Error); // flags 'a', 'u' and 'L' are incompatible
                }
                c = match self.src.get()? {
                    Tok::Ch(x) => x,
                    _ => return Err(Decline::Error),
                };
                if c == ch(')') || c == ch('-') || c == ch(':') {
                    break;
                }
                if flag(c).is_none() {
                    return Err(Decline::Error);
                }
            }
        }
        if c == ch(')') {
            self.tree.state.flags |= add_flags;
            return Ok(None);
        }
        if add_flags & GLOBAL_FLAGS != 0 {
            return Err(Decline::Error);
        }
        if c == ch('-') {
            c = match self.src.get()? {
                Tok::Ch(x) => x,
                _ => return Err(Decline::Error), // missing flag
            };
            if flag(c).is_none() {
                return Err(Decline::Error);
            }
            loop {
                let f = flag(c).ok_or(Decline::Error)?;
                if f & TYPE_FLAGS != 0 {
                    return Err(Decline::Error); // cannot turn off flags 'a', 'u' and 'L'
                }
                del_flags |= f;
                c = match self.src.get()? {
                    Tok::Ch(x) => x,
                    _ => return Err(Decline::Error), // missing :
                };
                if c == ch(':') {
                    break;
                }
                if flag(c).is_none() {
                    return Err(Decline::Error);
                }
            }
        }
        debug_assert!(c == ch(':'));
        if del_flags & GLOBAL_FLAGS != 0 {
            return Err(Decline::Error);
        }
        if add_flags & del_flags != 0 {
            return Err(Decline::Error); // flag turned on and off
        }
        Ok(Some((add_flags, del_flags)))
    }
}

/// `_uniq(items)`: `list(dict.fromkeys(items))`.
pub(crate) fn uniq(items: Vec<SetItem>) -> Vec<SetItem> {
    if items.len() <= 16 {
        let mut out: Vec<SetItem> = Vec::with_capacity(items.len());
        for it in items {
            if !out.contains(&it) {
                out.push(it);
            }
        }
        return out;
    }
    let mut seen = std::collections::HashSet::with_capacity(items.len());
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        if seen.insert(it.clone()) {
            out.push(it);
        }
    }
    out
}

/// `_parser.fix_flags(src, flags)`
fn fix_flags(istext: bool, mut flags: u32) -> Result<u32, Decline> {
    if istext {
        if flags & SRE_FLAG_LOCALE != 0 {
            return Err(Decline::Error);
        }
        if flags & SRE_FLAG_ASCII == 0 {
            flags |= SRE_FLAG_UNICODE;
        } else if flags & SRE_FLAG_UNICODE != 0 {
            return Err(Decline::Error);
        }
    } else {
        if flags & SRE_FLAG_UNICODE != 0 {
            return Err(Decline::Error);
        }
        if flags & SRE_FLAG_LOCALE != 0 && flags & SRE_FLAG_ASCII != 0 {
            return Err(Decline::Error);
        }
    }
    Ok(flags)
}

/// `_parser.parse(str, flags)`: the tree (with `state.flags` fixed), or why CPython must
/// parse it.
pub(crate) fn parse(pattern: &[u32], istext: bool, flags: u32, env: &Env<'_>) -> Result<Tree, Decline> {
    let src = Tokenizer::new(pattern, istext)?;
    let state = State {
        flags,
        groupdict: Vec::new(),
        groupwidths: {
            let mut w = Vec::with_capacity(4);
            w.push(None);
            w
        },
        lookbehindgroups: None,
        grouprefpos: Vec::new(),
    };
    let mut p =
        Parser { src, tree: Tree { subs: Vec::with_capacity(16), state, root: 0, maxrepeat: env.maxrepeat }, env };
    let root = p.parse_sub(flags & SRE_FLAG_VERBOSE != 0, 0)?;
    p.tree.root = root;
    p.tree.state.flags = fix_flags(istext, p.tree.state.flags)?;
    if p.src.next != Tok::End {
        return Err(Decline::Error); // unbalanced parenthesis
    }
    let groups = p.tree.state.groups();
    if p.tree.state.grouprefpos.iter().any(|&g| g >= groups) {
        return Err(Decline::Error); // invalid group reference
    }
    Ok(p.tree)
}
