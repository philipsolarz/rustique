"""PGO training run for an instrumented _rqeval build: install the copied loop as the
eval-frame function (so every frame runs in it), then run workloads and the micro cells.

    python benches/interp/rqeval/pgo_train.py MOD workload.py [workload.py ...]
"""
import os, sys, time
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
sys.path.insert(0, "/work/w2-interp/rqeval")
mod = __import__(sys.argv[1])
from opprof import load_workload
mod.install()
for w in sys.argv[2:]:
    m = load_workload(w)
    st = m.setup()
    t0 = time.perf_counter()
    while time.perf_counter() - t0 < 3:
        m.run(st)
    print("trained", os.path.basename(w), flush=True)
# micro cells
src = open(os.path.join(HERE, "..", "hook_cg.py")).read()
defs = src.split("DEFS = '''", 1)[1].split("'''", 1)[0]
ns = {}
exec(defs, ns)
fib, gen, coro, catch, catch_local, inner, P, O = (ns[k] for k in ("fib", "gen", "coro", "catch", "catch_local", "inner", "P", "O"))
o = O()
for _ in range(2000):
    fib(10); sum(gen(100)); catch(); catch_local(); inner(1); P(1, 2); o.m(1)
    [x * x for x in range(100)]
    c = coro(20)
    try:
        c.send(None)
    except StopIteration:
        pass
mod.uninstall()
print("done")
