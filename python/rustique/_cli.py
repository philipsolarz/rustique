"""The ``python -m rustique`` command line (docs/USAGE.md).

``run`` (the default) enables Rustique, then runs a script, module or command in this
process, before any of the program's code is compiled. ``status`` shows what would be
accelerated on this interpreter and why anything is refused. ``report -- <cmd>`` runs a
command with Rustique and its counters on and prints the per-target report of every
Python process it started. ``diff -- <cmd>`` runs a command plain (``RUSTIQUE=0``) and
enabled (``RUSTIQUE=1``) and compares exit codes, stdout and stderr (and, for pytest, every
test's outcome), reporting any difference and the timing.
"""

import os
import sys

COMMANDS = ("run", "status", "report", "diff", "estimate")
USAGE = """\
usage: python -m rustique [run] [--only G,..] [--exclude G,..] [--tiers T,..] [--report] [--check|--check-raise]
                          [--no-builtins] (script.py | -m module | -c command) [args...]
       python -m rustique status [--json] [--verbose]
       python -m rustique report [--json] [-o FILE] [--limit N] [--verbose] [--check] [--tiers T,..] -- <command...>
       python -m rustique report --from FILE.json [--json] [--verbose]
       python -m rustique diff [--repeat N] [--ignore REGEX]... [--no-normalize] [--keep DIR] [--check]
                               [--tiers T,..] [--confirm N] -- <command...>
       python -m rustique estimate [--json] [--tiers T,..] -- <command...>

  run      run a Python program with Rustique enabled (RUSTIQUE=0 still turns it off)
  status   what Rustique accelerates on this interpreter, and why anything is refused
  report   run a command with RUSTIQUE=1 and counters on; print the per-target report of every process
  diff     run a command plain and with RUSTIQUE=1; compare exit code, output, test outcomes
  estimate predict what Rustique would do for a command's run time, from the command itself

Opt-in tiers (--tiers, RUSTIQUE_TIERS): interp (opcode handlers), gc (adaptive collection policy).
Environment: RUSTIQUE=1|0, RUSTIQUE_ONLY, RUSTIQUE_EXCLUDE, RUSTIQUE_TIERS, RUSTIQUE_STATS, RUSTIQUE_REPORT,
RUSTIQUE_CHECK, RUSTIQUE_UNVALIDATED_PATCH (docs/USAGE.md).
"""
MIGRATION = ("rustique: --native was removed with Rustique's own container types (List, Dict, ...): "
             "the standard types are accelerated in place. See docs/MIGRATION.md.")
TIERS_OFF = ("", "0", "none", "off", "false", "no")


def add_tiers(tiers: str | None) -> None:
    """``--tiers interp,gc``: opt in to tiers for this process and its children (union with
    ``RUSTIQUE_TIERS``). ``RUSTIQUE_TIERS=none`` in the environment wins: it turns every tier off."""
    if not tiers:
        return
    cur = os.environ.get("RUSTIQUE_TIERS")
    if cur is not None and cur.strip().lower() in TIERS_OFF:
        sys.stderr.write(f"rustique: --tiers {tiers} ignored: RUSTIQUE_TIERS={cur!r} turns every tier off\n")
        return
    names = [n.strip() for n in (cur or "").split(",") + tiers.split(",") if n.strip()]
    os.environ["RUSTIQUE_TIERS"] = ",".join(dict.fromkeys(names))



def main(argv: list[str] | None = None) -> None:
    argv = sys.argv[1:] if argv is None else list(argv)
    if not argv or argv[0] in ("-h", "--help"):
        sys.stdout.write(USAGE)
        raise SystemExit(0 if argv else 2)
    if argv[0] == "--version":
        from rustique._rustique import __version__

        print(f"rustique {__version__}")
        return
    cmd = argv[0] if argv[0] in COMMANDS else "run"
    if cmd == argv[0]:
        argv = argv[1:]
    if cmd == "run":
        run(argv)
    elif cmd == "status":
        raise SystemExit(status_cmd(argv))
    elif cmd == "report":
        raise SystemExit(report_cmd(argv))
    elif cmd == "estimate":
        from rustique import _estimate

        raise SystemExit(_estimate.main(argv))
    else:
        from rustique import _diff

        raise SystemExit(_diff.main(argv))


# --- run ---------------------------------------------------------------------------------------

_VALUE_OPTS = ("--only", "--exclude", "--tiers")


