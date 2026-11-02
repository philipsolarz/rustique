#!/usr/bin/env python3
"""Why LOAD_ATTR stays generic: classify every execution of the generic LOAD_ATTR path.

    python benches/interp/attrprobe.py <workload.py> [--runs 2] [-o out.json]
    python benches/interp/attrprobe.py --show out.json [...]

Swaps the whole LOAD_ATTR family of the core interpreter for an analysis build
(_rqeval_attrprobe: rq_probe.h counts each generic execution by (type, name, method-load)
with instance-dict facts; specialized forms are verbatim copies, so their deopts land in the
probed generic handler too), runs the workload, then resolves each (type, name) statically:
owner kind, __getattr__/__getattribute__ hooks, the class attribute's descriptor kind along
the MRO, and whether the instance dict held the name.
"""
import argparse, collections, json, os, sys, types

HERE = os.path.dirname(os.path.abspath(__file__))
RQDIR = "/work/w2-interp/rqeval"


def descr_kind(v):
    if v is None:
        return "absent"
    t = type(v)
    if t is types.FunctionType:
        return "function"
    if t is property:
        return "property"
    if t.__name__ == "cached_property":
        return "cached_property"
    if t in (classmethod, staticmethod):
        return t.__name__
    if t in (types.MemberDescriptorType, types.GetSetDescriptorType, types.MethodDescriptorType,
             types.WrapperDescriptorType, types.ClassMethodDescriptorType, types.BuiltinFunctionType):
        return t.__name__
    if hasattr(t, "__get__"):
        if hasattr(t, "__set__") or hasattr(t, "__delete__"):
            return "data-descr:" + ("py" if t.__module__ != "builtins" else "c")
        return "nondata-descr:" + ("py" if t.__module__ != "builtins" else "c")
    return "plain"


def resolve(tp, name):
    mro = type.__dict__["__mro__"].__get__(tp)
    for k in mro:
        d = type.__dict__["__dict__"].__get__(k)
        if name in d:
            return d[name], k
    return None, None


def hooks(tp):
    mro = type.__dict__["__mro__"].__get__(tp)
    ga = any("__getattr__" in type.__dict__["__dict__"].__get__(k) for k in mro if k is not object)
    gab = resolve(tp, "__getattribute__")[1] not in (object, None, type, types.ModuleType)
    return ga, gab


def owner_kind(tp):
    if issubclass(tp, type):
        return "class object" + ("" if tp is type else " (metaclass)")
    if issubclass(tp, types.ModuleType):
        return "module"
    if tp.__module__ == "builtins":
        return "builtin instance"
    return "instance"


def classify(rows):
    out = collections.Counter()
    detail = collections.Counter()
    for tp, name, meth, n, nd, ndh, ni, nih in rows:
        try:
            cattr, where = resolve(tp, name)
            kind = descr_kind(cattr)
            ga, gab = hooks(tp)
            ok = owner_kind(tp)
        except Exception as e:  # pragma: no cover - exotic metatypes
            kind, ga, gab, ok = "error:" + type(e).__name__, False, False, "?"
        inst = "inst-has" if (ndh + nih) * 2 >= n and n else ("dict" if nd * 2 >= n else ("inline" if ni else "no-dict"))
        hook = "__getattribute__" if gab else ("__getattr__" if ga else "-")
        key = f"{ok} | hook {hook} | class attr {kind} | {inst}"
        out[key] += n
        detail[f"{tp.__module__}.{tp.__qualname__}.{name}{'()' if meth else ''} [{key}]"] += n
    return out, detail


def run(a):
    sys.path.insert(0, os.path.join(HERE, "rqeval"))
    sys.path.insert(0, HERE)
    import rqpatch
    import _opcode_metadata as m
    fam = ["LOAD_ATTR"] + list(m._specializations["LOAD_ATTR"])
    mod, table, done = rqpatch.swap(RQDIR, "_rqeval_attrprobe", fam)
    from opprof import load_workload
    w = load_workload(a.workload)
    st = w.setup()
    for _ in range(a.runs + 1):
        w.run(st)
    rows, lost = mod.probe_dump()
    rqpatch.unswap(mod, table, done)
    out, detail = classify(rows)
    tot = sum(out.values())
    res = {"workload": os.path.basename(a.workload)[:-3], "runs": a.runs + 1, "total": tot,
           "lost": lost, "by_kind": out.most_common(), "top": detail.most_common(60)}
    json.dump(res, open(a.o, "w"), indent=1)
    show(res)


def show(r):
    tot = r["total"] or 1
    print(f"== {r['workload']}: {r['total']:,} generic LOAD_ATTR executions in {r['runs']} runs (lost {r['lost']})")
    for k, v in r["by_kind"][:14]:
        print(f"  {100 * v / tot:5.1f}%  {k}")
    for k, v in r["top"][:12]:
        print(f"    {100 * v / tot:5.1f}%  {k}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("workload", nargs="?")
    ap.add_argument("--runs", type=int, default=2)
    ap.add_argument("-o")
    ap.add_argument("--show", nargs="*")
    a = ap.parse_args()
    if a.show:
        for p in a.show:
            show(json.load(open(p)))
    else:
        run(a)


if __name__ == "__main__":
    main()
