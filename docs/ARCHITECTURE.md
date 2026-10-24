# Rustique — Architecture & API Contract

> **Python's data model, Rust's memory.**
> Rustique gives Python list/dict/set ergonomics on top of typed, native Rust storage,
> and lets Rust hand those values to Python (and back) without converting them element by element.

This document is the single source of truth for the `next` rewrite. Implementation, tests,
stubs and docs must agree with it. If code and this document disagree, fix one of them
explicitly — never silently.

---

## 1. North Star

The historical branches (see `docs/ARCHAEOLOGY.md`) chased one idea from four angles —
Rust-typed primitives, typed `List[T]`, a "Rust engine" that replaced builtins, and a
Rust-owned object model. Every measured variant was *slower* than CPython's builtins
(1.4×–17×), for one root cause: **they stored `PyObject*` in Rust and paid PyO3 dispatch on
every element without ever getting a layout advantage back.** The only real wins (GMP
bigint 4–27×, ASCII `upper()` 7×) came when a single call did lots of work in Rust and the
result stayed there.

The next Rustique is built around that lesson:

1. **Native storage is the point.** Homogeneous data lives unboxed in Rust (`Vec<i64>`,
   `Vec<f64>`, `Vec<bool>`, `Vec<CompactString>`, …). `PyObject` storage exists only as the
   general fallback.
2. **Adaptive storage strategies.** An *untyped* `List` picks the tightest native strategy
   for its contents and transparently demotes to object storage when a value doesn't fit
   (PyPy-style list strategies). A *typed* `List[int]` validates once at the boundary and
   pins its strategy.
3. **Pay per operation, not per element.** Anything touching *n* elements runs as one Rust
   call: kernels (`sum`, `min`, `sort`, `index`, `count`, …) and vectorised expressions
   (`lst.map(X * 2 + 1)`, `lst.filter(X % 3 == 0)`). Python callables are supported but are
   the documented slow path.
4. **Python semantics are the spec.** Builtin `list`/`dict`/`set` behaviour (errors,
   slicing, negative indices, `NotImplemented`, live iterators, floor division) is
   verified by differential tests against the builtins.
5. **Thread-safe by construction.** Built for free-threaded CPython 3.15t. Invariant:
   **no Python code ever runs while a Rustique lock is held.**
6. **Zero-copy at the edges.** Numeric/bool storage exports the buffer protocol
   (`memoryview`, NumPy); Rust code moves a `Vec<T>` into a Python `List` in O(1).
7. **Pure-Rust core.** Everything that isn't binding glue lives in `rustique-core`
   (no PyO3), is tested with `cargo test`, and is usable from Rust directly.
8. **Honest performance.** Benchmarks build fresh inputs per run and verify results.

### Toolchain (latest stable, pinned)
- Rust **1.99.1** (`rust-toolchain.toml`), edition **2024**, resolver 3.
- PyO3 **0.30.1**, maturin (latest 1.x).
- CPython **3.15.1** and free-threaded **3.15.1t** are the primary test targets;
  `requires-python = ">=3.14"`: free-threading is officially supported from 3.14
  (PEP 779), PEP 757 integer export/import exists, and error messages match the running
  interpreter (3.14 unified the `ZeroDivisionError` wording).

---

## 2. Repository layout

