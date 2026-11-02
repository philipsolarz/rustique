#!/usr/bin/env python3
"""Per-opcode native profile of a workload on a tail-calling CPython (3.15 PBS builds).

In the tail-calling interpreter every opcode handler is its own function
(`_TAIL_CALL_<OPNAME>`), and python-build-standalone keeps frame pointers, so a plain
`perf record -g` (no `-X perf`: the perf trampoline is a PEP 523 hook that turns off
inlined Python-to-Python calls) attributes every native sample to the opcode handler
that is executing, and every C function to the handler that called it.

    python benches/interp/opprof.py record <workload.py> -o prof.json [--seconds 8]
                                           [--enable] [--jit] [--period 500009]
    python benches/interp/opprof.py show prof.json [--top 40]

`record` runs the workload in a fresh process (setup, one warm-up run, then run() in a loop
for --seconds under `perf record -e cycles:u -c <period> -g`, gated by a perf control FIFO),
then folds the call chains into:
  handlers   self samples per opcode handler (dispatch + the work the handler inlines),
             tagged specialized / generic (the adaptive base instruction) / instrumented / label
  callees    per handler: the C function it called directly (the "entry"), with the
             inclusive samples below it up to the next opcode handler (a nested eval)
  leaves     self samples per leaf symbol with its component (Classifier of _profiler.py)
  reentry    samples in the C-to-Python call machinery (eval entry/exit, frame push)
Each sample is attributed to the *nearest* opcode handler above its leaf.
"""

from __future__ import annotations

import argparse
import collections
import json
import os
import re
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "workloads", "data"))

EVAL_FRAMES = {"_PyEval_EvalFrameDefault", "_PyEval_Vector", "_PyEval_EvalFrame",
               "PyEval_EvalCode", "_PyEval_EvalFrameDefault.cold"}
# The machinery a C caller uses to run a Python function on a new C-level eval loop.
REENTRY = ("_PyEval_Vector", "_PyEval_EvalFrameDefault", "_PyEvalFramePushAndInit",
           "_PyFunction_Vectorcall", "_PyEval_FrameClearAndPop", "clear_thread_frame",
           "_PyFrame_ClearExceptCode", "_PyThreadState_PushFrame", "_PyThreadState_PopFrame",
           "_PyFrame_ClearLocals")
# Generic call plumbing between a handler and the function that does the work.
CALL_WRAPPERS = ("PyObject_Vectorcall", "_PyObject_VectorcallTstate", "PyObject_Call",
                 "_PyObject_Call", "_PyObject_MakeTpCall", "cfunction_call", "_PyVectorcall_Call",
                 "PyVectorcall_Call", "_PyObject_VectorcallDictTstate", "PyObject_VectorcallDict",
                 "_PyObject_FastCallDictTstate", "PyObject_CallOneArg", "_PyObject_CallNoArgs",
                 "PyObject_CallNoArgs", "PyObject_VectorcallMethod", "_PyObject_Call_Prepend",
                 "_PyEval_ExecutorExit")
# A sample whose chain below its handler passes through these is C-to-Python call overhead.
REENTRY_EVAL = {"_PyEval_Vector", "_PyEval_EvalFrameDefault", "_PyFunction_Vectorcall",
                "_PyEvalFramePushAndInit", "_PyEvalFramePushAndInit_Ex", "gen_send_ex2",
                "gen_iternext", "_PyEval_EvalFrame"}
SUFFIX_RE = re.compile(r"(\.(cold|warm|part|constprop|isra|lto_priv|llvm)(\.\d+)*)+$")


def base(sym: str) -> str:
    return SUFFIX_RE.sub("", sym.split("+0x")[0])


def opcode_families():
    import _opcode_metadata as m

    spec = {}
    for fam, members in m._specializations.items():
        for s in members:
            spec[s] = fam
    return spec, set(m._specializations)


# --------------------------------------------------------------------------------------
# driver (runs inside the profiled interpreter)
# --------------------------------------------------------------------------------------


def _ctl(cmd):
    ctl = os.environ.get("RQ_PERF_CTL")
    if not ctl:
        return
    with open(ctl, "w") as f:
        f.write(cmd + "\n")
    with open(os.environ["RQ_PERF_ACK"]) as f:
        f.read(5)


def load_workload(path):
    import importlib.util

    name = "rq_workload_" + os.path.splitext(os.path.basename(path))[0]
    sys.path.insert(0, os.path.dirname(os.path.abspath(path)))
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def driver(a):
    if a.enable:
        import rustique

        rustique.enable()
    mod = load_workload(a.workload)
    state = mod.setup()
    ref = mod.digest(mod.run(state))
    walls = []
    _ctl("enable")
    t_start = time.perf_counter()
    while True:
        t0 = time.perf_counter()
        res = mod.run(state)
        t1 = time.perf_counter()
        walls.append(t1 - t0)
        if t1 - t_start >= a.seconds and len(walls) >= a.min_iters:
            break
    _ctl("disable")
    out = {"walls": walls, "digest_ok": mod.digest(res) == ref, "pid": os.getpid(),
           "python": sys.version, "jit": bool(getattr(sys, "_jit", None) and sys._jit.is_enabled())}
    with open(a.out, "w") as f:
        json.dump(out, f)


