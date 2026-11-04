# Free-threading: what 3.15t costs, how threaded programs scale, what Rustique erases

Wave 2, Phase 1, slice `w2-threads` (branch `w2/p1-threads`). Builds: python-build-standalone
3.15.1 / 3.15.1t (GIL disabled), Rustique at `wave2` + this branch. Measurements on the shared
dev box (c7a.8xlarge, AMD EPYC 9R14, 1 thread per core), pinned to cores 24–31 (one L3
domain). Tags: **M** measured (how), **I** inferred (from what).

## Summary

1. **The big free-threading penalties come from one CPython bug, not from free-threading.**
   - `set.__reduce__` is a clinic `@critical_section` method whose body calls
     `PySequence_List(self)`, and that takes a two-object critical section on (new list,
     set). `_PyCriticalSection2_BeginSlow` only recognises a recursive lock when the previous
     section also held two mutexes, so the thread contends with itself: 40 × `sched_yield`,
     then park/detach.
   - Cost: `set.__reduce__` takes 34.5 µs on every t build from 3.14.9t to 3.16.0a1t, vs
     0.6 µs with the GIL (57×). `deepcopy(frozenset)` takes 37 µs vs 2.4 µs (**M**, §1.2).
   - It accounts for isort 4.2× and state_snapshot 1.9×: 92% / 66% of their t−GIL delta is
     lock + kernel time. They are the only two of 34 single-threaded workloads that call
     `sched_yield` at all.
   - Upstream fix: a one-liner, not filed this round.
2. **The rest of the t-build tax is 1.068×** (geomean of the other 32 workloads; 1.132 with the
   two). It is spread thin:
   - the free-threaded GC, +149 to −437 ms per run, so in some programs cheaper than the GIL
     build's;
   - specialized bytecodes 3–8% slower;
   - thread-safe dict lookups, merged refcounts, slotted/instance attribute stores, the
     type-cache update path.
   It is inherent or runtime policy, not a kernel target (§1.3, §5).
3. **`set/frozenset.__reduce__` kernel (prototype in this branch)**: identical to CPython on
   2,000 generated cases on both builds; GIL callgrind cells 0.993–1.008; on 3.15.1t
   `s.__reduce__()` 0.020×, `deepcopy(frozenset)` 0.074×. With *only* this kernel,
   isort 0.260× and state_snapshot 0.689× (all groups but copy: 0.273 / 0.703). It helps
   every caller, not only Rustique's deepcopy (§4.1).
4. **uvicorn + FastAPI on t is 1.29×, not 2.1×**, in steady state on dedicated cores.
   - The residual is a sync FastAPI dependency's anyio thread hop per request: 3.15t's
     `PyMutex` spin-waits (372k `sched_yield` per run).
   - An `async def` dependency brings it to 1.05×.
   - The reported 2.1× matches what spin-waiting does on oversubscribed cores (§1.4).
   - waitress and job_worker run 4.1× and 2× faster on t.
5. **Threaded programs (new `threads` family, 5 programs, §2)**:
   - 3.15.1t runs them 2.2–6.4× faster at 8 threads than at 1 thread; the GIL build gets
     slower with threads.
   - Rustique's gain on the t build is 0.73 (geomean) at 1 thread and 0.82 at 8 threads.
     It shrinks where a serial stage remains (Amdahl) and where the kernels contend on
     shared inputs.
   - Combined: enabled 3.15.1t at 8 threads vs plain 3.15.1 = 2.7–6.1× faster.
6. **Rustique under threads**, measured by CPU time per call at 8 threads (**M**, §3.1):
   - **Gains**: kernels that skip CPython 3.15t's own serialization points run 20–50×
     faster than plain at 8 threads: strptime (`_strptime._cache_lock`), strftime (per-call
     `import time`), fnmatch (private `lru_cache` critical section), statistics.
   - **Real Rustique cliffs**:
     - logging's global `FMT_CACHE` lock: Formatter.format's gain is gone at 8 threads;
     - Rust heap allocation through glibc `malloc` (`quote`: arena-lock futex storm, CPU
       ×19.6);
     - whole-list critical sections on *shared* lists (`x in`, `list.index`: 1.8–2.1×
       slower than plain);
     - extra refcount/lock traffic on shared arguments (deepcopy, asdict, isoformat,
       pathlib, json.dumps).
   - Fixes in §3.4/§4.2.