```
Cargo.toml                    # workspace (resolver 3), shared lints, release profile
rust-toolchain.toml           # channel = "1.99.1"
pyproject.toml                # maturin backend; python-source = "python"
crates/
  rustique-core/              # pure Rust, no PyO3
    src/lib.rs
    src/dtype.rs              # DType: the native element types
    src/column.rs             # Column: typed storage enum + structural ops
    src/slice.rs              # Python slice normalisation & slice-assignment plans
    src/kernels.rs            # sum/min/max/sort/search/count/reverse on columns
    src/expr.rs               # vectorised expression engine (map/filter)
    src/index.rs              # CPython-style compact hash index (Dict/Set)
    src/pyhash.rs             # CPython-compatible hashing of ints/floats
    src/pyfmt.rs              # CPython-compatible float repr
  rustique-api/               # downstream Rust client of the C-API capsule (§5.1); pyo3 only
  rustique-py/                # PyO3 binding → module `rustique._rustique`
    src/lib.rs                # #[pymodule] registration only
    src/storage.rs            # Storage = Native(Column) | Object(Vec<Py<PyAny>>), strategies
    src/schema.rs             # Python type expression → Schema; alias cache; ListType/DictType/SetType
    src/convert.rs            # Python ↔ native element conversion, bulk ingest fast paths
    src/sync.rs               # locking discipline (see §6)
    src/list.rs               # List pyclass + ListIterator
    src/dict.rs               # Dict pyclass + views/iterators
    src/set.rs                # Set pyclass + iterator
    src/scalars.rs            # i8..u64, f32, f64 scalar pyclasses
    src/expr.rs               # Python-facing expression builder `X`
    src/capi.rs               # versioned C-API capsule `rustique._rustique._C_API` (§5.1)
python/rustique/
  __init__.py                 # public re-exports
  __init__.pyi                # hand-written, precise type stubs
  py.typed
tests/                        # pytest (parity, typing, strategies, expr, threads, buffer)
benches/                      # honest benchmark harness
examples/                     # runnable examples; rust_extension/ is a downstream crate
docs/                         # ARCHITECTURE.md (this), ARCHAEOLOGY.md, ROADMAP.md
```

---

## 3. Core concepts

### 3.1 DType (rustique-core)
`Bool, I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, Str`.
Each numeric/bool dtype has a buffer-protocol format char
(`?`, `b`, `h`, `i`, `q`, `B`, `H`, `I`, `Q`, `f`, `d`) and item size.

### 3.2 Column (rustique-core)
```rust
pub enum Column {
    Bool(Vec<bool>), I8(Vec<i8>), I16(Vec<i16>), I32(Vec<i32>), I64(Vec<i64>),
    U8(Vec<u8>), U16(Vec<u16>), U32(Vec<u32>), U64(Vec<u64>),
    F32(Vec<f32>), F64(Vec<f64>), Str(Vec<CompactString>),
}
```
plus the scalar `Value` enum (one variant per dtype, `Str(CompactString)`), and
structural operations: `len`, `get`, `set`, `push`, `insert`, `remove`, `truncate`,
`extend_from(&Column)`, `slice(start, stop, step)`, `splice`, `reverse`, `repeat`,
`clear`, `dtype`, `with_capacity`. Type mismatches between `Value`/`Column` dtypes are
programming errors in the binding (return `Err(TypeMismatch)`, never panic across FFI).

### 3.3 Storage strategies (rustique-py)
```rust
pub enum Storage { Native(Column), Object(Vec<Py<PyAny>>) }
```
Strategy selection for **untyped** containers (`schema = Any`):

| Contents (exact types)                          | Strategy        |
|-------------------------------------------------|-----------------|
| empty                                            | `Empty` → first value decides |
| all `bool`                                       | `Bool`          |
| all `int` (not bool), fitting i64                | `I64`           |
| all `float`                                      | `F64`           |
| all `str` (no lone surrogates)                   | `Str`           |
| anything else / mixed                            | `Object`        |

A value that does not fit the current strategy **demotes** the storage to `Object` (O(n),
once). Demotion never loses information: untyped lists always read back values equal to
and of the same type as what was stored. Strategies never re-promote automatically.

For **typed** containers the schema pins the strategy (§4).

`List.strategy` (read-only property) exposes the current strategy name
(`"empty" | "bool" | "i8" … "f64" | "str" | "object"`) for introspection and tests.

### 3.4 Schema (rustique-py)
A compiled, hashable description of an element type, built once from a Python type
expression and cached:

