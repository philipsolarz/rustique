"""Where `import rustique; rustique.enable()` spends, by whole-process differences (target T8).

    python benches/startup/import_parts.py RUSTIQUE_VENV [PLAIN_VENV]

Each step is a `python -c CODE` process in RUSTIQUE_VENV (RUSTIQUE unset, PYTHONHASHSEED=0)
measured whole by callgrind; a part is the difference between consecutive steps. The steps:

    pass                              the installed-but-off baseline
    import _rustique                  + finding the extension, dlopen, module exec
    + ._accel                         + the registry/switch submodule (lazy)
    import rustique                   + the package (finder, pyc, __init__ body)
    + _accel.switch_enable()          + the quiet enable (what RUSTIQUE=1 runs)
    + rustique.enable()               + building the status dict that enable() returns
"""

from __future__ import annotations

import os
import re
import subprocess
import sys

STEPS = [
    ("pass", "pass"),
    ("import _rustique", "import _rustique"),
    ("+ ._accel", "import _rustique; _rustique._accel"),
    ("import rustique", "import rustique"),
    ("+ _accel.switch_enable()", "import rustique; rustique._accel.switch_enable()"),
    ("+ status (= rustique.enable())", "import rustique; rustique.enable()"),
]


def ir(py: str, code: str) -> int:
    env = {k: v for k, v in os.environ.items() if not k.startswith(("RUSTIQUE", "PYTHON", "VIRTUAL_ENV"))}
    env["PYTHONHASHSEED"] = "0"
    out = subprocess.run(["valgrind", "--tool=callgrind", "--callgrind-out-file=/dev/null", py, "-c", code],
                         env=env, capture_output=True, text=True)
    m = re.search(r"Collected : (\d+)", out.stderr)
    if out.returncode or not m:
        raise SystemExit(f"{code}: {out.stderr[-1500:]}")
    return int(m.group(1))


def main() -> None:
    py = os.path.join(sys.argv[1], "bin", "python")
    base = ir(os.path.join(sys.argv[2], "bin", "python"), "pass") if len(sys.argv) > 2 else None
    prev = None
    first = None
    print(f"{'step':<34} {'Ir':>12} {'part':>10}" + ("  ratio vs plain" if base else ""))
    for name, code in STEPS:
        v = ir(py, code)
        first = first or v
        part = "" if prev is None else f"{v - prev:>10,}"
        ratio = f"  {v / base:.4f}" if base else ""
        print(f"{name:<34} {v:>12,} {part:>10}{ratio}")
        prev = v
    print(f"{'total over pass':<34} {prev - first:>12,}")


if __name__ == "__main__":
    main()
