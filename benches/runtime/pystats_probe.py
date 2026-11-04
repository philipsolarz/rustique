#!/usr/bin/env python3
"""Allocation / freelist / GC-visit counts for one steady-state run() (needs a Py_STATS build).

    <pystats-python> benches/runtime/pystats_probe.py one <family/name> -o out.json [--policy SPEC]
    python benches/runtime/pystats_probe.py table DIR [--md]

`one`: setup(), one warm-up run(), then sys._stats_clear(); sys._stats_on(); run();
sys._stats_off(); sys._stats_dump() and parses the dump (CPython writes it to /tmp/py_stats/
under a random name: run one probe at a time). Reports object allocations (PyObject_Malloc
+ PyMem_Malloc calls, by size class), frees, freelist pops/pushes (all freelists summed:
Include/internal/pycore_freelist.h), increfs/decrefs, and per-generation GC collections,
object visits (tp_traverse visits) and objects collected (Include/cpython/pystats.h).

The CPython build: /work/cpython/v3.15.1 configured with --enable-pystats --with-mimalloc
(-O2, no PGO): counts are exact, times are not representative.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

KEYS = {
    "Object allocations from freelist": "from_freelist",
    "Object frees to freelist": "to_freelist",
    "Object allocations": "allocations",
    "Object allocations to 512 bytes": "alloc512",
    "Object allocations to 4 kbytes": "alloc4k",
    "Object allocations over 4 kbytes": "alloc_big",
    "Object frees": "frees",
    "Object mortal increfs": "increfs",
    "Object mortal decrefs": "decrefs",
    "Object interpreter mortal increfs": "interp_increfs",
    "Object interpreter mortal decrefs": "interp_decrefs",
    "Object inline values": "inline_values",
}
GC_RE = re.compile(r"^GC\[(\d)\] (collections|object visits|objects collected): (\d+)$")


def parse_dump(path):
    out, gc = {}, {}
    with open(path) as f:
        for line in f:
            line = line.rstrip("\n")
            k, sep, v = line.rpartition(": ")
            if sep and k in KEYS:
                out[KEYS[k]] = int(v)
                continue
            m = GC_RE.match(line)
            if m:
                gc.setdefault(m.group(1), {})[m.group(2).replace(" ", "_")] = int(m.group(3))
    out["gc"] = gc
    return out


def cmd_one(a):
    import gcpolicy
    import gcprobe

    pol = gcpolicy.make(a.policy)
    if a.enable:
        import rustique

        rustique.enable()
    pol.install()
    mod = gcprobe.load_workload(a.workload)
    state = mod.setup()
    pol.after_setup()
    mod.run(state)
    pol.after_warmup()
    before = set(glob.glob("/tmp/py_stats/*"))
    sys._stats_clear()
    sys._stats_on()
    mod.run(state)
    sys._stats_off()
    sys._stats_dump()
    new = sorted(set(glob.glob("/tmp/py_stats/*")) - before, key=os.path.getmtime)
    res = parse_dump(new[-1]) if new else {}
    for p in new:
        os.unlink(p)
    res.update(workload=a.workload, policy=a.policy, enabled=a.enable)
    with open(a.o, "w") as f:
        json.dump(res, f)


def cmd_table(a):
    rows = []
    for f in sorted(glob.glob(os.path.join(a.dir, "*.json"))):
        d = json.load(open(f))
        if "allocations" not in d and os.path.exists(f + ".err"):
            # no /tmp/py_stats directory: CPython printed the dump to stderr
            d.update(parse_dump(f + ".err"))
        if "allocations" not in d:
            continue
        allocs = d["allocations"]
        fl = d.get("from_freelist", 0)
        visits = sum(g.get("object_visits", 0) for g in d["gc"].values())
        rows.append((d["workload"], allocs, fl, fl / (fl + allocs) if fl + allocs else 0,
                     d.get("alloc_big", 0) + d.get("alloc4k", 0), d.get("frees", 0),
                     d.get("increfs", 0) + d.get("interp_increfs", 0), visits,
                     sum(g.get("objects_collected", 0) for g in d["gc"].values())))
    print("workload | allocations | freelist pops | freelist share of allocs % | allocs > 512 B | "
          "frees | increfs | GC object visits | GC collected")
    print("---|---|---|---|---|---|---|---|---")
    for r in rows:
        print(f"{r[0]} | {r[1]:,} | {r[2]:,} | {100 * r[3]:.1f} | {r[4]:,} | {r[5]:,} | {r[6]:,} | "
              f"{r[7]:,} | {r[8]:,}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    o = sub.add_parser("one")
    o.add_argument("workload")
    o.add_argument("-o", required=True)
    o.add_argument("--policy", default="default")
    o.add_argument("--enable", action="store_true", help="rustique.enable() first")
    t = sub.add_parser("table")
    t.add_argument("dir")
    a = ap.parse_args()
    {"one": cmd_one, "table": cmd_table}[a.cmd](a)


if __name__ == "__main__":
    main()
