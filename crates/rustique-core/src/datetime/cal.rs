//! Proleptic Gregorian calendar arithmetic, as `Modules/_datetimemodule.c` does it
//! (ordinal 1 = 0001-01-01, weekday 0 = Monday).

/// `date.max.toordinal()`.
pub const MAXORDINAL: i64 = 3_652_059;

const DAYS_IN_MONTH: [u8; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const DAYS_BEFORE_MONTH: [u16; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

pub fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days in `month` (1..=12) of `year`.
pub fn days_in_month(year: i64, month: u32) -> u32 {
    if month == 2 && is_leap(year) { 29 } else { u32::from(DAYS_IN_MONTH[month as usize]) }
}

/// Days in `year` before the first day of `month` (1..=12).
pub fn days_before_month(year: i64, month: u32) -> i64 {
    let mut d = i64::from(DAYS_BEFORE_MONTH[month as usize]);
    if month > 2 && is_leap(year) {
        d += 1;
    }
    d
}

/// Days before January 1 of `year` (year >= 1).
pub fn days_before_year(year: i64) -> i64 {
    let y = year - 1;
    y * 365 + y / 4 - y / 100 + y / 400
}

/// `date(year, month, day).toordinal()`; no range checks.
pub fn ymd_to_ord(year: i64, month: u32, day: u32) -> i64 {
    days_before_year(year) + days_before_month(year, month) + i64::from(day)
}

/// `date(y, m, d)` would be accepted: 1 <= y <= 9999, 1 <= m <= 12, 1 <= d <= days in month.
pub fn valid_date(year: i64, month: u32, day: u32) -> bool {
    (1..=9999).contains(&year) && (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month)
}

/// `date.fromordinal(n)` as (year, month, day); `None` outside 1..=MAXORDINAL.
pub fn ord_to_ymd(ordinal: i64) -> Option<(i64, u32, u32)> {
    if !(1..=MAXORDINAL).contains(&ordinal) {
        return None;
    }
    // CPython's ord_to_ymd.
    const DI400Y: i64 = 146_097;
    const DI100Y: i64 = 36_524;
    const DI4Y: i64 = 1_461;
    let mut n = ordinal - 1;
    let n400 = n / DI400Y;
    n %= DI400Y;
    let mut year = n400 * 400 + 1;
    let n100 = n / DI100Y;
    n %= DI100Y;
    let n4 = n / DI4Y;
    n %= DI4Y;
    let n1 = n / 365;
    n %= 365;
    year += n100 * 100 + n4 * 4 + n1;
    if n1 == 4 || n100 == 4 {
        return Some((year - 1, 12, 31));
    }
    let leap = n1 == 3 && (n4 != 24 || n100 == 3);
    let mut month = ((n + 50) >> 5) as u32;
    let mut preceding = i64::from(DAYS_BEFORE_MONTH[month as usize]) + i64::from(month > 2 && leap);
    if preceding > n {
        month -= 1;
        preceding -= i64::from(if month == 2 && leap { 29 } else { DAYS_IN_MONTH[month as usize] });
    }
    n -= preceding;
    Some((year, month, (n + 1) as u32))
}

/// Weekday of a valid date, Monday = 0.
pub fn weekday(year: i64, month: u32, day: u32) -> u32 {
    ((ymd_to_ord(year, month, day) + 6) % 7) as u32
}

/// Day of the year of a valid date, January 1 = 1.
pub fn day_of_year(year: i64, month: u32, day: u32) -> u32 {
    (days_before_month(year, month) + i64::from(day)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinal_round_trip() {
        let mut ord = 1;
        for y in 1..=9999i64 {
            for m in 1..=12u32 {
                for d in 1..=days_in_month(y, m) {
                    assert_eq!(ymd_to_ord(y, m, d), ord);
                    assert_eq!(ord_to_ymd(ord), Some((y, m, d)));
                    ord += 1;
                }
            }
        }
        assert_eq!(ord - 1, MAXORDINAL);
        assert_eq!(ord_to_ymd(0), None);
        assert_eq!(ord_to_ymd(MAXORDINAL + 1), None);
    }

    #[test]
    fn weekdays() {
        assert_eq!(weekday(2024, 3, 15), 4); // Friday
        assert_eq!(weekday(1, 1, 1), 0); // Monday
        assert_eq!(weekday(1900, 1, 1), 0);
        assert_eq!(day_of_year(2024, 12, 31), 366);
        assert_eq!(day_of_year(2023, 3, 1), 60);
    }
}
