STATUS: wave 2, workstream `re` (2026-11-02). Contract: docs/ACCELERATION.md.

# Group `re`

Two kinds of targets, all vectorcall sites on CPython's own function objects (contract kind C):

1. **`re._compiler.compile`** — CPython's regular-expression compiler, pure Python (~20 µs per
   pattern; 7.5% of real programs' import time, up to 42% of `import yaml`,
   `docs/atlas/w2-startup.md` §3). The kernel parses and compiles the pattern in Rust and calls
   CPython's own `_sre.compile` with **exactly the arguments `re._compiler.compile` would pass**:
   the pattern object itself, the final flags, the code list, the group count, `groupindex`
   (same insertion order) and `indexgroup` (holding the very key objects of `groupindex`). The
   `Pattern` that comes back is built by CPython, from identical inputs.
2. **The module functions** `re.match` (`re.prefixmatch` on 3.15: the same function object),
   `re.fullmatch`, `re.search`, `re.sub`, `re.subn`, `re.split`, `re.findall`, `re.finditer` and
   `re.compile`: `_compile`'s cache hit, done in Rust on **CPython's own `re._cache2`** with
   CPython's key, then the same `Pattern` method the original calls.

Code: `crates/rustique-core/src/re/` (the port: `parser.rs` = `re._parser`, `compiler.rs` =
`re._compiler`, `template.rs` = `parse_template`'s checks; `cargo test` on 5,242 recorded cases),
`crates/rustique-py/src/targets/re/` (kernels, validation, pins). Tests:
`tests/targets/test_re.py`. Benchmarks: `benches/targets/re_cells.py`, `re_imports.py`,
`re_install.py`, `re_coverage.py`; fingerprints recorded with `benches/targets/re_record.py`;
conformance with `benches/targets/re_conformance.sh`.

## How the compiler stays exact

The port is a statement-by-statement translation of `re._parser.parse` and
`re._compiler._code`, written against their 3.14.9 and 3.15.1 sources (identical except an unused
variable in `_parse_sub` and `bytes(charmap)` → `charmap.take_bytes()`, same results). It
reproduces CPython's object model where results depend on it:

- `SubPattern`s live in an arena, so "the same object" stays expressible: the common-prefix
  extraction of `_parse_sub` compares items with tuple equality, where `SubPattern`s compare by
  identity (items holding one never compare equal across alternatives).
- `getwidth()` is cached per sub-pattern like CPython's `self.width`, in Python's integer
  arithmetic (128-bit, capped at `MAXWIDTH = 2**64`), with the `hi = MAXWIDTH` *replacement* for
  unbounded repeats.
- `_optimize_charset` replays CPython's `IndexError` control flow: a store past the 256-entry
  charmap grows it to 65,536 and *retries the item* (LITERAL keeps its already lowered `av`), a
  store past the BMP sends the item to the tail (RANGE becomes RANGE_UNI_IGNORE under Unicode
  IGNORECASE) with the stores made before the error kept; runs, CHARSET bitmaps (`_mk_bitmap`
  bit order) and BIGCHARSET chunk tables (`_bytes_to_codes`: native-endian words) as CPython.
- Case folding comes from the running interpreter: `_PyUnicode_ToLowercase` /
  `_PyUnicode_ToUppercase` (what `_sre.unicode_tolower` / `_sre.unicode_iscased` call) and
  `re._casefix._EXTRA_CASES` read on the first call (content hash recorded per minor version).
  The result therefore follows the running CPython's Unicode database, whatever its version.
- `MAXREPEAT`, `MAXGROUPS`, `CODESIZE` and `MAGIC` are read from `_sre` and checked at install;
  every opcode, position, category and flag value is checked against `re._constants` on the
  first call (with the fingerprints: see Install).

**Fallback before anything observable.** The Rust compile has no side effects, so the kernel
falls back (calls the original with the unchanged arguments) whenever:

- CPython would raise (`re.error`, `OverflowError`, `ValueError` from `fix_flags`, ...): CPython's
  compiler then raises its own exception, with its message, position, line/column and frames;
- CPython would warn (`FutureWarning`: possible nested set, set operations `--`, `&&`, `~~`,
  `||`, `x--`): CPython warns, with its stack level;
- the pattern uses `\N{...}` (CPython imports `unicodedata` there), the `LOCALE` flag (argument
  or inline `(?L)`), `DEBUG` (CPython prints), a flag bit outside I/M/S/U/X/A, a non-ASCII group
  name (`str.isidentifier` follows the Unicode database), or nests groups deeper than 32 levels
  (CPython's recursion — and its recursion limit — apply);
- the pattern is not an exact `str`/`bytes` (subclasses can override `__getitem__`; a parsed
  `SubPattern` from `re.Scanner`), or flags are not an exact `int` in `[0, 2**31)`;
- `_sre.compile` rejects the arguments (never observed; nothing observable happened: the
  original runs and decides).

A decline costs the caller its inlined frame push plus the port's attempt (+0.3–2.7k Ir, at most
~6% of the cheapest compile, an error pattern). The net-benefit score (§5.6) credits 255 per hit
and debits 1 per decline: only a stream of declined patterns suspends the target (64 in a row
from a fresh state, ~320 after any hit), so a few error patterns early in a process (`test_re`)
do not cost it the acceleration of every later compile.

Check mode (`RUSTIQUE_CHECK`): the original compiles too and the two `Pattern`s must be equal
(`==` compares flags, code and pattern) with the same `groups`, `groupindex` items in order and
the same `pattern` object.

## The module functions

`re.match(p, s, flags)` is `_compile(p, flags).match(s)`. The kernel reproduces the hit path of
`_compile`: `flags.value` for a `RegexFlag` (`_value_` by generic attribute lookup, as the enum
property does), the key `(type(p), p, flags)`, `_cache2[key]`; then calls the `Pattern` method
descriptor (the type is immutable) with the original's arguments. Using CPython's own cache keeps
`re.purge()`, the FIFO/LRU eviction and the identity of returned patterns exactly CPython's: **no
private state is skipped**.

Exact subset (anything else runs the original):

- the original's signature as bound by CPython: `flags` positional or keyword (or the function's
  `__defaults__`/`__kwdefaults__`, read at call time), `count`/`maxsplit` by keyword only
  (positional ones take CPython's `DeprecationWarning` path);
- `pattern` an exact `str` or `bytes` found in `_cache2` (a miss compiles: the original runs and
  the compile kernel takes over), or a compiled `Pattern` with falsy flags (returned as is);
- `flags` an exact `int`, `re._zero_sentinel`, or a `RegexFlag` member;
- `string`: any `str` for a str pattern (subclasses included: `_sre` reads the buffer), an exact
  `bytes` or `bytearray` for a bytes pattern — so no type error can be raised without the
  original's frame, and no user `__buffer__` runs;
- `count`/`maxsplit`: `_zero_sentinel` or an exact `int` that fits `Py_ssize_t`;
- `sub`/`subn` replacement: an exact `str`/`bytes` of the pattern's type that is either a literal
  (no backslash: `_sre` uses it as is) or a template that CPython's `parse_template` accepts for
  this pattern (checked by the port in `template.rs`, allocation-free: escapes, octal, group
  numbers `\1`/`\g<1>` against `Pattern.groups`; templates naming groups, `\g<name>`, decline:
  the lookup through `Pattern.groupindex` costs more than the kernel saves). `_sre` then calls
  CPython's own `re._compile_template` exactly as under the original (its `lru_cache` sees the
  same calls). `_sre` resolves that function on the *first* template `sub` of the process, by an
  import through the calling frame's `__import__`: that first call always runs the original (so
  the resolution happens under `re.sub`'s frame, as in CPython); later template subs are
  accelerated. Callable replacements (user code, ruling 15) fall back.

