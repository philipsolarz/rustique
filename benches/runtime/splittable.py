#!/usr/bin/env python3
"""Table of allocsplit results: python benches/runtime/splittable.py DIR [DIR2 ...]

Per workload: alloc / dealloc / GC (visit, passes, free, other) shares of the measured run()s,
the GC share the probe's callbacks measured in the same process (cross-check), and the total
runtime-allocation layer. With several DIRs (e.g. 3.15.1, 3.15.1t, PYTHONMALLOC=mimalloc)
prints one block per DIR and a mean row each."""
import glob
import json
import os
import statistics as st
import sys


def rows(d):
    out = []
    for f in sorted(glob.glob(os.path.join(d, "*.json"))):
        j = json.load(open(f))
        s = j["shares"]
        gc = sum(v for k, v in s.items() if k.startswith("gc:") and k != "gc:track")
        out.append((j["workload"], s.get("alloc", 0), s.get("dealloc", 0), s.get("gc:visit", 0),
                    s.get("gc:passes", 0), s.get("gc:free", 0), s.get("gc:other", 0) + s.get("gc:track", 0),
                    gc, j["probe_gc_share"], st.median(j["walls"])))
    return out


for d in sys.argv[1:]:
    print(f"\n### {os.path.basename(d.rstrip('/'))}\n")
    print("workload | alloc % | dealloc % | GC visit % | GC passes % | GC free % | GC other % | GC total % | GC (callbacks) % | alloc+dealloc+GC % | run ms")
    print("---|---|---|---|---|---|---|---|---|---|---")
    rs = rows(d)
    for r in rs:
        print(f"{r[0]} | " + " | ".join(f"{100 * x:.1f}" for x in r[1:9]) + f" | {100 * (r[1] + r[2] + r[7]):.1f} | {1e3 * r[9]:.0f}")
    if rs:
        print("**mean** | " + " | ".join(f"{100 * st.mean(r[i] for r in rs):.1f}" for i in range(1, 9))
              + f" | {100 * st.mean(r[1] + r[2] + r[7] for r in rs):.1f} | ")
