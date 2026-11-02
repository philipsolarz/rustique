"""Recompute the estimator's predictions from saved validation runs (estimate_workloads.py -o) in
the three time-source modes, and print the doc's table (docs/atlas/w2-dx.md §5.2).

    python benches/dx/estimate_repredict.py BUILD 'DIR/val-BUILD-*.json' [table]
"""
import glob, json, os, statistics, sys
import importlib.util
HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("est", os.path.join(HERE, "..", "..", "python", "rustique", "_estimate.py"))
E = importlib.util.module_from_spec(spec); spec.loader.exec_module(E)
build = sys.argv[1]
rows = []
for f in sorted(glob.glob(sys.argv[2])):
    rows += json.load(open(f))
res = {}
for mode in ("profile", "samples", "hybrid"):
    errs = []
    for r in rows:
        pl, co = r["raw"]["plain"], r["raw"]["counters"]
        pred = E.predict(pl, pl["replay"], co["stats"], pl["plain_ns"], None if mode == "profile" else pl.get("sampled"), mode=mode)
        r.setdefault("pred", {})[mode] = (pred["ratio"], pred["under_targets"])
        errs.append(pred["ratio"] - r["measured"])
    res[mode] = errs
    print(f"{build} {mode:8}: n={len(errs)} MAE {statistics.mean(abs(e) for e in errs):.3f}  median|e| {statistics.median(abs(e) for e in errs):.3f}  max {max(errs, key=abs):+.3f}  within .02 {sum(abs(e)<=.02 for e in errs)}  within .05 {sum(abs(e)<=.05 for e in errs)}  bias {statistics.mean(errs):+.3f}")
cover = sum(min(r["pred"]["samples"][0], r["pred"]["hybrid"][0]) - 0.02 <= r["measured"] <= max(r["pred"]["samples"][0], r["pred"]["hybrid"][0]) + 0.02 for r in rows)
print(f"{build}: measured inside [min, max] of samples/hybrid (+-0.02): {cover}/{len(rows)}")
if len(sys.argv) > 3:
    print("| workload | measured | predicted | error | range (samples, hybrid) | under targets | top saver |")
    print("|---|---:|---:|---:|---|---:|---|")
    for r in rows:
        pl, co = r["raw"]["plain"], r["raw"]["counters"]
        pred = E.predict(pl, pl["replay"], co["stats"], pl["plain_ns"], pl.get("sampled"), mode="samples")
        top = pred["rows"][0] if pred["rows"] and pred["rows"][0]["saved_ns"] > 0.005 * pred["total_ns"] else None
        topt = f"{top['target']} ({top['saved_ns'] / pred['total_ns'] * 100:.0f}%)" if top else "-"
        h, s_ = r["pred"]["hybrid"][0], r["pred"]["samples"][0]
        print(f"| {r['workload']} | {r['measured']:.3f} | {s_:.3f} | {s_ - r['measured']:+.3f} | {min(h, s_):.2f}–{max(h, s_):.2f} | {r['pred']['samples'][1]*100:.0f}% | {topt} |")
