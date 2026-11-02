//! `strftime` for an exact subset: what `datetime`'s `wrap_strftime` (which rewrites
//! `%z`, `%:z`, `%Z`, `%f` and, for years below 1000, `%Y`/`%F`/`%C` in the format) and then
//! glibc's `strftime`/`wcsftime` (through `time.strftime`) produce together.
//!
//! The subset (anything else is `None` and CPython formats):
//! - ASCII formats whose `%` is followed by one of
//!   `Y m d H M S y j I p a A b B h e u w F T D R C n t c x X r % f z :z Z`;
//! - names (`a A b B h p c x X r`) only for the C locale's `LC_TIME` (the caller checks);
//! - years 1..=9999 (`%c`, whose year glibc does not pad, only 1000..=9999).

/// The broken-down time `time.strftime` hands to libc (after `gettmarg`/`checktm`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tm {
    pub year: i64,
    /// 1..=12
    pub month: u32,
    pub mday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Sunday = 0 (`tm_wday`).
    pub wday: u32,
    /// January 1 = 1 (`tm_yday + 1`).
    pub yday: u32,
}

/// What a format needs from its caller, in the order `wrap_strftime` computes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    /// Locale names (`%a %A %b %B %h %p %c %x %X %r`): C locale only.
    pub names: bool,
    /// `%f`
    pub micro: bool,
    /// The tzinfo calls in first-occurrence order: `b'z'` (`%z`), `b':'` (`%:z`),
    /// `b'Z'` (`%Z`); unused entries are 0.
    pub tz_order: [u8; 3],
    /// A year directive glibc does not pad (`%c`): needs 1000 <= year.
    pub plain_year: bool,
}

/// Check a format against the subset; `wrapped` = `datetime`'s strftime (with the `%z %:z %Z
/// %f` rewriting) rather than `time.strftime` directly.
pub fn scan(fmt: &[u8], wrapped: bool) -> Option<Needs> {
    let mut n = Needs::default();
    let mut tz = 0;
    let mut add_tz = |c: u8, n: &mut Needs| {
        if !n.tz_order.contains(&c) {
            n.tz_order[tz] = c;
            tz += 1;
        }
    };
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        if c >= 0x80 {
            return None;
        }
        i += 1;
        if c != b'%' {
            continue;
        }
        let d = *fmt.get(i)?;
        i += 1;
        match d {
            b'Y' | b'm' | b'd' | b'H' | b'M' | b'S' | b'y' | b'j' | b'I' | b'e' | b'u' | b'w' | b'F' | b'T' | b'D'
            | b'R' | b'C' | b'n' | b't' | b'%' => {}
            b'a' | b'A' | b'b' | b'B' | b'h' | b'p' | b'x' | b'X' | b'r' => n.names = true,
            b'c' => {
                n.names = true;
                n.plain_year = true;
            }
            b'f' if wrapped => n.micro = true,
            b'z' | b'Z' if wrapped => add_tz(d, &mut n),
            b':' if wrapped && fmt.get(i) == Some(&b'z') => {
                i += 1;
                add_tz(b':', &mut n);
            }
            _ => return None,
        }
    }
    Some(n)
}

