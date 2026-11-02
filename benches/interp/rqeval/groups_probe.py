"""Which override groups suspended themselves on a workload (net-benefit suspension).

    RQMOD=_rqeval_acKq python benches/interp/rqeval/groups_probe.py <workload.py>
"""
import os, sys
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE); sys.path.insert(0, os.path.join(HERE, ".."))
import rqpatch, _opcode_metadata as om
from opprof import load_workload
mod_name = os.environ.get("RQMOD", "_rqeval_acKq")
ops = ["CALL_ISINSTANCE", "CALL_NON_PY_GENERAL", "CALL_KW_NON_PY", "LOAD_ATTR"] + list(om._specializations["LOAD_ATTR"])
mod, t, done = rqpatch.swap("/work/w2-interp/rqeval", mod_name, ops)
w = load_workload(sys.argv[1]); st = w.setup(); w.run(st)
print("after setup+1 run:", mod.ac_stats())
w.run(st); w.run(st)
print("after 3 runs:     ", mod.ac_stats(), "(hits, fill-misses, attr-suspended, (attr, setattr, kw) suspended)")
