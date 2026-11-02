"""Hit/miss counts of the attribute-cache override on a workload, against the number of
generic LOAD_ATTR executions (attribute probe build).

    python benches/interp/rqeval/ac_hits.py <workload.py> [--family]
"""
import argparse, os, sys
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE); sys.path.insert(0, os.path.join(HERE, ".."))
import rqpatch, _opcode_metadata as om
from opprof import load_workload

ap = argparse.ArgumentParser(); ap.add_argument("workload"); ap.add_argument("--family", action="store_true")
a = ap.parse_args()
ops = ["LOAD_ATTR"] + (list(om._specializations["LOAD_ATTR"]) if a.family else [])
w = load_workload(a.workload); st = w.setup(); w.run(st)
mod, t, done = rqpatch.swap("/work/w2-interp/rqeval", os.environ.get("RQMOD", "_rqeval_acC"), ops)
h0, m0, _s0 = mod.ac_stats(); w.run(st); h1, m1, susp = mod.ac_stats()
rqpatch.unswap(mod, t, done)
pm, pt, pd = rqpatch.swap("/work/w2-interp/rqeval", "_rqeval_attrprobe", ops)
w.run(st)
rows, lost = pm.probe_dump()
rqpatch.unswap(pm, pt, pd)
gen = sum(r[3] for r in rows)
print(f"{os.path.basename(a.workload)[:-3]:20s} family={a.family} generic LOAD_ATTR execs/run={gen:,} "
      f"cache hits={h1 - h0:,} ({(h1 - h0) / max(gen, 1):.0%}) fill-misses={m1 - m0:,} suspended={susp}")