Dependencies: the `re` globals `_compile`, `_cache2`, `RegexFlag`, `_zero_sentinel`, `Pattern`,
`_compile_template`, `_parser`, `_sre` (key pins); builtins `isinstance`, `type`; what
`parse_template` and `_compile_template` read (`re._parser`'s `parse_template`, `Tokenizer`,
`MAXGROUPS`, `OCTDIGITS`, `DIGITS`, `ESCAPES`, `ASCIILETTERS`, the builtins it uses; `_sre`'s
`template` and `compile`: key pins); version tags of `RegexFlag`, `EnumType` and
`_ZeroSentinel` (a modified class falls back). Fingerprints: every module function, `_compile`,
`_compile_template.__wrapped__`, `parse_template` and the `Tokenizer` methods it uses.

Net benefit: a hit saves ~1,100 Ir, a fallback costs ~250–340 Ir, so `hit_credit = 3`,
`fallback_debit = 1` (installed while at least a quarter of the calls hit).

## Validation and pins of the compiler

Fingerprints (per minor version, `fps.rs`): `_compiler.compile`, `isstring`, `_code`,
`_compile_info`, `_compile`, `_compile_charset`, `_optimize_charset`, `_mk_bitmap`,
`_bytes_to_codes`, `_simple`, `_generate_overlap_table`, `_get_iscased`, `_get_literal_prefix`,
`_get_charset_prefix`, `_combine_flags`; `_parser.parse`, `fix_flags`, `_parse_sub`, `_parse`,
`_parse_flags`, `_escape`, `_class_escape`, `_uniq`; every method (and property getter) of
`Tokenizer`, `State`, `SubPattern` the success path calls (checked on the first call). Pins:
snapshot pins of the `re._compiler`, `re._parser` and `_sre` namespaces (rebinding
`_sre.compile`, `_parser.parse`, ... or adding a global that shadows a builtin falls back until
restored), the builtins' own entries the two modules use, and the version tags of `Tokenizer`,
`State`, `SubPattern` and `_NamedIntConstant` (checked per call: one load each; a modified class
falls back for the rest of the process).

