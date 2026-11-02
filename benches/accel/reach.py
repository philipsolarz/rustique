"""Reach per patch release (target T10): how many targets install on each interpreter, and why
the others refuse. Every target module is imported, everything enabled and validated eagerly,
in a fresh process per interpreter, with the default settings (no opt-in).

    python benches/accel/reach.py PYTHON [PYTHON ...] [--json out.json]
"""

from __future__ import annotations

import argparse
import collections
import json
import os
import subprocess
import sys

PROBE = r"""
import importlib, json
from rustique import _rustique
A = _rustique._accel
A._set_validation("eager")
for _, g, m in A.targets():
    if m and not g.startswith("_"):
        try:
            importlib.import_module(m)
        except ImportError:
            pass
A.enable()
A.validate()
st = A.status()
print(json.dumps({"patch": st["process"]["patch"], "refusal": A.process_refusal(),
                  "targets": {t["name"]: [t["state"], t["refusal"], t["reason"], t["tier"]] for t in st["targets"]}}))
"""


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("python", nargs="+")
    ap.add_argument("--json")
    a = ap.parse_args()
    env = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    out = {}
    for py in a.python:
        r = subprocess.run([py, "-c", PROBE], capture_output=True, text=True, env=env)
        if r.returncode:
            print(f"{py}: failed: {r.stderr[-500:]}")
            continue
        d = json.loads(r.stdout.strip().splitlines()[-1])
        default = {n: v for n, v in d["targets"].items() if v[3] is None}
        installed = sum(1 for v in default.values() if v[0] == "installed")
        why = collections.Counter(v[1] for v in default.values() if v[0] != "installed")
        print(f"{d['patch']:10s} installed {installed:3d} of {len(default)}  refused {dict(why)}"
              + (f"  process: {d['refusal']}" if d["refusal"] else ""))
        for n, v in sorted(default.items()):
            if v[0] != "installed":
                print(f"    {n}: {v[1]}: {(v[2] or '')[:110]}")
        out[d["patch"]] = {"installed": installed, "of": len(default), "refused": dict(why),
                           "detail": {n: v for n, v in default.items() if v[0] != "installed"}}
    if a.json:
        with open(a.json, "w") as f:
            json.dump(out, f, indent=1)


if __name__ == "__main__":
    main()
