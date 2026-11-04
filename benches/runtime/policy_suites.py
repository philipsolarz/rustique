#!/usr/bin/env python3
"""CPython's own test modules with and without a GC policy (eligibility criteria a and d).

    python benches/runtime/policy_suites.py --policy adaptive --modules test.test_gc,... \
        [--cpython-src $CPYTHON_SRC] [--json out.json] [--timeout 1800]

Each module runs in fresh subprocesses twice: plain, and with the policy installed (via
gcpolicy.make(spec).install()) *before* `test` is imported, as a launch-time switch would.
Outcomes per test id are compared (the recorder of tests/conformance/run_target_suites.py).
The child also reports whether the policy was still active at the end, and why it stepped
aside (a test module that tunes the GC itself must make it step aside, criterion d).
"""

from __future__ import annotations

import argparse
import importlib
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(ROOT, "tests", "conformance"))


def child(a):
    import run_target_suites as R

    pol = None
    if a.policy:
        import gcpolicy

        pol = gcpolicy.make(a.policy)
        pol.install()
    R.expose_test_package(a.cpython_src)
    from test import support

    support.verbose = False
    mod = importlib.import_module(a.child)
    tests = unittest.TestLoader().loadTestsFromModule(mod)
    runner = unittest.TextTestRunner(stream=open(os.devnull, "w"), verbosity=0, resultclass=R.Recorder)
    res = runner.run(tests)
    out = {"module": a.child, "outcomes": res.outcomes}
    if pol is not None:
        out["policy_report"] = pol.report()
    with open(a.json, "w") as f:
        json.dump(out, f)


def run(module, policy, a):
    fd, out = tempfile.mkstemp(prefix="pol-", suffix=".json")
    os.close(fd)
    cmd = [sys.executable, os.path.abspath(__file__), "--child", module, "--cpython-src", a.cpython_src,
           "--json", out] + (["--policy", policy] if policy else [])
    t = time.monotonic()
    try:
        p = subprocess.run(cmd, timeout=a.timeout, capture_output=True, text=True)
        with open(out) as f:
            d = json.load(f)
        d["status"] = "ran"
    except subprocess.TimeoutExpired:
        d = {"module": module, "status": "timeout", "outcomes": {}}
    except (OSError, ValueError):
        d = {"module": module, "status": f"crash ({p.returncode})", "outcomes": {}, "stderr": p.stderr[-2000:]}
    finally:
        try:
            os.remove(out)
        except OSError:
            pass
    d["seconds"] = round(time.monotonic() - t, 1)
    return d


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--child")
    ap.add_argument("--policy", default="")
    ap.add_argument("--modules", default="")
    ap.add_argument("--cpython-src", default=os.environ.get("CPYTHON_SRC", ""))
    ap.add_argument("--json")
    ap.add_argument("--timeout", type=int, default=1800)
    a = ap.parse_args()
    if a.child:
        return child(a)
    rep = {"python": sys.version, "policy": a.policy, "modules": {}}
    bad = 0
    for m in [x for x in a.modules.split(",") if x]:
        plain = run(m, "", a)
        pol = run(m, a.policy, a)
        po, eo = plain["outcomes"], pol["outcomes"]
        diffs = {t: [po.get(t, {}).get("outcome"), eo.get(t, {}).get("outcome"), eo.get(t, {}).get("detail", "")]
                 for t in set(po) | set(eo) if po.get(t, {}).get("outcome") != eo.get(t, {}).get("outcome")}
        status = (plain["status"], pol["status"])
        rep["modules"][m] = {"tests": len(po), "diffs": diffs, "status": status,
                             "policy_report": pol.get("policy_report"), "seconds": (plain["seconds"], pol["seconds"])}
        bad += len(diffs) + (status[0] != status[1])
        rp = pol.get("policy_report") or {}
        print(f"{m:40s} tests {len(po):5d}  diffs {len(diffs):3d}  status {status}  "
              f"policy active={rp.get('active')} stepped_aside={rp.get('stepped_aside')} fulls={rp.get('fulls')}",
              flush=True)
        for t, (x, y, det) in sorted(diffs.items())[:10]:
            print(f"    {t}: {x} -> {y}  {det[:200]}")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(rep, f, indent=1)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
