//! `_strptime` for an exact subset, without regular expressions.
//!
//! CPython compiles a format into one regular expression (`TimeRE.pattern`, flags
//! IGNORECASE) and takes the **first** match that sre's backtracking finds, anchored at the
//! start only; leftover input is an error. A format here is a concatenation of items whose
//! captured text is simply the input they consume (no backreferences, no lookaround), so
//! the first match is determined by the end position each item chooses: each item lists
//! its possible end positions in the order sre's backtracking tries them (alternatives
//! left to right, greedy quantifiers longest first, duplicates dropped because a repeated
//! end position cannot change what follows), and the matcher takes the first assignment,
//! item by item, for which the rest matches. That is exactly the match sre returns.
//!
//! The subset (anything else is `None`, and the caller runs CPython's `_strptime`):
//! - ASCII format and input; the C locale's `LocaleTime` (English names, no alternative
//!   digits); CPython 3.14 or 3.15 regexes ([`Version`]);
//! - directives `%Y %y %m %d %H %I %M %S %f %j %b %B %a %A %p %z %%`, and `%:z` on 3.15;
//!   each at most once (a repeated group name is a `re.error` in CPython);
//! - literals (case-insensitive) and whitespace runs (`\s+`);
//! - `%d` only together with `%Y`/`%y` (3.14 warns, 3.15 raises otherwise);
//! - inputs whose first match consumes the whole string and whose values `_strptime`
//!   converts without raising (valid dates, consistent `%z` colons).

use super::cal;

/// Which CPython `_strptime` the regexes follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    V3_14,
    V3_15,
}

/// One supported directive (its regex group).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// `%Y`
    Year4,
    /// `%y`
    Year2,
    /// `%m`
    Month,
    /// `%d`
    Day,
    /// `%H`
    Hour24,
    /// `%I`
    Hour12,
    /// `%M`
    Minute,
    /// `%S`
    Second,
    /// `%f`
    Fraction,
    /// `%j`
    Julian,
    /// `%b`
    MonthAbbr,
    /// `%B`
    MonthFull,
    /// `%a`
    WeekdayAbbr,
    /// `%A`
    WeekdayFull,
    /// `%p`
    AmPm,
    /// `%z`
    Z,
    /// `%:z` (3.15)
    ColonZ,
}

