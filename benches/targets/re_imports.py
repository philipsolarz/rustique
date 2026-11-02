"""Whole-process instructions of real programs' startup with and without the `re` group
(docs/targets/re.md): callgrind `--instr-atstart=yes`, PYTHONHASHSEED=0, one fresh process
per (program, variant):

    plain   RUSTIQUE unset
    no-re   RUSTIQUE=1 RUSTIQUE_EXCLUDE=re      (every other group)
    rq      RUSTIQUE=1                          (with the re group)

The re group's effect is rq / no-re. Programs: the 12 programs of
`benches/startup/import_pools.py` (core set), the 21 `import_startup` imports, and the
brief's imports (`yaml`, `requests`, `email`, `pip`, `docutils`, `pygments`). Also counts
`re._compiler.compile` calls per program (a plain run with a counting wrapper).

    python benches/targets/re_imports.py --python VENV/bin/python --workdir W -o out.json [-j 4] [--only a,b]
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "startup"))
import import_pools  # noqa: E402

EXTRA = {
    "imp:yaml": ["-c", "import yaml"],
    "imp:requests": ["-c", "import requests"],
    "imp:email": ["-c", "import email.message, email.parser, email.policy, email.utils, email.headerregistry"],
    "imp:pip": ["-c", "import pip._internal.cli.main"],
    "imp:docutils": ["-c", "import docutils.core, docutils.parsers.rst"],
    "imp:pygments": ["-c", "import pygments.lexers.python, pygments.formatters.html; import pygments"],
}

COUNT = ("import re._compiler as c, atexit, sys\n"
         "n = [0]\n_o = c.compile\n"
         "def compile(p, flags=0):\n    n[0] += 1\n    return _o(p, flags)\n"
         "c.compile = compile\n"
         "atexit.register(lambda: sys.stderr.write('RQ_RE_COMPILES %d\\n' % n[0]))\n")


def callgrind(py: str, argv: list[str], cwd: str, env: dict[str, str]) -> int:
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        subprocess.run(["valgrind", "--tool=callgrind", "-q", f"--callgrind-out-file={out}", "--dump-instr=no",
                        "--instr-atstart=yes", py, *argv], cwd=cwd, env=env, capture_output=True, check=False)
        with open(out) as f:
            for line in f:
                if line.startswith(("summary:", "totals:")):
                    return int(line.split()[1])
    raise RuntimeError("no callgrind totals")


def compiles(py: str, argv: list[str], cwd: str, env: dict[str, str], workdir: str) -> int:
    """`re._compiler.compile` calls of the program (sitecustomize wrapper, plain run)."""
    sc = os.path.join(workdir, "count_sc")
    os.makedirs(sc, exist_ok=True)
    with open(os.path.join(sc, "sitecustomize.py"), "w") as f:
        f.write(COUNT)
    e = dict(env, PYTHONPATH=sc)
    p = subprocess.run([py, *argv], cwd=cwd, env=e, capture_output=True, text=True, check=False)
    for line in p.stderr.splitlines():
        if line.startswith("RQ_RE_COMPILES"):
            return int(line.split()[1])
    return -1


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", required=True)
    ap.add_argument("--workdir", required=True)
    ap.add_argument("-o")
    ap.add_argument("-j", type=int, default=4)
    ap.add_argument("--only")
    a = ap.parse_args()
    progs = {**import_pools.programs(a.workdir, "all"), **EXTRA}
    if a.only:
        keep = a.only.split(",")
        progs = {k: v for k, v in progs.items() if any(x in k for x in keep)}
    base = import_pools.env_for()
    variants = {
        "plain": base,
        "no-re": dict(base, RUSTIQUE="1", RUSTIQUE_EXCLUDE="re"),
        "rq": dict(base, RUSTIQUE="1"),
    }
    cwd = os.path.join(a.workdir, "empty")
    jobs = [(name, v) for name in progs for v in variants]
    with ThreadPoolExecutor(a.j) as ex:
        irs = dict(zip(jobs, ex.map(lambda j: callgrind(a.python, progs[j[0]], cwd, variants[j[1]]), jobs)))
        ncomp = dict(zip(progs, ex.map(lambda n: compiles(a.python, progs[n], cwd, base, a.workdir), progs)))
    res = {}
    print(f"{'program':28s} {'compiles':>8s} {'plain':>13s} {'no-re':>13s} {'rq':>13s}  rq/no-re  rq/plain")
    for name in progs:
        r = {v: irs[(name, v)] for v in variants}
        r["compiles"] = ncomp[name]
        r["re_effect"] = r["rq"] / r["no-re"]
        r["vs_plain"] = r["rq"] / r["plain"]
        res[name] = r
        print(f"{name:28s} {r['compiles']:8d} {r['plain']:13,d} {r['no-re']:13,d} {r['rq']:13,d}  "
              f"{r['re_effect']:.3f}    {r['vs_plain']:.3f}")
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": a.python, "programs": res}, f, indent=1)


if __name__ == "__main__":
    main()
