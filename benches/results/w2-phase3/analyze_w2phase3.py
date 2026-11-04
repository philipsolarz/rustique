"""Tables for docs/atlas/w2-phase3.md from the raw results (run from anywhere; stdlib only).

    python analyze_w2phase3.py workloads [--tag T]   the 65 workloads, default tier: per workload, per family, vs Phase 0 and T1-T4
    python analyze_w2phase3.py tiers                 opt-in tiers (tiers_compare.py): per workload and family, vs plain and default
    python analyze_w2phase3.py status                target status per interpreter (T10)
    python analyze_w2phase3.py conformance           CPython's test modules, every default group at once
    python analyze_w2phase3.py est [--build B]       estimator predictions vs this run's measured ratios (T13)
    python analyze_w2phase3.py whole                 whole-process runs (pyperf command), plain vs RUSTIQUE=1
    python analyze_w2phase3.py durations             step durations
"""

from __future__ import annotations

import argparse
import collections
import glob
import gzip
import json
import math
import os
import statistics

HERE = os.path.dirname(os.path.abspath(__file__))
P0 = os.path.join(os.path.dirname(HERE), "w2-phase0")
FAMS = ("web", "data", "devtools", "frameworks", "libs", "custom", "services", "threads")
W1FAMS = ("web", "data", "devtools")
BUILDS = ("3.15.1", "3.15.1t")
T4 = {"custom": 0.95, "frameworks": 0.98, "libs": 0.95, "services": 0.85}


def gm(xs):
    xs = [x for x in xs if x and x > 0]
    return math.exp(sum(map(math.log, xs)) / len(xs)) if xs else float("nan")


def load(p):
    if p.endswith(".gz"):
        with gzip.open(p, "rt") as f:
            return json.load(f)
    with open(p) as f:
        return json.load(f)


def find(d, name):
    for ext in ("", ".gz"):
        if os.path.exists(os.path.join(d, name + ext)):
            return os.path.join(d, name + ext)
    return None


def rows(d, b, tag="", fams=FAMS):
    out = {}
    for f in fams:
        p = find(os.path.join(d, "workloads"), f"compare-{b}-{f}{tag}.json")
        if p:
            out.update({r["workload"]: r for r in load(p)["rows"]})
    return out


def fam(w):
    return w.split("/")[0]


def mark(ok):
    return "met" if ok else "**not met**"


