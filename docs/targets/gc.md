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
| `gc.collect(g)` | never intercepted: runs generation `g`, also while the policy defers; its collections feed the rule | semantics yes; return value timing-dependent |
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

PENDING-TABLES

### 5.2 Target T6 (workloads whose GC share is > 10%)

PENDING-T6

### 5.3 End to end with the rest of Rustique

PENDING-E2E

### 5.4 Adversarial programs (garbage that appears after the back-off)

PENDING-ADV

## 6. Conformance

CPython's own test modules, plain vs with the tier enabled before `test` is imported, every
test id's outcome compared (M, dev box, `benches/runtime/conformance.sh`: 3 parallel groups
per build, `CPYTHON_SRC` of the running patch release):

- **standard** (`tests/conformance/run_target_suites.py --targets gc`, `RUSTIQUE_TIERS=gc`):
  the modules that tune the GC (`test_gc`, `test_weakref`, `test_finalization` disable it in
  setUp, `test_generators` sets a threshold) make the policy step aside; the forwarder serves
  every callback test;
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

Rustique's own suites: `tests/test_gc_tier.py` (26), `tests/test_gc_tier_callbacks.py` (8,
one of them 50 Hypothesis programs by default, `RQ_GC_EXAMPLES`), `tests/test_gc_tier_threads.py`
(3) pass on the four primary builds; the whole `pytest -q tests` is green on all four
(PENDING-PYTEST).

## 7. Cost of the mechanism (callgrind)

PENDING-CG

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
the six hooked functions identical on all four primary builds). Other minors, pre-releases and
debug builds refuse (registry gate). Tests: `tests/test_gc_tier.py` (mechanics, a1–a3, d),
`tests/test_gc_tier_callbacks.py` (forwarder, differential + Hypothesis),
`tests/test_gc_tier_threads.py` (allocating threads, callback mutation, step-aside races,
enable/disable under load). Benchmarks and tools: `benches/runtime/` (`gcprobe.py`,
`gcpolicy.py` policies `default`/`loaded`/`rustique`/`tier[:k=v]`, `gctable.py`,
`policyrows.py`, `advtable.py`, `adv_tier.sh`, `tier_instr.py`, `gc_tier_cg.py`,
`conformance.sh`, `setup_tier.sh`, `run_quiet.sh`, `quiet_driver.sh`; the prototype
`gcadapt.py` and its tests stay as the reference).
