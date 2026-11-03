# Rustique benchmarks

Ratios are Rustique ÷ CPython (lower is faster; parity bar ≤ 1.05). Two things are
measured:

- **the default tier**: plain builtin `list`/`tuple`/`dict`/`set`/`frozenset` code in a
  process where `rustique.enable()` ran, against the same code in plain CPython;
- **Rustique's own types** (`List`, `Tuple`, `Dict`, `Set`, `FrozenSet`, used explicitly
  or through the opt-in native tier) running the same statements as the builtins, with
  `rustique.enable()` active.

Why the default tier keeps exact builtins: `docs/DECISIONS.md`, `docs/TAKEOVER.md`.

Since wave 1 (November 2026) a third thing is measured, and it is the headline: **real
programs** with every accelerated stdlib group on, below. Wave 2 widened the set from 34 to 65
programs (frameworks, third-party-heavy code, custom applications, services, threaded
programs). The sections after the two waves are the parity program's measurements (October
2026), re-run at the wave-1 commit where noted; Rustique's own types were retired in 0.3
(`docs/MIGRATION.md`), so their section is history.

## Wave 2: all kinds of programs (Phase 3, `7fec7e4`)

Full report: `docs/atlas/w2-phase3.md`; raw data and the rerunnable scripts:
`benches/results/w2-phase3/`; ground truth before wave 2: `docs/atlas/w2-phase0.md`. The same
dedicated AWS c7a.8xlarge, fat-LTO release builds; "enabled" = the default tier
(`rustique.enable()`, every default group, no opt-in tier). The code kept landing during the run;
which commit each stage measured is in the report's §0 (`7fec7e4` is the steady state;
`9d0abae` and `78751e2` changed only `re`'s first call and the path kernels, re-measured there).

**Method.** `benches/workloads/` now holds 65 programs in eight families (the three of wave 1
plus `frameworks`, `libs`, `custom`, `services`, `threads`; each family pins its requirements).
Same `run.py compare all -n 5 -p 5` as wave 1, one L3 domain per build; the threads family runs at
8 threads. Two replicates (`d40e7b9`, `7fec7e4`) agree within 0.006 per family.

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| **65 workloads, geometric mean** | **0.870** (1.15× faster) | **0.872** (1.15×) |
| wave 1's 34 (wave-1 code: 0.877 / 0.843) | 0.858 | 0.826 |
| web (10) / data (12) / devtools (12) | 0.882 / 0.752 / 0.957 | 0.903 / 0.735 / 0.861 |
| frameworks (6) / libs (6) / custom (10) | 0.940 / 0.961 / 0.936 | 0.955 / 0.966 / 0.973 |
| services (4) / threads, 8 threads (5) | 0.848 / 0.685 | 0.888 / 0.792 |
| best; worst | `report_render` 0.357; `mini_interp` 1.006 | `isort_stdlib` 0.201; `mini_interp` 1.018 |
| digest mismatches | 0 | 0 |
| 8 whole processes, `RUSTIQUE=1` through the `.pth` (`9d0abae`) | 0.743 | 0.742 |
| pyperformance 1.14.0, 122 benchmarks (wave-1 code: 0.942 / 0.955) | **0.911** | **0.936** |
| scoreboard 0–100, cells ≤ 1.05 (instructions, after isolation) | 3814 / 3814 | 3814 / 3814 |
| scoreboard 10k/1M, wall geomean; kernel paths | 0.826; 0.452 (wave 1: 0.828; 0.461) | not run |
| `python -c pass`, `RUSTIQUE=1` vs unset (Ir / wall, `9d0abae`) | 1.028 / 1.059 | (3.14.9: 1.029 / 1.075) |

**Opt-in tiers** (`d40e7b9`, all 65, against plain / against the default tier):
`RUSTIQUE_TIERS=interp` 0.859 / 0.987 (frameworks 0.933, custom 0.920; 3.15.1, verified
python-build-standalone builds only); `RUSTIQUE_TIERS=gc` 0.868 / 0.998 on 3.15.1 and 0.866 /
0.992 on 3.15.1t; both, on frameworks / custom / devtools, 0.936 / 0.922 / 0.942 against plain.
No tier slows a workload by more than 1.8% against the default tier.

