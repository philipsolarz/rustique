# Phase 0 — ground truth on the benchmark box

Rustique at `5abd54c` (code identical to `2922d13`, the commit of the published scoreboard;
the three commits in between touch only docs and CI), rebuilt and re-measured from scratch
on a fresh AWS c7a.8xlarge on 2026-11-01. Everything here is **measured** on that box unless
tagged *inferred*; "published" means the numbers committed in `docs/BENCHMARKS.md`,
`docs/DECISIONS.md`, `docs/TAKEOVER.md` and `docs/scoreboard/*.md`.

Raw data, logs and the rerunnable script: `benches/results/phase0/` (see §11).

## Summary

| | result | vs published |
|---|---|---|
| builds | fat-LTO release builds succeed on 3.14.9, 3.14.9t, 3.15.1, 3.15.1t **and 3.16.0a1/3.16.0a1t** | 3.16 was expected to fail; PyO3 0.30.1 accepts max + 1 as "experimental" (§5) |
| tests | pytest 0 failures on every build (3873–3883 passed); downstream extension 80/80; examples ok; `cargo fmt`/`clippy -D warnings`/`cargo test` (120) clean | — |
| conformance | container suites 67/69, 37/38, 35/36, 52/54, 31/33 (3.15.x), all documented deviations; native-tier identity gate passes on all 4 builds | suites identical; TAKEOVER.md §3.7 rustique-mode rows differ for dict/set/dictviews (−1/+6/−1) |
| scoreboard 0–100 (instr) | 3807/3814 (3814 after isolating 7 allocator-state cells) / 3814/3814 ≤ 1.05; geomean 0.931 / 0.935 | 3810 / 3814; 0.93 / 0.93 — reproduced |
| scoreboard 10k/1M (wall) | 1750 / 1773 of 1846 ≤ 1.05; geomean 0.871 / 0.877; kernel path (328 cells) **0.460 / 0.477** | 1741 / 1766; 0.88 / 0.88; 0.46 / 0.48 — reproduced in aggregate |
| kernel cells > 1.05 | 5 / 3 at 3 processes, **0 / 0 at 10 processes** | 5 / 0 |
| pyperformance geomean | **1.002 / 1.002** (two replicates: 1.000 & 1.004 / 0.999 & 1.006) | 1.00 / 1.01 |
| startup (`python_startup`) | 10.4 → 12.2 ms (1.17×) / 12.9 → 15.1 ms (1.17–1.19×) | 1.17× / 1.18× |
| apps graph / lint / etl (3.15.1) | 1.005–1.012 / 1.001–1.002 / 0.986–0.994 (3 runs) | 1.005 / **0.963** / 0.987 |

Deviations worth knowing (details in §6.2, §7, §8): (1) per-cell wall-time ratios at
3 pyperf processes move ±8% (p05–p95) between runs, so *which* cells exceed 1.05 is mostly
noise — at 10 processes every kernel cell is ≤ 1.05; (2) on 3.15.1 `sorted(tuple)` of 10k
elements measures 0.18–0.19 (published 0.28–0.30; `sorted(list)` 0.275 as published) and
`list.sort()` of 1M ints 0.21 (published 0.18); (3) the pyperformance "crashes" of
`asyncio_tcp`/`asyncio_tcp_ssl` on 3.15.1t are port collisions between concurrent
pyperformance runs, not crashes — run alone they pass; (4) the lint app's published 0.963 is
not reproduced — not with a ~90-file CPython test directory, not at the published commit
`2789558` (§8); (5) BENCHMARKS.md's prose quotes some headline values that are not in
its own committed matrices. `genshi` fails on plain CPython 3.15 on both builds.

Not done: nothing from the brief is missing. Extra beyond the brief: 3.16 tests, the
10-process recheck, a second pyperformance replicate per build, the port-benchmark re-runs,
the lint app on CPython's tests, `--impls rustique` instr + time, and the apps at `2789558`.
One side run was invalid and discarded (§10).

## 1. What was run

**Box.** AWS c7a.8xlarge (`machine/instance-type.txt`): AMD EPYC 9R14, 32 cores without SMT, 4 L3 domains of
8 cores (0–7, 8–15, 16–23, 24–31; 32 MiB L3 each), 61 GiB RAM, 200 GB gp3, Ubuntu 24.04.5,
kernel `7.0.0-1014-aws`, KVM guest. Facts in `machine/`.

**Toolchain** (all recorded in `machine/`, `logs/setup-*.log`, `tests/freeze-*.txt`):

