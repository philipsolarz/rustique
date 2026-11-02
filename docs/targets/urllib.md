STATUS: wave 1, group `urllib` (workstream utils). Contract: `docs/ACCELERATION.md`.
Code: `crates/rustique-py/src/targets/urllib/`, `crates/rustique-core/src/urlparse/`
(`cargo test`). Tests: `tests/targets/test_urllib.py`, `tests/targets/test_utils_mechanics.py`.
Benchmarks: `benches/targets/urllib_cells.py` (callgrind), `utils_pyperf.py urllib`.

# Targets `urllib.parse.quote`, `quote_plus`, `unquote`, `unquote_plus`, `parse_qsl`, `parse_qs`, `_urlsplit`

Vectorcall kernels on the module's function objects: the 63–90% of call sites that hold
`from urllib.parse import quote` reach them (tested). `urlsplit` is wrapped in
`functools.lru_cache`, whose `cache_info()` is public: the C wrapper stays CPython's and
the kernel sits behind it, on `_urlsplit` — the uncached work under `urlsplit` (cache
misses), `urlparse` (never cached) and `urljoin`. All seven are side-effect free and
`checkable`.

| target | validated | free-threaded | checkable |
|---|---|---|---|
| all seven | 3.14, 3.15 | yes | yes |

The seven functions (and the helpers they call) are identical in 3.14.9 and 3.15.1 (the
3.15 `missing_as_none` changes are in `urlsplit`/`urlparse`/`urlunsplit`, which stay
CPython's).

## 1. Exactness

- **`quote(string, safe='/', encoding=None, errors=None)`**: `str` → UTF-8 (`strict`; text
  with a lone surrogate falls back), or exact `bytes`; `safe` `str` (non-ASCII ignored, as
  `safe.encode('ascii', 'ignore')`) or `bytes`; `_ALWAYS_SAFE` bytes and `safe` bytes kept,
  others `%XX` (upper-case hex). `quote('')` returns its argument. `quote_plus`: if the input
  has a space, `safe + ' '` then spaces to `+`; else `quote` itself.
- **`unquote(string, encoding='utf-8', errors='replace')`**: no `%` → the argument itself;
  otherwise CPython's split into ASCII runs and non-ASCII runs (`_asciire`), each ASCII run
  percent-decoded with `_unquote_impl`'s rules (`%` + two hex digits of either case, else a
  literal `%`) and decoded **by CPython's own UTF-8 decoder** (`PyUnicode_DecodeUTF8` with
  `replace`, `strict`, `ignore` or `surrogateescape`), non-ASCII runs kept. A strict decode
  error falls back (nothing observable happened). `unquote_plus` replaces `+` first.
