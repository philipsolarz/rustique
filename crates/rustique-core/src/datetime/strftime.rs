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
const A_MONTH: [&[u8]; 12] = [b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec"];
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

#[inline]
fn num(out: &mut Vec<u8>, v: u64, width: usize, pad: u8) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    let mut v = v;
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for _ in (buf.len() - i)..width {
        out.push(pad);
    }
    out.extend_from_slice(&buf[i..]);
}

/// Render a format that [`scan`] accepted. `micro` is the microsecond field for `%f`.
pub fn render(fmt: &[u8], tm: &Tm, micro: u32, tz: &TzText<'_>, out: &mut Vec<u8>) {
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c != b'%' {
            out.push(c);
            continue;
        }
        let d = fmt[i];
        i += 1;
        if d == b':' {
            i += 1; // %:z
            out.extend_from_slice(tz.colon_z);
            continue;
        }
        directive(d, tm, micro, tz, out);
    }
}

fn hour12(h: u32) -> u64 {
    let h = h % 12;
    u64::from(if h == 0 { 12 } else { h })
}

fn directive(d: u8, tm: &Tm, micro: u32, tz: &TzText<'_>, out: &mut Vec<u8>) {
    let y = tm.year as u64;
    match d {
        b'Y' => num(out, y, 4, b'0'),
        b'y' => num(out, y % 100, 2, b'0'),
        b'C' => num(out, y / 100, 2, b'0'),
        b'm' => num(out, u64::from(tm.month), 2, b'0'),
        b'd' => num(out, u64::from(tm.mday), 2, b'0'),
        b'e' => num(out, u64::from(tm.mday), 2, b' '),
        b'H' => num(out, u64::from(tm.hour), 2, b'0'),
        b'I' => num(out, hour12(tm.hour), 2, b'0'),
        b'M' => num(out, u64::from(tm.minute), 2, b'0'),
        b'S' => num(out, u64::from(tm.second), 2, b'0'),
        b'j' => num(out, u64::from(tm.yday), 3, b'0'),
        b'u' => num(out, u64::from((tm.wday + 6) % 7 + 1), 1, b'0'),
        b'w' => num(out, u64::from(tm.wday), 1, b'0'),
        b'f' => num(out, u64::from(micro), 6, b'0'),
        b'p' => out.extend_from_slice(if tm.hour < 12 { b"AM" } else { b"PM" }),
        b'a' => out.extend_from_slice(A_WEEKDAY[tm.wday as usize]),
        b'A' => out.extend_from_slice(F_WEEKDAY[tm.wday as usize]),
        b'b' | b'h' => out.extend_from_slice(A_MONTH[tm.month as usize - 1]),
        b'B' => out.extend_from_slice(F_MONTH[tm.month as usize - 1]),
        b'n' => out.push(b'\n'),
        b't' => out.push(b'\t'),
        b'%' => out.push(b'%'),
        b'z' => out.extend_from_slice(tz.z),
        b'Z' => out.extend_from_slice(tz.name),
        b'F' => {
            num(out, y, 4, b'0');
            out.push(b'-');
            directive(b'm', tm, micro, tz, out);
            out.push(b'-');
            directive(b'd', tm, micro, tz, out);
        }
        b'T' | b'X' => {
            for (k, s) in [b'H', b'M', b'S'].into_iter().enumerate() {
                if k > 0 {
                    out.push(b':');
                }
                directive(s, tm, micro, tz, out);
            }
        }
        b'R' => {
            directive(b'H', tm, micro, tz, out);
            out.push(b':');
            directive(b'M', tm, micro, tz, out);
        }
        b'D' | b'x' => {
            directive(b'm', tm, micro, tz, out);
            out.push(b'/');
            directive(b'd', tm, micro, tz, out);
            out.push(b'/');
            directive(b'y', tm, micro, tz, out);
        }
        b'r' => {
            directive(b'I', tm, micro, tz, out);
            out.push(b':');
            directive(b'M', tm, micro, tz, out);
            out.push(b':');
            directive(b'S', tm, micro, tz, out);
            out.push(b' ');
            directive(b'p', tm, micro, tz, out);
        }
        b'c' => {
            // C locale d_t_fmt: "%a %b %e %H:%M:%S %Y"
            directive(b'a', tm, micro, tz, out);
            out.push(b' ');
            directive(b'b', tm, micro, tz, out);
            out.push(b' ');
            directive(b'e', tm, micro, tz, out);
            out.push(b' ');
            directive(b'T', tm, micro, tz, out);
            out.push(b' ');
            num(out, y, 1, b'0');
        }
        _ => unreachable!("scan() accepted only the directives above"),
    }
}

/// `format_utcoffset` text for an offset of `seconds` + `micros` (normalized timedelta:
/// `days` in {-1, 0}), with `sep` between the fields.
pub fn utcoffset_text(days: i32, seconds: i32, micros: i32, sep: &[u8], out: &mut Vec<u8>) {
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
    out.push(sign);
    num(out, hh as u64, 2, b'0');
    out.extend_from_slice(sep);
    num(out, mm as u64, 2, b'0');
    if us != 0 || ss != 0 {
        out.extend_from_slice(sep);
        num(out, ss as u64, 2, b'0');
        if us != 0 {
            out.push(b'.');
            num(out, us as u64, 6, b'0');
        }
    }
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