# --------------------------------------------------------------------------------------
# record + fold
# --------------------------------------------------------------------------------------


def perf_samples(data):
    """Yield (period, [(sym, dso), ...] leaf first) from `perf script`."""
    p = subprocess.Popen(["perf", "script", "-i", data, "-F", "period,ip,sym,dso",
                          "--no-inline"], stdout=subprocess.PIPE, text=True,
                         errors="replace", bufsize=1 << 20)
    period, chain = None, []
    for line in p.stdout:
        s = line.strip()
        if not s:
            if period is not None:
                yield period, chain
            period, chain = None, []
            continue
        if period is None:
            try:
                period = int(s.split()[0])
            except ValueError:
                period = 1
            continue
        # "addr sym+off (dso)"
        parts = s.split(" ", 1)
        if len(parts) < 2:
            continue
        rest = parts[1]
        dso = ""
        if rest.endswith(")") and " (" in rest:
            rest, dso = rest.rsplit(" (", 1)
            dso = dso[:-1]
        chain.append((base(rest), dso))
    if period is not None:
        yield period, chain
    p.wait()


def fold(data, cpython_src, cache):
    from _profiler import Classifier

    cls = Classifier(cpython_src, cache)
    spec, fams = opcode_families()
    handlers = collections.Counter()      # handler -> self
    callees = collections.Counter()       # (handler, entry) -> inclusive below handler
    callee_leaf = collections.Counter()   # (handler, entry, leafcomp) -> samples
    leaves = collections.Counter()        # (leaf, comp) -> self
    deep = collections.Counter()          # (handler, deep entry, reentry?) -> samples
    total = 0
    for period, chain in perf_samples(data):
        total += period
        if not chain:
            continue
        leaf, dso = chain[0]
        comp = cls.component(leaf, dso)
        leaves[(leaf, comp)] += period
        h_idx = None
        for i, (sym, _d) in enumerate(chain):
            if sym.startswith("_TAIL_CALL_"):
                h_idx = i
                break
        if h_idx is None:
            # no handler: either eval-frame body proper or outside the interpreter
            top = next((s for s, _ in chain if s in EVAL_FRAMES), None)
            handlers["(no handler)" if top is None else "(eval frame)"] += period
            if top is None:
                entry = chain[-1][0] if chain else "?"
                callees[("(no handler)", leaf)] += period
            continue
        h = chain[h_idx][0][len("_TAIL_CALL_"):]
        if h_idx == 0:
            handlers[h] += period
            continue
        entry = chain[h_idx - 1][0]
        callees[(h, entry)] += period
        callee_leaf[(h, entry, comp)] += period
        below = [s for s, _ in chain[:h_idx]]
        d = next((s for s in reversed(below) if not s.startswith(CALL_WRAPPERS)), below[0])
        reent = any(s in REENTRY_EVAL for s in below)
        deep[(h, d, reent)] += period
    return {"total": total, "handlers": dict(handlers),
            "deep": [[h, d, r, v] for (h, d, r), v in deep.items()],
            "callees": [[h, e, v] for (h, e), v in callees.items()],
            "callee_leaf": [[h, e, c, v] for (h, e, c), v in callee_leaf.items()],
            "leaves": [[l, c, v] for (l, c), v in leaves.items()],
            "families": {"spec": spec, "fams": sorted(fams)}}


def record(a):
    py = a.python or sys.executable
    tmp = tempfile.mkdtemp(prefix="opprof-")
    ctl, ack = os.path.join(tmp, "ctl"), os.path.join(tmp, "ack")
    os.mkfifo(ctl)
    os.mkfifo(ack)
    data = os.path.join(tmp, "perf.data")
    out = os.path.join(tmp, "drv.json")
    env = dict(os.environ, RQ_PERF_CTL=ctl, RQ_PERF_ACK=ack)
    if a.jit:
        env["PYTHON_JIT"] = "1"
    else:
        env.pop("PYTHON_JIT", None)
    cmd = ["perf", "record", "-q", "-e", "cycles:u", "-c", str(a.period), "-g", "-D", "-1",
           "--control", f"fifo:{ctl},{ack}", "-o", data, "--",
           py, os.path.abspath(__file__), "driver", os.path.abspath(a.workload),
           "--seconds", str(a.seconds), "--out", out] + (["--enable"] if a.enable else [])
    if a.taskset:
        cmd = ["taskset", "-c", a.taskset] + cmd
    subprocess.run(cmd, check=True, env=env, stdout=subprocess.DEVNULL)
    with open(out) as f:
        drv = json.load(f)
    res = fold(data, a.cpython_src or os.environ.get("CPYTHON_SRC", ""), tmp)
    res.update(workload=os.path.basename(a.workload)[:-3], enable=a.enable, jit=a.jit,
               driver=drv, period=a.period)
    with open(a.o, "w") as f:
        json.dump(res, f)
    if a.keep:
        os.replace(data, a.o[:-5] + ".perf.data")
    else:
        for fn in (data, out):
            os.unlink(fn)
    print(a.o, res["total"], "cycles sampled;", len(drv["walls"]), "runs, digest_ok",
          drv["digest_ok"])


