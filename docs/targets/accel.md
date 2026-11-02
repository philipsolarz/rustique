# The acceleration framework, wave 2 (`crates/rustique-py/src/accel/`)

What wave 2 changed in the framework every target stands on, and what was measured. The rules
themselves are in `docs/ACCELERATION.md` (amended for wave 2: §1, §3.1–3.2, §3.10, §3.12,
§3.15–3.18, §4.6, §5.2, §5.5–5.6, §5.8, §6, §7.3–7.4, §8, §9); the rulings in
`docs/DECISIONS.md` §10. Measurements: dev box (c7a.8xlarge, shared by nine workstreams: wall
time is noisy there, so claims use callgrind instruction counts or pass/fail), dev-release
builds unless stated. **M** = measured (how), **I** = inferred (from what).

## 1. Free-threading hardening (DECISIONS §10.9; w2-threads §3.4)

| item | change | evidence |
|---|---|---|
| S1: `PanicException` out of `enable()` | every activation (prepare, site swap, pins, shared groups) runs under `catch_unwind` with a rollback guard; a panic becomes a refusal (`other`, the panic as reason); pinned dicts are snapshotted with `PyDict_Next` under the dict's critical section | **M** `tests/test_accel_threads.py::test_enable_survives_concurrent_writes_to_snapshot_pinned_dicts`, 3.15.1t, 20 s: before 6 `PanicException`s, after 0, and no target refused by the race |
| S2: lock-order inversions | the pin-table lock is a leaf (nothing that locks, parks, decrefs or calls CPython under it); pins read and registered under the pinned dict's critical section (the watchers' order); `PyType_Watch` before any section; violations counted (`_accel._lock_violations()`) | **M** `test_pin_tables_lock_is_a_leaf` (every pin kind activated, mutated, released, exit hook: 0 violations); `test_class_attribute_pins_against_class_writers` (class-attribute pins vs two class writers): before, a hang until the faulthandler deadline; after, 20 s clean |
| S3: `Tools` on weak memory | one atomic word `(version << 1) | busy`, recomputed from a private state array | code |
| suspension on the main thread only | the thread whose fallback reaches the score's floor suspends the target itself | **M** `test_worker_fallbacks_suspend_while_main_thread_waits`: before, the target stayed on the kernel while the main thread waited in `join()` |
| stop-the-world on dependency resume | free-threaded builds swap nothing for a dependency: its fallbacks debit the score; at most one suspension, resumed by `enable()` | **M** `test_changed_dependency_swaps_nothing_until_net_benefit` |
| lazy activation inside the `sys.modules` watcher (t builds) | the watcher only queues; a pending call any thread runs (`_PyEval_AddPendingCall`) or the next registry operation activates. Reason: blocking inside a dict watcher suspends the critical section of the dict being mutated (`_PyThreadState_Detach` → `_PyCriticalSection_SuspendAll`, CPython 3.15 `Python/pystate.c`), and `insertdict` notifies watchers between its lookup and its store | **I** CPython source; **M** `test_lazy_activation_completes_without_a_registry_call` |
| watcher ids | registered race-free (the loser clears its own id) | code |

## 2. Site support

- **Opt-in tiers** (`TargetSpec::opt_in`, `RUSTIQUE_TIERS`, `enable(tiers=)`): `tests/test_accel_tiers.py` (environment union, `none`, exclusion by tier name, kill switch, warnings).
- **Opcode-handler sites** (kind D, `Prepare::opcode_handler`) and suspension of any site kind (`Counters::request_suspend()`), used by the `interp` tier.
- **Slots of heap types defined in Python** (`Prepare::heap_slot`): `tests/test_accel_sites.py` (install and restore, subclasses created while installed, a slot CPython rewrites shows `suspended: "conflict"`, the reference-class cross-check).
- **Dynamic vectorcall sites** (`accel::dynamic`), lazily retired on `Counters::live()`, and `TargetSpec.on_install` / `on_disable` (also on suspension). A weakref registry was measured by datamodel at 1.066 on `contextmanager(gen)` (12.9k Ir plain); the lazy retire costs one relaxed load per call (**M**, datamodel).
- **`ClassCache`**: 2-way sets, multiplicative hash of the type address (heap types a few KB apart collided with `(addr >> 4) % N`: an enum cell recomputed every call, 1.10, **M** datamodel), version-tag keyed, with weak dict invalidation (`guard::watch_dict_invalidates`).
- **Monitoring tax removed**: wave 1 calibrated the local-monitoring layout with a free `sys.monitoring` tool id; `free_tool_id` bumps the interpreter's monitoring version for good, so every code object created afterwards paid `_Py_Instrument` at its first `RESUME` (~930 Ir each, found by datamodel). Now a structural check. **M** `benches/accel/monitoring_tax.py` (callgrind, 3.15.1, plain vs enabled after the first vectorcall install): compile+exec+first call of 50 fresh functions 1.011 → 1.004; import of a 50-function module 1.026 → 0.921.