| Python type expression                         | Schema          | Storage | Accepts / converts |
|------------------------------------------------|-----------------|---------|--------------------|
| *(none)*, `object`, `typing.Any`               | `Any`           | adaptive | anything |
| `int`                                          | `Int`           | `I64`, demotes to `Object` on i64 overflow (still validated) | `int` incl. `bool` (stored as int) |
| `float`                                        | `Float`         | `F64`   | `float`, `int` (converted, must be exactly representable or OverflowError for huge) |
| `bool`                                         | `Bool`          | `Bool`  | `bool` only |
| `str`                                          | `Str`           | `Str`, demotes to `Object` for strings with lone surrogates (no UTF-8 form) | `str` only |
| `rs.i8 … rs.u64`, `rs.f32`, `rs.f64`           | `Fixed(DType)`  | that dtype | ints in range (`OverflowError` otherwise), the matching scalar type; floats for f32/f64 |
| `T \| None`, `Optional[T]`                     | `Optional(T)`   | `Object` | `None` or `T` |
| `A \| B`, `Union[A, B]`                        | `Union([..])`   | `Object` | any member (first match converts) |
| `List[T]`, `list[T]`                           | `List(T)`       | `Object` of rustique `List` objects | rustique `List` of identical schema, or any iterable → converted to `List[T]` |
| `Dict[K, V]`, `dict[K, V]`                     | `Dict(K, V)`    | `Object` of rustique `Dict` | analogous |
| `Set[T]`, `set[T]`                             | `Set(T)`        | `Object` of rustique `Set` | analogous |
| any other class `C`                            | `Class(C)`      | `Object` | `isinstance(x, C)` |
| `Annotated[T, ...]`                            | schema of `T`   | | |
| PEP 695 `type Alias = T`                       | schema of `T`   | | |

Validation errors are `TypeError` with the message
`"List[int] expects int, got str at index 3"` (index omitted when not applicable).
Range errors are `OverflowError`.

### 3.5 Type aliases: `List[int]`
`List[T]` returns a cached **alias object** (`ListType`, `DictType`, `SetType`):
- identity-stable: `List[int] is List[int]`;
- callable: `List[int](iterable=())` constructs;
- `isinstance(x, List[int])` checks schema equality; `isinstance(x, List)` checks the class;
- `repr(List[int]) == "List[int]"`; `List[int].schema` is a readable string;
- carries `__origin__` (= `List`) and `__args__` for introspection.

Instances are always of the single class `rustique.List`; `x.schema_type` returns its alias
(`List[int]`, or `List` for untyped).

---

## 4. Public Python API

```python
import rustique as rs
from rustique import List, Dict, Set, X
```

### 4.1 `List`
Constructor: `List(iterable=(), /)`, `List[T](iterable=(), /)`.
Bulk ingest fast paths: builtin `list`/`tuple` (direct item array), `range`
(computed natively), rustique `List` (column clone/convert), buffer-protocol objects whose
format matches a numeric dtype (memcpy; e.g. `array.array`, NumPy), generic iterables.

Full `list` parity (behaviour must match builtin `list`, including exception types):
`__len__`, `__bool__`, `__getitem__` (int → element; slice → new `List` of same schema),
`__setitem__` (int; slice incl. extended slices with length check, accepting any iterable),
`__delitem__` (int; slice), `__contains__`, `__iter__` (**live** iterator, like CPython),
`__reversed__`, `__eq__`/`__ne__` (vs rustique `List` of any schema, or builtin `list`;
other types → `NotImplemented`), `__lt__`/`__le__`/`__gt__`/`__ge__` (lexicographic, same
operand rules), `__add__` (`List` or `list` → new `List`; same schema if both typed equal,
otherwise untyped), `__radd__` (list + List → List), `__iadd__` (extend), `__mul__`,
`__rmul__`, `__imul__`, `__repr__`, `__copy__`, `__deepcopy__`, `__hash__ = None`,
`append`, `extend`, `insert`, `remove`, `pop`, `clear`, `index(value, start=0, stop=…)`,
`count`, `reverse`, `sort(*, key=None, reverse=False)` (native stable sort when `key is None`
and storage is native; NaN ordering must not panic), `copy`.

`repr`: untyped `List([1, 2, 3])`, typed `List[int]([1, 2, 3])`; element reprs are CPython's.

Element reads return Python builtins (`int`, `float`, `bool`, `str`, or the stored object).

Native extras (single Rust call, GIL detached for large inputs where safe):
- `sum()`, `min()`, `max()` — Python semantics (`min`/`max` of empty → `ValueError`;
  int sums overflowing i64 return exact Python `int`).
