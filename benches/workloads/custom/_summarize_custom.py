#!/usr/bin/env python3
"""Summarise ``_profile_custom.py`` profiles (+ ``run.py compare -o`` files) into tables.

    python _summarize_custom.py --prof <dir-3.15.1> [--prof-t <dir-3.15.1t>]
        [--compare compare-3.15.1.json] [--compare-t compare-3.15.1t.json] [--json out.json]

Prints Markdown: per-workload timing/ratios, the six layers, the dominant CPython machinery
(generated code, stdlib modules, C stdlib modules called from own code, language-level
machinery, runtime), type-slot dispatch of user dunders, and the t-build penalty by
component. Not a workload (leading underscore).
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import statistics
from collections import Counter, defaultdict

LAYERS = ["own", "generated", "language", "stdlib", "thirdparty", "runtime"]
LANG_COMPONENTS = {"attr/type", "calls", "str", "bytes", "int", "float", "dict", "list/tuple/set",
                   "iter/range/slice", "builtins-module", "abstract", "other-objects",
                   "generators", "exceptions", "libc", "libm", "other"}
INTERP = {"interpreter", "perf-trampoline"}
RUNTIME = {"memory/gc", "import", "kernel"}


def load(d):
    out = {}
    for p in sorted(glob.glob(os.path.join(d, "*.json"))):
        with open(p) as f:
            j = json.load(f)
        out[j["workload"]] = j
    return out


def load_compare(p):
    if not p or not os.path.exists(p):
        return {}
    with open(p) as f:
        return {r["workload"].split("/")[-1]: r for r in json.load(f)["rows"]}


def pct(x, nd=1):
    return f"{100 * x:.{nd}f}%"


def machinery(prof):
    """-> list of (label, share) of CPython-provided machinery, exclusive time."""
    x = prof["xperf"]
    items = Counter()
    for o, v in x["owners"].items():
        if o.startswith("gen:") or o.startswith("stdlib:"):
            items[o] += v
    for o, c, g, v in x.get("cells", []):
        if o == "own" and not g and c not in LANG_COMPONENTS | INTERP | RUNTIME:
            items["C:" + c] += v
    plain = prof["plain"]
    for k, v in plain["machinery_leaf"].items():
        items["lang:" + k] += v
    for comp in ("dict", "str", "list/tuple/set", "int", "float"):
        v = plain["component"].get(comp, 0.0)
        if v:
            items["lang:" + comp + " ops"] += v
    items["runtime:cyclic GC"] = plain["gc_share"]
    items["runtime:alloc/dealloc"] = max(0.0, plain["component"].get("memory/gc", 0.0)
                                         - plain["gc_share"])
    return items.most_common()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--prof", required=True)
    ap.add_argument("--prof-t")
    ap.add_argument("--compare")
    ap.add_argument("--compare-t")
    ap.add_argument("--json")
    a = ap.parse_args()
    P = load(a.prof)
    PT = load(a.prof_t) if a.prof_t else {}
    C = load_compare(a.compare)
    CT = load_compare(a.compare_t)
    names = sorted(P)
    summary = {}

    print("## Timing, Rustique ratio, t penalty\n")
    print("| workload | run() 3.15.1 | enabled/plain wall | enabled/plain instr | "
          "3.15.1t/3.15.1 wall | 3.15.1t/3.15.1 instr | t: enabled/plain wall |")
    print("|---|---:|---:|---:|---:|---:|---:|")
    for n in names:
        p = P[n]
        c, ct = C.get(n, {}), CT.get(n, {})
        st, se = p.get("stat", {}), p.get("stat_enabled", {})
        stt = PT.get(n, {}).get("stat", {})
        row = {
            "run_s": c.get("plain_s") or st.get("wall_median_s"),
            "ratio_wall": c.get("ratio"),
            "ratio_instr": (se["instructions_per_run"] / st["instructions_per_run"])
            if se and st else None,
            "t_wall": (ct["plain_s"] / c["plain_s"]) if ct and c else None,
            "t_instr": (stt["instructions_per_run"] / st["instructions_per_run"])
            if stt and st else None,
            "t_ratio_wall": ct.get("ratio"),
            "digest_t_same": (stt.get("digest") == st.get("digest")) if stt and st else None,
        }
        summary[n] = {"timing": row}
        f = lambda v, nd=3: "—" if v is None else f"{v:.{nd}f}"
        print(f"| {n} | {f(row['run_s'], 2)} s | {f(row['ratio_wall'])} | {f(row['ratio_instr'])} | "
              f"{f(row['t_wall'])} | {f(row['t_instr'])} | {f(row['t_ratio_wall'])} |")
    print()

    print("## Layers (3.15.1; plain-perf components, -X perf owners re-weighted)\n")
    print("| workload | " + " | ".join(LAYERS) + " | GC | samples plain / -X |")
    print("|---|" + "---:|" * (len(LAYERS) + 2))
    means = defaultdict(list)
    for n in names:
        x = P[n]["xperf"]
        lay = x["layers"]
        for L in LAYERS:
            means[L].append(lay.get(L, 0.0))
        means["GC"].append(P[n]["plain"]["gc_share"])
        summary[n]["layers"] = lay
        print(f"| {n} | " + " | ".join(pct(lay.get(L, 0.0)) for L in LAYERS)
              + f" | {pct(P[n]['plain']['gc_share'])} | {P[n]['plain']['samples']} / {x['samples']} |")
    print("| **mean** | " + " | ".join(pct(statistics.mean(means[L])) for L in LAYERS)
          + f" | {pct(statistics.mean(means['GC']))} | |\n")

    print("## Plain-perf native components (3.15.1)\n")
    comps = Counter()
    for n in names:
        for k, v in P[n]["plain"]["component"].items():
            comps[k] += v / len(names)
    top = [k for k, _ in comps.most_common(12)]
    print("| workload | " + " | ".join(top) + " |")
    print("|---|" + "---:|" * len(top))
    for n in names:
        cc = P[n]["plain"]["component"]
        print(f"| {n} | " + " | ".join(pct(cc.get(k, 0.0)) for k in top) + " |")
    print("| **mean** | " + " | ".join(pct(comps[k]) for k in top) + " |\n")

    print("## Dominant CPython machinery per workload (exclusive share of run time)\n")
    agg = Counter()
    for n in names:
        items = machinery(P[n])
        summary[n]["machinery"] = items
        for k, v in items:
            agg[k] += v / len(names)
        shown = ", ".join(f"{k} {pct(v)}" for k, v in items[:12] if v >= 0.004)
        print(f"- **{n}**: {shown}")
    print("\n**Family mean**: " + ", ".join(f"{k} {pct(v)}" for k, v in agg.most_common(30)
                                           if v >= 0.002) + "\n")

    print("## Type-slot dispatch of user dunders (inclusive, plain perf stacks)\n")
    for n in names:
        sl = P[n]["plain"].get("slot_inclusive", {})
        shown = ", ".join(f"{k} {pct(v)}" for k, v in list(sl.items())[:8] if v >= 0.003)
        print(f"- **{n}**: {shown or '—'}")
    print()

    print("## Top non-own Python functions (-X perf self, re-weighting not applied)\n")
    for n in names:
        rows = [r for r in P[n]["xperf"]["top_python_self_xperf"] if r[1] != "own"][:8]
        print(f"- **{n}**: " + ", ".join(f"`{r[0]}` {pct(r[2])}" for r in rows))
    print()

    print("## cProfile: machinery call counts per run()\n")
    for n in names:
        cp = P[n].get("cprofile")
        if not cp:
            continue
        calls = {k: v for k, v in cp["owner_calls"].items() if k != "own"}
        print(f"- **{n}** ({cp['total_calls']:,} calls): " + ", ".join(
            f"{k} {v:,}" for k, v in sorted(calls.items(), key=lambda kv: -kv[1])[:8]))
    print()

    if PT:
        print("## Free-threaded build: component deltas (plain perf share x cycles/run)\n")
        print("| workload | t/GIL cycles | top component growth (Gcycles/run) |")
        print("|---|---:|---|")
        for n in names:
            if n not in PT:
                continue
            g, t = P[n], PT[n]
            gc_, tc_ = g["stat"]["cycles_per_run"], t["stat"]["cycles_per_run"]
            delta = Counter()
            for k in set(g["plain"]["component"]) | set(t["plain"]["component"]):
                delta[k] = (t["plain"]["component"].get(k, 0.0) * tc_
                            - g["plain"]["component"].get(k, 0.0) * gc_) / 1e9
            shown = ", ".join(f"{k} {v:+.2f}" for k, v in delta.most_common(5))
            summary[n]["t_delta"] = dict(delta.most_common(8))
            print(f"| {n} | {tc_ / gc_:.3f} | {shown} |")
        print()
        print("t-build top native symbols (plain perf), first 6 per workload:\n")
        for n in names:
            if n in PT:
                print(f"- **{n}**: " + ", ".join(f"`{s}` {pct(v)}" for s, _c, v in
                                                 PT[n]["plain"]["top_native"][:6]))
        print()
    if a.json:
        with open(a.json, "w") as f:
            json.dump(summary, f, indent=1)


if __name__ == "__main__":
    main()
