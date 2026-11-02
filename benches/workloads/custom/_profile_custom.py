#!/usr/bin/env python3
"""Layer profile of one `custom` workload: the six layers + the CPython machinery it uses.

Not a workload (leading underscore). Reuses the wave-1 driver, perf parser and symbol
classifier of ``benches/workloads/data/_profiler.py`` and adds what this family needs:

  python _profile_custom.py <workload.py> --out <profile.json> [--seconds 6]
        [--cpython-src $CPYTHON_SRC] [--steps perf,xperf,cprofile,stat]

Steps (each a fresh process of the running interpreter: setup(), one warm-up run(), then
steady-state run() calls only, gated with perf's control FIFO):

  perf     plain ``perf record -e cpu-clock -F 1999 -g`` (frame pointers): the undistorted
           leaf-native-component split (``-X perf`` disables inlined Python-to-Python calls
           and inflates interpreter/call shares, up to 1.27-1.43x per w2-interp).
  xperf    the same with ``-X perf``: every sample's innermost Python frame, classified by
           code-object provenance into own / gen:<kind> / stdlib:<module>; the owner x
           component matrix is then re-weighted per component so its component marginal
           equals the plain profile's (the trampoline's own samples are folded into
           "interpreter" first). Inclusive shares (any frame on the stack) of generated code,
           stdlib modules and type-slot dispatch of user dunders (``slot_tp_*``) come from
           these stacks too (tagged as -X perf numbers: biased up for call-heavy code).
  cprofile one run() under cProfile: call counts and tottime of the machinery functions.
  stat     ``perf stat -e instructions,cycles`` over the gated run() loop: instructions per
           run(), the noise-robust number for 3.15.1 vs 3.15.1t and plain vs enabled
           (``--enable`` runs ``rustique.enable()`` before the workload is imported).

Layers (brief vocabulary): own = interpreter components under the workload's own frames;
generated = everything under generated code objects (dataclass ``__create_fn__`` and
``<string>`` code, namedtuple/NamedTuple methods, ``functools.total_ordering`` methods,
``enum.py`` machinery acting on the workload's enums, the ``reprlib`` wrapper of dataclass
``__repr__``); language = protocol/builtin native components under own frames; stdlib = under
stdlib frames + C stdlib modules called from own code; runtime = allocation, deallocation,
cyclic GC, import (any owner); thirdparty = none in this family.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "data"))
import _profiler as P  # noqa: E402  (wave-1 profiler: driver, perf parser, classifier)

LANGUAGE = {"attr/type", "calls", "str", "bytes", "int", "float", "dict", "list/tuple/set",
            "iter/range/slice", "builtins-module", "abstract", "other-objects", "generators",
            "exceptions", "libc", "libm", "other"}
RUNTIME = {"memory/gc", "import", "kernel"}
INTERP = {"interpreter", "perf-trampoline"}

TOTAL_ORDERING = re.compile(r"^_(gt|lt|ge|le)_from_(gt|lt|ge|le)$")

# leaf-symbol groups inside the language layer (base names; first match wins)
MACHINERY = [
    ("exceptions", ("_PyErr_", "PyErr_", "BaseException_", "_PyException", "PyException",
                    "exception_", "_PyTraceBack", "PyTraceBack", "tb_", "KeyError_",
                    "StopIteration_", "_Py_Offer_Suggestions", "_PyEval_MatchClass")),
    ("isinstance/issubclass", ("PyObject_IsInstance", "object_isinstance",
                               "recursive_isinstance", "object_recursive_isinstance",
                               "PyObject_IsSubclass", "recursive_issubclass",
                               "abstract_issubclass", "_PyObject_RealIs", "abstract_get_bases",
                               "builtin_isinstance", "builtin_issubclass", "_abc__abc_",
                               "_abc_")),
    ("slot dispatch (user dunders)", ("slot_tp_", "slot_nb_", "slot_sq_", "slot_mp_",
                                      "slot_am_", "vectorcall_method", "vectorcall_unbound",
                                      "lookup_maybe_method", "lookup_method", "call_unbound",
                                      "maybe_call_special", "wrap_binaryfunc", "wrapperdescr_",
                                      "wrapper_call", "wrap_")),
    ("descriptors/property", ("property_descr_", "property_", "func_descr_get", "method_get",
                              "classmethod_get", "sm_descr_get", "cm_descr_get",
                              "PyMember_", "member_get", "member_set", "getset_get",
                              "getset_set", "method_vectorcall", "_PyObject_GetMethod",
                              "PyMethod_New", "meth_")),
    ("generic attribute lookup", ("_PyType_Lookup", "PyObject_GenericGetAttr",
                                  "_PyObject_GenericGetAttrWithDict", "PyObject_GenericSetAttr",
                                  "_PyObject_GenericSetAttrWithDict", "PyObject_GetAttr",
                                  "PyObject_SetAttr", "_PyObject_LookupAttr",
                                  "PyObject_GetOptionalAttr", "find_name_in_mro",
                                  "_PyObject_LookupSpecial", "type_getattro", "type_setattro",
                                  "_Py_type_getattro", "module_getattro", "_PyObject_GetAttrId",
                                  "update_cache", "_PyType_LookupRef", "type_", "_PyType_")),
    ("calls/frames", ("_PyObject_Vectorcall", "PyObject_Vectorcall", "_PyFunction_Vectorcall",
                      "cfunction_", "PyObject_Call", "_PyObject_Call", "_PyObject_MakeTpCall",
                      "_Py_VectorCall", "PyVectorcall", "_PyVectorcall", "_PyCFunction",
                      "type_call", "object_vacall", "_PyArg_", "PyArg_", "vgetargs",
                      "method_vectorcall", "_PyStack_", "_PyEval_Vector")),
    ("comparison/hash", ("PyObject_RichCompare", "do_richcompare", "PyObject_Hash",
                         "_Py_HashDouble", "tuplehash", "tuple_hash", "long_hash",
                         "unicode_hash", "_Py_HashPointer", "object_richcompare",
                         "tuplerichcompare", "tuple_richcompare", "long_richcompare",
                         "float_richcompare", "unicode_compare", "PyUnicode_Compare",
                         "PyUnicode_RichCompare", "_PyUnicode_Equal", "unicode_eq")),
]


# --------------------------------------------------------------------------------------
# owner classification by code-object provenance
# --------------------------------------------------------------------------------------


def make_owner(env: dict, workload: str):
    stdlib = env["stdlib"].rstrip("/") + "/"
    wl = os.path.abspath(workload)

    def owner(qual: str, fname: str) -> str:
        if fname == wl:
            return "own"
        if fname == "<string>":
            if qual.startswith("__create_fn__"):
                return "gen:dataclass"
            if qual == "<lambda>" or qual.startswith("<lambda>:"):
                return "gen:namedtuple"
            return "gen:exec"
        if fname.startswith("<frozen "):
            rel = fname[8:-1].replace(".", "/") + ".py"
        elif fname.startswith(stdlib) and "/site-packages/" not in fname:
            rel = fname[len(stdlib):]
        elif fname.endswith("_profiler.py") or fname.endswith("_profile_custom.py"):
            return "<driver>"
        elif "/site-packages/" in fname:
            return "thirdparty:" + fname.split("/site-packages/", 1)[1].split("/")[0]
        else:
            return "other:" + os.path.basename(fname)
        if rel == "enum.py":
            return "gen:enum"
        if rel == "functools.py" and TOTAL_ORDERING.match(qual):
            return "gen:total_ordering"
        if rel == "collections/__init__.py" and qual.startswith("namedtuple.<locals>"):
            return "gen:namedtuple"
        if rel == "reprlib.py" and "recursive_repr" in qual:
            return "gen:dataclass"
        parts = rel.removesuffix(".py").split("/")
        if parts[-1] == "__init__":
            parts = parts[:-1]
        return "stdlib:" + parts[0]

    def pyname(qual: str, fname: str) -> str:
        if fname == wl:
            return f"own:{qual}"
        if fname.startswith(stdlib):
            return f"{fname[len(stdlib):]}:{qual}"
        return f"{os.path.basename(fname)}:{qual}"

    return owner, pyname


def classify_extra(cls, sym, dso) -> str:
    comp = cls.component(sym, dso)
    b = P.base_name(sym) if sym else ""
    if b.startswith("sqlite3") or comp in ("cursor", "connection", "statement", "row", "module",
                                            "blob", "microprotocols", "prepare_protocol"):
        return "sqlite"
    if comp.startswith("other:Python/structmember") or b.startswith("PyMember_"):
        return "attr/type"
    if comp == "runtime":
        return "other-objects"
    if comp.startswith("other:"):
        return "other"
    return comp


def layer_of(owner: str, comp: str, in_gc: bool) -> str:
    if in_gc or comp in RUNTIME:
        return "runtime"
    if owner.startswith("gen:"):
        return "generated"
    if owner.startswith("stdlib:"):
        return "stdlib"
    if owner.startswith("thirdparty:"):
        return "thirdparty"
    if owner == "own":
        if comp in INTERP:
            return "own"
        if comp in LANGUAGE:
            return "language"
        return "stdlib"  # a C stdlib module called directly from own code
    return "runtime"  # no Python frame in the gated loop: interpreter-level work


def machinery_of(sym: str | None) -> str | None:
    if not sym:
        return None
    b = P.base_name(sym)
    for name, prefixes in MACHINERY:
        if b.startswith(prefixes):
            return name
    return None


SLOT_RE = re.compile(r"^(slot_(tp|nb|sq|mp|am)_\w+|vectorcall_method|wrapperdescr_call)")


# --------------------------------------------------------------------------------------
# perf steps
# --------------------------------------------------------------------------------------


class Parser(P.PerfParser):
    """perf >= 7 right-aligns the tid column: a sample header starts with spaces, frame
    lines with a tab (the wave-1 parser took any indented line for a frame)."""

    def parse(self, data_file):
        proc = subprocess.Popen(["perf", "script", "-i", data_file, "-F",
                                 "tid,period,ip,sym,dso,dsoff"],
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                                errors="replace")
        samples = []
        cur = None
        for line in proc.stdout:
            line = line.rstrip("\n")
            if not line.strip():
                if cur is not None and cur[1]:
                    samples.append(cur)
                cur = None
                continue
            if not line.startswith("\t"):
                p = line.split()
                period = int(p[1]) if len(p) > 1 and p[1].isdigit() else 1
                cur = (period, [])
                continue
            m = P._LINE_RE.match(line)
            if not m or cur is None:
                continue
            ip, sym, dsopart = m.groups()
            if sym.startswith("py::"):
                cur[1].append(("py", sym[4:], None))
                continue
            dso, _, off = dsopart.rpartition("+0x")
            if not dso:
                dso, off = dsopart, "0"
            leaf = not cur[1]
            o = int(off, 16)
            s = self.sym(dso, o if leaf else max(o - 1, 0))
            cur[1].append(("n", s, dso))
        if cur is not None and cur[1]:
            samples.append(cur)
        proc.wait()
        return samples


def run_perf(py, wl, tmp, seconds, perf_x, env_extra=None):
    ctl, ack = os.path.join(tmp, "ctl"), os.path.join(tmp, "ack")
    for p in (ctl, ack):
        if os.path.exists(p):
            os.unlink(p)
        os.mkfifo(p)
    data = os.path.join(tmp, "perf-x.data" if perf_x else "perf.data")
    out = os.path.join(tmp, "drv.json")
    env = dict(os.environ, RQ_PERF_CTL=ctl, RQ_PERF_ACK=ack, **(env_extra or {}))
    cmd = (["perf", "record", "-q", "-e", "cpu-clock", "-F", "1999", "-g", "-D", "-1",
            "--control", f"fifo:{ctl},{ack}", "-o", data, "--"]
           + P._run_driver(py, wl, "perf", out, seconds, 3, perf_x))
    subprocess.run(cmd, check=True, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    with open(out) as f:
        drv = json.load(f)
    return data, drv


def walk(frames, cls):
    """-> (leaf component, leaf symbol, index of the innermost Python frame or None, in_gc,
    slot frames between the leaf and that Python frame, all python frames)."""
    kind, s, dso = frames[0]
    if kind == "py":
        leafc, leafsym = "perf-trampoline", None
    else:
        leafc, leafsym = classify_extra(cls, s, dso), s
    pyidx = None
    for i, (k, _s, _d) in enumerate(frames):
        if k == "py":
            pyidx = i
            break
    upto = pyidx if pyidx is not None else len(frames)
    in_gc = any(k == "n" and s2 is not None and P.base_name(s2).startswith(P.GC_FRAMES)
                for k, s2, _ in frames[:upto])
    return leafc, leafsym, pyidx, in_gc


def plain_profile(samples, cls) -> dict:
    total = sum(p for p, _ in samples) or 1
    comp = Counter()
    mach = Counter()
    gc = 0
    top = Counter()
    slot_incl = Counter()
    for period, frames in samples:
        leafc, leafsym, _pyidx, in_gc = walk(frames, cls)
        seen = set()
        for k, s2, _d in frames:
            if k == "n" and s2 is not None:
                m = SLOT_RE.match(P.base_name(s2))
                if m and m.group(1) not in seen:
                    seen.add(m.group(1))
                    slot_incl[m.group(1)] += period
        comp[leafc] += period
        if in_gc:
            gc += period
        m = machinery_of(leafsym)
        if m:
            mach[m] += period
        top[(P.base_name(leafsym) if leafsym else "?", leafc)] += period
    sh = lambda v: round(v / total, 5)
    return {"samples": len(samples), "component": {k: sh(v) for k, v in comp.most_common()},
            "gc_share": sh(gc), "machinery_leaf": {k: sh(v) for k, v in mach.most_common()},
            "slot_inclusive": {k: sh(v) for k, v in slot_incl.most_common(20)},
            "top_native": [[n, c, sh(v)] for (n, c), v in top.most_common(50)]}


def xperf_profile(samples, cls, owner_fn, pyname_fn, plain_comp: dict) -> dict:
    cell = Counter()           # (owner, comp, in_gc) -> period
    pyself = Counter()         # innermost python function -> period
    incl_owner = Counter()     # owner class anywhere on the stack
    slot_incl = Counter()      # slot frame anywhere on the stack (user dunder via type slot)
    mach_by_owner = defaultdict(Counter)
    total = 0
    for period, frames in samples:
        total += period
        leafc, leafsym, pyidx, in_gc = walk(frames, cls)
        if pyidx is None:
            own = "<no-python-frame>"
            func = "<none>"
        else:
            qual, _, fname = frames[pyidx][1].partition(":")
            own = owner_fn(qual, fname)
            func = pyname_fn(qual, fname)
        if own == "<driver>":
            own = "own"
        comp = "interpreter" if leafc == "perf-trampoline" else leafc
        cell[(own, comp, in_gc)] += period
        pyself[(func, own)] += period
        seen = set()
        for k, s2, _d in frames:
            if k == "py":
                q, _, fn = s2.partition(":")
                o = owner_fn(q, fn)
                if o not in seen:
                    seen.add(o)
                    incl_owner[o] += period
            elif s2 is not None:
                m = SLOT_RE.match(P.base_name(s2))
                if m and ("slot:" + m.group(1)) not in seen:
                    seen.add("slot:" + m.group(1))
                    slot_incl[m.group(1)] += period
        m = machinery_of(leafsym)
        if m:
            mach_by_owner[own][m] += period
    # re-weight: component marginal of -X perf -> plain perf's
    xcomp = Counter()
    for (o, c, g), v in cell.items():
        xcomp[c] += v
    xtot = sum(xcomp.values()) or 1
    w = {}
    for c, v in xcomp.items():
        px = v / xtot
        pp = plain_comp.get(c, 0.0)
        w[c] = (pp / px) if px > 0 else 0.0
    rw = Counter()
    for (o, c, g), v in cell.items():
        rw[(o, c, g)] += v * w.get(c, 0.0)
    rtot = sum(rw.values()) or 1
    layers = Counter()
    owners = Counter()
    layer_comp = defaultdict(Counter)
    for (o, c, g), v in rw.items():
        lay = layer_of(o, c, g)
        layers[lay] += v / rtot
        owners[o] += v / rtot
        layer_comp[lay][c] += v / rtot
    # raw -X perf layers for comparison
    raw_layers = Counter()
    for (o, c, g), v in cell.items():
        raw_layers[layer_of(o, c, g)] += v / (total or 1)
    sh = lambda v: round(v, 5)
    st = lambda v: round(v / (total or 1), 5)
    return {
        "samples": len(samples),
        "layers": {k: sh(v) for k, v in sorted(layers.items(), key=lambda kv: -kv[1])},
        "layers_raw_xperf": {k: sh(v) for k, v in sorted(raw_layers.items(), key=lambda kv: -kv[1])},
        "owners": {k: sh(v) for k, v in owners.most_common(40)},
        "layer_components": {k: {c: sh(v) for c, v in cc.most_common(12)}
                             for k, cc in layer_comp.items()},
        "inclusive_owner_xperf": {k: st(v) for k, v in incl_owner.most_common(30)},
        "slot_inclusive_xperf": {k: st(v) for k, v in slot_incl.most_common(20)},
        "top_python_self_xperf": [[f, o, st(v)] for (f, o), v in pyself.most_common(45)],
        "machinery_leaf_by_owner_xperf": {o: {m: st(v) for m, v in mc.most_common()}
                                          for o, mc in mach_by_owner.items()},
        "reweight": {c: round(v, 4) for c, v in sorted(w.items())},
        "cells": [[o, c, g, sh(v / rtot)] for (o, c, g), v in rw.most_common(120)],
    }


def run_stat(py, wl, tmp, seconds, enable=False, hashseed=None):
    ctl, ack = os.path.join(tmp, "sctl"), os.path.join(tmp, "sack")
    for p in (ctl, ack):
        if os.path.exists(p):
            os.unlink(p)
        os.mkfifo(p)
    out = os.path.join(tmp, "drv-stat.json")
    csv = os.path.join(tmp, "stat.csv")
    env = dict(os.environ, RQ_PERF_CTL=ctl, RQ_PERF_ACK=ack)
    if hashseed is not None:  # equal dict/set layouts on both sides of a comparison
        env["PYTHONHASHSEED"] = str(hashseed)
    drv = P._run_driver(py, wl, "perf", out, seconds, 3, False)
    if enable:
        drv = [py, "-c", "import rustique, runpy, sys; rustique.enable(); "
               "sys.argv = sys.argv[1:]; runpy.run_path(sys.argv[0], run_name='__main__')"] + drv[1:]
    cmd = (["perf", "stat", "-x", ",", "-e", "instructions:u,cycles:u", "-D", "-1",
            "--control", f"fifo:{ctl},{ack}", "-o", csv, "--"] + drv)
    subprocess.run(cmd, check=True, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    with open(out) as f:
        d = json.load(f)
    vals, running = {}, {}
    for line in open(csv):
        parts = line.strip().split(",")
        if len(parts) > 4 and parts[0].replace(".", "").isdigit():
            ev = parts[2].split(":")[0]
            vals[ev] = float(parts[0])
            running[ev] = float(parts[4]) if parts[4] else 100.0  # % of time counted
    n = len(d["walls"])
    return {"runs": n, "instructions_per_run": vals.get("instructions", 0) / n,
            "cycles_per_run": vals.get("cycles", 0) / n,
            "pct_counted": running,
            "wall_median_s": statistics.median(d["walls"]), "digest": d["digest"],
            "digest_ok": d["digest_ok"]}


def cprofile_driver(wl: str, out: str) -> None:
    """Runs inside the profiled interpreter. Reads _lsprof entries per code object
    (pstats merges entries with equal (file, line, name) labels, which loses most of the
    exec-generated dataclass/namedtuple methods)."""
    import cProfile

    mod = P._load_workload(wl)
    state = mod.setup()
    mod.run(state)
    prof = cProfile.Profile()
    prof.enable()
    mod.run(state)
    prof.disable()
    # name exec-generated methods after their class (and mark frozen dataclass __init__)
    gen_names = {}
    for obj in list(vars(mod).values()):
        if isinstance(obj, type):
            for attr, fn in vars(obj).items():
                c = getattr(fn, "__code__", None)
                if c is not None and c.co_filename == "<string>":
                    tag = " [frozen]" if "__dataclass_builtins_object__" in (c.co_names + c.co_freevars) else ""
                    gen_names[c] = f"{obj.__name__}.{attr}{tag}"
    agg = defaultdict(lambda: [0, 0.0])
    for e in prof.getstats():
        code = e.code
        if isinstance(code, str):
            key = ("~", code)
        elif code in gen_names:
            key = ("<string>", ("<lambda>:" if code.co_name == "<lambda>" else
                                "__create_fn__.<locals>.") + gen_names[code])
        else:
            key = (code.co_filename, code.co_qualname)
        agg[key][0] += e.callcount
        agg[key][1] += e.inlinetime
    rows = [{"file": f, "name": n, "ncalls": c, "tottime": t} for (f, n), (c, t) in agg.items()]
    with open(out, "w") as f:
        json.dump({"env": P._env_info(), "rows": rows}, f)


def run_cprofile(py, wl, tmp, owner_fn, pyname_fn) -> dict:
    out = os.path.join(tmp, "drv-cprofile.json")
    subprocess.run([py, os.path.abspath(__file__), wl, "--out", out, "--cprofile-driver"],
                   check=True)
    with open(out) as f:
        cp = json.load(f)
    rows = cp["rows"]
    tot = sum(r["tottime"] for r in rows) or 1.0
    by_owner_tt = Counter()
    by_owner_calls = Counter()
    merged = defaultdict(lambda: [0, 0.0])
    for r in rows:
        if r["file"] == "~":
            o = "<builtin>"
            name = r["name"]
        else:
            o = owner_fn(r["name"], r["file"])
            name = pyname_fn(r["name"], r["file"])
            if r["file"] == "<string>":
                name = r["name"].replace("__create_fn__.<locals>.", "<dataclass>.").replace(
                    "<lambda>:", "<namedtuple>.")
        if o == "<driver>":
            continue
        by_owner_tt[o] += r["tottime"]
        by_owner_calls[o] += r["ncalls"]
        merged[(name, o)][0] += r["ncalls"]
        merged[(name, o)][1] += r["tottime"]
    py_rows = [(t, c, n, o) for (n, o), (c, t) in merged.items()]
    py_rows.sort(key=lambda x: -x[0])
    calls_rank = sorted(py_rows, key=lambda x: -x[1])
    keep = py_rows[:45] + [x for x in calls_rank[:25] if x not in py_rows[:45]]
    return {"total_calls": sum(r["ncalls"] for r in rows),
            "owner_tottime_share": {k: round(v / tot, 5) for k, v in by_owner_tt.most_common()},
            "owner_calls": dict(by_owner_calls.most_common()),
            "top": [[n, o, c, round(t / tot, 5)] for t, c, n, o in keep]}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("workload")
    ap.add_argument("--out", required=True)
    ap.add_argument("--seconds", type=float, default=6.0)
    ap.add_argument("--steps", default="perf,xperf,cprofile,stat")
    ap.add_argument("--stat-enable", action="store_true",
                    help="also count instructions with rustique.enable()")
    ap.add_argument("--cpython-src", default=os.environ.get("CPYTHON_SRC",
                                                            os.path.expanduser("~/src/cpython-3.15.1")))
    ap.add_argument("--cache", default=os.path.join(tempfile.gettempdir(), "rq-profile-cache"))
    ap.add_argument("--hashseed", type=int, default=0,
                    help="PYTHONHASHSEED for the stat step (default 0; -1: random)")
    ap.add_argument("--cprofile-driver", action="store_true", help=argparse.SUPPRESS)
    a = ap.parse_args()
    if a.cprofile_driver:
        return cprofile_driver(os.path.abspath(a.workload), a.out)
    os.makedirs(a.cache, exist_ok=True)
    py = sys.executable
    wl = os.path.abspath(a.workload)
    steps = a.steps.split(",")
    result = {"workload": os.path.basename(wl)[:-3]}
    if os.path.exists(a.out):
        with open(a.out) as f:
            result = json.load(f)
    tmp = tempfile.mkdtemp(prefix="rqcustom-")
    cls = P.Classifier(a.cpython_src, a.cache)
    parser = Parser(cls, None, None)
    env = P._env_info()
    result["python"] = env["python"]
    owner_fn, pyname_fn = make_owner(env, wl)
    if "perf" in steps:
        data, drv = run_perf(py, wl, tmp, a.seconds, False)
        result["plain"] = plain_profile(parser.parse(data), cls)
        result["plain"]["wall_median_s"] = round(statistics.median(drv["walls"]), 4)
        print(f"[{result['workload']}] perf plain {result['plain']['samples']}", file=sys.stderr)
    if "xperf" in steps:
        data, drv = run_perf(py, wl, tmp, a.seconds, True)
        samples = parser.parse(data)
        P._rm_perf_map(drv)
        result["xperf"] = xperf_profile(samples, cls, owner_fn, pyname_fn,
                                        result["plain"]["component"])
        result["xperf"]["wall_median_s"] = round(statistics.median(drv["walls"]), 4)
        print(f"[{result['workload']}] perf -X {result['xperf']['samples']}", file=sys.stderr)
    if "cprofile" in steps:
        result["cprofile"] = run_cprofile(py, wl, tmp, owner_fn, pyname_fn)
    if a.hashseed is not None and a.hashseed < 0:
        a.hashseed = None
    if "stat" in steps:
        result["stat"] = run_stat(py, wl, tmp, a.seconds, hashseed=a.hashseed)
        if a.stat_enable:
            result["stat_enabled"] = run_stat(py, wl, tmp, a.seconds, enable=True,
                                              hashseed=a.hashseed)
        result["stat"]["hashseed"] = a.hashseed
    with open(a.out, "w") as f:
        json.dump(result, f, indent=1)


if __name__ == "__main__":
    main()