- `to_list()` → builtin `list`.
- `map(f)` / `filter(f)` → new `List`. `f` is an `Expr` (vectorised, native) or any Python
  callable (per-element slow path). Result schema is inferred from the expression.
- `strategy` (property), `schema_type` (property).

Buffer protocol: numeric/bool strategies export a **read-only** buffer with the dtype's
format. While any export is alive, **every mutation raises `BufferError`**
("cannot modify List while a buffer export exists") — memory safety over convenience.

Equality semantics note: native float storage compares NaN as identical to NaN (as if it
were the same object), matching the common `x in [x]` case for builtin lists.

### 4.2 `Dict`
`Dict(mapping_or_iterable=(), /, **kwargs)`, `Dict[K, V](...)`. Insertion-ordered.
Native key strategies: `I64`, `Str` (plus `Bool` folded into int keys per Python
equality `True == 1`); values use any `Storage` strategy. Object keys use Python
`__hash__`/`__eq__`.
API parity with `dict`: `__getitem__` (`KeyError`), `__setitem__`, `__delitem__`,
`__contains__`, `__len__`, `__iter__` (live; `RuntimeError: dictionary changed size during
iteration`), `__eq__` (vs `Dict` or `dict`), `__repr__` (`Dict[str, int]({'a': 1})`),
`__or__`, `__ror__`, `__ior__`, `keys()`, `values()`, `items()` (view objects with `len`,
`iter`, `contains`; keys/items views support set operators), `get`, `pop`, `popitem`
(LIFO, `KeyError` when empty), `setdefault`, `update`, `clear`, `copy`, `fromkeys`
(classmethod), `to_dict()`, `__hash__ = None`.
Lookup with a key that cannot be converted to the key schema is a miss (`KeyError` /
`False`), never `TypeError` — except unhashable keys, which raise `TypeError` like `dict`.
Numeric key equivalence follows Python: `1`, `1.0`, `True` are the same key.

### 4.3 `Set`
`Set(iterable=(), /)`, `Set[T](...)`. API parity with `set`: `add`, `discard`, `remove`,
`pop`, `clear`, `copy`, `__contains__`, `__len__`, `__iter__`, `__eq__` (vs `Set`/`set`/
`frozenset`), `__le__`/`__lt__`/`__ge__`/`__gt__`, `union`/`intersection`/`difference`/
`symmetric_difference` (+ operators and in-place variants), `update` variants,
`issubset`/`issuperset`/`isdisjoint`, `to_set()`, `__repr__` (`Set[int]({1, 2})`),
`__hash__ = None`.

### 4.4 Fixed-width scalars
`rs.i8, i16, i32, i64, u8, u16, u32, u64, f32, f64`.
- Construction: `i8(x)` accepts ints / objects with `__index__` in range
  (`OverflowError` otherwise); `f32(x)`/`f64(x)` accept floats/ints. `i8("5")` → `TypeError`.
- Operators keep the type: `i8(100) + 27 → i8(127)`; `i8(100) + 28 → OverflowError`.
  Operand rules: same scalar type or Python `int` (range-checked into the type); other
  operand types → `NotImplemented`. Operator *meaning* is Python's: `//` floors, `%` takes
  the divisor's sign, `/` returns a Python `float`, division by zero → `ZeroDivisionError`.
- Rust semantics by name: `wrapping_*`, `checked_*` (→ `None` on overflow), `saturating_*`,
  `overflowing_*` (→ `(value, overflowed)`) for `add, sub, mul, div, rem, neg, shl, shr, pow`
  as Rust defines them; `div_euclid`, `rem_euclid`, `count_ones`, `count_zeros`,
  `leading_zeros`, `trailing_zeros`, `rotate_left`, `rotate_right`, `swap_bytes`,
  `reverse_bits`, `to_be_bytes`, `to_le_bytes`, `from_be_bytes`, `from_le_bytes`,
  class attributes `MIN`, `MAX`, `BITS`.
