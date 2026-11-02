STATUS: wave 1, workstream `paths` (2026-11-01); wave 2, workstream `text` (2026-11-02: §Wave 2).
Contract: docs/ACCELERATION.md.

# Group `pathlib`

The pure-path core of `pathlib.PurePosixPath` / `PosixPath` (and so `Path()` on POSIX):
vectorcall sites on the functions in `PurePath.__dict__` (methods, and the `fget` of
properties). Code: `crates/rustique-py/src/targets/pathlib/mod.rs` (str buffers and
builders shared with `posixpath`, `rt.rs`). Tests: `tests/targets/test_pathlib.py`.

## Targets

| target | reached through | exact subset |
|---|---|---|
| `pathlib.PurePath.__new__`, `pathlib.Path.__new__` | `PurePosixPath(...)`, `Path(...)` (`slot_tp_new`: fallback +3 Ir) | `cls` is `PurePath`/`Path` (→ the POSIX class: `os.name` pinned to 'posix') or exactly `PurePosixPath`/`PosixPath`; no keywords |
| `pathlib.PurePath.__init__` | the same calls (`slot_tp_init`) | receiver of those classes; arguments exact str or exact POSIX paths (their `_raw_paths` are extended, as `isinstance(arg, PurePath)` with the same parser does) |
| `pathlib.PurePath.__str__` (3.15: also `__fspath__`, `__vfspath__`, the same function) | `str(p)`, f-strings, `os.fspath(p)` (slot / C callers: fallback +3 Ir) | receiver exactly `PurePosixPath` or `PosixPath` |
| `pathlib.PurePath.__truediv__` | `p / x` (slot) | key an exact `str` or an exact path of those two classes |
| `pathlib.PurePath.__eq__`, `__hash__` | `==`, `hash()`, dict/set (slots) | both operands of those classes |
| `pathlib.PurePath.relative_to` | method call | `other` an exact str or path of those classes; no `walk_up` |
| `pathlib.PurePath.parent`, `name`, `suffix`, `stem`, `parts` (3.15 only) | properties | receiver of those classes |

Common to all: the receiver's private slots hold what pathlib itself stores (`_raw_paths`
an exact list of exact str; parsed slots exact str / list; `_drv` empty), else fallback.

## State: filled exactly as CPython fills it

PurePath keeps its parsed state in private `__slots__` that its properties fill lazily.
The kernels compute the same values and store them in the same slots (read and written
under the object's critical section on free-threaded builds):

- `_drv`, `_root`, `_tail_cached` (what `_parse_path(_raw_path)` returns: `_raw_path`
  joins the raw paths with `posixpath.join` semantics, the root follows C `_Py_skiproot`,
  the tail is `[x for x in rel.split('/') if x and x != '.']`);
- `_str` (`_format_parsed_parts(...) or '.'`), `_str_normcase_cached` (the `_str` object
  itself on POSIX), `_hash` (`hash(_str_normcase)`);
- new objects (`Path(...)`, `p / x`, `parent`, `relative_to`) are created as
  `type(self)(...)` creates them (`object.__new__(cls)`, then the slots `__init__` /
  `_from_parsed_parts` / `_from_parsed_string` set): `_raw_paths` (which `pickle`/`copy` expose through
  `__reduce__`), and for derived paths `_str`, `_drv`, `_root`, `_tail_cached`.

Object sharing is reproduced where CPython shares: `_str` is the single tail item of a
one-component relative path, the root object of a root-only path, `'.'` for an empty
path; `_root`/`_drv` are the shared `''`/`'/'` singletons (`'//'` a fresh string);
`parts` returns the tail items themselves; `parent` of a path without tail is the path
itself; `relative_to` also fills the slots CPython's comparisons fill on `self` and on a
path `other` (`_str`, `_str_normcase_cached`, parsed slots). The differential test runs
random operation sequences with the kernels on and off and compares results *and* every
slot of every object involved, including which positions hold the same object.

## Guards and pins

Both exact classes must have the default metaclass and `parser is posixpath`. The class
attributes the originals reach through `self`/`cls` (`__new__`, `__init__`, `with_segments`,
`parser`, `_parse_path`, `_format_parsed_parts`, `_from_parsed_parts`, `_from_parsed_string`,
`_raw_path`, `drive`, `root`, `_tail`, `name`, `parents`, `__eq__`, `_str_normcase`,
`__str__`, `__hash__`) are guarded per call by the class's **type version tag**: CPython
gives a class a new tag whenever it or any base is modified (`__bases__` included). On a
new tag the kernel looks the attributes up again (`_PyType_Lookup`, no Python code) and
compares them with the validated (fingerprinted) objects: if a patch was undone they are the
same and the new tag is accepted; otherwise the call falls back. So patching
`PurePath.with_segments` (the public subclass hook) or any of these on a class sends calls
to CPython's code; `_PathParents` (iterated by `relative_to`'s `path in self.parents`) is
guarded the same way. Pinned (dict watcher): `posixpath.sep/altsep/join/splitroot/
splitdrive/_get_sep/os`, the pathlib globals each original reads (`os`, `posixpath`,
`PurePath`, `PurePosixPath`, `PosixPath`, `Path`, `PureWindowsPath`, `WindowsPath`, `chain`,
`_PathParents`), their builtins, `os.fspath` (called by `__init__`, `posixpath.join`,
`posixpath.splitdrive`) and `os.name` (read by `__new__`): 31 pins, one `GroupSpec`
(`pathlib::SHARED`, one dependency flag for the thirteen targets) with the validation.
Fingerprints: all 29 functions involved, once.

