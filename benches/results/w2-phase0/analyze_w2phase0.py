"""Tables for docs/atlas/w2-phase0.md from the raw results (run locally, from the repo root).

    python analyze_w2phase0.py wlcompare        workloads vs wave 1's Phase 3 (both replicates), per workload
    python analyze_w2phase0.py ftpenalty        plain 3.15.1t / plain 3.15.1: workloads and pyperformance
    python analyze_w2phase0.py gc               GC baseline per workload (gcstats.py), 3.15.1 and 3.15.1t
    python analyze_w2phase0.py jit              CPython's JIT on the workload set (jit_compare.py)
    python analyze_w2phase0.py status           target status per build (status_dump.py)
    python analyze_w2phase0.py durations        step durations (durations.tsv)

Wave 1's own tables (workloads, stats, whole, startup, conformance, junit, ppcompare,
sbcompare) come from benches/results/phase3/analyze_phase3.py run on this directory.
"""

from __future__ import annotations

import argparse
import glob
import gzip
import json
import math
import os
import statistics

HERE = os.path.dirname(os.path.abspath(__file__))
P3 = os.path.join(os.path.dirname(HERE), "phase3")
FAMS = ("data", "devtools", "web")
BUILDS = ("3.15.1", "3.15.1t")


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


def rows(d, b, tag=""):
    out = {}
    for f in FAMS:
        p = find(os.path.join(d, "workloads"), f"compare-{b}-{f}{tag}.json")
        if p:
            out.update({r["workload"]: r for r in load(p)["rows"]})
    return out


def cmd_wlcompare(a):
    new = {b: rows(HERE, b, a.tag) for b in BUILDS}
    old1 = {b: rows(P3, b) for b in BUILDS}
    old2 = {b: rows(P3, b, "-2") for b in BUILDS}
    names = sorted(set().union(*[set(x) for x in new.values()], *[set(x) for x in old1.values()]))
    print("| workload | " + " | ".join(f"{b} plain ms | {b} enabled ms | {b} ratio | {b} wave 1 (rep 1 / 2) | {b} Δ vs wave 1"
                                       for b in BUILDS) + " |")
    print("|---|" + "---:|---:|---:|---:|---:|" * len(BUILDS))
    for n in names:
        cells = []
        for b in BUILDS:
            r, o1, o2 = new[b].get(n), old1[b].get(n), old2[b].get(n)
            ow = gm([x["ratio"] for x in (o1, o2) if x]) if (o1 or o2) else None
            if r:
                flag = "" if r["same_result"] else " **DIGEST MISMATCH**"
                d = f"{r['ratio'] / ow - 1:+.1%}" if ow else "—"
                cells.append(f"{r['plain_s'] * 1e3:.1f} | {r['enabled_s'] * 1e3:.1f} | {r['ratio']:.3f}{flag} | "
                             f"{o1['ratio'] if o1 else float('nan'):.3f} / {o2['ratio'] if o2 else float('nan'):.3f} | {d}")
            else:
                cells.append("— | — | — | — | —")
        print(f"| {n} | " + " | ".join(cells) + " |")
    print()
    print("| | " + " | ".join(f"{b} wave 2 phase 0 | {b} wave 1 rep 1 | {b} wave 1 rep 2" for b in BUILDS) + " |")
    print("|---|" + "---:|---:|---:|" * len(BUILDS))
    for fam in FAMS + ("all",):
        cells = []
        for b in BUILDS:
            for src in (new[b], old1[b], old2[b]):
                rs = [r["ratio"] for k, r in src.items() if fam == "all" or k.startswith(fam + "/")]
                cells.append(f"{gm(rs):.3f} ({len(rs)})" if rs else "—")
        print(f"| geomean {fam} | " + " | ".join(cells) + " |")
    for b in BUILDS:
        if not new[b]:
            continue
        rs = new[b]
        best = min(rs.values(), key=lambda r: r["ratio"])
        worst = max(rs.values(), key=lambda r: r["ratio"])
        common = [n for n in rs if n in old1[b]]
        moved = sorted(((rs[n]["ratio"] / gm([x[n]["ratio"] for x in (old1[b], old2[b]) if n in x]), n) for n in common),
                       key=lambda t: -abs(math.log(t[0])))
        plain_drift = gm([rs[n]["plain_s"] / old1[b][n]["plain_s"] for n in common])
        print(f"\n{b}: {len(rs)} workloads; median {statistics.median(r['ratio'] for r in rs.values()):.3f}; "
              f"best {best['workload']} {best['ratio']:.3f}; worst {worst['workload']} {worst['ratio']:.3f}; "
              f"≤ 0.95: {sum(r['ratio'] <= 0.95 for r in rs.values())}; > 1.05: "
              f"{[(k, round(r['ratio'], 3)) for k, r in rs.items() if r['ratio'] > 1.05]}; digest mismatches: "
              f"{[k for k, r in rs.items() if not r['same_result']] or 'none'}")
        print(f"  plain medians vs wave 1 rep 1 (geomean of per-workload ratios): {plain_drift:.4f}; "
              f"ratio vs wave 1 (avg of 2 reps), geomean over workloads: "
              f"{gm([m for m, _ in moved]):.4f}; within 2%: {sum(abs(m - 1) <= 0.02 for m, _ in moved)}/{len(moved)}")
        print("  largest moves: " + ", ".join(f"{n} {m - 1:+.1%}" for m, n in moved[:8]))


