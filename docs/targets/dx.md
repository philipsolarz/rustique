STATUS: wave 2, workstream `dx`, 2026-11-03. Design decisions and measurements of the
user-facing layer. Usage: [`docs/USAGE.md`](../USAGE.md). Phase-1 evidence:
[`docs/atlas/w2-dx.md`](../atlas/w2-dx.md). The wave-1 version of this file (the switch's
history, the `.start` measurements, the wave-1 wheel proof) is at commit 9e1eb4a. Every
number is **M** (measured: how, where) or **I** (inferred: from what). "Box" = the shared
wave-2 dev box (c7a.8xlarge, shared by nine workstreams: wall times are indicative).

# dx: on, off, inside out, predicted, and proven in CI

## Summary

| deliverable | what ships | key measurement / decision |
|---|---|---|
| zero-code switch | `RUSTIQUE=1` through `_rustique_startup.pth`, which imports the top-level `_rustique` extension (engine's loader; wheels repacked in CI) | unchanged semantics; wave-1 switch tests now layout-agnostic (§1) |
| control | `RUSTIQUE=0` kill switch over everything (tiers too), `RUSTIQUE_ONLY`/`_EXCLUDE`, **`RUSTIQUE_TIERS`** (`--tiers`, `--rustique-tiers`), `RUSTIQUE_UNVALIDATED_PATCH`; `python -m rustique app.py` is `python app.py` (`__file__`, tracebacks, SIGINT, children); `--native` removed with a pointer to MIGRATION.md | §2 |
| visibility | reports count **every process once**: forked children start at zero, `os._exit`/killed workers counted from live counter files (accel), `report --from FILE.json`; suspensions, `continue`/`abandon` restarts, armed/unvalidated targets, tiers; each fallback reason and refusal category says **what to do** | §3 |
| CI proof | `diff`: 0 false alarms on 14 real suites (volatile-output rule, masks), **test node ids from the plugin and confirmation re-runs** (a test flaky without Rustique is not a difference); plugin: `--rustique-tiers`, `unvalidated-patch` expected, `--rustique-outcomes` | §4 |
| "will it help me?" | `python -m rustique estimate -- <cmd>`: CPU-time stack sampler (signal timer, safe on free-threaded builds), C-level targets sampled at their call sites, stacks of targets split by how often each kernel takes the call, entry points from `status()`, thread-CPU profiler for threaded programs | 50–51/65 within ±0.03 on the loaded box (T13 asks 52); 47/55 without the threaded and service families (§5.2) |
| wheels and platforms | `wheels.yml`: repack to top-level `_rustique` + layout check on all 32 wheels; interp tier built with clang ≥ 19 on Linux x86_64 and asserted; **per-platform job**: Rustique's tests + CPython's test modules for every installed target on 8 platforms × 4 builds; coverage table + floors; windows-arm64 promoted | §6 |
| docs | `docs/USAGE.md` rewritten (platforms, versions, patch releases, tiers, estimate, reports, CI recipes, troubleshooting, migration) | — |

## 1. Zero-code switch

The `.pth` line is engine's (DECISIONS §10.7):
`import os; os.environ.get('RUSTIQUE', '0') != '0' and __import__('_rustique').startup()`.
Release wheels carry the extension as a top-level `_rustique.<abi>.{so,pyd}` (repacked in CI,
§6). sdist and editable builds keep maturin's in-package layout behind a top-level
`_rustique.py` shim. The extension registers as both `_rustique` and `rustique._rustique`.
OFF values other than `0`, and `-E`/`-I`, unregister it again, so "nothing loaded" still holds.
The startup cost is w2-startup's and engine's to report.

dx's part:

- `tests/test_dx.py` is still the contract of the switch semantics, run against the shipped
  `.pth` line through `site.addsitedir`. Engine adapted its layout tests to the top-level
  module during the merge.
- `wheel_smoke.py` asserts the same in installed wheels: `RUSTIQUE=1` loads exactly
  `{_rustique, rustique._rustique}`, one module object, never the package.

## 2. Control

| path | `RUSTIQUE=0` | `RUSTIQUE_ONLY`/`_EXCLUDE` | `RUSTIQUE_TIERS` | `RUSTIQUE_STATS`/`_REPORT`/`_CHECK` |
|---|---|---|---|---|
| `RUSTIQUE=1` at startup | n/a | applied | applied | applied |
| `python -m rustique [--tiers T] app.py` | no-op, runs plain | applied (∩ / ∪ with `--only`/`--exclude`) | `--tiers` ∪ env (env `none` wins) | applied |
| `rustique.enable(tiers=...)` | no-op | applied | ∪ env (env `none` wins) | applied |
| `pytest --rustique [--rustique-tiers T]` | no-op; summary says so | applied | as the CLI | applied |
| `report`/`diff`/`estimate` | refuse (exit 2) / plain side | passed to the enabled run | `--tiers`, passed to the enabled run | set by the command |

Decisions:

- **Tiers are opt-in, and the environment can still turn them off.** `RUSTIQUE_TIERS=interp,gc`
  is added to the code's `tiers=` (the union). `none`/`0`/empty turns every tier off, even
  one the code asked for. `RUSTIQUE_EXCLUDE=interp` removes one, and `RUSTIQUE=0` kills all.
  Unknown tier names warn like unknown groups. Agreed with accel through the lead;
  implemented in `accel/switch.rs` (wave2 a000743), with `rustique.enable(tiers=)` (74eeb03).
  The CLI and the plugin pass `--tiers` as the
  environment variable (`_cli.add_tiers`). So spawned children get the same tiers, and an
  operator's `RUSTIQUE_TIERS=none` wins over the flag, with a warning.
- **`--native` is gone** (DECISIONS §10.1). `python -m rustique --native …` exits 2 with one
  line pointing to `docs/MIGRATION.md`. A `--native` after the program is the program's own
  argument (tested).
- **The runner is `python app.py`** (Phase-1 fixes, now shipped): absolute `__file__`, the
  traceback starts at the program's frame, exit 1 / death by SIGINT as CPython,
  `RUSTIQUE=1` and the selection exported to child interpreters.
  `test_run_is_like_python_script` compares bytes with `python script.py`.
- Value options of `report`/`diff`/`estimate` (`--repeat 3`, `--from F`, …) no longer swallow
  their value as the command (`split_command`, a latent wave-1 bug).

## 3. Visibility

### 3.1 Every process once (interface agreed with accel through the lead)

Phase 1 measured three ways the report lied about multi-process programs:

- a fork `Pool` doing 8 accelerated calls reported 0 (workers end in `os._exit`, or are
  SIGTERMed by `Pool.__exit__`);
- uvicorn's 2 workers served 50 requests and reported 1 hit;
- a forked child with 10 calls reported 110, its parent's 100 included.

The interface:

| piece | owner | contract |
|---|---|---|
| fork reset | accel | a `pthread_atfork` child handler zeroes every target's counters (hits, fallbacks, suspensions) and drops the inherited mapping; kernel state (score, suspension) stays. `status()["process"]["ppid"]` = the parent's pid for a process forked after enable |
| live counter file | accel | while counters run into a JSON report `P`, each process keeps its counters in a `MAP_SHARED` file `P.<pid>.cnt` (Linux, macOS; Windows: exit-hook JSON only this wave). A fork child creates its own file (`posix.register_at_fork`). Children started by `subprocess` (vfork/exec) create none until they enable |
| layout | both | header of 8 native-endian u64: magic `b"RQCNT\0\0" + version`, pid, ppid, n_targets, names_off, counters_off, flags (bit 0: the JSON record was written at exit), row_words. Names: NUL-terminated UTF-8. Rows v1: hits, args, deps, tools, interp, deopt, error, check, suspensions. v2 adds `continue`, `abandon` |
| reader | dx | `_report.collect(P)`: the JSON records, plus a counters-only record for each `.cnt` whose pid wrote none (`os._exit`, killed). A file still being created, or a layout with more columns than the reader knows, is read safely (the known columns) |
| merge/render | dx | counters add up (each process counts its own calls); the header says "N processes (k forked, m ended without exit hooks, counted from their live counters)" |

Tests: `test_collect_reads_live_counter_files` (v1 and v2 rows, exit flag, torn files,
forked pid), `test_counter_file_longer_rows`, `test_merge_adds_counters`, and
`test_report_counts_fork_pool_workers_once`. That last test runs a fork pool of 2 workers ×
40 tasks: 81 calls must be counted, once each. It skips until the extension reports
`ppid`, i.e. until accel's change is merged.

### 3.2 What the report says, and what a developer can do about it

- Only targets that ran are listed, with a `fast` column (hits / calls). "N installed
  targets were not called" goes on one line, and the targets waiting for their module are a
  count with their modules. `--verbose` lists every name.
- **What the fallbacks mean here**: one line per fallback reason that occurred, naming the
  top three targets and saying what the reason means and what to do (table in USAGE.md).
  Reasons the reader does not know are printed as they come.
- **Suspensions** (Phase 1: sqlglot's `deepcopy` was suspended and restarted 49% of the
  time, invisibly): `suspended Nx` per target, `suspended now (fallback|deps|conflict)`, and
  the meaning of each.
- **Refusals by category**, unexpected categories first, each with "what to do". Long
  lists of *expected* refusals are shortened. Free-threaded builds refuse many generated
  targets as `build`. Unexpected categories are never shortened, and the pytest plugin
  keeps the targets accepted with `--rustique-allow-refused` ahead of any cut.
  `unvalidated-patch` names `RUSTIQUE_UNVALIDATED_PATCH=1`. `armed` (validated at the first
  call) and `unvalidated` targets are marked.
- **Tiers**: "opt-in tiers: gc on (1 of 1 targets active), interp off (…);
  RUSTIQUE_TIERS=interp turns it on". `status` marks tier groups.
- `status` reads "N available here (A active now, P activate when their module is
  imported)", the Phase-1 wording fix. It also names `python -S` and "no startup file"
  (editable, `--target`) as causes when `RUSTIQUE=1` would do nothing.
