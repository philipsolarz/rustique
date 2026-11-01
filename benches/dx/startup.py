"""Startup cost of Rustique's startup switch (docs/targets/dx.md §1).

Measures a whole ``python -c pass`` (or ``-c CODE``) process in several fresh venvs that
differ only in what sits in site-packages:

    python benches/dx/startup.py synthetic --python PY [--python PY2] [-r 300]
        venvs built here: empty / .pth (inline env check) / .pth importing the hook /
        .start entry point / .pth + .start (what the wheel ships)
    python benches/dx/startup.py venvs NAME=VENV [NAME=VENV ...] [--env K=V] [-r 300]
        existing venvs (e.g. a fresh venv vs the same with the wheel `pip install`ed)
    python benches/dx/startup.py cells PLAIN_VENV RUSTIQUE_VENV [-r 300]
        the startup cells of docs/targets/dx.md §1.4, all against `python -c pass` in the
        plain venv: installed + unset, RUSTIQUE=1, `import rustique`,
        `import rustique; rustique.enable()`

Each variant gets: callgrind total Ir of the whole process (median of ``--ir-runs``,
PYTHONHASHSEED=0), and wall time from ``-r`` interleaved rounds (each round runs every
variant once; median and p10–p90). Ratios are against the first variant. Output: a table,
and JSON with ``-o``.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import statistics
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
PYSRC = os.path.join(ROOT, "python")

# A stand-in module for the variants that import one (the shipped line imports `rustique`,
# which is not installed in these venvs: with RUSTIQUE unset it is never imported).
HOOK = "import os, sys\n\ndef start():\n    pass\n"
PTH = open(os.path.join(PYSRC, "_rustique_startup.pth")).read()
# Not shipped (docs/targets/dx.md §1): on 3.15.1 a .start file costs 1.7x `python -c pass`.
START = "_rustique_startup:start\n"

SYNTHETIC = {
    "empty": {},
    # decomposition: any .pth file costs the `utf-8-sig` codec import; an import line its compile
    "pth-comment": {"_rustique_startup.pth": "# nothing\n"},
    "pth-import-os": {"_rustique_startup.pth": "import os\n"},
    "pth-inline": {"_rustique_startup.pth": PTH, "_rustique_startup.py": HOOK},
    "pth-import": {"_rustique_startup.pth": "import _rustique_startup; _rustique_startup.start()\n",
                   "_rustique_startup.py": HOOK},
    "start": {"_rustique_startup.start": START, "_rustique_startup.py": HOOK},
    "pth+start": {"_rustique_startup.pth": PTH, "_rustique_startup.start": START, "_rustique_startup.py": HOOK},
    # cheaper import lines (RUSTIQUE unset: only the compile and the env test run)
    "line-noattr": {"_rustique_startup.pth": "import os; 'RUSTIQUE' in os.environ and __import__('_rustique_startup')\n"},
    "line-get": {"_rustique_startup.pth": "import os; os.environ.get('RUSTIQUE') and __import__('_rustique_startup')\n"},
    "line-posix": {"_rustique_startup.pth": "import posix; b'RUSTIQUE' in posix.environ and __import__('_rustique_startup')\n"},
    "line-short": {"_rustique_startup.pth": "import os;'RUSTIQUE'in os.environ and __import__('_rustique_startup')\n"},
    # a venv that already has a .pth (setuptools, an editable install, virtualenv's _virtualenv.pth)
    "other": {"aaa_other.pth": "# stands for any other .pth\n"},
    "other+pth": {"aaa_other.pth": "# stands for any other .pth\n", "_rustique_startup.pth": PTH,
                  "_rustique_startup.py": HOOK},
}


def site_packages(py: str) -> str:
    return subprocess.check_output([py, "-c", "import sysconfig; print(sysconfig.get_path('purelib'))"],
                                   text=True).strip()


def make_venv(base_py: str, path: str, files: dict[str, str]) -> str:
    subprocess.check_call([base_py, "-m", "venv", "--without-pip", path])
    py = os.path.join(path, "bin", "python")
    sp = site_packages(py)
    for name, content in files.items():
        with open(os.path.join(sp, name), "w") as f:
            f.write(content)
    # Warm the .pyc of the hook so every variant starts from a cached state.
    subprocess.check_call([py, "-c", "pass"], env=env_for({}))
    if "_rustique_startup.py" in files:
        subprocess.check_call([py, "-c", "import _rustique_startup"], env=env_for({}))
    return py


def env_for(extra: dict[str, str]) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if not k.startswith(("RUSTIQUE", "PYTHON", "VIRTUAL_ENV"))}
    env["PYTHONHASHSEED"] = "0"
    env.update(extra)
    return env


def callgrind_ir(py: str, code: str, env: dict[str, str]) -> int:
    out = subprocess.run(["valgrind", "--tool=callgrind", "--callgrind-out-file=/dev/null", py, "-c", code],
                         env=env, capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit(f"{py} failed under callgrind:\n{out.stderr[-2000:]}")
    m = re.search(r"Collected : (\d+)", out.stderr)
    if not m:
        raise SystemExit(out.stderr[-2000:])
    return int(m.group(1))


def wall(variants: list[tuple], code: str, rounds: int) -> dict[str, list[float]]:
    times: dict[str, list[float]] = {v[0]: [] for v in variants}
    for _ in range(rounds):
        for v in variants:
            name, py, env = v[:3]
            c = v[3] if len(v) > 3 else code
            t0 = time.perf_counter_ns()
            pid = os.posix_spawn(py, [py, "-c", c], env)
            _, status = os.waitpid(pid, 0)
            t1 = time.perf_counter_ns()
            if status != 0:
                raise SystemExit(f"{name}: exit status {status}")
            times[name].append((t1 - t0) / 1e6)
    return times


def pct(xs: list[float], q: float) -> float:
    s = sorted(xs)
    return s[min(len(s) - 1, int(q * len(s)))]


def measure(variants: list[tuple], code: str, rounds: int, ir_runs: int) -> list[dict]:
    rows = []
    for v in variants:
        name, py, env = v[:3]
        c = v[3] if len(v) > 3 else code
        irs = [callgrind_ir(py, c, env) for _ in range(ir_runs)]
        rows.append({"variant": name, "python": py, "code": c, "ir": int(statistics.median(irs)), "ir_runs": irs})
    times = wall(variants, code, rounds)
    for row in rows:
        t = times[row["variant"]]
        row.update(ms_median=statistics.median(t), ms_p10=pct(t, 0.1), ms_p90=pct(t, 0.9))
    base = rows[0]
    for row in rows:
        row["ir_ratio"] = row["ir"] / base["ir"]
        row["ms_ratio"] = row["ms_median"] / base["ms_median"]
    return rows


def show(title: str, rows: list[dict]) -> None:
    print(f"\n{title}")
    print(f"{'variant':<26} {'Ir':>12} {'Ir ratio':>9} {'ms median (p10-p90)':>24} {'ms ratio':>9}")
    for r in rows:
        print(f"{r['variant']:<26} {r['ir']:>12,} {r['ir_ratio']:>9.4f} "
              f"{r['ms_median']:>9.2f} ({r['ms_p10']:.2f}-{r['ms_p90']:.2f}) {r['ms_ratio']:>9.4f}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("synthetic")
    s.add_argument("--python", action="append", required=True)
    s.add_argument("--variants", default=",".join(SYNTHETIC))
    v = sub.add_parser("venvs")
    v.add_argument("pairs", nargs="+", metavar="NAME=VENV")
    c = sub.add_parser("cells")
    c.add_argument("plain")
    c.add_argument("rustique")
    for p in (s, v, c):
        p.add_argument("--env", action="append", default=[], metavar="K=V", help="extra env for every variant")
        p.add_argument("-c", dest="code", default="pass")
        p.add_argument("-r", "--rounds", type=int, default=300)
        p.add_argument("--ir-runs", type=int, default=3)
        p.add_argument("-o", "--output")
    args = ap.parse_args()
    extra = dict(kv.split("=", 1) for kv in args.env)
    results = {}
    if args.cmd == "synthetic":
        tmp = tempfile.mkdtemp(prefix="rq-startup-")
        try:
            for base in args.python:
                variants = []
                for name in args.variants.split(","):
                    py = make_venv(base, os.path.join(tmp, f"{os.path.basename(base)}-{name}"), SYNTHETIC[name])
                    variants.append((name, py, env_for(extra)))
                ver = subprocess.check_output([base, "-c", "import sys; print(sys.version.split()[0], sys.abiflags)"],
                                              text=True).strip()
                rows = measure(variants, args.code, args.rounds, args.ir_runs)
                show(f"{base} ({ver}) -c {args.code!r}", rows)
                results[f"{base} {ver}"] = rows
        finally:
            shutil.rmtree(tmp)
    elif args.cmd == "cells":
        plain, rq = (os.path.join(v, "bin", "python") for v in (args.plain, args.rustique))
        env = env_for(extra)
        variants = [
            ("plain: pass", plain, env, "pass"),
            ("installed, unset: pass", rq, env, "pass"),
            ("RUSTIQUE=1: pass", rq, {**env, "RUSTIQUE": "1"}, "pass"),
            ("import rustique", rq, env, "import rustique"),
            ("import + enable()", rq, env, "import rustique; rustique.enable()"),
        ]
        rows = measure(variants, "pass", args.rounds, args.ir_runs)
        show("startup cells (ratios vs plain `python -c pass`)", rows)
        results["cells"] = rows
    else:
        variants = []
        for pair in args.pairs:
            name, venv = pair.split("=", 1)
            variants.append((name, os.path.join(venv, "bin", "python"), env_for(extra)))
        rows = measure(variants, args.code, args.rounds, args.ir_runs)
        show(f"-c {args.code!r} env {extra}", rows)
        results["venvs"] = rows
    if args.output:
        with open(args.output, "w") as f:
            json.dump(results, f, indent=1)


if __name__ == "__main__":
    main()
