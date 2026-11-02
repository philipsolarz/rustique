STATUS: wave 1, group `textwrap` (workstream utils). Contract: `docs/ACCELERATION.md`.
Code: `crates/rustique-py/src/targets/textwrap/` (kernels, options, identity),
`crates/rustique-core/src/textwrap/` (the algorithms on code points, `cargo test`).
Tests: `tests/targets/test_textwrap.py`, `tests/targets/test_utils_mechanics.py`.
Benchmarks: `benches/targets/textwrap_cells.py` (callgrind), `utils_pyperf.py textwrap`.

# Targets `textwrap.wrap`, `fill`, `shorten`, `dedent`, `indent`, `TextWrapper.wrap`, `TextWrapper.fill`

Vectorcall kernels on the module functions and on the two methods in
`TextWrapper.__dict__`; captured references (`from textwrap import dedent`, bound
`wrapper.fill`) reach them; identity, signature, source and pickling are unchanged.
All seven are side-effect free and `checkable` (check mode compares every fast-path result
with the original: values, types, identity with the arguments).

| target | validated | free-threaded | checkable |
|---|---|---|---|
| `textwrap.wrap`, `fill`, `shorten` | 3.14, 3.15 | yes | yes |
| `textwrap.dedent`, `indent` | 3.14, 3.15 | yes | yes |
| `textwrap.TextWrapper.wrap`, `.fill` | 3.14, 3.15 | yes | yes |

`textwrap.py` is identical in 3.14.9 and 3.15.1 (`dedent` is the 3.14 rewrite in both: still
pure Python, measured first — §6).

## 1. Exactness

The kernels run CPython's algorithm on the text's code points (`rustique_core::textwrap`):

- **Munging**: `str.expandtabs(tabsize)` (column reset on `\n`/`\r`, `tabsize <= 0` drops
  tabs) and `translate(unicode_whitespace_trans)` (the six ASCII whitespace characters to
  spaces).
- **Chunking** replicates `re.split` with `wordsep_re` / `wordsep_simple_re`. `wordsep_re`
  matches at every position (whitespace: first alternative; anything else: the third, at
  the end of the non-whitespace run at the latest), so the chunks are the consecutive
  matches, each found as `sre` would: alternatives in order, the lazy `\S+?` tried with 1,
  2, ... characters, lookbehinds reaching before the match start, `-{2,}` greedy with the
  backtracking outcome made explicit. Character classes use CPython's own Unicode
  database (`_PyUnicode_IsAlpha/IsDecimalDigit/IsDigit/IsNumeric` for `\w` and `\d`,
  `_PyUnicode_IsWhitespace` for `str.strip/isspace/split`), ASCII inline. A self-test at
  install compares the chunking of probe strings (hyphens, em-dashes, digits, Arabic-Indic
  digits, superscripts, non-breaking and em spaces) with the compiled patterns' `split`.
- **`_fix_sentence_endings`**, **`_wrap_chunks`** and **`_handle_long_word`** are ported
  line for line, including `max_lines`/`placeholder` (the placeholder placement loop, the
  previous-line `rstrip()` rule), `drop_whitespace` (Unicode `strip()`), `break_on_hyphens`
  (`is True` selects the pattern, truthiness the hyphen rule of `_handle_long_word`).
- **Identity**: CPython returns its argument in a few cases (`dedent` of one line with no
  margin; `indent` of a single all-whitespace line; `wrap`/`fill`/`shorten` with
  `replace_whitespace=False` when the text is one chunk: `PyUnicode_Substring` of the whole
  string, `''.join([x])` and `'' + x` return `x`). The kernels return the argument in
  exactly these cases (tested: `result is text` compared with CPython).
  Since wave 2 the kernels also build every other result object themselves, so the rule
  for one-character results is CPython's builder's: a line that is one chunk is a slice
  (`PyUnicode_Substring`: the Latin-1 singleton for one character), `dedent` of one line is
  `l[margin:]` (a slice), while `'\n'.join` of several lines and `indent`'s `''.join` of
  several parts are new objects even for one character (`_PyUnicode_JoinArray` does not
  return the singleton). Lines CPython builds as the placeholder object itself
  (`'' + placeholder`, `'' + placeholder.lstrip()` with nothing to strip) or as a fresh
  one-character join (a `max_lines` cut with an empty placeholder) are not expressible as a
  piece list: those calls fall back (`args`). Tested by `test_textwrap_identity.py`
  (identity with every argument and with the 256 singletons, every PEP 393 kind).