- Per-call-site attribution (`RUSTIQUE_STATS=sites`) is not in this wave. Accel ruled that
  finding the caller would materialise frame objects, which is observable. So the advice
  is per reason.

## 4. Proving "nothing changed except speed" in CI

### 4.1 `diff`

Phase-1 fixes, now shipped:

- one JUnit path for both runs;
- ISO-8601 / common-log timestamps and local ports masked;
- pytest progress characters dropped;
- test ids that differ only in digits paired;
- output that also differs between two plain runs reported as *volatile*
  (`--strict-output` to compare it anyway).

Measured on the 14 real suites: 6 false alarms before, 0 after (w2-dx §3.2).

New in Phase 2:

- **Node ids**: the plugin's `--rustique-outcomes PATH` records `{node id: outcome}` from
  `pytest_runtest_logreport` on the controller (xdist included). It is inactive and imports
  nothing unless asked. `diff` passes `-p rustique.pytest_plugin --rustique-outcomes=…`, and
  falls back to the JUnit file when the plugin is absent.
- **Confirmation re-runs** (`--confirm N`, default 2). Tests whose outcome differs are re-run
  alone, N times per side, interleaved. A re-run keeps the original command's interpreter,
  `pytest` and its `-p/-o/-c/-W/--rootdir/--confcutdir/--import-mode` options.
  - A test whose outcome also varies without Rustique is **flaky**: it is listed, not
    compared, and the 0/1 exit code and failure-report output it causes are not counted.
  - Anything else stays a difference. This includes a difference that does not reproduce in
    isolation: it may be order-dependent, which is not proof of innocence.
  - It answers Phase 1's Django `test_reporting_frames_for_cyclic_reference` case: it failed
    in 7 of 12 plain runs on 3.15.1t.
  - Tested: `test_diff_flaky_test_is_not_a_difference`, a test failing every third run, and
    `test_diff_pytest_outcome_change`, a real difference, confirmed.

