# Wave 2, Phase 3 — integrate and prove

Rustique `wave2`, measured on the dedicated AWS c7a.8xlarge of Phase 0 on 2026-11-03,
03:24–10:55Z. The code kept landing during the run; the lead fixed which commit each stage
measures (§0): the steady state on **`7fec7e4`** (headline replicate 2, pyperformance, the
scoreboard, the addenda), the opt-in tiers and headline replicate 1 on **`d40e7b9`**, install,
startup and whole-process runs on **`9d0abae`** (`re`'s first-call rework), the paths fix on
**`78751e2`**. Fat-LTO release builds; "plain" = the same venv and interpreter without Rustique
enabled; "enabled" = the default tier (`rustique.enable()`, every default group); "+interp",
"+gc", "+both" = the default tier plus `RUSTIQUE_TIERS`. Everything is **measured** unless
tagged *inferred*. Raw data, logs and the rerunnable scripts: `benches/results/w2-phase3/` (§13).

## Summary

**The headline (§4): the 65 workloads, default tier, `7fec7e4` (replicate 2; replicate 1 on
`d40e7b9` agrees within 0.006 per family). 0 digest mismatches in 4 × 65 comparisons.**

| | 3.15.1 | 3.15.1t | Phase 0 (wave-1 code) |
|---|---:|---:|---:|
| **all 65** | **0.870** | **0.872** | — |
| wave 1's 34 | **0.858** | **0.826** | 0.877 / 0.843 |
| the 31 new | 0.884 | 0.925 | — |
| web / data / devtools | 0.882 / 0.752 / 0.957 | 0.903 / 0.735 / 0.861 | 0.919 / 0.768 / 0.962 · 0.932 / 0.756 / 0.865 |
| frameworks / libs / custom | 0.940 / 0.961 / 0.936 | 0.955 / 0.966 / 0.973 | — |
| services / threads (8 thr) | 0.848 / 0.685 | 0.888 / 0.792 | — |
| best / worst | `report_render` 0.357 / `mini_interp` 1.006 | `isort_stdlib` 0.201 / `mini_interp` 1.018 | |
| 8 whole processes, `RUSTIQUE=1` (`9d0abae`) | 0.743 | 0.742 | wave 1's six: 0.696 / 0.680 (Phase 0 0.754 / 0.713) |
| pyperformance 1.14.0, 122 benchmarks | **0.911** | **0.936** | 0.942 / 0.955 |

**The targets** (IMPACT_ATLAS §14.4; measured unless tagged):

| # | target | result | |
|---|---|---|---|
| T1 | no workload > 1.02; no cell > 1.05 | workloads: worst 1.006 / 1.018; 0–100 cells: 3814 / 3814 on both builds (`7fec7e4`); **`sorted()` of a 1M-element set/frozenset of int/float 1.25–1.28** (kernel, alone, §8.2); `RUSTIQUE=1` wall 1.059 / 1.075 (§7.2); first-call install of `random` +0.29M Ir (§7.1); 8-thread rule met on `78751e2` (`posixpath.join` fallback 1.175 → 0.359); pyperformance `coroutines` 1.07 on 3.15.1t (layout, *inferred*, §6) | **not met** |
| T2 | 34 ≤ 0.85 / ≤ 0.80 | 0.858 / 0.826 | **not met** |
| T3 | 65 ≤ 0.88 / ≤ 0.84 | 0.870 / 0.872 | met / **not met** |
| T4 | custom ≤ 0.95, frameworks ≤ 0.98, libs ≤ 0.95, services ≤ 0.85; threads t ≤ 0.78 | 0.936, 0.940, **0.961**, 0.848; **0.792** | 3 of 5 met (libs, threads not) |
| T5 | `interp`: frameworks ≤ 0.96, custom ≤ 0.90 | 0.933 / **0.920** (`d40e7b9`); full regrtest not re-run here | frameworks met, custom **not met** |
| T6 | `gc`: GC > 10% geomean ≤ 0.87, RSS ≤ 1.05, pause ≤ default, others ≤ 1.02 | 0.982 / 0.952; RSS ≤ 1.032 / 1.038; pause `state_snapshot` 1.30 (3.15.1); others 1.021 / 1.037 | **not met** (RSS met) |
| T7 | isort / state_snapshot, t, copy off ≤ 0.35 / 0.75 | 0.260 / 0.668 | met |
| T8 | `RUSTIQUE=1` ≤ 1.05 Ir and wall; extension ≤ 2.5 MB | Ir 1.028 / 1.029; wall **1.059 / 1.075**; **3.63 MB** (`9d0abae`) | Ir met; wall, size **not met** |
| T9 | lazy install ≤ max(5%, 0.1M) | every lazy install within budget on `9d0abae` (on `7fec7e4` fnmatch, urllib.parse were over) | met (first call: `random` over) |
| T10 | ≥ 80 on 3.15.x, ≥ 75 on 3.14.x, refusals visible | 108–110 / 104 (t); 105 / 99 (t); 3.14.0 `unvalidated-patch`, 3.16.0a1 `version` | met on this box (platform floors: CI's, not measured) |
| T11 | CPython's test modules 0 differences | 0 differences on 4 builds, 50 modules (+ re-runs of the changed groups on each later commit) | met (real-project suites: dx's, not re-run) |
| T12 | pyperformance ≤ 0.93 / ≤ 0.94 | 0.911 / 0.936 | met |
| T13 | `estimate` within ±0.03 on ≥ 80% | 45 / 65 (69%) on 3.15.1; 52 / 65 (80%) on 3.15.1t | **not met** on 3.15.1 |

**Tests and conformance**: `pytest -q` 0 failures on 4 builds at `d40e7b9`, `445de19`, `7fec7e4`;
on `9d0abae` one test race on 3.14.9t (fixed test-only in `b2a12b1`). Rust fmt/clippy clean, 74
tests. Conformance 0 differences everywhere (18,210 outcomes per 3.15 build).

**Opt-in tiers (§5)**: +interp 0.859 against plain over the 65 (0.987 of the default tier); +gc
0.868 / 0.866 (0.998 / 0.992 of the default tier); +both on frameworks/custom/devtools 0.936 /
0.922 / 0.942. No tier slows a workload by more than 1.8% against the default tier.

**What did not get done**: CPython's full regression suite under the `interp` tier (T5's other
half; not in this brief), the 14 real-project suites (T11's other half: dx's, on the dev box),
Windows/macOS/musl coverage (T10's platform floors: CI), the 3.15.1t wall-time scoreboard (as in
Phase 0), and a decomposition of the `coroutines` slowdown.

## 0. Which commit each stage measured

Wave 2's code kept landing while the box ran. The lead fixed the measured commit per stage as it
landed (all on `wave2`):