- **Endless loops**: CPython's `_wrap_chunks` never terminates when an indent is at least
  as wide as `width` and the first line can take nothing (e.g. text starting with a space).
  The kernel detects an iteration that consumes nothing and lets CPython's original run (and
  loop, interruptibly) — a kernel loop would not be interruptible.

## 2. Exact subset and fallbacks

- text: exact `str`; module functions: `width` (and `tabsize`, `max_lines`) exact `int`,
  indents and `placeholder` exact `str`, the flags `bool`/`int`/`None` (truth tested
  without user code); keyword arguments are `TextWrapper`'s. `shorten` adds `max_lines=1`
  (a `max_lines` keyword falls back: CPython raises).
- `TextWrapper.wrap/fill`: the instance's class resolves every method and class attribute
  the algorithm uses (`wrap`, `_munge_whitespace`, `_split`, `_split_chunks`,
  `_fix_sentence_endings`, `_handle_long_word`, `_wrap_chunks`, `wordsep_re`,
  `wordsep_simple_re`, `sentence_end_re`, `unicode_whitespace_trans`) to CPython's own
  (subclasses that override none qualify; cached per type version), no class attribute is
  named like an option, the instance does not shadow a method (inline-values check, else
  `_PyObject_GetMethod`), and its options are of the types above.
- `dedent(text)`, `indent(text, prefix)` (predicate absent or `None`).

Falls back (`args`): str subclasses, other types, options of other types, a `predicate`,
a subclass overriding any method; every error (`width <= 0`, placeholder too large,
non-str text); the endless-loop case. `deps`: a replaced dependency (§3).

## 3. Dependencies

Pinned once for the whole group (`GroupSpec`, flag `TEXTWRAP_DEPS`): `textwrap.TextWrapper`
(module global) and what `LOAD_GLOBAL` resolves `dedent`'s builtins to (`min`, `max`,
`enumerate`); replacing any of them suspends all seven targets until it is back. The class
attributes above (and `fill`, `__init__`, `__new__`) are checked per call through the
type-version cache: patching one on `TextWrapper` or a subclass sends the calls to the
original. Checked per call:
`TextWrapper.__init__.__defaults__`/`__kwdefaults__` identity (the module functions use
its defaults) and the contents of `unicode_whitespace_trans` (a mutable dict: six lookups).

**Private state skipped:** none (no caches involved; the `TextWrapper` instance the module
functions would create is not created). The kernels keep per-thread scratch buffers (Rust
memory, not Python objects; reused between calls, released above 64 Ki elements), invisible
to any API.

## 4. Version coverage

3.14.9 and 3.15.1 and their free-threaded builds; refused elsewhere, on fingerprint
mismatch, and when the chunking self-test fails.

### Private CPython symbols and layouts (accepted by the lead)

