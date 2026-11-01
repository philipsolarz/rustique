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

**Dependencies** (Rustique's one dict watcher bumps the group's generation; the next call
revalidates by value and caches the outcome per generation): the `builtins` dict, the
`time` module dict, the `_strptime` and `locale` module dicts and `locale.locale_alias`
as whole dicts (`accel::guard::watch_dict_generation`: they change rarely), and the
`sys.modules` entries `time`, `_strptime`, `locale`, `zoneinfo`, `_zoneinfo`
(`accel::guard::watch`, per key: `sys.modules` changes on every import). Revalidation
checks by identity or value what the originals read: the builtins they call (`__import__`,
`isinstance`, `len`, `int`, `str`, `tuple`, `enumerate`, `range`, `ValueError`,
`TypeError`, `KeyError`); `time.strftime`, `time.struct_time`, `time._STRUCT_TM_ITEMS`,
`time.tzname`, `time.daylight`; every `_strptime` global the steady state reads
(functions, `time`, `locale`, `re`, the datetime types, `TimeRE`, `LocaleTime`,
`_cache_lock`, `_TimeRE_cache`, `_regex_cache`, `_CACHE_MAX_SIZE`, and builtin names a
module global could shadow); `locale.getlocale`, `_parse_localename`, `normalize`,
`_setlocale`, `locale_alias['c']`, `LC_TIME`, `LC_ALL`. Per call, the code objects of the functions the original
runs must be the ones fingerprinted (`f.__code__ = ...` falls back) and
`_strptime_time.__defaults__` unchanged (`time.strptime(s)`).

Revalidation also checks the cached `LocaleTime` (`_TimeRE_cache.locale_time`): `lang ==
(None, None)`, English lower-case names, `am_pm == ['am', 'pm']`, no alternative digits,
`tzname`/`daylight` equal to `time`'s. After `time.tzset()` (or a TimeRE rebuild by a
fallback call) the next call revalidates and keeps the fast path when the values are equal.

**`_strptime` module objects.** The kernels act for whichever `_strptime` `sys.modules`
holds (what `PyImport_Import` returns): each module object is fingerprinted and snapshotted
when the kernels first see it (`test_datetime` imports fresh copies; one built against
`_pydatetime` fails validation and falls back). The first call with `_strptime` not yet
imported, and the first call with each format (a `_regex_cache` miss), run the original.

**Locale, thread safety.** glibc returns the `LC_TIME` name pointer without locking; the
kernel only compares it with `_nl_C_name` (static, never freed; found via
`newlocale(LC_TIME_MASK, "C", NULL)`, layout sanity-checked), so a concurrent `setlocale`
cannot make it read freed memory. "C" and "POSIX" both use `_nl_C_name`; "C.UTF-8" and
every other locale fall back.

## 4. Fingerprints, self-tests, install cost

Fingerprints (`fingerprints.rs`, values for 3.14 and 3.15, identical on free-threaded
builds) use the group's cheap canonical hash (`fastfp.rs`: the fields of the contract's
canonical code fingerprint — bytecode, names, local names and kinds, flags, argument counts,
exception table, constants recursively, defaults — read from the code object and hashed
8 bytes at a time; ~10k Ir per function instead of ~250k through the generic
`fingerprint_code`):

- the 10 C methods (`ml_name`, `ml_flags`, `ml_doc`): checked in `prepare`; a mismatch
  refuses that target (category `fingerprint`);
- the Python code the strptime kernels reimplement: `_strptime._strptime`,
  `_strptime_datetime_{datetime,date,time}`, `_strptime_time`, `_getlang`, `_parse_tz`,
  `_fixmonths`, `TimeRE.{__init__, __seqToRE, pattern, compile}`,
  `locale.{getlocale, _parse_localename, normalize}` (`LocaleTime`'s code is not: its
  output is checked by value). Checked when the kernels first see a `_strptime` module
  object (the call after the one that imported it), together with the dependency
  snapshot; a mismatch makes the strptime kernels fall back (reason in
  `_accel._datetime_debug()["strptime_last_failure"]`), it does not show as a refusal in
  `status()` (open point for the registry: a "degraded" state).
- `benches/targets/_dt_fp_report.py` prints the current values.

Self-tests (kernel vs original on a probe) only for `time.strftime` and
`date.isoformat` (one call each); the strptime originals run Python code, which `prepare`
may not run inside the `sys.modules` watcher. Behaviour is covered by CPython's suites
(§7), the matcher's exhaustive tests against the regex tables, and the differential
fuzzing.

Install cost (`benches/targets/datetime_install.py`, callgrind, 3.15.1, wave1 5a690e2,
quiet `_accel.enable`; the registry's own baseline, `enable(only=["_selftest.meth"])`, is
24,700 Ir):

