"""Tables for docs/atlas/phase3.md from the raw Phase 3 results.

    python analyze_phase3.py workloads compare-3.15.1.json compare-3.15.1t.json
    python analyze_phase3.py stats workloads/stats-3.15.1 [--top 5]
    python analyze_phase3.py whole whole/whole-3.15.1.json [whole/whole-3.15.1t.json ...]
    python analyze_phase3.py startup startup/cells-3.15.1-bare.json ...
    python analyze_phase3.py conformance conformance/all-groups-3.15.1.json ...
    python analyze_phase3.py junit tests/pytest-3.15.1.xml ...
    python analyze_phase3.py sbcompare --repo CLONE --old P0FILES... --new P3FILES...
        scoreboard (--impls enabled) Phase 3 vs Phase 0, cell by cell, split into kernel-path
        cells (Phase 0's definition), wave-1 serial cells (json.dumps, copy.deepcopy) and the
        rest; run where benches/scoreboard.py and rustique are importable.
"""

from __future__ import annotations

import argparse
import glob
import json
import math
import os
import re
import statistics
import xml.etree.ElementTree as ET


def gm(xs):
    xs = [x for x in xs if x and x > 0]
    return math.exp(sum(map(math.log, xs)) / len(xs)) if xs else float("nan")


def build_of(path: str) -> str:
    m = re.search(r"(3\.\d+\.\d+t?)", os.path.basename(path))
    return m.group(1) if m else path


def load_json(path: str):
    if path.endswith(".gz"):
        import gzip

        with gzip.open(path, "rt") as f:
            return json.load(f)
    with open(path) as f:
        return json.load(f)


def cmd_workloads(a):
    data: dict = {}
    for p in a.files:  # one file per build and family (compare-<build>-<family>.json): merge per build
        data.setdefault(build_of(p), {}).update({r["workload"]: r for r in load_json(p)["rows"]})
    builds = list(data)
    names = sorted(set().union(*[set(d) for d in data.values()]))
    head = "| workload | " + " | ".join(f"{b} plain ms | {b} enabled ms | {b} ratio" for b in builds) + " |"
    print(head)
    print("|---|" + "---:|---:|---:|" * len(builds))
    for n in names:
        cells = []
        for b in builds:
            r = data[b].get(n)
            if r:
                flag = "" if r["same_result"] else " **DIGEST MISMATCH**"
                cells.append(f"{r['plain_s'] * 1e3:.1f} | {r['enabled_s'] * 1e3:.1f} | {r['ratio']:.3f}{flag}")
            else:
                cells.append("— | — | —")
        print(f"| {n} | " + " | ".join(cells) + " |")
    print()
    print("| | " + " | ".join(builds) + " |")
    print("|---|" + "---:|" * len(builds))
    for fam in ("data", "devtools", "web"):
        vals = []
        for b in builds:
            rs = [r["ratio"] for k, r in data[b].items() if k.startswith(fam + "/")]
            vals.append(f"{gm(rs):.3f} ({len(rs)})")
        print(f"| geomean {fam} | " + " | ".join(vals) + " |")
    print("| **geomean all** | " + " | ".join(f"**{gm([r['ratio'] for r in data[b].values()]):.3f}** ({len(data[b])})" for b in builds) + " |")
    print("| median | " + " | ".join(f"{statistics.median([r['ratio'] for r in data[b].values()]):.3f}" for b in builds) + " |")
    for label, fn in (("best", min), ("worst", max)):
        cells = []
        for b in builds:
            r = fn(data[b].values(), key=lambda r: r["ratio"])
            cells.append(f"{r['workload']} {r['ratio']:.3f}")
        print(f"| {label} | " + " | ".join(cells) + " |")
    for b in builds:
        bad = [k for k, r in data[b].items() if not r["same_result"]]
        print(f"\n{b}: {len(data[b])} workloads, digest mismatches: {bad or 'none'}; "
              f"≤ 0.95: {sum(r['ratio'] <= 0.95 for r in data[b].values())}, "
              f"> 1.05: {[(k, round(r['ratio'], 3)) for k, r in data[b].items() if r['ratio'] > 1.05]}")