- Protocols: `__int__`, `__index__`, `__float__`, `__bool__`, `__hash__` (== `hash(int(x))`,
  resp. `hash(float(x))`), rich comparisons with same type and Python numbers,
  `__repr__` → `i8(5)` / `f32(0.1)`, `__str__` → `5` / `0.1` (shortest round-trip digits
  in CPython repr style).
- Scalars are immutable and hashable; they double as element-type markers (`List[rs.u8]`).
- Decided details: `<<`/`>>` drop shifted-out bits like Rust (`u8(255) << 1 == u8(254)`);
  negative shift → `ValueError`, shift ≥ `BITS` → `OverflowError`. `~` is the bit
  complement within the width (`~u8(5) == u8(250)`). `int_scalar ** negative` returns a
  Python `float`; `pow(x, e, m)` is modular. Float `**` follows CPython (`ValueError` for a
  negative base with a fractional exponent, `OverflowError` on overflow) while `+ - *`
  overflow to `inf` per IEEE. Floats/ints out of `f32` range raise `OverflowError`.
  Different scalar classes never compare equal (`i8(1) != i16(1)`), although each equals
  `1`. Where Rust would panic, named methods raise `OverflowError` (`MIN.div_euclid(-1)`)
  or return `None` (`checked_*`). `round`/`math.floor`/… of an int scalar keep its type;
  of a float scalar return `int`.

### 4.5 Expressions: `X`
`X` is the placeholder for "the element". Expressions are immutable trees evaluated
column-at-a-time in Rust.
- Arithmetic: `+ - * / // % **` (with reflected forms), unary `-`, `+`, `abs(X)`.
- Comparison: `== != < <= > >=` (produce bool).
- Logic/bitwise: `& | ^ ~` (bool → logical, ints → bitwise).
- Constants: Python `int`, `float`, `bool`, `str` literals.
- Methods: `X.is_nan()`, `X.cast(dtype)` where dtype is `int`, `float`, `bool`, or a fixed
  scalar class.
- Semantics follow Python: floor division/modulo, true division → float, `**` with a
  negative int exponent → float, division by zero → `ZeroDivisionError`.
  On Python-`int` (`I64`) data, i64 overflow falls back to exact Python-int evaluation
  (result list demotes to object storage); on fixed-width data overflow raises
  `OverflowError`.
- `repr(X * 2 + 1)` → `((X * 2) + 1)`.
- `bool(expr)` raises `TypeError` (prevents `if X > 1:` mistakes; use `&`/`|`, not
  `and`/`or`).
- Evaluating on `Object` storage falls back to per-element Python evaluation.

---

## 5. Rust-facing API

`rustique-core` is a normal Rust library (Column, kernels, Expr, HashIndex, pyhash, pyfmt).
`rustique-py` is also built as an `rlib`, exposing for other Rust code:
```rust
List::from_column(py, Column::I64(vec)) -> PyResult<Bound<'_, List>>   // O(1), no per-element conversion
List::with_column(&self, |col: &Column| ...)                           // borrow native data under the read lock
```
`rustique.arange(start, stop=None, step=1, dtype=int)` demonstrates the Rust→Python path.

### 5.1 Rust interop: the C-API capsule

`List::from_column` only helps code compiled *into* `rustique._rustique`: another extension
that links `rustique-py` as an rlib gets its own, different `List` type object. Like NumPy's
`_ARRAY_API`, the extension therefore publishes a function table as the capsule
**`rustique._rustique._C_API`** (`crates/rustique-py/src/capi.rs`), and other extensions
reach it with `PyCapsule_Import` at run time. Downstream Rust code uses the
**`rustique-api`** crate (depends on PyO3 only, never links rustique):

```rust
let list = rustique_api::list_from_slice(py, &primes)?;      // rustique.List[int], one memcpy
let view = rustique_api::borrow_list::<f64>(&xs)?;           // ListView<f64>: Deref<Target = [f64]>
let total: f64 = { let s: &[f64] = &view; py.detach(|| s.iter().sum()) };
drop(view);                                                  // list mutable again
let words = rustique_api::copy_str_list(&ws)?;               // StrList: one UTF-8 buffer + offsets
let out = rustique_api::list_from_strs(py, words.iter())?;   // rustique.List[str]
```
`examples/rust_extension/` is a complete downstream extension (`rustique_demo`) with tests;
build it with `maturin develop --uv -m examples/rust_extension/Cargo.toml`.