## Private state, deviations

- None skipped (CPython's caches and `_compile_template`'s `lru_cache` are used as they are).
- Known edge, documented: `_sre` caches the `re._compile_template` it resolved on the first
  template `sub` of the process. If a program had replaced that private function at that moment
  and restored it before the group's first template `sub` (which runs the original), later
  accelerated template subs reach the cached replacement without `re.sub`'s frame on the stack.
  Out of contract (private function, replaced and restored around its first use).
- Like every vectorcall kernel, an accelerated call does not push the Python frames of the
  original: a program within ~100 frames of the recursion limit can compile a pattern that
  CPython's (recursive) compiler would have refused with `RecursionError`. Nesting is capped so
  the difference stays bounded.

## Tests

`tests/targets/test_re.py` (all four builds):

- **Differential, compiler**: the arguments the kernel would pass to `_sre.compile`
  (`_rustique._accel._re_compile_args`) against CPython's own `re._compiler.compile` code run
  with `_sre` replaced by a recorder (a fresh function over the original code object): equal
  pattern object, flags, code words, group count, `groupindex` items in order and `indexgroup`
  holding the same key objects — or, where the port declines, CPython must raise, warn or be
  outside the port. Inputs: the 8,522-pattern PyPI corpus with their own flags and 8 flag
  combinations (~70,000 compiles), every str/bytes constant of CPython's `test_re.py` and
  `re_tests.py` × 4 flag sets, every literal pattern passed to a `re` function in the standard
  library × 2, Hypothesis (str patterns from 100 regex tokens, bytes patterns, arbitrary text,
  character sets with non-BMP and case-folding characters; ~11,000 examples), 600 random large
  Unicode sets. Plus `cargo test` on 5,242 recorded cases (`crates/rustique-core/src/re/
  testdata/gen.py` regenerates them).
- **Differential, templates**: the port's verdict against `re._parser.parse_template` (4,000
  Hypothesis examples over escapes, group numbers and names, octal, bytes).
- **Differential, module functions**: every function against an unpatched clone (same code,
  same globals, default vectorcall) on patterns × subjects × replacements (literals,
  templates, invalid templates, callables, bytes) × flags × counts: results, `Match` spans,
  groups and `.re` identity, exceptions, warnings (3,000 examples).
- **Mechanics**: errors and warnings come from CPython's frames (traceback holds
  `_compiler.compile`), `re.DEBUG` prints, `re.Scanner` (parsed sub-patterns) falls back,
  `mock.patch` of `_sre.compile` and of `re._compile` falls back and re-arms, `sys.settrace`
  sees CPython's frames, introspection/pickling/deepcopy unchanged, captured references
  (`from re import search`, `functools.partial`) accelerated, `re.purge()` works on CPython's
  caches, callable replacements see `re.sub`'s frame, the first template `sub` of a process
  resolves `re._compile_template` under `re.sub`'s frame (subprocess), composite `RegexFlag`s
  keep the class unmodified, `disable()` restores.
- **Threads**: 8 threads compiling, purging and matching against the clone; 6 threads while
  the main thread disables and re-enables the group 15 times (free-threaded builds included).

## Conformance (ACCELERATION §7.1, `benches/targets/re_conformance.sh`)