| commit | what it is | stages measured on it |
|---|---|---|
| `d40e7b9` | the integrated wave 2 (the lead's GO) | stage 1–2 (builds, tests, status, Rust, conformance of all 50 modules), **headline replicate 1**, **opt-in tiers** (+interp, +gc, +both), callgrind scoreboard (first pass), estimator (first pass), counters of one `run()` |
| `445de19` | + text's fingerprint follow-up (install-time checks only) | stage 1 again, the text groups' 15 conformance modules |
| `7fec7e4` | + core's tiny-input sort/`sorted`/`any` fixes (made after the first scoreboard pass), a test fix; **the final steady-state code** | stage 1 again, conformance of the changed groups (19 modules), **callgrind scoreboard 0–100**, **headline replicate 2 (the headline)**, T7, estimator, threads and gc addenda, pyperformance, scoreboard wall time and probe, install costs (first look) |
| `9d0abae` | + `re`'s first-call validation rework (install/first-call behaviour only, no steady-state kernel) | stage 1 again (status/T10), the `re` group's 16 conformance modules, **install costs (T9)**, **startup cells (T8)**, **whole-process runs** |
| `b2a12b1` | + a test-only fix (the posixpath exit test's daemon-thread race found here, §2.1) | not measured (test only) |
| `78751e2` | + text's posixpath/pathlib/fnmatch fix for the 8-thread cell found here (§9.2); **the final code commit** | the paths groups' 6 conformance modules, the whole kernel table at 1–8 threads on 3.15.1t, `fs_inventory` and `pytest_networkx` against the headline, the `Pattern.match` re-measure |

The lead chose each split; what was not re-run on a later commit is unaffected by its changes
(the lead's statement: install-time, first-call or paths-only changes). Everything is
**measured** on the bench box unless tagged *inferred*. Raw data: `benches/results/w2-phase3/`
(`d40e7b9`), `textfix/` (`445de19`), `final/` (`7fec7e4`), `re9/` (`9d0abae`), `p78/`
(`78751e2`), `step0/` (Step-0 rehearsal on `58a91e8`).

## 1. What was run

**Box.** The same AWS c7a.8xlarge as Phase 0 (`docs/atlas/w2-phase0.md` §1): AMD EPYC 9R14,
32 cores without SMT, 4 L3 domains (0–7, 8–15, 16–23, 24–31), Ubuntu 24.04, kernel
`7.0.0-1014-aws`. Nobody else used it. `apt-daily*`, `unattended-upgrades`, `motd-news`,
`sysstat-*`, `fwupd-refresh`, `man-db`, `dpkg-db-backup`, `logrotate` and
`update-notifier-download` timers were stopped for the run (`logs/quiet.log`). Sustained clock
with one core busy: 18.40 G cycles in 5.00 s = 3.68 GHz, as Phase 0 (`machine/perf-stat-busy.txt`).
Single-CPU timing on CPUs 2, 10, 20, 30, as wave 1 and Phase 0. **Step 0** (Nov 2, 23:06–23:30Z,
`step0/`): the workload venvs of all 8 families on both builds (web/data/devtools package-for-
package identical to Phase 0's; frameworks/libs/services `--no-deps`; `uv pip check` clean), every
one of the 65 workloads imported and ran once plain on both builds (65 / 65 each), clang-20
installed, and every stage rehearsed end to end on `58a91e8`.

**Toolchain.** Rust 1.99.1, clang 20.1.2 and llvm-profdata 20 (Ubuntu `clang-20`/`llvm-20`, for
the interp tier's handler sets; `RUSTIQUE_INTERP_REQUIRE=1`), uv 0.13.6, PyO3 0.30.1, maturin
1.15.0, pytest 9.1.1, hypothesis 6.170.9, numpy 2.5.4, pyperf 2.10.0, pyperformance 1.14.0.
CPython: python-build-standalone 3.15.1, 3.15.1t, 3.14.9, 3.14.9t (20261021, as wave 1 and
Phase 0) for builds, tests and timing; for target status also 3.15.2(t), 3.14.10(t)
(20261026, Phase 0's), 3.15.0 (20261014), 3.14.0, 3.14.1, 3.14.5, 3.14.6, 3.14.8 (uv's
latest builds of each) and 3.16.0a1(t).

**Commands** — `benches/results/w2-phase3/run_w2phase3.sh` (stage names in brackets; Phase 0's
runner with the wave-2 changes):

```bash
# [build/wheel/status/test] per build b in 3.15.1 3.15.1t 3.14.9 3.14.9t
RUSTIQUE_INTERP_REQUIRE=1 maturin develop --uv --release; maturin build --release; python tools/repack_wheel.py dist/*.whl
RUSTIQUE_VALIDATE=eager python status_dump.py status-$b.json    # every target module imported, enable(), status()
python -m pytest -q -p no:cacheprovider --durations=25 --junitxml=...
# [statusx] the release wheel of the same minor/ABI in a fresh venv on 3.15.0/2(t), 3.14.0/1/5/6/8/10(t)
# [status316] 3.16.0a1(t): the cp315 wheel, then `uv pip install <source tree>`
# [rust] once (PYO3_PYTHON = 3.15.1)
cargo fmt --all --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace
# [conformance] 3.15.1, 3.15.1t, 3.14.9, 3.14.9t: every default group at once, 50 CPython test modules
RUSTIQUE_VALIDATE=eager CPYTHON_SRC=... python tests/conformance/run_target_suites.py --targets <every default group> \
    --modules <wave 1's 27 + 23 wave-2> --expect-installed <every default target installed on the build>
# [wlvenv] one venv per build and family: requirements (frameworks/libs/services --no-deps) + pyperf 2.10.0 + the repacked wheel
# [workloads] the headline, per family, pinned to one L3 domain, default tier
taskset -c <domain> python benches/workloads/run.py compare <family> -n 5 -p 5 -o ...
# [tiers] opt-in tiers: plain / default / default + RUSTIQUE_TIERS=<tier> interleaved, rotating order
taskset -c <domain> python tiers_compare.py <family> --sides default,interp -n 5 -p 5 -o ...
# [whole] 8 workloads, one setup()+run() per process (phase3/whole.py), plain vs RUSTIQUE=1, 4 rounds
# [pyperformance] Phase 0's method: PYTHONPATH=<clone>/python:<sitecustomize dir> RUSTIQUE_ENABLE=0|1, one run per domain
# [startup] repacked wheel in fresh `python -m venv --without-pip` venvs vs identical venvs without it (bare, other .pth)
taskset -c 30 python benches/dx/startup.py cells PLAIN RQ -r 300 --ir-runs 3; python benches/startup/decompose.py ...
# [instr/time/report/isolate-*] scoreboard, enabled tier, as Phase 0; [sbprobe] sb_probe.py on Phase 0's ten cells
# [gcquiet] benches/runtime/run_quiet.sh (the gc tier's addendum); [ftquiet] benches/ft/quietbox.sh; [est] benches/dx/est_validate.sh STEPS=est
# [t7] RUSTIQUE_EXCLUDE=copy run.py compare devtools/isort_stdlib data/state_snapshot -n 5 -p 5 (3.15.1t)
# [sbheap] sb_heap.py: 0-100 cells at heap offsets 0-3, plain and enabled (callgrind)
# the later commits: textfix.sh (445de19), final.sh (7fec7e4), re9.sh (9d0abae), p78.sh + pm_probe.sh (78751e2)
```

## 2. Builds, tests and target status (stage 1; T10)

Stage 1 ran four times (§0): on **`d40e7b9`** (wave A, 03:24–03:34Z), on **`445de19`** (text's
fingerprint follow-up, 03:39–04:06Z, `textfix/`), on **`7fec7e4`**, the final steady-state code
(04:24–04:51Z, `final/`), and on **`9d0abae`** (`re`'s rework, 05:08–05:31Z, `re9/`); builds and
status once more on **`78751e2`** (06:29–06:35Z, `p78/`). Fat-LTO release builds (`maturin develop --uv --release`, then
`maturin build --release` in the same target directory and `tools/repack_wheel.py`, so the wheel
holds the binary the tests ran), `RUSTIQUE_INTERP_REQUIRE=1`: 29–40 s each on 8 cores.

### 2.1 Tests

`pytest -q -p no:cacheprovider` (JUnit XML in `tests/`, `textfix/tests/`, `final/tests/`):

| build | `d40e7b9`: passed / failed / skipped (s) | `445de19` | `7fec7e4` | `9d0abae` |
|---|---|---|---|---|
| 3.15.1 | 2546 / **0** / 37 (154.5) | 2546 / **0** / 37 (161.0) | 2547 / **0** / 37 (156.3) | 2547 / **0** / 37 (165.0) |
| 3.15.1t | 2560 / **0** / 23 (185.7) | 2560 / **0** / 23 (210.5) | 2561 / **0** / 23 (191.2) | 2561 / **0** / 23 (216.8) |
| 3.14.9 | 2529 / **0** / 54 (155.8) | 2529 / **0** / 54 (164.6) | 2530 / **0** / 54 (157.4) | 2530 / **0** / 54 (159.0) |
| 3.14.9t | 2544 / **0** / 39 (161.2) | 2544 / **0** / 39 (177.9) | 2545 / **0** / 39 (162.9) | 2544 / **1** / 39 (160.6) |

- 2583 tests (2584 on `7fec7e4`: one added with the sort fix); 0 errors anywhere. Phase 0 ran
  5831 at `70c54fe`: the retired native types took most of the suite with them.
- Skips: the `set.__reduce__` tests on GIL builds (`not-needed` there), the free-threaded-only
  tests on GIL builds, 3.14-only skips, two `test_re` cases that need `CPYTHON_SRC`,
  `pytest-xdist` not installed (one test).
- The Step-0 rehearsal on `58a91e8` had one failure on all four builds,
  `test_gc_tier_callbacks.py::test_callbacks_identity_length_and_refcount`: the one-element list
  `[_after_fork_child]` that `rustique.enable()` registered through `os.register_at_fork`. Fixed
  before the run (`59cca45`, accel).
- **The one failure (`9d0abae`, 3.14.9t)**, verbatim:
  `tests/targets/test_posixpath.py::test_exit_release_and_daemon_threads` —
  `assert r.stdout.splitlines() == ["after True", "before True True 0"]` got
  `['after True', 'before True True 1']`: the atexit check counted one extra reference to
  `posixpath.__dict__` while three daemon threads were still calling into posixpath/pathlib.
  The full suite shared domain 0 with eight callgrind workers at the time. Alone, 15/15 passes on
  `9d0abae` and 15/15 on `7fec7e4` (3.14.9t). A test race, not a regression: fixed after the run
  as a test-only commit (`b2a12b1`: the threads stop, flag + join, before the count). The lead's
  dev-box suite on `78751e2`: 2550 / 2564 / 2533 / 2548 passed, 0 failed (not re-run here).
- **Rust** (`PYO3_PYTHON` = 3.15.1; on `d40e7b9` and again on `7fec7e4`): `cargo fmt --all
  --check` clean, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo test
  --workspace` **74 passed**, 0 failed (rustique-core 69, `float_repr_cpython` 1, rustique-py 4;
  6 doctests ignored).
- **Extension size (T8: ≤ 2.5 MB) — not met** (M): the repacked wheels' `_rustique` is
  3,634,144 B (cp315), 3,295,232 (cp315t), 3,556,976 (cp314), 3,205,888 (cp314t) on `9d0abae`
  (`7fec7e4`: 3,604,704 / 3,267,136 / 3,527,704 / 3,178,088; `d40e7b9`: 3,585,576 / 3,250,248 /
  3,508,448 / 3,161,280). Stripped, cp315 is still 2,970,320 B.
  The cp314 extension carries no interp handler sets and is 3.5 MB too, so the growth over
  engine's 2.33 MB is the wave's new groups more than the tier (*inferred*).

### 2.2 Target status on every interpreter the box has (T10)

`status_dump.py` with `RUSTIQUE_VALIDATE=eager` (every verdict final at enable) after importing
the module of every declared target: in the build venv for the four builds, and for the other
interpreters in a fresh venv with the release wheel of the same minor and ABI (what `pip install`
gives a user there). The tier columns repeat the dump with `RUSTIQUE_TIERS=interp,gc`; the last
column with `RUSTIQUE_UNVALIDATED_PATCH=1`. `d40e7b9` (`status/`); `445de19` (`textfix/status/`)
and `7fec7e4` (`final/status/`) give exactly the same table except where marked:

| interpreter (PBS build) | default targets installed | not installed (category) | `interp` tier (4 targets) | `gc` tier | `RUSTIQUE_UNVALIDATED_PATCH=1` |
|---|---:|---|---|---|---:|
| 3.15.1 | **110 / 111** | 1 `not-needed` (`set.__reduce__`) | 4 installed | installed | — |
| 3.15.1t | **104 / 111** | 7 `build` | 3 installed, `kwinit` `build` | installed | — |
| 3.15.2 | **110 / 111** | 1 `not-needed` | 4 installed | installed | 110 |
| 3.15.2t | **104 / 111** | 7 `build` | 3 installed, 1 `build` | installed | 104 |
| 3.15.0 | **108 / 111** | 2 `fingerprint` (`copy.copy`, `copy.deepcopy`), 1 `not-needed` | refused: "no verified interp handlers (verified releases: 3.15.1, 3.15.2)" | installed | 108 |
| 3.14.9, 3.14.10, 3.14.8, 3.14.6 | **105 / 111** | 5 `version` (pathlib properties, by design), 1 `not-needed` | refused (`version`) | installed | 105 |
| 3.14.5 | **105 / 111** | same | refused | refused: `fingerprint` (`gc.freeze`) | 105 |
| 3.14.1 | **105 / 111** | same | refused | refused: `layout` (`struct _gc_runtime_state` size 240, recorded 264) | 105 |
| 3.14.9t, 3.14.10t | **99 / 111** | 7 `build`, 5 `version` | refused | installed | 99 |
| 3.14.0 | **0 / 111** | 111 `unvalidated-patch` | refused | refused | 94 (`d40e7b9`) / **87** (`445de19`, `7fec7e4`: textwrap now refuses too) |
| 3.16.0a1, 3.16.0a1t | **0 / 111** | 111 `version` ("CPython 3.16 pre-release (0x31000a1)") | — | — | — |

- **T10 holds on this box** (M): ≥ 80 targets on every recorded 3.15.x (108–110 on GIL, 104 on
  t), ≥ 75 on every recorded 3.14.x (105; 99 on t), unvalidated patches and pre-releases refuse
  visibly with their category and a reason that names the remedy. Windows ≥ 48 and macOS/musl
  ≥ 77 cannot be measured on this box; they are CI's `platform-coverage` numbers
  (`docs/targets/dx.md` §6.2, not re-measured here).
- **3.16.0a1**: the cp315 wheel does not install (uv: "no wheels with a matching Python version
  tag"); `uv pip install <source tree>` builds it (uv treats the pre-release as satisfying
  `<3.16`, `docs/USAGE.md`) and every target then refuses `version`.
- **3.15.0, `re` (both commits)**: the ten `re` targets install (fingerprints checked at
  install match), but the first call of each validates helpers lazily and **degrades** all ten
  (`status()["degraded"]`: "fingerprint: re._compile: 6010666059431580 != expected
  da7d49c39bd21137", `re._compiler._optimize_charset` likewise): they fall back to CPython's
  code there. So 98 targets work on 3.15.0 (still ≥ 80). 0 degraded on 3.15.2, 3.14.1, 3.14.10
  (`textfix/status/degraded-re-probe.txt`, `re_probe.py`). **Gone on `9d0abae`** (`re`'s v3
  fingerprints): 0 degraded on 3.15.0 too.
- `set.__reduce__` is `not-needed` on GIL builds (it was a `build` refusal in the rehearsal
  before `2eb317d`).
- reach.py (accel's T10 tool, internal API) gives the same counts (`status/reach-*.json`).

### 2.3 The re-runs on `445de19`, `7fec7e4`, `9d0abae` and `78751e2`

`445de19` = `d40e7b9` + `7ccdd52` (the text groups fingerprint every CPython helper their kernels
reimplement; datetime/logging self-checks use the recorded v3 sets) + `f354797` (wheels.yml) and
docs. As the lead decided, stage 1 (builds, the four test suites, the status table), the text
groups' 15 conformance modules (§3) were re-run on it (its startup slot was superseded by
`9d0abae`); everything else stands on `d40e7b9`. It ran in its own tree (`/work/w2p3t`, `textfix.sh`) in domain 0, the
untimed lane, while the headline ran in domains 1–2. Results under `textfix/`. Differences
from `d40e7b9`: none in tests or installed counts; with `RUSTIQUE_UNVALIDATED_PATCH=1` on the
unrecorded 3.14.0, 87 instead of 94 targets install (the textwrap group now fingerprints the
long-word helper that changed in 3.14.1, which is what the opt-in is meant to catch).

`7fec7e4` = `445de19` + core's tiny-input fixes (`list.sort` of n < 2 returns at once, short `str`
lists stay native, `any()` of an empty dict answered natively), a test fix and docs/CI files. The
lead decided (§0) that the headline is re-measured on it as replicate 2 and that everything not
yet run when it landed (wave C, whole-process, startup, addenda) runs on it; stage 1, the
conformance modules of the changed groups and the callgrind scoreboard were re-run on it. Its
stage 1 gives the same status table as `445de19` and 0 failures (§2.1).

`9d0abae` = `7fec7e4` + `re`'s first-call validation rework (compiler validation deferred to the
4th compile, module functions validated at their first cache hit, v3 fingerprints, cheaper
pins). Re-run on it (`re9/`, domain 0 untimed, then domain 3): stage 1, the `re` group's 16
conformance modules, install costs, startup, whole-process runs. Status table identical to
`7fec7e4`'s; the 3.15.0 `re` degradation is gone (`re9/status/degraded-re-probe.txt`: none on
3.15.0, 3.15.2, 3.14.1, 3.14.10). Tests: one race on 3.14.9t (§2.1).

`78751e2` = `9d0abae` + `b2a12b1` (test only) + text's paths fix (`join` takes exact cached
pathlib paths natively; pathlib/fnmatch critical sections without refcount round trips on shared
arguments). Re-run on it (`p78/`, domains 1 and 3, 06:29–06:51Z): builds and status on the four
builds, the paths groups' 6 conformance modules (§3), the kernel table (§9.2) and a short headline
check (§4). The lead's full `pytest` on the dev box: 2550 / 2564 / 2533 / 2548 passed, 0 failed
(not re-run here).

## 3. Conformance, every default group at once (stage 2; T11)

`run_target_suites.py` with all 21 default groups enabled together (validation eager), each
module plain and enabled in fresh processes, outcomes compared per test id
(`conformance/all-groups-<build>.json`), `--expect-installed` = every default target the
build's status dump shows installed. Modules: wave 1's 27 and the wave-2 groups' 23 (the brief's
list plus `re`'s own list in `docs/targets/re.md`: `test_http_cookies`, `test_difflib`,
`test_string`, `test_shlex`, `test_gettext`, `test_csv`, `test_mailbox`). `CPYTHON_SRC` =
`Lib/test` of the running release.

**0 differences on all four builds** (`d40e7b9`): 18,210 test outcomes on 3.15.1 and 3.15.1t,
17,637 on 3.14.9 and 3.14.9t; every expected target installed; no crash or timeout. The text
groups' 15 modules on `445de19`: **0 differences**, 7,055 / 7,055 / 6,785 / 6,785 outcomes
(`textfix/conformance/`). On `7fec7e4`, the same 15 plus the four modules of the container and
builtin groups whose sort/`sorted`/`any` kernels changed (`test_list`, `test_tuple`, `test_sort`,
`test_builtin`): **0 differences**, 7,340 / 7,340 / 7,059 / 7,059 outcomes (`final/conformance/`).
On `9d0abae`, the `re` group's 16 modules of `docs/targets/re.md` (`test_re`, `test_email`,
`test_tokenize`, `test_configparser`, `test_fnmatch`, `test_textwrap`, `test_http_cookies`,
`test_difflib`, `test_string`, `test_shlex`, `test_argparse`, `test_gettext`, `test_glob`, `test_csv`,
`test_urllib`, `test_mailbox`): **0 differences**, 5,477 / 5,477 / 5,382 / 5,382 outcomes
(`re9/conformance/`). On `78751e2`, the paths groups' six (`test_posixpath`, `test_genericpath`,
`test_fnmatch`, `test_glob`, `test_pathlib`, `test_os`): **0 differences** on all four builds
(`p78/conformance/`). Those two runs exit 1 only on the harness's own expectation: it expects
every installed default target, and none of those modules imports `statistics`, so its five
targets stay `pending-import` — not a Rustique result.

| module | 3.15.1 | 3.15.1t | 3.14.9 | 3.14.9t |
|---|---|---|---|---|
| test_list | 68 pass, 1 skip; 0 diff | 68 pass, 1 skip; 0 diff | 67 pass, 1 skip; 0 diff | 67 pass, 1 skip; 0 diff |
| test_tuple | 38 pass; 0 diff | 38 pass; 0 diff | 38 pass; 0 diff | 38 pass; 0 diff |
| test_sort | 21 pass; 0 diff | 21 pass; 0 diff | 21 pass; 0 diff | 21 pass; 0 diff |
| test_builtin | 150 pass, 7 skip; 0 diff | 150 pass, 7 skip; 0 diff | 141 pass, 6 skip; 0 diff | 141 pass, 6 skip; 0 diff |
| test_json | 232 pass, 1 skip; 0 diff | 232 pass, 1 skip; 0 diff | 220 pass, 1 skip; 0 diff | 220 pass, 1 skip; 0 diff |
| test_float | 52 pass, 2 skip; 0 diff | 52 pass, 2 skip; 0 diff | 52 pass, 2 skip; 0 diff | 52 pass, 2 skip; 0 diff |
| test_format | 17 pass, 2 skip; 0 diff | 17 pass, 2 skip; 0 diff | 17 pass, 2 skip; 0 diff | 17 pass, 2 skip; 0 diff |
| test_strtod | 8 pass, 1 skip; 0 diff | 8 pass, 1 skip; 0 diff | 8 pass, 1 skip; 0 diff | 8 pass, 1 skip; 0 diff |
| test_fstring | 95 pass; 0 diff | 95 pass; 0 diff | 94 pass; 0 diff | 94 pass; 0 diff |
| test_copy | 84 pass; 0 diff | 84 pass; 0 diff | 81 pass; 0 diff | 81 pass; 0 diff |
| test_copyreg | 6 pass; 0 diff | 6 pass; 0 diff | 6 pass; 0 diff | 6 pass; 0 diff |
| test_dataclasses | 284 pass, 1 skip; 0 diff | 284 pass, 1 skip; 0 diff | 279 pass, 1 skip; 0 diff | 279 pass, 1 skip; 0 diff |
| test_strptime | 94 skip, 49 pass; 0 diff | 94 skip, 49 pass; 0 diff | 94 skip, 46 pass; 0 diff | 94 skip, 46 pass; 0 diff |
| test_datetime | 3614 pass, 54 skip; 0 diff | 3614 pass, 54 skip; 0 diff | 3560 pass, 54 skip; 0 diff | 3560 pass, 54 skip; 0 diff |
| test_time | 49 pass, 16 skip; 0 diff | 49 pass, 16 skip; 0 diff | 48 pass, 16 skip; 0 diff | 48 pass, 16 skip; 0 diff |
| test_posixpath | 93 pass, 3 skip; 0 diff | 93 pass, 3 skip; 0 diff | 91 pass, 3 skip; 0 diff | 91 pass, 3 skip; 0 diff |
| test_genericpath | 26 pass; 0 diff | 26 pass; 0 diff | 25 pass; 0 diff | 25 pass; 0 diff |
| test_fnmatch | 24 pass; 0 diff | 24 pass; 0 diff | 24 pass; 0 diff | 24 pass; 0 diff |
| test_glob | 20 pass, 2 skip; 0 diff | 20 pass, 2 skip; 0 diff | 22 pass, 2 skip; 0 diff | 22 pass, 2 skip; 0 diff |
| test_pathlib | 991 pass, 407 skip; 0 diff | 991 pass, 407 skip; 0 diff | 966 pass, 403 skip; 0 diff | 966 pass, 403 skip; 0 diff |
| test_os | 472 pass, 77 skip, 3 fail, 1 error; 0 diff | 472 pass, 77 skip, 3 fail, 1 error; 0 diff | 317 pass, 66 skip, 2 fail; 0 diff | 317 pass, 66 skip, 2 fail; 0 diff |
| test_logging | 283 pass, 4 skip; 0 diff | 283 pass, 4 skip; 0 diff | 274 pass, 3 skip; 0 diff | 274 pass, 3 skip; 0 diff |
| test_random | 114 pass, 2 skip; 0 diff | 114 pass, 2 skip; 0 diff | 114 pass, 2 skip; 0 diff | 114 pass, 2 skip; 0 diff |
| test_textwrap | 68 pass; 0 diff | 68 pass; 0 diff | 68 pass; 0 diff | 68 pass; 0 diff |
| test_urlparse | 78 pass, 5 skip; 0 diff | 78 pass, 5 skip; 0 diff | 74 pass, 5 skip; 0 diff | 74 pass, 5 skip; 0 diff |
| test_urllib | 104 pass, 2 skip; 0 diff | 104 pass, 2 skip; 0 diff | 104 pass, 2 skip; 0 diff | 104 pass, 2 skip; 0 diff |
| test_statistics | 400 pass; 0 diff | 400 pass; 0 diff | 400 pass; 0 diff | 400 pass; 0 diff |
| test_enum | 1083 pass, 4 skip; 0 diff | 1083 pass, 4 skip; 0 diff | 1077 pass, 4 skip; 0 diff | 1077 pass, 4 skip; 0 diff |
| test_collections | 120 pass; 0 diff | 120 pass; 0 diff | 116 pass; 0 diff | 116 pass; 0 diff |
| test_contextlib | 100 pass; 0 diff | 100 pass; 0 diff | 91 pass; 0 diff | 91 pass; 0 diff |
| test_contextlib_async | 70 pass; 0 diff | 70 pass; 0 diff | 58 pass; 0 diff | 58 pass; 0 diff |
| test_functools | 338 pass; 0 diff | 338 pass; 0 diff | 325 pass; 0 diff | 325 pass; 0 diff |
| test_re | 165 pass, 3 skip; 0 diff | 165 pass, 3 skip; 0 diff | 163 pass, 3 skip; 0 diff | 163 pass, 3 skip; 0 diff |
| test_email | 1816 pass, 1 skip; 0 diff | 1816 pass, 1 skip; 0 diff | 1808 pass, 1 skip; 0 diff | 1808 pass, 1 skip; 0 diff |
| test_tokenize | 132 pass; 0 diff | 132 pass; 0 diff | 132 pass; 0 diff | 132 pass; 0 diff |
| test_configparser | 378 pass, 5 skip; 0 diff | 378 pass, 5 skip; 0 diff | 378 pass, 5 skip; 0 diff | 378 pass, 5 skip; 0 diff |
| test_argparse | 1962 pass, 1 skip; 0 diff | 1962 pass, 1 skip; 0 diff | 1893 pass, 1 skip; 0 diff | 1893 pass, 1 skip; 0 diff |
| test_uuid | 103 pass, 17 skip; 0 diff | 103 pass, 17 skip; 0 diff | 103 pass, 17 skip; 0 diff | 103 pass, 17 skip; 0 diff |
| test_set | 644 pass; 0 diff | 644 pass; 0 diff | 630 pass; 0 diff | 630 pass; 0 diff |
| test_inspect | 373 pass, 7 skip, 1 fail, 1 error; 0 diff | 373 pass, 7 skip, 1 fail, 1 error; 0 diff | 356 pass, 7 skip; 0 diff | 356 pass, 7 skip; 0 diff |
| test_weakref | 136 pass, 2 skip, 1 fail; 0 diff | 136 pass, 2 skip, 1 fail; 0 diff | 135 pass, 2 skip, 1 fail; 0 diff | 135 pass, 2 skip, 1 fail; 0 diff |
| test_pickle | 1039 pass, 47 skip; 0 diff | 1039 pass, 47 skip; 0 diff | 960 pass, 55 skip; 0 diff | 960 pass, 55 skip; 0 diff |
| test_typing | 742 pass; 0 diff | 742 pass; 0 diff | 717 pass; 0 diff | 717 pass; 0 diff |
| test_http_cookies | 35 pass; 0 diff | 35 pass; 0 diff | 33 pass; 0 diff | 33 pass; 0 diff |
| test_difflib | 61 pass; 0 diff | 61 pass; 0 diff | 59 pass; 0 diff | 59 pass; 0 diff |
| test_string | 55 pass; 0 diff | 55 pass; 0 diff | 55 pass; 0 diff | 55 pass; 0 diff |
| test_shlex | 45 pass; 0 diff | 45 pass; 0 diff | 45 pass; 0 diff | 45 pass; 0 diff |
| test_gettext | 74 pass; 0 diff | 74 pass; 0 diff | 74 pass; 0 diff | 74 pass; 0 diff |
| test_csv | 136 pass, 4 skip; 0 diff | 136 pass, 4 skip; 0 diff | 130 pass, 4 skip; 0 diff | 130 pass, 4 skip; 0 diff |
| test_mailbox | 381 pass, 3 skip; 0 diff | 381 pass, 3 skip; 0 diff | 373 pass, 3 skip; 0 diff | 373 pass, 3 skip; 0 diff |

3.15.1: 18210 outcomes, 0 differences, ok=True, not installed={}, targets=21
3.15.1t: 18210 outcomes, 0 differences, ok=True, not installed={}, targets=21
3.14.9: 17637 outcomes, 0 differences, ok=True, not installed={}, targets=21
3.14.9t: 17637 outcomes, 0 differences, ok=True, not installed={}, targets=21

- Failures and errors that appear (`test_os` 3 + 1, `test_inspect` 1 + 1 on 3.15, `test_weakref`
  1) are the box's environment: identical plain and enabled.
- The 3.14 builds' wave-1 counts equal Phase 0's (and wave 1's) for every module of wave 1's 27.

## 4. The 65 workloads (the headline; stage 3; T1–T4)

`benches/workloads/run.py compare <family> -n 5 -p 5` in one venv per family (the repacked
release wheel; frameworks/libs/services installed `--no-deps` from their complete pins; web/data/
devtools package-for-package identical to Phase 0). Per workload, 5 interleaved pairs of fresh
processes (plain, `rustique.enable()`), each `setup()`, 1 warm-up and 5 timed `run()`s; each side
is the median of its 25 timed runs; the threads family at its default 8 threads. 3.15.1 in one
L3 domain and 3.15.1t in another, one timing job per domain, nothing untimed in either.
**Replicate 2 = `7fec7e4`, the headline** (3.15.1: domain 3, 04:51–05:42Z; 3.15.1t: domain 2,
05:43–06:35Z); **replicate 1 = `d40e7b9`** (domains 1 and 2, 03:34–04:26Z). The steady-state code
of the two differs only in core's tiny-input sort/`sorted`/`any` fixes. Phase 0 = wave-1 code on
the same box type (`w2-phase0.md` §4), the 34 wave-1 workloads only.

**Every digest matched: 0 mismatches in 4 × 65 workload comparisons.**

| family (n) | 3.15.1 | 3.15.1t | target (T4/T2/T3) | verdict |
|---|---:|---:|---|---|
| web (10) | 0.882 | 0.903 | — | |
| data (12) | 0.752 | 0.735 | — | |
| devtools (12) | 0.957 | 0.861 | — | |
| frameworks (6) | **0.940** | 0.955 | ≤ 0.98 (3.15.1) | **met** |
| libs (6) | **0.961** | 0.966 | ≤ 0.95 (3.15.1) | **not met** |
| custom (10) | **0.936** | 0.973 | ≤ 0.95 (3.15.1) | **met** |
| services (4) | **0.848** | 0.888 | ≤ 0.85 (3.15.1) | **met** (by 0.002) |
| threads (5, 8 threads) | 0.685 | **0.792** | ≤ 0.78 (3.15.1t) | **not met** |
| **wave 1's 34** | **0.858** | **0.826** | ≤ 0.85 / ≤ 0.80 (T2) | **not met** on either |
| the 31 new | 0.884 | 0.925 | — | |
| **all 65** | **0.870** | **0.872** | ≤ 0.88 / ≤ 0.84 (T3) | **met / not met** |

Both replicates and Phase 0 (geometric means):

| family (n) | 3.15.1 rep 2 | 3.15.1 rep 1 | 3.15.1 mean of 2 | 3.15.1 Phase 0 | 3.15.1t rep 2 | 3.15.1t rep 1 | 3.15.1t mean of 2 | 3.15.1t Phase 0 | target |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| web | **0.882** (10) | 0.886 | 0.884 | 0.919 | **0.903** (10) | 0.905 | 0.904 | 0.932 | — |
| data | **0.752** (12) | 0.749 | 0.751 | 0.768 | **0.735** (12) | 0.736 | 0.735 | 0.756 | — |
| devtools | **0.957** (12) | 0.955 | 0.956 | 0.962 | **0.861** (12) | 0.863 | 0.862 | 0.865 | — |
| frameworks | **0.940** (6) | 0.945 | 0.943 | — | **0.955** (6) | 0.957 | 0.956 | — | ≤ 0.98 |
| libs | **0.961** (6) | 0.960 | 0.961 | — | **0.966** (6) | 0.964 | 0.965 | — | ≤ 0.95 |
| custom | **0.936** (10) | 0.940 | 0.938 | — | **0.973** (10) | 0.975 | 0.974 | — | ≤ 0.95 |
| services | **0.848** (4) | 0.848 | 0.848 | — | **0.888** (4) | 0.885 | 0.887 | — | ≤ 0.85 |
| threads | **0.685** (5) | 0.687 | 0.686 | — | **0.792** (5) | 0.786 | 0.789 | — | t ≤ 0.78 |
| **wave 1's 34** | **0.858** (34) | 0.857 | 0.858 | 0.877 | **0.826** (34) | 0.827 | 0.827 | 0.843 | ≤ 0.85 / ≤ 0.80 |
| the 31 new | **0.884** (31) | 0.886 | 0.885 | — | **0.925** (31) | 0.924 | 0.925 | — | — |
| **all 65** | **0.870** (65) | 0.871 | 0.871 | 0.877 | **0.872** (65) | 0.872 | 0.872 | 0.843 | ≤ 0.88 / ≤ 0.84 |

- **T1 (workloads ≤ 1.02)**: met. Worst `mini_interp` 1.006 (3.15.1) and 1.018 (3.15.1t); no
  workload above 1.02 in either replicate. Medians 0.970 / 0.976; 26 / 25 workloads ≤ 0.95.
- **The replicates agree**: every family within 0.006, all 65 within 0.001; per workload, all 65
  within 2% on 3.15.1t and 63 of 65 on 3.15.1 (`billing_engine` +3.6%, `job_worker` −2.4%). Plain
  medians, replicate 2 ÷ 1: 1.001 on both builds; against Phase 0 (the 34): 0.998 / 0.999. So
  the numbers compare directly across the three runs (*inferred* from that agreement).
- **Against Phase 0 (wave-1 code)** on the 34: 0.877 → 0.858 and 0.843 → 0.826. The wave-2 gains
  there (3.15.1; 3.15.1t alike): `report_render` 0.438 → 0.357, `logging_service` 0.813 → 0.686,
  `json_api` 0.835 → 0.717, `flask_jinja` 0.997 → 0.958, `tz_events` 0.588 → 0.558,
  `fs_inventory` 0.739 → 0.720, `log_analytics` 0.591 → 0.578, `pytest_networkx` 0.954 → 0.939.
- **`78751e2`** (text's posixpath/pathlib fix, landed during wave C): `fs_inventory` 0.724 /
  0.718 and `pytest_networkx` 0.934 / 0.941 on 3.15.1 / 3.15.1t (replicate 2: 0.720 / 0.718 and
  0.939 / 0.942), `run.py compare -n 5 -p 5` (`p78/workloads/`): the headline does not move.

Per workload (ms = median steady-state `run()`; ratio = enabled ÷ plain, replicate 2):

| workload | 3.15.1 plain ms | 3.15.1 enabled ms | 3.15.1 **ratio** | 3.15.1 rep 1 | 3.15.1 Phase 0 | 3.15.1t plain ms | 3.15.1t enabled ms | 3.15.1t **ratio** | 3.15.1t rep 1 | 3.15.1t Phase 0 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| web/aiohttp_app | 273.9 | 246.0 | 0.898 | 0.909 | 0.906 | 301.3 | 280.5 | 0.931 | 0.932 | 0.932 |
| web/asyncio_http | 231.4 | 183.5 | 0.793 | 0.796 | 0.790 | 242.7 | 195.4 | 0.805 | 0.812 | 0.807 |
| web/django_blog | 489.1 | 481.4 | 0.984 | 0.988 | 0.996 | 545.6 | 538.8 | 0.988 | 0.997 | 0.995 |
| web/email_mime | 657.5 | 654.0 | 0.995 | 0.989 | 0.983 | 704.8 | 711.6 | 1.010 | 1.012 | 1.001 |
| web/fastapi_pydantic | 296.6 | 283.2 | 0.955 | 0.960 | 0.978 | 337.5 | 328.9 | 0.975 | 0.969 | 0.977 |
| web/flask_jinja | 418.7 | 401.0 | 0.958 | 0.964 | 0.997 | 459.9 | 447.4 | 0.973 | 0.970 | 0.994 |
| web/json_api | 393.9 | 282.4 | 0.717 | 0.714 | 0.835 | 439.4 | 333.9 | 0.760 | 0.752 | 0.839 |
| web/logging_service | 282.3 | 193.6 | 0.686 | 0.691 | 0.813 | 301.1 | 219.7 | 0.730 | 0.742 | 0.865 |
| web/requests_local | 251.4 | 229.1 | 0.911 | 0.921 | 0.930 | 250.5 | 231.0 | 0.922 | 0.928 | 0.944 |
| web/sqlalchemy_orm | 344.2 | 342.0 | 0.994 | 1.000 | 0.999 | 354.8 | 353.6 | 0.997 | 0.996 | 0.997 |
| data/backup_archive | 598.3 | 596.2 | 0.997 | 0.998 | 0.997 | 606.4 | 606.3 | 1.000 | 0.999 | 1.000 |
| data/billing_engine | 899.3 | 889.7 | 0.989 | 0.955 | 0.966 | 751.6 | 715.3 | 0.952 | 0.954 | 0.975 |
| data/config_merge | 541.5 | 533.7 | 0.986 | 0.993 | 1.001 | 608.7 | 606.8 | 0.997 | 0.991 | 0.990 |
| data/csv_etl | 390.8 | 242.5 | 0.621 | 0.611 | 0.618 | 429.5 | 271.4 | 0.632 | 0.637 | 0.644 |
| data/fs_inventory | 314.7 | 226.7 | 0.720 | 0.721 | 0.739 | 320.5 | 230.2 | 0.718 | 0.720 | 0.736 |
| data/log_analytics | 765.6 | 442.6 | 0.578 | 0.578 | 0.591 | 808.6 | 475.4 | 0.588 | 0.586 | 0.609 |
| data/pandas_etl | 529.9 | 527.8 | 0.996 | 0.998 | 0.998 | 591.0 | 589.7 | 0.998 | 0.997 | 0.995 |
| data/report_render | 407.1 | 145.5 | 0.357 | 0.356 | 0.438 | 463.3 | 162.0 | 0.350 | 0.351 | 0.423 |
| data/state_snapshot | 644.3 | 424.2 | 0.658 | 0.650 | 0.641 | 1227.7 | 587.0 | 0.478 | 0.477 | 0.474 |
| data/text_corpus | 606.8 | 603.2 | 0.994 | 1.000 | 0.999 | 631.5 | 631.1 | 0.999 | 1.000 | 1.007 |
| data/tz_events | 529.1 | 295.0 | 0.558 | 0.565 | 0.588 | 612.6 | 363.3 | 0.593 | 0.591 | 0.622 |
| data/xml_feeds | 505.0 | 505.3 | 1.001 | 0.998 | 0.990 | 570.9 | 571.0 | 1.000 | 1.000 | 1.001 |
| devtools/black_format | 1193.8 | 1173.0 | 0.983 | 0.990 | 0.996 | 1300.7 | 1281.5 | 0.985 | 0.994 | 0.993 |
| devtools/coverage_pyflakes_tests | 381.3 | 380.5 | 0.998 | 0.990 | 0.993 | 493.2 | 495.0 | 1.004 | 1.000 | 1.000 |
| devtools/import_startup | 1883.4 | 1879.4 | 0.998 | 1.001 | 1.000 | 2242.5 | 2240.1 | 0.999 | 0.999 | 1.001 |
| devtools/isort_stdlib | 283.8 | 217.7 | 0.767 | 0.760 | 0.769 | 1148.0 | 231.2 | 0.201 | 0.203 | 0.203 |
| devtools/pip_resolve | 561.0 | 528.1 | 0.941 | 0.939 | 0.943 | 626.9 | 587.4 | 0.937 | 0.940 | 0.952 |
| devtools/pycodestyle_stdlib | 1015.4 | 986.0 | 0.971 | 0.967 | 0.978 | 1069.4 | 1066.9 | 0.998 | 0.995 | 0.995 |
| devtools/pyflakes_stdlib | 1208.4 | 1210.6 | 1.002 | 0.987 | 0.994 | 1194.8 | 1198.4 | 1.003 | 0.995 | 1.010 |
| devtools/pylint_stdlib | 1185.3 | 1176.9 | 0.993 | 0.997 | 0.999 | 894.9 | 893.8 | 0.999 | 0.998 | 0.995 |
| devtools/pytest_networkx | 635.2 | 596.2 | 0.939 | 0.926 | 0.954 | 631.8 | 594.9 | 0.942 | 0.944 | 0.951 |
| devtools/rich_render | 285.6 | 278.6 | 0.975 | 0.976 | 0.994 | 310.5 | 308.9 | 0.995 | 0.991 | 0.997 |
| devtools/sphinx_autodoc | 856.5 | 838.0 | 0.978 | 0.981 | 0.988 | 899.7 | 883.8 | 0.982 | 1.002 | 0.980 |
| devtools/sqlglot_transpile | 879.9 | 853.4 | 0.970 | 0.972 | 0.962 | 1033.9 | 1007.9 | 0.975 | 0.975 | 0.976 |
| frameworks/django_admin | 1001.8 | 970.1 | 0.968 | 0.975 | — | 1129.4 | 1102.6 | 0.976 | 0.982 | — |
| frameworks/drf_api | 1172.1 | 1159.4 | 0.989 | 0.996 | — | 1352.3 | 1343.9 | 0.994 | 0.993 | — |
| frameworks/falcon_api | 388.3 | 308.5 | 0.795 | 0.788 | — | 428.2 | 349.7 | 0.817 | 0.814 | — |
| frameworks/fastapi_sqlalchemy | 1218.0 | 1204.9 | 0.989 | 1.002 | — | 1373.1 | 1358.7 | 0.989 | 0.997 | — |
| frameworks/flask_marshmallow | 740.9 | 719.0 | 0.970 | 0.975 | — | 819.8 | 801.7 | 0.978 | 0.988 | — |
| frameworks/strawberry_graphql | 920.5 | 868.7 | 0.944 | 0.954 | — | 1091.3 | 1077.7 | 0.988 | 0.980 | — |
| libs/attrs_cattrs | 802.6 | 680.1 | 0.847 | 0.856 | — | 877.4 | 767.8 | 0.875 | 0.877 | — |
| libs/boto3_client | 1500.4 | 1457.6 | 0.971 | 0.971 | — | 1647.5 | 1560.3 | 0.947 | 0.947 | — |
| libs/bs4_scrape | 1107.1 | 1077.8 | 0.974 | 0.978 | — | 1158.2 | 1158.4 | 1.000 | 0.991 | — |
| libs/jsonschema_validate | 1366.9 | 1355.8 | 0.992 | 0.975 | — | 1473.6 | 1467.9 | 0.996 | 0.982 | — |
| libs/pydantic_payloads | 647.3 | 640.7 | 0.990 | 0.992 | — | 748.3 | 741.9 | 0.991 | 0.995 | — |
| libs/yaml_config | 1027.9 | 1030.2 | 1.002 | 0.997 | — | 1127.7 | 1117.4 | 0.991 | 0.996 | — |
| custom/di_plugins | 781.1 | 747.3 | 0.957 | 0.959 | — | 826.8 | 818.0 | 0.989 | 0.982 | — |
| custom/ecs_arena | 951.9 | 931.3 | 0.978 | 0.990 | — | 1074.0 | 1065.4 | 0.992 | 1.000 | — |
| custom/event_ledger | 1357.1 | 1204.2 | 0.887 | 0.881 | — | 1553.1 | 1501.2 | 0.967 | 0.975 | — |
| custom/fulfilment_sim | 1238.0 | 1201.5 | 0.971 | 0.977 | — | 1357.1 | 1320.1 | 0.973 | 0.971 | — |
| custom/mini_interp | 1087.4 | 1093.5 | 1.006 | 1.003 | — | 1219.5 | 1241.6 | 1.018 | 1.015 | — |
| custom/order_domain | 1285.3 | 1233.5 | 0.960 | 0.969 | — | 1400.6 | 1372.5 | 0.980 | 0.986 | — |
| custom/orm_mapper | 800.9 | 752.7 | 0.940 | 0.939 | — | 854.2 | 808.4 | 0.946 | 0.951 | — |
| custom/report_engine | 949.3 | 800.5 | 0.843 | 0.854 | — | 1034.8 | 933.7 | 0.902 | 0.901 | — |
| custom/rules_engine | 862.2 | 728.4 | 0.845 | 0.849 | — | 1086.2 | 1050.9 | 0.968 | 0.972 | — |
| custom/task_planner | 488.0 | 483.8 | 0.991 | 0.993 | — | 532.0 | 531.9 | 1.000 | 1.001 | — |
| services/cache_service | 984.0 | 986.9 | 1.003 | 1.003 | — | 1040.2 | 1036.3 | 0.996 | 0.998 | — |
| services/job_worker | 625.1 | 336.5 | 0.538 | 0.552 | — | 296.5 | 202.4 | 0.682 | 0.682 | — |
| services/uvicorn_fastapi | 959.2 | 922.9 | 0.962 | 0.959 | — | 1011.5 | 1003.6 | 0.992 | 0.976 | — |
| services/waitress_flask | 1480.9 | 1471.0 | 0.993 | 0.975 | — | 185.2 | 170.5 | 0.921 | 0.925 | — |
| threads/compute_pool | 1197.7 | 1171.8 | 0.978 | 0.974 | — | 173.1 | 169.9 | 0.982 | 0.983 | — |
| threads/http_server | 631.7 | 579.0 | 0.917 | 0.919 | — | 87.2 | 79.1 | 0.906 | 0.893 | — |
| threads/pipeline_queue | 593.5 | 304.8 | 0.513 | 0.513 | — | 266.3 | 190.7 | 0.716 | 0.711 | — |
| threads/request_pool | 455.6 | 249.3 | 0.547 | 0.550 | — | 140.8 | 109.5 | 0.778 | 0.780 | — |
| threads/shared_cache | 525.5 | 314.3 | 0.598 | 0.606 | — | 228.2 | 143.1 | 0.627 | 0.616 | — |

## 5. Opt-in tiers (stage 4; T5, T6) — `d40e7b9`

`tiers_compare.py`: per workload, 5 rounds; each round runs plain, the default tier
(`rustique.enable()`) and the default tier plus `RUSTIQUE_TIERS=<tier>` once each in fresh
processes (`run.py one`: setup, 1 warm-up, 5 timed runs), the order rotating by one per round;
medians of the 25 timed runs per side. Every enabled child records its tier targets' states
after the timed runs: **the tier was installed in every process** (`interp:installed` 4 on
3.15.1; `gc:installed` 1 on both builds) and no target of a tier refused or suspended. Digests:
0 mismatches. One timing job per domain: +gc 3.15.1 in domain 3 (03:34–04:49Z), +interp 3.15.1
in domain 1 (04:25–05:40Z), +gc 3.15.1t in domain 2 (04:26–05:43Z), +both 3.15.1 in domain 1
(05:40–06:19Z). The "default" column is measured again in the same rounds (it agrees with the headline
within 0.01 per family).

| family (n) | 3.15.1 +interp ÷ plain | +interp ÷ default | 3.15.1 +gc ÷ plain | +gc ÷ default | 3.15.1t +gc ÷ plain | +gc ÷ default |
|---|---:|---:|---:|---:|---:|---:|
| web (10) | 0.878 | 0.993 | 0.885 | 1.000 | 0.907 | 1.001 |
| data (12) | 0.742 | 0.989 | 0.739 | 0.988 | 0.723 | 0.981 |
| devtools (12) | 0.941 | 0.983 | 0.958 | 1.000 | 0.861 | 1.001 |
| frameworks (6) | **0.933** | 0.986 | 0.945 | 1.002 | 0.959 | 0.996 |
| libs (6) | 0.944 | 0.981 | 0.965 | 1.003 | 0.967 | 1.000 |
| custom (10) | **0.920** | 0.987 | 0.935 | 0.999 | 0.963 | 0.989 |
| services (4) | 0.829 | 0.975 | 0.837 | 0.997 | 0.884 | 1.003 |
| threads (5) | 0.682 | 0.996 | 0.687 | 1.001 | 0.761 | 0.966 |
| **all (65)** | **0.859** | **0.987** | **0.868** | **0.998** | **0.866** | **0.992** |

- **`interp`** (T5: frameworks ≤ 0.96, custom ≤ 0.90 against plain): frameworks **0.933, met**;
  custom **0.920, not met** (the tier adds 0.987 on top of the default tier's 0.932). Best over
  the default tier: `sqlglot_transpile` 0.917, `waitress_flask` 0.925, `order_domain` 0.928
  (0.887 against plain), `strawberry_graphql` 0.967, `ecs_arena` 0.968, `orm_mapper` 0.966.
  Where the data-model kernels already took the machinery (`rules_engine`, `event_ledger`,
  `report_engine`: 0.84–0.87 with the default tier), the tier adds nothing (0.995–1.005). Worst:
  `di_plugins` 1.017, `email_mime` 1.016 over the default tier — none above 1.02. CPython's full
  regression suite under the tier (T5's other half) was not re-run here: `docs/targets/interp.md`
  §5.1 (dev box, not re-measured).
- **`gc`**: over the default tier 0.998 (3.15.1) and 0.992 (3.15.1t), never worse than 1.011;
  the gains are where the GC is: `billing_engine` 0.870 (3.15.1) / 0.905 (t), `rules_engine`
  0.967 / 0.908, `request_pool` 0.851 (t), `xml_feeds` 0.946 (t), `state_snapshot` 0.954 (t). The
  threads family on 3.15.1t reaches **0.761** with the tier (0.788 without). T6's own numbers
  (the > 10%-GC set, RSS, pauses) are the gc addendum's (§9.1).
- **+both** (`RUSTIQUE_TIERS=gc,interp`, 3.15.1, the three families the brief names; 28
  workloads): against plain frameworks **0.936**, custom **0.922**, devtools 0.942; against the
  default tier 0.990 / 0.982 / 0.986, all 28 0.985; worst `di_plugins` 1.018. Best:
  `order_domain` 0.907 over the default tier (0.893 against plain), `sqlglot_transpile` 0.919
  (0.887), `orm_mapper` 0.960, `rules_engine` 0.968 (**0.820** against plain).

## 6. pyperformance 1.14.0 (stage 5; T12) — `7fec7e4`

Phase 0's method: driver and benchmark venvs per build created fresh (`pyperformance venv create`),
plain vs `rustique.enable()` (the default tier) injected through `phase0/sitecustomize.py`
(`PYTHONPATH=<clone>/python:<site>`, `RUSTIQUE_ENABLE=0|1`), the full default suite. **One pair per
build, one run per domain**: 3.15.1 plain in domain 0 and enabled in domain 1, 3.15.1t plain in
domain 2 and enabled in domain 3, all four at once, 06:51–07:54Z. Then, alone, the three port
benchmarks (domain 1, 07:54–08:04Z; they collide on fixed ports when two runs are concurrent),
and in domain 2 (08:04–08:11Z) `sphinx` on 3.15.1t (its plain run collided with the concurrent
enabled run on the driver's shared `bm_sphinx/data/Doc/build` directory: `OSError: [Errno 39]
Directory not empty` in `shutil.rmtree`, plain side) and the two benchmarks above 1.05, two pairs
each in opposite order on both builds (`final/pyperformance/`; ratios are means, enabled ÷ plain).

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| benchmarks | 122 | 122 |
| **geometric mean** | **0.911** | **0.936** |
| Phase 0 (wave-1 code), same benchmarks | 0.942 | 0.955 |
| T12 | ≤ 0.93: **met** | ≤ 0.94: **met** |
| median | 0.994 | 0.997 |
| < 0.95 / > 1.02 / > 1.05 | 15 / 5 / 2 | 12 / 5 / 2 |

- **New in wave 2** (3.15.1 / 3.15.1t): `regex_compile` **0.044 / 0.122** (Phase 0 1.02: the Rust
  `re` compiler), `logging_format` 0.513 / 0.643 and `logging_simple` 0.520 / 0.671 (Phase 0
  0.66 / 0.78), and on 3.15.1 `regex_v8` 0.943, `nqueens` 0.933, `k_core` 0.955, `mdp` 0.963,
  `regex_dna` 0.971, `sphinx` 0.961. Wave 1's wins are unchanged (`deepcopy_memo` 0.132 / 0.183,
  `deepcopy` 0.234 / 0.280, `json_dumps` 0.250 / 0.319, `json_loads` 0.578 / 0.613).
- **Above 1.05**: `python_startup` 1.074 / 1.110 (the `enable()` in sitecustomize; Phase 0
  1.092 / 1.100). `unpack_sequence` 1.068 on 3.15.1 in the full run measures **0.972 / 0.994**
  alone: noise. **`coroutines` on 3.15.1t: 1.073 in the full run and 1.073 / 1.080 alone**
  (3.15.1: 1.014 / 1.019; Phase 0 on 3.15.1t: 1.010). It is pure Python (recursive `async def`,
  no target). Investigated (`final/coro/`): under pyperf directly, plain 17.7 / 17.3 ms vs enabled
  18.6 / 18.9 ms; with `RUSTIQUE_EXCLUDE` set to either half of the 21 groups the slowdown
  disappears (17.53, 17.41 vs plain 17.32), so no single group causes it; the same body outside
  pyperf's worker shows none (17.1–17.3 vs 17.2 ms) and makes 0 GC collections, while one
  standalone 3.15.1 process differed from its twins by 10%. *Inferred*: a code/stack-layout
  effect of deep coroutine recursion, not a kernel. Reported; flagged for the next wave.
- Between 1.02 and 1.05: `unpickle_pure_python`, `urlsafe_base64_small`, `bench_mp_pool`
  (3.15.1); `base32_small`, `bench_mp_pool`, `urlsafe_base64_small` (3.15.1t).
- `genshi` fails on plain CPython 3.15 (as in Phase 0), so every full run exits 1.
## 7. Install and startup (T8, T9) — `9d0abae` (install also `7fec7e4`)

### 7.1 Install costs (T9), `benches/accel/install_costs.py`

Accel's tool: callgrind regions in fresh processes, GC off in the region; per module, `import M`
with every group pending (`lazy`), the same plus one call of a target of M (`lazy+call`: under
first-call validation that call pays the validation), against plain `import M`. Budget (T9):
max(5% of the module's import, 0.1M Ir). Measured on **`9d0abae`** (`re9/startup/`), 3.15.1 and
3.14.9, and before that on `7fec7e4` (`final/startup/`, 3.15.1):

| `import M` (3.15.1) | budget | `7fec7e4` lazy | **`9d0abae` lazy** | `9d0abae` lazy+call | 3.14.9 `9d0abae` lazy / lazy+call |
|---|---:|---:|---:|---:|---:|
| json | +1.11M | 1.002 (+43k) | 1.045 (+992k) | 1.050 (**+1.114M**, 3k over) | 1.045 / 1.051 (**+1.136M**, 12k over) |
| copy | +0.20M | 1.017 | 1.017 (+69k) | 1.039 (+158k) | 1.020 / 1.041 |
| logging | +2.63M | 0.925 | 0.968 | 0.971 | 0.980 / 0.982 |
| random | +0.17M | 1.025 | 1.025 (+87k) | 1.083 (**+288k**) | 1.026 / 1.088 (**+289k**) |
| pathlib | +0.46M | 1.023 | 1.023 (+215k) | 1.051 (**+468k**, 7k over) | 1.035 / 1.042 |
| fnmatch | +0.93M | **1.082 (+1.52M)** | 1.043 (+795k) | 1.047 (+868k) | 1.042 / 1.046 |
| textwrap | +1.05M | 0.972 | 1.038 (+806k) | 1.045 (+956k) | 1.038 / 1.045 |
| statistics | +1.62M | 0.883 | 1.029 | 1.034 | 1.028 / 1.034 |
| urllib.parse | +1.42M | **1.054 (+1.53M)** | 1.028 (+798k) | 1.035 (+1.00M) | 1.028 / 1.035 |
| dataclasses | +0.57M | 1.016 | 1.016 | 1.016 | 1.021 / 1.021 |
| datetime | +0.10M (abs.) | 1.178 (+76k) | 1.178 (+76k) | 1.199 (+86k) | 1.184 / 1.207 (+86k) |

- **`7fec7e4`**: two lazy installs over budget, `fnmatch` (+1.52M vs +0.93M) and
  `urllib.parse` (+1.53M vs +1.42M): the first regex compile after `enable()` validated the `re`
  group (~1M Ir of fingerprints and prepare) inside whichever module's import compiled first
  (`final/startup/install-split-3.15.1.txt`: `import urllib.parse` after `import rustique` 1.053
  with every group, 0.951 without `re`). Reported to the lead; `9d0abae` reworked it (compiler
  validation deferred to the 4th compile, module functions at their first cache hit).
- **`9d0abae`: every lazy install is within its budget** on 3.15.1 and 3.14.9 (T9 as worded:
  **met**). The `re` validation now lands in the first module that compiles four patterns —
  `import json` here (+0.99M, 1.045, inside its 1.11M budget). Counting the first call as part of
  the install (first-call validation's cost), three cells exceed: `random` (+0.29M vs +0.17M,
  both versions), `json` (+3k / +12k over) and `pathlib` on 3.15.1 (+7k over).

### 7.2 Startup cells (T8) — `9d0abae`

`benches/dx/startup.py cells`: the repacked release wheel installed (`uv pip install --no-deps`)
into a fresh `python -m venv --without-pip` venv, against an identical venv without it; "other
.pth" = both venvs carry a comment-only `.pth`. Callgrind whole-process Ir (median of 3,
`PYTHONHASHSEED=0`) and wall time (median, p10–p90 of 300 interleaved rounds), pinned to CPU
30, nothing else timing in domain 3 (06:12–06:16Z). Ratios against `python -c pass` in the
plain venv (`re9/startup/cells-*.json`):

| cell (Ir / wall) | 3.15.1 bare | 3.15.1 other .pth | 3.14.9 bare | 3.14.9 other .pth |
|---|---|---|---|---|
| installed, `RUSTIQUE` unset | 1.027 / 1.033 | 1.008 / 1.011 | 1.047 / 1.050 | 1.014 / 1.016 |
| `RUSTIQUE=1` | 1.056 / 1.095 | 1.036 / 1.070 | 1.077 / 1.129 | 1.043 / 1.093 |
| `import rustique` | 1.063 / 1.088 | 1.043 / 1.064 | 1.079 / 1.108 | 1.045 / 1.073 |
| `import rustique; rustique.enable()` | 1.092 / 1.127 | 1.072 / 1.101 | 1.107 / 1.148 | 1.072 / 1.110 |
| **`RUSTIQUE=1` vs unset, same venv (T8)** | **1.028 / 1.059** (+0.58 ms) | **1.028 / 1.059** | **1.029 / 1.075** (+0.75 ms) | **1.029 / 1.076** |
| Phase 0 (wave-1 code), same row | 1.045 / 1.077 | 1.045 / 1.076 | 1.063 / 1.107 | 1.063 / 1.109 |

- **T8: instructions met** (1.028–1.029 ≤ 1.05 on both versions), **wall not met** (1.059 on
  3.15.1, 1.075 on 3.14.9; Phase 0: 1.077 / 1.107). Extension size not met (§2.1:
  3,634,144 B for cp315 on `9d0abae`).
- Installed but off stays ≤ 1.05 everywhere except the documented upstream floor (bare 3.14.9
  venv, 1.047 / 1.050: the `utf-8-sig` decode any `.pth` costs, w2-startup §1).
- Decomposition of `RUSTIQUE=1` − unset on 3.15.1 (`benches/startup/decompose.py`, 1,128,021 Ir,
  1.0284×): `startup()` (the quiet enable) 35.0%, `dlopen` + relocations + module object 32.4%,
  locating and importing `_rustique` 18.7%, GC/teardown 8.3%, exit hook 4.7%, the module exec slot
  0.9%; no loader string (0). Wall +0.57 ms, cycles 1.054, page faults +110
  (`re9/startup/decompose-*.json`).

## 8. Scoreboard (enabled tier = default tier)

### 8.1 0–100 elements, callgrind instruction counts (3814 cells per build; T1) — `d40e7b9`, `7fec7e4`

`scoreboard.py instr --impls enabled`, 8 callgrind shards in domain 0 (untimed lane), then every
cell above 1.05 again alone in its own callgrind worker, 3 runs (`sb_isolate.py`; identical Ir in
all runs). Measured on `d40e7b9` (03:34–03:59Z) and again on `7fec7e4` (04:51–05:14Z), which
carries core's tiny-input fixes made in reaction to the first.

| | 3.15.1 `d40e7b9` | 3.15.1 **`7fec7e4`** | 3.15.1t `d40e7b9` | 3.15.1t **`7fec7e4`** | Phase 0 (wave-1 code) |
|---|---:|---:|---:|---:|---|
| cells > 1.05 as run | 20 | 12 | 7 | **0** | 6 / 2 |
| cells > 1.05 alone | **9** | **0** | **4** | **0** | 0 / 0 |
| cells ≤ 1.05 after isolation | 3805 / 3814 | **3814 / 3814** | 3810 / 3814 | **3814 / 3814** | 3814 / 3814 |

The nine on `d40e7b9` (3.15.1; the four on 3.15.1t are the first two rows):

| cell | Phase 0 | `d40e7b9` alone (Ir) | `7fec7e4` as run | what it was |
|---|---:|---:|---:|---|
| `list/x.sort()/str/1` | 0.824 | **1.132** (363 → 411) | **0.766** | core's sort fix handed short `str` lists to CPython after a pre-check; fixed (n < 2 returns at once, short `str` lists native) |
| `dict`/`set`/`frozenset` `sorted(x)/str/1` | 0.957–0.969 | **1.072–1.073** (+85) | **0.959** | same |
| `dict/any(x)/float/0` | 1.029 | **1.074** | **0.464** | `any({})` now answered natively |
| `dict/from dict/str/100`, `dict/copy C(x)/str/100` | 1.000 | **1.084** (+453) | ≤ 1.05 | no kernel: pymalloc pool position (below) |
| `dict/for v in x/{object,mixed}/0` | 1.000 | **1.056** (+28) | ≤ 1.05 | no kernel: pool position |
| (`dict/x.copy()/str/100` 1.043, `dict/x \| t/int/0` 1.046 alone) | 1.000 | | ≤ 1.05 | no kernel: pool position |

Also on `d40e7b9`: `list.sort()` of 8 `str` 1.021 (Phase 0 0.835) — fixed: **0.658** on `7fec7e4`;
`sorted()` of a 1-element list/tuple of `str` 0.61 (Phase 0 0.97).

**Heap offsets** (`sb_heap.py`, the lead's request after core's finding): every cell that read
> 1.05 as run on `7fec7e4` (12) and the six GIL-only dict cells of `d40e7b9`, each plain and
enabled at heap offsets 0–3 (k junk objects of each of nine small size classes before the
measured region), Ir per execution, a fresh callgrind process per (cell, mode, offset)
(`final/scoreboard/heap-instr-3.15.1-asrun.txt`). **Plain CPython moves by the same amounts as
the enabled process**: `dict(x)` of 100 `str` keys 5,130 ↔ 5,355 Ir plain and 5,130 ↔ 5,357
enabled depending on the offset; `tuple` `hash` 399–413 plain, 404 enabled; empty-dict
iteration 442–461 both. Ratio at offset 0 ≤ 1.013 and ratio of the minima ≤ 1.013 for all 18:
**none is Rustique's**. On `7fec7e4` no cell is left above 1.05 alone, so the 0–100 part of T1
holds on both builds.

### 8.2 10k/1M elements, wall time (3.15.1) — `7fec7e4`

Phase 0's method: `scoreboard.py time --size 1e4,1e6`, pyperf medians of 3 processes × 3 values,
4 single-CPU shards on CPUs 2, 10, 20, 30 with nothing else on the box (09:18–10:15Z), then every
cell above 1.05 again alone on one CPU, 10 processes × 5 values, 2 runs (`sb_isolate.py`,
10:15–10:39Z; `final/scoreboard/`). Every result verified against plain CPython's: 0
mismatches, 0 errors. 3.15.1t's wall-time board was not run (as in Phase 0).

| | 3.15.1 `7fec7e4` | Phase 0 (wave-1 code) |
|---|---|---|
| cells ≤ 1.05, as run | 1767 / 1846 | 1773 / 1846 |
| cells ≤ 1.05 after re-measuring every cell > 1.05 alone | **1838 / 1846** | 1836 / 1846 |
| geomean, all | **0.826** | 0.828 |
| kernel path (328): geomean / > 1.05 as run | **0.452** / 10 | 0.461 / 2 |
| everything else (1518): geomean / median | 0.941 / 1.000 | 0.940 / 0.999 |

**Still above 1.05 alone: 8 cells, 4 of them on the kernel path — a T1 miss.**

| cell | as run | alone (run 1, run 2) | Phase 0 as run | `sb_probe` enabled ÷ imported (default / min over heap offsets) | verdict |
|---|---:|---|---:|---|---|
| `set/sorted(x)/int/1M` | 1.264 | 1.266, 1.256 | 0.869 | 1.259 / 1.259 | **kernel: slower** |
| `set/sorted(x)/float/1M` | 1.264 | 1.259, 1.241 | 0.906 | 1.234 / 1.238 | **kernel: slower** |
| `frozenset/sorted(x)/int/1M` | 1.256 | 1.279, 1.260 | 0.873 | 1.280 / 1.279 | **kernel: slower** |
| `frozenset/sorted(x)/float/1M` | 1.255 | 1.268, 1.264 | — | 1.254 / 1.258 | **kernel: slower** |
| `tuple/for v in x/mixed/10k` | 1.261 | 1.171, 1.174 | | 0.803 / 0.938 | process state |
| `tuple/for v in x/object/10k` | 1.077 | 1.064, 1.081 | | 0.922 / 0.997 | process state |
| `dict/x[k]/mixed/1M` | 1.271 | 1.021, 1.087 | | 1.002 / 1.002 | process state |
| `set/x == t/mixed/10k` | 1.098 | 1.059, 1.045 | | 1.021 / 0.953 | process state |

- `sorted()` of a 1M-element set or frozenset of `int`/`float`: 47.7 ms plain, 60.1 ms enabled
  (Phase 0 enabled: 41.9 ms); instructions per call 102.5M → 127.3M (perf counters, `sb_probe`);
  the extension merely loaded changes nothing (imported ÷ stub 1.00). A set's copy of small ints
  or floats is long, nearly ordered runs; *inferred*: wave 2's sort (Rust's driftsort above the
  small-list cutoff) does worse than wave 1's there. Other sizes and containers are fine
  (`set` 10k 0.228, `sorted(list)` 1M 0.297, `list.sort()` 1M 0.213). Reported to the lead after
  the run (it was not seen on the dev box, which has no wall-time board).
- The other six kernel-path cells above 1.05 as run (`dict any()` 1M, `frozenset/set all()`,
  `set max()` 10k) measure 1.008–1.023 alone.

### 8.3 Phase 0's ten non-kernel cells: does Rustique cause them? (`sb_probe.py`)

The brief's question. Each cell (the board's own statement and inputs) in fresh processes on
one CPU per domain (10:39–10:55Z, `final/scoreboard/probe-3.15.1.json`): `stub` (Rustique
replaced by an empty module: no extension), `stub` again (plain-vs-plain control), `imported`
(the extension loaded, nothing enabled: the board's builtin side, and what an installed but
not enabled process has once it imports Rustique), `enabled`; 10 processes per mode, modes
interleaved; stub/imported/enabled also at heap offsets 1–3 (k junk objects of each of nine
small size classes before the timed region); per-op hardware counters from `perf stat` at L
and 2L loops (instructions, cycles, branch- / cache- / dTLB-misses, page faults).

| cell (Phase 0 alone) | control stub ÷ stub | imported ÷ stub | enabled ÷ imported (min over offsets) | enabled ÷ stub (min) |
|---|---:|---:|---:|---:|
| `list/pickle.dumps/str/1M` (1.21) | 1.000 | 1.018 | 1.023 (1.021) | 1.041 (1.033) |
| `tuple/for v in x/int/10k` (1.15) | 0.927 | 0.999 | 0.829 (0.867) | 0.829 (0.865) |
| `frozenset/x == t/str/1M` (1.14) | 1.043 | 1.110 | 0.889 (1.012) | 0.987 (0.987) |
| `list/pickle round trip/str/1M` (1.11) | 1.000 | 0.945 | 1.002 (1.004) | 0.947 (0.947) |
| `set/x == t/object/10k` (1.13) | 1.083 | 1.063 | 1.021 (0.896) | 1.085 (0.984) |
| `list/repr(x)/object/1M` (1.06) | 1.001 | 1.013 | 0.997 (1.001) | 1.010 (1.005) |
| `tuple/for v in x/str/10k` (1.13) | 0.999 | 0.854 | 0.998 (0.968) | 0.852 (0.827) |
| `dict/x == t/object/10k` (1.16) | 0.986 | 0.999 | 1.026 (0.989) | 1.025 (0.988) |
| `dict/tuple(x)/mixed/10k` (1.11) | 0.928 | 0.744 | 1.368 (0.979) | 1.017 (0.954) |
| `dict/hit in x/object/1M` (1.11) | 1.049 | 1.034 | 1.021 (0.993) | 1.056 (0.998) |

**None of them is Rustique's.** At the best heap offset, enabled ÷ imported is 0.87–1.02 and
enabled ÷ stub 0.83–1.03, while two plain processes differ by up to 8% (control column) and
loading the extension alone moves cells by −26% to +11%. Where an operation's work is large
enough for the L/2L counter method (6 of the 10 cells), instructions per op agree across stub,
imported and enabled within 1.5% (e.g. `pickle.dumps` 546.1M / 547.3M / 547.2M, `tuple` iteration
730.5k / 730.5k / 730.5k); the differences are cycles, cache and TLB misses: memory placement,
not code (*inferred* from that). `list pickle.dumps str 1M`, Phase 0's
largest (1.21 alone), reads 1.02 now.

## 9. Quiet-box addenda of the workstreams (stage 8) — `7fec7e4`

### 9.1 The gc tier: wall, pauses, RSS (`benches/runtime/run_quiet.sh`; T6)

The gc workstream's quiet-box run as written (its `quiet_driver.sh` minus the dev-box wheel
build: the `7fec7e4` release wheels were already in the family venvs), domains 0 (3.15.1) and 1
(3.15.1t), one job at a time per domain, 08:04–09:18Z (logs in `final/gcq/`; the per-run JSON of
every sweep in `final/gcq-raw.tar.gz`, which extracts to `final/gcq/<stage>/`). Stages: `set` and
`extra` (the 61 non-service workloads, policies `default` = plain CPython and `tier` = the gc
tier alone, 5 processes × 5 runs), `adv` (adversarial programs, + `tier:cap=0`), `services`
(40 runs per process), `e2e` (the GC-heavy set, Rustique's default targets vs the same plus the
tier). Tables: the workstream's `t6table.py`, `gctable.py policies`, `advtable.py`,
`longtable.py` (`tables/gc-*.md`). The GC share that selects T6's set is the default policy's,
measured here.

**T6, the tier alone against plain CPython** (wall = median of per-round paired ratios):

| build | GC > 10% workloads | wall geomean (T6 ≤ 0.87) | peak RSS × max (≤ 1.05) | largest collection × max | max pause × | others: worst wall (≤ 1.02) |
|---|---|---:|---:|---:|---|---|
| 3.15.1 | 8: billing, state_snapshot, xml_feeds, black, coverage, pyflakes, pylint, bs4 | **0.982** | 1.032 | 1.02 | ≤ 1.01 except **`state_snapshot` 1.30** | `pydantic_payloads` **1.021** |
| 3.15.1t | 9: rules_engine, billing, state_snapshot, coverage, pyflakes, pylint, sqlglot, strawberry, request_pool | **0.952** | 1.038 | 1.06 | ≤ 1.01 | `fastapi_pydantic` **1.037** |

- **T6: not met** — the geometric mean (0.982 / 0.952) is far from 0.87, as gc.md §5.2 predicted
  for the decided rule (only full collections that find < 1% garbage are deferred; most GC-heavy
  workloads find garbage). Where it acts it is large: `billing_engine` 0.880 (3.15.1) / 0.908,
  `request_pool` 0.803, `rules_engine` 0.925, `state_snapshot` 0.945 (3.15.1t); `pylint` 1.003.
  Peak RSS on the set: met. Max pause: one exceedance (`state_snapshot` 3.15.1 1.30× the
  default's largest pause, with the largest collection unchanged, 1.00×). Others ≤ 1.02: missed by
  `pydantic_payloads` 1.021 (range 1.00–1.02) and `fastapi_pydantic` 1.037 on 3.15.1t.
- Peak RSS of the *other* workloads against plain CPython reaches 1.05–1.11 on 3.15.1 (15
  workloads, `yaml_config` 1.114) and 1.29 on 3.15.1t (`csv_etl`); the `tier` policy loads the
  extension and plain CPython does not, so this includes Rustique's own memory (*inferred*; the
  workstream's `loaded` baseline that would separate them was not part of the script). On top
  of Rustique (`e2e`: `rustique` vs `tier:all=1`) peak RSS is ≤ 1.002 (3.15.1) and ≤ 1.020
  (3.15.1t), the GC-heavy geomean 0.988 / 0.944 (billing 0.870 / 0.911, `request_pool` 0.780),
  largest collection ≤ 1.06×, max pause ≤ 1.05 (3.15.1) and 1.13 (billing, 3.15.1t).
- **The same from the 3-way tier run** (§5, +gc vs plain and vs the default tier, `d40e7b9`): on
  the GC > 10% set, +gc ÷ plain 0.920 (3.15.1) / 0.844 (3.15.1t), +gc ÷ default 0.983 / 0.955.
- **Adversarial programs** (garbage appearing after the back-off): peak RSS with the tier 1.03 /
  1.01 (burst), 1.02 / 1.05 (late cycles), 1.04 / **1.07** (steady, 3.15.1t) against plain;
  `tier:cap=0` (no pause cap, research setting) 1.04–1.66. No sampled cycle outlives a run
  under the tier where plain frees it (burst: 0 uncollected vs 3,093 plain on 3.15.1).
- **Services, 40 runs per process**: run time with the tier 0.97–1.03 of plain, GC time
  equal or lower (`uvicorn_fastapi` 61.6 → 46.9 ms/run on 3.15.1); RSS growth between the two
  halves of the 40 runs ≤ +4.3 MB per process (plain ≤ +2.5 MB), no leak trend; peak RSS +1–4 MB.

### 9.2 Threads: scaling at 1/2/4/8 threads and kernels under threads (`benches/ft/quietbox.sh`; T1's 8-thread rule)

The threads slice's script as written, in domain 0 alone (05:31–06:06Z), both builds in the
`services` venvs (the services workloads copied into `benches/ft/_ext/`):
`kernel_scaling.py` plain and `--enable` on 3.15.1t (`-r 5`), `scaling.py run threads -t 1,2,4,8 -p 5
-n 3` on both builds, and the two `uvicorn_fastapi` variants plain (`final/ftq/`).

Workloads, median ms per `run()` and enabled ÷ plain:

| workload | 3.15.1t plain 1 → 8 thr (×) | 3.15.1t enabled 1 → 8 thr (×) | enabled ÷ plain, 3.15.1t 1 / 2 / 4 / 8 thr | 3.15.1, 8 thr |
|---|---|---|---|---:|
| compute_pool | 1301 → 173 (7.51) | 1282 → 170 (7.54) | 0.985 / 0.981 / 0.980 / 0.981 | 0.977 |
| http_server | 448 → 89 (5.04) | 391 → 79 (4.97) | 0.872 / 0.872 / 0.854 / 0.884 | 0.914 |
| pipeline_queue | 672 → 265 (2.54) | 357 → 189 (1.89) | 0.531 / 0.608 / 0.671 / **0.714** | 0.512 |
| request_pool | 468 → 139 (3.38) | 270 → 107 (2.52) | 0.577 / 0.615 / 0.693 / **0.773** | 0.545 |
| shared_cache | 576 → 228 (2.53) | 366 → 141 (2.61) | 0.636 / 0.643 / 0.640 / 0.617 | 0.599 |

- Free-threading scales these programs 2.5–7.5× at 8 threads (the GIL build gets slower with
  threads: 0.78–1.00). Rustique keeps 0.62–0.98 of the time at 8 threads on 3.15.1t; as in the
  threads slice, its gain shrinks with threads where the kernels only shorten the parallel part
  (`pipeline_queue` 0.53 → 0.71, `request_pool` 0.58 → 0.77).
- **Kernels at 8 threads (the 8-thread rule, CPU time per call ≤ 1.05× CPython's)**: every one of
  the 80 cells (40 operations × per-thread / shared input) is ≤ 1.05 **except `posixpath.join_fb` with shared input: 1.175** (wall 1.215; 1
  thread 0.986; plain CPU ×8/×1 6.89, enabled 8.21) — `posixpath.join(p, 'static')` with a
  `PurePosixPath` argument, the fallback path, all threads passing the same `p`. Reported to the
  lead. Next highest: `class in tuple(5)` 1.036 / 1.040, `x in list(100)` shared 1.034. The
  former cliffs are gone: `strptime` 0.022–0.025, `strftime` 0.024 (local), `fnmatch` 0.017,
  `urllib.quote` 0.030, `statistics.mean` 0.049 (local) of CPython's CPU per call at 8 threads.
- `uvicorn_fastapi` (plain only, as the script runs it): 947 ms (sync dependency) / 807 ms
  (async) on 3.15.1, 1025 / 850 ms on 3.15.1t.
- **`78751e2`** (text's fix for the cell above: `join` takes exact cached pathlib paths natively,
  pathlib/fnmatch critical sections without refcount round trips on shared arguments): the whole
  kernel table again, `kernel_scaling.py -r 5` plain and `--enable` in the 78751e2 build venv on
  3.15.1t, domain 3 alone, 06:30–06:50Z (`p78/ftq/kernels.md`). `posixpath.join_fb` shared
  **0.359** (local 0.186), `posixpath.join` 0.199, `fnmatch` 0.017, `pathlib.join` 0.244 /
  0.497. One cell reads above 1.05: **`Pattern.match (compiled)` local 1.071** (1 thread 1.047;
  shared 1.011), against 0.863 / 0.877 on `7fec7e4`. It is not a Rustique target, its plain CPU
  rises 9.3× from 1 to 8 threads (the most contended cell of the table), and `9d0abae`'s `re`
  rework sits between the two runs. **Re-measured alone** (`pm_probe.sh`, domain 3 alone,
  08:04–08:16Z, 3 rounds × plain / `--enable` × `7fec7e4`, `9d0abae`, `78751e2`): CPU per call
  at 8 threads 0.991–1.010 in all 18 readings (1 thread 0.948–1.050) — the 1.071 was noise
  (`p78/pm/`). So on `78751e2` every cell of the table meets the 8-thread rule.

### 9.3 "Will it help me?" — the estimator on the 65 (T13), `benches/dx/est_validate.sh STEPS=est`

dx's harness as written (one core per family, 8 families in parallel, prediction only:
`estimate_workloads.py all <family> --build <b>`), in domain 0 after the threads addendum,
untimed lane (06:07–06:12Z, `final/est/`); compared with this run's measured replicate-2 ratios
(`analyze_w2phase3.py est`). Also on `d40e7b9` against replicate 1 (`est/`).

| build | within ±0.03 | ±0.02 | ±0.05 | median \|error\| | bias | T13 (≥ 80% within ±0.03) |
|---|---:|---:|---:|---:|---:|---|
| 3.15.1 (`7fec7e4`) | **45 / 65 (69%)** | 40 | 54 | 0.013 | +0.019 | **not met** |
| 3.15.1t (`7fec7e4`) | **52 / 65 (80%)** | 44 | 58 | 0.012 | +0.004 | **met** |
| 3.15.1 (`d40e7b9`, vs replicate 1) | 45 / 65 (69%) | 41 | 54 | 0.013 | +0.017 | not met |

- Per family on 3.15.1: custom 5/10, data 9/12, devtools 10/12, frameworks 5/6, libs 5/6,
  services 3/4, threads 2/5, web 6/10. On 3.15.1t custom is 10/10 and frameworks 6/6.
- The large misses predict too little gain: `logging_service` +0.313 (predicted 0.999,
  measured 0.686; +0.27 on t), `job_worker` +0.251, `shared_cache` +0.145, `rules_engine` +0.118,
  `event_ledger` +0.105, `report_engine` +0.068 — programs whose gain comes from wave 2's new
  targets (logging handler path, data-model machinery). On the GIL build those targets validate
  at their first call; that the estimator under-counts them there and not on 3.15.1t (eager) is
  *inferred* from the custom family's 5/10 vs 10/10, not diagnosed.
- Too much gain predicted: `state_snapshot` −0.107 / −0.155, `isort_stdlib` −0.097, threads
  `pipeline_queue` −0.209 and `request_pool` −0.124 on t (threaded programs, dx's known gap).

### 9.4 T7 — `isort_stdlib` and `state_snapshot` on 3.15.1t with the copy group off (`7fec7e4`)

`RUSTIQUE_EXCLUDE=copy run.py compare <w> -n 5 -p 5` on 3.15.1t in domain 2 right after the
headline (06:35–06:36Z, `final/workloads/t7-*.json`); the copy targets report `disabled`.
`isort_stdlib` **0.260** (plain 1139.4 ms → 296.1 ms; T7 ≤ 0.35: **met**); `state_snapshot`
**0.668** (1221.5 → 815.4 ms; ≤ 0.75: **met**). With the copy group on (the headline): 0.201 /
0.478.

## 10. Whole-process runs (stage 9) — `9d0abae`

Wave 1's method (`phase3/whole.py`): one `setup()` + one `run()` per process, interpreter start
and imports included, `RUSTIQUE=1` through the wheel's `.pth` (top-level `_rustique`);
`pyperf command --affinity 24-31 -p 5 -n 1 -w 1`, 4 rounds alternating the order, 20 values per
variant; wave 1's six workloads plus `frameworks/django_admin` and `custom/rules_engine`. Domain 3
alone, 05:42–06:12Z (`re9/whole/`).

| build | workload | plain s (mean ± std) | RUSTIQUE=1 s | ratio (means) | ratio (medians) | values |
|---|---|---:|---:|---:|---:|---:|
| 3.15.1 | web/json_api | 0.667 ± 0.055 | 0.471 ± 0.002 | 0.706 | 0.720 | 20/20 |
| 3.15.1 | data/log_analytics | 0.965 ± 0.003 | 0.604 ± 0.002 | 0.626 | 0.628 | 20/20 |
| 3.15.1 | data/report_render | 0.450 ± 0.002 | 0.187 ± 0.001 | 0.416 | 0.417 | 20/20 |
| 3.15.1 | data/state_snapshot | 0.880 ± 0.010 | 0.571 ± 0.003 | 0.649 | 0.651 | 20/20 |
| 3.15.1 | devtools/import_startup | 1.912 ± 0.004 | 1.893 ± 0.005 | 0.990 | 0.990 | 20/20 |
| 3.15.1 | devtools/pytest_networkx | 1.078 ± 0.007 | 1.033 ± 0.006 | 0.959 | 0.957 | 20/20 |
| 3.15.1 | frameworks/django_admin | 1.477 ± 0.005 | 1.442 ± 0.005 | 0.977 | 0.975 | 20/20 |
| 3.15.1 | custom/rules_engine | 1.039 ± 0.013 | 0.873 ± 0.004 | 0.840 | 0.843 | 20/20 |
| 3.15.1 | **geomean** | | | **0.743** | **0.746** | |

| build | workload | plain s (mean ± std) | RUSTIQUE=1 s | ratio (means) | ratio (medians) | values |
|---|---|---:|---:|---:|---:|---:|
| 3.15.1t | web/json_api | 0.709 ± 0.009 | 0.543 ± 0.003 | 0.766 | 0.767 | 20/20 |
| 3.15.1t | data/log_analytics | 1.030 ± 0.005 | 0.653 ± 0.005 | 0.635 | 0.637 | 20/20 |
| 3.15.1t | data/report_render | 0.505 ± 0.003 | 0.207 ± 0.001 | 0.411 | 0.411 | 20/20 |
| 3.15.1t | data/state_snapshot | 1.438 ± 0.005 | 0.743 ± 0.005 | 0.517 | 0.516 | 20/20 |
| 3.15.1t | devtools/import_startup | 2.265 ± 0.004 | 2.259 ± 0.005 | 0.997 | 0.997 | 20/20 |
| 3.15.1t | devtools/pytest_networkx | 1.212 ± 0.006 | 1.166 ± 0.005 | 0.962 | 0.961 | 20/20 |
| 3.15.1t | frameworks/django_admin | 1.653 ± 0.007 | 1.626 ± 0.004 | 0.984 | 0.985 | 20/20 |
| 3.15.1t | custom/rules_engine | 1.201 ± 0.008 | 1.131 ± 0.007 | 0.942 | 0.945 | 20/20 |
| 3.15.1t | **geomean** | | | **0.742** | **0.742** | |

- **0.743 / 0.742 over 8 workloads** (medians 0.746 / 0.742); **wave 1's six: 0.696 / 0.680**
  (Phase 0 with wave-1 code: 0.754 / 0.713). Biggest moves since Phase 0 (3.15.1): `json_api`
  0.790 → 0.706, `report_render` 0.494 → 0.416, `state_snapshot` 0.740 → 0.649,
  `import_startup` 1.018 → **0.990** (the whole `RUSTIQUE=1` process now beats plain on an
  import-heavy program).
- `django_admin` 0.977 / 0.984 and `rules_engine` 0.840 / 0.942 whole-process.
- `json_api`'s plain side on 3.15.1 has slow values in its first round only (703 ± 111 ms; the
  other rounds 654 ± 3 ms), hence the median-based 0.720 next to 0.706. Probably the first
  processes in a fresh workload venv (*inferred*, not investigated); not re-run.
- Digests are identical plain vs `RUSTIQUE=1` for 7 of 8, and every enabled process had the
  extension loaded; `import_startup` differs by construction (it counts `sys.modules` in 21
  children, which gain `_rustique`), as in wave 1 and Phase 0.

## 11. Step durations

All times Z on 2026-11-03 (`durations.tsv` of each results directory; Step 0 in `step0/`).

| stage | commit | domain(s) | when | each |
|---|---|---|---|---|
| Step 0 (venvs, checks, rehearsal of every stage) | `58a91e8` | all | Nov 2 23:06–23:30 | |
| setup, facts, quiet; 4 fat-LTO builds + wheels + status | `d40e7b9` | 0–3 | 03:24–03:25 | 29–31 s |
| pytest × 4 | `d40e7b9` | 0–3 | 03:25–03:28 | 155–187 s |
| status on 12 more interpreters, 3.16 source builds, Rust | `d40e7b9` | 0–3 | 03:28–03:30 | 35–66 s (3.16) |
| conformance × 4 (50 modules) | `d40e7b9` | 0–3 | 03:29–03:34 | 238–263 s |
| headline replicate 1 | `d40e7b9` | 1, 2 | 03:34–04:26 | 3046 / 3104 s |
| +gc 3.15.1 / +interp 3.15.1 / +gc 3.15.1t / +both 3.15.1 | `d40e7b9` | 3 / 1 / 2 / 1 | 03:34–06:19 | 4497 / 4480 / 4584 / 2353 s |
| callgrind scoreboard + isolation, estimator, counters | `d40e7b9` | 0 | 03:34–04:10 | 670 / 763 s (instr) |
| stage 1 + text conformance | `445de19` | 0 | 03:39–04:06 | |
| stage 1, conformance (19 modules), venvs | `7fec7e4` | 0 | 04:24–04:51 | 38–39 s builds, 157–193 s pytest |
| callgrind scoreboard + isolation + heap offsets | `7fec7e4` | 0 | 04:51–05:20 | 622 / 712 s |
| headline replicate 2 | `7fec7e4` | 3, 2 | 04:51–06:35 | 3062 / 3118 s |
| stage 1, `re` conformance, install costs | `9d0abae` | 0 | 05:08–05:31 | |
| threads addendum (`quietbox.sh`) | `7fec7e4` | 0 | 05:31–06:06 | 2074 s |
| whole-process × 2, startup × 2 | `9d0abae` | 3 | 05:42–06:16 | 802 / 945 s, 118 / 119 s |
| estimator × 2 | `7fec7e4` | 0 | 06:07–06:12 | 125 / 140 s |
| T7 | `7fec7e4` | 2 | 06:34–06:36 | 107 s |
| paths re-run: builds, conformance, short compare; kernel table | `78751e2` | 1; 3 | 06:29–06:51 | |
| pyperformance, 4 runs at once | `7fec7e4` | 0–3 | 06:51–07:54 | 3655–3802 s |
| port benchmarks alone; sphinx and two cells alone | `7fec7e4` | 1; 2 | 07:54–08:11 | 143–162 s |
| `Pattern.match` re-measure | 3 commits | 3 | 08:04–08:16 | |
| gc addendum | `7fec7e4` | 0, 1 | 08:04–09:18 | 4422 s |
| scoreboard wall time; isolation; sb_probe | `7fec7e4` | CPUs 2, 10, 20, 30 | 09:18–10:55 | 3393 / 1468 / 960 s |

The box ran 03:24–10:55Z (7 h 31 min), idle from 10:55Z. The long poles were the tier runs
(75 min each, 3-way), pyperformance (63 min) and the scoreboard's wall time (57 min).

## 12. Findings, anomalies and incidents

Findings (all reported to the lead as they appeared; **measured** unless tagged):

1. **Never-slower (T1), scoreboard 0–100, `d40e7b9`**: 9 cells on 3.15.1 and 4 on 3.15.1t stayed
   above 1.05 alone (`list.sort` of 1 `str` 1.132, `sorted()` of a 1-`str` dict/set/frozenset
   1.072, `any({})` 1.074, and GIL-only dict copy/iteration cells 1.056–1.084). Core fixed the
   kernel ones (`7fec7e4`: 0.766 / 0.959 / 0.464); the dict ones are pymalloc pool position —
   plain CPython moves by the same amounts at other heap offsets (§8.1). On `7fec7e4`: 3814 / 3814
   on both builds.
2. **T9, `7fec7e4`**: `import fnmatch` / `import urllib.parse` paid the `re` group's validation
   (+1.52M / +1.53M Ir, over budget). Reworked in `9d0abae`: every lazy install within budget;
   counting the first call, `random` (+0.29M vs +0.17M) and, by 3–12k Ir, `json` and `pathlib`
   are over (§7.1).
3. **8-thread rule, `7fec7e4`**: `posixpath.join` fallback with a shared `PurePosixPath` 1.175×
   CPython's CPU per call at 8 threads; fixed in `78751e2` (0.359). On `78751e2` one non-target
   cell, `Pattern.match (compiled)`, read 1.071; alone, 18 readings over three commits, 0.991–1.010:
   noise (§9.2).
4. **Never-slower (T1), wall time, `7fec7e4`** — found after the run, open: `sorted()` of a
   1M-element `set`/`frozenset` of `int`/`float` 1.25–1.28× CPython (Phase 0 0.87–0.91),
   instructions per call 102.5M → 127.3M; holds alone and at every heap offset (§8.2). The other
   wall cells left above 1.05 are process/heap state (§8.3).
5. **pyperformance `coroutines` on 3.15.1t** 1.073 / 1.080 alone under pyperf (3.15.1:
   1.014 / 1.019): pure Python, no single group, no GC; not reproducible outside pyperf's worker;
   *inferred* layout effect (§6). Open.
6. **T6**: the gc tier's GC-heavy geomean 0.982 / 0.952 is far from 0.87 (as the workstream
   predicted); one pause exceedance (`state_snapshot` 3.15.1, 1.30×); two "others" at 1.021 /
   1.037 (§9.1).
7. **T8**: extension 3.6 MB (≤ 2.5 MB not met); `RUSTIQUE=1` wall 1.059 / 1.075 (instructions
   1.028 / 1.029 met).
8. **T13**: the estimator is within ±0.03 on 69% of the 65 on 3.15.1 (80% on 3.15.1t); it misses
   the gains of wave 2's logging and data-model targets on the GIL build (§9.3).
9. **3.15.0 `re`** (`d40e7b9`, `445de19`): all ten `re` targets degraded at first use
   (fingerprints of `re._compile`, `re._compiler._optimize_charset`); none on `9d0abae`.
10. **A test race** (`9d0abae`, 3.14.9t, 1 in 31 runs): the posixpath exit test counted a transient
   reference held by a daemon thread mid-call; fixed test-only in `b2a12b1`.

Incidents and deviations:

- **The measured commit moved four times during the run** (§0); each move was the lead's
  decision, sent while stages ran. Each re-run used its own tree (`/work/w2p3t`, `w2p3f`,
  `w2p3r`, `w2p3p`) so that no timing job ever ran on a half-updated build, and untimed builds and
  tests of a new commit ran only in a domain without a timing job. The `d40e7b9` schedule's wave
  C was stopped (skip markers, its top-level process ended after wave B), and its whole-process
  and startup steps moved to `9d0abae`; the `445de19` startup slot was cancelled.
- The Step-0 rehearsal on `58a91e8` found the `test_gc_tier_callbacks` failure (§2.1), fixed
  before the measured run.
- `cargo clippy` first reported in 1 s on `d40e7b9` (incremental results from the rehearsal);
  re-run from a clean crate state: clean.
- My conformance harness expects every installed default target; the module subsets of the
  re-runs (`re`'s 16, the paths' 6) never import `statistics`, so those runs exit 1 with 0
  differences — the harness's expectation, not Rustique's.
- The estimator ran in domain 0 at 06:07Z, when the threads addendum freed it, instead of in
  domain 1 after the tiers (same harness and venvs; untimed lane).
- pyperformance: plain and enabled of one build share the benchmark venv while running in two
  domains at once (as in Phase 0).
- **SSH to the box was lost from about 08:30Z** (the lead's public IP changed; the security group
  was updated at 19:36Z). The schedule (`final.sh` and the gated steps) ran unattended to its end
  at 10:55Z; the results were synced at 19:36Z. Nothing after 08:30Z was watched live: the gc
  addendum, scoreboard wall time, isolation and `sb_probe` were analysed afterwards from their
  files (§8.2, §8.3, §9.1).
- Light activity outside the measured processes: results pulled with `rsync` at `nice 19`, pinned
  to one CPU (0, 8, 20 or 31; mostly in an untimed domain, a few times in one running a CPU-time
  measurement); `ssh` status checks (a few `grep`s every 2–3 min); one unpinned 1-second
  `scoreboard.py summary` at 03:46Z. Investigations ran only in domains without a timing job
  (the `coroutines` experiments on CPU 20, 08:13–08:17Z, after the solo pyperformance re-runs).
  No measured process shared a CPU with a build, test or callgrind job.

## 13. Files

`benches/results/w2-phase3/`:

- Scripts: `run_w2phase3.sh` (every stage; `waveA`/`waveB`/`waveC` = the `d40e7b9` schedule),
  `final.sh` (the `7fec7e4` schedule), `textfix.sh` (`445de19`), `re9.sh` (`9d0abae`), `p78.sh`
  and `pm_probe.sh` (`78751e2`), `est_d0.sh` (the estimator in domain 0), `pp_extra.sh` (the
  pyperformance cells re-run alone), `coro_probe.py` / `coro_gc.py` (the `coroutines`
  investigation); helpers
  `status_dump.py` (target status, every module imported), `tiers_compare.py` (plain / default /
  +tier interleaved, tier states recorded), `sb_heap.py` (callgrind cells at heap offsets),
  `sb_probe.py` (wall cells: stub / stub / imported / enabled at heap offsets, perf counters),
  `re_probe.py` (`re` degradation), `install_split.py`, `analyze_w2phase3.py` (the tables);
  `ft_ext/` (the services workloads for `benches/ft/quietbox.sh`).
- `d40e7b9`: `steps.log`, `durations.tsv`, `logs/`, `machine/`, `tests/`, `status/`,
  `conformance/`, `workloads/` (replicate 1, `stats-run-3.15.1/`, venv freezes), `tiers/`,
  `scoreboard/` (instr, isolation), `est/`.
- `textfix/` (`445de19`), `final/` (`7fec7e4`: `workloads/` = replicate 2 and T7, `scoreboard/`,
  `pyperformance/`, `gcq/` + `gcq-raw.tar.gz`, `ftq/`, `est/`, `coro/`, `startup/install-*`), `re9/` (`9d0abae`: `whole/`,
  `startup/`), `p78/` (`78751e2`: `ftq/`, `pm/`, `workloads/`), `step0/` (rehearsal).
- `tables/`: the generated tables of this document.

Files over 1 MB are gzipped. The box keeps everything under `/work/w2p3*` (clones `w/<build>`,
build venvs `venv/<build>`, workload venvs `wl/<build>-<family>`, wheels `dist/<build>/`).

