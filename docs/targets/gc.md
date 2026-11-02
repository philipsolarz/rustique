STATUS: wave 2, workstream `gc`, 2026-11-02. Every number is **measured** (M: how and where)
or **inferred** (I: from what). "Dev box" = the shared c7a.8xlarge (load average 12–100 during
these runs: wall times there are noise, instruction counts and objects examined are not);
quiet-box wall/pause/RSS: `benches/runtime/quiet_driver.sh` (§8), the lead's Phase 3.

# Target group `gc`: the adaptive GC policy (opt-in tier)

One target, `gc.adaptive`, in the opt-in tier `gc` (DECISIONS §10.3): **full collections that
keep finding nothing are deferred**, never past a pause cap; as soon as they find garbage
again CPython's own rule is back. Young and middle collections and explicit `gc.collect()`
are untouched, and the program's own GC tuning hands control back for the rest of the
process. Turn it on with `RUSTIQUE_TIERS=gc` (with `RUSTIQUE=1` at startup, or before
`rustique.enable()`), or `rustique._accel.enable([...], tiers=["gc"])`
(`rustique.enable(tiers=["gc"])` once the public switch has it): `TargetSpec.opt_in =
Some("gc")`; without the tier the target shows state `disabled`, reason "opt-in tier:
RUSTIQUE_TIERS=gc".

**Criterion (a): no documented-API deviation.** The prototype (`benches/runtime/gcadapt.py`,
w2-runtime §5.1) had two declared deviations: `gc.get_threshold()` reported the configured
thresholds while the collector ran with others, and `gc.callbacks` held one extra entry.
Both are gone in the product (§3): the policy never touches a threshold (it moves the
denominator of CPython's own 25% rule, `long_lived_total`, an internal field no documented
API reports), and the observer sits in a private list that `gcstate->callbacks` points to
and that forwards to the program's `gc.callbacks` exactly as CPython's
`invoke_gc_callback` does. Values that depend on collection timing (`get_count`, `get_stats`,
callback invocation counts, `collect()` return values) are outside the contract (DECISIONS
§10.3, ACCELERATION §5.8), as for every kernel that allocates less than CPython.

## 1. The rule (w2-runtime §4.1, prototype v4)

A full collection (GIL build: generation 2; free-threaded build: every collection, each walks
the whole heap) may run once the old generation has grown by `G` since the last one. CPython's
rule is `G = 1/4` (`Python/gc.c` `gc_select_generation`: `long_lived_pending >=
long_lived_total / 4`; `Python/gc_free_threading.c` `gc_should_collect`: `count >=
long_lived_total / 4`). The policy sets `G = 2^k / 4`, `k ∈ 0..=3`, from what full collections
find: yield (`(collected + uncollectable) / candidates`) < 1% → `k + 1`; ≥ 5% → `k = 0`; in
between `k − 1`.

**Pause cap.** A deferred full collection is released as soon as it would examine more than
`cap ×` the objects of the largest *legitimate* full collection so far — one run under CPython's
rule (`k = 0`) counts with everything it examined, one the policy deferred only with its
survivors, so deferred garbage never raises the cap (no ratchet).

| | GIL build | free-threaded build |
|---|---|---|
| next full collection's size | `gcstate->heap_size` (tracked objects: exactly what a full collection examines; the prototype estimated it from `sys.getallocatedblocks()`) | survivors + `young.count` (net allocations), in the unit of `long_lived_total` (every object of the GC heap) **and** in `candidates` (marked objects): the cap holds in both |
| cap | **1.1** (as decided) | **1.05** (§5.4: the trigger counts net allocations, so cyclic garbage that waits for a deferred collection fills the heap one for one; at 1.1 the adversarial `late_cycles` peaks at 1.07× RSS, prototype and port alike; the budget is 1.05×) |
| what the policy writes | `long_lived_total = survivors << k` (CPython then needs `pending ≥ 2^k/4 × survivors`), back to `survivors` when the cap releases (checked at every generation-1 collection) | `long_lived_total = max(survivors, 4 × limit)`, `limit = max(threshold0, min(2^k/4 × survivors, room under the cap))`; CPython's own value is put back in the `start` callback of every collection (the mark phase's prefetch heuristic reads it) |

## 2. Mechanism

