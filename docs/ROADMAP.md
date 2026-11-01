# Rustique Roadmap

Status of the `next` rewrite and where it goes from here. See `ARCHITECTURE.md` for the
contract and `ARCHAEOLOGY.md` for the history that motivated it.

## Done: wave 1, faster real programs (November 2026)

See `DECISIONS.md` §5–§9, `IMPACT_ATLAS.md`, `ACCELERATION.md` (the contract) and
`docs/atlas/phase3.md` (the measurements).

| Area | State |
|---|---|
| Framework (`accel`) | one contract for every target: `ml_meth`/slot/vectorcall sites installed all-or-nothing, version gate and code fingerprints per CPython minor (unknown interpreter → refuse), lazy install when a module finishes importing, dependency pins through one dict watcher and one type watcher, tools guard, net-benefit suspension, counters and status, check mode, JIT cache clearing, subinterpreter refusal, release at exit |
| Targets | 84 targets in 15 groups: json (+ exact float→text), copy + dataclasses, datetime text, posixpath, fnmatch, pathlib, logging, random, textwrap, urllib.parse, statistics, plus the parity program's containers and builtins |
| Results | 34 real workloads: geometric mean 0.880 (3.15.1) / 0.843 (3.15.1t), stdlib-heavy programs 1.2–2.3× faster, none slower than 1.013; CPython's own test modules of every group 0 differences with all groups on |
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

- **Per-element access to Rustique containers costs more than to a builtin.** CPython only
  fast-paths exact builtins: even an empty `list` subclass pays 2.4× for `x[i]`.
  `List` now runs at 1.0–1.16× of that floor (raw slots, no locks on GIL builds, an
  element cache for native storage), i.e. ~2× the builtin for `x[i]` on tiny lists. The
  drop-in speedup for existing code is `rustique.enable()`, which keeps the builtins.
- **Native float storage has no object identity**, so NaN compares equal to NaN in
  searches and comparisons (documented deviation).
- **Typed containers normalise values** (`List[int]` stores `True` as `1`; `List[float]`
  stores `1` as `1.0`), like `array.array` and NumPy. Untyped containers never do.
- **NaNs sort last**; CPython's order with NaNs is ill-defined.
- **Lock fairness over raw throughput**: on 3.15t a lone writer among many readers gets
  ~11k ops/s (it is never starved); read-heavy scaling is ~2.5× better than before the
  fairness work.
- **`remove()`/`extend()` re-entrancy**: `remove()` deletes the element that compared equal
  (CPython deletes whatever sits at that index afterwards) and `extend()` consumes a generic
  iterable before appending; both pinned by tests.
- **Big-endian buffers** are ingested by iteration (like `list(arr)`), so typed lists reject
  NumPy scalar elements from byte-swapped arrays.
- **Buffer exports freeze the container**: safer than `bytearray`'s resize-only rule,
  stricter for writers.
- **Rustique containers are not builtin subclasses** (by design, `TAKEOVER.md` R2): C code
  that requires a real `list`/`dict` (`json.dumps`, `type()` namespaces, PyO3 `cast`)
  rejects them. `match` works: `List`/`Tuple` set the sequence flag, `Dict` the mapping flag.

## Next steps

**Wave 2** (ranked, with evidence: `DECISIONS.md` §9): startup and install cost (engine
split, precompiled loader, deferred fingerprints); json `default=`/`cls=`; class-needle
membership and `key=` fast paths; HTTP header/e-mail parsing; `re`; deeper coverage of the
wave-1 targets (copy restarts, pathlib subclasses); bulk text kernels, `xml.etree`,
`tokenize`, `csv`, `ipaddress`, `uuid`.

Carried over from the parity program, ranked by expected gain and risk (details and
evidence: `DECISIONS.md` §4; the atlas demoted the bulk `str`/`bytes` kernels, since real
inputs are small, and wave 1 shipped the float→text core):

1. **`str` method kernels** behind the existing C entry points (`find`, `count`, `split`,
   `replace`, `join`, ASCII case mapping): SIMD on PEP 393 buffers, exact semantics,
   CPython's code otherwise.
2. **`bytes`/`bytearray` method kernels** (`find`/`count`/`split`/`replace`/`hex`/
   `fromhex`/`translate`).
3. **Large-`int` arithmetic** behind `nb_multiply`/`nb_floor_divide`/`nb_power` and
   `int`↔`str`, above a size threshold (subquadratic algorithms; PEP 757 digit access).
4. **`float` → text** (`repr`/`str`/`format`) with shortest-round-trip printing.
5. **Remaining container kernels**: `list ==`/`<`, `list.remove`, `key=` fast paths for
   `min`/`max`/`sorted`, a faster table walk for large builtin sets.
6. **Rustique containers**: single-allocation hash tables for `Dict`/`Set` (small sizes),
   cheaper iterator `next`, native `str` ingest, lock-free reads on free-threaded builds.
   Carried over from v0.2: big ints natively (PEP 757 export/import), struct-of-arrays
   records, validity bitmaps for `T | None`, expression-engine string kernels and fused
   pipelines, abi3t wheels, subinterpreter support, C-API capsule extensions, mimalloc for
   large fresh buffers.
7. **Upstream**: a CPython specialization hook for third-party containers is the
   prerequisite for native storage ever being the default.