def pp_plain(d, b, tag):
    import pyperf

    p = find(os.path.join(d, "pyperformance"), f"plain-{b}{tag}.json")
    return {x.get_name(): x.mean() for x in pyperf.BenchmarkSuite.load(p).get_benchmarks()} if p else {}


def cmd_ftpenalty(a):
    g, t = rows(HERE, "3.15.1"), rows(HERE, "3.15.1t")
    print("### Workloads: plain 3.15.1t / plain 3.15.1 (median steady-state run())\n")
    print("| workload | 3.15.1 plain ms | 3.15.1t plain ms | FT ÷ GIL | wave 1 (phase 3 rep 1) |")
    print("|---|---:|---:|---:|---:|")
    o1g, o1t = rows(P3, "3.15.1"), rows(P3, "3.15.1t")
    pen = {}
    for n in sorted(set(g) & set(t)):
        pen[n] = t[n]["plain_s"] / g[n]["plain_s"]
        ow = o1t[n]["plain_s"] / o1g[n]["plain_s"] if n in o1g and n in o1t else None
        mark = "**" if pen[n] > 1.3 else ""
        print(f"| {n} | {g[n]['plain_s'] * 1e3:.1f} | {t[n]['plain_s'] * 1e3:.1f} | {mark}{pen[n]:.3f}{mark} | "
              f"{f'{ow:.3f}' if ow else '—'} |")
    if pen:
        for fam in FAMS:
            print(f"\ngeomean {fam}: {gm([v for k, v in pen.items() if k.startswith(fam + '/')]):.3f}", end="")
        print(f"\ngeomean all ({len(pen)}): {gm(list(pen.values())):.3f}; median {statistics.median(pen.values()):.3f}")
        print("outliers > 1.3: " + ", ".join(f"{k} {v:.2f}" for k, v in sorted(pen.items(), key=lambda kv: -kv[1]) if v > 1.3))
        print("below 1.0: " + ", ".join(f"{k} {v:.2f}" for k, v in sorted(pen.items(), key=lambda kv: kv[1]) if v < 1.0))
    print("\n### pyperformance: plain 3.15.1t / plain 3.15.1 (mean per benchmark)\n")
    for tag in ("", "-B"):
        pg, pt = pp_plain(HERE, "3.15.1", tag), pp_plain(HERE, "3.15.1t", tag)
        if not pg or not pt:
            continue
        both = sorted(set(pg) & set(pt))
        r = {n: pt[n] / pg[n] for n in both}
        print(f"replicate {tag or 'A'}: {len(both)} benchmarks; geomean {gm(list(r.values())):.3f}; median "
              f"{statistics.median(r.values()):.3f}; > 1.3: {sum(v > 1.3 for v in r.values())}; < 1.0: "
              f"{sum(v < 1.0 for v in r.values())}")
    pa = {tag: (pp_plain(HERE, "3.15.1", tag), pp_plain(HERE, "3.15.1t", tag)) for tag in ("", "-B")}
    names = sorted(set().union(*[set(x[0]) & set(x[1]) for x in pa.values()]))
    avg = {}
    for n in names:
        vals = [x[1][n] / x[0][n] for x in pa.values() if n in x[0] and n in x[1]]
        avg[n] = gm(vals)
    if avg:
        print(f"\nreplicates averaged: {len(avg)} benchmarks, geomean {gm(list(avg.values())):.3f}")
        print("\n| benchmark | 3.15.1 plain | 3.15.1t plain | FT ÷ GIL (A) | FT ÷ GIL (B) | averaged |")
        print("|---|---:|---:|---:|---:|---:|")
        for n in sorted(avg, key=lambda n: -avg[n]):
            g_, t_ = pa[""][0].get(n), pa[""][1].get(n)
            ra = f"{t_ / g_:.3f}" if g_ and t_ else "—"
            gb, tb = pa["-B"][0].get(n), pa["-B"][1].get(n)
            rb = f"{tb / gb:.3f}" if gb and tb else "—"
            mark = "**" if avg[n] > 1.3 else ""
            print(f"| {n} | {fmt_t(g_) if g_ else '—'} | {fmt_t(t_) if t_ else '—'} | {ra} | {rb} | {mark}{avg[n]:.3f}{mark} |")
        print("\noutliers > 1.3: " + ", ".join(f"{n} {v:.2f}" for n, v in sorted(avg.items(), key=lambda kv: -kv[1]) if v > 1.3))