- **`parse_qsl`/`parse_qs`**: `qs.split(separator)`, `partition('=')`, blank handling,
  `max_num_fields`, then `unquote_plus` on each name and value; names/values are slices of
  `qs` made with `PyUnicode_Substring` (CPython's slicing), so a single field returns `qs`
  itself where CPython does. `parse_qs` builds the dict of lists in CPython's order.
- **`_urlsplit(url, scheme=None, allow_fragments=True)`**: `lstrip` of C0 controls and
  space, removal of tab/CR/LF (`_UNSAFE_URL_BYTES_TO_REMOVE`, a mutable list checked per
  call), the scheme rule (`url[0]` an ASCII letter, all of `url[:i]` in `scheme_chars`,
  lower-cased by calling `str.lower`), `_splitnetloc`, `#`/`?` splits; components are
  slices of the cleaned url (the argument itself when nothing was stripped), `None` where
  CPython returns `None`. Netlocs with brackets (`ValueError` or `_check_bracketed_netloc`
  via `ipaddress`) and non-ASCII netlocs (`_checknetloc`'s NFKC check) fall back.

- **Result identity** (wave 2: the kernels build every result object themselves, so they
  follow CPython's builders exactly, `test_urllib_identity.py`): slices and `bytes.decode()`
  give the Latin-1 singleton for one character (`quote('a')`, `parse_qsl('a=b')` names and
  values, `_urlsplit` components); `''.join` of several parts and `str.replace` give a new
  object even for one character (`unquote('%41')`, `unquote_plus('+')`, `quote_plus(' ')`,
  a url whose tab/CR/LF were removed); a one-byte input that needs quoting returns the
  `_Quoter` cache's shared `'%XX'` object in CPython (`quote(' ') is quote(' ')`), so that
  call falls back. The wave-1 kernels returned singletons in the first two cases and fresh
  objects in the third (5 identity differences, found by the new tests, now fixed).

## 2. Exact subset and fallbacks

Accelerated: exact `str` arguments (and `bytes` for `quote`/`quote_plus`); default codecs
(UTF-8; `encoding` given as `'utf-8'`/`'utf8'` accepted); `parse_qsl`/`parse_qs` with a
`str` separator, `strict_parsing` false, `keep_blank_values`/`allow_fragments`
`bool`/`int`/`None`, `max_num_fields` `None` or int; inputs below 200,000 bytes for `quote`
(CPython switches to a chunked join with `math.isqrt`).

Falls back (`args`): str/bytes subclasses, `bytearray`, other codecs or error handlers,
`strict_parsing=True`, bytes query strings, every error (bad separator, too many fields,
encode errors), bracketed or non-ASCII netlocs. `deps`: a replaced dependency (§3).

## 3. Dependencies

Pinned once for the whole group (`GroupSpec`, flag `URLLIB_DEPS`; replacing any of them
suspends all seven targets until it is back), identity, LOAD_GLOBAL semantics:
`quote_from_bytes`, `_ALWAYS_SAFE_BYTES`,
`_byte_quoter_factory` (quote, quote_plus), `quote` (quote_plus), `unquote`,
`_unquote_impl`, `_generate_unquoted_parts`, `_asciire` (unquote*, parse_qs*),
`unquote_plus` (parse_qsl), `parse_qsl` (parse_qs), `_WHATWG_C0_CONTROL_OR_SPACE`,
`_UNSAFE_URL_BYTES_TO_REMOVE` (+ contents per call), `scheme_chars`, `_splitnetloc`,
`_check_bracketed_netloc`, `_checknetloc` (_urlsplit), and the builtins they read.
Constants' contents are checked at install.

**Private state skipped:** `_byte_quoter_factory`'s `lru_cache` entries and the `_Quoter`
dicts `quote` would create; the lazily built `_hextobyte` table (`unquote` would build it on
its first call with a `%`). The kernels keep per-thread scratch buffers (Rust memory, not
Python objects; reused between calls, released above 64 Ki elements), invisible to any API.

## 4. Version coverage

3.14.9 and 3.15.1 and their free-threaded builds; refused elsewhere and on fingerprint or
constant mismatch.

Private CPython symbols and layouts: none (public C API only).

## 5. Conformance

`python tests/conformance/run_target_suites.py --targets urllib --modules ...` (CPython's own test
modules from `$CPYTHON_SRC`, plain vs `enable(only=["urllib"])` before the tests import the
module, per test id; every target asserted `installed`), on 3.15.1, 3.15.1t, 3.14.9 and
3.14.9t: `test.test_urlparse`: 78 pass, 5 skip (3.15) / 74 pass, 5 skip (3.14); `test.test_urllib`: 104 pass, 2 skip — **identical outcome sets on all four builds, 0 differences**
(`benches/targets/utils_matrix.sh`, which also runs `tests/targets` and the whole
`pytest -q`). The runner normalizes object addresses in subTest ids (`0x…`), which differ
between processes (`test_urlparse` parametrizes with function reprs).

Wave 2 (allocation-free kernels, merged base `ws2/text` c9fb335): `test.test_urlparse` 78 pass,
5 skip (3.15) / 74 pass, 5 skip (3.14) and `test.test_urllib` 104 pass, 2 skip, 0 differences
on 3.15.1, 3.15.1t, 3.14.9 and 3.14.9t; the whole `pytest -q` green on the four builds,
including `test_urllib_identity.py` (identity-level differential tests incl. one-character
singletons and the `_Quoter` cache object, Hypothesis, 8-thread test).

## 6. Measurements

### 6.0 Wave 2: allocation-free kernels (Phase 2, `ws2/text-tw`)

What changed (w2-floors §1: 15–30% of every kernel was Rust heap traffic plus UCS-4 copies;
w2-threads §3.1: `quote`'s `Vec<u8>` growth went through glibc's arena lock, CPU per call
×19.6 at 8 threads on 3.15.1t):

- **`quote`/`quote_plus`**: two passes over the argument's PEP 393 buffer (UTF-8 encoded on
  the fly; a lone surrogate falls back before anything is built): the output length, then
  the `%XX` text written straight into the result `str`. No allocation besides the result.
- **`unquote`/`unquote_plus`**: code units read in place; the percent-decoded bytes of an
  ASCII run go to a per-thread scratch buffer, then either straight into the result (ASCII)
  or through CPython's UTF-8 decoder, whose new string is returned as is when it is the
  whole result.
- **`parse_qsl`/`parse_qs`**: fields and `=` found on the buffer; a name or value without
  `%`/`+` is CPython's slice (`PyUnicode_Substring`), others are decoded from the range
  directly; pairs go straight into the result list/dict.
- **`_urlsplit`**: the cleaned url is the argument at the strip offset (a copy only when
  tab/CR/LF are removed); components are slices of it.
- **Identity** (§1) now follows CPython's builders, including one-character results and the
  `_Quoter` cache object (5 wave-1 identity differences fixed).

Instructions per call (`python benches/targets/utils_cg.py urllib_cells`, callgrind, 3.15.1,
dev box, merged base `ws2/text` c9fb335; wave-1 column: the same cells on `ws2/text` bc2ce1a
before this work). **M**.

| cell | CPython Ir | wave 1 Ir (ratio) | wave 2 Ir | ratio |
|---|---:|---:|---:|---:|
| `quote(path)` | 15,373 | 3,173 (0.207) | 1,824 | **0.119** |
| `quote('abc') [all safe]` | 4,790 | 1,639 (0.340) | 1,000 | **0.209** |
| `quote_plus('a b&c')` | 11,136 | 1,804 (0.161) | 1,110 | **0.100** |
| `unquote('a%20b%C3%A9')` | 21,788 | 3,708 (0.171) | 2,040 | **0.094** |
| `unquote('plain') [no %]` | 1,365 | 823 (0.603) | 562 | **0.411** |
| `unquote_plus('a+b')` | 2,457 | 1,399 (0.569) | 934 | **0.380** |
| `parse_qsl(5 fields)` | 38,881 | 12,905 (0.332) | 5,609 | **0.144** |
| `parse_qs(5 fields)` | 44,499 | 14,753 (0.332) | 7,099 | **0.160** |
| `urlparse(URL) [uncached]` | 27,082 | 15,866 (0.586) | 12,833 | **0.474** |
| `urlsplit(new url) [lru miss]` | 25,497 | 14,283 (0.560) | 11,375 | **0.446** |
| `urlsplit(URL) [lru hit]` | 975 | 975 (1.000) | 975 | **1.000** |
| `_urlsplit(URL)` | 18,386 | 7,075 (0.385) | 4,017 | **0.218** |
| `urljoin(base, rel)` | 45,185 | 29,899 (0.662) | 26,193 | **0.580** |
| `quote(path, UCS-2)` | 15,056 | — | 1,902 | **0.126** |
| `unquote(mixed, UCS-2)` | 24,017 | — | 2,734 | **0.114** |
| `quote(bytes)` | 8,504 | — | 1,025 | **0.120** |
| `quote(' ') [fallback: _Quoter cache object]` | 6,948 | — | 6,948 | **1.000** |
| `quote(str subclass) [fallback]` | 8,437 | 8,447 (1.001) | 8,437 | **1.000** |
| `quote(s, encoding='latin-1') [fallback]` | 14,752 | 14,772 (1.003) | 14,743 | **0.999** |
| `unquote(s, encoding='latin-1') [fallback]` | 16,645 | 16,639 (0.985) | 16,579 | **0.996** |
| `urlparse('http://[::1]/') [fallback]` | 50,440 | 50,538 (1.001) | 50,460 | **1.000** |
| `parse_qsl(bytes) [fallback]` | 23,748 | 23,772 (1.001) | 23,762 | **1.001** |

`quote` 1.7×, `unquote` 1.8×, `parse_qsl`/`parse_qs` 2.1–2.3×, `_urlsplit` 1.8× fewer
instructions than wave 1; `_urlsplit` (4.0k) is now mostly the five component objects, the
tuple and the `str.lower()` call. Every fallback cell is ≤ 1.001.

**First 64 fallback calls** (fresh process, one hit first; `utils_first64.py` method):
`quote(str subclass)` 1.046 (+433 Ir/call, as wave 1); `quote(' ')` / `quote('%')` (a one-byte
input that needs quoting: CPython returns its `_Quoter` cache's shared object, so the kernel
falls back) **1.069** (+528 Ir/call on a 7.7k-Ir call) — a first-64 exceedance (§5.6 of the
contract; listed for DECISIONS): after 64 consecutive such calls the target is suspended
(1.000), and mixed with any other `quote` call the hit pays for it many times over.

**Free-threaded scaling (8-thread rule)** — `python benches/targets/tw_scaling.py` plain
and `--enable` (fresh processes, `taskset` 8 cores, `-t 1,8 -r 3`), 3.15.1t, CPU time per
call (sum of the workers' `thread_time()` / calls), merged base. **M**.

| cell | inputs | plain ns (1 thr) | enabled/plain (1 thr) | plain CPU ns x8/x1 | enabled CPU ns x8/x1 | enabled/plain CPU (8 thr) | plain eff | enabled eff | enabled/plain wall (8 thr) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| textwrap.fill | local | 33016 | 0.034 | 1.02 | 1.02 | 0.031 | 0.31 | 0.28 | 0.037 |
| textwrap.fill | shared | 33952 | 0.032 | 1.00 | 1.06 | 0.034 | 0.29 | 0.31 | 0.031 |
| textwrap.wrap | local | 71089 | 0.040 | 1.01 | 0.99 | 0.038 | 0.34 | 0.32 | 0.042 |
| textwrap.wrap | shared | 69400 | 0.040 | 1.01 | 0.99 | 0.038 | 0.29 | 0.33 | 0.035 |
| textwrap.shorten | local | 7267 | 0.063 | 1.00 | 0.98 | 0.059 | 0.29 | 0.34 | 0.055 |
| textwrap.shorten | shared | 7213 | 0.063 | 1.02 | 0.98 | 0.058 | 0.35 | 0.25 | 0.090 |
| textwrap.dedent | local | 3485 | 0.235 | 1.01 | 1.01 | 0.218 | 0.37 | 0.28 | 0.313 |
| textwrap.dedent | shared | 3491 | 0.225 | 1.02 | 1.24 | 0.273 | 0.30 | 0.23 | 0.298 |
| textwrap.indent | local | 2640 | 0.184 | 1.02 | 1.00 | 0.183 | 0.30 | 0.32 | 0.172 |
| textwrap.indent | shared | 2518 | 0.195 | 1.04 | 1.21 | 0.220 | 0.34 | 0.26 | 0.254 |
| urllib.quote | local | 1139 | 0.137 | 2.56 | 1.01 | 0.053 | 0.16 | 0.31 | 0.071 |
| urllib.quote | shared | 1158 | 0.131 | 2.17 | 1.01 | 0.061 | 0.16 | 0.26 | 0.080 |
| urllib.quote_plus | local | 957 | 0.112 | 2.12 | 1.05 | 0.053 | 0.18 | 0.21 | 0.096 |
| urllib.quote_plus | shared | 1007 | 0.107 | 2.10 | 1.05 | 0.052 | 0.18 | 0.32 | 0.059 |
| urllib.unquote | local | 1989 | 0.075 | 1.75 | 1.01 | 0.040 | 0.17 | 0.29 | 0.045 |
| urllib.unquote | shared | 2035 | 0.071 | 1.68 | 1.01 | 0.042 | 0.17 | 0.29 | 0.041 |
| urllib.parse_qsl | local | 2882 | 0.124 | 1.02 | 1.03 | 0.140 | 0.22 | 0.22 | 0.124 |
| urllib.parse_qsl | shared | 2583 | 0.160 | 1.00 | 1.02 | 0.142 | 0.23 | 0.37 | 0.098 |
| urllib.parse_qs | local | 3322 | 0.150 | 1.00 | 1.00 | 0.154 | 0.29 | 0.26 | 0.167 |
| urllib.parse_qs | shared | 3531 | 0.148 | 1.00 | 0.99 | 0.156 | 0.33 | 0.20 | 0.236 |
| urllib._urlsplit | local | 1344 | 0.188 | 1.02 | 1.02 | 0.208 | 0.35 | 0.22 | 0.298 |
| urllib._urlsplit | shared | 1273 | 0.197 | 1.02 | 1.02 | 0.205 | 0.26 | 0.23 | 0.222 |

Every kernel of both groups: enabled/plain CPU per call at 8 threads **0.04–0.27** (rule:
≤ 1.05). `quote`'s own CPU per call grows ×1.01 from 1 to 8 threads (wave 1: ×19.6, the glibc
arena lock); `quote`/`quote_plus`/`unquote` now scale better than CPython itself (×2.1–2.6:
the original's `_Quoter`/`lru_cache` lookups on shared objects). Shared-input `dedent`/
`indent` grow ×1.2 (CPython ×1.0) and stay at 0.22–0.27 of the original. Wall-time
efficiency columns are not a claim (shared, loaded box).

**Workloads** — user-space instructions per steady-state `run()` (`python benches/targets/text_wl.py`,
`perf stat -e instructions:u`, (I_K − I_1)/(K − 1), median of 3 runs, `web/requests_local`
5: its server thread polls, so its count varies with wall time), 3.15.1, identical digests
plain/enabled in every run. **M**.

| workload | `--only textwrap,urllib`: wave 1 → wave 2 | all groups: wave 1 → wave 2 |
|---|---:|---:|
| `data/report_render` (`textwrap.fill` ×4,003/run, `shorten`, `indent`) | 0.593 → **0.538** | 0.348 → **0.293** |
| `web/requests_local` (`requests`' `quote`/`urlparse`, `parse_qs`) | 0.958 → **0.943** | 0.895 → **0.882** |
| `data/log_analytics` (`urlsplit` misses, `parse_qs`) | 0.980 → **0.971** | 0.614 → **0.601** |

(wave 1: `ws2/text` bc2ce1a built on the same box; wave 2: this branch on the merged base.)

### 6.1 Instructions per call (callgrind) — wave 1

`python benches/targets/utils_cg.py urllib_cells [--jit]`, CPython 3.15.1, dev box (method:
random.md §6.1). **M**.

| cell | orig Ir | rq Ir | ratio | JIT orig | JIT rq | JIT ratio |
|---|---:|---:|---:|---:|---:|---:|
| `quote('/api/v1/items/café list')` | 15368 | 3180 | 0.207 | 14225 | 3182 | 0.224 |
| `quote('abc')` (all safe) | 4809 | 1637 | 0.340 | 3913 | 1639 | 0.419 |
| `quote_plus('a b&c')` | 11164 | 1803 | 0.161 | 9221 | 1805 | 0.196 |
| `unquote('a%20b%C3%A9')` | 21710 | 3703 | 0.171 | 20471 | 3622 | 0.177 |
| `unquote('plain')` (no `%`: returns its argument) | 1365 | 831 | 0.609 | 1268 | 833 | 0.657 |
| `unquote_plus('a+b')` | 2457 | 1407 | 0.573 | 2200 | 1409 | 0.640 |
| `parse_qsl(5 fields)` | 38842 | 12839 | 0.331 | 33115 | 12910 | 0.390 |
| `parse_qs(5 fields)` | 44455 | 14696 | 0.331 | 38930 | 14767 | 0.379 |
| `urlparse(url)` (uncached; `_urlsplit` accelerated) | 27061 | 15740 | 0.582 | 23722 | 15277 | 0.644 |
| `urlsplit(new url)` (lru_cache miss) | 25497 | 14138 | 0.554 | 22522 | 13522 | 0.600 |
| `urlsplit(url)` (lru_cache hit: CPython's C wrapper, untouched) | 975 | 975 | 1.000 | 977 | 977 | 1.000 |
| `_urlsplit(url)` | 18370 | 6930 | 0.377 | 16428 | 6894 | 0.420 |
| `urljoin(base, '../x?y=1')` (two `urlparse`s inside) | 45178 | 30039 | 0.665 | 38121 | 27672 | 0.726 |
| fallbacks (steady state): | | | | | | |
| `quote(str subclass)` | 8422 | 8422 | 1.000 | 7522 | 7536 | 1.002 |
| `quote(s, encoding='latin-1')` | 14737 | 14732 | 1.000 | 13639 | 13634 | 1.000 |
| `unquote(s, encoding='latin-1')` | 16911 | 16583 | 0.981 | 15695 | 15871 | 1.011 |
| `urlparse('http://[::1]:80/')` (brackets: `_urlsplit` falls back) | 50485 | 50620 | 1.003 | | | |
| `parse_qsl(b'a=1&b=2')` | 23781 | 23856 | 1.003 | | | |

The cheapest fallback (`quote` of a str subclass, ~7.5–8.4k Ir) is above the JIT threshold;
every cell is ≤ 1.011, so the group is `jit_safe`.

### Wall time, large inputs (pyperf)

`python benches/targets/utils_pyperf.py urllib --fast` (busy shared box: only ≥ 1.5× claims). **M**.

| cell | plain | rustique | ratio |
|---|---:|---:|---:|
| `quote(9 KB, non-ASCII)` | 159.3 µs | 16.2 µs | 0.102 |
| `unquote(9 KB)` | 400.0 µs | 43.8 µs | 0.110 |
| `parse_qsl(500 fields)` | 906.1 µs | 119.2 µs | 0.132 |
| `parse_qs(500 fields)` | 945.4 µs | 141.6 µs | 0.150 |
| `[urlparse(u) for u in 1000 urls]` | 1800.6 µs | 1180.8 µs | 0.656 |

### Workloads

`python benches/workloads/run.py compare data/report_render data/log_analytics
data/billing_engine web/requests_local -n 5 -p 3 --only random,textwrap,urllib,statistics`
(3.15.1, busy shared box; medians of steady-state runs; digests identical in every pair). **M**.

| workload | plain | utils groups enabled | ratio | where the utils groups act |
|---|---:|---:|---:|---|
| `data/report_render` | 409.5 ms | 183.9 ms | **0.449** | `random` (choice/choices/sample/randint) + `textwrap` (fill/shorten/indent) |
| `data/log_analytics` | 770.4 ms | 751.5 ms | 0.975 | `urllib` (`urlsplit` misses, `parse_qs`); `ipaddress` (20%) is not a target yet |
| `web/requests_local` | 266.8 ms | 248.1 ms | 0.930 | `urllib` (`urlsplit`/`parse_qs` in the handler, `requests`' `quote`/`urlparse`) |
| `data/billing_engine` | 1024.2 ms | 1021.0 ms | 0.997 | `statistics` on `Decimal` totals falls back (by design); `stdev` of floats accelerated |

### Install cost (budget: lazy install ≤ max(5% of the module's import, 0.1M Ir))

| | cold | warm | budget (max(5%, 0.1M Ir)) |
|---|---:|---:|---:|
| `import urllib.parse` plain | 9.55M Ir | 9.63M Ir | |
| lazy install overhead | +212k (+2.2%) | **+175k (+1.8%)** | 481k: within |

Before this pass: +523k cold (+5.9%), +306k warm.

Method: `python benches/targets/utils_install_cost.py` (callgrind, fresh processes; the
module removed from `sys.modules` first, its dependencies imported; every setup ends with
`gc.collect()` so neither side is charged a collection the other's setup absorbed). *Cold*:
the first group of the process. *Warm*: another group with an imported vectorcall target is
enabled first, so the framework's one-time costs are not charged to this group
(REGISTRY_ANNOUNCE_2 item 16). `enable(only=[group])` with the module not imported costs
0.50M Ir, the same for every group (the registry's first `enable()`, not the group).

The group's install (wave1 b162e61, accel-core's second install-cost pass) is one
`GroupSpec` (`SHARED`): the statics, symbols, validations and **one set of pins for the
whole group** are declared once, and each target adds only its function's fingerprint and
its vectorcall site. Install-time keys are plain `str` objects (`utils_kit::key`): a
`pyo3::intern!` costs ~1.3k Ir on first use, interning ~650 Ir, a plain `str` ~200 Ir, and
dict lookups and the pin watcher compare keys by value.

### First 64 fallback calls (before the registry suspends a target that keeps falling back)

`python benches/targets/utils_first64.py` (Ir per call over 64 calls, fresh process):
`quote(str subclass)`: 9,449 → 9,884 Ir/call (1.046, +435). After 64 consecutive fallbacks the target is suspended and the cells above
measure 1.000 (steady state).
