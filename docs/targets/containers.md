STATUS: wave 1, accel-core (2026-11-01). Retrofit of the shipped safe-tier kernels
(docs/TAKEOVER.md §3.4, §7) onto the target registry (docs/targets/framework.md).
Wave 2, workstream `core`: the sort never-slower fix, `builtins.sorted` as a kernel,
identity and tuple needles, lock-free free-threaded list scans, the native-tier paths
removed from `builtins.rs` (§ "Wave 2" at the end; it supersedes the sort and FT notes above).

# Groups `containers` and `builtins`

## Targets

| target | site | original | kernel (`crates/rustique-py/src/`) |
|---|---|---|---|
| `list.sort` | `ml_meth` (`METH_FASTCALL\|METH_KEYWORDS`) | `list_sort` | `takeover.rs` `k_list_sort` |
| `list.index`, `tuple.index` | `ml_meth` (`METH_FASTCALL`) | `list_index`, `tuple_index` | `k_list_index`, `k_tuple_index` |
| `list.count`, `tuple.count` | `ml_meth` (`METH_O`) | `list_count`, `tuple_count` | `k_list_count`, `k_tuple_count` |
| `list.__contains__`, `tuple.__contains__` | static slot `sq_contains` | `list_contains`, `tuple_contains` | `k_list_contains`, `k_tuple_contains` |
| `builtins.sum`, `min`, `max`, `sorted` | `ml_meth` of the `builtins` method table (`METH_FASTCALL\|METH_KEYWORDS`) | `builtin_sum` … | `builtins.rs` `rq_sum` … |
| `builtins.any`, `all` | `ml_meth` (`METH_O`) | `builtin_any`, `builtin_all` | `rq_any`, `rq_all` |

All 13 targets: `module: None` (installed at `enable()`), validated on 3.14 and 3.15,
free-threaded included, `jit_safe`, not `checkable` (they compare with `__eq__`-free fast
paths, but the generic continuation runs user code). Fingerprints (`ml_name`, `ml_flags`,
`ml_doc`) are recorded per minor and identical on 3.14.9 and 3.15.1; the `in` slots are
checked against their wrapper descriptors (`d_wrapped`).

## Exact subset and fallbacks

Unchanged from the shipped kernels: docs/TAKEOVER.md §7 (`containers`) and the module docs
of `builtins.rs` (`builtins`). In short: exact `int`/`bool`/`float`/`str`/`None` elements
(and `tuple`/`list`/`bytes` for truthiness), no key function, builtin `list`/`tuple`
receivers (and `set`/`frozenset`/`dict` keys and Rustique containers for the builtins);
anything else calls the original with the unchanged arguments. A scan that meets a value
needing CPython's comparison restarts in the original (unobservable: no user code ran).
Changes in this round:

- `count` and `in` decide on the needle first: a needle that is not a plain scalar
  delegates at once (before, `count` of an object scanned first, then delegated: 1.12–1.16
  on small lists).
- `sort`'s keyword names are compared by identity with the interned `key`/`reverse`
  first (the fallback `sort(key=f)` paid ~100 Ir for two string comparisons).
- Counters: `hits`, `fallback["args"]` (direct fallback), `fallback["deopt"]` (a scan or a
  builtin restarted in / continued with the original).
- JIT: the registry clears the 3.15 executors after install and restore, so traces built
  before `enable()` reach the kernels and traces built during it stop reaching them after
  `disable()` (the shipped `rustique.enable()` had the stale-trace defect of
  docs/atlas/mechanisms.md §4; regression test `tests/test_accel_runtime.py`).

Private state skipped: none (the kernels keep no state beyond the captured `index` error
messages, C strings shared by all interpreters).

## Measured

callgrind, Ir per call, region-instrumented, fresh process per cell
(`benches/targets/containers.py`, 3.15.1, dev-release build), **measured**:

| cell | kind | plain | enabled | ratio |
|---|---|---:|---:|---:|
| `3 in ints` (8 ints) | fast | 275.5 | 219.4 | 0.796 |
| `4 in ints` (miss) | fast | 1142.5 | 408.4 | 0.357 |
| `o in objs` (identity hit) | fast | 145.5 | 116.4 | 0.800 |
| `1 in mixed` | fast | 149.5 | 125.4 | 0.839 |
| `o in mixed` (needle not a scalar) | fallback | 473.5 | 492.4 | 1.040 |
| `ints.index(7)` | fast | 778.2 | 428.1 | 0.550 |
| `ints.index(7, 1)` (extra argument) | fallback | 700.2 | 713.1 | 1.018 |
| `objs.index(o)` (identity) | fast | 263.2 | 243.1 | 0.924 |
| `ints.count(3)` | fast | 978.2 | 458.1 | 0.468 |
| `objs.count(o)` | fallback | 536.2 | 555.1 | 1.035 |
| `t.index(1)` | fast | 679.2 | 402.1 | 0.592 |
| `tobjs.count(o)` (2-tuple of objects) | fallback | 389.2 | 408.1 | 1.049 |
| `9 in t` | fast | 380.5 | 242.4 | 0.637 |
| `ints[:].sort()` | fast | 1931.2 | 1267.1 | 0.656 |
| `strs[:].sort(key=len)` | fallback | 1852.4 | 1883.3 | 1.017 |
| `sum(small)` / `sum(fl)` | fast | 659.3 / 998.3 | 432.2 / 790.2 | 0.656 / 0.792 |
| `min(3, 4)` / `max(ints)` | fast | 442.3 / 1791.3 | 323.2 / 529.2 | 0.731 / 0.295 |
| `max(ints, key=abs)` | fallback | 3058.3 | 3131.2 | 1.024 |
| `sorted(small)` (builtin list: original, which calls the `list.sort` kernel only with `containers`) | fallback | 1331.3 | 1344.2 | 1.010 |
| `any(small)` / `all(strs)` | fast | 443.3 / 708.3 | 324.2 / 373.2 | 0.731 / 0.527 |
| `any(d)` (dict: direct fallback) | fallback | 598.3 | 617.2 | 1.032 |
| `sum(range(3))` (iterator: direct fallback) | fallback | 1146.3 | 1183.2 | 1.032 |

Under the JIT (`PYTHON_JIT=1`, warmed 30 000 iterations; release build): fast paths
0.29–0.81; fallbacks `o in mixed` 1.044, `ints.index(7, 1)` 1.021, `objs.count(o)` 1.042,
`sort(key=len)` 1.020, `max(ints, key=abs)` 1.026, `sorted(small)` 1.010, `any(d)` 1.028,
`sum(range(3))` 1.035, and `tobjs.count(o)` 1.049 (2-element tuple of objects: the JIT
makes CPython's call 305 Ir; it was 1.062 until the count kernels learned to reject
heap-type needles with one flag test, f189e6c). The release build's plain-interpreter table matches the one above
within ±1 Ir on fallbacks.

**Phase 3 follow-up (wave1 after `f5090f9`).** The integrated scoreboard (Phase 3,
`docs/atlas/phase3.md` §7.1) found three cells above 1.05 that the cells above do not
cover: `x.count(hit)` on a 1-element list or tuple of tuples (1.065–1.070) and `miss in x`
on a 1-element list of tuples (1.050–1.055), where CPython's whole call is ~300–400 Ir and
the delegation cost +19–21 Ir. Two changes: non-scalar needles are rejected with one test
of the type flags (heap types and the tuple/list/dict/bytes subclass flags, which
`needs_generic` reads once), and the `count` kernels moved their scan out of line so that
the delegation path needs no stack frame (it had pushed three registers). Callgrind, release
build, dev box (scoreboard cells, enabled / plain):

| cell | 3.15.1 before → after | 3.15.1t after |
|---|---|---|
| `list/x.count(hit)/object/1` | 1.070 → **1.030** | 1.026 |
| `tuple/x.count(hit)/object/1` | 1.070 → **1.030** | 1.027 |
| `list/miss in x/object/1` | 1.050 → **1.040** | 1.045 |
| `tuple/miss in x/object/1` | 1.049 → **1.038** | 1.038 |

