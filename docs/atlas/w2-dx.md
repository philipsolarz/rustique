STATUS: wave 2, Phase 1, slice `w2-dx`, 2026-11-02. Branch `w2/p1-dx`. Every number is
**M** (measured: how, where) or **I** (inferred: from what). "Box" = the shared wave-2 dev box
(c7a.8xlarge, load 15–45 during these runs: wall times are indicative unless stated).
Release wheels = the CI artifacts of `wheels.yml` run 41362578712 (wave1 @ 4c53e96, whose
`crates/`, `python/` and `pyproject.toml` are identical to this branch's base).

# w2-dx: where the developer journey breaks

**Question.** What stops a developer from trying Rustique, trusting it, and keeping it on?
I walked every step as a newcomer would, on the release wheels:

1. Install them on 10 interpreters.
2. Turn the switch on under 15 process models.
3. Run 14 popular projects' own test suites through `python -m rustique diff`.
4. Prototype `python -m rustique estimate`, a tool that answers "what will Rustique do for
   my program?" from the program itself, and validate it against the measured ratios of
   the 34 workloads.

Short-lived processes and startup are w2-startup's; per-target fallback rates are w2-floors'.
I record what I saw of both.

## Summary

| # | finding | evidence |
|---|---|---|
| 1 | **No behaviour difference in 14 real projects.** No exit code, output or test outcome changed: requests, flask, attrs, rich, httpx, jinja, click, marshmallow, sqlglot, black, pydantic, Django (5,164 tests), arrow and dateutil; about 30,000 test outcomes plus 22,789 subtests on 3.15.1, and 7 of the projects again on 3.15.1t | §4, M |
| 2 | **But the shipped `diff` said DIFFERENT on 6 of the 14 suites (across two passes), and none of the six was Rustique.** The causes were its own `--junitxml` path printed by pytest, log timestamps, ephemeral ports, a test server's thread interleaving with progress dots, and test ids built from `now()`. A CI gate that cries wolf gets switched off. The prototype on this branch fixes all six: 0 false alarms. It also calibrates noise by re-running the plain side once when only the output differs | §3.2, M |
| 3 | **Found while measuring the estimator: the `list.sort` kernel is 1.4–3.2× slower** on presorted and string lists. `sorted()` reaches it too. Sent to the lead, then routed to w2-floors | §5.4, M |
| 4 | **The report under-counts multi-process programs and double-counts forked ones.** Forked workers end with `os._exit` and write nothing: a fork `Pool` doing 8 accelerated calls reports 0. uvicorn's 2 workers served 50 requests and the report shows 1 hit. A forked child inherits its parent's counters: a child with 10 calls reports 110 | §3.1, M |
| 5 | **`python -m rustique app.py` was not quite `python app.py`.** It added 5 runner frames to tracebacks, made `__file__` relative, and did not accelerate spawned children. All three are fixed on this branch | §2, M |
| 6 | **Installing from the sdist without Rust silently downloads a 1.4 GB toolchain** (maturin's puccinialin) and compiles (90 s on 32 cores). The result then enables every target with no platform validation. On CPython 3.16 pre-releases, uv still picks the sdist despite `requires-python <3.16` | §1, M; hazards I |
| 7 | **Platform coverage: Windows 30 → 48 targets and macOS/musl 74 → 77, from three small changes.** A run-time `GetProcAddress` lookup, a layout-correct read of `PyASCIIObject.state` on free-threaded MSVC builds, and an ungated `isoformat` (it never used the C library). Prototype patches pass `cargo check` for 4 target triples. One latent correctness hazard is closed along the way: fixed-mask string-state reads in the opt-in containers land on uninitialised padding under MSVC free-threaded builds. Reaching 84 everywhere takes M–L work, listed | §6, I + cargo check |
| 8 | **The estimator works on compute-bound programs.** It predicts each workload's enabled/plain ratio from the program itself: median absolute error 0.007, 22/34 workloads within ±0.02 and 27/34 within ±0.05 (3.15.1). It explains the result per target and states the share no in-place kernel can reach. The misses are understood and mostly under-promise: I/O-bound programs (`fs_inventory` +0.25, `asyncio_http` +0.21: stack samples under-see compute) and time held in one huge call (`state_snapshot` −0.14). The reported range (two time sources) covers 26/34 measured values (±0.02). On 3.15.1t the stack sampler segfaulted CPython (frame walking is racy on free-threaded builds), so free-threaded builds get the less accurate profiler-only mode for now | §5, M |
| 9 | **Everything else in the journey holds.** Patch releases 3.15.2/3.15.2t/3.14.10 activate the same targets. gunicorn, uvicorn, IPython, xdist, `uv tool`, shiv, `RUSTIQUE=0`, `disable()` and selection typos all work | §1, §2, M |

## 1. Install

| step | what happens | verdict |
|---|---|---|
| `pip install rustique`, the 8 wheel platforms × cp314/cp314t/cp315/cp315t | CI run 41362578712 passes all 33 smoke cells (8 × 4, plus the sdist), including Windows arm64, which is still labelled "experimental" (M) | ok. Promote windows-arm64 |
| patch releases: 3.14.10/t, 3.15.2/t | fresh venvs from a PEP 503 index (`benches/dx/simple_index.py`), the probe in `~/rustique-research/w2/dx/install_matrix.sh`. Same counts as the base patch: 3.14.9 = 3.14.10 = 79 installed + 5 `[version]`; 3.15.1 = 3.15.2 = 84; t builds equal. `RUSTIQUE=1` installs the 23 targets of startup-imported modules everywhere. CI's macOS, musl and Windows runners already ran 3.15.2 and 3.14.10 (M) | ok (per-minor fingerprints hold; w2-floors owns the policy) |
| CPython 3.16.0a1/t, pip | `ERROR: Ignored the following versions that require a different python version: 0.2.0 Requires-Python >=3.14,<3.16` (M) | ok, clear |
| CPython 3.16.0a1, uv 0.13.6 | **tries the sdist**: resolves `maturin` as a build dependency. It does so with `<3.16` and also with `<3.16.0a0` (M). On PyPI it would then build (see the next row) and refuse every target at run time (wave 1 §5.3) | **break (minor)**: minutes of build for zero benefit |
| sdist without Rust (`PATH` without cargo), pip | maturin's PEP 517 backend installs `puccinialin`, downloads rustup plus a toolchain into `~/.cache/puccinialin` (**1.4 GB**), and compiles with fat LTO: **90 s** first install, 72 s compile, on 32 cores. Success, silently (M). On a 4-core laptop: several minutes (I) | **break (medium)**: an exotic platform gets an unvalidated build (§6.3) after a surprise download |
| PyPy, GraalPy, CPython 3.12 (Ubuntu's `/usr/bin/python3`) | refused by `requires-python` (wave 1 M; same mechanism) | ok |
| `pip install --target DIR` + `PYTHONPATH` | `.pth` files are read only in site directories. `RUSTIQUE=1` does nothing; `python -m rustique status` says "no startup file found … use `python -m rustique app.py`" (M) | ok (the status text could name the cause) |
| `uv tool install black --with rustique` | the tool venv's site-packages holds the `.pth`. `RUSTIQUE=1 black …` reports "72 installed" (M) | ok |
| shiv zipapp | shiv extracts to `~/.shiv` and calls `site.addsitedir`, so the `.pth` runs and the report prints (M) | ok |
| `python -m zipapp` / pex without extraction | `ImportError: cannot import name '_rustique' from 'rustique'` (extension modules cannot load from a zip) (M) | **break (minor)**: misleading message. Document it, or raise "Rustique cannot run from a zip archive" |
| two installs (venv + an `addsitedir`'d second copy) | both `.pth` lines run. The second loads the same engine (shared counters; `disable()` restores everything; results identical) (M) | ok |
| Docker slim, distro and conda Pythons (shared `libpython`) | not run (no Docker on the box). `dlsym(RTLD_DEFAULT)` finds `PyAPI_FUNC` symbols of a `DT_NEEDED` `libpython.so`; `--strip-all` keeps `.dynsym` (I) | expected ok; add a `python:3.15-slim` cell to `wheels.yml` |
| upgrading Python to 3.16 later | `requires-python <3.16` makes `pip install -r requirements.txt` fail to resolve on the new interpreter. The team must drop Rustique to upgrade (I, from the cap) | **break (high, at 3.16.0)**: §7 item 9 |

## 2. Turning it on and off

Release cp315 wheel in a fresh venv; script `~/rustique-research/w2/dx/onoff/run_onoff.sh`;
output `onoff.out` (M).

| scenario | result | verdict |
|---|---|---|
| `RUSTIQUE=1` + multiprocessing fork / forkserver / spawn | every child is accelerated (`switch.source = RUSTIQUE=1`) | ok |
| … with `python -m rustique report` | fork: **1 process reported, `json.dumps` 0 hits** (8 calls in the pool workers, which end with `os._exit`). forkserver: 3 processes. spawn: 2 | **break**: §3.1 |
| `python -m rustique mp.py` (no env var) | fork children accelerated; **forkserver and spawn children not** (the runner set no `RUSTIQUE`) | **fixed on this branch**: the runner exports `RUSTIQUE=1` and the selection |
| gunicorn, 2 sync workers, 50 requests | 3 records (master 0, workers 26 + 24 hits) | ok (workers exit through `sys.exit`) |
| uvicorn `--workers 2`, 50 requests | 2 records, **1 hit in total**: the spawned workers that served the requests wrote nothing (terminated) | **break**: §3.1 |
| IPython | `RUSTIQUE=1 ipython`: enabled | ok |
| `python -S` / `-I` / `-E` with `RUSTIQUE=1` | plain, silently. `-I`/`-E` are documented; `-S` (no `site`, so no `.pth`) is not | doc |
| traceback of `python -m rustique boom.py` vs `python boom.py` | **5 extra frames** (`<frozen runpy>` ×2, `rustique/__main__.py`, `_cli.py` ×2), and the program's frames show `boom.py` instead of the absolute path: `__file__` was relative, so `os.path.dirname(__file__)` was `''` | **fixed on this branch**: absolute path, traceback starts at the program, exit 1, SIGINT death on `KeyboardInterrupt` (`tests/test_dx_cli.py::test_run_is_like_python_script` compares bytes with `python script.py`) |
| `rustique.disable()` mid-process, then `enable()` | hits 10 → 10 (no new hits after disable, even through `from json import dumps` taken before) → 20 | ok |
| `RUSTIQUE_EXCLUDE=jsn`, `RUSTIQUE=2`, `RUSTIQUE=yes/true` | one-line warning listing the groups; `2`: "ignoring RUSTIQUE='2' (expected 1 or 0)" but the extension is still loaded; `yes`/`true` enable | ok |
| `pytest -n 4 --rustique` (attrs, pytest-xdist) | the summary merges the 4 workers. `diff -- pytest -n 4`: identical, with stdout correctly reported as volatile (xdist's scheduling order) | ok (with the prototype diff) |
| celery prefork workers (billiard: fork, `os._exit`) | not run (no broker on the box); same mechanism as the fork pool above (I): accelerated, counters lost | the §3.1 fix covers it |
| embedded interpreters (mod_wsgi, uWSGI, Blender) | not run. `site` runs when the host initialises Python normally, so `RUSTIQUE=1` in the host's environment works. Hosts that pass `-S`-like config or `Py_IsolatedFlag` run plain silently (I) | doc: `python -m rustique status` from the embedded interpreter |
| `PYTHONSTARTUP` | interactive only. The `.pth` runs before it; nothing to do (I) | ok |
| coverage in CI (arrow's `addopts = --cov`) | every Python-level target falls back while coverage listens (`tools` 320 per target in arrow). `diff` then proves little for those targets, and CI timing shows no gain | **doc**: run the Rustique job without coverage |

## 3. Visibility and trust

### 3.1 The report (`RUSTIQUE_REPORT`, `report`, `--rustique`, the end of `diff`)

What a developer reads, and where it fails them (M: the outputs in
`~/rustique-research/w2/dx/onoff/onoff.out` and `~/rustique-research/w2/dx/rp/`):

| problem | evidence | fix | effort |
|---|---|---|---|
| **forked workers' counters are lost** (`os._exit`), and **forked children double-count** the parent's counters (inherited at fork) | fork pool: 0 of 8 calls reported; uvicorn: 1 of 50; `forkcount.py`: parent 100 + child 10 calls report 100 + 110 | counters in a file-backed `mmap` per process when `RUSTIQUE_REPORT` is a `.json` path. The reader merges the files: nothing is lost on `os._exit` or SIGKILL. Zero the counters in the child (`pthread_atfork` / `PyOS_AfterFork_Child` hook) | M (Rust) |
| **zero rows**: 16 of 25 rows in a small program are `0  0` | E14 | show only targets that ran; "16 installed targets not called" in one line | S |
| **no "so what"**: hits and fallbacks, but no time | every report | a `saved ≈` column from the estimator's per-hit constants (§5); estimated, labelled so | S |
| **suspensions are invisible**: `stats()` has `suspensions`, the renderer drops it | sqlglot: `copy.deepcopy` suspended once in 55 processes, but restarted (`deopt`) on **210,650 of 429,506 calls (49%)** with no suspension in the others: credit 4/1 keeps a target on while ≥ 20% of calls hit | render "suspended N× (net benefit)". Make the debit of a `deopt` restart proportional to its cost: a restart pays the kernel's partial walk *and* the original | S (render) / floors (credit) |
| **fallback reasons are not actionable**: `args 7,019` says nothing about which call site or argument | requests, sqlglot | `RUSTIQUE_STATS=sites`: per (target, caller code object, reason) counters, top 5 in the report. "`tuple.__contains__` falls back 1.7 M× from `sqlglot/expressions.py:…` (class needle)". This is the data floors needs too | M |
| `pending import` lists 30–60 targets in every report | E14 | one count ("58 waiting for their module"); the list under `--verbose` | S |
| `status` says "26 installed, 58 pending-import" right after install | requests venv | phrase it "84 available here (26 active now, 58 activate when their module is imported)" | S |

### 3.2 `diff`, the plugin, check mode: do they prove "nothing changed except speed"?

The shipped `diff` reported DIFFERENT on 6 of 14 real suites (M, `real_projects.py`). In every
case the exit codes and all outcomes were identical; the cause was in the diff itself or in the
program's nondeterminism:

| suite | shipped diff said | cause | prototype fix |
|---|---|---|---|
| arrow, flask, black | stdout differs | pytest prints "generated xml file: …/plain-0.xml" vs "…/rustique-0.xml": diff's own `--junitxml` path | one path for both runs |
| requests | stdout + stderr differ | access-log timestamps; the pytest-httpbin thread printing between progress dots | ISO-8601 and common-log-format date-times masked; pytest progress characters dropped (the outcomes carry them) |
| httpx | stdout differs | ephemeral ports `127.0.0.1:36272` vs `:44990` | `127.0.0.1/localhost/[::1]:<port>` masked |
| marshmallow | 2 tests "only plain", 2 "only rustique" | test ids parametrized with `now()` (`…[11-02-2026 19:09:21]` vs `…:22]`) | unmatched ids that pair up one-to-one with digits masked and equal outcomes are the same tests |
| (any) | — | output that varies run to run for reasons nobody can mask | when only stdout/stderr differ, run the plain side once more. A stream that differs plain-to-plain is reported as *volatile*, not as a difference (`--strict-output` to compare it anyway) |

With the prototype (`python/rustique/_diff.py`, tests in `tests/test_dx_cli.py`), all 14 suites
are "identical" (§4). Cost: two runs of the suite, plus a third when output varies. Proposals
for Phase 2:

1. **`diff --from-ci plain.xml`**: compare against the junit file the project's normal CI job
   already produced. The Rustique job then costs one run, not two (S).
2. **Confirm before failing**: re-run only the differing tests (node ids from junit
   `xunit1` `file` attributes), 3× each side, and report "confirmed" or "flaky in plain too" (M).
3. **pytest-randomly** reorders per run (seed not fixed by `diff`): pass
   `-p randomly --randomly-seed=<fixed>` when the plugin is present (S, I).
4. Document "run the Rustique CI job without coverage" (§2) and `--only-binary rustique` (§1).

The plugin (`--rustique`): xdist merge works (§2). It fails the session only on unexpected
refusal categories. On a platform with no validated targets, as Phase 0 saw on 3.16, the plugin
passes, but Rustique's own `tests/` fail on version refusals: those tests must skip on refusal
categories `version`/`platform` (lead's item; S). Check mode was not re-measured here
(wave 1: about 2× per checked call).

## 4. Real projects

`benches/dx/real_projects.py`: each project at its latest release tag, cloned shallow; a fresh
`uv venv` of python-build-standalone; the project editable plus its test dependencies, plus
**the release wheel** (so the `.pth` is there, as for users). One warm-up run, then
`python -m rustique diff --keep … -- <python> -m pytest -q tests` (Django: `tests/runtests.py
--parallel 1`, 14 apps). Pin fixes for old tags against pytest 9: flask and black `pytest<9`;
black `aiohttp`; pydantic `pytest-run-parallel`, `hypothesis`, and `--ignore=tests/pydantic_core`.
"diff" is the prototype diff (v1/v2); the shipped diff's false alarms are in §3.2. Time = one
plain and one enabled run in sequence on the loaded box: **±5% is noise here** (M: sqlglot
1.076 in one pass and 0.995 in the next, same code).

| project @ tag | tests (outcomes) | plain s | ratio (1 pair) | shipped diff | prototype diff | Python processes | accelerated calls / fallbacks | biggest fallback signal (floors) |
|---|---:|---:|---:|---|---|---:|---|---|
| requests v2.34.2 | 633 | 77.6 | 1.000 | DIFFERENT (log stamps, server thread) | DIFFERENT in v1 (server thread vs progress dots) → identical (v2) | 1 | 131k / 23k | `list.__contains__` 41% args |
| flask 3.1.3 | 490 | 1.25 | 0.988 | DIFFERENT (junit path) | identical | 1 | 362k / 19k | `list.__contains__` 45% args |
| attrs 26.1.0 | 1,387 | 6.75 | 1.000 | identical | identical | 1 | 337k / 156k | `builtins.min` 44% deopt; `tuple.__contains__` 39% args |
| rich v15.0.0 | 981 | 4.62 | 1.053 | identical | identical | 3 | 332k / 64k | `builtins.max` 10% args |
| httpx 0.28.1 | 1,418 | 4.42 | 1.004 | DIFFERENT (ports) | identical | 1 | 202k / 75k | `list.__contains__` 75% args |
| jinja 3.1.6 | 909 | 1.78 | 0.976 | identical | identical | 1 | 422k / 31k | (random 238k hits, 0 fallbacks) |
| click 8.5.0 | 2,016 | 5.85 | 0.902 | identical | identical | 2 | 364k / 47k | `list.__contains__` 50% args |
| marshmallow 4.3.1 | 1,188 | 1.16 | 1.028 | identical (same second, by luck) | DIFFERENT in v1 (ids with `now()`) → identical (v2) | 1 | 116k / 29k | `copy.deepcopy` 57% deopt |
| sqlglot v30.28.0 | 1,459 + 22,789 subtests | 104.9 | 0.995 (first pass 1.076) | identical | identical | 54 | 1.90M / 2.29M | `tuple.__contains__` 80% args; `copy.deepcopy` 49% deopt |
| black 26.10.1 | 573 | 37.8 | 1.016 | DIFFERENT (junit path) | identical | 4 | 8.86M / 4.50M | **`list.__contains__` 99.4% args (3.57M)** |
| pydantic v2.14.2 | 6,603 | 14.7 | 0.989 | identical | identical | 8 | 1.00M / 214k | `list.__contains__` 36% args |
| Django 6.1.2 (14 apps, `runtests.py`) | 5,164 (unittest; stdout compared) | 14.1 | 0.983 | identical | identical | 3 | 584k / 158k | `list.sort` 40% args; `copy.deepcopy` 21% deopt |
| arrow 1.4.0 (`--cov` in addopts) | 1,863 | 3.35 | 0.995 | DIFFERENT (junit path) | identical | 1 | 482k / 66k | `tools` 5,644 (coverage listening); `builtins.min` 6% |
| dateutil 4.0.0 | 5,343 | 15.0 | 0.989 | identical | identical | 2 | 2.76M / 709k | **`builtins.min` 49% deopt (438k)**; `list.sort` 72% args |

⟨T-TABLE⟩

Reading:

- **No behaviour difference anywhere.** Failures present in both runs are environmental (old
  tags against pytest 9 or Python 3.15, network tests) and identical between plain and enabled.
- **Timing is flat (0.90–1.05)** because test suites are dominated by pytest machinery,
  fixtures and assertion rewriting, not by stdlib calls. That is consistent with the workload
  atlas: framework-heavy code is out of reach of today's 84 targets. A developer who measures
  "speed" on their test suite will see nothing, and the estimator (§5) says so up front.
- **What the counters show** (floors' input; sent to the lead):
  - `list.__contains__` falls back on **99.4% of 3.59 M calls in black**, and on 36–75% in
    requests, flask, httpx, click and pydantic.
  - `tuple.__contains__` falls back on 80% of 2.17 M calls in sqlglot (class needles, the
    wave-1 item).
  - `copy.deepcopy` restarts on 49% in sqlglot, 57% in marshmallow and 21% in Django.
  - `builtins.min` restarts on 44% in attrs and **49% of 886k in dateutil**.
  - Every restart pays for the kernel's partial work and then the original.
- **sqlglot A/B** (plain / on / on without copy, interleaved, 3 rounds): ⟨SQLGLOT-AB⟩

## 5. "Will this help my program?": `python -m rustique estimate`

### 5.1 Design (prototype: `python/rustique/_estimate.py`)

`python -m rustique estimate -- <command>` runs the command three times and prints a prediction
with its reasons. A `sitecustomize` on `PYTHONPATH` puts the instruments into every Python
process of the command. The three runs:

1. **Plain, sampled.** A stack sampler thread (`sys._current_frames` of the main thread every
   0.5 ms, switch interval lowered to 0.1 ms) gives the share of wall time under each
   Python-level target, with no per-call cost. This run's wall time is `T`.
2. **Plain, profiled.** A `sys.monitoring` profiler (tool id 4):
   - Python-level targets: local `PY_START`/`PY_RETURN` events on exactly the targets' code
     objects. C-level targets: `CALL`→`C_RETURN`, with `DISABLE` on any call site that is not
     a target's after 3 misses.
   - It counts every call and times it (exclusive of other targets' calls; recursion folded
     into the outermost). It subtracts its own calibrated cost per event (`calibrate()`:
     about 190 ns per measured call, about 500 ns per nested event on the box).
   - It keeps a systematic sample of 24 calls per Python-level target. Arguments come from
     the callee's frame at `PY_START`, rebuilt as the caller most likely wrote them (trailing
     and keyword-only defaults dropped: kernels' exact subsets often exclude explicit
     keywords). It also keeps the target's longest outermost call.
   - At exit, **replay**: each kept call is timed in a loop (after one warm-up call), first
     with CPython's original, then with `rustique.enable(only=<target>)`, with counters on.
     This gives the per-call ratio and the fallback rate on the program's own arguments.
     The longest call is weighted by its share of the target's time. `logging` calls are
     never replayed (they emit); `random` replays on the program's own generator, after the
     program has finished.
3. **Enabled with counters** (what `report` does). Hit rates of the C-level targets, and the
   hits of `in` on tuples/lists and of `float.__repr__`, which no event can see.

Prediction: `ratio = 1 − Σ_t T_t (1 − r_t) / T`. The inputs:

- `T_t` comes from run 1 for Python-level targets and from run 2 for C-level targets called
  outside them.
- `r_t` is the replayed ratio for Python-level targets. C-level targets use a table ratio ×
  their hit rate (`benches/dx/c_ratios.py`, measured: `strftime` 0.09–0.13, `isoformat`
  0.15–0.26, `min`/`max` 0.34–0.46, `sort`/`sorted` 1.0, see §5.4).
- `in` and `float.__repr__` use a per-hit saving × hits: 12 ns for short containers,
  188 ns for `float.__repr__`.

The printed range comes from the other time source (the profiler's per-call times for flat
targets), so the user sees how much the answer depends on it.

The output names the targets that save the most and the share of time under no accelerated
entry point, which is the honest "out of reach" number for that program.

### 5.2 Validation against the 34 workloads

`benches/dx/estimate_workloads.py all <family>`: per workload, a fresh process. It runs
setup(), a warm-up run(), 3 timed run()s (the median gives T), 3 run()s under the stack
sampler, and 1 run() under the profiler, then the replay. A second fresh process runs
enabled with counters. Measured ratios: Phase 3, first replicate (`docs/atlas/phase3.md` §4,
quiet bench box). The prediction itself ran on the loaded box (M).

**3.15.1, 34 workloads** (M):

| time source for Python-level targets | MAE | median abs error | within ±0.02 | within ±0.05 | worst | bias |
|---|---:|---:|---:|---:|---:|---:|
| **stack samples** (the default) | 0.033 | **0.007** | **22** | **27** | +0.250 (`fs_inventory`) | +0.011 |
| hybrid: profiler per-call times, samples where a call holds > 4 profiler events | 0.032 | 0.013 | 21 | 26 | −0.141 (`state_snapshot`) | −0.018 |
| profiler per-call times only | 0.110 | 0.013 | 20 | 24 | −1.47 (over-counts recursion) | −0.095 |

The measured ratio lies inside the printed range (the two time sources, ±0.02) for 26/34.
Per workload ("under targets" = share of the run under accelerated entry points, by the
default source; "top saver" = the target saving the most, as % of the run):

| workload | measured | predicted | error | range (samples, hybrid) | under targets | top saver |
|---|---:|---:|---:|---|---:|---|
| data/backup_archive | 0.996 | 0.997 | +0.001 | 1.00–1.00 | 1% | - |
| data/billing_engine | 0.970 | 0.952 | -0.018 | 0.95–0.95 | 17% | statistics.stdev (4%) |
| data/config_merge | 1.013 | 0.999 | -0.014 | 1.00–1.00 | 0% | - |
| data/csv_etl | 0.625 | 0.602 | -0.023 | 0.60–0.60 | 55% | json.dumps (21%) |
| data/fs_inventory | 0.743 | 0.993 | +0.250 | 0.85–0.99 | 28% | posixpath.relpath (1%) |
| data/log_analytics | 0.594 | 0.581 | -0.013 | 0.58–0.65 | 47% | datetime.datetime.strptime (30%) |
| data/pandas_etl | 1.000 | 1.000 | -0.000 | 1.00–1.00 | 0% | - |
| data/report_render | 0.436 | 0.443 | +0.007 | 0.30–0.44 | 74% | textwrap.fill (32%) |
| data/state_snapshot | 0.651 | 0.513 | -0.138 | 0.51–0.51 | 86% | copy.deepcopy (45%) |
| data/text_corpus | 1.000 | 0.997 | -0.003 | 1.00–1.00 | 5% | - |
| data/tz_events | 0.591 | 0.556 | -0.035 | 0.56–0.56 | 55% | datetime.date.strftime (33%) |
| data/xml_feeds | 1.000 | 1.000 | -0.000 | 1.00–1.00 | 2% | - |
| devtools/black_format | 0.990 | 0.997 | +0.007 | 1.00–1.00 | 0% | - |
| devtools/coverage_pyflakes_tests | 0.995 | 0.991 | -0.004 | 0.99–0.99 | 2% | textwrap.dedent (1%) |
| devtools/import_startup | 1.000 | 1.000 | +0.000 | 1.00–1.00 | 0% | - |
| devtools/isort_stdlib | 0.771 | 0.777 | +0.006 | 0.78–0.78 | 50% | dataclasses.asdict (21%) |
| devtools/pip_resolve | 0.949 | 0.897 | -0.052 | 0.87–0.90 | 52% | builtins.any (9%) |
| devtools/pycodestyle_stdlib | 0.985 | 0.990 | +0.005 | 0.99–0.99 | 0% | list.__contains__ (1%) |
| devtools/pyflakes_stdlib | 0.997 | 0.999 | +0.002 | 1.00–1.00 | 0% | - |
| devtools/pylint_stdlib | 1.002 | 0.999 | -0.003 | 1.00–1.00 | 1% | - |
| devtools/pytest_networkx | 0.948 | 0.993 | +0.045 | 0.99–0.99 | 17% | - |
| devtools/rich_render | 0.998 | 0.973 | -0.025 | 0.97–0.97 | 12% | builtins.max (2%) |
| devtools/sphinx_autodoc | 0.977 | 0.991 | +0.014 | 0.98–0.99 | 5% | builtins.min (1%) |
| devtools/sqlglot_transpile | 0.968 | 0.921 | -0.047 | 0.92–0.92 | 19% | copy.deepcopy (8%) |
| web/aiohttp_app | 0.910 | 0.982 | +0.072 | 0.92–0.98 | 1% | list.__contains__ (1%) |
| web/asyncio_http | 0.791 | 0.997 | +0.206 | 0.71–1.00 | 1% | - |
| web/django_blog | 0.997 | 0.994 | -0.003 | 0.99–1.00 | 6% | - |
| web/email_mime | 0.995 | 0.997 | +0.002 | 1.00–1.00 | 11% | - |
| web/fastapi_pydantic | 0.983 | 0.984 | +0.001 | 0.97–0.98 | 4% | - |
| web/flask_jinja | 0.990 | 0.991 | +0.001 | 0.98–0.99 | 8% | - |
| web/json_api | 0.835 | 0.912 | +0.077 | 0.77–0.91 | 67% | json.loads (6%) |
| web/logging_service | 0.817 | 0.820 | +0.003 | 0.69–0.82 | 98% | logging.Logger.level (18%) |
| web/requests_local | 0.941 | 0.995 | +0.054 | 0.92–0.99 | 2% | - |
| web/sqlalchemy_orm | 0.997 | 0.998 | +0.001 | 1.00–1.00 | 1% | - |

**3.15.1t** (M, same method; 31 of 34). The stack sampler **segfaulted CPython 3.15.1t** in
`devtools/black_format`, `devtools/pytest_networkx` and `data/xml_feeds` (reproduced: 1 crash
in 3 runs of `xml_feeds`, in the sampler thread's frame walk). `sys._current_frames()` stops
the world only while it collects the frames; walking `f_back` of a frame that another thread
is still running is a data race on free-threaded builds (I, from `Python/pystate.c`
`_PyThread_CurrentFrames`; worth an upstream report). The prototype now disables the sampler
on free-threaded builds and falls back to the profiler's times (the "profiler only" row).

| time source | MAE | median abs error | within ±0.02 | within ±0.05 | worst |
|---|---:|---:|---:|---:|---:|
| stack samples (31 workloads that did not crash) | 0.046 | 0.009 | 18 | 26 | +0.638 (`sqlglot_transpile`) |
| profiler only (what free-threaded builds get now) | 0.128 | 0.010 | 16 | 21 | +1.88 |

`sqlglot_transpile` on 3.15.1t: replaying its sampled `deepcopy` calls (objects with
`__deepcopy__`, 610 restarts in 2,842 replayed calls) gives kernel/original **4.4** on 3.15.1t
against 0.57 on 3.15.1, same calls (M, replay). The workload as a whole measured 0.974 in
Phase 3, and a plain `deepcopy` with or without `memo` is 0.21–0.35 on both builds (M,
`~/rustique-research/w2/dx/dcmemo.py`). So restarts cost far more on the free-threaded build. That is an
unconfirmed signal for w2-floors and w2-threads, not a measured regression.

Reading:

- **Null workloads are predicted as null.** All 15 workloads measured at 0.99–1.013 are
  predicted at 0.97–1.00. The tool never promises a gain on framework-heavy code, which is
  the most important property for trust.
- **Compute-bound stdlib-heavy workloads are within ±0.04:** `csv_etl`, `log_analytics`,
  `report_render`, `tz_events`, `isort_stdlib`, `logging_service`.
- **The misses:**
  - Under-predictions in I/O-bound programs (`fs_inventory`, `asyncio_http`, `json_api`,
    `requests_local`, `aiohttp_app`): the sampler thread gets the GIL whenever the main thread
    blocks in a syscall, so samples over-represent waiting. The hybrid source catches
    `asyncio_http` (0.71 vs 0.79) and `json_api` (0.77 vs 0.84).
  - Over-predictions where one huge call dominates (`state_snapshot`: one `deepcopy` of a big
    graph, replayed hot).
  - `pip_resolve` −0.05 (`any()` with a generator: the table ratio does not fit).
- **Cost on the box**: the profiled `run()` takes 0.9–5.5× the plain one (median 1.4×); the
  replay adds 0–3 s. The CLI's three runs cost about 3–6× one run.
- **Demo** (`~/rustique-research/w2/dx/onoff/demo_app.py`: JSON, dates, `deepcopy`, `textwrap`
  and own loops). `python -m rustique estimate` printed 0.404 (strptime 22%, deepcopy 16%,
  json 11%, strftime 10% saved; "28% of the run is unchanged by Rustique").
  `diff --repeat 3` measured 0.419 (M).

### 5.3 Limits (where it is wrong, and why)

- **I/O-heavy programs** (`fs_inventory` +0.25): the sampler thread gets the GIL immediately
  whenever the main thread is in a syscall, so stack samples over-represent I/O. The profiler's
  per-call times are right there (`fnmatch` 416 ns corrected vs 398 ns replayed), but they
  over-count in recursive targets. Phase 2: choose per target by the profiler's own event
  density (the `hybrid` mode in `predict()`), or use `perf`-based samples where available.
- **Time concentrated in a few huge calls** (`state_snapshot` −0.14: one `deepcopy` of a large
  graph per run): the longest outermost call is replayed and weighted by its share, but replay
  in a hot loop flatters the kernel (warm caches). Phase 2: replay the longest call cold.
- **C-level targets use table ratios**: `CALL` events carry no arguments, so `sorted()` of
  presorted strings (ratio up to 3.2, §5.4) cannot be told apart from random ints (0.42). The
  table assumes 1.0 for sort.
- **Free-threaded builds**: no stack samples (they crash CPython, §5.2), so the profiler's
  per-call times are used. They over-count recursive targets (MAE 0.128 on 3.15.1t). Phase 2
  needs a sampler that never walks another running thread's frames: a `sys.monitoring`
  `PY_START`/`PY_RETURN`-driven shadow stack sampled by a timer flag, or `perf` where it exists.
- Costs 3 runs; the profiled run is 0.9–5.5× slower than plain. The prediction's noise is
  the noise of the run (a single T).

### 5.4 Side finding: the `list.sort` kernel is slower on presorted and string lists

From `c_ratios.py`, confirmed with `~/rustique-research/w2/dx/sortcheck.py`. Wall time, box,
pinned to one core, timeit best of 7, plain measured twice (M). Ratio = with Rustique /
without, for `sorted(data)`:

| data | n=20 | n=100 | n=1000 |
|---|---:|---:|---:|
| int ascending / descending / random | 0.91 / **1.47** / 0.72 | 0.79 / 0.67 / 0.61 | 0.86 / 0.84 / 0.42 |
| str ascending / descending / random | **1.68 / 3.21 / 1.38** | **1.78 / 1.70** / 0.92 | **1.90 / 1.99** / 0.69 |

CPython's timsort finds natural runs (sorted, reverse-sorted, partly ordered) in O(n); the
kernel apparently does not (I). Lead informed; routed to w2-floors for callgrind and a fix.

## 6. Platform coverage: what blocks each missing target

Measured from the CI smoke logs (run 41362578712, parsed by
`~/rustique-research/w2/dx/ci/parse_smoke.py`). The analysis comes from the code; prototype
patches pass `cargo check -p rustique-py --lib` for x86_64-pc-windows-msvc,
aarch64-apple-darwin, x86_64-unknown-linux-musl and x86_64-unknown-linux-gnu × {3.15, 3.15t}.
Full section: `~/rustique-research/w2/dx/platform.md`. Patches:
`platform-windows-symbols-strstate.patch` (A) and `platform-datetime-isoformat.patch` (B).
Neither is applied (evidence, per the lead).

| platform | on now (3.15 / 3.15t) | what refuses | why (file) |
|---|---:|---|---|
| Linux glibc x86_64/aarch64 | 84 / 84 (3.14: 79) | 3.14: 5 pathlib properties `[version]` | 3.14's specializer inlines property getters |
| macOS arm64/x86_64, musl x86_64/aarch64 | 74 / 74 | 10 datetime | one group gate `cfg!(linux ∧ gnu)` (`targets/datetime/mod.rs:73`), also over `isoformat`, which calls no C-library function |
| Windows x64 **and arm64** | 30 / 28 | 24 paths `[platform]`; 13 `_PyObject_GetMethod` (copy, random, logging); 7 `_PyUnicode_Is*` (textwrap); 10 datetime; t: json ×2 | `Prepare::symbol` is `dlsym`-only (`accel/prepare.rs:632`), though both symbols are `PyAPI_FUNC` (dllexport'ed). `pystr::calibrate` reads the first 32-bit word of `PyASCIIObject.state`, where MSVC free-threaded builds start a new bitfield unit (kind at bit 32: clang `-fdump-record-layouts`) |

Closing the gap, ranked by targets per effort:

| # | step | effort | Windows 3.15 / t | macOS, musl |
|---|---|---|---:|---:|
| 1 | patch A: `GetProcAddress(sys.dllhandle)` lookup and one `strstate` accessor for every reader of the state bits; logging refuses as `platform` off Unix (its `os.path is posixpath` pin would otherwise refuse as `deps` and fail `pytest --rustique` sessions); plus the per-platform CI job (below) | S | 45 / 45 | 74 |
| 2 | patch B: gate only the 7 C-library-dependent datetime targets | S | 48 / 48 | 77 |
| 3 | a `process_gate` allowlist (arch + OS + libc + pointer width + endianness: the 8 CI tuples), `platform` elsewhere; promote windows-arm64 | S | safety | safety |
| 4 | a portable "LC_TIME is C" probe (musl and macOS) → strptime | S–M | — | 81 |
| 5 | 6 pure posixpath functions + `fnmatchcase` on Windows | S | 55 / 55 | — |
| 6 | strftime: year < 1000 in the plain `time.strftime` path pads to 4 digits off glibc | M | — | **84** |
| 7 | Windows strptime/strftime (UCRT `%c`), logging (`ntpath` normcase via `LCMapStringEx`, `_localtime64_s`), `abspath`/`relpath`, `fnmatch`/`filter` | M | ~71 | — |
| 8 | Windows-flavour pathlib kernels (drives, UNC, case-insensitive `__eq__`/`__hash__`) | L | 84 | — |

**The validation gap behind every "on" count.** `ci.yml` runs the test suite on Ubuntu only. On
macOS, musl and Windows, the 74 / 74 / 30 "on" targets are exercised by the smoke battery
alone: about 17 calls, compared plain vs enabled. Phase 2 needs a job that installs the
wheel on {macos-15, windows-2025, windows-11-arm, ubuntu-24.04-arm, alpine} × {3.15, 3.15t}
and runs `tests/targets` plus `run_target_suites.py` against CPython's `Lib/test` of the
runner's exact version. Est. 10–15 min per cell (I). A target counts as "on" for a platform
only once this passes there.

### 6.3 Platforms with no wheel, and two safety items (lead: must fix or refuse)

- **Unvalidated platforms built from the sdist enable every target** that their `cfg` gates
  allow. `process_gate` (`accel/version.rs:172`) checks version, debug build and
  `PyFunctionObject` layout, never the platform. Hazards (I, from the code):
  - 32-bit glibc: `localtime_r` with a 32-bit `time_t` in logging's `formatTime`.
  - Big-endian: fixed masks on the state bitfield.
  - FreeBSD: `RTLD_DEFAULT` is `-2`, not `NULL`, so symbol targets refuse as `symbol` and
    `pytest --rustique` fails the session.
  - **Fix: step 3's allowlist.**
  - Weighed against failing the sdist build early with a clear message: the allowlist gives a
    working, never-wrong install everywhere (refused targets, `RUSTIQUE=1` harmless), and
    widening the list becomes a CI decision.
  - An early build failure (`build.rs` panicking on an unlisted target) saves the 1.4 GB
    download only if it runs before puccinialin. It doesn't: maturin fetches the toolchain
    before any build script.
  - So it would turn "slow install, safe result" into "slow install, then a failure", and it
    breaks every downstream that lists Rustique in requirements on such a platform.
  - **Recommend the allowlist, plus** documenting `pip install --only-binary rustique` for CI
    and Docker, plus an sdist-build message ("no wheel for this platform: building from source;
    targets will refuse as `platform`").
- **MSVC free-threaded state bits** (I): `convert.rs:113-131` (the opt-in containers' `str`
  fast path) and `targets/datetime/ustr.rs` test fixed bits 11/12. Under MSVC free-threaded
  builds those bits are uninitialised padding, so a sample check can pass by chance and
  non-ASCII text is then read as ASCII. **Must fix (patch A's accessor) or refuse** (the
  containers on Windows t). It disappears if the opt-in containers are retired (w2-startup).

## 7. Ranked DX work list for Phase 2

Ranked by how many developers a break stops × how early in the journey × effort.
"Owner" is a suggestion.

| # | item | journey step it unblocks | severity | effort | evidence | owner |
|---:|---|---|---|---|---|---|
| 1 | **Ship the diff fixes** on this branch: one junit path, timestamps and ports masked, pytest progress dropped, volatile ids paired, plain-vs-plain calibration, `--strict-output` | trust in CI | high (6/14 false alarms) | done (prototype + 4 tests) | §3.2 | dx |
| 2 | **`process_gate` platform allowlist** + Rustique's own tests skip on `version`/`platform` refusals + an sdist-build notice + `--only-binary` docs | install, safety | high (silent unvalidated targets) | S | §6.3 | accel/dx |
| 3 | **Report counters survive processes**: mmap'd per-process counters, zeroed after fork | visibility | high for servers and workers | M | §3.1 (0 of 8, 1 of 50, 110 vs 10) | accel |
| 4 | **`python -m rustique estimate`**: productise the prototype (pick the time source per target by the profiler's event density and detect I/O-bound runs, a sampler that is safe on free-threaded builds, cold replay of the longest call, `estimate --json` for CI) | "will it help me?" | high (the first question) | M | §5 (3.15.1: median error 0.007, 27/34 within ±0.05) | dx |
| 5 | **Platform steps 1–3 and 5** (patches A and B, allowlist, posixpath/fnmatchcase on Windows) + the per-platform CI job | every platform | high on Windows (30 → 55) | S each | §6 | platform |
| 6 | **The runner fixes on this branch** (absolute `__file__`, traceback, SIGINT, `RUSTIQUE=1` exported) | turn it on | medium | done (2 tests) | §2 | dx |
| 7 | **Report readability**: only targets that ran, a `saved ≈` column, suspensions rendered, pending as a count, status wording | visibility | medium | S | §3.1 | dx |
| 8 | **Per-site fallback attribution** (`RUSTIQUE_STATS=sites`) | "why did it fall back, and what can I do?" | medium | M | §3.1 | accel (shared with floors) |
| 9 | **The 3.16 upgrade path**: a 3.16 wheel at 3.16.0rc1. With the opt-in containers retired, a `py3-none-any` fallback wheel for every unvalidated interpreter (installs, refuses every target as `version`), so a requirements file never blocks a Python upgrade; or drop the upper cap again | keep it on | high on 3.16 day | S–M | §1 | dx (after w2-startup's containers decision) |
| 10 | `diff --from-ci`, confirmation re-runs, pytest-randomly seed | CI cost | medium | S–M | §3.2 | dx |
| 11 | Docs: `-S`, coverage in CI, zipapps, `--only-binary`; a `python:3.15-slim` CI cell | install, CI | low | S | §1, §2 | dx |
| 12 | Credit for `deopt` restarts weighted by their cost (sqlglot deepcopy 49%) | never slower | medium | S | §3.1, §4 | floors |

### 7.1 Candidate rows (the brief's format, for the lead's table)

| candidate | layer | entry points + mechanism | share mean / max | s | exact subset & fallback | floor vs current | risks | upstream | reach | prototype | evidence |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `list.sort` / `sorted` regression on presorted, reverse-sorted and string input: restore never-slower (run detection like timsort's `count_run`, or fall back when the first run covers the list) | language (builtins) | `list.sort` `ml_meth` (reached by `sorted`, which falls back `args` into it) | `list.sort` hit 422k / 252k / 52k times in black / sqlglot / dateutil suites; time share not measured | today 1.4–3.2× **slower** on those inputs → ≥ 1.0 | same subset; presorted runs → original | timsort's O(n) run scan vs the kernel's full sort | none new (perf only) | none | very high (`sorted(keys)`, `sorted(set)` of strings everywhere) | — (w2-floors) | §5.4 |
| platform reach: Windows 30 → 55, macOS/musl 74 → 77 (steps 1, 2, 5) | all groups | `GetProcAddress` symbol lookup; `strstate` accessor; gate only C-library datetime targets; 6 posixpath + `fnmatchcase` ungated | every workload's share of the gained groups on Windows (json, copy, logging, random, textwrap) | as on Linux | unchanged | — | MSVC/UCRT differences: needs the per-platform CI job before "on" | none | Windows ≈ a large share of developer machines (I) | patches A, B (`cargo check` 4 triples) | §6 |

**Settled / out of scope here:**

- Startup and extension-load cost: w2-startup's.
- Per-target fingerprint policy for patch releases: w2-floors' (3.15.2 and 3.14.10 activate
  all targets today, M).
- `[tool.rustique]` config: still not needed (wave-1 reasoning holds; no project here
  needed more than `RUSTIQUE_*`).

## 8. Files

- Branch `w2/p1-dx`:
  - `python/rustique/_diff.py` (diff fixes), `python/rustique/_cli.py` (runner fixes),
    `python/rustique/_estimate.py` (estimator), `tests/test_dx_cli.py` (+6 tests).
  - `benches/dx/real_projects.py`, `benches/dx/estimate_workloads.py`, `benches/dx/c_ratios.py`.
- `~/rustique-research/w2/dx/`:
  - `ci/` (CI logs, artifacts, `parse_smoke.py`), `install_matrix.{sh,out}`, `onoff/`,
    `rp/` (real-project results), `est/` (validation JSON).
  - `platform.md` and the two patches, `sortcheck.py`.
