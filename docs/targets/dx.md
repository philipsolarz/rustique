STATUS: wave 1, workstream `dx`, 2026-11-01. Design decisions and measurements of the user-facing
layer. Usage: [`docs/USAGE.md`](../USAGE.md). Every number is **M** (measured; how) or
**I** (inferred; from what).

# dx: turning Rustique on, off, inside out, and in CI

## Summary

| deliverable | what ships | key measurement / decision |
|---|---|---|
| zero-code switch | `_rustique_startup.pth` + `_rustique_startup.py` at the site-packages root (in the wheel and the sdist) | no PEP 829 `.start` file: on 3.15.1 it costs **1.72×** `python -c pass` (§1.2). Installed but unset: **1.025× Ir** (3.15), **1.044×** (3.14) in a bare venv; **≤ 1.012×** where any other `.pth` exists (§1.3) |
| control | `RUSTIQUE=0` kill switch (overrides code), `RUSTIQUE_ONLY` / `RUSTIQUE_EXCLUDE` (narrow only), `-E`/`-I` honoured | §2 |
| visibility | `RUSTIQUE_STATS`, `RUSTIQUE_REPORT` (stderr / file / JSON), `python -m rustique status`, `python -m rustique report -- <cmd>` | §3 |
| CI | `python -m rustique diff -- <cmd>` (exit code, stdout/stderr, per-test outcomes, timing); pytest plugin `--rustique` (fails on unexpected refusals or check mismatches, xdist-aware); check mode `RUSTIQUE_CHECK=1\|raise` | §4 |
| wheels | `.github/workflows/wheels.yml`: 8 platforms × cp314/cp314t/cp315/cp315t + sdist, each wheel `pip install`ed into a clean venv and smoke-tested; PR subset | local proof: manylinux_2_28 wheels (zig) on all 4 builds, 11/11 checks each (§5) |
| no-wheel interpreters | `requires-python = ">=3.14,<3.16"` | pip on PyPy 3.11 and CPython 3.16.0a1: "requires a different python version" (§5.3) |
| docs | README top rewritten; `docs/USAGE.md` | placeholders `⟨Phase 3⟩` for the lead's measured numbers |
| real projects | attrs, rich, sqlglot test suites: plain, `RUSTIQUE=1`, `--rustique`, `diff` | §7 |

## 1. Zero-code switch

### 1.1 Mechanism

The wheel puts two files at the root of site-packages (maturin `include`, format
`["sdist", "wheel"]`; `python/` is the python-source, so `python/X` lands at the wheel root):

- `_rustique_startup.pth`, one import line, executed by `site` at every interpreter start:

  ```
  import os; 'RUSTIQUE' in os.environ and __import__('_rustique_startup')
  ```

  With `RUSTIQUE` unset nothing is imported (`os` is already loaded by `site`).
- `_rustique_startup.py` (outside the `rustique` package: importing `rustique.anything` runs
  `rustique/__init__.py`, which loads the extension). Importing it runs `start()`: honour
  `-E`/`-I` (`sys.flags.ignore_environment`), parse `RUSTIQUE` (`1/true/yes/on` → enable;
  `0/false/no/off`/empty → nothing; anything else → one-line warning), then
  `rustique._switch.startup()` = `enable(_source="RUSTIQUE=1")`. Every failure (no extension
  for this interpreter, a broken install) prints one line to stderr and returns: the switch
  never breaks interpreter startup (tested with a `rustique` package that raises on import).
  `start()` is idempotent and is the entry point a future `.start` file will name.

`maturin develop` (editable) does not install `include` files, so development venvs have no
startup file and `RUSTIQUE=1` does nothing there (`python -m rustique status` says so). The
tests exercise the shipped files through `site.addsitedir(<copy>)` in subprocesses.

### 1.2 Why no PEP 829 `.start` file (yet) — an upstream issue to file

On 3.15 a `.start` file is the sanctioned replacement for `.pth` import lines, and a
matching `.start` file disables the `.pth`'s import lines. But `site` resolves each entry
point with `pkgutil.resolve_name(entrypoint, strict=True)` (`Lib/site.py`,
`_execute_start_entrypoints`), and `pkgutil` is imported lazily on first use: one `.start`
file imports `pkgutil` (→ `collections`, `functools`, `importlib.util`,
`importlib.machinery`) and `re` (→ `enum`, `re._compiler`, `re._parser`) and compiles
`resolve_name`'s regex.

