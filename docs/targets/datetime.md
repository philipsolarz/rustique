STATUS: wave 1 (2026-11-01). Contract: `docs/ACCELERATION.md`. Code:
`crates/rustique-py/src/targets/datetime/`, pure algorithms in
`crates/rustique-core/src/datetime/` (with `cargo test`s). Tests:
`tests/targets/test_datetime.py`, `tests/targets/test_datetime_hypothesis.py`. Benchmarks:
`benches/targets/datetime_cg.py` (callgrind), `benches/targets/datetime_pyperf.py` (wall).

# Group `datetime`: date/time text behind `_datetime` and `time`

Parsing (`strptime`), formatting (`strftime`) and ISO formatting (`isoformat`, `str()`) of
dates. Every entry point is a C method or function of `_datetime` or `time`, so each target
swaps one `ml_meth` and falls back to the original for a few instructions. The parsing
originals call the pure-Python `_strptime`, which compiles each format into a regular
expression; that is where most of the time goes (43k–67k instructions per call), and why
this is the biggest win in the group (0.07–0.09).

## 1. Targets

| target | site (`ml_meth`) | install trigger | calling convention 3.14 / 3.15 |
|---|---|---|---|
| `datetime.datetime.strptime` | `datetime.strptime` classmethod | `_datetime` imported | VARARGS / FASTCALL |
| `datetime.date.strptime` | `date.strptime` classmethod | `_datetime` | VARARGS / FASTCALL |
| `datetime.time.strptime` | `time.strptime` classmethod | `_datetime` | VARARGS / FASTCALL |
| `time.strptime` | `time.strptime` | `_datetime` (results use its types) | VARARGS |
| `datetime.date.strftime` | `date.strftime` (also `datetime.strftime`, `format(d, spec)`, f-strings: `__format__` calls `strftime`) | `_datetime` | VARARGS+KW / FASTCALL+KW |
| `datetime.time.strftime` | `time.strftime` method | `_datetime` | VARARGS+KW / FASTCALL+KW |
| `time.strftime` | `time.strftime` | `time` | VARARGS |
| `datetime.datetime.isoformat` | `datetime.isoformat` (also `str(datetime)`) | `_datetime` | VARARGS+KW / FASTCALL+KW |
| `datetime.date.isoformat` | `date.isoformat` (also `str(date)`) | `_datetime` | NOARGS |
| `datetime.time.isoformat` | `time.isoformat` (also `str(time)`) | `_datetime` | VARARGS+KW / FASTCALL+KW |

All targets: versions 3.14 and 3.15, free-threaded builds validated, `checkable` (check
mode compares every fast-path result with the original's), `jit_safe` (`ml_meth` sites; the
registry clears JIT caches). Linux/glibc only: every target refuses elsewhere (the glibc
`strftime` and locale behaviour it reproduces is not validated on other C libraries).
`datetime.fromisoformat` is already C and has no headroom (2,191 Ir; nothing to remove):
not a target.

## 2. Exact subset (everything else runs the original)

### 2.1 strptime (`datetime`/`date`/`time` classmethods, `time.strptime`)

The original: `PyImport_Import("_strptime")`, then `_strptime._strptime_datetime_datetime(cls,
string, format)` (`_date`, `_time`, `_strptime_time`), which runs `_strptime._strptime`:
locale check (`locale.getlocale(LC_TIME)`, `time.tzname`, `time.daylight`), the regex cache
(`_regex_cache`, at most 5 entries), `re.match`, then field conversion.

The kernel (`rustique_core::datetime::strptime`) takes the fast path when **all** hold:

- receiver: exactly `datetime`/`date`/`time` (subclasses: the original's `cls(...)`);
- `string` and `format` exact `str`, ASCII; format ≤ 96 and input ≤ 128 characters;
- directives `%Y %y %m %d %H %I %M %S %f %j %b %B %a %A %p %z %%` (and `%:z` on 3.15),
  each at most once (a repeated group is CPython's `re.error`), literals (ASCII,
  case-insensitive) and whitespace runs; `%d` only with `%Y`/`%y` (3.14 warns, 3.15
  raises otherwise). Not handled: `%U %W %w %u %G %V %e %Z %c %x %X %T %F %D %R`, flags
  (`%-d`), `%O`/`%E` modifiers, stray `%`;
- **the original would take its cache hit**: `len(_regex_cache) <= 5` and
  `_regex_cache[format]` is a compiled pattern. On a miss the original compiles (and may
  warn): the kernel falls back, so `_regex_cache` and warnings evolve exactly as in
  CPython (the first call with each format runs the original);
- **`_strptime` is in its validated steady state** (§3): the cached `LocaleTime` is the C
  locale's and matches `time.tzname`/`time.daylight`, so the original would not rebuild
  its caches or call `time.tzset()`;
- `LC_TIME` is C/POSIX (glibc's `setlocale(LC_TIME, NULL)` returns `_nl_C_name`);
- no tool listens (global `sys.monitoring` events, `settrace`, `setprofile`, coverage) and
  no local events on the original's code objects (`_strptime_datetime_*`, `_strptime`,
  `_getlang`, `_parse_tz`, `locale.getlocale/_parse_localename/normalize`): tools see
  CPython's frames;
- the match exists, consumes the whole input, and every conversion `_strptime` performs
  succeeds: valid date (`datetime_date(year, month, day)`), `%j` ordinal in range, seconds
  ≤ 59 for `datetime`/`time` results, consistent `%z` colons, `|offset| < 24 h`.

How the match is computed: CPython takes sre's *first* match (backtracking: alternatives
left to right, greedy quantifiers longest first), anchored at the start; leftover input is
an error. A format is a concatenation of items whose captured text is the input they
consume, so the first match is fixed by each item's end position: every directive lists its
possible ends in sre's order (hand-written per directive and tested exhaustively against
the regex alternation tables on all inputs of ≤ 4 characters), duplicates dropped (a
repeated end cannot change what follows), and a recursive matcher takes the first
assignment for which the rest matches (budget 4,096 steps, then fall back). `%Y%m%d` on
`2024111` gives 2024-11-01 as in CPython; `%Y%m%d` on `20241` is "unconverted data"
(fallback) even though another split would consume everything.

Results are built through the datetime C API exactly as `cls(*args)` does for the exact
types: `timezone(timedelta(seconds=gmtoff, microseconds=fraction))` is
`TimeZone_FromTimeZone`, which returns the `timezone.utc` singleton for a zero offset
(`+0000`, `-0000`, `Z`) as CPython does; `time.strptime` builds the 11-field `struct_time`
(`tm_isdst=-1`, `tm_zone=None`, `tm_gmtoff` from `%z`).

### 2.2 strftime (`date.strftime`, `datetime.time.strftime`)

The original: `self.timetuple()` (a `struct_time`; for an aware datetime it first calls
`tzinfo.dst(self)`), `PyImport_ImportModuleAttrString("time", "strftime")`, a rewrite of
`%z %:z %Z %f` (and, years < 1000, `%Y %G %F %C`) in the format, calling
`tzinfo.utcoffset(self)` / `tzinfo.tzname(self)` at their first occurrence, then glibc
`wcsftime` per ASCII run. The kernel renders the same text when:

- receiver exactly `datetime`/`date` (`date.strftime`) or `time`; format an exact ASCII
  `str` (rendered output ≤ 256 bytes);
- directives `Y m d H M S y j I p a A b B h e u w F T D R C n t c x X r % f z :z Z`;
  names (`a A b B h p c x X r`) only when `LC_TIME` is C and the thread uses the global
  locale (`uselocale(0) == LC_GLOBAL_LOCALE`); numeric directives in any locale (glibc's
  are locale-independent without `%O`/`%E`); `%c` (glibc does not pad its year) only for
  years ≥ 1000;
- tzinfo `None`; an exact `datetime.timezone` (its `offset`/`name` read directly — layout
  calibrated at install against `utcoffset()`; its C methods are pure: `dst()` is None);
  or an exact C `zoneinfo.ZoneInfo` (immutable type, no instance dict) for datetimes: its C
  methods `dst`, `utcoffset`, `tzname` are called in CPython's order; they are pure, so an
  unexpected result (out-of-range offset, non-ASCII name) restarts in the original, which
  repeats the calls and raises. Any other tzinfo (pytz, dateutil, user classes) falls back
  before any call;