| what | used for | obtained / checked at install | when missing or mismatched |
|---|---|---|---|
| `_PyUnicode_IsAlpha`, `_PyUnicode_IsDecimalDigit`, `_PyUnicode_IsDigit`, `_PyUnicode_IsNumeric`, `_PyUnicode_IsWhitespace` (exported, `Objects/unicodectype.c`) | `\w`, `\d`, `\s` of `wordsep_re`/`wordsep_simple_re` and `str.strip/isspace/split` on non-ASCII code points: CPython's own Unicode database | `cx.symbol`, then the chunking self-test against the compiled patterns' `split` | refused, category `symbol` ("symbol ... not exported by this interpreter"); a self-test mismatch refuses the group |
| `_PyObject_GetMethod` (exported, `Objects/object.c`) | the exact "does the `TextWrapper` instance shadow a method" answer when the inline-values check cannot answer | `cx.symbol` | refused, category `symbol` |
| `_PyDictKeys_StringLookup` + the inline-values layout (`utils_kit::InstLayout`, shared with `random`, see docs/targets/random.md §4) | the fast "no instance attribute can shadow a method" check | `dlsym` + calibration on `os.environ` | not refused: the fast check stays off and `_PyObject_GetMethod` answers (exact, slower) |
| `_PyType_Lookup` (`PyAPI_FUNC`, `Include/cpython/object.h`) | MRO lookups of the methods and class attributes (compared only) | linked (non-limited API) | exported by every 3.14/3.15 build |

## 5. Conformance

`python tests/conformance/run_target_suites.py --targets textwrap --modules ...` (CPython's own test
modules from `$CPYTHON_SRC`, plain vs `enable(only=["textwrap"])` before the tests import the
module, per test id; every target asserted `installed`), on 3.15.1, 3.15.1t, 3.14.9 and
3.14.9t: `test.test_textwrap`: 68 pass — **identical outcome sets on all four builds, 0 differences**
(`benches/targets/utils_matrix.sh`, which also runs `tests/targets` and the whole
`pytest -q`). The runner normalizes object addresses in subTest ids (`0x…`), which differ
between processes (`test_urlparse` parametrizes with function reprs).

Wave 2 (allocation-free kernels, merged base `ws2/text` c9fb335): `test.test_textwrap` 68 pass,
0 differences on 3.15.1, 3.15.1t, 3.14.9 and 3.14.9t; the whole `pytest -q` green on the four
builds (3.15.1/1t: 2212 passed, 5 skipped; 3.14.9/9t: 2214 passed, 3 skipped), including
`test_textwrap_identity.py` (identity-level differential tests, Hypothesis, 8-thread and
re-entrancy tests).

## 6. Measurements

### 6.0 Wave 2: allocation-free kernels (Phase 2, `ws2/text-tw`)

What changed (w2-floors §1: in wave 1, 45% of `fill`'s instructions were `malloc`/`free`/
`realloc` of per-word `Vec`s plus a UCS-4 copy of every input; w2-threads §3.1: per-call
heap traffic goes through glibc's arena lock on free-threaded builds):

- **Inputs are read in place** (`textwrap/pep393.rs`: the PEP 393 buffer as `u8`/`u16`/`u32`
  units, the algorithms in `rustique_core::textwrap` generic over the unit width) — no
  `PyUnicode_AsUCS4` copy.
- **Per-thread scratch** (munged text, chunks, line pieces, option texts) reused from call
  to call: no heap allocation in the steady state; a buffer grown past 64 Ki elements is
  released after the call; a re-entrant use (a finalizer calling a kernel) gets one-off
  buffers.
- **Lines are piece lists** (`Seg`: a range of the munged text, an indent, the placeholder,
  or the two spaces of `_fix_sentence_endings`), contiguous chunks merged, and each result
  `str` is written once, straight into `PyUnicode_New(len, maxchar)` with the exact PEP 393
  class (no intermediate `Vec<u32>`, no `PyUnicode_FromKindAndData` scan); `dedent` and
  `indent` write straight from the argument's buffer.
- **Chunking** skips to the next `-` or whitespace in one scan (no tail can match in between),
  munging is the identity map without tabs.
- **Identity** (§1): one-character results follow CPython's builders (slice: singleton;
  `''.join` of several parts: a new object); a line that is the placeholder object is
  returned as that object (`shorten(t, 5, placeholder='...')` → the argument itself);
  `max_lines` with an empty placeholder falls back before any other work.

Instructions per call (`python benches/targets/utils_cg.py textwrap_cells`, callgrind,
3.15.1, dev box, merged base `ws2/text` c9fb335; wave-1 column: the same cells on `ws2/text`
bc2ce1a before this work). **M**.

