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
Measured on CPython 3.15.1 (callgrind instructions per call on small realistic inputs, and
wall time of real workloads; time with Rustique / time without, lower is better; details in
[docs/targets/](docs/targets/) and [the Phase 3 report](docs/atlas/phase3.md)):

| area | entry points | per call | real workloads |
|---|---|---|---|
| JSON | `json.dumps`, `json.loads`, `float.__repr__` | dumps 0.06–0.29, loads 0.22–0.56 | API server 0.79, CSV/JSON ETL 0.63 |
| copying | `copy.deepcopy`, `copy.copy`, `dataclasses.asdict/astuple` | deepcopy 0.13–0.46 | state snapshots 0.65, isort 0.77 |
| paths | `os.path`/`posixpath`, `pathlib` pure paths, `fnmatch` | `join` 0.27, `fnmatch` 0.12, pathlib 0.05–0.76 | file inventory 0.74 |
| date and time text | `strptime`, `strftime`, `isoformat` (datetime, date, time; `time.strptime/strftime`) | strptime 0.07–0.09, strftime 0.09–0.20 | log analytics 0.59, time-zone events 0.59 |
| logging | `LogRecord`, `Formatter.format`/`formatTime`, `findCaller`, `Logger.level` | one record 0.45–0.51 | logging service 0.82 |
| utilities | `random` glue, `textwrap`, `urllib.parse`, `statistics` | 0.04–0.67 | report rendering 0.44 |
| containers, builtins | `list.sort`, `index`/`count`/`in` on lists and tuples; `sum`, `min`, `max`, `sorted`, `any`, `all` | 1M elements: `max(floats)` 0.13, `ints.sort()` 0.18 | spread thinly |

Whole programs, all groups on (34 workloads, plain vs enabled, identical results):

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| all 34 workloads (geometric mean) | **0.88** (1.14× faster) | **0.84** (1.19×) |
| data processing / developer tools / web | 0.77 / 0.96 / 0.92 | 0.76 / 0.86 / 0.93 |
| best | report rendering 0.44 | isort 0.20 |
| worst | config merging 1.01 | XML feeds 1.01 |

- Stdlib-heavy programs gain 1.2–2.3×: report generation, log analytics, time-zone
  processing, CSV/JSON ETL, state snapshots, file inventories, JSON API servers.
- Framework-heavy programs (Django, Flask, FastAPI, SQLAlchemy, black, pylint, pandas) run at
  0.97–1.01: their time is their own bytecode.
- pyperformance (~120 benchmarks): geometric mean 0.94 on 3.15.1 and 0.96 on 3.15.1t;
  `deepcopy` 0.13–0.28, `json_dumps` 0.25–0.33, `json_loads` 0.57–0.59, logging 0.61–0.79;
  only `python_startup` above 1.05 (1.08–1.10).

To see what your own program hits, run `python -m rustique report -- python app.py`.

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
  `python -c pass` (release wheels, callgrind and wall time, [details](docs/atlas/phase3.md#6-startup-cells)):
  - **Installed but off** (`RUSTIQUE` unset): one `.pth` line read at every interpreter
    start, nothing imported. Bare venv: 1.03× on 3.15 and 1.05× on 3.14 (+0.3–0.45 ms),
    most of it the `utf-8-sig` codec import that any `.pth` file costs; in a venv that
    already has a `.pth` (editable installs, setuptools): ≤ 1.014×.
  - **On** (`RUSTIQUE=1`, against the same venv with it unset): +0.76 ms, 1.08× wall on
    3.15, and +1.09 ms, 1.11× on 3.14. That is the extension loading (page faults and
    relocations) plus installing the targets of modules Python imports at startup.
  - `import rustique; rustique.enable()` in code costs a little more (it also builds the
    status it returns).

  This is a documented exceedance of the never-slower rule for wave 1. Short-lived
  processes (CLI tools run thousands of times) may not gain enough to pay it back.
- **Supported interpreters: CPython 3.14 and 3.15, including free-threaded (3.14t, 3.15t).**
  - Platforms: Linux x86_64/aarch64 (glibc and musl), macOS arm64/x86_64, Windows x64
    (arm64 experimental).
  - Each target installs only on the CPython versions and platforms it was validated on,
    and reports "refused" elsewhere: all 84 on Linux/glibc with 3.15; 74 on macOS and musl
    (no `datetime` text); 30 on Windows ([details](docs/USAGE.md#install)).
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

## Removed in 0.3: the opt-in native types

Rustique 0.2 also shipped its own container types (`List`, `Dict`, `Set`, `Tuple`,
`FrozenSet`), vectorised expressions (`X`), fixed-width scalars, a C-API capsule for other
extensions, and `enable(native=...)` / `--native` / `@rs.native` to build them from your
literals. They were a code change by nature, which is not what Rustique is for, and they
were most of its binary and startup cost. 0.3 removes them; the in-place kernels they led to
(`list.sort`, `sum`/`min`/`max`/`any`/`all`, `in`/`index`/`count`) stay. What to do instead,
and the tag to pin if you depend on them: **[docs/MIGRATION.md](docs/MIGRATION.md)**.

## Performance

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

## Develop

Users install wheels (`pip install rustique`). Building from source requires Rust 1.99.1 (pinned in `rust-toolchain.toml`) and CPython ≥ 3.14
(3.15.1 / 3.15.1t are the primary targets; 3.14 and 3.14t are tested too).

```bash
uv venv --python 3.15.1 .venv
uv pip install --python .venv/bin/python maturin pytest hypothesis
.venv/bin/maturin develop --uv --profile dev-release
.venv/bin/python -m pytest
cargo test --workspace
```

Free-threaded build: `uv venv --python 3.15.1t .venv-ft` and
`VIRTUAL_ENV=$PWD/.venv-ft CARGO_TARGET_DIR=target-ft .venv-ft/bin/maturin develop --uv`.

## Layout

```
crates/rustique-core   pure Rust: CPython-identical algorithms behind the kernels (float repr,
                       JSON, strftime/strptime, paths, random, statistics, logging, textwrap,
                       urllib.parse); no Python dependency
crates/rustique-py     the engine: the `_rustique` extension (registry, switch, targets)
python/rustique        Python package (enable/status/stats, CLI, report, diff) and type stubs
python/_rustique.py    loader for the in-package layout (sdist builds, editable installs)
tools/repack_wheel.py  moves the extension to the top level of release wheels
docs/                  USAGE.md, ACCELERATION.md (contract), DECISIONS.md, MIGRATION.md,
                       BENCHMARKS.md, IMPACT_ATLAS.md; history: TAKEOVER.md, ARCHITECTURE.md
```