def _parse_run(argv: list[str]):
    import argparse

    p = argparse.ArgumentParser(prog="python -m rustique [run]", description="Run a program with Rustique enabled.")
    p.add_argument("--only", metavar="G,..", help="enable only these groups or targets")
    p.add_argument("--exclude", metavar="G,..", help="do not enable these groups or targets")
    p.add_argument("--report", action="store_true", help="print the per-target report at exit (RUSTIQUE_REPORT=1)")
    p.add_argument("--check", action="store_true",
                   help="check mode (RUSTIQUE_CHECK=1): compare checkable fast paths with CPython's original")
    p.add_argument("--check-raise", action="store_true", help="check mode, a mismatch raises (RUSTIQUE_CHECK=raise)")
    p.add_argument("--tiers", metavar="T,..", help="opt-in tiers to turn on too (interp, gc; RUSTIQUE_TIERS)")
    p.add_argument("--no-builtins", action="store_true", help="do not accelerate the builtin functions")
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("-m", dest="module", metavar="MODULE", help="run library module as a script")
    g.add_argument("-c", dest="command", metavar="CMD", help="program passed in as string")
    g.add_argument("script", nargs="?", help="program file")
    # Everything after the program is the program's own argv.
    for i, a in enumerate(argv):
        if a in ("-m", "-c") and i + 1 < len(argv):
            return p.parse_args(argv[: i + 2]), argv[i + 2 :]
        if not a.startswith("-") and (i == 0 or argv[i - 1] not in _VALUE_OPTS):
            return p.parse_args(argv[: i + 1]), argv[i + 1 :]
    return p.parse_args(argv), []


def _fresh_main(path: str | None):
    import builtins
    import types

    main = types.ModuleType("__main__")
    main.__dict__.update(__file__=path, __cached__=None, __loader__=None, __package__=None, __spec__=None,
                         __builtins__=builtins)
    if path is None:
        del main.__file__
    sys.modules["__main__"] = main
    return main


def run(argv: list[str]) -> None:
    if any(a == "--native" or a.startswith(("--native=", "--native-main")) for a in argv[: _program_index(argv)]):
        sys.stderr.write(MIGRATION + "\n")
        raise SystemExit(2)
    opts, rest = _parse_run(argv)
    import rustique

    if opts.report and not os.environ.get("RUSTIQUE_REPORT", "").strip():
        rustique._accel.report_to("stderr")
    if (opts.check or opts.check_raise) and not rustique._killed():
        rustique.check("raise" if opts.check_raise else "warn")
    add_tiers(opts.tiers)
    rustique._enable(opts.only, opts.exclude, builtins=not opts.no_builtins, source="python -m rustique")
    if not rustique._killed() and "RUSTIQUE" not in os.environ:
        # child interpreters (multiprocessing spawn/forkserver, subprocess) start through the
        # startup switch, like under RUSTIQUE=1; the selection goes with them (env narrows)
        os.environ["RUSTIQUE"] = "1"
        if opts.only and "RUSTIQUE_ONLY" not in os.environ:
            os.environ["RUSTIQUE_ONLY"] = opts.only
        if opts.exclude:
            os.environ["RUSTIQUE_EXCLUDE"] = ",".join(filter(None, [os.environ.get("RUSTIQUE_EXCLUDE"), opts.exclude]))

    if opts.module is not None:
        import runpy

        sys.argv = [opts.module, *rest]
        _as_main(lambda: runpy.run_module(opts.module, run_name="__main__", alter_sys=True), skip_runpy=True)
        return
    if opts.command is not None:
        sys.argv = ["-c", *rest]
        sys.path.insert(0, "")
        code = compile(opts.command, "<string>", "exec")
        mod = _fresh_main(None)
        _as_main(lambda: exec(code, mod.__dict__))
        return
    sys.argv = [opts.script, *rest]
    path = os.path.abspath(opts.script)  # like `python script.py`: __file__ and code paths are absolute
    sys.path.insert(0, os.path.dirname(path))
    with open(path, "rb") as f:
        source = f.read()
    code = compile(source, path, "exec", dont_inherit=True)
    mod = _fresh_main(path)
    _as_main(lambda: exec(code, mod.__dict__))


def _program_index(argv: list[str]) -> int:
    """Index of the program (script, ``-m``, ``-c``) in the run options: options after it are
    the program's own."""
    for i, a in enumerate(argv):
        if a in ("-m", "-c") or (not a.startswith("-") and (i == 0 or argv[i - 1] not in _VALUE_OPTS)):
            return i
    return len(argv)