**pyperformance**, new wins over wave 1 (3.15.1 / 3.15.1t): `regex_compile` 0.044 / 0.122 (was
1.02), `logging_format` 0.513 / 0.643, `logging_simple` 0.520 / 0.671; on 3.15.1 `regex_v8`
0.943, `nqueens` 0.933, `k_core` 0.955. Above 1.05: `python_startup` 1.074 / 1.110 (the
`enable()` in `sitecustomize`) and `coroutines` on 3.15.1t 1.073–1.080 (pure Python, no group
alone causes it, not reproducible outside pyperf's worker: a layout effect, *inferred*, open).

**Free-threading.** `isort_stdlib` on 3.15.1t is 0.201 (CPython's `set.__reduce__`
contends with itself; Rustique's kernel does not), 0.260 with the copy group off;
`state_snapshot` 0.668 with the copy group off. At 8 threads the threads family scales 1.9–7.5×
with Rustique on, and CPU time per call of every target at 8 threads is ≤ 1.05× CPython's
(`78751e2`).

**The final code, `07786e2`** (core's large-sort fix on top; `w2-phase3.md` §14): a third
replicate of the 65 gives **0.872 / 0.871** (0 digest mismatches); `sorted()` of a 1M-element
set or frozenset of `int`/`float`, 1.23–1.28 at `7fec7e4`, is 0.714–0.744, and all 48 sort cells
at 10k/1M are ≤ 1.007 (wall, alone on one CPU).

**Open never-slower cells** (measured; DECISIONS §13): `custom/mini_interp` on 3.15.1t (1.015 /
1.018 / 1.023 over the replicates); startup wall time above; `coroutines` on 3.15.1t. The other scoreboard cells above 1.05 alone (tuple iteration, set
equality at 10k, dict lookup at 1M) are not Rustique's: enabled ÷ imported ≤ 1.026 at the best
heap offset, and plain CPython moves by as much between processes.

## Wave 1: real workloads (Phase 3, `11bad29`)

Full report: `docs/atlas/phase3.md`; raw data and the rerunnable script:
`benches/results/phase3/` (`run_phase3.sh <stage>`); ground truth before wave 1:
`docs/atlas/phase0.md`. Same box as below, fat-LTO release wheels installed with `pip`.

**Method.** `benches/workloads/` holds 34 programs in three families (`data`, `devtools`,
`web`; each pins its third-party requirements in `requirements.txt`), each a `setup()` and a
`run()` returning a digest of its result. `python benches/workloads/run.py compare all -n 5
-p 5` runs plain and enabled in fresh processes, interleaved, pinned to one L3 domain per
build, and fails on any digest mismatch. Whole-process cells use `pyperf command` with
`RUSTIQUE=1` through the shipped `.pth` (startup and imports included). Counters of one
steady-state `run()` per workload (`RUSTIQUE_STATS`) show which targets hit and why others
fell back. Per-target cells are each group's callgrind bench (`benches/targets/*`, region
instrumented, plain vs `enable(only=[group])`).

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| 34 workloads, geometric mean (enabled / plain) | **0.880** | **0.843** |
| second replicate (every workload within 2% of the first) | 0.879 | 0.843 |
| data (12) / devtools (12) / web (10) | 0.773 / 0.963 / 0.922 | 0.756 / 0.863 / 0.933 |
| median; best; worst | 0.980; `report_render` 0.436; `config_merge` 1.013 | 0.986; `isort_stdlib` 0.196; `xml_feeds` 1.005 |
| digest mismatches; workloads > 1.05 | 0; 0 | 0; 0 |
| 6 whole processes, `RUSTIQUE=1` (geomean) | 0.756 | 0.712 |
| scoreboard 0–100, cells ≤ 1.05 (after isolation re-runs); geomean | 3811 / 3814; 0.859 | 3811 / 3814; 0.864 |
| `python -c pass`, `RUSTIQUE=1` vs unset (Ir / wall) | 1.045 / 1.078 | (3.14.9: 1.063 / 1.108) |

**pyperformance 1.14.0**, full default suite, plain vs `rustique.enable()` (all groups)
through `sitecustomize`, two replicate pairs per build (A: plain first, B: enabled first),
one L3 domain each:

| | 3.15.1 (A / B) | 3.15.1t (A / B) | Phase 0 (`5abd54c`) |
|---|---|---|---|
| geometric mean | **0.942 / 0.940** | **0.956 / 0.957** | 1.002 / 1.002 |
| median | 0.998 / 0.998 | 0.999 / 1.000 | |
| benchmarks < 0.95 | 13 / 12 | 9 / 10 | |
| benchmarks > 1.05 | `python_startup` 1.098, `fannkuch` 1.114 (A only) / `python_startup` 1.078 | `python_startup` 1.092 / 1.098 | `python_startup` 1.18 |
| per benchmark, A and B averaged (122 benchmarks) | **0.942** | **0.957** | 1.002 / 1.002 |

Largest gains: `deepcopy_memo` 0.13 (3.15.1) / 0.18 (3.15.1t), `deepcopy` 0.23 / 0.28,
`json_dumps` 0.25 / 0.33, `json_loads` 0.57 / 0.59, `logging_silent` 0.61–0.62,
`logging_format` 0.65 / 0.77, `logging_simple` 0.67 / 0.79, `sympy_integrate` 0.83,
`meteor_contest` 0.93, `hexiom` 0.93, `pathlib` 0.93–0.95. Raw files and `pyperf
compare_to` tables: `benches/results/phase3/pyperformance/`.

The three scoreboard cells still above 1.05 at `11bad29` (`count`/`in` with a 1-element
list or tuple of tuples, 1.050–1.070) were fixed after Phase 3 (non-scalar needles rejected
by one type-flag test, and a frameless delegation path): 1.026–1.045 on 3.15.1 and 3.15.1t
(callgrind, dev box; `docs/targets/containers.md`).

## Setup (parity program)

AWS c7a.8xlarge (AMD EPYC 9R14, 32 cores, no SMT, 4 L3 domains), Ubuntu 24.04,
python-build-standalone CPython 3.15.1 and 3.15.1t (GIL disabled), fat-LTO release builds
of rustique at `2922d13` (pyperformance and applications: `2789558`, which differs only in startup cost and the routing of large builtin sets) (baseline: `c93e24f`, i.e. `next` before this work). Concurrent
jobs were pinned to different L3 domains; every benchmark verifies its result against
CPython's.

- **Scoreboard** (`benches/scoreboard.py`): every builtin operation (literals,
  comprehensions, unpacking, `in`, iteration, `len`, `==`, hashing, indexing, mutation,
  set algebra, `json`, `pickle`, `copy`, `sorted`/`sum`/`min`/`max`/`any`/`all`/
  `enumerate`/`zip`/`reversed`) × 5 types × 5 storages (int, float, str, mixed, object)
  × 6 sizes (0, 1, 8, 100, 10k, 1M): 3,814 cells at 0–100 elements and 1,846 at 10k/1M per
  build. Sizes 0–100: callgrind instruction counts of exactly the measured region
  (load-independent; noise-free). 10k/1M: pyperf medians (3 processes × 3 values).
- **pyperformance 1.14.0**, full default suite (97 benchmarks, ~120 results), plain vs
  `rustique.enable()` injected through `sitecustomize` into every benchmark process.
- **Applications** (`benches/apps/run_apps.py`, `pyperf command`, 5 fresh processes each):
  networkx graph algorithms, pyflakes over ~90 files of CPython's tests, and a
  csv → dict/set group-by → sort → json pipeline.

## Headline: the default tier, `rustique.enable()`

| | 3.15.1 | 3.15.1t |
|---|---|---|
| pyperformance, geometric mean | **1.00×** | **1.01×** |
| networkx graph / pyflakes lint / csv-json etl | 1.005 / 0.963 / 0.987 | 1.006 / 0.998 / 0.993 |
| scoreboard 0–100 elements: cells ≤ 1.05 | 3810/3814 | 3814/3814 |
| scoreboard 0–100 elements: geometric mean | 0.93 | 0.93 |
| scoreboard 10k/1M: cells ≤ 1.05 | 1741/1846 | 1766/1846 |
| scoreboard 10k/1M: geometric mean | 0.88 | 0.88 |
| startup (`python -c pass`) | 10.4 → 12.1 ms (1.17×) | 12.7 → 14.9 ms (1.18×) |

pyperformance: 122 results (3.15.1) / 120 (3.15.1t); 114 / 107 are not
significant. Faster: hexiom 1.09×, meteor_contest 1.08× / 1.07×, logging_silent 1.03×.
Slower: `python_startup` 1.39× / 1.42× at `2789558` (fixed since: 1.17× / 1.18× at
`2922d13`, measured), `create_gc_cycles` 1.03× / 1.04× (still at `2922d13`: more objects
for the collector after the import, *inferred*), and a handful of 1.02–1.04× results whose
noise overlaps (`asyncio_tcp_ssl`, `base16_small`, `spectral_norm`; on 3.15.1t several
`async_tree_*`, `pyflate`, `2to3`, `xdsl_constant_fold`). Benchmarks that crash on plain
CPython 3.15 crash identically with Rustique and are excluded: `genshi` (both builds),
`asyncio_tcp` and `asyncio_tcp_ssl` (3.15.1t).

**Where the scoreboard cells come from.** The kernels change the code path of 328
cells per build at 10k/1M; their geometric mean is **0.46× (3.15.1) / 0.48× (3.15.1t)**,
e.g. at 1M: `max(floats)` 0.13 / 0.21, `min(ints)` 0.17 / 0.21, `ints.sort()` 0.18 /
0.15, `sorted(ints)` 0.29 / 0.22, `x in ints` 0.26 / 0.33, `ints.count(v)` 0.25 / 0.33,
`sum(ints)` 0.37 / 0.54, `min` over dict keys 0.42 / 0.38. The other 1518 cells run unchanged
CPython code: geometric mean 1.004 / 0.999, median 1.000, 100 / 80 cells above 1.05 and
62 / 82 below 0.95, i.e. the spread of pyperf medians between processes. Of the kernel
cells, 5 (3.15.1) / 0 (3.15.1t) are above 1.05, each within 1–2σ of its measured noise.

At 0–100 elements (instruction counts, no noise) the 4 cells above 1.05 on 3.15.1 are
size-0 operations that run no Rustique code (`list(x)`, `x.copy()`, iterating an empty
set); re-measured in isolation they are 0.93–1.00 (allocator state left by the previous
cell in the same callgrind worker). The fallbacks into CPython cost at most one call layer:
worst kernel-path cell 1.028 (`any(dict)` at size 0).

**Regressions found by these runs and fixed before release:** (1) startup: every patch
site re-read `/proc/self/maps`, and the package imported `collections` (1.39× → 1.17×);
(2) builtin sets: walking a large set table in hash order lost to CPython's iterator for
`all()` and for `min`/`max` over str/tuple elements (up to 1.68× at 1M); sets of ≥ 2048
elements now use the kernels only for numbers, and `any(dict)` always uses CPython.

## Rustique's own types

Geometric mean of the ratio to the builtin type over all scoreboard operations, and cells
at parity, baseline (`c93e24f`) → final. Rustique containers run with
`rustique.enable()` active (builtin functions dispatch to their native kernels).

**0–100 elements (instruction counts)**

| type | 3.15.1 cells ≤ 1.05 | 3.15.1 geomean | 3.15.1t cells ≤ 1.05 | 3.15.1t geomean |
|---|---:|---:|---:|---:|
| `List` | 73 → 233 / 890 | 2.10 → 1.21 | 74 → 181 / 890 | 2.02 → 1.31 |
| `Tuple` (new) | 192 / 719 | 1.32 | 189 / 719 | 1.40 |
| `Dict` | 9 → 67 / 819 | 2.73 → 1.70 | 9 → 70 / 819 | 2.65 → 1.75 |
| `Set` | 39 → 83 / 743 | 2.41 → 1.63 | 35 → 76 / 743 | 2.35 → 1.66 |
| `FrozenSet` (new) | 133 / 643 | 1.49 | 149 / 643 | 1.48 |

**10k / 1M elements (wall time)**

| type | 3.15.1 cells ≤ 1.05 | 3.15.1 geomean | 3.15.1t cells ≤ 1.05 | 3.15.1t geomean |
|---|---:|---:|---:|---:|
| `List` | 100 → 189 / 429 | 1.44 → 0.75 | 93 → 159 / 429 | 1.48 → 0.81 |
| `Tuple` (new) | 148 / 346 | 0.95 | 139 / 346 | 0.99 |
| `Dict` | 25 → 117 / 405 | 3.84 → 1.18 | 7 → 76 / 405 | 3.78 → 1.42 |
| `Set` | 84 → 164 / 358 | 4.23 → 0.89 | 92 → 152 / 358 | 4.23 → 0.99 |
| `FrozenSet` (new) | 144 / 308 | 0.95 | 161 / 308 | 0.92 |

Per-element operations on Rustique containers stay above 1.00 at small sizes because
CPython's interpreter only fast-paths exact builtins (DECISIONS.md §1): `List` runs `x[i]`,
`x[i] = v`, `len`, `if x` and iteration at 1.0–1.16× of the cost of an empty `list`
subclass (the floor for any foreign type), `append` below it. At large sizes native storage
wins: `copy`/slicing 0.08–0.33×, `sort` 0.02–0.07×, `in` 0.04×, set algebra and `Dict`
copies/merges below 1. Remaining gaps: small `Dict`/`Set` construction (three allocations
per table), `List(list_of_strs)` 2.3–6.4× at 100–10k, unpacking ~1.5× the floor.

## Full matrices

`docs/scoreboard/{enabled,rustique}-{3.15.1,3.15.1t}.md`: every cell, instruction-count
ratios at 0–100 elements and wall-time ratios at 10k/1M.

## Reproduce

```bash
.venv/bin/maturin develop --uv --release
.venv/bin/python benches/scoreboard.py instr --impls rustique,enabled --shards 8 -o instr.json
.venv/bin/python benches/scoreboard.py time --size 1e4,1e6 --impls rustique,enabled --shards 4 -o time.json
.venv/bin/python benches/scoreboard.py report instr.json time.json.shard*.json [--impl enabled]
.venv/bin/python benches/scoreboard.py summary --before base.json --after instr.json [--impl enabled]
.venv/bin/python benches/apps/run_apps.py bench -o apps.json
CPYTHON_SRC=<cpython checkout> .venv/bin/python tests/conformance/run_cpython_suites.py
```

pyperformance: `PYTHONPATH=<repo>/python:<dir with a sitecustomize.py that calls
rustique.enable() when RUSTIQUE_ENABLE=1> pyperformance run --inherit-environ
PYTHONPATH,RUSTIQUE_ENABLE`, once with `RUSTIQUE_ENABLE=0` and once with `1`.

## Earlier bulk-operation benchmarks (v0.2, `a6be77a`)

Kept for reference: the explicit Rustique API before this work, on the development WSL2
machine; `benches/bench.py` still reproduces them.

Produced by `benches/bench.py --sizes 1e5,1e6 --repeat 7` against a `maturin develop
--release` build (fat LTO, one codegen unit) on an otherwise idle 8-CPU WSL2 machine
(Ryzen 5600X). Ratios are rustique time ÷ reference time (the `(ref)` row, usually the
builtin); lower is faster. Every measurement rebuilds its inputs and checks its result
against the reference result. Absolute times on this VM are noisy; compare ratios.
Reproduce with:

```bash
.venv/bin/maturin develop --uv --release
.venv/bin/python benches/bench.py --sizes 1e5,1e6 --repeat 7
.venv/bin/python benches/list_hotpaths.py        # per-element hot paths, incl. 3.15t
```

`3.15.1` GIL=on, rustique 0.2.0, numpy 2.5.4, 8 CPUs, git a6be77a (code; docs-only commits since); best of 7, fresh inputs per repetition, results verified.

  construct: from list[int] n=100,000: list=74.6 µs, List=229.4 µs, List[int]=248.6 µs, array('q')=1.16 ms, numpy=1.63 ms
  construct: from list[int] n=1,000,000: list=2.80 ms, List=2.59 ms, List[int]=2.80 ms, array('q')=11.61 ms, numpy=16.35 ms
  construct: from range n=100,000: list=625.7 µs, List=62.7 µs, arange=60.7 µs, array('q')=2.82 ms, numpy=50.0 µs
  construct: from range n=1,000,000: list=11.96 ms, List=656.8 µs, arange=658.2 µs, array('q')=28.59 ms, numpy=649.9 µs
  construct: from generator n=100,000: list=1.39 ms, List=1.78 ms, List[int]=2.96 ms, array('q')=3.52 ms
  construct: from generator n=1,000,000: list=14.88 ms, List=20.11 ms, List[int]=29.76 ms, array('q')=35.21 ms
  construct: from list[float] n=100,000: list=81.7 µs, List=97.4 µs, List[float]=104.0 µs, numpy=1.55 ms
  construct: from list[float] n=1,000,000: list=2.95 ms, List=2.23 ms, List[float]=2.34 ms, numpy=15.60 ms
  construct: from list[str] n=100,000: list=110.2 µs, List=1.03 ms, List[str]=1.01 ms
  construct: from list[str] n=1,000,000: list=5.28 ms, List=10.41 ms, List[str]=10.44 ms
  index: x[i] (100k random) n=100,000: list=5.03 ms, List=5.29 ms, array('q')=4.85 ms, numpy=6.44 ms
  index: x[i] (100k random) n=1,000,000: list=18.73 ms, List=11.01 ms, array('q')=11.14 ms, numpy=11.92 ms
  iterate: for x in c n=100,000: list=2.44 ms, List=3.75 ms, array('q')=3.08 ms, numpy=4.40 ms
  iterate: for x in c n=1,000,000: list=25.07 ms, List=38.02 ms, array('q')=30.77 ms, numpy=44.10 ms
  append: n x c.append(v) n=100,000: list=986.9 µs, List=3.43 ms, List[int]=4.04 ms, array('q')=3.39 ms
  append: n x c.append(v) n=1,000,000: list=9.58 ms, List=34.45 ms, List[int]=40.95 ms, array('q')=34.09 ms
  contains: v in c n=100,000: list=49.03 ms, List=1.14 ms, array('q')=101.06 ms
  contains: v in c n=1,000,000: list=49.98 ms, List=2.13 ms, array('q')=101.49 ms
  sum: int n=100,000: list=344.7 µs, List.sum()=18.6 µs, sum(List)=1.33 ms, array('q')=747.4 µs, numpy.sum=31.2 µs
  sum: int n=1,000,000: list=3.55 ms, List.sum()=417.5 µs, sum(List)=14.88 ms, array('q')=7.51 ms, numpy.sum=452.4 µs
  min: int n=100,000: list=849.3 µs, List.min()=23.2 µs, min(List)=1.86 ms, array('q')=1.28 ms, numpy.min=33.0 µs
  min: int n=1,000,000: list=8.63 ms, List.min()=400.7 µs, min(List)=20.17 ms, array('q')=12.91 ms, numpy.min=441.6 µs
  max: int n=100,000: list=905.3 µs, List.max()=24.7 µs, max(List)=1.91 ms, array('q')=1.33 ms, numpy.max=32.2 µs
  max: int n=1,000,000: list=9.13 ms, List.max()=403.0 µs, max(List)=20.06 ms, array('q')=13.31 ms, numpy.max=448.4 µs
  sum: float (compensated) n=100,000: list=402.1 µs, List.sum()=107.3 µs
  sum: float (compensated) n=1,000,000: list=4.11 ms, List.sum()=1.05 ms
  sort: int (in place) n=100,000: list=10.56 ms, List=539.9 µs, List[int]=561.9 µs, numpy=769.7 µs
  sort: int (in place) n=1,000,000: list=173.82 ms, List=4.42 ms, List[int]=4.22 ms, numpy=9.00 ms
  sort: float (in place) n=100,000: list=10.20 ms, List=636.9 µs, numpy=678.6 µs
  sort: float (in place) n=1,000,000: list=163.93 ms, List=5.59 ms, numpy=7.70 ms
  sort: str (in place) n=100,000: list=15.74 ms, List=2.04 ms
  sort: str (in place) n=1,000,000: list=290.71 ms, List=20.48 ms
  slice: c[n//4 : 3n//4] n=100,000: list=44.1 µs, List=23.6 µs, array('q')=19.1 µs, numpy (copy)=28.0 µs
  slice: c[n//4 : 3n//4] n=1,000,000: list=904.4 µs, List=437.2 µs, array('q')=388.5 µs, numpy (copy)=423.0 µs
  slice: c[::3] n=100,000: list=51.0 µs, List=24.5 µs, numpy (copy)=33.8 µs
  slice: c[::3] n=1,000,000: list=2.18 ms, List=548.3 µs, numpy (copy)=509.1 µs
  map: x*2+1 n=100,000: list comp=4.38 ms, List.map(X*2+1)=256.3 µs, List.map(lambda)=9.08 ms, numpy=177.7 µs
  map: x*2+1 n=1,000,000: list comp=41.13 ms, List.map(X*2+1)=2.42 ms, List.map(lambda)=101.07 ms, numpy=1.81 ms
  filter: x%3==0 n=100,000: list comp=2.41 ms, List.filter(X%3==0)=663.0 µs, List.filter(lambda)=6.48 ms, numpy=814.4 µs
  filter: x%3==0 n=1,000,000: list comp=27.17 ms, List.filter(X%3==0)=2.83 ms, List.filter(lambda)=68.29 ms, numpy=7.92 ms
  map: float x*0.5-1 n=100,000: list comp=3.39 ms, List.map(X*0.5-1)=183.2 µs, numpy=170.9 µs
  map: float x*0.5-1 n=1,000,000: list comp=32.15 ms, List.map(X*0.5-1)=2.38 ms, numpy=1.88 ms
  dict build: int keys n=100,000: dict=9.90 ms, Dict=13.92 ms
  dict build: int keys n=1,000,000: dict=122.52 ms, Dict=267.50 ms
  dict lookup: int keys (100k) n=100,000: dict=13.70 ms, Dict=16.51 ms
  dict lookup: int keys (100k) n=1,000,000: dict=16.57 ms, Dict=22.23 ms
  dict build: str keys n=100,000: dict=5.22 ms, Dict=9.27 ms
  dict build: str keys n=1,000,000: dict=125.48 ms, Dict=246.57 ms
  dict lookup: str keys (100k) n=100,000: dict=6.69 ms, Dict=9.33 ms
  dict lookup: str keys (100k) n=1,000,000: dict=13.71 ms, Dict=23.93 ms
  set | n=100,000: set=1.94 ms, Set=2.42 ms
  set | n=1,000,000: set=110.21 ms, Set=92.50 ms
  set & n=100,000: set=1.71 ms, Set=2.26 ms
  set & n=1,000,000: set=60.00 ms, Set=95.50 ms
  set - n=100,000: set=1.96 ms, Set=2.59 ms
  set - n=1,000,000: set=62.88 ms, Set=102.71 ms
  set ^ n=100,000: set=2.07 ms, Set=4.96 ms
  set ^ n=1,000,000: set=115.59 ms, Set=179.63 ms
  set build: int n=100,000: set=2.68 ms, Set=4.63 ms
  set build: int n=1,000,000: set=74.58 ms, Set=180.07 ms
| case | n | implementation | best | median | ratio vs builtin |
|---|---:|---|---:|---:|---:|
| construct: from list[int] | 100,000 | list | 74.6 µs | 87.2 µs | 1.00 (ref) |
| construct: from list[int] | 100,000 | List | 229.4 µs | 237.7 µs | 3.07 |
| construct: from list[int] | 100,000 | List[int] | 248.6 µs | 258.3 µs | 3.33 |
| construct: from list[int] | 100,000 | array('q') | 1.16 ms | 1.19 ms | 15.56 |
| construct: from list[int] | 100,000 | numpy | 1.63 ms | 1.64 ms | 21.81 |
| construct: from list[int] | 1,000,000 | list | 2.80 ms | 3.08 ms | 1.00 (ref) |
| construct: from list[int] | 1,000,000 | List | 2.59 ms | 2.71 ms | 0.93 |
| construct: from list[int] | 1,000,000 | List[int] | 2.80 ms | 2.84 ms | 1.00 |
| construct: from list[int] | 1,000,000 | array('q') | 11.61 ms | 11.73 ms | 4.15 |
| construct: from list[int] | 1,000,000 | numpy | 16.35 ms | 16.46 ms | 5.84 |
| construct: from range | 100,000 | list | 625.7 µs | 666.5 µs | 1.00 (ref) |
| construct: from range | 100,000 | List | 62.7 µs | 66.8 µs | 0.10 |
| construct: from range | 100,000 | arange | 60.7 µs | 63.1 µs | 0.10 |
| construct: from range | 100,000 | array('q') | 2.82 ms | 2.87 ms | 4.50 |
| construct: from range | 100,000 | numpy | 50.0 µs | 58.4 µs | 0.08 |
| construct: from range | 1,000,000 | list | 11.96 ms | 15.01 ms | 1.00 (ref) |
| construct: from range | 1,000,000 | List | 656.8 µs | 677.4 µs | 0.05 |
| construct: from range | 1,000,000 | arange | 658.2 µs | 679.9 µs | 0.06 |
| construct: from range | 1,000,000 | array('q') | 28.59 ms | 29.25 ms | 2.39 |
| construct: from range | 1,000,000 | numpy | 649.9 µs | 684.7 µs | 0.05 |
| construct: from generator | 100,000 | list | 1.39 ms | 1.44 ms | 1.00 (ref) |
| construct: from generator | 100,000 | List | 1.78 ms | 1.82 ms | 1.28 |
| construct: from generator | 100,000 | List[int] | 2.96 ms | 2.97 ms | 2.12 |
| construct: from generator | 100,000 | array('q') | 3.52 ms | 3.57 ms | 2.52 |
| construct: from generator | 1,000,000 | list | 14.88 ms | 15.33 ms | 1.00 (ref) |
| construct: from generator | 1,000,000 | List | 20.11 ms | 22.18 ms | 1.35 |
| construct: from generator | 1,000,000 | List[int] | 29.76 ms | 30.05 ms | 2.00 |
| construct: from generator | 1,000,000 | array('q') | 35.21 ms | 35.40 ms | 2.37 |
| construct: from list[float] | 100,000 | list | 81.7 µs | 94.2 µs | 1.00 (ref) |
| construct: from list[float] | 100,000 | List | 97.4 µs | 110.9 µs | 1.19 |
| construct: from list[float] | 100,000 | List[float] | 104.0 µs | 108.4 µs | 1.27 |
| construct: from list[float] | 100,000 | numpy | 1.55 ms | 1.56 ms | 18.99 |
| construct: from list[float] | 1,000,000 | list | 2.95 ms | 3.03 ms | 1.00 (ref) |
| construct: from list[float] | 1,000,000 | List | 2.23 ms | 2.38 ms | 0.75 |
| construct: from list[float] | 1,000,000 | List[float] | 2.34 ms | 2.39 ms | 0.79 |
| construct: from list[float] | 1,000,000 | numpy | 15.60 ms | 16.00 ms | 5.29 |
| construct: from list[str] | 100,000 | list | 110.2 µs | 120.0 µs | 1.00 (ref) |
| construct: from list[str] | 100,000 | List | 1.03 ms | 1.05 ms | 9.36 |
| construct: from list[str] | 100,000 | List[str] | 1.01 ms | 1.02 ms | 9.13 |
| construct: from list[str] | 1,000,000 | list | 5.28 ms | 5.71 ms | 1.00 (ref) |
| construct: from list[str] | 1,000,000 | List | 10.41 ms | 10.58 ms | 1.97 |
| construct: from list[str] | 1,000,000 | List[str] | 10.44 ms | 20.52 ms | 1.98 |
| index: x[i] (100k random) | 100,000 | list | 5.03 ms | 5.40 ms | 1.00 (ref) |
| index: x[i] (100k random) | 100,000 | List | 5.29 ms | 5.67 ms | 1.05 |
| index: x[i] (100k random) | 100,000 | array('q') | 4.85 ms | 4.93 ms | 0.96 |
| index: x[i] (100k random) | 100,000 | numpy | 6.44 ms | 6.56 ms | 1.28 |
| index: x[i] (100k random) | 1,000,000 | list | 18.73 ms | 21.47 ms | 1.00 (ref) |
| index: x[i] (100k random) | 1,000,000 | List | 11.01 ms | 12.62 ms | 0.59 |
| index: x[i] (100k random) | 1,000,000 | array('q') | 11.14 ms | 11.95 ms | 0.59 |
| index: x[i] (100k random) | 1,000,000 | numpy | 11.92 ms | 13.48 ms | 0.64 |
| iterate: for x in c | 100,000 | list | 2.44 ms | 2.53 ms | 1.00 (ref) |
| iterate: for x in c | 100,000 | List | 3.75 ms | 3.88 ms | 1.54 |
| iterate: for x in c | 100,000 | array('q') | 3.08 ms | 3.22 ms | 1.26 |
| iterate: for x in c | 100,000 | numpy | 4.40 ms | 4.48 ms | 1.80 |
| iterate: for x in c | 1,000,000 | list | 25.07 ms | 25.41 ms | 1.00 (ref) |
| iterate: for x in c | 1,000,000 | List | 38.02 ms | 39.99 ms | 1.52 |
| iterate: for x in c | 1,000,000 | array('q') | 30.77 ms | 31.27 ms | 1.23 |
| iterate: for x in c | 1,000,000 | numpy | 44.10 ms | 45.61 ms | 1.76 |
| append: n x c.append(v) | 100,000 | list | 986.9 µs | 1.53 ms | 1.00 (ref) |
| append: n x c.append(v) | 100,000 | List | 3.43 ms | 3.48 ms | 3.47 |
| append: n x c.append(v) | 100,000 | List[int] | 4.04 ms | 4.11 ms | 4.09 |
| append: n x c.append(v) | 100,000 | array('q') | 3.39 ms | 3.48 ms | 3.43 |
| append: n x c.append(v) | 1,000,000 | list | 9.58 ms | 15.25 ms | 1.00 (ref) |
| append: n x c.append(v) | 1,000,000 | List | 34.45 ms | 35.09 ms | 3.60 |
| append: n x c.append(v) | 1,000,000 | List[int] | 40.95 ms | 41.22 ms | 4.27 |
| append: n x c.append(v) | 1,000,000 | array('q') | 34.09 ms | 34.47 ms | 3.56 |
| contains: v in c | 100,000 | list | 49.03 ms | 49.99 ms | 1.00 (ref) |
| contains: v in c | 100,000 | List | 1.14 ms | 1.16 ms | 0.02 |
| contains: v in c | 100,000 | array('q') | 101.06 ms | 101.80 ms | 2.06 |
| contains: v in c | 1,000,000 | list | 49.98 ms | 52.54 ms | 1.00 (ref) |
| contains: v in c | 1,000,000 | List | 2.13 ms | 2.29 ms | 0.04 |
| contains: v in c | 1,000,000 | array('q') | 101.49 ms | 102.31 ms | 2.03 |
| sum: int | 100,000 | list | 344.7 µs | 351.1 µs | 1.00 (ref) |
| sum: int | 100,000 | List.sum() | 18.6 µs | 23.3 µs | 0.05 |
| sum: int | 100,000 | sum(List) | 1.33 ms | 1.34 ms | 3.86 |
| sum: int | 100,000 | array('q') | 747.4 µs | 754.5 µs | 2.17 |
| sum: int | 100,000 | numpy.sum | 31.2 µs | 38.2 µs | 0.09 |
| sum: int | 1,000,000 | list | 3.55 ms | 3.79 ms | 1.00 (ref) |
| sum: int | 1,000,000 | List.sum() | 417.5 µs | 469.1 µs | 0.12 |
| sum: int | 1,000,000 | sum(List) | 14.88 ms | 15.79 ms | 4.19 |
| sum: int | 1,000,000 | array('q') | 7.51 ms | 7.56 ms | 2.11 |
| sum: int | 1,000,000 | numpy.sum | 452.4 µs | 466.3 µs | 0.13 |
| min: int | 100,000 | list | 849.3 µs | 862.8 µs | 1.00 (ref) |
| min: int | 100,000 | List.min() | 23.2 µs | 24.4 µs | 0.03 |
| min: int | 100,000 | min(List) | 1.86 ms | 1.91 ms | 2.19 |
| min: int | 100,000 | array('q') | 1.28 ms | 1.30 ms | 1.51 |
| min: int | 100,000 | numpy.min | 33.0 µs | 46.8 µs | 0.04 |
| min: int | 1,000,000 | list | 8.63 ms | 8.89 ms | 1.00 (ref) |
| min: int | 1,000,000 | List.min() | 400.7 µs | 483.5 µs | 0.05 |
| min: int | 1,000,000 | min(List) | 20.17 ms | 20.82 ms | 2.34 |
| min: int | 1,000,000 | array('q') | 12.91 ms | 13.02 ms | 1.49 |
| min: int | 1,000,000 | numpy.min | 441.6 µs | 474.7 µs | 0.05 |
| max: int | 100,000 | list | 905.3 µs | 913.0 µs | 1.00 (ref) |
| max: int | 100,000 | List.max() | 24.7 µs | 32.9 µs | 0.03 |
| max: int | 100,000 | max(List) | 1.91 ms | 1.98 ms | 2.11 |
| max: int | 100,000 | array('q') | 1.33 ms | 1.34 ms | 1.47 |
| max: int | 100,000 | numpy.max | 32.2 µs | 42.7 µs | 0.04 |
| max: int | 1,000,000 | list | 9.13 ms | 9.42 ms | 1.00 (ref) |
| max: int | 1,000,000 | List.max() | 403.0 µs | 504.7 µs | 0.04 |
| max: int | 1,000,000 | max(List) | 20.06 ms | 20.97 ms | 2.20 |
| max: int | 1,000,000 | array('q') | 13.31 ms | 13.39 ms | 1.46 |
| max: int | 1,000,000 | numpy.max | 448.4 µs | 467.1 µs | 0.05 |
| sum: float (compensated) | 100,000 | list | 402.1 µs | 418.6 µs | 1.00 (ref) |
| sum: float (compensated) | 100,000 | List.sum() | 107.3 µs | 112.3 µs | 0.27 |
| sum: float (compensated) | 1,000,000 | list | 4.11 ms | 4.37 ms | 1.00 (ref) |
| sum: float (compensated) | 1,000,000 | List.sum() | 1.05 ms | 1.06 ms | 0.26 |
| sort: int (in place) | 100,000 | list | 10.56 ms | 10.73 ms | 1.00 (ref) |
| sort: int (in place) | 100,000 | List | 539.9 µs | 622.9 µs | 0.05 |
| sort: int (in place) | 100,000 | List[int] | 561.9 µs | 672.2 µs | 0.05 |
| sort: int (in place) | 100,000 | numpy | 769.7 µs | 780.6 µs | 0.07 |
| sort: int (in place) | 1,000,000 | list | 173.82 ms | 181.53 ms | 1.00 (ref) |
| sort: int (in place) | 1,000,000 | List | 4.42 ms | 4.70 ms | 0.03 |
| sort: int (in place) | 1,000,000 | List[int] | 4.22 ms | 4.51 ms | 0.02 |
| sort: int (in place) | 1,000,000 | numpy | 9.00 ms | 9.15 ms | 0.05 |
| sort: float (in place) | 100,000 | list | 10.20 ms | 10.28 ms | 1.00 (ref) |
| sort: float (in place) | 100,000 | List | 636.9 µs | 801.1 µs | 0.06 |
| sort: float (in place) | 100,000 | numpy | 678.6 µs | 707.3 µs | 0.07 |
| sort: float (in place) | 1,000,000 | list | 163.93 ms | 171.23 ms | 1.00 (ref) |
| sort: float (in place) | 1,000,000 | List | 5.59 ms | 5.92 ms | 0.03 |
| sort: float (in place) | 1,000,000 | numpy | 7.70 ms | 7.92 ms | 0.05 |
| sort: str (in place) | 100,000 | list | 15.74 ms | 15.86 ms | 1.00 (ref) |
| sort: str (in place) | 100,000 | List | 2.04 ms | 2.10 ms | 0.13 |
| sort: str (in place) | 1,000,000 | list | 290.71 ms | 325.89 ms | 1.00 (ref) |
| sort: str (in place) | 1,000,000 | List | 20.48 ms | 20.86 ms | 0.07 |
| slice: c[n//4 : 3n//4] | 100,000 | list | 44.1 µs | 47.5 µs | 1.00 (ref) |
| slice: c[n//4 : 3n//4] | 100,000 | List | 23.6 µs | 29.0 µs | 0.53 |
| slice: c[n//4 : 3n//4] | 100,000 | array('q') | 19.1 µs | 26.5 µs | 0.43 |
| slice: c[n//4 : 3n//4] | 100,000 | numpy (copy) | 28.0 µs | 40.2 µs | 0.64 |
| slice: c[n//4 : 3n//4] | 1,000,000 | list | 904.4 µs | 1.20 ms | 1.00 (ref) |
| slice: c[n//4 : 3n//4] | 1,000,000 | List | 437.2 µs | 523.2 µs | 0.48 |
| slice: c[n//4 : 3n//4] | 1,000,000 | array('q') | 388.5 µs | 464.6 µs | 0.43 |
| slice: c[n//4 : 3n//4] | 1,000,000 | numpy (copy) | 423.0 µs | 499.5 µs | 0.47 |
| slice: c[::3] | 100,000 | list | 51.0 µs | 57.2 µs | 1.00 (ref) |
| slice: c[::3] | 100,000 | List | 24.5 µs | 36.3 µs | 0.48 |
| slice: c[::3] | 100,000 | numpy (copy) | 33.8 µs | 43.4 µs | 0.66 |
| slice: c[::3] | 1,000,000 | list | 2.18 ms | 2.47 ms | 1.00 (ref) |
| slice: c[::3] | 1,000,000 | List | 548.3 µs | 777.7 µs | 0.25 |
| slice: c[::3] | 1,000,000 | numpy (copy) | 509.1 µs | 538.3 µs | 0.23 |
| map: x*2+1 | 100,000 | list comp | 4.38 ms | 4.77 ms | 1.00 (ref) |
| map: x*2+1 | 100,000 | List.map(X*2+1) | 256.3 µs | 271.1 µs | 0.06 |
| map: x*2+1 | 100,000 | List.map(lambda) | 9.08 ms | 9.75 ms | 2.08 |
| map: x*2+1 | 100,000 | numpy | 177.7 µs | 184.4 µs | 0.04 |
| map: x*2+1 | 1,000,000 | list comp | 41.13 ms | 51.87 ms | 1.00 (ref) |
| map: x*2+1 | 1,000,000 | List.map(X*2+1) | 2.42 ms | 2.49 ms | 0.06 |
| map: x*2+1 | 1,000,000 | List.map(lambda) | 101.07 ms | 107.58 ms | 2.46 |
| map: x*2+1 | 1,000,000 | numpy | 1.81 ms | 1.94 ms | 0.04 |
| filter: x%3==0 | 100,000 | list comp | 2.41 ms | 2.46 ms | 1.00 (ref) |
| filter: x%3==0 | 100,000 | List.filter(X%3==0) | 663.0 µs | 673.1 µs | 0.27 |
| filter: x%3==0 | 100,000 | List.filter(lambda) | 6.48 ms | 6.87 ms | 2.68 |
| filter: x%3==0 | 100,000 | numpy | 814.4 µs | 827.0 µs | 0.34 |
| filter: x%3==0 | 1,000,000 | list comp | 27.17 ms | 28.71 ms | 1.00 (ref) |
| filter: x%3==0 | 1,000,000 | List.filter(X%3==0) | 2.83 ms | 3.03 ms | 0.10 |
| filter: x%3==0 | 1,000,000 | List.filter(lambda) | 68.29 ms | 75.64 ms | 2.51 |
| filter: x%3==0 | 1,000,000 | numpy | 7.92 ms | 8.23 ms | 0.29 |
| map: float x*0.5-1 | 100,000 | list comp | 3.39 ms | 4.27 ms | 1.00 (ref) |
| map: float x*0.5-1 | 100,000 | List.map(X*0.5-1) | 183.2 µs | 188.1 µs | 0.05 |
| map: float x*0.5-1 | 100,000 | numpy | 170.9 µs | 179.3 µs | 0.05 |
| map: float x*0.5-1 | 1,000,000 | list comp | 32.15 ms | 40.82 ms | 1.00 (ref) |
| map: float x*0.5-1 | 1,000,000 | List.map(X*0.5-1) | 2.38 ms | 2.50 ms | 0.07 |
| map: float x*0.5-1 | 1,000,000 | numpy | 1.88 ms | 1.99 ms | 0.06 |
| dict build: int keys | 100,000 | dict | 9.90 ms | 11.17 ms | 1.00 (ref) |
| dict build: int keys | 100,000 | Dict | 13.92 ms | 14.78 ms | 1.41 |
| dict build: int keys | 1,000,000 | dict | 122.52 ms | 136.42 ms | 1.00 (ref) |
| dict build: int keys | 1,000,000 | Dict | 267.50 ms | 290.79 ms | 2.18 |
| dict lookup: int keys (100k) | 100,000 | dict | 13.70 ms | 14.79 ms | 1.00 (ref) |
| dict lookup: int keys (100k) | 100,000 | Dict | 16.51 ms | 16.96 ms | 1.21 |
| dict lookup: int keys (100k) | 1,000,000 | dict | 16.57 ms | 21.17 ms | 1.00 (ref) |
| dict lookup: int keys (100k) | 1,000,000 | Dict | 22.23 ms | 26.55 ms | 1.34 |
| dict build: str keys | 100,000 | dict | 5.22 ms | 6.10 ms | 1.00 (ref) |
| dict build: str keys | 100,000 | Dict | 9.27 ms | 11.45 ms | 1.78 |
| dict build: str keys | 1,000,000 | dict | 125.48 ms | 140.37 ms | 1.00 (ref) |
| dict build: str keys | 1,000,000 | Dict | 246.57 ms | 277.35 ms | 1.97 |
| dict lookup: str keys (100k) | 100,000 | dict | 6.69 ms | 6.93 ms | 1.00 (ref) |
| dict lookup: str keys (100k) | 100,000 | Dict | 9.33 ms | 9.47 ms | 1.39 |
| dict lookup: str keys (100k) | 1,000,000 | dict | 13.71 ms | 16.00 ms | 1.00 (ref) |
| dict lookup: str keys (100k) | 1,000,000 | Dict | 23.93 ms | 24.86 ms | 1.75 |
| set \| | 100,000 | set | 1.94 ms | 3.49 ms | 1.00 (ref) |
| set \| | 100,000 | Set | 2.42 ms | 3.89 ms | 1.25 |
| set \| | 1,000,000 | set | 110.21 ms | 128.00 ms | 1.00 (ref) |
| set \| | 1,000,000 | Set | 92.50 ms | 93.74 ms | 0.84 |
| set & | 100,000 | set | 1.71 ms | 2.02 ms | 1.00 (ref) |
| set & | 100,000 | Set | 2.26 ms | 2.88 ms | 1.32 |
| set & | 1,000,000 | set | 60.00 ms | 61.08 ms | 1.00 (ref) |
| set & | 1,000,000 | Set | 95.50 ms | 101.47 ms | 1.59 |
| set - | 100,000 | set | 1.96 ms | 2.32 ms | 1.00 (ref) |
| set - | 100,000 | Set | 2.59 ms | 2.69 ms | 1.32 |
| set - | 1,000,000 | set | 62.88 ms | 65.95 ms | 1.00 (ref) |
| set - | 1,000,000 | Set | 102.71 ms | 112.80 ms | 1.63 |
| set ^ | 100,000 | set | 2.07 ms | 2.64 ms | 1.00 (ref) |
| set ^ | 100,000 | Set | 4.96 ms | 5.49 ms | 2.39 |
| set ^ | 1,000,000 | set | 115.59 ms | 117.77 ms | 1.00 (ref) |
| set ^ | 1,000,000 | Set | 179.63 ms | 188.22 ms | 1.55 |
| set build: int | 100,000 | set | 2.68 ms | 2.85 ms | 1.00 (ref) |
| set build: int | 100,000 | Set | 4.63 ms | 5.26 ms | 1.73 |
| set build: int | 1,000,000 | set | 74.58 ms | 76.67 ms | 1.00 (ref) |
| set build: int | 1,000,000 | Set | 180.07 ms | 207.69 ms | 2.41 |

Notes:
- sum: float (compensated): Python's float sum is compensated (Neumaier); List.sum() must return the identical value. numpy.sum (pairwise) and math.fsum (exact) differ in the last bits, so they are not listed.