const A_WEEKDAY: [&[u8]; 7] = [b"Sun", b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat"];
const F_WEEKDAY: [&[u8]; 7] = [b"Sunday", b"Monday", b"Tuesday", b"Wednesday", b"Thursday", b"Friday", b"Saturday"];
const A_MONTH: [&[u8]; 12] =
    [b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec"];
const F_MONTH: [&[u8]; 12] = [
    b"January",
    b"February",
    b"March",
    b"April",
    b"May",
    b"June",
    b"July",
    b"August",
    b"September",
    b"October",
    b"November",
    b"December",
];

/// The replacement texts `wrap_strftime` substitutes (ASCII; empty for a naive object).
#[derive(Clone, Copy, Debug, Default)]
pub struct TzText<'a> {
    pub z: &'a [u8],
    pub colon_z: &'a [u8],
    pub name: &'a [u8],
}

/// "00" .. "99".
const DIGITS2: [u8; 200] = {
    let mut t = [0u8; 200];
    let mut i = 0;
    while i < 100 {
        t[2 * i] = b'0' + (i / 10) as u8;
        t[2 * i + 1] = b'0' + (i % 10) as u8;
        i += 1;
    }
    t
};

/// Output buffer of bounded size: rendering reports overflow (the caller falls back).
pub struct Out {
    /// Initialised up to `len`.
    buf: [std::mem::MaybeUninit<u8>; Out::CAP],
    len: usize,
    overflow: bool,
}

impl Out {
    pub const CAP: usize = 256;

    #[inline(always)]
    pub fn new() -> Self {
        Out { buf: [std::mem::MaybeUninit::uninit(); Out::CAP], len: 0, overflow: false }
    }

    /// The rendered bytes, `None` if they did not fit.
    pub fn bytes(&self) -> Option<&[u8]> {
        // SAFETY: the first `len` bytes were written by `put`/`extend`.
        (!self.overflow).then(|| unsafe { std::slice::from_raw_parts(self.buf.as_ptr().cast::<u8>(), self.len) })
    }

    #[inline(always)]
    fn put(&mut self, b: u8) {
        if self.len < Out::CAP {
            self.buf[self.len] = std::mem::MaybeUninit::new(b);
            self.len += 1;
        } else {
            self.overflow = true;
        }
    }

    #[inline(always)]
    fn extend(&mut self, b: &[u8]) {
        if self.len + b.len() <= Out::CAP {
            let dst = self.buf[self.len..].as_mut_ptr().cast::<u8>();
            if b.len() <= 16 {
                // Short runs (separators, names, offsets): a byte loop beats a memcpy call.
                for (i, &x) in b.iter().enumerate() {
                    // SAFETY: in bounds (checked above).
                    unsafe { *dst.add(i) = x };
                }
            } else {
                // SAFETY: in bounds; u8 and MaybeUninit<u8> have the same layout.
                unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), dst, b.len()) };
            }
            self.len += b.len();
        } else {
            self.overflow = true;
        }
    }

    /// `v` in decimal, padded to `width` with `pad`.
    #[inline(always)]
    fn num(&mut self, v: u64, width: usize, pad: u8) {
        if v < 100 && width == 2 {
            if self.len + 2 > Out::CAP {
                self.overflow = true;
                return;
            }
            let i = 2 * v as usize;
            let hi = if v < 10 { pad } else { DIGITS2[i] };
            self.buf[self.len] = std::mem::MaybeUninit::new(hi);
            self.buf[self.len + 1] = std::mem::MaybeUninit::new(DIGITS2[i + 1]);
            self.len += 2;
            return;
        }
        if v < 10_000 && width == 4 {
            if self.len + 4 > Out::CAP {
                self.overflow = true;
                return;
            }
            let (a, b) = ((v / 100) as usize * 2, (v % 100) as usize * 2);
            self.buf[self.len] = std::mem::MaybeUninit::new(DIGITS2[a]);
            self.buf[self.len + 1] = std::mem::MaybeUninit::new(DIGITS2[a + 1]);
            self.buf[self.len + 2] = std::mem::MaybeUninit::new(DIGITS2[b]);
            self.buf[self.len + 3] = std::mem::MaybeUninit::new(DIGITS2[b + 1]);
            self.len += 4;
            return;
        }
        self.num_slow(v, width, pad);
    }

    /// The general case, out of line (keeps the hot directives small: LLVM otherwise
    /// hoists every division of an unrolled digit loop to the function entry).
    #[inline(never)]
    fn num_slow(&mut self, v: u64, width: usize, pad: u8) {
        let mut tmp = [0u8; 20];
        let mut i = tmp.len();
        let mut v = v;
        loop {
            i -= 1;
            tmp[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for _ in (tmp.len() - i)..width {
            self.put(pad);
        }
        self.extend(&tmp[i..]);
    }
}

impl Default for Out {
    fn default() -> Self {
        Self::new()
    }
}

/// A format checked by [`scan`], compiled to a plan: composite directives (`%F %T %D %R
/// %x %X %r %c`) expanded, adjacent literal text merged, each numeric directive resolved to
/// its field, width and pad (the format is read once, not per call).
#[derive(Clone, Debug)]
pub struct Compiled {
    fmt: Box<[u8]>,
    /// Literal bytes the ops point into (the format's literal runs and the separators of
    /// expanded composites).
    lits: Box<[u8]>,
    ops: Box<[Op]>,
    /// Formats made only of literals and fixed-width numbers (`%Y-%m-%d %H:%M:%S`): the
    /// output with the literals in place and where each number goes.
    fixed: Option<Fixed>,
    /// The plan reads `Tm::wday` or `Tm::yday` (`%a %A %u %w %j %c`).
    calendar: bool,
    pub needs: Needs,
}

/// A fixed-width plan: copy `template`, then write each number at its offset.
#[derive(Clone, Debug)]
struct Fixed {
    template: Box<[u8]>,
    slots: Box<[(u16, Field, u8, u8)]>,
}

/// One step of a compiled format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// `lits[start..end]`
    Lit(u16, u16),
    /// A number: field, width, pad byte.
    Num(Field, u8, u8),
    /// `%a %A %b %B %p` (C locale names).
    Name(u8),
    /// `%z`, `%:z`, `%Z` texts.
    Tz(u8),
    /// `%c`'s year: decimal, no padding (glibc).
    YearPlain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Year,
    Year2,
    Century,
    Month,
    Day,
    Hour,
    Hour12,
    Minute,
    Second,
    Yday,
    WdayMon1,
    Wday,
    Micro,
}

