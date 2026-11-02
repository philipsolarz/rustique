STATUS: wave 1, workstream `paths` (2026-11-01); wave 2, workstream `text` (2026-11-02: §Wave 2).
Contract: docs/ACCELERATION.md.

# Group `fnmatch`

`fnmatch.fnmatch`, `fnmatch.fnmatchcase` and `fnmatch.filter` without building or running
a regular expression. Vectorcall sites on the existing function objects (contract kind C).

Code: `crates/rustique-py/src/targets/fnmatch/mod.rs`; the pattern port in
`crates/rustique-core/src/paths/glob.rs`. Tests: `tests/targets/test_fnmatch.py`.

## How it stays exact

CPython translates the shell pattern with `fnmatch._translate` /
`_join_translated_parts` into `(?s:PREFIX(?>.*?SEG)….*TAIL)\z`, compiles it with `re` and
matches. The kernel does not interpret shell patterns its own way: it

1. ports `_translate` statement by statement, including the bracket handling (`!` → `^`,
   a leading `]` kept literal, range chunks split at `-` with the `k+3` skip, removal of
   empty ranges, escaping of `\`, `-`, `&`, `~`, `|`, a leading `^`/`[` escaped), and builds
   the *set text* the regex would contain;
2. reads that set text the way `re._parser` reads a character set (escapes, ranges, a
   leading `]`), and refuses (falls back) on anything `re` would warn about
   (FutureWarning for possible nested sets / set operations), anything outside the
   grammar `_translate` produces, and a set that would end early;
3. evaluates the regex's meaning: every token matches exactly one code point (`?` and a
   negated empty range are `.` under DOTALL, an empty range is `(?!)`), so the prefix is
   fixed, each middle segment commits to its leftmost match (atomic lazy group), and the
   tail must end the string (`\z`).

The port is fuzzed against `re.compile(fnmatch.translate(p)).match` itself (str and bytes,
patterns built from `* ? [ ] ! - ^ \ & ~ |` and non-ASCII), and fingerprinted against
`re._parser._parse` and `_class_escape` so a change in `re`'s set syntax refuses the
targets.

## Exact subset

- Pattern: exact `str` or exact `bytes` (a subclass could hash/compare differently in the
  `lru_cache` of `_compile_pattern`).
- Name (`fnmatch`, `fnmatchcase`): for a str pattern any `str`, subclasses included
  (`os.path.normcase` = `os.fspath` returns them unchanged and `re` reads their buffer
  without calling methods); for a bytes pattern an exact `bytes` (a subclass could define
  `__buffer__`). `os.PathLike` names, bytearray/memoryview, mixed types fall back.
- `filter(names, pat)`: `names` an exact list or tuple whose items are all in the name
  subset (checked before anything happens; the list is read under its critical section on
  free-threaded builds, matched items are referenced before the result list is
  allocated). Iterators and other iterables fall back. The result holds the same
  objects in the same order.

Dependencies pinned: the whole `fnmatch` namespace (snapshot pin: `_compile_pattern`,
`translate`, `_translate`, `_join_translated_parts`, `_re_escape`, `_re_setops_sub`, `re`,
`os`, `posixpath`, and no new global shadowing a builtin), the builtins they use,
`re.compile`, `re._compile`, `os.path` (must be `posixpath`), `posixpath.normcase`,
`posixpath.os`, `os.fspath`. Fingerprints: those functions, `re.escape`,
`re._parser._parse`, `re._parser._class_escape`, `posixpath.normcase`.

## Private state skipped

- `fnmatch._compile_pattern`'s `lru_cache` is not populated (its `cache_info()` counts
  differ; private name).
- `re`'s compile cache (`re._cache`, cleared by `re.purge()`) is not populated.
- `fnmatch._re_escape`'s `lru_cache` is not populated.

No public API exposes these.

## Wave 2 (workstream `text`, 2026-11-02)

- **Compiled patterns: one process-wide, insert-only table** instead of wave 1's
  per-thread cache, which compared the pattern's content against up to 32 entries per call
  (the `any(fnmatch(f, p) for p in 12 patterns)` scan of `data/fs_inventory` paid ~150 Ir
  per call for it, w2-floors §1). 256 slots, 4 probes, keyed by the pattern object's own
  hash (`PyObject_Hash`: cached in the str/bytes after its first computation; CPython's
  original computes it too, as the key of `_compile_pattern`'s `lru_cache`) and confirmed
  by content (kind + code points), so a reused address or an equal pattern built at run
  time both resolve correctly. An entry is published once with a release CAS and never
  changed or freed: lookups are acquire loads, with no lock and no shared write
  (free-threaded builds: threads matching the same patterns never bounce a cache line).
  When all 4 probe slots hold other patterns, the pattern is compiled for that call only
  (correct, slower; tested with 1,000+ patterns and 4 threads). The table holds no Python
  objects (code points only); it lives as long as the process.
- **Pattern shapes.** A pattern made of literals and at most one `*` (`*.py`, `coverage.*`,
  `.DS_Store`, `a*b`) is matched by comparing its ends (`PRE*SUF`: length ≥ |PRE|+|SUF|,
  prefix and suffix equal by code point; no `*`: equality) instead of interpreting the
  program: the same result, since every literal token matches exactly its code point and
  `*` under DOTALL matches anything. Other patterns use the port's program as before.
- **`filter`** keeps matched names in a 64-entry inline buffer (heap beyond), no zeroing.
- **Platforms.** `fnmatchcase` is the same code on every OS: its declarations (the
  `fnmatch` namespace snapshot, `re.compile/_compile`, `re._parser`) stay in the group's
  shared prepare and it installs everywhere. `fnmatch` and `filter` normalize with
  `os.path.normcase` (`ntpath`'s `LCMapStringEx` lowering on Windows): their
  `os.path`-is-`posixpath` declarations moved to their own prepare, which refuses as
  `platform` off POSIX. Compile-checked for Windows/macOS/musl (posixpath.md §Wave 2);
  runtime validation needs dx's per-platform CI job.

Callgrind (3.15.1, wave 1 = bc2ce1a, wave 2 = ws2/text-paths on wave2 c9fb335):

| cell | CPython Ir | wave 1 Ir | wave 2 Ir | ratio (wave 2) |
|---|---:|---:|---:|---:|
| `fnmatch(f, '*.py')` | 4,828 | 629 | 522 | 0.108 |
| `fnmatch(f, 'coverage.*') miss` | 4,258 | 668 | 535 | 0.126 |
| `fnmatch(f, '[!t]*_[0-9]*.py')` | 4,258 | 660 | 625 | 0.147 |
| `fnmatch(fb, b'*.py') bytes` | 4,920 | 590 | 491 | 0.100 |
| `fnmatch(fs, '*.py') str subclass` | 4,714 | 648 | 537 | 0.114 |
| `fnmatch(Path, '*.py')` | 5,348 | 2,781 | 2,674 | 0.500 |
| `fnmatch(f, '*.py') os.fspath patched` | 5,300 | 5,285 | 5,255 | 0.992 |
| `fnmatch(f, '*.py') cProfile on` | 15,221 | 15,224 | 15,216 | 1.000 |
| `fnmatchcase(f, '*.py')` | 3,307 | 629 | 522 | 0.158 |
| `fnmatchcase(fs, '*.py') str subclass` | 3,305 | 648 | 537 | 0.162 |
| `any(fnmatch(f, pat) for 12 pats)` | 58,934 | 13,535 | 11,480 | 0.195 |
| `filter(32 names, '*.py')` | 61,975 | 5,287 | 3,604 | 0.058 |
| `filter(iter, '*.py')` | 63,373 | 64,251 | 59,771 | 0.943 |

8-thread rule (`benches/targets/paths_scaling.py`, 3.15.1t, enabled ÷ plain CPU per call at
8 threads, `local` / `shared`): `fnmatch` 0.046 / 0.064, `filter(32 names)` 0.053 / 0.102,
12-pattern scan 0.093 / 0.144. CPython's own `fnmatch` serializes on the critical section
of `_compile_pattern`'s `lru_cache` (CPU ×2.1–2.9 at 8 threads); the kernel's table has
no lock (×1.00–1.01 local).

## `glob` and `os.scandir` (wave 2: settled, with the numbers)

With Rustique on, `glob`/`os` are 54% of `data/fs_inventory`'s *cycles*
(w2-attribution §6). Measured (wave 2):

- **60% of the enabled run's cycles are system calls** (`getdents64`, `openat`,
  `close`, `fstat`, `lstat`/`stat`: the Phase-1 eBPF profile of the enabled run,
  `w2-attribution`), which user-space acceleration cannot touch.
- In **instructions** (callgrind, enabled, per section of `run()`), `glob.glob(**)` is
  379M of 843M: 36% `os.scandir` iteration and libc (`ScandirIterator_iternext`,
  `PyUnicode_DecodeFSDefaultAndSize`, `readdir`, `opendir`'s `malloc`, the GIL release
  around each syscall), 36% the interpreter running glob's **generators** (`_iglob`,
  `_glob2`, `_rlistdir`, `_iterdir`, the hidden-name generator expression), 15%
  allocation, 5% `sre` (below), 4% kernels (`join`, `relpath`, `x in tuple`).
- What a kernel could take, and why it does not:
  - the generators: the contract never replaces a function that returns a generator
    (ACCELERATION §2.6);
  - `glob._glob1` / `_listdir` are plain functions, but their work is `os.scandir`, which
    raises the `os.scandir` audit event: from a kernel the audit hooks (user code) would
    run without the `_glob1`/`_listdir`/`_iterdir` frames CPython has on the stack
    (ruling 15), and the interpreter exposes no public "are audit hooks installed" check;
  - `fnmatch.filter(names, pat)` inside `_glob1` receives the generator
    `(x for x in names if not _ishidden(x))`: iterating it from native code would run
    glob's Python code below the kernel (ruling 15), so `filter` falls back there (the
    5% `sre` above is CPython's own regex match; the `filter(iter, '*.py')` fallback
    cell stays at 0.943 in steady state);
  - `_join`/`_ishidden` are 2–5-bytecode functions inlined by the specializer: a
    vectorcall kernel would cost more than it saves.
- `Path.rglob` (190M) is the same story through `glob._StringGlobber`'s closures
  (`select_wildcard`, `select_recursive_step`) plus `Path` construction per result
  (`_from_parsed_string` → `with_segments` → `__new__`/`__init__` kernels).

Verdict: **settled** for this wave: the reachable user-space part of `glob`/`scandir` is
the kernels already in place (`join`, `relpath`, `fnmatch`, `x in tuple`), ≤ 4% of the
section; the rest is syscalls, C (`posix`), generator frames and allocation.

## Measurements (wave 1)

Callgrind as in docs/targets/posixpath.md (`benches/targets/paths_cg.py`, CPython 3.15.1).
`f` = `'test_api_12.py'`; `pats` = the 12 ignore patterns of `data/fs_inventory`; `names` =
32 file names. `fnmatch(Path, …)` and `os.fspath patched` run the original `fnmatch` (suspended)
whose inner `fnmatchcase` call is still accelerated.

| cell | CPython Ir | Rustique Ir | ratio | first 60 calls |
|---|---:|---:|---:|---:|
| `fnmatch(f, '*.py')` | 4828 | 625 | 0.130 |  |
| `fnmatch(f, 'coverage.*') miss` | 4258 | 663 | 0.156 |  |
| `fnmatch(f, '[!t]*_[0-9]*.py')` | 4258 | 655 | 0.154 |  |
| `fnmatch(fb, b'*.py') bytes` | 4920 | 586 | 0.119 |  |
| `fnmatch(fs, '*.py') str subclass` | 4714 | 644 | 0.137 | 0.116 |
| `fnmatch(Path, '*.py')` | 5348 | 2778 | 0.519 | 0.391 |
| `fnmatch(f, '*.py') os.fspath patched` | 5300 | 2618 | 0.494 | 0.345 |
| `fnmatch(f, '*.py') cProfile on` | 15197 | 15199 | 1.000 | 1.045 |
| `fnmatchcase(f, '*.py')` | 3307 | 625 | 0.189 |  |
| `fnmatchcase(fs, '*.py') str subclass` | 3305 | 644 | 0.195 | 0.141 |
| `any(fnmatch(f, pat) for 12 pats)` | 58929 | 13518 | 0.229 |  |
| `filter(32 names, '*.py')` | 61975 | 5319 | 0.086 |  |
| `filter(iter, '*.py')` | 63373 | 60947 | 0.962 | 0.969 |


Under `PYTHON_JIT=1` (3.15.1, warmed 30,000 iterations; steady state):

| cell | CPython Ir | Rustique Ir | ratio |
|---|---:|---:|---:|
| `fnmatch(f, '*.py')` | 4292 | 628 | 0.146 |
| `fnmatch(f, 'coverage.*') miss` | 3715 | 666 | 0.179 |
| `fnmatch(f, '[!t]*_[0-9]*.py')` | 3715 | 658 | 0.177 |
| `fnmatch(fb, b'*.py') bytes` | 4394 | 589 | 0.134 |
| `fnmatch(fs, '*.py') str subclass` | 4290 | 647 | 0.151 |
| `fnmatch(Path, '*.py')` | 4989 | 2509 | 0.503 |
| `fnmatch(f, '*.py') os.fspath patched` | 4707 | 2214 | 0.470 |
| `fnmatch(f, '*.py') cProfile on` | 15202 | 15226 | 1.002 |
| `fnmatchcase(f, '*.py')` | 3104 | 628 | 0.202 |
| `fnmatchcase(fs, '*.py') str subclass` | 3102 | 647 | 0.209 |
| `any(fnmatch(f, pat) for 12 pats)` | 50896 | 13516 | 0.266 |
| `filter(32 names, '*.py')` | 56561 | 5322 | 0.094 |
| `filter(iter, '*.py')` | 57621 | 59227 | 1.028 |

## Conformance

`tests/conformance/run_target_suites.py --targets posixpath,fnmatch,pathlib --modules
test.test_posixpath,test.test_genericpath,test.test_fnmatch,test.test_glob,test.test_pathlib,test.test_os`
(each module plain and with `enable(only=...)` before the module is imported, per-test-id
outcomes compared; all 24 targets of the three groups asserted installed). CPython's
`Lib/test` from the matching source tree.

| module | 3.15.1 | 3.15.1t | 3.14.9 | 3.14.9t |
|---|---|---|---|---|
| test_posixpath | 93 pass, 3 skip | same | 91 pass, 3 skip | same |
| test_genericpath | 26 pass | same | 25 pass | same |
| test_fnmatch | 24 pass | same | 24 pass | same |
| test_glob | 20 pass, 2 skip | same | 22 pass, 2 skip | same |
| test_pathlib | 991 pass, 407 skip | same | 966 pass, 403 skip | same |
| test_os | 472 pass, 77 skip, 3 fail, 1 error | same | 317 pass, 66 skip, 2 fail | same |
| **differences plain vs enabled** | **0** | **0** | **0** | **0** |

The `test_os` failures and error are the box's environment (identical without Rustique).
Plus `tests/targets/test_posixpath.py`, `test_fnmatch.py`, `test_pathlib.py` (differential
Hypothesis tests, identity rules, fallbacks, mechanics, threads) in the full `pytest -q` on
the four builds.


pyperf (wall time, `benches/targets/paths_pyperf.py`, 3.15.1, median of pyperf runs, plain vs enabled in separate processes):

| cell | CPython | Rustique | ratio |
|---|---:|---:|---:|
| fnmatch.filter(10k names, '*.py') | 403.1 µs | 32.3 µs | 0.080 |
| 12 ignore patterns x 1k names | 3719.6 µs | 809.9 µs | 0.218 |

## Workloads

`python benches/workloads/run.py compare data/fs_inventory devtools/pip_resolve
devtools/pytest_networkx --only posixpath,fnmatch,pathlib -n 5 -p 3` (3.15.1, shared dev box,
wall time, median of steady-state runs; digests identical plain vs enabled):

| workload | plain | enabled | ratio |
|---|---:|---:|---:|
| `data/fs_inventory` (os.walk + fnmatch, scandir, Path.rglob/relative_to/parts/stem, glob; 41% kernel time) | 312.2 ms | 231.7 ms | **0.742** |
| `devtools/pip_resolve` | 565.1 ms | 536.3 ms | 0.949 |
| `devtools/pytest_networkx` | 605.7 ms | 550.1 ms | 0.908 |

The three groups together; ratios on a shared box are indicative (±~5%).

## Startup

With `fnmatch` imported, `enable(only=["fnmatch"])` costs **0.28M Ir** (3.15.1, registry
warmed with an imported vectorcall target of another group; 0.34M before the
`GroupSpec`); 0.17M of it is the fingerprints of the `fnmatch`, `re` and `re._parser`
functions (`re._parser._parse` dominates). Everything the three targets share (the
`fnmatch` namespace snapshot, `re.compile/_compile`, the `os.path.normcase` chain, the
builtins) is one `GroupSpec` (`fnmatch::SHARED`, one dependency flag) declared once. When
`fnmatch` is imported after `enable()` the same work runs at its import (`python -c pass`
does not import it: no startup cost, see posixpath.md "Startup"). At exit its `release`
hook drops the references to the verified functions (its pattern caches hold no Python
objects).

## Version coverage

3.14 and 3.15 (`fnmatch.py` is identical in both; fingerprints recorded per minor).
Platforms (wave 2): `fnmatchcase` on every OS (compile-checked off Linux, pending dx's CI
job); `fnmatch` and `filter` on POSIX only (refusal category `platform` elsewhere).
`jit_safe: true` (see Measurements: under `PYTHON_JIT=1` the steady-state cells, fallbacks included, stay within 1.05; a fallback before suspension costs +250–350 Ir, inferred from docs/atlas/mechanisms.md §1.3).
