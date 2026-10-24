# Rustique Archaeology: Rediscovered Vision & Gold-Nugget Registry

This document records what the historical branches tried, what they proved, and what the
`next` rewrite keeps, fixes, or discards. Every claim was checked against the git history
(`git show origin/<branch>:<path>`) and, where possible, by building the historical code and
probing it. **[V]** = verified by reading or running; **[H]** = inference.

## 1. The branches

History is linear: `rapid-development → main → hashmap → engine → engine3`
(19 commits, 2024-12-05 → 2025-02-02).

| Era | Commits | Idea under test | State at tip |
|---|---|---|---|
| A. Fast list | 8b57fba | `List` of `PyObject` with an i32 API and a Rust `.sum()` | did not compile [V] |
| B. Rust builtins | 1f60d2d–7befdb5 | `rs.int` backed by num-bigint → GMP (`rug`); conversion speed | moved to `src/old/` |
| C. Rust primitives + typed vectors | 3c938a2 | `i8…u128`, `f32/f64`, `bool`, `char`; `Vector[T]` via `__class_getitem__` | only `i8` complete |
| D. Typed `List[T]` | 44bee42 (`main`) | `List(*values, _type=T)` with runtime `isinstance` checks | `main` HEAD does not compile (`Py<PyType> !=`) [V] |
| E. Builtin parity + FFI speed | 95681d6–34bd1fa (`hashmap`) | ABC-faithful `Dict`, FFI hashing, 1292-line `list` parity suite, lazy pipelines | compiles; 23/23 tests pass [V] |
| F. "Engine" | 212ffd5–4684c6b (`engine`) | `with Rustique():` monkey-patches builtins; Rust-owned object graph + GC | does not compile (21 errors) [V] |
| G. Memory engine | 7d91c6a–143861a | pooled allocator, context manager, `RustDict` | deleted next day; use-after-free [V] |
| H. Value types | 07ad41e (`engine3`) | `Int` (GMP), `Float`, `Str` (UTF-8, 45 methods) | builds with `m4`; only Int/Float/Str exported |

## 2. The rediscovered vision

Across all eight eras the author was pursuing **one** idea from four angles:

> *Python ergonomics with Rust's type discipline and Rust's speed* — strict, Rust-typed
> primitives (`i8` with explicit overflow semantics), generic containers that enforce their
> element type (`List[int]`), drop-in replacements for builtins that obey the
> `collections.abc` contracts, and eventually a Rust-owned value engine so data could stay
> on the Rust side.

The ambition grew while the working surface shrank: the last commit exports three classes,
and no branch has a container that beats a builtin.

### Why every measured variant was slower [V]

| Measurement (CPython 3.12, release builds) | Ratio vs builtin |
|---|---|
| hashmap `List`: construct / iterate / append / `==` / `sorted` | 4.9× / 14.2× / 6.2× / 17.7× / 7.4× slower |
| engine3 `List`: `for _ in x` / `sum` / `x[i]` | 5.5× / 3.3× / 1.9× slower |
| engine3 `Int` ctor / `a+b` | 5.0× / 4.4× slower |
| engine3 `Str` `len()` / `s[i]` (120k chars) | 133× / 300–416× slower (O(n) UTF-8 indexing) |
| early `List[int](*r)` construction | ~17× slower |
| GMP `big*big` (317k bits) / `str(big)` | **0.09× / 0.10× (11×/10× faster)** |
| Rust ASCII `upper()` | **0.14× (7× faster)** |

**Root cause:** the containers stored `PyObject*` — the same layout as CPython's list — and
added PyO3 dispatch, borrow checks and per-element callbacks on top. They paid the
boundary cost on every element and never got a data-layout advantage back. The only wins
came when one call did a lot of work in Rust and the result stayed there.

The headline benchmark claims in the notebooks are invalid [V]: "0.000014 s" timed an
already-emptied list (`collect()` used `mem::take`), "9052× faster `list()`" timed the
creation of a lazy iterator, and `Engine.__exit__` returned `True`, swallowing the
assertion errors that would have exposed both.

## 3. Gold-nugget registry (ranked)

