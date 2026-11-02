"""Wall-time probe: cost of PEP 523 hooks on 3.15 (no hook / pass-through hook / generated
interpreter copy with specialization allowed), for calls, methods, generators and loops.

    python benches/interp/hook_probe.py [none|record|interp] [reps]

`interp` installs `_testinternalcapi.set_eval_frame_interp()`: CPython's own test of a
PEP 523 hook that is a generated copy of the 3.15 interpreter (Modules/_testinternalcapi/
interpreter.c) with `_PyInterpreterState_SetEvalFrameAllowSpecialization(interp, 1)`.
`record` installs `set_eval_frame_record(list)`: a hook that records each function and calls
`_PyEval_EvalFrameDefault` (specialization not allowed; the classic PEP 523 cost).
"""
import sys, time

def fib(n):
    return n if n < 2 else fib(n - 1) + fib(n - 2)

class O:
    def m(self, x):
        return x

def calls(n):
    o = O()
    for i in range(n):
        o.m(i)

def gen(n):
    for i in range(n):
        yield i

def gens(n):
    s = 0
    for x in gen(n):
        s += x
    return s

def loop(n):
    s = 0
    for i in range(n):
        s += i * i
    return s

CELLS = [("fib(25)", lambda: fib(25)), ("o.m(i) x1e6", lambda: calls(1_000_000)),
         ("gen 1e6 items", lambda: gens(1_000_000)), ("loop 3e6", lambda: loop(3_000_000))]

def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "none"
    reps = int(sys.argv[2]) if len(sys.argv) > 2 else 7
    import _testinternalcapi as t
    rec = []
    if mode == "interp":
        t.set_eval_frame_interp()
    elif mode == "record":
        t.set_eval_frame_record(rec)
    out = {}
    for name, fn in CELLS:
        fn()
        ts = []
        for _ in range(reps):
            t0 = time.perf_counter(); fn(); ts.append(time.perf_counter() - t0)
            rec.clear()
        ts.sort()
        out[name] = ts[len(ts) // 2]
    t.set_eval_frame_default()
    print(mode, {k: round(v * 1000, 2) for k, v in out.items()}, "spec_enabled_during" )

if __name__ == "__main__":
    main()