### 4.2 Plugin, check mode

- The plugin adds `--rustique-tiers` and `--rustique-outcomes`.
- `unvalidated-patch` joins `version`/`build`/`jit`/`platform` as an expected refusal (never
  fails a session).
- Check mode is unchanged from wave 1.

## 5. `python -m rustique estimate`

### 5.1 Design

Three runs of the command, in this order:

1. **Enabled, counters on**: hit and fallback counts per target, and the **manifest** of entry
   points from `status()["targets"][i]["sites"]`. `name (vectorcall)` + `module` → a
   Python-level function; `owner.name (ml_meth)` → a C-level method; `(slot)` → counted
   only. So targets added by other workstreams are found without a table here. Known
   targets keep their hand-written entries (`strptime` is timed through its Python helper;
   `logging` never replays). The hook enables through `sitecustomize`, so editable installs
   without a startup file work too.
2. **Plain, profiled** (`sys.monitoring`, tool 4): calls and per-call times of every target, a
   systematic sample of 24 calls' arguments per Python-level target, the longest outermost
   call, **the call sites (file, qualname, first line, CALL offset) of C-level targets**,
   and, at exit, the **replay**. Each kept call is timed with CPython's original and with
   `enable(only=<target>)`; each side is the fastest of 5 batch means (interference only adds
   time). Once the program has a second thread, intervals are taken in thread CPU time,
   because wall intervals include waiting for the GIL. The profiler's own cost is calibrated
   for that clock too.
3. **Plain, sampled** (`Sampler`, signal mode): `setitimer(ITIMER_PROF)` fires every 0.5 ms
   of *process CPU time*. The handler runs in the main thread and walks the main thread's
   own frames, which is safe on free-threaded builds.
   - Waiting (I/O, sleeps, locks) is not sampled, and samples do not cluster where the GIL
     is released.
   - A frame stopped at a CALL that the profiled run saw call a C-level target is inside
     that target: the handler runs at the CALL's own periodic check. So C-level targets are
     sampled too, with no per-call cost to subtract (2 µs per call on the box, more than
     many of the calls themselves).
   - With several threads, each sample is split by every thread's CPU time since the
     previous one (`pthread_getcpuclockid`). Other threads' stacks come from
     `sys._current_frames()` on GIL builds, where they are parked. On free-threaded builds
     their CPU is unattributed, reported, and not promised.

**Prediction**: `ratio = 1 − Σ_t T_t (1 − r_t) / max(wall, CPU)`.

- `T_t` comes from samples: each sampled stack of distinct targets (outer → inner) gives its
  time to the outermost target in the proportion of calls its kernel takes (counters). A
  kernel that takes the call replaces everything under it. The rest goes to the next target
  inside. A `Logger.info` that always deopts leaves its time to `findCaller`, `LogRecord`
  and `Formatter.format` inside it. `relative_to`, which always hits, keeps the `PurePath`
  calls under it.
