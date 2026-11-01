"""Startup cost of the `RUSTIQUE=1` path (the `.pth` line loads only the extension and calls
`_accel.startup()`), in fresh venvs, against plain CPython without rustique.

Usage (on the box, after `maturin build --release`):
    python benches/startup_switch.py --wheel dist/rustique-*.whl --python cpython-3.15.1-linux-x86_64-gnu \
        --work /tmp/sw [--wall 40] [--pth-line-from tests/test_accel_startup.py]

Builds four venvs: plain, plain + another `.pth`, rustique, rustique + another `.pth`
(the "other" `.pth` holds only a comment: what any project with an editable install or
setuptools' distutils-precedence.pth has). Replaces the wheel's `_rustique_startup.pth`
with the extension-only line of tests/test_accel_startup.py. Cells: `python -c pass` with
RUSTIQUE unset and `RUSTIQUE=1`; whole-process callgrind Ir, wall time (interleaved
runs), perf page faults.
"""

from __future__ import annotations

import argparse
import ast
import glob
import os
import shutil
import statistics
import subprocess
import tempfile
import time


def pth_line(path: str) -> str:
    """`PTH_LINE` of the test module (its module-level constant assignments evaluated)."""
    ns: dict[str, object] = {}
    for node in ast.parse(open(path).read()).body:
        if isinstance(node, ast.Assign) and getattr(node.targets[0], "id", "") in ("LOADER", "PTH_LINE"):
            exec(compile(ast.Module([node], []), path, "exec"), ns)
    if "PTH_LINE" not in ns:
        raise SystemExit("PTH_LINE not found")
    return str(ns["PTH_LINE"])


def venv(work: str, name: str, python: str, wheel: str | None, other: bool, line: str) -> str:
    d = os.path.join(work, name)
    shutil.rmtree(d, ignore_errors=True)
    subprocess.run(["uv", "venv", "-q", "--python", python, d], check=True)
    py = os.path.join(d, "bin", "python")
    site = subprocess.run([py, "-c", "import sysconfig; print(sysconfig.get_paths()['purelib'])"],
                          capture_output=True, text=True, check=True).stdout.strip()
    if wheel:
        subprocess.run(["uv", "pip", "install", "-q", "--python", py, wheel], check=True)
        with open(os.path.join(site, "_rustique_startup.pth"), "w") as f:
            f.write(line + "\n")
        for extra in glob.glob(os.path.join(site, "_rustique_startup.py")):
            os.remove(extra)
    if other:
        with open(os.path.join(site, "zz_other.pth"), "w") as f:
            f.write("# another .pth\n")
    return py


def ir(py: str, env: dict[str, str]) -> int:
    with tempfile.TemporaryDirectory() as t:
        out = os.path.join(t, "cg")
        subprocess.run(["valgrind", "--tool=callgrind", f"--callgrind-out-file={out}", "-q", py, "-c", "pass"],
                       check=True, env=env)
        r = subprocess.run(["callgrind_annotate", out], capture_output=True, text=True).stdout
    return next(int(x.split()[0].replace(",", "")) for x in r.splitlines() if "PROGRAM TOTALS" in x)


def faults(py: str, env: dict[str, str]) -> float:
    vals = []
    for _ in range(5):
        r = subprocess.run(["perf", "stat", "-x,", "-e", "page-faults", py, "-c", "pass"], capture_output=True,
                           text=True, env=env)
        vals += [float(x.split(",")[0]) for x in r.stderr.splitlines() if "page-faults" in x]
    return statistics.median(vals)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--wheel", required=True)
    ap.add_argument("--python", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--wall", type=int, default=40)
    ap.add_argument("--pth-line-from", default="tests/test_accel_startup.py")
    a = ap.parse_args()
    line = pth_line(a.pth_line_from)
    os.makedirs(a.work, exist_ok=True)
    base = {k: v for k, v in os.environ.items() if not k.startswith(("RUSTIQUE", "VIRTUAL_ENV", "PYTHON"))}
    on = {**base, "RUSTIQUE": "1"}
    for other in (False, True):
        plain = venv(a.work, f"plain{'-other' if other else ''}", a.python, None, other, line)
        rq = venv(a.work, f"rq{'-other' if other else ''}", a.python, a.wheel, other, line)
        cells = {"plain": (plain, base), "rustique, unset": (rq, base), "rustique, RUSTIQUE=1": (rq, on)}
        check = subprocess.run([rq, "-c", "import sys; print('rustique._rustique' in sys.modules, 'rustique' in sys.modules)"],
                               capture_output=True, text=True, env=on)
        print(f"== {'venv with another .pth' if other else 'bare venv'} (RUSTIQUE=1 loads ext/package: {check.stdout.strip()} {check.stderr.strip()})")
        irs = {k: ir(py, env) for k, (py, env) in cells.items()}
        t: dict[str, list[float]] = {k: [] for k in cells}
        for _ in range(a.wall):
            for k, (py, env) in cells.items():
                s = time.perf_counter()
                subprocess.run([py, "-c", "pass"], check=True, env=env)
                t[k].append((time.perf_counter() - s) * 1e3)
        b_ir, b_wall = irs["plain"], statistics.median(t["plain"])
        for k in cells:
            w = statistics.median(t[k])
            q = statistics.quantiles(t[k], n=10)
            pf = faults(*cells[k])
            print(f"  {k:24s} Ir {irs[k]:>12,d} ({irs[k] / b_ir:.3f})   wall {w:6.2f} ms ({q[0]:.2f}–{q[-1]:.2f}) ({w / b_wall:.3f})   faults {pf:.0f}")


if __name__ == "__main__":
    main()
