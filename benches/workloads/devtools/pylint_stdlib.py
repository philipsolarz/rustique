"""pylint over 3 real standard-library modules (~4.5k lines), all default checkers, jobs=1.

Stands for the heavyweight static-analysis lint in CI and IDEs: astroid builds its own
inference-capable tree from `ast.parse`, pylint walks it with ~40 checkers and runs
astroid inference (attribute/call resolution through stdlib stubs and "brains"). Each
`run()` starts from a cleared astroid cache, like a fresh `pylint` process does after
startup, so the builtins/stdlib modules it infers through are rebuilt every run. Inputs
are the running interpreter's own `Lib/` files.
"""

import hashlib
import io
import os
import sysconfig

REQUIRES = ["pylint==4.1.3", "astroid==4.3.4"]

FILES = ("shlex.py", "textwrap.py", "fractions.py")


def setup():
    import pylint.lint  # noqa: F401
    from pylint.reporters.text import TextReporter  # noqa: F401

    root = sysconfig.get_path("stdlib")
    return [os.path.join(root, f) for f in FILES]


def run(state):
    import astroid
    from pylint.lint import Run
    from pylint.reporters.text import TextReporter

    astroid.MANAGER.clear_cache()
    out = io.StringIO()
    Run(
        ["--jobs=1", "--persistent=n", "--rcfile=/dev/null", "--score=n",
         "--msg-template={path}:{line}:{column}: {msg_id} {symbol}", *state],
        reporter=TextReporter(out),
        exit=False,
    )
    root = sysconfig.get_path("stdlib")
    return sorted(line.replace(root + os.sep, "") for line in out.getvalue().splitlines())


def digest(result):
    return hashlib.sha256("\n".join(result).encode()).hexdigest()