CPython's own test modules with the group installed before they import, against plain runs:
`test_re`, `test_email`, `test_tokenize`, `test_configparser`, `test_fnmatch`, `test_textwrap`,
`test_http_cookies`, `test_difflib`, `test_string`, `test_shlex`, `test_argparse`,
`test_gettext`, `test_glob`, `test_csv`, `test_urllib`, `test_mailbox` (~5,500 tests): **0
differences** on 3.15.1, 3.15.1t, 3.14.9 and 3.14.9t, every target installed (recorded as well
on 3.15.2: 0 differences). One transient difference (a `test_mailbox` temp directory removed
under the *plain* run by a concurrent sync of the source tree) led to running the suites outside
the synced tree; rerun clean. The whole Rustique suite (`pytest -q tests`, after merging wave2
a000743): 3.15.1 and 3.15.1t 2,230 passed / 5 skipped, 3.14.9 and 3.14.9t 2,232 passed / 3
skipped; `cargo test -p rustique-core` 62 passed; clippy `-D warnings` and `cargo fmt` clean.

## Version coverage

3.14 and 3.15 (GIL and free-threaded builds; same fingerprints on both; 3.14.9/3.15.1 and the
recorded 3.15.2 give the same `re` sources). Other minors refuse (`version`). The fingerprints
are checked on the first kernel call: a patch release that changed one degrades the group
(kernels fall back, then suspend). `python -O` strips the compiler's asserts (other code
objects): the group degrades there. The module functions declare `jit_safe: false` (their
fallbacks were not measured under the JIT) and refuse when `PYTHON_JIT=1`; the compiler target
is JIT-safe (a fallback is a small share of any compile).

## Measured results

All instruction counts are callgrind on the dev box (c7a.8xlarge, 3.15.1, release build with
fat LTO, PYTHONHASHSEED=0); M = measured.

### Coverage: how much the port compiles itself (M, `benches/targets/re_coverage.py`)

| corpus | compiles | port | fallback: CPython raises | warns | outside the port |
|---|---:|---:|---:|---:|---:|
| PyPI corpus (8,522 distinct patterns of popular packages, own flags) | 8,522 | **8,498 (99.7%; 100% of those CPython compiles silently)** | 24 | 0 | 0 |
| standard-library literal patterns | 1,121 | 1,065 (98.8% of the silent ones) | 29 | 14 | 13 |
| patterns 12 real programs compile at import (`upper_bounds.py` capture) | 515 | **515 (100%)** | 0 | 0 | 0 |

### Install (M, `benches/targets/re_install.py`)

`import re` costs 21.6M Ir. With accel's first-call validation (the default on GIL builds) the
group is *armed* at its end: **52k Ir (0.2%)**, sites holding first-call stubs (87k when `re` was
already imported at `enable()`). Each target validates at its first call: the group's full
`prepare` (identities, `_sre` limits, version tags, pins: ~0.65M for the compiler, ~0.15M for the
module functions) and then, on the kernel's first call, the group's own deferred checks
(fingerprints of 59 functions and the table checks, ~0.55M; ACCELERATION §5.7: a failure
degrades the targets). Free-threaded builds (and `RUSTIQUE_VALIDATE=eager`) run the full
`prepare` at install: 0.69M Ir (3.2% of `import re`; T9 budget 5%); with the fingerprints at
install it was 1.58M (7.3%).

### Cells (M, `benches/targets/re_cells.py`: instructions per call, plain vs group installed)