- `r_t` is the replayed ratio (Python-level); the measured per-call ratio × hit rate for
  C-level targets (`C_RATIO`; `sort`/`sorted` 1.0, §5.3); or hits × saving per hit (`in` on
  tuples/lists, `float.__repr__`). A target with no model saves nothing.
- Threaded programs use the profiler's thread-CPU times instead of the other threads'
  samples, which are biased toward GIL hand-offs at system calls. That bias was measured: in
  `job_worker`, 92% of the samples landed in the flush of a log handler.
- The printed range is the other time source. Notes say when the program waited, when
  threads went unattributed, which tiers are not predicted, and which targets fall back on
  ≥ 30% of ≥ 100 calls.

### 5.2 Validation on the 65 workloads (T13: within ±0.03 of the measured ratio on ≥ 80%)

`benches/dx/est_setup.sh` (one venv per family, a release wheel), `est_validate.sh` (per
family, one core each), `estimate_workloads.py all|measure`. "Measured" = Phase 3's
quiet-box ratio (`docs/atlas/phase3.md` §4) for wave 1's 34 workloads. For the 31 new ones,
it is the dev-box `measure` (5 interleaved fresh-process pairs × 5 runs, median wall; two
rounds averaged where both exist). The prediction ran on the loaded box (M).

**Result (3.15.1, M):**

- Final code (v5): **50/65 within ±0.03 (77%)**, 47 within ±0.02, 57 within ±0.05, median
  |error| 0.011, mean 0.027, bias +0.009.
- A second run of the same model (v4): 51/65 (78%), 58 within ±0.05.
- T13 asks for ≥ 80% (52). **Not met on the dev box**, by 1–2 workloads.
- The run-to-run spread at load 50–100 is ±2 workloads. Two of the box references disagree
  between their two rounds by 0.08 (`cache_service`) and 0.22 (`django_admin`).
- The systematic gap is threaded programs: 3 of the 5 `threads` workloads and 3 of the 4
  `services` miss (§5.3).
- Without the `threads` and `services` families and the reference that varies by 0.22:
  47/55 (85%). Per family: `custom` 10/10, `libs` 6/6, `devtools` 10/12, `data` 9/12,
  `web` 8/10, `frameworks` 4/6.

How the model got there (same 65 workloads, same references):

| version | change | within ±0.03 |
|---|---|---:|
| Phase 1 | wall-clock sampler thread, outermost target, profiler for C-level targets | 46 |
| v2 | CPU-time signal sampler, innermost target, thread-CPU clock with threads | 50 |
| v3 | stacks split by hit rate, C-level targets sampled at their call sites, replay = fastest of 5 batches | 48 |
| v4 | run-queue delay out of the denominator, C-level targets only when innermost, threads sample C-level targets | 51 |
| v5 | CPU, wall and delay from the timed runs (validation harness) | 50 |

What the signal sampler fixed (Phase 1 → v5):

| workload | Phase 1 error | v5 error |
|---|---:|---:|
| `fs_inventory` (I/O) | +0.250 | within 0.03 |
| `asyncio_http` | +0.206 | within 0.03 |
| `json_api` (`default=` → `asdict`) | +0.077 | within 0.03 |
| `logging_service` | +0.003 | −0.040 |
| `state_snapshot` | −0.138 | within 0.03 |

Per workload (v5; `P3` = Phase 3 quiet box; `box ×n` = n rounds on the dev box; **x** =
outside ±0.03):