| cell | CPython Ir | wave 1 Ir (ratio) | wave 2 Ir | ratio |
|---|---:|---:|---:|---:|
| `fill(desc, 88, indents) [report_render]` | 501,170 | 76,194 (0.152) | 18,875 | **0.038** |
| `shorten(title, 30, placeholder) [report_render]` | 108,453 | 18,100 (0.167) | 6,863 | **0.063** |
| `indent(30 lines, '  ') [report_render]` | 34,092 | 11,742 (0.345) | 8,457 | **0.248** |
| `wrap(540 chars)` | 1,085,786 | 158,154 (0.151) | 40,753 | **0.038** |
| `fill(short)` | 31,818 | 7,528 (0.236) | 3,441 | **0.108** |
| `wrap(word)` | 18,781 | 5,541 (0.296) | 3,333 | **0.177** |
| `dedent(20 lines)` | 52,906 | 24,575 (0.464) | 12,028 | **0.227** |
| `dedent('x')` | 6,468 | 1,018 (0.157) | 622 | **0.096** |
| `indent('x', '> ')` | 2,545 | 2,106 (0.821) | 1,212 | **0.476** |
| `TextWrapper.wrap(540)` | 1,082,714 | 165,852 (0.159) | 43,580 | **0.040** |
| `TextWrapper.fill(desc)` | 502,366 | 81,771 (0.162) | 20,979 | **0.042** |
| `fill(desc, Latin-1)` | 541,027 | — | 19,321 | **0.036** |
| `fill(desc, UCS-2)` | 602,412 | — | 19,418 | **0.032** |
| `shorten(title, 12) [max_lines cut]` | 101,224 | — | 6,096 | **0.060** |
| `shorten(title, 5, '...') [line is the placeholder object]` | 102,861 | — | 5,649 | **0.055** |
| `fill(max_lines, empty placeholder) [fallback]` | 97,942 | — | 95,512 | **0.975** |
| `wrap(str subclass) [fallback]` | 103,352 | 103,135 (0.999) | 103,352 | **1.000** |
| `subclass with _split [fallback]` | 215,107 | 215,102 (1.001) | 215,284 | **1.001** |
| `indent(predicate) [fallback]` | 41,778 | 41,773 (1.000) | 41,778 | **1.000** |
| `dedent(str subclass) [fallback]` | 7,091 | 7,096 (0.998) | 7,077 | **0.998** |

The `report_render` cells are **4.0× (`fill`), 2.6× (`shorten`) and 1.4× (`indent`) fewer
instructions than wave 1**. `fill` of ~330 characters is now ~57 Ir per character; a
breakdown of the cell (`benches/floors/cg_breakdown.py`, build before the last tuning) put
two thirds of it in chunking and `_wrap_chunks` and none in `malloc`/`free`. `indent` and
`dedent` stay at 1.5–1.7× the floors w2-floors estimated (line scanning). Every fallback
cell is ≤ 1.001.

**First 64 fallback calls** (fresh process, one hit of the target first so one-time
validation is outside the region; `utils_first64.py` method): `dedent(str subclass)` 1.045
(+337 Ir/call, as wave 1), `fill(..., max_lines=1, placeholder='')` 1.028 (+2.7k: the
module function's fallback reaches `TextWrapper.fill`, whose kernel reads two options and
falls back too). After 64 consecutive fallbacks the target is suspended (1.000).

**Free-threaded scaling** (`python benches/targets/tw_scaling.py`, 3.15.1t, 8 cores, CPU
time per call; the 8-thread rule is enabled/plain CPU at 8 threads ≤ 1.05): see the table
in docs/targets/urllib.md §6.0 (both groups are measured together). textwrap: enabled/plain
CPU at 8 threads 0.03–0.27 (worst: `dedent` of one shared input 0.273); per-call CPU
grows ≤ ×1.06 from 1 to 8 threads with per-thread inputs.

