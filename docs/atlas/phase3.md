# Phase 3 — integrate and prove (wave 1 at `11bad29`)

Rustique `wave1` at **`11bad29`** ("Merge branch 'ws/paths' into wave1"), fat-LTO release
builds, measured on the same AWS c7a.8xlarge as Phase 0 on 2026-11-01/02. Everything here is
**measured** on that box unless tagged *inferred*. "Enabled" = every target group on
(`rustique.enable()`, or `RUSTIQUE=1` through the shipped `.pth` where it says so); "plain" =
the same venv and interpreter without Rustique enabled. Raw data, logs and the rerunnable
script: `benches/results/phase3/` (§12).

## Summary

All results are measured on the c7a.8xlarge with every group on, at `11bad29`.

**Real workloads (§4)**

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| 34 workloads, geometric mean (replicate 1 / 2) | **0.880 / 0.879** (1.14× faster) | **0.843 / 0.843** (1.19×) |
| data scripts | 0.773 | 0.756 |
| web services | 0.922 | 0.933 |
| developer tools | 0.963 | 0.863 |
| 6 whole processes with `RUSTIQUE=1`, startup and imports included (§4.1) | **0.756** | **0.712** |

- Best: `report_render` 0.436 (3.15.1) and `isort_stdlib` 0.196 (3.15.1t).
- No workload is above 1.05. The worst is 1.013, which measures 1.003 in the second
  replicate.
- The two replicates agree within 2% on every workload.
- Every result digest is identical to plain CPython's.

**pyperformance, full suite (§5).** Geometric mean **0.942** on 3.15.1 and **0.957** on
3.15.1t, averaged over two replicate pairs. Phase 0 measured 1.002.

- `deepcopy` 0.13–0.28, `json_dumps` 0.25–0.33, `json_loads` 0.57–0.59, `logging_*`
  0.61–0.79, `sympy_integrate` 0.83.
- Only `python_startup` is above 1.05 in both pairs: 1.08–1.10, against 1.18 in Phase 0.

**Correctness**

- CPython's own test modules for every group, with all 15 groups on (§3): **0 differences**
  in 8,125 test outcomes on 3.15.1 and 3.15.1t, and in 7,823 on 3.14.9.
- The full test suite passes on all four builds (§2).
- Every scoreboard result is identical to CPython's (§7).
- One segfault in 9 repeated test runs, 3.15.1 run 2 (§2). It is the `json.loads`
  use-after-free on documents that repeat a key, which CI had found and which was fixed
  after this run (`dac4ab9`).
  - Memcheck reproduces it on `11bad29` and is clean on the fix.
  - The other 8 runs passed.

**Exceedances**

- **Scoreboard at 0–100 elements:** 3811 of 3814 cells ≤ 1.05 after the isolation
  re-runs. The remaining 3 are kernel-path `count`/`in` cells with 1-element object
  lists (1.050–1.066), since fixed to 1.026–1.045 (§7.1, §11).
- **Wall time at 10k/1M** (§7.2): kernel-path geomean 0.462 / 0.482, the same as
  Phase 0. As in Phase 0, single cells at 3 processes are noise-dominated. The 8
  kernel-path cells that read above 1.05 measure 0.967–1.040 alone, at 10 processes, twice
  per build on both builds.
- **Startup:** `RUSTIQUE=1` costs 1.045× instructions / 1.078× wall on `python -c pass`
  (3.15.1) and 1.063 / 1.108 on 3.14.9 (§6). Installed but off, every cell is ≤ 1.05.
- **pyperformance:** `xdsl_constant_fold` is 1.033 on 3.15.1t in both pairs.

**Per-target spot checks (§8):** every cell ≤ 1.05, and the headline cells reproduce the
target docs.

**What changed after the run (§11):**

- the 3 scoreboard cells;
- the pytest plugin, which no longer loads the extension;
- the `json.loads` crash;
- Windows linking;
- platform refusals.

## 1. What was run

**Box.** The Phase 0 box (`docs/atlas/phase0.md` §1–2): AWS c7a.8xlarge, AMD EPYC 9R14, 32
cores without SMT, 4 L3 domains (0–7, 8–15, 16–23, 24–31), 61 GiB, Ubuntu 24.04, kernel
`7.0.0-1014-aws`, no frequency control visible to the guest (boost on, ~3.65–3.68 GHz under
load). Machine facts were recorded in Phase 0 and not re-measured.

**Toolchain** (`logs/setup.log`, `machine/pythons.txt`, `tests/freeze-*.txt`,
`workloads/freeze-*.txt`): identical to Phase 0 — Rust 1.99.1 (LLVM 23.1.1), clippy 0.1.99,
rustfmt 1.10.0, uv 0.13.5, python-build-standalone `20261021` CPython 3.15.1, 3.15.1t, 3.14.9,
3.14.9t (same `python -VV` as Phase 0), PyO3 0.30.1, maturin 1.15.0, pytest 9.1.1,
hypothesis 6.170.9, numpy 2.5.4, pyperf 2.10.0, pyperformance 1.14.0 (Phase 0's driver and
benchmark venvs, reused), valgrind 3.22.0, CPython `Lib/test` from tags `v3.15.1` / `v3.14.9`.

**Builds.** One clone, venv and `CARGO_TARGET_DIR` per interpreter (under `~/phase3`);
`maturin develop --uv --release` (fat LTO, one codegen unit, 70–73 s), then
`maturin build --release` in the same target directory, which reuses the same artifact
(0.04 s): the wheel's `_rustique.*.so` (6.8 MB, `rustique-0.2.0-cp315-cp315-manylinux_2_34_x86_64.whl`
etc.) is the binary the tests ran. Wheels were installed (`uv pip install` / `pip install`)
into the workload venvs and the startup venvs, so the shipped `_rustique_startup.pth` is the
one measured. 15 groups / 84 targets on 3.15.1 (`builtins, containers, copy, dataclasses,
datetime, float, fnmatch, json, logging, pathlib, posixpath, random, statistics, textwrap,
urllib`).

**Commands** — what `benches/results/phase3/run_phase3.sh` runs (stage names in brackets):

```bash
# [build/wheel/test] per build b in 3.15.1 3.15.1t 3.14.9 3.14.9t
maturin develop --uv --release; maturin build --release -o dist/$b
python -m pytest -q -p no:cacheprovider --durations=25 --junitxml=...
# [rust] once (PYO3_PYTHON = 3.15.1)
cargo fmt --all --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace
# [conformance] 3.15.1, 3.15.1t, 3.14.9: all 15 groups at once, 27 CPython test modules
CPYTHON_SRC=... python tests/conformance/run_target_suites.py --targets <all groups> \
    --modules test.test_list,...,test.test_statistics --expect-installed <all 84 targets>
# [wlvenv] one venv per build and family (data/devtools pins conflict: python-dateutil 4.0.0 vs boto3's < 3)
uv pip install -r benches/workloads/<family>/requirements.txt pyperf==2.10.0; uv pip install dist/$b/*.whl
# [workloads] per family, pinned to one L3 domain
taskset -c <domain> python benches/workloads/run.py compare <family> -n 5 -p 5 -o ...
# [wlstats-run] counters of one steady-state run() per workload (wlstats_run.py), and
# [wlstats] the whole process once with RUSTIQUE_REPORT=<file>.json (setup + warm-up + run)
# [whole] 6 workloads, one setup()+run() per process (whole.py), plain vs RUSTIQUE=1, 4 rounds alternating order
python -m pyperf command --affinity <domain> -p 5 -n 1 -w 1 --inherit-environ RUSTIQUE,WHOLE_DIGEST --append ...
# [pyperformance] Phase 0's method: PYTHONPATH=<clone>/python:<dir with sitecustomize.py> RUSTIQUE_ENABLE=0|1
pyperformance run --python <cpython> --inherit-environ PYTHONPATH,RUSTIQUE_ENABLE --affinity <domain> -o ...
# [startup] release wheel pip-installed into a fresh `python -m venv` vs an identical venv without it
taskset -c 30 python benches/dx/startup.py cells PLAIN_VENV RUSTIQUE_VENV -r 300 --ir-runs 3   # bare, and both with another .pth
# [instr/time/report/isolate] scoreboard, enabled tier, as Phase 0
python benches/scoreboard.py instr --impls enabled --shards 8 ...; ... time --size 1e4,1e6 --shards 4 --affinity 2,10,20,30
# [spot] benches/targets/{json_cg,copy_cells,datetime_cg,paths_cg,logging_cg}.py, utils_cg.py random_cells
```

**Pinning.** Builds, tests and conformance used all cores before any timing started. Timing
jobs: at most one per L3 domain (workloads 3.15.1 on 8–15, 3.15.1t on 16–23; whole-process
and startup on 24–31 / CPU 30; pyperformance one run per domain); domain 0 carried only
untimed or callgrind jobs (stats runs, scoreboard `instr`, isolation re-runs, spot checks)
until the second pyperformance replicate moved there. The scoreboard's wall-time shards ran after every
pyperformance run had finished, one shard per domain on single CPUs 2, 10, 20 and 30 (as in
Phase 0); the last wave (port benchmarks alone, the repeated test runs, a second workload
replicate) kept the same rule: timing jobs on domains 1–2, untimed ones on 0 and 3.

