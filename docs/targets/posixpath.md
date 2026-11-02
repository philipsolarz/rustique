STATUS: wave 1, workstream `paths` (2026-11-01); wave 2, workstream `text` (2026-11-02: §Wave 2).
Contract: docs/ACCELERATION.md.

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

Dependencies pinned once for the group (`GroupSpec` `posixpath::SHARED`, 20 pins, one
dependency flag; a change sends all eight targets to CPython's code until restored):
`posixpath.os/_get_sep/join/normpath/abspath/genericpath`, `os.fspath/getcwd/getcwdb`,
`genericpath._splitext`, `genericpath._commonprefix` (3.15) / `posixpath.commonprefix`
(3.14), and the builtins the code resolves (`isinstance`, `bytes`, `len` in `posixpath`;
`isinstance`, `list`, `tuple`, `min`, `max`, `enumerate` in `genericpath`; `pin_global`
also pins "not shadowed in the module"). One flag instead of one per target: patching
`os.getcwd` (say) now also sends `join` to the original while patched (a `Deps` fallback:
+3 Ir, no suspension). Fingerprints: every Python function the kernels reproduce (`join`,
`_get_sep`, `_splitext`, `abspath`, `relpath`, `(_)commonprefix`, …) and the C functions
they call (`os.fspath`, `os.getcwd(b)`, `posix._path_normpath`), per minor version, once
per object.

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
suspends the target until it is restored (deopt while unpinned), and the net-benefit
balance suspends it until the next `enable()` (adaptive suspension: a hit credits 4, any
other fallback debits 1, suspended at −64, so after 64 fallbacks without a hit; a target
stays installed while ≥ 20% of its calls hit). Steady-state cells of
these classes are therefore measured after ≥ 2,500 calls; the cost before suspension (the
first 60 calls after `enable()`) is reported separately: 1.044–1.059 for join, split,
splitext and dirname with a user PathLike or `pathlib.Path` argument or cProfile active,
1.065–1.105 for basename and isabs (the cheapest functions: 2.0k / 1.5k Ir). Mixed traffic
(exact str with occasional fallbacks, ≥ 20% hits) is never suspended and stays far below 1.

`normcase` (517 Ir: a fallback would cost 1.4×, and its only common caller, `fnmatch`, is
accelerated as a whole), `commonpath`, `commonprefix` (3.15: emits a DeprecationWarning),
`normpath` (already C), `expanduser`/`expandvars` (environment, `pwd`) are not targets;
the table lists `normcase` and `commonpath` to show they are untouched.

## Wave 2 (workstream `text`, 2026-11-02)

What changed (same exact subset, same identity rules, same fallbacks):

- **`abspath`/`relpath` without per-call heap.** Wave 1 copied every argument and the
  current directory into `Vec<u32>`s, normalized into a fresh `Vec`, split components into
  two `Vec<&[u32]>`s and built the result in another `Vec` (~40% of the call in
  `malloc`/`free`/`realloc`, w2-floors §1). Now:
  - an argument that is already an absolute normalized path (one leading `/`, no empty,
    `.` or `..` component, no trailing `/`: `posix::is_normal_abs`, one pass) is read in
    place: `_Py_normpath_and_size` would copy it character by character to the same
    positions (exhaustively checked against the port on every string of ≤ 7 characters
    over `/`, `.`, `a`, plus the Hypothesis differential tests);
  - anything else (relative paths joined to `os.getcwd()`, `.`/`..`/`//`) is normalized in
    a stack scratch buffer (`u8` when every source is one byte per unit, else `u32`; one
    heap buffer only beyond 512 units) by the same port, now in place
    (`posix::normpath_in_place`, as the C function works);
  - `relpath` finds the common component prefix in **one scan** of the two normalized
    strings (code points compared across widths; a component boundary is a `/` or the end
    on both sides), then writes `'../' * ups + rest` straight into the result object.
    The result objects are what CPython builds (`'.'`/`'..'` code constants by identity,
    a single component as a `split` substring would be: narrowest kind, latin-1
    singletons; `PyUnicode_Join`'s kind otherwise).
- **`join(a, b)` of two exact `str`**: the plan arrays of the general path (8 views, 8
  join args, 8 pieces, initialised per call) are skipped; CPython's loop for one step
  (`b` absolute or `a` empty → `b` itself; `a` ends with `/` and `b` empty → `a` itself;
  else one allocation). `new_str` takes the data pointer from the calibrated layout.
- **Platforms.** The six string-only functions (`join split splitext basename dirname
  isabs`) are platform-independent code (`posixpath` is importable on Windows and used
  explicitly by `zipfile._path`, `mimetypes`, `http.server`); the group's shared prepare
  now declares only what they read (`os.fspath`, `_get_sep`, `genericpath._splitext`,
  builtins) and installs on every OS. `abspath`/`relpath` add their own declarations
  (`os.getcwd(b)`, the C `posix._path_normpath`, `join`, `abspath`, `(_)commonprefix`,
  `relpath`'s constants; pinned on the same group flag) and refuse as `platform` off
  POSIX (Windows' `posixpath.normpath` is the Python fallback and `getcwd` returns
  `C:\…`). The paths runtime (`rt.rs`, shared with `fnmatch`/`pathlib`) reads the
  kind/compact/ascii bits from the `PyASCIIObject.state` unit at offset 4 on MSVC
  free-threaded builds (MSVC starts a new bitfield unit after `unsigned char interned`;
  the first unit there is `interned` plus three never-written padding bytes): the
  calibration verifies the bits and never searches uninitialised memory. Checked with
  `cargo check -p rustique-py --lib` for `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`
  and `x86_64-unknown-linux-musl` × {3.14, 3.15, 3.15t} (dx's cross toolchain on the dev
  box, compile-time layout assertion included); **runtime validation off Linux needs dx's
  per-platform CI job** before these count as "on" there (Windows: +6 targets of this
  group, +1 `fnmatchcase`).

Callgrind (same method as §Measurements; 3.15.1, ws2/text-paths on wave2 c9fb335; "wave 1"
is the same bench on bc2ce1a). The uniform +13 Ir on untouched kernels (`split`,
`basename`, …) comes with the merged framework (accel merge A), not this group:

| cell | CPython Ir | wave 1 Ir | wave 2 Ir | ratio (wave 2) |
|---|---:|---:|---:|---:|
| `join(d, f)` | 3,540 | 973 | 705 | 0.199 |
| `join(root, 'src', 'pkg', f)` | 7,339 | 1,356 | 1,376 | 0.187 |
| `join(d, '/abs')` | 2,380 | 600 | 416 | 0.175 |
| `join(db, fb) bytes` | 3,582 | 717 | 735 | 0.205 |
| `join(latin1, f)` | 3,565 | 989 | 722 | 0.202 |
| `join(ucs2, ucs4)` | 2,366 | 590 | 412 | 0.174 |
| `join(ds, fs) str subclass` | 3,920 | 2,305 | 2,324 | 0.593 |
| `join(PathLike, f)` | 4,165 | 4,165 | 4,165 | 1.000 |
| `join(Path, f)` | 4,177 | 4,177 | 4,177 | 1.000 |
| `join(d, f) os.fspath patched` | 4,014 | 4,014 | 4,014 | 1.000 |
| `join(d, f) _get_sep patched` | 3,603 | 3,603 | 3,603 | 1.000 |
| `join(d, f) cProfile on` | 13,853 | 13,853 | 13,853 | 1.000 |
| `split(p)` | 3,951 | 1,103 | 1,116 | 0.282 |
| `split(pb) bytes` | 4,051 | 996 | 1,011 | 0.249 |
| `split(ps) str subclass` | 4,455 | 1,779 | 1,792 | 0.402 |
| `split(PathLike)` | 4,576 | 4,557 | 4,557 | 0.996 |
| `split(Path)` | 4,583 | 4,564 | 4,564 | 0.996 |
| `split(p) os.fspath patched` | 4,188 | 4,169 | 4,169 | 0.995 |
| `split(p) cProfile on` | 14,482 | 14,482 | 14,482 | 1.000 |
| `splitext(f)` | 3,921 | 1,072 | 1,085 | 0.277 |
| `splitext(p)` | 3,872 | 1,073 | 1,086 | 0.280 |
| `splitext(pb) bytes` | 4,140 | 967 | 980 | 0.237 |
| `splitext(pw) ucs2` | 4,321 | 1,289 | 1,302 | 0.301 |
| `splitext(ps) str subclass` | 4,691 | 1,749 | 1,762 | 0.376 |
| `splitext(PathLike)` | 4,497 | 4,497 | 4,497 | 1.000 |
| `splitext(Path)` | 4,504 | 4,504 | 4,504 | 1.000 |
| `splitext(p) os.fspath patched` | 4,109 | 4,109 | 4,109 | 1.000 |
| `basename(p)` | 2,047 | 604 | 617 | 0.301 |
| `basename(pb) bytes` | 1,986 | 543 | 556 | 0.280 |
| `basename(ps) str subclass` | 2,375 | 1,279 | 1,292 | 0.544 |
| `basename(PathLike)` | 2,672 | 2,672 | 2,672 | 1.000 |
| `basename(Path)` | 2,679 | 2,679 | 2,679 | 1.000 |
| `basename(p) os.fspath patched` | 2,284 | 2,284 | 2,284 | 1.000 |
| `basename(p) cProfile on` | 9,400 | 9,400 | 9,400 | 1.000 |
| `dirname(p)` | 3,385 | 616 | 629 | 0.186 |
| `dirname(pb) bytes` | 3,389 | 552 | 567 | 0.167 |
| `dirname(ps) str subclass` | 3,704 | 1,291 | 1,304 | 0.352 |
| `dirname(PathLike)` | 4,010 | 3,991 | 3,991 | 0.995 |
| `dirname(p) os.fspath patched` | 3,622 | 3,603 | 3,603 | 0.995 |
| `isabs(p)` | 1,527 | 310 | 323 | 0.211 |
| `isabs(pb) bytes` | 1,389 | 294 | 307 | 0.221 |
| `isabs(ps) str subclass` | 1,670 | 986 | 999 | 0.598 |
| `isabs(PathLike)` | 2,152 | 2,152 | 2,152 | 1.000 |
| `isabs(p) os.fspath patched` | 1,764 | 1,764 | 1,764 | 1.000 |
| `normcase(p)` | 517 | 517 | 517 | 1.000 |
| `normcase(PathLike)` | 1,142 | 1,142 | 1,142 | 1.000 |
| `relpath(p, root)` | 20,849 | 6,993 | 1,635 | 0.078 |
| `relpath(pb, rootb) bytes` | 21,428 | 5,120 | 1,429 | 0.067 |
| `relpath(f) relative` | 28,916 | 6,511 | 3,494 | 0.121 |
| `relpath(ps, root) str subclass` | 21,180 | 7,692 | 2,336 | 0.110 |
| `relpath(p, root) os.fspath patched` | 21,797 | 21,773 | 21,773 | 0.999 |
| `abspath(p)` | 3,071 | 2,194 | 936 | 0.305 |
| `abspath(f) relative` | 7,603 | 4,142 | 2,458 | 0.323 |
| `abspath(ps) str subclass` | 3,211 | 2,870 | 1,612 | 0.502 |
| `commonpath([p, d])` | 23,109 | 23,061 | 23,085 | 0.999 |
| `commonpath((pb, db)) bytes` | 24,734 | 24,686 | 24,737 | 1.000 |

Every fallback and reject cell stays ≤ 1.000 in steady state (unchanged).

**8-thread rule** (DECISIONS §10.9; `benches/targets/paths_scaling.py run [--enable] -t 1,8
-r 3`, 3.15.1t, CPU time per call summed over threads): enabled ÷ plain CPU per call at 8
threads, `local` / `shared` inputs: `join` 0.197 / 0.198, `join` fallback (PurePosixPath)
0.928 / 0.586, `relpath` 0.065 / 0.065, `relpath` relative (`os.getcwd`) 0.277 / 0.277,
`abspath` 0.474 / 0.490, `splitext` 0.301 / 0.309: all ≤ 1.05; the kernels' own CPU per
call at 8 threads is 0.99–1.06× their 1-thread cost (no shared writes; the scratch buffers
are on the stack, so the glibc arena locks that cost `urllib.quote` ×19.6 are never taken).

**Workload** (`benches/targets/text_wl.py data/fs_inventory -r 3`, instructions:u per
steady-state `run()`, digests identical): with all groups 0.492 → **0.462** (enabled
966.8M → 899.4M Ir per run), with `--only posixpath,pathlib,fnmatch` 0.496 → 0.463.
Per section of the run (callgrind, enabled, Ir): `os.walk` + `fnmatch` + `relpath`
205M → 158M; `os.scandir` recursion (user code) 117M → 116M; `Path.rglob` +
`relative_to` 190M → 190M; `glob.glob(**)` 397M → 379M. What is left is fnmatch.md
§"glob and os.scandir".

## Measurements (wave 1)

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


Under `PYTHON_JIT=1` (3.15.1, warmed 30,000 iterations; steady state):

| cell | CPython Ir | Rustique Ir | ratio |
|---|---:|---:|---:|
| `join(d, f)` | 3471 | 981 | 0.283 |
| `join(root, 'src', 'pkg', f)` | 7184 | 1364 | 0.190 |
| `join(d, '/abs')` | 2330 | 603 | 0.259 |
| `join(db, fb) bytes` | 3514 | 715 | 0.203 |
| `join(latin1, f)` | 3496 | 997 | 0.285 |
| `join(ds, fs) str subclass` | 3879 | 2313 | 0.596 |
| `join(PathLike, f)` | 4155 | 3566 | 0.858 |
| `join(Path, f)` | 4157 | 3568 | 0.858 |
| `join(d, f) os.fspath patched` | 3780 | 3780 | 1.000 |
| `join(d, f) _get_sep patched` | 3596 | 3596 | 1.000 |
| `join(d, f) cProfile on` | 13854 | 13854 | 1.000 |
| `split(p)` | 3257 | 1117 | 0.343 |
| `split(p) os.fspath patched` | 3455 | 3450 | 0.999 |
| `split(p) cProfile on` | 14468 | 14468 | 1.000 |
| `splitext(p)` | 2962 | 1091 | 0.368 |
| `splitext(p) os.fspath patched` | 3170 | 3170 | 1.000 |
| `basename(p)` | 1590 | 605 | 0.381 |
| `basename(p) os.fspath patched` | 1816 | 1816 | 1.000 |
| `basename(p) cProfile on` | 9403 | 9403 | 1.000 |
| `dirname(p)` | 2885 | 631 | 0.219 |
| `dirname(p) os.fspath patched` | 2986 | 2967 | 0.994 |
| `isabs(p)` | 1142 | 310 | 0.272 |
| `isabs(p) os.fspath patched` | 1368 | 1368 | 1.000 |
| `relpath(p, root)` | 18091 | 6733 | 0.372 |
| `relpath(pb, rootb) bytes` | 19081 | 4953 | 0.260 |
| `relpath(ps, root) str subclass` | 19188 | 7432 | 0.387 |
| `relpath(p, root) os.fspath patched` | 18880 | 18875 | 1.000 |
| `abspath(p)` | 2691 | 2194 | 0.815 |

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
| join + splitext over 1k files | 532.7 µs | 160.6 µs | 0.302 |
| relpath over 1k files | 1459.6 µs | 413.4 µs | 0.283 |
| split/dirname/basename over 1k | 644.5 µs | 166.8 µs | 0.259 |

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

`benches/targets/_prof_install.py` (callgrind, 3.15.1, the registry warmed first with an
imported vectorcall target of another group so one-time calibrations are not counted; wave1
672009b + release hooks). `enable(only=["posixpath"])` with `posixpath` imported (it always
is) costs **0.22M Ir** (0.36M before the `GroupSpec`): eight vectorcall sites, 11
Python-function and 4 C-function fingerprints (≈ 0.12M), 20 pins (one `GroupSpec`), the
normpath identity check. Nothing is imported. Still over the 0.1M budget: the fingerprints
dominate. Pre-interned keys (`pin_s`/`pin_global_s` with `intern!`) measured *slower* here
(each `intern!` site initialises once per process, while `pin`'s interning cache is shared
with the other groups), so the groups keep `pin`.

Whole process (`benches/targets/_whole.sh`: `python -c pass` through the `.pth` loader,
callgrind totals, `PYTHONHASHSEED=0` (string hashing otherwise moves the totals by
±0.15M), RUSTIQUE=1 vs RUSTIQUE=1 with `RUSTIQUE_EXCLUDE=posixpath,fnmatch,pathlib`): the
paths groups add **0.30M Ir** (+0.75% of 39.9M; seed 1: the same). With `os._exit(0)` at
the end (no interpreter teardown) they add 0.26M: posixpath's enable (fnmatch and pathlib
stay pending) and the first install's framework work. Teardown is now ≈ 0.04M. It was
≈ 0.75M before accel-core's exit hook (sites restored, watchers stopped at finalization)
and the groups' `release` hooks, which drop the references the groups keep themselves:
verified functions (whose globals kept the `posixpath`/`genericpath`/`pathlib` dicts
alive), `os.getcwd(b)` (its `__self__` is `posix`), the pathlib classes and their recorded
attributes. Kernels entered on a stale path after the release decline: their flags are
false, the type cells are null, and `cwd_units` checks for a null `os.getcwd`
(`test_exit_release_and_daemon_threads`).

## Version coverage

3.14 and 3.15 (fingerprints recorded for both; identical on GIL and free-threaded builds).
Platforms (wave 2): `join`, `split`, `splitext`, `basename`, `dirname`, `isabs` on every OS
(runtime-validated on Linux; Windows/macOS/musl compile-checked, pending dx's per-platform CI
job); `abspath`, `relpath` on POSIX only (refusal category `platform` elsewhere).
Free-threaded builds: the kernels read immutable str/bytes only. `jit_safe: true`: under
`PYTHON_JIT=1` the steady-state cells stay within 1.05 (table below); a fallback before
suspension costs +250–350 Ir (docs/atlas/mechanisms.md §1.3).
