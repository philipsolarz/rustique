# Migrating to Rustique 0.3

Rustique 0.3 removes the **opt-in native types**: Rustique's own containers and everything
built around them. Nothing changes for programs that use Rustique the way it is meant to be
used (`RUSTIQUE=1`, `python -m rustique`, `rustique.enable()`, `pytest --rustique`): the
engine, its kernels and its switches are the same. If your code never mentions
`rustique.List`, `rustique.X`, a scalar type, `native=`/`--native`/`@rustique.native` or
the C-API capsule, you have nothing to do.

## What was removed

| removed | it was | instead |
|---|---|---|
| `rustique.List`, `Dict`, `Set`, `Tuple`, `FrozenSet`, the typed forms `List[int]` & co., `ListType`/`DictType`/`SetType`/`FrozenSetType` | containers with unboxed Rust storage | the builtin `list`/`dict`/`set`/`tuple`/`frozenset`; with Rustique on, `sort`/`sorted`, `sum`/`min`/`max`/`any`/`all`, `in`/`index`/`count` run Rust kernels on them in place. Typed bulk data: `array.array`, NumPy. Validation of element types: your type checker, or pydantic/attrs at the boundary |
| `rustique.X`, `Expr`, `arange` | vectorised expressions over `List` columns | NumPy, Polars |
| `rustique.i8` … `u64`, `f32`, `f64` | fixed-width scalars with Rust arithmetic | NumPy scalars (`numpy.int8`, …), `ctypes` |
| buffer export/import of `List` | zero-copy views | `memoryview`/`array.array`/NumPy (the buffer protocol) |
| the C-API capsule `rustique._rustique._C_API`, the `rustique-api` crate, `examples/rust_extension` | let other PyO3 extensions create and borrow `rustique.List` | in Rust: `pyo3` (`PyList`, `PyBuffer`), `numpy` (rust-numpy), Arrow (`pyo3-arrow`) |
| `rustique.enable(native=...)`, `python -m rustique --native PATTERN` / `--native-main`, `@rustique.native`, the import hook `rustique._takeover._hook` and the rewriter `rustique._takeover._rewrite` | rewrote your literals and `list()`/`dict()`/`set()` calls to build Rustique containers | nothing: `RUSTIQUE=1` accelerates the builtin containers your code already builds |

Using a removed name now fails with a pointer here:

```
>>> rustique.List
AttributeError: module 'rustique' has no attribute 'List': the opt-in native types were removed in Rustique 0.3 (pin rustique<0.3 to keep them); see https://github.com/philipsolarz/rustique/blob/main/docs/MIGRATION.md
>>> rustique.enable(native="mypkg.*")
TypeError: rustique: native= was removed in Rustique 0.3; see https://github.com/philipsolarz/rustique/blob/main/docs/MIGRATION.md
```

## Why

Rustique's promise is CPython with a Rust engine: the same objects and the same results,
faster, with no new APIs (docs/ACCELERATION.md). The native types were the opposite: a
program had to construct them, and then it held objects that are not `list` or `dict`
(`type(x) is list` is false, C extensions see a foreign type, the interpreter's
specialisations for exact builtins do not fire). The measurements that decided it
(docs/atlas/w2-startup.md §4, DECISIONS §10.1):

- None of them speeds up an existing program without a code change.
- They were 60% of the extension's machine code (6.8 MB → 2.3 MB without them), and loading
  that code cost every `RUSTIQUE=1` process +0.12M instructions and ~30 page faults, a third
  of the startup budget.
- They were 68% of the test suite's time and a quarter of the release build time.

What came out of them stays: the in-place kernels for `list.sort`, the builtin functions and
the containers' `in`/`index`/`count` are part of the engine.

## Keeping them

The last commit with the native types is tagged **`legacy-native-types`** (version 0.2.0).
Pin it if you depend on them:

```bash
pip install "rustique<0.3"                                                       # a 0.2 wheel you have
pip install "rustique @ git+https://github.com/philipsolarz/rustique@legacy-native-types"   # from source (Rust 1.99.1)
```

0.2 is not maintained: it gets no new targets, CPython versions or fixes.

## Other changes in 0.3

- **The extension is a top-level module, `_rustique`.** Release wheels install it at the top
  of site-packages, so that the `RUSTIQUE=1` startup line loads it with one import
  (DECISIONS §10.7). `rustique._rustique` remains the same module object under its old name.
  Wheels built from the sdist and editable installs keep it inside the package and reach it
  through a small top-level `_rustique.py`.
- `RUSTIQUE=off`/`false`/`no`/empty still leave nothing loaded at startup, as `0` does; the
  extension is now imported and immediately unregistered in those (rare) spellings, instead
  of never imported. `RUSTIQUE=0` or unset costs what it did.
- `rustique.status()["process"]` no longer has a `native` field.
- Startup: `RUSTIQUE=1` costs 2.8% instructions over an installed-but-off start instead of
  4.5–6.3%, and the extension is 2.3 MB instead of 6.8 MB (docs/targets/engine.md).
