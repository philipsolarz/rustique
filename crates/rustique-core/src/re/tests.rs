//! The port against CPython's compiler on recorded cases (`testdata/gen.py` regenerates them
//! from a CPython 3.15 build: handcrafted patterns over every construct, `re_tests.py`, a
//! sample of PyPI patterns). The full differential suites (8,078 PyPI patterns, every
//! pattern CPython's `test_re` compiles, Hypothesis) run in `tests/targets/test_re*.py`.

use super::consts::*;
use super::*;

/// The recorded case mappings of the CPython that generated the cases.
struct Recorded {
    lower: std::collections::HashMap<u32, (u32, bool)>,
    extra: ExtraCases,
}

impl Recorded {
    fn load() -> Self {
        let mut lower = std::collections::HashMap::new();
        for line in include_str!("testdata/casemap.txt").lines() {
            let mut it = line.split(' ');
            let c = u32::from_str_radix(it.next().unwrap(), 16).unwrap();
            let lo = u32::from_str_radix(it.next().unwrap(), 16).unwrap();
            let cased = it.next().unwrap() == "1";
            lower.insert(c, (lo, cased));
        }
        let mut pairs = Vec::new();
        for line in include_str!("testdata/extra.txt").lines() {
            let (k, v) = line.split_once(": ").unwrap();
            let k = u32::from_str_radix(k, 16).unwrap();
            let v = v.split(' ').map(|x| u32::from_str_radix(x, 16).unwrap()).collect();
            pairs.push((k, v));
        }
        Recorded { lower, extra: ExtraCases::new(pairs) }
    }
}

impl CaseMap for Recorded {
    fn lower(&self, c: u32) -> u32 {
        self.lower.get(&c).map_or(c, |e| e.0)
    }
    /// Only `c != upper(c)` matters (`iscased`): recorded as the `cased` bit.
    fn upper(&self, c: u32) -> u32 {
        match self.lower.get(&c) {
            Some(&(lo, true)) if lo == c => c ^ 1,
            _ => c,
        }
    }
    fn extra(&self, c: u32) -> Option<&[u32]> {
        self.extra.get(c)
    }
    fn extra_is_empty(&self) -> bool {
        self.extra.is_empty()
    }
}

fn env(case: &dyn CaseMap) -> Env<'_> {
    Env { maxrepeat: u32::MAX, maxgroups: i32::MAX as u32 / 2, case }
}

fn units(s: &str) -> Vec<u32> {
    s.chars().map(|c| c as u32).collect()
}

#[test]
fn recorded_cases_match_cpython() {
    let rec = Recorded::load();
    let env = env(&rec);
    let (mut same, mut declined, mut errors) = (0, 0, 0);
    let mut failures = Vec::new();
    for (n, line) in include_str!("testdata/cases.txt").lines().enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        assert_eq!(f.len(), 4, "line {n}");
        let istext = f[0] == "s";
        let flags: u32 = f[1].parse().unwrap();
        let pattern: Vec<u32> = if f[2].is_empty() {
            vec![]
        } else {
            f[2].split(',').map(|x| u32::from_str_radix(x, 16).unwrap()).collect()
        };
        let got = compile(&pattern, istext, flags, &env);
        let shown = || pattern.iter().map(|&c| char::from_u32(c).unwrap_or('?')).collect::<String>();
        if f[3] == "ERR" {
            match got {
                Err(Decline::Error | Decline::Warning | Decline::Unsupported) => errors += 1,
                Ok(c) => {
                    failures.push(format!("line {n}: {:?} flags {flags}: CPython raises, port gave {c:?}", shown()))
                }
            }
            continue;
        }
        let mut parts = f[3].split('|');
        let want_flags: u32 = parts.next().unwrap().parse().unwrap();
        let code: Vec<u32> = parts.next().unwrap().split(',').map(|x| x.parse().unwrap()).collect();
        let groups: u32 = parts.next().unwrap().parse().unwrap();
        let gi: Vec<(String, u32)> = parts
            .next()
            .unwrap()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|kv| {
                let (k, v) = kv.rsplit_once(':').unwrap();
                (k.to_string(), v.parse().unwrap())
            })
            .collect();
        let want = Compiled { flags: want_flags, code, groups, groupindex: gi };
        match got {
            Ok(c) if c == want => same += 1,
            Ok(c) => {
                failures.push(format!("line {n}: {:?} flags {flags}:\n  port    {c:?}\n  cpython {want:?}", shown()))
            }
            Err(Decline::Unsupported) => declined += 1,
            Err(e) => {
                failures.push(format!("line {n}: {:?} flags {flags}: CPython compiles, port says {e:?}", shown()))
            }
        }
    }
    eprintln!("same {same}, declined {declined}, errors {errors}");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert!(same > 3000, "same {same}, declined {declined}, errors {errors}");
}

#[test]
fn simple_literal() {
    let rec = Recorded::load();
    let c = compile(&units("abc"), true, 0, &env(&rec)).unwrap();
    assert_eq!(c.flags, SRE_FLAG_UNICODE);
    assert_eq!(
        c.code,
        vec![
            INFO,
            12,
            SRE_INFO_PREFIX | SRE_INFO_LITERAL,
            3,
            3,
            3,
            3,
            97,
            98,
            99,
            0,
            0,
            0,
            LITERAL,
            97,
            LITERAL,
            98,
            LITERAL,
            99,
            SUCCESS
        ]
    );
    assert_eq!(c.groups, 0);
}

#[test]
fn declines() {
    let rec = Recorded::load();
    let env = env(&rec);
    let d = |p: &str, f: u32| compile(&units(p), true, f, &env).err();
    assert_eq!(d("a", SRE_FLAG_DEBUG), Some(Decline::Unsupported));
    assert_eq!(d("a", 1), Some(Decline::Unsupported));
    assert_eq!(d("\\N{EM DASH}", 0), Some(Decline::Unsupported));
    assert_eq!(d("[[a]", 0), Some(Decline::Warning));
    assert_eq!(d("[a--b]", 0), Some(Decline::Warning));
    assert_eq!(d("(?P<n\u{e9}>a)", 0), Some(Decline::Unsupported));
    assert_eq!(d("(a", 0), Some(Decline::Error));
    assert_eq!(d(&format!("{}a{}", "(".repeat(40), ")".repeat(40)), 0), Some(Decline::Unsupported));
    let b = |p: &[u8], f: u32| compile(&p.iter().map(|&c| c as u32).collect::<Vec<_>>(), false, f, &env).err();
    assert_eq!(b(b"(?L)a", 0), Some(Decline::Unsupported));
    assert_eq!(b(b"a", SRE_FLAG_LOCALE), Some(Decline::Unsupported));
}

#[test]
fn templates() {
    let idx = |n: &str| if n == "name" { Some(1) } else { None };
    let ok = |t: &str, groups: u32| template::check(&units(t), groups, 1 << 30, &idx);
    assert!(ok("\\1", 1).is_ok());
    assert!(ok("\\2", 1).is_err());
    assert!(ok("\\g<name>-\\g<0>", 1).is_ok());
    assert!(ok("\\g<other>", 1).is_err());
    assert!(ok("\\g<1", 1).is_err());
    assert!(ok("\\n\\t\\\\x", 0).is_ok());
    assert!(ok("\\q", 0).is_err());
    assert!(ok("\\-", 0).is_ok());
    assert!(ok("\\0\\012\\101", 0).is_ok());
    assert!(ok("\\477", 0).is_err());
    assert!(ok("x\\", 0).is_err());
}