impl Dir {
    fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

#[derive(Clone, Copy, Debug)]
enum Item {
    /// One literal character, matched ASCII-case-insensitively (IGNORECASE).
    Lit(u8),
    /// `\s+` (a run of format whitespace).
    Ws,
    /// A directive and the index of its capture.
    Field(Dir, u8),
}

/// A compiled format.
#[derive(Clone, Debug)]
pub struct Program {
    items: Vec<Item>,
    fields: Vec<Dir>,
    version: Version,
    /// Every item's end is unique given the next item (see [`separated`]): when the greedy
    /// pass fails, sre finds no match either, and the backtracking matcher is skipped.
    separated: bool,
}

/// Can byte `c` occur in a match of `dir` at an index >= 1? (Numbers: digits; names:
/// letters; `%z`: digits, `:` and `.`; the leading space of `%d %H %I` and the sign/`Z` of
/// `%z` only at index 0.)
fn inner(dir: Dir, c: u8) -> bool {
    match dir {
        Dir::MonthAbbr | Dir::MonthFull | Dir::WeekdayAbbr | Dir::WeekdayFull | Dir::AmPm => c.is_ascii_alphabetic(),
        Dir::Z | Dir::ColonZ => c.is_ascii_digit() || c == b':' || c == b'.',
        _ => c.is_ascii_digit(),
    }
}

/// Can a match of `dir` start with byte `c`?
fn first(dir: Dir, c: u8) -> bool {
    match dir {
        Dir::Day | Dir::Hour24 | Dir::Hour12 => c.is_ascii_digit() || c == b' ',
        Dir::Z | Dir::ColonZ => matches!(c, b'+' | b'-' | b'Z'),
        _ => inner(dir, c),
    }
}

/// A program where sre's backtracking can never reach an end the greedy pass did not try:
/// two ends of a directive both followed by the next literal would put that literal's
/// character inside the longer match (excluded by `inner`); a shorter whitespace run can
/// only help a directive that starts with one space (`%d %H %I`), whose " d" match ends
/// where its one-digit match after the full run ends; `%z` (which may be empty on 3.15) is
/// excluded after whitespace and before a literal that could start it. Directives directly
/// followed by directives are not separated.
fn separated(items: &[Item]) -> bool {
    items.iter().enumerate().all(|(i, &it)| {
        let next = items.get(i + 1).copied();
        match it {
            Item::Lit(_) => true,
            Item::Ws => match next {
                None | Some(Item::Lit(_)) => !matches!(next, Some(Item::Lit(c)) if is_space(c)),
                // A shorter run leaves one space for `%d %H %I`'s " d" form, which ends where
                // their one-digit form after the full run ends: the same continuations.
                Some(Item::Field(d, _)) => !matches!(d, Dir::Z | Dir::ColonZ),
                Some(Item::Ws) => false,
            },
            Item::Field(d, _) => match next {
                None | Some(Item::Ws) => true,
                Some(Item::Lit(c)) => {
                    let (lo, up) = (c.to_ascii_lowercase(), c.to_ascii_uppercase());
                    !inner(d, lo)
                        && !inner(d, up)
                        && !(matches!(d, Dir::Z | Dir::ColonZ) && (first(d, lo) || first(d, up)))
                }
                Some(Item::Field(..)) => false,
            },
        }
    })
}

/// Longest format / input handled (longer ones fall back; real ones are ≤ 30 chars).
pub const MAX_FORMAT: usize = 96;
pub const MAX_INPUT: usize = 128;
/// Backtracking budget; beyond it the caller falls back (CPython then does the same work).
const STEP_LIMIT: u32 = 4096;

/// Python's `\s` (and `str.isspace`) on ASCII.
#[inline]
pub fn is_space(b: u8) -> bool {
    matches!(b, 9..=13 | 28..=32)
}

/// `TimeRE.pattern` + compile for the supported subset.
pub fn compile(fmt: &[u8], version: Version) -> Option<Program> {
    if fmt.len() > MAX_FORMAT {
        return None;
    }
    let mut items = Vec::with_capacity(fmt.len());
    let mut fields = Vec::new();
    let mut seen = 0u32;
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        if c >= 0x80 {
            return None;
        }
        if is_space(c) {
            while i < fmt.len() && is_space(fmt[i]) {
                i += 1;
            }
            items.push(Item::Ws);
            continue;
        }
        if c != b'%' {
            items.push(Item::Lit(c));
            i += 1;
            continue;
        }
        let d = *fmt.get(i + 1)?;
        i += 2;
        let dir = match d {
            b'%' => {
                items.push(Item::Lit(b'%'));
                continue;
            }
            b'Y' => Dir::Year4,
            b'y' => Dir::Year2,
            b'm' => Dir::Month,
            b'd' => Dir::Day,
            b'H' => Dir::Hour24,
            b'I' => Dir::Hour12,
            b'M' => Dir::Minute,
            b'S' => Dir::Second,
            b'f' => Dir::Fraction,
            b'j' => Dir::Julian,
            b'b' => Dir::MonthAbbr,
            b'B' => Dir::MonthFull,
            b'a' => Dir::WeekdayAbbr,
            b'A' => Dir::WeekdayFull,
            b'p' => Dir::AmPm,
            b'z' => Dir::Z,
            b':' if version == Version::V3_15 && fmt.get(i) == Some(&b'z') => {
                i += 1;
                Dir::ColonZ
            }
            _ => return None,
        };
        if seen & dir.bit() != 0 {
            return None; // "redefinition of group name" (re.error)
        }
        seen |= dir.bit();
        items.push(Item::Field(dir, fields.len() as u8));
        fields.push(dir);
    }
    let has_year = seen & (Dir::Year4.bit() | Dir::Year2.bit()) != 0;
    if seen & Dir::Day.bit() != 0 && !has_year {
        return None; // 3.14: DeprecationWarning; 3.15: ValueError
    }
    let separated = separated(&items);
    Some(Program { items, fields, version, separated })
}

/// End positions of one item, in sre's order, without duplicates.
struct Ends {
    buf: [u8; 24],
    len: usize,
}