struct Builder {
    lits: Vec<u8>,
    ops: Vec<Op>,
}

impl Builder {
    fn lit(&mut self, b: &[u8]) {
        if b.is_empty() {
            return;
        }
        let start = self.lits.len();
        self.lits.extend_from_slice(b);
        let end = self.lits.len();
        if let Some(Op::Lit(_, e)) = self.ops.last_mut()
            && *e as usize == start
        {
            *e = end as u16;
            return;
        }
        self.ops.push(Op::Lit(start as u16, end as u16));
    }
    fn num(&mut self, f: Field, width: u8, pad: u8) {
        self.ops.push(Op::Num(f, width, pad));
    }
    fn dir(&mut self, d: u8) {
        match d {
            b'Y' => self.num(Field::Year, 4, b'0'),
            b'y' => self.num(Field::Year2, 2, b'0'),
            b'C' => self.num(Field::Century, 2, b'0'),
            b'm' => self.num(Field::Month, 2, b'0'),
            b'd' => self.num(Field::Day, 2, b'0'),
            b'e' => self.num(Field::Day, 2, b' '),
            b'H' => self.num(Field::Hour, 2, b'0'),
            b'I' => self.num(Field::Hour12, 2, b'0'),
            b'M' => self.num(Field::Minute, 2, b'0'),
            b'S' => self.num(Field::Second, 2, b'0'),
            b'j' => self.num(Field::Yday, 3, b'0'),
            b'u' => self.num(Field::WdayMon1, 1, b'0'),
            b'w' => self.num(Field::Wday, 1, b'0'),
            b'f' => self.num(Field::Micro, 6, b'0'),
            b'p' | b'a' | b'A' | b'b' | b'h' | b'B' => self.ops.push(Op::Name(if d == b'h' { b'b' } else { d })),
            b'n' => self.lit(b"\n"),
            b't' => self.lit(b"\t"),
            b'%' => self.lit(b"%"),
            b'z' | b'Z' | b':' => self.ops.push(Op::Tz(d)),
            b'F' => {
                self.dir(b'Y');
                self.lit(b"-");
                self.dir(b'm');
                self.lit(b"-");
                self.dir(b'd');
            }
            b'T' | b'X' => {
                self.dir(b'H');
                self.lit(b":");
                self.dir(b'M');
                self.lit(b":");
                self.dir(b'S');
            }
            b'R' => {
                self.dir(b'H');
                self.lit(b":");
                self.dir(b'M');
            }
            b'D' | b'x' => {
                self.dir(b'm');
                self.lit(b"/");
                self.dir(b'd');
                self.lit(b"/");
                self.dir(b'y');
            }
            b'r' => {
                self.dir(b'I');
                self.lit(b":");
                self.dir(b'M');
                self.lit(b":");
                self.dir(b'S');
                self.lit(b" ");
                self.dir(b'p');
            }
            b'c' => {
                // C locale d_t_fmt: "%a %b %e %H:%M:%S %Y" (glibc's %Y: no padding)
                self.dir(b'a');
                self.lit(b" ");
                self.dir(b'b');
                self.lit(b" ");
                self.dir(b'e');
                self.lit(b" ");
                self.dir(b'T');
                self.lit(b" ");
                self.ops.push(Op::YearPlain);
            }
            _ => unreachable!("scan() accepted only the directives above"),
        }
    }
}