Compile (`re._compiler.compile(P, F)`, the kernel's site, no cache):

| pattern | plain Ir | Rustique Ir | ratio |
|---|---:|---:|---:|
| `\d+` | 79,504 | 9,598 | **0.121** |
| `(?P<major>[0-9]+)\.(?P<minor>[0-9]+)` | 268,488 | 24,286 | **0.090** |
| URL with 5 named groups | 943,008 | 69,357 | **0.074** |
| verbose tokenizer (6 named alternatives, `re.X`) | 2,901,281 | 256,465 | **0.088** |
| RFC 5322 e-mail (`re.I`, ASCII sets) | 11,220,781 | 3,460,098 | 0.308 |
| Unicode IGNORECASE set (`[a-z\u00e0-\u017f]+`) | 1,294,925 | 363,985 | 0.281 |
| BIGCHARSET (Cyrillic, Armenian, Hebrew, Arabic, kana, CJK) | 1,322,651 | 347,761 | 0.263 |
| ASCII IGNORECASE (`(?ai)content-type...`) | 555,601 | 47,234 | 0.085 |
| bytes HTTP request line | 393,748 | 39,872 | 0.101 |
| `''` | 29,712 | 3,761 | 0.127 |
| `re.purge(); re.compile(medium)` (a real miss) | 285,516 | 41,670 | **0.146** |
| `re.purge(); re.compile(\d+)` | 96,559 | 26,593 | 0.275 |

The Unicode-IGNORECASE and big-set cells are bounded by `_sre.compile` itself and by the
per-code-point case mapping over ranges (as CPython's compiler, but in Rust).

Declined patterns (fall back to CPython's compiler). An all-decline stream suspends the target
after 64 calls (ACCELERATION §5.6): `\N{...}` 1.000, bytes LOCALE 1.000, FutureWarning 0.998,
error 0.997, `RegexFlag` flags (`_compiler.compile` called directly) 1.000, `int`-subclass flags
1.000. Unsuspended (each decline paired with a hit on `\d+`, which saves 69.9k): a decline
costs **+1,752 Ir on an error pattern (1.036 of its 48.8k)**, +1,217 on a FutureWarning pattern
(1.013), +1,395 on `\N{...}` (1.016): the caller's inlined frame push plus the port's attempt
(no allocation for patterns up to 160 code units).

Module functions (cache hits):

| call | plain Ir | Rustique Ir | ratio |
|---|---:|---:|---:|
| `re.match(P, s)` | 4,060 | 2,757 | **0.679** |
| `re.match(P, s, re.I)` | 4,868 | 3,070 | **0.631** |
| `re.match(P, s, flags=0)` | 4,280 | 3,010 | 0.703 |
| `re.match` (no match) | 3,595 | 2,287 | 0.636 |
| `re.fullmatch` | 4,335 | 3,178 | 0.733 |
| `re.search` (version in a sentence) | 6,009 | 4,790 | 0.797 |
| `re.search` (URL in a log line) | 10,289 | 8,893 | 0.864 |
| `re.findall` (digits of a log line) | 32,671 | 32,076 | 0.982 |
| `list(re.finditer(...))` | 14,803 | 13,610 | 0.919 |
| `re.split(r'\s+', s)` | 10,793 | 9,880 | 0.915 |
| `re.split(..., maxsplit=1)` | 6,449 | 5,041 | 0.782 |
| `re.sub` literal | 11,286 | 9,972 | 0.884 |
| `re.sub` template `\1` | 18,367 | 17,535 | 0.955 |
| `re.subn` literal | 11,668 | 10,354 | 0.887 |
| `re.sub(..., count=1)` | 6,522 | 5,143 | 0.789 |
| `re.compile(P)` (hit) | 2,214 | 995 | **0.449** |
| `re.compile(P, re.I)` (hit) | 2,928 | 1,255 | 0.428 |
| `re.match(bytes P, bytes)` | 4,890 | 3,659 | 0.748 |
| `re.match(compiled, s)` | 7,339 | 2,407 | **0.328** |

Fallbacks: callable replacement mixed 1:1 with hits 1.006; positional `count` (DeprecationWarning
path) mixed 0.964; str/bytes mismatch (TypeError) mixed 0.965; `int`-subclass flags mixed 0.870;
`re.purge(); re.search(...)` (a miss: the compile kernel takes it) 0.302; an all-callable `re.sub`
stream 0.994 (suspended); an all-`int`-subclass-flags `re.match` stream **1.046** (suspended
after 64 calls; the remaining 4.6% is the call site staying unspecialized after the
suspension restores CPython's vectorcall — a property of every suspended vectorcall target).
A template naming groups (`\g<name>`) measured 1.094 when checked (the lookup through
`Pattern.groupindex`, a mapping proxy built per access, costs more than the kernel saves), so
such templates decline: 0.999 as a stream (suspended); unsuspended it costs the fallback plus
the template scan up to the name (~+0.5k on 13.3k, I from the decline cells).

### Threads (M, `benches/targets/re_threads.py`, 3.15.1t, CPU time per call, median of 5–9)

| cell | 1 thread | 8 threads |
|---|---:|---:|
| `re.match` hit | 0.650 | **0.579** |
| `re.sub` template hit | 0.939 | 0.847 |
| `re.compile` hit | 0.635 | 0.398 |
| `_compiler.compile` (no cache) | 0.118 | 0.078 |
| `re.sub` callable (all fallbacks, suspended) | 0.999 / 1.005 | 1.101 / 0.864 (two runs) |
| `re.sub` callable + literal (1:1) | 1.026 | 0.991 |
| control: plain vs plain with `rustique` imported | 0.996 | 1.066 |

The kernels hold no lock and write no shared state on a hit (the net-benefit score stays at
its ceiling: loads only). On the shared dev box (load 60–80) 8-thread CPU times vary by
±7–10% between identical runs (control row); the all-fallback cell is within that noise and
needs the quiet box to settle.

### Steady state (M, `benches/targets/re_workloads.py`: `perf stat -e instructions:u`, per run, median of 3; digests identical)

| workload | plain Ir/run | re group only vs plain | all groups vs all but `re` | `re` kernel calls per run |
|---|---:|---:|---:|---|
| web/email_mime | 6.30G | 1.0008 | 1.0021 | none (Pattern methods) |
| data/log_analytics | 9.33G | 0.9994 | 0.9992 | 6 `re.compile` hits |
| data/text_corpus | 8.03G | 0.9993 | 0.9987 | 6,000 `re.sub` hits |
| devtools/pycodestyle_stdlib | 12.36G | 1.0016 | 1.0003 | 1 `re.compile` hit |
| devtools/sphinx_autodoc | 9.25G | **0.9946** | 0.9965 | 305 match, 1,067 search, 1,339 sub hits |
| devtools/rich_render | 3.27G | 1.0027 | 0.9997 | 27 split/finditer hits |

No workload falls back in steady state (`RUSTIQUE_STATS`: 0 fallbacks); the module functions are
a small steady-state share (atlas §11: 0.3–0.5% mean), and the differences of ±0.3% on runs
with a handful of kernel calls are the shared box's run-to-run noise. The machinery
prototype's email_mime 1.004 (template/callable fallbacks in its setup) is gone.

### Startup and import (M, `benches/targets/re_imports.py`: whole-process callgrind, release wheel repacked, warm bytecode caches)

Three variants per program: plain; `RUSTIQUE=1 RUSTIQUE_EXCLUDE=re` (every other group); `RUSTIQUE=1`.
The `re` group's effect is the third over the second; `re` compiles counted in a plain run.

| program | `re` compiles | all-but-re Ir (M) | all Ir (M) | **re effect** | all vs plain |
|---|---:|---:|---:|---:|---:|
| imp:yaml | 10 | 140.3 | 85.3 | 0.608 | 0.613 |
| imp:requests | 84 | 452.4 | 391.3 | 0.865 | 0.870 |
| imp:email | 23 | 109.4 | 105.3 | 0.962 | 0.975 |
| imp:pip | 9 | 122.1 | 117.2 | 0.959 | 0.971 |
| imp:docutils | 55 | 197.9 | 167.7 | 0.847 | 0.856 |
| imp:pygments | 9 | 103.1 | 99.5 | 0.965 | 0.979 |
| django_setup | 95 | 749.3 | 712.0 | 0.950 | 0.954 |
| flask_app | 82 | 516.7 | 476.0 | 0.921 | 0.927 |
| fastapi_app | 78 | 1,130.5 | 1,091.6 | 0.966 | 0.965 |
| sqlalchemy_orm | 53 | 1,002.7 | 982.0 | 0.979 | 0.976 |
| pydantic_model | 11 | 496.1 | 487.5 | 0.983 | 0.981 |
| requests | 84 | 452.4 | 391.3 | 0.865 | 0.870 |
| pytest_co | 51 | 1,089.9 | 1,072.2 | 0.984 | 0.969 |
| pip_version | 86 | 563.3 | 501.3 | 0.890 | 0.894 |
| black_version | 33 | 679.9 | 651.0 | 0.958 | 0.958 |
| json_tool | 13 | 189.6 | 187.1 | 0.987 | 1.001 |
| typer_help | 97 | 525.4 | 480.4 | 0.914 | 0.916 |
| rich_print | 14 | 267.1 | 256.8 | 0.961 | 0.976 |
| import_startup workload (21 processes) | 828 | 9,069.7 | 8,586.7 | 0.947 | 0.950 |

geometric mean of the 12 programs: 0.946; worst: is:click 1.007, is:attrs 1.001, bare 1.000, is:<bare> 1.000

- `import yaml` **0.608**, `import requests` 0.865, `import docutils` 0.847, `pip --version`
  0.890, typer `--help` 0.914, flask app 0.921 (atlas upper bounds, compile at zero cost: 41.9%,
  15.2%, —, 12.0%, 10.3%, 8.9% saved; measured 39.2%, 13.5%, 15.3%, 11.0%, 8.6%, 7.9%).
- A program that compiles one pattern pays the install (0.69M) and the compiler's first-call
  validation (~0.55M) for little gain: `import click` 1.007, `import attrs` 1.001 (≤ 1.05;
  T1's 1.02 holds). Programs that compile none (`python -c pass`, `import rich.console`) pay
  nothing measurable (1.000, 0.995).