# --------------------------------------------------------------------------------------
# report
# --------------------------------------------------------------------------------------

LABELS = {"error", "exception_unwind", "exit_unwind", "start_frame", "resume_frame",
          "pop_1_error", "pop_2_error", "pop_3_error", "pop_4_error", "record_previous_inst",
          "stop_tracing"}


def kind(h, spec, fams):
    if h.startswith("("):
        return "eval"
    if h in LABELS or h.islower():
        return "label"
    if h.startswith("INSTRUMENTED_"):
        return "instrumented"
    if h in spec:
        return "specialized"
    if h in fams:
        return "generic"
    return "unspecializable"


def show(a):
    for path in a.files:
        r = json.load(open(path))
        spec, fams = r["families"]["spec"], set(r["families"]["fams"])
        T = r["total"]
        pc = lambda v: 100.0 * v / T
        print(f"== {r['workload']} enable={r['enable']} jit={r['jit']}  "
              f"samples={T // r['period']}  median run {sorted(r['driver']['walls'])[len(r['driver']['walls'])//2]:.3f}s")
        hs = r["handlers"]
        bykind = collections.Counter()
        for h, v in hs.items():
            bykind[kind(h, spec, fams)] += v
        inc = collections.Counter()
        for h, e, v in r["callees"]:
            inc[h] += v
        print("handler self by kind:", {k: round(pc(v), 2) for k, v in bykind.most_common()})
        print(f"  all handler self {pc(sum(v for h, v in hs.items() if not h.startswith('('))):.2f}%"
              f"  native under handlers {pc(sum(inc.values())):.2f}%")
        print("top handlers: self% / native-below%")
        allh = set(hs) | set(inc)
        for h in sorted(allh, key=lambda h: -(hs.get(h, 0) + inc.get(h, 0)))[: a.top]:
            print(f"  {h:40s} {kind(h, spec, fams):15s} {pc(hs.get(h, 0)):6.2f} {pc(inc.get(h, 0)):6.2f}")
        print("top (handler -> entry) native-below%:")
        for h, e, v in sorted(r["callees"], key=lambda x: -x[2])[: a.top]:
            print(f"  {pc(v):6.2f}  {h:32s} -> {e}")
        print("top leaves self%:")
        for l, c, v in sorted(r["leaves"], key=lambda x: -x[2])[: a.top]:
            print(f"  {pc(v):6.2f}  {c:16s} {l}")
        print()


def refold(a):
    for path in a.files:
        r = json.load(open(path))
        data = path[:-5] + ".perf.data"
        r.update(fold(data, a.cpython_src or os.environ.get("CPYTHON_SRC", ""),
                      tempfile.mkdtemp(prefix="opprof-")))
        json.dump(r, open(path, "w"))
        print("refolded", path)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("driver")
    d.add_argument("workload")
    d.add_argument("--seconds", type=float, default=8)
    d.add_argument("--min-iters", type=int, default=3)
    d.add_argument("--out", required=True)
    d.add_argument("--enable", action="store_true")
    r = sub.add_parser("record")
    r.add_argument("workload")
    r.add_argument("-o", required=True)
    r.add_argument("--python")
    r.add_argument("--seconds", type=float, default=8)
    r.add_argument("--period", type=int, default=500009)
    r.add_argument("--enable", action="store_true")
    r.add_argument("--jit", action="store_true")
    r.add_argument("--keep", action="store_true")
    r.add_argument("--taskset")
    r.add_argument("--cpython-src")
    s = sub.add_parser("show")
    s.add_argument("files", nargs="+")
    s.add_argument("--top", type=int, default=30)
    f = sub.add_parser("refold")
    f.add_argument("files", nargs="+")
    f.add_argument("--cpython-src")
    a = ap.parse_args()
    {"driver": driver, "record": record, "show": show, "refold": refold}[a.cmd](a)


if __name__ == "__main__":
    main()