7. **Safety** (§3.3–3.4):
   - **S1 (confirmed)**: `enable()` raises pyo3 `PanicException` when another thread
     mutates a snapshot-pinned module dict (1 in 834 cycles; `activate()` iterates under
     `TABLES`, outside `catch_unwind`). The lazy-install path refuses the target instead (a
     silent reach loss).
   - **S2 (latent)**: two lock-order inversions around `TABLES` (dict critical sections and
     CPython's global `TYPE_LOCK` via `PyType_Watch`). Not reachable with wave 1's targets
     (0 hangs in ~195k racing activations); reachable by the first target that pins a
     class attribute.
   - Clean: 15.8M checked calls under enable/disable, 38k cross-thread `mock.patch` rounds,
     200 daemon-thread exits.
   - **Never-slower gap**: net-benefit suspension runs only on the main thread
     (`Py_AddPendingCall`), so fallback-heavy calls in worker threads keep paying the
     vectorcall fallback while the main thread waits (1.185 measured).

## 1. The free-threading penalty (single-threaded programs)

### 1.1 Size

Plain run time on 3.15.1t ÷ 3.15.1 (Phase 3 medians, `docs/atlas/phase3.md` §4, **M**), with
`sched_yield` calls per steady-state `run()` on 3.15.1t (**M**,
`benches/ft/syscall_sweep.sh`, perf tracepoint `syscalls:sys_enter_sched_yield`) and the same
ratio with Rustique enabled:

| workload | 3.15.1 ms | 3.15.1t ms | t/GIL plain | t/GIL enabled | sched_yield / run |
|---|---:|---:|---:|---:|---:|
| devtools/isort_stdlib | 284 | 1187 | **4.19** | 1.06 | 947,600 |
| data/state_snapshot | 648 | 1249 | **1.93** | 1.41 | 432,000 |
| devtools/coverage_pyflakes_tests | 382 | 494 | 1.29 | 1.30 | 0 |
| devtools/import_startup | 1899 | 2246 | 1.18 | 1.18 | 160 (children) |
| devtools/sqlglot_transpile | 879 | 1030 | 1.17 | 1.18 | 0 |
| data/tz_events | 526 | 614 | 1.17 | 1.21 | 0 |
| data/report_render | 406 | 464 | 1.14 | 1.11 | 0 |
| web/fastapi_pydantic | 296 | 339 | 1.14 | 1.15 | 32,120 (thread pool) |
| 22 more workloads | | | 1.00–1.13 | | 0 (pandas 120, requests_local 80) |
| data/billing_engine | 918 | 758 | 0.83 | 0.83 | 0 |
| devtools/pylint_stdlib | 1182 | 899 | 0.76 | 0.76 | 0 |
| **geomean, 34 workloads** | | | **1.132** | **1.084** | |
| geomean without isort, state_snapshot | | | 1.068 | | |
| services/uvicorn_fastapi (w2-wl-frameworks; §1.4, steady state) | 1049 | 1354 | 1.29 | | 372,121 (anyio thread hop) |
| services/waitress_flask (threaded WSGI) | 1009 | 248 | 0.25 | | 229,491 (real threads) |
| services/job_worker (4 threads) | 677 | 340 | 0.50 | | 1,061,816 (real threads) |

A single-threaded program should make no `sched_yield` call: CPython's `PyMutex` yields only
while it spins on a lock held by someone else. `fastapi_pydantic` runs sync endpoints in
anyio's thread pool, `requests_local` has a server thread, `import_startup` counts its child
processes; isort and state_snapshot have no other thread.

### 1.2 Root cause of the large penalties: `set.__reduce__` contends with itself

Call chain (perf call graph of isort on 3.15.1t, **M**): `copy.deepcopy` →
`object.__reduce_ex__` → `set___reduce__` (clinic wrapper: `Py_BEGIN_CRITICAL_SECTION(so)`)
→ `set___reduce___impl` → `PySequence_List(so)` → `_list_extend` →
`Py_BEGIN_CRITICAL_SECTION2(list, so)` → `_PyCriticalSection2_BeginSlow` → `PyMutex_Lock`
on the set's mutex, held by the same thread → `_PyMutex_LockTimed` spins
`MAX_SPIN_COUNT` (40) × `_Py_yield()` = `sched_yield`, then parks with detach;
`_PyThreadState_Detach` → `_PyCriticalSection_SuspendAll` unlocks the outer section (the set),
the park returns, the lock is taken. `_PyCriticalSection_BeginSlow` (one mutex) has the
"already held by the top-most section" shortcut for both one- and two-mutex predecessors;
`_PyCriticalSection2_BeginSlow` only for a two-mutex predecessor with the same pair
(`Python/critical_section.c`, 3.15.1).

Cost per call, `timeit` min of 5 × 20,000, one pinned core (**M**):

| build | `s.__reduce__()` (5 ints) | `deepcopy(frozenset 5)` | `deepcopy([1,2,3])` | `list(s)` |
|---|---:|---:|---:|---:|
| 3.15.1 | 600 ns | 2,415 ns | 633 ns | 66 ns |
| 3.15.1t | **34,508 ns** | **36,892 ns** | 690 ns | 61 ns |
| 3.15.2t | 34,581 ns | 37,100 ns | 715 ns | 63 ns |
| 3.14.9 | 568 ns | 2,693 ns | 655 ns | 65 ns |
| 3.14.9t | 34,510 ns | 37,322 ns | 731 ns | 64 ns |
| 3.14.10t | 34,705 ns | 37,411 ns | 734 ns | 62 ns |
| 3.16.0a1t | 34,800 ns | 37,247 ns | 693 ns | 62 ns |

Who reaches it: everything that reduces a set or frozenset through `__reduce_ex__` —
`copy.deepcopy` of sets/frozensets (and of anything containing them), `copy.copy` of set
subclasses, `pickle` protocol 0–3 of sets and protocol ≥ 4 of set subclasses
(`save_set` handles only exact sets at protocol ≥ 4), `dataclasses.asdict`/`astuple`
(deepcopy of leaves), pydantic `model_copy(deep=True)`, `multiprocessing` hand-off of set
subclasses. A static scan of CPython's 467 clinic `@critical_section` functions for bodies
that convert or iterate `self` through a function that locks it again finds only this one
on a hot path (others: `deque.extend(self)`, `bytearray.extend` of itself — edge cases).

**Upstream fix** (not filed this round, per the brief): either drop `@critical_section` from
`set.__reduce__` (its only shared read, `PySequence_List`, locks the set itself;
`_PyObject_GetState` reads `__dict__` under its own locks), or teach
`_PyCriticalSection2_BeginSlow` to skip locking a mutex already held by the top-most
one-mutex section (lock only the other one). Evidence: the perf call graph above, the
`sched_yield` counts, and the table. Overlap U for a Rustique kernel: high once reported
(a one-line fix is likely to be backported to 3.14.x/3.15.x), so the kernel is a bridge.

### 1.3 The general tax: where the rest of the t−GIL difference goes

Steady-state profiles of 12 workloads on both builds (`benches/ft/perfprof.sh record`, plain
`perf record -F 1999 -g`, no `-X perf`: its trampoline disables inlined Python calls, 3 runs
after setup and warm-up), compared by `benches/ft/ftdiff.py` (each symbol's share × median
run time = ms per run, folded into categories). Delta t − GIL in ms per `run()` (**M**; the
two builds run in parallel on separate cores of the same L3, so ratios differ a little from
§1.1):

| workload | GIL ms | t ms | t/GIL | locks | kernel | gc | refcount | dict | containers | attr | interp spec. | interp generic | alloc | other |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| isort_stdlib | 343 | 1346 | 3.92 | +197 | +721 | +2 | +8 | +10 | +20 | -0 | +11 | +1 | +2 | +24 |
| state_snapshot | 801 | 1501 | 1.87 | +82 | +383 | +43 | +15 | +39 | +7 | +3 | +43 | +2 | +20 | +61 |
| coverage_pyflakes_tests | 450 | 688 | 1.53 | -0 | +12 | **+149** | +9 | +7 | +3 | +0 | +19 | +3 | +4 | +25 |
| sqlglot_transpile | 996 | 1230 | 1.24 | +2 | -4 | **+111** | +15 | +19 | +12 | +33 | +20 | +3 | -5 | +13 |
| flask_jinja | 470 | 558 | 1.19 | -0 | -2 | +5 | +7 | +10 | +3 | +4 | +28 | +5 | +2 | +21 |
| csv_etl | 413 | 485 | 1.17 | +0 | +21 | -1 | +3 | +10 | +7 | -2 | +21 | -2 | -7 | +18 |
| tz_events | 607 | 691 | 1.14 | -0 | +4 | +38 | +19 | +31 | +4 | -5 | +7 | -2 | -10 | -8 |
| report_render | 448 | 502 | 1.12 | +1 | -1 | +8 | +4 | -1 | +13 | +0 | +3 | -1 | -8 | +39 |
| django_blog | 582 | 650 | 1.12 | -0 | +9 | +11 | +6 | +14 | +6 | -5 | +18 | -2 | +1 | +8 |
| black_format | 1427 | 1492 | 1.05 | +0 | +11 | **-119** | +25 | +19 | +25 | +35 | +78 | -4 | -23 | +10 |
| pylint_stdlib | 1437 | 1169 | 0.81 | -0 | +11 | **-437** | +14 | +20 | +14 | +10 | +43 | +0 | +14 | +34 |
| billing_engine | 1189 | 896 | 0.75 | +0 | -50 | **-372** | +58 | +4 | +1 | +1 | +13 | +8 | -36 | +87 |

Reading (largest symbols from `<workload>.ftdiff.md`, ~/rustique-research/w2/threads/pen2/):

- **GC, both directions.** The free-threaded collector marks the whole heap by walking
  mimalloc pages (`mi_heap_visit_blocks`, `gc_mark_enqueue_no_buffer_visitproc`,
  `update_refs`, `scan_heap_visitor`, every `*_traverse`): +149 ms on coverage_pyflakes,
  +111 sqlglot, +38 tz_events. The GIL build's collector costs far more on other programs:
  pylint 417 vs 104 ms, billing_engine 461 vs 132, black 193 vs 107 (**M**). GC policy is
  `w2-runtime`'s; the t build needs its own policy evidence (same thresholds, different
  collector).
- **Interpreter**: specialized instructions 3–8% slower (`STORE_FAST`, `COMPARE_OP_INT`,
  `LOAD_ATTR_INSTANCE_VALUE`, `CALL_METHOD_DESCRIPTOR_FAST`: stack references with deferred
  refcount tags, atomic inline-cache reads). Inherent to the build.
- **Refcount**: `_Py_MergeZeroLocalRefcount`, `_Py_DecRefShared` (5–60 ms; billing_engine 58:
  `Decimal` objects created by one call and released in another context). Inherent; a kernel
  avoids it only for the objects it touches internally.
- **Dict**: `_Py_dict_lookup_threadsafe(_stackref)`, `_PyDict_SetItem_Take2`,
  `_PyObjectDict_SetItem`: lock-free reads with try-incref, writes under the dict's critical
  section (+10–39 ms).
- **Attribute stores**: `_PyObject_StoreInstanceAttribute` (black +35 ms) and
  `PyMember_SetOne` for `__slots__` (sqlglot +20 ms): atomic stores and delayed frees of the
  old value. Type lookups: `update_cache_gil_disabled` (tz_events +20, sqlglot +9).
- **Containers**: `PyList_Append`, `list_pop`, `_list_extend`, `_PySet_Contains`,
  `frozenset_vectorcall` (+3–25 ms): release stores and per-object locks.
- **Allocation is not a penalty**: mimalloc is faster than pymalloc here (−5 to −36 ms).
- `other`: C-module code compiled differently (`_random_Random_random` +14 ms under its
  per-object lock, `sre_*` +10–19 ms), Python-function call paths.