def fmt_t(s):
    for unit, f in (("s", 1), ("ms", 1e3), ("µs", 1e6), ("ns", 1e9)):
        if s * f >= 1:
            return f"{s * f:.3g} {unit}"
    return f"{s * 1e9:.3g} ns"


def gc_load(b):
    p = find(os.path.join(HERE, "gc"), f"gc-{b}.jsonl")
    if not p:
        return {}
    op = gzip.open if p.endswith(".gz") else open
    with op(p, "rt") as f:
        return {r["workload"]: r for r in map(json.loads, filter(str.strip, f))}


def cmd_gc(a):
    data = {b: gc_load(b) for b in BUILDS}
    for b in BUILDS:
        if data[b]:
            r0 = next(iter(data[b].values()))
            print(f"{b}: gc enabled {r0['gc_enabled']}, threshold {r0['threshold']}, extra callback keys "
                  f"{sorted({k for r in data[b].values() for k in r['callback_info_extra_keys']})}")
    print("\n| workload | " + " | ".join(f"{b} collections (per gen) | {b} GC ms (share of run) | {b} collected | {b} peak RSS MiB"
                                       for b in BUILDS) + " |")
    print("|---|" + "---:|---:|---:|---:|" * len(BUILDS))
    names = sorted(set().union(*[set(d) for d in data.values()]))
    for n in names:
        cells = []
        for b in BUILDS:
            r = data[b].get(n)
            if not r:
                cells.append("— | — | — | —")
                continue
            gens = "/".join(f"{g}:{d['collections']}" for g, d in sorted(r["per_generation"].items()))
            cells.append(f"{r['collections']} ({gens or '—'}) | {r['gc_s'] * 1e3:.1f} ({(r['gc_share'] or 0) * 100:.1f}%) | "
                         f"{r['collected']:,} | {r['maxrss_kib'] / 1024:.0f}")
        print(f"| {n} | " + " | ".join(cells) + " |")
    for b in BUILDS:
        d = data[b]
        if not d:
            continue
        shares = {n: r["gc_share"] or 0 for n, r in d.items()}
        tot_gc = sum(r["gc_s"] for r in d.values())
        tot_run = sum(r["run_s"] for r in d.values())
        print(f"\n{b}: {len(d)} workloads; GC time {tot_gc * 1e3:.0f} ms of {tot_run * 1e3:.0f} ms run time "
              f"({tot_gc / tot_run:.1%}); median share {statistics.median(shares.values()):.1%}; "
              f"top: " + ", ".join(f"{n} {v:.1%}" for n, v in sorted(shares.items(), key=lambda kv: -kv[1])[:6]))
        unc = {n: r["uncollectable"] for n, r in d.items() if r["uncollectable"]}
        print(f"  uncollectable: {unc or 'none'}; callback vs get_stats() collections agree: "
              f"{all(sum(x.get('collections', 0) for x in r['get_stats_delta']) == r['collections'] for r in d.values())}")