and in the cells above (release build): `objs.count(o)` 1.017, `tobjs.count(o)` 1.023 (JIT
1.029), `o in mixed` 1.032 (JIT 1.035). Scalar `count` hits pay up to +21 Ir for the extra
call level (e.g. `x.count(hit)/int/8` 0.449 → 0.467).

Large inputs (wall time): docs/TAKEOVER.md §3.4 (unchanged kernels).

## Conformance

`tests/conformance/run_target_suites.py --targets containers,builtins --modules
test.test_list,test.test_tuple,test.test_sort,test.test_builtin` (`benches/_accel_final.sh`),
**measured**, same outcome for every test id with and without `enable()`, all 13 targets
installed:

| build | test_list | test_tuple | test_sort | test_builtin |
|---|---|---|---|---|
| 3.15.1 | 68 pass, 1 skip | 38 pass | 21 pass | 150 pass, 7 skip |
| 3.15.1t | 68 pass, 1 skip | 38 pass | 21 pass | 150 pass, 7 skip |
| 3.14.9 | 67 pass, 1 skip | 38 pass | 21 pass | 141 pass, 6 skip |
| 3.14.9t | 67 pass, 1 skip | 38 pass | 21 pass | 141 pass, 6 skip |

Differential and mechanism tests: `tests/test_takeover_safe.py`, `tests/test_builtins.py`
(Hypothesis, kernels vs originals), `tests/targets/test_accel_mechanics.py` (identity,
references, disable, free-threaded stress), `tests/test_accel_runtime.py` (JIT traces,
subinterpreters, enable/disable under concurrent calls).

## Wave 2 (workstream `core`)

### `list.sort` / `sorted`: never slower (the w2-dx / w2-floors §6 violation, fixed)

The shipped kernel sorted `(key, item)` pairs with Rust's stable sort: insertion sort up to
20 elements (quadratic on descending input) and 48-byte `str` keys (~70 Ir per element).
It was up to 1.84× instructions / 3.2× wall on ordered and short-string input. Now
(`crates/rustique-py/src/takeover.rs`, `try_sort` and below):

- **Keys** (16-byte pairs): exact ints in `i64`, floats as `f64` (no NaN), strings as the
  8 bytes (1-byte strings; 3 code points otherwise) **after the prefix all strings share**:
  the prefix is guessed from the first, middle and last strings, verified on every string,
  recomputed exactly when wrong or when every key comes out equal; equal keys are resolved
  by comparing the strings. Keys are built in one pass that also checks the types.
- **Order of the keys**, tracked while they are built: strictly increasing or decreasing →
  the list is already sorted or is reversed (one pass, no sort); few steps against the
  requested order (≤ n/16) → a natural merge sort (timsort's run detection, merge policy
  and run trimming, no galloping); ≤ 20 elements → timsort's small-list strategy (the
  first natural run, a strictly descending one reversed, then insertion); otherwise Rust's
  driftsort. Stability and `reverse=True` (equal elements keep their order) as CPython.
- Lists of fewer than 2 items return at once (after the arguments are checked: CPython's
  sort changes nothing there). Short `str` lists are sorted natively too: an earlier
  hand-off of `str` lists shorter than 32 to CPython cost 1.13 on 1 string and 1.02 on 8 in
  Phase 3's scoreboard; with the one-pass keys every in-place `list.sort()` cell of 1–32
  str/int/float items in three orders is 0.53–1.003 (`sort_cells.py small`, the copy made
  outside the measured statement; 2 strings: 1.000–1.003).
- **`builtins.sorted` is a kernel** (it was a pass-through to the original, which only
  existed for the retired native types): for an exact `list`/`tuple` whose first item is an
  exact `int`/`bool`/`float`/`str`, with `key=None` and an exact `bool`/`int` `reverse`, it
  makes CPython's copy (`PySequence_List`: same list, same capacity) and sorts it natively,
  without the attribute lookup and bound-method call of `newlist.sort()` (~250 Ir). When
  the native sort declines (mixed types, big ints, NaN), `list.sort` runs on that same copy
  with the same keywords (CPython's own when the `list.sort` kernel is installed, which
  would only repeat the declined work). Hits/deopt counted on `builtins.sorted`.