| workload | measured | source | predicted | error | under targets | top saver |
|---|---:|---|---:|---:|---:|---|
| custom/di_plugins | 0.991 | box ×1 | 0.998 | +0.007 | 0% | tuple.__contains__ (0%) |
| custom/ecs_arena | 0.987 | box ×1 | 0.998 | +0.011 | 6% | builtins.min (0%) |
| custom/event_ledger | 0.982 | box ×1 | 1.000 | +0.018 | 0% | dataclasses.asdict (0%) |
| custom/fulfilment_sim | 0.995 | box ×1 | 0.997 | +0.002 | 0% | random.randrange (0%) |
| custom/mini_interp | 0.988 | box ×1 | 1.000 | +0.012 | 0% | tuple.__contains__ (0%) |
| custom/order_domain | 1.001 | box ×1 | 0.995 | -0.005 | 1% | builtins.min (0%) |
| custom/orm_mapper | 0.971 | box ×1 | 0.979 | +0.008 | 3% | datetime.date.isoformat (1%) |
| custom/report_engine | 0.900 | box ×1 | 0.916 | +0.016 | 9% | datetime.date.strftime (3%) |
| custom/rules_engine | 0.998 | box ×1 | 0.996 | -0.002 | 1% | float.repr (0%) |
| custom/task_planner | 0.997 | box ×1 | 0.996 | -0.001 | 5% | builtins.max (0%) |
| data/backup_archive | 0.996 | P3 | 0.991 | -0.005 | 2% | builtins.sum (0%) |
| data/billing_engine | 0.970 | P3 | 0.905 | -0.065 **x** | 16% | builtins.min (1%) |
| data/config_merge | 1.013 | P3 | 0.997 | -0.016 | 4% | json.loads (0%) |
| data/csv_etl | 0.625 | P3 | 0.642 | +0.017 | 50% | json.dumps (5%) |
| data/fs_inventory | 0.743 | P3 | 0.749 | +0.006 | 33% | pathlib.PurePath.relative_to (6%) |
| data/log_analytics | 0.594 | P3 | 0.637 | +0.043 **x** | 41% | datetime.datetime.strptime (7%) |
| data/pandas_etl | 1.000 | P3 | 1.000 | -0.000 | 0% | tuple.__contains__ (0%) |
| data/report_render | 0.436 | P3 | 0.457 | +0.021 | 71% | textwrap.fill (9%) |
| data/state_snapshot | 0.651 | P3 | 0.624 | -0.027 | 83% | copy.deepcopy (9%) |
| data/text_corpus | 1.000 | P3 | 0.994 | -0.006 | 2% | builtins.min (0%) |
| data/tz_events | 0.591 | P3 | 0.531 | -0.060 **x** | 58% | datetime.date.strftime (14%) |
| data/xml_feeds | 1.000 | P3 | 1.000 | -0.000 | 3% | tuple.__contains__ (0%) |
| devtools/black_format | 0.990 | P3 | 0.997 | +0.007 | 1% | tuple.__contains__ (0%) |
| devtools/coverage_pyflakes_tests | 0.995 | P3 | 0.989 | -0.006 | 2% | textwrap.dedent (0%) |
| devtools/import_startup | 1.000 | P3 | 1.000 | +0.000 | 0% | - |
| devtools/isort_stdlib | 0.771 | P3 | 0.686 | -0.085 **x** | 50% | copy.deepcopy (6%) |
| devtools/pip_resolve | 0.949 | P3 | 0.963 | +0.014 | 9% | pathlib.PurePath.relative_to (0%) |
| devtools/pycodestyle_stdlib | 0.985 | P3 | 0.989 | +0.004 | 0% | list.__contains__ (0%) |
| devtools/pyflakes_stdlib | 0.997 | P3 | 0.999 | +0.002 | 0% | tuple.__contains__ (0%) |
| devtools/pylint_stdlib | 1.002 | P3 | 0.998 | -0.004 | 1% | posixpath.abspath (0%) |
| devtools/pytest_networkx | 0.948 | P3 | 0.979 | +0.031 **x** | 10% | pathlib.PurePath.__eq__ (1%) |
| devtools/rich_render | 0.998 | P3 | 0.992 | -0.006 | 4% | builtins.max (0%) |
| devtools/sphinx_autodoc | 0.977 | P3 | 0.989 | +0.012 | 5% | logging.LogRecord.__init__ (0%) |
| devtools/sqlglot_transpile | 0.968 | P3 | 0.952 | -0.016 | 19% | copy.deepcopy (4%) |
| frameworks/django_admin | 1.052 | box ×2, spread 0.22 | 0.985 | -0.067 **x** | 5% | urllib.parse.parse_qsl (0%) |
| frameworks/drf_api | 1.018 | box ×2 | 0.984 | -0.033 **x** | 9% | copy.deepcopy (0%) |
| frameworks/falcon_api | 0.801 | box ×2 | 0.794 | -0.007 | 27% | json.dumps (5%) |
| frameworks/fastapi_sqlalchemy | 0.998 | box ×2 | 0.993 | -0.004 | 1% | json.dumps (0%) |
| frameworks/flask_marshmallow | 1.005 | box ×2 | 0.992 | -0.013 | 4% | datetime.date.isoformat (0%) |
| frameworks/strawberry_graphql | 0.989 | box ×2 | 1.000 | +0.010 | 2% | float.repr (0%) |
| libs/attrs_cattrs | 0.912 | box ×2, spread 0.03 | 0.892 | -0.020 | 23% | json.dumps (2%) |
| libs/boto3_client | 0.980 | box ×2 | 0.972 | -0.008 | 6% | json.loads (0%) |
| libs/bs4_scrape | 0.985 | box ×2 | 0.997 | +0.013 | 0% | tuple.__contains__ (0%) |
| libs/jsonschema_validate | 0.993 | box ×2 | 0.999 | +0.006 | 1% | urllib.parse.unquote (0%) |
| libs/pydantic_payloads | 1.000 | box ×2 | 0.992 | -0.008 | 2% | copy.deepcopy (0%) |
| libs/yaml_config | 1.010 | box ×2 | 0.999 | -0.010 | 0% | datetime.datetime.isoformat (0%) |
| services/cache_service | 0.962 | box ×2, spread 0.08 | 0.999 | +0.038 **x** | 0% | tuple.__contains__ (0%) |
| services/job_worker | 0.628 | box ×2 | 0.876 | +0.248 **x** | 116% | json.dumps (4%) |
| services/uvicorn_fastapi | 1.003 | box ×2 | 0.967 | -0.036 **x** | 11% | copy.copy (1%) |
| services/waitress_flask | 0.971 | box ×2 | 0.985 | +0.014 | 13% | logging.Logger.findCaller (1%) |
| threads/compute_pool | 0.978 | box ×2 | 0.960 | -0.018 | 7% | builtins.min (4%) |
| threads/http_server | 0.909 | box ×2 | 0.936 | +0.028 | 11% | json.dumps (5%) |
| threads/pipeline_queue | 0.558 | box ×2, spread 0.03 | 0.889 | +0.331 **x** | 26% | datetime.datetime.strptime (6%) |
| threads/request_pool | 0.591 | box ×2 | 0.698 | +0.106 **x** | 51% | json.dumps (13%) |
| threads/shared_cache | 0.679 | box ×2 | 0.770 | +0.091 **x** | 35% | json.dumps (14%) |
| web/aiohttp_app | 0.910 | P3 | 0.944 | +0.034 **x** | 9% | json.dumps (0%) |
| web/asyncio_http | 0.791 | P3 | 0.773 | -0.018 | 31% | json.dumps (3%) |
| web/django_blog | 0.997 | P3 | 0.993 | -0.004 | 4% | urllib.parse.quote (0%) |
| web/email_mime | 0.995 | P3 | 0.996 | +0.001 | 2% | datetime.date.strftime (0%) |
| web/fastapi_pydantic | 0.983 | P3 | 0.977 | -0.006 | 6% | json.dumps (1%) |
| web/flask_jinja | 0.990 | P3 | 0.985 | -0.005 | 8% | json.loads (0%) |
| web/json_api | 0.835 | P3 | 0.843 | +0.008 | 63% | json.loads (2%) |
| web/logging_service | 0.817 | P3 | 0.777 | -0.040 **x** | 93% | logging.Logger.level (4%) |
| web/requests_local | 0.941 | P3 | 0.952 | +0.011 | 9% | json.dumps (2%) |
| web/sqlalchemy_orm | 0.997 | P3 | 0.998 | +0.001 | 1% | datetime.datetime.isoformat (0%) |

