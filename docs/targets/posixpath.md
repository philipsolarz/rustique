STATUS: wave 1, workstream `paths` (2026-11-01). Contract: docs/ACCELERATION.md.

# Group `posixpath`

Rust kernels behind the pure-Python functions of `posixpath` (= `os.path` on POSIX),
installed with `PyFunction_SetVectorcall` on the existing function objects (contract kind
C): every captured reference (`from os.path import join`, `os.path.join`, `map(join, …)`)
is accelerated; identity, pickling, `__code__`, signature and source are unchanged.

Code: `crates/rustique-py/src/targets/posixpath/` (`mod.rs` kernels and targets, `rt.rs`
runtime shared with `fnmatch` and `pathlib`), algorithms in
`crates/rustique-core/src/paths/posix.rs` (with `cargo test`s). Tests:
`tests/targets/test_posixpath.py`. Benchmarks: `benches/targets/paths_cg.py`.

## Targets and exact subset

| target | exact subset (everything else runs CPython's code) |
|---|---|
| `posixpath.join(a, *p)` | positional args, all `str` or all exact `bytes`, no keywords |
| `posixpath.split(p)`, `splitext`, `basename`, `dirname`, `isabs` | one positional `str` or exact `bytes` |
| `posixpath.abspath(p)` | as above; bytes only under UTF-8 + surrogateescape filesystem encoding |
| `posixpath.relpath(path, start=None)` | `path` non-empty, `start` None or the same type; positional or `start=` keyword |

"`str`" means exact `str` (all PEP 393 kinds: ASCII, latin-1, UCS-2, UCS-4, lone
surrogates, embedded NUL) or a **plain str subclass**: one that overrides none of the
operations CPython's code applies to it (`startswith`, `endswith`, `rfind`, `__class__`,
slicing, concatenation, truthiness, comparison; checked per call through `_PyType_Lookup`
and the type's slots) and whose instance `__dict__` is absent or empty (an instance
attribute would shadow a method). Bytes subclasses, `os.PathLike` objects, mixed
str/bytes, empty `relpath`, any error, all fall back.