- Exactness: results compared by identity (stability) against CPython in
  `tests/targets/test_sort.py` (sizes 0–300 around every threshold, 11 value families incl.
  shared prefixes longer than a key, ties beyond 8 bytes, 1/2/4-byte and mixed kinds,
  -0.0/inf, big ints; six orders; both directions; declines, errors and argument forms;
  Hypothesis) and in the wave-1 suites (`tests/test_takeover_safe.py`, `tests/test_builtins.py`).

callgrind, Ir per call ratio enabled / plain, 3.15.1, `builtins` + `containers` enabled,
**measured** (`benches/targets/sort_cells.py sort`, w2-floors' 90-cell grid through
`sorted`; shipped kernel in w2-floors §6.3, worst 1.841): **every cell 0.37–0.94**.

| kind, n | asc | desc | rand | halves | nearly | dups |
|---|---:|---:|---:|---:|---:|---:|
| int 2 | 0.625 | 0.636 | 0.636 | 0.636 | 0.636 | 0.625 |
| int 5 | 0.620 | 0.595 | 0.643 | 0.677 | 0.657 | 0.671 |
| int 20 | 0.607 | 0.531 | 0.736 | 0.845 | 0.560 | 0.683 |
| int 100 | 0.602 | 0.486 | 0.477 | 0.735 | 0.491 | 0.389 |
| int 1000 | 0.597 | 0.578 | 0.418 | 0.781 | 0.633 | 0.369 |
| float 2 | 0.620 | 0.632 | 0.632 | 0.632 | 0.632 | 0.620 |
| float 5 | 0.625 | 0.599 | 0.687 | 0.701 | 0.625 | 0.691 |
| float 20 | 0.634 | 0.551 | 0.908 | 0.942 | 0.566 | 0.760 |
| float 100 | 0.649 | 0.517 | 0.569 | 0.779 | 0.553 | 0.458 |
| float 1000 | 0.688 | 0.651 | 0.534 | 0.867 | 0.773 | 0.468 |
| str 2 | 0.705 | 0.714 | 0.705 | 0.714 | 0.705 | 0.705 |
| str 5 | 0.697 | 0.670 | 0.640 | 0.694 | 0.697 | 0.797 |
| str 20 | 0.738 | 0.667 | 0.683 | 0.774 | 0.697 | 0.844 |
| str 100 | 0.690 | 0.599 | 0.453 | 0.745 | 0.376 | 0.778 |
| str 1000 | 0.688 | 0.673 | 0.372 | 0.782 | 0.523 | 0.721 |

