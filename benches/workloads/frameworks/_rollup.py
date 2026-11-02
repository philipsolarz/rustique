#!/usr/bin/env python3
"""Roll up ``_layers.py`` profiles of the frameworks/libs/services families into the
markdown tables of docs/atlas/w2-wl-frameworks.md.

Not a workload (leading underscore).

    python benches/workloads/frameworks/_rollup.py <prof_dir> [--compare compare.json]
        [--steady <steady_dir>] > tables.md

Reads ``<prof_dir>/<workload>.json`` (from ``_layers.py``), optionally run.py's compare
JSON (wall ratios plain vs ``rustique.enable()``) and ``services/_steady.py`` outputs
(``<name>-plain.json``, ``<name>-enabled.json``, ``<name>-t-plain.json``).
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import statistics
from collections import Counter, defaultdict

FAMILIES = {
    "frameworks": ["drf_api", "django_admin", "flask_marshmallow", "fastapi_sqlalchemy", "strawberry_graphql", "falcon_api"],
    "libs": ["boto3_client", "pydantic_payloads", "jsonschema_validate", "yaml_config", "attrs_cattrs", "bs4_scrape"],
    "services": ["uvicorn_fastapi", "waitress_flask", "job_worker", "cache_service"],
}
LAYERS = ["own", "generated", "language", "stdlib", "thirdparty", "runtime"]


def pct(x, nd=1):
    return f"{100 * x:.{nd}f}%"


def load(prof_dir):
    out = {}
    for fam, names in FAMILIES.items():
        for n in names:
            for suffix in (".v2.json", ".json", ".new.json"):  # newest profile first
                p = os.path.join(prof_dir, n + suffix)
                if os.path.exists(p):
                    break
            if os.path.exists(p):
                with open(p) as f:
                    d = json.load(f)
                d["family"] = fam
                out[n] = d
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("prof_dir")
    ap.add_argument("--compare")
    ap.add_argument("--steady")
    a = ap.parse_args()
    P = load(a.prof_dir)
    ratios = {}
    if a.compare and os.path.exists(a.compare):
        for r in json.load(open(a.compare))["rows"]:
            ratios[r["workload"].split("/")[1]] = r

    # 1. per-workload layers
    print("### Layers per workload (share of steady-state CPU samples; plain-perf components, -X perf owners)\n")
    print("| workload | kernel (sys) | own | generated | language | stdlib | thirdparty | runtime (alloc / gc) | top distributions (all layers under their frames) | Rustique instr ratio |")
    print("|---|---:|---:|---:|---:|---:|---:|---|---|---:|")
    fam_layers = defaultdict(lambda: defaultdict(list))
    for n, d in P.items():
        x = d["xperf"]
        L = x["layers"]
        st = d.get("stat", {})
        u, s = st.get("utime_per_run_s", 0), st.get("stime_per_run_s", 0)
        rt = x["runtime"]
        dists = ", ".join(f"{k} {pct(v, 0)}" for k, v in list(x["dist_total"].items())[:3])
        r = ratios.get(n)
        if "stat_enabled" in d:
            d["instr_ratio_enabled"] = round(d["stat_enabled"]["instructions_per_run"] / st["instructions_per_run"], 3)
        print(f"| `{n}` | {pct(s / (u + s) if u + s else 0, 0)} | "
              + " | ".join(pct(L.get(k, 0)) for k in LAYERS[:5])
              + f" | {pct(L.get('runtime', 0))} ({pct(rt.get('alloc/dealloc', 0), 0)} / {pct(rt.get('gc', 0), 0)}) | {dists} | "
              + f"{d.get('instr_ratio_enabled', '–')} |")
        for k in LAYERS:
            fam_layers[d["family"]][k].append(L.get(k, 0))
            fam_layers["all"][k].append(L.get(k, 0))
    print("\n### Family means\n")
    print("| family | n | " + " | ".join(LAYERS) + " |")
    print("|---|---:|" + "---:|" * len(LAYERS))
    for fam in ["frameworks", "libs", "services", "all"]:
        if fam_layers[fam]["own"]:
            print(f"| {fam} | {len(fam_layers[fam]['own'])} | " + " | ".join(pct(statistics.fmean(fam_layers[fam][k])) for k in LAYERS) + " |")

    # 2. third-party distributions
    print("\n### Third-party distributions: time under their frames, and where it goes\n")
    print("| distribution | workloads | share mean / max (all layers under its frames) | own bytecode | language | stdlib | generated | runtime | native ext |")
    print("|---|---|---|---:|---:|---:|---:|---:|---:|")
    dist = defaultdict(list)
    for n, d in P.items():
        x = d["xperf"]
        for c, v in x["per_caller"].items():
            if c.startswith("thirdparty:"):
                b = v["by_layer"]
                native = sum(val for lay, lab, val in v["top"] if lab.startswith("ext:"))
                dist[c[11:]].append((n, v["total"], b, native))
    rows = []
    for k, lst in dist.items():
        tot = sum(t for _, t, _, _ in lst)
        rows.append((tot, k, lst))
    rows.sort(reverse=True)
    nwl = len(P)
    for tot, k, lst in rows[:22]:
        mx = max(lst, key=lambda r: r[1])
        agg = Counter()
        for _, t, b, nat in lst:
            for lay, val in b.items():
                agg[lay] += val
            agg["native"] += nat
        T = sum(t for _, t, _, _ in lst) or 1
        print(f"| {k} | {len(lst)} | {pct(tot / nwl)} / {pct(mx[1], 0)} ({mx[0]}) | {pct((agg['thirdparty'] - agg['native']) / T, 0)} | "
              f"{pct(agg['language'] / T, 0)} | {pct(agg['stdlib'] / T, 0)} | {pct(agg['generated'] / T, 0)} | "
              f"{pct(agg['runtime'] / T, 0)} | {pct(agg['native'] / T, 0)} |")

    # 2b. per distribution: its top CPython entry points
    print("\n### Per distribution: the CPython-provided entry points it spends time in (share of the workload's samples)\n")
    print("| distribution (workload) | under its frames | top entry points (layer: entry share) |")
    print("|---|---:|---|")
    best = []
    for n, d in P.items():
        for c, v in d["xperf"]["per_caller"].items():
            if c.startswith("thirdparty:") and v["total"] >= 0.04:
                best.append((v["total"], c[11:], n, v))
    for tot, dname, n, v in sorted(best, reverse=True)[:24]:
        items = [f"{lay}: `{lab}` {pct(val)}" for lay, lab, val in v["top"] if lab != "bytecode"][:6]
        print(f"| {dname} ({n}) | {pct(tot, 0)} | " + "; ".join(items) + " |")

    # 3. CPython entry points under third-party (and own) callers
    print("\n### CPython-provided entry points entered from third-party and own code (mean share over all workloads)\n")
    ent = defaultdict(lambda: [0.0, 0.0, "", Counter()])
    for n, d in P.items():
        for c, v in d["xperf"]["per_caller"].items():
            if not (c.startswith("thirdparty:") or c == "own"):
                continue
            for lay, lab, val in v["top"]:
                if lay in ("thirdparty", "own"):
                    continue
                key = (lay, lab)
                e = ent[key]
                e[0] += val / nwl
                if val > e[1]:
                    e[1], e[2] = val, n
                e[3][c.removeprefix("thirdparty:")] += val
    print("| layer | entry point | mean | max (workload) | main callers |")
    print("|---|---|---:|---|---|")
    for (lay, lab), (m, mx, wn, callers) in sorted(ent.items(), key=lambda kv: -kv[1][0])[:45]:
        cs = ", ".join(k for k, _ in callers.most_common(3))
        print(f"| {lay} | `{lab}` | {pct(m, 2)} | {pct(mx, 1)} ({wn}) | {cs} |")

    # 4. stdlib modules, language components, generated
    for title, key in (("stdlib modules (Python frames and C modules entered directly)", "stdlib_modules"),
                       ("language components (leaf)", "language_components"),
                       ("language entries (C entry point reached from bytecode)", "language_entries"),
                       ("generated code", "generated"), ("runtime", "runtime"),
                       ("kernel samples by layer", "kernel_by_layer")):
        acc = defaultdict(lambda: [0.0, 0.0, ""])
        for n, d in P.items():
            for k, v in d["xperf"].get(key, {}).items():
                e = acc[k]
                e[0] += v / nwl
                if v > e[1]:
                    e[1], e[2] = v, n
        print(f"\n### {title}\n")
        print("| item | mean | max (workload) |")
        print("|---|---:|---|")
        for k, (m, mx, wn) in sorted(acc.items(), key=lambda kv: -kv[1][0])[:25]:
            print(f"| `{k}` | {pct(m, 2)} | {pct(mx, 1)} ({wn}) |")

    # 5. steady state
    if a.steady and os.path.isdir(a.steady):
        print("\n### Steady state (services/_steady.py, 60 iterations in one process)\n")
        print("| workload | build | steady wall s | steady CPU s | warm s | RSS after setup / it 1 / end (MB) | growth MB/100 it | peak RSS MB | GC collections per iteration (gen0/1/2) | digest constant |")
        print("|---|---|---:|---:|---:|---|---:|---:|---|---|")
        for f in sorted(glob.glob(os.path.join(a.steady, "*.json"))):
            d = json.load(open(f))
            tag = os.path.basename(f)[:-5].split("-", 1)[1]
            r = d["rss_mb"]
            half = d["rows"][len(d["rows"]) // 2:]
            wall = statistics.median(x["wall_s"] for x in half)
            cpu = statistics.median(x["cpu_s"] for x in half)
            print(f"| `{d['workload'].split('/')[1]}` | {tag} | {wall:.3f} | {cpu:.3f} | {d['warm_s']:.3f} | "
                  f"{r['setup']} / {r['iter1']} / {r['end']} | {d['rss_growth_mb_per_100']} | {d['peak_rss_mb']} | "
                  f"{' / '.join(str(x) for x in d['gc_per_iter'])} | {d['digest_constant']} |")


if __name__ == "__main__":
    main()