/// [`scan`] and compile `fmt` for [`render_compiled`].
pub fn compile(fmt: &[u8], wrapped: bool) -> Option<Compiled> {
    if fmt.len() > 1024 {
        return None;
    }
    let needs = scan(fmt, wrapped)?;
    let mut b = Builder { lits: Vec::with_capacity(fmt.len() + 16), ops: Vec::new() };
    let mut i = 0;
    let mut lit = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            i += 1;
            continue;
        }
        b.lit(&fmt[lit..i]);
        let d = fmt[i + 1];
        if d == b':' {
            b.dir(b':');
            i += 3;
        } else {
            b.dir(d);
            i += 2;
        }
        lit = i;
    }
    b.lit(&fmt[lit..]);
    if b.lits.len() > u16::MAX as usize {
        return None;
    }
    let fixed = fixed_plan(&b.lits, &b.ops);
    let calendar = b
        .ops
        .iter()
        .any(|op| matches!(op, Op::Num(Field::Yday | Field::Wday | Field::WdayMon1, ..) | Op::Name(b'a' | b'A')));
    Some(Compiled { fmt: fmt.into(), lits: b.lits.into(), ops: b.ops.into(), fixed, calendar, needs })
}

/// The width every value of `f` takes with `width` (the ranges a valid `Tm` has: years
/// 1..=9999, seconds <= 61, microseconds < 10**6), or `None` when it varies.
fn fixed_width(f: Field, width: u8) -> Option<usize> {
    let max_digits = match f {
        Field::Year => 4,
        Field::Year2 | Field::Century | Field::Month | Field::Day | Field::Hour | Field::Hour12 => 2,
        Field::Minute | Field::Second => 2,
        Field::Yday => 3,
        Field::WdayMon1 | Field::Wday => 1,
        Field::Micro => 6,
    };
    (max_digits <= width as usize).then_some(width as usize)
}

fn fixed_plan(lits: &[u8], ops: &[Op]) -> Option<Fixed> {
    let mut template = Vec::new();
    let mut slots = Vec::new();
    for &op in ops {
        match op {
            Op::Lit(a, b) => template.extend_from_slice(&lits[a as usize..b as usize]),
            Op::Num(f, w, pad) => {
                let n = fixed_width(f, w)?;
                slots.push((template.len() as u16, f, n as u8, pad));
                template.resize(template.len() + n, b'0');
            }
            _ => return None,
        }
    }
    (template.len() <= Out::CAP).then(|| Fixed { template: template.into(), slots: slots.into() })
}

#[inline(always)]
fn field_value(f: Field, tm: &Tm, micro: u32) -> u64 {
    match f {
        Field::Year => tm.year as u64,
        Field::Year2 => tm.year as u64 % 100,
        Field::Century => tm.year as u64 / 100,
        Field::Month => u64::from(tm.month),
        Field::Day => u64::from(tm.mday),
        Field::Hour => u64::from(tm.hour),
        Field::Hour12 => hour12(tm.hour),
        Field::Minute => u64::from(tm.minute),
        Field::Second => u64::from(tm.second),
        Field::Yday => u64::from(tm.yday),
        Field::WdayMon1 => u64::from((tm.wday + 6) % 7 + 1),
        Field::Wday => u64::from(tm.wday),
        Field::Micro => u64::from(micro),
    }
}

impl Compiled {
    /// Does rendering read `Tm::wday`/`Tm::yday`? (Otherwise the caller may leave them 0.)
    #[inline(always)]
    pub fn uses_calendar(&self) -> bool {
        self.calendar
    }

    /// The output length of a fixed-width plan (`None`: the format has names, zone texts
    /// or variable-width numbers; use [`render_compiled`]).
    #[inline(always)]
    pub fn fixed_len(&self) -> Option<usize> {
        self.fixed.as_ref().map(|f| f.template.len())
    }

