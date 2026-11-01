# Rustique

**CPython with a Rust engine: same objects, same results, faster.**

Install it and set one environment variable. Your existing program runs unchanged: builtins
and stdlib functions run Rust code wherever the result is provably the one CPython
computes, and CPython's own code everywhere else. There are no new APIs. Every gain arrives
through the entry points your code already calls (`json.dumps`, `copy.deepcopy`,
`datetime.strptime`, `os.path.join`, `sorted`, …), on the same objects.

```bash
pip install rustique                        # prebuilt wheels, no Rust toolchain
RUSTIQUE=1 python app.py                    # on, no code change (also: RUSTIQUE=1 pytest, gunicorn, …)
RUSTIQUE=0 python app.py                    # off everywhere, even where the code calls rustique.enable()
python -m rustique status                   # what is accelerated on this interpreter, and why not
python -m rustique diff -- pytest -q        # CI: same exit code, output and test outcomes, and the timing
```

Other ways to turn it on: `python -m rustique app.py`, `import rustique; rustique.enable()`,
`pytest --rustique`. All the switches, reports and CI recipes are in
**[docs/USAGE.md](docs/USAGE.md)**.

## What gets faster

Rustique accelerates the parts of the standard library and builtins where real programs
spend measurable time ([impact atlas](docs/IMPACT_ATLAS.md): 35 real workloads profiled).
The numbers below are from the Phase 1 atlas and prototypes. The measured numbers of the
shipped targets replace them in Phase 3.

| area | entry points | where it shows | atlas (time vs CPython) | measured |
|---|---|---|---|---|
| JSON | `json.dumps`, `json.loads` | API services, config, ETL | exact Rust prototype 0.31 small, 0.60 large (M) | ⟨Phase 3⟩ |
| copying | `copy.deepcopy`, `copy.copy`, `dataclasses.asdict/astuple` | state snapshots, isort (44% of its time) | an exact C deepcopy: 0.11–0.18 (M) | ⟨Phase 3⟩ |
| paths | `os.path`/`posixpath`, `pathlib` pure paths, `fnmatch` | file inventories, build tools | `posixpath.join` kernel 0.29 (M) | ⟨Phase 3⟩ |
| date and time text | `strptime`, `strftime`, `isoformat` (datetime, date, time; `time.strptime/strftime`) | log analytics, ETL | `strptime` is 3.7 µs of interpreted Python (M) | ⟨Phase 3⟩ |
| logging | `LogRecord`, `Formatter.format`/`formatTime` | services (66% of a logging-heavy one) | ≤ 0.5 inferred | ⟨Phase 3⟩ |
| utilities | `random` glue, `textwrap`, `urllib.parse`, `statistics` | scripts, web | 0.1–0.35 (M, per call) | ⟨Phase 3⟩ |
| containers, builtins | `list.sort`, `index`/`count`/`in` on lists and tuples; `sum`, `min`, `max`, `sorted`, `any`, `all` | sorting, searching, aggregating | 1M elements: `max(floats)` 0.13, `min(ints)` 0.17, `ints.sort()` 0.18, `x in ints` 0.26 (M) | ⟨Phase 3⟩ |

Whole programs (atlas, inferred from where their time goes):

- stdlib-heavy programs gain 1.3–2×: snapshots, report generation, log analytics, JSON
  services, file inventories;
- the mean of the 35 workloads spends ~10% less time;
- framework-heavy web apps gain ~1.0–1.05×, because their time is in their own bytecode.

⟨Phase 3: measured table of the workload set, plain vs `RUSTIQUE=1`⟩. To see what your own
program hits, run `python -m rustique report -- python app.py`.

## The catch

- **Exact or not at all.** A kernel runs only for inputs where it provably returns what
  CPython returns: same values, types, exceptions and messages, warnings, side effects and
  their order. For anything else (a subclass, an unusual keyword, a monkeypatched
  dependency, a debugger or coverage listening) CPython's original runs. That fallback is
  never measurably slower: ≤ 1.05 on every measured cell, small inputs included. The report
  shows how often each target fell back and why.