def cmd_workloads(a):
    new = {b: rows(HERE, b, a.tag) for b in BUILDS}
    old = {b: rows(P0, b, "", W1FAMS) for b in BUILDS}
    names = sorted(set().union(*[set(x) for x in new.values()]), key=lambda w: (FAMS.index(fam(w)), w))
    print("| workload | " + " | ".join(f"{b} plain ms | {b} enabled ms | {b} ratio | {b} Phase 0 | {b} Δ" for b in BUILDS) + " |")
    print("|---|" + "---:|---:|---:|---:|---:|" * len(BUILDS))
    for n in names:
        cells = []
        for b in BUILDS:
            r, o = new[b].get(n), old[b].get(n)
            if r:
                flag = "" if r["same_result"] else " **DIGEST MISMATCH**"
                hi = "**" if r["ratio"] > 1.02 else ""
                cells.append(f"{r['plain_s'] * 1e3:.1f} | {r['enabled_s'] * 1e3:.1f} | {hi}{r['ratio']:.3f}{hi}{flag} | "
                             + (f"{o['ratio']:.3f} | {r['ratio'] / o['ratio'] - 1:+.1%}" if o else "— | —"))
            else:
                cells.append("— | — | — | — | —")
        print(f"| {n} | " + " | ".join(cells) + " |")
    print()
    # per family
    print("| family (n) | " + " | ".join(f"{b} geomean | {b} Phase 0 (wave-1 code)" for b in BUILDS) + " | target (3.15.1 / 3.15.1t) |")
    print("|---|" + "---:|---:|" * len(BUILDS) + "---|")
    for f in FAMS:
        cells = []
        n = 0
        for b in BUILDS:
            rs = [r["ratio"] for w, r in new[b].items() if fam(w) == f]
            os_ = [r["ratio"] for w, r in old[b].items() if fam(w) == f]
            n = max(n, len(rs))
            cells.append(f"{gm(rs):.3f} | " + (f"{gm(os_):.3f}" if os_ else "—"))
        tgt = {"custom": "≤ 0.95", "frameworks": "≤ 0.98", "libs": "≤ 0.95", "services": "≤ 0.85",
               "threads": "— / ≤ 0.78 (8 threads)"}.get(f, "—")
        print(f"| {f} ({n}) | " + " | ".join(cells) + f" | {tgt} |")
    for label, sel, tg in (("wave 1's 34", lambda w: fam(w) in W1FAMS, ("≤ 0.85", "≤ 0.80")),
                           ("the 31 new", lambda w: fam(w) not in W1FAMS, ("—", "—")),
                           ("all 65", lambda w: True, ("≤ 0.88", "≤ 0.84"))):
        cells = []
        for b in BUILDS:
            rs = [r["ratio"] for w, r in new[b].items() if sel(w)]
            os_ = [r["ratio"] for w, r in old[b].items() if sel(w)]
            cells.append(f"**{gm(rs):.3f}** ({len(rs)}) | " + (f"{gm(os_):.3f} ({len(os_)})" if os_ else "—"))
        print(f"| {label} | " + " | ".join(cells) + f" | {tg[0]} / {tg[1]} |")
    print()
    for b in BUILDS:
        rs = new[b]
        if not rs:
            continue
        rat = sorted((r["ratio"], w) for w, r in rs.items())
        over = [(w, round(x, 3)) for x, w in rat if x > 1.02]
        mism = [w for w, r in rs.items() if not r["same_result"]]
        print(f"{b}: n={len(rs)} median {statistics.median(x for x, _ in rat):.3f}; best {rat[0][1]} {rat[0][0]:.3f}; "
              f"worst {rat[-1][1]} {rat[-1][0]:.3f}; <= 0.95: {sum(x <= 0.95 for x, _ in rat)}; > 1.02: {over}; "
              f"> 1.05: {sum(x > 1.05 for x, _ in rat)}; digest mismatches: {mism}")
        p = [r["plain_s"] / old[b][w]["plain_s"] for w, r in rs.items() if w in old[b]]
        if p:
            print(f"   plain medians vs Phase 0 (34): geomean {gm(p):.4f}; enabled-ratio moves > 2%: "
                  f"{[(w, round(r['ratio'] / old[b][w]['ratio'] - 1, 3)) for w, r in rs.items() if w in old[b] and abs(r['ratio'] / old[b][w]['ratio'] - 1) > 0.02]}")
    # targets
    g = {b: {f: gm([r["ratio"] for w, r in new[b].items() if fam(w) == f]) for f in FAMS} for b in BUILDS}
    a34 = {b: gm([r["ratio"] for w, r in new[b].items() if fam(w) in W1FAMS]) for b in BUILDS}
    a65 = {b: gm([r["ratio"] for r in new[b].values()]) for b in BUILDS}
    print()
    print(f"T2 (34): 3.15.1 {a34['3.15.1']:.3f} <= 0.85 {mark(a34['3.15.1'] <= 0.85)}; 3.15.1t {a34['3.15.1t']:.3f} <= 0.80 {mark(a34['3.15.1t'] <= 0.80)}")
    print(f"T3 (65): 3.15.1 {a65['3.15.1']:.3f} <= 0.88 {mark(a65['3.15.1'] <= 0.88)}; 3.15.1t {a65['3.15.1t']:.3f} <= 0.84 {mark(a65['3.15.1t'] <= 0.84)}")
    print("T4: " + "; ".join(f"{f} {g['3.15.1'][f]:.3f} <= {t} {mark(g['3.15.1'][f] <= t)}" for f, t in T4.items())
          + f"; threads 3.15.1t {g['3.15.1t']['threads']:.3f} <= 0.78 {mark(g['3.15.1t']['threads'] <= 0.78)}")
    worst = {b: max(r["ratio"] for r in new[b].values()) for b in BUILDS if new[b]}
    print(f"T1 (workloads): worst {worst}; > 1.02: {[(b, w) for b in BUILDS for w, r in new[b].items() if r['ratio'] > 1.02]}")