| piece | where (`crates/rustique-py/src/targets/gc/`) | what |
|---|---|---|
| layout | `layout.rs` | the main interpreter's `struct _gc_runtime_state` located through `_PyRuntime.debug_offsets` (the table remote debuggers read): cookie, `version == Py_Version`, `free_threaded`, `interpreter_state.gc`, `gc.size` and `gc.collecting` must equal the values recorded per (minor, build); then every field is checked against a freshly executed private instance of the `gc` module (`garbage` and `callbacks` pointers, `isenabled()`, `get_threshold()`, `get_debug()`), and at the first full collection survivors ≤ examined. Any mismatch refuses (`layout`; `symbol` if `_PyRuntime` is not exported) |
| observer + forwarder | `observer.rs` | `gcstate->callbacks` → a private, untracked `[observer]` list; the program's list is kept (its reference moved from the collector state). Swapped only while holding the collector's own `collecting` flag (CAS 0→1: no collection, so no callback loop, can be in flight) at install, `disable()` and exit |
| policy | `policy.rs` | the rule; writes `gcstate->long_lived_total` only, re-applied in the `stop` callback after CPython resets it at each full collection |
| hooks | `mod.rs` | `ml_meth` of `gc.set_threshold`, `enable`, `disable`, `freeze`, `unfreeze`, `set_debug` (static `GcMethods` table, fingerprinted per minor): CPython's function first (same validation, errors, effect), then — if it succeeded, the policy is watching and this is the main interpreter — step aside |
| `gc.collect` hook | `mod.rs`, `observer.rs` | `ml_meth` of `gc.collect`: for the duration of an explicit collection `gcstate->callbacks` points back at the program's own list (CPython runs exactly its plain path: no `info` dict when the list is empty, the program's callbacks called by CPython itself), then the private list again, and the policy's value is re-applied if the collection reset it. Explicit collections no longer feed the rule. Window: GIL, the swap happens with `collecting == 0` and no Python code runs until the collection takes the flag; free-threaded, the swap holds the `collecting` flag, and a collection another thread starts in the window runs with the program's list (CPython's plain behaviour, the policy misses it) while this `collect()` returns 0, as it can without Rustique. A `collect()` from inside a collection (callback, finalizer) swaps nothing |

**Every reader of `long_lived_total`** (grep of 3.14.9 and 3.15.1, `Python/`, `Modules/`,
`Include/`): the generation choice (`gc.c` `gc_select_generation`) and the trigger
(`gc_free_threading.c` `gc_should_collect`) — the point; the free-threaded mark phase's
`use_prefetch = long_lived_total > 200000` (runs after the `start` callback, where the policy
restores CPython's value); CPython resets it at each full collection (`gc.c:1534`,
`gc_free_threading.c:2164`) and the observer re-applies the policy's value in that
collection's `stop` callback (`tests/test_gc_tier.py::test_reinflated_after_every_full_collection`);
`_testinternalcapi.get_long_lived_total()` (CPython's internal test module, not a documented
API; its one test, FT `test_gc.test_tuple_untrack_counts`, passes with the policy armed, §6).
Nothing else reads it.

**Validated at install**, never armed for first-call validation (ACCELERATION §3.2): the
policy acts from `enable()` on, without any call of its sites (in arming mode its `prepare`
declares no site, so the registry validates it at once).

**`disable()`** (`TargetSpec.on_disable`) and the exit hook restore at once: CPython's value in
`long_lived_total`, the program's list in `gcstate->callbacks`. When `disable()` is called from
inside a GC callback (a collection is running, the flag cannot be taken) the observer stays in
place, forwarding only, until the next restore (`test_disable_from_inside_a_callback`).

## 3. Criterion (a): what each documented API sees

| API | with the tier | same as CPython? |
|---|---|---|
| `gc.get_threshold()` | the thresholds in force: the policy never changes them | **yes** (the prototype's deviation is gone) |
| `gc.callbacks` | the program's list: same object, length, contents, `sys.getrefcount`, `gc.get_referrers` (M: `test_installed_armed_and_invisible`, `test_callbacks_identity_length_and_refcount`) | **yes** (the prototype's deviation is gone) |
| callback invocations | the forwarder iterates the list CPython would iterate (`gcstate->callbacks` at install: rebinding the module attribute changes nothing, as in CPython), by index re-reading the length before each entry, with CPython's own `(phase, info)` argument array (one `info` dict per invocation, shared by every callback, mutations visible to the later ones), reports a raising entry with `PyErr_FormatUnraisable("Exception ignored while calling GC callback %R", cb)` and carries on; the policy runs first, in both phases | **yes**: `tests/test_gc_tier_callbacks.py`, differential plain vs tier vs tier-with-policy-armed, incl. 100 Hypothesis programs of list mutations, raising/uncallable callbacks, `gc.collect()` inside callbacks |
| `gc.set_threshold/enable/disable/freeze/unfreeze/set_debug` | CPython's function (errors, messages, effect), then step aside | yes (`test_set_threshold_errors_are_cpythons_and_do_not_step_aside`) |
| `gc.isenabled/get_debug/get_freeze_count` | untouched | yes |
| `gc.collect(g)` | CPython's function, run with the program's own callback list (the observer detached for its duration); runs generation `g`, also while the policy defers; does not feed the rule | semantics yes; return value timing-dependent |
| `gc.get_count()`, `gc.get_stats()` | the collections that ran (fewer full ones); GIL `count[2]` can exceed threshold2 while deferred, a state CPython reaches whenever its 25% rule holds a full collection back | timing-dependent (outside the contract) |
| `gc.get_objects/get_referrers/get_referents/is_tracked/is_finalized` | untouched; the observer and its list are untracked (never listed) | yes (`test_no_freeze_inventory_exact`) |
| `gc.garbage`, `DEBUG_*` | the same objects, possibly later | timing-dependent |
| finalizers / weakref callbacks of cyclic garbage | run when the collection that frees the cycle runs: later on deferred programs (§5.4 garbage lifetime) | allowed (`datamodel.rst` "do not depend on immediate finalization") |

Known limits (not documented-API deviations; tested): a re-executed `gc` module
(`importlib.reload(gc)`, or a fresh import after `del sys.modules['gc']`) binds its `callbacks`
attribute to whatever `gcstate->callbacks` holds; the observer notices (a third reference to the
private list) at the next collection and binds the program's list back, moving entries appended
meanwhile (`test_reload_gc_keeps_the_program_list`: the invocations are identical to CPython's).
On the free-threaded build an `enable()` that races with a running collection refuses ("a
garbage collection is running") and is retried by the next `enable()`.

## 4. Stepping aside (criterion d)

Triggers: a successful call of `gc.set_threshold`, `gc.enable`, `gc.disable`, `gc.freeze`,
`gc.unfreeze` or `gc.set_debug` in the main interpreter, through any reference (`from gc import
set_threshold` taken before `enable()` included: the site is the method table); and, at
install, a GC configuration the program already changed (disabled, thresholds ≠ (2000, 10, 10),
debug flags, frozen objects). Effect: `long_lived_total` gets CPython's value at once, the
policy never acts again in the process (`rustique._accel._t_gc.report()["stepped_aside"]` says
why); the forwarder stays (invisible). Not triggers: `gc.collect()`, reads, failed calls,
calls in subinterpreters, the C API (`PyGC_Enable/Disable` change nothing the policy relies
on). Cost after stepping aside: one relaxed load per hooked call (§7).
Tests: `tests/test_gc_tier.py` (each trigger, before-enable states, subinterpreter, captured
reference), both builds.

## 5. Results

### 5.1 The 65 workloads (dev box, deterministic proxies)

M: `benches/runtime/gcprobe.py sweep all`, 3 processes × 5 steady-state runs per cell, cells of
a round interleaved; baseline `loaded` = `import rustique` with nothing enabled (so the tier's
numbers exclude the cost of loading the extension; against plain CPython the extension adds
~1–2 MB of RSS); GC work = objects examined by automatic collections per run.

Whole set (65 workloads), `tier` vs `loaded`:

build | GC work × (objects examined, whole set) | best | largest collection × worst (n > 1.10) | p99 collection × worst | peak RSS × worst (n > 1.05) | digests
---|---|---|---|---|---|---
3.15.1 | **0.948** | 0.649 (rules_engine) | 1.05 (pipeline_queue; 0) | 1.04 | 1.09 (config_merge; 2) | 65/65 equal
3.15.1t | **0.823** | 0.257 (state_snapshot) | 1.18 (report_render; 1) | 1.18 | 1.30 (csv_etl; 2) | 65/65 equal

The peak-RSS outliers are workloads the policy cannot touch: config_merge (GIL, 1.7% GC, work
1.00) and csv_etl / sqlalchemy_orm (t: no automatic collection at all in the measured runs),
where one process of three peaks higher in either cell (FT allocator noise: csv_etl
loaded 56–69 MB, tier 59–90 MB across processes). report_render (t, 2% GC): the cap is relative
to the largest legitimate collection *of the process*, which includes the larger collections
of its setup phase (58 k objects); the steady-state runs then see one collection of 52.8 k
where CPython's steady state peaks at 44.6 k (1.18×, a sub-millisecond pause).

Per workload (automatic GC ≥ 2% on either build; wall × is the dev box's paired-round median,
noise ±3–5%):

workload | GC % (3.15.1) | work × | largest coll. × | peak RSS × | wall × | GC % (t) | work × | largest coll. × | peak RSS × | wall ×
---|---|---|---|---|---|---|---|---|---|---
custom/event_ledger | 2.4 | 1.00 | 1.00 | 1.01 | 0.97 | 2.1 | 0.65 | 1.05 | 1.00 | 0.98
custom/fulfilment_sim | 2.2 | 1.00 | 1.00 | 1.04 | 1.00 | 1.4 | 0.78 | 1.04 | 1.03 | 0.99
custom/order_domain | 11.6 | 1.00 | 1.00 | 1.01 | 0.96 | 6.5 | 1.11 | 1.00 | 1.00 | 0.98
custom/orm_mapper | 3.9 | 1.00 | 1.00 | 1.01 | 1.01 | 6.4 | 0.66 | 0.97 | 1.01 | 0.97
custom/rules_engine | 8.4 | **0.65** | 1.00 | 1.01 | 0.95 | 14.7 | **0.38** | 1.00 | 1.01 | 0.92
data/billing_engine | 42.7 | **0.79** | 1.02 | 1.01 | **0.82** | 15.1 | **0.50** | 1.06 | 1.00 | 0.91
data/report_render | 1.1 | 1.00 | 1.00 | 1.02 | 1.00 | 2.2 | 0.42 | 1.18 | 0.98 | 0.99
data/state_snapshot | 18.2 | 1.04 | 1.00 | 1.01 | 1.03 | 12.0 | **0.26** | 1.00 | 1.00 | 0.95
data/tz_events | 0.0 | 1.00 | 1.00 | 1.02 | 1.01 | 6.8 | 1.00 | 1.00 | 1.02 | 1.00
data/xml_feeds | 21.9 | 1.00 | 1.00 | 1.01 | 1.02 | 10.6 | **0.26** | 0.63 | 1.01 | 0.91
devtools/black_format | 23.7 | 1.00 | 1.00 | 1.01 | 1.00 | 11.8 | 1.03 | 0.99 | 1.02 | 1.00
devtools/coverage_pyflakes_tests | 17.0 | 1.00 | 1.00 | 1.01 | 1.00 | 38.4 | 1.00 | 1.00 | 1.01 | 1.01
devtools/pyflakes_stdlib | 35.4 | 1.00 | 1.00 | 1.01 | 1.01 | 23.2 | 1.00 | 1.00 | 1.03 | 1.00
devtools/pylint_stdlib | 47.2 | 1.00 | 1.00 | 1.00 | 1.00 | 14.8 | 1.00 | 1.00 | 1.00 | 0.99
devtools/rich_render | 1.9 | 1.00 | 1.00 | 0.99 | 1.01 | 4.8 | 0.51 | 0.98 | 1.03 | 0.97
devtools/sphinx_autodoc | 3.9 | 1.00 | 1.00 | 1.01 | 1.01 | 10.2 | 1.00 | 1.00 | 1.01 | 1.00
devtools/sqlglot_transpile | 10.7 | 1.00 | 1.00 | 1.02 | 1.00 | 18.9 | 1.00 | 1.00 | 1.01 | 1.00
frameworks/drf_api | 6.6 | 1.00 | 1.00 | 1.01 | 1.00 | 10.6 | 1.00 | 1.00 | 1.01 | 1.00
frameworks/strawberry_graphql | 11.3 | 1.00 | 1.00 | 1.01 | 0.99 | 21.9 | 1.00 | 1.00 | 1.01 | 0.98
libs/bs4_scrape | 21.3 | 1.00 | 1.00 | 1.01 | 1.00 | 10.7 | 1.00 | 1.00 | 1.02 | 1.00
services/job_worker | 3.5 | 1.00 | 1.00 | 1.02 | 1.01 | 22.3 | 1.00 | 0.99 | 1.02 | 1.01
services/uvicorn_fastapi | 8.5 | 0.97 | 1.00 | 1.02 | 0.99 | 1.8 | 1.00 | 1.00 | 1.01 | 0.97
threads/pipeline_queue | 0.3 | 1.08 | 1.05 | 1.05 | 1.00 | 2.9 | 1.00 | 1.00 | 1.03 | 1.01
threads/request_pool | 6.1 | 1.00 | 1.03 | 1.01 | 1.01 | 24.0 | **0.51** | 1.07 | 1.00 | 0.92
web/email_mime | 3.4 | 1.00 | 1.00 | 1.03 | 0.99 | 6.6 | 1.00 | 1.00 | 1.02 | 1.01
web/sqlalchemy_orm | 3.7 | 0.99 | 0.11 | 0.99 | 1.01 | 0.0 | – | – | 1.12 | 0.98

(M: `benches/runtime/gcprobe.py sweep all --policies "loaded;tier"`, run
`final-sweep`; tables `gctable.py scoreboard`, `policyrows.py`. The port reproduces the
prototype: on the three workloads compared directly, `adaptive` (gcadapt.py) vs `tier`, GIL,
2 × 8 runs: billing 0.67 / 0.65 GC work, rules_engine 0.80 / 0.80, state_snapshot 1.07 / 1.08.
GC work on billing depends on how many runs one process makes: the deferral spans runs.)

Reading it: **the policy acts only where full collections find nothing**; every workload whose
GC finds garbage (pylint 5%, black 26%, pyflakes 38%, coverage 59%, sqlglot 62%, strawberry 55%,
bs4 41% yield) stays at CPython's rule (work 1.00). On the GIL build that leaves billing,
rules_engine and uvicorn; on the free-threaded build, where every collection walks the heap, the
same rule takes 50–74% of the GC work of nine workloads. state_snapshot (GIL) is 1.04×: its old
generation returns to the same peak every run, the cap releases each deferred collection a
little later and larger than CPython's (prototype: 1.04–1.07 as well).

### 5.2 Target T6 (workloads whose GC share is > 10%)

T6 (IMPACT_ATLAS §14.4): workloads whose GC share is > 10% reach a geometric mean ≤ 0.87, peak
RSS ≤ 1.05×, max pause ≤ the default's; every other workload ≤ 1.02. Dev-box proxies (the wall
ratio is the quiet box's, §8): GC work, the estimate `1 − GC share × (1 − work)` (it
under-states full collections, which cost more per object than young ones: billing measures
0.82 where it estimates 0.91), instructions of the measured runs (`tier_instr.py`, 3 processes
× 5 runs; they under-state GC savings the most, a full collection is memory-bound), and the
dev box's paired wall ratio.

build | GC > 10% workloads | GC work × (those that change) | est geomean | instr geomean | dev wall geomean | largest collection × max | peak RSS × max | others: worst est / instr
---|---|---|---|---|---|---|---|---
3.15.1 | 11 | billing 0.79 (others 1.00; state_snapshot 1.04) | 0.992 | 0.996 (10 measured) | 0.983 | 1.02 | 1.018 | 1.000 / 1.002
3.15.1t | 15 | billing 0.50, state_snapshot 0.26, xml_feeds 0.26, rules_engine 0.38, request_pool 0.51 | 0.969 | 0.989 | 0.972 | 1.07 | 1.025 | 1.007 / 1.003

**T6's 0.87 is out of reach for the decided rule** (I, from the table and w2-runtime §3.1): the
rule takes only full collections that find < 1% garbage, and 8 of the 11 GIL workloads above
10% GC find real garbage in theirs (pylint 5%, black 26%, pyflakes 38%, bs4 41%, strawberry 55%,
coverage 59%, sqlglot 62% yield): their GC work is 1.00 by design. Even removing *all*
automatic GC from billing alone moves the GIL geomean by ≤ 0.05. What would reach 0.87 trades
memory or pauses (w2-runtime §4.2: `thr:10000` gives pylint 0.54× wall at 1.15× RSS and 5× young
pauses), which the RSS and pause budgets of the same target exclude. "Max pause ≤ the
default's" also conflicts with the decided pause cap (1.1× / 1.05× by definition allows a
deferred collection up to that size; measured ≤ 1.02 GIL, ≤ 1.07 t on this set): a cap of 1.0
would satisfy it at the prior cost the atlas measured for v2 (GC work 0.95 / 0.85). Others:
≤ 1.007 estimated, ≤ 1.005 instructions everywhere, every digest equal.

### 5.3 End to end with the rest of Rustique

The tier on top of Rustique's default targets (`tier:all=1` vs `rustique`), instructions of
the measured runs (M: `benches/runtime/tier_instr.py`, run `final-instr`, 16 workloads with
the most GC, 3 processes × 5 runs, digests equal everywhere):

build | billing | state_snapshot | xml_feeds | rules_engine | request_pool | the other 11 (range)
---|---|---|---|---|---|---
3.15.1 | **0.965** | 1.004 | 1.000 | 0.989 | 0.996 | 0.998–1.005
3.15.1t | **0.952** | **0.960** | **0.964** | **0.978** | **0.974** | 0.998–1.003

The gains are the same with and without the rest of Rustique (`tier` vs `loaded`: billing 0.967
/ 0.957, state 0.998 / 0.962, rules 0.988 / 0.978, request_pool t 0.982): the kernels and the
policy act on different costs. (state_snapshot t: with Rustique's `deepcopy` kernel its young
collections fall from 94 to 3 per run, w2-runtime §3.1, and the policy's share of the
remaining work is 0.40 instead of 0.26.)

### 5.4 Adversarial programs (garbage that appears after the back-off)

`benches/runtime/adversarial.py` (w2-runtime §4.3): each run first builds a 600 k-object
acyclic structure (the policy backs off to k = 3), then produces cyclic garbage that lives long
enough to reach the old generation (`late_cycles`: 400 k self-referencing nodes in a 60 k FIFO
window; `burst`: three 300 k-node cyclic graphs; `steady`: a cyclic window from the start).
M: dev box, `benches/runtime/adv_tier.sh`, 5 processes × 4 runs per cell, `tier` vs `loaded`
(run `final-adv2`); `tier:cap=0` (no pause cap) vs plain CPython from run `final-adv`, 3
processes:

scenario | build | peak RSS × (max of processes) | median × | largest collection × | GC work × | without the cap: peak RSS ×
---|---|---|---|---|---|---
burst | 3.15.1 | 0.99 | 0.99 | 1.00 | 0.95 | 1.51
burst | 3.15.1t | 0.99 | 0.91 | 0.98 | 0.77 | 1.52
late_cycles | 3.15.1 | 1.00 | 1.00 | 1.00 | 1.00 | 1.05
late_cycles | 3.15.1t | **1.03** | 1.00 | **1.05** | 0.65 | 1.71
steady | 3.15.1 | 1.00 | 1.00 | 1.00 | 1.00 | 1.04
steady | 3.15.1t | **1.03** | 1.03 | **1.05** | 0.97 | 1.19

**Peak RSS ≤ 1.05× in every scenario on both builds with the cap; without it 1.5–1.7×.** On the
free-threaded build at cap 1.1 (the prototype's, and this port's first setting) `late_cycles`
reached 1.07–1.08× and `steady` 1.06× (runs `adv2`, `adv3`; the prototype measured 1.07× in the
same session), hence 1.05 there. Garbage lifetime (time from a sampled cycle becoming
unreachable to its collection): GIL p50 equal or lower; FT `late_cycles` 2.0× (123 vs 60 ms
p50), the price of deferral that `datamodel.rst` allows ("do not depend on immediate
finalization").

## 6. Conformance

CPython's own test modules, plain vs with the tier enabled before `test` is imported, every
test id's outcome compared (M, dev box, `benches/runtime/conformance.sh`: 3 parallel groups
per build, `CPYTHON_SRC` of the running patch release):

- **standard** (`benches/runtime/policy_suites.py --policy tier`: `RUSTIQUE_TIERS=gc` +
  `rustique.enable(only=["gc"])` before `test` is imported — the gate of
  `tests/conformance/run_target_suites.py`, whose environment-free `_takeover.enable` cannot
  request a tier): the modules that tune the GC make the policy step aside where expected
  (`gc.disable` in `test_gc`, `test_weakref`, `test_finalization`; `gc.set_threshold` in
  `test_generators`; `gc.freeze` in `test_free_threading.test_gc`), the forwarder serves every
  callback test;
- **armed** (`benches/runtime/policy_suites.py --policy tier:step_aside=0`): the policy is kept
  acting through the modules' own GC tuning (research switch), so deferral, re-inflation and
  the forwarder all run under `test_gc` (incl. `GCCallbackTests`), `test_weakref`,
  `test_finalization`, ... (e.g. 127 full collections seen by the policy during `test_gc`).

build | modules | tests | standard: differences | armed: differences
---|---|---|---|---
3.15.1 | 22 | 3,185 | **0** | **0**
3.15.1t | 23 (+ `test_free_threading.test_gc`) | 3,192 | **0** | **0**
3.14.9 | 21 (no `test_gc_stats`) | 3,086 | **0** | **0**
3.14.9t | 22 | 3,093 | **0** | **0**

Modules: `test_gc`, `test_weakref`, `test_finalization`, `test_gc_stats` (3.15), `test_sys`,
`test_copy`, `test_deque`, `test_descr`, `test_dict`, `test_list`, `test_set`,
`test_generators`, `test_coroutines`, `test_class`, `test_ordered_dict`, `test_functools`,
`test_exceptions`, `test_contextlib`, `test_scope`, `test_json`, `test_dataclasses`,
`test_tracemalloc` (+ `test_free_threading.test_gc` on t builds). The one failure of
`test_gc`, `test_weakref`, `test_sys` and `test_exceptions` on 3.15.1 fails identically
without Rustique (environment).

Rustique's own suites: `tests/test_gc_tier.py` (28), `tests/test_gc_tier_callbacks.py` (8,
one of them 50 Hypothesis programs by default, `RQ_GC_EXAMPLES`), `tests/test_gc_tier_threads.py`
(3): 39 pass on the four primary builds, alone and with the `interp` tier on too
(`RQ_GC_TIERS=gc,interp RQ_GC_ONLY=gc,interp`), and on 3.15.2, 3.15.2t, 3.14.10. The whole
`pytest -q tests` at the merged head (wave2 f127702): 3.15.1 2,533 passed + 1 failure of
`test_dx_estimate.py::test_signal_sampler_samples_cpu_not_waiting` under box load (passes
alone), 3.15.1t 2,548, 3.14.9 2,517, 3.14.9t 2,532 passed; `cargo clippy --workspace
--all-targets -D warnings` and `cargo fmt --check` clean.

