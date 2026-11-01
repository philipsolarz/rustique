//! Decimal text of machine integers (what `int.__repr__` prints for them).

const DIGITS2: &[u8; 200] = b"00010203040506070809101112131415161718192021222324252627282930313233343536373839\
404142434445464748495051525354555657585960616263646566676869707172737475767778798081828384858687888990\
919293949596979899";

/// The decimal representation of `v`, written at the end of `buf`.
#[inline]
pub fn format_i64(buf: &mut [u8; 20], v: i64) -> &[u8] {
    let mut u = v.unsigned_abs();
    let mut i = buf.len();
    while u >= 100 {
        let r = (u % 100) as usize;
        u /= 100;
        i -= 2;
        buf[i..i + 2].copy_from_slice(&DIGITS2[2 * r..2 * r + 2]);
    }
    if u >= 10 {
        i -= 2;
        buf[i..i + 2].copy_from_slice(&DIGITS2[2 * u as usize..2 * u as usize + 2]);
    } else {
        i -= 1;
        buf[i] = b'0' + u as u8;
    }
    if v < 0 {
        i -= 1;
        buf[i] = b'-';
    }
    &buf[i..]
}

/// Append the decimal representation of `v` to `out`.
#[inline]
pub fn push_i64(out: &mut Vec<u8>, v: i64) {
    if v < 0 {
        out.push(b'-');
    }
    push_u64(out, v.unsigned_abs());
}

/// Append the decimal representation of `v` to `out`.
#[inline]
pub fn push_u64(out: &mut Vec<u8>, mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while v >= 100 {
        let r = (v % 100) as usize;
        v /= 100;
        i -= 2;
        buf[i..i + 2].copy_from_slice(&DIGITS2[2 * r..2 * r + 2]);
    }
    if v >= 10 {
        i -= 2;
        buf[i..i + 2].copy_from_slice(&DIGITS2[2 * v as usize..2 * v as usize + 2]);
    } else {
        i -= 1;
        buf[i] = b'0' + v as u8;
    }
    out.extend_from_slice(&buf[i..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn edges() {
        for v in [0i64, 1, -1, 9, 10, 99, 100, 101, -100, 12345, i64::MAX, i64::MIN, 1 << 53] {
            let mut out = Vec::new();
            push_i64(&mut out, v);
            assert_eq!(String::from_utf8(out).unwrap(), v.to_string());
        }
        let mut b = [0u8; 20];
        assert_eq!(format_i64(&mut b, i64::MIN), i64::MIN.to_string().as_bytes());
        let mut out = Vec::new();
        push_u64(&mut out, u64::MAX);
        assert_eq!(String::from_utf8(out).unwrap(), u64::MAX.to_string());
    }

    proptest! {
        #[test]
        fn same_as_std(v in any::<i64>()) {
            let mut out = Vec::new();
            push_i64(&mut out, v);
            prop_assert_eq!(String::from_utf8(out).unwrap(), v.to_string());
            let mut b = [0u8; 20];
            prop_assert_eq!(std::str::from_utf8(format_i64(&mut b, v)).unwrap(), v.to_string());
        }
    }
}