def cmd_headline(a):
    """Replicate 2 (final/, 7fec7e4) as the headline, replicate 1 (d40e7b9) and Phase 0 beside it."""
    r2 = {b: rows(os.path.join(HERE, "final"), b) for b in BUILDS}
    r1 = {b: rows(HERE, b) for b in BUILDS}
    p0 = {b: rows(P0, b, "", W1FAMS) for b in BUILDS}
    names = sorted(set(r2["3.15.1"]) | set(r1["3.15.1"]), key=lambda w: (FAMS.index(fam(w)), w))
    print("| workload | " + " | ".join(f"{b} plain ms | {b} enabled ms | {b} **ratio** | {b} rep 1 | {b} Phase 0" for b in BUILDS) + " |")
    print("|---|" + "---:|---:|---:|---:|---:|" * len(BUILDS))
    for n in names:
        cells = []
        for b in BUILDS:
            x, y, z = r2[b].get(n), r1[b].get(n), p0[b].get(n)
            if x:
                fl = "" if x["same_result"] else " **MISMATCH**"
                hi = "**" if x["ratio"] > 1.02 else ""
                cells.append(f"{x['plain_s'] * 1e3:.1f} | {x['enabled_s'] * 1e3:.1f} | {hi}{x['ratio']:.3f}{hi}{fl} | "
                             + (f"{y['ratio']:.3f}" if y else "—") + " | " + (f"{z['ratio']:.3f}" if z else "—"))
            else:
                cells.append("— | — | — | " + (f"{y['ratio']:.3f}" if y else "—") + " | —")
        print(f"| {n} | " + " | ".join(cells) + " |")
    print()
    print("| family (n) | " + " | ".join(f"{b} rep 2 | {b} rep 1 | {b} mean of 2 | {b} Phase 0" for b in BUILDS) + " | target |")
    print("|---|" + "---:|---:|---:|---:|" * len(BUILDS) + "---|")
    groups = [(f, lambda w, f=f: fam(w) == f) for f in FAMS] + [
        ("**wave 1's 34**", lambda w: fam(w) in W1FAMS), ("the 31 new", lambda w: fam(w) not in W1FAMS),
        ("**all 65**", lambda w: True)]
    tg = {"custom": "≤ 0.95", "frameworks": "≤ 0.98", "libs": "≤ 0.95", "services": "≤ 0.85",
          "threads": "t ≤ 0.78", "**wave 1's 34**": "≤ 0.85 / ≤ 0.80", "**all 65**": "≤ 0.88 / ≤ 0.84"}
    out = {}
    for label, sel in groups:
        cells = []
        for b in BUILDS:
            g2 = gm([x["ratio"] for w, x in r2[b].items() if sel(w)])
            g1 = gm([x["ratio"] for w, x in r1[b].items() if sel(w)])
            both = gm([gm([r2[b][w]["ratio"], r1[b][w]["ratio"]]) for w in r2[b] if sel(w) and w in r1[b]])
            g0 = gm([x["ratio"] for w, x in p0[b].items() if sel(w)])
            n2 = sum(1 for w in r2[b] if sel(w))
            out[(label, b)] = (g2, g1, both)
            cells.append(f"**{g2:.3f}** ({n2}) | {g1:.3f} | {both:.3f} | " + (f"{g0:.3f}" if g0 == g0 else "—"))
        print(f"| {label} | " + " | ".join(cells) + f" | {tg.get(label, '—')} |")
    print()
    for b in BUILDS:
        rs = r2[b]
        if not rs:
            continue
        rat = sorted((x["ratio"], w) for w, x in rs.items())
        agree = [(w, round(rs[w]["ratio"] / r1[b][w]["ratio"] - 1, 3)) for w in rs if w in r1[b]
                 and abs(rs[w]["ratio"] / r1[b][w]["ratio"] - 1) > 0.02]
        pl = gm([rs[w]["plain_s"] / r1[b][w]["plain_s"] for w in rs if w in r1[b]])
        print(f"{b} rep 2: n={len(rs)} median {statistics.median(x for x, _ in rat):.3f}; best {rat[0][1]} {rat[0][0]:.3f}; "
              f"worst {rat[-1][1]} {rat[-1][0]:.3f}; > 1.02: {[(w, round(x, 3)) for x, w in rat if x > 1.02]}; "
              f"mismatches: {[w for w, x in rs.items() if not x['same_result']]}; plain rep2/rep1 geomean {pl:.4f}; "
              f"ratio moves > 2% vs rep 1: {agree}")


