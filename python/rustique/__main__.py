"""``python -m rustique``: run a program with the takeover enabled (docs/TAKEOVER.md).

    python -m rustique [--native PATTERN]... [--no-builtins] script.py [args...]
    python -m rustique [options] -m module [args...]
    python -m rustique [options] -c command [args...]

The safe tier (exact builtins, Rust kernels behind their methods) is enabled before any
of the program's code is compiled. ``--native PATTERN`` (repeatable, ``fnmatch`` syntax on
module names) opts modules into the native tier; when it is given, the main script, module
or command is rewritten too (this is what ``rustique.enable()`` cannot do for a ``__main__``
that is already running). ``--native-main`` rewrites only the main program.
"""

from __future__ import annotations

import argparse
import builtins
import os
import runpy
import sys
import types


def _parse(argv: list[str]) -> tuple[argparse.Namespace, list[str]]:
    p = argparse.ArgumentParser(prog="python -m rustique", description=__doc__.split("\n\n")[0])
    p.add_argument("--native", action="append", default=[], metavar="PATTERN",
                   help="rewrite modules matching PATTERN (and the main program) to Rustique containers")
    p.add_argument("--native-main", action="store_true", help="rewrite the main program only")
    p.add_argument("--no-builtins", action="store_true", help="do not install the replacement builtin functions")
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("-m", dest="module", metavar="MODULE", help="run library module as a script")
    g.add_argument("-c", dest="command", metavar="CMD", help="program passed in as string")
    g.add_argument("script", nargs="?", help="program file")
    # Everything after the program is the program's own argv.
    for i, a in enumerate(argv):
        if a in ("-m", "-c") and i + 1 < len(argv):
            return p.parse_args(argv[: i + 2]), argv[i + 2 :]
        if not a.startswith("-") and (i == 0 or argv[i - 1] != "--native"):
            return p.parse_args(argv[: i + 1]), argv[i + 1 :]
    return p.parse_args(argv), []


def _fresh_main(path: str | None) -> types.ModuleType:
    main = types.ModuleType("__main__")
    main.__dict__.update(__file__=path, __cached__=None, __loader__=None, __package__=None, __spec__=None,
                         __builtins__=builtins)
    if path is None:
        del main.__file__
    sys.modules["__main__"] = main
    return main


def main(argv: list[str] | None = None) -> None:
    opts, rest = _parse(sys.argv[1:] if argv is None else argv)
    import rustique
    from rustique._takeover import _rewrite

    rustique.enable(native=opts.native or None, builtins=not opts.no_builtins)
    native_main = bool(opts.native) or opts.native_main
    helpers = _rewrite.helpers() if native_main else {}

    if opts.module is not None:
        sys.argv = [opts.module, *rest]
        if native_main:
            from rustique._takeover import _hook

            _hook.install((opts.module,))
        runpy.run_module(opts.module, run_name="__main__", alter_sys=True, init_globals=helpers or None)
        return
    if opts.command is not None:
        sys.argv = ["-c", *rest]
        sys.path.insert(0, "")
        code = _rewrite.compile_rewritten(opts.command, "<string>") if native_main else compile(opts.command, "<string>", "exec")
        mod = _fresh_main(None)
        mod.__dict__.update(helpers)
        exec(code, mod.__dict__)
        return
    path = opts.script
    sys.argv = [path, *rest]
    sys.path.insert(0, os.path.dirname(os.path.abspath(path)))
    with open(path, "rb") as f:
        source = f.read()
    code = _rewrite.compile_rewritten(source, path) if native_main else compile(source, path, "exec", dont_inherit=True)
    mod = _fresh_main(path)
    mod.__dict__.update(helpers)
    exec(code, mod.__dict__)


if __name__ == "__main__":
    main()