    /// Render a fixed-width plan into `out` (exactly [`Self::fixed_len`] bytes). Values
    /// out of the `Tm` ranges are the caller's to exclude (as for `render_compiled`).
    #[inline]
    pub fn render_fixed(&self, tm: &Tm, micro: u32, out: &mut [u8]) {
        let Some(fx) = &self.fixed else { return };
        out.copy_from_slice(&fx.template);
        for &(at, f, w, pad) in fx.slots.iter() {
            let mut v = field_value(f, tm, micro);
            let at = at as usize;
            let mut i = w as usize;
            // digits from the right; leading zeros become `pad` (only `%e` pads with ' ')
            loop {
                i -= 1;
                out[at + i] = b'0' + (v % 10) as u8;
                v /= 10;
                if i == 0 {
                    break;
                }
                if v == 0 {
                    while i > 0 {
                        i -= 1;
                        out[at + i] = pad;
                    }
                    break;
                }
            }
        }
    }
}

impl Compiled {
    /// The format this was compiled from.
    pub fn format(&self) -> &[u8] {
        &self.fmt
    }
}

/// Render a compiled format. `micro` is the microsecond field for `%f`.
#[inline]
pub fn render_compiled(c: &Compiled, tm: &Tm, micro: u32, tz: &TzText<'_>, out: &mut Out) {
    for &op in c.ops.iter() {
        match op {
            Op::Lit(a, b) => out.extend(&c.lits[a as usize..b as usize]),
            Op::Num(f, w, pad) => out.num(field_value(f, tm, micro), w as usize, pad),
            Op::Name(d) => out.extend(match d {
                b'p' => {
                    if tm.hour < 12 {
                        b"AM"
                    } else {
                        b"PM"
                    }
                }
                b'a' => A_WEEKDAY[tm.wday as usize],
                b'A' => F_WEEKDAY[tm.wday as usize],
                b'b' => A_MONTH[tm.month as usize - 1],
                _ => F_MONTH[tm.month as usize - 1],
            }),
            Op::Tz(d) => out.extend(match d {
                b'z' => tz.z,
                b':' => tz.colon_z,
                _ => tz.name,
            }),
            Op::YearPlain => out.num_slow(tm.year as u64, 1, b'0'),
        }
    }
}

/// Render a format that [`scan`] accepted (convenience for tests).
pub fn render(fmt: &[u8], tm: &Tm, micro: u32, tz: &TzText<'_>, out: &mut Vec<u8>) {
    let c = compile(fmt, true).or_else(|| compile(fmt, false)).expect("scanned format");
    let mut o = Out::new();
    render_compiled(&c, tm, micro, tz, &mut o);
    out.extend_from_slice(o.bytes().expect("fits"));
}

/// The reference renderer (one directive at a time, as glibc's strftime walks a format):
/// the compiled plans are tested against it.
#[cfg(test)]
fn render_reference(fmt: &[u8], tm: &Tm, micro: u32, tz: &TzText<'_>, out: &mut Out) {
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.put(fmt[i]);
            i += 1;
            continue;
        }
        if fmt[i + 1] == b':' {
            out.extend(tz.colon_z);
            i += 3;
        } else {
            directive(fmt[i + 1], tm, micro, tz, out);
            i += 2;
        }
    }
}

fn hour12(h: u32) -> u64 {
    let h = h % 12;
    u64::from(if h == 0 { 12 } else { h })
}