def tier_rows():
    out = collections.defaultdict(dict)  # (sides, build) -> workload -> row
    for p in sorted(glob.glob(os.path.join(HERE, "tiers", "tiers-*.json*"))):
        base = os.path.basename(p).split(".json")[0]  # tiers-<tag>-<b>-<family>
        parts = base.split("-")
        tag, b = "-".join(parts[1:-2]), parts[-2]
        d = load(p)
        for r in d["rows"]:
            out[(tag, b)][r["workload"]] = r
    return out


def cmd_tiers(a):
    tr = tier_rows()
    base = {b: rows(HERE, b) for b in BUILDS}
    for (tag, b), rs in sorted(tr.items()):
        sides = [s for s in next(iter(rs.values()))["median_s"] if s != "plain"]
        tiers = [s for s in sides if s != "default"]
        print(f"### {tag} on {b} ({len(rs)} workloads)\n")
        print("| workload | plain ms | " + " | ".join(f"{s} ms" for s in sides) + " | "
              + " | ".join(f"{s} ÷ plain" for s in sides) + " | " + " | ".join(f"{s} ÷ default" for s in tiers)
              + " | default (headline run) | tier targets per process |")
        print("|---|---:|" + "---:|" * len(sides) * 2 + "---:|" * len(tiers) + "---:|---|")
        for w in sorted(rs, key=lambda w: (FAMS.index(fam(w)), w)):
            r = rs[w]
            hb = base[b].get(w)
            flag = "" if r["same_result"] else " **DIGEST MISMATCH**"
            line = (f"| {w}{flag} | {r['median_s']['plain'] * 1e3:.1f} | "
                    + " | ".join(f"{r['median_s'][s] * 1e3:.1f}" for s in sides) + " | "
                    + " | ".join(f"{r['vs_plain'][s]:.3f}" for s in sides) + " | "
                    + " | ".join(f"{r['vs_default'][s]:.3f}" for s in tiers)
                    + (f" | {hb['ratio']:.3f}" if hb else " | —") + " | "
                    + "; ".join(f"{s}: {r['tiers'].get(s)}" for s in tiers) + " |")
            print(line)
        print()
        print("| family (n) | " + " | ".join(f"{s} ÷ plain" for s in sides) + " | " + " | ".join(f"{s} ÷ default" for s in tiers) + " |")
        print("|---|" + "---:|" * (len(sides) + len(tiers)))
        for f in FAMS + ("all",):
            sel = [r for w, r in rs.items() if f == "all" or fam(w) == f]
            if not sel:
                continue
            print(f"| {f} ({len(sel)}) | " + " | ".join(f"{gm([r['vs_plain'][s] for r in sel]):.3f}" for s in sides) + " | "
                  + " | ".join(f"{gm([r['vs_default'][s] for r in sel]):.3f}" for s in tiers) + " |")
        print()
        worst = {s: max((r["vs_default"][s], w) for w, r in rs.items()) for s in tiers}
        print("worst vs default: " + "; ".join(f"{s} {w} {x:.3f}" for s, (x, w) in worst.items()))
        print("over 1.02 vs default: " + "; ".join(f"{s}: {[(w, round(r['vs_default'][s], 3)) for w, r in rs.items() if r['vs_default'][s] > 1.02]}" for s in tiers))
        states = collections.Counter()
        for r in rs.values():
            for s in tiers:
                for k in (r.get("tiers", {}).get(s) or {}):
                    states[(s, k)] += 1
        print("tier target states seen (workloads):", dict(states))
        print()