**Table layout (version 1).** `#[repr(C)]`, offsets in machine words (`W = size_of::<usize>()`):

| Offset | Field | Signature |
|---|---|---|
| 0 | `api_version` | `u32` (= 1) |
| W | `abi_size` | `usize`: size of the provider's table in bytes (= 9W) |
| 2W | `list_check` | `fn(obj) -> c_int` |
| 3W | `list_dtype` | `fn(obj) -> c_int` |
| 4W | `list_from_slice` | `fn(dtype: u8, data: *const u8, len: usize) -> *mut PyObject` |
| 5W | `list_from_utf8` | `fn(ptrs: *const *const u8, lens: *const usize, count: usize) -> *mut PyObject` |
| 6W | `list_borrow` | `fn(obj, dtype_out: *mut u8, data_out: *mut *const u8, len_out: *mut usize, token_out: *mut *mut c_void) -> c_int` |
| 7W | `list_release` | `fn(token: *mut c_void)` |
| 8W | `list_copy_utf8` | `fn(obj, data: *mut u8, data_cap, offsets: *mut usize, offsets_cap, data_len_out: *mut usize, count_out: *mut usize) -> c_int` |

All functions are `unsafe extern "C"`; both crates assert these offsets at compile time.

**Versioning.** Fields are only ever appended. Each addition bumps `api_version`; version
*n* contains every field of earlier versions, unchanged in position, type and meaning.
A consumer requires `api_version >=` the version it was written against and uses a field
only if it lies entirely within `abi_size` bytes (`rustique-api` checks both on import and
raises `ImportError` naming both versions if rustique is too old or missing). An
incompatible change needs a new capsule name; the capsule name and table address never
change within a process (the table is a `static`, so the pointer is valid for the life of
the process).

**DType codes** (`u8`, stable; a code is never renumbered or reused, new types get new
codes): `Bool = 0, I8 = 1, I16 = 2, I32 = 3, I64 = 4, U8 = 5, U16 = 6, U32 = 7, U64 = 8,
F32 = 9, F64 = 10, Str = 11`. Bool elements are one byte holding 0 or 1.

**Calling convention.** Callers must be attached to the interpreter (hold the GIL on GIL
builds). Errors return NULL / -1 with a Python exception set; no function unwinds: Rust
panics become `PanicException`. Returned objects are new (owned) references.

**Semantics.**
- `list_from_slice` copies `len` elements (one `memcpy` into a fresh `Vec`, then
  `List::from_column`, so no per-element Python objects). `data` needs no alignment, may be
  NULL when `len == 0`, and must not be written during the call. Result type by dtype:
  i64 → `List[int]`, f64 → `List[float]`, bool → `List[bool]`, others → `List[rs.<dtype>]`.
  Unknown codes and `Str` raise `ValueError`; bool bytes other than 0/1 raise `ValueError`
  (validated on the private copy, so a racing writer cannot inject an invalid `bool`).
- `list_from_utf8` copies `count` strings into a `List[str]`; invalid UTF-8 raises
  `UnicodeDecodeError` (with a note naming the string index).
- `list_borrow` gives zero-copy, read-only access to *fixed-size* native storage. It takes
  a buffer export of the list (`PyObject_GetBuffer`), so it shares the export counter of
  the buffer protocol (§4.1): until the token is passed to `list_release`, the list is
  frozen — every mutation from any thread raises `BufferError` — the data pointer stays
  valid (non-NULL, aligned for the element type, possibly dangling when empty) and the list is kept
  alive by the export's strong reference. Readers may use the data from any thread and
  with the interpreter detached; `list_release` (exactly once per successful borrow, from
  an attached thread; NULL is a no-op) ends it. Non-lists and lists whose strategy is
  `str`, `object` or `empty` raise `TypeError`. The dtype written is that of the storage,
  so an untyped `List([1.0])` borrows as f64 and `List[int]` holding a big int (object
  storage) is rejected. `rustique-api`'s `ListView` releases on `Drop`, checks the dtype
  against `T`, and reads an untyped empty `List()` as an empty slice of any `T` (nothing is
  frozen in that case).
