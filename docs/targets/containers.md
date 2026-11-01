STATUS: wave 1, accel-core (2026-11-01). Retrofit of the shipped safe-tier kernels
(docs/TAKEOVER.md §3.4, §7) onto the target registry (docs/targets/framework.md).

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

Under the JIT (`PYTHON_JIT=1`, warmed), the same cells measured so far: fast paths
0.33–0.74; fallbacks `o in mixed` 1.044, `ints.index(7, 1)` 1.021, `objs.count(o)` 1.042,
**`tobjs.count(o)` 1.062** (2-element tuple of objects: the JIT makes CPython's call 305 Ir,
so the kernel's needle-type check, counter and extra call level, 19 Ir, exceed 5%). Open:
see the accel-core report.

Large inputs (wall time): docs/TAKEOVER.md §3.4 (unchanged kernels).

## Conformance

`tests/conformance/run_target_suites.py --targets containers,builtins --modules
test.test_list,test.test_tuple,test.test_sort,test.test_builtin` — results in the
accel-core report.
