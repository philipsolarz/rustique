STATUS: wave 2, Phase 1, slice `w2-attribution`, 2026-11-02. Every number is **measured**
(M) on the dev box (c7a.8xlarge, shared, hardware cycles) unless tagged **inferred** (I).
Tool: `benches/atlas/` (`layers.py`, `report.py`, `_perfdata.py`, `_taxonomy.py`, `_whole.py`).
Raw profiles: `~/rustique-research/w2/attribution/profiles/bpf2/` (one JSON per workload ×
build × mode, with the full sample counters). Generated tables: `~/rustique-research/w2/attribution/rollup/`.

# Where the time goes, by layer, in 65 real programs

## 1. Answer

1. **Layers, mean of 65 workloads on 3.15.1 (M):**
   - own 10.1%, generated 2.1%, language 13.8%, stdlib 32.0%, thirdparty 20.5%, runtime 19.7%, unattributed 1.9%.
   - The 34 wave-1 workloads alone: own 3.6, generated 0.8, language 11.6, stdlib 38.2, thirdparty 21.3, runtime 22.8.
   - Family means differ a lot:
     - custom applications: 41.8% own.
     - frameworks and libraries: 36–49% thirdparty and 16–20% language.
     - data and web scripts: ~50% stdlib.
     - devtools: 32% runtime, GC 6–49%.
   - The free-threaded build has the same shape (§3).
2. **The interpreter is the largest single pool, and it is now measurable per opcode.**
   - 90.1% of all cycles have an opcode handler (`_TAIL_CALL_<OP>`) on the stack; 37.7% are inside handlers themselves.
   - Specialized handlers take 56.2% of cycles (inline plus callees); generic or non-specializing ones 31.8% (M, §4).
   - The 15 largest (handler → C entry) pairs make up 14.1% of all time.
   - The largest generic path is LOAD_ATTR → `PyObject_GenericGetAttr` / `_PyObject_GetMethodStackRef`: 3.7% mean, 7.9% in the frameworks family.
   - CALL_ISINSTANCE is 1.6% (max 11.4%, sqlglot).
3. **Third-party code spends about half of its own time in CPython-provided code.** In the 35 programs that use third-party code, it is innermost for 71.2% of cycles; 34.6 of those points go to language operations, runtime and C stdlib modules.
   - Under the innermost third-party frame, mean % of all cycles: its own bytecode 16.5% (handlers inline); native language operations 7.4% (attribute lookup 2.5, dict 1.6, str 0.8, calls 0.8); stdlib Python functions 5.8%; C stdlib modules 2.9%; runtime 4.2% plus GC 3.0% (M, §5).
4. **What remains in wave-1-accelerated modules with Rustique enabled is small.**
   - The mean remaining share is 4.8% of plain run time (12.1% before enabling).
   - It sits in specific uncovered paths (M, §6):
     - `logging` handlers/formatting: 53% of `logging_service`, 21% of `threads/shared_cache`;
     - `glob` / `os.scandir`: 37% + 17% of `fs_inventory`;
     - `dataclasses`: 16% of `isort`;
     - `json.encoder`: 10% of `json_api`;
     - `datetime`: 17% of `tz_events`;
     - `urllib.parse`: 1–5%.
5. **Runtime:**
   - deallocation 7.4%, cyclic GC 6.1% (max 49%), allocation 3.8%, import 1.1% (M).
   - Whole process: interpreter startup is only 1.3% and the workload import 0.8%; setup 14.1%; finalization 6.0%. In finalization, the exit-time GC is 40.6% of all `pylint_stdlib` cycles and 19% of `pytest`/`fastapi` (M, §7).
   - Free threading: CPU time +17% (t/GIL geometric mean of CPU s per run over 64 pairs, median +14%; M, noisy box).
     - Largest components: `rt:ft` +7.1% of GIL time, handlers +3.4%, dict +2.5%, mimalloc allocation +2.4%.
     - One CPython bug alone costs 70% of `isort` and 27% of `state_snapshot` on 3.15.1t: `set.__reduce__` contends with its own critical section (§7.3).
6. **Method:**
   - `-X perf` (used by wave 1) installs a PEP 523 frame evaluator, which despecializes every Python-to-Python call.
   - This tool reads the current Python frame from `tstate` with eBPF instead, so the interpreter runs as in production (§2).
   - Wave 1's "46% interpreter + calls + attribute lookup" was about 5 points high. The same bucket measured distortion-free on the same 34 workloads is **41.1%**; with `-X perf` on the same runs it is 44.8% (M, §8). The -X perf processes ran 1.06× (34) to 1.12× (65) longer, geometric mean, up to 1.4× (I, noisy box).

## 2. Method and validation

### 2.1 Sampling without distortion (`--modes bpf`, the headline numbers)

- **Event:** hardware `cycles`, one sample every 2,000,003 cycles. The sampler is an eBPF program (bpftrace 0.20, `hardware:cycles`) filtered to the workload's PID, all threads. The steady-state loop is gated: `setup()`, one warm-up `run()`, then `run()` repeatedly for 8 s while sampling.
- **Per sample the program records:**
  - the user stack by frame pointers (python-build-standalone builds with `-fno-omit-frame-pointer -mno-omit-leaf-frame-pointer`), leaf first, 48 frames;
  - the current Python frame of the sampled thread, read through `_PyRuntime._Py_DebugOffsets` (the interface remote debuggers use): `interp → threads list (match native_thread_id) → current_frame → f_executable, instr_ptr`, plus 6 caller frames;
  - `co_filename` / `co_qualname` of the code object, so that code freed during the run is still named.
- **Exact owners:** `tstate->current_frame` is updated on every inlined push and pop. Owner attribution is therefore exact without a PEP 523 hook: specialized calls (`CALL_PY_EXACT_ARGS`, `CALL_ALLOC_AND_ENTER_INIT`, `FOR_ITER_GEN`, `LOAD_ATTR_PROPERTY`) stay inlined, as in production.
- **Code objects:** the driver dumps every code object reachable at the start and end of the gated loop: `id`, bytecode range, file, qualname, globals `__name__`, adaptive bytecode.
- **Opcode and instruction state:** the opcode is the innermost `_TAIL_CALL_<OP>` symbol on the stack. `instr_ptr` gives the exact instruction. Its adaptive form at the end of the run says whether a generic handler ran an unspecialized instruction or a deopt (§4).
- **Coverage checks** (per workload in the raw profiles):
  - sampled cycles ÷ CPU seconds = 2.3–3.5 GHz, so no samples were lost. Thread workloads read lower because of all-core clocks;
  - owner resolved exactly for 98–100% of samples. `coverage_pyflakes_tests` (49%) and `pytest_networkx` (72%) are the exceptions: they compile and free code every run, so those samples were named from the code object's name strings.
- **Pitfalls found and fixed:**
  - bpftrace's default map size (4,096 keys) silently dropped 60% of the samples of diverse workloads. Now 262,144 keys, verified by the GHz check.
  - Garbage names read from frames being torn down (RETURN_VALUE), and the interpreter's `__init__` shim frame, are re-labelled "unresolved". They are imputed from the owner distribution of the same opcode (≤ 1.2% of any workload).
  - About 1.9% of samples had no user stack (bpf_get_stack failed). They are reported as "other: no native stack".
- **Two other modes, kept for calibration:**
  - `--modes xperf` is the wave-1 method: `perf record -g` with `python -X perf`, full Python stacks through `py::` trampolines. It is distorted (§8).
  - `--modes plain` finds the bytecode pointer in registers and in a DWARF stack dump. It is rejected: next_instr is often not kept across C calls, and stale pointers misattributed 50% of the dataclass case's CALL_ALLOC_AND_ENTER_INIT time to the callee (M, below).

### 2.2 Classification (the six layers)

The owner is the innermost Python frame's code object.

| owner | rule |
|---|---|
| own | the workload file, its directory, and code it writes under /tmp (sphinx `conf.py`) |
| generated | dataclasses `__create_fn__.<locals>.*` (`<string>`); namedtuple (`namedtuple_<T>` globals) and `collections.namedtuple.<locals>.*`; `functools._{lt,gt,le,ge}_from_*`; all of `enum.py`; `dataclasses.py` closures; `<attrs generated …>`, `<cattrs generated …>` |
| stdlib | path under `sysconfig` stdlib (two-level for packages: `json.encoder`, `email._header_value_parser`), `<frozen X>` |
| import | importlib bootstrap (frozen, `importlib/__init__.py`) |
| thirdparty | file → distribution through every installed distribution's RECORD (`importlib.metadata`), else the top-level package; Jinja2 templates (`<template>`) count as Jinja2 |

Each sample's native component comes from its leaf symbol, using an own ELF symbolizer (`nm` + PT_LOAD, BOLT-aware), prefix rules and a CPython-3.15.1 source index. Leaves that only move bytes (`memcpy`, kernel) take their caller's component.

The layer is the first rule that matches, in this order (exclusive, so each sample counts once):

1. **runtime:**
   - GC frames anywhere below the Python frame;
   - import machinery (import frames, importlib owner, `IMPORT_NAME`/`IMPORT_FROM`);
   - leaf in allocation, deallocation (incl. a `*_dealloc` caller chain), free-threading sync, or GIL/locks.
2. **generated:** by owner.
3. **stdlib:** by owner.
4. **Own and third-party bytecode, decided by the C entry point:**
   - the outermost native frame below the handler is a C stdlib module → stdlib:<module> (`_sre` called from own code is `stdlib:re`);
   - a third-party extension → thirdparty:<dist>;
   - Rustique → stdlib (accelerated).
5. Otherwise, interpreter components → **own** / **thirdparty** (bytecode); anything else → **language:<category>**.

Owner-inclusive views (e.g. everything under a third-party frame) are reported separately (§5).

### 2.3 Validation (M, `layers.py validate`, `benches/atlas/cases/`, 4 s each)

