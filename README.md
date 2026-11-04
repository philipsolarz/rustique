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

Rustique accelerates the parts of builtins and the standard library where real programs
spend measurable time ([impact atlas](docs/IMPACT_ATLAS.md): 65 real programs profiled, from
stdlib-heavy scripts to framework apps, services and threaded programs). Measured on CPython
3.15.1 (callgrind instructions per call on small realistic inputs, and wall time of real
programs; time with Rustique ÷ time without, lower is better; details in
[docs/targets/](docs/targets/) and [the wave-2 report](docs/atlas/w2-phase3.md)):

| area | entry points | per call | real programs |
|---|---|---|---|
| JSON | `json.dumps` (also with `default=`), `json.loads`, `float.__repr__` | dumps 0.06–0.29, loads 0.22–0.56 | JSON API server 0.72 |
| copying, dataclasses | `copy.deepcopy`, `copy.copy`, `dataclasses.asdict/astuple` | deepcopy 0.13–0.46, `asdict` 0.34–0.40 | state snapshots 0.65, isort 0.77 |
| regular expressions | `re.compile` (the pattern compiler), `re.match`/`search`/`sub`… module functions | compile 0.15–0.28, `re.match` 0.68 | pyperformance `regex_compile` 0.04 |
| logging | `LogRecord`, `Formatter.format`, `findCaller`, `Handler.handle`, `Logger.level` | one record 0.33 | logging service 0.69 |
| enums, generated classes | `Enum` lookups and `Flag` operators; namedtuple `_make`/`_replace`; frozen dataclass `__init__`; `@contextmanager`, `singledispatch` bookkeeping | 0.18–0.73 | custom applications 0.94 |
| paths | `os.path`/`posixpath`, `pathlib` pure paths, `fnmatch` | `join` 0.27, `relpath` 0.08, `fnmatch` 0.12 | file inventory 0.72 |
| date and time text | `strptime`, `strftime`, `isoformat` (datetime, date, time; `time.strptime/strftime`) | strptime 0.07–0.09, strftime 0.07–0.09 | log analytics 0.58, time-zone events 0.56 |
| utilities | `random`, `textwrap`, `urllib.parse`, `statistics` | 0.04–0.67 | report rendering 0.36 |
| containers, builtins | `list.sort`, `sorted`, `sum`, `min`, `max`, `any`, `all`; `index`/`count`/`in` on lists and tuples | sort 0.37–0.94; 1M elements: `max(floats)` 0.13, `ints.sort()` 0.18 | spread thinly |
| free-threaded builds | `set.__reduce__` (pickling and copying sets) without CPython's lock contention | | isort 0.20 on 3.15t (CPython: 4× slower than with the GIL) |

Whole programs, every default group on (65 programs, plain vs enabled, identical results):

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| all 65 programs (geometric mean) | **0.870** (1.15× faster) | **0.872** (1.15×) |
| data processing / developer tools / web | 0.75 / 0.96 / 0.88 | 0.74 / 0.86 / 0.90 |
| framework apps / third-party-heavy / custom applications | 0.94 / 0.96 / 0.94 | 0.96 / 0.97 / 0.97 |
| services / threaded programs (8 threads) | 0.85 / 0.69 | 0.89 / 0.79 |
| best | report rendering 0.36 | isort 0.20 |
| worst | a small bytecode interpreter 1.006 | the same, 1.018 |
| whole processes, startup included (8 programs) | 0.74 | 0.74 |
| pyperformance (122 benchmarks) | 0.911 | 0.936 |

- Stdlib-heavy programs gain 1.4–2.8×: report generation, log analytics, time-zone
  processing, logging services, JSON APIs, state snapshots, file inventories.
- Framework apps (Django, DRF, Flask, FastAPI, falcon, Strawberry) and custom domain code now
  gain too, mostly 1–6% (falcon 0.80, a rules engine 0.85): enums, generated classes, `re`,
  logging and json reach them.
- Third-party-heavy code gains least (0.96; attrs/cattrs 0.85, pydantic, jsonschema and PyYAML
  ~0.99–1.00): its time is the library's own bytecode.