def cmd_stats(a):
    for d in a.dirs:
        print(f"\n### {d}\n")
        print("| workload | targets hit (hits, fallbacks by reason) | installed / pending / refused |")
        print("|---|---|---|")
        for f in sorted(glob.glob(os.path.join(d, "*.json"))):
            w = os.path.basename(f)[:-5].replace("__", "/")
            recs = [json.loads(ln) for ln in open(f) if ln.strip()]
            if not recs:
                print(f"| {w} | no record | |")
                continue
            rec = recs[-1]
            st = rec.get("stats") or {}
            states = {}
            for t in rec["status"]["targets"]:
                states[t["state"]] = states.get(t["state"], 0) + 1
            rows = []
            for name, c in st.items():
                fb = {k: v for k, v in (c.get("fallback") or {}).items() if v}
                if c.get("hits") or fb:
                    rows.append((c.get("hits", 0), sum(fb.values()), name, fb))
            rows.sort(key=lambda r: (-r[0], -r[1]))
            txt = "; ".join(
                f"{name} {h:,}" + (f" (fb {', '.join(f'{k} {v:,}' for k, v in fb.items())})" if fb else "")
                for h, _, name, fb in rows[: a.top])
            more = f"; +{len(rows) - a.top} more" if len(rows) > a.top else ""
            print(f"| {w} | {txt or 'no target called'}{more} | {states.get('installed', 0)} / "
                  f"{states.get('pending-import', 0)} / {states.get('refused', 0)} |")


def cmd_whole(a):
    import pyperf

    print("| build | workload | plain s (mean ± std) | RUSTIQUE=1 s | ratio | values |")
    print("|---|---|---:|---:|---:|---:|")
    for p in a.files:
        b = build_of(p)
        suite = pyperf.BenchmarkSuite.load(p)
        by = {x.get_name(): x for x in suite.get_benchmarks()}
        ratios = []
        for w in sorted({n.rsplit("/", 1)[0] for n in by}):
            pl, en = by.get(f"{w}/plain"), by.get(f"{w}/enabled")
            if not (pl and en):
                continue
            r = en.mean() / pl.mean()
            ratios.append(r)
            print(f"| {b} | {w} | {pl.mean():.3f} ± {pl.stdev():.3f} | {en.mean():.3f} ± {en.stdev():.3f} | "
                  f"{r:.3f} | {len(pl.get_values())}/{len(en.get_values())} |")
        print(f"| {b} | **geomean** | | | **{gm(ratios):.3f}** | |")
        dd = os.path.join(os.path.dirname(p), f"digests-{b}")
        if os.path.isdir(dd):
            for w in sorted({os.path.basename(f).rsplit("-", 1)[0] for f in glob.glob(dd + "/*.txt")}):
                ds = {}
                for v in ("plain", "enabled"):
                    fp = os.path.join(dd, f"{w}-{v}.txt")
                    ds[v] = sorted(set(open(fp).read().split("\n")) - {""}) if os.path.exists(fp) else []
                dig = {v: {x.split(" rustique=")[0] for x in ds[v]} for v in ds}
                loaded = {v: {x.split(" rustique=")[1] for x in ds[v] if " rustique=" in x} for v in ds}
                ok = dig["plain"] == dig["enabled"] and len(dig["plain"]) == 1
                print(f"<!-- {b} {w}: digests {'same' if ok else 'DIFFER ' + str(dig)}; rustique loaded "
                      f"plain={sorted(loaded['plain'])} enabled={sorted(loaded['enabled'])} -->")


def cmd_startup(a):
    print("| build | venv | cell | Ir | Ir ratio | wall ms median (p10–p90) | wall ratio |")
    print("|---|---|---|---:|---:|---:|---:|")
    for p in a.files:
        d = load_json(p)
        m = re.search(r"cells-(\S+?)-(bare|pth)\.json", os.path.basename(p))
        b, kind = (m.group(1), m.group(2)) if m else (p, "")
        for r in d["cells"]:
            print(f"| {b} | {kind} | {r['variant']} | {r['ir']:,} | {r['ir_ratio']:.4f} | "
                  f"{r['ms_median']:.2f} ({r['ms_p10']:.2f}–{r['ms_p90']:.2f}) | {r['ms_ratio']:.4f} |")
        mg = d["marginal"]
        print(f"| {b} | {kind} | marginal RUSTIQUE=1 vs unset | | {mg['ir']:.4f} | | {mg['ms']:.4f} |")
        cells = {r["variant"]: r for r in d["cells"]}
        on, en = cells.get("RUSTIQUE=1: pass"), cells.get("import + enable()")
        if on and en:
            print(f"<!-- {b} {kind}: import+enable() - RUSTIQUE=1 = {en['ir'] - on['ir']:+,} Ir -->")