- `list_copy_utf8` copies a str list out as one UTF-8 buffer plus `count + 1` Arrow-style
  offsets, under the list's read lock (no Python runs). It always reports the needed sizes;
  it returns 1 without copying (and without an exception) when a buffer is too small, so
  callers measure, allocate and retry (the list may grow in between). An untyped empty list
  copies as zero strings; other storage raises `TypeError`.
- `list_check` returns 1 for a `rustique.List`, else 0; `list_dtype` returns the storage's
  dtype code, or -1 for non-lists, object storage and untyped empty lists. Neither raises.

**Cost** (CPython 3.15.1, optimised builds, `examples/rust_extension/bench_demo.py`):
returning the 664,579 primes below 10M as a `rustique.List[int]` adds 0.8 ms to the 13.6 ms
sieve, against 12.6 ms for PyO3's `Vec<i64>` → `list` conversion. Borrowing and summing a
10M-element `List[float]` takes 7.0 ms, against 119 ms for PyO3 `Vec<f64>` extraction from
a builtin `list` (309 ms from a rustique `List` through the sequence protocol). The fixed
cost of a borrow/release pair is about 0.1 µs.

---

## 6. Concurrency & safety

- Every container is a `#[pyclass(frozen)]` whose mutable state lives behind a lock.
  Readers share, writers exclude. Lock acquisition must never deadlock against the GIL /
  interpreter: when contended, wait while *detached* from the interpreter.
- **No Python code under a Rustique lock.** Operations are three-phase:
  1. *prepare* (outside the lock): convert/validate Python inputs, compute hashes, may run
     arbitrary Python code;
  2. *commit* (under the lock): pure Rust moves, increfs, index math — no Python calls,
     no decrefs of objects that might run finalizers;
  3. *finalize* (outside the lock): drop replaced `Py<PyAny>` objects (decref → may run
     `__del__`), build Python result objects.
  Comparisons that need Python (`Object` storage `__eq__`, object-keyed dict lookups)
  snapshot the needed references under the lock, compare outside it, and re-validate with
  a mutation counter (retry / `RuntimeError` like CPython).
- Bulk reads of *builtin* mutable sources (`list`, `dict`, `set`) happen inside **one**
  critical section on the source object (`pyo3::sync::with_critical_section`), never
  element-by-element without a lock — on 3.15t that is a data race. Only exact-type
  `int`/`float`/`bool`/`str` extraction runs inside it; anything that could call Python
  code collects strong references first and runs afterwards.
- GC support (`__traverse__`) never blocks: it uses `try_lock` (GC is stop-the-world on
  free-threaded builds).
- Buffer exports freeze the container (see §4.1).
- No `unsafe` outside small, commented FFI fast paths (e.g. reading `PyListObject` items,
  which must not be used for list subclasses). Every `unsafe` block carries a `// SAFETY:`
  comment.
- No panics across FFI: binding code returns `PyResult`; core code returns `Result` for
  anything input-dependent.

---

## 7. Testing & validation
- `cargo test -p rustique-core` — unit + property tests of columns, kernels, expr, index,
  hashing, float formatting.
- `pytest` (Python 3.15.1 and 3.15.1t):
  - `test_list_parity.py` — the historical 1292-line parity suite, ported, plus new cases
    (sort, `+`, ordering, repr, NaN, live iterators).
  - `test_differential.py` — Hypothesis: random operation sequences applied to a builtin
    and a rustique container must produce identical observable results, for every strategy.
  - `test_typed.py`, `test_strategies.py`, `test_expr.py`, `test_scalars.py`,
    `test_dict.py`, `test_set.py`, `test_buffer.py`, `test_threads.py` (free-threaded
    stress; skipped where meaningless).
- `benches/bench.py` — fresh inputs per repetition, result verification, reports
  rustique/builtin ratios.
