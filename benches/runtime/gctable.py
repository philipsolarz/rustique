#!/usr/bin/env python3
"""Tables from gcprobe sweeps.

    python benches/runtime/gctable.py base DIR [--python 3.15.1] [--md]
        per workload, one policy (default): wall, GC share, per-generation collections and
        time per run, pauses (max, p99), objects collected vs examined, peak RSS
    python benches/runtime/gctable.py policies DIR --base default [--python 3.15.1] [--md]
        per workload x policy vs base: wall ratio (median of per-round paired ratios),
        GC time saved, peak-RSS ratio, max / p99 pause ratio, pending/frozen garbage
    python benches/runtime/gctable.py json DIR -o out.json     (all cells, condensed)

Numbers are per steady-state run() (median over all measured runs of all rounds); pauses
pool every collection of every measured run.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import statistics as st


def load(d):
    cells = {}
    for f in glob.glob(os.path.join(d, "*", "*", "*.json")):
        py = os.path.basename(os.path.dirname(os.path.dirname(f)))
        wl = os.path.basename(os.path.dirname(f)).replace("__", "/")
        pol, rnd, _ = os.path.basename(f).rsplit(".", 2)
        try:
            with open(f) as fh:
                cells.setdefault((py, wl, pol), {})[int(rnd)] = json.load(fh)
        except (OSError, ValueError):
            pass
    return cells


def pct(xs, q):
    if not xs:
        return 0.0
    xs = sorted(xs)
    k = min(len(xs) - 1, max(0, int(round(q * (len(xs) - 1)))))
    return xs[k]


def mmu_worst(runs, window):
    """Max GC time inside any window of `window` seconds (pooled over runs): the
    'worst-window GC load' (1 - minimum mutator utilisation) for that window."""
    worst = 0.0
    for r in runs:
        cols = sorted((c[1], c[1] + c[2]) for c in (r.get("collections") or [])
                      if not (len(c) > 6 and c[6]))
        j = 0
        for i, (s, _e) in enumerate(cols):
            # window [s, s + window): sum overlap of collections starting at i..
            tot = 0.0
            k = i
            while k < len(cols) and cols[k][0] < s + window:
                tot += min(cols[k][1], s + window) - cols[k][0]
                k += 1
            worst = max(worst, tot)
    return worst / window if window else 0.0


def summarize_cell(rounds):
    runs = [r for d in rounds.values() for r in d["runs"]]
    if not runs:
        return None
    wall = st.median(r["wall_s"] for r in runs)
    gens = {}
    for g in ("0", "1", "2"):
        n = [r["gc"].get(g, {}).get("n", 0) for r in runs]
        t = [r["gc"].get(g, {}).get("time_s", 0.0) for r in runs]
        c = [r["gc"].get(g, {}).get("collected", 0) for r in runs]
        cand = [r["gc"].get(g, {}).get("candidates", 0) for r in runs]
        gens[g] = {"n": st.median(n), "time_s": st.median(t), "collected": st.median(c),
                   "candidates": st.median(cand)}
    xg = {"n": st.median(sum(v["n"] for k, v in r["gc"].items() if k.startswith("x")) for r in runs),
          "time_s": st.median(sum(v["time_s"] for k, v in r["gc"].items() if k.startswith("x")) for r in runs),
          "collected": st.median(sum(v["collected"] for k, v in r["gc"].items() if k.startswith("x")) for r in runs),
          "candidates": st.median(sum(v["candidates"] for k, v in r["gc"].items() if k.startswith("x")) for r in runs)}
    gens["x"] = xg
    # automatic collections only (what a policy can change); explicit gc.collect() apart
    gct = [sum(v.get("time_s", 0.0) for k, v in r["gc"].items() if not k.startswith("x")) for r in runs]
    gct_all = [sum(v.get("time_s", 0.0) for v in r["gc"].values()) for r in runs]
    auto_cols = lambda r: [c for c in (r.get("collections") or []) if not (len(c) > 6 and c[6])]
    pauses = [c[2] for r in runs for c in auto_cols(r)]
    # deterministic proxies (immune to box load): objects examined per run, and per collection
    sizes = [c[5] for r in runs for c in auto_cols(r)]
    work = [sum(v.get("candidates", 0) for k, v in r["gc"].items() if not k.startswith("x")) for r in runs]
    hwm = [r["hwm_kb"] for r in runs if r["hwm_kb"] > 0]
    any0 = next(iter(rounds.values()))
    return {
        "wall_s": wall, "walls": [r["wall_s"] for r in runs],
        "gc_s": st.median(gct), "gc_share": st.median(g / r["wall_s"] for g, r in zip(gct, runs)),
        "gc_all_s": st.median(gct_all),
        "gc_all_share": st.median(g / r["wall_s"] for g, r in zip(gct_all, runs)),
        "gens": gens,
        "pause_max_s": max(pauses) if pauses else 0.0, "pause_p99_s": pct(pauses, 0.99),
        "pause_p50_s": pct(pauses, 0.5), "n_pauses_per_run": len(pauses) / len(runs),
        "work": st.median(work), "size_max": max(sizes) if sizes else 0, "size_p99": pct(sizes, 0.99),
        "mmu10ms": mmu_worst(runs, 0.010), "mmu100ms": mmu_worst(runs, 0.100),
        "hwm_kb": max(hwm) if hwm else -1, "hwm_med_kb": st.median(hwm) if hwm else -1,
        "maxrss_kb": max(d["maxrss_kb"] for d in rounds.values()),
        "tracked": st.median(d["tracked_after_warmup"] for d in rounds.values()),
        "pending_garbage": max(d["pending_garbage"] for d in rounds.values()),
        "frozen_garbage": max(d["frozen_garbage"] for d in rounds.values()),
        "frozen": max(d.get("frozen_at_end", 0) for d in rounds.values()),
        "digest_ok": all(d["digest_ok"] for d in rounds.values()),
        "digests": sorted({d["digest"] for d in rounds.values()}),
        "rounds": {k: [r["wall_s"] for r in v["runs"]] for k, v in rounds.items()},
        "policy_report": any0.get("policy_report"),
    }


def ms(x):
    return f"{x * 1e3:.1f}"


def cmd_base(a):
    cells = load(a.dir)
    rows = []
    for (py, wl, pol), rounds in sorted(cells.items()):
        if py != a.python or pol != a.policy:
            continue
        s = summarize_cell(rounds)
        if s:
            rows.append((wl, s))
    hdr = ("workload | run ms | auto GC % | gen0 n / ms | gen1 n / ms | gen2 n / ms | explicit n / ms | "
           "collected/run | examined/run | yield % | pause max ms | p99 ms | GC load worst 100 ms | tracked | peak RSS MB")
    print(hdr)
    print("|".join(["---"] * hdr.count("|")) + "|---")
    shares = []
    for wl, s in rows:
        g = s["gens"]
        col = sum(v["collected"] for k, v in g.items() if k != "x")
        cand = sum(v["candidates"] for k, v in g.items() if k != "x")
        shares.append(s["gc_share"])
        print(f"{wl} | {ms(s['wall_s'])} | {s['gc_share'] * 100:.1f} | "
              f"{g['0']['n']:.0f} / {ms(g['0']['time_s'])} | {g['1']['n']:.0f} / {ms(g['1']['time_s'])} | "
              f"{g['2']['n']:.0f} / {ms(g['2']['time_s'])} | {g['x']['n']:.0f} / {ms(g['x']['time_s'])} | "
              f"{col:.0f} | {cand:.0f} | "
              f"{100 * col / cand if cand else 0:.2f} | {ms(s['pause_max_s'])} | {ms(s['pause_p99_s'])} | "
              f"{s['mmu100ms'] * 100:.0f}% | {s['tracked']:.0f} | {s['hwm_kb'] / 1024:.0f}")
    if shares:
        print(f"mean GC share {100 * st.mean(shares):.1f}%  median {100 * st.median(shares):.1f}%  "
              f"max {100 * max(shares):.1f}%  (n={len(shares)})")


def paired_ratio(base, pol):
    rs = []
    for k in set(base["rounds"]) & set(pol["rounds"]):
        rs.append(st.median(pol["rounds"][k]) / st.median(base["rounds"][k]))
    return (st.median(rs), min(rs), max(rs)) if rs else (float("nan"),) * 3


def cmd_policies(a):
    cells = load(a.dir)
    by = {}
    for (py, wl, pol), rounds in cells.items():
        if py != a.python:
            continue
        s = summarize_cell(rounds)
        if s:
            by.setdefault(wl, {})[pol] = s
    pols = sorted({p for v in by.values() for p in v if p != a.base})
    for pol in pols:
        print(f"\n### {pol} vs {a.base} ({a.python})\n")
        print("workload | ratio (min–max) | GC ms base → pol | GC saved % of run | peak RSS × | "
              "pause max base → pol ms | p99 base → pol ms | pending / frozen garbage")
        print("---|---|---|---|---|---|---|---")
        ratios = []
        for wl in sorted(by):
            b, p = by[wl].get(a.base), by[wl].get(pol)
            if not b or not p:
                continue
            r, lo, hi = paired_ratio(b, p)
            ratios.append(r)
            print(f"{wl} | {r:.3f} ({lo:.2f}–{hi:.2f}) | {ms(b['gc_s'])} → {ms(p['gc_s'])} | "
                  f"{100 * (b['gc_s'] - p['gc_s']) / b['wall_s']:.1f} | "
                  f"{p['hwm_kb'] / b['hwm_kb'] if b['hwm_kb'] > 0 else float('nan'):.3f} | "
                  f"{ms(b['pause_max_s'])} → {ms(p['pause_max_s'])} | {ms(b['pause_p99_s'])} → {ms(p['pause_p99_s'])} | "
                  f"{p['pending_garbage']} / {p['frozen_garbage']}{'' if p['digest_ok'] else ' DIGEST!'}")
        if ratios:
            print(f"\ngeomean {st.geometric_mean(ratios):.3f}, worst {max(ratios):.3f}, best {min(ratios):.3f}")


def cmd_scoreboard(a):
    """One row per policy: the whole set against the base policy, with the budgets of
    docs/atlas/w2-runtime.md §5 (time <= 1.05, peak RSS <= 1.05, max/p99 pause <= 1.10)."""
    cells = load(a.dir)
    by = {}
    for (py, wl, pol), rounds in cells.items():
        if py != a.python:
            continue
        s = summarize_cell(rounds)
        if s:
            by.setdefault(wl, {})[pol] = s
    pols = sorted({p for v in by.values() for p in v if p != a.base})
    WORK = []
    print(f"policy ({a.python}) | n | geomean | best | worst | > 1.05 | GC saved, mean % of run | "
          "peak RSS × max (n > 1.05) | max pause × worst (n > 1.10) | p99 × worst (n > 1.10) | pending garbage max")
    print("---|---|---|---|---|---|---|---|---|---|---")
    for pol in pols:
        rs, saved, mem, pmax, p99, pend, work, smax, s99 = [], [], [], [], [], [], [], [], []
        for wl in sorted(by):
            b, p = by[wl].get(a.base), by[wl].get(pol)
            if not b or not p:
                continue
            r = paired_ratio(b, p)[0]
            rs.append((r, wl))
            saved.append((b["gc_s"] - p["gc_s"]) / b["wall_s"])
            if b["hwm_kb"] > 0 and p["hwm_kb"] > 0:
                mem.append((p["hwm_kb"] / b["hwm_kb"], wl))
            # pauses only where the base has collections worth comparing (>= 1 ms)
            if b["pause_max_s"] >= 1e-3:
                pmax.append(((p["pause_max_s"] or 1e-9) / b["pause_max_s"], wl))
            if b["pause_p99_s"] >= 1e-3:
                p99.append(((p["pause_p99_s"] or 1e-9) / b["pause_p99_s"], wl))
            pend.append(p["pending_garbage"])
            if b["work"] > 0:
                work.append((p["work"], b["work"], b["wall_s"], wl))
            if b["size_max"] > 1000:
                smax.append(((p["size_max"] or 1) / b["size_max"], wl))
            if b["size_p99"] > 1000:
                s99.append(((p["size_p99"] or 1) / b["size_p99"], wl))
        if not rs:
            continue
        worst = max(rs)
        m = max(mem) if mem else (float("nan"), "")
        pm = max(pmax) if pmax else (float("nan"), "")
        pp = max(p99) if p99 else (float("nan"), "")
        print(f"{pol} | {len(rs)} | {st.geometric_mean(r for r, _ in rs):.3f} | {min(rs)[0]:.3f} ({min(rs)[1].split('/')[-1]}) | "
              f"{worst[0]:.3f} ({worst[1].split('/')[-1]}) | {sum(r > 1.05 for r, _ in rs)} | {100 * st.mean(saved):.1f} | "
              f"{m[0]:.2f} ({m[1].split('/')[-1]}; {sum(x > 1.05 for x, _ in mem)}) | "
              f"{pm[0]:.2f} ({pm[1].split('/')[-1]}; {sum(x > 1.10 for x, _ in pmax)}) | "
              f"{pp[0]:.2f} ({pp[1].split('/')[-1]}; {sum(x > 1.10 for x, _ in p99)}) | {max(pend)}")
        if work:
            tw = sum(w for w, _, _, _ in work) / sum(b for _, b, _, _ in work)
            sm = max(smax) if smax else (float("nan"), "")
            sp = max(s99) if s99 else (float("nan"), "")
            WORK.append(f"{pol} | {tw:.3f} | {min(w / b for w, b, _, _ in work):.3f} | "
                        f"{max(w / b for w, b, _, _ in work):.2f} ({max(work, key=lambda t: t[0] / t[1])[3].split('/')[-1]}) | "
                        f"{sm[0]:.2f} ({sm[1].split('/')[-1]}; {sum(x > 1.10 for x, _ in smax)}) | "
                        f"{sp[0]:.2f} ({sp[1].split('/')[-1]}; {sum(x > 1.10 for x, _ in s99)})")
    print(f"\npolicy ({a.python}) | objects examined, whole set × | best workload × | worst workload × | "
          "largest collection × worst (n > 1.10) | p99 collection size × worst (n > 1.10)")
    print("---|---|---|---|---|---")
    for line in WORK:
        print(line)


def cmd_json(a):
    cells = load(a.dir)
    out = {}
    for (py, wl, pol), rounds in cells.items():
        s = summarize_cell(rounds)
        if s:
            s.pop("rounds", None)
            out.setdefault(py, {}).setdefault(wl, {})[pol] = s
    with open(a.o, "w") as f:
        json.dump(out, f, indent=1, sort_keys=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("base")
    b.add_argument("dir")
    b.add_argument("--python", default="3.15.1")
    b.add_argument("--policy", default="default")
    p = sub.add_parser("policies")
    p.add_argument("dir")
    p.add_argument("--python", default="3.15.1")
    p.add_argument("--base", default="default")
    sb = sub.add_parser("scoreboard")
    sb.add_argument("dir")
    sb.add_argument("--python", default="3.15.1")
    sb.add_argument("--base", default="default")
    j = sub.add_parser("json")
    j.add_argument("dir")
    j.add_argument("-o", required=True)
    a = ap.parse_args()
    {"base": cmd_base, "policies": cmd_policies, "json": cmd_json, "scoreboard": cmd_scoreboard}[a.cmd](a)


if __name__ == "__main__":
    main()