def cmd_conformance(a):
    reps = {build_of(p): load_json(p) for p in a.files}
    builds = list(reps)
    mods = sorted(set().union(*[set(r["modules"]) for r in reps.values()]))
    print("| module | " + " | ".join(builds) + " |")
    print("|---|" + "---|" * len(builds))
    for m in mods:
        cells = []
        for b in builds:
            r = reps[b]["modules"].get(m)
            if not r:
                cells.append("—")
                continue
            c = r["counts"]
            txt = ", ".join(f"{v} {k}" for k, v in sorted(c.items(), key=lambda kv: -kv[1]))
            nd = len([d for d in r["differences"] if not d["allowed"]])
            st = "" if r["plain"] == r["enabled"] == "ran" else f" plain={r['plain']} enabled={r['enabled']}"
            cells.append(f"{txt}; **{nd} diff**{st}")
        print(f"| {m.replace('test.', '')} | " + " | ".join(cells) + " |")
    for b, r in reps.items():
        nd = sum(len(x["differences"]) for x in r["modules"].values())
        tot = sum(sum(x["counts"].values()) for x in r["modules"].values())
        print(f"\n{b}: ok={r['ok']}, {tot} test outcomes, {nd} differences, groups {','.join(r['targets'])}; "
              f"not installed: {r['not_installed'] or 'none'}")
        for m, x in r["modules"].items():
            for d in x["differences"]:
                print(f"  DIFF {m} {d['id']}: {d['plain']} -> {d['enabled']}")


def cmd_junit(a):
    print("| run | tests | passed | failed | errors | skipped | time s |")
    print("|---|---:|---:|---:|---:|---:|---:|")
    for p in a.files:
        r = ET.parse(p).getroot()
        ts = r if r.tag == "testsuite" else r.find("testsuite")
        t, f, e, s = (int(ts.get(k, 0)) for k in ("tests", "failures", "errors", "skipped"))
        print(f"| {os.path.basename(p)} | {t} | {t - f - e - s} | {f} | {e} | {s} | {float(ts.get('time', 0)):.1f} |")
        for tc in ts.iter("testcase"):
            for kind in ("failure", "error"):
                el = tc.find(kind)
                if el is not None:
                    print(f"  {kind.upper()} {tc.get('classname')}::{tc.get('name')}: {(el.get('message') or '')[:300]}")


KERNEL_BUILTINS = {"sorted(x)", "sum(x)", "min(x)", "max(x)", "any(x)", "all(x)"}
KERNEL_SEQ = {"hit in x", "miss in x", "x.index(hit)", "x.count(hit)"}
WAVE1_OPS = {"json.dumps", "copy.deepcopy"}


def category(typ, op):
    if op in KERNEL_BUILTINS or (typ in ("list", "tuple") and op in KERNEL_SEQ) or (typ == "list" and op == "x.sort()"):
        return "kernel path"
    if op in WAVE1_OPS:
        return "wave-1 serial (json.dumps, copy.deepcopy)"
    return "other ops"