def cmd_jit(a):
    rs = {}
    for f in FAMS:
        p = find(os.path.join(HERE, "jit"), f"jit-{a.build}-{f}.json")
        if p:
            rs.update({r["workload"]: r for r in load(p)["rows"]})
    print("| workload | no JIT ms | PYTHON_JIT=1 ms | ratio | digest |")
    print("|---|---:|---:|---:|---|")
    for n in sorted(rs):
        r = rs[n]
        print(f"| {n} | {r['nojit_s'] * 1e3:.1f} | {r['jit_s'] * 1e3:.1f} | {r['ratio']:.3f} | "
              f"{'same' if r['same_result'] else '**MISMATCH**'} |")
    for fam in FAMS:
        v = [r["ratio"] for k, r in rs.items() if k.startswith(fam + "/")]
        print(f"\ngeomean {fam} ({len(v)}): {gm(v):.3f}", end="")
    if rs:
        print(f"\ngeomean all ({len(rs)}): {gm([r['ratio'] for r in rs.values()]):.3f}; median "
              f"{statistics.median(r['ratio'] for r in rs.values()):.3f}; best "
              f"{min(rs.values(), key=lambda r: r['ratio'])['workload']} {min(r['ratio'] for r in rs.values()):.3f}; "
              f"worst {max(rs.values(), key=lambda r: r['ratio'])['workload']} {max(r['ratio'] for r in rs.values()):.3f}; "
              f"> 1.05: {sum(r['ratio'] > 1.05 for r in rs.values())}; < 0.95: {sum(r['ratio'] < 0.95 for r in rs.values())}; "
              f"JIT state as requested in every child: {all(r['jit_enabled'] == {'nojit': [False], 'jit': [True]} for r in rs.values())}")


def cmd_status(a):
    print("| build | targets | installed | refused (category: targets) | pending | python |")
    print("|---|---:|---:|---|---:|---|")
    for p in sorted(glob.glob(os.path.join(HERE, "status", "status-*.json")),
                    key=lambda p: [int(x) if x.isdigit() else x for x in os.path.basename(p)[7:-5].replace("t", ".t").split(".")]):
        s = load(p)["summary"]
        b = os.path.basename(p)[7:-5]
        ref: dict[str, list] = {}
        for r in s["refused"]:
            ref.setdefault(f"{r['refusal']} — {r['reason']}", []).append(r["name"])
        rtxt = "; ".join(f"{k}: {', '.join(v)}" for k, v in ref.items()) or "0"
        print(f"| {b} | {s['targets']} | {s['counts'].get('installed', 0)} | {len(s['refused'])}: {rtxt} | "
              f"{len(s['pending'])}{(': ' + ', '.join(s['pending'])) if s['pending'] else ''} | {s['python'].split()[0]} |")


def cmd_durations(a):
    p = os.path.join(HERE, "durations.tsv")
    rows_ = [ln.rstrip("\n").split("\t") for ln in open(p) if ln.strip()]
    print("| step | start | end | s | rc |")
    print("|---|---|---|---:|---:|")
    for name, s, e, sec, rc in rows_:
        print(f"| {name} | {s[11:19]} | {e[11:19]} | {sec} | {rc} |")
    print(f"\nfirst start {min(r[1] for r in rows_)}, last end {max(r[2] for r in rows_)}")


def main():
    global HERE, P3
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--res", default=HERE, help="this run's results directory (default: this script's)")
    ap.add_argument("--p3", default=P3, help="wave 1's Phase 3 results (default: ../phase3)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("wlcompare"); s.add_argument("--tag", default="")
    sub.add_parser("ftpenalty")
    sub.add_parser("gc")
    s = sub.add_parser("jit"); s.add_argument("--build", default="3.15.1")
    sub.add_parser("status")
    sub.add_parser("durations")
    a = ap.parse_args()
    HERE, P3 = os.path.abspath(a.res), os.path.abspath(a.p3)
    globals()["cmd_" + a.cmd](a)


if __name__ == "__main__":
    main()