## 2. Builds and tests

Fat-LTO release builds succeed on all four interpreters (70–73 s each, 4 in parallel with 8
cores each). `pytest -q` (JUnit XML in `tests/`):

| build | tests | passed | failed | skipped | time |
|---|---:|---:|---:|---:|---:|
| 3.15.1 | 5822 | 5798 | 0 | 24 | 217.7 s |
| 3.15.1t | 5822 | 5801 | 0 | 21 | 287.6 s |
| 3.14.9 | 5822 | 5799 | 0 | 23 | 231.1 s |
| 3.14.9t | 5822 | 5802 | 0 | 20 | 265.8 s |

Skips are by design: 12 × `test_takeover_conformance` (needs `CPYTHON_SRC`), 5 that need
`rustique_demo` (not built in this run; Phase 0 showed they pass once it is), 1 that needs
`pytest-xdist` (`tests/test_dx_cli.py:356`, not installed), the free-threaded-only tests on
GIL builds, and 3.14-only skips (copy's mutable type sets, `frozendict`).

**Rust** (once, `PYO3_PYTHON` = 3.15.1): `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace`
**173 passed**, 0 failed (rustique-core 148 unit + `kernels_cpython` 4 + `float_repr_cpython`
1, rustique-py 17, rustique-api 2 + 1 doctest; 4 doctests ignored in rustique-py). Phase 0: 120.

**Repeated test runs** (stage `pytest-repeat`, after pyperformance). This hunts the crash CI
had seen. The same builds ran `pytest -q` three more times each on 3.15.1 and 3.15.1t, with
fresh Hypothesis seeds. Domains 0 and 3 were used, alongside the second workload replicate.

| build | run 1 | run 2 | run 3 |
|---|---|---|---|
| 3.15.1 | 5798 passed, 24 skipped (211 s) | **segfault after 29 s** | 5798 passed, 24 skipped (212 s) |
| 3.15.1t | 5801 passed, 21 skipped (278 s) | 5801 passed, 21 skipped (279 s) | 5801 passed, 21 skipped (282 s) |

**The crash is the `json.loads` use-after-free fixed after Phase 3** (`dac4ab9`, §11).

- **Where:** `tests/targets/test_json_loads.py::test_hypothesis_roundtrip_documents`, with
  the C stack in `PyUnicode_New` under the json decoder (`logs/pytest-repeat-3.15.1-2.log`).
- **How a round trip repeats a key:** the strategy's text includes both `'😀'`
  (two lone surrogates) and `'\U0001F600'`. In a Python dict these are two different keys,
  but `json.dumps` (ASCII output) writes both as `"😀"`. The document therefore
  repeats a key. The second value replaces the first, which frees the first value's subtree
  while `11bad29`'s per-call key memo still holds borrowed pointers to that subtree's keys.
- **Reproduced afterwards, measured.** The test document was
  `{'😀': {'inner_key': [1, 'xxx']}, '\U0001F600': 2, 'inner_key': 3}`, dumped
  and loaded under valgrind memcheck with json on (`PYTHONMALLOC=malloc`).
  - With the `11bad29` build on this box, memcheck reports an invalid read in
    `Parser::object` of a key that `PyDict_SetItem` → `dict_dealloc` → `unicode_dealloc`
    freed.
  - With the fixed build (dev box, `4c53e96`) there are 0 errors, and `json.loads` has
    1 hit and 0 fallbacks.
  - A one-character key such as `"k"` does not reproduce it, because CPython caches those
    strings and freeing the inner dict never frees them.

No other run failed. The decoder is the same code on 3.15.1t, so that build has the bug
too. Its three runs did not crash. A read of freed memory crashes only when the block has
been reused in between, and that was not investigated per build.

## 3. Conformance, all groups at once

`run_target_suites.py` with **all 15 groups enabled together** (`_takeover.enable(only=<all
groups>)` before each test module is imported), each module plain and enabled in fresh
processes, outcomes compared per test id:

| module | 3.15.1 | 3.15.1t | 3.14.9 |
|---|---|---|---|
| test_builtin | 150 pass, 7 skip; **0 diff** | 150 pass, 7 skip; **0 diff** | 141 pass, 6 skip; **0 diff** |
| test_copy | 84 pass; **0 diff** | 84 pass; **0 diff** | 81 pass; **0 diff** |
| test_copyreg | 6 pass; **0 diff** | 6 pass; **0 diff** | 6 pass; **0 diff** |
| test_dataclasses | 284 pass, 1 skip; **0 diff** | 284 pass, 1 skip; **0 diff** | 279 pass, 1 skip; **0 diff** |
| test_datetime | 3614 pass, 54 skip; **0 diff** | 3614 pass, 54 skip; **0 diff** | 3560 pass, 54 skip; **0 diff** |
| test_float | 52 pass, 2 skip; **0 diff** | 52 pass, 2 skip; **0 diff** | 52 pass, 2 skip; **0 diff** |
| test_fnmatch | 24 pass; **0 diff** | 24 pass; **0 diff** | 24 pass; **0 diff** |
| test_format | 17 pass, 2 skip; **0 diff** | 17 pass, 2 skip; **0 diff** | 17 pass, 2 skip; **0 diff** |
| test_fstring | 95 pass; **0 diff** | 95 pass; **0 diff** | 94 pass; **0 diff** |
| test_genericpath | 26 pass; **0 diff** | 26 pass; **0 diff** | 25 pass; **0 diff** |
| test_glob | 20 pass, 2 skip; **0 diff** | 20 pass, 2 skip; **0 diff** | 22 pass, 2 skip; **0 diff** |
| test_json | 232 pass, 1 skip; **0 diff** | 232 pass, 1 skip; **0 diff** | 220 pass, 1 skip; **0 diff** |
| test_list | 68 pass, 1 skip; **0 diff** | 68 pass, 1 skip; **0 diff** | 67 pass, 1 skip; **0 diff** |
| test_logging | 283 pass, 4 skip; **0 diff** | 283 pass, 4 skip; **0 diff** | 274 pass, 3 skip; **0 diff** |
| test_os | 472 pass, 77 skip, 3 fail, 1 error; **0 diff** | 472 pass, 77 skip, 3 fail, 1 error; **0 diff** | 317 pass, 66 skip, 2 fail; **0 diff** |
| test_pathlib | 991 pass, 407 skip; **0 diff** | 991 pass, 407 skip; **0 diff** | 966 pass, 403 skip; **0 diff** |
| test_posixpath | 93 pass, 3 skip; **0 diff** | 93 pass, 3 skip; **0 diff** | 91 pass, 3 skip; **0 diff** |
| test_random | 114 pass, 2 skip; **0 diff** | 114 pass, 2 skip; **0 diff** | 114 pass, 2 skip; **0 diff** |
| test_sort | 21 pass; **0 diff** | 21 pass; **0 diff** | 21 pass; **0 diff** |
| test_statistics | 400 pass; **0 diff** | 400 pass; **0 diff** | 400 pass; **0 diff** |
| test_strptime | 94 skip, 49 pass; **0 diff** | 94 skip, 49 pass; **0 diff** | 94 skip, 46 pass; **0 diff** |
| test_strtod | 8 pass, 1 skip; **0 diff** | 8 pass, 1 skip; **0 diff** | 8 pass, 1 skip; **0 diff** |
| test_textwrap | 68 pass; **0 diff** | 68 pass; **0 diff** | 68 pass; **0 diff** |
| test_time | 49 pass, 16 skip; **0 diff** | 49 pass, 16 skip; **0 diff** | 48 pass, 16 skip; **0 diff** |
| test_tuple | 38 pass; **0 diff** | 38 pass; **0 diff** | 38 pass; **0 diff** |
| test_urllib | 104 pass, 2 skip; **0 diff** | 104 pass, 2 skip; **0 diff** | 104 pass, 2 skip; **0 diff** |
| test_urlparse | 78 pass, 5 skip; **0 diff** | 78 pass, 5 skip; **0 diff** | 74 pass, 5 skip; **0 diff** |

**0 differences on all three builds** (8125 test outcomes on 3.15.x, 7823 on 3.14.9). The
`test_os` failures/error are the box's environment and identical plain and enabled (as in
the paths docs). On 3.15.1 and 3.15.1t every one of the 84 targets ended up installed. On
3.14.9 the runner reports 5 targets as not installed — `pathlib.PurePath.{name, parent,
parts, stem, suffix}` — which is by design: those five are validated for 3.15 only
(`docs/targets/pathlib.md`), and `rustique.enable()` on 3.14.9 reports them `refused`
("CPython 3.14 not validated for this target version", checked separately). The runner shows
them as "pending-import" because it keeps the state of the first module that reported a
target unless a later one reports `installed`, so a later `refused` never replaces it (a
reporting nit of the runner, not of Rustique). 3.14.9t was not run (time was kept for steps
3–6; the per-group runs in the target docs cover it).