`benches/dx/startup.py synthetic`, fresh `python -m venv` per variant, `python -c pass`,
callgrind whole-process Ir (PYTHONHASHSEED=0) and wall medians of 150–300 interleaved runs,
dev box (**M**):

| variant | 3.15.1 Ir | wall | 3.15.1t Ir | wall | 3.14.9 Ir | wall |
|---|---:|---:|---:|---:|---:|---:|
| empty venv | 38 666 921 | 9.41 ms | 51 239 107 | 11.75 ms | 40 610 825 | 9.43 ms |
| `.start` → `_rustique_startup:start` | **1.724×** | **1.685×** (+6.4 ms) | 1.707× | 1.692× | 1.000× (3.14 ignores `.start`) | 1.001× |
| `.pth` import line (shipped) | 1.026× | 1.033× | ~1.00× | 1.013× | 1.045× | 1.049× |

`-X importtime` of the `.start` variant (3.15.1): `pkgutil` 2.97 ms cumulative, `re`
2.19 ms (of which `enum` 0.92 ms). **Upstream**: any package adopting `.start` (as PEP 829
asks) makes every Python process ~6 ms slower until another import pays for `re`/`pkgutil`.
Suggested fix for CPython: resolve `pkg.mod:callable` in `site` with
`importlib.import_module` + `getattr` (both already loaded) and keep `pkgutil` for the
error path. To file (lead), with this table.

Plan: ship `_rustique_startup.start` (content: `_rustique_startup:start`) once `site` stops
paying this, or for 3.18+ where `.pth` import lines are silently ignored (PEP 829 schedule;
`Lib/site.py` `_warn_future_us(..., remove=(3, 18))`). Wheels are per-minor (cp314, cp315),
so the `.start` file can be added for the minors that need it without touching the others.

### 1.3 What the `.pth` line costs (`RUSTIQUE` unset)

Most of it is not the line: **any** `.pth` (or `.start`) file makes `site` decode it with
`utf-8-sig`, which imports `encodings.utf_8_sig` (+770k Ir on 3.15, +1.44M on 3.14). Venvs
that already have a `.pth` (editable installs of the project itself — `uv sync`, `pip install
-e .` —, setuptools' `distutils-precedence.pth`, virtualenv's `_virtualenv.pth`) pay that
already. Decomposition (**M**, same method, Ir ratio vs empty venv):

| variant | 3.15.1 | 3.14.9 |
|---|---:|---:|
| a `.pth` with only a comment | 1.0199 | 1.0355 |
| `import os` | 1.0228 | 1.0404 |
| shipped line | 1.0249 | 1.0445 |
| alternatives: `os.environ.get(...)` / `posix.environ` bytes test / no spaces / with `.start()` call | 1.0254 / 1.0248 / 1.0250 / 1.0255 | 1.0451 / 1.0440 / 1.0444 / 1.0452 |
| another `.pth` present, without → with ours | 1.0068 | 1.0119 |

All line variants are within 0.1% of each other: the exec/compile of any statement dominates
(the shipped one is the cheapest portable variant; `posix.environ` would not work on
Windows). 3.15t: within noise (Ir 0.997, wall 1.003 with another `.pth`).

**With the real wheel** (`pip install` of the zig-built manylinux_2_28 wheel into a fresh
`python -m venv`, against an identical venv without it; `benches/dx/startup.py venvs`,
3 callgrind runs, 300 interleaved wall runs; **M**):

| build | `RUSTIQUE` unset: Ir | wall | `RUSTIQUE=1`: Ir | wall |
|---|---:|---:|---:|---:|
| 3.15.1 | 1.0251 | 1.033 (+0.31 ms) | 1.249 | 1.257 (+2.42 ms) |
| 3.15.1t | 0.996 | 1.014 (+0.16 ms) | 1.230 | 1.235 (+2.77 ms) |
| 3.14.9 | 1.0443 | 1.046 (+0.45 ms) | 1.252 | 1.271 (+2.63 ms) |
| 3.14.9t | 1.001 | 1.020 (+0.24 ms) | 1.220 | 1.237 (+2.88 ms) |

Unset: within the ≤ 1.05 bar on every build; the ≤ 1.01 target is met only where another
`.pth` exists — impossible on a bare venv for any `.pth`/`.start` mechanism (the codec
import is in `site`). Lead ruling (2026-11-01): ship the `.pth`.