def cmd_status(a):
    rows_ = []
    for p in sorted(glob.glob(os.path.join(HERE, "status", "status-*.json"))):
        name = os.path.basename(p)[len("status-"):-len(".json")]
        if name.startswith(("tiers-", "unvalidated-")):
            continue
        s = load(p)["summary"]
        t = find(os.path.join(HERE, "status"), f"status-tiers-{name}.json")
        ts = load(t)["summary"]["counts_tiers"] if t else {}
        u = find(os.path.join(HERE, "status"), f"status-unvalidated-{name}.json")
        us = load(u)["summary"]["installed_default"] if u else None
        cats = s["refusal_categories_default"]
        rows_.append((name, s, ts, us, cats))
    print("| interpreter | default targets installed | refused (category) | interp tier (RUSTIQUE_TIERS=interp) | gc tier | with RUSTIQUE_UNVALIDATED_PATCH=1 |")
    print("|---|---:|---|---|---|---:|")
    for name, s, ts, us, cats in rows_:
        tiers = {k: ", ".join(f"{n} {st}" for st, n in v.items()) for k, v in ts.items()}
        other = {k: v for k, v in s["counts_default"].items() if k not in ("installed", "refused")}
        print(f"| {name} | {s['installed_default']} / {s['default_targets']} | "
              + ", ".join([f"{n} `{c}`" for c, n in cats.items()] + [f"{n} {k}" for k, n in other.items()])
              + f" | {tiers.get('interp', '—')} | {tiers.get('gc', '—')} | "
              + (str(us) if us is not None else "—") + " |")
    print()
    for name, s, ts, us, cats in rows_:
        reasons = collections.Counter((r["refusal"], r["reason"]) for r in s["refused"] if not r.get("tier"))
        for (c, why), n in sorted(reasons.items(), key=lambda x: -x[1])[:6]:
            print(f"- {name}: {n} × `{c}`: {why}")


def cmd_conformance(a):
    res = {}
    for p in sorted(glob.glob(os.path.join(HERE, "conformance", "all-groups-*.json*"))):
        b = os.path.basename(p).split("all-groups-")[1].split(".json")[0]
        res[b] = load(p)
    builds = [b for b in ("3.15.1", "3.15.1t", "3.14.9", "3.14.9t") if b in res] + [b for b in res if b not in BUILDS + ("3.14.9", "3.14.9t")]
    mods = list(next(iter(res.values()))["modules"])
    print("| module | " + " | ".join(builds) + " |")
    print("|---|" + "---|" * len(builds))
    tot, diffs = collections.Counter(), collections.Counter()
    for m in mods:
        cells = []
        for b in builds:
            md = res[b]["modules"].get(m)
            if not md:
                cells.append("—")
                continue
            c = md["counts"]
            nd = len(md["differences"])
            tot[b] += sum(c.values())
            diffs[b] += nd
            st = "" if md["plain"] == md["enabled"] == "ran" else f" ({md['plain']}/{md['enabled']})"
            cells.append(", ".join(f"{n} {k}" for k, n in sorted(c.items(), key=lambda x: -x[1])) + st
                         + (f"; **{nd} diff**" if nd else "; 0 diff"))
        print(f"| {m.split('.')[-1]} | " + " | ".join(cells) + " |")
    print()
    for b in builds:
        print(f"{b}: {tot[b]} outcomes, {diffs[b]} differences, ok={res[b]['ok']}, not installed={res[b]['not_installed']}, "
              f"targets={len(res[b]['targets']) if isinstance(res[b]['targets'], list) else res[b]['targets']}")