| case | expected | bpf (headline) | dwarf stack scan | -X perf |
|---|---|---|---|---|
| `json.dumps` of 60k dicts | > 95% under `encoder_*` | 98.6% | 98.9% | 98.8% |
| dataclass `__init__` loop | generated owns `__init__`; the allocating `CALL_ALLOC_AND_ENTER_INIT` runs in the caller | generated 44.1%; CALL_ALLOC_AND_ENTER_INIT 25.0/25.6% own | generated 69.4% (misattributes the caller's alloc) | generated 35.5% (PEP 523 despecializes the call; 16% moves to `type_call`/`slot_tp_init`) |
| GC loop over a 1.2M-object heap | runtime:gc | 100% | 100% | 100% |
| method/function call loop | own; specialized calls visible | own 84.8%, CALL_PY_EXACT_ARGS + CALL_BOUND_METHOD_EXACT_ARGS 17.6% | own 81.8%, 19.1% | own 75.1%, **0.0%** |

Run-to-run note (M): tight loops show two microarchitectural states. In `calls_attrs`, repeated profiles give CALL_PY_EXACT_ARGS 11.6% or 19.8% of cycles at 183M or 172M cycles per run, independent of method. Per-opcode shares of single microbenchmarks are ±50%; means over real workloads are stable.

Reproduce:

```sh
# on the dev box (perf, bpftrace, sudo); one venv per family (pins conflict)
benches/atlas/setup_venvs.sh /work/<ws>/venvs 3.15.1 web
python benches/atlas/layers.py validate --python $(which python) --outdir out/val
python benches/atlas/layers.py batch web data devtools --venvs /work/<ws>/venvs --build 3.15.1 \
    --outdir out/prof --modes bpf,xperf          # + --enable --pythonpath <unpacked wheel>
python benches/atlas/report.py out/prof/*__3.15.1.json.gz --md all.md --json all.json
python benches/atlas/report.py handlers out/prof/*__3.15.1.json.gz --md handlers.md --json h.json
python benches/atlas/report.py enabled --plain … --enabled … --md enabled.md --json e.json
python benches/atlas/report.py ftdiff --gil … --ft … --md ftdiff.md --json ft.json
python benches/atlas/layers.py batch … --modes whole,wholeplain --suffix=-whole
python benches/atlas/report.py whole out/prof/*-whole.json.gz --md whole.md --json w.json
```

## 3. Layers per family and per workload (M, 3.15.1 steady state, % of cycles)

Families: web/data/devtools (wave 1, 34), frameworks (6), libs (6), services (4) from
`w2-wl-frameworks`, custom (10) from `w2-wl-custom`, threads (5, 8 threads each) from
`w2-threads`. `(x)` = the steady state runs in child processes (`import_startup`); that row
is the -X perf profile including the children.

| family | n | own | gen. | lang. | stdlib | 3rd-party | runtime | other |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| custom | 10 | 41.8 | 6.8 | 17.5 | 18.4 | 0.0 | 14.0 | 1.6 |
| data | 12 | 6.9 | 0.8 | 9.6 | 50.5 | 10.9 | 20.4 | 1.0 |
| devtools | 12 | 0.1 | 1.0 | 13.5 | 18.0 | 33.3 | 32.2 | 1.8 |
| frameworks | 6 | 1.7 | 1.0 | 20.2 | 21.4 | 35.7 | 16.7 | 3.3 |
| libs | 6 | 1.3 | 4.9 | 16.4 | 11.1 | 49.2 | 14.9 | 2.2 |
| services | 4 | 3.1 | 0.2 | 10.9 | 37.0 | 25.0 | 21.9 | 2.0 |
| threads | 5 | 16.8 | 0.5 | 12.4 | 50.4 | 0.0 | 18.4 | 1.6 |
| web | 10 | 3.9 | 0.6 | 11.6 | 47.8 | 19.4 | 14.3 | 2.4 |
| **all** | 65 | 10.1 | 2.1 | 13.8 | 32.0 | 20.5 | 19.7 | 1.9 |

Same on **3.15.1t** (M; layer shares are close to 3.15.1; the costs differ, §7.3):

| family | n | own | gen. | lang. | stdlib | 3rd-party | runtime | other |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| custom | 10 | 39.4 | 6.9 | 20.3 | 17.7 | 0.0 | 14.1 | 1.7 |
| data | 12 | 7.5 | 0.8 | 9.9 | 50.2 | 10.4 | 20.0 | 1.2 |
| devtools | 12 | 0.4 | 0.7 | 15.2 | 23.0 | 30.0 | 28.5 | 2.3 |
| frameworks | 6 | 2.0 | 0.8 | 21.6 | 20.6 | 33.4 | 18.3 | 3.3 |
| libs | 6 | 1.4 | 5.0 | 17.4 | 11.1 | 48.0 | 15.1 | 2.1 |
| services | 4 | 3.0 | 0.2 | 12.2 | 40.0 | 23.6 | 18.4 | 2.8 |
| threads | 5 | 16.6 | 0.3 | 9.8 | 49.7 | 0.0 | 21.7 | 2.0 |
| web | 10 | 4.0 | 0.5 | 12.5 | 46.7 | 19.0 | 14.7 | 2.6 |
| **all** | 65 | 9.9 | 2.0 | 14.8 | 32.6 | 19.3 | 19.3 | 2.1 |

Per workload (3.15.1):

| workload | own | gen. | lang. | stdlib | 3rd-party | runtime | other | largest parts (stdlib / third-party / runtime / generated) |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| custom/di_plugins | 37.3 | 3.5 | 16.7 | 28.3 | 0.0 | 12.2 | 2.0 | inspect 17.2, typing 8.4, dealloc 9.2, alloc 3.0 |
| custom/ecs_arena | 57.3 | 10.4 | 20.0 | 0.9 | 0.0 | 10.3 | 1.1 | dealloc 8.5, dataclasses 10.4 |
| custom/event_ledger | 30.5 | 4.2 | 14.5 | 34.6 | 0.0 | 14.8 | 1.4 | functools 28.3, weakref 5.7, dealloc 9.6, alloc 2.8, dataclasses 4.2 |
| custom/fulfilment_sim | 60.6 | 1.4 | 13.5 | 12.9 | 0.0 | 10.5 | 1.0 | heapq 8.1, random 3.4, dealloc 6.8, gc 2.3 |
| custom/mini_interp | 61.3 | 0.0 | 29.4 | 0.2 | 0.0 | 7.2 | 1.8 | dealloc 4.8, alloc 2.4 |
| custom/order_domain | 36.1 | 18.7 | 14.2 | 8.8 | 0.0 | 20.3 | 2.0 | decimal 8.7, gc 9.5, dealloc 7.9, dataclasses 16.3 |
| custom/orm_mapper | 23.9 | 2.2 | 18.5 | 38.6 | 0.0 | 14.4 | 2.5 | sqlite3 30.5, weakref 5.1, dealloc 5.8, gc 3.6, enum 2.2 |
| custom/report_engine | 36.5 | 10.4 | 15.4 | 19.7 | 0.0 | 16.0 | 2.0 | decimal 8.7, datetime 6.2, dealloc 8.5, alloc 4.8, dataclasses 10.2 |
| custom/rules_engine | 28.5 | 15.5 | 8.9 | 22.7 | 0.0 | 22.8 | 1.7 | contextlib 16.5, inspect 2.2, gc 9.4, dealloc 9.3, dataclasses 10.9 |
| custom/task_planner | 45.7 | 1.6 | 23.7 | 17.0 | 0.0 | 11.2 | 0.8 | heapq 14.9, dealloc 7.5, alloc 2.5 |
| data/backup_archive | 0.1 | 0.0 | 0.0 | 99.3 | 0.0 | 0.4 | 0.2 | gzip 62.3, zipfile 32.1 |
| data/billing_engine | 10.4 | 3.0 | 6.7 | 22.4 | 0.0 | 56.5 | 1.0 | decimal 12.0, statistics 6.0, gc 41.2, dealloc 7.7, enum 2.3 |
| data/config_merge | 2.1 | 0.0 | 15.2 | 15.8 | 55.7 | 10.4 | 0.9 | configparser 7.1, tomllib._parser 4.2, PyYAML 55.7, dealloc 5.7, alloc 2.6 |
| data/csv_etl | 11.2 | 0.0 | 10.7 | 57.0 | 0.0 | 19.4 | 1.7 | json.encoder 21.2, csv 19.5, dealloc 9.4, alloc 5.9 |
| data/fs_inventory | 3.7 | 0.0 | 2.5 | 81.3 | 0.0 | 11.8 | 0.7 | glob 35.9, os 15.8, dealloc 5.9, alloc 3.9 |
| data/log_analytics | 5.6 | 0.2 | 6.6 | 70.7 | 0.0 | 15.5 | 1.4 | _strptime 21.5, re 18.1, dealloc 7.8, alloc 4.3 |
| data/pandas_etl | 2.0 | 0.0 | 13.1 | 0.9 | 74.6 | 8.5 | 0.9 | pandas 54.4, numpy 20.2, dealloc 5.0, alloc 3.2 |
| data/report_render | 7.5 | 0.0 | 9.7 | 67.7 | 0.0 | 14.0 | 1.1 | textwrap 34.9, random 24.7, alloc 6.4, dealloc 5.8 |
| data/state_snapshot | 0.5 | 6.6 | 0.4 | 55.1 | 0.0 | 36.4 | 1.0 | copy 28.5, pickle 23.2, gc 18.0, dealloc 13.3, dataclasses 6.5 |
| data/text_corpus | 22.9 | 0.0 | 25.6 | 41.2 | 0.0 | 9.8 | 0.5 | re 19.9, collections 11.9, dealloc 5.4, alloc 3.9 |
| data/tz_events | 12.3 | 0.0 | 17.0 | 44.7 | 0.0 | 24.5 | 1.5 | datetime 38.3, _strptime 5.1, dealloc 10.1, import 8.2 |
| data/xml_feeds | 3.9 | 0.0 | 8.0 | 49.8 | 0.0 | 37.2 | 1.1 | xml.etree 49.7, gc 24.0, dealloc 8.0 |
| devtools/black_format | 0.0 | 0.9 | 15.9 | 3.1 | 47.8 | 30.3 | 2.0 | black 38.2, pytokens 9.6, gc 21.7, dealloc 5.3 |
| devtools/coverage_pyflakes_tests | 0.0 | 0.7 | 14.2 | 24.0 | 30.8 | 28.0 | 2.3 | ast 8.5, compile 2.8, pyflakes 26.4, coverage 4.3, gc 17.3, dealloc 5.5 |
| devtools/import_startup (x) | 0.0 | 2.3 | 3.4 | 16.8 | 3.4 | 74.1 | 0.0 | typing 2.2, import 36.1, gc 22.4, enum 2.3 |
| devtools/isort_stdlib | 1.0 | 2.9 | 1.4 | 38.0 | 43.2 | 12.1 | 1.5 | copy 24.8, dataclasses 8.4, isort 43.2, dealloc 7.7, alloc 4.1, dataclasses 2.7 |
| devtools/pip_resolve | 0.0 | 0.5 | 15.6 | 33.2 | 26.8 | 21.5 | 2.5 | zipfile 8.4, email.feedparser 4.2, pip 26.8, dealloc 7.4, gc 7.2 |
| devtools/pycodestyle_stdlib | 0.0 | 1.5 | 16.1 | 22.1 | 48.2 | 11.2 | 0.9 | re 15.7, tokenize 5.2, pycodestyle 48.2, dealloc 7.7, alloc 3.4 |
| devtools/pyflakes_stdlib | 0.0 | 0.0 | 12.8 | 21.2 | 25.4 | 38.8 | 1.8 | ast 17.8, pyflakes 25.4, gc 29.5, dealloc 4.6 |
| devtools/pylint_stdlib | 0.0 | 0.1 | 13.1 | 8.4 | 27.6 | 48.7 | 2.1 | ast 4.9, astroid 22.8, pylint 4.7, gc 39.9, dealloc 4.7 |
| devtools/pytest_networkx | 0.1 | 0.4 | 14.6 | 10.6 | 17.7 | 55.5 | 1.1 | pathlib 3.2, pytest 10.1, networkx 4.6, gc 49.4, dealloc 4.0 |
| devtools/rich_render | 0.1 | 2.6 | 17.7 | 10.7 | 45.5 | 20.2 | 3.2 | re 5.3, functools 4.1, rich 40.3, Pygments 3.1, dealloc 10.7, alloc 4.8, namedtuple 2.4 |
| devtools/sphinx_autodoc | 0.0 | 0.1 | 13.8 | 22.9 | 33.4 | 27.9 | 1.9 | re 11.1, Pygments 13.1, docutils 9.9, dealloc 10.0, gc 8.8 |
| devtools/sqlglot_transpile | 0.0 | 0.1 | 23.8 | 4.9 | 50.0 | 18.8 | 2.4 | copy 3.6, sqlglot 50.0, gc 10.2, dealloc 5.1 |
| frameworks/django_admin | 0.1 | 0.1 | 23.8 | 14.7 | 42.3 | 15.9 | 3.0 | sqlite3 3.8, urllib.parse 2.8, Django 41.6, dealloc 8.5, alloc 3.8 |
| frameworks/drf_api | 0.2 | 0.2 | 28.2 | 17.1 | 34.2 | 16.1 | 4.1 | sqlite3 8.1, copy 2.8, Django 26.0, djangorestframework 6.6, gc 6.5, dealloc 5.9 |
| frameworks/falcon_api | 7.5 | 0.3 | 10.0 | 50.4 | 13.9 | 15.0 | 2.8 | wsgiref.validate 27.3, inspect 10.5, falcon 13.9, dealloc 8.3, alloc 5.2 |
| frameworks/fastapi_sqlalchemy | 0.3 | 0.5 | 16.3 | 22.6 | 36.3 | 20.6 | 3.4 | asyncio.selector_events 4.0, sqlite3 3.1, SQLAlchemy 20.8, fastapi 3.1, dealloc 6.5, ft 5.6 |
| frameworks/flask_marshmallow | 0.7 | 0.4 | 23.7 | 16.3 | 44.1 | 11.2 | 3.6 | sqlite3 8.1, SQLAlchemy 31.5, Werkzeug 6.3, dealloc 7.5, alloc 2.5 |
| frameworks/strawberry_graphql | 1.7 | 4.2 | 18.9 | 7.5 | 43.6 | 21.6 | 2.6 | graphql-core 36.8, strawberry-graphql 6.8, gc 10.8, dealloc 6.8, namedtuple 2.4 |
| libs/attrs_cattrs | 4.5 | 20.8 | 14.1 | 19.5 | 20.7 | 18.2 | 2.1 | json.encoder 8.4, json.decoder 6.1, attrs 16.3, cattrs 4.4, dealloc 12.6, alloc 4.5, attrs 10.7 |
| libs/boto3_client | 0.3 | 0.5 | 23.2 | 19.0 | 42.2 | 12.0 | 2.8 | xml.etree 7.0, botocore 23.4, python-dateutil 16.9, dealloc 7.7, alloc 3.9 |
| libs/bs4_scrape | 0.2 | 0.0 | 14.4 | 17.0 | 42.2 | 23.9 | 2.3 | html 12.5, beautifulsoup4 24.0, soupsieve 18.2, gc 15.3, dealloc 4.8 |
| libs/jsonschema_validate | 0.1 | 6.6 | 18.9 | 4.4 | 51.1 | 16.3 | 2.6 | jsonschema 35.7, referencing 7.6, dealloc 11.3, alloc 4.6, attrs 6.6 |
| libs/pydantic_payloads | 3.0 | 1.2 | 7.9 | 5.3 | 66.4 | 13.5 | 2.7 | pydantic_core 64.0, pydantic 2.3, dealloc 8.5, alloc 4.8 |
| libs/yaml_config | 0.1 | 0.0 | 19.9 | 1.1 | 72.3 | 5.5 | 1.0 | PyYAML 72.3, dealloc 3.9 |
| services/cache_service | 5.1 | 0.0 | 11.5 | 44.9 | 21.8 | 15.2 | 1.4 | asyncio.selector_events 21.5, asyncio.events 4.1, redis 21.2, dealloc 10.1, alloc 4.1 |
| services/job_worker | 4.8 | 0.2 | 5.9 | 51.4 | 7.1 | 28.4 | 2.3 | logging 15.3, statistics 7.7, tenacity 4.0, cachetools 3.0, ft 7.2, dealloc 6.9 |
| services/uvicorn_fastapi | 1.4 | 0.5 | 11.2 | 21.3 | 42.5 | 21.0 | 2.1 | logging 3.3, asyncio.streams 2.7, uvloop 10.7, anyio 9.8, dealloc 8.6, alloc 3.9 |
| services/waitress_flask | 1.2 | 0.0 | 14.8 | 30.5 | 28.6 | 22.9 | 2.0 | socket 10.1, logging 5.6, waitress 13.8, Werkzeug 7.6, locks 8.4, dealloc 5.6 |
| threads/compute_pool | 58.0 | 0.0 | 27.3 | 5.4 | 0.0 | 9.1 | 0.3 | contextvars 2.8, dealloc 4.8, alloc 2.9 |
| threads/http_server | 2.0 | 0.2 | 2.4 | 70.4 | 0.0 | 22.9 | 2.0 | http.client 18.3, socket 11.7, dealloc 8.2, locks 6.7 |
| threads/pipeline_queue | 5.9 | 0.2 | 10.0 | 63.2 | 0.0 | 18.7 | 1.9 | _strptime 20.9, ipaddress 17.3, dealloc 9.1, alloc 3.9 |
| threads/request_pool | 8.0 | 1.9 | 8.8 | 53.1 | 0.0 | 26.6 | 1.6 | json.encoder 9.6, copy 8.7, dealloc 9.4, gc 6.5 |
| threads/shared_cache | 10.0 | 0.0 | 13.4 | 59.9 | 0.0 | 14.6 | 2.1 | logging 34.6, json.encoder 9.7, dealloc 6.7, alloc 5.7 |
| web/aiohttp_app | 6.9 | 0.0 | 12.1 | 28.9 | 35.8 | 14.3 | 2.1 | asyncio.selector_events 12.0, json.encoder 3.9, aiohttp 30.4, yarl 2.3, dealloc 9.9, alloc 3.9 |
| web/asyncio_http | 5.6 | 0.0 | 3.6 | 75.7 | 0.0 | 13.9 | 1.2 | asyncio.selector_events 29.1, json.encoder 12.8, dealloc 8.6, alloc 4.7 |
| web/django_blog | 0.3 | 0.1 | 22.8 | 17.6 | 40.9 | 14.7 | 3.6 | sqlite3 7.6, Django 39.4, dealloc 8.1, alloc 3.4 |
| web/email_mime | 0.3 | 0.0 | 0.2 | 78.7 | 0.0 | 17.7 | 3.1 | email._header_value_parser 38.1, email.headerregistry 18.5, dealloc 9.9, alloc 4.8 |
| web/fastapi_pydantic | 1.2 | 0.9 | 12.5 | 28.3 | 37.3 | 17.3 | 2.6 | asyncio.selector_events 2.9, re 2.7, httpx 13.3, fastapi 10.6, dealloc 8.6, alloc 3.6 |
| web/flask_jinja | 2.8 | 0.1 | 26.6 | 15.3 | 37.8 | 13.9 | 3.5 | urllib.parse 4.6, Werkzeug 15.5, Jinja2 10.1, dealloc 8.8, alloc 3.8 |
| web/json_api | 14.4 | 4.4 | 13.8 | 50.4 | 0.0 | 14.9 | 2.1 | json.decoder 9.9, json.encoder 9.2, dealloc 8.4, alloc 4.5, enum 3.0 |
| web/logging_service | 5.7 | 0.0 | 2.7 | 80.2 | 0.0 | 10.1 | 1.2 | logging 63.2, json.encoder 7.0, dealloc 5.9, alloc 3.5 |
| web/requests_local | 0.9 | 0.5 | 8.4 | 57.8 | 16.8 | 13.2 | 2.4 | http.client 10.0, urllib.parse 7.2, requests 8.4, urllib3 8.4, dealloc 8.1, alloc 3.7 |
| web/sqlalchemy_orm | 0.6 | 0.3 | 13.6 | 45.2 | 25.2 | 13.1 | 2.2 | sqlite3 43.6, SQLAlchemy 25.2, gc 5.4, dealloc 5.0 |

### 3.1 Parts of each layer (mean / max over the 65)

| stdlib module | mean | max (workload) | workloads ≥ 1% |
|---|---:|---|---:|
| logging | 1.93 | 63.2 (web/logging_service) | 6 |
| json.encoder | 1.72 | 21.2 (data/csv_etl) | 16 |
| sqlite3 | 1.63 | 43.6 (web/sqlalchemy_orm) | 7 |
| re | 1.53 | 19.9 (data/text_corpus) | 17 |
| datetime | 1.40 | 38.3 (data/tz_events) | 11 |
| copy | 1.29 | 28.5 (data/state_snapshot) | 12 |
| asyncio.selector_events | 1.07 | 29.1 (web/asyncio_http) | 5 |
| gzip | 0.96 | 62.3 (data/backup_archive) | 1 |
| xml.etree | 0.87 | 49.7 (data/xml_feeds) | 2 |
| functools | 0.80 | 28.3 (custom/event_ledger) | 6 |
| _strptime | 0.79 | 21.5 (data/log_analytics) | 4 |
| urllib.parse | 0.71 | 8.5 (threads/request_pool) | 11 |
| decimal | 0.66 | 12.0 (data/billing_engine) | 8 |
| inspect | 0.65 | 17.2 (custom/di_plugins) | 8 |
| json.decoder | 0.63 | 9.9 (web/json_api) | 7 |
| zipfile | 0.62 | 32.1 (data/backup_archive) | 2 |
| email._header_value_parser | 0.59 | 38.1 (web/email_mime) | 1 |
| textwrap | 0.56 | 34.9 (data/report_render) | 1 |
| glob | 0.55 | 35.9 (data/fs_inventory) | 1 |
| ipaddress | 0.53 | 17.3 (threads/pipeline_queue) | 2 |
| ast | 0.53 | 17.8 (devtools/pyflakes_stdlib) | 4 |
| random | 0.45 | 24.7 (data/report_render) | 2 |
| http.client | 0.44 | 18.3 (threads/http_server) | 2 |
| pickle | 0.43 | 23.2 (data/state_snapshot) | 2 |
| socket | 0.43 | 11.7 (threads/http_server) | 3 |

| distribution | mean | max (workload) | workloads ≥ 1% |
|---|---:|---|---:|
| PyYAML | 1.97 | 72.3 (libs/yaml_config) | 2 |
| Django | 1.65 | 41.6 (frameworks/django_admin) | 3 |
| SQLAlchemy | 1.20 | 31.5 (frameworks/flask_marshmallow) | 3 |
| pydantic_core | 1.12 | 64.0 (libs/pydantic_payloads) | 4 |
| pandas | 0.84 | 54.4 (data/pandas_etl) | 1 |
| pyflakes | 0.80 | 26.4 (devtools/coverage_pyflakes_tests) | 2 |
| sqlglot | 0.77 | 50.0 (devtools/sqlglot_transpile) | 1 |
| pycodestyle | 0.74 | 48.2 (devtools/pycodestyle_stdlib) | 1 |
| isort | 0.66 | 43.2 (devtools/isort_stdlib) | 1 |
| rich | 0.62 | 40.3 (devtools/rich_render) | 1 |
| black | 0.59 | 38.2 (devtools/black_format) | 1 |
| graphql-core | 0.57 | 36.8 (frameworks/strawberry_graphql) | 1 |
| jsonschema | 0.55 | 35.7 (libs/jsonschema_validate) | 1 |
| aiohttp | 0.47 | 30.4 (web/aiohttp_app) | 1 |
| Werkzeug | 0.45 | 15.5 (web/flask_jinja) | 3 |
| pip | 0.41 | 26.8 (devtools/pip_resolve) | 1 |
| beautifulsoup4 | 0.37 | 24.0 (libs/bs4_scrape) | 1 |
| botocore | 0.36 | 23.4 (libs/boto3_client) | 1 |
| astroid | 0.35 | 22.8 (devtools/pylint_stdlib) | 1 |
| fastapi | 0.33 | 10.6 (web/fastapi_pydantic) | 3 |

| runtime part | mean | max (workload) | workloads ≥ 1% |
|---|---:|---|---:|
| dealloc | 7.39 | 13.3 (data/state_snapshot) | 64 |
| gc | 6.06 | 49.4 (devtools/pytest_networkx) | 35 |
| alloc | 3.80 | 8.7 (devtools/sphinx_autodoc) | 64 |
| import | 1.14 | 36.1 (devtools/import_startup) | 11 |
| locks | 0.67 | 8.4 (services/waitress_flask) | 11 |
| ft | 0.46 | 7.2 (services/job_worker) | 7 |
| startup | 0.23 | 8.8 (devtools/import_startup) | 2 |

| language category | mean | max (workload) | workloads ≥ 1% |
|---|---:|---|---:|
| attr | 3.35 | 12.6 (frameworks/drf_api) | 45 |
| dict | 3.03 | 9.2 (custom/task_planner) | 52 |
| str | 1.85 | 13.7 (data/text_corpus) | 40 |
| calls | 1.46 | 4.0 (custom/di_plugins) | 42 |
| list/tuple | 1.17 | 4.4 (custom/ecs_arena) | 31 |
| builtins | 0.62 | 7.4 (threads/compute_pool) | 7 |
| compare | 0.32 | 2.0 (threads/pipeline_queue) | 4 |
| abstract | 0.31 | 2.9 (data/billing_engine) | 6 |
| set | 0.25 | 5.2 (custom/task_planner) | 3 |
| generators | 0.21 | 1.2 (custom/ecs_arena) | 2 |
| int | 0.21 | 4.8 (threads/compute_pool) | 1 |
| format | 0.20 | 2.6 (data/tz_events) | 1 |
| iter | 0.20 | 5.0 (threads/compute_pool) | 1 |
| exceptions | 0.18 | 1.8 (web/flask_jinja) | 1 |

| generator | mean | max (workload) | workloads ≥ 1% |
|---|---:|---|---:|
| dataclasses | 1.08 | 16.3 (custom/order_domain) | 11 |
| enum | 0.35 | 4.6 (custom/rules_engine) | 7 |
| attrs | 0.27 | 10.7 (libs/attrs_cattrs) | 2 |
| namedtuple | 0.20 | 2.4 (frameworks/strawberry_graphql) | 4 |
| cattrs | 0.15 | 9.5 (libs/attrs_cattrs) | 1 |
| total_ordering | 0.02 | 1.4 (custom/fulfilment_sim) | 1 |
| exec:gettext | 0.00 | 0.0 (frameworks/drf_api) | 0 |

Largest stdlib functions (innermost frame, self + native callees; a workload name means a C
stdlib module called directly from that workload's code):

| module | function (innermost frame, self + native callees) | mean % |
|---|---|---:|
| stdlib:json.encoder | `json/encoder.py:JSONEncoder.iterencode` | 1.54 |
| generated:dataclasses | `<string>:__create_fn__.<locals>.__init__` | 0.84 |
| stdlib:_strptime | `_strptime.py:_strptime` | 0.66 |
| stdlib:sqlite3 | `sqlalchemy/engine/default.py:DefaultDialect.do_execute` | 0.64 |
| stdlib:copy | `copy.py:deepcopy` | 0.62 |
| stdlib:gzip | `gzip.py:GzipFile._write_raw` | 0.60 |
| stdlib:datetime | `tz_events.py:run` | 0.59 |
| stdlib:json.decoder | `json/decoder.py:JSONDecoder.raw_decode` | 0.56 |
| stdlib:asyncio.selector_events | `asyncio/selector_events.py:_SelectorSocketTransport.write` | 0.56 |
| stdlib:ast | `ast.py:parse` | 0.52 |
| stdlib:zipfile | `zipfile/__init__.py:_ZipWriteFile.write` | 0.42 |
| stdlib:logging | `logging/__init__.py:StreamHandler.flush` | 0.39 |
| stdlib:glob | `glob.py:_iterdir` | 0.36 |
| stdlib:textwrap | `textwrap.py:TextWrapper._split` | 0.36 |
| stdlib:pickle | `state_snapshot.py:run` | 0.36 |
| stdlib:sqlite3 | `orm_mapper.py:Session.execute` | 0.34 |
| stdlib:gzip | `gzip.py:compress` | 0.32 |
| stdlib:logging | `logging/__init__.py:LogRecord.__init__` | 0.30 |
| stdlib:re | `text_corpus.py:run` | 0.29 |
| stdlib:dataclasses | `dataclasses.py:_asdict_inner` | 0.29 |
| stdlib:csv | `csv.py:DictReader.__next__` | 0.28 |
| stdlib:re | `log_analytics.py:run` | 0.28 |
| stdlib:sqlite3 | `django/db/backends/sqlite3/base.py:SQLiteCursorWrapper.execute` | 0.28 |
| stdlib:collections | `collections/__init__.py:Counter.update` | 0.26 |
| stdlib:logging | `logging/__init__.py:Formatter.formatTime` | 0.24 |

## 4. Inside the interpreter: per opcode handler (M)

Interpreter time by opcode family and owner (GC that ran inside a handler's eval-breaker
check is excluded). *inline* = in the handler or the eval loop; *callee* = native code the
handler called:

| family | own inline | own callee | 3rd-party inline | 3rd-party callee | stdlib inline | stdlib callee | generated | all | specialized | generic |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| attribute | 1.8 | 0.6 | 3.5 | 1.9 | 1.6 | 0.8 | 0.3 | 10.3 | 6.0 | 4.4 |
| call | 2.0 | 9.8 | 3.3 | 9.8 | 2.2 | 14.4 | 1.2 | 42.7 | 39.1 | 3.6 |
| frame/return | 1.0 | 0.8 | 2.0 | 1.3 | 1.1 | 0.6 | 0.2 | 7.0 | 0.1 | 4.9 |
| binary/compare | 1.7 | 2.2 | 2.1 | 1.1 | 1.2 | 1.0 | 0.2 | 9.5 | 5.6 | 3.9 |
| subscript-store | 0.1 | 0.4 | 0.1 | 0.2 | 0.0 | 0.2 | 0.0 | 1.0 | 0.6 | 0.5 |
| globals/closures | 0.6 | 0.0 | 1.5 | 0.0 | 1.1 | 0.0 | 0.2 | 3.4 | 2.6 | 0.8 |
| locals/stack | 1.3 | 0.8 | 2.3 | 0.3 | 1.3 | 0.2 | 0.2 | 6.4 | 0.0 | 6.4 |
| control | 0.2 | 0.0 | 0.5 | 0.0 | 0.2 | 0.0 | 0.0 | 0.9 | 0.2 | 0.7 |
| iteration | 0.4 | 0.3 | 0.6 | 0.7 | 0.3 | 0.6 | 0.0 | 3.0 | 1.0 | 2.0 |
| build/unpack | 0.5 | 0.2 | 0.6 | 0.3 | 0.4 | 0.2 | 0.1 | 2.3 | 0.5 | 1.8 |
| format | 0.0 | 0.4 | 0.0 | 0.2 | 0.0 | 0.3 | 0.0 | 0.9 | 0.0 | 0.9 |
| function/class | 0.1 | 0.0 | 0.1 | 0.1 | 0.1 | 0.0 | 0.0 | 0.4 | 0.0 | 0.4 |
| exception/with | 0.1 | 0.0 | 0.1 | 0.1 | 0.1 | 0.1 | 0.0 | 0.6 | 0.0 | 0.6 |
| import | 0.0 | 0.0 | 0.0 | 0.1 | 0.0 | 0.0 | 0.0 | 0.2 | 0.0 | 0.2 |
| **total** | 9.9 | 15.6 | 16.8 | 16.3 | 9.5 | 18.4 | 2.3 | 88.7 | 55.7 | 31.1 |

How much generic-handler time comes from instructions that are still unspecialized vs
specialized-but-deopting, judged by the executing instruction's adaptive form at the end of the
run. Only the specializing families are meaningful: attribute, call, binary/compare,
subscript, globals, iteration. The other families have no specializations.

| family | unspecialized | deopt | ? |
|---|---:|---:|---:|
| attribute | 2.86 | 1.36 | 0.16 |
| call | 0.14 | 3.27 | 0.22 |
| frame/return | 1.17 | 0.01 | 3.71 |
| binary/compare | 3.61 | 0.07 | 0.26 |
| subscript-store | 0.46 | 0.00 | 0.02 |
| globals/closures | 0.68 | 0.00 | 0.09 |
| locals/stack | 5.03 | 0.00 | 1.38 |
| control | 0.51 | 0.00 | 0.20 |
| iteration | 1.89 | 0.01 | 0.12 |
| build/unpack | 1.66 | 0.00 | 0.15 |
| format | 0.86 | 0.00 | 0.02 |
| function/class | 0.37 | 0.00 | 0.03 |
| exception/with | 0.30 | 0.00 | 0.28 |
| import | 0.16 | 0.00 | 0.02 |

Top 40 handlers, all 65 workloads. kind: s = specialized variant, g = generic base of a
specializing family, - = opcode without specializations. Per-family tables (25 handlers + 15
pairs each) are in `~/rustique-research/w2/attribution/rollup/handlers-65.md`.

| handler | kind | mean % (incl. callees) | inline | callees | top callees (mean %) |
|---|:-:|---:|---:|---:|---|
| CALL_NON_PY_GENERAL | s | 10.50 | 1.10 | 9.40 | `encoder_call` 1.51, `_sre_SRE_Pattern_prefixmatch` 1.06, `zlib_Compress_compress` 1.01 |
| CALL_KW_NON_PY | s | 5.22 | 0.09 | 5.12 | `?` 1.16, `CPyPy_api___sort_code_string` 0.77, `builtin_compile` 0.66 |
| RETURN_VALUE | - | 4.46 | 2.58 | 1.88 | `subtype_dealloc` 0.56, `list_dealloc` 0.32, `dict_dealloc` 0.30 |
| LOAD_ATTR | g | 3.66 | 1.09 | 2.57 | `PyObject_GenericGetAttr` 1.63, `_PyObject_GetMethodStackRef` 0.59, `_Py_slot_tp_getattr_hook` 0.17 |
| CALL | g | 3.46 | 0.31 | 3.15 | `datetime_date_strftime` 0.97, `_socket_socket_send` 0.69, `_socket_socket_sendall` 0.42 |
| CALL_METHOD_DESCRIPTOR_FAST_WITH_KEYWORDS | s | 2.95 | 0.15 | 2.80 | `_sre_SRE_Pattern_split` 0.53, `unicode_split` 0.33, `_sre_SRE_Pattern_findall` 0.25 |
| CALL_BUILTIN_FAST | s | 2.75 | 0.35 | 2.40 | `builtin_getattr` 0.85, `builtin_next` 0.29, `builtin_hasattr` 0.27 |
| CALL_METHOD_DESCRIPTOR_FAST | s | 2.43 | 0.49 | 1.94 | `_pysqlite_query_execute` 0.65, `pysqlite_connection_execute` 0.37, `dict_get` 0.33 |
| CALL_BUILTIN_FAST_WITH_KEYWORDS | s | 2.36 | 0.26 | 2.10 | `builtin_sorted` 0.29, `marshal_loads` 0.21, `builtin_min` 0.20 |
| (eval) | - | 2.17 | 0.85 | 1.32 | `subtype_dealloc` 0.12, `as_ucs4` 0.10, `func_dealloc` 0.10 |
| STORE_FAST | - | 2.07 | 1.28 | 0.79 | `subtype_dealloc` 0.32, `list_dealloc` 0.16, `dict_dealloc` 0.15 |
| CALL_METHOD_DESCRIPTOR_NOARGS | s | 1.90 | 0.22 | 1.68 | `_io_TextIOWrapper_flush` 0.39, `pysqlite_cursor_fetchall` 0.31, `ascii_upper_or_lower` 0.23 |
| CALL_PY_EXACT_ARGS | s | 1.76 | 1.76 | 0.00 |  |
| BINARY_OP | g | 1.73 | 0.20 | 1.54 | `PyNumber_Remainder` 0.37, `PyNumber_Multiply` 0.25, `PyNumber_Add` 0.19 |
| CALL_ISINSTANCE | s | 1.64 | 0.57 | 1.07 | `object_recursive_isinstance` 1.04, `tuple_dealloc` 0.02, `PyObject_GetOptionalAttr` 0.01 |
| CALL_METHOD_DESCRIPTOR_O | s | 1.58 | 0.17 | 1.40 | `_elementtree_XMLParser_feed` 0.48, `unicode_join` 0.34, `_PyUnicode_TranslateCharmap` 0.18 |
| FOR_ITER | g | 1.53 | 0.27 | 1.27 | `ScandirIterator_iternext` 0.29, `calliter_iternext` 0.23, `enum_next` 0.11 |
| CALL_BUILTIN_CLASS | s | 1.51 | 0.19 | 1.32 | `list_vectorcall` 0.37, `type_new` 0.19, `dict_vectorcall` 0.12 |
| LOAD_GLOBAL_BUILTIN | s | 1.42 | 1.42 | 0.00 |  |
| BINARY_OP_SUBSCR_DICT | s | 1.37 | 0.36 | 1.01 | `_PyDict_Subscript` 0.92, `?` 0.05, `_Py_Dealloc` 0.01 |
| CALL_EX_NON_PY_GENERAL | s | 1.34 | 0.13 | 1.21 | `slot_tp_init` 0.19, `_Py_Dealloc` 0.18, `context_run` 0.11 |
| LOAD_ATTR_INSTANCE_VALUE | s | 1.24 | 1.24 | 0.00 | `subtype_dealloc` 0.00 |
| LOAD_GLOBAL_MODULE | s | 1.21 | 1.21 | 0.00 |  |
| CALL_BUILTIN_O | s | 1.01 | 0.22 | 0.80 | `heappop_internal` 0.27, `object___reduce_ex__` 0.14, `builtin_repr` 0.08 |
| LOAD_FAST_BORROW | - | 1.01 | 1.01 | 0.00 |  |
| LOAD_ATTR_SLOT | s | 0.96 | 0.96 | 0.01 | `structseq_dealloc` 0.00, `subtype_dealloc` 0.00, `cursor_dealloc` 0.00 |
| CONTAINS_OP | g | 0.79 | 0.28 | 0.51 | `list_contains` 0.13, `PyUnicode_Contains` 0.11, `tuple_contains` 0.10 |
| LOAD_ATTR_METHOD_NO_DICT | s | 0.69 | 0.69 | 0.00 |  |
| COMPARE_OP | g | 0.68 | 0.24 | 0.44 | `do_richcompare` 0.39, `tuple_dealloc` 0.02, `_Py_EnterRecursiveCallTstate` 0.01 |
| STORE_ATTR | g | 0.67 | 0.21 | 0.47 | `PyObject_GenericSetAttr` 0.39, `type_setattro` 0.06, `local_setattro` 0.01 |
| LOAD_ATTR_METHOD_WITH_VALUES | s | 0.67 | 0.67 | 0.00 |  |
| COMPARE_OP_INT | s | 0.57 | 0.57 | 0.00 |  |
| LOAD_ATTR_MODULE | s | 0.57 | 0.57 | 0.00 |  |
| CALL_ALLOC_AND_ENTER_INIT | s | 0.55 | 0.48 | 0.08 | `_PyObject_Malloc` 0.08, `_PyObject_GetMethodStackRef` 0.00, `object_recursive_isinstance` 0.00 |
| TO_BOOL_BOOL | s | 0.54 | 0.54 | 0.00 |  |
| POP_ITER | - | 0.54 | 0.11 | 0.43 | `gen_dealloc` 0.18, `list_dealloc` 0.08, `dictiter_dealloc` 0.04 |
| STORE_SUBSCR_DICT | s | 0.54 | 0.14 | 0.40 | `_PyDict_SetItem_Take2` 0.38, `?` 0.01, `_PyObject_Malloc` 0.00 |
| CALL_KW_PY | s | 0.51 | 0.40 | 0.12 | `PyDict_GetItemRef` 0.05, `PyDict_SetItem` 0.03, `PyObject_RichCompareBool` 0.02 |
| CALL_PY_GENERAL | s | 0.51 | 0.43 | 0.08 | `_PyTuple_FromStackRefStealOnSuccess` 0.04, `PyDict_GetItemRef` 0.04, `_PyObject_Malloc` 0.00 |
| STORE_ATTR_INSTANCE_VALUE | s | 0.51 | 0.48 | 0.03 | `subtype_dealloc` 0.01, `list_dealloc` 0.01, `func_dealloc` 0.00 |

Top 30 (handler → C entry point) pairs (callee time only):

| # | handler | callee (C entry point) | callee layer | mean % |
|---:|---|---|---|---:|
| 1 | LOAD_ATTR | `PyObject_GenericGetAttr` | language:attr | 1.63 |
| 2 | CALL_NON_PY_GENERAL | `encoder_call` | stdlib:json.encoder | 1.51 |
| 3 | CALL_KW_NON_PY | `?` | thirdparty:pydantic_core | 1.16 |
| 4 | CALL_NON_PY_GENERAL | `_sre_SRE_Pattern_prefixmatch` | stdlib:re | 1.06 |
| 5 | CALL_ISINSTANCE | `object_recursive_isinstance` | language:attr | 1.04 |
| 6 | CALL_NON_PY_GENERAL | `zlib_Compress_compress` | stdlib:gzip | 1.01 |
| 7 | CALL | `datetime_date_strftime` | stdlib:datetime | 0.97 |
| 8 | BINARY_OP_SUBSCR_DICT | `_PyDict_Subscript` | language:dict | 0.92 |
| 9 | CALL_BUILTIN_FAST | `builtin_getattr` | language:attr | 0.85 |
| 10 | CALL_KW_NON_PY | `CPyPy_api___sort_code_string` | thirdparty:isort | 0.77 |
| 11 | CALL | `_socket_socket_send` | stdlib:asyncio.selector_events | 0.69 |
| 12 | CALL_KW_NON_PY | `builtin_compile` | stdlib:ast | 0.66 |
| 13 | CALL_METHOD_DESCRIPTOR_FAST | `_pysqlite_query_execute` | stdlib:sqlite3 | 0.65 |
| 14 | CALL_NON_PY_GENERAL | `scanner_call` | stdlib:json.decoder | 0.64 |
| 15 | LOAD_ATTR | `_PyObject_GetMethodStackRef` | language:attr | 0.59 |
| 16 | RETURN_VALUE | `subtype_dealloc` | runtime:dealloc | 0.56 |
| 17 | CALL_METHOD_DESCRIPTOR_FAST_WITH_KEYWORDS | `_sre_SRE_Pattern_split` | stdlib:textwrap | 0.53 |
| 18 | CALL_METHOD_DESCRIPTOR_O | `_elementtree_XMLParser_feed` | stdlib:xml.etree | 0.48 |
| 19 | CALL | `_socket_socket_sendall` | stdlib:http.client | 0.42 |
| 20 | CALL_NON_PY_GENERAL | `_sre_SRE_Pattern_search` | stdlib:re | 0.41 |
| 21 | CALL_KW_NON_PY | `_pickle_dumps` | stdlib:pickle | 0.41 |
| 22 | CALL_METHOD_DESCRIPTOR_NOARGS | `_io_TextIOWrapper_flush` | stdlib:logging | 0.39 |
| 23 | COMPARE_OP | `do_richcompare` | stdlib:decimal | 0.39 |
| 24 | STORE_ATTR | `PyObject_GenericSetAttr` | language:attr | 0.39 |
| 25 | STORE_SUBSCR_DICT | `_PyDict_SetItem_Take2` | language:dict | 0.38 |
| 26 | CALL_BUILTIN_CLASS | `list_vectorcall` | language:list/tuple | 0.37 |
| 27 | BINARY_OP | `PyNumber_Remainder` | stdlib:logging | 0.37 |
| 28 | CALL_METHOD_DESCRIPTOR_FAST | `pysqlite_connection_execute` | stdlib:sqlite3 | 0.37 |
| 29 | CALL_NON_PY_GENERAL | `sock_recv` | stdlib:asyncio.selector_events | 0.35 |
| 30 | CALL_METHOD_DESCRIPTOR_O | `unicode_join` | language:str | 0.34 |

Reading these for a handler-swap mechanism (w2-interp):
- **CALL_*_NON_PY / METHOD_DESCRIPTOR rows.** Their callee time is the C function's own work (zlib, `_json`, `_sre`, sqlite). Swapping the handler reaches only its inline part (1.1% for CALL_NON_PY_GENERAL) and the vectorcall overhead, unless the new handler also implements the callee.
- **Generic paths that end in a type slot or a C entry point.** These are reachable by an entry-point mechanism:
  - LOAD_ATTR → `PyObject_GenericGetAttr` (1.6%), `_PyObject_GetMethodStackRef` (0.6%), `slot_tp_getattr_hook` (0.2%);
  - CALL_ISINSTANCE → `object_recursive_isinstance` (1.1%);
  - CALL_BUILTIN_FAST → `builtin_getattr`/`builtin_hasattr` (1.1%);
  - BINARY_OP_SUBSCR_DICT → `_PyDict_Subscript` (0.9%);
  - STORE_ATTR → `PyObject_GenericSetAttr` (0.4%);
  - COMPARE_OP → `do_richcompare` (0.4%).
  
  Of the generic attribute time, 2.9% runs unspecialized instructions and 1.4% deopts.
- **Pure-inline specialized handlers are the interpreter itself:** CALL_PY_EXACT_ARGS 1.8%, LOAD_GLOBAL_* 2.6%, LOAD_ATTR_* specialized 5.0%, RETURN_VALUE 2.6% inline. No entry point reaches them; only a different execution engine can.

## 5. Third-party code and the CPython entry points it calls (M)

In the 35 workloads where third-party frames are innermost for more than 5% of cycles, they
are innermost for **71.2%** of cycles on average. Of that:
- the third-party code's own bytecode and extensions: 36.6%;
- CPython-provided code reached from it: 34.6%, made up of language operations 14.1%, runtime 13.4% (allocation, deallocation, GC) and C stdlib modules 5.5%;
- unattributed: 1.6%.

Python stdlib functions it calls appear as stdlib owners; they are in the entry table below.

The per-distribution table is a mean over all 65 workloads, most of which don't use a given
distribution; per-workload values are in §3.
- *exclusive*: its bytecode and its extensions, i.e. the thirdparty layer;
- *inclusive*: everything with one of its frames innermost.

| distribution | exclusive | inclusive | of which language | stdlib | runtime |
|---|---:|---:|---:|---:|---:|
| Django | 1.65 | 3.71 | 0.98 | 0.40 | 0.56 |
| SQLAlchemy | 1.20 | 3.14 | 0.65 | 0.84 | 0.34 |
| PyYAML | 1.97 | 2.72 | 0.52 | 0.02 | 0.18 |
| pyflakes | 0.80 | 1.62 | 0.38 | 0.00 | 0.41 |
| pycodestyle | 0.74 | 1.42 | 0.25 | 0.26 | 0.16 |
| sqlglot | 0.77 | 1.41 | 0.37 | 0.01 | 0.23 |
| pydantic | 0.06 | 1.36 | 0.04 | 0.00 | 0.12 |
| pandas | 0.84 | 1.30 | 0.08 | 0.01 | 0.09 |
| rich | 0.62 | 1.24 | 0.27 | 0.11 | 0.20 |
| black | 0.59 | 1.23 | 0.20 | 0.02 | 0.39 |
| astroid | 0.35 | 1.21 | 0.17 | 0.00 | 0.66 |
| jsonschema | 0.55 | 1.09 | 0.21 | 0.02 | 0.19 |
| graphql-core | 0.57 | 1.02 | 0.23 | 0.02 | 0.19 |
| pip | 0.41 | 0.94 | 0.24 | 0.05 | 0.21 |
| botocore | 0.36 | 0.86 | 0.24 | 0.14 | 0.10 |
| aiohttp | 0.47 | 0.80 | 0.12 | 0.03 | 0.12 |
| Werkzeug | 0.45 | 0.79 | 0.18 | 0.04 | 0.08 |
| beautifulsoup4 | 0.37 | 0.78 | 0.16 | 0.01 | 0.23 |
| networkx | 0.07 | 0.74 | 0.14 | 0.00 | 0.52 |
| Pygments | 0.25 | 0.71 | 0.03 | 0.18 | 0.24 |

What third-party code calls. The innermost third-party frame and the CPython-provided code
entered directly from it come from exact Python stacks (bpf mode, 6 caller frames). Python
callees are inclusive of everything below them.

| kind | mean % |
|---|---:|
| thirdparty bytecode (handlers inline) | 16.50 |
| native: language | 7.42 |
| Python function: stdlib | 5.78 |
| native: runtime | 4.22 |
| cyclic GC (run from an eval-breaker check) | 2.97 |
| native: stdlib | 2.93 |
| native: thirdparty | 2.87 |
| Python frame in transition (unresolved) | 1.95 |
| native, no stack or no symbol | 0.97 |
| Python function: generated | 0.28 |
| native: generated | 0.01 |
| Python function: other | 0.01 |
| Python function: import | 0.00 |

| callee | mean % |
|---|---:|
| thirdparty:pydantic_core ← CALL_KW_NON_PY `?` | 1.05 |
| stdlib:ast:parse | 0.96 |
| thirdparty:pandas ←  `?` | 0.76 |
| language:attr ← LOAD_ATTR `PyObject_GenericGetAttr` | 0.73 |
| stdlib:sqlite3 ← CALL_METHOD_DESCRIPTOR_FAST `_pysqlite_query_execute` | 0.64 |
| language:attr ← LOAD_ATTR `_PyObject_GetMethodStackRef` | 0.34 |
| language:attr ← CALL_ISINSTANCE `object_recursive_isinstance` | 0.34 |
| stdlib:html:HTMLParser.feed | 0.32 |
| stdlib:wsgiref.validate:validator.<locals>.lint_app | 0.29 |
| language:attr ← CALL_BUILTIN_FAST `builtin_getattr` | 0.28 |
| language:builtins ← CALL_ISINSTANCE `object_recursive_isinstance` | 0.28 |
| stdlib:sqlite3 ← CALL `_pysqlite_query_execute` | 0.28 |
| stdlib:re ← CALL_NON_PY_GENERAL `_sre_SRE_Pattern_prefixmatch` | 0.27 |
| stdlib:json:dumps | 0.23 |
| runtime:dealloc ← RETURN_VALUE `subtype_dealloc` | 0.23 |
| stdlib:wsgiref.validate:validator.<locals>.lint_app.<locals>.start_response_wrapper | 0.21 |
| stdlib:re ← FOR_ITER `calliter_iternext` | 0.21 |
| thirdparty:numpy ←  `?` | 0.19 |
| stdlib:copy:deepcopy | 0.19 |
| stdlib:asyncio.streams:StreamWriter.writelines | 0.16 |
| language:attr ← STORE_ATTR `PyObject_GenericSetAttr` | 0.16 |
| runtime:alloc ← CALL_NON_PY_GENERAL `_sre_SRE_Pattern_prefixmatch` | 0.16 |
| generated:cattrs:structure_StockItem | 0.15 |
| stdlib:sqlite3 ← CALL_METHOD_DESCRIPTOR_NOARGS `pysqlite_cursor_fetchall` | 0.15 |
| stdlib:json:loads | 0.15 |
| stdlib:functools ← CALL_NON_PY_GENERAL `bounded_lru_cache_wrapper` | 0.15 |
| stdlib:inspect:getfullargspec | 0.14 |
| language:dict ← BINARY_OP_SUBSCR_DICT `_PyDict_Subscript` | 0.14 |
| stdlib:tokenize:_generate_tokens_from_c_tokenizer | 0.14 |
| language:dict ← STORE_SUBSCR_DICT `_PyDict_SetItem_Take2` | 0.13 |

The same for own code. "Python frame in transition" is mostly the interpreter's `__init__`
shim frame between a class call and its `__init__`, i.e. constructing instances of classes
with a Python `__init__`.

| kind | mean % |
|---|---:|
| Python function: stdlib | 12.07 |
| own bytecode (handlers inline) | 9.65 |
| native: language | 6.11 |
| Python frame in transition (unresolved) | 6.08 |
| native: stdlib | 4.81 |
| native: runtime | 3.87 |
| cyclic GC (run from an eval-breaker check) | 1.07 |
| native: thirdparty | 0.77 |
| native, no stack or no symbol | 0.44 |
| native: own | 0.20 |
| native: generated | 0.02 |
| Python function: unresolved | 0.01 |
| Python function: generated | 0.00 |

| callee | mean % |
|---|---:|
| stdlib:json:dumps | 1.99 |
| stdlib:json:loads | 0.70 |
| thirdparty:isort ← CALL_KW_NON_PY `CPyPy_api___sort_code_string` | 0.66 |
| stdlib:tarfile:TarFile.addfile | 0.58 |
| stdlib:datetime ← CALL `datetime_date_strftime` | 0.58 |
| stdlib:textwrap:fill | 0.55 |
| language:dict ← BINARY_OP_SUBSCR_DICT `_PyDict_Subscript` | 0.52 |
| stdlib:zipfile:ZipFile.writestr | 0.46 |
| stdlib:dataclasses:asdict | 0.40 |
| stdlib:asyncio.streams:StreamWriter.write | 0.37 |
| stdlib:sqlite3 ← CALL_METHOD_DESCRIPTOR_FAST `pysqlite_connection_execute` | 0.35 |
| stdlib:pickle ← CALL_KW_NON_PY `_pickle_dumps` | 0.35 |
| stdlib:asyncio.base_events:BaseEventLoop.run_until_complete | 0.35 |
| stdlib:gzip:compress | 0.32 |
| stdlib:heapq ← CALL_BUILTIN_O `heappop_internal` | 0.26 |
| runtime:dealloc ← STORE_FAST `subtype_dealloc` | 0.26 |
| stdlib:asyncio.runners:run | 0.25 |
| stdlib:xml.etree:XML | 0.24 |
| stdlib:http.client:HTTPConnection.getresponse | 0.24 |
| stdlib:http.client:HTTPConnection.request | 0.24 |
| language:attr ← LOAD_ATTR `PyObject_GenericGetAttr` | 0.24 |
| stdlib:collections:Counter.update | 0.23 |
| runtime:import ← CALL `datetime_date_strftime` | 0.22 |
| stdlib:random:Random.choices | 0.22 |
| stdlib:inspect:signature | 0.21 |
| stdlib:statistics:pstdev | 0.21 |
| stdlib:re ← CALL_METHOD_DESCRIPTOR_FAST_WITH_KEYWORDS `_sre_SRE_Pattern_findall` | 0.20 |
| stdlib:asyncio.streams:StreamReader.readline | 0.18 |
| stdlib:xml.etree:tostring | 0.18 |
| stdlib:re ← CALL_NON_PY_GENERAL `_sre_SRE_Pattern_search` | 0.17 |

Language-layer entry points (all owners):

| category | C entry point (called by the handler) | opcode | mean % |
|---|---|---|---:|
| attr | `PyObject_GenericGetAttr` | LOAD_ATTR | 0.96 |
| dict | `_PyDict_Subscript` | BINARY_OP_SUBSCR_DICT | 0.66 |
| attr | `_PyObject_GetMethodStackRef` | LOAD_ATTR | 0.41 |
| attr | `builtin_getattr` | CALL_BUILTIN_FAST | 0.41 |
| attr | `object_recursive_isinstance` | CALL_ISINSTANCE | 0.38 |
| builtins | `object_recursive_isinstance` | CALL_ISINSTANCE | 0.33 |
| dict | `dict_get` | CALL_METHOD_DESCRIPTOR_FAST | 0.24 |
| dict | `_PyDict_SetItem_Take2` | STORE_SUBSCR_DICT | 0.21 |
| attr | `PyObject_GenericSetAttr` | STORE_ATTR | 0.20 |
| calls | `slot_mp_ass_subscript` | STORE_SUBSCR | 0.18 |
| str | `PyUnicode_Concat` | BINARY_OP_ADD_UNICODE | 0.18 |
| str | `_PyUnicode_JoinArray` | BUILD_STRING | 0.16 |
| dict | `_PyDict_FromItems` | CALL_KW_NON_PY | 0.16 |
| str | `unicode_join` | CALL_METHOD_DESCRIPTOR_O | 0.15 |
| list/tuple | `builtin_sorted` | CALL_BUILTIN_FAST_WITH_KEYWORDS | 0.15 |
| str | `unicode_split` | CALL_METHOD_DESCRIPTOR_FAST_WITH_KEYWORDS | 0.15 |
| dict | `PyDict_Contains` | CONTAINS_OP_DICT | 0.14 |
| str | `ascii_upper_or_lower` | CALL_METHOD_DESCRIPTOR_NOARGS | 0.12 |
| builtins | `builtin_min` | CALL_BUILTIN_FAST_WITH_KEYWORDS | 0.12 |
| list/tuple | `list_sort` | CALL_KW_NON_PY | 0.11 |

## 6. Stdlib after wave 1: Rustique enabled (M, 3.15.1, `rustique.enable()`, all 15 groups)

Columns:
- CPU s/run comes from `getrusage` over the gated loop. Plain and enabled were separate processes on a loaded shared box, so **the ratio column is noisy (±20%) and is not a speed measurement**. Phase 3 measures speed.
- Shares are from each run's own samples, scaled to the plain run's CPU time.
- "Wave-1 modules" = json, copy, datetime/_strptime, posixpath/genericpath/pathlib/fnmatch/glob/os, logging, random, textwrap, urllib.parse, statistics, dataclasses (the modules wave 1 targets, whole module).

| workload | CPU s/run plain | enabled | ratio | wave-1 modules plain → enabled (% of plain time) | Rustique kernels | rest of stdlib | largest remaining wave-1 parts (enabled) |
|---|---:|---:|---:|---|---:|---:|---|
| custom/di_plugins | 0.971 | 1.047 | 1.079 | 0.0 → 0.0 | 0.2 | 28.3 → 30.3 |  |
| custom/ecs_arena | 1.169 | 1.222 | 1.045 | 0.1 → 0.0 | 1.5 | 0.9 → 0.8 |  |
| custom/event_ledger | 1.814 | 2.101 | 1.158 | 0.0 → 0.0 | 0.0 | 34.6 → 37.7 |  |
| custom/fulfilment_sim | 1.558 | 1.679 | 1.078 | 3.4 → 3.3 | 0.2 | 9.4 → 11.7 | random 3.3 |
| custom/mini_interp | 1.344 | 1.360 | 1.012 | 0.0 → 0.0 | 0.0 | 0.2 → 0.2 |  |
| custom/order_domain | 1.662 | 1.992 | 1.199 | 0.0 → 0.0 | 0.6 | 8.8 → 9.5 |  |
| custom/orm_mapper | 1.006 | 1.131 | 1.124 | 2.8 → 0.9 | 0.9 | 35.8 → 41.6 |  |
| custom/report_engine | 1.133 | 1.158 | 1.022 | 6.2 → 0.5 | 1.8 | 13.6 → 16.4 |  |
| custom/rules_engine | 1.114 | 1.140 | 1.023 | 0.1 → 0.1 | 0.2 | 22.6 → 23.8 |  |
| custom/task_planner | 0.598 | 0.612 | 1.023 | 0.0 → 0.0 | 2.4 | 17.0 → 18.2 |  |
| data/backup_archive | 0.741 | 0.745 | 1.005 | 0.2 → 0.0 | 0.1 | 99.1 → 99.7 |  |
| data/billing_engine | 1.276 | 1.345 | 1.054 | 6.0 → 4.9 | 1.7 | 16.4 → 17.6 | statistics 4.9 |
| data/config_merge | 0.690 | 0.709 | 1.028 | 3.1 → 2.7 | 0.2 | 12.7 → 13.7 | json.encoder 2.7 |
| data/csv_etl | 0.500 | 0.316 | 0.632 | 36.3 → 1.8 | 10.2 | 20.6 → 19.9 | datetime 1.8 |
| data/fs_inventory | 0.408 | 0.314 | 0.770 | 80.3 → 59.0 | 3.3 | 1.0 → 0.1 | glob 37.0, os 16.5, genericpath 4.2 |
| data/log_analytics | 0.951 | 0.589 | 0.619 | 31.9 → 2.1 | 4.6 | 38.9 → 36.6 | urllib.parse 2.1 |
| data/pandas_etl | 0.678 | 0.724 | 1.067 | 0.0 → 0.0 | 0.0 | 0.9 → 0.5 |  |
| data/report_render | 0.514 | 0.249 | 0.485 | 59.6 → 1.8 | 13.1 | 8.1 → 7.8 | random 1.8 |
| data/state_snapshot | 0.958 | 0.778 | 0.812 | 31.0 → 1.0 | 11.9 | 24.1 → 26.6 |  |
| data/text_corpus | 0.742 | 0.848 | 1.144 | 0.0 → 0.0 | 0.1 | 41.2 → 48.3 |  |
| data/tz_events | 0.659 | 0.414 | 0.629 | 43.4 → 17.1 | 7.3 | 1.3 → 0.4 | datetime 17.1 |
| data/xml_feeds | 0.741 | 0.774 | 1.045 | 0.0 → 0.0 | 0.1 | 49.8 → 51.6 |  |
| devtools/black_format | 1.761 | 1.891 | 1.074 | 0.0 → 0.0 | 0.2 | 3.1 → 3.5 |  |
| devtools/coverage_pyflakes_tests | 0.517 | 0.576 | 1.114 | 1.6 → 1.9 | 0.2 | 22.3 → 25.6 |  |
| devtools/isort_stdlib | 0.362 | 0.281 | 0.776 | 33.5 → 17.1 | 2.9 | 4.5 → 1.8 | dataclasses 16.3 |
| devtools/pip_resolve | 0.790 | 0.802 | 1.015 | 9.0 → 4.9 | 2.0 | 24.1 → 24.6 | urllib.parse 1.7, genericpath 1.1 |
| devtools/pycodestyle_stdlib | 1.221 | 1.267 | 1.038 | 0.0 → 0.0 | 1.3 | 22.1 → 22.6 |  |
| devtools/pyflakes_stdlib | 1.754 | 1.966 | 1.121 | 0.0 → 0.0 | 0.1 | 21.2 → 23.5 |  |
| devtools/pylint_stdlib | 1.677 | 1.740 | 1.037 | 0.7 → 0.6 | 0.1 | 7.8 → 8.3 |  |
| devtools/pytest_networkx | 1.178 | 1.179 | 1.001 | 5.4 → 2.4 | 0.5 | 5.3 → 5.1 |  |
| devtools/rich_render | 0.388 | 0.401 | 1.033 | 0.1 → 0.1 | 0.7 | 10.5 → 10.4 |  |
| devtools/sphinx_autodoc | 1.136 | 1.110 | 0.977 | 4.4 → 3.3 | 0.5 | 18.5 → 18.2 | logging 1.6, pathlib 1.0 |
| devtools/sqlglot_transpile | 1.128 | 1.063 | 0.942 | 3.6 → 0.6 | 1.1 | 1.3 → 1.1 |  |
| frameworks/django_admin | 1.493 | 1.674 | 1.121 | 4.7 → 3.4 | 1.7 | 10.0 → 10.8 | urllib.parse 1.7, copy 1.1 |
| frameworks/drf_api | 1.774 | 2.053 | 1.157 | 4.4 → 3.7 | 1.4 | 12.7 → 14.0 | copy 2.2 |
| frameworks/falcon_api | 0.882 | 0.878 | 0.996 | 8.3 → 0.0 | 3.8 | 42.1 → 45.9 |  |
| frameworks/fastapi_sqlalchemy | 1.974 | 2.172 | 1.101 | 1.1 → 0.5 | 0.6 | 21.5 → 24.9 |  |
| frameworks/flask_marshmallow | 1.008 | 1.212 | 1.203 | 3.9 → 3.6 | 0.8 | 12.4 → 14.9 | urllib.parse 1.6, json.encoder 1.5 |
| frameworks/strawberry_graphql | 1.253 | 1.433 | 1.144 | 0.9 → 1.0 | 0.1 | 6.7 → 7.4 |  |
| libs/attrs_cattrs | 1.050 | 1.007 | 0.960 | 16.3 → 0.3 | 9.4 | 3.2 → 2.6 |  |
| libs/boto3_client | 1.944 | 2.051 | 1.055 | 5.5 → 2.8 | 2.4 | 13.5 → 14.3 |  |
| libs/bs4_scrape | 1.483 | 1.767 | 1.191 | 0.0 → 0.0 | 1.0 | 17.0 → 19.3 |  |
| libs/jsonschema_validate | 1.728 | 2.030 | 1.175 | 0.4 → 0.1 | 0.4 | 4.0 → 5.4 |  |
| libs/pydantic_payloads | 0.869 | 1.014 | 1.167 | 2.2 → 1.7 | 0.7 | 3.2 → 3.4 | datetime 1.7 |
| libs/yaml_config | 1.235 | 1.326 | 1.074 | 0.0 → 0.0 | 0.1 | 1.1 → 1.2 |  |
| services/cache_service | 1.280 | 1.423 | 1.112 | 0.0 → 0.0 | 0.0 | 44.9 → 50.0 |  |
| services/job_worker | 1.036 | 0.718 | 0.693 | 36.0 → 14.1 | 6.9 | 15.4 → 10.1 | logging 9.5, json.encoder 3.2 |
| services/uvicorn_fastapi | 1.511 | 1.551 | 1.026 | 6.6 → 3.1 | 1.5 | 14.7 → 15.5 | logging 2.7 |
| services/waitress_flask | 1.273 | 1.284 | 1.009 | 9.2 → 7.3 | 1.4 | 21.3 → 24.5 | logging 4.4, json.encoder 1.5 |
| threads/compute_pool | 1.578 | 1.439 | 0.912 | 0.2 → 0.1 | 10.1 | 5.3 → 10.2 |  |
| threads/http_server | 0.991 | 1.131 | 1.141 | 7.7 → 1.6 | 3.6 | 62.7 → 64.9 | urllib.parse 1.5 |
| threads/pipeline_queue | 0.966 | 0.454 | 0.470 | 33.6 → 1.3 | 3.6 | 29.7 → 23.0 | urllib.parse 1.3 |
| threads/request_pool | 0.824 | 0.486 | 0.589 | 40.1 → 2.4 | 10.4 | 13.0 → 11.7 | urllib.parse 2.3 |
| threads/shared_cache | 0.756 | 0.472 | 0.624 | 53.6 → 22.5 | 5.6 | 6.3 → 5.1 | logging 21.0, datetime 1.6 |
| web/aiohttp_app | 0.355 | 0.355 | 1.002 | 7.6 → 0.0 | 4.6 | 21.3 → 23.4 |  |
| web/asyncio_http | 0.291 | 0.249 | 0.857 | 25.2 → 1.0 | 8.5 | 50.5 → 54.2 |  |
| web/django_blog | 0.632 | 0.720 | 1.140 | 3.6 → 3.0 | 0.7 | 14.0 → 16.5 | copy 1.5 |
| web/email_mime | 0.818 | 0.889 | 1.087 | 0.0 → 0.0 | 0.4 | 78.7 → 85.5 |  |
| web/fastapi_pydantic | 0.377 | 0.423 | 1.120 | 5.9 → 3.2 | 2.2 | 22.4 → 24.5 | urllib.parse 1.6 |
| web/flask_jinja | 0.517 | 0.552 | 1.069 | 6.4 → 4.9 | 1.6 | 8.9 → 10.0 | urllib.parse 3.2, json.encoder 1.2 |
| web/json_api | 0.493 | 0.447 | 0.906 | 38.7 → 24.2 | 7.1 | 11.8 → 13.2 | json.encoder 10.0, dataclasses 9.4, copy 2.1 |
| web/logging_service | 0.334 | 0.298 | 0.892 | 74.9 → 63.1 | 6.8 | 5.3 → 5.5 | logging 53.4, json.encoder 8.0, json 1.7 |
| web/requests_local | 0.353 | 0.344 | 0.976 | 15.8 → 9.0 | 4.2 | 42.0 → 40.9 | urllib.parse 5.5, os 1.6, genericpath 1.3 |
| web/sqlalchemy_orm | 0.475 | 0.617 | 1.298 | 0.1 → 0.0 | 0.2 | 45.1 → 49.4 |  |

| module | mean % |
|---|---:|
| logging | 1.45 |
| glob | 0.58 |
| json.encoder | 0.48 |
| datetime | 0.46 |
| dataclasses | 0.42 |
| urllib.parse | 0.39 |
| os | 0.33 |
| copy | 0.16 |
| genericpath | 0.12 |
| random | 0.09 |
| statistics | 0.09 |
| json | 0.08 |
| pathlib | 0.04 |
| posixpath | 0.04 |
| textwrap | 0.02 |

What remains, and why. Inferred from the remaining entry points in each profile.

| module | remaining | why |
|---|---|---|
| `logging` | 53% of `logging_service`, 21% of `shared_cache`, 2–10% of services | handler / `StreamHandler.emit` / `flush` / formatter paths outside the wave-1 kernels |
| `glob` + `os` | 37% + 17% of `fs_inventory` | `os.scandir` iteration and `glob._iterdir`, partly syscalls: the remaining path work is not pure Python |
| `dataclasses` | 16% of `isort` | `dataclasses.replace` / `__init__` paths beyond the kernels |
| `json.encoder` | 10% of `json_api`, 1.5–3% elsewhere | `dumps(default=...)` / `cls=` fall back (open item in DECISIONS §9) |
| `datetime` | 17% of `tz_events` | zoneinfo conversions, `astimezone`, arithmetic |

Rustique's own kernels take 0–13% of enabled runs; the largest are `report_render` 13%,
`state_snapshot` 12% and `csv_etl` 10%.

## 7. Runtime (M)

### 7.1 Steady state

- **Deallocation 7.4% mean.** RETURN_VALUE / STORE_FAST / POP_ITER callees: `subtype_dealloc`, `dict_dealloc`, `list_dealloc`, `gen_dealloc`.
- **Cyclic GC 6.1% mean**, 35 workloads ≥ 1%:
  - devtools: pytest 49%, pyflakes 30%, pylint 40%, black 22%;
  - data: billing 41%, xml_feeds 24%, state_snapshot 18%.
- **Allocation 3.8%.**
- **Import 1.1%.** In steady state this is mostly `datetime.strftime` calling `PyImport_ImportModuleAttr("time", …)` on every call: 8.2% of `tz_events`. Wave 1's datetime kernels remove it when enabled.

### 7.2 Whole process: startup, imports, setup, finalization

`--modes wholeplain` (phase sizes, no -X perf) and `whole` (owners). `_whole.py` runs
startup, imports the workload, then `setup()`, one `run()`, and exit. Phases come from
CLOCK_MONOTONIC marks.

Means over 65 workloads (% of whole-process cycles): startup 1.3, import 0.8, setup 14.1, run 77.7, finalize 6.0; runtime outside the first run: import 3.6, gc 5.7, alloc 1.0, dealloc 0.9, startup 0.1

| workload | process Gcycles | startup | import wl | setup | first run | finalize | runtime in startup+import+setup+finalize: import / gc / alloc+dealloc / startup |
|---|---:|---:|---:|---:|---:|---:|---|
| custom/di_plugins | 3.85 | 0.7 | 1.4 | 11.7 | 78.3 | 7.8 | 1.0 / 7.0 / 2.8 / 0.0 |
| custom/ecs_arena | 3.68 | 0.9 | 1.5 | 0.0 | 97.1 | 0.5 | 0.9 / 0.6 / 0.3 / 0.0 |
| custom/event_ledger | 6.69 | 0.4 | 1.2 | 4.7 | 92.6 | 1.1 | 0.6 / 0.8 / 1.1 / 0.0 |
| custom/fulfilment_sim | 4.87 | 0.6 | 0.3 | 0.0 | 98.8 | 0.3 | 0.3 / 0.2 / 0.1 / 0.0 |
| custom/mini_interp | 4.14 | 0.7 | 0.9 | 0.2 | 97.7 | 0.5 | 0.7 / 0.6 / 0.3 / 0.1 |
| custom/order_domain | 5.52 | 0.6 | 1.2 | 4.5 | 88.5 | 5.2 | 0.6 / 6.3 / 1.2 / 0.0 |
| custom/orm_mapper | 3.75 | 0.8 | 0.8 | 14.1 | 82.9 | 1.3 | 0.7 / 1.1 / 2.6 / 0.1 |
| custom/report_engine | 3.78 | 0.6 | 1.5 | 3.2 | 93.7 | 1.0 | 1.1 / 1.0 / 0.7 / 0.0 |
| custom/rules_engine | 4.17 | 0.7 | 1.3 | 11.6 | 81.5 | 4.9 | 0.8 / 3.9 / 4.0 / 0.0 |
| custom/task_planner | 2.08 | 1.4 | 1.4 | 3.7 | 92.5 | 1.0 | 1.6 / 1.1 / 0.7 / 0.1 |
| data/backup_archive | 2.65 | 1.1 | 0.1 | 10.3 | 87.4 | 1.0 | 0.6 / 0.7 / 1.4 / 0.1 |
| data/billing_engine | 4.97 | 0.6 | 0.9 | 13.5 | 78.4 | 6.6 | 0.7 / 6.9 / 3.8 / 0.0 |
| data/config_merge | 2.26 | 1.6 | 0.0 | 2.1 | 95.3 | 1.0 | 1.2 / 0.7 / 0.7 / 0.1 |
| data/csv_etl | 2.56 | 1.3 | 0.2 | 35.7 | 61.9 | 0.9 | 1.4 / 0.6 / 3.8 / 0.1 |
| data/fs_inventory | 3.55 | 0.8 | 0.1 | 41.0 | 35.2 | 22.9 | 1.1 / 0.6 / 1.5 / 0.1 |
| data/log_analytics | 3.84 | 0.8 | 0.1 | 18.4 | 79.9 | 0.8 | 1.2 / 0.7 / 3.2 / 0.0 |
| data/pandas_etl | 6.94 | 0.4 | 0.1 | 61.7 | 32.1 | 5.7 | 7.3 / 6.4 / 4.8 / 0.1 |
| data/report_render | 1.74 | 1.7 | 0.2 | 0.0 | 97.1 | 1.0 | 0.8 / 0.8 / 0.5 / 0.1 |
| data/state_snapshot | 4.38 | 0.7 | 0.6 | 15.1 | 76.2 | 7.5 | 0.5 / 6.9 / 4.2 / 0.1 |
| data/text_corpus | 3.46 | 1.0 | 0.1 | 27.9 | 70.4 | 0.6 | 0.6 / 0.5 / 2.9 / 0.1 |
| data/tz_events | 2.63 | 1.2 | 0.2 | 9.9 | 86.1 | 2.5 | 0.9 / 1.9 / 2.5 / 0.1 |
| data/xml_feeds | 2.90 | 1.1 | 0.1 | 24.4 | 72.9 | 1.4 | 2.6 / 1.2 / 4.4 / 0.1 |
| devtools/black_format | 6.27 | 0.5 | 0.2 | 5.8 | 83.8 | 9.7 | 2.4 / 8.4 / 0.9 / 0.0 |
| devtools/coverage_pyflakes_tests | 2.08 | 1.5 | 0.4 | 9.6 | 83.0 | 5.6 | 5.8 / 5.7 / 1.1 / 0.1 |
| devtools/import_startup | 0.11 | 26.9 | 30.0 | 0.0 | 31.3 | 11.8 | 31.3 / 14.1 / 4.8 / 1.6 |
| devtools/isort_stdlib | 1.44 | 2.1 | 0.8 | 13.9 | 80.1 | 3.1 | 7.7 / 4.5 / 1.2 / 0.2 |
| devtools/pip_resolve | 4.15 | 0.9 | 1.5 | 9.7 | 75.9 | 12.0 | 1.6 / 12.7 / 0.9 / 0.1 |
| devtools/pycodestyle_stdlib | 4.19 | 0.8 | 0.4 | 2.4 | 95.7 | 0.7 | 1.3 / 0.8 / 0.5 / 0.1 |
| devtools/pyflakes_stdlib | 7.01 | 0.5 | 0.2 | 0.7 | 98.0 | 0.6 | 0.6 / 0.4 / 0.3 / 0.0 |
| devtools/pylint_stdlib | 7.52 | 0.4 | 0.2 | 3.6 | 53.2 | 42.6 | 2.6 / 40.6 / 0.5 / 0.0 |
| devtools/pytest_networkx | 6.68 | 0.5 | 0.2 | 8.6 | 70.2 | 20.4 | 5.6 / 19.3 / 1.1 / 0.1 |
| devtools/rich_render | 1.72 | 1.8 | 0.7 | 14.9 | 77.0 | 5.6 | 8.9 / 5.2 / 1.8 / 0.1 |
| devtools/sphinx_autodoc | 7.15 | 0.4 | 0.6 | 5.6 | 74.9 | 18.5 | 3.8 / 15.7 / 0.9 / 0.1 |
| devtools/sqlglot_transpile | 4.81 | 0.8 | 0.2 | 6.0 | 85.9 | 7.1 | 3.2 / 4.7 / 0.7 / 0.1 |
| frameworks/django_admin | 7.09 | 0.4 | 0.1 | 22.7 | 68.0 | 8.9 | 6.6 / 11.0 / 2.1 / 0.1 |
| frameworks/drf_api | 7.35 | 0.4 | 0.1 | 15.0 | 77.4 | 7.1 | 6.3 / 7.5 / 1.5 / 0.1 |
| frameworks/falcon_api | 3.29 | 1.0 | 0.1 | 15.4 | 77.0 | 6.4 | 6.0 / 6.2 / 2.0 / 0.5 |
| frameworks/fastapi_sqlalchemy | 12.19 | 0.3 | 0.0 | 21.3 | 69.5 | 8.9 | 4.7 / 8.9 / 2.0 / 0.1 |
| frameworks/flask_marshmallow | 6.97 | 0.4 | 0.1 | 28.9 | 57.9 | 12.8 | 7.1 / 14.2 / 3.2 / 0.1 |
| frameworks/strawberry_graphql | 5.09 | 0.5 | 0.1 | 15.9 | 78.9 | 4.6 | 4.6 / 7.2 / 1.8 / 0.1 |
| libs/attrs_cattrs | 4.41 | 0.7 | 0.1 | 19.0 | 79.2 | 1.0 | 2.1 / 1.3 / 2.0 / 0.1 |
| libs/boto3_client | 7.72 | 0.4 | 0.1 | 10.8 | 81.6 | 7.2 | 3.2 / 7.6 / 1.3 / 0.0 |
| libs/bs4_scrape | 5.42 | 0.6 | 0.1 | 1.5 | 85.2 | 12.7 | 0.5 / 11.2 / 0.3 / 0.0 |
| libs/jsonschema_validate | 5.88 | 0.5 | 0.1 | 8.2 | 87.4 | 3.8 | 1.7 / 3.3 / 1.8 / 0.0 |
| libs/pydantic_payloads | 4.39 | 0.6 | 0.1 | 33.5 | 59.6 | 6.2 | 3.1 / 7.4 / 5.4 / 0.1 |
| libs/yaml_config | 4.73 | 0.7 | 0.1 | 16.2 | 82.7 | 0.4 | 0.7 / 0.3 / 1.0 / 0.0 |
| services/cache_service | 4.80 | 0.6 | 0.1 | 9.8 | 83.2 | 6.3 | 4.1 / 5.4 / 1.3 / 0.1 |
| services/job_worker | 3.85 | 0.7 | 0.1 | 13.0 | 84.4 | 1.7 | 2.2 / 1.9 / 1.8 / 0.4 |
| services/uvicorn_fastapi | 6.54 | 0.4 | 0.1 | 15.6 | 76.3 | 7.7 | 4.9 / 7.5 / 1.4 / 0.1 |
| services/waitress_flask | 4.95 | 0.7 | 0.1 | 8.7 | 86.2 | 4.4 | 4.0 / 3.6 / 0.7 / 0.1 |
| threads/compute_pool | 4.74 | 0.6 | 0.1 | 1.7 | 97.1 | 0.5 | 0.4 / 0.4 / 0.3 / 0.0 |
| threads/http_server | 4.86 | 0.6 | 0.1 | 2.8 | 95.6 | 0.9 | 1.6 / 1.0 / 0.4 / 0.0 |
| threads/pipeline_queue | 3.08 | 1.0 | 0.1 | 21.2 | 76.7 | 1.0 | 0.5 / 0.7 / 2.7 / 0.0 |
| threads/request_pool | 2.63 | 1.0 | 0.1 | 10.7 | 86.0 | 2.2 | 0.9 / 1.9 / 2.2 / 0.0 |
| threads/shared_cache | 2.57 | 1.1 | 0.1 | 11.3 | 85.3 | 2.1 | 0.6 / 1.8 / 2.4 / 0.0 |
| web/aiohttp_app | 1.86 | 1.7 | 0.2 | 22.1 | 67.9 | 8.1 | 12.3 / 10.0 / 2.0 / 0.2 |
| web/asyncio_http | 1.71 | 2.1 | 0.3 | 0.5 | 94.3 | 2.7 | 0.7 / 2.2 / 0.8 / 0.2 |
| web/django_blog | 3.84 | 0.8 | 0.1 | 25.2 | 62.9 | 11.1 | 11.0 / 10.8 / 2.3 / 0.1 |
| web/email_mime | 3.13 | 1.2 | 0.1 | 5.5 | 92.4 | 0.8 | 0.5 / 0.6 / 0.4 / 0.1 |
| web/fastapi_pydantic | 3.06 | 1.1 | 0.1 | 26.4 | 51.1 | 21.2 | 8.9 / 19.3 / 2.9 / 0.2 |
| web/flask_jinja | 2.64 | 1.1 | 0.1 | 16.0 | 73.3 | 9.4 | 9.1 / 7.2 / 1.6 / 0.1 |
| web/json_api | 2.55 | 1.1 | 0.2 | 33.4 | 64.2 | 1.1 | 1.2 / 0.6 / 3.7 / 0.2 |
| web/logging_service | 1.30 | 2.5 | 0.3 | 5.0 | 89.7 | 2.5 | 4.2 / 2.4 / 0.7 / 0.1 |
| web/requests_local | 1.57 | 1.9 | 0.2 | 20.6 | 70.0 | 7.3 | 11.9 / 6.3 / 1.5 / 0.2 |
| web/sqlalchemy_orm | 6.95 | 0.5 | 0.1 | 64.6 | 26.7 | 8.1 | 4.9 / 19.5 / 7.9 / 0.1 |

Findings:
- **Interpreter startup is 1–2% of these processes**, and 27% of the 0.11 G-cycle `import_startup` child. Importing frameworks happens in `setup()` and is up to 12% (aiohttp, requests, django).
- **Finalization is the surprise:**
  - The exit-time GC (`_PyGC_CollectNoFail` in `finalize_modules`) costs 40.6% of all `pylint_stdlib` cycles, 19% of `pytest_networkx`, `sqlalchemy_orm` and `fastapi_pydantic`, and 16% of `sphinx`.
  - For short CLI runs (linters, test runners) that is a large reachable pool.
  - It is behaviour-visible: finalizers and `__del__` order, so any fix is runtime policy, opt-in (for w2-runtime and w2-startup).

### 7.3 Free-threading cost: 3.15.1t vs 3.15.1, same workload

Δ in CPU ms per run, by layer and by component. CPU time includes all threads.

| workload | CPU s/run GIL | t | t/GIL | Δ by layer (ms/run): own / lang / stdlib / 3rd / runtime | largest growing components (ms/run) |
|---|---:|---:|---:|---|---|
| custom/di_plugins | 0.971 | 1.080 | 1.11 | +26 / +31 / +23 / +0 / +17 | lang:dict +28, interp:op +20, lang:attr +16, rt:alloc +15 |
| custom/ecs_arena | 1.169 | 1.300 | 1.11 | +24 / +92 / +0 / +0 / -7 | lang:attr +33, lang:list/tuple +26, rt:ft +24, lang:set +21 |
| custom/event_ledger | 1.814 | 2.338 | 1.29 | +301 / +97 / +107 / +0 / +9 | interp:op +291, lang:dict +79, lang:attr +54, interp:eval +30 |
| custom/fulfilment_sim | 1.558 | 1.830 | 1.18 | +104 / +87 / +52 / +0 / +24 | interp:op +98, lang:attr +48, lang:calls +26, rt:ft +21 |
| custom/mini_interp | 1.344 | 1.536 | 1.14 | +72 / +85 / +10 / +0 / +17 | lang:attr +104, interp:op +68, lang:dict +50, lang:hash +32 |
| custom/order_domain | 1.662 | 1.903 | 1.14 | +12 / +154 / +24 / +0 / -34 | lang:attr +136, lang:dict +41, rt:ft +38, lang:builtins +33 |
| custom/orm_mapper | 1.006 | 1.236 | 1.23 | +21 / +64 / +49 / +0 / +92 | lang:dict +54, rt:gc +35, lang:attr +31, rt:alloc +30 |
| custom/report_engine | 1.133 | 1.312 | 1.16 | +32 / +50 / +28 / +0 / +39 | lang:attr +61, rt:ft +36, lang:list/tuple +24, interp:op +23 |
| custom/rules_engine | 1.114 | 1.460 | 1.31 | +9 / +79 / +62 / +0 / +148 | rt:alloc +158, lang:attr +72, lang:generators +44, lang:dict +23 |
| custom/task_planner | 0.598 | 0.688 | 1.15 | +20 / +43 / +18 / +0 / +8 | interp:op +20, lang:set +18, lang:calls +10, cmod:heapq +10 |
| data/backup_archive | 0.741 | 0.718 | 0.97 | -0 / +0 / -26 / +0 / +3 | lang:bytes +6, rt:alloc +3, interp:op +1, cmod:hashlib +1 |
| data/billing_engine | 1.276 | 0.963 | 0.75 | +28 / +29 / +17 / +0 / -390 | rt:ft +60, lang:attr +40, interp:op +26, lang:builtins +23 |
| data/config_merge | 0.690 | 0.752 | 1.09 | +4 / +9 / +17 / -0 / +28 | rt:gc +14, rt:alloc +13, lang:list/tuple +9, lang:dict +9 |
| data/csv_etl | 0.500 | 0.531 | 1.06 | +10 / +18 / +1 / +0 / +1 | lang:dict +11, interp:eval +7, cmod:csv +6, rt:alloc +6 |
| data/fs_inventory | 0.408 | 0.391 | 0.96 | +4 / +1 / -19 / +0 / -2 | rt:ft +7, rt:gc +3, lang:generators +2, cmod:functools +2 |
| data/log_analytics | 0.951 | 0.955 | 1.00 | +6 / -0 / -3 / +0 / -3 | lang:dict +37, rt:alloc +12, rt:ft +11, interp:eval +4 |
| data/pandas_etl | 0.678 | 0.744 | 1.10 | +2 / -4 / +6 / +40 / +16 | ext:pandas +47, libc +28, ext:numpy +11, rt:ft +8 |
| data/report_render | 0.514 | 0.572 | 1.11 | +1 / +7 / +40 / +0 / +8 | cmod:random +20, lang:list/tuple +12, rt:gc +9, rt:ft +7 |
| data/state_snapshot | 0.958 | 1.667 | 1.74 | -0 / -2 / +168 / +0 / +522 | rt:ft +447, rt:alloc +131, interp:op +47, lang:dict +45 |
| data/text_corpus | 0.742 | 0.785 | 1.06 | -10 / +20 / +19 / +0 / +11 | lang:dict +27, lang:generators +19, cmod:re +10, rt:dealloc +7 |
| data/tz_events | 0.659 | 0.747 | 1.13 | +8 / -1 / +5 / +0 / +72 | lang:dict +34, rt:alloc +32, rt:ft +21, lang:attr +20 |
| data/xml_feeds | 0.741 | 0.744 | 1.00 | +14 / -12 / +67 / +0 / -68 | rt:ft +28, lang:list/tuple +21, lang:dict +20, interp:op +17 |
| devtools/black_format | 1.761 | 1.647 | 0.94 | +0 / +65 / +13 / -11 / -185 | lang:attr +32, rt:alloc +23, lang:list/tuple +23, rt:ft +22 |
| devtools/coverage_pyflakes_tests | 0.517 | 0.760 | 1.47 | +0 / +25 / +12 / +1 / +196 | rt:gc +114, rt:alloc +73, lang:dict +21, rt:ft +9 |
| devtools/isort_stdlib | 0.362 | 1.352 | 3.73 | +1 / +1 / +27 / +10 / +952 | rt:ft +951, interp:op +10, ext:24eba0d76380778f559b__mypyc +7, lang:dict +6 |
| devtools/pip_resolve | 0.790 | 0.816 | 1.03 | +0 / +22 / +8 / +3 / -7 | lang:dict +17, rt:ft +11, rt:alloc +10, lang:set +9 |
| devtools/pycodestyle_stdlib | 1.221 | 1.295 | 1.06 | +0 / +41 / +17 / +25 / -17 | lang:abstract +21, rt:ft +20, interp:op +19, cmod:re +13 |
| devtools/pyflakes_stdlib | 1.754 | 1.751 | 1.00 | +0 / +40 / +30 / +36 / -112 | lang:dict +67, rt:alloc +44, rt:ft +35, interp:op +31 |
| devtools/pylint_stdlib | 1.677 | 1.240 | 0.74 | -0 / +22 / +36 / -13 / -491 | lang:dict +27, rt:ft +14, unknown +10, cmod:compile +9 |
| devtools/pytest_networkx | 1.178 | 0.956 | 0.81 | -0 / -18 / +17 / -3 / -229 | rt:alloc +116, lang:dict +15, unknown +11, rt:ft +11 |
| devtools/rich_render | 0.388 | 0.409 | 1.05 | +0 / +13 / +13 / -5 / -3 | cmod:functools +11, lang:list/tuple +10, rt:ft +5, lang:calls +3 |
| devtools/sphinx_autodoc | 1.136 | 1.104 | 0.97 | -0 / +32 / +24 / -8 / -96 | lang:dict +23, rt:ft +15, cmod:re +15, unknown +15 |
| devtools/sqlglot_transpile | 1.128 | 1.314 | 1.17 | +0 / +41 / +15 / -11 / +121 | rt:gc +93, lang:dict +25, rt:alloc +24, unknown +21 |
| frameworks/django_admin | 1.493 | 1.833 | 1.23 | +0 / +125 / +28 / +81 / +78 | interp:op +85, lang:dict +57, rt:alloc +50, lang:attr +33 |
| frameworks/drf_api | 1.774 | 2.110 | 1.19 | +0 / +118 / +15 / +65 / +123 | rt:alloc +62, lang:dict +58, rt:gc +56, interp:op +53 |
| frameworks/falcon_api | 0.882 | 1.034 | 1.17 | +31 / +13 / +55 / +17 / +34 | interp:op +70, lang:dict +25, rt:alloc +24, rt:ft +14 |
| frameworks/fastapi_sqlalchemy | 1.974 | 2.328 | 1.18 | +5 / +100 / +129 / +158 / -59 | interp:op +144, lang:dict +79, cmod:selectors +42, rt:alloc +39 |
| frameworks/flask_marshmallow | 1.008 | 1.289 | 1.28 | -1 / +96 / +24 / +117 / +35 | interp:op +88, lang:dict +61, lang:attr +36, rt:alloc +20 |
| frameworks/strawberry_graphql | 1.253 | 1.737 | 1.39 | +7 / +110 / +38 / +59 / +260 | rt:gc +162, rt:alloc +85, interp:op +70, lang:dict +58 |
| libs/attrs_cattrs | 1.050 | 1.125 | 1.07 | +6 / +26 / +9 / +10 / -12 | lang:dict +48, lang:attr +23, interp:op +15, lang:list/tuple +14 |
| libs/boto3_client | 1.944 | 2.170 | 1.12 | +0 / +71 / +32 / +68 / +60 | lang:dict +70, interp:op +69, rt:ft +64, lang:list/tuple +30 |
| libs/bs4_scrape | 1.483 | 1.616 | 1.09 | +1 / +54 / +26 / +78 / -28 | interp:op +83, rt:alloc +30, cmod:functools +22, lang:attr +21 |
| libs/jsonschema_validate | 1.728 | 1.788 | 1.03 | -0 / +42 / +10 / +21 / -21 | lang:attr +35, interp:eval +34, rt:ft +25, lang:dict +20 |
| libs/pydantic_payloads | 0.869 | 1.002 | 1.15 | +2 / +9 / +11 / +33 / +85 | rt:dealloc +40, rt:ft +39, lang:dict +33, lang:attr +12 |
| libs/yaml_config | 1.235 | 1.336 | 1.08 | -0 / +21 / +2 / +66 / +10 | interp:op +56, lang:list/tuple +21, interp:eval +8, rt:alloc +7 |
| services/cache_service | 1.280 | 1.330 | 1.04 | +7 / +17 / +33 / +19 / -25 | interp:op +40, rt:ft +17, lang:dict +12, rt:alloc +12 |
| services/job_worker | 1.036 | 1.453 | 1.40 | +8 / +27 / +222 / +11 / +135 | cmod:threading +146, rt:alloc +140, rt:gc +71, interp:op +37 |
| services/uvicorn_fastapi | 1.511 | 2.557 | 1.69 | +10 / +170 / +400 / +335 / +92 | interp:op +282, cmod:threading +216, lang:dict +95, ext:uvloop +68 |
| services/waitress_flask | 1.273 | 1.558 | 1.22 | +5 / +75 / +142 / +73 / -54 | interp:op +110, cmod:threading +67, rt:alloc +43, lang:attr +34 |
| threads/compute_pool | 1.578 | 1.812 | 1.15 | +192 / -2 / +20 / +0 / +19 | interp:op +193, rt:ft +43, lang:list/tuple +24, lang:calls +16 |
| threads/http_server | 0.991 | 1.038 | 1.05 | +3 / +3 / +59 / +0 / -30 | interp:op +48, rt:alloc +41, unknown +14, lang:dict +13 |
| threads/pipeline_queue | 0.966 | 1.806 | 1.87 | +3 / +7 / +381 / +0 / +437 | rt:ft +357, cmod:threading +219, rt:alloc +75, lang:dict +49 |
| threads/request_pool | 0.824 | 1.048 | 1.27 | +17 / +37 / +49 / +0 / +106 | rt:ft +77, lang:dict +50, rt:alloc +43, interp:op +19 |
| threads/shared_cache | 0.756 | 1.470 | 1.94 | +46 / -6 / +554 / +0 / +112 | cmod:threading +445, rt:ft +82, interp:op +80, rt:alloc +24 |
| web/aiohttp_app | 0.355 | 0.447 | 1.26 | +10 / +16 / +22 / +24 / +19 | interp:op +24, lang:dict +10, rt:alloc +10, rt:ft +9 |
| web/asyncio_http | 0.291 | 0.317 | 1.09 | +3 / -0 / +21 / +0 / +2 | interp:op +8, cmod:json +6, lang:dict +6, rt:ft +4 |
| web/django_blog | 0.632 | 0.768 | 1.21 | -1 / +47 / +13 / +36 / +35 | interp:op +32, lang:dict +21, rt:alloc +20, lang:attr +16 |
| web/email_mime | 0.818 | 0.968 | 1.18 | -0 / -0 / +93 / +0 / +49 | rt:gc +36, lang:list/tuple +26, interp:op +22, lang:dict +21 |
| web/fastapi_pydantic | 0.377 | 0.486 | 1.29 | +2 / +15 / +39 / +34 / +18 | interp:op +34, cmod:threading +14, lang:dict +8, rt:alloc +8 |
| web/flask_jinja | 0.517 | 0.606 | 1.17 | +6 / +21 / +15 / +29 / +17 | interp:op +40, rt:alloc +11, lang:attr +10, lang:exceptions +8 |
| web/json_api | 0.493 | 0.584 | 1.18 | +11 / +7 / +53 / +0 / +20 | lang:dict +32, interp:op +16, rt:ft +12, lang:calls +11 |
| web/logging_service | 0.334 | 0.372 | 1.11 | +3 / +7 / +20 / +0 / +7 | interp:eval +7, interp:op +6, cmod:io +6, lang:dict +5 |
| web/requests_local | 0.353 | 0.363 | 1.03 | +0 / +4 / -4 / +6 / +1 | interp:op +9, lang:dict +6, rt:ft +5, rt:alloc +3 |
| web/sqlalchemy_orm | 0.475 | 0.512 | 1.08 | -1 / +27 / -4 / +19 / -11 | lang:dict +15, interp:op +15, rt:ft +6, lang:attr +6 |

| component | mean Δ (% of GIL time) |
|---|---:|
| rt:ft | 7.1 |
| interp:op | 3.4 |
| lang:dict | 2.5 |
| rt:alloc | 2.4 |
| cmod:threading | 1.9 |
| lang:attr | 1.6 |
| lang:list/tuple | 0.8 |
| lang:calls | 0.6 |
| interp:eval | 0.5 |
| unknown | 0.5 |
| lang:set | 0.3 |
| lang:generators | 0.3 |
| lang:exceptions | 0.2 |
| cmod:functools | 0.2 |

- **`set.__reduce__` self-contention** (found here independently; w2-threads found it first and owns the kernel prototype).
  - `set.__reduce__` is `@critical_section` on the set and calls `PySequence_List(set)`.
  - That reaches `_list_extend`, which opens `Py_BEGIN_CRITICAL_SECTION2(list, set)` on a mutex the thread already holds.
  - `_PyCriticalSection2_BeginSlow` → `_PyMutex_LockTimed` spins with `sched_yield`, parks, suspends the outer section, and only then succeeds.
  - Source: Objects/setobject.c:2711, clinic wrapper; Objects/listobject.c:1489.
  - Cost: 69.5% of `isort_stdlib`'s cycles on 3.15.1t, through `copy.deepcopy` → `object.__reduce_ex__` → `set.__reduce__`. That is 3.73× its 3.15.1 CPU time. Also 27% of `state_snapshot` (1.74×).
- **The rest of the FT cost is spread out:**
  - `rt:ft` +7.1% of GIL time on average (mostly the spin above, plus `_Py_MergeZeroLocalRefcount`, `_Py_DecRefShared`, `_Py_TryIncrefCompare`);
  - handlers +3.4% (atomic refcounting inline);
  - dict +2.5%;
  - allocation +2.4% (mimalloc for every object);
  - `threading` locks +1.9% in the services/threads families.
- **GC-heavy programs run *faster* on t:** billing 0.75×, pylint 0.74×, pytest 0.81× (the FT collector runs fewer full collections).

## 8. The -X perf distortion and wave 1's 46%

The same workload, bpf vs -X perf, in two separate processes:

| workload | own/lang/stdlib/3rd/runtime bpf | same, -X perf | interp+calls+attr bpf | -X perf | CALL_PY_* + inlined frames bpf | wall -X perf / bpf |
|---|---|---|---:|---:|---:|---:|
| custom/di_plugins | 36/18/29/0/12 | 36/19/28/0/13 | 67.9 | 71.9 | 5.0 | 1.26 |
| custom/ecs_arena | 55/21/1/0/10 | 53/26/1/0/11 | 73.0 | 76.8 | 8.0 | 1.07 |
| custom/event_ledger | 30/15/35/0/15 | 30/16/36/0/14 | 73.2 | 76.4 | 5.2 | 1.30 |
| custom/fulfilment_sim | 59/14/13/0/11 | 58/19/11/0/10 | 75.0 | 77.7 | 7.2 | 1.27 |
| custom/mini_interp | 61/31/0/0/7 | 56/37/0/0/7 | 73.2 | 77.9 | 9.9 | 1.37 |
| custom/order_domain | 35/16/9/0/20 | 37/19/8/0/18 | 60.2 | 66.1 | 5.2 | 1.12 |
| custom/orm_mapper | 23/21/39/0/14 | 25/20/38/0/15 | 41.8 | 44.6 | 2.1 | 1.12 |
| custom/report_engine | 35/17/20/0/16 | 35/23/17/0/15 | 57.9 | 63.4 | 6.8 | 1.24 |
| custom/rules_engine | 27/10/23/0/23 | 27/13/22/0/21 | 66.9 | 72.4 | 5.3 | 1.04 |
| custom/task_planner | 46/24/17/0/11 | 47/23/17/0/11 | 52.2 | 54.3 | 0.8 | 0.93 |
| data/backup_archive | 0/0/99/0/0 | 0/0/99/0/0 | 1.7 | 1.9 | 0.1 | 0.95 |
| data/billing_engine | 10/7/23/0/56 | 11/8/23/0/56 | 20.9 | 22.3 | 2.1 | 1.00 |
| data/config_merge | 2/16/16/55/10 | 2/20/15/53/10 | 73.6 | 77.2 | 9.1 | 1.30 |
| data/csv_etl | 11/12/57/0/19 | 12/11/57/0/20 | 28.5 | 31.5 | 1.0 | 1.30 |
| data/fs_inventory | 4/3/82/0/12 | 4/3/82/0/11 | 29.2 | 34.2 | 2.3 | 1.09 |
| data/log_analytics | 6/7/71/0/15 | 6/7/71/0/15 | 40.4 | 43.4 | 2.3 | 1.05 |
| data/pandas_etl | 2/14/1/74/9 | 2/11/1/73/9 | 11.3 | 12.6 | 0.2 | 0.92 |
| data/report_render | 7/10/68/0/14 | 8/10/69/0/13 | 40.7 | 45.2 | 2.1 | 0.93 |
| data/state_snapshot | 1/1/55/0/36 | 1/0/57/0/35 | 30.3 | 33.8 | 2.1 | 0.92 |
| data/text_corpus | 22/26/41/0/10 | 26/23/40/0/10 | 24.1 | 27.5 | 0.1 | 0.85 |
| data/tz_events | 12/18/45/0/24 | 12/17/46/0/24 | 36.7 | 36.3 | 0.2 | 1.08 |
| data/xml_feeds | 4/8/50/0/37 | 5/8/50/0/37 | 24.5 | 26.0 | 1.3 | 1.01 |
| devtools/black_format | 0/18/3/47/30 | 0/17/3/48/30 | 55.4 | 58.0 | 2.5 | 1.30 |
| devtools/coverage_pyflakes_tests | 0/16/25/30/28 | 0/20/23/30/26 | 45.0 | 51.2 | 4.0 | 1.41 |
| devtools/isort_stdlib | 1/2/38/43/12 | 1/1/41/40/13 | 36.7 | 40.6 | 2.2 | 1.12 |
| devtools/pip_resolve | 0/17/34/25/21 | 0/17/32/30/20 | 55.2 | 60.9 | 3.6 | 1.14 |
| devtools/pycodestyle_stdlib | 0/17/22/48/11 | 0/19/19/50/10 | 59.5 | 63.9 | 3.3 | 1.09 |
| devtools/pyflakes_stdlib | 0/14/22/25/39 | 0/16/20/28/36 | 36.6 | 42.0 | 2.4 | 1.32 |
| devtools/pylint_stdlib | 0/15/9/27/49 | 0/15/8/29/48 | 37.6 | 41.3 | 2.6 | 1.07 |
| devtools/pytest_networkx | 0/15/11/17/55 | 0/16/12/18/53 | 31.6 | 35.3 | 1.6 | 1.01 |
| devtools/rich_render | 0/21/11/44/20 | 0/22/9/47/19 | 55.9 | 61.4 | 4.1 | 1.07 |
| devtools/sphinx_autodoc | 0/15/23/33/28 | 0/16/22/33/28 | 45.4 | 47.8 | 2.3 | 1.08 |
| devtools/sqlglot_transpile | 0/26/5/50/19 | 0/27/6/49/17 | 66.1 | 70.3 | 4.7 | 1.07 |
| frameworks/django_admin | 0/26/15/42/16 | 0/28/14/42/15 | 59.6 | 65.4 | 5.0 | 0.97 |
| frameworks/drf_api | 0/32/17/33/16 | 0/31/18/35/16 | 55.4 | 60.0 | 3.6 | 0.89 |
| frameworks/falcon_api | 7/11/52/13/15 | 7/10/53/14/15 | 47.2 | 50.9 | 4.2 | 0.99 |
| frameworks/fastapi_sqlalchemy | 0/18/23/36/21 | 0/19/24/35/20 | 48.7 | 52.3 | 4.4 | 1.22 |
| frameworks/flask_marshmallow | 1/27/16/43/11 | 0/27/16/44/11 | 58.0 | 62.7 | 5.0 | 1.18 |
| frameworks/strawberry_graphql | 2/21/8/43/22 | 2/23/7/43/21 | 63.4 | 67.9 | 4.3 | 1.10 |
| libs/attrs_cattrs | 4/15/20/20/18 | 5/15/18/23/17 | 48.6 | 53.5 | 3.1 | 1.03 |
| libs/boto3_client | 0/26/19/42/12 | 0/26/18/43/12 | 56.9 | 62.8 | 4.9 | 1.08 |
| libs/bs4_scrape | 0/16/17/42/24 | 0/18/16/44/22 | 58.6 | 64.3 | 4.1 | 1.11 |
| libs/jsonschema_validate | 0/21/4/50/16 | 0/23/4/50/15 | 61.3 | 65.7 | 7.2 | 1.09 |
| libs/pydantic_payloads | 3/10/5/66/14 | 3/8/6/68/13 | 18.7 | 21.8 | 0.4 | 0.96 |
| libs/yaml_config | 0/21/1/72/6 | 0/24/1/70/5 | 82.0 | 85.6 | 9.5 | 1.20 |
| services/cache_service | 5/12/45/22/15 | 5/14/45/23/13 | 49.1 | 55.4 | 4.3 | 1.60 |
| services/job_worker | 5/7/53/7/28 | 4/7/50/7/29 | 38.3 | 41.5 | 3.0 | 1.45 |
| services/uvicorn_fastapi | 1/13/22/42/21 | 1/14/21/43/20 | 48.1 | 52.6 | 3.8 | 1.20 |
| services/waitress_flask | 1/16/31/28/23 | 1/17/31/33/17 | 41.3 | 49.0 | 3.4 | 1.39 |
| threads/compute_pool | 58/28/5/0/9 | 60/27/3/0/9 | 59.9 | 61.3 | 0.3 | 1.24 |
| threads/http_server | 2/3/72/0/23 | 2/2/61/0/32 | 33.3 | 32.0 | 2.5 | 3.10 |
| threads/pipeline_queue | 6/11/64/0/19 | 6/10/64/0/19 | 45.9 | 47.9 | 2.1 | 1.13 |
| threads/request_pool | 8/9/54/0/27 | 8/8/56/0/25 | 42.1 | 47.7 | 4.3 | 1.15 |
| threads/shared_cache | 10/14/61/0/15 | 9/14/60/0/16 | 45.7 | 49.6 | 3.4 | 1.37 |
| web/aiohttp_app | 7/14/29/35/14 | 6/15/28/36/14 | 48.0 | 51.1 | 3.2 | 0.93 |
| web/asyncio_http | 6/4/76/0/14 | 6/5/76/0/13 | 28.5 | 33.4 | 2.0 | 0.94 |
| web/django_blog | 0/26/18/40/15 | 0/26/17/41/14 | 57.1 | 61.5 | 4.5 | 0.94 |
| web/email_mime | 0/0/81/0/18 | 0/0/81/0/18 | 54.7 | 59.2 | 2.1 | 0.98 |
| web/fastapi_pydantic | 1/14/29/37/17 | 1/16/27/37/17 | 54.5 | 58.5 | 5.3 | 1.11 |
| web/flask_jinja | 3/29/16/37/14 | 3/30/15/38/13 | 55.5 | 61.2 | 5.4 | 1.02 |
| web/json_api | 14/14/51/0/15 | 16/14/51/0/14 | 43.2 | 47.8 | 2.4 | 0.96 |
| web/logging_service | 6/3/81/0/10 | 5/3/81/0/10 | 48.2 | 52.8 | 3.2 | 1.01 |
| web/requests_local | 1/9/59/16/13 | 1/10/56/17/14 | 49.5 | 53.2 | 4.1 | 1.11 |
| web/sqlalchemy_orm | 1/16/45/25/13 | 0/17/43/26/13 | 31.6 | 35.3 | 2.7 | 1.02 |
| **mean** | | | 47.8 | 51.7 | 3.6 | 1.12 (geo) |

- **The interpreter + calls + attribute bucket.** Native components `interp:*`, perf trampolines, `lang:calls`, `lang:attr`; all owners; the bucket wave 1 reported as 46.2%.
  - Distortion-free on the 34 wave-1 workloads: **41.1%**. -X perf on the same runs: 44.8%.
  - Wave 1 measured 46.2% with -X perf and cpu-clock on WSL2.
  - So the 46% was about 5 points (11% relative) too high (M). The conclusion "the largest pool is interpreting bytecode" stands. Its size is 41% for wave-1-style programs and 48% over all 65 (custom 64%).
- **Layer shares barely move under -X perf** (mean |Δ| 0.3–1.4 points per layer). The trampoline's cost spreads across owners.
- **The opcode split does move.** -X perf shows **0%** of specialized Python calls: CALL_PY_EXACT_ARGS, CALL_BOUND_METHOD_EXACT_ARGS, CALL_ALLOC_AND_ENTER_INIT, CALL_KW_PY. These are 3.6% of all cycles in bpf.
- **Owner shares by caller also move.** Dataclass construction moves 16% from generated to `language:calls`.
- **Never use -X perf for per-opcode or call-cost numbers.**
- **Wall time** under -X perf: ×1.06 (34) and ×1.12 (65) geometric mean, up to 1.4×. Single values are noisy on the shared box; `threads/http_server` 3.1× is noise from thread scheduling.

## 9. The largest pools, and how reachable they are

Mean % of cycles over 65 workloads (max), and the mechanism that could reach each.
Candidates are in the brief's format for the lead's table; the *s*, F, K, U, R estimates are
inferred, and the owning slice's prototypes supersede them.

| pool | mean (max) | reachable by | owner slice |
|---|---|---|---|
| inline handler time (the interpreter itself) | 37.7% (custom 52%) | a different execution engine (JIT / handler swap) only | w2-interp |
| generic attribute paths (LOAD_ATTR/STORE_ATTR generic, `getattr`/`hasattr`) | 5.5% (frameworks 11.6%) | handler swap of generic LOAD_ATTR; slot-level fast paths | w2-interp / w2-machinery |
| deallocation | 7.4% (13.3%) | no entry point (refcount → dealloc chain); allocator policy only | w2-runtime |
| cyclic GC, steady state | 6.1% (49.4%) | opt-in GC policy tier | w2-runtime |
| exit-time GC + finalization | 6.0% of whole process (pylint 42.6%) | opt-in policy (skip the final full collection when no finalizers are pending) | w2-runtime / w2-startup |
| dict operations from bytecode | 3.0% (task_planner 9.2%) | specialized handlers already inline them; generic `_PyDict_Subscript` 0.9% | w2-interp |
| isinstance (CALL_ISINSTANCE) | 1.6% (sqlglot 11.4%) | CALL_ISINSTANCE handler or `__instancecheck__` caching | w2-machinery |
| generated code (dataclasses, enum, attrs, namedtuple) | 2.1% (order_domain 18.7%) | vectorcall on generated functions (`__init__`/`__eq__`/`__repr__`) | w2-machinery |
| remaining wave-1 modules (logging, glob/scandir, json `default=`, datetime tz) | 4.8% after enable (logging_service 53%) | extend existing groups | w2-floors |
| stdlib not covered by wave 1: `re` 1.6, `sqlite3` 1.8 (C library), `functools.singledispatchmethod` 28% of event_ledger, `inspect.signature` 18% of di_plugins, `contextlib` 17% of rules_engine, `email` 59% of email_mime, `xml.etree` 50% of xml_feeds | 0.3–1.8% each (up to 59%) | pure-Python vectorcall kernels (inspect, contextlib, singledispatch, email headers) | w2-machinery |
| free-threading `set.__reduce__` | 0 on GIL; 69.5% of isort on t | `ml_meth` swap (exact) | w2-threads |

Candidate rows (only for pools this slice is first to quantify):

| candidate | layer | entry points + mechanism | share mean / max (which) | s | exact subset & fallback | floor vs current | risks | upstream | reach | prototype | evidence |
|---|---|---|---|---:|---|---|---|---|---|---|---|
| exit-time collection policy (opt-in): skip or shorten `_PyGC_CollectNoFail` at finalization when the program installed no finalizers or `gc.callbacks` | runtime | policy (atexit / `Py_AtExit` hook) | 6.0% of whole-process mean / 42.6% (pylint), 19–21% (pytest, sqlalchemy, fastapi_pydantic) | ∞ on the skipped part (I) | only when no `__del__`/weakref callbacks/`gc.callbacks` can observe the difference; else unchanged | ~0 vs 40% of a pylint run | finalizer order, `ResourceWarning`, open files flushed by finalizers | none known for 3.16 | every short-lived CLI using big heaps (I) | none | §7.2, `rollup/whole.md` |
| `datetime.strftime` import per call (3.15 `wrap_strftime` imports `time` each call) | runtime:import | `ml_meth` (already done by the wave-1 datetime kernel) | 1.1% steady import mean / 8.2% (tz_events) | — | — | — | — | upstream fix candidate | — | wave-1 kernel removes it (M, §6) | §7.1 |

## 10. Settled / out of reach, because…

- **Inline handler time (36.8% of cycles, plus 0.9% eval entry/exit):**
  - CALL_PY_EXACT_ARGS, LOAD_GLOBAL_*, LOAD_ATTR_INSTANCE_VALUE/SLOT/MODULE/METHOD_*, LOAD_FAST*, STORE_FAST, RETURN_VALUE inline.
  - No C entry point exists, so entry-point acceleration cannot reach them by construction. Only an execution-engine change can (w2-interp's handler swap or JIT).
- **Callee work of CALL_*_NON_PY handlers that is already C** (zlib, `_json`, `_sre`, `_pickle`, sqlite, expat): a Rust swap pays only where wave 1 showed headroom (§4 of IMPACT_ATLAS).
- **Kernel time** (syscalls of `os.scandir`, sockets) is in the stdlib layer and is irreducible for Rust.
- **Third-party bytecode (16.5% inclusive of the handlers)** is not a target by contract. Its calls into CPython (language 7.4%, stdlib 5.8%) are, through the entry points in §5.

## 11. Recommendations

1. **Use the bpf mode** (`benches/atlas/layers.py --modes bpf`) for every Phase-2 attribution and per-opcode number. Drop `-X perf` from the method (it removes 3.6% of real call work and adds ~5 points to the interpreter bucket).
2. **Make w2-interp's handler-swap evaluation start from §4.**
   - Pairs worth a swap are generic LOAD_ATTR (3.7%; frameworks 7.9%), CALL_ISINSTANCE (1.6%), generic CALL into slots, and BINARY_OP_SUBSCR_DICT (1.3%).
   - Everything CALL_*_NON_PY is the callee's work, not dispatch.
3. **Treat runtime policy as the next big pool after the interpreter:** steady GC 6.1% (max 49%), exit-time GC 6.0% of whole runs (max 42.6%), deallocation 7.4%.
4. **For "all Python programs":**
   - generated code (dataclass/enum/attrs) is 2.1% mean but up to 18.7% in domain-model programs;
   - `inspect`, `contextlib`, `functools.singledispatchmethod` are 17–28% where used.
   
   These are the stdlib machinery frameworks and custom code lean on (w2-machinery).
5. **Fix the wave-1 gaps that remain** (logging handlers, `glob`/`scandir`, json `default=`, zoneinfo arithmetic) before new stdlib targets. They are 4.8% mean after enabling, concentrated in the programs wave 1 already helps.

## 12. Not finished

- 3.15.1t with Rustique enabled was measured for the threads family only (profiles in the raw directory, not tabulated).
- Rustique's own kernels are symbolized by Rust symbol, not split per target.
- `pydantic_core` and other stripped extensions show as `?`.
- Whole-process profiles exist for 3.15.1 only.

## Appendix: sampling quality per workload

| workload | mode | samples | sampled GHz | owner exact | by name | imputed |
|---|---|---:|---:|---:|---:|---:|
| custom/di_plugins | bpf | 11787 | 3.035 | 98.5 | 1.5 | 0.0 |
| custom/ecs_arena | bpf | 10690 | 3.047 | 97.2 | 2.8 | 0.0 |
| custom/event_ledger | bpf | 13807 | 3.044 | 97.8 | 2.2 | 0.0 |
| custom/fulfilment_sim | bpf | 11821 | 3.036 | 97.8 | 2.1 | 0.0 |
| custom/mini_interp | bpf | 12152 | 3.014 | 99.5 | 0.5 | 0.0 |
| custom/order_domain | bpf | 12544 | 3.019 | 98.2 | 1.8 | 0.0 |
| custom/orm_mapper | bpf | 12386 | 3.077 | 99.2 | 0.8 | 0.0 |
| custom/report_engine | bpf | 12295 | 3.1 | 98.4 | 1.6 | 0.0 |
| custom/rules_engine | bpf | 12008 | 3.08 | 98.1 | 1.9 | 0.0 |
| custom/task_planner | bpf | 11969 | 3.077 | 99.8 | 0.2 | 0.0 |
| data/backup_archive | bpf | 12196 | 2.991 | 100.0 | 0.0 | 0.0 |
| data/billing_engine | bpf | 13440 | 3.009 | 99.6 | 0.3 | 0.0 |
| data/config_merge | bpf | 12209 | 2.948 | 99.3 | 0.7 | 0.0 |
| data/csv_etl | bpf | 11736 | 2.935 | 99.5 | 0.5 | 0.0 |
| data/fs_inventory | bpf | 11295 | 2.916 | 99.3 | 0.7 | 0.0 |
| data/log_analytics | bpf | 11353 | 2.985 | 98.9 | 1.1 | 0.0 |
| data/pandas_etl | bpf | 9061 | 2.968 | 99.1 | 0.9 | 0.0 |
| data/report_render | bpf | 10103 | 3.022 | 99.6 | 0.4 | 0.0 |
| data/state_snapshot | bpf | 9936 | 2.963 | 99.9 | 0.1 | 0.0 |
| data/text_corpus | bpf | 9085 | 3.061 | 98.7 | 1.2 | 0.0 |
| data/tz_events | bpf | 12302 | 3.113 | 99.9 | 0.1 | 0.0 |
| data/xml_feeds | bpf | 11179 | 3.018 | 99.5 | 0.4 | 0.0 |
| devtools/black_format | bpf | 13324 | 3.027 | 99.3 | 0.7 | 0.0 |
| devtools/coverage_pyflakes_tests | bpf | 11598 | 2.991 | 49.2 | 50.8 | 0.0 |
| devtools/import_startup | xperf | 19584 |  | 0.0 | 0.0 | 0.0 |
| devtools/isort_stdlib | bpf | 10667 | 2.944 | 99.7 | 0.3 | 0.0 |
| devtools/pip_resolve | bpf | 11974 | 3.03 | 97.7 | 2.3 | 0.0 |
| devtools/pycodestyle_stdlib | bpf | 11411 | 3.116 | 99.9 | 0.0 | 0.0 |
| devtools/pyflakes_stdlib | bpf | 13620 | 3.106 | 99.8 | 0.2 | 0.0 |
| devtools/pylint_stdlib | bpf | 11696 | 2.79 | 99.4 | 0.6 | 0.0 |
| devtools/pytest_networkx | bpf | 11646 | 2.824 | 72.1 | 27.9 | 0.0 |
| devtools/rich_render | bpf | 10726 | 2.764 | 97.8 | 2.2 | 0.0 |
| devtools/sphinx_autodoc | bpf | 12352 | 3.107 | 99.4 | 0.6 | 0.0 |
| devtools/sqlglot_transpile | bpf | 11921 | 3.021 | 99.4 | 0.6 | 0.0 |
| frameworks/django_admin | bpf | 10421 | 2.792 | 98.7 | 1.3 | 0.0 |
| frameworks/drf_api | bpf | 12627 | 2.847 | 98.4 | 1.5 | 0.0 |
| frameworks/falcon_api | bpf | 11320 | 2.853 | 98.2 | 1.8 | 0.0 |
| frameworks/fastapi_sqlalchemy | bpf | 10895 | 2.76 | 95.3 | 1.6 | 0.0 |
| frameworks/flask_marshmallow | bpf | 12296 | 3.051 | 98.7 | 1.3 | 0.0 |
| frameworks/strawberry_graphql | bpf | 13017 | 2.968 | 98.7 | 1.3 | 0.0 |
| libs/attrs_cattrs | bpf | 12435 | 2.962 | 98.6 | 1.3 | 0.0 |
| libs/boto3_client | bpf | 11770 | 3.027 | 98.6 | 1.3 | 0.0 |
| libs/bs4_scrape | bpf | 12993 | 2.921 | 99.0 | 1.0 | 0.0 |
| libs/jsonschema_validate | bpf | 12458 | 2.885 | 98.7 | 1.3 | 0.0 |
| libs/pydantic_payloads | bpf | 11122 | 2.843 | 99.5 | 0.5 | 0.0 |
| libs/yaml_config | bpf | 11480 | 3.098 | 99.5 | 0.5 | 0.0 |
| services/cache_service | bpf | 13318 | 2.972 | 99.4 | 0.6 | 0.0 |
| services/job_worker | bpf | 12115 | 2.925 | 98.8 | 1.2 | 0.0 |
| services/uvicorn_fastapi | bpf | 13329 | 2.941 | 98.5 | 1.5 | 0.0 |
| services/waitress_flask | bpf | 11344 | 2.971 | 99.0 | 0.8 | 0.0 |
| threads/compute_pool | bpf | 11141 | 2.825 | 99.9 | 0.1 | 0.0 |
| threads/http_server | bpf | 10337 | 2.607 | 98.8 | 0.8 | 0.5 |
| threads/pipeline_queue | bpf | 9045 | 2.342 | 98.8 | 1.1 | 0.8 |
| threads/request_pool | bpf | 10132 | 2.458 | 98.6 | 1.2 | 0.4 |
| threads/shared_cache | bpf | 11051 | 2.924 | 98.5 | 1.4 | 0.5 |
| web/aiohttp_app | bpf | 11786 | 2.891 | 99.3 | 0.7 | 0.0 |
| web/asyncio_http | bpf | 12204 | 2.995 | 99.6 | 0.4 | 0.0 |
| web/django_blog | bpf | 12223 | 2.975 | 98.4 | 1.6 | 0.0 |
| web/email_mime | bpf | 12271 | 3.0 | 98.3 | 1.7 | 0.0 |
| web/fastapi_pydantic | bpf | 13061 | 3.148 | 98.9 | 1.1 | 0.0 |
| web/flask_jinja | bpf | 12496 | 3.024 | 99.0 | 1.0 | 0.0 |
| web/json_api | bpf | 12797 | 3.054 | 99.1 | 0.9 | 0.0 |
| web/logging_service | bpf | 12428 | 3.1 | 98.5 | 1.5 | 0.0 |
| web/requests_local | bpf | 11944 | 2.944 | 98.6 | 1.4 | 0.0 |
| web/sqlalchemy_orm | bpf | 11989 | 2.968 | 99.2 | 0.8 | 0.0 |
