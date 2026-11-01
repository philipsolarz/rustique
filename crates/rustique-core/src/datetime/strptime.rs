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
    Some(Program { items, fields, version })
}

/// End positions of one item, in sre's order, without duplicates.
struct Ends {
    buf: [u8; 24],
    len: usize,
}

impl Ends {
    fn new() -> Self {
        Ends { buf: [0; 24], len: 0 }
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

/// A character class of the directive regexes.
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

impl C {
    #[inline]
    fn matches(self, b: u8) -> bool {
        match self {
            C::D => b.is_ascii_digit(),
            C::R(lo, hi) => (lo..=hi).contains(&b),
            C::L(x) => b == x,
            C::Sp => b == b' ',
        }
    }
}

use C::{D, L, R, Sp};

// The alternatives of each numeric directive, in regex order (`TimeRE.__init__`).
const ALT_M: &[&[C]] = &[&[L(b'1'), R(b'0', b'2')], &[L(b'0'), R(b'1', b'9')], &[R(b'1', b'9')]];
const ALT_D: &[&[C]] =
    &[&[L(b'3'), R(b'0', b'1')], &[R(b'1', b'2'), D], &[L(b'0'), R(b'1', b'9')], &[R(b'1', b'9')], &[Sp, R(b'1', b'9')]];
const ALT_H: &[&[C]] = &[&[L(b'2'), R(b'0', b'3')], &[R(b'0', b'1'), D], &[D], &[Sp, D]];
const ALT_I: &[&[C]] = &[&[L(b'1'), R(b'0', b'2')], &[L(b'0'), R(b'1', b'9')], &[R(b'1', b'9')], &[Sp, R(b'1', b'9')]];
const ALT_MIN: &[&[C]] = &[&[R(b'0', b'5'), D], &[D]];
const ALT_S: &[&[C]] = &[&[L(b'6'), R(b'0', b'1')], &[R(b'0', b'5'), D], &[D]];
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
const ALT_Y2: &[&[C]] = &[&[D, D]];
const ALT_Y4: &[&[C]] = &[&[D, D, D, D]];

/// C-locale `LocaleTime` names (lower case), calendar order.
pub const F_MONTH: [&str; 12] =
    ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
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

fn alts(s: &[u8], p: usize, alts: &[&[C]], out: &mut Ends) {
    for alt in alts {
        if p + alt.len() <= s.len() && alt.iter().zip(&s[p..]).all(|(c, &b)| c.matches(b)) {
            out.push(p + alt.len());
        }
    }
}

fn names(s: &[u8], p: usize, list: &[&str], order: &[u8], out: &mut Ends) {
    for &k in order {
        let n = list[k as usize].as_bytes();
        if s.len() - p >= n.len() && s[p..p + n.len()].eq_ignore_ascii_case(n) {
            out.push(p + n.len());
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

    fn ends(&self, dir: Dir, s: &[u8], p: usize, out: &mut Ends) {
        let v315 = self.version == Version::V3_15;
        match dir {
            Dir::Year4 => alts(s, p, ALT_Y4, out),
            Dir::Year2 => alts(s, p, ALT_Y2, out),
            Dir::Month => alts(s, p, ALT_M, out),
            Dir::Day => alts(s, p, ALT_D, out),
            Dir::Hour24 => alts(s, p, ALT_H, out),
            Dir::Hour12 => alts(s, p, ALT_I, out),
            Dir::Minute => alts(s, p, ALT_MIN, out),
            Dir::Second => alts(s, p, ALT_S, out),
            Dir::Julian => alts(s, p, ALT_J, out),
            Dir::Fraction => {
                let k = s[p..].iter().take(6).take_while(|b| b.is_ascii_digit()).count();
                for kk in (1..=k).rev() {
                    out.push(p + kk);
                }
            }
            Dir::MonthAbbr => names(s, p, &A_MONTH, &A_MONTH_RE, out),
            Dir::MonthFull => names(s, p, &F_MONTH, &F_MONTH_RE, out),
            Dir::WeekdayAbbr => names(s, p, &A_WEEKDAY, &A_WEEKDAY_RE, out),
            Dir::WeekdayFull => names(s, p, &F_WEEKDAY, &F_WEEKDAY_RE, out),
            Dir::AmPm => names(s, p, &AM_PM, &AM_PM_RE, out),
            Dir::Z => z_ends(s, p, false, v315, out),
            Dir::ColonZ => z_ends(s, p, true, true, out),
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

    /// Run `_strptime` on `s` (ASCII): `Some` when the result is CPython's and no exception
    /// would be raised; `None` means "let CPython decide" (no match, leftover input, an
    /// invalid date, an inconsistent `%z`, ...).
    pub fn parse(&self, s: &[u8]) -> Option<Parsed> {
        if s.len() > MAX_INPUT || !s.is_ascii() {
            return None;
        }
        let mut caps = [(0u8, 0u8); 24];
        let mut steps = 0;
        let end = self.first_match(0, 0, s, &mut caps, &mut steps).ok()??;
        if end != s.len() {
            return None; // "unconverted data remains"
        }
        resolve(&self.fields, s, &caps)
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
    let mut v: Vec<u8> = z.to_vec();
    if v[3] == b':' {
        v.remove(3);
        if v.len() > 5 {
            if v[5] != b':' {
                return None; // "Inconsistent use of : in ..."
            }
            v.remove(5);
        }
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

fn resolve(fields: &[Dir], s: &[u8], caps: &[(u8, u8)]) -> Option<Parsed> {
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
            i64::from(cal::day_of_year(year, month, day))
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
        None => {
            if !cal::valid_date(year, month, day) {
                return None;
            }
            cal::weekday(year, month, day)
        }
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
        compile(f.as_bytes(), Version::V3_15)?.parse(s.as_bytes())
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
    fn backtracking_order() {
        // %m%d on "111": sre takes 11/1.
        let r = p("2024111", "%Y%m%d").unwrap();
        assert_eq!((r.month, r.day), (11, 1));
        // leftover input is an error, even though another split would consume it all
        assert_eq!(p("20241", "%Y%m%d"), None);
        assert_eq!(p("1234567", "%f%S").map(|r| (r.fraction, r.second)), Some((123456, 7)));
        assert_eq!(p("12", "%H%M").map(|r| (r.hour, r.minute)), Some((1, 2)));
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
        assert!(compile(b"%Y %z", Version::V3_14).unwrap().parse(b"2024 ").is_none());
    }
}