#[cfg(test)]
fn directive(d: u8, tm: &Tm, micro: u32, tz: &TzText<'_>, out: &mut Out) {
    let y = || tm.year as u64;
    match d {
        b'Y' => out.num(y(), 4, b'0'),
        b'y' => out.num(y() % 100, 2, b'0'),
        b'C' => out.num(y() / 100, 2, b'0'),
        b'm' => out.num(u64::from(tm.month), 2, b'0'),
        b'd' => out.num(u64::from(tm.mday), 2, b'0'),
        b'e' => out.num(u64::from(tm.mday), 2, b' '),
        b'H' => out.num(u64::from(tm.hour), 2, b'0'),
        b'I' => out.num(hour12(tm.hour), 2, b'0'),
        b'M' => out.num(u64::from(tm.minute), 2, b'0'),
        b'S' => out.num(u64::from(tm.second), 2, b'0'),
        b'j' => out.num(u64::from(tm.yday), 3, b'0'),
        b'u' => out.num(u64::from((tm.wday + 6) % 7 + 1), 1, b'0'),
        b'w' => out.num(u64::from(tm.wday), 1, b'0'),
        b'f' => out.num(u64::from(micro), 6, b'0'),
        b'p' => out.extend(if tm.hour < 12 { b"AM" } else { b"PM" }),
        b'a' => out.extend(A_WEEKDAY[tm.wday as usize]),
        b'A' => out.extend(F_WEEKDAY[tm.wday as usize]),
        b'b' | b'h' => out.extend(A_MONTH[tm.month as usize - 1]),
        b'B' => out.extend(F_MONTH[tm.month as usize - 1]),
        b'n' => out.put(b'\n'),
        b't' => out.put(b'\t'),
        b'%' => out.put(b'%'),
        b'z' => out.extend(tz.z),
        b'Z' => out.extend(tz.name),
        b'F' => {
            out.num(y(), 4, b'0');
            out.put(b'-');
            directive(b'm', tm, micro, tz, out);
            out.put(b'-');
            directive(b'd', tm, micro, tz, out);
        }
        b'T' | b'X' => {
            directive(b'H', tm, micro, tz, out);
            out.put(b':');
            directive(b'M', tm, micro, tz, out);
            out.put(b':');
            directive(b'S', tm, micro, tz, out);
        }
        b'R' => {
            directive(b'H', tm, micro, tz, out);
            out.put(b':');
            directive(b'M', tm, micro, tz, out);
        }
        b'D' | b'x' => {
            directive(b'm', tm, micro, tz, out);
            out.put(b'/');
            directive(b'd', tm, micro, tz, out);
            out.put(b'/');
            directive(b'y', tm, micro, tz, out);
        }
        b'r' => {
            directive(b'I', tm, micro, tz, out);
            out.put(b':');
            directive(b'M', tm, micro, tz, out);
            out.put(b':');
            directive(b'S', tm, micro, tz, out);
            out.put(b' ');
            directive(b'p', tm, micro, tz, out);
        }
        b'c' => {
            // C locale d_t_fmt: "%a %b %e %H:%M:%S %Y" (glibc's %Y: no padding)
            directive(b'a', tm, micro, tz, out);
            out.put(b' ');
            directive(b'b', tm, micro, tz, out);
            out.put(b' ');
            directive(b'e', tm, micro, tz, out);
            out.put(b' ');
            directive(b'T', tm, micro, tz, out);
            out.put(b' ');
            out.num_slow(y(), 1, b'0');
        }
        _ => unreachable!("scan() accepted only the directives above"),
    }
}

/// `format_utcoffset` text for an offset of `seconds` + `micros` (normalized timedelta:
/// `days` in {-1, 0}), with `sep` between the fields.
pub fn utcoffset_text(days: i32, seconds: i32, micros: i32, sep: &[u8], out: &mut Vec<u8>) {
    let mut b = [0u8; 32];
    let n = utcoffset_into(days, seconds, micros, sep, &mut b);
    out.extend_from_slice(&b[..n]);
}

