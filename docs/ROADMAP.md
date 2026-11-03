# Rustique Roadmap

Status of the `next` rewrite and where it goes from here. See `ARCHITECTURE.md` for the
contract and `ARCHAEOLOGY.md` for the history that motivated it.

## Done: wave 2, all kinds of programs (November 2026)

See `DECISIONS.md` §10–§17, `IMPACT_ATLAS.md` Part 2 (§7–§15), `ACCELERATION.md` and
`docs/atlas/w2-phase3.md` (the measurements).

| Area | State |
|---|---|
| Framework (`accel`) | heap-type-slot sites, opcode-handler sites (the `interp` tier), validate-on-first-call, fingerprint v3 with recorded patch releases (`unvalidated-patch` refusal), platform allowlist, cost-weighted credits v2, fork-safe live counters, opt-in tiers (`RUSTIQUE_TIERS`, `enable(tiers=)`), free-threading hardening |
| Targets | 111 targets in 21 default groups (new: `re` compiler and module functions, `enum`, generated namedtuple/dataclass code, `contextlib`/`functools` bookkeeping, `set.__reduce__` on free-threaded builds; json `default=`, logging handlers, `relpath`, `sorted`) plus two opt-in tiers: `interp` (generated opcode handlers on verified python-build-standalone builds) and `gc` (defers full collections that find nothing) |
| Results | 65 real programs in 8 families: 0.870 (3.15.1) / 0.872 (3.15.1t), framework apps 0.940, custom applications 0.936, services 0.848, threaded programs 0.685 / 0.792; wave 1's 34 0.858 / 0.826; pyperformance 0.911 / 0.936; 0 differences in CPython's test modules (50 modules, 4 builds) and 14 real projects' suites |
| Removed | the opt-in native types (`List`, `Dict`, …, `X`, scalars, the C-API capsule): `docs/MIGRATION.md`, tag `legacy-native-types` |
| DX | `python -m rustique estimate`, `report` with per-target reasons, `pytest --rustique-tiers`, per-platform wheel CI with a target-coverage report, release-ready wheels (not published) |
| Open | `sorted()` of large sets of numbers, the `gc` tier's pause placement, startup wall time 1.06–1.08, extension 3.6 MB, pyperformance `coroutines` on 3.15.1t (DECISIONS §13) |

## Done: wave 1, faster real programs (November 2026)

See `DECISIONS.md` §5–§9, `IMPACT_ATLAS.md`, `ACCELERATION.md` (the contract) and
`docs/atlas/phase3.md` (the measurements).

| Area | State |
|---|---|
| Framework (`accel`) | one contract for every target: `ml_meth`/slot/vectorcall sites installed all-or-nothing, version gate and code fingerprints per CPython minor (unknown interpreter → refuse), lazy install when a module finishes importing, dependency pins through one dict watcher and one type watcher, tools guard, net-benefit suspension, counters and status, check mode, JIT cache clearing, subinterpreter refusal, release at exit |
| Targets | 84 targets in 15 groups: json (+ exact float→text), copy + dataclasses, datetime text, posixpath, fnmatch, pathlib, logging, random, textwrap, urllib.parse, statistics, plus the parity program's containers and builtins |
| Results | 34 real workloads: geometric mean 0.880 (3.15.1) / 0.843 (3.15.1t), stdlib-heavy programs 1.2–2.3× faster, none slower than 1.013 (two replicates agree within 2% per workload); pyperformance 0.942 / 0.957; CPython's own test modules of every group 0 differences with all groups on |
| DX | `RUSTIQUE=1` (no code change), `RUSTIQUE=0` kill switch, `RUSTIQUE_ONLY`/`EXCLUDE`, `RUSTIQUE_STATS`/`REPORT`/`CHECK`, `python -m rustique status|report|diff`, `pytest --rustique`, wheels for 8 platforms × 3.14/3.14t/3.15/3.15t, `docs/USAGE.md` |
| Known exceedances | startup with `RUSTIQUE=1` 1.045–1.063× instructions; a few install costs over budget; first ~64 calls of all-fallback streams (DECISIONS §5) |

## Done: parity program (October 2026)

See `DECISIONS.md` for the decisions and `BENCHMARKS.md` for the numbers.

