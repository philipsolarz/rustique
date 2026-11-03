#!/usr/bin/env bash
# z07i.sh: the four set/frozenset 1M int/float sorted() cells with sb_isolate.py itself (one scoreboard.py time
# run per cell and repetition: --shards 1 -p 10 -n 5, alone on CPU 10 = domain 1), from a result file
# that holds only those cells (taken from sort-time-3.15.1-run1), --bar 0.5, --reps 2.
S=/work/w2p3/results/z07/sort
taskset -c 12 python3 - <<'PY'
import json
d = json.load(open("/work/w2p3/results/z07/sort/sort-time-3.15.1-run1.json.shard0.json"))
keep = {f"{t}/sorted(x)/{s}/1000000" for t in ("set", "frozenset") for s in ("int", "float")}
d["benchmarks"] = [b for b in d["benchmarks"] if b["metadata"]["name"].rsplit("/", 1)[0] in keep]
json.dump(d, open("/work/w2p3/results/z07/sort/four-cells.json.shard0.json", "w"))
print(len(d["benchmarks"]))
PY
echo "[$(date -u +%FT%TZ)] START isolate-four-3.15.1 (CPU 10)" >> /work/w2p3/results/z07/steps.log
cd /work/w2p3z/w/3.15.1 && /work/w2p3z/venv/3.15.1/bin/python benches/results/w2-phase0/sb_isolate.py time \
    --results $S/four-cells.json.shard0.json --cpus 10 --reps 2 --bar 0.5 -o $S/isolated-four-3.15.1.txt > $S/isolated-four-3.15.1.log 2>&1
echo "[$(date -u +%FT%TZ)] END   isolate-four-3.15.1 rc=$?" >> /work/w2p3/results/z07/steps.log