- **Most program time is out of reach.** About two thirds of typical CPU time goes to the
  program's own bytecode, attribute lookups, allocation and garbage collection. Rustique
  does not change any of that. If your profile shows no time in the stdlib, expect ~1.0×.
- **Startup costs something: with Rustique on, it exceeds our own 1.05 bar.** Measured on
  `python -c pass` (release wheels, callgrind and wall time; [details](docs/targets/dx.md#14-startup-cells-release-build-m)):
  - **Installed but off** (`RUSTIQUE` unset): one `.pth` line read at every interpreter
    start, nothing imported.
    - bare venv: +0.3 ms, 1.026× on 3.15 and 1.047× on 3.14. Most of it is the
      `utf-8-sig` codec import that any `.pth` file costs;
    - venv that already has a `.pth` (editable installs, setuptools): ≤ 1.014×.
  - **On** (`RUSTIQUE=1`, against the same venv with it unset): +0.5 ms, 1.05× wall on
    3.15, and +0.7 ms, 1.07× on 3.14. That is the extension loading (page faults and
    relocations) plus installing the targets.
  - `import rustique; rustique.enable()` in code costs about the same.

  This is a documented exceedance of the never-slower rule for wave 1. Short-lived
  processes (CLI tools run thousands of times) may not gain enough to pay it back.
- **Supported interpreters: CPython 3.14 and 3.15, including free-threaded (3.14t, 3.15t).**
  - Platforms: Linux x86_64/aarch64 (glibc and musl), macOS arm64/x86_64, Windows x64
    (arm64 experimental).
  - Each target installs only on the CPython versions it was validated on, and reports
    "refused" elsewhere.
  - PyPy, GraalPy and CPython 3.16: `pip` refuses to install.
- **Turning it off.**
  - `RUSTIQUE=0` disables everything without a deploy, including `rustique.enable()`
    calls in code.
  - `RUSTIQUE_EXCLUDE=json` turns off one group; `RUSTIQUE_ONLY=json,copy` keeps only
    those groups.
  - `rustique.disable()` restores every original in a running process.
- **Checking it in CI.**
  - `python -m rustique diff -- <tests>` compares a plain run with an accelerated one:
    exit code, stdout/stderr and every test's outcome.
  - `pytest --rustique` runs the session accelerated and reports per target.
  - `RUSTIQUE_CHECK=1` re-runs CPython's original behind every checkable fast path and
    compares the results.

## Rustique's own containers (opt-in)

Independently of the switch above, Rustique has its own typed containers: `List`, `Tuple`,
`Dict`, `Set`, `FrozenSet`, plus vectorised expressions (`X`) and fixed-width scalars. They
store homogeneous data unboxed in Rust (`Vec<i64>`, `Vec<f64>`, strings…), for typed data,
bulk kernels and zero-copy export. Unlike everything above, they are *not* the builtin
types, so using them is a code change. Use them explicitly, or let
`rustique.enable(native="mypkg.*")` build them from the literals of opted-in modules
([docs/TAKEOVER.md](docs/TAKEOVER.md)).

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

### Why

The original Rustique experiments (see [`docs/ARCHAEOLOGY.md`](docs/ARCHAEOLOGY.md))
wrapped Python objects in Rust and were 1.4–17× *slower* than the builtins: they paid the
Python↔Rust boundary cost on every element and never gained a layout advantage. This
rewrite stores data natively and pays per *operation* instead of per *element*.

### Features

- **In-place acceleration of the builtins** — `rustique.enable()` swaps the C entry points
  CPython calls for `list.sort`, `list`/`tuple` `index`/`count`/`__contains__` and the
  builtin functions `sum`/`min`/`max`/`sorted`/`any`/`all`; nothing else changes (same
  types, identity, `repr`, pickling, specializer and JIT paths). Process-wide, reversible
  (`rustique.disable()`), and decided by measurement: see
  [`docs/DECISIONS.md`](docs/DECISIONS.md) and [`docs/TAKEOVER.md`](docs/TAKEOVER.md).
- **Opt-in native tier** — `rs.enable(native="pkg.*")`, `python -m rustique --native
  'pkg.*' app.py` or `@rs.native` rewrite the literals, displays, comprehensions and
  `list()`/`dict()`/`set()` calls of chosen modules to Rustique containers.
- **Typed containers** — `List[T]`, `Dict[K, V]`, `Set[T]`, `FrozenSet[T]` (plus an
  immutable, hash-compatible `Tuple`) with schemas compiled from
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
  buffers (`array.array`, NumPy) by `memcpy`.
- **Rust interop for other extensions** — a versioned C-API capsule
  (`rustique._rustique._C_API`, like NumPy's) and the `rustique-api` crate let any PyO3
  extension return real `rustique.List[...]` objects from Rust slices with one `memcpy`, and
  borrow a list's native data zero-copy (the list is frozen while borrowed). See
  `examples/rust_extension`: returning 10M-sieve primes costs 0.8 ms of conversion instead
  of 12.6 ms through PyO3's `Vec<i64> → list`, and summing 10M floats from a borrowed list
  takes 7 ms instead of 119 ms through `Vec<f64>` extraction.
- **Free-threading ready** — every container is a frozen PyO3 class with interior locking;
  no Python code ever runs while a Rustique lock is held. Tested on CPython 3.15t with the
  GIL disabled.

### Performance

Measured on a dedicated AWS c7a.8xlarge, CPython 3.15.1 and 3.15.1t (GIL disabled), release
builds; ratio = Rustique ÷ CPython (lower is faster), every result verified against
CPython's. Full report: [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md).

**`rustique.enable()` against plain CPython** (3.15.1 — 3.15.1t):

| | |
|---|---|
| pyperformance (~120 benchmarks), geometric mean | 1.00× — 1.01× |
| pyflakes lint / csv→json pipeline / networkx graph (whole processes) | 0.963 / 0.987 / 1.005 — 0.998 / 0.993 / 1.006 |
| scoreboard, 0–100 elements: cells within 1.05 (instructions) | 3810/3814 — 3814/3814 (every result identical) |
| scoreboard, 10k/1M elements: geometric mean (wall time) | 0.88 — 0.88 (kernel paths 0.46 — 0.48; untouched code 1.00) |
| 1M elements: `max(floats)`, `min(ints)`, `ints.sort()`, `x in ints`, `ints.count(v)` | 0.13, 0.17, 0.18, 0.26, 0.25 — 0.21, 0.21, 0.15, 0.33, 0.33 |
| process startup (`import rustique; rustique.enable()`) | +1.7 ms (1.17×) — +2.2 ms (1.18×) |

**Rustique's own containers against the builtins**, geometric mean over every operation
of the scoreboard (3.15.1):

| | 0–100 elements | 10k/1M elements |
|---|---|---|
| `List` | 1.21 | 0.75 |
| `Tuple` | 1.32 | 0.95 |
| `Dict` | 1.70 | 1.18 |
| `Set` | 1.63 | 0.89 |
| `FrozenSet` | 1.49 | 0.95 |

Bulk operations run as single Rust calls: `lst.sort()` of ints 0.02–0.03, `x in lst` 0.04,
`lst.min()` 0.05, `lst.map(X * 2 + 1)` 0.06 vs a comprehension. Per-element operations on
Rustique containers cannot reach 1.00 on CPython: the interpreter only fast-paths exact
builtins (an empty subclass of `list` already costs 2.4× for `x[i]`), which is why
`rustique.enable()` keeps the builtins and accelerates them in place.

## Develop

Users install wheels (`pip install rustique`). Building from source requires Rust 1.99.1 (pinned in `rust-toolchain.toml`) and CPython ≥ 3.14
(3.15.1 / 3.15.1t are the primary targets; 3.14 and 3.14t are tested too).

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
crates/rustique-py     PyO3 bindings → rustique._rustique (incl. the C-API capsule)
crates/rustique-api    safe Rust client of the capsule, for downstream PyO3 extensions
examples/rust_extension  a downstream extension built on rustique-api
python/rustique        Python package and type stubs
docs/                  ARCHITECTURE.md (contract), DECISIONS.md, TAKEOVER.md, BENCHMARKS.md,
                       ROADMAP.md, ARCHAEOLOGY.md
```