## 4. The workload set (the headline)

`benches/workloads/run.py compare <family> -n 5 -p 5`: per workload 5 interleaved pairs of
fresh processes (plain, enabled), each process `setup()` + 1 warm-up + 5 timed `run()`s;
median of the 25 timed runs per side. `enable()` is called before the workload module is
imported. Every digest matched: **0 mismatches** in 2 × 34 workloads, in the second replicate
below, and in the whole-process runs (§4.1).

**Second replicate** (same command, after pyperformance; `workloads/compare-<build>-<family>-2.json`,
`tables/workloads-2.md`):

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| geomean | **0.879** | **0.843** |
| data / devtools / web | 0.771 / 0.966 / 0.919 | 0.755 / 0.863 / 0.934 |
| worst | `xml_feeds` 1.007 | `email_mime` 1.009 |

- Every workload's ratio is within 2% of the first replicate: 29 / 33 of 34 are within 1%.
- The geomean of replicate 2 ÷ replicate 1 is 0.9995 / 1.0003, and the plain medians agree
  to 0.08%.
- `config_merge` is 1.003 / 0.993 here, against 1.013 / 1.001 in the first replicate. That
  was noise, as read below.
- Averaged over both replicates the overall geomean is 0.879 / 0.843. The worst workloads
  are `config_merge` 1.008 (3.15.1) and `xml_feeds` 1.006 (3.15.1t).

The tables below are from the first replicate.

Per workload (ms = median steady-state `run()`; ratio = enabled ÷ plain):

| workload | 3.15.1 plain ms | 3.15.1 enabled ms | 3.15.1 ratio | 3.15.1t plain ms | 3.15.1t enabled ms | 3.15.1t ratio |
|---|---:|---:|---:|---:|---:|---:|
| data/backup_archive | 599.9 | 597.6 | 0.996 | 607.0 | 606.3 | 0.999 |
| data/billing_engine | 918.1 | 890.4 | 0.970 | 758.1 | 738.9 | 0.975 |
| data/config_merge | 536.8 | 543.6 | 1.013 | 607.3 | 607.9 | 1.001 |
| data/csv_etl | 388.1 | 242.4 | 0.625 | 432.5 | 276.9 | 0.640 |
| data/fs_inventory | 315.3 | 234.2 | 0.743 | 320.6 | 235.6 | 0.735 |
| data/log_analytics | 766.7 | 455.1 | 0.594 | 807.1 | 491.2 | 0.609 |
| data/pandas_etl | 530.6 | 530.8 | 1.000 | 592.5 | 592.5 | 1.000 |
| data/report_render | 405.6 | 176.6 | 0.436 | 464.3 | 196.3 | 0.423 |
| data/state_snapshot | 648.1 | 422.0 | 0.651 | 1248.7 | 593.8 | 0.476 |
| data/text_corpus | 604.5 | 604.5 | 1.000 | 631.7 | 632.0 | 1.001 |
| data/tz_events | 525.7 | 310.8 | 0.591 | 614.0 | 377.3 | 0.615 |
| data/xml_feeds | 505.8 | 505.7 | 1.000 | 569.6 | 572.6 | 1.005 |
| devtools/black_format | 1197.9 | 1186.2 | 0.990 | 1293.1 | 1292.0 | 0.999 |
| devtools/coverage_pyflakes_tests | 382.2 | 380.1 | 0.995 | 494.5 | 493.9 | 0.999 |
| devtools/import_startup | 1899.3 | 1898.6 | 1.000 | 2246.2 | 2248.2 | 1.001 |
| devtools/isort_stdlib | 283.7 | 218.8 | 0.771 | 1187.4 | 232.8 | 0.196 |
| devtools/pip_resolve | 562.0 | 533.6 | 0.949 | 624.9 | 591.8 | 0.947 |
| devtools/pycodestyle_stdlib | 1012.7 | 997.0 | 0.985 | 1067.9 | 1067.0 | 0.999 |
| devtools/pyflakes_stdlib | 1211.5 | 1208.2 | 0.997 | 1194.4 | 1200.1 | 1.005 |
| devtools/pylint_stdlib | 1181.5 | 1184.2 | 1.002 | 899.0 | 898.1 | 0.999 |
| devtools/pytest_networkx | 632.3 | 599.6 | 0.948 | 632.7 | 603.7 | 0.954 |
| devtools/rich_render | 285.0 | 284.4 | 0.998 | 310.3 | 310.3 | 1.000 |
| devtools/sphinx_autodoc | 858.9 | 839.3 | 0.977 | 898.9 | 888.2 | 0.988 |
| devtools/sqlglot_transpile | 879.4 | 851.4 | 0.968 | 1030.0 | 1003.2 | 0.974 |
| web/aiohttp_app | 270.9 | 246.7 | 0.910 | 301.7 | 279.1 | 0.925 |
| web/asyncio_http | 230.9 | 182.6 | 0.791 | 240.5 | 196.0 | 0.815 |
| web/django_blog | 487.3 | 485.6 | 0.997 | 544.6 | 541.7 | 0.995 |
| web/email_mime | 654.5 | 651.4 | 0.995 | 708.8 | 709.8 | 1.001 |
| web/fastapi_pydantic | 296.2 | 291.1 | 0.983 | 338.8 | 334.0 | 0.986 |
| web/flask_jinja | 419.3 | 415.0 | 0.990 | 463.5 | 457.2 | 0.986 |
| web/json_api | 396.8 | 331.3 | 0.835 | 442.0 | 369.2 | 0.835 |
| web/logging_service | 283.5 | 231.6 | 0.817 | 300.8 | 259.8 | 0.864 |
| web/requests_local | 250.3 | 235.5 | 0.941 | 251.3 | 237.2 | 0.944 |
| web/sqlalchemy_orm | 343.6 | 342.4 | 0.997 | 352.7 | 354.3 | 1.005 |

Summary:

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| geomean data | 0.773 (12) | 0.756 (12) |
| geomean devtools | 0.963 (12) | 0.863 (12) |
| geomean web | 0.922 (10) | 0.933 (10) |
| **geomean all** | **0.880** (34) | **0.843** (34) |
| median | 0.980 | 0.986 |
| best | data/report_render 0.436 | devtools/isort_stdlib 0.196 |
| worst | data/config_merge 1.013 | data/xml_feeds 1.005 |

Reading:

- **Overall: 0.880 on 3.15.1, 0.843 on 3.15.1t** (geometric mean of 34 workloads; median
  0.980 / 0.986). Data scripts gain most (0.773 / 0.756), web services 0.922 / 0.933,
  developer tools 0.963 / 0.863. 14 / 13 workloads are at ≤ 0.95; **none is above 1.05**;
  the worst are `data/config_merge` 1.013 (3.15.1; 1.001 on 3.15.1t) and `data/xml_feeds`
  1.005 (3.15.1t; 1.000 on 3.15.1).
- **Best:** `data/report_render` 0.436 / 0.423 (`random` + `textwrap` + `list.count`),
  `devtools/isort_stdlib` 0.771 / **0.196**, `data/state_snapshot` 0.651 / 0.476,
  `data/tz_events` 0.591 / 0.615, `data/log_analytics` 0.594 / 0.609, `data/csv_etl` 0.625 /
  0.640, `data/fs_inventory` 0.743 / 0.735, `web/asyncio_http` 0.791 / 0.815,
  `web/logging_service` 0.817 / 0.864, `web/json_api` 0.835 / 0.835.
- **3.15.1t isort and state_snapshot.** Plain `isort_stdlib` takes 1187 ms on 3.15.1t against
  284 ms on 3.15.1 (4.2×), and plain `state_snapshot` 1249 ms against 648 ms; enabled they
  take 233 / 594 ms, close to the GIL build. Both are `copy.deepcopy`-heavy (isort: 26,265
  deep copies per run; state_snapshot: one deep copy of a large object graph per run, plus 640
  `copy.copy` and 20 `asdict`): CPython's pure-Python `deepcopy` is far
  slower on the free-threaded build in these workloads and the kernel removes most of it
  (*inferred* from the counters and the two workloads; the cause of the slow plain
  free-threaded `deepcopy` was not investigated).
- **Workloads that run (almost) no target code stay at 1.00 ± 0.01**: `pandas_etl` 1.000 /
  1.000, `text_corpus` 1.000 / 1.001, `xml_feeds` 1.000 / 1.005, `backup_archive` 0.996 /
  0.999, `pylint_stdlib`, `pyflakes_stdlib`, `rich_render`, `django_blog`, `email_mime`,
  `sqlalchemy_orm` 0.995–1.005. `run.py` reports medians only (no per-workload spread); these
  null workloads put the run-to-run noise of a median ratio at about ±1% (*inferred*), so
  `config_merge` 1.013 is at the edge of noise. Its run calls `list.sort` 2,123 times with
  530 fallbacks, `isoformat` 727 times and `json.loads` 240 times: a few hundred µs of
  kernel work at most (*inferred*). The second replicate measures it at 1.003 / 0.993.