The other sort-family cells (`sort_cells.py family`, 90 cells: `reverse=True`, in-place
`x[:].sort()` at n = 2/20/1000, 2-byte strings, long strings with a 9-byte shared prefix,
tuples, sets, generators, `key=`, declines after the copy): see the workstream report for
**max 1.049** (`x[:].sort()` of 2 strings: the check before CPython's sort). Above 1.0
only: those hand-offs (1.046–1.049), long strings sharing a 9-byte prefix, presorted
(1.012; 0.97–0.99 at n = 20/100), `key=` 1.018, tuples 1.004–1.007; raw outputs in
`benches/targets/results/core/` (`family-v11.txt`, `family-str-v12.txt` after the last
str change). `min`/`max`
(`sort_cells.py minmax`, 133 cells: str/2-byte/long str at n = 1–1000 in three orders,
int/float, two- and three-argument forms, `key=`, `default=`): **max 1.012** (`max(key=len)`,
a fallback), every fast cell ≤ 0.90.

### Identity and tuple needles in `in` / `index` / `count` (w2-floors §2 gaps #4, #5)

- **Identity needles**: a needle whose type's `tp_richcompare` is `object`'s (classes with
  metatype `type`, plain instances, functions, modules). Against such a needle, an item that
  is identity-`==` too, or an exact `int`/`bool`/`float`/`str`/`None`, compares unequal
  without running code (both slots return `NotImplemented`, then identity), so the scan is
  `item is needle`.
- **Tuple needles** (exact `tuple`): `tuplerichcompare` element by element (no length
  early-out, as CPython) while each element pair compares without code (exact str bytes,
  exact numbers, nested exact tuples up to depth 8, mixed kinds of those: unequal).
- At the first item that could run code (an item with `__eq__`, a metaclass `__eq__`, a
  tuple subclass, an element with `__eq__`) the kernel **continues CPython's loop from that
  item** (`PyObject_RichCompareBool` with a reference held) instead of restarting: the items
  before it compared unequal without running anything, as in CPython. User `__eq__`s run in
  CPython's order with CPython's arguments (`tests/targets/test_needles.py` logs them).

callgrind (`benches/targets/core_cells.py contains`), 3.15.1, **measured**:

| cell | plain | enabled | ratio |
|---|---:|---:|---:|
| `type(node) in NODE_TYPES` (5 classes, hit at 5th) | 799 | 383 | 0.480 |
| same, list | 853 | 389 | 0.456 |
| `C in (A, B, C)` / `D in (A, B, C)` (miss) | 419 / 561 | 241 / 261 | 0.576 / 0.466 |
| `type(o) in CLS` (hit at 1st) | 215 | 203 | 0.944 |
| `o in [A(), A(), o]` / miss | 455 / 590 | 239 / 259 | 0.526 / 0.439 |
| `D in (A, 1, 'x', None, C)` | 915 | 504 | 0.551 |
| `(4, 'd') in pairs` (list of 5 pairs) / miss | 903 / 1,383 | 507 / 713 | 0.562 / 0.516 |
| `(4, 'd') in tpairs` (tuple) | 858 | 503 | 0.586 |
| `pairs.count(p)` / `pairs.index(p)` | 1,226 / 1,028 | 725 / 644 | 0.591 / 0.626 |
| `objs.count(o)` | 536 | 364 | 0.679 |
| fallback: needle with `__eq__` | 1,119 | 1,141 | 1.020 |
| continuation after 1 item: metaclass `__eq__` / item `__eq__` | 789 / 789 | 784 / 778 | 0.994 / 0.986 |
| `3 in (1, 2, 3)` (scalar path, unchanged) | 383 | 246 | 0.643 |

The wave-1 cells (`benches/targets/containers.py`) stay ≤ 1.04 (max: `any(d)`, a direct
fallback, 1.040; `objs.index(o)` is now a hit, 0.928).

### Free-threaded lists: CPython's lock-free protocol

`x in list`, `list.index`, `list.count` on free-threaded builds no longer take the list's
critical section for lists of 4+ items: every item is read with `PyList_GetItemRef` (a
strong reference, CPython's own protocol). Threads scanning one shared list no longer
serialize on its mutex (it made `x in shared_list` 1.84× and `list.index` 2.10× slower than
plain at 8 threads, `w2-threads.md` §3.1); immortal items (small ints, interned strings)
cost no reference-count traffic, as in CPython.

CPU time per call at 8 threads, enabled ÷ plain (DECISIONS §10.9 rule: ≤ 1.05), 3.15.1t,
dev box, `benches/ft/kernel_scaling.py -t 1,8 -r 3`, **measured** (wave 1 in parentheses,
`w2-threads.md` §3.1):

| cell | local | shared |
|---|---:|---:|
| `99 in list(range(100))` | 0.869 | **0.944** (1.836) |
| `x.index(99)` | 0.813 | **0.873** (2.102) |
| `'TRACE' in tuple(8)` | 0.430 | 0.428 |
| `ast.Call in (5 node classes)` | 1.038 | 1.041 |
| `(19, '19') in list(20 pairs)` | 0.726 | 0.559 |
| `sorted(50 ints)` | 0.699 | 0.959 |
| `y = x[:]; y.sort()` (50 str) | 0.585 | 0.590 |
| `max(list 100 floats)` | 0.098 | 0.048 |

(The class-needle cell costs the same CPU time as CPython's loop on 3.15.1t although it runs
0.42× the instructions (callgrind, same build): ~58 ns per call either way, dominated by
the loop around it; it is within the rule.)

### Native tier removed from `builtins.rs`

`sum`/`min`/`max`/`any`/`all` no longer recognise the retired `rustique.List`/`Set`/... types
(DECISIONS §10.1); builtin containers keep their wave-1 behaviour.

### Conformance (wave 2, `benches/targets/core_conformance.sh`, measured)

Same outcome for every test id with and without the groups, all targets installed, on
3.15.1, 3.15.1t, 3.14.9 and 3.14.9t: `test_list` (68 / 67 on 3.14 pass, 1 skip),
`test_tuple` 38, `test_sort` 21, `test_builtin` (150 + 7 skip / 141 + 6 skip),
`test_contains` 4; 0 differences (`benches/targets/results/core/conformance.log`, which
also has the json/float, copy/dataclasses/uuid and set runs, all 0 differences).

### Workloads (workstream `core`, all groups enabled, measured)

Instructions per steady-state `run()`, enabled ÷ plain, dev box, `perf stat` over
`run.py one -n 1 / -n 4` (`benches/targets/core_workloads.sh`); digests identical in every
pair. Run-to-run noise on the loaded box: ±1–3% (isort measured 0.737 and 0.758). Wave-1
code (w2-floors' Phase-1 table, same method) in the last column.

| workload | 3.15.1 | 3.15.1t | wave 1 (3.15.1) |
|---|---:|---:|---:|
| `web/json_api` | **0.772** | 0.795 | 0.858 |
| `web/logging_service` | **0.608** | 0.673 | 0.735 |
| `data/csv_etl` | 0.591 | 0.609 | 0.591 |
| `data/state_snapshot` | 0.568 | 0.583 | 0.566 |
| `devtools/isort_stdlib` | 0.737–0.758 | 0.727 (wall 0.26: T7) | 0.734 |
| `devtools/sqlglot_transpile` | 0.952 | 0.954 | 0.956 |
| `data/xml_feeds` | 1.001 | | 0.999 |
| custom family (10), geometric mean | 0.989 (0.919–1.005) | | |
| threads family (5): compute_pool / http_server / pipeline_queue / request_pool / shared_cache | 0.956 / 0.897 / 0.551 / 0.585 / 0.610 | 0.967 / 0.906 / 0.573 / 0.615 / 0.674 | |

Custom family per workload (3.15.1): di_plugins 0.996, ecs_arena 0.993, event_ledger 1.000,
fulfilment_sim 0.997, mini_interp 1.000, order_domain 1.004, orm_mapper 0.980,
report_engine 0.919, rules_engine 1.004, task_planner 1.005 (all within the box's noise of
1.00 where above it; none of these reaches a changed kernel on a hot path).

### Phase 3 follow-up (scoreboard, wave2 d40e7b9 → ws2/core)

Phase 3's scoreboard (callgrind, sizes 0–100) found the sort hand-offs above 1.05 on tiny
inputs. `benches/scoreboard.py instr` on the same cells, 3.15.1, before → after:

| cell | before | after |
|---|---:|---:|
| `list/x.sort()/str/1` | 1.129 | **0.766** |
| `list/x.sort()/str/8` | 1.017 | **0.584** |
| `sorted(x)/str/1` on dict, set, frozenset | 1.084 | **0.959–0.971** |
| `sorted(x)/str/8` on dict, set, frozenset | 1.024–1.032 | **0.734–0.775** |
| `dict/any(x)/str/0` (empty dict: now answered natively) | 1.086 | **0.475** |
| `dict/any(x)/*/1..100` (dict: direct fallback, +28 Ir) | 1.037 | 1.043 |

The remaining cells of that report above 1.02 (`dict/x.copy()`, `dict(x)`, `list(x)`,
`for v in x` over empty dicts/frozensets, `x.values()` iteration) reach no Rustique site.
They move by a constant 14–56 Ir with the allocator's state, not with any kernel: in plain
CPython a few junk allocations before the cell shift `any({})` by +14 Ir and
`dict(dict100)` by −97; `import rustique` without `enable()` shifts empty-frozenset
iteration by +14; enabling only `containers` shifts `{}.copy()` by +14; enabling everything
but the core groups shifts `dict(8 ints)` by −5 and everything by +56 (callgrind,
deterministic with `PYTHONHASHSEED=0`, `benches/targets/_heapstate.py`). These are pymalloc
pool boundaries crossed or not in the measured loop (an alloc/free pair that empties and
refills a pool each iteration), a harness effect.

### Phase 3 follow-up 2: large structured inputs (wall time)

Phase 3's quiet-box scoreboard (wave2 d40e7b9/7fec7e4) measured `sorted()` of a 10^6-element
set or frozenset of ints or floats at 1.234–1.280× wall. Their iteration order is one or two
long ascending runs (a hash is the value, and a large table keeps it in order). The keyed
sort executed fewer instructions there (0.857 / 0.923), but its 16 MB pairs buffer and the
write-back were memory traffic that timsort's near-linear pass does not have. The same
showed for presorted and concatenated sorted lists of 10^5–10^6 strings (1.6–2.1× wall),
and for nearly sorted lists of 10^5–10^6 items (1.4–2.0×; there a natural merge over pairs
moved every element at every level, where timsort gallops).

Now, for 4,096+ ints/floats/strs (`sort_runs_inplace`): a probe of 8 adjacent pairs; when
they are all in order, one pass checks the types and records the breaks. With at most
`16 + n/65536` breaks, the runs are merged in place on the item pointers (timsort's policy
and run trimming; a run that precedes the other moves as a block). With more, the list is
nearly sorted: CPython's own sort runs (nothing changed, the pass cost ~1%). A list strictly
against the order throughout is reversed in place. Anything else keeps the keyed sort.

Measured, 3.15.1, `benches/targets/_wall_big_sort.py` (wall, best of 9, dev box) and
`sort_big_cells.py` (callgrind, one call), enabled ÷ plain:

| input | wall before | wall after | instructions after |
|---|---:|---:|---:|
| `sorted(set of 10^6 ints)` / frozenset | 1.43 (Phase 3: 1.23–1.28) | **0.80 / 0.80** | 0.616 / 0.624 |
| `sorted(set of 10^6 floats)` / frozenset | (Phase 3: 1.23–1.28) | **0.72 / 0.72** | 0.653 / 0.661 |
| `sorted(list)`, 10^6 str presorted / reversed / two halves | 2.02 / 2.15 / 2.07 | **0.82 / 0.80 / 0.78** | 0.876 / 0.845 / 0.811 (10^5) |
| `sorted(list)`, 10^5–10^6 int/float, n/50 swaps | 1.36–2.02 | **0.99–1.02** | 1.000–1.008 |
| `sorted(list)`, 10^5–10^6 str, n/50 swaps | 0.98–1.02 | **1.00–1.02** | 1.001–1.025 |
| `sorted(list)`, 10^6 int/float/str random | 0.31 / 0.33 / 0.40 | 0.31 / 0.34 / 0.40 | 0.39 / 0.48 / 0.37 (10^5) |

The 90-cell grid (max 0.942), the 90 family cells (max 1.018), `min`/`max` (133 cells, max
1.012) and the in-place small cells (117 cells, max 1.013) are unchanged (n < 4096 never takes
the new path); raw outputs in `benches/targets/results/core/` (`*-v13.txt`, `big-*.txt`).
Tests: `tests/targets/test_sort.py::test_large_structured_by_identity` (4,096–20,000 items:
one/two/five/forty runs with ties, strictly and non-strictly descending, nearly sorted,
random; both directions; by identity) and `test_large_sets_and_declines_by_identity`.