impl Ends {
    #[inline(always)]
    fn new() -> Self {
        Ends { buf: [0; 24], len: 0 }
    }
    /// Append an end position known to differ from the ones already listed.
    #[inline(always)]
    fn add(&mut self, e: usize) {
        self.buf[self.len] = e as u8;
        self.len += 1;
    }
    #[inline]
    fn push(&mut self, e: usize) {
        let e = e as u8;
        if !self.buf[..self.len].contains(&e) && self.len < self.buf.len() {
            self.buf[self.len] = e;
            self.len += 1;
        }
    }
    fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// A character class of the directive regexes (reference tables for the tests of the
/// hand-written matchers below).
#[cfg(test)]
#[derive(Clone, Copy)]
enum C {
    /// `\d` (ASCII input: `[0-9]`)
    D,
    /// `[a-b]`
    R(u8, u8),
    /// a literal digit
    L(u8),
    /// a literal space
    Sp,
}

#[cfg(test)]
impl C {
    fn matches(self, b: u8) -> bool {
        match self {
            C::D => b.is_ascii_digit(),
            C::R(lo, hi) => (lo..=hi).contains(&b),
            C::L(x) => b == x,
            C::Sp => b == b' ',
        }
    }
}

#[cfg(test)]
use C::{D, L, R, Sp};

// The alternatives of each numeric directive, in regex order (`TimeRE.__init__`).
#[cfg(test)]
const ALT_M: &[&[C]] = &[&[L(b'1'), R(b'0', b'2')], &[L(b'0'), R(b'1', b'9')], &[R(b'1', b'9')]];
#[cfg(test)]
const ALT_D: &[&[C]] = &[
    &[L(b'3'), R(b'0', b'1')],
    &[R(b'1', b'2'), D],
    &[L(b'0'), R(b'1', b'9')],
    &[R(b'1', b'9')],
    &[Sp, R(b'1', b'9')],
];
#[cfg(test)]
const ALT_H: &[&[C]] = &[&[L(b'2'), R(b'0', b'3')], &[R(b'0', b'1'), D], &[D], &[Sp, D]];
#[cfg(test)]
const ALT_I: &[&[C]] = &[&[L(b'1'), R(b'0', b'2')], &[L(b'0'), R(b'1', b'9')], &[R(b'1', b'9')], &[Sp, R(b'1', b'9')]];
#[cfg(test)]
const ALT_MIN: &[&[C]] = &[&[R(b'0', b'5'), D], &[D]];
#[cfg(test)]
const ALT_S: &[&[C]] = &[&[L(b'6'), R(b'0', b'1')], &[R(b'0', b'5'), D], &[D]];
#[cfg(test)]
const ALT_J: &[&[C]] = &[
    &[L(b'3'), L(b'6'), R(b'0', b'6')],
    &[L(b'3'), R(b'0', b'5'), D],
    &[R(b'1', b'2'), D, D],
    &[L(b'0'), R(b'1', b'9'), D],
    &[L(b'0'), L(b'0'), R(b'1', b'9')],
    &[R(b'1', b'9'), D],
    &[L(b'0'), R(b'1', b'9')],
    &[R(b'1', b'9')],
];
#[cfg(test)]
const ALT_Y2: &[&[C]] = &[&[D, D]];
#[cfg(test)]
const ALT_Y4: &[&[C]] = &[&[D, D, D, D]];

/// C-locale `LocaleTime` names (lower case), calendar order.
pub const F_MONTH: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
pub const A_MONTH: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
pub const F_WEEKDAY: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
pub const A_WEEKDAY: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
pub const AM_PM: [&str; 2] = ["am", "pm"];

/// `__seqToRE` order: stable sort by length, longest first (indices into the list above).
const F_MONTH_RE: [u8; 12] = [8, 1, 10, 11, 0, 9, 7, 2, 3, 5, 6, 4];
const A_MONTH_RE: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const F_WEEKDAY_RE: [u8; 7] = [2, 3, 5, 1, 0, 4, 6];
const A_WEEKDAY_RE: [u8; 7] = [0, 1, 2, 3, 4, 5, 6];
const AM_PM_RE: [u8; 2] = [0, 1];

#[cfg(test)]
fn alts(s: &[u8], p: usize, alts: &[&[C]], out: &mut Ends) {
    for alt in alts {
        if p + alt.len() <= s.len() && alt.iter().zip(&s[p..]).all(|(c, &b)| c.matches(b)) {
            out.push(p + alt.len());
        }
    }
}

#[inline(always)]
fn at(s: &[u8], i: usize) -> u8 {
    if i < s.len() { s[i] } else { 0 }
}
#[inline(always)]
fn dg(b: u8) -> bool {
    b.wrapping_sub(b'0') < 10
}
#[inline(always)]
fn rg(b: u8, lo: u8, hi: u8) -> bool {
    b.wrapping_sub(lo) <= hi - lo
}

/// End positions of a numeric directive, in the order of its regex alternatives (see the
/// `ALT_*` reference tables; equivalence is tested exhaustively).
#[inline(always)]
fn numeric_ends(dir: Dir, s: &[u8], p: usize, out: &mut Ends) {
    let (a, b) = (at(s, p), at(s, p + 1));
    match dir {
        // 1[0-2]|0[1-9]|[1-9]
        Dir::Month => {
            if (a == b'1' && rg(b, b'0', b'2')) || (a == b'0' && rg(b, b'1', b'9')) {
                out.add(p + 2);
            }
            if rg(a, b'1', b'9') {
                out.add(p + 1);
            }
        }
        // 3[0-1]|[1-2]\d|0[1-9]|[1-9]| [1-9]
        Dir::Day => {
            if (a == b'3' && rg(b, b'0', b'1')) || (rg(a, b'1', b'2') && dg(b)) || (a == b'0' && rg(b, b'1', b'9')) {
                out.add(p + 2);
            }
            if rg(a, b'1', b'9') {
                out.add(p + 1);
            } else if a == b' ' && rg(b, b'1', b'9') {
                out.add(p + 2);
            }
        }
        // 2[0-3]|[0-1]\d|\d| \d
        Dir::Hour24 => {
            if (a == b'2' && rg(b, b'0', b'3')) || (rg(a, b'0', b'1') && dg(b)) {
                out.add(p + 2);
            }
            if dg(a) {
                out.add(p + 1);
            } else if a == b' ' && dg(b) {
                out.add(p + 2);
            }
        }
        // 1[0-2]|0[1-9]|[1-9]| [1-9]
        Dir::Hour12 => {
            if (a == b'1' && rg(b, b'0', b'2')) || (a == b'0' && rg(b, b'1', b'9')) {
                out.add(p + 2);
            }
            if rg(a, b'1', b'9') {
                out.add(p + 1);
            } else if a == b' ' && rg(b, b'1', b'9') {
                out.add(p + 2);
            }
        }
        // [0-5]\d|\d
        Dir::Minute => {
            if rg(a, b'0', b'5') && dg(b) {
                out.add(p + 2);
            }
            if dg(a) {
                out.add(p + 1);
            }
        }
        // 6[0-1]|[0-5]\d|\d
        Dir::Second => {
            if (a == b'6' && rg(b, b'0', b'1')) || (rg(a, b'0', b'5') && dg(b)) {
                out.add(p + 2);
            }
            if dg(a) {
                out.add(p + 1);
            }
        }
        // \d\d
        Dir::Year2 => {
            if dg(a) && dg(b) {
                out.add(p + 2);
            }
        }
        // \d\d\d\d
        Dir::Year4 => {
            if dg(a) && dg(b) && dg(at(s, p + 2)) && dg(at(s, p + 3)) {
                out.add(p + 4);
            }
        }
        // 36[0-6]|3[0-5]\d|[1-2]\d\d|0[1-9]\d|00[1-9]|[1-9]\d|0[1-9]|[1-9]
        Dir::Julian => {
            let c = at(s, p + 2);
            if (a == b'3' && b == b'6' && rg(c, b'0', b'6'))
                || (a == b'3' && rg(b, b'0', b'5') && dg(c))
                || (rg(a, b'1', b'2') && dg(b) && dg(c))
                || (a == b'0' && rg(b, b'1', b'9') && dg(c))
                || (a == b'0' && b == b'0' && rg(c, b'1', b'9'))
            {
                out.add(p + 3);
            }
            if (rg(a, b'1', b'9') && dg(b)) || (a == b'0' && rg(b, b'1', b'9')) {
                out.add(p + 2);
            }
            if rg(a, b'1', b'9') {
                out.add(p + 1);
            }
        }
        // [0-9]{1,6}
        Dir::Fraction => {
            let mut k = 0;
            while k < 6 && dg(at(s, p + k)) {
                k += 1;
            }
            while k > 0 {
                out.add(p + k);
                k -= 1;
            }
        }
        _ => unreachable!("not numeric"),
    }
}

/// The name alternatives (`__seqToRE`: longest first). No name of a list is a prefix of
/// another one of the same list (checked by a test), so at most one matches at `p`.
fn names(s: &[u8], p: usize, list: &[&str], order: &[u8], out: &mut Ends) {
    let rest = &s[p.min(s.len())..];
    // ASCII lower case of the next few bytes, once (IGNORECASE on ASCII input).
    let mut low = [0u8; 9];
    let k = rest.len().min(9);
    for (d, &b) in low.iter_mut().zip(&rest[..k]) {
        *d = b.to_ascii_lowercase();
    }
    for &i in order {
        let n = list[i as usize].as_bytes();
        if n.len() <= k && &low[..n.len()] == n {
            out.add(p + n.len());
            return;
        }
    }
}

/// `[+-]\d\d:?[0-5]\d(:?[0-5]\d(\.\d{1,6})?)?` (colons optional) or the `%:z` variant
/// (colons required), then `(?-i:Z)`, then (3.15: the group is optional) the empty match.
fn z_ends(s: &[u8], p: usize, colons_required: bool, optional: bool, out: &mut Ends) {
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let d = |i: usize| at(i).is_ascii_digit();
    let d05 = |i: usize| (b'0'..=b'5').contains(&at(i));
    if matches!(at(p), b'+' | b'-') && d(p + 1) && d(p + 2) {
        let q = p + 3;
        for c1 in [true, false] {
            if c1 && at(q) != b':' || !c1 && colons_required {
                continue;
            }
            let r = q + usize::from(c1);
            if d05(r) && d(r + 1) {
                let t = r + 2;
                for c2 in [true, false] {
                    if c2 && at(t) != b':' || !c2 && colons_required {
                        continue;
                    }
                    let u = t + usize::from(c2);
                    if d05(u) && d(u + 1) {
                        let v = u + 2;
                        if at(v) == b'.' && d(v + 1) {
                            let k = (v + 1..s.len().min(v + 7)).take_while(|&i| s[i].is_ascii_digit()).count();
                            for kk in (1..=k).rev() {
                                out.push(v + 1 + kk);
                            }
                        }
                        out.push(v);
                    }
                }
                out.push(t);
            }
        }
    }
    if at(p) == b'Z' {
        out.push(p + 1);
    }
    if optional {
        out.push(p);
    }
}

impl Program {
    /// The directives of this format, in format order.
    pub fn fields(&self) -> &[Dir] {
        &self.fields
    }