| region | Ir | group's share |
|---|---:|---:|
| `enable(only=["datetime"])`, `datetime`/`_strptime`/`zoneinfo` imported (10 targets install) | 169,400 | ~145,000 |
| `enable(only=["datetime"])`, nothing imported (`time.strftime` installs, 9 pending) | 109,700 | ~85,000 |
| `import datetime` after `enable()` (9 targets install lazily) | 433,700 → 539,600 | +105,900 (+24.4%) |
| the strptime call after `_strptime` was imported (captures it once) | 41,900 → 287,100 | +245,200, once (2.9% of `import _strptime`, 8.6M Ir) |

Before trimming the first row was 6.35M Ir (the generic fingerprints of `_strptime`, four
times, plus a self-test per target). The budget (≤ 0.1M Ir per group at `enable()`, ≤ 5% of
the triggering import) is still exceeded by the first and third rows. Of the 145k: the
registry's local-monitoring calibration for module targets (~26k plus a probe compile),
`phdr` writability checks of 10 sites (~14k), the group's environment capture (builtins,
`time`, glibc locale, layout calibrations: ~40k), 10 cheap C fingerprints (~11k), two
probes, and ~2–3k of registry work per target. `import datetime` pays the same per-target
work against a cheap import (434k Ir); folding the 10 targets into fewer (one per entry
family) or sharing the registry's per-target overhead would be the next steps.

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

| cell | CPython Ir | Rustique Ir | ratio | ratio, JIT on |
|---|---:|---:|---:|---:|
| strptime log %d/%b/%Y:%H:%M:%S %z | 62633 | 4929 | 0.079 |  |
| strptime %Y-%m-%d %H:%M:%S | 42774 | 3108 | 0.073 |  |
| strptime iso %f%z | 66758 | 4546 | 0.068 |  |
| strptime %m/%d/%Y %I:%M %p | 45728 | 3344 | 0.073 |  |
| date.strptime %Y-%m-%d | 31399 | 2366 | 0.075 |  |
| time.strptime %Y-%m-%d %H:%M:%S | 43386 | 3749 | 0.086 |  |
| strptime fallback: subclass receiver | 42944 | 42975 | 1.001 | 0.997 |
| strptime fallback: unsupported %U | 39213 | 39545 | 1.008 | 1.007 |
| strptime fallback: no match (error) | 22602 | 23460 | 1.038 | 1.040 |
| strptime fallback: LC_TIME en_US | 56587 | 56767 | 1.003 | 1.006 |
| strptime fallback: dependency changed (deps) | 42700 | 42993 | 1.007 | 1.003 |
| strptime fallback: _regex_cache miss (deopt) | 103021 | 103759 | 1.007 | 1.006 |
| strptime fallback: tracing tool (tools) | 43723 | 43998 | 1.006 | 1.006 |
| strftime %Y-%m-%d %H:%M:%S | 17185 | 1874 | 0.109 |  |
| strftime %Y-%m (utc) | 16820 | 1423 | 0.085 |  |
| strftime %Y-%m-%d %H:00 (fixed offset) | 17727 | 1729 | 0.098 |  |
| strftime %a %d %b %Y %H:%M %Z (ZoneInfo) | 21737 | 4393 | 0.202 |  |
| strftime %A (ZoneInfo) | 16551 | 2343 | 0.142 |  |
| strftime %a, %d %b %Y %H:%M:%S GMT (utc) | 18954 | 2173 | 0.115 |  |
| date.strftime %Y-%m-%d | 15787 | 1348 | 0.085 |  |
| time.strftime method %H:%M:%S.%f | 11878 | 1492 | 0.126 |  |
| f-string {dt:%Y-%m-%d} | 16296 | 1835 | 0.113 |  |
| time.strftime(fmt, struct_time) | 5346 | 1972 | 0.369 |  |
| time.strftime(fmt, tuple) | 5267 | 1945 | 0.369 |  |
| time.strftime fallback: year < 1000 | 6146 | 6354 | 1.034 | 1.040 |
| strftime fallback: subclass | 17221 | 13834 | 0.803 | 0.811 |
| strftime fallback: unsupported %U | 15644 | 16091 | 1.029 | 1.020 |
| strftime fallback: names in en_US | 15479 | 16069 | 1.038 | 1.023 |
| strftime numeric in en_US | 17095 | 1874 | 0.110 |  |
| isoformat() | 6350 | 747 | 0.118 |  |
| isoformat() utc | 8775 | 1247 | 0.142 |  |
| isoformat(timespec='minutes') ZoneInfo | 8600 | 2525 | 0.294 |  |
| str(dt) | 6663 | 1075 | 0.161 |  |
| date.isoformat() | 3079 | 512 | 0.166 |  |
| time.isoformat() | 3908 | 676 | 0.173 |  |
| isoformat(sep=' ', timespec='seconds') | 6030 | 906 | 0.150 |  |
| isoformat fallback: subclass | 5616 | 5667 | 1.009 | 1.011 |
| isoformat fallback: unknown timespec (error) | 2660 | 2772 | 1.042 | 1.018 |
| fromisoformat full tz | 2172 | 2172 | 1.000 |  |

