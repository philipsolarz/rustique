"""What a first-call (deferred) install would save at startup (docs/atlas/w2-startup.md §2.4).

    python benches/startup/deferral_sim.py [--python VENV/bin/python]

Callgrind regions (install_costs.py's worker, GC off): `_accel.enable(only=S)` in a fresh
process where the interpreter's own modules are imported (posixpath, time, builtins), for
S = every target installed at RUSTIQUE=1 startup, and S = the subsets that real short
processes call (eager_use.py, `import_startup` children). A deferred install validates only
targets that get called, so enable(all) − enable(used) is its saving in that process
(stub installation, ~1k Ir per target, not counted).
"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from install_costs import ACCEL, region  # noqa: E402

EAGER = ["list.sort", "list.index", "list.count", "tuple.index", "tuple.count", "list.__contains__",
         "tuple.__contains__", "builtins.sum", "builtins.min", "builtins.max", "builtins.sorted", "builtins.any",
         "builtins.all", "float.repr", "posixpath.join", "posixpath.split", "posixpath.splitext",
         "posixpath.basename", "posixpath.dirname", "posixpath.isabs", "posixpath.relpath", "posixpath.abspath",
         "time.strftime"]
USED = {
    "bare `-c pass` (3 called)": ["builtins.max", "posixpath.join", "tuple.__contains__"],
    "import requests (11)": ["builtins.any", "builtins.max", "builtins.min", "builtins.sorted", "builtins.sum",
                             "list.__contains__", "list.sort", "posixpath.join", "time.strftime",
                             "tuple.__contains__", "tuple.index"],
    "import black (15)": ["builtins.any", "builtins.max", "builtins.min", "builtins.sorted", "builtins.sum",
                          "list.__contains__", "list.count", "list.index", "list.sort", "posixpath.dirname",
                          "posixpath.isabs", "posixpath.join", "posixpath.splitext", "tuple.__contains__",
                          "tuple.index"],
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", default=sys.executable)
    a = ap.parse_args()
    full = region(a.python, ACCEL, f"A.enable(only={EAGER!r})")
    print(f"{'all 23 eager targets':<28} {full:>10,}")
    for name, used in USED.items():
        ir = region(a.python, ACCEL, f"A.enable(only={used!r})")
        print(f"{name:<28} {ir:>10,}  saving {full - ir:>10,} ({100 * (full - ir) / full:.0f}%)")


if __name__ == "__main__":
    main()