Subclasses (including any overriding `with_segments`, `__init__` or `parser`),
`PureWindowsPath`, and all other inputs run CPython's code. Reached through slots
(`__new__`, `__init__`, `__str__`, `__truediv__`, `__eq__`, `__hash__`) that costs +3 Ir;
for the properties and `relative_to` a fallback costs +210–300 Ir: the framework suspends a
target after 64 fallbacks without a hit (net-benefit balance; steady state ≈ 1.00, measured), and the first 60
calls on a subclass or a `PureWindowsPath` cost 1.27–1.28 for `name`/`suffix` (0.7–2k Ir
properties), 1.05 for `parent`. Plain-subclass support (as `posixpath` has for str) would
remove that; not done in wave 1.

## Private state skipped

None: the kernels fill the same private slots with the same values. Not filled where
CPython would not fill them either.

## Wave 2 (workstream `text`, 2026-11-02)

Same targets, subset and stored state. Changed: no per-call Rust heap in parsing and
formatting. `_parse_path` of several raw segments joins them (as `parser.join` would) into
a stack buffer of code points (heap only beyond 512 units or 16 segments) instead of a
`Vec<u32>` plus three plan `Vec`s; one segment is parsed in place; `_format_parsed_parts`
measures and writes in two passes over the tail list (no `Vec` of views; no Python code
runs in between, so the list is the same). The tail list itself is still built by
appends, so its capacity matches the list comprehension's. Windows-flavour paths
(`PureWindowsPath`, `WindowsPath`) stay CPython's (unchanged; w2-dx §6 step 8).

Callgrind (3.15.1, wave 1 = bc2ce1a, wave 2 = ws2/text-paths on wave2 c9fb335; the
uniform +13 Ir on untouched property kernels comes with the merged framework):

| cell | CPython Ir | wave 1 Ir | wave 2 Ir | ratio (wave 2) |
|---|---:|---:|---:|---:|
| `Path(p)` | 4,528 | 1,813 | 1,839 | 0.406 |
| `str(Path(p))` | 26,231 | 6,421 | 6,172 | 0.235 |
| `Path(d) / f` | 10,647 | 3,241 | 3,280 | 0.308 |
| `str(pp / f)` | 32,517 | 9,103 | 7,556 | 0.232 |
| `pp.name` | 715 | 391 | 404 | 0.565 |
| `pp.suffix` | 2,080 | 694 | 726 | 0.349 |
| `pp.stem` | 2,208 | 675 | 688 | 0.311 |
| `pp.parent` | 9,215 | 2,684 | 2,572 | 0.279 |
| `pp.parts` | 2,488 | 699 | 712 | 0.286 |
| `pp.with_suffix('.txt')` | 13,443 | 9,432 | 9,491 | 0.706 |
| `pp.relative_to(root)` | 108,479 | 4,899 | 4,624 | 0.043 |
| `hash(Path(p))` | 38,282 | 7,167 | 6,918 | 0.181 |
| `Path(p) == pp` | 33,068 | 6,557 | 6,308 | 0.191 |
| `pp.as_posix()` | 1,221 | 1,015 | 1,028 | 0.842 |
| `pp.is_absolute()` | 912 | 912 | 912 | 1.000 |
| `PurePosixPath(d, f)` | 5,475 | 1,877 | 1,903 | 0.348 |
| `MyP(p) subclass` | 4,376 | 4,390 | 4,390 | 1.003 |
| `str(pp)` | 609 | 403 | 416 | 0.683 |
| `os.fspath(pp)` | 895 | 689 | 702 | 0.784 |
| `pp == pq` | 1,595 | 497 | 510 | 0.320 |
| `hash(pp)` | 830 | 616 | 629 | 0.758 |
| `PurePosixPath(p).name (fresh)` | 18,083 | 5,230 | 5,122 | 0.283 |
| `pw.name PureWindowsPath` | 715 | 715 | 715 | 1.000 |
| `pw.parent PureWindowsPath` | 9,156 | 9,165 | 9,151 | 0.999 |
| `ms.name subclass` | 715 | 715 | 715 | 1.000 |
| `ms.suffix subclass` | 2,080 | 2,094 | 2,099 | 1.009 |
| `ms.parent subclass` | 9,343 | 9,371 | 9,376 | 1.004 |
| `str(ms / f) subclass` | 30,991 | 31,043 | 31,062 | 1.002 |
| `pp.name cProfile on` | 2,817 | 2,817 | 2,817 | 1.000 |
| `pp.relative_to(root) cProfile on` | 301,325 | 301,409 | 301,409 | 1.000 |