    #[inline(always)]
    fn ends(&self, dir: Dir, s: &[u8], p: usize, out: &mut Ends) {
        let v315 = self.version == Version::V3_15;
        match dir {
            Dir::MonthAbbr => names(s, p, &A_MONTH, &A_MONTH_RE, out),
            Dir::MonthFull => names(s, p, &F_MONTH, &F_MONTH_RE, out),
            Dir::WeekdayAbbr => names(s, p, &A_WEEKDAY, &A_WEEKDAY_RE, out),
            Dir::WeekdayFull => names(s, p, &F_WEEKDAY, &F_WEEKDAY_RE, out),
            Dir::AmPm => names(s, p, &AM_PM, &AM_PM_RE, out),
            Dir::Z => z_ends(s, p, false, v315, out),
            Dir::ColonZ => z_ends(s, p, true, true, out),
            _ => numeric_ends(dir, s, p, out),
        }
    }

    /// sre's first match from position 0: `Ok(Some(end))` with `caps` filled, `Ok(None)` if
    /// there is none, `Err(())` if the backtracking budget ran out.
    fn first_match(
        &self,
        i: usize,
        pos: usize,
        s: &[u8],
        caps: &mut [(u8, u8)],
        steps: &mut u32,
    ) -> Result<Option<usize>, ()> {
        *steps += 1;
        if *steps > STEP_LIMIT {
            return Err(());
        }
        let Some(&item) = self.items.get(i) else { return Ok(Some(pos)) };
        match item {
            Item::Lit(c) => {
                if pos < s.len() && s[pos].eq_ignore_ascii_case(&c) {
                    self.first_match(i + 1, pos + 1, s, caps, steps)
                } else {
                    Ok(None)
                }
            }
            Item::Ws => {
                let k = s[pos..].iter().take_while(|&&b| is_space(b)).count();
                for e in (pos + 1..=pos + k).rev() {
                    if let Some(end) = self.first_match(i + 1, e, s, caps, steps)? {
                        return Ok(Some(end));
                    }
                }
                Ok(None)
            }
            Item::Field(dir, fi) => {
                let mut ends = Ends::new();
                self.ends(dir, s, pos, &mut ends);
                for &e in ends.as_slice() {
                    caps[fi as usize] = (pos as u8, e);
                    if let Some(end) = self.first_match(i + 1, e as usize, s, caps, steps)? {
                        return Ok(Some(end));
                    }
                }
                Ok(None)
            }
        }
    }

