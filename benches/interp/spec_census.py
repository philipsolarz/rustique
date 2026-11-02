#!/usr/bin/env python3
"""Specialization census: which instruction sites of a warmed workload stay generic.

    python benches/interp/spec_census.py <workload.py> [--enable] [--runs 3] -o census.json
    python benches/interp/spec_census.py --diff plain.json enabled.json

After setup() and --runs run()s, walks every function's code object (gc) and reads the
adaptive bytecode (`dis.get_instructions(co, adaptive=True)`): each site is either a
specialized form, or the generic (adaptive) base instruction with its backoff counter. A
generic site whose backoff field is >= 5 failed to specialize at least 5 times and ran at
least ~670 times (pycore_backoff.h: 2, 7, 31, 127, 509, 2053 executions between attempts):
a *hot generic site*. Owner layer = workload / generated / stdlib / thirdparty by
co_filename. `--diff` lists sites whose form differs between two censuses (e.g. call
sites de-specialized by a vectorcall patch).
"""

from __future__ import annotations

import argparse
import collections
import dis
import gc
import json
import os
import sys
import sysconfig
import types

HERE = os.path.dirname(os.path.abspath(__file__))


def owner(fn: str, wl: str) -> str:
    if fn.startswith("<"):
        return "generated"
    if os.path.abspath(fn) == wl:
        return "own"
    if "site-packages" in fn:
        return "thirdparty:" + fn.split("site-packages/", 1)[1].split("/")[0].removesuffix(".py")
    if fn.startswith(sysconfig.get_paths()["stdlib"]):
        return "stdlib"
    return "other"


def census(wl: str) -> dict:
    import _opcode_metadata as m

    fams = set(m._specializations)
    spec = {s: f for f, ss in m._specializations.items() for s in ss}
    codes = {}
    for o in gc.get_objects():
        if type(o) is types.FunctionType:
            co = o.__code__
            codes[id(co)] = co
    sites = []
    agg = collections.Counter()
    for co in codes.values():
        own = owner(co.co_filename, wl)
        try:
            ins = list(dis.get_instructions(co, adaptive=True, show_caches=False))
        except Exception:
            continue
        for i in ins:
            name = i.opname
            if name in spec:
                fam, form = spec[name], "spec"
            elif name in fams:
                fam, form = name, "generic"
            else:
                continue
            if fam in ("RESUME", "JUMP_BACKWARD"):
                continue
            backoff = None
            if form == "generic" and i.cache_info:
                data = i.cache_info[0][2]
                if data and len(data) >= 2:
                    backoff = int.from_bytes(data[:2], "little") & 7
            # backoff b >= 2: at least two failed specialization attempts (>= 9 executions);
            # b >= 4: >= 167 executions. Backoff 0 on a generic site: a specialized site that
            # deopted back (polymorphic).
            hot = form == "generic" and backoff is not None and 2 <= backoff <= 6
            key = (fam, form if not hot else "generic-hot", own.split(":")[0])
            agg[key] += 1
            if hot or form == "spec":
                sites.append([co.co_filename, co.co_qualname, i.positions.lineno if i.positions else None,
                              i.offset, name, own, i.argrepr[:60], backoff])
    return {"agg": [[*k, v] for k, v in agg.items()], "sites": sites}


def driver(a):
    if a.enable:
        import rustique

        rustique.enable()
    sys.path.insert(0, os.path.join(HERE))
    from opprof import load_workload

    wl = os.path.abspath(a.workload)
    mod = load_workload(wl)
    st = mod.setup()
    for _ in range(a.runs):
        mod.run(st)
    res = census(wl)
    res.update(workload=os.path.basename(wl)[:-3], enable=a.enable)
    json.dump(res, open(a.o, "w"))
    hot = [s for s in res["sites"] if s[7] is not None]
    print(a.o, len(res["sites"]), "sites listed,", len(hot), "hot generic")


def show(path, top):
    r = json.load(open(path))
    agg = collections.defaultdict(collections.Counter)
    for fam, form, own, v in r["agg"]:
        agg[fam][form] += v
    print(f"== {r['workload']} enable={r['enable']}")
    print(f"{'family':22s} {'spec':>6s} {'generic':>8s} {'hot gen':>8s}")
    for fam, c in sorted(agg.items(), key=lambda x: -x[1]["generic-hot"]):
        print(f"{fam:22s} {c['spec']:6d} {c['generic']:8d} {c['generic-hot']:8d}")
    hot = [s for s in r["sites"] if s[7] is not None]
    byown = collections.Counter(s[5].split(':')[0] + ' ' + s[4] for s in hot)
    print("hot generic sites by owner x opcode:", byown.most_common(20))
    for s in hot[:top]:
        print("  ", s[5], s[4], os.path.basename(s[0]), s[1], s[2], s[6])


def diff(p1, p2, top):
    a, b = json.load(open(p1)), json.load(open(p2))
    key = lambda s: (s[0], s[1], s[3])
    da = {key(s): s for s in a["sites"]}
    db = {key(s): s for s in b["sites"]}
    ch = collections.Counter()
    rows = []
    for k in set(da) | set(db):
        x, y = da.get(k), db.get(k)
        fx = x[4] if x else "(generic/absent)"
        fy = y[4] if y else "(generic/absent)"
        if fx != fy:
            ch[(fx, fy)] += 1
            rows.append((k, fx, fy))
    print("form changes", p1, "->", p2)
    for (fx, fy), v in ch.most_common(top):
        print(f"  {v:5d}  {fx} -> {fy}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("workload", nargs="?")
    ap.add_argument("-o")
    ap.add_argument("--enable", action="store_true")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--show", nargs="*")
    ap.add_argument("--diff", nargs=2)
    ap.add_argument("--top", type=int, default=40)
    a = ap.parse_args()
    if a.show:
        for p in a.show:
            show(p, a.top)
    elif a.diff:
        diff(*a.diff, a.top)
    else:
        driver(a)


if __name__ == "__main__":
    main()