Raw data: `~/rustique-research/w2/ws-dx/est/v{1..5}-3.15.1/` (predictions with their
inputs), `m2/` and `v1-3.15.1/measured-*.json` (box measurements). Not repeated on 3.15.1t this
phase (box time). There, the sampler's safety is what was tested: `tests/test_dx_estimate.py`
passes on 3.15.1t, and the estimate command runs end to end in that suite.

### 5.3 Limits

- **Threaded programs** (`threads/*`, `services/job_worker`) are the weakest family.
  - On GIL builds, other threads are sampled where they handed the GIL over: system calls,
    the flush of a log handler.
  - The profiler's thread-CPU clock costs a system call per reading; its calibration is
    noisy under load (1.6–7.5 µs per event measured on the box) and is subtracted from small
    calls.
  - The result under-predicts: `job_worker` is predicted 0.88 and measured 0.63. That is the
    safe direction, but it is wrong.
  - Fix: per-thread CPU-time sampling (a per-thread timer, `timer_create` with
    `SIGEV_THREAD_ID`), which needs native code. On free-threaded builds other threads are
    not sampled at all (§5.4).
- **One huge call** (`state_snapshot`: one `deepcopy` of a big graph is most of the run):
  the replay runs the call repeatedly with warm caches and allocator, so it flatters the
  kernel (−0.03).
- **C-level targets cannot be replayed**: `CALL` events carry no arguments. The per-call
  ratio is a table (`C_RATIO`, from `benches/dx/c_ratios.py`) × the hit rate.
  - `strftime` formats vary (`tz_events` −0.05).
  - `sort`/`sorted` is set to 1.0 (presorted input was slower; now fixed by `core`).
- **A C-level target that calls back Python** (`any(genexpr)`, `min(key=...)`) is credited
  only with samples taken inside it, not inside the callback. The kernel cannot make the
  callback faster (`pip_resolve`: −0.07 → −0.01 with this rule).
- **Opt-in tiers** have no entry points: counted, not predicted.
- **Cost**: three runs. The profiled one is 1–5× slower than plain, and the replay adds
  0–3 s.

### 5.4 Free-threaded builds, and an upstream item

Phase 1 crashed CPython 3.15.1t with an in-process sampler thread: 1 crash in 3 runs of
`data/xml_feeds`, in the walk of another thread's `f_back` chain (M, w2-dx §5.2).
`sys._current_frames()` stops the world only while it collects the frame pointers. Walking
`f_back` of a frame that another thread is still running is a data race on free-threaded
builds (I, from `Python/pystate.c` `_PyThread_CurrentFrames`).

The signal sampler never does that: the handler walks the main thread's own stack, and other
threads count as unattributed (`test_signal_sampler_is_safe_with_threads` runs on 3.15.1t).
**Upstream item (to file, lead)**: document that the frames returned by
`sys._current_frames()` must not be walked while their threads run on free-threaded builds,
or return snapshots there. Every sampling profiler written in Python (py-spy is
out-of-process; `pyinstrument`, `yappi`-style samplers are not) is exposed.

## 6. Wheels and platforms

### 6.1 `wheels.yml`