- `devtools/import_startup` is 1.000 by construction here: only the parent calls
  `enable()`; its 21 child processes run plain. The whole-process run (§4.1) measures it
  with `RUSTIQUE=1` reaching every child.
- **Against the per-target docs** (one group at a time, dev box, `-p 3`): the all-groups
  results reproduce or improve on every claim — `report_render` 0.449 → 0.436,
  `log_analytics` 0.617 (datetime only) → 0.594, `tz_events` 0.593 → 0.591, `csv_etl` 0.799
  (json) / 0.823 (datetime) → 0.625 (both), `fs_inventory` 0.742 → 0.743, `state_snapshot`
  0.675 (copy) → 0.651, `isort_stdlib` 0.763 → 0.771, `json_api` 0.916 (json) / 0.961 (copy) →
  0.835, `logging_service` 0.830 → 0.817, `asyncio_http` 0.789 → 0.791, `aiohttp_app` 0.906 →
  0.910, `requests_local` 0.930 → 0.941, `pip_resolve` 0.949 → 0.949, `sqlglot_transpile`
  0.971 → 0.968, `billing_engine` 0.997 → 0.970 — except `pytest_networkx`, 0.908 claimed
  (paths) and **0.948 / 0.954** here.

### 4.1 Whole-process timing (`pyperf command`, plain vs `RUSTIQUE=1`)

One `setup()` + one `run()` per process (`benches/results/phase3/whole.py`), interpreter
start and imports included; `RUSTIQUE=1` enables through the wheel's `.pth`. 4 rounds of 5
processes per variant, the order alternating per round (20 values per variant), pinned to
domain 3:

| build | workload | plain s (mean ± std) | RUSTIQUE=1 s | ratio | values |
|---|---|---:|---:|---:|---:|
| 3.15.1 | data/log_analytics | 0.969 ± 0.008 | 0.619 ± 0.003 | 0.639 | 20/20 |
| 3.15.1 | data/report_render | 0.451 ± 0.002 | 0.223 ± 0.001 | 0.494 | 20/20 |
| 3.15.1 | data/state_snapshot | 0.893 ± 0.009 | 0.669 ± 0.005 | 0.749 | 20/20 |
| 3.15.1 | devtools/import_startup | 1.917 ± 0.006 | 1.959 ± 0.005 | 1.022 | 20/20 |
| 3.15.1 | devtools/pytest_networkx | 1.081 ± 0.006 | 1.051 ± 0.014 | 0.972 | 20/20 |
| 3.15.1 | web/json_api | 0.655 ± 0.002 | 0.520 ± 0.006 | 0.794 | 20/20 |
| 3.15.1 | **geomean** | | | **0.756** | |
| 3.15.1t | data/log_analytics | 1.029 ± 0.009 | 0.664 ± 0.002 | 0.645 | 20/20 |
| 3.15.1t | data/report_render | 0.504 ± 0.002 | 0.242 ± 0.001 | 0.481 | 20/20 |
| 3.15.1t | data/state_snapshot | 1.471 ± 0.006 | 0.765 ± 0.004 | 0.520 | 20/20 |
| 3.15.1t | devtools/import_startup | 2.278 ± 0.006 | 2.326 ± 0.005 | 1.021 | 20/20 |
| 3.15.1t | devtools/pytest_networkx | 1.225 ± 0.008 | 1.191 ± 0.005 | 0.973 | 20/20 |
| 3.15.1t | web/json_api | 0.710 ± 0.003 | 0.575 ± 0.003 | 0.810 | 20/20 |
| 3.15.1t | **geomean** | | | **0.712** | |

- Digests are identical plain vs `RUSTIQUE=1` for 5 of 6 workloads, and every enabled
  process had `rustique._rustique` loaded. `devtools/import_startup`'s digest differs **by
  construction**, not because a result changed: its result is `len(sys.modules)` in each of
  21 child processes, and with `RUSTIQUE=1` every child also loads `rustique._rustique`
  (+1 module in every child, checked: 34 → 35 for bare `python`, 318 → 319 for `requests`,
  …). No minimal repro is needed; the workload's digest is not invariant under the switch.
- `import_startup` 1.022 / 1.021: 21 child processes (`python -c "import X"`), each paying
  the `RUSTIQUE=1` startup (≈ +2 ms per child on a ≈ 91 ms average child, *inferred* from
  1.917 → 1.959 s / 21); in line with §6.
- Whole-process ratios differ from the in-process ones (§4) where setup, imports and the
  first, not-yet-warm run weigh in: `state_snapshot` 0.749 vs 0.651 (3.15.1) and 0.520 vs
  0.476 (3.15.1t), `report_render` 0.494 vs 0.436, `log_analytics` 0.639 vs 0.594,
  `pytest_networkx` 0.972 vs 0.948; `json_api` is better whole-process (0.794 vs 0.835)
  because its `setup()` is also accelerated.
- **`pytest` loads Rustique even when it is off.** In the *plain* `pytest_networkx`
  processes `rustique._rustique` was loaded too: pytest autoloads the `pytest11` entry point
  `rustique.pytest_plugin`; the plugin defers its own `import rustique` into its hooks, but
  as a submodule of the package its import runs `rustique/__init__.py`, which imports the
  extension (`from rustique import _rustique`, line 36) — in every pytest process of a venv
  where the wheel is installed, without `--rustique` (`python -X importtime -m pytest` lists
  `rustique` and `rustique._rustique`).
  By the `import rustique` startup cell (§6) that is ≈ +1.56M Ir / +0.65 ms per pytest
  process (*inferred*), outside the "installed but off ≤ 1.05" cells, which only cover
  `python -c pass`. **Flagged for dx** (e.g. a plugin module that does not import the package
  until `--rustique` is given).

### 4.2 Which targets ran (counters)

`wlstats_run.py`: `enable()`, counters on, `setup()`, one warm-up `run()`, counters reset,
one `run()`, then `rustique.stats()` — so these are the calls of one steady-state `run()`
(3.15.1; 3.15.1t has identical counters in 32 of 34 workloads). Top 3 targets by hits, with
fallbacks by reason; full records in `workloads/stats-run-<build>/*.json`, and whole-process
records (setup + warm-up + run, via `RUSTIQUE_REPORT`) in `workloads/stats-<build>/`.

