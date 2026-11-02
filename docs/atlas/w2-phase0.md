# Wave 2, Phase 0 — ground truth on the benchmark box (`wave2` at `70c54fe`)

Rustique `wave2` at **`70c54fe`** (`main` `7f4c939`, the merged wave 1, plus a one-line
change of `run.py`'s family list), fat-LTO release builds. Measured on a dedicated AWS
c7a.8xlarge, the same type as wave 1's, on 2026-11-02, 18:30–23:00Z. Everything here is
**measured** unless tagged *inferred*. "Enabled" = all 15 groups on (`rustique.enable()`,
or `RUSTIQUE=1` through the shipped `.pth` where it says so); "plain" = the same venv and
interpreter without it. "Wave 1" = `docs/atlas/phase3.md` (`11bad29`). Raw data, logs and
the rerunnable script: `benches/results/w2-phase0/` (§11).

## Summary

**Real workloads (§4)** — wave 1 reproduces:

| | 3.15.1 | 3.15.1t | wave 1 (Phase 3) |
|---|---:|---:|---|
| 34 workloads, geometric mean (enabled ÷ plain) | **0.877** | **0.843** | 0.880 / 0.879 · 0.843 / 0.843 |
| data / devtools / web | 0.768 / 0.962 / 0.919 | 0.756 / 0.865 / 0.932 | 0.773 / 0.963 / 0.922 · 0.756 / 0.863 / 0.933 |
| best / worst | `report_render` 0.438 / `config_merge` 1.001 | `isort_stdlib` 0.203 / `pyflakes_stdlib` 1.010 | |
| 6 whole processes, `RUSTIQUE=1` | **0.754** | **0.713** | 0.756 · 0.712 |

- 0 digest mismatches. No workload is above 1.05.
- Every 3.15.1 workload is within 2% of wave 1, and 33 of 34 on 3.15.1t. The exception
  is `isort_stdlib`: its plain median got 3.4% faster.

**pyperformance 1.14.0** (§5), two replicate pairs per build, averaged per benchmark:

- **0.942** on 3.15.1 and **0.956** on 3.15.1t (wave 1: 0.942 / 0.957).
- Slower by > 2% in both pairs: only wave 1's three, `python_startup` 1.09–1.11, and
  `xdsl_constant_fold` 1.03 and `pyflate` 1.03 on 3.15.1t.

**Startup (§6):**

- installed but off: ≤ 1.05 everywhere (worst 1.047 Ir / 1.049 wall, 3.14.9 bare);
- `RUSTIQUE=1` marginal: 1.045 / 1.077 on 3.15.1 and 1.063 / 1.107 on 3.14.9, unchanged;
- `import rustique; enable()`: 0.16–0.18M Ir cheaper than in wave 1.

**Scoreboard (§7):**

- 0–100 elements (callgrind): **3814 / 3814 cells ≤ 1.05** after isolation on both
  builds. Wave 1's three count/`in` exceedances now measure 1.026–1.045.
- 10k/1M wall time on 3.15.1: geomean 0.828, kernel path 0.461 (wave 1: 0.830 / 0.462).
  Of 73 cells above 1.05 as run, 63 are ≤ 1.05 alone, among them both kernel-path cells.
  **10 non-kernel cells stay 1.06–1.22 alone** (tuple iteration, pickle, set equality,
  repr), most of them above 1.05 in wave 1 too. Flagged.

**Tests (§2–3):**

- `pytest -q` passes with 0 failures on 3.15.1, 3.15.1t, 3.14.9, 3.14.9t, **3.15.2,
  3.15.2t, 3.14.10, 3.14.10t** (5831 tests). Rust fmt and clippy are clean, and
  `cargo test` passes 173 tests.
- Conformance with all groups on: **0 differences** on 3.15.1, 3.15.1t, 3.14.9, 3.15.2 and
  3.14.10 (7,823–8,129 outcomes each).
- **The patch releases trip no fingerprint.** 3.15.2(t): 84 / 84 installed.
  3.14.10(t): 79 / 84, with the five pathlib property targets refused by design, as on
  3.14.9.
- **3.16.0a1/a1t** builds (numpy from source), but every target is `refused`
  ("pre-release"), and the suite then fails: 278 failures, 1504 errors, 4025 passed. All
  of them assert `installed`. **Flagged**: the suite should skip there.

**Wave-2 baselines (§8):**

- **Free-threading penalty** (plain 3.15.1t ÷ 3.15.1):
  - workloads: geomean 1.127; outliers `isort_stdlib` 4.03 and `state_snapshot` 1.90;
    faster on 3.15.1t: `pylint_stdlib` 0.76, `billing_engine` 0.80;
  - pyperformance: geomean 1.074; outliers `bench_thread_pool` 1.37, `dask` 1.36,
    `connected_components` 1.30.
- **GC:** collections take **7.0%** of steady-state run time on 3.15.1 (5.5% on 3.15.1t).
  `billing_engine` spends 33.8% in 375 collections that free nothing; `pytest_networkx`
  32.4%; `pylint_stdlib` and `pyflakes_stdlib` ~19%.
- **CPython's JIT** is built (off by default) on every GIL build and absent on
  free-threaded builds. `PYTHON_JIT=1` on plain 3.15.1: geomean **1.015**, i.e. slower
  (data 0.988, devtools 1.007, web 1.058, `fastapi_pydantic` 1.109).

**Not done:**

- the 3.15.1t scoreboard wall time: dropped when the lead needed the box back by
  23:30Z (§10);
- the replicate-B pyperformance pairs were not dropped, because both had already started.


## 1. What was run

**Commit.** Rustique branch `wave2` at **`70c54fe`** ("workloads: wave-2 families in run.py"):
`main` `7f4c939` (the merged wave 1, including the fixes made after wave 1's Phase 3 at
`11bad29`, `docs/atlas/phase3.md` §11) plus a one-line change of
`benches/workloads/run.py`'s family list. The Rust and Python code is identical to `7f4c939`.
The box built exactly this commit from a git bundle of `wave2`
(`logs/setup.log`: `70c54fed1350e2047cd2124b43413e67b81b3004`).

**Box.** A new AWS c7a.8xlarge, the same type as wave 1's Phase 0/3 (`machine/`): AMD EPYC
9R14, 32 cores without SMT, 4 L3 domains (0–7, 8–15, 16–23, 24–31; `machine/l3.txt`),
61 GiB, Ubuntu 24.04, kernel `7.0.0-1014-aws` (the same as wave 1), no frequency control
visible to the guest. Sustained clock with one core busy: 18.40 G cycles in 5.00 s =
**3.68 GHz** (`machine/perf-stat-busy.txt`; wave 1: 3.68 GHz). THP `madvise`, full ASLR,
irqbalance active. Hardware counters available (`perf_event_paranoid` −1). Nobody else
used the box. For the run, `apt-daily`, `apt-daily-upgrade`, `motd-news` timers and
`unattended-upgrades` were stopped (`logs/quiet.log`).

Timing CPUs: wave 1 used CPUs 2, 10, 20, 30 (one per L3 domain) for single-CPU timing; at
setup, CPUs 2, 10 and 30 had no device IRQ, while every CPU of domain 2 (16–23) had one
(CPU 20: one ENA queue; `machine/interrupts.txt`). 2/10/20/30 were kept for comparability.

**Toolchain** (`logs/setup.log`, `machine/pythons.txt`, `tests/freeze-*.txt`,
`workloads/freeze-*.txt`): Rust 1.99.1 (LLVM 23.1.1), clippy 0.1.99, rustfmt 1.10.0,
uv 0.13.6 (wave 1: 0.13.5), valgrind 3.22.0, perf 7.0.14, hyperfine 1.18.0; PyO3 0.30.1,
maturin 1.15.0, pytest 9.1.1, hypothesis 6.170.9, numpy 2.5.4, pyperf 2.10.0, pyperformance
1.14.0. CPython: python-build-standalone, `x86_64` baseline builds —

| build | `python -VV` | release | as wave 1 |
|---|---|---|---|
| 3.15.1, 3.15.1t | Oct 21 2026, 17:48:50 / 17:49:09, Clang 23.1.3 | 20261021 | identical |
| 3.14.9, 3.14.9t | Oct 21 2026, 17:48:02 / 17:49:33 | 20261021 | identical |
| 3.15.2, 3.15.2t | Oct 26 2026, 14:06:56 / 14:06:54 | 20261026 | new |
| 3.14.10, 3.14.10t | Oct 26 2026, 14:09:22 / 14:06:35 | 20261026 | new |
| 3.16.0a1, 3.16.0a1t | Oct 26 2026, 14:10:58 / 14:09:27 | 20261026 | newer build than wave 1's (Oct 21) |

CPython `Lib/test` from tags `v3.15.1` (`c37e74c`), `v3.14.9` (`ead82c0`), `v3.15.2`
(`5e58eaa`), `v3.14.10` (`b703b2c`).

**CPython's JIT** (`machine/jit.txt`): every GIL build is configured
`--enable-experimental-jit=yes-off` (`sys._jit.is_available()` True, off unless
`PYTHON_JIT=1`); no free-threaded build has the JIT.

**Commands** — `benches/results/w2-phase0/run_w2phase0.sh` (stage names in brackets, as
wave 1's `run_phase3.sh`):

```bash
# [build/wheel/status/test] per build b (10 builds)
maturin develop --uv --release; maturin build --release -o dist/$b
python status_dump.py status/status-$b.json        # enable() with the target modules imported
python -m pytest -q -p no:cacheprovider --durations=25 --junitxml=...
# [rust] once (PYO3_PYTHON = 3.15.1)
cargo fmt --all --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace
# [conformance] 3.15.1, 3.15.1t, 3.14.9, 3.15.2, 3.14.10: all 15 groups at once, 27 CPython test modules
CPYTHON_SRC=... python tests/conformance/run_target_suites.py --targets <all groups> \
    --modules test.test_list,...,test.test_statistics --expect-installed <all 84 targets>
# [wlvenv] one venv per build and family; requirements + pyperf 2.10.0 + the release wheel
# [workloads] per family, pinned to one L3 domain
taskset -c <domain> python benches/workloads/run.py compare <family> -n 5 -p 5 -o ...
# [wlstats-run] counters of one steady-state run() per workload (phase3/wlstats_run.py)
# [whole] 6 workloads, one setup()+run() per process (phase3/whole.py), plain vs RUSTIQUE=1, 4 rounds
python -m pyperf command --affinity <domain> -p 5 -n 1 -w 1 --inherit-environ RUSTIQUE,WHOLE_DIGEST --append ...
# [pyperformance] PYTHONPATH=<clone>/python:<dir with phase0/sitecustomize.py> RUSTIQUE_ENABLE=0|1
pyperformance run --python <cpython> --inherit-environ PYTHONPATH,RUSTIQUE_ENABLE --affinity <domain> -o ...
# [startup] release wheel pip-installed into a fresh `python -m venv` vs an identical venv without it
taskset -c 30 python benches/dx/startup.py cells PLAIN_VENV RUSTIQUE_VENV -r 300 --ir-runs 3
# [instr/time/report] scoreboard, enabled tier; [isolate-instr/isolate-time] sb_isolate.py
python benches/scoreboard.py instr --impls enabled --shards 8 ...; ... time --size 1e4,1e6 --shards 4 --affinity 2,10,20,30
# [gcstats] plain, one process per workload: gcstats.py; [jit] jit_compare.py compare <family> -n 5 -p 3
```

**New in this run** (scripts in `benches/results/w2-phase0/`, deployed to the box's
`$BASE/bin`):

- `status_dump.py` — per build: imports the stdlib modules the 15 groups patch
  (`json`, `copy`, `dataclasses`, `datetime`, `_strptime`, `time`, `posixpath`, `fnmatch`,
  `glob`, `pathlib`, `logging`, `random`, `textwrap`, `urllib.parse`, `statistics`), calls
  `rustique.enable()`, imports the module of any target still `pending-import`, and records
  `rustique.status()` (`status/status-<build>.json`).
- `gcstats.py` — GC baseline: as `run.py one` (plain): `setup()`, one warm-up `run()`, then
  one `run()` with a `gc.callbacks` hook that timestamps each collection's start and stop
  (`perf_counter_ns`); collections, time, objects collected and `candidates` per generation,
  `gc.get_stats()` deltas as a cross-check, `ru_maxrss` (`gc/gc-<build>.jsonl`).
- `jit_compare.py` — `run.py compare`'s protocol with the sides changed: both plain, one
  with `PYTHON_JIT` removed from the environment, one with `PYTHON_JIT=1`; every child
  reports `sys._jit.is_enabled()` (`jit/jit-<build>-<family>.json`).
- `sb_isolate.py` — Phase 3's isolation method for every scoreboard cell above 1.05: instr
  cells each in their own callgrind worker (3 runs), 10k/1M cells each alone on one CPU at
  10 processes × 5 values (2 runs). The scoreboard selects ops by substring, so a cell is
  selected with exact type/storage/size and the longest comma-free piece of its op name;
  the output says when that filter also selected a sibling cell.
- `analyze_w2phase0.py`, `tables.sh` — the tables of this document (with wave 1's
  `phase3/analyze_phase3.py`), generated on the box after the schedule (`tables/`).


## 2. Builds, tests and target status

Fat-LTO release builds (`maturin develop --uv --release`, then `maturin build --release` in
the same target directory: the wheel holds the binary the tests ran) succeed on all ten
interpreters: 72–75 s each, four in parallel with 8 cores each; 146–147 s on 3.16.0a1/a1t,
where `uv` first builds numpy 2.5.4 from source (no cp316 wheel, as in wave 1's Phase 0;
`logs/build-3.16.0a1*.log`). PyO3 0.30.1 still builds 3.16 as "experimental" without a
visible warning (wave 1 Phase 0 §5).

`pytest -q -p no:cacheprovider` (JUnit XML in `tests/`) and the target status after
`rustique.enable()` with the target modules imported (`status/status-<build>.json`):

| build | build s | tests | passed | failed | errors | skipped | pytest s | targets installed | refused |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 3.15.1 | 75 | 5831 | 5807 | 0 | 0 | 24 | 228.2 | **84 / 84** | 0 |
| 3.15.1t | 74 | 5831 | 5810 | 0 | 0 | 21 | 307.9 | **84 / 84** | 0 |
| 3.14.9 | 75 | 5831 | 5808 | 0 | 0 | 23 | 238.5 | 79 / 84 | 5 (by design, below) |
| 3.14.9t | 74 | 5831 | 5811 | 0 | 0 | 20 | 280.0 | 79 / 84 | 5 (by design) |
| **3.15.2** | 73 | 5831 | 5807 | 0 | 0 | 24 | 223.9 | **84 / 84** | 0 |
| **3.15.2t** | 72 | 5831 | 5810 | 0 | 0 | 21 | 307.6 | **84 / 84** | 0 |
| **3.14.10** | 73 | 5831 | 5808 | 0 | 0 | 23 | 237.8 | 79 / 84 | 5 (by design) |
| **3.14.10t** (extra) | 72 | 5831 | 5811 | 0 | 0 | 20 | 288.0 | 79 / 84 | 5 (by design) |
| **3.16.0a1** | 147 | 5831 | 4025 | **278** | **1504** | 24 | 181.0 | **0 / 84** | 84: `version` — "CPython 3.16 pre-release (0x31000a1)" |
| **3.16.0a1t** | 146 | 5831 | 4028 | **278** | **1504** | 21 | 222.1 | **0 / 84** | 84: same |

- **The patch releases trip no fingerprint.** On 3.15.2 and 3.15.2t all 84 targets
  install, as on 3.15.1. On 3.14.10 and 3.14.10t the same 79 install as on 3.14.9. The
  five refusals on every 3.14 build are `pathlib.PurePath.{parent, name, suffix, stem,
  parts}`, category `version`, "CPython 3.14 not validated for this target". That is by
  design (`docs/DECISIONS.md` §7): the 3.14 specializer inlines property getters without
  checking the function version.
  - So every per-minor fingerprint (code-object hash of each Python original, `ml_*` +
    self-test of each C original) matches on the October patch releases. Their stdlib
    changes do not touch a patched function (*inferred* from the fingerprints matching).
  - 0 targets were left `pending-import` on any build.
- **3.16.0a1 refuses every target, by design.** The version gate requires a final
  release (`docs/ACCELERATION.md` §2, rule 2). The test suite does not expect that: 278
  tests fail and 1504 error at setup on both 3.16 builds. **Every one of them** asserts
  that a target is `installed` (or that it hit), and gets `refused`.
  - Grouped by message (`tests/pytest-3.16.0a1.xml`): `json.dumps` refused 781 setup
    errors, the container takeover fixtures 237 + 44 + 14, `copy.deepcopy` 191,
    `json.loads` 132, `random.randint` 57, `float.repr` 51, the paths groups 62, …;
    1678 of them in `tests/targets/`.
  - The other 4025 / 4028 tests pass. No crash, and no result differs.
  - Minimal reproduction (3.16.0a1): `import rustique; rustique.enable()` gives every
    target either `refused` with reason "CPython 3.16 pre-release (0x31000a1)",
    category `version`, or `pending-import` until its module is imported, then `refused`.
  - Wave 1's Phase 0 ran this suite green on 3.16.0a1. It measured `5abd54c`, which
    predates the acceleration framework's version gate (*inferred* from the history).
  - **Flagged** as a test-suite gap: on an interpreter where the targets are refused by
    version, the suite fails instead of skipping.
- Skips on the supported builds are wave 1's (`docs/atlas/phase3.md` §2): 12 ×
  `test_takeover_conformance` (needs `CPYTHON_SRC`), 5 that need `rustique_demo`, 1 that
  needs `pytest-xdist`, the free-threaded-only tests on GIL builds, and 3.14-only skips.
  There are 9 more tests than at `11bad29` (5831 vs 5822), from the fixes after Phase 3.
- Wave 1's Phase 3 found one `json.loads` segfault in 9 runs (`docs/atlas/phase3.md` §2,
  fixed in `dac4ab9`). The suite ran once per build here: **0 crashes in 10 runs**, and
  in the conformance, workload and pyperformance processes.

**Rust** (once, `PYO3_PYTHON` = 3.15.1): `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets -- -D warnings` clean (13 s); `cargo test
--workspace` **173 passed**, 0 failed (rustique-core 148 + `float_repr_cpython` 1 +
`kernels_cpython` 4, rustique-py 17, rustique-api 2 + 1 doctest; 4 doctests ignored) —
the same as wave 1's Phase 3.

## 3. Conformance, all 15 groups at once

`run_target_suites.py` with all 15 groups enabled together, the 27 CPython test modules of
wave 1, each module plain and enabled in fresh processes, outcomes compared per test id
(`conformance/all-groups-<build>.json`). Run on 3.15.1, 3.15.1t, 3.14.9 (the brief), and
3.15.2 and 3.14.10 (the patch releases):

| module | 3.15.1 | 3.15.1t | 3.14.9 | 3.15.2 | 3.14.10 |
|---|---|---|---|---|---|
| test_builtin | 150 pass, 7 skip; **0 diff** | 150 pass, 7 skip; **0 diff** | 141 pass, 6 skip; **0 diff** | 150 pass, 7 skip; **0 diff** | 141 pass, 6 skip; **0 diff** |
| test_copy | 84 pass; **0 diff** | 84 pass; **0 diff** | 81 pass; **0 diff** | 84 pass; **0 diff** | 81 pass; **0 diff** |
| test_copyreg | 6 pass; **0 diff** | 6 pass; **0 diff** | 6 pass; **0 diff** | 6 pass; **0 diff** | 6 pass; **0 diff** |
| test_dataclasses | 284 pass, 1 skip; **0 diff** | 284 pass, 1 skip; **0 diff** | 279 pass, 1 skip; **0 diff** | 284 pass, 1 skip; **0 diff** | 279 pass, 1 skip; **0 diff** |
| test_datetime | 3614 pass, 54 skip; **0 diff** | 3614 pass, 54 skip; **0 diff** | 3560 pass, 54 skip; **0 diff** | 3614 pass, 54 skip; **0 diff** | 3560 pass, 54 skip; **0 diff** |
| test_float | 52 pass, 2 skip; **0 diff** | 52 pass, 2 skip; **0 diff** | 52 pass, 2 skip; **0 diff** | 52 pass, 2 skip; **0 diff** | 52 pass, 2 skip; **0 diff** |
| test_fnmatch | 24 pass; **0 diff** | 24 pass; **0 diff** | 24 pass; **0 diff** | 24 pass; **0 diff** | 24 pass; **0 diff** |
| test_format | 17 pass, 2 skip; **0 diff** | 17 pass, 2 skip; **0 diff** | 17 pass, 2 skip; **0 diff** | 17 pass, 2 skip; **0 diff** | 17 pass, 2 skip; **0 diff** |
| test_fstring | 95 pass; **0 diff** | 95 pass; **0 diff** | 94 pass; **0 diff** | 96 pass; **0 diff** | 95 pass; **0 diff** |
| test_genericpath | 26 pass; **0 diff** | 26 pass; **0 diff** | 25 pass; **0 diff** | 26 pass; **0 diff** | 25 pass; **0 diff** |
| test_glob | 20 pass, 2 skip; **0 diff** | 20 pass, 2 skip; **0 diff** | 22 pass, 2 skip; **0 diff** | 20 pass, 2 skip; **0 diff** | 22 pass, 2 skip; **0 diff** |
| test_json | 232 pass, 1 skip; **0 diff** | 232 pass, 1 skip; **0 diff** | 220 pass, 1 skip; **0 diff** | 232 pass, 1 skip; **0 diff** | 220 pass, 1 skip; **0 diff** |
| test_list | 68 pass, 1 skip; **0 diff** | 68 pass, 1 skip; **0 diff** | 67 pass, 1 skip; **0 diff** | 68 pass, 1 skip; **0 diff** | 67 pass, 1 skip; **0 diff** |
| test_logging | 283 pass, 4 skip; **0 diff** | 283 pass, 4 skip; **0 diff** | 274 pass, 3 skip; **0 diff** | 285 pass, 4 skip; **0 diff** | 276 pass, 3 skip; **0 diff** |
| test_os | 472 pass, 77 skip, 3 fail, 1 error; **0 diff** | 472 pass, 77 skip, 3 fail, 1 error; **0 diff** | 317 pass, 66 skip, 2 fail; **0 diff** | 472 pass, 77 skip, 3 fail, 1 error; **0 diff** | 317 pass, 66 skip, 2 fail; **0 diff** |
| test_pathlib | 991 pass, 407 skip; **0 diff** | 991 pass, 407 skip; **0 diff** | 966 pass, 403 skip; **0 diff** | 991 pass, 407 skip; **0 diff** | 966 pass, 403 skip; **0 diff** |
| test_posixpath | 93 pass, 3 skip; **0 diff** | 93 pass, 3 skip; **0 diff** | 91 pass, 3 skip; **0 diff** | 93 pass, 3 skip; **0 diff** | 91 pass, 3 skip; **0 diff** |
| test_random | 114 pass, 2 skip; **0 diff** | 114 pass, 2 skip; **0 diff** | 114 pass, 2 skip; **0 diff** | 114 pass, 2 skip; **0 diff** | 114 pass, 2 skip; **0 diff** |
| test_sort | 21 pass; **0 diff** | 21 pass; **0 diff** | 21 pass; **0 diff** | 21 pass; **0 diff** | 21 pass; **0 diff** |
| test_statistics | 400 pass; **0 diff** | 400 pass; **0 diff** | 400 pass; **0 diff** | 400 pass; **0 diff** | 400 pass; **0 diff** |
| test_strptime | 94 skip, 49 pass; **0 diff** | 94 skip, 49 pass; **0 diff** | 94 skip, 46 pass; **0 diff** | 94 skip, 49 pass; **0 diff** | 94 skip, 46 pass; **0 diff** |
| test_strtod | 8 pass, 1 skip; **0 diff** | 8 pass, 1 skip; **0 diff** | 8 pass, 1 skip; **0 diff** | 8 pass, 1 skip; **0 diff** | 8 pass, 1 skip; **0 diff** |
| test_textwrap | 68 pass; **0 diff** | 68 pass; **0 diff** | 68 pass; **0 diff** | 68 pass; **0 diff** | 68 pass; **0 diff** |
| test_time | 49 pass, 16 skip; **0 diff** | 49 pass, 16 skip; **0 diff** | 48 pass, 16 skip; **0 diff** | 49 pass, 16 skip; **0 diff** | 48 pass, 16 skip; **0 diff** |
| test_tuple | 38 pass; **0 diff** | 38 pass; **0 diff** | 38 pass; **0 diff** | 39 pass; **0 diff** | 39 pass; **0 diff** |
| test_urllib | 104 pass, 2 skip; **0 diff** | 104 pass, 2 skip; **0 diff** | 104 pass, 2 skip; **0 diff** | 104 pass, 2 skip; **0 diff** | 104 pass, 2 skip; **0 diff** |
| test_urlparse | 78 pass, 5 skip; **0 diff** | 78 pass, 5 skip; **0 diff** | 74 pass, 5 skip; **0 diff** | 78 pass, 5 skip; **0 diff** | 74 pass, 5 skip; **0 diff** |

**0 differences on all five builds**: 8125 test outcomes on 3.15.1 and 3.15.1t, 7823 on
3.14.9, 8129 on 3.15.2, 7827 on 3.14.10.

- Every count on 3.15.1, 3.15.1t and 3.14.9 equals wave 1's Phase 3 table.
- The patch releases add 4 test outcomes each (`test_fstring`, `test_logging`,
  `test_tuple`). All pass alike plain and enabled.
- The `test_os` failures and error are the box's environment. They are identical plain and
  enabled, as in wave 1.
- 3.14.9 and 3.14.10 exit 1 only because of `--expect-installed`. The runner reports the
  five 3.15-only pathlib targets as not installed, which is by design (wave 1 §3 explains
  why it shows them as `pending-import`).

## 4. The workload set (the headline)

`benches/workloads/run.py compare <family> -n 5 -p 5` in one venv per family. Per workload
there are 5 interleaved pairs of fresh processes (plain, enabled). Each process runs
`setup()`, 1 warm-up and 5 timed `run()`s, and each side is the median of its 25 timed
runs. `enable()` runs before the workload module is imported. 3.15.1 ran on domain 1
(8–15) and 3.15.1t on domain 2 (16–23), concurrently, 18:55–19:19Z, with nothing else
timing in those domains. The family venvs are package-for-package identical to wave 1's
(`workloads/freeze-*.txt`; only the wheel's path differs).

**Every digest matched: 0 mismatches in 2 × 34 workloads.**

Per workload (ms = median steady-state `run()`; ratio = enabled ÷ plain; wave 1 = Phase 3
at `11bad29`, replicate 1 / 2; Δ = this ratio ÷ the geometric mean of wave 1's two):

| workload | 3.15.1 plain ms | 3.15.1 enabled ms | 3.15.1 ratio | 3.15.1 wave 1 (rep 1 / 2) | 3.15.1 Δ vs wave 1 | 3.15.1t plain ms | 3.15.1t enabled ms | 3.15.1t ratio | 3.15.1t wave 1 (rep 1 / 2) | 3.15.1t Δ vs wave 1 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| data/backup_archive | 598.5 | 596.8 | 0.997 | 0.996 / 0.996 | +0.1% | 605.5 | 605.4 | 1.000 | 0.999 / 0.999 | +0.1% |
| data/billing_engine | 932.3 | 900.3 | 0.966 | 0.970 / 0.972 | -0.5% | 748.9 | 730.5 | 0.975 | 0.975 / 0.971 | +0.3% |
| data/config_merge | 540.6 | 541.3 | 1.001 | 1.013 / 1.003 | -0.6% | 611.7 | 605.9 | 0.990 | 1.001 / 0.993 | -0.6% |
| data/csv_etl | 393.0 | 242.9 | 0.618 | 0.625 / 0.620 | -0.7% | 428.8 | 276.4 | 0.644 | 0.640 / 0.645 | +0.3% |
| data/fs_inventory | 316.0 | 233.4 | 0.739 | 0.743 / 0.742 | -0.5% | 321.1 | 236.2 | 0.736 | 0.735 / 0.734 | +0.2% |
| data/log_analytics | 772.2 | 456.1 | 0.591 | 0.594 / 0.592 | -0.3% | 804.5 | 490.2 | 0.609 | 0.609 / 0.610 | +0.0% |
| data/pandas_etl | 528.2 | 527.3 | 0.998 | 1.000 / 1.000 | -0.2% | 590.6 | 587.7 | 0.995 | 1.000 / 0.998 | -0.4% |
| data/report_render | 406.9 | 178.1 | 0.438 | 0.436 / 0.438 | +0.3% | 463.5 | 196.0 | 0.423 | 0.423 / 0.421 | +0.3% |
| data/state_snapshot | 647.3 | 414.7 | 0.641 | 0.651 / 0.655 | -1.9% | 1229.5 | 582.3 | 0.474 | 0.476 / 0.475 | -0.4% |
| data/text_corpus | 604.3 | 603.5 | 0.999 | 1.000 / 0.991 | +0.3% | 628.1 | 632.7 | 1.007 | 1.001 / 1.000 | +0.7% |
| data/tz_events | 530.6 | 312.1 | 0.588 | 0.591 / 0.583 | +0.2% | 609.1 | 378.6 | 0.622 | 0.615 / 0.614 | +1.2% |
| data/xml_feeds | 506.0 | 500.9 | 0.990 | 1.000 / 1.007 | -1.4% | 571.6 | 572.2 | 1.001 | 1.005 / 1.008 | -0.5% |
| devtools/black_format | 1194.6 | 1190.1 | 0.996 | 0.990 / 0.996 | +0.3% | 1298.2 | 1289.4 | 0.993 | 0.999 / 1.003 | -0.8% |
| devtools/coverage_pyflakes_tests | 380.9 | 378.1 | 0.993 | 0.995 / 0.989 | +0.1% | 491.5 | 491.4 | 1.000 | 0.999 / 1.006 | -0.2% |
| devtools/import_startup | 1888.7 | 1889.1 | 1.000 | 1.000 / 1.002 | -0.1% | 2240.4 | 2243.0 | 1.001 | 1.001 / 0.999 | +0.1% |
| devtools/isort_stdlib | 284.9 | 219.2 | 0.769 | 0.771 / 0.766 | +0.0% | 1146.9 | 233.4 | 0.203 | 0.196 / 0.197 | +3.6% |
| devtools/pip_resolve | 563.0 | 530.7 | 0.943 | 0.949 / 0.955 | -1.0% | 623.7 | 593.5 | 0.952 | 0.947 / 0.950 | +0.3% |
| devtools/pycodestyle_stdlib | 1013.7 | 991.0 | 0.978 | 0.985 / 0.995 | -1.2% | 1065.7 | 1060.7 | 0.995 | 0.999 / 0.999 | -0.4% |
| devtools/pyflakes_stdlib | 1210.5 | 1203.1 | 0.994 | 0.997 / 1.001 | -0.5% | 1190.3 | 1202.4 | 1.010 | 1.005 / 1.003 | +0.6% |
| devtools/pylint_stdlib | 1184.4 | 1183.7 | 0.999 | 1.002 / 1.005 | -0.4% | 894.7 | 890.0 | 0.995 | 0.999 / 1.001 | -0.5% |
| devtools/pytest_networkx | 629.5 | 600.3 | 0.954 | 0.948 / 0.958 | +0.0% | 631.9 | 600.9 | 0.951 | 0.954 / 0.954 | -0.3% |
| devtools/rich_render | 284.8 | 283.0 | 0.994 | 0.998 / 0.992 | -0.1% | 309.2 | 308.1 | 0.997 | 1.000 / 0.992 | +0.1% |
| devtools/sphinx_autodoc | 848.9 | 838.5 | 0.988 | 0.977 / 0.989 | +0.5% | 903.3 | 884.9 | 0.980 | 0.988 / 0.988 | -0.9% |
| devtools/sqlglot_transpile | 878.0 | 844.7 | 0.962 | 0.968 / 0.972 | -0.8% | 1027.1 | 1002.1 | 0.976 | 0.974 / 0.974 | +0.2% |
| web/aiohttp_app | 273.3 | 247.5 | 0.906 | 0.910 / 0.905 | -0.2% | 300.6 | 280.1 | 0.932 | 0.925 / 0.929 | +0.5% |
| web/asyncio_http | 231.2 | 182.5 | 0.790 | 0.791 / 0.793 | -0.3% | 242.9 | 196.1 | 0.807 | 0.815 / 0.815 | -0.9% |
| web/django_blog | 489.1 | 487.1 | 0.996 | 0.997 / 0.999 | -0.2% | 544.9 | 542.2 | 0.995 | 0.995 / 0.997 | -0.1% |
| web/email_mime | 656.2 | 645.2 | 0.983 | 0.995 / 0.988 | -0.8% | 706.4 | 707.1 | 1.001 | 1.001 / 1.009 | -0.4% |
| web/fastapi_pydantic | 297.0 | 290.4 | 0.978 | 0.983 / 0.977 | -0.2% | 340.4 | 332.5 | 0.977 | 0.986 / 0.989 | -1.0% |
| web/flask_jinja | 417.3 | 416.0 | 0.997 | 0.990 / 0.990 | +0.7% | 460.2 | 457.3 | 0.994 | 0.986 / 0.992 | +0.4% |
| web/json_api | 396.1 | 330.6 | 0.835 | 0.835 / 0.839 | -0.3% | 440.0 | 369.3 | 0.839 | 0.835 / 0.835 | +0.5% |
| web/logging_service | 283.1 | 230.1 | 0.813 | 0.817 / 0.812 | -0.2% | 300.2 | 259.6 | 0.865 | 0.864 / 0.853 | +0.8% |
| web/requests_local | 250.5 | 232.9 | 0.930 | 0.941 / 0.927 | -0.5% | 250.1 | 236.0 | 0.944 | 0.944 / 0.943 | +0.0% |
| web/sqlalchemy_orm | 341.7 | 341.4 | 0.999 | 0.997 / 0.996 | +0.3% | 354.5 | 353.4 | 0.997 | 1.005 / 1.007 | -0.9% |

Summary (geometric means; workloads per family in parentheses):

| | 3.15.1 wave 2 phase 0 | 3.15.1 wave 1 rep 1 | 3.15.1 wave 1 rep 2 | 3.15.1t wave 2 phase 0 | 3.15.1t wave 1 rep 1 | 3.15.1t wave 1 rep 2 |
|---|---:|---:|---:|---:|---:|---:|
| geomean data | 0.768 (12) | 0.773 (12) | 0.771 (12) | 0.756 (12) | 0.756 (12) | 0.755 (12) |
| geomean devtools | 0.962 (12) | 0.963 (12) | 0.966 (12) | 0.865 (12) | 0.863 (12) | 0.863 (12) |
| geomean web | 0.919 (10) | 0.922 (10) | 0.919 (10) | 0.932 (10) | 0.933 (10) | 0.934 (10) |
| geomean all | 0.877 (34) | 0.880 (34) | 0.879 (34) | 0.843 (34) | 0.843 (34) | 0.843 (34) |

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| median | 0.978 | 0.978 |
| best | `data/report_render` 0.438 | `devtools/isort_stdlib` 0.203 |
| worst | `data/config_merge` 1.001 | `devtools/pyflakes_stdlib` 1.010 |
| ≤ 0.95 / > 1.05 | 13 / 0 | 12 / 0 |

Reading:

- **Wave 1 reproduces: 0.877 on 3.15.1, 0.843 on 3.15.1t.** Wave 1 measured 0.880 / 0.879
  and 0.843 / 0.843. The family geomeans agree within 0.005. No workload is above 1.05.
  The worst are `config_merge` 1.001 (3.15.1) and `pyflakes_stdlib` 1.010 (3.15.1t; wave 1
  1.005 / 1.003).
- **Per workload:** on 3.15.1 every ratio is within 2% of wave 1's two-replicate average
  (34 / 34). The largest moves are `state_snapshot` −1.9%, `xml_feeds` −1.4% and
  `pycodestyle_stdlib` −1.2%. On 3.15.1t 33 / 34 are within 2%.
- **The one exception is `isort_stdlib` on 3.15.1t**, at 0.203 against 0.196 / 0.197.
  Its *plain* median is 3.4% faster than wave 1's (1146.9 vs 1187.4 ms). Its enabled
  median is unchanged (233.4 vs 232.8 ms). The kernel did not get slower: the ratio moved
  with CPython's own `deepcopy` time on the free-threaded build.
- **Plain CPython is the same speed as in wave 1.** The geometric mean of per-workload
  plain medians, this run ÷ wave 1 replicate 1, is 1.0010 on 3.15.1 and 0.9968 on
  3.15.1t. That is the same box type, kernel and interpreter build. So wave 1's numbers
  and this run's can be compared directly (*inferred* from that agreement).


### 4.1 Whole-process timing (`pyperf command`, plain vs `RUSTIQUE=1`)

Wave 1's method (`phase3/whole.py`): one `setup()` + one `run()` per process,
interpreter start and imports included, `RUSTIQUE=1` through the wheel's `.pth`. 4 rounds
of 5 processes per variant, the order alternating per round (20 values per variant),
pinned to domain 3 (24–31), 18:55–19:16Z.

| build | workload | plain s (mean ± std) | RUSTIQUE=1 s | ratio | values | wave 1 ratio |
|---|---|---:|---:|---:|---:|---:|
| 3.15.1 | data/log_analytics | 0.968 ± 0.006 | 0.625 ± 0.015 | 0.645 | 20/20 | 0.639 |
| 3.15.1 | data/report_render | 0.450 ± 0.002 | 0.222 ± 0.002 | 0.494 | 20/20 | 0.494 |
| 3.15.1 | data/state_snapshot | 0.891 ± 0.005 | 0.659 ± 0.005 | 0.740 | 20/20 | 0.749 |
| 3.15.1 | devtools/import_startup | 1.907 ± 0.004 | 1.941 ± 0.007 | 1.018 | 20/20 | 1.022 |
| 3.15.1 | devtools/pytest_networkx | 1.077 ± 0.007 | 1.043 ± 0.007 | 0.969 | 20/20 | 0.972 |
| 3.15.1 | web/json_api | 0.656 ± 0.009 | 0.518 ± 0.002 | 0.790 | 20/20 | 0.794 |
| 3.15.1 | **geomean** | | | **0.754** | | **0.756** |
| 3.15.1t | data/log_analytics | 1.028 ± 0.006 | 0.669 ± 0.005 | 0.651 | 20/20 | 0.645 |
| 3.15.1t | data/report_render | 0.502 ± 0.002 | 0.241 ± 0.001 | 0.479 | 20/20 | 0.481 |
| 3.15.1t | data/state_snapshot | 1.446 ± 0.008 | 0.754 ± 0.003 | 0.522 | 20/20 | 0.520 |
| 3.15.1t | devtools/import_startup | 2.261 ± 0.005 | 2.311 ± 0.003 | 1.022 | 20/20 | 1.021 |
| 3.15.1t | devtools/pytest_networkx | 1.214 ± 0.005 | 1.181 ± 0.006 | 0.973 | 20/20 | 0.973 |
| 3.15.1t | web/json_api | 0.706 ± 0.002 | 0.573 ± 0.002 | 0.812 | 20/20 | 0.810 |
| 3.15.1t | **geomean** | | | **0.713** | | **0.712** |

- **0.754 / 0.713, against wave 1's 0.756 / 0.712.** Every workload is within 0.010 of
  wave 1's ratio.
- Digests are identical plain vs `RUSTIQUE=1` for 5 of 6 workloads, and every enabled
  process had `rustique._rustique` loaded. `devtools/import_startup` differs **by
  construction**, as in wave 1: its result counts `sys.modules` in 21 children, which
  gain `rustique._rustique` (`whole/digests-*`).
- **Wave 1's pytest-plugin finding is fixed on the box.** In the *plain*
  `pytest_networkx` processes `rustique._rustique` is no longer loaded: `rustique=False`
  in every plain digest line, where wave 1 had `True`. That is the fix `838cd18`
  (`docs/atlas/phase3.md` §11).

### 4.2 Which targets ran (counters of one steady-state `run()`)

`phase3/wlstats_run.py` per workload (`workloads/stats-run-<build>/*.json`). The 3.15.1
counters equal wave 1's (`docs/atlas/phase3.md` §4.2) in every hit count of the top
targets, with two kinds of differences:

- **`tuple.__contains__` fallbacks moved from `deopt` to `args`.** This is the
  post-Phase-3 change "non-scalar needles rejected by one type-flag test" (`32b6950`). The
  needle is now rejected before the scan instead of restarting from inside it. Examples:
  - `black_format`: 13,442 `deopt` → 13,442 `args`;
  - `sqlglot_transpile`: 3,115 `args` + 6,908 `deopt` → 10,023 `args`;
  - `django_blog`: 2,392 + 2,702 → 5,094;
  - `sphinx_autodoc`: 85 + 1,621 → 1,706.

  Hits are unchanged.
- Small run-to-run differences in workloads whose call counts are not fixed:
  - `pylint_stdlib` `tuple.__contains__` 31,198 vs 30,909;
  - `pytest_networkx` `PurePath.__eq__` 19,434 vs 19,518.

## 5. pyperformance 1.14.0

Wave 1's method: driver and benchmark venvs per build created as in wave 1's Phase 0, plain
vs `rustique.enable()` (all 15 groups) injected through `phase0/sitecustomize.py`
(`RUSTIQUE_ENABLE=0|1`), full default suite. Two replicate pairs per build: **A** runs
plain first, **B** enabled first. Each run was pinned to one L3 domain, at most one timing
job per domain:

- 3.15.1 A on domain 1 and B on domain 3;
- 3.15.1t A on domain 2 and B on domain 0, after domain 0's callgrind jobs.

The benchmark venvs equal wave 1's except `anyio` 4.16.0 (wave 1: 4.15.1, a dependency of
httpx/starlette; `pyperformance/bench-venv-freeze-*.txt`). Ratios are means enabled ÷ plain.

| | 3.15.1 A | 3.15.1 B | 3.15.1t A | 3.15.1t B | wave 1 (Phase 3) A / B |
|---|---:|---:|---:|---:|---|
| benchmarks compared | 122 | 122 | 120 | 119 | 121 / 120 · 120 / 122 |
| **geometric mean** | **0.941** | **0.943** | **0.955** | **0.955** | 0.942 / 0.940 · 0.956 / 0.957 |
| median | 0.998 | 0.999 | 0.998 | 0.999 | 0.998 / 0.998 · 0.999 / 1.000 |
| < 0.95 | 12 | 12 | 10 | 10 | 13 / 12 · 9 / 10 |
| > 1.05 | 1 | 2 | 1 | 1 | 2 / 1 · 1 / 1 |

**Per benchmark, both pairs averaged** (geometric mean of the two ratios; the three port
benchmarks from their solo re-run where a pair lost them; `tables/pyperformance.md`, wave
1's `analyze_phase3.py ppcompare` against wave 1's raw files):

- **0.942 on 3.15.1 and 0.956 on 3.15.1t** over 122 benchmarks each. Wave 1, computed the
  same way: **0.942 / 0.957**.
- Faster by > 2% in both pairs:
  - 3.15.1, 15 benchmarks: `deepcopy_memo` 0.132 / 0.136, `deepcopy` 0.228 / 0.226,
    `json_dumps` 0.252 / 0.252, `json_loads` 0.576 / 0.578, `logging_silent`
    0.626 / 0.625, `logging_format` 0.662 / 0.665, `logging_simple` 0.676 / 0.678,
    `sympy_integrate` 0.836 / 0.834, `deepcopy_reduce` 0.909 / 0.909, `hexiom`
    0.930 / 0.909, `meteor_contest` 0.928 / 0.924, `pathlib` 0.934 / 0.931,
    `sympy_str` 0.964 / 0.965, `chaos` 0.969 / 0.972, `bench_thread_pool` 0.975 / 0.974.
  - 3.15.1t, 14 benchmarks: `deepcopy_memo` 0.185 / 0.181, `deepcopy` 0.288 / 0.288,
    `json_dumps` 0.317 / 0.320, `json_loads` 0.612 / 0.611, `logging_format`
    0.781 / 0.779, `logging_simple` 0.784 / 0.788, `sympy_integrate` 0.834 / 0.837,
    `deepcopy_reduce` 0.920 / 0.924, `meteor_contest` 0.937 / 0.937, `pathlib`
    0.943 / 0.940, `sympy_str`, `async_tree_eager_memoization`, `logging_silent`
    0.975 / 0.977, `richards_super` 0.977 / 0.979.
- Slower by > 2% in both pairs:

  | build | benchmark | ratio (A / B) | wave 1 (A/B avg) |
  |---|---|---|---:|
  | 3.15.1 | **`python_startup`** | 1.088 / 1.096 | 1.088 |
  | 3.15.1t | **`python_startup`** | 1.109 / 1.092 | 1.095 |
  | 3.15.1t | `xdsl_constant_fold` | 1.029 / 1.035 | 1.033 |
  | 3.15.1t | `pyflate` | 1.028 / 1.029 | 1.029 |

  These are exactly wave 1's three. `python_startup` is the startup cost of §6 (the
  `enable()` in `sitecustomize`). Wave 1 attributed `pyflate` to the box (1.021 in its
  Phase 0). `xdsl_constant_fold` on 3.15.1t reproduces for the third and fourth time:
  **still flagged, cause not investigated**.
- **Above 1.05 in one pair only**: `unpack_sequence` 1.053 (3.15.1 B). Wave 1's
  `fannkuch` 1.114 (3.15.1 pair A) measures 1.009 / 0.995 here, which confirms it was
  noise.
- **Port benchmarks.** `asyncio_tcp` and `asyncio_tcp_ssl` died with `OSError: [Errno 98]
  … ('127.0.0.1', 8882): address already in use` in both 3.15.1t enabled runs (A and B
  ran concurrently), and `asyncio_websockets` in 3.15.1t B plain. These are the
  collisions wave 1 also saw. They were re-run alone, plain then enabled per build, with
  nothing else timing (21:38–21:48Z; `pyperformance/*-ports.json`):
  - 3.15.1: 1.000, 1.001, 0.998;
  - 3.15.1t: 1.016, 0.997, 0.996.
- `genshi` fails on plain CPython 3.15 (as in wave 1), so every full run exits 1.


## 6. Startup cells

`benches/dx/startup.py cells`, wave 1's method: the release wheel `pip install`ed into a
fresh `python -m venv`, against an identical venv without it. Callgrind total Ir (median
of 3, `PYTHONHASHSEED=0`) and wall time (median and p10–p90 of 300 interleaved rounds,
pinned to CPU 30, nothing else timing in domain 3). Ratios are against `python -c pass` in
the plain venv. "pth" = both venvs carry a comment-only `.pth`. Run 19:16–19:19Z
(`startup/cells-*.json`).

| build | venv | cell | Ir | Ir ratio | wall ms median (p10–p90) | wall ratio |
|---|---|---|---:|---:|---:|---:|
| 3.15.1 | bare | plain: pass | 38,670,172 | 1.0000 | 9.45 (9.41–9.50) | 1.0000 |
| 3.15.1 | bare | installed, unset: pass | 39,705,529 | 1.0268 | 9.77 (9.72–9.83) | 1.0332 |
| 3.15.1 | bare | RUSTIQUE=1: pass | 41,505,064 | 1.0733 | 10.52 (10.47–10.58) | 1.1131 |
| 3.15.1 | bare | import rustique | 41,265,315 | 1.0671 | 10.40 (10.36–10.46) | 1.1002 |
| 3.15.1 | bare | import + enable() | 42,681,183 | 1.1037 | 10.73 (10.69–10.80) | 1.1355 |
| 3.15.1 | bare | marginal RUSTIQUE=1 vs unset | | 1.0453 | | 1.0773 |
| 3.15.1 | pth | plain: pass | 39,438,206 | 1.0000 | 9.68 (9.63–9.73) | 1.0000 |
| 3.15.1 | pth | installed, unset: pass | 39,760,066 | 1.0082 | 9.77 (9.72–9.83) | 1.0094 |
| 3.15.1 | pth | RUSTIQUE=1: pass | 41,559,852 | 1.0538 | 10.52 (10.46–10.59) | 1.0863 |
| 3.15.1 | pth | import rustique | 41,319,292 | 1.0477 | 10.40 (10.34–10.47) | 1.0741 |
| 3.15.1 | pth | import + enable() | 42,735,783 | 1.0836 | 10.73 (10.68–10.80) | 1.1088 |
| 3.15.1 | pth | marginal RUSTIQUE=1 vs unset | | 1.0453 | | 1.0762 |
| 3.14.9 | bare | plain: pass | 40,561,684 | 1.0000 | 9.52 (9.46–9.57) | 1.0000 |
| 3.14.9 | bare | installed, unset: pass | 42,488,008 | 1.0475 | 9.99 (9.93–10.04) | 1.0493 |
| 3.14.9 | bare | RUSTIQUE=1: pass | 45,180,170 | 1.1139 | 11.06 (11.00–11.12) | 1.1619 |
| 3.14.9 | bare | import rustique | 44,006,275 | 1.0849 | 10.70 (10.63–10.76) | 1.1242 |
| 3.14.9 | bare | import + enable() | 45,424,559 | 1.1199 | 11.03 (10.97–11.08) | 1.1587 |
| 3.14.9 | bare | marginal RUSTIQUE=1 vs unset | | 1.0634 | | 1.1073 |
| 3.14.9 | pth | plain: pass | 41,985,164 | 1.0000 | 9.88 (9.83–9.95) | 1.0000 |
| 3.14.9 | pth | installed, unset: pass | 42,577,570 | 1.0141 | 10.03 (9.96–10.09) | 1.0144 |
| 3.14.9 | pth | RUSTIQUE=1: pass | 45,264,946 | 1.0781 | 11.11 (11.04–11.18) | 1.1246 |
| 3.14.9 | pth | import rustique | 44,096,093 | 1.0503 | 10.75 (10.69–10.81) | 1.0878 |
| 3.14.9 | pth | import + enable() | 45,514,236 | 1.0841 | 11.08 (11.01–11.15) | 1.1214 |
| 3.14.9 | pth | marginal RUSTIQUE=1 vs unset | | 1.0631 | | 1.1086 |

| cell (Ir / wall vs plain `-c pass`) | 3.15.1 bare | 3.15.1 other .pth | 3.14.9 bare | 3.14.9 other .pth |
|---|---|---|---|---|
| installed, `RUSTIQUE` unset | **1.027 / 1.033** (wave 1: 1.027 / 1.032) | **1.008 / 1.009** (1.008 / 1.011) | **1.047 / 1.049** (1.047 / 1.048) | **1.014 / 1.014** (1.014 / 1.014) |
| `RUSTIQUE=1` | 1.073 / 1.113 (1.073 / 1.112) | 1.054 / 1.086 (1.054 / 1.088) | 1.114 / 1.162 (1.114 / 1.161) | 1.078 / 1.125 (1.078 / 1.125) |
| `import rustique` | 1.067 / 1.100 (1.067 / 1.100) | 1.048 / 1.074 (1.048 / 1.077) | 1.085 / 1.124 (1.085 / 1.124) | 1.050 / 1.088 (1.050 / 1.088) |
| `import rustique; rustique.enable()` | **1.104 / 1.136** (1.108 / 1.139) | 1.084 / 1.109 (1.088 / 1.113) | **1.120 / 1.159** (1.124 / 1.162) | 1.084 / 1.121 (1.088 / 1.124) |
| marginal: `RUSTIQUE=1` vs unset, same venv | **1.045 / 1.077** (1.045 / 1.078) | **1.045 / 1.076** (1.045 / 1.076) | **1.063 / 1.107** (1.063 / 1.108) | **1.063 / 1.109** (1.063 / 1.109) |

- **Every cell reproduces wave 1's within 0.003**, except `import + enable()`, which is
  **0.16–0.18M Ir cheaper**:
  - 3.15.1: 42,681,183 vs 42,841,505 Ir;
  - 3.14.9: 45,424,559 vs 45,605,596 Ir.

  That matches the post-Phase-3 fix "shared strings in the status" (`9e8f7c9`, −0.14M Ir;
  *inferred* from the size of the change). `enable()` after `import rustique` now adds
  +1.42M Ir on both versions (wave 1: +1.58M).
- **Installed but off stays ≤ 1.05 on every cell.** The closest is 3.14.9 bare at
  1.047 / 1.049, and with another `.pth` every cell is ≤ 1.014.
- The `RUSTIQUE=1` marginal cost is unchanged: 1.045 Ir / 1.077 wall (+0.75 ms) on
  3.15.1, and 1.063 / 1.107 (+1.07 ms) on 3.14.9. Both remain wave 2's first item
  (`docs/DECISIONS.md` §9.1).

## 7. Scoreboard (enabled tier, against wave 1's Phase 3)

`benches/scoreboard.py` with `--impls enabled`, which turns on all 15 groups. Cell
categories are wave 1's:

- **kernel path**: `sorted/sum/min/max/any/all`, `in`/`index`/`count` on list/tuple,
  `list.sort()`;
- **wave-1 serial**: `json.dumps`, `copy.deepcopy`;
- **other**: everything else.

The comparison is cell by cell against wave 1's Phase 3 raw files, with wave 1's
`analyze_phase3.py sbcompare` (`tables/sbcompare-*.md`). Every cell's result was verified
against plain CPython's: **0 mismatches, 0 errors**.

### 7.1 0–100 elements, callgrind instruction counts (3814 cells per build)

8 callgrind shards on domain 0, 18:57–19:19Z (3.15.1: 619 s; 3.15.1t: 669 s).

| | 3.15.1 | 3.15.1 wave 1 | 3.15.1t | 3.15.1t wave 1 |
|---|---|---|---|---|
| cells ≤ 1.05, as run | 3808 / 3814 | 3798 / 3814 | 3812 / 3814 | 3809 / 3814 |
| cells ≤ 1.05 after re-measuring the cells > 1.05 alone | **3814 / 3814** | 3811 / 3814 | **3814 / 3814** | 3811 / 3814 |
| geomean, all | **0.859** | 0.859 | **0.864** | 0.864 |
| kernel path (586): geomean / ≤ 1.05 | 0.633 / **586** | 0.632 / 583 | 0.649 / **586** | 0.649 / 583 |
| wave-1 serial (152): geomean | 0.154 | 0.154 | 0.161 | 0.161 |
| other (3076): geomean / median | 0.992 / 1.000 | 0.992 / 1.000 | 0.992 / 1.000 | 0.992 / 1.000 |

- **No op's geomean moved by more than 3%** against wave 1, on either build.
- **Wave 1's three kernel-path exceedances are gone**: they now measure 1.026–1.045. These
  were `count`/`in` with a 1-element list or tuple of objects, fixed after Phase 3 in
  `32b6950`.

  | cell | 3.15.1 Ir (ratio) | 3.15.1t Ir (ratio) | wave 1 Phase 3 |
  |---|---|---|---|
  | `list/x.count(hit)/object/1` | 297.9 → 306.9 (1.030) | 343.7 → 352.7 (1.026) | 1.066 / 1.062 |
  | `tuple/x.count(hit)/object/1` | 301.9 → 310.9 (1.030) | 328.7 → 337.7 (1.027) | 1.065 / 1.065 |
  | `list/miss in x/object/1` | 375.1 → 390.1 (1.040) | 400.1 → 418.1 (1.045) | 1.050 / 1.055 |

- **Cells above 1.05 as run, each re-measured alone 3 times** in its own callgrind worker
  (`sb_isolate.py`, `scoreboard/isolated-instr-*.txt`; identical Ir in all 3 runs):

  | build | cell | as run | alone |
  |---|---|---:|---:|
  | 3.15.1 | `set/for v in x/mixed/0` | 1.076 | 0.997 |
  | 3.15.1 | `frozenset/for v in x/mixed/0` | 1.076 | 0.997 |
  | 3.15.1 | `frozenset/tuple(x)/str/0` | 1.069 | 0.997 |
  | 3.15.1 | `tuple/repr(x)/int/0` | 1.062 | 0.999 |
  | 3.15.1 | `tuple/pickle.dumps/str/8` | 1.058 | 1.029 |
  | 3.15.1 | `list/insert(0, v)/float/100` | 1.057 | 0.860 |
  | 3.15.1t | `list/x + y/str/8` | 1.074 | 1.046 |
  | 3.15.1t | `list/x * 2/str/8` | 1.068 | 1.043 |

  The 3.15.1 cells are allocator state left by the previous cell in a shared worker, as
  in wave 1. They are code Rustique does not change. The two 3.15.1t cells are the same
  as in wave 1, with the same values alone (+42 Ir).

### 7.2 10k/1M elements, wall time (3.15.1)

Wave 1's method: `scoreboard.py time --size 1e4,1e6`, pyperf medians of 3 processes × 3
values, 4 single-CPU shards on CPUs 2, 10, 20 and 30, after every pyperformance run had
finished, 21:48–22:45Z (`scoreboard/time-enabled-3.15.1.json.shard*.json.gz`). 3.15.1t
was not run (§10).

| | 3.15.1 | 3.15.1 wave 1 |
|---|---|---|
| cells ≤ 1.05, as run | 1773 / 1846 | 1744 / 1846 |
| cells ≤ 1.05 after re-measuring every cell > 1.05 alone | **1836 / 1846** | (wave 1 re-measured only the kernel-path cells) |
| geomean, all | **0.828** | 0.830 |
| kernel path (328): geomean / cells > 1.05 as run | **0.461** / 2 | 0.462 / 5 |
| wave-1 serial (51): geomean | 0.249 | 0.243 |
| everything else (1467): geomean / median | 0.985 / 1.000 | 0.987 / — |

- **The aggregates reproduce wave 1.** Single cells at 3 processes do not, as both of wave
  1's phases found. 26 ops moved by more than 3% against wave 1, in both directions
  (`tables/sbcompare-3.15.1.md`).
- One of those moves is an anomaly of the *plain* measurement. `tuple (*x, *y)` reads
  0.499 because its builtin side measured 2.4× slower than in wave 1 (e.g. `float/10000`:
  161.7 vs 66.3 µs, relative std 0.2%). Its enabled side is unchanged (66.6 vs 66.7 µs).
  Rustique does not touch that code, and the cells are below 1.05, so they were not
  re-measured.
- **Every cell above 1.05 was re-measured alone.** `sb_isolate.py time` ran each of the 73
  cells twice, alone on one CPU (2, 10, 20 or 30), at 10 processes × 5 values, 22:45–23:00Z
  (`scoreboard/isolated-time-3.15.1.txt`).
  - 63 measure ≤ 1.05, including both kernel-path cells: `set/all(x)/str/10k` 1.061 and
    `frozenset/min(x)/str/10k` 1.052 as run.
  - **10 stay above 1.05** on the mean of the two runs. None is a kernel-path or
    wave-1-serial cell. Each is code that no target changes, measured in a process where
    `enable()` ran:

  | cell | as run | alone (run 1, run 2) | wave 1 as run |
  |---|---:|---|---:|
  | `list/pickle.dumps/str/1M` | 1.076 | 1.211, 1.225 | 1.216 |
  | `tuple/for v in x/int/10k` | 1.194 | 1.144, 1.158 | 1.355 |
  | `frozenset/x == t/str/1M` | 1.079 | 1.134, 1.140 | 0.990 |
  | `list/pickle round trip/str/1M` | 1.073 | 1.120, 1.108 | 1.138 |
  | `set/x == t/object/10k` | 1.097 | 1.160, 1.094 | 1.079 |
  | `list/repr(x)/object/1M` | 1.067 | 1.063, 1.057 | 1.061 |
  | `tuple/for v in x/str/10k` | 1.180 | 1.100, 1.169 | 0.987 |
  | `dict/x == t/object/10k` | 1.130 | 1.304, 1.011 | 0.933 |
  | `dict/tuple(x)/mixed/10k` | 1.181 | 1.008, 1.210 | 0.905 |
  | `dict/hit in x/object/1M` | 1.052 | 1.051, 1.166 | 1.099 |

  - The last four disagree with themselves between the two isolated runs (noise at this
    resolution).
  - The first six are consistent. Five of them were above 1.05 as run in wave 1 too, so
    they are not new: the code is identical to the merged wave 1.
  - Wave 1 never re-measured non-kernel cells, so this is the first time they are shown to
    persist alone. Iterating a tuple, comparing sets, pickling and repr of a large list
    read 5–22% slower in the enabled process. The likely mechanism is process state rather
    than a kernel: memory layout and allocator state after loading the extension and
    `enable()` (*inferred*, not decomposed). **Flagged** for the wave-2 slices that own
    startup and the engine split.


## 8. Wave-2 baselines

### 8.1 Free-threading penalty (plain 3.15.1t ÷ plain 3.15.1)

**Workloads**, from the stage-4 plain medians (§4). The last column is the same quotient
from wave 1's Phase 3, replicate 1:

| workload | 3.15.1 plain ms | 3.15.1t plain ms | FT ÷ GIL | wave 1 (phase 3 rep 1) |
|---|---:|---:|---:|---:|
| data/backup_archive | 598.5 | 605.5 | 1.012 | 1.012 |
| data/billing_engine | 932.3 | 748.9 | 0.803 | 0.826 |
| data/config_merge | 540.6 | 611.7 | 1.131 | 1.131 |
| data/csv_etl | 393.0 | 428.8 | 1.091 | 1.114 |
| data/fs_inventory | 316.0 | 321.1 | 1.016 | 1.017 |
| data/log_analytics | 772.2 | 804.5 | 1.042 | 1.053 |
| data/pandas_etl | 528.2 | 590.6 | 1.118 | 1.117 |
| data/report_render | 406.9 | 463.5 | 1.139 | 1.145 |
| data/state_snapshot | 647.3 | 1229.5 | **1.899** | 1.927 |
| data/text_corpus | 604.3 | 628.1 | 1.039 | 1.045 |
| data/tz_events | 530.6 | 609.1 | 1.148 | 1.168 |
| data/xml_feeds | 506.0 | 571.6 | 1.130 | 1.126 |
| devtools/black_format | 1194.6 | 1298.2 | 1.087 | 1.079 |
| devtools/coverage_pyflakes_tests | 380.9 | 491.5 | 1.290 | 1.294 |
| devtools/import_startup | 1888.7 | 2240.4 | 1.186 | 1.183 |
| devtools/isort_stdlib | 284.9 | 1146.9 | **4.025** | 4.186 |
| devtools/pip_resolve | 563.0 | 623.7 | 1.108 | 1.112 |
| devtools/pycodestyle_stdlib | 1013.7 | 1065.7 | 1.051 | 1.055 |
| devtools/pyflakes_stdlib | 1210.5 | 1190.3 | 0.983 | 0.986 |
| devtools/pylint_stdlib | 1184.4 | 894.7 | 0.755 | 0.761 |
| devtools/pytest_networkx | 629.5 | 631.9 | 1.004 | 1.001 |
| devtools/rich_render | 284.8 | 309.2 | 1.086 | 1.089 |
| devtools/sphinx_autodoc | 848.9 | 903.3 | 1.064 | 1.047 |
| devtools/sqlglot_transpile | 878.0 | 1027.1 | 1.170 | 1.171 |
| web/aiohttp_app | 273.3 | 300.6 | 1.100 | 1.114 |
| web/asyncio_http | 231.2 | 242.9 | 1.051 | 1.042 |
| web/django_blog | 489.1 | 544.9 | 1.114 | 1.118 |
| web/email_mime | 656.2 | 706.4 | 1.076 | 1.083 |
| web/fastapi_pydantic | 297.0 | 340.4 | 1.146 | 1.144 |
| web/flask_jinja | 417.3 | 460.2 | 1.103 | 1.105 |
| web/json_api | 396.1 | 440.0 | 1.111 | 1.114 |
| web/logging_service | 283.1 | 300.2 | 1.060 | 1.061 |
| web/requests_local | 250.5 | 250.1 | 0.999 | 1.004 |
| web/sqlalchemy_orm | 341.7 | 354.5 | 1.037 | 1.026 |

- **Geometric mean 1.127** (data 1.109, devtools 1.187, web 1.079), median 1.089. Wave 1's
  quotients agree within ±0.03 per workload, except `isort_stdlib` (4.03 vs 4.19, its
  plain 3.15.1t median moved, §4).
- **Outliers above 1.3: `devtools/isort_stdlib` 4.03 and `data/state_snapshot` 1.90.**
  Both are the `copy.deepcopy`-heavy workloads of wave 1's finding. `coverage_pyflakes_tests`
  is just below, at 1.29.
- **Faster on the free-threaded build: `pylint_stdlib` 0.755 and `billing_engine` 0.803**,
  and `pyflakes_stdlib` 0.98. These are among the workloads with the largest GC shares on
  the GIL build (§8.2):
  - `billing_engine` spends 297 ms in collections per run on 3.15.1 and 105 ms on 3.15.1t.
    That difference (192 ms) is the whole gap between the plain medians (183 ms)
    (*inferred* from §8.2's numbers).
  - `pylint_stdlib`: 174 vs 110 ms in collections, a fifth of its 290 ms gap. The rest is
    *not investigated*.

**pyperformance**, from the stage-4 plain runs (mean per benchmark; pair A 122 benchmarks,
pair B 121; `tables/ftpenalty.md` has every benchmark):

- **Geometric mean 1.074** (A 1.073, B 1.077), median 1.10.
- **Outliers above 1.3: `bench_thread_pool` 1.37, `dask` 1.36, `connected_components`
  1.30** (1.29 / 1.32). Next: `nbody` 1.27, `shortest_path` 1.26, `python_startup_no_site`
  1.25, `asyncio_tcp_ssl` 1.25, `docutils` 1.24, `async_tree_none` 1.24, `python_startup` 1.23.
- Faster on 3.15.1t: `gc_traversal` 0.51, `base16_large` 0.65, `create_gc_cycles` 0.70,
  `bench_mp_pool` 0.74, `async_tree_eager_io_tg` 0.77, `async_tree_eager_io` 0.85,
  `sqlite_synth` 0.85.
- **`deepcopy` is only 1.09–1.11 there** (`deepcopy` 1.095, `deepcopy_memo` 1.113,
  `deepcopy_reduce` 1.092). The 4× of `isort_stdlib` is therefore not `copy.deepcopy` in
  general but something in that workload's use of it: 26,265 small deep copies per run,
  1,030 of which restart in the original (wave 1 §4.2). *Not investigated*; the brief does
  not ask for it, and it is a wave-2 lead.

### 8.2 GC baseline (plain, one steady-state `run()`)

`gcstats.py`, one process per workload per build, plain. Both builds were run
21:38–21:40Z on domain 0, an untimed lane, while the port benchmarks ran alone on
domain 1. Columns, per build:

- collections in one `run()` (per generation);
- time inside collections (`gc.callbacks` start→stop), and its share of that `run()`'s
  wall time;
- objects collected;
- peak RSS of the process (`ru_maxrss`; setup included).

The thresholds are `(2000, 10, 10)` on both builds. The callback counts equal the
`gc.get_stats()` deltas on every workload. No workload has uncollectable objects.

| workload | 3.15.1 collections (per gen) | 3.15.1 GC ms (share of run) | 3.15.1 collected | 3.15.1 peak RSS MiB | 3.15.1t collections (per gen) | 3.15.1t GC ms (share of run) | 3.15.1t collected | 3.15.1t peak RSS MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| data/backup_archive | 0 (—) | 0.0 (0.0%) | 0 | 41 | 0 (—) | 0.0 (0.0%) | 0 | 49 |
| data/billing_engine | 375 (0:341/1:31/2:3) | 296.5 (33.8%) | 0 | 138 | 4 (0:4) | 104.6 (14.2%) | 0 | 147 |
| data/config_merge | 29 (0:27/1:2) | 7.9 (1.5%) | 18,018 | 36 | 5 (0:5) | 17.3 (2.9%) | 18,488 | 47 |
| data/csv_etl | 0 (—) | 0.0 (0.0%) | 0 | 48 | 0 (—) | 0.0 (0.0%) | 0 | 56 |
| data/fs_inventory | 8 (0:7/1:1) | 0.2 (0.1%) | 0 | 28 | 1 (0:1) | 2.8 (0.9%) | 10 | 31 |
| data/log_analytics | 1 (0:1) | 0.1 (0.0%) | 0 | 54 | 0 (—) | 0.0 (0.0%) | 0 | 66 |
| data/pandas_etl | 0 (—) | 0.0 (0.0%) | 0 | 256 | 0 (—) | 0.0 (0.0%) | 0 | 300 |
| data/report_render | 20 (0:19/1:1) | 2.8 (0.7%) | 0 | 49 | 3 (0:3) | 7.5 (1.7%) | 22 | 56 |
| data/state_snapshot | 103 (0:94/1:8/2:1) | 99.2 (15.4%) | 0 | 138 | 3 (0:3) | 112.2 (9.3%) | 0 | 164 |
| data/text_corpus | 7 (0:7) | 2.0 (0.3%) | 0 | 40 | 1 (0:1) | 2.4 (0.4%) | 22 | 42 |
| data/tz_events | 48 (0:44/1:4) | 0.6 (0.1%) | 0 | 55 | 3 (0:3) | 20.6 (3.5%) | 0 | 63 |
| data/xml_feeds | 72 (0:65/1:6/2:1) | 72.4 (14.2%) | 824 | 100 | 3 (0:3) | 52.4 (9.6%) | 887 | 109 |
| devtools/black_format | 158 (0:144/1:13/2:1) | 132.2 (11.1%) | 208,327 | 76 | 14 (0:14) | 114.1 (8.8%) | 256,697 | 61 |
| devtools/coverage_pyflakes_tests | 164 (0:149/1:14/2:1) | 47.6 (12.7%) | 286,837 | 63 | 17 (0:17) | 115.6 (24.6%) | 289,330 | 72 |
| devtools/import_startup | 0 (—) | 0.0 (0.0%) | 0 | 24 | 0 (—) | 0.0 (0.0%) | 0 | 27 |
| devtools/isort_stdlib | 1 (0:1) | 0.4 (0.2%) | 1,928 | 45 | 0 (—) | 0.0 (0.0%) | 0 | 54 |
| devtools/pip_resolve | 24 (0:22/1:2) | 10.0 (1.8%) | 13,709 | 72 | 1 (0:1) | 17.9 (2.9%) | 14,569 | 84 |
| devtools/pycodestyle_stdlib | 0 (—) | 0.0 (0.0%) | 0 | 30 | 0 (—) | 0.0 (0.0%) | 0 | 35 |
| devtools/pyflakes_stdlib | 125 (0:115/1:10) | 222.5 (19.3%) | 548,028 | 79 | 52 (0:52) | 199.6 (16.6%) | 792,692 | 61 |
| devtools/pylint_stdlib | 167 (0:152/1:14/2:1) | 174.3 (19.8%) | 103,740 | 123 | 3 (0:3) | 109.9 (12.4%) | 103,603 | 139 |
| devtools/pytest_networkx | 42 (0:34/1:2/2:6) | 199.3 (32.4%) | 14,254 | 131 | 6 (2:6) | 129.2 (22.0%) | 14,700 | 152 |
| devtools/rich_render | 36 (0:33/1:3) | 4.9 (1.7%) | 17 | 43 | 1 (0:1) | 4.9 (1.6%) | 26 | 53 |
| devtools/sphinx_autodoc | 70 (0:64/1:6) | 23.6 (2.8%) | 132,971 | 99 | 2 (0:2) | 49.2 (5.7%) | 82,308 | 125 |
| devtools/sqlglot_transpile | 156 (0:142/1:13/2:1) | 73.7 (8.4%) | 332,206 | 49 | 15 (0:15) | 137.4 (13.6%) | 324,168 | 57 |
| web/aiohttp_app | 1 (1:1) | 2.1 (0.8%) | 212 | 49 | 0 (—) | 0.0 (0.0%) | 0 | 57 |
| web/asyncio_http | 2 (0:2) | 0.2 (0.1%) | 0 | 32 | 0 (—) | 0.0 (0.0%) | 0 | 36 |
| web/django_blog | 17 (0:16/1:1) | 8.0 (1.6%) | 29,842 | 62 | 1 (0:1) | 17.7 (3.3%) | 30,745 | 81 |
| web/email_mime | 30 (0:28/1:2) | 14.9 (2.3%) | 71,352 | 34 | 15 (0:15) | 35.9 (5.1%) | 74,584 | 40 |
| web/fastapi_pydantic | 11 (0:10/1:1) | 4.1 (1.4%) | 13,800 | 64 | 0 (—) | 0.0 (0.0%) | 0 | 78 |
| web/flask_jinja | 7 (0:6/1:1) | 2.7 (0.7%) | 14,748 | 50 | 1 (0:1) | 9.3 (2.0%) | 21,917 | 62 |
| web/json_api | 20 (0:18/1:2) | 3.6 (0.9%) | 0 | 45 | 2 (0:2) | 6.1 (1.4%) | 156 | 53 |
| web/logging_service | 0 (—) | 0.0 (0.0%) | 0 | 33 | 0 (—) | 0.0 (0.0%) | 0 | 38 |
| web/requests_local | 0 (—) | 0.0 (0.0%) | 0 | 39 | 0 (—) | 0.0 (0.0%) | 0 | 47 |
| web/sqlalchemy_orm | 31 (0:28/1:3) | 10.7 (3.1%) | 2,008 | 119 | 0 (—) | 0.0 (0.0%) | 0 | 137 |

- **Collections take 7.0% of the steady-state run time over all 34 workloads on 3.15.1**
  (1,417 ms of 20,334 ms), median 1.1%, and **5.5% on 3.15.1t** (1,266 of 23,084 ms),
  median 1.6%.
- **Top shares on 3.15.1:**

  | workload | GC share | collections | objects collected |
  |---|---:|---:|---:|
  | `billing_engine` | **33.8%** | 375 | **0** |
  | `pytest_networkx` | **32.4%** | 42, 6 of them full | 14,254 |
  | `pylint_stdlib` | 19.8% | 167 | 103,740 |
  | `pyflakes_stdlib` | 19.3% | 125 | 548,028 |
  | `state_snapshot` | 15.4% | 103 | 0 |
  | `xml_feeds` | 14.2% | 72 | 824 |
  | `coverage_pyflakes_tests` | 12.7% | 164 | 286,837 |
  | `black_format` | 11.1% | 158 | 208,327 |

  Several workloads pay for collections that find nothing (`billing_engine`,
  `state_snapshot`, `report_render`, `json_api`). Those collections are triggered by
  allocation counts while the live object graph grows.
- **3.15.1t collects far less often** (generation 0 only, plus 6 full collections in
  `pytest_networkx`), but each collection costs more. That shows in `tz_events`:
  48 collections take 0.6 ms on 3.15.1, and 3 collections take 20.6 ms on 3.15.1t.
- Peak RSS is 24–256 MiB on 3.15.1 (`pandas_etl` the largest). On 3.15.1t it is a median
  17% higher (+5% to +30%), except `pyflakes_stdlib` (0.77×) and `black_format` (0.80×),
  which collect less often there and peak lower.
- These are single measurements of one `run()` per workload (no replicate). Counts are
  deterministic per process (*inferred*: callback and `get_stats()` agree), and times
  are wall clock on a quiet domain.

### 8.3 CPython's own JIT (`PYTHON_JIT=1`, plain 3.15.1)

Which builds have the JIT (`machine/jit.txt`): every GIL build — 3.14.9, 3.14.10, 3.15.1,
3.15.2, 3.16.0a1 — is configured `--enable-experimental-jit=yes-off`, so
`sys._jit.is_available()` is True and the JIT is off unless `PYTHON_JIT=1`. No
free-threaded build has it.

`jit_compare.py compare <family> -n 5 -p 3` runs the workload set **plain on both sides**:
`PYTHON_JIT` unset against `PYTHON_JIT=1`. It ran on domain 3, 19:18–19:32Z, with nothing
else timing in that domain. Every child reported the JIT state it was asked for, and
every digest matched (`jit/jit-3.15.1-*.json`).

| workload | no JIT ms | PYTHON_JIT=1 ms | ratio | digest |
|---|---:|---:|---:|---|
| data/backup_archive | 599.2 | 598.8 | 0.999 | same |
| data/billing_engine | 914.0 | 899.7 | 0.984 | same |
| data/config_merge | 538.8 | 545.7 | 1.013 | same |
| data/csv_etl | 392.8 | 389.4 | 0.991 | same |
| data/fs_inventory | 315.2 | 307.1 | 0.974 | same |
| data/log_analytics | 769.3 | 758.0 | 0.985 | same |
| data/pandas_etl | 535.0 | 530.0 | 0.991 | same |
| data/report_render | 404.8 | 388.2 | 0.959 | same |
| data/state_snapshot | 637.0 | 625.2 | 0.982 | same |
| data/text_corpus | 603.3 | 608.1 | 1.008 | same |
| data/tz_events | 528.6 | 532.2 | 1.007 | same |
| data/xml_feeds | 503.5 | 488.1 | 0.970 | same |
| devtools/black_format | 1196.0 | 1233.4 | 1.031 | same |
| devtools/coverage_pyflakes_tests | 383.0 | 376.8 | 0.984 | same |
| devtools/import_startup | 1881.9 | 1885.9 | 1.002 | same |
| devtools/isort_stdlib | 283.6 | 272.6 | 0.961 | same |
| devtools/pip_resolve | 564.3 | 573.2 | 1.016 | same |
| devtools/pycodestyle_stdlib | 1011.1 | 950.6 | 0.940 | same |
| devtools/pyflakes_stdlib | 1217.1 | 1221.4 | 1.004 | same |
| devtools/pylint_stdlib | 1184.8 | 1239.7 | 1.046 | same |
| devtools/pytest_networkx | 641.0 | 654.2 | 1.021 | same |
| devtools/rich_render | 283.9 | 294.3 | 1.037 | same |
| devtools/sphinx_autodoc | 847.8 | 842.7 | 0.994 | same |
| devtools/sqlglot_transpile | 862.5 | 913.1 | 1.059 | same |
| web/aiohttp_app | 269.8 | 283.6 | 1.051 | same |
| web/asyncio_http | 232.3 | 246.0 | 1.059 | same |
| web/django_blog | 489.5 | 517.9 | 1.058 | same |
| web/email_mime | 659.2 | 700.8 | 1.063 | same |
| web/fastapi_pydantic | 296.2 | 328.4 | 1.109 | same |
| web/flask_jinja | 419.2 | 436.3 | 1.041 | same |
| web/json_api | 396.3 | 434.7 | 1.097 | same |
| web/logging_service | 282.2 | 280.4 | 0.994 | same |
| web/requests_local | 250.2 | 270.9 | 1.083 | same |
| web/sqlalchemy_orm | 341.6 | 351.6 | 1.029 | same |

- **Geometric mean 1.015**: CPython's JIT makes the workload set 1.5% *slower*. By family:
  data 0.988, devtools 1.007, **web 1.058**. Median 1.007.
- 8 workloads are above 1.05, all web services or framework-heavy:
  - `fastapi_pydantic` 1.109, `json_api` 1.097, `requests_local` 1.083;
  - `email_mime` 1.063, `asyncio_http` 1.059, `sqlglot_transpile` 1.059, `django_blog`
    1.058, `aiohttp_app` 1.051.
- One workload is below 0.95: `pycodestyle_stdlib` 0.940. Then `report_render` 0.959,
  `isort_stdlib` 0.961, `xml_feeds` 0.970, `fs_inventory` 0.974.
- So on these programs, Rustique's enabled ratio (0.877) is not something CPython's JIT
  provides today. The JIT would also not stack on it for the web family. The protocol is
  1 warm-up `run()`, then 5 timed, in fresh processes (as `run.py`), which gives a JIT a
  short warm-up; a longer one was not measured.

## 9. Step durations

From `durations.tsv` (start, end, seconds and exit code of every step; the full table is
`tables/durations.md`). Steps ran in parallel lanes, as §1 describes.

| stage | steps | each | notes |
|---|---:|---|---|
| build (fat LTO) | 10 | 72–75 s | 146–147 s on 3.16 (numpy built from source); 4 in parallel, 8 cores each |
| pytest | 10 | 224–308 s | 181 / 222 s on 3.16 (failures, §2) |
| status, wheel | 10 + 10 | 0–1 s | |
| workload venvs / pyperformance venvs | 2 / 2 | 3–4 s / 43–44 s | |
| cargo fmt / clippy / test | 3 | 1 / 13 / 24 s | |
| conformance (all groups) | 5 | 176–184 s | |
| workloads (`run.py compare`, 3 families) | 2 | 1307 / 1415 s | |
| wlstats-run | 2 | 54 / 58 s | |
| whole-process (`pyperf command`) | 2 | 565 / 664 s | |
| startup cells | 2 | 80 / 82 s | 300 rounds each |
| scoreboard `instr` (8 callgrind shards) | 2 | 619 / 669 s | |
| isolation, instr / wall time | 2 / 1 | 5–14 s / 901 s | |
| JIT comparison | 1 | 819 s | |
| pyperformance, one full run | 8 | 3772–3993 s | exit 1 by design (`genshi`) |
| port re-runs | 4 | 143–161 s | |
| GC baseline | 2 | 51 / 56 s | |
| scoreboard `time` (4 single-CPU shards) | 1 | 3415 s | |
| reports, compares, tables | 6 + 1 | 0–4 s | |

The box schedule ran **18:30–23:00Z (4 h 30 min)**:

- Wave A, builds/tests/Rust/conformance: 18:30–18:55;
- Wave B, timing in four lanes: 18:55–21:38;
- Wave C: ports and GC 21:38–21:48, scoreboard wall time and isolation 21:48–23:00;
- tables at 23:00.

The long poles were pyperformance (63–67 min per run, two per domain) and the scoreboard's
wall time (57 min on 4 shards). The box was idle, with no process of this run left, from
23:01Z.

## 10. Findings, anomalies and incidents

Findings to act on (all **measured**; causes *inferred* where tagged):

1. **On 3.16.0a1/a1t the test suite fails instead of skipping** (§2). There are 278
   failures and 1504 setup errors. Every one of them asserts `installed` for a target that
   the version gate refuses ("CPython 3.16 pre-release (0x31000a1)", category `version`).
   The build itself works: 4025 / 4028 tests pass, and nothing crashes or returns a
   different result.
2. **The GC is a measurable share of real workloads** (§8.2): 7.0% of steady-state run time
   on 3.15.1, and up to 33.8% (`billing_engine`, 375 collections that collect nothing) and
   32.4% (`pytest_networkx`). This is a wave-2 lead outside the current targets (GC timing
   is outside the acceleration contract, `docs/DECISIONS.md` §5.5). It also explains the
   largest free-threaded "speedup" (`billing_engine`).
3. **CPython's JIT slows the web workloads** (§8.3): `PYTHON_JIT=1` gives web 1.058, all
   1.015, and `fastapi_pydantic` 1.109.
4. **Free-threading outliers** (§8.1):
   - workloads: `isort_stdlib` 4.03×, `state_snapshot` 1.90×;
   - pyperformance: `bench_thread_pool` 1.37×, `dask` 1.36×, `connected_components` 1.30×;
   - pyperformance's own `deepcopy` is only 1.09–1.11×, so the 4× is specific to
     `isort_stdlib`'s use of `deepcopy` (*inferred*; not investigated).
5. **pyperformance on 3.15.1t: `xdsl_constant_fold` 1.029 / 1.035** in both pairs (§5),
   as in wave 1 (1.033 in both pairs). Still not investigated.
6. Unchanged since wave 1 and already on wave 2's list: the `RUSTIQUE=1` startup cost
   (§6: marginal 1.045 / 1.077 on 3.15.1, 1.063 / 1.107 on 3.14.9).

What reproduced (nothing regressed):

- the workload set (0.877 / 0.843), whole processes (0.754 / 0.713) and pyperformance
  (0.942 / 0.956), each within 0.003 of wave 1;
- the startup cells, within 0.003 except `import + enable()`, which is 0.16–0.18M Ir
  cheaper;
- the scoreboard: every 0–100 cell ≤ 1.05 after isolation, the same geomeans as wave 1;
- conformance: 0 differences on five builds;
- the tests: 0 failures on all eight supported builds.

Wave 1's post-Phase-3 fixes show on the box:

- the count/`in` cells now measure 1.026–1.045;
- the pytest plugin no longer loads the extension in plain processes;
- the status build is 0.16–0.18M Ir cheaper;
- `tuple.__contains__` fallbacks are now classified `args`, not `deopt`.

Not bugs, recorded so nobody chases them:

- `devtools/import_startup`'s digest differs plain vs `RUSTIQUE=1` in the whole-process
  runs, by construction (wave 1 §4.1).
- The conformance runs on 3.14.9 and 3.14.10 exit 1 on `--expect-installed` for the five
  3.15-only pathlib targets, as designed.
- Every full pyperformance run exits 1 because of `genshi` (fails on plain 3.15).
- Port benchmarks collided between concurrent runs and were re-run alone (§5).
- `isort_stdlib`'s 3.15.1t ratio moved +3.6% because its *plain* median got 3.4% faster
  (§4).

Incidents:

- **My runner's `gcstats` stage died at once** at 18:57Z, on both builds, with
  `run_w2phase0.sh: line 266: b: unbound variable`. The cause was `local b=$1 …
  out="…$b…"` in one statement under `set -u`, the same trap wave 1's `wlstats` hit.
  - `set -u` ends the whole process, so `steps.log` has the two `START` lines and no `END`.
  - The lane went on with its callgrind jobs. The fix was deployed by atomic rename.
  - Both GC baselines ran at 21:38–21:40Z on domain 0, an untimed lane, alongside the
    port re-runs on domain 1 (`gc_after_b.sh`).
- **The lead asked at ~21:30Z to free the box by 23:30Z.**
  - Both replicate-B pyperformance pairs had already started by then. The lead asked to
    let running runs finish, so both builds have two pairs.
  - The **3.15.1t scoreboard wall time was dropped**, through a skip marker deployed into
    the running schedule (`skip/time-3.15.1t`, `skip/isolate-time-3.15.1t`; `steps.log`
    shows `SKIP`).
- Light activity outside the measured processes:
  - The helper scripts were smoke-tested on CPUs 4–7 during Wave A, before any timing.
  - During timing, a results pull ran every 3 min (`rsync`, `nice -n 19`, pinned to CPU 0)
    and one `ssh tail -F` followed `steps.log`. This is the same practice as wave 1, and
    no measured process shared a CPU with them.
  - One 2-second instr summary ran on CPU 0 at ~19:08Z, while domain 0 carried only
    callgrind jobs.

## 11. Files

`benches/results/w2-phase0/`:

- `run_w2phase0.sh` runs every stage and can be rerun (`./run_w2phase0.sh <stage> ...`;
  `all` is the schedule used, `stage_all`). Wave 1's helpers are reused from the clone:
  `phase3/whole.py`, `phase3/wlstats_run.py`, `phase0/sitecustomize.py`,
  `phase0/analyze_phase0.py`, `phase3/analyze_phase3.py`.
- New helpers:
  - `status_dump.py`, `gcstats.py`, `jit_compare.py`, `sb_isolate.py` (§1);
  - `analyze_w2phase0.py` and `tables.sh`, which build the tables in `tables/`. The
    scoreboard comparison of §7 ran locally with a stub `rustique` module, because only
    `benches/scoreboard.py`'s cell definitions are needed. It ran again on the box in
    `tables.sh`.
- `steps.log`, `durations.tsv` — every step with start/end/seconds/exit code.
- `logs/` — one log per step.
- `machine/` — interpreters, CPU, IRQs, JIT availability.
- `tests/` — JUnit XML and freezes per build.
- `status/status-<build>.json` — the target status of §2.
- `conformance/all-groups-<build>.json`.
- `workloads/` — `compare-<build>-<family>.json`, `stats-run-<build>/`,
  `freeze-<build>-<family>.txt`.
- `whole/` — `whole-<build>.json`, `digests-<build>/`.
- `gc/gc-<build>.jsonl`.
- `jit/jit-3.15.1-<family>.json`.
- `pyperformance/` — `{plain,enabled}-<build>[-B|-ports].json`, `compare-*.md`,
  `analysis-*.txt`, `bench-venv-freeze-*.txt`.
- `startup/cells-<build>-{bare,pth}.json`.
- `scoreboard/` — `instr-enabled-*.json`, `time-enabled-3.15.1.json.shard*.json`,
  `enabled-*.md`, `summary-*`, `analysis-*`, `compare-phase3-*`, `isolated-*.txt`.
- `tables/` — the generated tables.

Files over 1 MB are gzipped. The box keeps everything under `/work/w2p0/`: clones
`w/<build>`, venvs `venv/<build>`, workload venvs `wl/<build>-<family>`, pyperformance venvs
`pyperf/<build>`, wheels `dist/<build>/`, startup venvs `su/<build>/`.