- **Repack**: right after `maturin-action`, `tools/repack_wheel.py dist/*.whl` (engine's,
  stdlib-only, idempotent). Then `.github/scripts/check_wheels.py --layout top` checks:
  - exactly one root `_rustique.<abi>.{so,pyd}`;
  - nothing at `rustique/_rustique.*`, and no shim;
  - `_rustique_startup.pth` at the root;
  - every RECORD hash and size.

  The sdist job checks its wheel with `--layout shim` (maturin's layout, loaded through
  `_rustique.py`).
- **interp tier** (crates/rustique-interp, merged in 0d73921):
  - The Linux x86_64 job installs `clang` + `llvm` in the manylinux_2_28 container
    (`before-script-linux`, version ≥ 19 checked). It passes `RUSTIQUE_INTERP_REQUIRE=1`, so
    a missing compiler or `llvm-profdata` (PGO) fails the build.
  - Every other job builds the empty handler set: the crate's build script compiles nothing
    off x86_64 Linux glibc and needs no clang there (it reads the target cfg; musl is
    excluded too).
  - Smoke, every wheel: without `tiers=` every interp target is `disabled` with the opt-in
    reason.
  - Linux x86_64: asked for (`enable(tiers=["interp"])`), they install. This runs on the
    python-build-standalone builds of every release in `src/builds.rs` (3.15.1, 3.15.2, GIL
    and t), installed with uv 0.13.6: the tier recognises a binary by its ELF build-id, and
    each uv version pins one PBS build per release.
  - Everywhere else, asked for, they refuse ("validated on x86_64 Linux (glibc) only", or
    "built without handlers").
  - setup-python's interpreters are not PBS builds and get no interp assertion.
- **`platform` job** (new): 8 platforms × 4 builds (PRs: Linux x86_64 × 3.15, 3.15t). Each
  cell runs `.github/scripts/platform_tests.py` (stdlib only):
  1. a fresh venv with the platform's wheel from the `wheels` job;
  2. the **coverage probe**: enable, import every target module, record each target's state
     and refusal. A category outside version/build/jit/platform/unvalidated-patch fails;
  3. **Rustique's tests** for every fully installed group (`tests/targets/test_<group>*.py`),
     from a copy of `tests/` outside the source tree, plus `tests/test_dx*.py` off Windows;
  4. **CPython's own test modules** of every installed group, plain against enabled
     (`run_target_suites.py`). They come from the exact source release of the runner's
     interpreter (`--fetch-cpython`).

  This replaces the ~17 smoke calls that were the only validation off Linux (w2-dx §6).
- **`platform-report`**: `platform_report.py` tabulates every cell (installed / total,
  refusals by category, partly-on groups) into the job summary and a `platform-coverage`
  artifact, and fails a cell below its floor in `.github/platform-floors.json`.
  - The floors are wave 1's measured counts (glibc 84/79, musl and macOS 74/69, Windows
    30/28).
  - T10's bar (Windows ≥ 48, macOS/musl ≥ 77) is raised there when accel's Windows symbol
    lookup and allowlist and text's datetime gating are merged.
- windows-arm64 is promoted from experimental (all smoke cells passed in run 41362578712),
  and the `complete` job requires its wheels.
- `ci.yml`: the native-tier `Examples` and C-API capsule steps are removed (engine).

**Not run on GitHub from here** (no push). Validated locally:

- YAML parsing and a simulation of the `plan` step (8 wheel cells, 32 platform cells);
- `check_wheels.py` on repacked and maturin-layout wheels;
- the Linux x86_64 cell end to end on the box (`benches/dx/ci_local.sh`, M), with a
  `maturin build --release` wheel for 3.15.1:
  - the repack moved the extension to the root and dropped the shim, and the layout check
    passed;
  - smoke: all checks passed;
  - coverage: 84/84 installed;
  - Rustique's tests from the installed wheel: 1,773 passed (after one fix: a switch test
    must run with `-S` when the interpreter's own startup line has already loaded the
    extension);
  - conformance: 26 CPython test modules, 0 differences;
  - report: the floor is met.

  Again at 0d73921, with accel D and the interp crate:
  - the build with `RUSTIQUE_INTERP_REQUIRE=1` compiled the handler sets;
  - smoke: 15/15, including "interp tier installable";
  - coverage: 90/95 (interp's targets are opt-in);
  - tests: 1,878 passed;
  - conformance: OK.

actionlint could not be fetched in this environment (the download was not permitted). Run it
before the first dispatch.

### 6.2 Platform coverage (T10) and install guidance

- Coverage code is accel's (Windows `GetProcAddress` lookup, the `PyASCIIObject.state`
  accessor, the platform allowlist) and text's (datetime gating). Prototype patches: w2-dx
  §6 (`~/rustique-research/w2/dx/platform-*.patch`). dx owns the CI that validates it and
  the coverage report above. Status at this branch's head (wave2 c245899 merged): the
  Windows and macOS/musl changes have not landed yet, so the floors stay at wave 1's counts
  (Windows 30/28, macOS/musl 74). T10 is measured by the first platform-coverage artifact
  after accel B and text are merged.