**Workloads** (`python benches/targets/text_wl.py`, user-space instructions per steady-state
`run()`, identical digests, details in docs/targets/urllib.md §6.0): `data/report_render`
0.593 → **0.538** with only `textwrap`,`urllib` on, 0.348 → **0.293** with every group.

### 6.1 Instructions per call (callgrind) — wave 1

`python benches/targets/utils_cg.py textwrap_cells [--jit]`, CPython 3.15.1, dev box
(method: random.md §6.1). **M**.

| cell | orig Ir | rq Ir | ratio | JIT orig | JIT rq | JIT ratio |
|---|---:|---:|---:|---:|---:|---:|
| `fill(desc ~330 chars, 88, initial/subsequent indent)` (report_render) | 500111 | 77011 | 0.154 | 468994 | 77013 | 0.164 |
| `shorten(title, 30, placeholder='...')` (report_render) | 107876 | 17867 | 0.166 | 97302 | 17869 | 0.184 |
| `indent(30 lines, '  ')` (report_render) | 34054 | 12476 | 0.366 | 26973 | 12478 | 0.463 |
| `wrap(540 chars)` | 1089578 | 162015 | 0.149 | 989530 | 162017 | 0.164 |
| `fill('hello world')` | 31932 | 7519 | 0.235 | 27857 | 7521 | 0.270 |
| `wrap('word')` | 18719 | 5529 | 0.295 | 16240 | 5536 | 0.341 |
| `dedent(20 lines)` | 52906 | 24686 | 0.467 | 46594 | 24688 | 0.530 |
| `dedent('x')` | 6468 | 1017 | 0.157 | 6324 | 1019 | 0.161 |
| `indent('x', '> ')` | 2545 | 2104 | 0.826 | 2302 | 2111 | 0.917 |
| `TextWrapper(width=60).wrap(540 chars)` | 1086416 | 165243 | 0.152 | 987801 | 165245 | 0.167 |
| `TextWrapper.fill(desc)` | 509272 | 80556 | 0.158 | 466870 | 80558 | 0.173 |
| fallbacks (steady state): | | | | | | |
| `wrap(str subclass)` | 102797 | 103336 | 1.005 | 89079 | 89079 | 1.000 |
| subclass overriding `_split` | 214942 | 215278 | 1.002 | 170251 | 169999 | 0.999 |
| `indent(text, prefix, predicate)` | 41735 | 41735 | 1.000 | 35526 | 35545 | 1.001 |
| `dedent(str subclass)` | 7077 | 7077 | 1.000 | 7154 | 7140 | 0.998 |

The cheapest fallback (`dedent` of a str subclass, ~7k Ir) is above the ~7k Ir JIT
threshold; every cell is ≤ 1.005 with and without the JIT, so the group is `jit_safe`.

Per-call cost of the class checks that replace pins (TextWrapper methods, `__init__`
defaults, the translation table): one `tp_version_tag` compare for the class, two pointer
compares for the defaults, six dict lookups for the table (~150 Ir, included above).

### Wall time, large inputs (pyperf)

`python benches/targets/utils_pyperf.py textwrap --fast` (busy shared box: only ≥ 1.5× claims). **M**.

| cell | plain | rustique | ratio |
|---|---:|---:|---:|
| `wrap(540 chars)` | 64.4 µs | 11.0 µs | 0.170 |
| `fill(540 chars, 40, indents)` | 66.9 µs | 11.0 µs | 0.165 |
| `dedent(500 lines)` | 69.9 µs | 33.5 µs | 0.480 |
| `indent(500 lines)` | 41.5 µs | 34.5 µs | 0.831 |

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
| `import textwrap` plain | 3.04M Ir | 3.08M Ir | |
| lazy install overhead | +184k (+6.1%) | **+130k (+4.2%)** | 154k: within |

Before this pass: +302k cold, +203k warm.

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
`textwrap.dedent(str subclass)`: 7,548 → 7,893 Ir/call (1.046, +345). After 64 consecutive fallbacks the target is suspended and the cells above
measure 1.000 (steady state).