## 3. Validate on first call (w2-startup §1.7, w2-floors §4; target T9)

Targets are *armed* at install (sites resolved, first-call stubs in place) and validated at
their first call (GIL builds; free-threaded builds validate eagerly). `tests/test_accel_firstcall.py`.

Install costs, callgrind regions in fresh processes, GC off in the region, 3.15.1 dev-release
(`benches/accel/install_costs.py`; **M**):

| cell | plain import | eager (wave 1 behaviour) | first call | first call vs plain (budget) |
|---|---:|---:|---:|---|
| `import json` lazy | 22.23M | +265k | +155k | 1.007 (+1.11M) |
| `import copy` lazy | 4.05M | +175k | +90k | 1.022 (+0.20M) |
| `import logging` lazy | 52.57M | +1.10M | +0.69M | 1.013 (+2.63M) |
| `import random` lazy | 3.46M | +137k | +61k | 1.018 (+0.17M) |
| `import pathlib` lazy | 9.23M | +405k | +234k | 1.025 (+0.46M) |
| `import datetime` lazy | 0.43M | +92k | +89k | 1.207 (+0.10M) |
| enable `logging` (imported) | | 554k | 324k | (0.1M: over) |
| enable `copy` (imported) | | 230k | 117k | (over) |
| enable all, fresh process (`RUSTIQUE=1`'s work) | | 521k | 355k | |

A lazy install plus the first call of one target (`import M` lazy+call) stays within budget for
every module but random (+253k with the call, the group's shared validation). The remaining
arming cost is the targets' own `prepare` (interning of names, lookups): logging and pathlib
dominate. `status()`: first call 881k → 413k Ir, later calls ~270k (84 targets; templates,
shared strings, cached site labels).

## 4. Fingerprint v3 and recorded patch releases (DECISIONS §10.5; target T10)

v3 drops docstrings (`co_consts[0]` with `CO_HAS_DOCSTRING`, `ml_doc`) and canonicalizes the
exception table to the handler of each instruction that can raise synchronously (tables
generated from CPython's opcode metadata, `accel/opcodes.rs`). Accepted hashes live in
`accel/fingerprints.txt` per (target or group, original, minor), with the recorded patch
releases. An unrecorded release refuses (`unvalidated-patch`); `RUSTIQUE_UNVALIDATED_PATCH=1`
installs targets whose fingerprints match. `tests/test_accel_gate.py` (docstring
independence, a split exception-table entry hashes equal, a moved handler differs, the gate's
decisions).

Recorded (conformance first, `benches/accel/record_patch.py`): see §6.

## 5. Platforms (DECISIONS §10.8)

The process gate refuses every target with category `platform` outside 64-bit little-endian
x86-64/AArch64 on Linux glibc/musl, macOS and Windows. Windows resolves exported interpreter
symbols with `GetProcAddress(sys.dllhandle)` (w2-dx patch A). The test-suite skips every test
not marked `refused_ok` when the interpreter is refused for `version`, `platform` or
`unvalidated-patch`. **M** 3.16.0a1: refused (`version`, pre-release), suite skipped.

## 6. Credits v2 and per-process counters (w2-floors §3, w2-dx §3.1)

- Reasons `continue` and `abandon` split the deopts; `abandon_debit` weights an abandoned
  restart; `suspensions` in `stats()`.
- A decision window of 256 scored calls for targets that have hit; a target with no hit at
  all suspends at its floor (64 fallbacks with the default debit), so all-fallback streams
  keep the wave-1 exceedance length (lead's ruling).
- A forked child starts from zero and names its parent (`process["ppid"]`); with counters on
  and `RUSTIQUE_REPORT=<P>.json`, the live counter file `<P>.<pid>.cnt` survives `os._exit`
  (layout in `accel/counters_file.rs`). `tests/test_accel_counters.py`.

## 7. The 8-thread rule

`benches/accel/thread_rule.py` (on `benches/ft/kernel_scaling.py`): CPU time per call at 8
threads, plain vs enabled, per-thread and shared inputs; exit status non-zero above 1.05.