## 7. Cost of the mechanism (callgrind)

M: `benches/runtime/gc_tier_cg.py` (benches/targets/_callgrind.py: fresh processes,
region-instrumented, per-execution instructions; the tier requested in code at the end of the
setup), dev box. "plain" runs without importing Rustique, so cells that walk the heap also count
the objects `import rustique` adds.

cell | 3.15.1 plain → tier | × | 3.15.1t plain → tier | ×
---|---|---|---|---
`gc.enable()` (hooked, steady state: one relaxed load) | 241 → 251 | 1.041 | 250 → 260 | 1.039
`gc.disable(); gc.enable()` | 476 → 496 | 1.042 | 492 → 512 | 1.040
`gc.set_threshold(2000, 10, 10)` | 957 → 967 | 1.010 | 1,179 → 1,189 | 1.008
`gc.set_debug(0)` | 247 → 257 | 1.040 | 256 → 266 | 1.038
`gc.freeze(); gc.unfreeze()` | 565 → 585 | 1.035 | 1,351,599 → 1,367,355 | 1.012
`gc.get_threshold()`, `gc.isenabled()` (not hooked) | 858, 246 | 1.000 | 867, 254 | 1.000
the first tuning call (steps aside, one-time) | 3,561 → 3,602 | 1.012 | 4,134 → 4,422 | 1.070 (once) |
`gc.collect()`, bare interpreter heap | 4,633,539 → 4,666,000 | 1.007 | 4,915,101 → 4,962,240 | 1.010 |
`gc.collect()`, 100 k-object heap | 27,183,803 → 27,216,189 | 1.001 | 29,944,612 → 29,976,485 | 1.001 |
automatic young collection (`[[] for _ in range(2001)]`, 100 k heap) | 897,016 → 884,757 | 0.986 | 908,489 → 908,667 | 1.000 |
`gc.collect(0)` + one program callback | 10,527 → 10,613 | 1.008 | 4,930,913 → 4,959,907 | 1.006 |
`gc.collect(0)`, empty young generation, no callback | 1,285 → 1,361 | **1.059** | 4,915,209 → 4,962,347 | 1.010 |
`gc.collect(1)`, empty generations | 1,313 → 1,389 | **1.058** | | |
install: `enable(["gc"], tiers=["gc"])` vs the same call without the tier | 28,187 → 64,277 | +36 k | 29,120 → 63,731 | +35 k |

