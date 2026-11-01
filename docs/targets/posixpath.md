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

RESULTS_PLACEHOLDER

## Conformance

CONFORMANCE_PLACEHOLDER

## Version coverage

3.14 and 3.15 (fingerprints recorded for both; identical on GIL and free-threaded builds).
Free-threaded builds: the kernels read immutable str/bytes only. `jit_safe: false`: under
`PYTHON_JIT=1` a fallback costs +250–350 Ir (refused with reason "jit").