| workload | targets hit (hits, fallbacks by reason) | installed / pending / refused |
|---|---|---|
| data/backup_archive | tuple.__contains__ 3,136 (fb args 780); builtins.max 2,086; builtins.sum 1,040; +6 more | 49 / 35 / 0 |
| data/billing_engine | builtins.min 230,327; list.sort 1 (fb args 2); statistics.stdev 1; +3 more | 38 / 46 / 0 |
| data/config_merge | list.sort 2,123 (fb args 530); datetime.datetime.isoformat 727; json.loads 240; +3 more | 40 / 44 / 0 |
| data/csv_etl | tuple.__contains__ 40,000; json.dumps 32,690; datetime.date.strftime 32,689; +5 more | 43 / 41 / 0 |
| data/fs_inventory | fnmatch.fnmatch 69,193; tuple.__contains__ 47,092; posixpath.join 24,590; +12 more | 47 / 37 / 0 |
| data/log_analytics | datetime.datetime.strptime 45,000; datetime.date.strftime 45,000; urllib.parse._urlsplit 19,513; +5 more | 50 / 34 / 0 |
| data/pandas_etl | tuple.__contains__ 866 (fb args 55, deopt 5); datetime.date.strftime 19; list.__contains__ 17; +11 more | 79 / 5 / 0 |
| data/report_render | random.randint 60,000; list.count 40,000; random.sample 40,000; +11 more | 38 / 46 / 0 |
| data/state_snapshot | datetime.datetime.isoformat 3,924; list.sort 1,200; float.repr 1,200; +7 more | 44 / 40 / 0 |
| data/text_corpus | builtins.min 55,395; list.sort 0 (fb args 3,003); builtins.sorted 0 (fb args 3,001); +1 more | 31 / 53 / 0 |
| data/tz_events | datetime.date.strftime 122,593; builtins.max 87,192; datetime.datetime.isoformat 24,000; +9 more | 56 / 28 / 0 |
| data/xml_feeds | list.sort 73,832 (fb args 4); tuple.__contains__ 1; builtins.sorted 0 (fb args 73,836); +1 more | 52 / 32 / 0 |
| devtools/black_format | tuple.__contains__ 302,188 (fb deopt 13,442); list.sort 24,488 (fb args 8); builtins.min 8,105; +5 more | 72 / 12 / 0 |
| devtools/coverage_pyflakes_tests | tuple.__contains__ 15,058 (fb args 3,064); builtins.min 2,378 (fb args 265); builtins.max 1,969 (fb args 17); +12 more | 72 / 12 / 0 |
| devtools/import_startup | posixpath.dirname 42; list.sort 21; list.count 0 (fb args 21); +1 more | 25 / 59 / 0 |
| devtools/isort_stdlib | tuple.__contains__ 226,383; copy.deepcopy 26,265 (fb deopt 1,030); list.__contains__ 11,768; +7 more | 68 / 16 / 0 |
| devtools/pip_resolve | tuple.__contains__ 24,423 (fb args 36); posixpath.basename 19,180; builtins.all 12,680; +25 more | 79 / 5 / 0 |
| devtools/pycodestyle_stdlib | list.__contains__ 655,813; tuple.__contains__ 317,434 (fb args 119, deopt 106); posixpath.join 2; +4 more | 28 / 56 / 0 |
| devtools/pyflakes_stdlib | tuple.__contains__ 135,002 (fb args 67,863); list.sort 493 (fb args 61); list.__contains__ 460 (fb args 56); +2 more | 25 / 59 / 0 |
| devtools/pylint_stdlib | tuple.__contains__ 30,909 (fb args 23,318); list.__contains__ 20,966 (fb args 13,069); list.index 3,658 (fb args 2,213); +26 more | 74 / 10 / 0 |
| devtools/pytest_networkx | tuple.__contains__ 21,337 (fb args 561, deopt 1); pathlib.PurePath.__eq__ 19,518; pathlib.PurePath.__hash__ 17,748; +31 more | 79 / 5 / 0 |
| devtools/rich_render | builtins.max 74,814 (fb args 7,283); builtins.min 44,212; tuple.__contains__ 14,722 (fb deopt 100); +7 more | 79 / 5 / 0 |
| devtools/sphinx_autodoc | builtins.min 67,669; tuple.__contains__ 26,138 (fb args 85, deopt 1,621); list.__contains__ 11,738 (fb args 123); +30 more | 79 / 5 / 0 |
| devtools/sqlglot_transpile | copy.deepcopy 36,909 (fb deopt 10,361); list.sort 12,931 (fb args 112); tuple.__contains__ 5,418 (fb args 3,115, deopt 6,908); +7 more | 69 / 15 / 0 |
| web/aiohttp_app | list.__contains__ 327,003; tuple.__contains__ 4,240; builtins.max 2,345; +4 more | 79 / 5 / 0 |
| web/asyncio_http | tuple.__contains__ 8,533 (fb args 14,507); json.loads 5,974 (fb deopt 852); json.dumps 5,974; +3 more | 52 / 32 / 0 |
| web/django_blog | urllib.parse.quote 3,056; datetime.datetime.isoformat 1,604; tuple.__contains__ 1,440 (fb args 2,392, deopt 2,702); +13 more | 84 / 0 / 0 |
| web/email_mime | tuple.__contains__ 82,749; list.__contains__ 480; random.randrange 360; +8 more | 49 / 35 / 0 |
| web/fastapi_pydantic | copy.deepcopy 7,770 (fb deopt 1,915); tuple.__contains__ 6,320 (fb args 1,360, deopt 800); builtins.max 3,206; +12 more | 79 / 5 / 0 |
| web/flask_jinja | tuple.__contains__ 17,600; urllib.parse.quote 13,200; urllib.parse.quote_plus 4,080; +8 more | 79 / 5 / 0 |
| web/json_api | list.count 49,680; copy.deepcopy 36,351 (fb deopt 5,173); datetime.datetime.isoformat 25,130; +7 more | 44 / 40 / 0 |
| web/logging_service | logging.Formatter.formatTime 15,170; tuple.__contains__ 14,586; logging.Logger.findCaller 14,154; +8 more | 61 / 23 / 0 |
| web/requests_local | urllib.parse._urlsplit 6,000; list.__contains__ 5,940; tuple.__contains__ 4,263 (fb args 7,800); +11 more | 77 / 7 / 0 |
| web/sqlalchemy_orm | list.sort 2,604 (fb args 864); tuple.__contains__ 2,019 (fb args 5,890); datetime.datetime.isoformat 1,500; +3 more | 76 / 8 / 0 |

## 5. pyperformance 1.14.0

Phase 0's method (`docs/atlas/phase0.md` §6): Phase 0's driver and benchmark venvs, plain
vs `rustique.enable()` (all 15 groups) injected through `sitecustomize`
(`RUSTIQUE_ENABLE=0|1`), full default suite, two replicate pairs per build (A: plain run
first, B: enabled first), each run pinned to one L3 domain, at most one timing job per
domain. Ratios are means enabled / plain (`analyze_phase0.py pyperformance`); significance
per benchmark in `pyperformance/compare-<build>[-B].md` (`pyperf compare_to`).

| | 3.15.1 A | 3.15.1 B | 3.15.1t A | 3.15.1t B | Phase 0 3.15.1 / 3.15.1t |
|---|---:|---:|---:|---:|---:|
| benchmarks compared | 121 | 120 | 120 | 122 | |
| **geometric mean** | **0.942** | **0.940** | **0.956** | **0.957** | 1.002 / 1.002 (replicates 1.000 & 1.004 / 0.999 & 1.006) |
| median | 0.998 | 0.998 | 0.999 | 1.000 | |
| < 0.95 | 13 | 12 | 9 | 10 | |
| > 1.05 | 2 | 1 | 1 | 1 | |

**Per benchmark, both replicates averaged** (geometric mean of the two ratios; the three port
benchmarks are taken from the port re-run where a replicate lost them):

- **0.942 on 3.15.1 and 0.957 on 3.15.1t**, over 122 benchmarks per build. Phase 0, computed
  the same way, gives 1.002 / 1.002.
- The tables are `tables/pyperformance.md` (`analyze_phase3.py ppcompare`). They list each
  benchmark's plain time, its ratio in each replicate, and Phase 0's ratio.

Benchmarks beyond 2% in *both* replicates:

| build | faster by > 2% | slower by > 2% |
|---|---|---|
| 3.15.1 | 15 benchmarks | only `python_startup` (1.098 / 1.078) |
| 3.15.1t | 14 benchmarks | `python_startup` (1.092 / 1.098), `xdsl_constant_fold` (1.033 / 1.033), `pyflate` (1.025 / 1.032) |

`pyflate` was 1.021 in Phase 0 as well, so its result is the box's, not wave 1's
(*inferred*).

- **Gains** (both replicates; 3.15.1 / 3.15.1t): `deepcopy_memo` 0.13 / 0.18, `deepcopy`
  0.23 / 0.28, `json_dumps` 0.25 / 0.33, `json_loads` 0.57 / 0.59, `logging_silent` 0.61–0.62
  (3.15.1t: within noise), `logging_format` 0.65 / 0.77, `logging_simple` 0.67 / 0.79,
  `sympy_integrate` 0.83 / 0.83, `meteor_contest` 0.93 / 0.93, `deepcopy_reduce`
  0.93 / 0.92, `hexiom` 0.93 (3.15.1), `pathlib` 0.93–0.94 / 0.95, `scimark_lu` 0.94
  (3.15.1 A only).
- **Above 1.05**: `python_startup` in all four pairs, at 1.078–1.098. This is the startup
  cost of §6, from `enable()` in `sitecustomize`. Phase 0 measured 1.176 / 1.180; `fannkuch` 1.114 in 3.15.1 pair A only (≤ 1.03
  in pair B; integer list work Rustique does not touch: noise, as Phase 0 saw for other
  bimodal benchmarks).
- **1.03–1.05**: `xdsl_constant_fold` 1.033 on 3.15.1t in both pairs (not on 3.15.1;
  cause not investigated, flagged); in one pair only: `pickle_pure_python` 1.035 (3.15.1 A),
  `pyflate` 1.032, `richards_super` 1.039 and `typing_runtime_protocols` 1.046 (3.15.1t B;
  the last two with standard deviations of 7% and 22%).
- **Missing results**: `asyncio_tcp`/`asyncio_tcp_ssl` (in two runs) and
  `asyncio_websockets` (in one) failed with port collisions between concurrent runs (as in
  Phase 0). `genshi` fails on plain 3.15, as in Phase 0. `pyperformance-ports` reran the
  three port benchmarks alone (`pyperformance/{plain,enabled}-<build>-ports.json`). There was
  one plain and one enabled run per build, run one after another with nothing else on the
  box, 00:13–00:23Z.
  - 3.15.1: `asyncio_tcp` 1.018, `asyncio_tcp_ssl` 0.998, `asyncio_websockets` 1.001.
  - 3.15.1t: 1.005, 1.001 and 1.000.
  - Every pyperformance step exits 1, because pyperformance does so whenever a benchmark
    fails (`genshi`).

## 6. Startup cells