Guards per call (≤ 40 Ir): exact shape, one relaxed load of the target's dependency flag,
the tools guard (global `PY_START` listeners, local events on the function's code).

Dependencies pinned (dict watcher; a change deopts the target until restored):
`posixpath.os`, `os.fspath`, `posixpath._get_sep`, the builtins the code resolves
(`isinstance`, `bytes`, `len`, …, via `pin_global`, which also pins "not shadowed in the
module"), and per target: `posixpath.genericpath` + `genericpath._splitext` (splitext);
`os.getcwd`/`os.getcwdb`, `posixpath.join`, `posixpath.normpath` (abspath, relpath);
`posixpath.abspath`, `genericpath._commonprefix` (3.15) / `posixpath.commonprefix` (3.14)
and the builtins of `genericpath` (relpath). Fingerprints: every Python function the
kernels reproduce (`join`, `_get_sep`, `_splitext`, `abspath`, `relpath`,
`(_)commonprefix`, …) and the C functions they call (`os.fspath`, `os.getcwd(b)`,
`posix._path_normpath`), per minor version.

### Identity rules reproduced

- `join`: returns an argument itself where CPython's concatenations do (`join(a)`, an
  absolute or empty-path reset `join(x, '/abs')`, `join('a/', '')` on exact str); with a
  str subclass on either side of `path += ''` CPython goes through `PyUnicode_Concat`,
  which returns a fresh exact copy, and so does the kernel. Results are built in one
  allocation with the widest kind of the pieces (what chained `PyUnicode_Concat` gives).
- Slices go through `PyUnicode_Substring` / `PyBytes_FromStringAndSize`: the argument
  itself for the whole of an exact object, the shared empty and one-character singletons,
  fresh objects otherwise — exactly CPython's slicing.
- `splitext` without extension returns `(p, p[:0])` (the argument itself).
- `relpath` returns `relpath.__code__`'s own `'.'`/`'..'` (and bytes) constants where
  CPython does; single components come from the same constructors as `str.split`'s.
- `abspath`/`relpath` normalize with a statement-by-statement port of C
  `_Py_normpath_and_size` (POSIX branch), self-tested against `posix._path_normpath` at
  install and fuzzed in the tests; the current directory comes from the pinned
  `os.getcwd` (a C function: no Python code, no audit event). If it fails, the kernel
  declines and CPython raises.

### Private state skipped

None: these functions keep no state.

## Fallback cost, adaptive suspension

A vectorcall fallback costs +210–270 Ir per call (the caller loses the inlined frame push;
docs/atlas/mechanisms.md §1.2). For functions of 1.5–4.5k Ir that is 1.05–1.15 on any input
the kernel cannot run. Inputs it cannot run without changing observable behaviour:

- user-defined `os.PathLike` objects (calling their `__fspath__` from Rust would drop the
  posixpath frame from tracebacks and from the stack `__fspath__` sees);
- a tool listening (profilers, tracers, debuggers on the function);
- a patched dependency.

The framework handles these classes (lead ruling, 2026-11-01): a patched dependency
suspends the target until it is restored (deopt while unpinned), and 64 consecutive
fallbacks suspend it until the next `enable()` (adaptive suspension). Steady-state cells of
these classes are therefore measured after ≥ 2,500 calls; the cost before suspension (the
first 60 calls after `enable()`) is reported separately.

## Measurements

Callgrind, region-instrumented, Ir per call = (Ir(2n) − Ir(n)) / n, statements unrolled
10×, each cell in a fresh process (`benches/targets/paths_cg.py`); CPython 3.15.1,
dev-release build, dev box (c7a.8xlarge). `orig` = plain CPython, `rs` = `enable(only=["posixpath"])`.

Callgrind, region-instrumented, Ir per call = (Ir(2n) − Ir(n)) / n after ≥ 2,500 warm-up
calls, statements unrolled 10×, each cell in a fresh process (`benches/targets/paths_cg.py`);
CPython 3.15.1, `--profile dev-release`, dev box. "CPython" = plain; "Rustique" =
`rustique.enable(only=["posixpath"])`. Steady state: fallback-class cells (user PathLike,
`pathlib.Path` arguments, patched dependency, cProfile active) show the target suspended by
the framework (≈ 1.00). "First 60 calls" = Ir per call over the first 60 calls after
`enable()` in a fresh process (no warm-up, `--first`): the cost before suspension.
`d`, `f`, `p` are a 36-char directory, a 14-char file name and their join; `pb`/`db`/`fb`
bytes; `ps`/`ds`/`fs` instances of `class S(str): pass`; `PL` a minimal user PathLike.

| cell | CPython Ir | Rustique Ir | ratio | first 60 calls |
|---|---:|---:|---:|---:|
| `join(d, f)` | 3540 | 977 | 0.276 |  |
| `join(root, 'src', 'pkg', f)` | 7339 | 1362 | 0.186 |  |
| `join(d, '/abs')` | 2380 | 600 | 0.252 |  |
| `join(db, fb) bytes` | 3582 | 711 | 0.199 |  |
| `join(latin1, f)` | 3565 | 993 | 0.279 |  |
| `join(ucs2, ucs4)` | 2366 | 591 | 0.250 |  |
| `join(ds, fs) str subclass` | 3920 | 2309 | 0.589 | 0.555 |
| `join(PathLike, f)` | 4165 | 4165 | 1.000 | 1.048 |
| `join(Path, f)` | 4177 | 4177 | 1.000 | 1.050 |
| `join(d, f) os.fspath patched` | 4014 | 4014 | 1.000 | 1.001 |
| `join(d, f) _get_sep patched` | 3603 | 3631 | 1.008 | 1.001 |
| `join(d, f) cProfile on` | 13853 | 13852 | 1.000 | 1.046 |
| `split(p)` | 3951 | 1099 | 0.278 |  |
| `split(pb) bytes` | 4051 | 1010 | 0.249 |  |
| `split(ps) str subclass` | 4455 | 1775 | 0.398 | 0.420 |
| `split(PathLike)` | 4576 | 4557 | 0.996 | 1.049 |
| `split(Path)` | 4583 | 4564 | 0.996 | 1.049 |
| `split(p) os.fspath patched` | 4188 | 4169 | 0.995 | 0.997 |
| `split(p) cProfile on` | 14482 | 14481 | 1.000 | 1.043 |
| `splitext(f)` | 3921 | 1086 | 0.277 |  |
| `splitext(p)` | 3872 | 1087 | 0.281 |  |
| `splitext(pb) bytes` | 4140 | 979 | 0.237 |  |
| `splitext(pw) ucs2` | 4321 | 1303 | 0.302 |  |
| `splitext(ps) str subclass` | 4705 | 1763 | 0.375 | 0.409 |
| `splitext(PathLike)` | 4497 | 4516 | 1.004 | 1.059 |
| `splitext(Path)` | 4504 | 4523 | 1.004 | 1.058 |
| `splitext(p) os.fspath patched` | 4109 | 4109 | 1.000 | 1.001 |
| `basename(p)` | 2047 | 601 | 0.294 |  |
| `basename(pb) bytes` | 1986 | 557 | 0.281 |  |
| `basename(ps) str subclass` | 2375 | 1276 | 0.537 | 0.550 |
| `basename(PathLike)` | 2672 | 2672 | 1.000 | 1.084 |
| `basename(Path)` | 2679 | 2679 | 1.000 | 1.083 |
| `basename(p) os.fspath patched` | 2284 | 2284 | 1.000 | 1.002 |
| `basename(p) cProfile on` | 9400 | 9400 | 1.000 | 1.066 |
| `dirname(p)` | 3385 | 613 | 0.181 |  |
| `dirname(pb) bytes` | 3389 | 548 | 0.162 |  |
| `dirname(ps) str subclass` | 3704 | 1288 | 0.348 | 0.357 |
| `dirname(PathLike)` | 4010 | 3991 | 0.995 | 1.054 |
| `dirname(p) os.fspath patched` | 3622 | 3603 | 0.995 | 0.997 |
| `isabs(p)` | 1527 | 306 | 0.201 |  |
| `isabs(pb) bytes` | 1389 | 290 | 0.209 |  |
| `isabs(ps) str subclass` | 1670 | 982 | 0.588 | 0.594 |
| `isabs(PathLike)` | 2152 | 2152 | 1.000 | 1.102 |
| `isabs(p) os.fspath patched` | 1764 | 1764 | 1.000 | 1.003 |
| `normcase(p)` | 517 | 517 | 1.000 |  |
| `normcase(PathLike)` | 1142 | 1142 | 1.000 | 1.004 |
| `relpath(p, root)` | 20872 | 6726 | 0.322 |  |
| `relpath(pb, rootb) bytes` | 21428 | 4949 | 0.231 |  |
| `relpath(f) relative` | 28418 | 5420 | 0.191 |  |
| `relpath(ps, root) str subclass` | 21203 | 7425 | 0.350 | 0.352 |
| `relpath(p, root) os.fspath patched` | 21817 | 21820 | 1.000 | 1.000 |
| `abspath(p)` | 3071 | 2190 | 0.713 |  |
| `abspath(f) relative` | 7417 | 3349 | 0.452 |  |
| `abspath(ps) str subclass` | 3211 | 2866 | 0.893 | 0.810 |
| `commonpath([p, d])` | 23109 | 23080 | 0.999 |  |
| `commonpath((pb, db)) bytes` | 24737 | 24729 | 1.000 |  |


## Conformance

CONFORMANCE_PLACEHOLDER

## Version coverage

3.14 and 3.15 (fingerprints recorded for both; identical on GIL and free-threaded builds).
Free-threaded builds: the kernels read immutable str/bytes only. `jit_safe: false`: under
`PYTHON_JIT=1` a fallback costs +250–350 Ir (refused with reason "jit").