### 1.4 Services from `w2-wl-frameworks` (uvicorn, waitress, job worker)

The `services` family (w2-wl-frameworks commit d9e1293) reported uvicorn + FastAPI 2.1×
slower on 3.15.1t (2.51 s vs 1.18 s, single runs on cores 0–23 while ~10 agents ran there).
Steady-state measurement here: run time from `run.py one -n 5` × 3 processes per build
(median), syscall counts from `syscall_sweep.sh` (perf stat, 3 runs); cores 24–31; **M**:

| workload | 3.15.1 s/run | 3.15.1t s/run | t/GIL | sched_yield / run (t) | futex / run (GIL / t) | context switches (GIL / t) |
|---|---:|---:|---:|---:|---:|---:|
| uvicorn_fastapi | 1.049 | 1.354 (runs 1.10–1.52) | **1.29** | 372,121 | 74,691 / 18,265 | 27,172 / 17,177 |
| same, `get_catalogue` made `async def` | 0.896 | 0.939 | 1.05 | 0 | 0 / 0 | 4 / 6 |
| waitress_flask (8 clients, 4 workers) | 1.009 | 0.248 | **0.25** | 229,491 | 406,628 / 5,252 | 144,185 / 25,761 |
| job_worker (4 threads) | 0.677 | 0.340 | **0.50** | 1,061,816 | 112,175 / 22,500 | 37,535 / 12,983 |

- **uvicorn's penalty is a thread hop, not a CPython bug.** The app's `Depends(get_catalogue)`
  is a plain `def`, so FastAPI runs it on anyio's worker thread for every request (a
  `queue.Queue` hand-off out, `loop.call_soon_threadsafe` back). Making it `async def` removes
  the hop: −15% (GIL) and −31% (t) run time, and t/GIL falls to 1.05 (the general tax, §1.3).
  With the hop, 3.15t hands off between the loop thread and the worker by spinning:
  `PyMutex` yields up to 40 times before parking, ~58 `sched_yield` per hand-off (**I**: 372k
  over ~6,400 hand-offs per run). That costs CPU (task-clock 1.59 s per 1.42 s run under perf)
  instead of the GIL build's futex sleeps, and makes the t run time noisy (1.10–1.52 s).
- **The reported 2.1× is an oversubscription effect** (**I**, from this measurement plus the
  conditions of theirs). Spin-waiting threads compete for cores. When the CPU set is busy,
  every hand-off pays scheduler delays on top. The same workload re-timed here while my own
  8-thread benchmark ran on the same cores: GIL 1.8–2.6 s, t 2.1–3.9 s per run. A
  production server with spare cores sees ~1.3×; an oversubscribed container sees much
  more. Rustique cannot change `PyMutex`'s spin policy: the policy is inherent to the build.
  The cure is in the program (async dependencies), which is ruled out by the "no code
  change" vision.
- waitress (4.1× faster on t) and job_worker (2× faster) show the other side: CPU work in
  worker threads scales.

### 1.5 Penalty sources: reachable or inherent

| source | where it shows | share of t−GIL (I, from §1.3) | reachable by a kernel? |
|---|---|---|---|
| **self-contended critical section** (`set.__reduce__`) | deepcopy/copy/pickle of sets | 66–92% in 2 of 34 workloads, 0 elsewhere | **yes**: `ml_meth` on `set/frozenset.__reduce__` (§4.1), or any kernel that reduces sets itself (copy kernel today) |
| uncontended per-object locks in C methods (`_random` per instance, `list`/`dict` writes, clinic `@critical_section` methods) | random, list/dict mutation | 1–5% | only inside a kernel's own work (a kernel that already owns the object, or builds a fresh one, takes no lock: e.g. json encoding into a fresh buffer) |
| **contended** per-object locks and shared refcounts (multi-threaded programs) | shared containers, module-level `random`, `functools.lru_cache` caches, ABC `isinstance`, shared compiled regexes, `_strptime._cache_lock`, logging handler lock | 0 single-threaded; dominant in §2/§3 | partly: a kernel can avoid a lock CPython takes (private `lru_cache`s, `_cache_lock`, ABC cache: §3.1, §4.2) or refcount traffic on shared inputs; not the program's own locks, not objects it must return (regex `Match.re`) |
| free-threaded GC (full-heap mark) | GC-heavy, large heaps | up to 63% of the delta where it costs more (+38 to +149 ms); −119 to −437 ms where the GIL build's GC costs more | runtime policy (`w2-runtime`), not a kernel |
| interpreter tax (stackrefs, atomic cache reads, no-GIL specializations) | all bytecode | 20–40% of the general tax | no (inherent); a kernel removes it only for the bytecode it replaces |
| merged / shared refcounts | objects freed by another thread or context | 5–20% | no, except inside kernels |
| dict thread-safe lookups/stores, attribute stores, type-cache updates | attribute-heavy OO code | 10–30% | no (language layer; `w2-machinery`/`w2-interp` territory) |
| mimalloc vs pymalloc | all | negative (a gain) | n/a |
| QSBR | — | < 1% (`_Py_qsbr*`, delayed frees) | n/a |

## 2. Multi-threaded workloads

### 2.1 The `threads` family

`benches/workloads/threads/` (README, empty requirements: stdlib only). Every `run()` does a
**fixed amount of work** on `WL_THREADS` threads (default 8; read in `setup()`), so time
falls as threads are added when the build runs them in parallel. Results are kept per
request index or aggregated commutatively and sorted: the digest is the same for every
thread count, schedule and build (checked: one digest per workload across 1/2/4/8 threads,
plain and enabled, on each build).

| workload | stands for | shared state the threads touch |
|---|---|---|
| `request_pool` | gunicorn `gthread` / `ThreadPoolExecutor` job API: `urllib.parse`, `json`, compiled regexes, `fromisoformat`/`strptime`, dataclasses + `asdict`, `deepcopy` of a response template (12,000 requests) | read-only catalog dict, compiled patterns, the executor's queue and futures |
| `pipeline_queue` | log shipper / ETL stages: producer → bounded `queue.Queue` → N parser threads (regex, `strptime`, `urlsplit`/`parse_qsl`, `ipaddress`) → aggregator (`Counter`) (36,000 lines) | two queues (Condition + deque), one aggregator thread |
| `http_server` | thread-per-connection `ThreadingHTTPServer` JSON service + N keep-alive `http.client` clients over 127.0.0.1 (3,600 requests) | the store dict under a `Lock`, sockets |
| `compute_pool` | CPU-bound pure Python on a pool: tokenizing, TF-IDF cosine, edit-distance fuzzy matching, Dijkstra (300 documents) | read-only profiles/vocabulary/graph |
| `shared_cache` | a service's shared state: read-through dict cache + `Lock`, `lru_cache`, `threading.local`, metrics under a lock, `logging` to one handler, `Decimal` pricing, `json.dumps` (36,000 requests) | cache dict, `lru_cache`, logging handler lock, metrics lock |

Rustique targets hit per `run()` (8 threads, 3.15.1t, `rustique.stats()`, **M**):
- request_pool: `tuple.__contains__` 24k, `parse_qs`/`_urlsplit`/`json.dumps`/`copy.deepcopy`
  12k each, `isoformat` 8.8k, `json.loads` 6k, `asdict` 5.8k.
- pipeline_queue: `unquote`/`parse_qsl`/`strptime`/`strftime` 36k each, `_urlsplit` 12k.
- http_server: `tuple.__contains__` 7k (35k fall back: `args`), `json.dumps` 3.6k, `min` 3.3k,
  `parse_qs` 2.4k.