`benches/dx/startup.py cells`: the release wheel `pip install`ed into a fresh `python -m
venv`, against an identical venv without it; callgrind total Ir (median of 3,
`PYTHONHASHSEED=0`) and wall time (median and p10–p90 of **300** interleaved rounds, pinned
to CPU 30, nothing else running in that L3 domain). Ratios against `python -c pass` in the
plain venv; "other .pth" = both venvs carry a comment-only `.pth`.

| build | venv | cell | Ir | Ir ratio | wall ms median (p10–p90) | wall ratio |
|---|---|---|---:|---:|---:|---:|
| 3.15.1 | bare | plain: pass | 38,668,471 | 1.0000 | 9.44 (9.38–9.53) | 1.0000 |
| 3.15.1 | bare | installed, unset: pass | 39,703,483 | 1.0268 | 9.74 (9.69–9.84) | 1.0321 |
| 3.15.1 | bare | RUSTIQUE=1: pass | 41,502,051 | 1.0733 | 10.50 (10.43–10.61) | 1.1120 |
| 3.15.1 | bare | import rustique | 41,265,567 | 1.0672 | 10.39 (10.32–10.49) | 1.0999 |
| 3.15.1 | bare | import + enable() | 42,841,505 | 1.1079 | 10.75 (10.68–10.85) | 1.1385 |
| 3.15.1 | bare | marginal RUSTIQUE=1 vs unset | | 1.0453 | | 1.0775 |
| 3.15.1 | pth | plain: pass | 39,441,267 | 1.0000 | 9.69 (9.62–9.80) | 1.0000 |
| 3.15.1 | pth | installed, unset: pass | 39,758,137 | 1.0080 | 9.80 (9.73–9.90) | 1.0113 |
| 3.15.1 | pth | RUSTIQUE=1: pass | 41,556,586 | 1.0536 | 10.55 (10.47–10.67) | 1.0882 |
| 3.15.1 | pth | import rustique | 41,320,341 | 1.0476 | 10.44 (10.37–10.54) | 1.0765 |
| 3.15.1 | pth | import + enable() | 42,895,710 | 1.0876 | 10.79 (10.72–10.91) | 1.1127 |
| 3.15.1 | pth | marginal RUSTIQUE=1 vs unset | | 1.0452 | | 1.0761 |
| 3.14.9 | bare | plain: pass | 40,589,415 | 1.0000 | 9.57 (9.52–9.65) | 1.0000 |
| 3.14.9 | bare | installed, unset: pass | 42,504,877 | 1.0472 | 10.02 (9.96–10.10) | 1.0476 |
| 3.14.9 | bare | RUSTIQUE=1: pass | 45,196,342 | 1.1135 | 11.11 (11.04–11.21) | 1.1608 |
| 3.14.9 | bare | import rustique | 44,030,935 | 1.0848 | 10.76 (10.68–10.85) | 1.1240 |
| 3.14.9 | bare | import + enable() | 45,605,596 | 1.1236 | 11.12 (11.06–11.21) | 1.1623 |
| 3.14.9 | bare | marginal RUSTIQUE=1 vs unset | | 1.0633 | | 1.1081 |
| 3.14.9 | pth | plain: pass | 42,011,008 | 1.0000 | 9.88 (9.81–9.97) | 1.0000 |
| 3.14.9 | pth | installed, unset: pass | 42,594,518 | 1.0139 | 10.02 (9.94–10.12) | 1.0143 |
| 3.14.9 | pth | RUSTIQUE=1: pass | 45,281,779 | 1.0779 | 11.11 (11.02–11.22) | 1.1247 |
| 3.14.9 | pth | import rustique | 44,120,392 | 1.0502 | 10.75 (10.65–10.84) | 1.0875 |
| 3.14.9 | pth | import + enable() | 45,695,096 | 1.0877 | 11.11 (11.02–11.22) | 1.1241 |
| 3.14.9 | pth | marginal RUSTIQUE=1 vs unset | | 1.0631 | | 1.1089 |

| cell (Ir / wall vs plain `-c pass`) | 3.15.1 bare | 3.15.1 other .pth | 3.14.9 bare | 3.14.9 other .pth |
|---|---|---|---|---|
| installed, `RUSTIQUE` unset | **1.027 / 1.032** | **1.008 / 1.011** | **1.047 / 1.048** | **1.014 / 1.014** |
| `RUSTIQUE=1` | 1.073 / 1.112 | 1.054 / 1.088 | 1.114 / 1.161 | 1.078 / 1.125 |
| `import rustique` | 1.067 / 1.100 | 1.048 / 1.077 | 1.085 / 1.124 | 1.050 / 1.088 |
| `import rustique; rustique.enable()` | 1.108 / 1.139 | 1.088 / 1.113 | 1.124 / 1.162 | 1.088 / 1.124 |
| marginal: `RUSTIQUE=1` vs unset, same venv | **1.045 / 1.078 (+0.76 ms)** | **1.045 / 1.076 (+0.75 ms)** | **1.063 / 1.108 (+1.09 ms)** | **1.063 / 1.109 (+1.09 ms)** |
| dx.md §1.4 "after" (13 targets, `ece21e2`), marginal | 1.030 / 1.053 | 1.030 / 1.051 | 1.043 / 1.067 | 1.043 / 1.069 |

Reading:

- **Installed but off stays within ≤ 1.05 on every cell** (3.14.9 bare 1.047 / 1.048 is the
  closest), and ≤ 1.014 next to another `.pth` — the same values as dx.md §1.4.
- **`RUSTIQUE=1` on 3.15.1**: marginal 1.045 Ir / 1.076–1.078 wall (+0.75 ms), 1.073 / 1.112
  against a venv without Rustique (bare). The marginal cost is inside the accepted
  1.03–1.06 Ir / 1.05–1.08 wall, at the top end of the wall range. It is higher than dx.md
  §1.4's 1.030 / 1.053, which was measured with the 13 wave-0 targets; the quiet startup now
  installs 23 targets eagerly and registers 61 pending ones (*inferred* cause, not
  decomposed).
- **`RUSTIQUE=1` on 3.14.9** (not covered by the accepted figure): marginal **1.063 Ir /
  1.108 wall (+1.09 ms)**, 1.114 / 1.161 against a bare venv without Rustique (dx.md: 1.043 /
  1.067 and 1.092 / 1.117). Flagged.
- `import rustique; rustique.enable()` costs **+1.34M Ir** more than `RUSTIQUE=1` on 3.15.1
  (+0.41M on 3.14.9, where the `.pth` loader string compiles slower). This includes the
  `rustique` package import (the `RUSTIQUE=1` path loads only the extension); the accepted
  "+0.3–0.6M Ir" is about `enable()` itself and these cells do not separate the two.
  `enable()` after `import rustique` adds +1.58M Ir on both versions.

## 7. Scoreboard regression check (enabled tier, vs Phase 0)

Same scoreboard as Phase 0 (`benches/scoreboard.py` is unchanged since `5abd54c`), enabled
tier, which now turns on all 15 groups. Categories: **kernel path** = Phase 0's definition
(`sorted/sum/min/max/any/all`, `in`/`index`/`count` on list/tuple, `list.sort()`);
**wave-1 serial** = `json.dumps` and `copy.deepcopy` cells, which wave 1 now accelerates
(unchanged code in Phase 0); **other** = everything else (this includes `repr(x)`, now faster
through `float.repr`). Every cell's result was verified against plain CPython's:
**0 mismatches, 0 errors** on both builds. Comparison per cell against Phase 0's raw files
(`analyze_phase3.py sbcompare`, outputs `scoreboard/sbcompare-*.txt`).

### 7.1 0–100 elements, callgrind instruction counts (3814 cells per build)

| | 3.15.1 phase 3 | 3.15.1 phase 0 | 3.15.1t phase 3 | 3.15.1t phase 0 |
|---|---|---|---|---|
| cells ≤ 1.05, as run | 3798 / 3814 | 3807 / 3814 | 3809 / 3814 | 3814 / 3814 |
| cells ≤ 1.05 after re-measuring the cells > 1.05 alone | **3811 / 3814** | 3814 / 3814 | **3811 / 3814** | 3814 / 3814 |
| geomean, all | **0.859** | 0.931 | **0.864** | 0.935 |
| kernel path (586): geomean / ≤ 1.05 | 0.632 / 583 | 0.627 / 586 | 0.649 / 583 | 0.644 / 586 |
| wave-1 serial (152): geomean | **0.154** | 1.000 | **0.161** | 1.000 |
| other (3076): geomean / median | 0.992 / 1.000 | 1.000 / 1.000 | 0.992 / 1.000 | 1.000 / 1.000 |