def _as_main(body, skip_runpy: bool = False) -> None:
    """Run the program like the interpreter runs ``__main__``: an uncaught exception prints
    a traceback that starts at the program's own frame (not at this runner's), exit status 1;
    ``KeyboardInterrupt`` ends the process by SIGINT, as CPython does."""
    try:
        body()
    except SystemExit:
        raise
    except BaseException as exc:  # noqa: BLE001
        tb = exc.__traceback__.tb_next  # drop this function's frame
        if tb is not None:
            tb = tb.tb_next  # and the lambda's
        here = os.path.dirname(os.path.abspath(__file__))
        while skip_runpy and tb is not None and (tb.tb_frame.f_code.co_filename.startswith(here)
                                                 or tb.tb_frame.f_code.co_filename == "<frozen runpy>"):
            tb = tb.tb_next
        if tb is not None:
            exc.__traceback__ = tb  # the default hook prints the exception's own traceback
        sys.excepthook(type(exc), exc, exc.__traceback__)
        if isinstance(exc, KeyboardInterrupt):
            import signal

            sys.stdout.flush()
            sys.stderr.flush()
            signal.signal(signal.SIGINT, signal.SIG_DFL)
            os.kill(os.getpid(), signal.SIGINT)
        raise SystemExit(1) from None


# --- status ------------------------------------------------------------------------------------


def startup_files() -> list[str]:
    """Rustique's startup files found in this interpreter's site directories."""
    import site

    dirs = list(site.getsitepackages()) if hasattr(site, "getsitepackages") else []
    if site.ENABLE_USER_SITE:
        dirs.append(site.getusersitepackages())
    found = []
    for d in dict.fromkeys(dirs):
        for name in ("_rustique_startup.pth", "_rustique_startup.start"):
            p = os.path.join(d, name)
            if os.path.exists(p):
                found.append(p)
    return found


def status_cmd(argv: list[str]) -> int:
    import argparse

    p = argparse.ArgumentParser(prog="python -m rustique status",
                                description="What Rustique accelerates on this interpreter, and why anything is refused.")
    p.add_argument("--json", action="store_true", help="print rustique.status() as JSON")
    p.add_argument("--verbose", "-v", action="store_true", help="every target name and refusal reason")
    opts = p.parse_args(argv)
    import rustique
    from rustique import _report

    rustique._enable(source="python -m rustique status")
    st = rustique.status()
    hooks = startup_files()
    if opts.json:
        import json

        st["process"]["startup_files"] = hooks
        st["process"]["executable"] = sys.executable
        print(json.dumps(st, indent=1, default=list))
        return 0
    import platform

    proc = st["process"]
    sw = proc["switch"]
    print(f"Python      {sys.executable}")
    print(f"            CPython {proc['python']} ({'free-threaded' if proc['free_threaded'] else 'GIL'}"
          f"{', debug build' if hasattr(sys, 'gettotalrefcount') else ''}), {sys.platform} {platform.machine()}"
          f"{' ' + ' '.join(platform.libc_ver()).strip() if sys.platform.startswith('linux') else ''}")
    print(f"Rustique    {proc['rustique']} at {os.path.dirname(rustique.__file__)}")
    if not sys.flags.no_site and hooks:
        print(f"Startup     RUSTIQUE=1 enables at startup ({', '.join(os.path.basename(h) for h in hooks)} "
              f"in {os.path.dirname(hooks[0])})")
    elif sys.flags.no_site:
        print("Startup     python -S: no site packages are processed, so RUSTIQUE=1 does nothing at startup "
              "(use `python -m rustique app.py` or rustique.enable())")
    else:
        print("Startup     no startup file in this interpreter's site directories (an editable/development install, "
              "or `pip install --target`): RUSTIQUE=1 does nothing at startup here "
              "(use `python -m rustique app.py` or rustique.enable())")
    env = sw.get("env") or {}
    print(f"Environment {' '.join(f'{k}={v}' for k, v in env.items()) if env else '(no RUSTIQUE* variables set)'}"
          f"{' [ignored: -E/-I]' if sys.flags.ignore_environment else ''}")
    if sw.get("killed"):
        print("\nRUSTIQUE=0 is set: the kill switch makes every enable path a no-op in this environment.")
        return 0
    if proc.get("interpreter"):
        print(f"Refused     {proc['interpreter']}: no target installs on this interpreter")
    if proc.get("jit"):
        print("JIT         on (sys._jit): targets not validated with the JIT are refused")
    print()
    print(_report.render_status(st), end="")
    return 0


# --- report ------------------------------------------------------------------------------------


