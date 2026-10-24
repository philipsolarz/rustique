# Rustique Roadmap

Status of the `next` rewrite and where it goes from here. See `ARCHITECTURE.md` for the
contract and `ARCHAEOLOGY.md` for the history that motivated it.

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
| Hardening | two independent safety reviews (≈30 findings incl. a use-after-free, deadlocks, a stack overflow, livelocks) — all fixed with regression tests (`tests/test_hardening*.py`) |
| Validation | 107 Rust unit/property tests incl. CPython-recorded oracles; ~3400 pytest cases on 3.14, 3.14t, 3.15.1, 3.15.1t incl. the historical parity suite, Hypothesis state machines vs builtins, thread stress, crash/deadlock regressions in subprocesses; mypy/pyright strict stubs; honest benchmark harness |

## Known trade-offs

- **Per-element access from Python costs more than a builtin list.** Every `x[i]`, `next()`
  or `append` pays PyO3 dispatch, a lock and (for native storage) creation of a Python
  object. Rustique wins when work stays in Rust between operations (bulk kernels,
  expressions, buffer export); it is not a drop-in speedup for element-at-a-time loops.
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
- **`match lst: case [a, b]`** does not work: PyO3 types are immutable, so the sequence
  flag cannot be set by ABC registration.

## Next steps

1. **Per-element hot paths** — lower-overhead slot implementations for `__getitem__` /
   `__next__` / `append`, bulk object creation for `to_list()`/`list(x)`.
2. **Big integers natively** — a big-int column backed by PEP 757 `PyLong_Export` /
   `PyLongWriter` (zero-copy 30-bit digit access, measured 9–160 ns per conversion) so
   `List[int]` never needs to demote; optional GMP/`malachite` arithmetic for `X` on big data.
3. **Struct-of-arrays records** — `List[Point]` for dataclasses/`TypedDict`s stored as one
   column per field (schema already compiles nested types; needs a record column and
   annotationlib-based field discovery that handles 3.14+ deferred annotations).
4. **Optional values without demotion** — validity bitmaps for `List[int | None]`.
5. **Expression engine** — string kernels (`X.len()`, `X.startswith()`, `X.upper()`),
   reductions in expressions (`(X * 2).sum()` without materialising), `where`/`clip`,
   fused `map`+`filter` pipelines.
6. **abi3t wheels** — one wheel for GIL and free-threaded 3.15 (PEP 803) once the ~3×
   slower list ingest through the limited API is addressed; version-specific wheels until then.
7. **Subinterpreters** — per-interpreter module state (PEP 734 / PEP 793) so Rustique loads
   in `concurrent.interpreters`.
8. **Rust interop** — extend the C-API capsule (owned-buffer hand-off without memcpy via a
   drop callback, `Dict`/`Set` construction and borrowing, `List[str]` borrowing without
   copies) and publish `rustique-api` to crates.io for downstream PyO3 crates.
9. **Allocator** — evaluate mimalloc for Rust allocations to remove the first-touch page
   fault cost of large fresh buffers on free-threaded builds.