- Two opt-in tiers go further (`RUSTIQUE_TIERS=interp,gc`): framework apps 0.933 and custom
  applications 0.920 with the `interp` tier; the `gc` tier gives GC-heavy programs 1–5%. Off by
  default ([why](docs/DECISIONS.md)); details in [USAGE](docs/USAGE.md#opt-in-tiers).

To see what your own program hits, run `python -m rustique report -- python app.py`; to get
an estimate before you try it, `python -m rustique estimate -- python app.py`.

## The catch

- **Exact or not at all.** A kernel runs only for inputs where it provably returns what
  CPython returns: same values, types, exceptions and messages, warnings, side effects and
  their order. For anything else (a subclass, an unusual keyword, a monkeypatched
  dependency, a debugger or coverage listening) CPython's original runs. That fallback is
  never measurably slower: ≤ 1.05 on every measured cell, small inputs included, and at 8
  threads on free-threaded builds. The report shows how often each target fell back and why.
- **Most program time is out of reach by default.** In the 65 programs, about a third of CPU
  time is the standard library; the rest is the program's own bytecode, third-party code,
  attribute lookups, allocation and garbage collection, which the default tier leaves to
  CPython. If your profile shows no time in the standard library, expect ~0.97–1.0×.
- **Open never-slower items** (measured; [DECISIONS §13](docs/DECISIONS.md)): one custom
  program (a small bytecode interpreter) runs 1.02× on 3.15.1t; pyperformance `coroutines`
  reads 1.07× on 3.15.1t (a layout effect, *inferred*).
- **Startup costs something: with Rustique on, its wall time exceeds our own 1.05 bar.**
  Measured on `python -c pass` (release wheels, callgrind and wall time,
  [details](docs/atlas/w2-phase3.md#72-startup-cells-t8--9d0abae)):
  - **Installed but off** (`RUSTIQUE` unset): one `.pth` line read at every interpreter
    start, nothing imported. Bare venv: 1.03× on 3.15 and 1.05× on 3.14, most of it the
    `utf-8-sig` codec import that any `.pth` file costs; in a venv that already has a `.pth`
    (editable installs, setuptools): ≤ 1.016×.
  - **On** (`RUSTIQUE=1`, against the same venv with it unset): 1.028× instructions, +0.58 ms
    (1.06× wall) on 3.15 and +0.75 ms (1.08×) on 3.14: the extension loading plus installing
    the targets of modules Python imports at startup. Targets validate at their first call.
  - Short-lived processes (CLI tools run thousands of times) may not gain enough to pay it back.
- **Supported interpreters: CPython 3.14 and 3.15, including free-threaded (3.14t, 3.15t).**
  - Each target installs only on the CPython releases and platforms it was validated on, and
    reports "refused" elsewhere, with the reason. On Linux with 3.15.1: 110 targets (104 on
    3.15.1t); with a recorded 3.14 release (3.14.1, .5, .6, .8, .9, .10): 105 (99
    free-threaded); macOS and musl 103 on 3.15; Windows 80.
  - Patch releases this Rustique release did not validate (newer ones, and 3.14.0,
    3.14.2–3.14.4, 3.14.7) refuse (`unvalidated-patch`; [details](docs/USAGE.md#python-versions-and-patch-releases)).
  - Platforms: Linux x86_64/aarch64 (glibc and musl), macOS arm64/x86_64, Windows x64/arm64;
    fewer targets outside Linux/glibc ([what refuses where](docs/USAGE.md#install)).
  - PyPy, GraalPy and CPython 3.16: `pip` refuses to install.
- **Turning it off.**
  - `RUSTIQUE=0` disables everything without a deploy, including `rustique.enable()`
    calls in code; `RUSTIQUE_TIERS=none` turns off the opt-in tiers.
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
| 65 real programs, geometric mean | 0.870 — 0.872 |
| pyperformance (122 benchmarks), geometric mean | 0.911 — 0.936 |
| scoreboard, 0–100 elements: cells within 1.05 (instructions) | 3814/3814 — 3814/3814 (every result identical) |
| scoreboard, 10k/1M elements: geometric mean (wall time) | 0.83 (kernel paths 0.45) — not run |
| 1M elements: `max(floats)`, `ints.sort()`, `sorted(list)` | 0.12, 0.21, 0.30 |
| process startup, `RUSTIQUE=1` vs installed-but-off (instructions / wall) | 1.028 / 1.059 (3.14.9: 1.029 / 1.075) |

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