    /// sre's first match found in one forward pass, when it is on the greedy path: each
    /// item takes the first of its end positions (in sre's order) that passes a check the
    /// next item needs anyway (the next literal is there, whitespace is there, the next
    /// directive has an end there), and `\s+` takes the whole run (sre tries it first).
    /// Every end sre tries before the one taken fails that check, so sre fails it too; if
    /// the rest then matches along this path, sre's first match is this path. `None` when
    /// the path fails somewhere (sre may still find a match by backtracking: the caller
    /// runs [`Self::first_match`] then).
    fn first_match_greedy(&self, s: &[u8], caps: &mut [(u8, u8)]) -> Option<usize> {
        let mut pos = 0usize;
        let items = &self.items;
        for (i, &item) in items.iter().enumerate() {
            match item {
                Item::Lit(c) => {
                    if at(s, pos).eq_ignore_ascii_case(&c) && pos < s.len() {
                        pos += 1;
                    } else {
                        return None;
                    }
                }
                Item::Ws => {
                    let k = s[pos..].iter().take_while(|&&b| is_space(b)).count();
                    if k == 0 {
                        return None;
                    }
                    pos += k;
                }
                Item::Field(dir, fi) => {
                    let mut ends = Ends::new();
                    self.ends(dir, s, pos, &mut ends);
                    let next = items.get(i + 1).copied();
                    let e = *ends.as_slice().iter().find(|&&e| {
                        let e = e as usize;
                        match next {
                            None => true,
                            Some(Item::Lit(c)) => e < s.len() && s[e].eq_ignore_ascii_case(&c),
                            Some(Item::Ws) => is_space(at(s, e)),
                            Some(Item::Field(d2, _)) => {
                                let mut n2 = Ends::new();
                                self.ends(d2, s, e, &mut n2);
                                n2.len > 0
                            }
                        }
                    })?;
                    caps[fi as usize] = (pos as u8, e);
                    pos = e as usize;
                }
            }
        }
        Some(pos)
    }

    /// Run `_strptime` on `s` (ASCII): `Some` when the result is CPython's and no exception
    /// would be raised; `None` means "let CPython decide" (no match, leftover input, an
    /// invalid date, an inconsistent `%z`, ...).
    ///
    /// `full`: also compute `weekday` and `julian` (only `time.strptime` returns them;
    /// otherwise they are 0).
    pub fn parse(&self, s: &[u8], full: bool) -> Option<Parsed> {
        if s.len() > MAX_INPUT || !s.is_ascii() {
            return None;
        }
        let mut caps = [(0u8, 0u8); 24];
        let end = match self.first_match_greedy(s, &mut caps) {
            Some(e) => e,
            None if self.separated => return None,
            None => {
                let mut steps = 0;
                self.first_match(0, 0, s, &mut caps, &mut steps).ok()??
            }
        };
        if end != s.len() {
            return None; // "unconverted data remains"
        }
        resolve(&self.fields, s, &caps, full)
    }
}

/// The values `_strptime._strptime` returns (its 3-tuple, flattened).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Microseconds from `%f`.
    pub fraction: u32,
    /// Monday = 0.
    pub weekday: u32,
    /// Day of the year.
    pub julian: i64,
    /// Seconds east of UTC (`%z`), `None` without one.
    pub gmtoff: Option<i64>,
    /// Microseconds part of the offset (same sign as `gmtoff`).
    pub gmtoff_fraction: i64,
}