- **Automatic collections** cost ~9.3 k instructions more (≈ 2–3 µs), of which ~8.5 k is
  CPython's own work: `invoke_gc_callback` builds the `info` dict (`Py_BuildValue` of five keys,
  twice per collection) whenever `gcstate->callbacks` is non-empty, and the observer's list never
  is; the observer, the policy and the forwarder are the remaining ~0.8 k (M: callgrind function
  profile). Against a collection that examines objects this is ≤ 1%: an automatic young
  collection (≈ 2,000 objects) is 0.98–1.00×, any full collection ≥ 30 k objects.
- **Explicit collections** run without the observer (the `gc.collect` hook, §2): +76 instructions
  on the GIL build (the interpreter check, the swap and the swap back, the re-application test).
  **Listed opt-in exceedance:** `gc.collect(0)`/`(1)` on an empty generation, the cheapest
  collection there is (1.3 k instructions), is 1.059× (+76 instructions; it was 8.4×, +9.3 k,
  before the hook); with a program callback 1.008×; a full `gc.collect()` 1.005×. On the
  free-threaded build every collection walks the heap (≥ 4.9 M instructions in a bare
  interpreter): 1.006–1.010.
- Hooked calls: +10 instructions in steady state (one relaxed load and the indirect call),
  ≤ 1.042 on the cheapest one (`gc.enable()`).
