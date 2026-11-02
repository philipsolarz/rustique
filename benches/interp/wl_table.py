"""Markdown tables from wl_cg.py JSON outputs (exact Ir of one steady-state run).

    python benches/interp/wl_table.py fc.json [rest.json ...]
"""
import json
import statistics
import sys

rows = {}
for p in sys.argv[1:]:
    rows.update(json.load(open(p)))
variants = ["tier", "default", "default+tier"]
fams = {}
for w, r in sorted(rows.items()):
    if not r.get("plain") or not r.get("tier"):
        continue
    fams.setdefault(w.split("/")[0], []).append((w, r))
print("| workload | plain (G Ir) | tier | default | default + tier | tier vs default |")
print("|---|---:|---:|---:|---:|---:|")
summ = {}
for fam, ws in fams.items():
    for w, r in ws:
        p = r["plain"]
        cells = [f"{r[v] / p:.4f}" if r.get(v) else "–" for v in variants]
        td = f"{r['default+tier'] / r['default']:.4f}" if r.get("default") and r.get("default+tier") else "–"
        print(f"| {w} | {p / 1e9:.2f} | " + " | ".join(cells) + f" | {td} |")
    g = {}
    for v in variants:
        vals = [r[v] / r["plain"] for _, r in ws if r.get(v)]
        if vals:
            g[v] = statistics.geometric_mean(vals)
    summ[fam] = (len(ws), g)
print()
print("| family | n | tier | default | default + tier |")
print("|---|---:|---:|---:|---:|")
allv = {v: [] for v in variants}
for fam, (n, g) in summ.items():
    print(f"| {fam} | {n} | " + " | ".join(f"{g[v]:.4f}" if v in g else "–" for v in variants) + " |")
for w, r in rows.items():
    for v in variants:
        if r.get(v) and r.get("plain"):
            allv[v].append(r[v] / r["plain"])
print(f"| all | {sum(n for n, _ in summ.values())} | " + " | ".join(
    f"{statistics.geometric_mean(allv[v]):.4f}" if allv[v] else "–" for v in variants) + " |")
worst = max(((r["tier"] / r["plain"], w) for w, r in rows.items() if r.get("tier") and r.get("plain")), default=None)
print(f"\nworst tier/plain: {worst}")
