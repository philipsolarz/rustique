#!/usr/bin/env python3
"""Long-run table (memory over time): python benches/runtime/longtable.py DIR

DIR is a gcprobe sweep with many runs per process (e.g. -n 40). Per build x workload x policy
(processes pooled): median wall per run, automatic GC ms and objects examined per run, RSS after
the first and the last run, RSS growth (mean of the second half minus mean of the first half of
the runs), peak RSS."""
import glob
import json
import os
import statistics as st
import sys

d = sys.argv[1]
print("build | workload | policy | run ms | GC ms/run | examined/run | RSS first → last MB | RSS growth 2nd−1st half MB | peak RSS MB")
print("---|---|---|---|---|---|---|---|---")
for py in sorted(os.listdir(d)):
    for w in sorted(os.listdir(os.path.join(d, py))):
        for pol in ("default", "adaptive"):
            fs = sorted(glob.glob(os.path.join(d, py, w, f"{pol}.*.json")))
            if not fs:
                continue
            walls, gcs, work, first, last, growth, peak = [], [], [], [], [], [], []
            for f in fs:
                runs = json.load(open(f))["runs"]
                rss = [r["rss_kb"] / 1024 for r in runs]
                h = len(rss) // 2
                walls += [r["wall_s"] for r in runs]
                gcs += [sum(v["time_s"] for k, v in r["gc"].items() if not k.startswith("x")) for r in runs]
                work += [sum(v["candidates"] for k, v in r["gc"].items() if not k.startswith("x")) for r in runs]
                first.append(rss[0]); last.append(rss[-1])
                growth.append(st.fmean(rss[h:]) - st.fmean(rss[:h]))
                peak.append(max(r["hwm_kb"] for r in runs) / 1024)
            print(f"{py} | {w.replace('__', '/')} | {pol} | {1e3 * st.median(walls):.0f} | {1e3 * st.median(gcs):.1f} | "
                  f"{st.median(work):,.0f} | {st.median(first):.0f} → {st.median(last):.0f} | "
                  f"{'/'.join(f'{g:+.1f}' for g in growth)} | {max(peak):.0f}")