- Install: +36 k instructions over the registry's own `enable()` pass (budget 0.1 M): the
  layout is located once per process (`_PyRuntime.debug_offsets`), the six sites come from the
  static method table without creating a module object.

## 8. Quiet-box run (the lead's Phase 3)

`benches/runtime/quiet_driver.sh <host>` (from this checkout): builds release wheels for
3.15.1 and 3.15.1t on the dev box, deploys `benches/` and the wheels, runs
`benches/runtime/run_quiet.sh` (stages `venvs` → family venvs with the wheel; `set`, `extra`:
the 65 workloads, `default` vs `tier`, 5 processes × 5 runs, one L3 domain per build;
`adv`: the adversarial suite with `tier:cap=0`; `services`: 40-iteration services;
`e2e`: `rustique` vs `tier:all=1` on the GC > 10% set) and pulls results every 2 minutes.
Tables: `gctable.py scoreboard DIR --base default`, `policyrows.py DIR --policy tier`,
`advtable.py DIR`, `longtable.py`. It decides T6's wall ratios, pause milliseconds and RSS.

## 9. Versions and files

Validated: 3.14 and 3.15, GIL and free-threaded (layout tables in `layout.rs`, fingerprints of
the six hooked functions identical on all four primary builds); recorded on 3.15.2, 3.15.2t and
3.14.10 (the tier installs, its 37 tests pass). Other minors, pre-releases and
debug builds refuse (registry gate). Tests: `tests/test_gc_tier.py` (mechanics, a1–a3, d),
`tests/test_gc_tier_callbacks.py` (forwarder, differential + Hypothesis),
`tests/test_gc_tier_threads.py` (allocating threads, callback mutation, step-aside races,
enable/disable under load). Benchmarks and tools: `benches/runtime/` (`gcprobe.py`,
`gcpolicy.py` policies `default`/`loaded`/`rustique`/`tier[:k=v]`, `gctable.py`,
`policyrows.py`, `t6table.py`, `advtable.py`, `adv_tier.sh`, `tier_instr.py`, `gc_tier_cg.py`,
`conformance.sh`, `setup_tier.sh`, `run_quiet.sh`, `quiet_driver.sh`; the prototype
`gcadapt.py` and its tests stay as the reference).

## 10. Decisions and open issues

1. **Mechanism** (decided): writing the calibrated collector fields (`long_lived_total`,
   `callbacks`) is an opt-in-tier mechanism (DECISIONS entry); not a default.
2. **T6** (decided): reported as not met (§5.2: the rule takes only near-zero-yield full
   collections; estimates 0.99 GIL / 0.97 t, instructions 0.996 / 0.989).
3. **Pause budget of the tier** (decided): every collection ≤ 1.1× (GIL) / 1.05× (t) of the
   largest default collection replaces "≤ the default's" for the opt-in tier; "no longer
   pauses" stays a default-eligibility criterion. The free-threaded cap of 1.05 is confirmed.
4. **Listed opt-in exceedance** (§7): an explicit `gc.collect(0)`/`(1)` on an empty generation
   is 1.059× on the GIL build (+76 instructions over 1.3 k; 8.4× before the `gc.collect` hook).
   Every other cell ≤ 1.05.
5. **Known limits** (not documented-API deviations): `importlib.reload(gc)` shows the
   observer's private list in `gc.callbacks` until the next collection repairs it (§3); an
   `enable()` that races with a running collection on a free-threaded build waits up to 0.2 s,
   then refuses ("a garbage collection is running") until the next `enable()`.