8-thread rule (`benches/targets/paths_scaling.py`, 3.15.1t, enabled ÷ plain CPU per call
at 8 threads, `local` / `shared`): `p / 'x' / 'y'` 0.256 / 0.208, `relative_to` 0.027 /
0.048. The shared-receiver cells keep wave 1's extra reference-count traffic on the shared
path (CPU ×1.47–2.10 from 1 to 8 threads, w2-threads §3.1), still far below CPython's
cost per call.

Not taken (measured, `data/fs_inventory` §`Path.rglob`, 190M Ir enabled): the pathlib
kernels are 10M of it; the rest is `glob._StringGlobber`'s generators and closures,
`os.scandir`, regex matching of names and allocation (fnmatch.md §"glob and os.scandir").
pathlib subclasses (w2-floors gap #13: sphinx ~2,500, pytest ~600 calls/run, ≤ 0.08%) stay
fallbacks.

## Measurements (wave 1)

Callgrind as in docs/targets/posixpath.md (`benches/targets/paths_cg.py`, CPython 3.15.1).
`pp` = `Path(p)` with `str()` already computed; `pq` an equal path; `pw` a `PureWindowsPath`;
`ms` an instance of `class MyP(type(pp)): pass` (subclass: falls back). `with_suffix`,
`as_posix` are not targets themselves (they gain through `__str__`, `parent`, ...);
`is_absolute` is unchanged.

| cell | CPython Ir | Rustique Ir | ratio | first 60 calls |
|---|---:|---:|---:|---:|
| `Path(p)` | 4528 | 1803 | 0.398 | 0.433 |
| `str(Path(p))` | 26211 | 6406 | 0.244 | 0.254 |
| `Path(d) / f` | 10647 | 3217 | 0.302 | 0.322 |
| `str(pp / f)` | 32525 | 9084 | 0.279 |  |
| `pp.name` | 715 | 386 | 0.539 |  |
| `pp.suffix` | 2080 | 676 | 0.325 |  |
| `pp.stem` | 2208 | 650 | 0.294 |  |
| `pp.parent` | 9215 | 2674 | 0.290 |  |
| `pp.parts` | 2488 | 674 | 0.271 |  |
| `pp.with_suffix('.txt')` | 13443 | 9397 | 0.699 |  |
| `pp.relative_to(root)` | 108415 | 4901 | 0.045 |  |
| `hash(Path(p))` | 38261 | 7135 | 0.186 | 0.194 |
| `Path(p) == pp` | 33038 | 6530 | 0.198 | 0.203 |
| `pp.as_posix()` | 1221 | 1010 | 0.827 |  |
| `pp.is_absolute()` | 912 | 912 | 1.000 |  |
| `PurePosixPath(d, f)` | 5475 | 1869 | 0.341 | 0.374 |
| `MyP(p) subclass` | 4371 | 4385 | 1.003 | 1.035 |
| `str(pp)` | 609 | 398 | 0.653 |  |
| `os.fspath(pp)` | 895 | 684 | 0.764 |  |
| `pp == pq` | 1595 | 480 | 0.301 |  |
| `hash(pp)` | 830 | 597 | 0.719 |  |
| `PurePosixPath(p).name (fresh)` | 18073 | 5215 | 0.289 | 0.298 |
| `pw.name PureWindowsPath` | 715 | 715 | 1.000 | 1.282 |
| `pw.parent PureWindowsPath` | 9156 | 9184 | 1.003 | 1.049 |
| `ms.name subclass` | 715 | 715 | 1.000 | 1.277 |
| `ms.suffix subclass` | 2080 | 2080 | 1.000 | 1.266 |
| `ms.parent subclass` | 9338 | 9366 | 1.003 | 1.049 |
| `str(ms / f) subclass` | 30966 | 31064 | 1.003 | 1.010 |
| `pp.name cProfile on` | 2817 | 2817 | 1.000 | 1.099 |
| `pp.relative_to(root) cProfile on` | 301414 | 301483 | 1.000 | 1.005 |


Under `PYTHON_JIT=1` (3.15.1, warmed 30,000 iterations; steady state):

| cell | CPython Ir | Rustique Ir | ratio |
|---|---:|---:|---:|
| `Path(p)` | 4030 | 1826 | 0.453 |
| `str(Path(p))` | 23988 | 6385 | 0.266 |
| `Path(d) / f` | 9578 | 3234 | 0.338 |
| `str(pp / f)` | 29203 | 9005 | 0.308 |
| `pp.name` | 568 | 376 | 0.663 |
| `pp.parent` | 8041 | 2657 | 0.330 |
| `pp.relative_to(root)` | 100332 | 4878 | 0.049 |
| `hash(Path(p))` | 36060 | 7132 | 0.198 |
| `Path(p) == pp` | 30889 | 6547 | 0.212 |
| `PurePosixPath(d, f)` | 4679 | 1881 | 0.402 |
| `str(pp)` | 611 | 335 | 0.548 |
| `pp == pq` | 1357 | 459 | 0.338 |
| `hash(pp)` | 833 | 535 | 0.642 |
| `PurePosixPath(p).name (fresh)` | 16532 | 5265 | 0.319 |
| `pw.name PureWindowsPath` | 568 | 568 | 1.000 |
| `ms.name subclass` | 568 | 568 | 1.000 |
| `str(ms / f) subclass` | 27692 | 27747 | 1.002 |
| `pp.name cProfile on` | 2824 | 2824 | 1.000 |
| `pp.relative_to(root) cProfile on` | 301423 | 301601 | 1.001 |

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
| str(base / n) over 1k | 2442.3 µs | 461.8 µs | 0.189 |
| relative_to + parts + stem over 1k | 8345.2 µs | 296.0 µs | 0.035 |
| Path(f).parent.name over 1k | 2228.0 µs | 562.7 µs | 0.253 |

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

`pathlib` is not imported by `enable()`: its thirteen targets install when it is imported.
`benches/targets/_prof_install.py` (callgrind, 3.15.1, registry warmed with an imported
vectorcall target of another group):

| region | Ir |
|---|---:|
| `import pathlib`, plain | 8.06M |
| `import pathlib` after `enable(only=["pathlib"])` | 8.50M (**+0.44M, +5.5%**) |
| `enable(only=["pathlib"])` with pathlib imported | 0.46M |

History: +24M at first (per-target MRO pins), +4.2M (fewer pins), +2.4M (accel-core's v2
fingerprints and shared pins), +1.04M (class attributes guarded by type version tag, shared
validation once per pass), +0.45M (one `GroupSpec`: ~130 per-target pins became 31). What
remains: 29 fingerprints (≈ 0.12M), the validation (`record_class`, slot offsets, ≈ 0.1M),
31 pins, thirteen vectorcall sites. Against the lazy-install budget (5% of the import,
0.40M) it is 0.04M over: an accepted wave-1 overage (docs/DECISIONS.md §5). At exit the `release` hook drops the classes, their recorded attributes and
the verified functions (the type cells go null first: a stale kernel declines).
`python -c pass` never imports pathlib (no startup cost; see posixpath.md
"Startup" for the whole-process cells).

## Version coverage

3.14 and 3.15, except the five property targets: 3.15 only. On 3.14 `LOAD_ATTR_PROPERTY`
caches the getter function object and pushes its frame inline without checking
`func_version` (3.15's `_LOAD_ATTR_PROPERTY_FRAME` checks it, and the specializer refuses a
function whose version `PyFunction_SetVectorcall` cleared), so every specialized `p.name`
site would run the original and never reach the kernel; they refuse with reason "version"
there. The PurePath core is otherwise the same in both versions except `__fspath__` (3.15: alias of
`__str__`, accelerated with it; 3.14: its own function calling `str(self)`, which reaches
the `__str__` kernel) and `relative_to`'s loop (same result without `walk_up`).
`jit_safe: true` (see Measurements: under `PYTHON_JIT=1` the steady-state cells, fallbacks included, stay within 1.05; a fallback before suspension costs +250–350 Ir, inferred from docs/atlas/mechanisms.md §1.3).