Other builds (`matrix_final`, same cells): 3.15.1t strptime 0.067–0.085, strftime
0.084–0.39, isoformat 0.11–0.28, fallback max 1.044; 3.14.9 strptime 0.066–0.082,
strftime 0.089–0.37, isoformat 0.12–0.28, fallback max 1.044; 3.14.9t strptime 0.065–0.082, strftime
0.087–0.39, isoformat 0.12–0.28, fallback max 1.045 (3.14 rows re-measured after the keyword fix, `matrix_final/rerun314.txt`).

Fast paths: strptime 0.068–0.086 (the original runs `_strptime`'s Python code: 31k–67k Ir),
strftime 0.086–0.20 (0.37 for `time.strftime`, whose original is already a thin C call
into `wcsftime`), isoformat 0.12–0.29. ZoneInfo cells keep the cost of calling its C
methods (`dst`, `utcoffset`, `tzname`) through the method protocol.

Fallback and reject paths, every one ≤ 1.05 (max 1.042; JIT on, measured on the previous build: ≤ 1.04): subclass receivers, unsupported
directives, errors (the original raises), non-C `LC_TIME`, a changed dependency (`deps`), a
`_regex_cache` miss (the original compiles), a tool listening (`tools`), years < 1000 in
`time.strftime`, unknown `timespec`. "strftime fallback: subclass" is 0.80 because the
subclass's original `strftime` calls `time.strftime`, which is accelerated.

**Workloads** (`benches/workloads/run.py compare … --only datetime -n 5 -p 3`, 3.15.1,
wall time on the shared box, digests identical):

| workload | plain | enabled | ratio |
|---|---:|---:|---:|
| data/log_analytics (`strptime` `%d/%b/%Y:%H:%M:%S %z`, `strftime`, `isoformat`) | 759 ms | 468 ms | **0.617** |
| data/tz_events (`strptime` ×3 formats, ZoneInfo `strftime` `%A` / `%a %d %b %Y %H:%M %Z`, `isoformat`) | 545 ms | 323 ms | **0.593** |
| data/csv_etl (`strftime('%Y-%m')`, `isoformat`) | 390 ms | 321 ms | **0.823** |
| geometric mean | | | 0.670 |

**Bulk wall time** (`benches/targets/datetime_pyperf.py --fast`, pyperf, 3.15.1, shared box:
only ≥ 1.5× claims), 1,000 calls per loop:

| benchmark | plain | Rustique | ratio |
|---|---:|---:|---:|
| `datetime.strptime` log format with `%b` and `%z` | 4.73 ms | 0.278 ms | 0.059 |
| `datetime.strptime` `%Y-%m-%d %H:%M:%S` | 3.29 ms | 0.165 ms | 0.050 |
| `strftime("%a %d %b %Y %H:%M:%S %z")`, fixed-offset tz | 1.73 ms | 0.188 ms | 0.109 |
| `time.strftime("%Y-%m-%d %H:%M:%S", struct_time)` | 0.296 ms | 0.118 ms | 0.40 |

## 7. Conformance

`tests/conformance/run_target_suites.py --targets datetime --modules
test.test_strptime,test.test_datetime,test.test_time` (per-test-id outcomes, plain vs
enabled, fresh processes), every target installed:

| build | test_strptime | test_datetime (C and `_Pure` variants) | test_time | differences | full `pytest -q` |
|---|---|---|---|---:|---|
| 3.15.1 | 49 pass, 94 skip | 3614 pass, 54 skip | 49 pass, 16 skip | 0 | 4133 passed, 10 skipped |
| 3.15.1t | same | same | same | 0 | 4136 passed, 7 skipped |
| 3.14.9 | 46 pass, 94 skip | 3560 pass, 54 skip | 48 pass, 16 skip | 0 | 4132 passed, 11 skipped |
| 3.14.9t | 46 pass, 94 skip | 3560 pass, 54 skip | 48 pass, 16 skip | 0 | 4135 passed, 8 skipped |

(Skips are the suites' own: `test_strptime` runs most cases under `run_with_locales`
for locales the box does not have.)

`tests/conformance/_datetime_hits.py` shows the kernels are exercised by those suites
(3.15.1 fast-path hits: `datetime.strptime` 66, `date.strptime` 6, `time.strptime` (class)
14, `time.strptime` 32, `date.strftime` 217, `time.strftime` (method) 28, `time.strftime`
3184, `datetime.isoformat` 513, `date.isoformat` 19, `time.isoformat` 329; most fallbacks
are the suites' subclasses, custom tzinfo classes and first calls per format).

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