Ops whose geomean moved by more than 3% against Phase 0 (both builds alike): `json.dumps`
0.12–0.15 (was 1.00), `copy.deepcopy` 0.10–0.22 (was 1.00), `repr(x)` of every container
0.73–0.84 (was 1.00; `float.repr` makes the float and mixed storages' element reprs cheaper),
and `x.count(hit)` on list/tuple +3–4% (below).

**Cells above 1.05**, each re-measured alone 3 times in its own callgrind worker (Phase 0's
method; `scoreboard/isolated-instr-*.txt`; identical Ir in all 3 runs):

- 3.15.1: 16 cells as run; **13 are 0.90–1.007 alone** (allocator state left by the previous
  cell in the shared worker, as in Phase 0: `tuple/from list/*/100` 1.090 → 1.000,
  `list/repr(x)/float/0` 1.056 → 0.998, …). 3.15.1t: 5 as run; `list/x + y/str/8` 1.074 →
  1.046 and `list/x * 2/str/8` 1.068 → 1.043 alone (code Rustique does not touch; +42 Ir).
- **3 kernel-path cells stay above 1.05 alone, on both builds** (Phase 0: all ≤ 1.043):

  | cell | 3.15.1 plain → enabled Ir (ratio) | 3.15.1t | phase 0 (3.15.1) |
  |---|---|---|---|
  | `list/x.count(hit)/object/1` | 297.7 → 317.3 (**1.066**) | 344.1 → 365.4 (**1.062**) | 297.9 → 272.9 (0.916) |
  | `tuple/x.count(hit)/object/1` | 301.7 → 321.3 (**1.065**) | 329.1 → 350.4 (**1.065**) | 301.9 → 271.9 (0.901) |
  | `list/miss in x/object/1` | 375.1 → 393.9 (**1.050**) | 400.1 → 422.0 (**1.055**) | 375.1 → 391.1 (1.043) |

  Cause (from the code history, *inferred*): since `c39c8c8`/`f189e6c` the `count`/`in`
  kernels delegate a needle that is not a plain scalar at once, instead of scanning (which
  made larger object lists 1.12–1.16). The delegation costs ~+20 Ir, which is ≤ 1.05 on the
  containers doc's own cells (`objs.count(o)` 1.035, `tobjs.count(o)` 1.049) but not on
  the scoreboard's 1-element object list/tuple, where CPython's whole call is ~300 Ir. At
  size 8 the same cells are 1.010 and at 100 1.001. A never-slower exceedance of +20 Ir on
  the cheapest calls; **flagged** (not in the known-accepted list).

### 7.2 10k/1M elements, wall time (1846 cells per build)

Phase 0's method: `scoreboard.py time --size 1e4,1e6`, pyperf medians of 3 processes ×
3 values, with 4 single-CPU shards on CPUs 2, 10, 20 and 30, after every pyperformance run
had finished. The data is `scoreboard/time-enabled-<build>.json.shard*.json.gz`, with the
analysis in `scoreboard/analysis-enabled-*.txt` and `compare-phase0-enabled-*.txt`.

| | 3.15.1 phase 3 | 3.15.1 phase 0 | 3.15.1t phase 3 | 3.15.1t phase 0 |
|---|---|---|---|---|
| cells ≤ 1.05, as run | 1744 / 1846 | 1750 / 1846 | 1771 / 1846 | 1773 / 1846 |
| geomean, all | **0.830** | 0.871 | **0.836** | 0.877 |
| kernel path (328): geomean / cells > 1.05 | 0.462 / 5 | 0.460 / 5 | 0.482 / 3 | 0.477 / 3 |
| everything else (1518): geomean / median | 0.942 / 1.000 | 1.000 / 1.000 | 0.942 / 0.999 | 1.000 / 1.000 |

- **As in Phase 0, wall time reproduces in aggregate, not cell by cell** (`phase0.md` §6.2).
  - Against Phase 0, the cells cross the 1.05 bar in both directions in equal numbers: 80
    up and 77 down on 3.15.1, 57 and 55 on 3.15.1t.
  - The cells above 1.05 are mostly different cells on the two builds: 7 are shared among
    the 60 worst of each. They carry their own noise (the worst, `frozenset/x <= t/str/10k`
    at 1.40, has a relative standard deviation of 4.5%).
- **The kernel-path geomeans equal Phase 0's** (0.462 vs 0.460, and 0.482 vs 0.477).
  `json.dumps`, `copy.deepcopy` and `repr(x)` account for everything else's 0.942.
- **The 8 kernel-path cells above 1.05 were re-measured alone** (`scoreboard/isolated-time.txt`).
  - The 8 cells (5 on 3.15.1 and 3 on 3.15.1t):
    - `frozenset/all(x)/{mixed,object}`, `frozenset/{min,max}(x)/str`;
    - `set/all(x)/{str,mixed,float}`, `dict/any(x)/mixed`;
    - all at 10k elements, as run 1.054–1.191.
  - Method: each cell measured alone on CPU 2, with 10 pyperf processes × 5 values, twice
    per build, on both builds. Phase 0 showed that at 10 processes every kernel cell is
    ≤ 1.05.
  - **All 32 measurements are ≤ 1.05**: 0.967–1.040, and the means of the two runs per cell
    are 0.988–1.027.
  - On 3.15.1 the `set`/`frozenset` `min`/`max`/`all` cells of `str` and `object` sit
    consistently at 1.008–1.025: a small cost, inside the bar, that was not decomposed.

## 8. Per-target spot checks (callgrind, 3.15.1)

Each target's own callgrind bench, as its doc's reproduce section runs it, on 3.15.1, alone
in domain 0 with other callgrind jobs (region-instrumented Ir per call, noise ±15 Ir; the
targets' groups enabled as each bench does, not all groups). Output: `spot/<bench>-3.15.1.txt`.

| bench (target docs) | cells | worst cell (ratio) | cells > 1.05 | headline cells, phase 3 (doc) |
|---|---:|---|---:|---|
| `json_cg.py` (json.md, float.md) | 43 | `repr(inf)` fallback 1.036 (doc 1.005) | 0 | `dumps(record)` 0.203 (0.211), `loads(record)` 0.465 (0.465), `dumps(small list)` 0.120 (0.131), `repr(1.23)` 0.363 (0.371) |
| `copy_cells.py` (copy.md) | 52 | `deepcopy(OrderedDict)` 1.030 (doc 1.033) | 0 | `deepcopy(record)` 0.175 (0.171), list of 5 records 0.169 (0.164), dataclass tree 0.259 (0.257), `asdict` small 0.387 (0.398) |
| `datetime_cg.py` (datetime.md) | 39 | `strptime` no match (error fallback) 1.038 (doc 1.038); `strftime` names in en_US 1.038 | 0 | `strptime` log format 0.076 (0.079), ISO `%f%z` 0.067 |
| `paths_cg.py` (posixpath.md, fnmatch.md, pathlib.md) | 99 | 1.003 (`MyP(p)` subclass) | 0 | `join(d, f)` 0.266 (posixpath.md: 0.29 with all guards), `fnmatch(f, '*.py')` 0.124 (0.130) |
| `logging_cg.py` (logging.md) | 48 | `info` with `time.strftime` replaced [deps] 1.006 | 0 | `info %(message)s` 0.509 (0.512), asctime 0.451 (0.453), `LogRecord()` 0.394 (0.397), `formatTime` 0.201 (0.210) |
| `utils_cg.py random_cells` (random.md) | 41 | 1.001 (`choices` with Fraction weights, fallback) | 0 | `randint(1, 6)` 0.367 (0.355), `shuffle(L100)` 0.051, `sample(W, 5)` 0.135 |

**Every spot-check cell is ≤ 1.05** and the headline cells reproduce the docs within ±0.01–0.02.
The one fallback cell that moved is `float repr(inf)`: 775 → 803 Ir (1.036; float.md: 794 →
798, 1.005), still within the bar.

## 9. Step durations

From `durations.tsv` (per step: start, end, seconds, exit code). Steps of a stage ran in
parallel where §1 says so.

| stage | steps | each | notes |
|---|---:|---|---|
| build (fat LTO) | 4 | 71–73 s | in parallel, 8 cores each |
| pytest | 4 | 220–290 s | |
| cargo fmt / clippy / test | 3 | 1 / 13 / 25 s | |
| conformance (all groups) | 3 | 176–185 s | |
| workloads (`run.py compare`, 3 families) | 2 + 2 | 1306–1420 s | second replicate after pyperformance |
| wlstats / wlstats-run | 2 / 2 | 59–62 / 48–51 s | |
| whole-process (`pyperf command`) | 2 | 568–668 s | |
| startup cells | 2 | 81–84 s | 300 rounds each |
| scoreboard `instr` (8 callgrind shards) | 2 | 638–700 s | |
| isolation re-runs (instr) | 2 | 75–223 s | |
| spot checks | 1 | 769 s | |
| pyperformance, one full run | 8 | 3698–4008 s | exit 1 by design (§5) |
| scoreboard `time` (4 single-CPU shards) | 2 | 3410–3428 s | |
| port re-runs | 4 | 144–161 s | |
| pytest-repeat | 6 | 211–284 s (one crash after 29 s) | §2 |
| reports | 6 | 0–3 s | |

The box schedule ran from **19:21Z to 00:47Z (5 h 26 min)**. The wall-time isolation re-runs
of §7.2 ran after it, 00:54–01:01Z. The long poles were the same as in Phase 0:
pyperformance (≈ 63–67 min per run, two runs per domain in sequence) and the scoreboard's
wall time (57 min per build on 4 shards).

## 10. Anomalies and incidents

Findings to act on (all **measured**; causes *inferred* where tagged):

1. **Three kernel-path scoreboard cells above 1.05** at 1 element, on both builds:
   `list/x.count(hit)/object/1` 1.066 / 1.062, `tuple/x.count(hit)/object/1` 1.065 /
   1.065, `list/miss in x/object/1` 1.050 / 1.055 (+19–21 Ir on a ~300–400 Ir call;
   Phase 0: 0.90–0.92 and 1.043). The count/`in` kernels now delegate non-scalar needles at
   once (§7.1).
2. **`RUSTIQUE=1` startup on 3.14.9**: marginal 1.063 Ir / 1.108 wall (+1.09 ms), 1.114 /
   1.161 against a bare venv without Rustique (§6). On 3.15.1 the marginal 1.045 / 1.077 is
   inside the accepted range (top of the wall range). Both are higher than dx.md §1.4
   (13 targets then, 84 now; *inferred*).
3. **pytest imports Rustique in every session** of a venv where the wheel is installed,
   `--rustique` or not (the `pytest11` entry point imports the package and the extension;
   ≈ +1.5M Ir / +0.65 ms per pytest process, *inferred* from the startup cells; §4.1).
4. **pyperformance on 3.15.1t: `xdsl_constant_fold` 1.033 in both replicate pairs** (§5).
   The only other result above 1.03 in more than one pair is `python_startup` (§6). `pyflate`
   is 1.025 / 1.032, but it was 1.021 in Phase 0.
5. **One segfault in 9 repeated test runs** (§2): `json.loads` read a freed key when a
   document repeats a key. Hypothesis produces such documents through surrogate-pair
   escapes. Fixed after the run (§11).

Not bugs, recorded so nobody chases them:

- `devtools/import_startup`'s digest differs plain vs `RUSTIQUE=1` in the whole-process runs
  by construction (its result counts `sys.modules` in 21 children, which gain
  `rustique._rustique`; §4.1). In `run.py compare` (children not enabled) it matches.
- The conformance runner lists 3.14.9's five 3.15-only `pathlib.PurePath` property targets as
  "not installed (pending-import)"; they are `refused` by version, as designed (§3).
- On 3.15.1t, plain `copy.deepcopy`-heavy workloads are 2–4× slower than on 3.15.1
  (`isort_stdlib` 1187 vs 284 ms, `state_snapshot` 1249 vs 648 ms); enabled they are close
  to the GIL build. Cause of the slow free-threaded `deepcopy` not investigated.
- `tests/test_dx_cli.py:356` is skipped because `pytest-xdist` is not in the test
  dependencies (the xdist path of the pytest plugin was not exercised here).

Incidents (none affected a measurement):

- Script fixes during the run, each visible in `steps.log` as an immediately failing step:
  the `wheel` stage listed the wheel with `unzip` (not installed; the wheel itself was
  built), the first `wlvenv` tried one venv for all three families (unsatisfiable: data pins
  `python-dateutil==4.0.0`, devtools' `boto3==1.43.123` needs `< 3`; now one venv per
  family), and `wlstats` hit a `local`-expansion bug under `set -u` (aborted at once, rerun).
  One deployment `scp` of two files to a single target created a directory instead of
  replacing the script; caught before anything ran with it; deployments were by atomic
  rename afterwards (Phase 0's lesson).
- 20:06:30–20:06:45Z a ~15 s analysis script (nice'd) ran in domain 0 while
  `pyperformance enabled 3.15.1t -B` was still in its setup phase (venv check, before the
  first benchmark); no benchmark overlapped it. All later analysis ran locally.
- About 21:00Z the operator stopped the Phase 3 agent by mistake.
  - The box's schedule (`run_phase3.sh`, detached lanes) was not affected. Nothing was
    interrupted or restarted.
  - The lead finished the run from that schedule. That covered the scoreboard wall time,
    the port re-runs, the repeated tests, the second workload replicate and the reports.
  - The lead then added the wall-time isolation re-runs (§7.2) and wrote §2's repeat
    results, §5's averaged pyperformance, §7.2, §9 and the summary.
- `pytest-repeat-3.15.1-2` exited 139 (segfault). This is a finding, not an incident: §2 and
  §11.
- `conformance-3.14.9` exits 1, as designed. The runner's `--expect-installed` check reports
  the five 3.15-only `pathlib.PurePath` targets (§3). Every module has 0 differences.

## 11. After Phase 3: fixes on `wave1` after `11bad29`

Phase 3 measured `11bad29`. What it and the first CI run of the integrated branch found was
fixed on `wave1` before merging (each with tests; four-build pytest green after each):

| finding | fix | evidence |
|---|---|---|
| 3 kernel-path scoreboard cells > 1.05 (§7.1) | non-scalar needles rejected by one type-flag test; frameless `count` delegation (`32b6950`) | 1.026–1.045 on 3.15.1/3.15.1t (callgrind, dev box); `docs/targets/containers.md` |
| pytest loads the extension in every session (§4.1) | the plugin is the top-level module `_rustique_pytest` (`838cd18`) | test: a session without `--rustique` imports no `rustique` module |
| public `enable()` builds its status (§6) | shared strings in the status (`9e8f7c9`) | −0.14M Ir of ≈ 1.57M; the rest is wave 2 (lazy status) |
| CI: segfault in `json.loads` (3.14/3.15 GIL jobs) | the key memo owns its keys: a use-after-free on documents that repeat a key (`dac4ab9`); Phase 2's "unreproduced crash" | memcheck: invalid read in `Memo::find`; seed 208 of 300 crashed before, 0 of 300 after; Phase 3's repeat crash (§2) reproduces on `11bad29` under memcheck and is clean on the fix |
| CI: Windows wheels did not link; macOS/musl/Windows smoke failed | `raw-dylib` imports for this crate's CPython symbols (`705ddd3`); refusal category `platform` (`9ce8eda`, `4c53e96`) | full wheel matrix green on 8 platforms × 4 builds |

The changed code paths are the container `count`/`in` delegation (faster), the pytest plugin's
import, status construction, and `json.loads`' key memo (one reference count per distinct key
per call). None changes a measured kernel's work beyond that; the numbers above stand for the
merged branch (*inferred*; the scoreboard cells were re-measured).

## 12. Files

`benches/results/phase3/`:

- `run_phase3.sh` — every stage, rerunnable (`./run_phase3.sh <stage> ...`; `all` = the
  schedule used, `stage_all` at the end); `whole.py` (one workload per process, for
  `pyperf command`); `wlstats_run.py` (counters of one steady-state `run()`);
  `analyze_phase3.py` (the tables of this document: `workloads`, `stats`, `whole`,
  `startup`, `conformance`, `junit`, `sbcompare`, `ppcompare`). Phase 0's `analyze_phase0.py` and
  `sitecustomize.py` were reused from `benches/results/phase0/`.
- `tables/` — the generated Markdown tables (inputs of this document).
- `steps.log`, `durations.tsv` — every step with start/end/seconds/exit code.
- `logs/` — one log per step (build, pytest with durations, conformance, workloads,
  pyperformance, scoreboard, spot checks, …).
- `machine/pythons.txt` — interpreter paths and `python -VV`.
- `tests/` — JUnit XML per build (and per repeat run), `freeze-*.txt`.
- `conformance/all-groups-<build>.json` — per-module, per-test-id comparison.
- `workloads/` — `compare-<build>-<family>[-2].json` (run.py output, two replicates), `stats-run-<build>/`
  (counters of one `run()`), `stats-<build>/` (whole-process `RUSTIQUE_REPORT` records),
  `freeze-<build>-<family>.txt`.
- `whole/` — `whole-<build>.json` (pyperf suites), `digests-<build>/` (per-process digests).
- `pyperformance/` — `{plain,enabled}-<build>[-B|-ports].json`, `compare-*.md`,
  `analysis-*.txt`.
- `startup/cells-<build>-{bare,pth}.json`.
- `scoreboard/` — `instr-enabled-*.json`, `time-enabled-*.json.shard*.json`, matrices
  `enabled-*.md`, `summary-*`, `analysis-*`, `compare-phase0-*`, `sbcompare-*.txt`,
  `isolated-instr-*.txt`, `isolated-time.txt` (§7.2).
- `spot/` — per-target callgrind benches (`<bench>-3.15.1.{txt,json}`).

Files over 1 MB are gzipped. The box keeps everything under `~/phase3/` (clones `w/<build>`,
venvs `venv/<build>`, workload venvs `wl/<build>-<family>`, wheels `dist/<build>/`,
startup venvs `su/<build>/`).