- `PyImport_Import` would resolve `time` to the validated module and `time.strftime` /
  `time.struct_time` are the originals: a monkeypatched `time.strftime` is honoured
  (`mock.patch("time.strftime")` sees every call).

### 2.3 `time.strftime(format, t)`

Exact 9-tuple of exact ints or exact `struct_time` (its `tm_zone` None or ASCII `str`,
`tm_gmtoff` None or an int that fits a C long), values in the ranges `checktm` accepts
without normalising (year 1000–9999, month 1–12, day 1–31, …), the same directives minus
`%f %z %:z %Z`. `time.strftime(format)` (local time) falls back.

### 2.4 isoformat (`datetime`/`date`/`time`, `str()`)

Exact receiver; `sep` an exact one-character ASCII `str`; `timespec` one of the six
interned literals (`"auto"`, `"hours"`, … — matched by identity: call sites pass literals;
other objects fall back, including unknown values, which raise); keyword names by identity
then value. Offset suffix through the same tzinfo rules as strftime (`time.isoformat`:
`None`/`timezone` only, since CPython passes `None` to `utcoffset`).

## 3. Guards and their costs

Per call (instruction counts from callgrind deltas, 3.15.1):

| guard | targets | cost |
|---|---|---|
| exact receiver / argument types | all | ~5 Ir |
| calling frame's `globals['__builtins__']` is the main interpreter's builtins (so `PyImport_Import` calls the original `__import__`; doubles as the interpreter check; no frame → fallback) | strptime, strftime methods | ~35 Ir (`PyEval_GetGlobals` + one dict lookup) |
| dependency generation unchanged (one relaxed load; revalidation by value when it moved) | strptime, strftime methods | ~2 Ir |
| `LC_TIME` pointer compare (+ `uselocale(0)` for names) | strptime; strftime with names | ~10 Ir |
| tools: `PyMonitoring_EnterScope` over all ungrouped events + local events of the 6–7 code objects the original runs | strptime | ~50 Ir |
| `_regex_cache` hit and size ≤ 5 | strptime | ~40 Ir |
| compiled-format lookup (by identity; per-thread by content for formats built at run time) | all but date/isoformat | ~5–60 Ir |

The strptime guards total ~140 Ir on a ≥ 22,000 Ir original (< 1%), within the amended
budget "≤ 40 Ir or ≤ 1% of the original's cheapest realistic call".

**Dependencies** (watched with `accel::guard::watch` on Rustique's one dict watcher; any
change bumps the group's generation, the next call revalidates by value and caches the
outcome per generation): `builtins` entries the originals read (`__import__`, `isinstance`,
`len`, `int`, `str`, `tuple`, `enumerate`, `range`, `ValueError`, `TypeError`,
`KeyError`); `sys.modules` entries `time`, `_strptime`, `locale`, `zoneinfo`, `_zoneinfo`;
`time` module `tzname`, `daylight`, `struct_time`, `strftime`, `_STRUCT_TM_ITEMS`; every
`_strptime` global the steady state reads (functions, `time`, `locale`, `re`, the datetime
types, `TimeRE`, `LocaleTime`, `_cache_lock`, `_TimeRE_cache`, `_regex_cache`,
`_CACHE_MAX_SIZE`, and builtin names that a module global could shadow); `locale` globals
(`getlocale`, `_parse_localename`, `normalize`, `_setlocale`, `locale_alias`, `LC_TIME`,
`LC_ALL`) and `locale_alias['c']`. Per call, the code objects of the functions the original
runs must be the ones fingerprinted (`f.__code__ = ...` falls back) and
`_strptime_time.__defaults__` unchanged (`time.strptime(s)`).

Revalidation also checks the cached `LocaleTime` (`_TimeRE_cache.locale_time`): `lang ==
(None, None)`, English lower-case names, `am_pm == ['am', 'pm']`, no alternative digits,
`tzname`/`daylight` equal to `time`'s. After `time.tzset()` (or a TimeRE rebuild by a
fallback call) the next call revalidates and keeps the fast path when the values are equal.

