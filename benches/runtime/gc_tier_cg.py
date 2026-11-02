#!/usr/bin/env python3
"""Callgrind cells of the gc tier (docs/targets/gc.md §7): what a hooked `gc` call, an explicit
collection and an automatic young collection cost with the tier, against plain CPython.

    python benches/runtime/gc_tier_cg.py [--json out.json]

Each cell runs in fresh processes (benches/targets/_callgrind.py: region-instrumented,
per-execution instructions from the difference of n and 2n iterations). The tier is
requested in code (`_accel.enable(["gc"], tiers=["gc"])`) and enabled alone at the end of the
setup. Hooked calls
are measured in steady state: the first tuning call makes the policy step aside (one-time),
so the cells show what every later call costs; `steps aside` measures that first call.
"""

from __future__ import annotations

import argparse
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "targets"))

from _callgrind import per_call, region, table  # noqa: E402

# The tier is requested in code (benches/targets/_callgrind.py's `enable=` reads no environment).
TIER_ON = "\nimport rustique\nrustique._accel.enable(['gc'], tiers=['gc'])\n"
HEAP = "import gc\nheap = [(i, [i]) for i in range(50_000)]\n"

CELLS = [
    # hooked functions (steady state)
    ("gc.enable()", "import gc"),
    ("gc.disable(); gc.enable()", "import gc"),
    ("gc.set_threshold(2000, 10, 10)", "import gc"),
    ("gc.set_debug(0)", "import gc"),
    ("gc.freeze(); gc.unfreeze()", "import gc"),
    # not hooked (identical by construction)
    ("gc.get_threshold()", "import gc"),
    ("gc.isenabled()", "import gc"),
    # explicit collections: the observer and the forwarder run twice per collection
    ("gc.collect(0)", "import gc"),
    ("gc.collect(1)", "import gc"),
    ("gc.collect(0)", "import gc\ncb = lambda p, i: None\ngc.callbacks.append(cb)"),
    ("gc.collect()", "import gc"),
    ("gc.collect()", HEAP),
    # automatic young collections (one per execution, ~2000 allocations each)
    ("x = [[] for _ in range(2001)]", "import gc"),
    ("x = [[] for _ in range(2001)]", HEAP),
]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json")
    ap.add_argument("-n", type=int, default=100)
    a = ap.parse_args()
    rows: dict[str, dict[str, float]] = {}
    for stmt, setup in CELLS:
        label = stmt + ("  [heap 100k]" if setup == HEAP else "  [+1 callback]" if "callbacks" in setup else
                        "  [bare heap]" if stmt == "gc.collect()" else "")
        n = 20 if "2001" in stmt or stmt == "gc.collect()" else a.n
        rows[label] = {
            "plain": per_call(stmt, setup, n=n),
            "tier": per_call(stmt, setup + TIER_ON, n=n),
        }
        print(table({label: rows[label]}), flush=True)
    # one-time costs
    first = "gc.set_threshold(2000, 10, 10)"
    rows["steps aside (first tuning call)"] = {
        "plain": region(first, "import gc"),
        "tier": region(first, "import gc" + TIER_ON),
    }
    print(table({"steps aside (first tuning call)": rows["steps aside (first tuning call)"]}), flush=True)
    # install: the gc tier enabled alone (gc already imported, the common case) vs the same
    # call that installs nothing (tier not requested)
    rows["install: enable(['gc'], tiers=['gc'])"] = {
        "plain": region("A.enable(['gc'])", "import gc, rustique\nA = rustique._accel"),
        "tier": region("A.enable(['gc'], tiers=['gc'])", "import gc, rustique\nA = rustique._accel"),
    }
    print(table({"install": rows["install: enable(['gc'], tiers=['gc'])"]}), flush=True)
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"python": sys.version, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main()