| # | Nugget | Evidence | Decision in `next` |
|---|---|---|---|
| 1 | **Monomorphised native storage per element type** — `Vector[i8] → _Vector_i8` type map, never built | `rapid-development:rustique/vector.py:5-39` | **Core of the rewrite**: `Column` enum of typed `Vec`s; `List[rs.i8]` stores `Vec<i8>` |
| 2 | **Rust overflow families exposed to Python** (`checked_/wrapping_/overflowing_/saturating_*`, bit ops, byte conversions, MIN/MAX) | `main:src/primitives/i8.rs:68-90` | **Kept and completed** for all 10 fixed-width types via one macro; operators keep their type (the original returned plain `int`) and use Python's floor semantics |
| 3 | **Generic syntax via `__class_getitem__`** (`List[int]`) | `main:rustique/collections/list.py:6-16` | **Reinvented**: cached, identity-stable alias objects; `isinstance(x, List[int])` works; nested typing (`List[List[int]]`, `int | None`) — the README's open TODO — supported |
| 4 | **Dual representation with lazy conversion** (`enum List6Data { Elements, Buffer(Py<PyList>) }`) | `engine:src/list.rs:11-18` | **Generalised** into adaptive *storage strategies* (PyPy-style): native column while homogeneous, transparent demotion to objects |
| 5 | **Bulk ingest of a list without per-item conversion** (`ob_item` slice) | `hashmap:src/list.rs:42-60` | **Kept** as a fast path, restricted to exact `list`/`tuple` (the old generic branch cast tuple subclasses to `PyListObject`: UB) |
| 6 | **Live iterator** (`Py<List>` + index, sees mutations, O(1) to create) | `218c77c:src/list.rs:590-612` | **Kept** (later branches regressed to O(n) snapshot clones) |
| 7 | **Lazy map/filter pipelines in Rust** | `engine:src/list.rs:1010-1056` | **Reinvented**: callbacks were Python lambdas, so Rust saved nothing. `next` evaluates vectorised `X` expressions (`lst.map(X * 2 + 1)`) entirely in Rust |
| 8 | **Exact PyLong → GMP via bytes** (`_PyLong_AsByteArray`) and an i64 fast path | `main:src/old/old.rs:72-116` | **Lesson kept, code not**: it mis-sized the sign bit (`Int(128)` overflowed, `Int(-1)` → 255). Big ints use exact Python objects (adaptive demotion); a native big-int backend is on the roadmap via PEP 757 `PyLong_Export` |
| 9 | **FFI hash/eq with identity shortcut** (`PyObject_RichCompareBool`) | `faster.rs:131-153` | **Kept**, with error propagation (the old code `eprintln!`-ed hash failures and accepted unhashable keys) |
| 10 | **The 1292-line `list` parity suite** | `origin/engine3:rustique/tests/test_list.py` | **Kept as the spec**, ported and extended (sort, `+`, ordering, repr, NaN, iterator mutation) and complemented by Hypothesis differential tests (hypothesis was a dependency, never used) |
| 11 | **ABC hierarchy as Rust traits** (`PySized … PyMutableMapping`) | `hashmap:src/collections/collections.rs:11-137` | Dropped as an abstraction; protocol parity is enforced by tests instead |
| 12 | **Protocol-based input dispatch** ("only check PyMapping, PySequence, PyIterator") | README at 218c77c | **Adopted**: one ingest path with exact-type fast paths, not a type matrix |
| 13 | Release profile ideas (`lto = "fat"`, `codegen-units = 1`) | README | **Applied** (they never reached `Cargo.toml`; `[build] rustflags` in `Cargo.toml` was silently ignored) |

## 4. Dead ends (and why)

- **Hijacking builtins** (`with Rustique():` replacing `list/map/filter`): slower
  (188 ms vs 93 ms), wrong argument order in `filter`, globally mutates `builtins`, broke
  Jupyter, swallowed exceptions. *Discarded.*
- **Rust-owned object graph with its own GC** (`RustiqueValue` + `Arc` + mark/sweep): every
  object was a root, hashing by value collapsed distinct objects, no conversion protocol,
  never compiled against PyO3 0.23's `Sync` rule. *Discarded* — Python's GC already owns
  Python objects; Rustique owns *unboxed data*, not objects.
- **Pooled byte allocator + context manager**: not an arena, recycled unzeroed blocks →
  use-after-free in `RustDict` (dangling entries visible from a fresh dict) [V].
  *Discarded.*
- **UTF-8 `String` as a drop-in `str`**: O(n) `len`/indexing, no lone surrogates, hash
  inconsistent with `str`. *`next` stores strings natively only inside containers, where
  sorting/searching benefit, and returns real `str` objects.*
- **Hashes that differ from Python's** (`DefaultHasher` for Str/Float, `to_u64_wrapping` for
  Int): broke dict interop. *`next` scalars reproduce CPython's numeric hash exactly.*
- **`#[pyclass(unsendable)]` and `&mut self` containers**: unusable across threads; on
  free-threaded 3.15t `&mut self` raises `RuntimeError: Already borrowed` under contention
  [V]. *`next` containers are `frozen` with interior locks.*

## 5. Bugs catalogued in the historical code [V]
`Int(128)` OverflowError, `Int(-1) == 255`, truncating `//` and `%`, `Int(5) + '3' == 8`,
`round(Float(2.5)) == 3`, `Float` unhashable, `Str('a') in {'a'} is False`, `List.collect()`
empties the list, slicing `L[3:1]` panics, `v[-1]` panics (rapid-development `Vector`),
`List([nan]) == [nan]` is False, `swap_remove` broke dict order, dict `pop` raised
ValueError, `filter(None, xs)` TypeError under the engine. Each has a regression test in
`next`.
