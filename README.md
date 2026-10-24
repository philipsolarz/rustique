# Rustique

**Python's data model, Rust's memory.**

Rustique gives you `list`, `dict` and `set` with the Python behaviour you know, backed by
typed, unboxed Rust storage. Data is validated once at the boundary and then lives in
native memory, where bulk operations run as single Rust calls, exportable zero-copy to
`memoryview`/NumPy. It runs safely on free-threaded Python.

```python
import rustique as rs
from rustique import List, Dict, Set, X

nums = List[int](range(1_000_000))     # Vec<i64> inside, built natively from the range
nums.sum(), nums.max()                  # one Rust call each
evens = nums.filter(X % 2 == 0)         # vectorised expression, no Python lambda
scaled = nums.map(X * 3 + 1)            # -> List[int]
nums.sort(reverse=True)                 # native stable sort

List[int]([1, "x"])                     # TypeError: List[int] expects int, got str at index 1
List[rs.u8]([1, 2, 300])                # OverflowError: List[u8]: 300 is out of range for u8 at index 2
List[int] is List[int]                  # True — aliases are cached
isinstance(nums, List[int])             # True

import numpy as np
np.asarray(List[float]([1.5, 2.5]))     # zero-copy view of the Rust buffer
```

## Why

The original Rustique experiments (see [`docs/ARCHAEOLOGY.md`](docs/ARCHAEOLOGY.md))
wrapped Python objects in Rust and were 1.4–17× *slower* than the builtins: they paid the
Python↔Rust boundary cost on every element and never gained a layout advantage. This
rewrite stores data natively and pays per *operation* instead of per *element*.

## Features

- **Typed containers** — `List[T]`, `Dict[K, V]`, `Set[T]` with schemas compiled from
  ordinary type expressions: `int`, `float`, `bool`, `str`, `int | None`, unions,
  `List[List[int]]`, `dict[str, list[float]]`, `Annotated`, PEP 695 aliases, any class.
  Full builtin parity: slicing (incl. extended slice assignment), live iterators,
  comparisons, `sort(key=, reverse=)`, views, set algebra, pickling.
- **Adaptive storage strategies** — an untyped `List([1, 2, 3])` stores `Vec<i64>`; append
  `"x"` and it transparently becomes object storage (PyPy-style list strategies). Inspect
  with `.strategy`.
- **Vectorised expressions** — `X` builds expression trees (`X * 2 + 1`, `(X > 0) & (X < 10)`,
  `X.cast(float)`, `X.is_nan()`) evaluated column-at-a-time in Rust, multi-threaded for
  large inputs, with exact Python semantics (floor division, true division, int/float
  comparisons, `ZeroDivisionError`; overflow on Python `int` data falls back to exact
  big-int evaluation).
- **Rust fixed-width scalars** — `rs.i8 … rs.u64`, `rs.f32`, `rs.f64`: arithmetic keeps the
  type and raises `OverflowError`, or pick Rust semantics by name
  (`wrapping_add`, `checked_mul`, `saturating_sub`, `overflowing_shl`, `rotate_left`,
  `to_le_bytes`, …). Hashes match the equal Python number; `f32` reprs are shortest
  round-trip.
- **Zero-copy interop** — numeric lists export read-only buffers (`memoryview`, NumPy);
  while an export is alive the list is frozen (`BufferError` on mutation). Lists ingest
  buffers (`array.array`, NumPy) by `memcpy`. Rust code builds a Python `List` from a
  `Vec<T>` in O(1) (`rustique_py::list::List::from_column`).
- **Free-threading ready** — every container is a frozen PyO3 class with interior locking;
  no Python code ever runs while a Rustique lock is held. Tested on CPython 3.15t with the
  GIL disabled.

## Performance

Indicative ratios versus builtins (rustique time ÷ builtin time, lower is better;
1M elements, CPython 3.15.1, see `benches/` to reproduce):

| Operation | Ratio |
|---|---|
| `List(range(n))` | 0.03 |
| `x in lst` (miss) | 0.08 |
| `lst.sum()` vs `sum(list)` | 0.14 |
| `lst.sort()` (ints) | 0.02 |
| `lst.sort()` (strs) | 0.09 |
| `lst.map(X * 2 + 1)` vs list comprehension | ~0.04 |
| `lst[::2]` | 0.21 |
| `lst[i]` in a Python loop | ~1.6 |
| `for v in lst` | ~1.8 |

Bulk work is dramatically faster; per-element access from Python is slower than a builtin
list (each read must create a Python object). Use Rustique where data stays in Rust
between operations.

## Install & develop

Requires Rust 1.99.1 (pinned in `rust-toolchain.toml`) and CPython ≥ 3.13
(3.15.1 / 3.15.1t are the primary targets).

```bash
uv venv --python 3.15.1 .venv
uv pip install --python .venv/bin/python maturin pytest hypothesis numpy
.venv/bin/maturin develop --uv --profile dev-release
.venv/bin/python -m pytest
cargo test --workspace
```

Free-threaded build: `uv venv --python 3.15.1t .venv-ft` and
`VIRTUAL_ENV=$PWD/.venv-ft CARGO_TARGET_DIR=target-ft .venv-ft/bin/maturin develop --uv`.

## Layout

```
crates/rustique-core   pure Rust: columns, slices, kernels, expression engine, hash index,
                       CPython-compatible hashing and float repr (no Python dependency)
crates/rustique-py     PyO3 bindings → rustique._rustique
python/rustique        Python package and type stubs
docs/                  ARCHITECTURE.md (contract), ARCHAEOLOGY.md, ROADMAP.md
```