# Options of report/diff/estimate that take a value (so the value is not the command).
VALUE_OPTIONS = ("-o", "--output", "--limit", "--tiers", "--from", "--repeat", "--ignore", "--keep", "--timeout",
                 "--max-lines", "--confirm", "--mode")


def split_command(argv: list[str]) -> tuple[list[str], list[str]]:
    """Options before ``--`` (or before the first non-option) and the command after it."""
    if "--" in argv:
        i = argv.index("--")
        return argv[:i], argv[i + 1 :]
    for i, a in enumerate(argv):
        if not a.startswith("-") and (i == 0 or argv[i - 1] not in VALUE_OPTIONS):
            return argv[:i], argv[i:]
    return argv, []


def resolve_command(cmd: list[str]) -> list[str]:
    """``app.py args`` runs with this interpreter; anything else runs as given."""
    if cmd and cmd[0].endswith(".py") and not os.access(cmd[0], os.X_OK):
        return [sys.executable, *cmd]
    return cmd


def read_records(path: str) -> list[dict]:
    """JSON lines of ``path`` (a report file: :func:`rustique._report.collect` adds the processes
    that left only live counters)."""
    from rustique import _report

    return _report.read_json_records(path)


NO_RECORDS = ("rustique: the command wrote no report: Rustique was not enabled in any Python process "
              "it ran. Is rustique installed in the interpreter the command uses? "
              "(Run `python -m rustique status` with that interpreter.)\n")


def report_cmd(argv: list[str]) -> int:
    import argparse
    import subprocess
    import tempfile
    import time

    opts_argv, cmd = split_command(argv)
    p = argparse.ArgumentParser(prog="python -m rustique report",
                                description="Run a command with RUSTIQUE=1 and counters on; print the per-target report.")
    p.add_argument("--json", action="store_true", help="print the merged record as JSON")
    p.add_argument("-o", "--output", help="write the report to FILE instead of stderr")
    p.add_argument("--limit", type=int, default=None, help="show at most N targets")
    p.add_argument("--verbose", "-v", action="store_true", help="every installed target and every name")
    p.add_argument("--check", action="store_true", help="check mode in the command (RUSTIQUE_CHECK=1)")
    p.add_argument("--tiers", metavar="T,..", help="opt-in tiers to turn on too (RUSTIQUE_TIERS)")
    p.add_argument("--from", dest="source", metavar="FILE.json",
                   help="render the records a run with RUSTIQUE_REPORT=FILE.json left (no command)")
    opts = p.parse_args(opts_argv)
    from rustique import _report

    if opts.source:
        if cmd:
            p.error("--from takes no command")
        records = _report.collect(opts.source)
        if not records:
            sys.stderr.write(f"rustique report: no record in {opts.source} (nor live counter files next to it)\n")
            return 2
        return _emit(_report.merge(records), opts, "")
    if not cmd:
        p.error("missing command (python -m rustique report -- <command...>)")
    import rustique

    if rustique._killed():
        sys.stderr.write("rustique report: RUSTIQUE=0 (kill switch) is set; unset it to report.\n")
        return 2
    add_tiers(opts.tiers)
    with tempfile.TemporaryDirectory(prefix="rustique-report-") as tmp:
        path = os.path.join(tmp, "report.json")
        env = {**os.environ, "RUSTIQUE": "1", "RUSTIQUE_STATS": "1", "RUSTIQUE_REPORT": path}
        if opts.check:
            env["RUSTIQUE_CHECK"] = "1"
        t0 = time.perf_counter()
        try:
            rc = subprocess.call(resolve_command(cmd), env=env)
        except FileNotFoundError as exc:
            sys.stderr.write(f"rustique report: {exc}\n")
            return 127
        elapsed = time.perf_counter() - t0
        records = _report.collect(path)
    if not records or not any(r.get("status") for r in records):
        sys.stderr.write(NO_RECORDS)
        return rc
    _emit(_report.merge(records), opts,
          f"  command exit code {rc}, {elapsed:.2f} s wall, {len(records)} Python process(es) reported\n")
    return rc


def _emit(rec: dict, opts, footer: str) -> int:
    from rustique import _report

    if opts.json:
        import json

        text = json.dumps(rec, indent=1, default=list) + "\n"
    else:
        text = f"\n{_report.render(rec, limit=opts.limit, verbose=opts.verbose)}{footer}"
    if opts.output:
        with open(opts.output, "w") as f:
            f.write(text)
    elif opts.source:  # nothing else writes: the report is the output
        sys.stdout.write(text.lstrip("\n"))
    else:  # after the command: its stdout stays clean
        sys.stderr.write(text)
    return 0