/// `int(text)` for regex-validated numeric text (optional leading space, ASCII digits).
fn num(t: &[u8]) -> i64 {
    t.iter().filter(|b| b.is_ascii_digit()).fold(0i64, |a, &b| a * 10 + i64::from(b - b'0'))
}

fn index_of(list: &[&str], t: &[u8]) -> u32 {
    list.iter().position(|n| n.as_bytes().eq_ignore_ascii_case(t)).expect("matched name") as u32
}

/// The `z`/`colon_z` branch of `_strptime`: (gmtoff, gmtoff_fraction), or `None` if
/// CPython raises (inconsistent colons, `int()` of a colon).
fn parse_z(z: &[u8]) -> Option<(i64, i64)> {
    if z == b"Z" {
        return Some((0, 0));
    }
    // z = z[:3] + z[4:] (and z[:5] + z[6:]) without allocating.
    let mut v = [0u8; 20];
    let mut n = 0;
    let mut i = 0;
    let colon1 = z.len() > 3 && z[3] == b':';
    while i < z.len() && n < v.len() {
        if colon1 && i == 3 {
            i += 1;
            continue;
        }
        v[n] = z[i];
        n += 1;
        i += 1;
    }
    let mut v = &mut v[..n];
    let mut tmp = [0u8; 20];
    if colon1 && v.len() > 5 {
        if v[5] != b':' {
            return None; // "Inconsistent use of : in ..."
        }
        tmp[..5].copy_from_slice(&v[..5]);
        tmp[5..v.len() - 1].copy_from_slice(&v[6..]);
        let len = v.len() - 1;
        v = &mut tmp[..len];
    }
    let digits = |r: &[u8]| -> Option<i64> { r.iter().all(u8::is_ascii_digit).then(|| num(r)) };
    let hours = digits(&v[1..3])?;
    let minutes = digits(&v[3..5])?;
    let seconds = if v.len() > 5 { digits(&v[5..v.len().min(7)])? } else { 0 };
    let rem = if v.len() > 8 { &v[8..] } else { &[][..] };
    if rem.len() > 6 || !rem.iter().all(u8::is_ascii_digit) || v.len() == 6 || (v.len() > 7 && v[7] != b'.') {
        return None;
    }
    let mut frac = num(rem);
    for _ in rem.len()..6 {
        frac *= 10;
    }
    let mut off = hours * 3600 + minutes * 60 + seconds;
    if v[0] == b'-' {
        off = -off;
        frac = -frac;
    }
    Some((off, frac))
}