def cmd_sbcompare(a):
    import importlib.util
    import sys

    spec = importlib.util.spec_from_file_location("scoreboard", os.path.join(a.repo, "benches", "scoreboard.py"))
    sb = importlib.util.module_from_spec(spec)
    sys.modules["scoreboard"] = sb
    spec.loader.exec_module(sb)
    new, _ = sb.load_results(a.new)
    old, _ = sb.load_results(a.old)
    by_key = {c.key: c for c in sb.all_cells()}
    rows = []
    for (build, key), e in new.items():
        cell = by_key.get(key)
        if cell is None:
            continue
        r, st, _m = sb.cell_ratio(e, "enabled", cell.n)
        oe = old.get((build, key))
        ro = sb.cell_ratio(oe, "enabled", cell.n)[0] if oe else None
        nz = (e.get("noise") or {})
        rows.append(dict(build=build, key=key, typ=cell.type, op=cell.op.name, n=cell.n, small=cell.n in sb.SMALL_SIZES,
                         r=r, st=st, ro=ro, cat=category(cell.type, cell.op.name),
                         nb=nz.get("builtin"), ne=nz.get("enabled")))
    for build in sorted({x["build"] for x in rows}):
        for small, label in ((True, "0-100 elements, callgrind Ir"), (False, "10k/1M elements, wall time")):
            sel = [x for x in rows if x["build"] == build and x["small"] == small and x["r"] and x["r"] > 0
                   and x["st"] not in ("error", "mismatch")]
            if not sel:
                continue
            bad = [x for x in rows if x["build"] == build and x["small"] == small and x["st"] in ("error", "mismatch")]
            print(f"\n### {build} — {label}\n")
            print(f"cells {len(sel)}; mismatches/errors: {len(bad)} {[(x['key'], x['st']) for x in bad][:10]}\n")
            print("| category | cells | ≤ 1.05 phase 3 | ≤ 1.05 phase 0 | geomean phase 3 | geomean phase 0 | median phase 3 | geomean phase3/phase0 per cell |")
            print("|---|---:|---:|---:|---:|---:|---:|---:|")
            cats = ["kernel path", "wave-1 serial (json.dumps, copy.deepcopy)", "other ops"]
            for cat in cats + ["all"]:
                g = sel if cat == "all" else [x for x in sel if x["cat"] == cat]
                if not g:
                    continue
                both = [x for x in g if x["ro"]]
                print(f"| {cat} | {len(g)} | {sum(x['r'] <= 1.05 for x in g)} | {sum(x['ro'] <= 1.05 for x in both)} | "
                      f"{gm([x['r'] for x in g]):.4f} | {gm([x['ro'] for x in both]):.4f} | "
                      f"{statistics.median([x['r'] for x in g]):.4f} | {gm([x['r'] / x['ro'] for x in both]):.4f} |")
            above = sorted((x for x in sel if x["r"] > 1.05), key=lambda x: -x["r"])
            print(f"\ncells > 1.05: {len(above)}" + (" (top 80)" if len(above) > 80 else ""))
            for x in above[:80]:
                nz = "" if small else f"  rel.std builtin {x['nb'] or 0:.3f} enabled {x['ne'] or 0:.3f}"
                ro = f"{x['ro']:.3f}" if x["ro"] else "—"
                print(f"  {x['r']:.3f} (phase 0 {ro})  [{x['cat'][:6]}] {x['key']}{nz}")
            ops = {}
            for x in sel:
                if x["ro"]:
                    ops.setdefault((x["typ"], x["op"]), []).append(x)
            moved = []
            for (t, op), g in ops.items():
                ch = gm([x["r"] / x["ro"] for x in g])
                if ch < 0.97 or ch > 1.03:
                    moved.append((ch, t, op, gm([x["r"] for x in g]), gm([x["ro"] for x in g]), len(g)))
            print(f"\nops whose geomean moved > 3% vs phase 0: {len(moved)}")
            for ch, t, op, gn, go, k in sorted(moved):
                print(f"  {t:9} {op:28} cells {k:3}  phase 3 {gn:.3f}  phase 0 {go:.3f}  change {ch:.3f}")


def _pp_load(d, name):
    import pyperf

    for ext in (".json", ".json.gz"):
        f = os.path.join(d, name + ext)
        if os.path.exists(f):
            return {b.get_name(): b for b in pyperf.BenchmarkSuite.load(f).get_benchmarks()}
    return {}


def _pp_ratios(d, b):
    """{replicate: {bench: ratio}} for A ('' tag), B ('-B') and the port re-run ('-ports')."""
    out = {}
    for tag in ("", "-B", "-ports"):
        p, e = _pp_load(d, f"plain-{b}{tag}"), _pp_load(d, f"enabled-{b}{tag}")
        if p and e:
            out[tag or "A"] = {n: e[n].mean() / p[n].mean() for n in set(p) & set(e)}
            out[(tag or "A") + ":plain"] = {n: p[n].mean() for n in p}
            out[(tag or "A") + ":only"] = sorted(set(p) ^ set(e))
    return out


