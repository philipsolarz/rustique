# Using Rustique

Rustique puts Rust behind CPython's existing entry points (builtins, stdlib functions).
Your objects and functions stay CPython's own. A Rust kernel runs only when its result is
provably the one CPython computes; otherwise CPython's original code runs. This page covers
turning it on and off, seeing what it does, and checking in CI that nothing changed except
speed. How it works: [`ACCELERATION.md`](ACCELERATION.md). Measurements and design
decisions for this layer: [`targets/dx.md`](targets/dx.md).

- [Install](#install)
- [Turn it on](#turn-it-on)
- [Turn it off, or turn parts of it off](#turn-it-off-or-turn-parts-of-it-off)
- [See what it does](#see-what-it-does)
- [Check in CI that nothing changed](#check-in-ci-that-nothing-changed)
- [Reference: environment variables](#reference-environment-variables)
- [Reference: Python API](#reference-python-api)
- [Reference: `python -m rustique`](#reference-python--m-rustique)
- [FAQ](#faq)
- [Troubleshooting](#troubleshooting)

## Install

```bash
pip install rustique          # or: uv pip install rustique / uv add rustique
```

Prebuilt wheels cover CPython 3.14 and 3.15, both the default and the free-threaded build
(`3.14t`, `3.15t`), on:

- Linux x86_64 and aarch64, glibc (manylinux_2_28) and musl (musllinux_1_2);
- macOS arm64 and x86_64;
- Windows x64. Windows arm64 is experimental.

No Rust toolchain is needed. On other interpreters, such as PyPy, GraalPy or CPython 3.16
pre-releases, `pip` refuses to install Rustique with a "requires a different Python" message
(`requires-python = ">=3.14,<3.16"`). Rustique never guesses on an interpreter nobody has
validated it on.

Installing has a cost even when you never turn Rustique on. The wheel adds one line to
site-packages (`_rustique_startup.pth`), which Python reads at every start. It costs about
0.3 ms per process in a bare venv and about 0.04 ms where another `.pth` file is already
present ([`targets/dx.md` §1](targets/dx.md#1-zero-code-switch)). Rustique itself is not
imported unless `RUSTIQUE` is set.

## Turn it on

| how | when to use it |
|---|---|
| `RUSTIQUE=1 python app.py` (any command: `RUSTIQUE=1 pytest`, `RUSTIQUE=1 gunicorn app:app`) | no code change; also reaches every Python subprocess that inherits the environment |
| `python -m rustique app.py` / `python -m rustique -m pkg.cli` / `python -m rustique -c "…"` | one command, no environment variable |
| `import rustique; rustique.enable()` | from code, e.g. at the top of your entry point |
| `pytest --rustique` | a test session, with a per-target report at the end ([CI](#check-in-ci-that-nothing-changed)) |

All of these do the same thing: the targets that are validated on this interpreter are
installed process-wide. A target whose module is not imported yet (say `json`) is
installed when that module is first imported. Rustique never imports a module itself.
Turning it on twice is harmless.

Turn it on as early as possible. The order does not matter for correctness, because a
kernel works no matter when the code calling it was imported. Turning it on early just
means more calls are accelerated.

## Turn it off, or turn parts of it off

- **`RUSTIQUE=0` is the kill switch.** Every way of turning Rustique on becomes a no-op:
  the startup switch, `python -m rustique`, `pytest --rustique`, and also a
  `rustique.enable()` written in the program. `rustique.native` returns functions
  unchanged. The process runs as plain CPython, so if you suspect Rustique, set
  `RUSTIQUE=0` and rerun. No deploy is needed.
- **`rustique.disable()`** restores every original entry point in a running process.
- **Parts:** `RUSTIQUE_EXCLUDE=json` leaves one group out. `RUSTIQUE_ONLY=json,copy`
  keeps only those. Both take groups (`json`, `copy`, `datetime`, `paths`, `logging`,
  `builtins`, `containers`, …) or target names (`json.dumps`, `list.sort`). From code:
  `rustique.enable(only=[...], exclude=[...])`, or the CLI flags `--only`/`--exclude`.

**The environment can turn things off, never on.** `RUSTIQUE_ONLY` narrows the code's
`only=` (the intersection), `RUSTIQUE_EXCLUDE` is added to its `exclude=`, and
`RUSTIQUE=0` overrides everything. An operator can always switch off a group, or all of
Rustique, without touching the code. A misspelled name in an environment variable prints
a warning on stderr (`rustique: RUSTIQUE_EXCLUDE: no target or group named 'jsn'`) and
installs nothing extra. A misspelled name in code raises `ValueError`.

Like the `PYTHON*` variables, `RUSTIQUE*` variables are ignored when Python runs with `-E`
or `-I`.

## See what it does

```bash
python -m rustique status                  # what is accelerated on this interpreter, and why not
RUSTIQUE=1 RUSTIQUE_REPORT=1 python app.py # report at exit, on stderr
python -m rustique report -- python app.py # run with counters on, report covers every Python process
```

A report looks like this (format; the numbers depend on your program):

```
rustique 0.3.0 on CPython 3.15.1 (GIL), pid 4242: enabled by RUSTIQUE=1
  14 targets: 11 installed, 2 pending-import, 1 refused

  target         group     hits  fallbacks (reason n)
  json.dumps     json    18,204  311 (args 290, deps 21)
  copy.deepcopy  copy     2,048  0
  list.sort      containers  57  0

  pending import (patched when their module is imported): logging.Formatter.format, ...
  refused:
    re.match  CPython 3.14.9 is not validated (validated: 3.15)
```

- **hits**: calls that ran the Rust kernel.
- **fallbacks**: calls that ran CPython's original, by reason:
  - `args`: inputs outside the kernel's exact subset, such as a subclass, an unsupported
    keyword or a huge input;
  - `deps`: something the target depends on was monkeypatched, e.g. by `mock.patch`;
  - `tools`: a tracer, profiler, coverage or debugger is listening;
  - `interp`: a subinterpreter;
  - `deopt`: the kernel met something it could not decide and restarted in the original;
  - `error`: an error path re-run by the original so that CPython raises its own exception.
- **refused**: the target is not installed on this interpreter, for the reason given.
  CPython's code runs.

Counters are off by default, because each kernel call would pay a few instructions. They
are on with `RUSTIQUE_STATS=1`, `RUSTIQUE_REPORT`, `python -m rustique report`,
`pytest --rustique`, or `rustique.stats(True)`. From code, `rustique.status()` and
`rustique.stats()` return the same data as dicts ([API](#reference-python-api)).

## Check in CI that nothing changed

Two tools, plus a check mode. Use any of them, or all of them.

**1. `python -m rustique diff -- <command>`** runs your command twice: plain (`RUSTIQUE=0`)
and with `RUSTIQUE=1`. Then it compares:

- the exit codes;
- stdout and stderr;
- for pytest, every test's outcome.

For pytest it adds `--junitxml` and `-p no:cacheprovider`, so that the first run cannot
change the second through `.pytest_cache`. Both runs use the same `PYTHONHASHSEED` (0 unless
you set one). Output that differs between any two runs of a program is masked before
comparing:

- memory addresses (`0x7f…`);
- pytest durations and `tmp_path` directories;
- `pytest-randomly` seeds.

Mask more with `--ignore REGEX`, or compare byte for byte with `--no-normalize`.

```
$ python -m rustique diff -- pytest -q tests
rustique diff: pytest -q tests
  identical: exit code 0, stdout 12 lines, stderr 0 lines, 1532 test outcomes
  time: plain 41.902 s, rustique 38.117 s, ratio 0.910

rustique 0.3.0 on CPython 3.15.1 (GIL), 3 processes: enabled by RUSTIQUE=1
  ...per-target report of the enabled run...
```

Exit status:

- `0`: identical;
- `1`: something differs. A unified diff of the first lines, or the tests whose outcome
  changed, is printed;
- `2`: usage error, or Rustique was not active in the command. "Identical" proves nothing
  if the command's interpreter has no Rustique. diff detects this because no process
  reported.

`--repeat N` runs N plain/enabled pairs for steadier timing. `--keep DIR` saves every run's
output.

GitHub Actions:

```yaml
- run: pip install rustique
- run: python -m rustique diff -- python -m pytest -q
```

**2. The pytest plugin**. It is installed with the wheel and does nothing unless you pass
`--rustique`:

```bash
pytest --rustique                                   # enable for the session, report in the summary
pytest --rustique --rustique-exclude re             # leave a group out
pytest --rustique --rustique-allow-refused datetime # accept that datetime refuses here
```

The plugin turns Rustique and its counters on before collection, and prints the
per-target report in the terminal summary. It **fails the session** (exit 1) if a selected
target was refused for an unexpected reason: a fingerprint mismatch, a site patched by
someone else, or a failed self-test. Version refusals are expected and never fail the
session, because "this target is not validated on CPython 3.14" is normal on a matrix. Under
`pytest-xdist` every worker is accelerated and the summary adds up their counters. If
plugin autoloading is off (`PYTEST_DISABLE_PLUGIN_AUTOLOAD`), use `-p rustique.pytest_plugin`.

**3. Check mode** is for the paranoid CI job. With `RUSTIQUE_CHECK=1`, every fast path of
a *checkable* target also runs CPython's original on the same arguments and compares the
two results. A target is checkable when its exact subset calls no user code and has no
side effects: `json.dumps` of plain data, float formatting, `strptime`, `os.path.join`, …
A mismatch is a Rustique bug. The program still gets CPython's result, the mismatch is
counted, and it fails the run. Expect roughly 2× the cost of each checked call.

```bash
RUSTIQUE_CHECK=1 python -m rustique diff -- pytest -q   # or: python -m rustique diff --check -- ...
pytest --rustique --rustique-check                      # mismatches fail the session
python -m rustique --check app.py                       # --check-raise: raise RustiqueCheckError
```

`--rustique` and `diff` answer different questions. `--rustique` asks: do my tests pass
accelerated, and what ran? `diff` asks: is everything observable the same as without
Rustique? Don't combine them: `diff` turns Rustique on itself, and refuses a command
containing `--rustique`.

## Reference: environment variables

| variable | values | effect |
|---|---|---|
| `RUSTIQUE` | `1`/`true`/`yes`/`on` | enable at interpreter startup (through `_rustique_startup.pth`) |
| | `0`/`false`/`no`/`off` | kill switch: every enable path is a no-op |
| | unset or empty | nothing at startup; explicit `enable()` works |
| `RUSTIQUE_ONLY` | `json,copy` (groups or targets) | narrow every enable path to these |
| `RUSTIQUE_EXCLUDE` | `re,json.loads` | leave these out of every enable path |
| `RUSTIQUE_STATS` | `1` | counters on when Rustique is enabled |
| `RUSTIQUE_REPORT` | `1` / `stderr` | report at exit on stderr (implies `RUSTIQUE_STATS=1`) |
| | a path (`{pid}` is replaced) | append the text report to the file; with `.json`, one JSON record per process (what `report`/`diff` use) |
| `RUSTIQUE_CHECK` | `1` / `warn` | check mode: every fast path of a checkable target also runs CPython's original and compares. A mismatch is counted (`fallback["check"]`, shown as `CHECK MISMATCHES` in the report) and CPython's result is returned |
| | `raise` | check mode where a mismatch raises `RustiqueCheckError` |

All are read when Rustique is enabled, except `RUSTIQUE=1`, which is read at startup. All
are ignored under `-E`/`-I`.

## Reference: Python API

```python
rustique.enable(*, only=None, exclude=None, native=None, builtins=True) -> dict
rustique.disable() -> dict
rustique.status() -> dict
rustique.stats(enabled=None, reset=False) -> dict
rustique.check(mode=...) -> str | None        # "warn" | "raise" | "off"; no argument: current mode
rustique.RustiqueCheckError                    # raised by check mode "raise" on a mismatch
```

- `enable`: installs the selected targets. It is idempotent and additive.
  - `only`/`exclude`: lists, or comma-separated strings, of group or target names.
  - `builtins=False`: same as `exclude=["builtins"]`.
  - `native`: module patterns opted into Rustique's own containers (see below).

  Returns `status()`. It is a no-op under `RUSTIQUE=0`.
- `disable`: restores every original entry point.
- `status`: returns `{"targets": [...], "process": {...}}`.
  - Each target is `{name, group, state, reason, sites, versions}`. `state` is
    `installed`, `pending-import`, `refused` or `disabled`.
  - `process` holds the CPython version and build, the Rustique version, and `switch`:
    how Rustique was turned on (`RUSTIQUE=1`, `rustique.enable()`, `python -m rustique`,
    `pytest --rustique`), whether the kill switch is set, and which `RUSTIQUE*` variables
    are set.
- `check`: check mode from code, same as `RUSTIQUE_CHECK`. It returns the previous mode.
- `stats`: returns `{name: {"hits": n, "fallback": {reason: n}}}`. `stats(True)` turns
  counting on, `stats(reset=True)` zeroes the counters after the snapshot.

## Reference: `python -m rustique`

```
python -m rustique [run] [--only G,..] [--exclude G,..] [--report] [--check | --check-raise]
                         [--native PATTERN]... [--no-builtins] (script.py | -m module | -c command) [args...]
python -m rustique status [--json]
python -m rustique report [--json] [-o FILE] [--limit N] [--check] -- <command...>
python -m rustique diff [--repeat N] [--ignore REGEX]... [--no-normalize] [--keep DIR]
                        [--timeout S] [--quiet] [--check] -- <command...>
```

`run` is the default subcommand. To run a script that is literally named `status`, write
`python -m rustique run status`. In `report` and `diff`, a command starting with
`something.py` runs with the current interpreter.

## FAQ

**Will my program behave differently?** It shouldn't. Every target is held to the contract
in [`ACCELERATION.md`](ACCELERATION.md):

- same results, types and identities;
- same exceptions and messages;
- same warnings, side effects and their order;
- same introspection.

Each target is also checked against CPython's own test suite and by differential fuzzing.
If you find a difference, `RUSTIQUE=0` makes it go away while you report it, and
`python -m rustique diff` is the evidence.

**Which of my code gets faster?** The code that spends its time in the accelerated stdlib
and builtins: JSON, `copy.deepcopy`, path manipulation, date parsing and formatting,
logging, … `python -m rustique report -- <your command>` shows exactly which targets your
program hits and how often. Code whose time is in its own bytecode (most framework request
handling) sees little change. See the README for measured numbers.

**Does it work with free-threaded Python, subinterpreters, debuggers, coverage,
profilers?**

- Free-threaded 3.14t/3.15t: yes, with wheels for both.
- Subinterpreters: the extension refuses to load in them, so code there runs plain CPython.
- Debuggers, coverage, profilers and `sys.settrace`: pure-Python targets fall back while a
  tool is listening, so tools see CPython's own frames. The report counts these calls
  under `tools`.

**Does `RUSTIQUE=1` reach subprocesses?** Yes, if they inherit the environment and use an
interpreter with Rustique installed. That includes `multiprocessing` with `spawn` or
`forkserver`, pytest-xdist workers, and `subprocess.run([sys.executable, ...])`. A forked
child inherits the parent's state as is. A forked child that exits with `os._exit` (the
`fork` start method) writes no report.

**Is `rustique.enable()` in a library a good idea?** No. It is process-wide; leave the
decision to the application. Libraries can tell users about `RUSTIQUE=1`.

**What does `RUSTIQUE=1` cost at startup?** It imports Rustique's extension and installs
the targets that are already imported (others install on import). The measured cost is in
the README. With `RUSTIQUE` unset, Rustique is never imported. The only cost then is the
`.pth` line (see [Install](#install)).

**Can I use Rustique's own containers (`rustique.List`, `Dict`, …)?** Yes. They are an
opt-in tier with different trade-offs, described further down in the README and in
[`TAKEOVER.md`](TAKEOVER.md). `enable(native=...)` rewrites chosen modules to build them.
Unlike everything above, this changes types, so it is never on by default.

## Troubleshooting

- **`RUSTIQUE=1` does nothing.** Run `python -m rustique status` with the same interpreter.
  - "no startup file found" means Rustique is installed as an editable/development build,
    which does not install `_rustique_startup.pth`. Use `python -m rustique app.py` or
    `rustique.enable()`.
  - "(no RUSTIQUE* variables set)" means the variable did not reach the process, or Python
    runs with `-E`/`-I`.
- **`diff` says the command wrote no report.** The command ran an interpreter without
  Rustique (a different venv, a system `pytest`), or no Python at all. Use
  `python -m pytest` with the right interpreter.
- **`diff` reports a difference.**
  1. Rerun it. If the plain side also differs between runs, the program is
     nondeterministic (time, randomness, network). Mask the volatile part with
     `--ignore REGEX`.
  2. Narrow it down with `RUSTIQUE_ONLY=<group>`, one group at a time.
  3. Report it with the diff output.
- **A target shows as refused.**
  - Version refusals ("not validated") are expected on interpreters outside a target's
    validated set.
  - "fingerprint" or "conflict" means the stdlib function is not the one Rustique was
    validated against, or another tool patched it first (e.g. gevent, a monkeypatching
    APM agent). CPython's code runs, so this is safe. Tell us if you think it should be
    supported.
- **pytest: "option names {'--rustique'} already added".** The plugin was loaded twice
  under different names. Use `-p rustique.pytest_plugin`, the entry point's name, or rely
  on autoloading.
- **Rustique in `pip list`, but `import rustique` fails.** The wheel does not match the
  interpreter (`pip debug --verbose` lists supported tags), or the venv is broken. The
  startup switch never breaks Python startup: it prints
  `rustique: RUSTIQUE=1 ignored: cannot import rustique: ...` and continues plain.