- compute_pool: `min` 7.7M (in the edit distance), no other kernel of weight.
- shared_cache: `json.dumps`/`isoformat` 36k each; logging `findCaller`/`LogRecord`/
  `formatTime` 29k each; `Logger.level` 0 hits and 29k `deopt` fallbacks. That vectorcall
  target is never suspended: its fallbacks happen on worker threads while the main thread
  waits in `join()`, so the pending call that would suspend it never runs (§3.2). Cost
  ≈ 29k × ~200 Ir ≈ 2 ms per 425 ms run (**I**, from M§1.2's fallback cost).

### 2.2 Scaling, plain vs Rustique

`benches/ft/scaling.py run threads -t 1,2,4,8 -p 3 -n 3` per build (fresh process per cell:
setup, warm-up, 3 timed runs; median of 9; plain and enabled interleaved). Dev box, cores
24–31, after the pin guard (**M**; wall time, so the quiet-box addendum repeats it with p=5).
Every digest matched across thread counts, modes and builds. The enabled side includes the
`set` prototype group.

Median ms per `run()`:

| workload | build | mode | 1 thr | 2 thr | 4 thr | 8 thr | speedup ×8 |
|---|---|---|---:|---:|---:|---:|---:|
| compute_pool | 3.15.1 | plain | 1233 | 1263 | 1283 | 1299 | 0.95 |
| compute_pool | 3.15.1t | plain | 1377 | 700 | 367 | 216 | **6.38** |
| compute_pool | 3.15.1t | enable | 1388 | 698 | 365 | 218 | 6.38 |
| http_server | 3.15.1 | plain | 555 | 608 | 763 | 728 | 0.76 |
| http_server | 3.15.1t | plain | 468 | 247 | 169 | 103 | **4.56** |
| http_server | 3.15.1t | enable | 409 | 219 | 152 | 91 | 4.50 |
| request_pool | 3.15.1 | plain | 468 | 508 | 506 | 550 | 0.85 |
| request_pool | 3.15.1 | enable | 284 | 292 | 290 | 287 | 0.99 |
| request_pool | 3.15.1t | plain | 501 | 320 | 211 | 153 | **3.27** |
| request_pool | 3.15.1t | enable | 306 | 203 | 149 | 127 | 2.41 |
| pipeline_queue | 3.15.1 | plain | 643 | 653 | 648 | 659 | 0.98 |
| pipeline_queue | 3.15.1 | enable | 354 | 358 | 360 | 360 | 0.98 |
| pipeline_queue | 3.15.1t | plain | 709 | 504 | 362 | 323 | 2.20 |
| pipeline_queue | 3.15.1t | enable | 392 | 315 | 249 | 233 | 1.68 |
| shared_cache | 3.15.1 | plain | 539 | 538 | 557 | 574 | 0.94 |
| shared_cache | 3.15.1 | enable | 355 | 355 | 379 | 388 | 0.92 |
| shared_cache | 3.15.1t | plain | 610 | 435 | 309 | 283 | 2.16 |
| shared_cache | 3.15.1t | enable | 425 | 314 | 218 | 197 | 2.16 |

Enabled ÷ plain:

| workload | 3.15.1, 1 thr | 3.15.1, 8 thr | 3.15.1t, 1 thr | 3.15.1t, 2 thr | 3.15.1t, 4 thr | 3.15.1t, 8 thr |
|---|---:|---:|---:|---:|---:|---:|
| compute_pool | 0.992 | 0.975 | 1.007 | 0.997 | 0.995 | 1.008 |
| http_server | 0.908 | 0.986 | 0.874 | 0.887 | 0.903 | 0.885 |
| request_pool | 0.606 | 0.523 | 0.610 | 0.635 | 0.707 | **0.829** |
| pipeline_queue | 0.550 | 0.547 | 0.554 | 0.624 | 0.687 | **0.722** |
| shared_cache | 0.659 | 0.676 | 0.696 | 0.722 | 0.706 | 0.694 |
| **geomean** | 0.72 | 0.71 | **0.73** | | | **0.82** |

Reading:

- **Free-threading works for these programs.** At 8 threads 3.15.1t runs them 2.2–6.4×
  faster than at 1 thread, while the GIL build gets slower with threads (0.76–0.99). Pure
  Python compute scales best (6.4×). Pipelines with one producer and one aggregator scale
  least (Amdahl).
- **Rustique and free-threading compound.** Against plain 3.15.1 at 1 thread, enabled
  3.15.1t at 8 threads: http_server 555 → 91 ms (6.1×), compute_pool 5.7×, request_pool
  468 → 127 (3.7×), pipeline_queue 2.8×, shared_cache 2.7×.
- **On the t build Rustique's gain shrinks as threads are added** in request_pool (0.61 →
  0.83) and pipeline_queue (0.55 → 0.72). Both effects are present:
  - Amdahl: the kernels shrink only the parallel part (parsing); the serial producer,
    aggregator and executor bookkeeping stay;
  - the kernel cliffs of §3.1: request_pool deep-copies one *shared* response template
    (`copy.deepcopy` shared input: CPU ×5.9) and calls the `lru_cache`d public `urlsplit`.
  Splitting the two needs CPU-time accounting per stage (quiet-box addendum).
- On the GIL build the gains are flat in the thread count: the GIL serializes everything
  anyway.

## 3. Rustique under threads

### 3.1 Kernel throughput at 1–8 threads

`benches/ft/kernel_scaling.py` on 3.15.1t, plain and `--enable` (fresh process each), 62 cells
(31 operations × per-thread inputs `local` / one input shared by all threads `shared`).
Every thread runs the same K calls; K is calibrated per process for ~0.15 s at 1 thread.
**Two metrics** (**M**, dev box, cores 24–31):

