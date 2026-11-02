"""Install-time costs of the groups (DECISIONS §5 item 4, "accepted overages"), callgrind
regions in fresh processes, for one or more interpreters (venvs with different builds).

    python benches/startup/install_costs.py [--python VENV/bin/python ...] [--only json,copy] [-o out.json]

Cells (Ir of the statement only; GC disabled in the region so that collection timing,
which moves by ±0.6M Ir between cells, does not mask install work):
    import M plain        `import M` after `import rustique` (nothing enabled)
    import M lazy         `import M` after `_accel.enable()` of every group (M's targets are
                          pending: the lazy install runs inside the import)
    enable G (imported)   `_accel.enable(only=[G])` (quiet: no status) with G's module imported
    enable all (fresh)    `_accel.enable()` after `import rustique` (= the RUSTIQUE=1 work)
Budgets (DECISIONS §5 item 3): lazy <= max(5% of the import, 0.1M); enable <= 0.1M.

The worker is this file run under valgrind with `_worker SETUP STMT`; it imports nothing
but `sys`, `gc` and the client-request helper of benches/targets/_callgrind.py (built
there on first use), so no measured module is pre-imported.
"""

from __future__ import annotations

import sys

if len(sys.argv) > 1 and sys.argv[1] == "_worker":
    import gc

    sys.path.insert(0, sys.argv[2])
    import _rq_callgrind_v1 as h  # type: ignore[import-not-found]

    ns: dict = {"__name__": "__bench__"}
    exec(sys.argv[3], ns)
    code = compile(sys.argv[4], "<region>", "exec")
    gc.collect()
    gc.disable()
    h.start()
    exec(code, ns)
    h.dump("r")
    h.stop()
    raise SystemExit(0)

import argparse  # noqa: E402
import glob  # noqa: E402
import json  # noqa: E402
import os  # noqa: E402
import subprocess  # noqa: E402
import tempfile  # noqa: E402

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "targets"))
import _callgrind  # noqa: E402

IMPORTS = ["json", "copy", "logging", "random", "pathlib", "fnmatch", "textwrap", "statistics",
           "urllib.parse", "dataclasses", "datetime"]
ENABLES = [("logging", "logging"), ("copy", "copy"), ("posixpath", "posixpath"), ("fnmatch", "fnmatch"),
           ("pathlib", "pathlib"), ("json", "json"), ("random", "random"), ("dataclasses", "dataclasses")]
ACCEL = "import rustique; A = rustique._rustique._accel"


def region(py: str, setup: str, stmt: str) -> int:
    cache = os.path.dirname(_callgrind.helper().__file__)
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        env = {k: v for k, v in os.environ.items() if not k.startswith(("RUSTIQUE", "PYTHON"))}
        env["PYTHONHASHSEED"] = "0"
        r = subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "-q",
                            f"--callgrind-out-file={out}", py, os.path.abspath(__file__), "_worker", cache, setup,
                            stmt], env=env, capture_output=True, text=True)
        if r.returncode:
            raise RuntimeError(r.stderr[-2000:])
        for fn in glob.glob(out + "*"):
            with open(fn) as f:
                text = f.read()
            if "Client Request: r" in text:
                for line in text.splitlines():
                    if line.startswith(("totals:", "summary:")):
                        return int(line.split()[1])
    raise RuntimeError("no region dump")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--python", action="append", default=[])
    ap.add_argument("--only", default="")
    ap.add_argument("-o", "--output")
    a = ap.parse_args()
    pys = a.python or [sys.executable]
    only = set(a.only.split(",")) if a.only else None
    res: dict[str, dict[str, int]] = {}
    for py in pys:
        r: dict[str, int] = {}
        for mod in IMPORTS:
            if only and mod.split(".")[0] not in only:
                continue
            r[f"import {mod} plain"] = region(py, ACCEL, f"import {mod}")
            r[f"import {mod} lazy"] = region(py, ACCEL + "; A.enable()", f"import {mod}")
        for group, mod in ENABLES:
            if only and group not in only:
                continue
            r[f"enable {group} (imported)"] = region(py, f"{ACCEL}; import {mod}", f"A.enable(only=[{group!r}])")
        if not only:
            r["enable all (fresh)"] = region(py, ACCEL, "A.enable()")
        res[py] = r
        print(f"\n{py}")
        for k, v in r.items():
            extra = ""
            if k.endswith(" lazy"):
                p = r[k[: -len(" lazy")] + " plain"]
                extra = f"  lazy/plain {v / p:.3f} (+{v - p:,})"
            print(f"  {k:<32} {v:>12,}{extra}", flush=True)
    if a.output:
        with open(a.output, "w") as f:
            json.dump(res, f, indent=1)


if __name__ == "__main__":
    main()
