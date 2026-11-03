"""Sort cells of the scoreboard at 10k/1M (z07 runs): enabled ÷ builtin per cell and run, with Phase 3
(7fec7e4) and Phase 0 (wave-1 code) beside them.   python sortcells.py <build>"""
import glob, gzip, json, math, os, sys
HERE = os.path.dirname(os.path.abspath(__file__))
b = sys.argv[1] if len(sys.argv) > 1 else "3.15.1"

def load(pattern):
    out = {}
    for p in glob.glob(pattern):
        d = json.load(gzip.open(p, "rt")) if p.endswith(".gz") else json.load(open(p))
        for bench in d["benchmarks"]:
            name = bench["metadata"]["name"]
            v = [x for r in bench["runs"] for x in r.get("values", [])]
            if v:
                out[name] = sum(v) / len(v)
    return {k[: -len("/enabled")]: out[k] / out[k[: -len("/enabled")] + "/builtin"]
            for k in out if k.endswith("/enabled") and k[: -len("/enabled")] + "/builtin" in out}

runs = [load(os.path.join(HERE, f"z07/sort/sort-time-{b}-run{r}.json.shard*.json*")) for r in (1, 2)]
p3 = load(os.path.join(HERE, f"final/scoreboard/time-enabled-{b}.json.shard*.json*"))
p0 = load(os.path.join(HERE, f"../w2-phase0/scoreboard/time-enabled-{b}.json.shard*.json*"))
cells = sorted(set(runs[0]) | set(runs[1]))
gm = lambda xs: math.exp(sum(map(math.log, xs)) / len(xs)) if xs else float("nan")
print(f"| cell | run 1 | run 2 | mean | Phase 3 (7fec7e4, as run) | Phase 0 |")
print("|---|---:|---:|---:|---:|---:|")
over = []
for c in cells:
    r1, r2 = runs[0].get(c), runs[1].get(c)
    m = gm([x for x in (r1, r2) if x])
    if m > 1.05:
        over.append((c, m))
    f = lambda x: f"{x:.3f}" if x else "—"
    bold = "**" if m > 1.05 else ""
    print(f"| {c} | {f(r1)} | {f(r2)} | {bold}{m:.3f}{bold} | {f(p3.get(c))} | {f(p0.get(c))} |")
ms = [gm([x for x in (runs[0].get(c), runs[1].get(c)) if x]) for c in cells]
print(f"\n{len(cells)} cells; geomean {gm(ms):.3f}; max {max(ms):.3f}; > 1.05 (mean of 2 runs): {over}")