- **CPU time per call** (sum of the workers' `thread_time()` / calls): rises at 8 threads
  only through contention (spinning on locks, cache-line bouncing on shared reference
  counts or statics). A lock that parks its waiter shows in wall time only. This metric is
  insensitive to other processes on the CPU set. Until the pin guard went in, other slices'
  unpinned processes ran on 24–31, and some early wall numbers were wrong.
- Wall-time efficiency (time(1 thread) / time(8 threads), 1.00 = perfect, 0.12 =
  serialized). Final wall curves are re-measured on the quiet box (addendum).

Selected rows (full table: `kernel_scaling.py table` on
`~/rustique-research/w2/threads/ks-3.15.1t-{plain,enabled}.json`). "×8/×1" = CPU per call at
8 threads ÷ at 1 thread; "e/p" = enabled ÷ plain:

| operation | inputs | plain ns (1 thr) | e/p 1 thr | plain CPU ×8/×1 | enabled CPU ×8/×1 | e/p CPU 8 thr | reading |
|---|---|---:|---:|---:|---:|---:|---|
| `datetime.strptime` | local | 3714 | 0.063 | **3.71** | 1.33 | **0.023** | CPython serializes on `_strptime._cache_lock`; the kernel does not take it |
| `date.strftime` | local | 1308 | 0.097 | **7.23** | 1.81 | **0.024** | CPython imports `time.strftime` per call (`PyImport_ImportModuleAttrString`): shared module refcounts |
| `fnmatch.fnmatch` | local | 309 | 0.162 | **7.98** | 1.10 | **0.022** | CPython's `_compile_pattern` `lru_cache` takes its critical section per call; the kernel's cache is per thread |
| `statistics.mean` | local | 7628 | 0.116 | 2.54 | 1.06 | 0.049 | CPython contends in `Fraction` arithmetic (I: `numbers` ABC `isinstance` checks, see the ABC cells) |
| `json.loads` | local | 1457 | 0.439 | 1.47 | 1.04 | 0.311 | |
| `json.dumps` | local | 2122 | 0.229 | 1.44 | 1.05 | 0.166 | |
| `logging.LogRecord` | local | 3277 | 0.410 | 1.54 | 1.06 | 0.282 | |
| `posixpath.join` | local | 490 | 0.200 | 1.07 | 1.08 | 0.201 | no contention either way |
| `textwrap.wrap`, `float.repr`, `x in tuple`, `sorted`, `list.sort`, `min(dict)` | local | | 0.16–0.80 | 1.0–1.1 | 1.0–1.2 | 0.17–0.83 | no contention either way (earlier wall-time "losses" of `sorted`/`list.sort`/`min` were foreign processes on the CPU set) |
| **`logging.Formatter.format`** | local | 1969 | 0.393 | 1.26 | **3.25** | **1.010** | **Rustique contention**: global `FMT_CACHE` (`parking_lot::Mutex::try_lock` + `Arc` clone/drop per call); the whole gain is gone at 8 threads |
| **`urllib.parse.quote`** | local | 1022 | 0.178 | 5.10 | **19.56** | **0.683** | **Rustique contention**: the kernel's `Vec<u8>` growth → glibc `realloc` → arena lock (`__lll_lock_wait_private`, futex, 21.5% of cycles in the kernel's run-queue spinlock; perf call graph) |
| **`x in list(100)`** | shared | 897 | 0.199 | 1.06 | **9.74** | **1.836** | **Rustique contention**: whole-list critical section for len ≥ 4 (`takeover.rs:497`); CPython reads lock-free (small ints are immortal: no refcount traffic) |
| **`list.index(100)`** | shared | 819 | 0.238 | 0.98 | **8.64** | **2.102** | same |
| `copy.deepcopy` | shared | 4286 | 0.223 | 1.57 | **5.93** | 0.840 | per-item critical section on the *shared* dict (`copy/rt.rs:576`) + increfs; gain shrinks from 4.5× to 1.2× |
| `datetime.isoformat` | shared | 409 | 0.166 | 1.55 | **8.30** | 0.894 | extra refcount traffic on the shared receiver |
| `dataclasses.asdict` | shared | 1489 | 0.397 | 3.44 | 7.71 | 0.889 | `__dataclass_fields__`/`Field` increfs per call (`dc.rs:393-441`) |
| `pathlib` `p / 'x'` | shared | 1106 | 0.203 | 3.02 | 7.44 | 0.499 | `Bound::from_borrowed_ptr` incref/decref of the shared path in `locked()` (`pathlib/mod.rs:278,288`) |
| `json.dumps` | shared | 2192 | 0.219 | 4.56 | 9.05 | 0.435 | per-item `Held` increfs on shared values + critical section per container |
| `max(list 100 floats)` | shared | 2061 | 0.055 | **17.80** | 11.21 | **0.035** | CPython increfs every shared float; the kernel locks once and reads |
| `sum(list 100 floats)` | shared | 1064 | 0.195 | 10.30 | 11.08 | 0.210 | |
| `posixpath.join` fallback (`PurePosixPath`) | local | 306 | 0.941 / 1.185 ¹ | 1.60 | 1.61 | 0.951 | ¹ suspension depends on the main thread (§3.2): 1.185 in a run where it was not suspended |
| `random.randint` (module) | local | 190 | 0.415 | 6.12 | 8.50 | 0.577 | one shared `Random` instance: serialized by contract |
| `urllib.parse.urlsplit` | local | 73 | 1.005 | **10.20** | 9.59 | 0.946 | public `lru_cache` wrapper (CPython's, kept for `cache_info()`): serializes |
| `functools.cache` hit | shared | 47 | (no kernel) | 4.87 | | | CPython: the wrapper's critical section per call |
| `lru_cache(128)` hit | shared | 58 | (no kernel) | 7.02 | | | same, plus LRU relinking |
| `re.match(str, s)` | local | 315 | (no kernel) | **6.56** | | | module-level `re` functions: one cached `Pattern` shared by all threads |
| `p.match(s)`, compiled pattern | per-thread `re.compile` (returns the cached, shared object) | 129 | (no kernel) | **7.23** | | | every `Match` takes a strong reference to the shared `Pattern` (`sre.c:2748`): shared-refcount bouncing; eff 0.13 |
| **`isinstance(x, collections.abc.Mapping)`** | local | 162 | (no kernel) | **7.57** | | | fully serialized (eff 0.12): `_abc_instancecheck` → `_in_weak_set` takes the ABC's `_abc_data` critical section, creates a weakref to the class, probes the shared cache set |
| `isinstance(x, numbers.Number)` | local | 170 | (no kernel) | **7.92** | | | same |
| `isinstance(d, dict)` (reference) | local | 25 | (no kernel) | 1.08 | | | scales (eff 0.82) |
| `functools.cache` hit | local / shared | 40 / 38 | (no kernel) | 1.03 / **5.76** | | | a cached function shared by threads serializes on the wrapper's critical section |

Reading:

1. **Rustique already removes several of CPython 3.15t's own serialization points**:
   `strptime`, `strftime`, `fnmatch` and `statistics.mean` run 20–50× faster than plain at
   8 threads, against 6–16× at 1 thread. The kernels don't take `_strptime._cache_lock`, the
   private `lru_cache` critical sections, or the per-call `import time`. This is a gain
   specific to threaded programs, and wave 1 never measured it.
2. **Four real Rustique scaling cliffs**:
   - logging's `FMT_CACHE` lock;
   - Rust heap allocation through glibc `malloc` in hot kernels (`quote`; any kernel that
     grows a `Vec`/`Box` per call is exposed);
   - whole-container critical sections on shared lists (`in`, `index`, `count`): ×2 slower
     than plain;
   - extra reference-count/critical-section traffic on **shared** arguments (deepcopy,
     `asdict`, `isoformat`, pathlib, `json.dumps`): gains shrink, but stay ≤ 1 except
     `x in list`/`list.index`.
3. **CPython 3.15t serialization points with no kernel yet** (extra cells, `ks-extra-plain.json`):
   - ABC `isinstance` (`Mapping`, `Number`, any `abc.ABC`) is **fully serialized** across
     threads: CPU ×7.6–7.9, efficiency 0.11–0.12. It is reachable (§4.2).
   - Shared `functools.cache`/`lru_cache` functions: CPU ×5.8–7.0.
   - Compiled-regex matching on the shared cached `Pattern`: ×7.2, out of reach (shared
     refcount, §5).
   - Public `urlsplit`'s `lru_cache`, and module-level `random` (by contract).

### 3.2 Shared state on kernel hot paths (code audit, verified)

A read-only audit of `crates/rustique-py/src/{accel,targets,takeover.rs,builtins.rs}` for
state that 8 threads calling the same kernel would share (**I** from code, file:line on
`w2/p1-threads`; items marked ✓ verified by reading the code again and/or a measurement):

| item | where | per call? | effect on 3.15t |
|---|---|---|---|
| ✓ net-benefit score `Counters::score` | `accel/stats.rs:149-196`; every vectorcall target (json, copy, dataclasses, logging, random, statistics, textwrap, urllib, posixpath, fnmatch, pathlib) | a load at the ceiling; a **store** on every fallback and on the next hits until the ceiling | one shared line written under mixed hit/fallback traffic (e.g. deepcopy with restarts; `ml_meth`/slot targets such as `tuple.__contains__` keep no score). Not padded: shares lines with the target's `DepFlag`/`Tools` and neighbouring targets' statics |
| ✓ logging `FMT_CACHE` | `targets/logging/format.rs:584-617` | `parking_lot::Mutex::try_lock` + `Arc` clone + drop: 4 atomic RMWs on 2 global lines | `Formatter.format` from N threads bounces the cache line; a failed `try_lock` re-parses the format and allocates its key strings |
| copy `RESTARTING` | `targets/copy/mod.rs:297,334,360,414` | load per deepcopy; `fetch_add/sub` per restart | written only on restarts |
| ✓ whole-container critical sections | `takeover.rs:497,755-763` (`list.__contains__/index/count`, len ≥ 4), `builtins.rs:291,440` (`sum/min/max/any/all` over list/set/dict), `fnmatch.filter`, `statistics` scan | one CS per call | CPython 3.15t reads lists/dicts lock-free with per-item try-incref; the kernels lock the container for the scan: threads scanning the **same** container serialize on its mutex (but avoid the per-item refcount traffic) |
| json `Held` increfs | `targets/json/encode.rs:236-252` | per item | atomic increments on shared inputs (as CPython's encoder) |
| ✓ `Tools` guard (`UnsafeCell`s, `unsafe impl Sync`) | `accel/guard.rs:84-135` | reads per call; writes when the monitoring version changes | a data race in Rust's model; on x86 (TSO) benign; on aarch64 a thread can pair a new `version` with old `states` and cache `busy = false` until the next monitoring change (§3.4 S3) |
| suspension via `Py_AddPendingCall` | `accel/registry.rs:447-455` | once per suspension | pending calls run on the **main thread** only: a vectorcall target that falls back in worker threads while the main thread waits in `join()`/`Event.wait()` keeps paying the vectorcall fallback (+180–260 Ir per call) until the main thread runs bytecode (§3.1 `posixpath.join_fb`) |
| deps resume reinstalls with `PyFunction_SetVectorcall` | `accel/registry.rs:478-489,531` | per `mock.patch` exit | stop-the-world in steady state (ACCELERATION §3.10 says never): ~1 ms with busy threads (M§5), up to `MAX_SUSPENSIONS` = 16 times per target |
| `PyO3` attach | kernels use `Python::assume_attached()` | — | free; no `Python::attach` on hot paths |
| caches that assume the GIL | `logging/caller.rs:96-102`, `list/alloc.rs:36-80`, `list/iter.rs:37-48` | — | correctly `cfg(not(Py_GIL_DISABLED))` |
| datetime format table, fnmatch/json thread-locals, utils `TypeCache` seqlock, copy `RootCache` | various | per call | correct (insert-only CAS tables, thread-locals, seqlock) |

### 3.3 Safety under threads (stress, **M** on 3.15.1t unless noted)

`benches/ft/stress.py` (faulthandler watchdog: a hang dumps every thread and fails) and two
targeted probes (`~/rustique-research/w2/threads/repro/`):

| scenario | load | result |
|---|---|---|
| `toggle` (30 s) | 6 workers × 19 accelerated operations checked against plain results; `disable()`/`enable()` loop; `sys.modules` churn; `gc.collect()` loop | 15.8M calls, 186 cycles, **0 wrong results** (3.15.1: 17.1M calls, 5,337 cycles, 0) |
| `patch` (30 s) | 5 pinned dependencies (`os.fspath`, `json.encoder.encode_basestring_ascii`, `_strptime._strptime_datetime_datetime`, `Formatter.converter`, `copy._deepcopy_dispatch[list]`) patched by one thread; a checker on another thread (Event hand-off) must see the patched object used and, after restore, not used; 6 hammer threads | 38,339 rounds, 78.8M hammer calls, **0 failures** (3.15.1: 441 rounds, 0) |
| `tables` (30 s) | `disable()`/`enable()` loop; writers on copy's globals and json's default encoder `__dict__`; imports; `gc.collect()`; callers | **no hang**; 13,884 activations against 3.5M pinned-dict mutations; **1 `PanicException` out of `enable()`** (S1) |
| `tables_abba.py` (40 s) | `enable(json.dumps)`, then loop activation of `json.loads`; 2 writers on `JSONEncoder`'s class dict; `gc.collect()` | no hang, 173,615 cycles, 78M mutations; 1 escaped panic; `json.dumps` ended **refused** (a concurrent writer during `prepare` turns into a silent refusal) |
| `type_lock_abba.py` (60 s) | activations that watch types against 2 writers on a watched class (TYPE_LOCK + dict section held in the watcher) | no hang, 7,490 cycles, 188M mutations (no production target calls `PyType_Watch` in `activate`: §3.4 S2) |
| `shutdown` | 200 child processes: 6 daemon threads hammering kernels, main thread exits at a random point (half via `sys.exit`) | **0 bad exits** |
| lazy install race (`lazy_abort.py`) | `import copy` (lazy install from the `sys.modules` watcher) while a thread mutates copy's globals | 210 runs, 0 aborts; copy `refused` in ≥ 12 of 60 (safe, silent reach loss) |

### 3.4 Safety findings and fixes

**S1 — `enable()` raises `PanicException` when another thread mutates a pinned module dict
(confirmed, robustness).** Repro `~/rustique-research/w2/threads/repro/enable_panic.py`: a
thread loops `copy.__dict__["_k"] = 1; del copy.__dict__["_k"]`, the main thread loops
`disable(); enable()`: 1 `PanicException("dictionary changed size during iteration")` in
834 cycles on 3.15.1t, 0 in 53,250 on 3.15.1. Source: `accel/guard.rs` `activate()`,
`PendingPin::Dict`, `for (k, v) in d.iter()` (PyO3's iterator checks the size between
`PyDict_Next` calls; on 3.15t each call takes the dict's critical section separately, so a
writer gets in between). It runs **under the `TABLES` mutex and outside `prepare`'s
`catch_unwind`**. The same snapshot iteration in `prepare.rs:612` (`pin_dict`) is inside
`catch_unwind` (`registry.rs:601,679`): there the race ends as a refusal — in a lazy
install of `copy` racing a writer, copy was `refused` in ≥ 12 of 60 runs and no run aborted
(`repro/lazy_abort.py`, 210 runs in all), i.e. a silent reach loss, safe. Note: on 3.15.1t one
`disable()+enable()` cycle takes ~12 ms with one busy thread (stop-the-world
`PyFunction_SetVectorcall` per vectorcall site), 0.19 ms on 3.15.1.

**S2 — lock-order inversions around `TABLES` (latent today; reachable as soon as a target
pins a class attribute or two targets snapshot the same dict).** `activate()` holds `TABLES` (a `std::sync::Mutex`) while it
calls `d.iter()`/`d.contains()` on pinned dicts (on 3.15t these take, or may take on a
read conflict, the dict's critical section; `PyDict_GetItemRef` falls back to it when a
lock-free read fails). `on_dict_event` runs while the mutating thread holds that dict's
critical section and blocks on `TABLES` **without detaching**. Interleaving: A takes
`TABLES`, B takes dict D's section and enters the watcher (waits for `TABLES`), A needs D's
section → deadlock. Second form: A detaches inside the `TABLES` section (any contended
`PyMutex` slow path) while a third thread requests stop-the-world (GC, `SetVectorcall`):
the world waits for B (attached, blocked on `TABLES`), B waits for A, A cannot re-attach
until the world resumes. Third form (verified in CPython's source): `type_setattro` holds
`BEGIN_TYPE_DICT_LOCK` (the global `TYPE_LOCK` + the type dict's mutex) while the dict watcher
runs, so `on_dict_event` waits for `TABLES` holding `TYPE_LOCK`; `activate` calls
`PyType_Watch` (which takes `TYPE_LOCK`) under `TABLES`. Why no hang in ~195,000 racing
activations (§3.3): today only the hidden `_selftest` group uses `pin_attr` (the only pin that
calls `PyType_Watch`), every snapshot-pinned dict is pinned by one group (copy, fnmatch, json
dumps / loads use distinct dicts), and a pinned dict is snapshotted *before* it is watched.
`PyDict_GetItemRef` falls back to the dict's lock only when a lock-free read races a
modification that is past its watcher call. So: not reachable with the 84 targets of wave 1,
reachable by construction for the first wave-2 target that pins a class attribute (json
`cls=`/`default=` encoder subclasses, logging `Formatter` subclasses). Fix 2 below is required
before such a target lands.

**S3 — `Tools` race on weak memory (theoretical, aarch64).** §3.2.

**S4 (low)** — `posixpath` `cwd_units` calls the cached `os.getcwd` without a reference
while `release_cell` may drop it (`posixpath/mod.rs:551-566`); safe today because
`os.__dict__` keeps the function alive until finalization, when daemon threads are parked.

**Fixes (Phase 2, accel):**

1. *Snapshot pinned dicts privately, outside `TABLES`*: in `prepare` (`pin_dict`) and in
   `activate`, take `PyDict_Copy(d)` (one consistent copy under the dict's own critical
   section) and compare/iterate the private copy; acquire `TABLES` only afterwards, for pure
   Rust bookkeeping. Equivalent: iterate with `PyDict_Next` inside one
   `PyCriticalSection_Begin(d)` — but never while holding `TABLES`.
2. *Lock order*: `TABLES` is a **leaf** lock. While holding it, call nothing that can take a
   per-object critical section, detach, allocate Python objects or run Python code
   (`PyDict_GetItemRef`, `PyDict_Watch`, `PyType_Watch`, `d.iter()`, `Py_DECREF` that may
   free): gather everything first, then lock. Watcher callbacks (dict section held →
   `TABLES`) then define the only order. Add a debug assertion: a thread-local "holding
   TABLES" flag checked at every CPython call site in `guard.rs` (and in `on_dict_event`: a
   watcher firing on the thread that holds `TABLES` is a bug).
3. *No panic escapes `enable()`*: wrap `activate` + site install per target in
   `catch_unwind`, roll back the owner's pins (`deactivate(owner)`) and turn the panic into
   a refusal with category `other` and the panic message as reason.
4. *Suspension from worker threads*: when `request_suspend` fires on a non-main thread,
   perform the resync there (the kernel is attached and not inside a watcher), or
   from the next `enable()`/import hook, instead of waiting for the main thread's pending
   calls.
5. *Deps resume without steady-state stop-the-world* on t builds: keep the kernel installed
   while a pin is broken (the per-call `DepFlag` load already routes to the original) and
   suspend only after the pin stays broken for N calls; resume at `enable()`.
6. *`Tools`*: `AtomicU64` version with Acquire, recompute `busy` from `states` and re-check
   the version before publishing `seen` (Release).
7. *Contention* (from §3.1): logging `FMT_CACHE` → an insert-only lock-free table handing out
   `&'static` entries (as datetime's `fmtcache`); pad `Counters` (and `DepFlag`/`Tools`) to
   128 bytes; whole-container scans of `list`/`builtins` (see §3.1 for when locking beats
   per-item increfs).

**Stress tests for the Phase-2 suite**: `stress.py tables` (enable/disable loop + writers
on copy's globals, json's default encoder `__dict__`, `sys.modules` churn, `gc.collect()`
loop, deepcopy/json callers; asserts no exception out of `enable()` and no hang within the
watchdog) catches S1 within seconds; S2 needs the lock-order assertion of fix 2 to be
caught deterministically (a timing race otherwise). `stress.py toggle`, `patch`,
`shutdown` cover enable/disable under load, pins patched from other threads, daemon threads
at exit.

## 4. What Rustique can erase: candidates

### 4.1 Prototype: `set.__reduce__` / `frozenset.__reduce__` (`crates/rustique-py/src/proto/set_reduce.rs`)

CPython's own body (`PySequence_List`, `PyTuple_Pack`, `_PyObject_GetState`, `PyTuple_Pack`)
without the clinic wrapper's outer critical section. The only lock on the set is
`PySequence_List`'s own (list, set) section, now uncontended. Calling `PySequence_List`
itself, rather than filling a pre-sized list, keeps the list's capacity identical
(`sys.getsizeof` of `args[0]` is CPython's). `_PyObject_GetState` keeps the single
`__getstate__` lookup a user `__getattribute__` would see. All **M**:

| check | result |
|---|---|
| differential, 2,000 generated sets/frozensets/subclasses (`__dict__`, `__slots__`, custom `__getstate__`, `__iter__`, logging `__getattribute__`, raising `__getstate__`) + `deepcopy`/`copy`/`pickle` protocol 2 of each, `PYTHONHASHSEED=0` (`set_reduce_proto.py check`) | identical digest plain vs `enable(only=set)`, on 3.15.1 and 3.15.1t |
| 3.15.1 (GIL) callgrind, Ir per call, enabled/plain (`set_reduce_proto.py cells`) | `set()` 0.993, `{1..5}` 0.996, `set(range(100))` 0.997, `frozenset` 0.996, subclass with `__dict__` 0.997, `deepcopy(frozenset)` 1.001, `deepcopy(set)` 0.998, `pickle.dumps(set, 2)` 1.008 |
| 3.15.1t wall ns per call, enabled/plain (`set_reduce_proto.py time`) | `s.__reduce__()` 39,036 → 794 ns (**0.020**), `set(range(100))` 0.027, subclass 0.004, `deepcopy(frozenset)` 44,044 → 3,258 ns (**0.074**), `deepcopy(set)` 0.069 |
| 3.15.1t isort_stdlib, `--only set` / all groups but copy | 1192 → 310 ms (**0.260**) / 0.273 (`run.py compare -n 3 -p 3`) |
| 3.15.1t state_snapshot, `--only set` / all groups but copy | 1402 → 965 ms (**0.689**) / 0.703 |

`pickle.dumps(set)` itself never reaches `__reduce__` (`_pickle.save_set` handles exact sets at
every protocol); set *subclasses* do.

### 4.2 Candidate rows (format of the brief)

Shares are of 3.15.1t run time unless noted.

| candidate | layer | entry points + mechanism | share mean / max | s on the part | exact subset & fallback | floor vs current | risks | upstream overlap | reach | prototype | evidence |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **set/frozenset `__reduce__` without the self-contended section** | runtime (FT) | `ml_meth` of `set.__reduce__`, `frozenset.__reduce__` | t build: 3.1% mean of 34 (isort 74%, state_snapshot 31%, 0 elsewhere); GIL: 0 | ~50× per call | all receivers (CPython's body; no fallback needed); errors are CPython's own | 0.7 µs vs 34.5 µs | none found (differential + subclass hooks); version-gate per patch release once fixed upstream | **high** (one-line upstream fix; U 0.5: released patches keep the bug) | sets deep-copied or pickled on t builds (common in config/AST/domain models) | done, §4.1 | §1.2, §4.1 |
| **FT scaling fixes in existing kernels** | runtime (FT) | logging `FMT_CACHE` → insert-only lock-free table; Rust heap → per-thread allocator (mimalloc or `PyMem_RawMalloc`-backed) or per-thread scratch; list `in`/`index`/`count` on lists not owned by the calling thread → per-item `PyList_GetItemRef` reads, no whole-list lock; arguments as `Borrowed`, one critical section per container | threaded programs only: Formatter.format and quote lose 60–100% of their gain at 8 threads; `x in shared list` 1.8–2.1× slower than plain | restores the 1-thread ratio at 8 threads (Formatter.format 1.01 → ~0.39, quote 0.68 → ~0.18, `x in` shared 1.84 → ≤ 0.3) | unchanged | n/a | allocator swap: memory accounting (`tracemalloc` does not see Rust heap either way) | none | every threaded program using those kernels | none yet | §3.1 |
| `functools.cache` (unbounded) hits without the wrapper's critical section | language / runtime (FT) | type slot `tp_call` of `functools._lru_cache_wrapper` (heap type of `_functools`), hit path only | single-threaded ~0 (CS uncontended); shared cached function at 8 threads: CPU ×4.9 | 3–5× at 8 threads on hits | `maxsize=None` only (bounded LRU hits relink: observable eviction order → original); exact `hits` count (atomic add); misses → original | ~10 ns vs 47 ns + contention | `cache_info()` counters must stay exact; `cache_clear` concurrency | medium (FT lru_cache work upstream) | `@cache` is common in libraries; `@lru_cache` (bounded, default 128) more common and **not** covered | none | §3.1 |
| **ABC `isinstance` positive-cache hits** | language (FT) | `ml_meth` of `_abc._abc_instancecheck` (called by `ABCMeta.__instancecheck__`) | single-threaded: small (the Python `__instancecheck__` frame stays); threaded: fully serialized today, CPU ×7.6–7.9 at 8 threads (`isinstance(x, Mapping)` 162 → 1226 CPU ns) | ~5–8× at 8 threads on hits (I: removes the CS, the weakref and the shared-set probe) | instances whose type keeps `object.__getattribute__` and its own `__class__` (so `__class__` is `type(x)`), positive-cache hits recorded by a lock-free table keyed by (ABC, class, `tp_version_tag`) after the original returned True with the class in `_abc_cache`; misses, negative results, registry changes → original. An epoch bump on `_abc._reset_caches` / `_reset_registry` (their `ml_meth`s) clears the table | lock-free table read vs CS + weakref + set probe | class freed and address reused (version tag check), `_abc_caches_clear`, ABCs with a custom metaclass `__instancecheck__` (not reached) | medium (FT ABC work upstream likely) | very high: collections.abc/numbers/typing checks in libraries and frameworks (overlap: `w2-machinery`, language layer) | none | §3.1 extra cells; `Modules/_abc.c` `_in_weak_set` |
| `re` module functions with str patterns | stdlib (FT) | (`re._compile` vectorcall) | CPU ×6.5 at 8 threads | — | — | — | returned Pattern identity, cache eviction order | — | high | none: **I**: the contention is reference counting on the one shared cached `Pattern` and its dict entry, which any exact kernel must keep (same object returned) → **out of reach** | §3.1 |

Lead-score estimates (`I × F × (1 − K) × (1 − U) × (0.5 + R)`), t build:
- set `__reduce__`: I = 3.1 × 0.98 ≈ 3.0, F 0.95, K 0.05, U 0.5, R 0.3 → **1.08**.
- FT scaling fixes: not scored by I. They are contract work: "never slower" must hold at 8
  threads too. Today they violate it on shared lists.
- `functools.cache`: I ≈ 0 on single-threaded workloads, F 0.6, K 0.2, U 0.4, R 0.4. Measure on
  the threads family first.
- ABC `isinstance`: I ≈ 0 on single-threaded workloads. On threaded programs that check
  ABCs per request (pydantic/typing-heavy services), plausibly the largest single
  serialization point. F 0.5, K 0.3, U 0.4, R 0.9. Worth a Phase-2 measurement on the
  framework services under threads.

## 5. Settled / out of reach

- **The interpreter tax of the t build** (specialized instructions 3–8% slower, stack
  references, atomic inline-cache reads): out of reach for kernels, because it is in the
  program's own bytecode. A kernel removes it only for the bytecode it replaces, as for any
  target. Upstream work (3.16/3.17 FT specialization) owns it.
- **Shared and merged reference counts on user objects** (`_Py_MergeZeroLocalRefcount`,
  `_Py_DecRefShared`): inherent to biased reference counting. A kernel avoids them only for
  the temporaries it creates itself.
- **The free-threaded GC**: a runtime-policy question (`w2-runtime`), not a kernel one. The
  t build's collector is cheaper than the GIL build's on 3 of 12 profiled workloads and
  costs up to +149 ms (coverage_pyflakes) on others (§1.3). Any GC policy tier needs its
  evidence measured separately on the t build.
- **mimalloc**: not a penalty (−5 to −36 ms per run against pymalloc).
- **Locks the program takes itself** (`threading.Lock` around a shared dict, the logging
  `Handler` lock, `queue.Queue`): their semantics are the program's. Out of reach.
- **Module-level `random` functions** share one `Random` instance whose stream position is
  public state (ACCELERATION §2.5). Calls must serialize (CPython takes the instance's
  critical section, and the kernel does too, `random/mod.rs`). Out of reach by contract.
  `Random()` instances per thread scale (§3.1).
- **Compiled-regex matching across threads** (`p.match(s)` on a shared `Pattern`, CPU ×7.2 at
  8 threads): every `Match` holds a strong reference to the `Pattern` (`m.re is p`), so the
  shared reference count bounces between cores. An exact kernel must return the same
  object graph. Out of reach. Upstream remedy: deferred reference counting for compiled
  patterns.
- **`set.__reduce__`**: once the upstream fix lands, the kernel becomes unnecessary on those
  patch releases. Keep it version-gated to the releases that have the bug (fingerprint of
  the clinic wrapper's critical section is not observable; gate on `major.minor.micro`).

## 6. Recommendation

1. **Harden `accel/` for free-threading before any wave-2 target pins a class attribute**
   (fixes 1–6 of §3.4): private snapshots outside `TABLES`, `TABLES` as a leaf lock with a
   debug assertion, no panic out of `enable()` (refusal + rollback), suspension performed by
   the thread that requests it, no stop-the-world on dependency resume, atomic `Tools`.
   Small, contained, and it closes the only reachable safety defect (S1) plus the latent
   deadlock (S2). Add `stress.py tables` and the two probes to the FT suite.
2. **Ship the `set/frozenset.__reduce__` kernel on t builds** (prototype ready, exact by
   construction, ≤ 1.008 on GIL cells). Gate it to the patch releases that have the bug, and
   document the upstream one-liner. It removes the only pathological t-build penalty found
   in 34 + 5 workloads for every caller, not only Rustique's deepcopy.
3. **Make "never slower" hold at 8 threads.** Add `kernel_scaling.py` (CPU time per call) to
   the benchmark obligations of ACCELERATION §7.4: for per-thread inputs, enabled CPU ×8/×1
   ≤ 1.3; for shared inputs, enabled ≤ 1.05 × plain at 8 threads. Then fix the four cliffs of
   §3.1: logging `FMT_CACHE`, Rust heap allocation in hot kernels, whole-list locks on shared
   lists, refcount/lock traffic on shared arguments.
4. **Count the threaded gain.** strptime/strftime/fnmatch/statistics kernels are 20–50×
   faster than plain at 8 threads because they skip CPython 3.15t's own serialization
   points. Report the threads family in the scored set (§2).
5. **Not Rustique's to erase**: the general t-build tax (1.068× geomean without the bug),
   which is the interpreter, reference counting, dict/attribute paths and GC (§1.3, §5);
   uvicorn's 1.29× (a sync dependency's thread hop, worse when cores are oversubscribed);
   module-level `random`, explicit user locks, refcounts on shared compiled regexes. Hand
   the free-threaded GC numbers (§1.3: +149 ms to −437 ms) to `w2-runtime`.
6. **New threaded-program targets, measured first on the framework services under threads**:
   - ABC `isinstance` positive-cache hits (`_abc._abc_instancecheck`): fully serialized on
     3.15t today, very high reach (collections.abc/numbers/typing checks in libraries).
     Overlaps `w2-machinery`'s language layer.
   - `functools.cache` hits (unbounded caches only).
   Both erase CPython 3.15t serialization points the way the strptime/fnmatch kernels
   already do.

## 7. Reproduce

Dev box, `R=~/rustique-research/w2/devbox/rbox`, workspace pinned to one L3 domain (8 cores);
builds `$R w2-threads build --py 3.15.1` and `--py 3.15.1t`; wave-1 workload requirements
installed into both venvs. Raw outputs: `~/rustique-research/w2/threads/`.

```bash
# §1.1 sched_yield/futex per steady-state run, every workload (sudo perf tracepoints)
$R w2-threads run --py 3.15.1t -- benches/ft/syscall_sweep.sh out.csv -n 1 -- $(python benches/workloads/run.py list)
# §1.2 the set.__reduce__ cost on any build
python -c "import timeit; s={1,2,3,4,5}; print(min(timeit.repeat('s.__reduce__()', globals=globals(), number=20000, repeat=5))/20000*1e9, 'ns')"
# §1.3 steady-state profiles on both builds, then the per-category diff
benches/ft/penalty_batch.sh <outdir> 3 data/state_snapshot devtools/isort_stdlib ...   # once per build
python benches/ft/ftdiff.py <w>-3.15.1.data <w>-3.15.1.times.json <w>-3.15.1t.data <w>-3.15.1t.times.json --json <w>.ftdiff.json
python benches/ft/ftdiff.py table *.ftdiff.json
# §2 workload scaling (per build), then tables
python benches/ft/scaling.py run threads -t 1,2,4,8 -p 3 -n 3 -o scaling-<build>.json
python benches/ft/scaling.py table scaling-3.15.1.json scaling-3.15.1t.json
# §3.1 kernel scaling (3.15.1t), then the comparison
python benches/ft/kernel_scaling.py -o ks-plain.json; python benches/ft/kernel_scaling.py --enable -o ks-enabled.json
python benches/ft/kernel_scaling.py table ks-plain.json ks-enabled.json
# §3.3 stress
python benches/ft/stress.py toggle|patch|tables --secs 30; python benches/ft/stress.py shutdown --runs 200
# Quiet-box addendum (final 8-thread wall curves), end to end; QUICK=1 for a 2-minute smoke run
PY_GIL=<venv-3.15.1>/bin/python PY_FT=<venv-3.15.1t>/bin/python OUT=<dir> CORES=<8 cores, one L3> benches/ft/quietbox.sh all
# §4.1 set.__reduce__ prototype: differential check, GIL-build callgrind cells, t-build timings
python benches/ft/set_reduce_proto.py check|cells|time
python benches/workloads/run.py compare devtools/isort_stdlib data/state_snapshot -n 3 -p 3 --only set
RUSTIQUE_EXCLUDE=copy python benches/workloads/run.py compare devtools/isort_stdlib data/state_snapshot -n 3 -p 3
```
