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

COMMANDS = ("run", "status", "report", "diff")
USAGE = """\
usage: python -m rustique [run] [--only G,..] [--exclude G,..] [--report] [--check|--check-raise] [--native PATTERN]...
                          [--no-builtins] (script.py | -m module | -c command) [args...]
       python -m rustique status [--json]
       python -m rustique report [--json] [-o FILE] [--limit N] [--check] -- <command...>
       python -m rustique diff [--repeat N] [--ignore REGEX]... [--no-normalize] [--keep DIR] [--check] -- <command...>

  run      run a Python program with Rustique enabled (RUSTIQUE=0 still turns it off)
  status   what Rustique accelerates on this interpreter, and why anything is refused
  report   run a command with RUSTIQUE=1 and counters on; print the per-target report
  diff     run a command plain and with RUSTIQUE=1; compare exit code, output, test outcomes

Environment: RUSTIQUE=1|0, RUSTIQUE_ONLY, RUSTIQUE_EXCLUDE, RUSTIQUE_STATS, RUSTIQUE_REPORT, RUSTIQUE_CHECK
(docs/USAGE.md).
"""


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
    else:
        from rustique import _diff

        raise SystemExit(_diff.main(argv))


# --- run ---------------------------------------------------------------------------------------

_VALUE_OPTS = ("--native", "--only", "--exclude")


def _parse_run(argv: list[str]):
    import argparse

    p = argparse.ArgumentParser(prog="python -m rustique [run]", description="Run a program with Rustique enabled.")
    p.add_argument("--only", metavar="G,..", help="enable only these groups or targets")
    p.add_argument("--exclude", metavar="G,..", help="do not enable these groups or targets")
    p.add_argument("--report", action="store_true", help="print the per-target report at exit (RUSTIQUE_REPORT=1)")
    p.add_argument("--check", action="store_true",
                   help="check mode (RUSTIQUE_CHECK=1): compare checkable fast paths with CPython's original")
    p.add_argument("--check-raise", action="store_true", help="check mode, a mismatch raises (RUSTIQUE_CHECK=raise)")
    p.add_argument("--native", action="append", default=[], metavar="PATTERN",
                   help="rewrite modules matching PATTERN (and the main program) to Rustique containers")
    p.add_argument("--native-main", action="store_true", help="rewrite the main program only")
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
    opts, rest = _parse_run(argv)
    import rustique

    if opts.report and rustique._report_target() is None:
        rustique._report_to("stderr")
    if (opts.check or opts.check_raise) and not rustique._killed():
        rustique.check("raise" if opts.check_raise else "warn")
    rustique._enable(opts.only, opts.exclude, opts.native or None, not opts.no_builtins, source="python -m rustique")
    native_main = (bool(opts.native) or opts.native_main) and not rustique._killed()
    if native_main:
        from rustique._takeover import _rewrite

        helpers = _rewrite.helpers()
    else:
        helpers = {}

    if opts.module is not None:
        import runpy

        sys.argv = [opts.module, *rest]
        if native_main:
            from rustique._takeover import _hook

            _hook.install((opts.module,))
        runpy.run_module(opts.module, run_name="__main__", alter_sys=True, init_globals=helpers or None)
        return
    if opts.command is not None:
        sys.argv = ["-c", *rest]
        sys.path.insert(0, "")
        if native_main:
            code = _rewrite.compile_rewritten(opts.command, "<string>")
        else:
            code = compile(opts.command, "<string>", "exec")
        mod = _fresh_main(None)
        mod.__dict__.update(helpers)
        exec(code, mod.__dict__)
        return
    path = opts.script
    sys.argv = [path, *rest]
    sys.path.insert(0, os.path.dirname(os.path.abspath(path)))
    with open(path, "rb") as f:
        source = f.read()
    if native_main:
        code = _rewrite.compile_rewritten(source, path)
    else:
        code = compile(source, path, "exec", dont_inherit=True)
    mod = _fresh_main(path)
    mod.__dict__.update(helpers)
    exec(code, mod.__dict__)


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
    proc = st["process"]
    sw = proc["switch"]
    print(f"Python      {sys.executable}")
    print(f"            CPython {proc['python']} ({'free-threaded' if proc['free_threaded'] else 'GIL'}"
          f"{', debug build' if hasattr(sys, 'gettotalrefcount') else ''}), {sys.platform}")
    print(f"Rustique    {proc['rustique']} at {os.path.dirname(rustique.__file__)}")
    if hooks:
        print(f"Startup     RUSTIQUE=1 enables at startup ({', '.join(os.path.basename(h) for h in hooks)} "
              f"in {os.path.dirname(hooks[0])})")
    else:
        print("Startup     no startup file found: RUSTIQUE=1 does nothing at startup here "
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
        print("JIT         on (sys._jit): targets marked not JIT-safe are refused")
    print()
    print(_report.render_status(st), end="")
    return 0


# --- report ------------------------------------------------------------------------------------


def split_command(argv: list[str]) -> tuple[list[str], list[str]]:
    """Options before ``--`` (or before the first non-option) and the command after it."""
    if "--" in argv:
        i = argv.index("--")
        return argv[:i], argv[i + 1 :]
    for i, a in enumerate(argv):
        if not a.startswith("-"):
            return argv[:i], argv[i:]
    return argv, []


def resolve_command(cmd: list[str]) -> list[str]:
    """``app.py args`` runs with this interpreter; anything else runs as given."""
    if cmd and cmd[0].endswith(".py") and not os.access(cmd[0], os.X_OK):
        return [sys.executable, *cmd]
    return cmd


def read_records(path: str) -> list[dict]:
    import json

    if not os.path.exists(path):
        return []
    out = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line:
                try:
                    out.append(json.loads(line))
                except ValueError:
                    pass  # a process killed mid-write
    return out


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
    p.add_argument("--limit", type=int, default=None, help="show at most N installed targets")
    p.add_argument("--check", action="store_true", help="check mode in the command (RUSTIQUE_CHECK=1)")
    opts = p.parse_args(opts_argv)
    if not cmd:
        p.error("missing command (python -m rustique report -- <command...>)")
    import rustique
    from rustique import _report

    if rustique._killed():
        sys.stderr.write("rustique report: RUSTIQUE=0 (kill switch) is set; unset it to report.\n")
        return 2
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
        records = read_records(path)
    if not records:
        sys.stderr.write(NO_RECORDS)
        return rc
    rec = _report.merge(records)
    if opts.json:
        import json

        text = json.dumps(rec, indent=1, default=list) + "\n"
    else:
        text = (f"\n{_report.render(rec, limit=opts.limit)}"
                f"  command exit code {rc}, {elapsed:.2f} s wall, {len(records)} Python process(es) reported\n")
    if opts.output:
        with open(opts.output, "w") as f:
            f.write(text)
    else:
        sys.stderr.write(text)
    return rc