**`RUSTIQUE=1` costs 1.22–1.25×** (`python -c pass`, dev-release wheel, 13 wave-0
targets). Decomposition on 3.15.1 (callgrind Ir, **M**): `import rustique` +7.46M (the
extension: dlopen of a 6.4 MB `.so` and PyO3 module init — 86%), `enable()` +1.0M,
`status()` +0.26M; the DX Python layer (`_rustique_startup`, `_switch`) is ~0.1 ms wall
(re-import timing). Dropping a separate adapter module saved one import (~35 µs). The
extension's import cost is accel-core's/the lead's (Phase 0 measured `import rustique;
enable()` at 1.17× too): this is above the 1.05 bar for `python -c pass` and is the biggest
open startup item (§9).

## 2. Control

| path | `RUSTIQUE=0` | `RUSTIQUE_ONLY` / `RUSTIQUE_EXCLUDE` | `RUSTIQUE_STATS` / `_REPORT` / `_CHECK` |
|---|---|---|---|
| `RUSTIQUE=1` at startup | n/a (0 is not 1) | applied | applied |
| `python -m rustique app.py` | no-op, runs plain | applied (∩ / ∪ with `--only`/`--exclude`) | applied |
| `rustique.enable()` in code | **no-op** | applied (∩ / ∪ with `only=`/`exclude=`) | applied |
| `rustique.native(f)` | returns `f` unchanged | – | – |
| `pytest --rustique` | no-op; summary says "disabled by RUSTIQUE=0" | applied | applied |
| `report` / `diff` commands | refuse (exit 2): they exist to enable | passed to the enabled run | set by the command |

Decisions:

- **The kill switch overrides an explicit `rustique.enable()`** (recommended in the brief;
  the contract says "every enable path"). It also disables the native tier: under
  `RUSTIQUE=0` a process is plain CPython, even for code that opted into Rustique
  containers through `native=` (such code still works with explicit `rustique.List`).
  `status()["process"]["switch"]` records `killed` and how many enable calls were ignored.
- **The environment narrows, never widens**: env `only` ∩ code `only` (resolved to target
  names through `status()` when both mix groups and targets), env `exclude` ∪ code
  `exclude`. An operator can always switch a group off without touching code; nothing in
  the environment can turn on what the code excluded.
- **Unknown names**: from the environment → one stderr warning listing the groups, nothing
  installed extra (never an exception at startup); from code → `ValueError` (accel-core).
- **`-E`/`-I`**: all `RUSTIQUE*` variables are ignored, like `PYTHON*` ones (so
  `python -I` helpers in CI do not start Rustique).
- **Values**: `1/true/yes/on`, `0/false/no/off`, case-insensitive; anything else warns.
- **Per-project config** (`[tool.rustique]` in pyproject.toml): not added. Reading it would
  mean locating and parsing a TOML file at every enable (I: `tomllib` import alone is
  ~1 ms), the environment and `enable()` arguments cover every case met in §7, and a file
  that silently changes behaviour per directory works against "the environment can only
  turn things off". Revisit if users ask.

## 3. Visibility

- `status()` = the framework's `{"targets": [...], "process": {...}}` plus
  `process["switch"]` = `{source, killed, killed_calls, only, exclude, stats, report, check,
  env}`; `source` is the first enable path (`RUSTIQUE=1`, `rustique.enable()`,
  `python -m rustique`, `pytest --rustique`, `python -m rustique status`).
- Report (`rustique/_report.py`): header (version, CPython and build, pid, how enabled),
  state counts, check-mode line, then installed targets sorted by hits with fallbacks by
  reason, pending imports, refusals with `[category] reason`, groups not selected. Killed
  processes say so.
- `RUSTIQUE_REPORT`: `1`/`stderr` → text on stderr at exit (`atexit`); a path → appended
  (`{pid}` substituted; one `os.write` of `O_APPEND` per process, so concurrent processes
  don't interleave in practice); `*.json` → one JSON record per process
  (`{pid, argv, status, stats, counters}`), which `report`/`diff` merge (counters add up; a
  target counts as installed if it was in any process). Processes ending in `os._exit`
  (forked `multiprocessing` children) write nothing (documented).
- `python -m rustique status [--json]`: interpreter, Rustique location, whether the startup
  file is installed, the `RUSTIQUE*` variables, process-level refusal (e.g.
  "CPython 3.16 pre-release"), JIT, then per group the targets by state and every refusal.
- `python -m rustique report [--json] [-o F] [--check] -- <cmd>`: runs the command with
  `RUSTIQUE=1 RUSTIQUE_STATS=1 RUSTIQUE_REPORT=<tmp>.json`, prints the merged report on
  stderr (the command's stdout stays clean), exits with the command's exit code; says so
  when no Python process reported (wrong interpreter).

## 4. CI verification

### 4.1 `python -m rustique diff -- <cmd>` (`rustique/_diff.py`)

Plain run with `RUSTIQUE=0` (so a `rustique.enable()` in the program is neutralised too —
a true baseline), enabled run with `RUSTIQUE=1` + counters + JSON report; same
`PYTHONHASHSEED` (0 unless set). Compared: exit code, stdout, stderr after normalisation
(addresses `0x…`, pytest `in 1.23s`, `--durations` lines, `pytest-of-<user>/pytest-<n>`,
`--randomly-seed=`; `--ignore REGEX` adds, `--no-normalize` removes), and for pytest
commands (`pytest`, `py.test`, `python -m pytest`) the outcome of every test from an added
`--junitxml` (`-o junit_family=xunit2`; `-p no:cacheprovider` unless the user passes
`--lf`/`--ff`/`--sw`…, so run 1 cannot steer run 2). Outcomes: passed / failed / error /
skipped / xfailed; tests present on one side only are listed. Timing: wall per run,
medians with `--repeat N` (pairs interleaved). The enabled run's report is printed, and an
"identical" with **no accelerated call** is called out — and if no Python process reported
at all (Rustique not installed in the command's interpreter), exit 2, so CI cannot pass
vacuously. `--check` adds check mode to the enabled run (mismatches → exit 1). Refuses a
command containing `--rustique` (the plugin's summary would differ between runs).

### 4.2 pytest plugin (`rustique/pytest_plugin.py`)

Entry point `pytest11: "rustique.pytest_plugin" = "rustique.pytest_plugin"` — named like the
module, so `-p rustique.pytest_plugin` (needed with `PYTEST_DISABLE_PLUGIN_AUTOLOAD`) loads
the entry point instead of registering the module twice (pluggy would raise "Plugin already
registered under a different name"). Inactive without `--rustique` (no output; the only
cost is importing `rustique` when pytest loads plugins, ~1.5 ms of a ≥ 100 ms pytest start).

- `--rustique`: counters on, `enable()` in `pytest_configure` (before collection).
- Terminal summary: `=== rustique ===` section with the report (`--rustique-report-limit`).
- Session fails (exit 1, if it would otherwise pass) when a selected target is refused with
  a category other than `version`/`build`/`jit` (accel-core's `refusal` field), unless
  `--rustique-allow-refused NAMES|all`; or when check mode found a mismatch.
- `--rustique-only` / `--rustique-exclude`; `--rustique-check` / `--rustique-check-raise`.
- xdist: each worker enables Rustique (the option is forwarded) and returns its record via
  `workeroutput`; the controller merges them (`pytest_testnodedown`, an optional hook).

### 4.3 Check mode

Agreed with the lead and implemented by accel-core (`TargetSpec.checkable`, a CHECK flag
sharing the counters' byte, `accel::check::verify`, fallback reason `check`). dx wires
`RUSTIQUE_CHECK=1|warn|raise|0`, `python -m rustique --check|--check-raise`, `report
--check`, `diff --check`, `pytest --rustique-check[-raise]`, the report's check-mode line
and `CHECK MISMATCHES` (any mismatch fails `diff` and the pytest session).

## 5. Wheels

### 5.1 CI (`.github/workflows/wheels.yml`)

- Triggers: tags `v*` and manual dispatch → full matrix + sdist + completeness check; pull
  requests touching the build (`pyproject.toml`, `Cargo.*`, `crates/**`, `python/**`, the
  workflow) → linux x86_64 cp315 + cp315t only. The old tag-only `wheels`/`sdist` jobs of
  `ci.yml` are replaced.
- Matrix: manylinux_2_28 x86_64 (`ubuntu-24.04`) and aarch64 (`ubuntu-24.04-arm`, native),
  musllinux_1_2 x86_64 and aarch64, macOS arm64 (`macos-15`) and x86_64 (`macos-15-intel`),
  Windows x64 (`windows-2025`), Windows arm64 (`windows-11-arm`, `continue-on-error`) ×
  cp314, cp314t, cp315, cp315t, built with `PyO3/maturin-action` (`--release`). Linux builds
  run in the manylinux/musllinux containers (`-i python3.14t` …); macOS/Windows build
  against `actions/setup-python` interpreters by exact path.
- Smoke (`.github/scripts/smoke_all.sh` + `wheel_smoke.py`, stdlib only): for each
  interpreter, a fresh `python -m venv`, **`pip install --no-index --find-links dist
  rustique`**, then from outside the source tree: import; enable + a battery
  (`sorted`/`max`/`sum`/`in`/`index`/`count`, `json.dumps/loads`, `copy.deepcopy`,
  `posixpath.join`, `datetime.strptime`, float reprs) identical to plain; `RUSTIQUE` unset
  imports nothing; `RUSTIQUE=1` enables at startup through the shipped `.pth` with targets
  installed; same battery output; `RUSTIQUE=0` makes `enable()` a no-op; `python -m rustique
  status`; `python -m rustique diff` identical. musl wheels are smoke-tested in `alpine:3.22`
  with uv-provided musl CPythons (an interpreter uv cannot provide is skipped with a
  warning).
- sdist job: `maturin sdist`, then `pip wheel` from the sdist and the same smoke test (the
  sdist must be complete: it carries the startup files and `rust-toolchain.toml`).
- `complete` job: fails unless every (cp314, cp314t, cp315, cp315t) × (manylinux,
  musllinux) × (x86_64, aarch64), macOS × 2, win_amd64 wheel and the sdist exist.
- Validated with `actionlint` 1.7.12 (no findings). Not run on GitHub from here (no push);
  the first tag/dispatch run is the real proof (§9).

### 5.2 Local proof (**M**, dev box)

`maturin build --profile dev-release --zig --compatibility manylinux_2_28` (ziglang 0.17
from PyPI) for 3.15.1, 3.15.1t, 3.14.9, 3.14.9t → `rustique-0.2.0-cp31{4,5}-cp31{4,5}{,t}
-manylinux_2_28_x86_64.whl`; `smoke_all.sh` with real `pip` into fresh venvs: **11/11
checks on each of the 4 builds** (44 ok). The sdist contains `python/_rustique_startup.{py,pth}`.

### 5.3 Interpreters without a wheel

Decision: **`requires-python = ">=3.14,<3.16"`** — refuse at install time, never by a failed
Rust build. Measured with a PEP 503 index that carries Requires-Python like PyPI
(`benches/dx/simple_index.py`) serving the 4 wheels and the sdist:

| interpreter | pip | uv |
|---|---|---|
| PyPy 3.11.17 | `ERROR: Ignored the following versions that require a different python version: 0.2.0 Requires-Python >=3.14,<3.16` | "the current Python version (3.11.17) does not satisfy Python>=3.14,<3.16" |
| CPython 3.16.0a1 | same message as PyPy | **builds from the sdist** (uv lets 3.16 pre-releases satisfy `<3.16`); with Rust available the build succeeds (PyO3 0.30.1 does build for 3.16.0a1) and every target is refused at runtime: `status()["process"]["interpreter"] = "CPython 3.16 pre-release (0x31000a1)"`, nothing accelerated, never wrong |
| CPython 3.15 (control) | installs the cp315 wheel | installs |

Without an index (plain `--find-links`), pip cannot see Requires-Python before building the
sdist and fails with a build error instead; PyPI provides the metadata, so users get the
messages above. Alternatives rejected: no cap (pip on PyPy/3.16 attempts a Rust build and
fails with cargo/PyO3 errors, or without Rust "Cargo is not installed" — misleading); a pure
fallback wheel (installs silently where nothing can work; `rustique.List` would fail later).
Cost of the cap: projects resolving for `python >=3.14` without an upper bound (Poetry, PDM)
need a marker (`rustique; python_version < "3.16"`); raise the cap when a release validates
the next minor.

## 6. Docs

README top: one-line pitch, five-line quick start, "What gets faster" (atlas numbers, tagged,
with `⟨Phase 3⟩` columns for the lead), "The catch" (exactness and fallback, out-of-reach
time, startup with measured numbers, supported interpreters/platforms, off switches, CI),
then the opt-in containers further down. `docs/USAGE.md`: install, on/off, fine-grained
control, report, CI recipes (diff, plugin, check mode), env var and API reference, CLI
reference, FAQ, troubleshooting.

## 7. Real projects

`benches/dx/real_projects.sh` (re-run it at integration with all targets merged): for each
project a fresh venv of python-build-standalone 3.15.1, `pip install` of the rustique wheel
(so the startup `.pth` is there, as for users), the project at its default-branch HEAD
(shallow clone; commit recorded) installed editable with its test dependencies, one warm-up
run (compiles the `.pyc` files), then `pytest` plain, `RUSTIQUE=1 pytest`,
`pytest --rustique` and `python -m rustique diff -- pytest`. 13 wave-0 targets (containers,
builtins) — the stdlib targets were not merged yet. Dev box, shared, 2 CPUs (wall times are
indicative only; **M**).

| project (commit) | tests | plain | `RUSTIQUE=1` | `--rustique` | `diff` |
|---|---|---:|---:|---:|---|
| attrs (c8bb9bf, 2026-10-24) | 1413 | 6.96 s | 7.09 s | 7.14 s | identical (1413 outcomes), 7.58 vs 7.62 s (1.006) |
| rich (9d8f9a3, 2026-06-23) | 981 | 3.78 s | 3.79 s | 3.79 s | identical (981 outcomes), 4.19 vs 4.18 s (0.998) |
| sqlglot (6de2df3, 2026-10-31) | 1452 (+22,683 subtests) | 70.91 s | 69.41 s | 69.19 s | identical (1452 outcomes; 8 processes reported), 72.39 vs 70.52 s (0.974) |

Same pass/fail sets in every mode. attrs fails 2 packaging-metadata tests, rich 14 and
sqlglot 21 tests in the *plain* runs as well (editable install / 3.15 environment); `diff` reports
"identical" and exits 0 — it compares, it does not judge.

What the experience surfaced, and what changed:

1. **False alarm, fixed**: under `RUSTIQUE=1` every pytest run printed two
   `PytestAssertRewriteWarning: Module already imported so cannot be rewritten; rustique`
   (and `_rustique_startup`). pytest marks every top-level module of a distribution that
   has a `pytest11` entry point for assertion rewriting and warns when one is already
   imported — and the startup switch imports both before pytest starts. `diff` flagged it
   on attrs and rich (stdout and the warnings count differed). Fix: `PYTEST_DONT_REWRITE`
   in both module docstrings (pytest's documented opt-out), and the `.pth` line no longer
   imports the hook under `RUSTIQUE=0`. The wheel smoke test now checks that
   `RUSTIQUE=1 pytest` adds no warning and that `pytest --rustique` loads through the entry
   point.
2. **Timing skew, fixed in the runner**: the first run of a fresh checkout compiles every
   `.pyc` (attrs: 11.4 s vs 7.0 s). The runner does a warm-up run; `diff --repeat N`
   interleaves pairs for users.
3. **Project quirks, not Rustique**: attrs needs `-p no:mypy-plugins` without mypy; sqlglot's
   `tests/test_sqlglotc.py` needs the compiled `sqlglotc` extension (ignored) and its other
   tests need `pytz`, `duckdb`, `pandas`.
4. **Signals for accel-core** from the reports (counters on): in attrs `builtins.min` took
   35,968 fast paths and **27,425 `deopt` restarts** (43% of calls start in Rust and restart
   in CPython); `tuple.__contains__` falls back on 39% of calls (`args`); in rich
   `builtins.sum` falls back on 85% (`args`, 14,196 of 16,707); in sqlglot
   `tuple.__contains__` falls back on **86%** of 1.88M calls (1,245,390 `args`, **375,948
   `deopt`**) and `builtins.sum` on 89%. Restarts cost time; these are the
   inputs to look at.
5. No friction with the switches themselves: `RUSTIQUE=1` reached pytest through the `.pth`
   in every project, `--rustique` printed the report, `diff` needed no `--ignore`.

## 8. Tests

`tests/test_dx.py` (switch logic against a fake framework: kill switch values, narrowing,
unknown names, stats/check/report variables; report rendering and merging; real framework
end to end; the startup hook through `site.addsitedir` in subprocesses: unset → nothing
imported, off values, on values, selection, invalid value, `-E`/`-I`, broken install,
idempotence; report at exit on stderr / JSON / killed) and `tests/test_dx_cli.py` (runner,
status, report, diff incl. pytest outcome changes and normalisation, the plugin incl.
refusal categories, check mode, mismatches, xdist): 100 tests, ~7 s. Full suite on 3.15.1:
4006 passed, 9 skipped.

## 9. Open issues and decisions for the lead

⟨see the final report⟩