def cmd_ppcompare(a):
    """pyperformance: per build, both replicates (+ the port re-run), Phase 3 vs Phase 0."""
    for b in a.builds:
        new, old = _pp_ratios(a.new, b), _pp_ratios(a.old, b)
        reps = [r for r in ("A", "-B") if r in new]
        names = sorted(set().union(*[set(new[r]) for r in reps]))
        ports = new.get("-ports", {})

        def best(n, rep):
            # a port benchmark that failed in a replicate (collision) is taken from the re-run
            return new[rep].get(n, ports.get(n))

        avg = {}
        for n in names + sorted(set(ports) - set(names)):
            vals = [best(n, r) for r in reps if best(n, r)]
            if vals:
                avg[n] = gm(vals)
        o_avg = {}
        for n in set().union(*[set(old.get(r, {})) for r in ("A", "-B")]) | set(old.get("-ports", {})):
            vals = [old[r].get(n, old.get("-ports", {}).get(n)) for r in ("A", "-B") if r in old]
            vals = [v for v in vals if v]
            if vals:
                o_avg[n] = gm(vals)
        print(f"\n### {b}\n")
        for r in reps:
            print(f"replicate {r}: {len(new[r])} results, geomean {gm(list(new[r].values())):.4f}; "
                  f"only one side: {new[r + ':only']}")
        if ports:
            print(f"port re-run: " + ", ".join(f"{n} {v:.3f}" for n, v in sorted(ports.items())))
        print(f"**geomean, replicates averaged per benchmark ({len(avg)}): {gm(list(avg.values())):.4f}**; "
              f"phase 0 same way: {gm(list(o_avg.values())):.4f} ({len(o_avg)})")
        both = lambda n, f: all(f(best(n, r)) for r in reps if best(n, r)) and len([r for r in reps if best(n, r)]) == len(reps)
        slow = sorted((n for n in avg if both(n, lambda x: x > 1.02)), key=lambda n: -avg[n])
        fast = sorted((n for n in avg if both(n, lambda x: x < 0.98)), key=lambda n: avg[n])
        print("slower by > 2% in every replicate: " + ", ".join(f"{n} {'/'.join(f'{best(n, r):.3f}' for r in reps)}" for n in slow))
        print("faster by > 2% in every replicate: " + ", ".join(f"{n} {'/'.join(f'{best(n, r):.3f}' for r in reps)}" for n in fast))
        plain = new.get("A:plain", {})
        print("\n| benchmark | plain (A) | " + " | ".join(f"ratio {r.strip('-')}" for r in reps) + " | phase 0 (A/B avg) |")
        print("|---|---:|" + "---:|" * len(reps) + "---:|")
        for n in sorted(avg):
            pl = plain.get(n) or new.get("-ports:plain", {}).get(n)
            cells = [f"{best(n, r):.3f}" if best(n, r) else "—" for r in reps]
            o = f"{o_avg[n]:.3f}" if n in o_avg else "—"
            print(f"| {n} | {fmt_t(pl) if pl else '—'} | " + " | ".join(cells) + f" | {o} |")
        missing = sorted(set(o_avg) - set(avg))
        if missing:
            print(f"\nin phase 0 but not here: {missing}")


def fmt_t(s):
    for unit, f in (("s", 1), ("ms", 1e3), ("µs", 1e6), ("ns", 1e9)):
        if s * f >= 1:
            return f"{s * f:.3g} {unit}"
    return f"{s * 1e9:.3g} ns"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("workloads"); s.add_argument("files", nargs="+")
    s = sub.add_parser("stats"); s.add_argument("dirs", nargs="+"); s.add_argument("--top", type=int, default=5)
    s = sub.add_parser("whole"); s.add_argument("files", nargs="+")
    s = sub.add_parser("startup"); s.add_argument("files", nargs="+")
    s = sub.add_parser("conformance"); s.add_argument("files", nargs="+")
    s = sub.add_parser("junit"); s.add_argument("files", nargs="+")
    s = sub.add_parser("ppcompare"); s.add_argument("--new", required=True); s.add_argument("--old", required=True)
    s.add_argument("builds", nargs="+")
    s = sub.add_parser("sbcompare"); s.add_argument("--repo", default=".")
    s.add_argument("--old", nargs="+", required=True); s.add_argument("--new", nargs="+", required=True)
    a = ap.parse_args()
    globals()["cmd_" + a.cmd](a)


if __name__ == "__main__":
    main()