fn resolve(fields: &[Dir], s: &[u8], caps: &[(u8, u8)], full: bool) -> Option<Parsed> {
    let text = |i: usize| &s[caps[i].0 as usize..caps[i].1 as usize];
    let ampm_pm = fields.iter().position(|&d| d == Dir::AmPm).is_some_and(|i| text(i).eq_ignore_ascii_case(b"pm"));
    let mut year: Option<i64> = None;
    let (mut month, mut day, mut hour, mut minute, mut second, mut fraction) = (1u32, 1u32, 0u32, 0u32, 0u32, 0u32);
    let mut weekday: Option<u32> = None;
    let mut julian: Option<i64> = None;
    let mut gmtoff: Option<i64> = None;
    let mut gmtoff_fraction = 0i64;
    for (i, &dir) in fields.iter().enumerate() {
        let t = text(i);
        match dir {
            Dir::Year2 => {
                let y = num(t);
                year = Some(if y <= 68 { y + 2000 } else { y + 1900 });
            }
            Dir::Year4 => year = Some(num(t)),
            Dir::Month => month = num(t) as u32,
            Dir::MonthFull => month = index_of(&F_MONTH, t) + 1,
            Dir::MonthAbbr => month = index_of(&A_MONTH, t) + 1,
            Dir::Day => day = num(t) as u32,
            Dir::Hour24 => hour = num(t) as u32,
            Dir::Hour12 => {
                hour = num(t) as u32;
                if ampm_pm {
                    if hour != 12 {
                        hour += 12;
                    }
                } else if hour == 12 {
                    hour = 0;
                }
            }
            Dir::Minute => minute = num(t) as u32,
            Dir::Second => second = num(t) as u32,
            Dir::Fraction => {
                let mut f = num(t);
                for _ in t.len()..6 {
                    f *= 10;
                }
                fraction = f as u32;
            }
            Dir::WeekdayFull => weekday = Some(index_of(&F_WEEKDAY, t)),
            Dir::WeekdayAbbr => weekday = Some(index_of(&A_WEEKDAY, t)),
            Dir::Julian => julian = Some(num(t)),
            Dir::Z | Dir::ColonZ => {
                if !t.is_empty() {
                    let (o, f) = parse_z(t)?;
                    gmtoff = Some(o);
                    gmtoff_fraction = f;
                }
            }
            Dir::AmPm => {}
        }
    }
    let mut leap_year_fix = false;
    let mut year = match year {
        Some(y) => y,
        None if month == 2 && day == 29 => {
            leap_year_fix = true;
            1904
        }
        None => 1900,
    };
    // (No %U/%W/%G/%V: the week-based branches are not reachable.)
    let julian = match julian {
        None => {
            if !cal::valid_date(year, month, day) {
                return None; // datetime_date(year, month, day) raises
            }
            if full { i64::from(cal::day_of_year(year, month, day)) } else { 0 }
        }
        Some(j) => {
            if !(1..=9999).contains(&year) {
                return None; // datetime_date(year, 1, 1) raises
            }
            let (y, m, d) = cal::ord_to_ymd((j - 1) + cal::ymd_to_ord(year, 1, 1))?;
            year = y;
            month = m;
            day = d;
            j
        }
    };
    let weekday = match weekday {
        Some(w) => w,
        None if full => cal::weekday(year, month, day),
        None => 0,
    };
    if leap_year_fix {
        year = 1900;
    }
    Some(Parsed { year, month, day, hour, minute, second, fraction, weekday, julian, gmtoff, gmtoff_fraction })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str, f: &str) -> Option<Parsed> {
        compile(f.as_bytes(), Version::V3_15)?.parse(s.as_bytes(), true)
    }

    #[test]
    fn regex_name_order_is_stable_length_sort() {
        fn order(list: &[&str]) -> Vec<u8> {
            let mut idx: Vec<u8> = (0..list.len() as u8).collect();
            idx.sort_by_key(|&i| std::cmp::Reverse(list[i as usize].len()));
            idx
        }
        assert_eq!(order(&F_MONTH), F_MONTH_RE);
        assert_eq!(order(&A_MONTH), A_MONTH_RE);
        assert_eq!(order(&F_WEEKDAY), F_WEEKDAY_RE);
        assert_eq!(order(&A_WEEKDAY), A_WEEKDAY_RE);
        assert_eq!(order(&AM_PM), AM_PM_RE);
    }

    #[test]
    fn basics() {
        let r = p("2024-03-15 13:45:30", "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!((r.year, r.month, r.day, r.hour, r.minute, r.second), (2024, 3, 15, 13, 45, 30));
        assert_eq!((r.weekday, r.julian, r.gmtoff), (4, 75, None));
        let r = p("15/Mar/2024:13:45:30 +0100", "%d/%b/%Y:%H:%M:%S %z").unwrap();
        assert_eq!((r.month, r.gmtoff), (3, Some(3600)));
        let r = p("03/15/2024 12:05 AM", "%m/%d/%Y %I:%M %p").unwrap();
        assert_eq!(r.hour, 0);
        let r = p("03/15/2024 01:05 pm", "%m/%d/%Y %I:%M %p").unwrap();
        assert_eq!(r.hour, 13);
        let r = p("2024-03-15T13:45:30.12-02:30:15.5", "%Y-%m-%dT%H:%M:%S.%f%z").unwrap();
        assert_eq!((r.fraction, r.gmtoff, r.gmtoff_fraction), (120000, Some(-(2 * 3600 + 30 * 60 + 15)), -500000));
    }

    #[test]
    fn names_are_not_prefixes_of_each_other() {
        for list in [&F_MONTH[..], &A_MONTH[..], &F_WEEKDAY[..], &A_WEEKDAY[..], &AM_PM[..]] {
            for a in list {
                for b in list {
                    assert!(a == b || !b.starts_with(a), "{a} / {b}");
                }
            }
        }
    }

    #[test]
    fn greedy_path_agrees_with_backtracking() {
        // Inputs over a small alphabet (all of length <= 4, and every one- and two-position
        // mutation of realistic inputs), for formats mixing adjacent directives, names,
        // whitespace and %z: when the one-pass greedy matcher succeeds, its match is the
        // backtracking matcher's (sre's) first match.
        let cases: [(&str, &str); 16] = [
            ("%Y%m%d", "2024111"),
            ("%d%m%y", "11124"),
            ("%H%M%S", "12345"),
            ("%m/%d/%Y %I:%M %p", "03/15/2024 01:45 PM"),
            ("%d/%b/%Y:%H:%M:%S %z", "15/Mar/2024:13:45:30 +0100"),
            ("%Y-%m-%d %H:%M", "2024-03-15 13:45"),
            ("%d.%m.%Y %H:%M", "5.3.2024  9:05"),
            ("%j%y", "1124"),
            ("%f%S", "1234567"),
            ("%I%p", "11pm"),
            ("%b%d", "Mar15"),
            ("%H %z %M", "10  30"),
            ("%y%z", "24+01:00"),
            ("%d %H", " 5  9"),
            ("%A %d %B", "Friday 15 March"),
            ("%Y-%m-%dT%H:%M:%S.%f%z", "2024-03-15T13:45:30.123456+02:00"),
        ];
        let alphabet: &[u8] = b"0123 59/:+-.ZpaMmJT";
        let mut checked = 0;
        let mut check = |prog: &Program, s: &[u8], f: &str| {
            let mut c1 = [(0u8, 0u8); 24];
            let greedy = prog.first_match_greedy(s, &mut c1);
            if greedy.is_none() && prog.separated {
                let mut c2 = [(0u8, 0u8); 24];
                let mut steps = 0;
                assert_eq!(prog.first_match(0, 0, s, &mut c2, &mut steps), Ok(None), "separated {f} {s:?}");
            }
            if let Some(e1) = greedy {
                let mut c2 = [(0u8, 0u8); 24];
                let mut steps = 0;
                let e2 = prog.first_match(0, 0, s, &mut c2, &mut steps).unwrap().unwrap();
                assert_eq!((e1, &c1[..prog.fields.len()]), (e2, &c2[..prog.fields.len()]), "{f} {s:?}");
                checked += 1;
            }
        };
        for (f, seed) in cases {
            for v in [Version::V3_14, Version::V3_15] {
                let Some(prog) = compile(f.as_bytes(), v) else { continue };
                let mut s = Vec::new();
                for n in 0..=4usize {
                    for mut k in 0..alphabet.len().pow(n as u32) {
                        s.clear();
                        for _ in 0..n {
                            s.push(alphabet[k % alphabet.len()]);
                            k /= alphabet.len();
                        }
                        check(&prog, &s, f);
                    }
                }
                let seed = seed.as_bytes();
                for i in 0..seed.len() {
                    for &a in alphabet {
                        let mut m = seed.to_vec();
                        m[i] = a;
                        check(&prog, &m, f);
                        for j in i + 1..seed.len() {
                            for &b in alphabet {
                                let mut m2 = m.clone();
                                m2[j] = b;
                                check(&prog, &m2, f);
                            }
                        }
                        let mut ins = seed.to_vec();
                        ins.insert(i, a);
                        check(&prog, &ins, f);
                        let mut del = seed.to_vec();
                        del.remove(i);
                        check(&prog, &del, f);
                    }
                }
            }
        }
        assert!(checked > 10_000, "{checked}");
        for (f, sep) in [
            ("%Y-%m-%d %H:%M:%S", true),
            ("%m/%d/%Y %I:%M %p", true),
            ("%d/%b/%Y:%H:%M:%S %z", false),
            ("%Y%m%d", false),
            ("%d.%m.%Y %H:%M", true),
        ] {
            assert_eq!(compile(f.as_bytes(), Version::V3_15).unwrap().separated, sep, "{f}");
        }
    }

    #[test]
    fn backtracking_order() {
        // %m%d on "111": sre takes 11/1.
        let r = p("2024111", "%Y%m%d").unwrap();
        assert_eq!((r.month, r.day), (11, 1));
        // leftover input is an error, even though another split would consume it all
        assert_eq!(p("20241", "%Y%m%d"), None);
        assert_eq!(p("1234567", "%f%S").map(|r| (r.fraction, r.second)), Some((123456, 7)));
        assert_eq!(p("12", "%H%M").map(|r| (r.hour, r.minute)), Some((1, 2)));
    }

    /// The hand-written numeric matchers equal the regex alternation tables on every
    /// input of up to 4 characters over a relevant alphabet.
    #[test]
    fn numeric_matchers_match_regex_tables() {
        let alphabet = b"0123456789 :x";
        let dirs: [(Dir, &[&[C]]); 9] = [
            (Dir::Month, ALT_M),
            (Dir::Day, ALT_D),
            (Dir::Hour24, ALT_H),
            (Dir::Hour12, ALT_I),
            (Dir::Minute, ALT_MIN),
            (Dir::Second, ALT_S),
            (Dir::Julian, ALT_J),
            (Dir::Year2, ALT_Y2),
            (Dir::Year4, ALT_Y4),
        ];
        let mut inputs: Vec<Vec<u8>> = vec![vec![]];
        for _ in 0..4 {
            let mut next = Vec::new();
            for i in &inputs {
                for &c in alphabet {
                    let mut j = i.clone();
                    j.push(c);
                    next.push(j);
                }
            }
            inputs.extend(next);
            inputs.sort();
            inputs.dedup();
        }
        for (dir, table) in dirs {
            for inp in &inputs {
                let (mut a, mut b) = (Ends::new(), Ends::new());
                alts(inp, 0, table, &mut a);
                numeric_ends(dir, inp, 0, &mut b);
                assert_eq!(a.as_slice(), b.as_slice(), "{dir:?} on {:?}", String::from_utf8_lossy(inp));
            }
        }
    }

    #[test]
    fn rejects() {
        assert!(compile(b"%d", Version::V3_15).is_none());
        assert!(compile(b"%Y %Y", Version::V3_15).is_none());
        assert!(compile(b"%:z", Version::V3_14).is_none());
        assert!(compile(b"%Q", Version::V3_15).is_none());
        assert!(compile(b"%", Version::V3_15).is_none());
        assert!(compile(b"%-d", Version::V3_15).is_none());
        assert_eq!(p("2023-02-29", "%Y-%m-%d"), None);
        assert_eq!(p("+05:3015", "%z"), None);
        assert_eq!(p("+0530:15", "%z"), None);
    }

    #[test]
    fn optional_z_315() {
        let r = p("2024 ", "%Y %z").unwrap();
        assert_eq!(r.gmtoff, None);
        assert!(compile(b"%Y %z", Version::V3_14).unwrap().parse(b"2024 ", true).is_none());
    }
}