def cmd_est(a):
    meas = rows(HERE, a.build)
    out = []
    for p in sorted(glob.glob(os.path.join(HERE, "est", a.build, "est-*.json*"))):
        for r in load(p):
            m = meas.get(r["workload"])
            out.append((r["workload"], r["predicted"], m["ratio"] if m else None))
    errs = [(w, p - m) for w, p, m in out if m is not None]
    print("| workload | predicted | measured (this run) | error |")
    print("|---|---:|---:|---:|")
    for w, p, m in sorted(out, key=lambda x: (FAMS.index(fam(x[0])), x[0])):
        e = p - m if m is not None else None
        print(f"| {w} | {p:.3f} | {m:.3f} | " + (f"{'**' if abs(e) > 0.03 else ''}{e:+.3f}{'**' if abs(e) > 0.03 else ''}" if e is not None else "—") + " |")
    if errs:
        n = len(errs)
        w3 = sum(abs(e) <= 0.03 for _, e in errs)
        print(f"\n{n} workloads: within ±0.03: {w3} ({w3 / n:.0%}), ±0.02: {sum(abs(e) <= 0.02 for _, e in errs)}, "
              f"±0.05: {sum(abs(e) <= 0.05 for _, e in errs)}; median |error| {statistics.median(abs(e) for _, e in errs):.3f}, "
              f"mean {statistics.mean(abs(e) for _, e in errs):.3f}, bias {statistics.mean(e for _, e in errs):+.3f}; "
              f"T13 (>= 80% within ±0.03): {mark(w3 / n >= 0.8)}")
        per = collections.defaultdict(lambda: [0, 0])
        for w, e in errs:
            per[fam(w)][0] += abs(e) <= 0.03
            per[fam(w)][1] += 1
        print("per family:", {f: f"{x[0]}/{x[1]}" for f, x in per.items()})
        print("misses:", [(w, round(e, 3)) for w, e in errs if abs(e) > 0.03])


def cmd_whole(a):
    for b in BUILDS:
        p = find(os.path.join(HERE, "whole"), f"whole-{b}.json")
        if not p:
            continue
        d = load(p)  # pyperf --append: one suite
        vals = {}
        for bench in d["benchmarks"]:
            name = bench["metadata"]["name"]
            vals[name] = [x for run in bench["runs"] for x in run.get("values", [])]
        ws = sorted({n.rsplit("/", 1)[0] for n in vals}, key=lambda w: (FAMS.index(fam(w)), w))
        print("| build | workload | plain s (mean ± std) | RUSTIQUE=1 s | ratio (means) | ratio (medians) | values |")
        print("|---|---|---:|---:|---:|---:|---:|")
        rm, rd = [], []
        for w in ws:
            p_, e = vals.get(f"{w}/plain", []), vals.get(f"{w}/enabled", [])
            if not p_ or not e:
                continue
            r = statistics.mean(e) / statistics.mean(p_)
            r2 = statistics.median(e) / statistics.median(p_)
            rm.append(r); rd.append(r2)
            print(f"| {b} | {w} | {statistics.mean(p_):.3f} ± {statistics.stdev(p_):.3f} | {statistics.mean(e):.3f} ± {statistics.stdev(e):.3f} | "
                  f"{r:.3f} | {r2:.3f} | {len(p_)}/{len(e)} |")
        print(f"| {b} | **geomean** | | | **{gm(rm):.3f}** | **{gm(rd):.3f}** | |\n")


def pp_means(path):
    d = load(path)
    out = {}
    for bench in d["benchmarks"]:
        name = bench.get("metadata", {}).get("name") or d["metadata"]["name"]  # one-benchmark files
        vals = [x for run in bench["runs"] for x in run.get("values", [])]
        if vals:
            out[name] = statistics.mean(vals)
    return out


