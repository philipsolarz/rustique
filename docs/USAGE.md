# Using Rustique

Rustique is CPython with a Rust engine. It puts Rust behind CPython's existing entry points
(builtins, the standard library, and in opt-in tiers the interpreter and the garbage
collector). Your objects and functions stay CPython's own. A Rust kernel runs only when its
result is provably the one CPython computes; otherwise CPython's original code runs. There is
no new API to learn, and nothing to change in your code.

This page covers:

- installing it, and the platforms and Python versions it covers;
- turning it on and off, and the opt-in tiers;
- predicting what it will do for your program;
- seeing what it did, and checking in CI that nothing changed except speed.

How it works: [`ACCELERATION.md`](ACCELERATION.md). Measurements and design decisions of this
layer: [`targets/dx.md`](targets/dx.md). Upgrading from 0.2: [`MIGRATION.md`](MIGRATION.md).

- [Quick start](#quick-start)
- [Install](#install) · [Platforms](#platforms) · [Python versions and patch releases](#python-versions-and-patch-releases)
- [Turn it on](#turn-it-on) · [Turn it off](#turn-it-off-or-turn-parts-of-it-off) · [Opt-in tiers](#opt-in-tiers)
- [Will it help my program?](#will-it-help-my-program)
- [See what it does](#see-what-it-does)
- [Check in CI that nothing changed](#check-in-ci-that-nothing-changed) · [CI recipes](#ci-recipes)
- [Reference](#reference-environment-variables): environment variables, Python API, `python -m rustique`
- [FAQ](#faq) · [Troubleshooting](#troubleshooting)

## Quick start

```bash
pip install rustique
python -m rustique estimate -- python app.py   # what would it do for this program?
RUSTIQUE=1 python app.py                       # run it accelerated; RUSTIQUE=0 turns it off
python -m rustique diff -- python -m pytest -q # prove in CI that nothing changed except speed
```

## Install

```bash
pip install rustique          # or: uv pip install rustique / uv add rustique
```

Prebuilt wheels cover CPython 3.14 and 3.15, both the default and the free-threaded build
(`3.14t`, `3.15t`), on the [platforms below](#platforms). No Rust toolchain is needed.

Installing costs almost nothing while Rustique is off. The wheel adds one line to
site-packages (`_rustique_startup.pth`), which Python reads at every start. With `RUSTIQUE`
unset or `0`, that line imports nothing, and it costs about 0.3 ms per process in a bare
venv. Where another `.pth` file is already present, it costs about 0.04 ms
([`targets/dx.md` §1](targets/dx.md#1-zero-code-switch)).

**On a platform without a wheel, pip builds Rustique from the source distribution.** That
download is about 1.4 GB: maturin fetches a Rust toolchain on its own. On a platform Rustique
has not validated (say Linux on s390x, or FreeBSD), every target then refuses with category
`platform` and CPython's code runs. The result is safe, but the build is slow and buys you
nothing. In CI and Docker images, ask for wheels only:

```bash
pip install --only-binary rustique rustique      # fails fast where no wheel exists
uv pip install --only-binary rustique rustique   # uv: also `no-build-package = ["rustique"]` in uv.toml
```

## Platforms

Every wheel installs and runs correctly on every listed platform, but not every target is
validated on every platform. A target that is not refuses with category `platform`, and
CPython's own code runs.

| platform | wheels | targets on (3.15) | what refuses there |
|---|---|---|---|
| Linux x86_64 and aarch64, glibc (manylinux_2_28) | cp314, cp314t, cp315, cp315t | all (3.14: all but 5 `pathlib` properties, which 3.14's specializer inlines) | — |
| Linux x86_64 and aarch64, musl (musllinux_1_2) | same | all but the `datetime`/`time` text targets | `strptime`, `strftime`, `isoformat` (validated with glibc's locale and `%`-directive behaviour only) |
| macOS arm64 and x86_64 | same | as musl | as musl |
| Windows x64 and arm64 | same | containers, builtins, `float`, `json` (not on 3.15t yet), `statistics`, `urllib.parse`, `dataclasses` | POSIX path semantics (`posixpath`, `fnmatch`, `pathlib`); `datetime` text; the groups that look up a CPython function at run time (`copy`, `logging`, `random`, `textwrap`), until that lookup is validated on Windows |

The exact numbers for a release are in its CI run: the `platform-coverage` artifact of the
`Wheels` workflow tabulates every platform × build. It counts a target as "on" only after
Rustique's own tests and CPython's test modules for that target have passed on that
platform. On your machine, `python -m rustique status` lists every refusal with its reason.

The opt-in `interp` tier ([below](#opt-in-tiers)) exists only in the Linux x86_64 wheels.
There it runs on python-build-standalone builds (`uv python install`, PBS-based images). On
every other build and platform it refuses, with a reason that says why.

## Python versions and patch releases

| interpreter | what happens |
|---|---|
| CPython 3.14.x, 3.15.x (default and free-threaded) | wheels; every target installs on the patch releases this Rustique release was validated on: 3.14.1, 3.14.5, 3.14.6, 3.14.8, 3.14.9, 3.14.10, 3.15.0, 3.15.1, 3.15.2 (free-threaded: 3.14.9t, 3.14.10t, 3.15.1t, 3.15.2t) |
| a patch release this Rustique release did not validate: a newer one (e.g. 3.15.4), or 3.14.0, 3.14.2–3.14.4, 3.14.7 | installs; every target refuses with category `unvalidated-patch`. Upgrade Rustique, or set `RUSTIQUE_UNVALIDATED_PATCH=1` to install the targets whose code is byte-for-byte the code of a validated release (they are reported as `unvalidated`) |
| CPython 3.16 and later, pre-releases, PyPy, GraalPy, CPython ≤ 3.13 | pip refuses to install it: `requires-python = ">=3.14,<3.16"` |

Why refuse unknown patch releases? A patch release can change C behaviour that a fingerprint
of the Python-level code cannot see (3.15.2 changed `tuple` hashing's recursion guard). So
Rustique installs a target only on releases its CI ran CPython's test suite on
([DECISIONS §10.5](DECISIONS.md)).

**Upgrading Python to 3.16 later.** Until a Rustique release ships cp316 wheels, pip refuses
to install Rustique on 3.16, and a `requirements.txt` that lists it fails to resolve. Keep
the dependency conditional, and never import `rustique` unconditionally in application code:

```text
rustique; python_version < "3.16"        # requirements.txt / pyproject dependencies
```

Turn Rustique on with `RUSTIQUE=1` or `python -m rustique`, never `import rustique` at the top
of your code. Then the same code runs on an interpreter without Rustique: `RUSTIQUE=1` does
nothing where Rustique is not installed. If you call `rustique.enable()` from code, guard
the import (`try: import rustique ... except ImportError: pass`).

**uv and the `<3.16` cap.** uv treats a CPython 3.16 *pre-release* as satisfying `<3.16`. It
picks the source distribution there and builds it, and every target then refuses as
`version` at run time. Pass `--only-binary rustique` (or set `no-build-package` in uv.toml) to
get a clear "no matching distribution" instead.

## Turn it on

| how | when to use it |
|---|---|
| `RUSTIQUE=1 python app.py` (any command: `RUSTIQUE=1 pytest`, `RUSTIQUE=1 gunicorn app:app`, `RUSTIQUE=1 celery worker`) | no code change. Also reaches every Python subprocess that inherits the environment (multiprocessing spawn/forkserver, pytest-xdist, `subprocess.run([sys.executable, ...])`) |
| `python -m rustique app.py` / `python -m rustique -m pkg.cli` / `python -m rustique -c "…"` | one command, no environment variable. It behaves like `python app.py`: same `__file__`, same tracebacks, same exit status. It exports `RUSTIQUE=1` so child interpreters are accelerated too |
| `import rustique; rustique.enable()` | from code, e.g. at the top of your entry point |
| `pytest --rustique` | a test session, with a per-target report at the end ([CI](#check-in-ci-that-nothing-changed)) |

All of these do the same thing: they install the targets that are validated on this
interpreter, process-wide. A target whose module is not imported yet (say `json`) is
installed when that module is first imported; Rustique never imports a module itself.
Turning it on twice is harmless. Turn it on as early as possible. The order does not matter
for correctness, but more calls are accelerated.

Startup: `RUSTIQUE=1` loads one extension module (2.3 MB) and installs the targets of the
modules already imported. Against the same venv with `RUSTIQUE` unset, it costs 2.8% more
instructions for `python -c pass` on 3.15.1 and 2.9% on 3.14.9
([`targets/engine.md`](targets/engine.md)).

## Turn it off, or turn parts of it off

- **`RUSTIQUE=0` is the kill switch.** `off`, `false`, `no` and an empty value work too, but
  `0` is the one to use. With `0` (or `RUSTIQUE` unset) the startup line stops at its first
  test and costs nothing more. The other spellings, and `-E`/`-I`, briefly load the
  extension and unregister it again (about 0.5M instructions per process); nothing stays
  loaded and nothing is enabled. Every way of turning Rustique on becomes a no-op, opt-in
  tiers included: the startup switch,
  `python -m rustique`, `pytest --rustique`, and a `rustique.enable()` written in the
  program. The process runs as plain CPython. If you suspect Rustique, set `RUSTIQUE=0` and
  rerun. No deploy is needed.
- **`rustique.disable()`** restores every original entry point in a running process.
- **Parts:** `RUSTIQUE_EXCLUDE=json` leaves one group out, and `RUSTIQUE_ONLY=json,copy` keeps
  only those. Both take groups (`json`, `copy`, `datetime`, `paths`, `logging`, `builtins`,
  `containers`, …) or target names (`json.dumps`, `list.sort`). From code:
  `rustique.enable(only=[...], exclude=[...])`. On the command line: `--only`/`--exclude`.

**The environment can always turn things off.**

- `RUSTIQUE_ONLY` narrows the code's `only=` (the intersection).
- `RUSTIQUE_EXCLUDE` is added to its `exclude=`.
- `RUSTIQUE_TIERS=none` turns off every tier, even one the code asked for.
- `RUSTIQUE=0` overrides everything.

So an operator can always switch off a group, a tier or all of Rustique without touching the
code. A misspelled name in an environment variable prints a one-line warning on stderr
(`rustique: RUSTIQUE_EXCLUDE: no target or group named 'jsn'`) and installs nothing extra. A
misspelled name in code raises `ValueError`. Like the `PYTHON*` variables, `RUSTIQUE*`
variables are ignored under `-E` and `-I`.

## Opt-in tiers

Two tiers go further than replacing standard-library and builtin entry points. They are off
unless you ask for them, because each has a limit that the default path does not have:

| tier | what it does | where it runs | the catch |
|---|---|---|---|
| `interp` | replaces chosen opcode handlers of CPython's interpreter (generic attribute lookups, `isinstance`, `object.__setattr__`, …) with handlers generated from the running patch release's own `bytecodes.c`, each verified before the swap | Linux x86_64 wheels, on python-build-standalone 3.15 builds (tail-calling interpreter). Elsewhere it refuses: "built without handlers", or the build is not a verified one | one generated handler set per patch release; framework and custom code benefit most |
| `gc` | an adaptive garbage-collection policy: defers full collections while they find nothing, never touches an explicit `gc.collect()`, steps aside when you tune `gc` yourself | every platform | two documented deviations: `gc.get_threshold()` reports the thresholds you set, not the ones in force, and `gc.callbacks` holds one extra entry. Collection timing differs (it already does between CPython releases) |

Turn them on:

```bash
RUSTIQUE=1 RUSTIQUE_TIERS=interp,gc python app.py
python -m rustique --tiers gc app.py
pytest --rustique --rustique-tiers interp
```

From code: `rustique.enable(tiers=["gc"])`. `RUSTIQUE_TIERS` is added to what the code asks
for. `RUSTIQUE_TIERS=none` (or `0`) turns off every tier, even one requested in code, and
`RUSTIQUE_EXCLUDE=interp` removes one tier. `python -m rustique status` shows each tier as
`off` (available), `on`, or refused with its reason.

## Will it help my program?

```bash
python -m rustique estimate -- python app.py args...
python -m rustique estimate -- python -m pytest -q tests/
```

`estimate` runs your command three times and predicts the run time with Rustique from the
program itself:

1. once with Rustique on and counters on, to see which accelerated entry points your program
   reaches, and how often each takes the fast path;
2. once plain, sampling the stack on a CPU-time timer, to measure how much of the run is
   spent under each entry point;
3. once plain, with a profiler on the entry points. It keeps a sample of the real arguments
   and, at exit, replays them with CPython's original and with the Rust kernel.

```
rustique estimate: python app.py
  predicted: 0.612 x the plain run time (1.63x faster); range 0.60-0.62 by the other time source
  44.9% of the run is under accelerated entry points; the other 55.1% is unchanged by Rustique.

  target                                 calls   share  ratio   saves  from
  datetime.datetime.strptime            11,977   30.1%   0.07   28.0%  replayed 47 calls; time from stack samples
  json.dumps                            32,690    9.2%   0.38    5.7%  replayed 32 calls; time from stack samples
  ...
  note: json.loads fell back on 41% of 9,600 calls (mostly args: inputs outside the kernel's exact subset ...)
```

How to read it:

- **predicted**: the ratio, time with Rustique divided by time without. The range is the
  same prediction from the other time source (profiler per-call times instead of samples).
  When the two disagree, trust the measurement (`diff --repeat 5`, below).
- **share**: the part of the run spent under that entry point.
- **ratio**: the kernel's time over CPython's on *your* calls, fallbacks included.
- **saves**: that target's saving, as a part of the whole run.
- **"the other N% is unchanged"**: the honest ceiling. Time in your own code, in third-party
  code and in the interpreter is out of reach of the default targets. A verdict of "no
  noticeable change" for a framework-heavy program is the expected answer, not a failure.

Validated on the 65 benchmark workloads against measured ratios
([`targets/dx.md` §5](targets/dx.md#5-estimate)). It is least accurate for:

- multi-threaded programs, which are timed per call in thread CPU time;
- programs dominated by one huge call, where the replay runs hot and over-promises;
- `sort`/`sorted` on presorted input, where C-level calls carry no arguments.

The opt-in tiers are not predicted: measure them with `diff --tiers`. On free-threaded
builds, threads other than the main one are not sampled; the note says how much time that
leaves out.

Cost: three runs of your command, the profiled one 1–5× slower than plain. `--json` prints
the prediction for scripts.

## See what it does

```bash
python -m rustique status                  # what is accelerated on this interpreter, and why not
python -m rustique report -- python app.py # run with counters on; report covers every Python process
RUSTIQUE=1 RUSTIQUE_REPORT=1 python app.py # report at exit, on stderr
RUSTIQUE=1 RUSTIQUE_REPORT=/var/tmp/rq.json gunicorn app:app   # every worker appends its record
python -m rustique report --from /var/tmp/rq.json              # ... merged later
```

A report (format; the numbers depend on your program):

```
rustique 0.3.0 on CPython 3.15.1 (GIL), 5 processes (4 forked): enabled by RUSTIQUE=1
  84 targets: 9 called, 17 installed but not called, 58 waiting for their module
  opt-in tiers: gc off (0 of 1 targets active), interp off (0 of 12 targets active); RUSTIQUE_TIERS=gc,interp turns them on

  target                     group            hits   fast  fallbacks (reason n)
  json.dumps                 json           18,204    98%  311 (args 290, deps 21)
  copy.deepcopy              copy            2,048    51%  1,957 (abandon 1,957)  [suspended 1x]
  list.sort                  containers         57   100%  0
  (17 installed targets were not called)

  what the fallbacks mean here:
    abandon (copy.deepcopy 1,957): the kernel restarted in CPython's code after partial work, which was wasted. ...
    args (json.dumps 290): inputs outside the kernel's exact subset (a subclass, an unsupported keyword ...) ...
    deps (json.dumps 21): something the target depends on was replaced (mock.patch, a monkeypatch, gevent) ...
    suspended (copy.deepcopy): persistent fallback: too few calls took the fast path, so CPython's original ...

  58 targets wait for their module (datetime, logging, pathlib, ...): installed when it is imported
```

- **hits**: calls that ran the Rust kernel. **fast**: hits as a share of the calls.
- **fallbacks**: calls that ran CPython's original, by reason. Each reason that occurred is
  explained under "what the fallbacks mean here", with the targets it hit most:

  | reason | meaning | what you can do |
  |---|---|---|
  | `args` | inputs outside the kernel's exact subset (a subclass, an unsupported keyword or type, a huge input) | nothing needed; `docs/targets/<group>.md` lists each subset |
  | `deps` | a dependency of the target was replaced (`mock.patch`, monkeypatching, gevent) | expected in tests; the target resumes when the dependency is restored |
  | `tools` | a tracer, profiler, debugger or coverage is listening; Python-level targets step aside so tools see CPython's frames | run timing and Rustique CI jobs without coverage |
  | `interp` | called in a subinterpreter | — |
  | `deopt` | the kernel met something it cannot decide and restarted in CPython's code before doing anything observable | — |
  | `continue` | the kernel handed over midway; CPython's algorithm finished from the same state, with no work repeated | — |
  | `abandon` | the kernel restarted after partial work, which was wasted | persistent waste suspends the target; `RUSTIQUE_EXCLUDE=<target>` if you want to skip the attempts |
  | `error` | an error path: CPython's code re-ran the call to raise its own exception | — |
- **suspended Nx**: the target fell back so persistently that Rustique restored CPython's
  original. For `fallback` suspensions that lasts until the next `enable()`; for `deps`
  suspensions, until the dependency is back. Remaining calls cost what CPython's do.
- **armed**: the target's entry point is swapped and it validates at its first call (then
  installed, or refused). A target that is never called costs no validation. `status`
  validates every armed target before printing, so it shows the final verdicts;
  `RUSTIQUE_VALIDATE=eager` does the same for a whole process.
- **refused**: grouped by category, each with what to do:

  | category | meaning | what to do |
  |---|---|---|
  | `version` | this CPython version is not one the target is validated for | nothing |
  | `unvalidated-patch` | a patch release newer than this Rustique release | upgrade Rustique, or `RUSTIQUE_UNVALIDATED_PATCH=1` ([above](#python-versions-and-patch-releases)) |
  | `platform` | not validated on this OS, C library or architecture | nothing ([Platforms](#platforms)) |
  | `build` | a debug build or an unvalidated configuration | — |
  | `not-needed` | this build does not need the target (the CPython behaviour it fixes is absent here) | nothing |
  | `jit` | the JIT is on and the target is not validated with it | `PYTHON_JIT=0` to accelerate it |
  | `fingerprint` | the standard-library code differs from the validated release (a patched or vendored CPython) | report it if this is a stock build |
  | `conflict` | another tool replaced the entry point first (gevent, an APM agent) | — |
  | `layout`, `symbol`, `selftest` | this CPython's internals differ from the validated ones | report it, with `python -m rustique status --json` |
  | `deps`, `other` | see the reason | — |

**Every process counts once.** Reports add up all the Python processes of a command: forked
workers (multiprocessing `fork`, gunicorn, celery prefork), spawned ones and xdist workers.

- A forked child starts with zeroed counters, so the parent's calls are not counted twice.
- On Linux and macOS, while counters run into a JSON report, each process keeps its counters
  in a small live file next to the report (`<report>.json.<pid>.cnt`). So a worker that ends
  in `os._exit` or is killed is still counted. On Windows, only processes that exit normally
  report.
- `report`, `diff` and `report --from` merge everything. The header says how many processes
  were forked, and how many were counted from their live files.

Counters are off by default, because each kernel call would pay a few instructions. They are
on with `RUSTIQUE_STATS=1`, `RUSTIQUE_REPORT`, `python -m rustique report`,
`pytest --rustique`, or `rustique.stats(True)`. From code, `rustique.status()` and
`rustique.stats()` return the same data as dicts ([API](#reference-python-api)).
`--verbose` (`report`, `status`) lists every target name.

## Check in CI that nothing changed

Two tools and a check mode. Use any of them, or all of them.

**1. `python -m rustique diff -- <command>`** runs your command twice, plain (`RUSTIQUE=0`)
and with `RUSTIQUE=1`, and compares:

- the exit codes;
- stdout and stderr;
- for pytest commands, every test's outcome.

Both runs use the same `PYTHONHASHSEED` (0 unless you set one). For pytest it adds a JUnit
file, `-p no:cacheprovider` (so the first run cannot steer the second through
`.pytest_cache`), and the plugin's per-test record (`--rustique-outcomes`, which imports
nothing else).

`diff` does not cry wolf. It reports a difference only when Rustique made it:

- **Masked before comparing**: output that differs between any two runs of a program —
  memory addresses, ISO-8601 and log-format timestamps, ephemeral ports of local servers,
  pytest durations, `tmp_path` directories, `pytest-randomly` seeds, and pytest's progress
  characters (threads interleave with them; the per-test outcomes carry the same
  information). Mask more with `--ignore REGEX`, or compare byte for byte with
  `--no-normalize`.
- **Volatile output**: when only stdout/stderr differ, `diff` runs the plain side once
  more. A stream that also differs plain-to-plain is reported as *volatile*, not as a
  difference. `--strict-output` compares it anyway.
- **Flaky tests**: when test outcomes differ, `diff` re-runs just those tests, alone, twice
  on each side (`--confirm N`; `0` turns it off). A test whose outcome also varies without
  Rustique is flaky: it is listed as such and not counted. A test that keeps its changed
  outcome is a confirmed difference.
- Test ids built from `now()` or random values pair up when only their digits differ.

```
$ python -m rustique diff -- python -m pytest -q tests
rustique diff: python -m pytest -q tests
  identical: exit code 0, stdout 12 lines, stderr 0 lines, 1532 test outcomes
  time: plain 41.902 s, rustique 38.117 s, ratio 0.910

rustique 0.3.0 on CPython 3.15.1 (GIL), 3 processes: enabled by RUSTIQUE=1
  ...per-target report of the enabled run...
```

Exit status: `0` identical; `1` something differs (a unified diff of the first lines, or the
tests whose outcome changed); `2` usage error, or Rustique was not active in the command.
"Identical" proves nothing if the command's interpreter has no Rustique, so diff checks that
some process reported.

Also: `--repeat N` (N interleaved plain/enabled pairs, for steadier timing), `--keep DIR`
(every run's output), `--tiers T` (opt-in tiers in the enabled run), `--check` (check mode,
below).

**2. The pytest plugin**. It is installed with the wheel and does nothing unless you pass
`--rustique`:

```bash
pytest --rustique                                   # enable for the session, report in the summary
pytest --rustique --rustique-exclude re             # leave a group out
pytest --rustique --rustique-tiers gc               # an opt-in tier too
pytest --rustique --rustique-allow-refused datetime # accept that datetime refuses here
```

The plugin turns Rustique and its counters on before collection, and prints the per-target
report in the terminal summary. It **fails the session** (exit 1) if a selected target was
refused for an unexpected reason: a fingerprint mismatch, a site patched by someone else, or
a failed self-test. Refusals of category `version`, `unvalidated-patch`, `build`, `jit`,
`platform` and `not-needed` are expected on a test matrix and never fail the session. Under `pytest-xdist`
every worker is accelerated and the summary adds up their counters. If plugin autoloading is
off (`PYTEST_DISABLE_PLUGIN_AUTOLOAD`), use `-p rustique.pytest_plugin`.

**3. Check mode** is for the paranoid CI job. With `RUSTIQUE_CHECK=1`, every fast path of a
*checkable* target also runs CPython's original on the same arguments and compares the two
results. A target is checkable when its exact subset calls no user code and has no side
effects (`json.dumps` of plain data, float formatting, `strptime`, `os.path.join`, …). A
mismatch is a Rustique bug: the program still gets CPython's result, the mismatch is
counted, and it fails the run. Expect roughly 2× the cost of each checked call.

```bash
python -m rustique diff --check -- pytest -q
pytest --rustique --rustique-check                  # mismatches fail the session
python -m rustique --check app.py                   # --check-raise: raise RustiqueCheckError
```

`--rustique` and `diff` answer different questions. `--rustique` asks whether your tests pass
accelerated, and what ran. `diff` asks whether everything observable is the same as without
Rustique. Don't combine them: `diff` turns Rustique on itself, and refuses a command
containing `--rustique`.

### CI recipes

GitHub Actions. The Rustique job runs next to your normal test job, without coverage:
coverage makes every Python-level target step aside, so the job would prove little.

```yaml
rustique:
  runs-on: ubuntu-24.04
  strategy:
    matrix:
      python: ["3.14", "3.15", "3.15t"]
  steps:
    - uses: actions/checkout@v6
    - uses: actions/setup-python@v6
      with: { python-version: "${{ matrix.python }}" }
    - run: pip install --only-binary rustique rustique -r requirements-test.txt
    - name: Nothing changed except speed
      run: python -m rustique diff --check -- python -m pytest -q -p no:randomly
    - name: Will it help? (informational)
      run: python -m rustique estimate -- python -m pytest -q -x tests/perf_smoke
```

- Matrix over the interpreters you deploy on. A refusal on one of them (a platform, a
  version) is shown in the report and is not a failure.
- `diff` costs two runs of the suite (more only when something differs). For speed, run it
  on one interpreter and `pytest --rustique` on the others.
- tox/nox: `commands = python -m rustique diff -- python -m pytest {posargs}`.
- Docker: `RUSTIQUE=1` in the image's environment turns it on for every process.
  `RUSTIQUE=0` at `docker run` turns it off again.

## Reference: environment variables

| variable | values | effect |
|---|---|---|
| `RUSTIQUE` | `1`/`true`/`yes`/`on` | enable at interpreter startup (through `_rustique_startup.pth`) |
| | `0` (also `false`/`no`/`off`/empty) | kill switch: every enable path is a no-op, tiers included, and nothing stays loaded. `0` costs nothing at startup; the other spellings briefly load the extension (~0.5M instructions) |
| | unset | nothing at startup; explicit `enable()` works |
| `RUSTIQUE_ONLY` | `json,copy` (groups or targets) | narrow every enable path to these |
| `RUSTIQUE_EXCLUDE` | `re,json.loads` | leave these out of every enable path (also tier names) |
| `RUSTIQUE_TIERS` | `interp,gc` | turn these opt-in tiers on too (added to `enable(tiers=)`) |
| | `none`/`0`/empty | every tier off, even ones the code asked for |
| `RUSTIQUE_UNVALIDATED_PATCH` | `1` | on a patch release this Rustique release did not validate, install the targets whose code matches a validated release (reported `unvalidated`) |
| `RUSTIQUE_VALIDATE` | `first-call` (default) / `eager` | when a target is validated: at its first call (a program pays only for what it calls; targets show as `armed` until then), or at enable/import (every verdict at once: CI, `python -m rustique status`). Free-threaded builds always validate eagerly |
| `RUSTIQUE_STATS` | `1` | counters on when Rustique is enabled |
| `RUSTIQUE_REPORT` | `1` / `stderr` | report at exit on stderr (implies `RUSTIQUE_STATS=1`) |
| | a path (`{pid}` is replaced) | append the text report to the file; with `.json`, one JSON record per process, plus live counter files `<path>.<pid>.cnt` (what `report`/`diff`/`report --from` read) |
| `RUSTIQUE_CHECK` | `1` / `warn` | check mode: a mismatch is counted (`CHECK MISMATCHES` in the report) and CPython's result is returned |
| | `raise` | check mode where a mismatch raises `RustiqueCheckError` |

All are read when Rustique is enabled, except `RUSTIQUE=1`, which is read at startup. All
are ignored under `-E`/`-I`.

## Reference: Python API

```python
rustique.enable(*, only=None, exclude=None, tiers=None, builtins=True) -> dict
rustique.disable() -> dict
rustique.status() -> dict
rustique.stats(enabled=None, reset=False) -> dict
rustique.check(mode=...) -> str | None        # "warn" | "raise" | "off"; no argument: current mode
rustique.RustiqueCheckError                    # raised by check mode "raise" on a mismatch
```

- `enable`: installs the selected targets. It is idempotent and additive.
  - `only`/`exclude`: lists, or comma-separated strings, of group or target names.
  - `tiers`: opt-in tiers to turn on (`["interp", "gc"]`).
  - `builtins=False`: same as `exclude=["builtins"]`.

  Returns `status()`. It is a no-op under `RUSTIQUE=0`.
- `disable`: restores every original entry point.
- `status`: returns `{"targets": [...], "process": {...}}`.
  - Each target is `{name, group, state, reason, refusal, sites, versions, tier, validation, ...}`.
    `state` is `installed`, `armed`, `pending-import`, `refused` or `disabled`. `refusal` is
    the category of a refusal; `suspended` (`fallback`, `deps`, `conflict`) is present while
    a target is suspended.
  - `process` holds the CPython version and build, the Rustique version, `ppid` (the parent's
    pid for a forked child), and `switch`: how Rustique was turned on, whether the kill switch
    is set, the tiers in force, and which `RUSTIQUE*` variables are set.
- `stats`: returns `{name: {"hits": n, "fallback": {reason: n}, "suspensions": n}}`.
  `stats(True)` turns counting on, `stats(reset=True)` zeroes the counters after the snapshot.
- `check`: check mode from code, same as `RUSTIQUE_CHECK`. It returns the previous mode.

The opt-in native types of Rustique 0.2 (`rustique.List`, `Dict`, `X`, the scalars,
`native=`) were removed in 0.3. See [`MIGRATION.md`](MIGRATION.md).

## Reference: `python -m rustique`

```
python -m rustique [run] [--only G,..] [--exclude G,..] [--tiers T,..] [--report] [--check | --check-raise]
                         [--no-builtins] (script.py | -m module | -c command) [args...]
python -m rustique status [--json] [--verbose]
python -m rustique report [--json] [-o FILE] [--limit N] [--verbose] [--check] [--tiers T,..] -- <command...>
python -m rustique report --from FILE.json [--json] [--verbose]
python -m rustique diff [--repeat N] [--confirm N] [--ignore REGEX]... [--no-normalize] [--strict-output]
                        [--keep DIR] [--timeout S] [--quiet] [--check] [--tiers T,..] -- <command...>
python -m rustique estimate [--json] [--limit N] [--mode samples|hybrid|profile] [--tiers T,..] -- <command...>
```

`run` is the default subcommand. To run a script that is literally named `status`, write
`python -m rustique run status`. In `report`, `diff` and `estimate`, a command starting with
`something.py` runs with the current interpreter.

## FAQ

**Will my program behave differently?** It shouldn't. Every target is held to the contract
in [`ACCELERATION.md`](ACCELERATION.md):

- same results, types and identities;
- same exceptions and messages;
- same warnings, side effects and their order;
- same introspection.

Each target is checked against CPython's own test suite and by differential fuzzing, on every
platform it is on. Fourteen popular projects' own test suites (requests, flask, attrs, rich,
httpx, jinja, click, marshmallow, sqlglot, black, pydantic, Django, arrow, dateutil) show no
difference ([`targets/dx.md` §7](targets/dx.md)). If you find one, `RUSTIQUE=0` makes it go away
while you report it, and `python -m rustique diff` is the evidence.

**Which of my code gets faster?** Code that spends its time in the accelerated standard
library and builtins: JSON, `copy.deepcopy`, path manipulation, date parsing and formatting,
logging, … With the opt-in tiers, also the interpreter's generic paths and the garbage
collector. `python -m rustique estimate -- <your command>` answers it for your program.

**Free-threaded Python, subinterpreters, debuggers, coverage, profilers?**

- Free-threaded 3.14t/3.15t: yes, with wheels for both.
- Subinterpreters: the extension does not load in them, so code there runs plain CPython.
- Debuggers, coverage, profilers and `sys.settrace`: Python-level targets fall back while a
  tool is listening, so tools see CPython's own frames. The report counts these calls under
  `tools`.

**Does `RUSTIQUE=1` reach subprocesses?** Yes, if they inherit the environment and use an
interpreter with Rustique installed. That includes `multiprocessing` with any start method,
pytest-xdist workers, gunicorn/uvicorn/celery workers, and
`subprocess.run([sys.executable, ...])`. A forked child inherits the parent's installed state
and starts with its own counters.

**Is `rustique.enable()` in a library a good idea?** No. It is process-wide, so leave the
decision to the application. Libraries can tell users about `RUSTIQUE=1`.

**`python -S`, embedded interpreters, zipapps?**

- `-S` skips `site`, so the `.pth` switch never runs: use `python -m rustique` or
  `rustique.enable()`.
- Hosts that initialise Python normally (mod_wsgi, uWSGI, Blender) honour `RUSTIQUE=1` in
  their environment. Check from inside with `python -m rustique status`, or
  `rustique.status()`.
- Extension modules cannot load from a zip archive, so a `python -m zipapp` or an unextracted
  pex cannot carry Rustique. shiv extracts its archive and works.

**Rustique's own containers (`rustique.List`, …)?** Removed in 0.3. With Rustique on, the
builtin `list`/`dict`/`set`/`tuple` get the same kernels in place. See
[`MIGRATION.md`](MIGRATION.md).

## Troubleshooting

- **`RUSTIQUE=1` does nothing.** Run `python -m rustique status` with the same interpreter.
  - "no startup file": an editable/development install (`maturin develop`), or `pip install
    --target`. Those do not install `_rustique_startup.pth`. Use `python -m rustique app.py`
    or `rustique.enable()`.
  - "python -S": `site` did not run.
  - "(no RUSTIQUE* variables set)": the variable did not reach the process, or Python runs
    with `-E`/`-I`.
- **Everything is refused.** The status header says why: an unvalidated patch release (upgrade
  Rustique, or `RUSTIQUE_UNVALIDATED_PATCH=1`), a pre-release, a debug build, or a platform
  Rustique has not validated.
- **`diff` says the command wrote no report.** The command ran an interpreter without
  Rustique (a different venv, a system `pytest`), or no Python at all. Use `python -m pytest`
  with the right interpreter.
- **`diff` reports a difference.**
  1. Look at what kind it is. A test difference was confirmed by re-runs; an output
     difference did not appear between two plain runs.
  2. Narrow it down with `RUSTIQUE_ONLY=<group>`, one group at a time, or `RUSTIQUE_EXCLUDE`.
  3. Report it with the diff output and `python -m rustique status --json`.
- **A report shows many fallbacks or a suspension.** That is a cost, not a correctness issue:
  CPython's code ran. The "what the fallbacks mean here" lines say whether you can do
  anything (coverage on, mocks); otherwise tell us the target and reason.
- **pytest: "option names {'--rustique'} already added".** The plugin was loaded twice under
  different names. Use `-p rustique.pytest_plugin` (the entry point's name), or rely on
  autoloading.
- **Rustique in `pip list`, but `import rustique` fails.** The wheel does not match the
  interpreter (`pip debug --verbose` lists the supported tags), or the venv is broken. The
  startup switch never breaks Python startup: if the extension cannot be loaded, `site`
  prints the failing `_rustique_startup.pth` line with the error, and Python starts plain.
- **`pip install` is downloading a Rust toolchain.** There is no wheel for this platform or
  interpreter ([Install](#install)). Use `--only-binary rustique`.