**`_strptime` module objects.** The kernels act for whichever `_strptime` `sys.modules`
holds (what `PyImport_Import` returns): each module object is fingerprinted and snapshotted
when first seen (`test_datetime` imports fresh copies; one built against `_pydatetime`
fails validation and falls back).

**Locale, thread safety.** glibc returns the `LC_TIME` name pointer without locking; the
kernel only compares it with `_nl_C_name` (static, never freed; found via
`newlocale(LC_TIME_MASK, "C", NULL)`, layout sanity-checked), so a concurrent `setlocale`
cannot make it read freed memory. "C" and "POSIX" both use `_nl_C_name`; "C.UTF-8" and
every other locale fall back.

## 4. Fingerprints and self-tests

`fingerprints.rs`: canonical code fingerprints (3.14, 3.15; identical on free-threaded
builds) of `_strptime._strptime`, `_strptime_datetime_{datetime,date,time}`,
`_strptime_time`, `_getlang`, `_parse_tz`, `_fixmonths`, `TimeRE.{__init__, __seqToRE,
pattern, compile}`, `LocaleTime.{__init__, __calc_weekday, __calc_month, __calc_am_pm,
__calc_alt_digits}`, `locale.{getlocale, _parse_localename, normalize}`; and `ml_name`/
`ml_flags`/`ml_doc` fingerprints of the 10 C methods. Self-tests at install compare kernel
and original on probe values for the strftime and isoformat targets (C originals). The
strptime originals run Python code, which `prepare` may not do inside the `sys.modules`
watcher: they rely on the fingerprints, the matcher's exhaustive tests and the
differential fuzzing.

## 5. Private state

- `_strptime._regex_cache`: identical to CPython's (fast path only on a hit).
- `_strptime._TimeRE_cache`: identical as long as nobody mutates it in place. Mutating its
  `locale_time` attributes directly (as `test_strptime`'s cache tests do through
  `_strptime._strptime_time`, not through the C entry points) is not re-checked per call:
  the kernel can skip a rebuild the original would have made. The rebuilt cache would hold
  the same values; only its identity differs.
- `_strptime._cache_lock` is not taken on the fast path (timing only).

No documented deviation in results, exceptions, warnings or calls into user code.

## 6. Measured (callgrind, 3.15.1, `benches/targets/datetime_cg.py`)

Ir per call, region-instrumented, each cell in its own process, Ir(2n) − Ir(n).

RESULTS_TABLE

## 7. Conformance

`tests/conformance/run_target_suites.py --targets datetime --modules
test.test_strptime,test.test_datetime,test.test_time`: CONFORMANCE

`tests/conformance/_datetime_hits.py` shows the kernels are exercised by those suites
(3.15.1: HITS).

## 8. Version coverage and upstream

3.14 and 3.15 (GIL and free-threaded). The regexes differ (`%z` is optional in 3.15, `%:z`
and `%n %t %D %F` are new in 3.15; 3.14 warns and 3.15 raises for `%d` without a year): the
matcher follows the running minor. CPython 3.16 merges a C fast path for numeric
`strptime` formats; formats with names (`%b`, `%a`, `%p`) and `%z` stay interpreted there,
and 3.14/3.15 keep the full gain. On 3.16 the target refuses (PyO3 0.30 does not build for
it).

## 9. Reproduce

```bash
R=~/rustique-research/devbox/rbox
$R datetime build --py 3.15.1
$R datetime run --py 3.15.1 -- python -m pytest -q tests/targets/test_datetime.py tests/targets/test_datetime_hypothesis.py
$R datetime run --py 3.15.1 -- cargo test -p rustique-core --lib datetime
$R datetime run --py 3.15.1 -- python benches/targets/datetime_cg.py -o cg.json   # report: datetime_cg.py report cg.json
$R datetime run --py 3.15.1 -- python tests/conformance/run_target_suites.py --targets datetime \
    --modules test.test_strptime,test.test_datetime,test.test_time
$R datetime run --py 3.15.1 -- python benches/workloads/run.py compare data/log_analytics data/tz_events data/csv_etl --only datetime -n 5 -p 3
```
