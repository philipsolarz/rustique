STATUS: wave 1, workstream `paths` (2026-11-01). Contract: docs/ACCELERATION.md.

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

## Measurements

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


## Conformance

CONFORMANCE_PLACEHOLDER

## Version coverage

3.14 and 3.15 (`fnmatch.py` is identical in both; fingerprints recorded per minor).
`jit_safe: false`.