| Area | State |
|---|---|
| Takeover | `rustique.enable()`: exact builtins accelerated in place, process-wide, by swapping CPython's C entry points (`list.sort`, `list`/`tuple` `index`/`count`/`in`, builtins `sum`/`min`/`max`/`sorted`/`any`/`all`); every candidate mechanism measured (subclassing, rewriting, PEP 523, boundary materialization) in `TAKEOVER.md`. Opt-in native tier (`native=`, runner, decorator) |
| Parity, default tier | pyperformance 1.00× / 1.01× (3.15.1 / 3.15.1t); scoreboard: every result identical to CPython's, kernels 0.1–0.5× on 10k/1M inputs |
| `List` / `Tuple` | raw C slots, borrow-flag locking on GIL builds, element cache, CPython-layout object sort via `list.sort`; new immutable lock-free `Tuple` with CPython's tuple hash |
| `Dict` | CPython's compact dict index (index width by size, perturb probing, growth policy), str keys as objects with cached hashes, raw slots, bulk builds |
| `Set` / `FrozenSet` | smaller-operand algebra, early exits, set-of-sets semantics, weakrefs, subclassing; new immutable `FrozenSet` with CPython's frozenset hash |
| Measurement | parity scoreboard (5 types × 5 storages × 6 sizes, callgrind + pyperf), CPython `Lib/test` conformance gate, pyperformance + application harness |
| Validation | ~3,880 pytest cases on 3.14, 3.14t, 3.15.1, 3.15.1t; CPython conformance list 67/69, tuple 37/38, dict 35/36, set 52/54, frozenset 31/33 |

## Done (v0.2 "next")

| Area | State |
|---|---|
| Toolchain | Rust 1.99.1 / edition 2024 / PyO3 0.30.1 / maturin 1.15 / CPython 3.14–3.15.1, incl. free-threaded 3.14t and 3.15.1t |
| `rustique-core` | `Column` typed storage, Python slice semantics, CPython-exact kernels (sum bit-exact incl. Neumaier, min/max, stable sort, search, lexicographic compare), vectorised expression engine (block-wise, multi-threaded), compact hash index, CPython-compatible numeric hashing and float repr |
| `List` | full `list` parity, adaptive storage strategies, typed schemas, live iterators, buffer protocol with freeze-on-export, pickling, GC support, `map`/`filter` with `X` expressions, `sum`/`min`/`max`, native `sort` |
| `Dict` / `Set` | full `dict`/`set` parity, native int/str key strategies with Python numeric key equivalence, object keys with per-candidate re-validation (no livelock), CPython iteration and growth-compaction rules, views, set algebra |
| Typing | schemas compiled from `int`, `float`, `bool`, `str`, unions/optionals, nested containers, `Annotated`, PEP 695 aliases, `NewType`, classes; cached identity-stable aliases (`List[int] is List[int]`, ~70 ns) |
| Scalars | `i8 … u64`, `f32`, `f64` with type-preserving checked operators and Rust-named overflow families |
| Concurrency | frozen pyclasses with interior locks, detached waiting that never holds or queues on a lock (GC-consistent), writer-starvation-free, no Python code under locks (including rejected inputs on error paths), critical sections for bulk reads of builtin sources; tested on 3.14t and 3.15t with the GIL disabled |
| Rust interop | versioned C-API capsule (`rustique._rustique._C_API`) + `rustique-api` crate: other PyO3 extensions build real `rustique.List`s from slices with one memcpy and borrow list data zero-copy; demo extension with tests |
| Hardening | two independent safety reviews (26 findings incl. a use-after-free, deadlocks, a stack overflow, livelocks) — all fixed with regression tests (`tests/test_hardening*.py`) |
| Validation | 107 Rust unit/property tests incl. CPython-recorded oracles; ~3400 pytest cases on 3.14, 3.14t, 3.15.1, 3.15.1t incl. the historical parity suite, Hypothesis state machines vs builtins, thread stress, crash/deadlock regressions in subprocesses; mypy/pyright strict stubs; honest benchmark harness |

## Known trade-offs

The trade-offs of Rustique's own container types (per-element access cost, typed
normalisation, lock fairness, buffer exports, …) left with those types in 0.3
(`docs/MIGRATION.md`). What remains are the accepted overages of the acceleration layer,
each measured and bounded: DECISIONS §10 rulings 18–19 and §13.

## Next steps

**Wave 3** (ranked, with evidence: `DECISIONS.md` §17):

1. Close the open never-slower items: large-set sort, the `gc` tier's pause placement, startup
   wall time (smaller engine, lazily loaded handler sets), first-call costs over budget.
2. The `interp` tier as the default on verified builds (its whole-run gate, remaining C→Python
   frame pushes, a `STORE_ATTR` cache, aarch64 and macOS builds).
3. Third-party-heavy code: generic attribute lookups, allocation and dict work through the
   `interp` tier and the runtime layer; `getattr`/`hasattr` with the attribute cache.
4. email/http header parsing, the `xml.etree` serializer, the json `make_encoder` route,
   `inspect.signature`, weakref containers.
5. Free-threading: generated-code targets without stop-the-world installs, the
   `isinstance(x, ABC)` cache, threaded programs in the estimator.
6. Platform reach: Windows and macOS/musl target counts proven per release; recording each
   new CPython patch release as it ships.
7. Upstream reports (DECISIONS §17.7).