def cmd_pp(a):
    """pyperformance: enabled / plain per benchmark (means), this run and Phase 0 (A/B pairs)."""
    pdir = os.path.join(HERE, "pyperformance")
    p0dir = os.path.join(P0, "pyperformance")
    for b in BUILDS:
        pl, en = find(pdir, f"plain-{b}.json"), find(pdir, f"enabled-{b}.json")
        if not (pl and en):
            continue
        P, E = pp_means(pl), pp_means(en)
        for tag in ("-ports", "-sphinx"):  # benchmarks re-run alone after a collision in the full runs
            pp, pe = find(pdir, f"plain-{b}{tag}.json"), find(pdir, f"enabled-{b}{tag}.json")
            if pp and pe:
                P.update(pp_means(pp)); E.update(pp_means(pe))
        solo = {}
        for tag in ("-solo1", "-solo2"):
            pp, pe = find(pdir, f"plain-{b}{tag}.json"), find(pdir, f"enabled-{b}{tag}.json")
            if pp and pe:
                SP, SE = pp_means(pp), pp_means(pe)
                for n in SP:
                    if n in SE:
                        solo.setdefault(n, []).append(SE[n] / SP[n])
        r = {n: E[n] / P[n] for n in P if n in E}
        old = {}
        for tag in ("", "-B"):
            op, oe = find(p0dir, f"plain-{b}{tag}.json"), find(p0dir, f"enabled-{b}{tag}.json")
            if op and oe:
                OP, OE = pp_means(op), pp_means(oe)
                for n in OP:
                    if n in OE:
                        old.setdefault(n, []).append(OE[n] / OP[n])
        old = {n: gm(v) for n, v in old.items()}
        common = [n for n in r if n in old]
        print(f"### {b}: {len(r)} benchmarks, geomean **{gm(list(r.values())):.3f}** "
              f"(Phase 0 on the same {len(common)}: {gm([old[n] for n in common]):.3f}, this run {gm([r[n] for n in common]):.3f}); "
              f"median {statistics.median(r.values()):.3f}; < 0.95: {sum(x < 0.95 for x in r.values())}; "
              f"> 1.02: {sum(x > 1.02 for x in r.values())}; > 1.05: {sum(x > 1.05 for x in r.values())}")
        print("faster than 0.95: " + ", ".join(f"`{n}` {x:.3f}" for n, x in sorted(r.items(), key=lambda kv: kv[1]) if x < 0.95))
        print("slower than 1.02: " + ", ".join(f"`{n}` {x:.3f}" + (f" (Phase 0 {old[n]:.3f})" if n in old else "")
                                         for n, x in sorted(r.items(), key=lambda kv: -kv[1]) if x > 1.02))
        if solo:
            print("re-run alone (two pairs, opposite order): " + ", ".join(f"`{n}` {' / '.join(f'{x:.3f}' for x in v)} (full run {r.get(n, float('nan')):.3f})" for n, v in sorted(solo.items())))
        missing = sorted(set(P) ^ set(E))
        print(f"in one side only: {missing}")
        moved = sorted(((n, r[n], old[n]) for n in common if abs(r[n] / old[n] - 1) > 0.03), key=lambda t: t[1] / t[2])
        print("moved > 3% vs Phase 0: " + ", ".join(f"`{n}` {x:.3f} (was {y:.3f})" for n, x, y in moved))
        print()


def cmd_durations(a):
    p = os.path.join(HERE, "durations.tsv")
    print("| step | start | end | s | rc |")
    print("|---|---|---|---:|---:|")
    for line in open(p):
        n, s, e, d, rc = line.rstrip("\n").split("\t")
        print(f"| {n} | {s[11:19]} | {e[11:19]} | {d} | {rc} |")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--res", help="results directory (default: this script's; e.g. textfix/ for the 445de19 re-run)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("workloads"); s.add_argument("--tag", default="")
    sub.add_parser("tiers")
    sub.add_parser("headline")
    sub.add_parser("pp")
    sub.add_parser("status")
    sub.add_parser("conformance")
    s = sub.add_parser("est"); s.add_argument("--build", default="3.15.1")
    sub.add_parser("whole")
    sub.add_parser("durations")
    a = ap.parse_args()
    if a.res:
        global HERE
        HERE = os.path.abspath(a.res)
    globals()["cmd_" + a.cmd](a)


if __name__ == "__main__":
    main()