- USAGE.md now documents:
  - `pip install --only-binary rustique` (uv: `--only-binary` / `no-build-package`) for CI
    and Docker;
  - the 1.4 GB toolchain download an sdist build triggers;
  - unvalidated platforms refusing as `platform`;
  - the uv/3.16-pre-release sdist pick;
  - the `rustique; python_version < "3.16"` marker and "never import rustique
    unconditionally", as the 3.16 upgrade path;
  - patch releases and `RUSTIQUE_UNVALIDATED_PATCH=1`.

## 7. Real projects

Phase 1 (w2-dx §4): 14 projects' own suites showed 0 behaviour differences on 3.15.1 (6 again
on 3.15.1t), with the release wheels of wave 1. `benches/dx/real_projects.py` re-runs them
under `diff`. The final run on the integrated `wave2` (14 suites on 3.15.1, 6 on 3.15.1t) is
for when the lead asks, once the target workstreams are merged:
`real_projects.py setup --wheels DIR` with a release wheel of the integrated branch, then
`run` and `table`. Not run in this phase.

## 8. Tests

- `tests/test_dx.py` — the switch contract, the report renderer and merger, live counter
  files.
- `tests/test_dx_cli.py` — the runner (`--native`, `--tiers`), status, report (`--from`,
  fork pools), diff (outcome changes, flaky tests, re-run commands, volatile output, masks),
  and the plugin (tiers, outcomes, refusal categories, check mode, xdist).
- `tests/test_dx_estimate.py` — the manifest, chain attribution, prediction arithmetic, the
  signal sampler (CPU not waiting, threads, handler restored, steps aside for a program's
  own SIGPROF), the profiler (C children, thread clock), and the command.

Full suite per build, at 9012995 / 12ab2e5 (wave2 d1c0a78 merged, with accel D, core, text,
datamodel and interp; dev box, `pytest -q tests`, M):

| build | result |
|---|---|
| 3.15.1 | 2,466 passed, 36 skipped |
| 3.15.1t | 2,481 passed, 21 skipped |
| 3.14.9 | 2,450 passed, 52 skipped |
| 3.14.9t | 2,465 passed, 37 skipped |

Skips that are dx's:
- the fork-pool counter test, until accel D;
- the xdist test, where `pytest-xdist` is not installed.

dx changes no Rust and no target, so there is no conformance or callgrind cell of its own.
The CI cell above ran CPython's test modules for all 84 targets against the release wheel:
0 differences.

## 9. Open issues and decisions for the lead

1. **T13** (§5.2): the remaining misses are threaded programs (§5.3) and references measured
   on the loaded box. Two references disagree between two measurement rounds by 0.08 and
   0.22. Please re-measure the 31 new workloads' ratios on the quiet box in Phase 3 and run
   `benches/dx/est_validate.sh` (STEPS=est) there: it prints the table of §5.2.
2. **Accel D** (counters): `test_report_counts_fork_pool_workers_once` skips until
   `status()["process"]["ppid"]` exists. The reader and renderer already handle the
   agreed `.cnt` layout v1/v2 (tested with synthetic files). Windows gets the exit-hook JSON
   only this wave, so a killed Windows worker is not counted (documented).
3. **Tiers**: done end to end (accel's switch, `enable(tiers=)`, the CLI and plugin through
   `RUSTIQUE_TIERS`, status and report). Until `interp`/`gc` merge there is no tier target,
   so `--tiers gc` warns "no tier named 'gc'".
4. **wheels.yml has never run on GitHub** (no push from here). The first tag or dispatch is
   the proof for:
   - the repack on Windows (`.pyd`);
   - the clang 19+/llvm install with `dnf` in manylinux_2_28 (AlmaLinux 8; if its clang is
     older, use `RUSTIQUE_INTERP_CC` with a toolset clang);
   - uv 0.13.6 still serving the PBS builds that `src/builds.rs` records;
   - `--fetch-cpython` on every runner;
   - the platform job's run time (estimated 10–40 min per cell, I).

   Please run actionlint first.
5. **Platform floors** are wave 1's counts. Raise `.github/platform-floors.json` to T10
   (Windows 48, macOS/musl 77) when accel's and text's platform changes are merged, and update
   USAGE.md's platform table (today's coverage: Windows refuses `copy`/`logging`/`random`/
   `textwrap` for want of run-time symbol lookup, macOS/musl refuse `isoformat` with the
   rest of datetime text) from the first platform-coverage artifact.
6. **Upstream** (to file): `sys._current_frames()` frames must not be walked on
   free-threaded builds (§5.4). The earlier items stand: the `.pth` `utf-8-sig` decode, and
   `.start` entry points importing `pkgutil` (w2-startup §6).
7. Not done, ranked: `diff --from-ci plain.xml` (compare with the JUnit file the normal CI
   job already produced, halving the cost); a fixed `pytest-randomly` seed in `diff` (today
   the recipe passes `-p no:randomly`); per-thread CPU sampling for `estimate` (native).