| | version |
|---|---|
| Rust | 1.99.1 (from `rust-toolchain.toml`; LLVM 23.1.1), rustfmt 1.10.0, clippy 0.1.99 |
| uv | 0.13.5 (pinned: same python-build-standalone release as the dev machine) |
| CPython | python-build-standalone `20261021`, `x86_64` baseline builds, identical `python -VV` to the dev machine: 3.14.9 (Oct 21 2026, 17:48:02), 3.14.9t (17:49:33), 3.15.1 (17:48:50), 3.15.1t (17:49:09), 3.16.0a1 (17:49:48), 3.16.0a1t (17:45:35), all `[Clang 23.1.3]` |
| PyO3 / maturin | 0.30.1 / 1.15.0 (pip's isolated sdist build pulled maturin 1.16.0) |
| test deps | pytest 9.1.1, hypothesis 6.170.9, numpy 2.5.4, pyperf 2.10.0 (pinned to the dev venv), networkx 3.7, pyflakes 4.1.0 |
| pyperformance | 1.14.0 (benchmark venv freeze: `pyperformance/bench-venv-freeze-*.txt`) |
| valgrind / perf | 3.22.0 / 7.0.14 |
| CPython sources for the gates | tags `v3.15.1`, `v3.14.9` (sparse `Lib/test`) |

**Build.** One clone, venv and `CARGO_TARGET_DIR` per interpreter;
`maturin develop --uv --release` (fat LTO, one codegen unit). `maturin develop` also installs
the `dev` dependency group (mypy, pyright), which is harmless.

**Commands** — exactly what `benches/results/phase0/run_phase0.sh` runs:

```bash
# per build b in 3.15.1 3.15.1t 3.14.9 3.14.9t (+ 3.16.0a1 3.16.0a1t, §5)
maturin develop --uv --release
python -m pytest -q --durations=25 --junitxml=...                 # "pytest"
for f in examples/*.py; do python $f; done
maturin develop --uv --release -m examples/rust_extension/Cargo.toml
python -m pytest -q examples/rust_extension tests/test_capi.py   # "downstream"
python -m pytest -q                                               # "pytest-full": again, with rustique_demo built
# once (PYO3_PYTHON=3.15.1, LD_LIBRARY_PATH=its LIBDIR, as in ci.yml)
cargo fmt --all --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace
# conformance, per build, CPYTHON_SRC = matching v3.15.1 / v3.14.9 checkout
python tests/conformance/run_cpython_suites.py --json ...
python tests/test_takeover_conformance.py --json ...              # plain / identity / rustique
python -m pytest -q tests/test_takeover_conformance.py            # the identity==plain gate
# scoreboard (enabled tier), 3.15.1 and 3.15.1t
taskset -c 0-7 python benches/scoreboard.py instr --impls enabled --shards 8 -o instr.json
python benches/scoreboard.py time --size 1e4,1e6 --impls enabled --shards 3 --affinity 10,20,30 -o time.json
python benches/scoreboard.py report|summary instr.json time.json.shard*.json --impl enabled
python benches/scoreboard.py time ... --type T --op OP --storage S --shards 1 --affinity 2 -p 10   # "recheck", §6.2
# optional: the same instr/time with --impls rustique (Rustique's own types), §6.3
# pyperformance, per build: replicate A = plain then enabled, B = enabled then plain, each on one L3 domain
PYTHONPATH=<clone>/python:<dir with sitecustomize.py> RUSTIQUE_ENABLE=0|1 \
  pyperformance run --python <cpython> --inherit-environ PYTHONPATH,RUSTIQUE_ENABLE --affinity <domain> -o ...
pyperformance run ... -b asyncio_tcp,asyncio_tcp_ssl,asyncio_websockets     # again, alone (fixed ports), §7
pyperf compare_to plain.json enabled.json --table -G
# applications (twice), the lint app on CPython's Lib/test/test_importlib, and the apps at 2789558
taskset -c <domain> python benches/apps/run_apps.py bench -o apps.json
```

**Pinning.** Builds/tests used all cores. Benchmarks: callgrind (instruction counts) only in
domain 0; every wall-time job alone in its L3 domain, single-CPU shards on CPUs 10, 20, 30
(and 2), chosen because `/proc/interrupts` showed no device IRQs there (the ENA queues sit
on CPUs 5, 7, 9, 11, 16, 18, 21, 26, 28, NVMe on 15/31; of the first choice 1/9/17/25, CPU 9
carries the ENA management IRQ and 17 another active one). No `pyperf system tune` (KVM
guest: no cpufreq, MSRs not readable).

## 2. Machine facts (measured)

| | value | how |
|---|---|---|
| CPU | AMD EPYC 9R14 (Zen 4, family 25 model 17), 32 cores, 1 thread/core, 1 socket, 1 NUMA node | `lscpu` |
| L3 domains | 0–7, 8–15, 16–23, 24–31 (`shared_cpu_list` of `index3`) | sysfs |
| frequency control | none visible to the guest: no `cpufreq` directory, no `boost` file, no amd_pstate; cpuidle `acpi_idle` | sysfs |
| sustained clock, 1 core busy | 3.68 GHz (18.39 G cycles / 5.00 s task-clock) | `perf stat -e cycles,task-clock` on a busy loop |
| sustained clock, all 32 busy | 3.65 GHz (585.2 G cycles / 160.3 s) | same, system-wide |
| `/proc/cpuinfo` MHz | 2600 (nominal) on idle cores, 3.58–3.70 GHz on busy ones | i.e. boost is on and not controllable |
| HW counters | **available**: `perf stat -e instructions,cycles true` → 705 214 instructions, 1 628 820 cycles; `sum(range(10**7))` on system python3: 2.34 G instructions:u, 469 M cycles:u | `perf` 7.0.14 (via sudo; `perf_event_paranoid` = 4) |
| ASLR / THP | full randomization / `madvise` | procfs/sysfs |
| IRQs | irqbalance active; every CPU has some IRQ assigned; device IRQ activity only on 0, 3, 5, 7, 9, 11, 13–19, 21–26, 28, 31 | `/proc/interrupts`, `pyperf system show` |

## 3. Builds and tests

All builds succeeded: fat-LTO release build of `rustique-py` in 54–58 s per interpreter (4
in parallel, 8 cores each). Test counts from the JUnit XML (`tests/*.xml`):

| build | pytest (CI order: before `rustique_demo` exists) | pytest-full (after) | downstream: `examples/rust_extension` + `tests/test_capi.py` | examples/*.py |
|---|---|---|---|---|
| 3.15.1 | 3874 passed, 0 failed, 21 skipped (149.7 s) | 3879 passed, 0 failed, 16 skipped (147.8 s) | 80 passed | 5/5 ok |
| 3.15.1t | 3878 passed, 0 failed, 17 skipped (195.5 s) | 3883 passed, 0 failed, 12 skipped (189.6 s) | 80 passed | 5/5 ok |
| 3.14.9 | 3873 passed, 0 failed, 22 skipped (161.6 s) | 3878 passed, 0 failed, 17 skipped (157.6 s) | 80 passed | 5/5 ok |
| 3.14.9t | 3877 passed, 0 failed, 18 skipped (188.8 s) | 3882 passed, 0 failed, 13 skipped (181.4 s) | 80 passed | 5/5 ok |
| 3.16.0a1 | collection error (numpy, §5) | 3879 passed, 0 failed, 16 skipped (160.8 s; numpy built from source) | 80 passed | 4/5 (numpy_interop: no numpy wheel) |
| 3.16.0a1t | collection error (numpy, §5) | 3883 passed, 0 failed, 12 skipped (193.6 s; numpy built from source) | 80 passed | 4/5 (same) |

Skips in pytest-full are all by design: 12 × `test_takeover_conformance` (needs
`CPYTHON_SRC`; run separately in §4), and on GIL builds 4 tests that need the free-threaded
build (3.14.9 also skips `frozendict needs Python 3.15`). The 5 extra skips in the CI-order
run are the tests that need `rustique_demo` (4 in `test_capi.py`, which the downstream step
then runs, and `test_hardening_dictset.py:707`, which only pytest-full covers; CI never runs
that one).

**Rust** (once, `PYO3_PYTHON` = 3.15.1): `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace`
120 passed, 0 failed (rustique-core 96 + 4 `kernels_cpython`, rustique-py 17, rustique-api
2 + 1 doctest).

## 4. Conformance

**CPython container suites against the Rustique types** (`run_cpython_suites.py`, exit
status 1 on every build because no `--allow` file exists in the repo; every failure is a
documented deviation of DECISIONS.md §3):

| suite | 3.15.1 | 3.15.1t | 3.14.9 | 3.14.9t | published (3.15.1) |
|---|---|---|---|---|---|
| list | 67/69 | 67/69 | 66/68 | 66/68 | 67/69 |
| tuple | 37/38 | 37/38 | 37/38 | 37/38 | 37/38 |
| dict | 35/36 | 35/36 | 35/36 | 35/36 | 35/36 |
| set | 52/54 | 52/54 | 52/54 | 52/54 | 52/54 |
| frozenset | 31/33 | 31/33 | 31/33 | 31/33 | 31/33 |

Failures (identical on all builds): `test_repr` of list/tuple/dict (`'List([])' != '[]'`
etc.), set/frozenset `test_do_not_rehash_dict_keys` (`AssertionError: 20 != 10`) and
`test_pickling` (`AttributeError: 'rustique.Set' object has no attribute 'x' and no __dict__
for setting new attributes`); list also has 1 skip.

**Native-tier conformance** (`test_takeover_conformance.py`): the gate — identity rewrite
gives exactly the unrewritten outcomes — **passes on all four builds** (12/12 modules, 0
differing outcomes; 1288 tests on 3.15.1). Rustique mode (a measurement, not a gate):

| module | 3.15.1 = 3.15.1t | 3.14.9 = 3.14.9t | published (TAKEOVER.md §3.7) |
|---|---|---|---|
| test_list | 63/69 | 62/68 | 63/69 |
| test_tuple | 38/38 | 38/38 | 38/38 |
| test_dict | **117/144** | 99/122 | 118/144 |
| test_set | **631/644** | 617/630 | 625/644 |
| test_dictviews | **12/16** | 12/16 | 13/16 |
| test_grammar | 71/75 | 71/75 | 71/75 |
| test_scope | 39/41 | 39/41 | 39/41 |
| test_super | 38/39 | 38/39 | 38/39 |
| test_class | fails at import (0/1) | same | fails at import |
| test_named_expressions | 42/62 | 42/62 | 42/62 |
| test_copy | 81/84 | 78/81 | 81/84 |
| test_sort | 17/21 | 17/21 | 17/21 |

Three rows differ from TAKEOVER.md §3.7 (dict −1, set +6, dictviews −1). The run is
deterministic (3.15.1 and 3.15.1t agree exactly; `PYTHONHASHSEED=0`), so these are not noise:
§3.7 was written before later commits changed the Rustique types (*inferred*: the
section is not regenerated by any script, and the remaining set failures are all reprs,
`do_not_rehash_dict_keys` and unhashable-element messages). The current failures are listed
in `conformance/takeover-*.json`.

## 5. CPython 3.16.0a1 / 3.16.0a1t

Expected to fail (the brief: "PyO3 0.30.1 supports at most CPython 3.15"); **it does not**.

- `maturin develop --uv --release` **succeeds** on both (65–66 s, wheels
  `rustique-0.2.0-cp316-cp316-linux_x86_64.whl` / `cp316-cp316t`). Reason (read in
  `pyo3-ffi-0.30.1/build.rs`): the maximum is 3.15, but `ensure_python_version` accepts
  *max + 1* — `if interp_version == expected_next_version { warn!("Using experimental support
  for the CPython 3.16 ABI. Build artifacts may not be compatible with the final release of
  CPython, so do not distribute them.") }` — and errors only above that. Cargo does not print
  build-script warnings of registry crates, so the build log shows no warning at all.
- `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`: **no effect** (cargo reused the build, 0.02 s);
  that branch is only reached for versions above max + 1. In 0.30.1 the variable is
  also superseded by `PYO3_USE_STABLE_ABI_FORWARD_COMPATIBILITY` (the old name is still
  honoured for GIL builds; the free-threaded branch only checks the new one).
- `pip install rustique-0.2.0.tar.gz` (sdist from `maturin sdist`, CPython's own pip 26.2.2,
  isolated PEP 517 build): **succeeds** (maturin 1.16.0 from PyPI, Rust 1.99.1 via the
  shipped `rust-toolchain.toml`, 54 s), and the installed package imports and
  `enable()`s: `x.sort()`, `sum`, `max`, `rs.List` all work (`py316/pip-installed-smoke.txt`).
- Tests: plain `pytest -q` **stops at collection** on both builds because numpy 2.5.4 has no
  cp316 wheel, verbatim:

  ```
  ERROR collecting tests/test_takeover_compat.py
  tests/test_takeover_compat.py:27: in <module>
      _spec.loader.exec_module(tc)
  benches/takeover_compat.py:39: in <module>
      import numpy as np
  E   ModuleNotFoundError: No module named 'numpy'. Did you mean: 'runpy'?
  !!!!!!!!!!!!!!!!!!!! Interrupted: 1 error during collection !!!!!!!!!!!!!!!!!!!!
  ```

  With `--continue-on-collection-errors`: 3738 passed, 20 skipped, 1 error (3.16.0a1) and
  3742 passed, 16 skipped, 1 error (3.16.0a1t). numpy 2.5.4 **builds from source** on both
  (101–115 s); with it the whole suite passes exactly as on 3.15: 3879 passed / 16 skipped and
  3883 passed / 12 skipped. Downstream extension: 80 passed on both.
- So on 3.16.0a1 everything works today; the risk is that PyO3 calls it experimental and the
  CPython ABI/layouts may still change before 3.16.0 (the install-time layout checks of
  `enable()` are the safety net). A test-suite nit: `tests/test_takeover_compat.py` imports
  numpy unconditionally (through `benches/takeover_compat.py`), so a missing numpy aborts
  the whole pytest run instead of skipping like `test_buffer.py` does.

## 6. Scoreboard, default tier (`--impls enabled`)

Measured: callgrind instruction counts at 0–100 elements (`instr`, 3814 cells per build,
13.6 / 11.9 min on 8 shards) and pyperf medians, 3 processes × 3 values, at 10k/1M (`time`,
1846 cells, 64 min on 3 single-CPU shards, per build). Every cell's result was verified
against plain CPython's: **0 mismatches, 0 errors** on both builds. Files:
`scoreboard/{instr,time}-enabled-*.json*`, matrices `scoreboard/enabled-*.md` (same format
as `docs/scoreboard/`), `summary-*`, `analysis-*`, `compare-matrix-*`.

"Kernel path" = the cells whose code path `enable()` changes: `sorted/sum/min/max/any/all(x)`
on all five types, `in`/`index`/`count` on list and tuple, `list.sort()` (586 cells at 0–100,
328 at 10k/1M; this reproduces the published count of 328). "Unchanged" = all other cells.

| | 3.15.1 phase 0 | 3.15.1 published | 3.15.1t phase 0 | 3.15.1t published |
|---|---|---|---|---|
| 0–100: cells ≤ 1.05 | **3807/3814** as run; 3814/3814 with the 7 re-measured in isolation | 3810/3814 | **3814/3814** | 3814/3814 |
| 0–100: geometric mean | **0.931** | 0.93 | **0.935** | 0.93 |
| 0–100: kernel path / unchanged geomean | 0.627 / 1.000 | 0.627 / — (from the matrix) | 0.644 / 1.000 | 0.644 / — |
| 0–100: worst kernel-path cell | 1.043 `list/miss in x/object/1` (a fallback) | "1.028, `any(dict)` at size 0" (its matrix also shows 1.04 for this cell) | 1.047 (same cell) | — |
| 10k/1M: cells ≤ 1.05 | **1750/1846** | 1741/1846 | **1773/1846** | 1766/1846 |
| 10k/1M: geometric mean | **0.871** | 0.88 | **0.877** | 0.88 |
| 10k/1M kernel path (328): geomean | **0.460** (median 0.527) | 0.46 | **0.477** (median 0.547) | 0.48 |
| 10k/1M kernel path: cells > 1.05 | 5 | 5 | 3 | 0 |
| 10k/1M unchanged (1518): geomean / median | 1.000 / 1.000 | 1.004 / 1.000 | 1.000 / 1.000 | 0.999 / 1.000 |
| 10k/1M unchanged: > 1.05 / < 0.95 | 91 / 80 | 100 / 62 | 70 / 53 | 80 / 82 |

**Instruction counts reproduce.** Same code, same interpreter builds, same valgrind version:
3.15.1t agrees with the committed matrix in 3769/3814 cells to two decimals (all within
0.05); 3.15.1 in 3240 (3547 within 0.01, 19 differ by more than 0.05; ratio new/published
p05–p95 0.990–1.010, geomean 0.9999). The 3.15.1 cells that moved are all size 0–1 non-kernel
cells of a few hundred instructions (`repr([])`, `reversed(())`, `hash(frozenset)` of one
element), i.e. allocator/cache state left by the previous cell in the same callgrind worker
(10 cells per worker): the 7 cells above 1.05 this time (`frozenset/hash(x)/mixed/1` 1.096,
`list/repr(x)/float/0` 1.090, `tuple/reversed/str/0` 1.074, `frozenset/tuple(x)/mixed/1`,
`set/tuple(x)/object/1`, `set/g(*x)/int/100`, `frozenset/for v in x/float/0`) are different
from the published 4, and **re-measured alone they are 1.000, 1.000, 1.000, 1.000, 1.018,
1.017, 1.000** (`scoreboard/isolated-instr-3.15.1.txt`). Why 3.15.1 drifts and 3.15.1t does
not: *inferred* — the GIL build's pymalloc arenas are sensitive to the exact allocation
history (paths, environment and `sys.path` differ between the two boxes), the free-threaded
build uses mimalloc. Kernel-path geomeans are identical to 4 digits (0.6270 vs 0.6272).

**Wall time reproduces in aggregate, not cell by cell.** Geometric means and the
kernel-path/unchanged split agree with the published numbers to within 0.01; the published
"0.88" geomean at 10k/1M is 0.871/0.877 here (−1%/−0.3%). Cell by cell, the ratio
phase-0 ÷ published has median 1.000 and p05–p95 of 0.91–1.08 (3.15.1) and 0.92–1.09
(3.15.1t), for kernel-path and unchanged cells alike; 134 / 126 cells crossed the 1.05 bar
(66 up + 68 down / 59 + 67), and the cells above 1.05 are mostly *different* cells from the
published ones. That is the run-to-run spread of 3-process pyperf medians (different
processes, ASLR, allocation addresses), not a change: there is no shift (geomean of new ÷
published 0.995 / 0.999) and the cells above 1.05 carry their own noise (e.g. the worst,
`frozenset/x.isdisjoint(t)/str/10k` 1.60, has rel. std 11%/29%). The 1.05 count is therefore
a noise-dominated statistic at 3 processes; ±10–30 cells between runs is expected
(*inferred* from the spread above).

Kernel-path cells above 1.05: 3.15.1 `frozenset/all(x)/object/10k` 1.115,
`frozenset/all(x)/float/10k` 1.087, `list/x.sort()/object/10k` 1.073 (object elements: the
kernel declines and calls CPython), `dict/any(x)/float/1M` 1.061, `dict/any(x)/str/1M`
1.052 (`any(dict)` always uses CPython since `2922d13`); 3.15.1t `dict/any(x)/str/10k` 1.179
(enabled rel. std 16%), `frozenset/all(x)/int/10k` 1.077, `dict/any(x)/str/1M` 1.054. All are
fallback/thin-wrapper cells; re-measured with 10 processes in §6.2.

Headline cells at 1M (ratio enabled ÷ plain):

| cell | 3.15.1 phase 0 | published matrix | BENCHMARKS.md text | 3.15.1t phase 0 | published matrix | text |
|---|---|---|---|---|---|---|
| `max(floats)` | 0.13 | 0.14 | 0.13 | 0.18 | 0.19 | 0.21 |
| `min(ints)` | 0.16 | 0.17 | 0.17 | 0.19 | 0.20 | 0.21 |
| `ints.sort()` | **0.21** | 0.18 | 0.18 | **0.17** | 0.15 | 0.15 |
| `sorted(ints)` | 0.31 | 0.31 | 0.29 | 0.23 | 0.22 | 0.22 |
| `x in ints` (hit / miss) | 0.24 / 0.25 | 0.26 / 0.26 | 0.26 | 0.29 / 0.34 | 0.30 / 0.33 | 0.33 |
| `ints.count(v)` | 0.24 | 0.25 | 0.25 | 0.33 | 0.33 | 0.33 |
| `sum(ints)` | 0.36 | 0.36 | 0.37 | 0.44 | 0.47 | **0.54** |
| `min` over dict keys | 0.38 | 0.38 | **0.42** | 0.36 | 0.35 | 0.38 |

The BENCHMARKS.md prose quotes a few values that are not in its own committed matrices
(3.15.1 `min` over dict keys 0.42 vs 0.38, 3.15.1t `sum(ints)` 0.54 vs 0.47, `max(floats)`
0.21 vs 0.19): *inferred* to come from an earlier run than the matrices. Phase 0 agrees with
the matrices within 0.03 except `ints.sort()` at 1M (0.18 → 0.21, 0.15 → 0.17 on both
builds), and three cells that differ far beyond their noise on 3.15.1 only:

| cell (3.15.1) | published | phase 0 (rel. std builtin / enabled) | 3.15.1t published / phase 0 |
|---|---|---|---|
| `tuple/sorted(x)/int/10k` | 0.28 | **0.18** (0.1% / 0.3%) | 0.29 / 0.28 |
| `tuple/sorted(x)/float/10k` | 0.30 | **0.19** (0.2% / 0.2%) | 0.30 / 0.30 |
| `dict/sum(x)/int/10k` | 0.44 | **0.59** (11.5% / 0.4%) | 0.68 / 0.68 |
| `dict/sum(x)/int/1M` | 0.57 | **0.75** (0.4% / 3.8%) | 0.73 / 0.73 |

Phase 0 makes 3.15.1 look like 3.15.1t for `dict sum` and better than both for `sorted(tuple)`
at 10k; see §6.2 for the 10-process re-measurement.

### 6.2 Re-measurement of the notable cells with 10 processes

`run_phase0.sh recheck` re-ran 40 cells per build (every kernel-path cell above 1.05, the
worst unchanged cells, the cells that differ from the published matrix, the headline
cells) with `-p 10` instead of 3, alone on CPU 2. Full tables:
`scoreboard/recheck-table-3.15.1{,t}.md`. Findings (**measured**):

- **Every kernel-path cell is ≤ 1.05 at 10 processes**, on both builds:
  `frozenset/all(x)/object/10k` 1.115 → 0.996, `frozenset/all(x)/float/10k` 1.087 → 0.991,
  `list/x.sort()/object/10k` 1.073 → 1.003, `dict/any(x)/*/1M` 1.05–1.06 → 1.018–1.020
  (3.15.1); `dict/any(x)/str/10k` 1.179 → 1.021 (3.15.1t). The thin fallback layer costs
  1–2% on `any(dict)` (it always calls CPython), nothing measurable elsewhere.
- **The worst unchanged-code cells were noise**: `list/x.clear()/int|mixed/10k` 1.46/1.51 →
  1.03/0.99, `frozenset/x.isdisjoint(t)/str/10k` 1.60 → 0.99, `dict/tuple(x)/int/10k` 1.45
  → 1.00, `tuple/reversed/mixed/10k` 1.40 → 1.04. Four unchanged cells stay above 1.05 with
  5–23% rel. std (`tuple/reversed/mixed/1M` 1.07 and `frozenset/x.isdisjoint(t)/str/1M` 1.06
  on 3.15.1; `tuple/reversed/mixed/10k` 1.11 and `frozenset/x.isdisjoint(t)/str/1M` 1.23 on
  3.15.1t): code Rustique does not touch, whose own spread is larger than the bar.
- **Real differences from the published matrix (stable at 10 processes, low noise):**
  - `sorted(tuple)` at 10k on **3.15.1 only**: 0.178 (int) / 0.193 (float), published
    0.28/0.30. Absolute (3-process run): builtin 937 µs for the tuple and 934 µs for the list, enabled
    **168 µs** for the tuple vs **257 µs** for the list with the same elements (`list/sorted(x)/int/10k` 0.275, as
    published). On 3.15.1t tuple and list are both 0.28–0.30 (as published). So phase 0
    found `sorted()` of a 10k tuple ~35% cheaper than of a 10k list on the GIL build only;
    the published run did not. Cause **not investigated** (candidates, *inferred*: where the
    copied 80 KB pointer array lands with pymalloc vs mimalloc, or a difference in the
    kernel's tuple input path that only the GIL build hits). Worth a look in a later phase:
    it suggests `sorted(list)` leaves ~1/3 on the table on 3.15.1.
  - `list.sort()` of 1M ints: 0.211 (3.15.1) / 0.170 (3.15.1t), published 0.18 / 0.15
    (builtin 186 ms vs enabled 39 ms here). 15% slower kernel relative to CPython on this box
    on both builds; *inferred* host memory-bandwidth/cache differences between the two EC2
    instances (sort of 1M elements is memory-bound), not investigated.
  - `dict/sum(x)/int` on 3.15.1: 0.65 (10k) / 0.76 (1M) vs published 0.44/0.57 — but the
    *builtin* side has 19% rel. std across processes even at 10 processes (summing a dict's
    1M int keys is layout-sensitive), so this cell cannot be pinned down with this design;
    3.15.1t matches the published 0.68/0.73 with < 1% noise.
  - `tuple/x.count(hit)/float/10k`: 0.20 (3.15.1) / 0.14–0.17 (3.15.1t) vs published 0.14 /
    0.21 — the enabled side has 9–12% noise in every run; noise.

### 6.3 Rustique's own types (`--impls rustique`, optional)

Instruction counts at 0–100 elements reproduce the published table exactly in the geometric
means (cells ≤ 1.05 / geomean; published in parentheses):

| type | 3.15.1 | 3.15.1t |
|---|---|---|
| `List` | 231/890 (233) / 1.21 (1.21) | 181/890 (181) / 1.31 (1.31) |
| `Tuple` | 190/719 (192) / 1.32 (1.32) | 189/719 (189) / 1.40 (1.40) |
| `Dict` | 68/819 (67) / 1.70 (1.70) | 70/819 (70) / 1.75 (1.75) |
| `Set` | 82/743 (83) / 1.63 (1.63) | 76/743 (76) / 1.66 (1.66) |
| `FrozenSet` | 130/643 (133) / 1.49 (1.49) | 149/643 (149) / 1.48 (1.48) |

(3.15.1t: 3442/3762 cells identical to the committed matrix at two decimals, ratio p05–p95
1.000–1.000; 3.15.1: 2747 identical, p05–p95 0.983–1.018 — the same pymalloc-state drift as
in §6.) 

Wall time at 10k/1M (3 processes, 2 single-CPU shards per build, 112–113 min per build;
cells ≤ 1.05 / geomean, published in parentheses):

| type | 3.15.1 | 3.15.1t |
|---|---|---|
| `List` | 187/429 (189) / 0.76 (0.75) | 155/429 (159) / 0.83 (0.81) |
| `Tuple` | 151/346 (148) / 0.95 (0.95) | 136/346 (139) / 1.00 (0.99) |
| `Dict` | 126/405 (117) / 1.18 (1.18) | 77/405 (76) / 1.44 (1.42) |
| `Set` | 169/358 (164) / 0.89 (0.89) | 155/358 (152) / 0.98 (0.99) |
| `FrozenSet` | 146/308 (144) / 0.95 (0.95) | 159/308 (161) / 0.91 (0.92) |

Reproduced within 0.02 in every geomean. (Matrices: `scoreboard/rustique-*.md`.)

## 7. pyperformance 1.14.0, full default suite

Plain vs `rustique.enable()` injected by `benches/results/phase0/sitecustomize.py` on
`PYTHONPATH` (`--inherit-environ PYTHONPATH,RUSTIQUE_ENABLE`), default run mode (20
processes × 3 values), each run pinned to one whole L3 domain (`--affinity`). Verified while
running: the enabled workers had `_rustique.*.so` mapped and `RUSTIQUE_ENABLE=1`, the plain
ones neither (`/proc/<pid>/maps`). Two replicates per build, each a sequential pair on one
domain: **A** = plain then enabled, **B** = enabled then plain (3.15.1: A on domain 0, B on
domain 3; 3.15.1t: A on domain 1, B on domain 2). One pyperformance run takes 62–66 min.

| | 3.15.1 | 3.15.1t | published (3.15.1 / 3.15.1t) |
|---|---|---|---|
| results per run | 122 (B-plain 121: `asyncio_websockets` port clash) | 119–122 (port clashes, below) | 122 / 120 |
| geomean enabled ÷ plain, replicate A | **0.9999** | **0.9990** | 1.00× / 1.01× |
| geomean, replicate B | **1.0042** | **1.0056** | |
| geomean, A/B averaged per benchmark (122) | **1.002** | **1.002** | |
| replicate agreement (ratio B ÷ ratio A per benchmark) | median 1.003, p05–p95 0.991–1.023 | median 1.004, p05–p95 0.989–1.049 | |
| "significant" per `pyperf compare_to` (A / B) | 51 / 57 of 122 | 77 / 71 of 120 | 8 / 13 |
| slower by > 2% in **both** replicates | `python_startup` 1.17/1.18, `create_gc_cycles` 1.02/1.03 | `python_startup` 1.17/1.19, `create_gc_cycles` 1.03/1.05, `async_tree_eager_io_tg` 1.05/1.04, `async_tree_io_tg` 1.03/1.04, `bench_mp_pool` 1.03/1.03, `gc_traversal` 1.03/1.03, `base32_small` 1.02/1.03 | `python_startup` 1.17 / 1.18 (at `2922d13`), `create_gc_cycles` 1.03 / 1.04 |
| faster by > 2% in both | `meteor_contest` 0.93/0.94, `hexiom` 0.93/0.94 | `meteor_contest` 0.92/0.94, `unpickle` 0.97/0.97 | hexiom 1.09×, meteor_contest 1.08× / 1.07×, logging_silent 1.03× |
| `python_startup` plain → enabled | 10.4 → 12.2 ms (A), 10.5 → 12.3 ms (B) | 12.9 → 15.1 ms (A), 12.8 → 15.3 ms (B) | 10.4 → 12.1 ms / 12.7 → 14.9 ms |
| fails on plain CPython (and enabled alike) | `genshi` | `genshi` | `genshi`; 3.15.1t also `asyncio_tcp`, `asyncio_tcp_ssl` |

`genshi` fails on CPython 3.15 itself (verbatim, plain run): `TypeError:
ast.Expression.__init__ missing 1 required positional argument: 'body'` (genshi's AST
transformer calls `node.__class__()`, which 3.15 no longer allows).

**`asyncio_tcp` / `asyncio_tcp_ssl` / `asyncio_websockets` do not fail on 3.15.1t.** In the
replicates they died with `OSError: [Errno 98] error while attempting to bind on address
('127.0.0.1', 8882): [errno 98] address already in use` (websockets: port 8001): these
benchmarks bind fixed ports, and two pyperformance runs that reach them at the same moment
collide (the two 3.15.1t runs started together and ran in lockstep). Re-run alone
(`run_phase0.sh pyperformance-ports`, nothing else running), all three pass on both builds,
plain and enabled: ratios 3.15.1 0.997 / 1.000 / 1.001, 3.15.1t 1.000 / 1.005 / 0.999. The
published "crash on 3.15.1t" is very likely the same collision (*inferred*: the published
setup also ran concurrent jobs).

Reading of the numbers: the default tier is **neutral on pyperformance** (geomean 1.00 on
both builds; the replicate spread, ±0.4%, is larger than any systematic effect except
startup). The paired-sequential design makes pyperf's per-benchmark significance test
fire on drifts of 0.5–1% between the two halves (51–77 "significant" results, of which
only 2–9 agree in direction and exceed 2% in both replicates), so use the both-replicates
column, not the raw significance counts. The published runs had far fewer significant
results (8 / 13), *inferred* to be because plain and enabled were run differently
(concurrently, or closer in time); the method is not recorded.

Consistent real effects: startup +1.8 ms (3.15.1) / +2.3 ms (3.15.1t) per process = 1.17–1.19×
`python_startup` (identical to the published post-fix numbers); the collector traverses more
objects after the import (`create_gc_cycles` 1.02–1.05, `gc_traversal` 1.01–1.03; *inferred*
cause as in DECISIONS.md); on 3.15.1t also `bench_mp_pool` 1.03 (every pool worker process
pays the startup) and two `async_tree_*_tg` variants 1.03–1.05 (*not investigated*). Wins:
`meteor_contest` 6–8% (both builds), `hexiom` 6–7% (3.15.1 only; published 9%), `unpickle`
3% (3.15.1t only).

The full per-benchmark table, with the **absolute plain wall time** of every benchmark (the
workload picture for later phases), is in Appendix A; raw files `pyperformance/*.json`,
`compare-*.md` (`pyperf compare_to --table -G`), `table-*.md`.

## 8. Applications (`benches/apps/run_apps.py bench`)

`pyperf command`, 5 fresh processes × (1 warm-up + 1 value) per variant, each build alone in
one L3 domain (domain 0), run twice (a third run below). Mean ± std of the plain run (run 1);
ratio = enabled ÷ plain:

| app | 3.15.1 plain | ratio run 1 / run 2 | published | 3.15.1t plain | ratio run 1 / run 2 | published |
|---|---|---|---|---|---|---|
| graph (networkx 3.7) | 0.841 ± 0.003 s | 1.005 / 1.012 | 1.005 | 0.943 ± 0.002 s | 1.000 / 1.003 | 1.006 |
| lint (pyflakes 4.1.0, default path: repo `tests/` + `benches/`, 37 files) | 0.464 ± 0.003 s | 1.002 / 1.001 | 0.963 | 0.502 ± 0.003 s | 1.000 / 1.000 | 0.998 |
| etl (csv → dict/set → sort → json) | 0.848 ± 0.003 s | 0.994 / 0.988 | 0.987 | 0.991 ± 0.002 s | 1.002 / 0.995 | 0.993 |

graph and etl reproduce the published ratios within ±0.007. **lint does not**: 1.00 here vs
0.963 published on 3.15.1. Two differences from the published setup, either of which could
explain it: (1) BENCHMARKS.md describes the published lint workload as "pyflakes over ~90
files of CPython's tests", while `run_apps.py` defaults to this repository's `tests/` and
`benches/` (37 files) and the published command line is not recorded; (2) the published apps
ran at `2789558`, before `c48fdc0`/`2922d13` routed `all(set)`, `any(dict)` and large
non-numeric builtin sets back to CPython — pyflakes is set-heavy. Measured to separate them:
lint over CPython's `Lib/test/test_importlib` (89 files, `apps/lint-importlib-*.json`):
**1.012** (3.15.1, 0.210 → 0.212 s) and **1.016** (3.15.1t, 0.244 → 0.248 s), i.e. the 1.8–2.3 ms
startup on a 0.2 s process and nothing else, so (1) alone does not bring back 0.963.

**And (2) does not either.** The apps at `2789558` (built and run on this box next to a third
run at `5abd54c`, same domains, `apps/apps-*@2789558.json`, `apps-*-3.json`):

| app | 3.15.1 @2789558 | 3.15.1 @5abd54c (run 3) | 3.15.1t @2789558 | 3.15.1t @5abd54c (run 3) |
|---|---|---|---|---|
| graph | 1.006 | 1.007 | 0.995 | 0.999 |
| lint, default path | 1.006 | 1.002 | 0.996 | 1.000 |
| lint, `test_importlib` | 1.009 | 1.012 (earlier run) | 1.013 | 1.016 (earlier run) |
| etl | 0.979 | 0.986 | 1.020 (enabled rel. std 1.8%) | 1.000 |

So the published lint 0.963 is reproduced neither by the default path, nor by a ~90-file
CPython test directory, nor at the published commit; its exact workload is not recorded and
cannot be recovered from the repository (*inferred*: a different `--lint-path`). What is
measured: pyflakes gets no measurable benefit from `enable()` beyond paying the startup
(+0.2–1.6%), at either commit. etl is consistently 1–2% faster on 3.15.1 (0.979–0.994 over
four runs) and neutral on 3.15.1t; graph is neutral to +1%.

## 9. How long it took (measured, `durations.tsv`)

| step | wall time | notes |
|---|---|---|
| setup (apt, rustup + 1.99.1, uv, 6 CPythons, CPython `Lib/test` ×2, clone) | 0:55 | fast network; 6 CPythons in 2.3 s |
| machine facts | 0:11 | |
| fat-LTO release build, per interpreter | 0:54–0:58 (Rust compile 54 s) | 4 in parallel, 8 cores each |
| pytest, per build | 2:30 (3.15.1) – 3:17 (3.15.1t) | free-threaded builds ~25% slower |
| examples / downstream extension + `test_capi` | 0–1 s / 9–14 s | |
| pytest-full (second run) | 2:28–3:10 | |
| cargo fmt / clippy / test | 0:01 / 0:11 / 0:21 | |
| conformance (suites + takeover + gate), per build | ~0:30 | |
| 3.16: develop, pip sdist build, tests, numpy from source | ~1:05 + 0:57 + 2:40 + 1:41–1:55 per build | |
| pyperformance venv creation, per build | 0:42–0:45 | wheels cached by pip |
| scoreboard `instr` (enabled), 8 callgrind shards | 13:35 (3.15.1), 11:56 (3.15.1t) | |
| scoreboard `instr` (rustique), 8 shards | 10:43, 11:43 | optional |
| scoreboard `time` (enabled), 3 single-CPU shards | 64:04, 63:57 | the long pole of the scoreboard |
| recheck (40 cells, 10 processes) | 12:15, 12:21 | |
| pyperformance, one full run | 62:21–66:25 (8 runs) | + 2:23–2:40 per port-benchmark re-run |
| apps (`run_apps.py bench`) | 0:56–1:04 per build | |
| scoreboard `time` (rustique), 2 single-CPU shards | 111:46 (3.15.1), 113:01 (3.15.1t) | optional, run last |
| build at `2789558` + apps there | 1:00 + 1:04–1:12 per build | |
| reports (`report`, `pyperformance-compare`) | 2–3 s each | |

Box time used: SSH at 10:32Z, setup done 10:39, Wave A (builds, tests, conformance, 3.16)
done 11:06 while the first benchmarks were already running (callgrind in domain 0 only),
scoreboard done 12:58, pyperformance replicates done 15:10, port re-runs 15:20, optional
`--impls rustique` wall time 15:20–17:13, everything finished 17:17Z: **6 h 45 min** of box
time. Without the optional and extra parts (rustique types, recheck, second pyperformance
replicate, 2789558) the same schedule needs about **4 h 30 min** (*inferred* from the step
durations: setup + Wave A 0:35, scoreboard 2:10 with callgrind in parallel, one pyperformance
pair per build in parallel 2:10, port re-runs and apps 0:15). The long poles are the
scoreboard wall time (64 min per build on 3 shards; it scales with shard count, one shard
per L3 domain) and pyperformance (~64 min per run).

## 10. Incident: one invalid side run (discarded)

At 10:44:54–10:45:01Z `run_phase0.sh` was overwritten in place (scp) while a tmux session was
still executing it (stage `py316 3.16.0a1t`); bash resumed reading the new file at a stale
offset and executed the tail of `stage_all`: `pyperformance-compare 3.15.1t` (exit 127, no
inputs), `apps 3.15.1` + `apps 3.15.1t` concurrently on a box busy with builds and tests, and
the start of `pyperformance enabled 3.15.1t`, killed at ~10:49Z. Those outputs are kept in
`benches/results/phase0/invalid/` for the record and **not used anywhere**; all numbers in
this document come from later, properly scheduled runs. Script updates were deployed by
atomic rename from then on (a running bash keeps reading the old inode). No other step was
affected (the 3.16 stages it interrupted were rerun as `py316-test`/`py316-extra`).

## 11. Files

`benches/results/phase0/`:

- `run_phase0.sh` — the whole run, rerunnable (`./run_phase0.sh all`, or stage by stage);
  `sitecustomize.py` (the pyperformance injection); `analyze_phase0.py` (scoreboard
  split/headlines, matrix comparison, pyperformance ratios); `recheck_table.py`,
  `pyperformance_table.py` (the §6.2 and §7 tables).
- `steps.log`, `durations.tsv` — every step with start/end/seconds/exit code (the lines of
  the invalid side run of §10 are still in `steps.log` at 10:45–10:46Z).
- `machine/` — lscpu, cpufreq, perf stat, interrupts, pyperf system show, python -VV, …
- `logs/` — one log per step (pytest output incl. durations, build logs, pyperformance logs).
- `tests/` — JUnit XML per build and run, `freeze-*.txt` per venv.
- `conformance/` — `suites-*.json`, `takeover-*.json`, `takeover-gate-*.xml`.
- `py316/` — the 3.16 build/install logs and the smoke test of the pip-installed package.
- `scoreboard/` — raw `instr-*.json` / `time-*.json.shard*.json` (+ `.status.json`),
  `recheck-*.json*`, matrices `enabled-*.md` / `rustique-*.md` (same format as
  `docs/scoreboard/`), `summary-*.md`, `analysis-*.txt`, `compare-matrix-*.txt`,
  `isolated-instr-3.15.1.txt`, `recheck-table-*.md`.
- `pyperformance/` — `{plain,enabled}-<build>[-B|-ports].json`, `compare-*.md`,
  `analysis-*.txt`, `table-*.md`, benchmark-venv freezes.
- `apps/` — `apps-<build>[-2|-3].json`, `lint-importlib-*.json`, `apps-<build>@2789558.json`.
- `invalid/` — the discarded side run of §10.

Files over 512 KB are gzipped (`*.json.gz`, `*.log.gz`; 37 files): pyperf reads `.json.gz`
directly, the other scripts need `gunzip` first. The box keeps everything uncompressed under
`~/phase0/` (clones `w/<build>`, venvs, pyperformance venvs in `pyperf/<build>/`, results).

## Appendix A. pyperformance per benchmark: plain wall time and enabled ÷ plain

Plain wall time = mean over the plain runs (pyperf mean of 60 values each); ratios per
replicate (A = plain first, B = enabled first); "alone" = the port re-run. `genshi` fails on
CPython 3.15.

| benchmark | 3.15.1 plain | 3.15.1 enabled ÷ plain (A / B) | 3.15.1t plain | 3.15.1t enabled ÷ plain (A / B) |
|---|---:|---:|---:|---:|
| 2to3 | 203 ms | 1.010 / 1.008 | 220 ms | 1.016 / 1.020 |
| ascii85_large | 13.1 ms | 1.002 / 0.998 | 13.1 ms | 1.001 / 0.999 |
| ascii85_small | 490 µs | 1.001 / 1.002 | 496 µs | 0.998 / 1.004 |
| async_generators | 311 ms | 1.008 / 1.013 | 318 ms | 1.004 / 1.007 |
| async_tree_cpu_io_mixed | 468 ms | 0.997 / 1.001 | 548 ms | 0.989 / 0.981 |
| async_tree_cpu_io_mixed_tg | 494 ms | 0.999 / 0.997 | 520 ms | 1.006 / 0.995 |
| async_tree_eager | 74.3 ms | 0.985 / 0.989 | 86.8 ms | 1.001 / 1.000 |
| async_tree_eager_cpu_io_mixed | 325 ms | 0.996 / 0.997 | 353 ms | 0.998 / 1.002 |
| async_tree_eager_cpu_io_mixed_tg | 433 ms | 1.000 / 1.000 | 505 ms | 1.005 / 0.991 |
| async_tree_eager_io | 731 ms | 0.996 / 1.009 | 621 ms | 0.990 / 0.970 |
| async_tree_eager_io_tg | 776 ms | 0.998 / 1.004 | 596 ms | 1.049 / 1.036 |
| async_tree_eager_memoization | 174 ms | 0.994 / 0.995 | 207 ms | 1.001 / 0.996 |
| async_tree_eager_memoization_tg | 309 ms | 0.996 / 0.994 | 342 ms | 0.991 / 0.979 |
| async_tree_eager_tg | 230 ms | 1.001 / 1.005 | 250 ms | 1.002 / 0.993 |
| async_tree_io | 688 ms | 0.994 / 1.001 | 662 ms | 0.993 / 0.985 |
| async_tree_io_tg | 732 ms | 0.999 / 0.991 | 634 ms | 1.030 / 1.036 |
| async_tree_memoization | 341 ms | 1.000 / 1.006 | 399 ms | 0.974 / 0.989 |
| async_tree_memoization_tg | 338 ms | 0.999 / 1.003 | 370 ms | 0.997 / 1.010 |
| async_tree_none | 257 ms | 1.007 / 1.006 | 320 ms | 1.016 / 1.004 |
| async_tree_none_tg | 274 ms | 1.001 / 1.006 | 298 ms | 0.982 / 0.994 |
| asyncio_tcp | 257 ms | 0.990 / 0.990 | 261 ms | 1.000 (alone) |
| asyncio_tcp_ssl | 879 ms | 0.993 / 0.995 | 1.09 s | 1.005 (alone) |
| asyncio_websockets | 437 ms | 0.997; alone 1.001 | 405 ms | 0.997; alone 0.999 |
| base16_large | 7.94 ms | 0.997 / 0.999 | 5.18 ms | 1.002 / 1.001 |
| base16_small | 338 µs | 0.998 / 1.012 | 339 µs | 1.008 / 1.018 |
| base32_large | 2.56 ms | 0.999 / 1.000 | 2.56 ms | 1.000 / 1.000 |
| base32_small | 198 µs | 1.010 / 1.003 | 206 µs | 1.021 / 1.026 |
| base64_large | 2.02 ms | 1.000 / 1.000 | 2.02 ms | 1.001 / 1.001 |
| base64_small | 214 µs | 1.005 / 1.001 | 241 µs | 0.998 / 0.994 |
| base85_large | 5.35 ms | 0.998 / 1.003 | 5.38 ms | 0.999 / 0.998 |
| base85_small | 199 µs | 1.013 / 1.006 | 201 µs | 1.001 / 0.998 |
| bench_mp_pool | 7.9 ms | 1.000 / 1.003 | 5.74 ms | 1.034 / 1.031 |
| bench_thread_pool | 848 µs | 1.004 / 0.994 | 1.15 ms | 1.010 / 1.012 |
| bpe_tokeniser | 3.18 s | 0.997 / 0.999 | 3.27 s | 0.996 / 0.992 |
| chameleon | 10.3 ms | 0.987 / 0.987 | 12.1 ms | 0.988 / 1.008 |
| chaos | 40.3 ms | 1.010 / 1.014 | 45.9 ms | 0.994 / 0.985 |
| comprehensions | 10.8 µs | 0.997 / 1.000 | 12.7 µs | 0.997 / 1.003 |
| connected_components | 470 ms | 1.000 / 1.011 | 620 ms | 0.981 / 1.014 |
| coroutines | 17.8 ms | 1.000 / 1.013 | 17.4 ms | 0.997 / 1.005 |
| coverage | 52.5 ms | 1.000 / 1.005 | 61.5 ms | 0.989 / 1.000 |
| create_gc_cycles | 1.61 ms | 1.024 / 1.031 | 1.14 ms | 1.033 / 1.046 |
| crypto_pyaes | 54.6 ms | 1.002 / 1.012 | 62.9 ms | 0.992 / 0.999 |
| dask | 419 ms | 1.002 / 1.001 | 573 ms | 0.996 / 0.998 |
| deepcopy | 164 µs | 0.996 / 1.003 | 178 µs | 0.956 / 1.031 |
| deepcopy_memo | 20.6 µs | 1.006 / 1.007 | 23 µs | 0.996 / 0.997 |
| deepcopy_reduce | 1.86 µs | 0.998 / 1.008 | 2.02 µs | 0.976 / 1.028 |
| deltablue | 2.43 ms | 0.991 / 0.993 | 2.93 ms | 1.005 / 1.002 |
| django_template | 26.5 ms | 0.997 / 1.000 | 28 ms | 0.993 / 1.002 |
| docutils | 1.67 s | 1.005 / 1.003 | 2.06 s | 1.012 / 1.024 |
| dulwich_log | 30.2 ms | 0.994 / 1.003 | 31.1 ms | 0.993 / 0.998 |
| fannkuch | 271 ms | 1.000 / 1.007 | 295 ms | 0.987 / 0.991 |
| fastapi_http | 218 ms | 0.996 / 1.004 | 193 ms | 1.016 / 1.009 |
| float | 59.8 ms | 1.004 / 1.008 | 62.4 ms | 1.004 / 1.014 |
| gc_traversal | 3.88 ms | 1.012 / 1.015 | 1.96 ms | 1.026 / 1.034 |
| generators | 24.6 ms | 0.999 / 1.001 | 26.5 ms | 1.006 / 1.013 |
| go | 86.1 ms | 0.998 / 1.000 | 93 ms | 1.007 / 1.009 |
| hexiom | 4.39 ms | 0.931 / 0.940 | 4.87 ms | 1.007 / 1.015 |
| html5lib | 37.8 ms | 1.001 / 1.001 | 42 ms | 1.000 / 1.005 |
| json_dumps | 6.03 ms | 1.005 / 1.001 | 6.3 ms | 0.996 / 0.999 |
| json_loads | 17.6 µs | 1.005 / 1.003 | 18.5 µs | 0.973 / 0.984 |
| k_core | 2.45 s | 0.996 / 1.022 | 2.6 s | 0.975 / 1.031 |
| logging_format | 4.99 µs | 1.004 / 1.009 | 5.48 µs | 1.012 / 1.028 |
| logging_silent | 68.6 ns | 1.001 / 1.007 | 73.1 ns | 0.994 / 0.995 |
| logging_simple | 4.65 µs | 1.001 / 0.986 | 5.11 µs | 1.003 / 1.011 |
| mako | 10 ms | 1.003 / 1.007 | 10.2 ms | 0.996 / 1.007 |
| many_optionals | 396 µs | 1.004 / 1.003 | 441 µs | 1.014 / 1.009 |
| mdp | 951 ms | 0.997 / 1.000 | 1.05 s | 0.991 / 1.002 |
| meteor_contest | 76.4 ms | 0.926 / 0.936 | 85.8 ms | 0.923 / 0.941 |
| nbody | 73.7 ms | 0.989 / 1.012 | 93.6 ms | 0.977 / 1.024 |
| nqueens | 56.3 ms | 0.995 / 1.000 | 64.2 ms | 0.997 / 1.001 |
| pathlib | 17.3 ms | 0.999 / 1.000 | 17.2 ms | 1.005 / 1.004 |
| pickle | 7.61 µs | 1.014 / 1.029 | 7.7 µs | 0.993 / 1.002 |
| pickle_dict | 15.6 µs | 1.003 / 0.998 | 16.3 µs | 0.999 / 0.999 |
| pickle_list | 2.53 µs | 1.006 / 1.009 | 2.78 µs | 0.985 / 0.983 |
| pickle_pure_python | 223 µs | 1.005 / 1.010 | 253 µs | 0.994 / 0.999 |
| pidigits | 175 ms | 1.000 / 1.001 | 172 ms | 0.999 / 0.999 |
| pprint_pformat | 1.12 s | 1.009 / 0.991 | 1.21 s | 0.996 / 1.000 |
| pprint_safe_repr | 541 ms | 1.005 / 1.011 | 584 ms | 1.007 / 1.014 |
| pyflate | 319 ms | 0.980 / 0.985 | 354 ms | 1.014 / 1.028 |
| python_startup | 10.5 ms | 1.172 / 1.180 | 12.9 ms | 1.168 / 1.192 |
| python_startup_no_site | 7.3 ms | 1.002 / 1.009 | 9.11 ms | 0.987 / 1.003 |
| raytrace | 188 ms | 0.998 / 0.997 | 213 ms | 0.995 / 1.002 |
| regex_compile | 74.6 ms | 1.012 / 1.018 | 81.8 ms | 1.006 / 1.011 |
| regex_dna | 132 ms | 0.991 / 1.002 | 150 ms | 0.997 / 0.991 |
| regex_effbot | 2.13 ms | 1.002 / 1.013 | 2.47 ms | 0.997 / 0.991 |
| regex_v8 | 17.2 ms | 0.999 / 0.999 | 17.8 ms | 0.977 / 0.997 |
| richards | 32.7 ms | 0.993 / 1.006 | 36.4 ms | 0.997 / 1.018 |
| richards_super | 36.9 ms | 1.000 / 0.997 | 41.5 ms | 1.011 / 1.019 |
| scimark_fft | 193 ms | 1.000 / 1.005 | 231 ms | 1.006 / 1.010 |
| scimark_lu | 80.2 ms | 1.003 / 1.038 | 88 ms | 0.972 / 1.022 |
| scimark_monte_carlo | 44.8 ms | 0.996 / 0.998 | 52.2 ms | 0.998 / 1.009 |
| scimark_sor | 80.4 ms | 1.007 / 0.997 | 94.3 ms | 1.017 / 1.019 |
| scimark_sparse_mat_mult | 3.31 ms | 1.005 / 1.010 | 4.06 ms | 1.010 / 1.006 |
| shortest_path | 497 ms | 0.985 / 1.019 | 628 ms | 1.000 / 1.027 |
| spectral_norm | 71.5 ms | 1.000 / 1.002 | 82.5 ms | 0.989 / 1.003 |
| sphinx | 715 ms | 1.000 / 1.000 | 832 ms | 0.986 / 0.999 |
| sqlalchemy_declarative | 76.7 ms | 1.004 / 1.004 | 88.1 ms | 1.003 / 1.009 |
| sqlalchemy_imperative | 7.03 ms | 1.000 / 0.998 | 8.59 ms | 1.003 / 1.009 |
| sqlglot_v2_normalize | 73.3 ms | 0.998 / 0.995 | 80.3 ms | 1.006 / 1.012 |
| sqlglot_v2_optimize | 36.5 ms | 1.000 / 0.995 | 40.5 ms | 0.989 / 0.990 |
| sqlglot_v2_parse | 819 µs | 0.996 / 1.006 | 991 µs | 1.004 / 1.012 |
| sqlglot_v2_transpile | 1.03 ms | 0.997 / 1.000 | 1.23 ms | 0.993 / 0.995 |
| sqlite_synth | 1.53 µs | 0.997 / 1.010 | 1.3 µs | 1.000 / 1.001 |
| subparsers | 7.43 ms | 1.003 / 1.007 | 8.31 ms | 1.002 / 1.001 |
| sympy_expand | 272 ms | 0.997 / 0.994 | 303 ms | 0.993 / 0.996 |
| sympy_integrate | 13.1 ms | 0.998 / 0.997 | 14.5 ms | 0.990 / 1.001 |
| sympy_str | 158 ms | 0.987 / 0.996 | 176 ms | 0.993 / 0.997 |
| sympy_sum | 87.1 ms | 1.000 / 1.007 | 97.5 ms | 1.000 / 0.999 |
| telco | 4.68 ms | 1.000 / 1.009 | 5.36 ms | 1.014 / 1.027 |
| tomli_loads | 1.45 s | 1.004 / 0.988 | 1.48 s | 1.000 / 1.001 |
| tornado_http | 80.8 ms | 0.996 / 0.995 | 80 ms | 0.991 / 0.998 |
| typing_runtime_protocols | 111 µs | 1.012 / 0.996 | 128 µs | 0.996 / 0.988 |
| unpack_sequence | 34.7 ns | 0.985 / 1.087 | 35.5 ns | 0.964 / 1.029 |
| unpickle | 10.1 µs | 1.001 / 1.005 | 11.2 µs | 0.968 / 0.965 |
| unpickle_list | 2.85 µs | 0.984 / 0.993 | 3.41 µs | 0.987 / 1.015 |
| unpickle_pure_python | 157 µs | 1.015 / 1.010 | 173 µs | 0.993 / 0.990 |
| urlsafe_base64_small | 244 µs | 0.994 / 0.984 | 262 µs | 1.004 / 1.007 |
| xdsl_constant_fold | 28 ms | 0.966 / 1.010 | 31 ms | 1.002 / 1.010 |
| xml_etree_generate | 59.8 ms | 1.009 / 1.032 | 66 ms | 0.998 / 0.991 |
| xml_etree_iterparse | 73.8 ms | 1.006 / 1.002 | 71.8 ms | 0.988 / 0.992 |
| xml_etree_parse | 121 ms | 1.002 / 0.999 | 116 ms | 0.999 / 0.998 |
| xml_etree_process | 43.3 ms | 1.006 / 1.009 | 50.8 ms | 1.004 / 0.995 |
| genshi | fails | — | fails | — |
