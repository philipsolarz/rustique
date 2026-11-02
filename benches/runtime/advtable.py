#!/usr/bin/env python3
"""Adversarial-suite table: python benches/runtime/advtable.py OUTROOT (holds adv-<scenario>/)

Per scenario x build x policy (pooled over processes and measured runs): peak RSS (VmHWM
per run, max and median), garbage lifetime (time from a sampled cycle becoming unreachable
to the collection that freed it: median and max over runs), sampled cycles still uncollected
when the run ended, objects examined by automatic collections per run, largest collection,
run time (median)."""
import glob
import json
import os
import statistics as st
import sys

root = sys.argv[1]
print("scenario | build | policy | peak RSS MB max (median) | × default | garbage lifetime p50 / max ms | uncollected at run end (of sampled) | examined per run | largest collection | run ms")
print("---|---|---|---|---|---|---|---|---|---")
for d in sorted(glob.glob(os.path.join(root, "adv-*"))):
    sc = d.split("adv-")[-1]
    for py in sorted(os.listdir(d)):
        base = None
        rows = []
        for pol in ("default", "adaptive", "adaptive:cap=1.0", "adaptive:cap=0", "adaptive:y_high=0.25", "adaptive:kmax=5"):
            fs = glob.glob(os.path.join(d, py, "*", f"{pol}.*.json"))
            if not fs:
                continue
            runs = [r for f in fs for r in json.load(open(f))["runs"]]
            hwm = [r["hwm_kb"] / 1024 for r in runs]
            ex = [r["extra"] or {} for r in runs]
            p50 = [e["lag_p50"] for e in ex if "lag_p50" in e]
            mx = [e["lag_max"] for e in ex if "lag_max" in e]
            unc = [e.get("uncollected_at_end", 0) for e in ex]
            samp = ex[0].get("sampled", 0)
            work = [sum(v["candidates"] for k, v in r["gc"].items() if not k.startswith("x")) for r in runs]
            sizes = [c[5] for r in runs for c in (r.get("collections") or []) if not (len(c) > 6 and c[6])]
            row = (max(hwm), st.median(hwm))
            if pol == "default":
                base = row[0]
            rows.append(f"{sc} | {py} | {pol} | {row[0]:.0f} ({row[1]:.0f}) | {row[0] / base if base else float('nan'):.2f} | "
                        f"{1e3 * st.median(p50) if p50 else float('nan'):.0f} / {1e3 * max(mx) if mx else float('nan'):.0f} | "
                        f"{max(unc)} ({samp}) | {st.median(work):,.0f} | {max(sizes) if sizes else 0:,.0f} | "
                        f"{1e3 * st.median(r['wall_s'] for r in runs):.0f}")
        print("\n".join(rows))
