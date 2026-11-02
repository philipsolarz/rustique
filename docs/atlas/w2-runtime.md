STATUS: wave 2, Phase 1 slice `w2-runtime`, 2026-11-02. Branch `w2/p1-runtime`. Every number is
**measured** (M: how and where is stated) or **inferred** (I: from what). "Dev box" = the shared
c7a.8xlarge (10 agents, load average 25–50 during these runs: wall times there are noisy, counts
are not); "quiet box" = the bench box. Raw data: `~/rustique-research/w2/runtime/out/`; tools:
`benches/runtime/` (README in each module's docstring).

# Runtime: allocation and cyclic GC — where the time goes, what a policy tier can take

## 0. The answer

1. **The runtime layer is 21% of steady-state time** on the 34 wave-1 workloads (M, perf, 3.15.1:
   allocation 6.0%, deallocation 7.4%, cyclic GC 7.8%; 3.15.1t: 6.9 / 6.4 / 6.8%). Allocation
   and deallocation are out of reach except through kernels, and the wave-1 kernels already
   cut allocations + freelist pops by 10–64% on 14 workloads (Py_STATS). `PYTHONMALLOC=mimalloc`
   on the GIL build is 1.051× *slower* with +7.5% RSS: settled.
2. **Automatic GC is 6.9% mean but concentrated**: eight workloads hold 89% of it, and the part
   a time-for-nothing policy can take — full collections that find < 1% garbage — is **1.6%
   (GIL) / 1.9% (FT)** of mean run time, up to 34% (billing_engine) and 21.7% (request_pool, t).
   Explicit `gc.collect()` (1.3 pp, all pytest_networkx) is untouchable and counted apart.
3. **Prototype `benches/runtime/gcadapt.py` (public `gc` APIs only)**: defers full collections
   while they find nothing, never lets one exceed 1.1× the largest legitimate full collection
   (pause cap), leaves young collections and explicit `gc.collect()` alone, steps aside on the
   program's own GC tuning. Whole set: **GC work 0.886× (3.15.1) / 0.772× (3.15.1t)**, est.
   0.5% / 1.0% of mean run time; billing 0.53× / 0.31× GC work (0.71× wall); peak RSS ≤ 1.03×,
   largest collection ≤ 1.05× on all 34; adversarial worst cases ≤ 1.04× RSS. Without the cap
   the same policy holds 1.45× RSS when garbage arrives after the back-off (§4.3).
4. **Default eligibility**: (b), (c), (d) met on every dev-box cell but three borderline ones (pipeline_queue RSS 1.10×, uvicorn +1.4 MB over 20 iterations, FT adversarial `steady` collection 1.11×); (a)
   met under a precise "documented semantics" definition (§5.1) with two declared deviations
   (`get_threshold()` reports configured, not in-force, thresholds; one extra `gc.callbacks`
   entry). The strict reading of (a) is impossible for any timing change — and Rustique's
   kernels already change collection timing (state_snapshot: 94 → 3 young collections/run).
   Quiet-box wall/pause-ms/RSS confirmation is scripted (`run_quiet.sh`) and pending.
5. **Exit-time GC** (w2-attribution's 40.6% of a pylint process) is the shutdown collections
   after module teardown (`pylifecycle.c:1948, 1991`), 0.15–1.33 s per CLI process: not
   reachable exactly; an opt-in "fast exit" (`gc.freeze()` at the last atexit slot) cuts
   pylint's exit 1330 → 17 ms at the price of not finalizing cyclic garbage at exit (§4.6).
6. **Recommendation**: ship R1 as an opt-in tier now; make it the default on the free-threaded
   build first after the quiet-box run and a DECISIONS entry adopting §5.1's definition of (a);
   fast exit opt-in only; stop allocator and freeze/threshold work.

## 1. Method and tools

| question | tool (benches/runtime/) | what it measures | where |
|---|---|---|---|
| time inside collections, per generation, pauses, yield, peak RSS | `gcprobe.py one/sweep` | `gc.callbacks` start/stop + 3.15's C-measured `info["duration"]`; `candidates` (objects examined) and `collected` per collection; explicit `gc.collect()` calls flagged apart (the probe wraps `gc.collect`; policies keep the original); peak RSS per run = `VmHWM` reset through `/proc/self/clear_refs` before each run; policy installed before the workload is imported | dev box, 3 processes × 5 steady-state `run()`s per cell, cells of one round interleaved |
| tables | `gctable.py base/policies/scoreboard` | per-workload GC table; policy vs default (paired per round); whole-set scoreboard with the deterministic proxies below | local |
| alloc / dealloc / GC split | `allocsplit.py`, `splittable.py` | plain `perf record -g -F 1999` (no `-X perf`, so no trampoline distortion), gated by a perf control FIFO to the measured `run()`s; a sample is GC if a collection frame is on its stack, else alloc/dealloc if an allocation/deallocation frame lies between the leaf and the innermost interpreter frame | dev box; cross-check: perf's GC share matches the callbacks' within 0.5 pp on 33/34 workloads |
| allocation counts, freelist hits, `tp_traverse` visits | `pystats_probe.py` | CPython 3.15.1 built `--enable-pystats` (`/work/w2-runtime/py-pystats`), one steady-state `run()` between `sys._stats_on()`/`_stats_off()`; plain and with `rustique.enable()` (wave-1 wheel, dev-release) | dev box (counts are exact) |
| GC policies | `gcpolicy.py` (static and oracle policies), `gcadapt.py` (the prototype) | see §4 | |
| worst cases | `adversarial.py` + `advtable.py` | programs whose garbage cycles appear after a policy has backed off; garbage lifetime measured by weakref callbacks on sampled cycles | dev box |
| eligibility | `test_gcadapt.py`, `policy_suites.py` | criteria (a) and (d): unit tests per API and per stepping-aside trigger; CPython's own test modules plain vs with the policy installed before `test` is imported | dev box, both builds |
| quiet-box run | `run_quiet.sh`, `quiet_driver.sh` | the final wall/pause/RSS numbers (§4.5), one L3 domain per build | bench box |

Phase 0's quiet-box GC baseline (`w2-phase0/benches/results/w2-phase0/gcstats*`) was not
available when this was written; the dev-box baseline here uses the same method
(`gc.callbacks` start/stop) and will be cross-checked against it.

**Deterministic proxies.** On the dev box a cell where the policy changed nothing still swung
0.5–2.0× in wall time (M: e.g. sqlglot under a policy whose collections were identical,
`policies1`). Policies are therefore ranked on quantities the box load cannot move:
**GC work** = objects examined by automatic collections per run (the collector's cost is
~linear in it: 20–60 ns per object in cache-warm young collections, 250–600 ns in full
collections, up to ~1 µs when most of what is examined is garbage to free; M, §3.1), **collection size** = objects
examined by one collection (the pause proxy), **peak RSS**. Wall time and pause milliseconds
come from the quiet box.

## 2. CPython 3.15's collector and allocator, from the source

Paths are relative to the CPython 3.15.1 tree (`~/src/cpython-3.15.1`, box
`/work/cpython/v3.15.1`); `[3.14]` = 3.14.9. Checked against the running interpreters where
marked (✓).

### 2.1 What changed from 3.14

| item | 3.14.9 | 3.15.1 |
|---|---|---|
| GIL-build collector | generational (3.14.0–3.14.4 shipped an incremental GC; reverted in 3.14.5 "due to … significant memory pressure in production", `Doc/whatsnew/3.15.rst:1253-1259`, gh-142516/148726) | the same generational collector (`Python/gc.c`, NEWS `3.15.0b1.rst:308-310`) |
| FT collector trigger | count rule **plus** an RSS-growth deferral (`[3.14] Python/gc_free_threading.c:1945, 2084-2120`) | RSS deferral reverted: "RSS does not immediately decrease after cyclic garbage is freed since mimalloc defers returning memory" (`Misc/NEWS.d/3.15.0b1.rst:148-153`) |
| callback `info` / `gc.get_stats()` keys | `generation, collected, uncollectable` / `collections, collected, uncollectable` | adds `candidates` (objects examined) and `duration` (s) (`Python/gc.c:1272-1277`, `Modules/gcmodule.c:371-411`) |
| per-collection ring buffer | – | `ts_start/ts_stop/heap_size` per collection, exposed only to `_remote_debugging` and `_testinternalcapi` (`Include/internal/pycore_interp_structs.h:180-203`) |
| FT raw-domain allocator | malloc | mimalloc (`Doc/whatsnew/3.15.rst:1991-1994`) |
| freelists | 19 | + complexes, bytes_writers (`Include/internal/pycore_freelist_state.h:11-33`) |

Upstream's own lesson from 3.14 is the strongest prior for this slice: a GC that traded memory
for time was reverted within five patch releases.

### 2.2 GIL build: the generational collector (`Python/gc.c`)

| question | answer (citation) |
|---|---|
| generations, defaults | 3 generations + a permanent one; thresholds (2000, 10, 10) (`pycore_interp_structs.h:273-279`) |
| what counts toward a young collection | every GC-type **allocation** increments `generations[0].count` (`_PyObject_GC_Link`, `gc.c:1980-1999`); every GC-type free decrements it, clamped at 0 (`PyObject_GC_Del`, `gc.c:2125-2127`). Freelist reuse bypasses both (`tupleobject.c:46`, `listobject.c:269`, `dictobject.c:939`). Untracked tuples still count (✓: 1000 one-tuples → count0 +1002, heap +1) |
| when it runs | `count0 > threshold0 && enabled && threshold0 != 0 && !collecting && !PyErr_Occurred()` sets the eval-breaker bit (`gc.c:1991-1997`); handled at RESUME, JUMP_BACKWARD and the end of CALL (`ceval_macros.h:541-547`, `bytecodes.c:220/3544/4457`) and in `PyErr_CheckSignals` (`Modules/signalmodule.c:1775-1777`): native code can trigger a collection |
| which generation | the oldest `g` with `count[g] > threshold[g]` (`gc_select_generation`, `gc.c:1316-1365`), except that generation 2 also needs `long_lived_pending >= long_lived_total / 4` (`gc.c:1356-1357`, the 2008 quadratic-behaviour fix) |
| what a collection of `g` traverses | generations `0..g` merged (`gc.c:1501-1503`); `update_refs`, `subtract_refs` (one `tp_traverse` per object), `move_unreachable` (a second `tp_traverse` per reachable object) over that list only (`deduce_unreachable`, `gc.c:1169-1232`). Older objects are never traversed: they act as roots (`visit_decref` touches only COLLECTING objects, `gc.c:450-454`) |
| survivors | merged into `g+1`; a gen-1 collection adds survivors to `long_lived_pending` (`gc.c:1520-1521`); a full collection sets `pending = 0`, `total = survivors` (`gc.c:1533-1534`); tuples whose items are all untracked are untracked on survival (`untrack_tuples`, `gc.c:664-676`) and are never tracked at creation by `PyTuple_Pack`/BUILD_TUPLE when no item is a GC type (`tupleobject.c:165-169, 461-484`) |
| dicts | always tracked, never untracked (GH-127010; comment `gc.c:1526-1532`) (✓ `gc.is_tracked({})` is True; the docs example `Doc/library/gc.rst:209-212` is stale) |
| cost model | a full collection costs ~2 `tp_traverse` calls + 3 list passes per tracked object; on the workloads 250–600 ns per object examined in full collections, 20–60 ns in young ones (M: billing 367 ms / 1.31 M, pylint 637 ms / 1.09 M, billing gen 0 20 ms / 0.68 M): it is memory-bound |
| freelists | cleared only by a full collection (`gc.c:1616-1618` → `Objects/object.c:927-958`) |
| `gc.collect(g)` | skips `gc_select_generation` (no 25% rule), **runs even when GC is disabled** (`gcmodule.c:93`, ✓), returns collected + uncollectable (`gc.c:1655`); the C API `PyGC_Collect` returns 0 when disabled (`gc.c:1794-1808`) and is what `Py_FinalizeEx` calls (`pylifecycle.c:2480`) |
| `gc.freeze()` | merges generations 0–2 into the permanent generation and zeroes the three counts, leaving `long_lived_total` stale (`gc.c:1742-1749`, ✓); `unfreeze()` merges it into generation 2 without touching `pending` (`gc.c:1752-1756`) |
| frozen objects and the documented APIs | **absent from `gc.get_objects()`** (it walks generations 0–2 only, `gc.c:1711-1733`) and from `gc.get_referrers()` results (`gc.c:1693-1708`, ✓); `is_tracked()` still True; never collected while frozen |
| callbacks | invoked outside the timed window, before `start` and after `stop` (`gc.c:1475-1482, 1649`); `start` gets zeros |

### 2.3 Free-threaded build (`Python/gc_free_threading.c`)

| question | answer |
|---|---|
| generations | none in practice: `_Py_RunGC` always collects "generation 0" (`:2729`) and **every collection walks the whole heap** (mimalloc heap walk `gc_visit_heaps`, `:369-434`; `InternalDocs/garbage_collector.md:386-387`). The generation number only selects counters and the callback's `generation` |
| trigger | per-thread allocation counts flushed every 512 allocations (`LOCAL_ALLOC_COUNT_THRESHOLD`, `:46`, `record_allocation` `:2022-2041`); collect when `count > threshold0` **and** `count >= long_lived_total / 4` (`gc_should_collect`, `:2000-2019`); `threshold1 == 0` forces immediate collection (a test hook); threshold2 unused |
| phases | stop the world; merge per-thread refcounts; process delayed (QSBR) frees; "mark alive" from roots (skipped once `gc.freeze()` was used); `deduce_unreachable_heap`; restart the world for finalizers; stop again for resurrection checks and weakref clearing (`:2069-2185`) |
| freelists | every collection clears every thread's freelists (`:2159`, `:2277`) |
| `freeze` / `get_freeze_count` / `set_threshold` | stop-the-world heap walks / a stop-the-world (`:2467-2536`, `gcmodule.c:167-176`) |
| `get_count()` | `young.count` plus only the **calling thread's** unflushed buffer (`gcmodule.c:222-248`) |

### 2.4 Allocators and freelists

| item | GIL build | FT build |
|---|---|---|
| default object/mem allocator | pymalloc (`obmalloc.c:436-439`): 32 size classes 16–512 B, 16 KiB pools, 1 MiB arenas (`pycore_obmalloc.h:137-240`); > 512 B → malloc | mimalloc, per-thread heaps (MEM, OBJECT, GC, GC_PRE; `pycore_mimalloc.h:12-66`), QSBR-deferred page freeing (`obmalloc.c:164-256`) |
| `PYTHONMALLOC=mimalloc` | accepted when built `--with-mimalloc` (python-build-standalone 3.15.1 is: ✓, `_testinternalcapi.get_configs()` reports allocator 7) — switches all three domains to `mi_malloc` (`obmalloc.c:854-875`) | only `mimalloc[_debug]` / `debug` accepted |
| when it is chosen | preconfig, before `site` runs (`preconfig.c:705-725, 936-941`); once initialised `PyMem_SetAllocator` may only *wrap* the current allocator (`Doc/c-api/memory.rst:549-561`): a `.pth`-activated tier cannot switch allocators |
| freelists (max entries) | tuples 2000 per size 1–20, lists 80, dicts 80, dict keys 80, floats 100, ints 100, complexes 100, slices 1, ranges 6, list/tuple iterators 10, contexts 255, async gens 80, futures 255, pycfunction/pycmethod 16, method objects 20, … (`pycore_freelist_state.h:11-33`); per interpreter (GIL) / per thread (FT) | |
| `Py_STATS` | `--enable-pystats` counts allocations by size, frees, freelist pops/pushes (all freelists summed), and per generation collections / `tp_traverse` visits / objects collected (`Include/cpython/pystats.h:74-110`) | |

### 2.5 What the documentation promises about collection timing

- "An implementation is allowed to postpone garbage collection or omit it altogether — it is a
  matter of implementation quality … as long as no objects are collected that are still
  reachable." (`Doc/reference/datamodel.rst:61-64`); "Do not depend on immediate finalization"
  (`:68-75`).
- `gc.rst:132-136` documents the young/middle trigger rule in terms of the thresholds, and
  defers the oldest generation to `InternalDocs` ("things are a bit more complicated").
- `gc.rst:98` / `:308`: the `get_stats()` / callback key sets "may change"; they did in 3.15.

So collection *timing* is explicitly unspecified; the thresholds, the enabled flag, the frozen
set, the contents of `gc.garbage` and what `get_objects()` returns are documented state.

## 3. Where the time goes

### 3.1 The runtime layer per workload (34 wave-1 workloads)

Columns: perf split of the steady-state `run()`s (M, dev box, 3.15.1 and 3.15.1t, ~1.5–10 k
samples each); automatic collections per run by generation (count × / ms, M, `gcprobe`, 3.15.1,
median of 15 runs); explicit `gc.collect()` time per run; objects collected / examined by
automatic collections and their ratio (yield); largest automatic pause (ms, noisy); peak RSS;
`Py_STATS` allocations per run (`PyObject_Malloc` + `PyMem_Malloc` calls), the share of object
creations served by freelists (pops / (pops + allocations)), and allocations + freelist pops
with Rustique's wave-1 kernels enabled ÷ plain (M, Py_STATS build).

workload | 3.15.1 alloc / dealloc / GC % | runtime % | 3.15.1t alloc / dealloc / GC % | runtime %t | automatic GC: gen0 / gen1 / gen2 per run (ms) | explicit gc.collect() ms/run | collected / examined per run | yield % | largest pause ms | peak RSS MB | allocations per run (Py_STATS) | freelist share % | allocations with Rustique enabled ×
---|---|---|---|---|---|---|---|---|---|---|---|---|---
data/backup_archive | 0.2 / 0.3 / 0.0 | **0.5** | 0.7 / 0.3 / 0.0 | **1.0** | 0×/0 · 0×/0 · 0×/0 | 0 | 0 / 0 | 0.0 | 0 | 44 | 0.06 M | 52 | 0.95
data/billing_engine | 7.9 / 9.2 / 36.5 | **53.6** | 9.4 / 11.8 / 14.5 | **35.7** | 341×/20 · 31×/47 · 3×/367 | 0 | 0 / 2,717,466 | 0.0 | 202 | 143 | 26.14 M | 57 | 1.00
data/config_merge | 3.7 / 5.3 / 1.4 | **10.4** | 5.4 / 4.3 / 4.3 | **14.0** | 27×/5 · 2×/4 · 0×/0 | 0 | 18,018 / 88,018 | 20.5 | 3 | 44 | 1.67 M | 48 | 1.00
data/csv_etl | 8.5 / 9.6 / 0.0 | **18.1** | 9.3 / 7.3 / 0.0 | **16.5** | 0×/0 · 0×/0 · 0×/0 | 0 | 0 / 0 | 0.0 | 0 | 70 | 2.22 M | 52 | 0.53
data/fs_inventory | 5.5 / 5.8 / 0.3 | **11.6** | 5.6 / 4.1 / 1.1 | **10.7** | 7×/0 · 1×/0 · 0×/0 | 0 | 0 / 141 | 0.0 | 1 | 36 | 1.52 M | 34 | 0.54
data/log_analytics | 7.3 / 7.8 / 0.1 | **15.2** | 7.7 / 5.5 / 0.0 | **13.2** | 1×/0 · 0×/0 · 0×/0 | 0 | 0 / 274 | 0.0 | 2 | 38 | 4.25 M | 49 | 0.55
data/pandas_etl | 4.6 / 5.9 / 0.0 | **10.5** | 4.0 / 4.7 / 0.0 | **8.7** | 0×/0 · 0×/0 · 0×/0 | 0 | 0 / 0 | 0.0 | 0 | 263 | 2.62 M | 13 | 1.00
data/report_render | 7.1 / 6.9 / 1.1 | **15.0** | 5.7 / 6.7 / 2.3 | **14.8** | 19×/2 · 2×/3 · 0×/0 | 0 | 0 / 70,264 | 0.0 | 3 | 53 | 1.33 M | 71 | 0.36
data/state_snapshot | 7.8 / 13.9 / 15.4 | **37.1** | 7.3 / 9.9 / 11.8 | **29.0** | 94×/63 · 8×/23 · 1×/83 | 0 | 0 / 756,966 | 0.0 | 273 | 145 | 2.24 M | 46 | 0.55
data/text_corpus | 5.9 / 6.2 / 0.8 | **12.9** | 5.0 / 5.5 / 0.6 | **11.0** | 6×/3 · 1×/1 · 0×/0 | 0 | 0 / 20,985 | 0.0 | 7 | 46 | 2.14 M | 36 | 1.00
data/tz_events | 7.4 / 11.0 / 0.3 | **18.7** | 7.9 / 8.3 / 6.0 | **22.2** | 44×/0 · 4×/0 · 0×/0 | 0 | 0 / 281 | 0.0 | 14 | 58 | 3.96 M | 46 | 0.62
data/xml_feeds | 8.0 / 8.9 / 17.0 | **33.9** | 11.3 / 10.6 / 10.3 | **32.2** | 65×/40 · 6×/63 · 1×/69 | 0 | 824 / 874,784 | 0.1 | 116 | 101 | 3.47 M | 35 | 1.00
devtools/black_format | 4.1 / 6.0 / 15.2 | **25.4** | 6.0 / 5.2 / 10.2 | **21.3** | 144×/21 · 13×/42 · 1×/245 | 0 | 213,260 / 827,572 | 25.8 | 278 | 79 | 4.70 M | 55 | 1.00
devtools/coverage_pyflakes_tests | 4.7 / 5.6 / 15.4 | **25.8** | 6.6 / 3.7 / 33.8 | **44.1** | 149×/34 · 14×/14 · 1×/19 | 0 | 287,055 / 486,617 | 59.0 | 27 | 74 | 1.40 M | 37 | 1.00
devtools/import_startup | 13.3 / 5.6 / 18.9 | **37.8** | 7.1 / 10.9 / 23.4 | **41.4** | 0×/0 · 0×/0 · 0×/0 | 0 | 0 / 0 | 0.0 | 0 | 25 | 0.00 M | 48 | 1.01
devtools/isort_stdlib | 5.3 / 8.5 / 0.1 | **13.9** | 2.2 / 2.0 / 0.1 | **4.2** | 1×/0 · 0×/0 · 0×/0 | 0 | 1,931 / 2,056 | 93.9 | 1 | 53 | 1.46 M | 42 | 0.69
devtools/pip_resolve | 5.0 / 8.5 / 4.0 | **17.5** | 7.7 / 7.2 / 4.3 | **19.2** | 21×/6 · 2×/9 · 0×/0 | 0 | 7,250 / 85,486 | 8.5 | 84 | 83 | 2.20 M | 42 | 0.95
devtools/pycodestyle_stdlib | 4.9 / 6.6 / 0.0 | **11.5** | 5.1 / 5.0 / 0.0 | **10.2** | 0×/0 · 0×/0 · 0×/0 | 0 | 0 / 0 | 0.0 | 1 | 33 | 2.32 M | 69 | 1.00
devtools/pyflakes_stdlib | 6.7 / 4.9 / 25.3 | **36.9** | 7.6 / 4.7 / 20.2 | **32.5** | 114×/93 · 10×/320 · 1×/186 | 0 | 666,385 / 1,746,450 | 38.2 | 220 | 83 | 6.02 M | 30 | 1.00
devtools/pylint_stdlib | 4.4 / 4.8 / 40.7 | **49.9** | 7.6 / 6.9 / 13.4 | **27.8** | 152×/37 · 14×/67 · 1×/637 | 0 | 104,264 / 1,986,432 | 5.2 | 1284 | 218 | 2.84 M | 39 | 1.00
devtools/pytest_networkx | 2.6 / 4.2 / 41.7 | **48.5** | 8.8 / 5.0 / 29.4 | **43.3** | 34×/6 · 2×/5 · 0×/0 | 450 | 13,902 / 100,645 | 13.8 | 27 | 135 | 1.36 M | 51 | 0.89
devtools/rich_render | 7.9 / 10.4 / 2.6 | **20.8** | 8.6 / 8.6 / 2.4 | **19.6** | 33×/4 · 3×/2 · 0×/0 | 0 | 17 / 108,755 | 0.0 | 21 | 48 | 1.34 M | 56 | 1.00
devtools/sphinx_autodoc | 10.8 / 9.3 / 7.4 | **27.4** | 9.3 / 5.5 / 9.1 | **23.8** | 64×/17 · 6×/15 · 0×/0 | 0 | 129,930 / 274,971 | 47.3 | 339 | 129 | 4.44 M | 33 | 0.99
devtools/sqlglot_transpile | 4.6 / 5.4 / 9.3 | **19.3** | 7.2 / 4.2 / 16.8 | **28.1** | 142×/50 · 13×/64 · 1×/30 | 0 | 328,207 / 532,542 | 61.6 | 102 | 53 | 3.04 M | 41 | 0.90
web/aiohttp_app | 5.4 / 9.2 / 0.2 | **14.9** | 6.2 / 8.1 / 0.0 | **14.3** | 1×/1 · 0×/0 · 0×/0 | 0 | 215 / 4,065 | 5.3 | 4 | 50 | 0.67 M | 43 | 0.84
web/asyncio_http | 6.8 / 8.3 / 0.2 | **15.3** | 7.8 / 7.5 / 0.0 | **15.3** | 2×/0 · 0×/0 · 0×/0 | 0 | 0 / 2,287 | 0.0 | 2 | 35 | 1.34 M | 37 | 0.61
web/django_blog | 5.1 / 8.0 / 2.2 | **15.4** | 8.7 / 6.5 / 4.8 | **20.0** | 15×/9 · 1×/1 · 0×/0 | 0 | 29,149 / 38,747 | 75.2 | 24 | 63 | 1.44 M | 49 | 0.99
web/email_mime | 6.6 / 9.4 / 2.4 | **18.4** | 7.9 / 6.9 / 6.0 | **20.7** | 28×/19 · 3×/2 · 0×/0 | 0 | 73,236 / 103,151 | 71.0 | 22 | 36 | 3.23 M | 48 | 1.00
web/fastapi_pydantic | 3.9 / 8.5 / 1.4 | **13.8** | 6.7 / 8.1 / 3.2 | **17.9** | 10×/5 · 1×/2 · 0×/0 | 0 | 13,545 / 31,237 | 43.4 | 3 | 67 | 0.74 M | 50 | 0.98
web/flask_jinja | 5.4 / 8.7 / 0.6 | **14.8** | 7.3 / 7.0 / 1.5 | **15.8** | 6×/3 · 1×/0 · 0×/0 | 0 | 14,693 / 17,138 | 85.7 | 1 | 52 | 1.59 M | 48 | 0.97
web/json_api | 5.7 / 8.3 / 1.1 | **15.1** | 7.9 / 7.5 / 1.8 | **17.2** | 18×/1 · 2×/3 · 0×/0 | 0 | 0 / 44,990 | 0.0 | 6 | 49 | 2.49 M | 50 | 0.84
web/logging_service | 7.1 / 6.8 / 0.0 | **13.9** | 7.5 / 5.4 / 0.0 | **12.9** | 0×/0 · 0×/0 · 0×/0 | 0 | 0 / 0 | 0.0 | 0 | 45 | 1.13 M | 47 | 0.75
web/requests_local | 5.5 / 9.6 / 0.0 | **15.1** | 6.9 / 7.5 / 0.0 | **14.3** | 0×/0 · 0×/0 · 0×/0 | 0 | 0 / 0 | 0.0 | 0 | 41 | 0.86 M | 41 | 0.90
web/sqlalchemy_orm | 3.2 / 5.6 / 4.2 | **13.0** | 5.7 / 5.5 / 0.0 | **11.2** | 28×/14 · 3×/3 · 0×/0 | 0 | 1,752 / 160,081 | 1.1 | 38 | 90 | 0.41 M | 53 | 1.00
**mean (34)** | 6.0 / 7.4 / 7.8 | **21.2** | 6.9 / 6.4 / 6.8 | **20.1** | | | | | | | | |

Reading it:

- **Runtime layer = 21.2% (3.15.1) / 20.1% (3.15.1t) of run time** on the mean workload (M):
  allocation 6.0%, deallocation 7.4%, cyclic GC 7.8% (3.15.1); 6.9 / 6.4 / 6.8% (3.15.1t).
  The wave-1 atlas's 16.2% classified by leaf symbol only; this split counts the whole
  allocator/deallocator call (pymalloc internals, kernel page faults, `*_dealloc` cascades).
- **GC is concentrated.** Automatic collections: mean **6.9%**, median **1.1%** of run time on
  3.15.1; **5.6% / 2.4%** on 3.15.1t (M, `policies1` default cells). Explicit `gc.collect()` adds
  1.3 pp, all of it pytest_networkx (450 ms/run = 6 calls: networkx's memory-leak tests and
  pytest's session-end `gc_collect_harder`): untouchable by any policy and counted apart.
  Eight workloads carry 89% of all automatic GC time (share of their run, callbacks): pylint
  44.0%, billing 40.5, pyflakes 33.4, xml_feeds 23.8, black 22.2, state_snapshot 17.5,
  coverage 15.7, sqlglot 11.1; 15 workloads spend < 1%.
- **Full collections dominate wherever GC is large** (GIL build): pylint 637 of 741 ms/run in one
  full collection, billing 367 of 434 ms in three, black 245 of 308 ms in one. Young
  collections are cheap (≤ 60 ms/run except pyflakes' gen-1 320 ms, where they find 73%
  garbage).
- **Many full collections find nothing.** Yield of automatic collections: billing 0.0%,
  state_snapshot 0.0%, xml_feeds 0.1%, json_api 0.0%, rich_render 0.0%, report_render 0.0%; on
  the wave-2 families rules_engine 0.0%, request_pool 0.0% (both builds), task_planner 0.0%.
  These programs build large acyclic structures (records, trees of dataclasses, parsed
  documents) and the collector re-examines them because the old generation grew by 25%. Full
  collections that find < 1% garbage are **1.6% of mean run time on 3.15.1 (34 workloads; 34% of
  billing, 10.6% of xml_feeds, 9.4% of state_snapshot)** and **1.9% on 3.15.1t** (13.9%
  billing, 11.9% state, 10.4% xml, 6.1% tz_events, 5.6% pylint) (M): that is the most a
  policy that only skips useless work can take.
- **Free-threaded build:** every collection walks the whole heap (§2.3), so the t build does
  ~4–20× fewer, larger collections (billing: 4 collections of ~530 k objects per run vs 375
  smaller ones). Its GC share is lower on GIL-heavy cases (pylint 13.4 vs 40.7%) and higher
  where young collections were cheap and frequent (coverage 33.8 vs 15.4%, sqlglot 16.8 vs
  9.3%, tz_events 6.0 vs 0.3%).
- **Freelists already serve 13–71% (median 47%) of object creations** (M, Py_STATS).
- **Wave-1 kernels already allocate less:** allocations + freelist pops with Rustique enabled
  are 0.36× (report_render), 0.53× (csv_etl), 0.54–0.55× (fs_inventory, log_analytics,
  state_snapshot), 0.61–0.69× (asyncio_http, tz_events, isort), 0.75–0.90× (logging_service,
  json_api, aiohttp, sqlglot, pytest, requests) of plain (M). state_snapshot's young
  collections drop from 94 to 3 per run (GC visits 6.5 M → 1.7 M) because the `deepcopy`
  kernel creates no memo-dict entries and keep-alive lists: part of its measured speedup is
  runtime-layer time. Consequence for §5: **Rustique, as shipped, already changes when
  collections happen** — `threshold0` counts allocations (`gc.c:1990`).

### 3.2 The wave-2 families (custom, frameworks, libs, threads)

Snapshots of `w2-wl-custom` a0ae3ac/3e0b6f7, `w2-wl-frameworks` 896769f, `w2-threads`
426abe9/b1efc29, copied to `/work/w2-runtime/extra` (M, `gcprobe`, dev box, 3 × 5 runs; the
services family is in §4.4).

workload | 3.15.1 auto GC % | gen0 / gen1 / gen2 per run (n×/ms) | yield % | largest pause ms | peak RSS MB | 3.15.1t auto GC % | t yield % | t peak RSS MB
---|---|---|---|---|---|---|---|---
custom/di_plugins | 0.0 | 0×/0 · 0×/0 · 0×/0 | 0.0 | 0 | 47 | 0.0 | 0.0 | 55
custom/ecs_arena | 0.0 | 1×/0 · 0×/0 · 0×/0 | 0.0 | 2 | 28 | 0.0 | 0.0 | 31
custom/event_ledger | 2.3 | 88×/12 · 8×/14 · 1×/12 | 0.7 | 39 | 73 | 2.2 | 0.7 | 81
custom/fulfilment_sim | 2.2 | 116×/20 · 10×/6 · 1×/2 | 0.0 | 3 | 29 | 1.1 | 0.1 | 32
custom/mini_interp | 0.1 | 3×/1 · 0×/0 · 0×/0 | 11.9 | 3 | 37 | 0.3 | 25.5 | 49
custom/order_domain | 9.5 | 77×/8 · 7×/15 · 1×/103 | 25.8 | 217 | 78 | 5.7 | 30.6 | 70
custom/orm_mapper | 4.0 | 102×/7 · 9×/11 · 1×/15 | 7.8 | 29 | 64 | 6.1 | 6.8 | 69
custom/report_engine | 0.0 | 2×/0 · 0×/0 · 0×/0 | 0.8 | 1 | 46 | 0.0 | 0.0 | 52
custom/rules_engine | 8.3 | 152×/10 · 14×/12 · 1×/54 | 0.0 | 72 | 81 | 15.0 | 0.0 | 93
custom/task_planner | 1.1 | 29×/3 · 3×/3 · 0×/0 | 0.0 | 5 | 47 | 2.2 | 0.0 | 51
frameworks/django_admin | 2.6 | 29×/16 · 3×/13 · 0×/0 | 57.6 | 8 | 77 | 4.6 | 26.4 | 98
frameworks/drf_api | 6.0 | 85×/40 · 8×/35 · 0×/0 | 62.5 | 39 | 71 | 10.3 | 27.2 | 87
frameworks/falcon_api | 0.9 | 8×/5 · 1×/2 · 0×/0 | 70.8 | 5 | 56 | 1.6 | 23.1 | 70
frameworks/fastapi_sqlalchemy | 0.7 | 18×/10 · 2×/1 · 0×/0 | 48.0 | 1 | 106 | 1.8 | 15.2 | 131
frameworks/flask_marshmallow | 0.6 | 9×/5 · 1×/1 · 0×/0 | 60.3 | 4 | 85 | 0.0 | 0.0 | 102
frameworks/strawberry_graphql | 10.7 | 218×/76 · 20×/31 · 1×/23 | 55.0 | 51 | 57 | 20.0 | 26.2 | 67
libs/attrs_cattrs | 0.7 | 14×/3 · 1×/4 · 0×/0 | 0.0 | 7 | 49 | 0.8 | 0.0 | 59
libs/boto3_client | 0.2 | 10×/3 · 1×/0 · 0×/0 | 23.7 | 3 | 69 | 0.0 | 0.0 | 85
libs/bs4_scrape | 19.6 | 91×/12 · 8×/32 · 1×/205 | 41.4 | 248 | 76 | 10.1 | 26.5 | 49
libs/jsonschema_validate | 0.4 | 10×/5 · 1×/1 · 0×/0 | 87.9 | 1 | 47 | 1.0 | 22.5 | 61
libs/pydantic_payloads | 0.2 | 3×/1 · 0×/0 · 0×/0 | 0.0 | 2 | 76 | 0.0 | 0.0 | 89
libs/yaml_config | 0.0 | 2×/0 · 0×/0 · 0×/0 | 0.0 | 2 | 29 | 0.0 | 0.0 | 33
threads/compute_pool | 0.0 | 2×/0 · 0×/0 · 0×/0 | 0.0 | 0 | 31 | 0.0 | 0.0 | 41
threads/http_server | 0.3 | 1×/2 · 0×/0 · 0×/0 | 0.0 | 3 | 39 | 0.0 | 0.0 | 47
threads/pipeline_queue | 0.2 | 20×/1 · 2×/1 · 0×/0 | 0.0 | 4 | 53 | 2.8 | 0.0 | 72
threads/request_pool | 6.1 | 77×/5 · 7×/10 · 1×/17 | 0.0 | 28 | 83 | 21.7 | 0.2 | 103
threads/shared_cache | 0.1 | 2×/0 · 0×/0 · 0×/0 | 0.0 | 2 | 77 | 0.0 | 0.0 | 115

- Mean automatic GC share on these 27: 3.6% (3.15.1), 3.9% (3.15.1t). Zero-yield full
  collections: 0.35% / 1.9% of mean run time (rules_engine 5.9 / 15.0%, request_pool 3.5 /
  21.7%).
- Framework apps (django_admin, drf_api, falcon, fastapi_sqlalchemy, flask_marshmallow,
  strawberry) produce real cyclic garbage (yield 48–71%): their GC is doing useful work; a
  policy should leave it alone. strawberry_graphql's 10.7% (20% on t) is young/middle
  collections at 55% yield.
- bs4_scrape (19.6%) and order_domain (9.5%) spend it in one full collection per run that
  finds 41% / 26% garbage (parse trees with parent links; domain objects with back-references).

### 3.3 Allocation: what Rustique can reach

| question | answer |
|---|---|
| pymalloc vs mimalloc on the GIL build | `PYTHONMALLOC=mimalloc` is accepted by the python-build-standalone 3.15.1 build (✓). Paired, 5 rounds × 5 runs, 34 workloads (M, dev box, interleaved): **mimalloc is slower**, geomean **1.051**, best 0.969 (text_corpus), worst **1.296** (pyflakes), 1.21 pylint, 1.20 state_snapshot, 1.19 pytest; GC time itself grows (pyflakes 819 → 1249 ms/run: collections walk objects scattered over mimalloc pages instead of pymalloc pools); peak RSS +7.5% geomean, up to +54% (log_analytics). Settled: pymalloc is the right allocator on the GIL build. |
| can Rustique switch allocators at launch? | No: the allocator is chosen in preconfig, before `site` (`preconfig.c:705-725`); after initialisation `PyMem_SetAllocator` may only wrap the current one (`Doc/c-api/memory.rst:549-561`). A wrapper that routes new blocks elsewhere would also change `sys.getallocatedblocks()` (documented). Out of reach and, per the row above, not wanted. |
| freelists | hit rates 13–71% (median 47%); sizes are compile-time constants (`pycore_freelist_state.h`) — not reachable from entry points |
| kernels that allocate less | already happening (table 3.1, last column): the 15 wave-1 groups cut allocations + freelist pops by 10–64% on 14 of 34 workloads, and fewer GC allocations mean fewer young collections. Each new kernel that replaces pure-Python glue removes temporaries (frames' arguments, intermediate tuples/lists, bound methods); kernels cannot remove the result objects themselves, which are what the program keeps. No separate candidate: the credit belongs to each kernel's own speedup |
| deallocation (7.4%) | the cascade `_Py_Dealloc` → `*_dealloc` → free of objects the program drops; identical by construction under every kernel. Out of reach |

## 4. What a policy tier can save

### 4.1 Policies tried

| policy | what it does (public `gc` APIs) | kind |
|---|---|---|
| `off` | `gc.disable()` at launch | bound: no automatic GC |
| `thr:10000`, `thr:50000` | `gc.set_threshold(T0)`: young collections 5× / 25× rarer | static |
| `thr:2000,10,40`, `thr:2000,10,1000` | full collections 4× / ~never (25% rule still applies) | static |
| `freeze@setup`, `freeze@warmup` | `gc.freeze()` after `setup()` / after one warm-up `run()` (Instagram-style) | oracle (knows where warm-up ends) |
| `adaptive` v1 | yield-driven deferral of full collections (below), no cap | prototype, first cut |
| `adaptive` v2 | v1 + pause cap at 1.0× the largest full collection seen | too conservative (§4.2) |
| **`adaptive` (v4)** | v1 + pause cap at 1.1× the largest *legitimate* full collection | **the prototype** |
| `adaptive:y_high=0.25` | back off even when full collections find 5–25% garbage (trades memory) | variant |

**The prototype (`benches/runtime/gcadapt.py`, ~230 lines, Python, public `gc` APIs only).**
Young and middle collections are untouched. A full collection (GIL: generation 2; FT: every
collection) is allowed once the old generation has grown by a factor G since the last one,
where CPython's own rule is G = 0.25 (`gc.c:1356`, `gc_free_threading.c:2013`) and the policy
sets G = 0.25 · 2^k, k ∈ 0..3, from what full collections *find*: yield < 1% → k+1; yield ≥ 5%
→ k = 0; in between k−1. **Pause cap**: a deferred full collection is released as soon as it
would examine more than 1.1× the objects of the largest legitimate full collection so far
(GIL estimate: survivors of the last full collection × growth of `sys.getallocatedblocks()`,
an O(arenas) read; FT: survivors + net allocations, which FT's counter already is). A
collection run under CPython's rule counts with everything it examined; one the policy
deferred counts only with its survivors — otherwise each deferred collection could raise the
cap by 10% (a ratchet: measured 1.32× peak RSS on the FT burst scenario before the fix). So no
collection examines more than 1.1× what CPython's rule already ran, and garbage the policy
lets pile up stays below 10% of the largest live heap. Mechanism: one `gc.callbacks` entry
reads `generation`, `collected`, `candidates` (3.15) and sets the real threshold2 (GIL: 10 or
2^30) or threshold0 (FT: max(2000, min(G·survivors, 1.1·cap − survivors)), only on change,
since `set_threshold` stops the world on FT). Explicit `gc.collect()` is never intercepted.

### 4.2 Whole-set results (34 workloads × 2 builds, deterministic proxies)

M, dev box, `gcprobe sweep`, 3 processes × 5 runs per cell, cells of a round interleaved
(`out/policies1`, `policies2`, `policies4`). "est. run time saved" = mean over workloads of
(automatic GC share × (1 − GC-work ratio)): it assumes GC time ∝ objects examined, which
*under*-states full collections (they cost more per object: billing's measured wall ratio is
0.71 where the estimate says 0.81). "(a2) inventory exact": `gc.get_objects()` still lists every
tracked object.

policy | build | GC work × (whole set) | est. run time saved, mean % (max) | workloads with work < 0.9 | largest collection × worst | p99 collection × worst | peak RSS × worst (n > 1.05) | (a2) inventory exact | default-safe?
---|---|---|---|---|---|---|---|---|---
off | 3.15.1 | 0.009 | 6.84 (44.0) | 26 | 1.00 (pytest_networkx) | 1.00 (pytest_networkx) | 10.45 (pyflakes_stdlib; 13) | yes | no (RSS, pauses)
off | 3.15.1t | 0.000 | 5.64 (36.5) | 21 | 0.00 (json_api) | 0.00 (json_api) | 15.06 (pyflakes_stdlib; 14) | yes | no (RSS, pauses)
thr:10000 | 3.15.1 | 0.575 | 2.89 (26.5) | 26 | 4.84 (isort_stdlib) | 5.94 (django_blog) | 1.25 (black_format; 5) | yes | no (RSS, pauses)
thr:10000 | 3.15.1t | 0.993 | 0.05 (1.2) | 2 | 1.13 (email_mime) | 1.13 (email_mime) | 1.11 (csv_etl; 1) | yes | no (little effect on FT; RSS 1.11×)
thr:50000 | 3.15.1 | 0.433 | 3.90 (35.4) | 25 | 10.82 (flask_jinja) | 25.00 (billing_engine) | 1.71 (email_mime; 8) | yes | no (RSS, pauses)
thr:50000 | 3.15.1t | 0.756 | 1.94 (16.6) | 14 | 2.75 (email_mime) | 2.75 (email_mime) | 1.47 (email_mime; 9) | yes | no (RSS, pauses)
thr:2000,10,40 | 3.15.1 | 0.721 | 1.90 (24.0) | 6 | 2.70 (black_format) | 1.01 (sphinx_autodoc) | 1.95 (black_format; 5) | yes | no (RSS, pauses)
thr:2000,10,40 | 3.15.1t | 1.002 | -0.01 (0.0) | 0 | 1.00 (sphinx_autodoc) | 1.00 (sphinx_autodoc) | 1.02 (json_api; 0) | yes | n/a (threshold2 is unused on FT)
thr:2000,10,1000 | 3.15.1 | 0.687 | 2.08 (24.8) | 6 | 1.00 (flask_jinja) | 1.00 (flask_jinja) | 3.36 (black_format; 6) | yes | no (RSS, pauses)
thr:2000,10,1000 | 3.15.1t | 1.002 | -0.01 (0.0) | 0 | 1.00 (django_blog) | 1.00 (django_blog) | 1.03 (state_snapshot; 0) | yes | n/a (threshold2 is unused on FT)
freeze@setup | 3.15.1 | 0.968 | 0.22 (3.0) | 2 | 1.49 (asyncio_http) | 1.49 (asyncio_http) | 1.03 (pip_resolve; 0) | no | no (a2, frozen garbage)
freeze@setup | 3.15.1t | 0.706 | 1.99 (13.6) | 18 | 0.98 (pylint_stdlib) | 0.98 (pylint_stdlib) | 1.06 (csv_etl; 1) | no | no (a2, frozen garbage)
freeze@warmup | 3.15.1 | 0.904 | 0.68 (10.9) | 3 | 1.17 (fastapi_pydantic) | 1.17 (fastapi_pydantic) | 1.21 (black_format; 5) | no | no (a2, frozen garbage)
freeze@warmup | 3.15.1t | 0.576 | 2.84 (19.9) | 20 | 0.94 (xml_feeds) | 0.94 (xml_feeds) | 1.15 (pylint_stdlib; 4) | no | no (a2, frozen garbage)
adaptive v1 (no cap) | 3.15.1 | 0.867 | 0.68 (19.1) | 2 | 1.04 (pylint_stdlib) | 1.01 (log_analytics) | 1.02 (black_format; 0) | yes | no (adversarial 1.45× RSS; FT pauses 1.59×)
adaptive v1 (no cap) | 3.15.1t | 0.627 | 1.66 (13.9) | 10 | 1.59 (black_format) | 1.59 (black_format) | 1.14 (black_format; 2) | yes | no (adversarial 1.45× RSS; FT pauses 1.59×)
adaptive v2 (cap 1.0) | 3.15.1 | 0.951 | 0.23 (8.7) | 1 | 1.03 (state_snapshot) | 1.01 (log_analytics) | 1.01 (report_render; 0) | yes | yes, conservative
adaptive v2 (cap 1.0) | 3.15.1t | 0.853 | 0.58 (6.8) | 5 | 1.17 (report_render) | 1.17 (report_render) | 1.03 (black_format; 0) | yes | yes, conservative
**adaptive v4 (cap 1.1)** | 3.15.1 | 0.886 | 0.53 (18.7) | 1 | 1.04 (billing_engine) | 1.01 (log_analytics) | 1.03 (logging_service; 0) | yes | **yes**
**adaptive v4 (cap 1.1)** | 3.15.1t | 0.772 | 1.01 (10.2) | 5 | 1.05 (billing_engine) | 1.05 (billing_engine) | 1.02 (logging_service; 0) | yes | **yes**

- **Static policies are not default-safe**: `off` saves 6.8% mean but peak RSS reaches 10–15×;
  raising threshold0 lengthens young pauses 5–25× and costs 1.25–1.7× RSS; deferring full
  collections statically (`thr:2000,10,40`) costs black 1.95× RSS (it holds 190 k garbage
  objects per run). `freeze` hides objects from `gc.get_objects()` (fails (a2)) and leaves frozen
  garbage uncollected.
- **v1 (no cap)** is the best GIL saver among safe-looking policies (work 0.867) but on FT it
  makes black's collection 1.59× larger with 1.14× RSS, and it fails the adversarial suite
  (§4.3: 1.45× RSS).
- **v4 (the prototype)** keeps every proxy inside the budgets on all 34 workloads and both
  builds: largest collection ≤ 1.05×, p99 ≤ 1.05×, peak RSS ≤ 1.03×; whole-set GC work
  **0.886 (3.15.1) / 0.772 (3.15.1t)**, est. **0.5% / 1.0%** of mean run time, concentrated:
  billing −47% / −69% of GC work, state_snapshot −74% (t), xml_feeds −74% (t).
- **The free-threaded build benefits more, with the same rule**: every
  FT collection walks the whole heap, so deferring collections that find nothing is where all
  of FT's GC work is; FT's young count is already net of deallocations, which makes the cap's
  size estimate exact. No separate rule is needed. On the GIL build the cap binds early on
  workloads whose old generation returns to the same peak every run (state_snapshot: 1.04×),
  because a full collection there is already as large as any CPython ran.
- `y_high = 0.25` (back off even when 5–25% is garbage) gives the same numbers as v4: with the
  cap in place, workloads with real garbage (pylint 9%, black 79%) are bound by the cap, not
  by the yield rule. Taking pylint's 640 ms full collection would need memory (thr:10000:
  pylint 0.54× wall, 1.15× RSS): that is a throughput knob users already have
  (`gc.set_threshold`), not a default.

Per workload, v4 vs default (workloads with ≥ 3% automatic GC on either build; wall × from the
dev box is noise-dominated — pylint 1.43 with identical GC work):

workload | GC % (3.15.1) | work × | largest coll. × | peak RSS × | wall × (noisy) | GC % (t) | work × | largest coll. × | peak RSS × | wall ×
---|---|---|---|---|---|---|---|---|---|---
data/billing_engine | 39.7 | 0.53 | 1.04 | 1.00 | 0.71 | 14.8 | 0.31 | 1.05 | 0.87 | 0.75
data/config_merge | 1.9 | 1.00 | 1.00 | 1.00 | 0.77 | 4.7 | 1.03 | 1.03 | 1.01 | 1.25
data/state_snapshot | 19.5 | 1.04 | 1.00 | 1.00 | 1.00 | 12.4 | 0.26 | 1.00 | 1.00 | 0.90
data/tz_events | 0.0 | 1.00 | 1.00 | 1.00 | 1.00 | 6.1 | 1.00 | 1.00 | 1.00 | 1.00
data/xml_feeds | 24.2 | 1.00 | 1.00 | 1.00 | 1.01 | 10.6 | 0.26 | 0.63 | 1.00 | 0.91
devtools/black_format | 23.6 | 1.00 | 1.00 | 1.00 | 1.00 | 11.9 | 0.96 | 1.00 | 0.99 | 0.99
devtools/coverage_pyflakes_tests | 16.7 | 1.00 | 1.00 | 1.00 | 1.01 | 38.2 | 1.01 | 1.04 | 1.00 | 1.08
devtools/pip_resolve | 3.2 | 1.00 | 1.00 | 1.00 | 0.98 | 4.7 | 1.05 | 1.01 | 1.01 | 1.18
devtools/pyflakes_stdlib | 44.7 | 1.00 | 1.00 | 1.00 | 0.96 | 23.0 | 1.00 | 1.00 | 1.00 | 0.91
devtools/pylint_stdlib | 47.3 | 1.00 | 1.00 | 1.00 | 1.43 | 13.2 | 1.00 | 1.00 | 1.00 | 0.97
devtools/rich_render | 2.8 | 1.00 | 1.00 | 1.00 | 1.13 | 5.1 | 1.00 | 1.00 | 1.00 | 0.94
devtools/sphinx_autodoc | 4.6 | 1.00 | 1.00 | 1.00 | 0.93 | 10.1 | 1.00 | 1.00 | 1.00 | 0.94
devtools/sqlglot_transpile | 12.5 | 1.00 | 1.00 | 1.00 | 0.78 | 19.1 | 1.00 | 1.00 | 1.00 | 1.09
web/django_blog | 2.1 | 1.00 | 1.00 | 1.00 | 0.95 | 4.0 | 1.00 | 1.00 | 1.00 | 0.99
web/email_mime | 3.6 | 1.00 | 1.00 | 1.00 | 1.13 | 5.9 | 1.05 | 1.00 | 0.99 | 0.84
web/fastapi_pydantic | 2.1 | 1.00 | 1.00 | 1.00 | 1.03 | 6.1 | 0.00 | 0.98 | 1.01 | 0.95
web/sqlalchemy_orm | 5.4 | 0.99 | 0.11 | 0.99 | 0.86 | 0.0 | nan | nan | 0.87 | 1.29

### 4.3 Worst cases: garbage that appears after the policy backed off

`benches/runtime/adversarial.py` (M, dev box, 3 processes × 4 runs; `RQ_ADV=<scenario>`).
Each `run()` first builds a 600 k-object acyclic structure (full collections find nothing →
the policy backs off to k = 3), then switches phase: **late_cycles** — 400 k self-referencing
nodes kept in a 60 k FIFO window, so every cycle reaches the old generation before it dies;
**burst** — three 300 k-node cyclic graphs built, used and dropped; **steady** — no build phase,
a cyclic window from the start (the policy must stay at CPython's rule). 1 in 97 cycles
carries a weakref callback: **garbage lifetime** = time from the moment the cycle became
unreachable to the collection that freed it.

scenario | build | policy | peak RSS MB max (median) | × default | garbage lifetime p50 / max ms | uncollected at run end (of sampled) | examined per run | largest collection | run ms
---|---|---|---|---|---|---|---|---|---
burst | 3.15.1 | default | 120 (118) | 1.00 | 103 / 196 | 3093 (9279) | 10,603,617 | 1,108,944 | 1072
burst | 3.15.1 | adaptive | 117 (114) | 0.98 | 103 / 1239 | 0 (9279) | 10,516,868 | 1,104,999 | 1118
burst | 3.15.1 | adaptive:cap=1.0 | 108 (108) | 0.90 | 79 / 196 | 0 (9279) | 9,895,083 | 987,544 | 1024
burst | 3.15.1 | adaptive:cap=0 | 173 (147) | 1.45 | 219 / 398 | 9279 (9279) | 8,821,906 | 1,390,953 | 902
burst | 3.15.1t | default | 145 (143) | 1.00 | 49 / 125 | 0 (9279) | 7,910,380 | 1,150,654 | 631
burst | 3.15.1t | adaptive | 148 (144) | 1.02 | 93 / 122 | 3093 (9279) | 4,887,769 | 1,188,963 | 558
burst | 3.15.1t | adaptive:cap=1.0 | 123 (123) | 0.85 | 45 / 188 | 0 (9279) | 6,463,908 | 1,029,722 | 578
burst | 3.15.1t | adaptive:cap=0 | 213 (194) | 1.47 | 153 / 302 | 9279 (9279) | 3,158,799 | 1,848,440 | 539
late_cycles | 3.15.1 | default | 108 (107) | 1.00 | 123 / 245 | 721 (4124) | 5,314,747 | 998,818 | 521
late_cycles | 3.15.1 | adaptive | 108 (108) | 1.00 | 121 / 276 | 741 (4124) | 5,306,928 | 998,855 | 537
late_cycles | 3.15.1 | adaptive:cap=1.0 | 108 (107) | 1.00 | 128 / 224 | 741 (4124) | 5,306,928 | 998,855 | 549
late_cycles | 3.15.1 | adaptive:cap=0 | 110 (108) | 1.02 | 131 / 222 | 1006 (4124) | 5,250,384 | 1,035,913 | 549
late_cycles | 3.15.1t | default | 112 (111) | 1.00 | 53 / 156 | 1858 (4124) | 4,116,516 | 916,671 | 353
late_cycles | 3.15.1t | adaptive | 117 (117) | 1.04 | 75 / 127 | 1153 (4124) | 2,684,774 | 1,009,785 | 326
late_cycles | 3.15.1t | adaptive:cap=1.0 | 113 (112) | 1.01 | 56 / 88 | 2088 (4124) | 2,682,458 | 918,137 | 307
late_cycles | 3.15.1t | adaptive:cap=0 | 154 (130) | 1.37 | 48 / 114 | 4124 (4124) | 1,848,838 | 1,348,740 | 276
steady | 3.15.1 | default | 59 (59) | 1.00 | 91 / 391 | 2953 (10310) | 5,151,104 | 363,030 | 858
steady | 3.15.1 | adaptive | 59 (59) | 1.00 | 114 / 484 | 2953 (10310) | 5,151,314 | 363,086 | 1199
steady | 3.15.1 | adaptive:cap=1.0 | 59 (59) | 1.00 | 102 / 419 | 2953 (10310) | 5,151,314 | 363,086 | 993
steady | 3.15.1 | adaptive:cap=0 | 59 (59) | 1.00 | 91 / 274 | 2953 (10310) | 5,151,314 | 363,086 | 820
steady | 3.15.1t | default | 41 (41) | 1.00 | 13 / 125 | 678 (10310) | 4,808,081 | 127,679 | 818
steady | 3.15.1t | adaptive | 43 (42) | 1.03 | 13 / 91 | 549 (10310) | 4,653,408 | 141,407 | 803
steady | 3.15.1t | adaptive:cap=1.0 | 41 (41) | 1.01 | 12 / 55 | 678 (10310) | 4,650,056 | 128,607 | 731
steady | 3.15.1t | adaptive:cap=0 | 44 (44) | 1.07 | 13 / 119 | 663 (10310) | 4,561,056 | 159,258 | 729

- **Without the cap (`cap=0`, v1)**: peak RSS 1.45× (GIL burst), 1.47× (FT burst), 1.37× (FT
  late_cycles); *every* sampled cycle still uncollected when the run ends; garbage lifetime
  2–3×. This is the "garbage appears right after the back-off" failure, measured.
- **With the cap (v4)**: peak RSS **0.98–1.04×** default in all six scenario × build cells;
  largest collection ≤ 1.10× (FT late_cycles 1.10, FT steady 1.11 — at the budget's edge);
  garbage lifetime p50 equal on GIL, up to 1.9× on FT burst (93 vs 49 ms), max within the
  noise of the box; no cycle stays uncollected longer than in the default on GIL.
- `cap=1.0` (v2) is the conservative point: RSS ≤ 1.01×, largest collection ≤ 1.0×, but it
  gives back most of the savings on real workloads (§4.2).
- Time-to-collect after the phase change is bounded by construction: the first full
  collection after garbage starts arriving runs no later than when the old generation reaches
  1.1× the largest legitimate heap; it then finds ≥ 5% garbage and resets k = 0.

### 4.4 The wave-2 families and long-running services

v4 vs default on the 31 wave-2 workloads (custom, frameworks, libs, services, threads; M, dev
box, `out/policies4`): **GC work 0.994 (3.15.1) / 0.811 (3.15.1t)**; est. run time saved
−0.03% / **1.06%** mean (max 3.3% rules_engine / 18.7% request_pool); largest collection ≤ 1.06×
everywhere; peak RSS ≤ 1.01× except one cell (threads/pipeline_queue, 3.15.1: 1.10×, see below).
Rows with ≥ 2% automatic GC:

workload | GC % (3.15.1) | work × | largest coll. × | peak RSS × | wall × (noisy) | GC % (t) | work × | largest coll. × | peak RSS × | wall ×
---|---|---|---|---|---|---|---|---|---|---
custom/event_ledger | 2.6 | 1.00 | 1.00 | 1.00 | 1.01 | 2.4 | 0.60 | 0.88 | 1.00 | 1.03
custom/fulfilment_sim | 2.4 | 1.00 | 0.96 | 0.99 | 0.98 | 1.6 | 0.47 | 1.06 | 1.00 | 0.93
custom/order_domain | 12.7 | 1.00 | 1.00 | 1.00 | 1.47 | 6.4 | 0.71 | 0.96 | 1.00 | 0.98
custom/orm_mapper | 5.0 | 1.00 | 0.99 | 0.99 | 0.90 | 6.2 | 0.72 | 0.99 | 1.00 | 0.99
custom/rules_engine | 9.9 | 0.66 | 1.00 | 1.00 | 0.95 | 15.0 | 0.40 | 0.98 | 0.99 | 0.90
custom/task_planner | 1.1 | 1.00 | 0.99 | 1.00 | 0.94 | 2.1 | 1.01 | 1.03 | 1.00 | 1.03
frameworks/django_admin | 2.6 | 1.00 | 1.00 | 1.00 | 0.98 | 4.1 | 1.00 | 1.00 | 1.00 | 0.94
frameworks/drf_api | 6.6 | 1.00 | 1.00 | 1.00 | 1.10 | 10.6 | 1.00 | 1.00 | 1.00 | 1.14
frameworks/strawberry_graphql | 10.7 | 1.00 | 1.00 | 1.00 | 0.98 | 20.7 | 1.00 | 1.00 | 1.01 | 1.10
libs/bs4_scrape | 19.1 | 1.00 | 1.00 | 1.00 | 1.05 | 11.7 | 1.01 | 0.99 | 1.01 | 0.95
services/job_worker | 3.7 | 1.00 | 1.00 | 0.99 | 0.99 | 22.1 | 1.00 | 1.01 | 1.01 | 0.97
services/uvicorn_fastapi | 4.8 | 1.84 | 1.00 | 1.00 | 0.95 | 1.8 | 1.00 | 0.99 | 1.00 | 1.00
services/waitress_flask | 0.7 | 0.99 | 1.00 | 0.99 | 0.99 | 3.7 | 1.00 | 0.98 | 1.00 | 0.86
threads/pipeline_queue | 0.3 | 1.01 | 1.00 | 1.10 | 0.99 | 3.2 | 1.00 | 1.00 | 1.00 | 1.00
threads/request_pool | 6.5 | 1.02 | 0.96 | 0.97 | 1.01 | 26.3 | 0.29 | 1.05 | 1.01 | 0.92

- The framework apps (django_admin, drf, strawberry) and bs4 make real cyclic garbage (yields
  26–71%): the policy correctly leaves them at CPython's rule (work 1.00).
- rules_engine (0% yield) and request_pool (threaded, 0.2% yield on t) are the new
  billing-like cases: **request_pool on 3.15.1t: 26% of its run is GC, work → 0.29×**.
- Two cells need the quiet box: services/uvicorn_fastapi (3.15.1, work 1.84×) is not a policy
  effect — it runs 38–183 young collections per run *under the default* (a server thread's
  allocation pattern) and the policy only acts on full collections; threads/pipeline_queue
  (3.15.1, RSS 1.10×) does 0.3% GC, the policy saw one zero-yield full collection there and
  could have deferred at most the next one, yet RSS drifts 45 → 52 MB in two of three policy
  processes: unexplained, counted as a (b) miss until the quiet box says otherwise.
- **Long-running services** (40 iterations per process, 2 processes, `services-long`, M): with
  v1 the per-iteration RSS growth equals the default's (+0.3 to +2.3 MB over 20 iterations
  on both), GC per iteration equal or lower (uvicorn 93 → 61 ms GIL); v4: §4.5.

### 4.5 Memory over time (long-running services, v4) and what the quiet box must confirm

`services` family (w2-wl-frameworks d9e1293: servers persist across `run()`s), 40 iterations per
process, 2 processes per cell (M, dev box, `out/services-long4`, `benches/runtime/longtable.py`):

build | workload | policy | run ms | GC ms/run | examined/run | RSS first → last MB | RSS growth 2nd−1st half MB | peak RSS MB
---|---|---|---|---|---|---|---|---
3.15.1 | services/cache_service | default | 1044 | 0.0 | 0 | 55 → 60 | +0.8/+0.7 | 60
3.15.1 | services/cache_service | adaptive | 1042 | 0.0 | 0 | 55 → 59 | +0.9/+0.4 | 60
3.15.1 | services/job_worker | default | 842 | 32.1 | 144,755 | 41 → 45 | +1.1/+1.3 | 45
3.15.1 | services/job_worker | adaptive | 867 | 32.3 | 144,914 | 42 → 44 | +0.5/+1.0 | 44
3.15.1 | services/uvicorn_fastapi | default | 1403 | 70.2 | 366,917 | 72 → 74 | +1.5/+0.9 | 75
3.15.1 | services/uvicorn_fastapi | adaptive | 1384 | 50.8 | 327,394 | 71 → 77 | +2.6/+2.6 | 77
3.15.1 | services/waitress_flask | default | 1273 | 9.1 | 36,004 | 51 → 52 | +0.4/+0.6 | 52
3.15.1 | services/waitress_flask | adaptive | 1376 | 9.2 | 36,018 | 51 → 52 | +0.4/+0.6 | 52
3.15.1t | services/cache_service | default | 1113 | 0.0 | 0 | 62 → 63 | +0.1/+0.1 | 63
3.15.1t | services/cache_service | adaptive | 1118 | 0.0 | 0 | 63 → 63 | +0.1/+0.1 | 63
3.15.1t | services/job_worker | default | 584 | 187.7 | 439,126 | 57 → 60 | +1.1/+0.7 | 62
3.15.1t | services/job_worker | adaptive | 592 | 212.2 | 440,067 | 58 → 60 | +0.6/+0.8 | 61
3.15.1t | services/uvicorn_fastapi | default | 2291 | 48.9 | 128,076 | 96 → 99 | +1.1/+0.1 | 100
3.15.1t | services/uvicorn_fastapi | adaptive | 2004 | 51.8 | 127,734 | 97 → 99 | +0.1/+0.5 | 100
3.15.1t | services/waitress_flask | default | 569 | 41.7 | 135,340 | 69 → 71 | +1.1/+0.6 | 72
3.15.1t | services/waitress_flask | adaptive | 533 | 36.3 | 135,860 | 69 → 72 | +0.9/+0.7 | 73

- RSS growth per process with v4 is within the default's range everywhere except
  uvicorn_fastapi on 3.15.1 (+2.6/+2.6 MB vs +1.5/+0.9 MB over 20 iterations; peak 77 vs 75 MB,
  1.03×) — the same workload whose young-collection count varies 38–183 per run under the
  default; GC per iteration 70 → 51 ms there. Counted as a borderline (b) cell.
- **Quiet-box run (pending, ~1.5 h, scripted end to end):**
  `benches/runtime/quiet_driver.sh <host>` deploys `benches/runtime`, the workloads and the
  wave-2 families, creates missing venvs, runs `run_quiet.sh` (default vs v4; 34 + 31
  workloads × 5 processes × 5 runs, one L3 domain per build, threads workloads get the whole
  domain; the adversarial suite incl. `cap=0`; services × 40 iterations) and pulls results every
  2 minutes into `~/rustique-research/w2/runtime/quiet/`. Tables: `gctable.py scoreboard`,
  `policyrows.py`, `advtable.py`, `longtable.py`. Smoke-tested on the dev box (all four stages).
  It decides: wall ratios (≤ 1.05 everywhere, billing ~0.7), pause ms (max and p99 ≤ 1.10×),
  peak RSS (≤ 1.05×) incl. the two borderline cells.

### 4.6 Exit-time GC (w2-attribution's pool: 40.6% of a whole pylint process)

**What CPython 3.15.1 runs at exit** (`Python/pylifecycle.c`, `_Py_Finalize`):

| step | where | collection | callbacks / disabled GC | what is still alive |
|---|---|---|---|---|
| 1 | `make_pre_finalization_calls` (`:2273-2310`) | none: join non-daemon threads, pending calls, **atexit** handlers | – | everything |
| 2 | `:2461`, `:2465` | flush stdio, signal handling off | – | everything |
| 3 | `PyGC_Collect()` **`:2480`** | full collection, reason MANUAL | `gc.callbacks` fire; **skipped if GC is disabled** (`gc.c:1794-1808`) | all modules intact: finalizers and weakref callbacks run with a working interpreter |
| 4 | `finalize_modules` `:1893-1992`: special `sys`/`builtins` names deleted, `sys.modules` values → None, modules dict cleared, original builtins restored | – | – | module objects only via other references |
| 5 | `_PyGC_CollectNoFail` **`:1948`** | full collection, reason SHUTDOWN | callbacks **not** invoked (`gc.c:1476, 1648`); runs **even if GC is disabled**; errors cleared (`gc.c:1621`) | modules torn down: this is where the program's big object graphs (caches, ASTs) become cyclic garbage and are traversed *and freed* |
| 6 | `:1969` remaining modules' dicts cleared (reverse import order), `sys`/`builtins` dicts cleared | – | – | – |
| 7 | `_PyGC_CollectNoFail` **`:1991`** | full collection, SHUTDOWN | as 5 | – |
| (8) | `_PyGC_CollectIfEnabled` `:2511` | disabled (`#if 0`, "caused too many problems") | | |
| 9 | `finalize_interp_clear` → `_PyGC_Fini` (`gc.c:1880-1907`) | none: GC lists unlinked, **frozen objects never collected** | | |

**Measured** (M, `benches/runtime/exitcost.py`, dev box, 3 processes per mode; exit = from the
program's last statement to process end, CLOCK_MONOTONIC across processes; one `run()` after
`setup()`):

workload | build | tracked at exit | run() ms | exit ms: CPython | of which PyGC_Collect (before teardown) | `gc.disable()` at exit | `gc.freeze()` at exit | `os._exit` (floor) | exit ÷ run
---|---|---|---|---|---|---|---|---|---
data/billing_engine | 3.15.1 | 93,582 | 1313 | **28** | 5 | 27 | 18 | 5 | 0.02
data/billing_engine | 3.15.1t | 96,481 | 947 | **42** | 3 | 43 | 31 | 13 | 0.04
devtools/black_format | 3.15.1 | 123,680 | 1907 | **180** | 155 | 217 | 10 | 5 | 0.09
devtools/black_format | 3.15.1t | 54,568 | 1739 | **84** | 17 | 79 | 33 | 5 | 0.05
devtools/pylint_stdlib | 3.15.1 | 348,225 | 1280 | **1330** | 207 | 999 | 17 | 8 | 1.04
devtools/pylint_stdlib | 3.15.1t | 360,645 | 1373 | **375** | 50 | 289 | 98 | 12 | 0.27
devtools/pytest_networkx | 3.15.1 | 135,915 | 1478 | **440** | 83 | 365 | 33 | 11 | 0.30
devtools/pytest_networkx | 3.15.1t | 173,041 | 1299 | **382** | 38 | 322 | 146 | 37 | 0.29
devtools/sphinx_autodoc | 3.15.1 | 154,805 | 1559 | **419** | 97 | 327 | 21 | 7 | 0.27
devtools/sphinx_autodoc | 3.15.1t | 204,422 | 1836 | **327** | 42 | 287 | 114 | 13 | 0.18
web/django_blog | 3.15.1 | 66,669 | 915 | **152** | 30 | 103 | 14 | 5 | 0.17
web/django_blog | 3.15.1t | 103,953 | 834 | **184** | 25 | 170 | 66 | 8 | 0.22
web/fastapi_pydantic | 3.15.1 | 78,783 | 425 | **174** | 38 | 205 | 16 | 5 | 0.41
web/fastapi_pydantic | 3.15.1t | 100,653 | 591 | **189** | 22 | 180 | 74 | 7 | 0.32
web/sqlalchemy_orm | 3.15.1 | 66,311 | 517 | **196** | 38 | 140 | 15 | 8 | 0.38
web/sqlalchemy_orm | 3.15.1t | 87,157 | 565 | **218** | 36 | 216 | 138 | 9 | 0.39

- Exit costs **0.27–1.04× a whole `run()`** for the CLI-shaped programs (pylint 1.33 s after a
  1.28 s run; pytest 0.44 s; sphinx 0.42 s; sqlalchemy/fastapi 0.17–0.2 s). Step 3 is only
  15–25% of it: **steps 5–7 dominate** — traversing and freeing everything that becomes cyclic
  garbage once modules are torn down (pylint: astroid's whole module cache). `gc.disable()`
  removes only step 3; `gc.freeze()` at exit removes nearly all of it (pylint 1330 → 17 ms,
  sphinx 419 → 21 ms) because frozen objects are skipped by steps 5–7 and never collected.
- **Observable at exit**: `atexit` order (unchanged by any of this: step 1 comes first);
  finalizers (`__del__`, `tp_finalize`), weakref callbacks, `weakref.finalize` objects and
  their order relative to module teardown (step 3 vs 5 vs 7); `ResourceWarning: unclosed file`
  emitted by io finalizers; **data flushed by `BufferedWriter`/`TextIOWrapper` finalizers of
  files that are still open and only reachable from cycles**; `gc.callbacks` invocations (step
  3 only); `DEBUG_STATS`/`DEBUG_UNCOLLECTABLE` output and the "uncollectable objects at
  shutdown" warning (`_PyGC_DumpShutdownStats`, `:1952`).
- **Can it be cut exactly?** Only by proving that the garbage steps 3–7 would free contains
  nothing whose destruction is observable. A conservative proof — "no object with a finalizer
  and no weakref with a callback anywhere in the heap" — never holds: at the end of a run
  pylint has 13 finalizable objects (stdio `TextIOWrapper`/`BufferedWriter`/`FileIO`) and 197
  weakrefs with callbacks, sqlalchemy 10 (incl. a `sqlite3.Connection`) and 853, sphinx 45
  (26 suspended generators) and 557 (M, `finscan.py`; a Python scan costs 46–130 ms, ~10% of
  the exit it would save). A precise proof needs to know which objects become garbage after
  teardown, which *is* the shutdown collection. Skipping traversal of immortal objects is
  already done by CPython (`update_refs` untracks them, `gc.c:405-411`). **Verdict: not
  reachable exactly; no default.**
- **Opt-in "fast exit" (candidate R3)**: `gc.freeze()` in Rustique's last `atexit` slot (after
  every user handler, after stdio is flushed by CPython at step 2) when the program has not
  touched `gc` debug flags/callbacks. Exactly what it changes: cyclic garbage that exists at
  exit is not finalized (no `__del__`, no weakref callbacks, no `ResourceWarning`, no flush of
  buffered files that are only reachable from cycles — the one real data-loss risk); acyclic
  objects are still freed by refcounting in steps 4–7 with their finalizers. Saves 0.15–1.3 s
  per process on the CLI-shaped workloads; it is a documented-trade-off switch for linters,
  test runners and build tools (they can already do it themselves; mypy and ray call
  `gc.freeze()` in library code), not a candidate for the default.

## 5. Default eligibility: the four criteria, made testable

### 5.1 (a) "Documented `gc` APIs report nothing different"

**The strict reading cannot be met by any policy that changes when collections run — and is
not met by Rustique today.** `gc.get_count()`, `gc.get_stats()` counters, the number of
`gc.callbacks` invocations and their `info`, `gc.collect()`'s return value and `DEBUG_STATS`
output are all functions of collection *timing*, and `threshold0` counts allocations
(`gc.c:1990`). Rustique's wave-1 kernels already change them: state_snapshot runs 3 young
collections per `run()` with Rustique enabled instead of 94 (M, Py_STATS, §3.1); 14 of 34
workloads allocate 10–64% fewer objects. CPython's own test modules pass identically either
way (wave 1: 8,125 outcomes; here 22–23 modules × 2 builds under the policy, below). The
language reference allows postponing collection (`datamodel.rst:61-64`); the gc docs leave
the oldest generation's trigger to `InternalDocs`.

**Proposed definition** — four testable clauses:

| clause | what must hold | how the prototype satisfies it | test |
|---|---|---|---|
| (a1) configuration round-trip | `get_threshold()`, `isenabled()`, `get_debug()`, `get_freeze_count()` return exactly what CPython would given the program's own calls | see the API table: `get_threshold()` is virtualised; nothing else is touched (the policy never disables, never freezes, never sets debug flags) | `test_gcadapt.py::test_threshold_virtualized_while_deferring`, `::test_step_aside_on_each_tuning_call` (both builds ✓) |
| (a2) inventory exact | `get_objects()`, `get_referrers()`, `get_referents()`, `is_tracked()`, `is_finalized()` see every tracked object, none hidden | no `freeze` → nothing leaves generations 0–2 (`gc.c:1711-1733`) | `::test_no_freeze_inventory_exact` ✓; **freeze-based policies fail this clause** (frozen objects vanish from `get_objects`/`get_referrers`, `gc.c:1693-1733`, ✓ verified) |
| (a3) truthful collection reports | every collection that runs is CPython's own algorithm on its own lists; callbacks fire for each with CPython's `info`; `get_stats()`/`get_count()` describe the collections that ran; `gc.collect(g)` runs exactly generation g, never skipped, never redirected, and afterwards no cyclic garbage remains in any generation | the policy only moves the trigger point; it never runs, skips or alters a collection | `::test_explicit_collect_is_cpythons` ✓; CPython test modules 0 diffs ✓ |
| (a4) one visible artefact | `gc.callbacks` holds one extra entry (the policy's observer) | inherent: 3.15 offers no other per-collection hook (the ring buffer of §2.1 is only readable remotely) | static reach of code that reads the list: 5 of the top 1000 packages touch `gc.callbacks` (ddtrace, mypy, hypothesis, torch, beartype; 0.3% download-weighted, M) and all of them only append/insert/remove their own entry |

**What the policy does to each documented API** (v4, both builds):

| API | effect while the policy acts | same as CPython? |
|---|---|---|
| `gc.get_threshold()` | returns the *visible* thresholds: CPython's default (2000, 10, 10) or what the program set. **While deferring, the collector actually runs with threshold2 = 2^30 (GIL) or threshold0 = max(2000, G·survivors) (FT); `get_threshold()` does not report that.** | Round-trip yes; "the thresholds in force" no. Reporting the real value would put 1073741824 in front of the program — a value CPython never shows. We choose round-trip: the thresholds are configuration, and the program reads back what it configured. Either way one reading is broken; this is the one the docs' wording ("Return the current collection thresholds") leaves open, and no test in `test_gc` or the top-1000 corpus distinguishes them (3 packages read it: datamodel-code-generator, litellm, torch — to save and restore around `set_threshold`, which makes the policy step aside) |
| `gc.set_threshold(...)` | CPython's function runs first (same validation, same errors), the named thresholds are applied over the visible vector, then the policy steps aside for good: the real thresholds become exactly the program's | yes (`::test_set_threshold_errors_are_cpythons`, `::test_step_aside_on_each_tuning_call`) |
| `gc.enable/disable/isenabled` | untouched; `enable`/`disable` calls make the policy step aside | yes |
| `gc.collect(generation)` | untouched (never intercepted). Its return value can be larger than without the policy: garbage CPython would have found in an earlier automatic full collection is found by this one — the same effect wave-1 kernels have through allocation counts | semantics yes; value timing-dependent (a3) |
| `gc.get_count()` | raw counters. GIL: `count[2]` (gen-1 collections since the last full) grows past threshold2 while deferring — CPython shows the same whenever its 25% rule holds a full collection back | values timing-dependent, state space CPython's (a3) |
| `gc.get_stats()` | cumulative counters of the collections that ran: fewer generation-2 (GIL) / fewer (FT) collections, same keys | timing-dependent (a3) |
| `gc.callbacks` | the program's callbacks fire for every collection with CPython's `info` (`generation, collected, uncollectable, candidates, duration`); the list holds one extra entry | (a3) yes; (a4) |
| `gc.freeze/unfreeze/get_freeze_count` | untouched; `freeze`/`unfreeze` make the policy step aside; count exact (the policy never freezes) | yes |
| `gc.get_objects/get_referrers/get_referents/is_tracked/is_finalized` | untouched; between collections they may list cyclic garbage CPython would already have freed (it is tracked garbage, as between any two collections) | yes (a2) |
| `gc.garbage`, `DEBUG_SAVEALL`, `DEBUG_UNCOLLECTABLE` | the same objects end up there, possibly later | yes, timing-dependent |
| `gc.set_debug/get_debug`, `DEBUG_STATS` | `set_debug` makes the policy step aside; stats lines describe the collections that ran | yes |
| finalizers / weakref callbacks of cyclic garbage | run when the collection that frees the cycle runs: later on deferred programs (garbage lifetime, §4.3) | allowed by `datamodel.rst:61-75` |

**Tests for (a).** (1) `benches/runtime/test_gcadapt.py` — the discriminating tests above (6/6
pass on 3.15.1 and 3.15.1t). (2) CPython's own modules, plain vs policy installed first
(`policy_suites.py`): **22 modules, 3,185 tests on 3.15.1 and 23 modules, 3,192 tests on
3.15.1t (incl. `test_free_threading.test_gc`): 0 differences** (test_gc, test_weakref,
test_finalization, test_gc_stats, test_sys, test_copy, test_deque, test_descr, test_dict,
test_list, test_set, test_generators, test_coroutines, test_class, test_ordered_dict,
test_functools, test_exceptions, test_contextlib, test_scope, test_json, test_dataclasses,
test_tracemalloc; test_threading hangs under the runner's capture in *both* runs and was
dropped). Caveat, measured: these modules do not discriminate. test_gc, test_weakref and
test_finalization disable the GC in setUp (the policy steps aside at once) and test_generators
sets a threshold; re-run with stepping aside switched off (`adaptive:hooks=0`) they still
show 0 differences on both builds. So CPython's suite is a regression net, and the evidence for
(a) is the API table plus `test_gcadapt.py`. (3) The adversarial suite (§4.3) for the timing of
finalizers and memory.

### 5.2 (b) Peak-memory budget

**Budget: peak RSS ≤ 1.05× CPython's on every workload (median VmHWM over ≥ 3 processes) and
on every adversarial scenario, and RSS growth over a 40-iteration service run within ±1 MB of
CPython's.** Justification: (1) it mirrors the time budget (≤ 1.05) — a default must be
invisible to the memory dashboards users already watch; (2) upstream's own evidence: the 3.14
incremental GC was reverted after five patch releases for "significant memory pressure in
production" (`whatsnew/3.15.rst:1253-1259`) and the FT RSS heuristic was reverted in 3.15
(`NEWS 3.15.0b1:148-153`) — memory is where GC changes get rejected; (3) run-to-run noise: the
peak RSS of the same cell differs between processes by 0.1% (median), 1.1–1.3% (p90) and
3.4–3.6% (max) on these workloads (M, `policies1` default cells, both builds), so 5% is the
smallest budget we can verify per workload; (4) a hard bound by design: with the pause cap, a deferred full
collection never examines more objects than the largest one CPython's rule already ran in the
same process, so tracked garbage cannot push the heap beyond its earlier peak by more than the
young generations.

### 5.3 (c) Pauses no longer

**Per workload, over all automatic collections of the steady-state runs: the largest
collection and the 99th-percentile collection, in objects examined (deterministic) and in ms
(quiet box), ≤ 1.10× CPython's; the worst 100-ms window's GC load not higher.** Objects
examined is the pause proxy (time per object is ~constant for a given generation within a
process: 250–600 ns for full collections, M);
1.10 is the noise allowance of the ms measurement.

### 5.4 (d) Stepping aside

**Triggers**: the program calls `gc.set_threshold`, `gc.enable`, `gc.disable`, `gc.freeze`,
`gc.unfreeze` or `gc.set_debug` (or GC is disabled / threshold0 is 0 at launch). **Effect**:
the real thresholds become exactly what the program set (applied over the visible vector),
the observer leaves `gc.callbacks`, and the policy never acts again in that process.
**Not triggers**: `gc.collect()` (explicit collections are honoured as they are — pytest,
networkx and 40 of the top-1000 packages call it), reads (`get_*`, `is_*`). **Mechanism in the
product**: `ml_meth` swaps on the six entries of `GcMethods` (`Modules/gcmodule.c:523-543`,
writable, shared by all interpreters → the swapped functions must look up the calling
interpreter's policy state) plus `get_threshold`; identity, `__module__`, and
`from gc import set_threshold` taken earlier are preserved. The prototype replaces module
attributes instead (enough to measure, not to ship). **Tests**: `test_gcadapt.py` (each
trigger, both builds ✓); CPython modules that tune the GC step aside where expected
(test_gc/test_weakref/test_finalization at `disable`, test_generators at `set_threshold`,
test_free_threading.test_gc at `freeze`) with 0 differences. **Reach**: 18 of the top-1000
packages tune the GC in library code (mypy, ray, torch, gevent, dask, ipython, scipy, cython,
boltons, …; 1.1% download-weighted, M): in those programs the policy hands control back at
their first call.

### 5.5 Verdicts for the prototype (v4)

| criterion | definition (above) | v4, 3.15.1 | v4, 3.15.1t | evidence |
|---|---|---|---|---|
| (a1) configuration round-trip | getters return what the program set | **met** (get_threshold virtualised; nothing else touched) | **met** | `test_gcadapt.py` 6/6 both builds |
| (a2) inventory exact | no object hidden | **met** (never freezes) | **met** | same |
| (a3) truthful reports, explicit collect untouched | | **met** | **met** | CPython modules 0 diffs (22 / 23 modules); `test_explicit_collect_is_cpythons` |
| (a4) one visible artefact | | one `gc.callbacks` entry; `get_threshold()` ≠ thresholds in force while deferring | same | stated, not hidden |
| (b) peak RSS ≤ 1.05× | | **met** on 34/34 (max 1.03×) and 30/31 wave-2 (pipeline_queue 1.10×, unexplained, threaded) and 3/3 adversarial (max 1.00×); long runs: uvicorn grows +1.4 MB more over 20 iterations (borderline) | **met** on 34/34 (max 1.02×), 31/31 (max 1.01×), 3/3 adversarial (max 1.04×) | §4.2–4.4, dev box; quiet box pending |
| (c) largest and p99 collection ≤ 1.10× | | **met** (max 1.04× / 1.01×) | **met** (1.05× / 1.05×; adversarial steady 1.11× — at the edge) | collection size; ms on the quiet box pending |
| (d) steps aside on GC tuning | | **met** | **met** | tests + CPython modules that tune the GC |

**Overall: v4 meets (b), (c) and (d) on every measured cell but three borderline ones, and (a)
under the "documented semantics" reading — with two declared deviations a strict reading
would count: `get_threshold()` reports the configured, not the in-force, thresholds, and
`gc.callbacks` holds one extra entry.** The strict reading is not met by Rustique today either
(§5.1).

## 6. Candidates

Format of the shared brief. Shares are % of steady-state `run()` time on the 34 wave-1
workloads unless stated; I = mean share × (1 − 1/s) as the % of mean run time saved.

| candidate | layer | entry points + mechanism | share mean / max (which) | s on the targeted part | exact subset & what falls back | floor vs current | risks | upstream overlap | reach | prototype result (M) | evidence |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **R1 GC policy tier: yield-driven deferral of full collections with pause cap (v4)** | runtime | policy: one `gc.callbacks` observer (a Rust builtin in the product) + `ml_meth` swaps on `gc.set_threshold/enable/disable/freeze/unfreeze/set_debug` (step aside) and `gc.get_threshold` (virtualise), `Modules/gcmodule.c:523-543`; switch `RUSTIQUE_GC=adaptive`; no other API | automatic GC 6.9% / 44% (pylint), 3.15.1t 5.6% / 38%; **targeted** (full collections finding < 1%): 1.6% / 34% (billing), t 1.9% / 13.9% (billing); wave-2: 0.35% / 5.9% (rules_engine), t 1.9% / 21.7% (request_pool) | GC work on billing 0.53× (GIL) / 0.31× (t), state_snapshot 0.26× (t), xml_feeds 0.26× (t), request_pool 0.29× (t), rules_engine 0.66× / 0.40× | all programs; results unchanged by construction (only trigger points move); steps aside when the program tunes the GC; explicit collections untouched | floor: 0 for collections that find nothing, vs 1.6–1.9% mean now; v4 takes ~1/3 (GIL) to 1/2 (t) of it | finalizers/weakref callbacks of cyclic garbage run later (garbage lifetime up to 1.9× on FT burst); visible `gc.callbacks` entry; `get_threshold` reports configured values; `GcMethods` shared by all interpreters (per-interpreter state needed); FT `set_threshold` stops the world (only on change); a workload whose garbage arrives after back-off holds ≤ 10% of its peak live heap extra | none in tree: 3.14.5 reverted the incremental GC, 3.15 reverted FT's RSS deferral; 3.16.0a1 still generational with (2000, 10, 10) (✓) | programs that build large acyclic structures (ETL, compilers/linters' caches, document processing) and FT services with worker threads: 3/34 + 2/31 workloads take ≥ 3% (I) | whole set GC work **0.886 / 0.772**, est. **0.53% / 1.01%** of mean run time (max 18.7% billing / 18.7% request_pool); billing wall 0.71× (GIL, dev box); RSS ≤ 1.03×; collection size ≤ 1.05×; adversarial RSS ≤ 1.04× | §4, §5, `benches/runtime/gcadapt.py` |
| R2 opt-in fast exit (`gc.freeze()` in Rustique's last `atexit` slot) | runtime | policy (atexit) | exit-time GC: 6.0% of whole-process time mean, 40.6% pylint, 16–21% pytest/sphinx/sqlalchemy/fastapi (w2-attribution); exit 0.15–1.33 s per process (M, §4.6) | pylint exit 1330 → 17 ms, sphinx 419 → 21, pytest 440 → 33 (GIL) | **not exact**: cyclic garbage at exit is never finalized (no `__del__`, weakref callbacks, `ResourceWarning`, flush of cyclic open files); acyclic objects still freed | floor ≈ `os._exit` 5–13 ms | data loss if an open buffered file is only reachable from a cycle at exit; tools relying on exit finalizers | none | CLI tools (linters, test runners, build tools) | measured on 8 workloads × 2 builds | §4.6, `exitcost.py` |
| – kernels that allocate less | runtime | (the kernels' own entry points) | allocations + freelist pops 0.36–0.90× with wave-1 kernels on 14/34 workloads | already in the kernels' speedups | – | result objects are irreducible | – | – | – | Py_STATS plain vs enabled | §3.1 |

Scores (§2 of IMPACT_ATLAS: I × F × (1 − K) × (1 − U) × (0.5 + R)), my estimates:

| candidate | I | F | K | U | R | score | reasons |
|---|---:|---:|---:|---:|---:|---:|---|
| R1 on 3.15.1 | 0.53 | 0.8 | 0.3 | 0.15 | 0.25 | **0.19** | F: works with public APIs today; product = Rust observer + 7 `ml_meth` swaps + per-interpreter state + tests. K: timing of finalizers, the two declared (a) deviations. R: few programs, big wins |
| R1 on 3.15.1t | 1.01 | 0.8 | 0.3 | 0.15 | 0.3 | **0.38** | every FT collection is a full collection; FT services with worker threads (request_pool) |
| R2 (opt-in only) | ≈ 5 (I: whole-process view, finalization 6.0% of which most is GC) | 0.9 | 0.6 | 0.1 | 0.4 | ≈ 1.5 | large but a behaviour change: never a default; the score overstates it because the brief's formula has no "opt-in only" term |

## 7. Settled / out of reach

| item | verdict | because (M unless I) |
|---|---|---|
| `PYTHONMALLOC=mimalloc` on the GIL build / allocator choice at launch | **settled: no** | 1.051× slower geomean, worst 1.30×, RSS +7.5%; cannot be switched after preconfig anyway (§3.3) |
| freelist sizes / hit rates | out of reach | compile-time constants; already 13–71% of creations |
| deallocation (7.4%) | out of reach | the program's own objects dying; identical by construction |
| GC traversal in Rust | out of reach | `tp_traverse` slots are CPython's (and every extension's); the collector is memory-bound (250–600 ns per object); no entry point to swap `gc_collect_main` |
| static thresholds as a default (`thr:*`) | settled: no | RSS 1.25–3.4×, young pauses 5–25× (§4.2); users already have `gc.set_threshold` |
| `gc.freeze()`-based policies | settled: no (default and tier) | hide objects from `gc.get_objects()`/`get_referrers()` (fails (a2)), frozen garbage never collected; FT freeze is a stop-the-world heap walk |
| exact skipping of exit-time collections | out of reach | the proof needs the shutdown collection itself; every process has finalizable objects and hundreds of weakref callbacks at exit (§4.6) |
| young-generation tuning (threshold0/threshold1) | settled: not a default | raising threshold0 cuts GC work a lot (thr:10000: 0.575×, pylint 0.54× wall) only because it also delays middle and full collections; it lengthens every young pause 5× and costs up to 1.25× RSS; threshold1 alone moves little (each survivor is examined once per generation whatever its value). A throughput knob users already have |
| explicit `gc.collect()` | untouchable | documented API; 40 of the top-1000 packages call it; pytest's session-end collect is 46% of pytest_networkx's steady state on this set (an artefact of one `pytest.main()` per `run()`) |

## 8. Recommendation

1. **Ship R1 as an opt-in tier now** (`RUSTIQUE_GC=adaptive`), Phase 2: port `gcadapt.py` to
   Rust — the observer as a builtin in `gc.callbacks`, `ml_meth` swaps for stepping aside and
   `get_threshold`, per-interpreter state — and keep `test_gcadapt.py`, `adversarial.py` and
   `policy_suites.py` as its gates. Expected: −0.5% (GIL) / −1.0% (t) of mean run time,
   −20–30% on programs that build large acyclic heaps (billing 0.71×), more on FT services.
2. **Default: yes on the evidence of (b)(c)(d) once the quiet box confirms the wall, pause-ms
   and RSS numbers, and only after a DECISIONS entry adopts §5.1's definition of (a)** —
   documented semantics hold, configuration round-trips, timing-derived values
   (`get_count`, `get_stats`, callback counts, `collect()` return values) are not part of the
   contract, as they already are not for Rustique's kernels. Start with the free-threaded
   build (twice the benefit, same rule); the GIL build's expected gain (0.5% mean) is small
   enough that the risk budget may favour leaving it opt-in.
3. **R2 (fast exit) only as an explicitly documented opt-in for CLI tools**, never default.
4. **Stop**: allocator switching, freeze-based or static-threshold policies, any attempt to
   make the collector itself faster from entry points.