/// [`utcoffset_text`] into a fixed buffer (no allocation): the length written. `sep` is
/// at most one byte in practice (`""` or `":"`); the longest text is 19 bytes.
pub fn utcoffset_into(days: i32, seconds: i32, micros: i32, sep: &[u8], out: &mut [u8; 32]) -> usize {
    let mut n = 0usize;
    let mut put = |b: &[u8]| {
        out[n..n + b.len()].copy_from_slice(b);
        n += b.len();
    };
    let two = |v: i64| [b'0' + (v / 10 % 10) as u8, b'0' + (v % 10) as u8];
    let sep = &sep[..sep.len().min(1)];
    // offset = days * 86400 + seconds (+ micros); negative iff days < 0.
    let (sign, secs, us) = if days < 0 {
        // -(offset): CPython calls delta_negative and reads the normalized fields.
        let total = -(i64::from(days) * 86_400_000_000 + i64::from(seconds) * 1_000_000 + i64::from(micros));
        (b'-', total / 1_000_000, total % 1_000_000)
    } else {
        (b'+', i64::from(seconds), i64::from(micros))
    };
    let (mm, ss) = (secs / 60, secs % 60);
    let (hh, mm) = (mm / 60, mm % 60);
    put(&[sign]);
    put(&two(hh));
    put(sep);
    put(&two(mm));
    if us != 0 || ss != 0 {
        put(sep);
        put(&two(ss));
        if us != 0 {
            put(b".");
            let mut d = [0u8; 6];
            let mut v = us;
            for x in d.iter_mut().rev() {
                *x = b'0' + (v % 10) as u8;
                v /= 10;
            }
            put(&d);
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tm() -> Tm {
        Tm { year: 2024, month: 3, mday: 5, hour: 13, minute: 4, second: 9, wday: 2, yday: 65 }
    }

    fn r(f: &str) -> String {
        let n = scan(f.as_bytes(), true).unwrap();
        let _ = n;
        let mut out = Vec::new();
        render(f.as_bytes(), &tm(), 7, &TzText { z: b"+0100", colon_z: b"+01:00", name: b"CET" }, &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn formats() {
        assert_eq!(r("%Y-%m-%d %H:%M:%S.%f"), "2024-03-05 13:04:09.000007");
        assert_eq!(r("%a %A %b %B %h %p %I %e|%j %u %w %y %C"), "Tue Tuesday Mar March Mar PM 01  5|065 2 2 24 20");
        assert_eq!(r("%F %T %D %R %x %X %r"), "2024-03-05 13:04:09 03/05/24 13:04 03/05/24 13:04:09 01:04:09 PM");
        assert_eq!(r("%c"), "Tue Mar  5 13:04:09 2024");
        assert_eq!(r("%z %:z %Z %% %n%t"), "+0100 +01:00 CET % \n\t");
    }

    #[test]
    fn plans_match_the_reference_renderer() {
        let dirs = [
            "Y", "m", "d", "H", "M", "S", "y", "j", "I", "p", "a", "A", "b", "B", "h", "e", "u", "w", "F", "T", "D",
            "R", "C", "n", "t", "c", "x", "X", "r", "%", "f", "z", ":z", "Z",
        ];
        let tz = TzText { z: b"-0130", colon_z: b"-01:30", name: b"XYZ" };
        let mut fmts: Vec<String> = Vec::new();
        for a in &dirs {
            fmts.push(format!("%{a}"));
            for b in &dirs {
                fmts.push(format!("x%{a}-%{b}y"));
                fmts.push(format!("%{a}%{b}"));
            }
        }
        fmts.push("%Y-%m-%d %H:%M:%S.%f%z %% plain".into());
        let tms = [
            tm(),
            Tm { year: 1000, month: 12, mday: 31, hour: 0, minute: 0, second: 61, wday: 6, yday: 366 },
            Tm { year: 9999, month: 1, mday: 1, hour: 12, minute: 59, second: 0, wday: 0, yday: 1 },
            Tm { year: 1, month: 7, mday: 9, hour: 23, minute: 5, second: 5, wday: 3, yday: 190 },
        ];
        for f in &fmts {
            let c = compile(f.as_bytes(), true).unwrap();
            for t in &tms {
                for micro in [0, 7, 999_999] {
                    let (mut a, mut b) = (Out::new(), Out::new());
                    render_compiled(&c, t, micro, &tz, &mut a);
                    render_reference(f.as_bytes(), t, micro, &tz, &mut b);
                    assert_eq!(a.bytes(), b.bytes(), "{f}");
                    if let Some(n) = c.fixed_len() {
                        let mut v = vec![0u8; n];
                        c.render_fixed(t, micro, &mut v);
                        assert_eq!(Some(&v[..]), b.bytes(), "fixed {f}");
                    }
                }
            }
        }
    }

    #[test]
    fn scan_rules() {
        assert!(scan(b"%Q", true).is_none());
        assert!(scan(b"%", true).is_none());
        assert!(scan(b"%-d", true).is_none());
        assert!(scan(b"%f", false).is_none());
        assert!(scan(b"%:Y", true).is_none());
        assert_eq!(scan(b"%Z %z %:z %z", true).unwrap().tz_order, [b'Z', b'z', b':']);
        assert!(scan(b"\xc3\xa9", true).is_none());
    }

    #[test]
    fn offsets() {
        let t = |d, s, u, sep: &[u8]| {
            let mut o = Vec::new();
            utcoffset_text(d, s, u, sep, &mut o);
            String::from_utf8(o).unwrap()
        };
        assert_eq!(t(0, 3600, 0, b""), "+0100");
        assert_eq!(t(-1, 82800, 0, b":"), "-01:00");
        assert_eq!(t(-1, 86399, 999_999, b":"), "-00:00:00.000001");
        assert_eq!(t(0, 5, 0, b""), "+000005");
        assert_eq!(t(-1, 0, 0, b":"), "-24:00");
    }
}
