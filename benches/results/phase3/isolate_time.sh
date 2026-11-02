#!/usr/bin/env bash
# Re-measure the kernel-path wall-time cells that read > 1.05, alone, 10 processes x 5 values, CPU 2.
set -u
OUT=~/phase3/results/scoreboard/isolated-time.txt
CELLS="frozenset/all(x)/mixed frozenset/max(x)/str set/all(x)/str frozenset/all(x)/object frozenset/min(x)/str set/all(x)/mixed dict/any(x)/mixed set/all(x)/float"
for b in 3.15.1 3.15.1t; do
  for cell in $CELLS; do
    IFS=/ read -r t op st <<<"$cell"
    for rep in 1 2; do
      j=$(mktemp -d)/t.json
      ( cd ~/phase3/w/$b && VIRTUAL_ENV=~/phase3/venv/$b PATH=~/phase3/venv/$b/bin:$PATH \
        python benches/scoreboard.py time --type "$t" --op "$op" --storage "$st" --size 1e4 --impls enabled \
          --shards 1 --affinity 2 -p 10 -n 5 -o "$j" >/dev/null 2>&1 )
      ~/phase3/venv/$b/bin/python - "$j.shard0.json" "$t/$op/$st/10000" "$b" "$rep" <<'PY' | tee -a "$OUT"
import sys, pyperf
s = pyperf.BenchmarkSuite.load(sys.argv[1])
m = {b.get_name(): b for b in s.get_benchmarks()}
k = sys.argv[2]
bi, en = m[k + "/builtin"], m[k + "/enabled"]
print(f"{sys.argv[3]:8} {k:32} builtin {bi.mean()*1e6:9.1f} us (rsd {bi.stdev()/bi.mean():.3f})  "
      f"enabled {en.mean()*1e6:9.1f} us (rsd {en.stdev()/en.mean():.3f})  ratio {en.mean()/bi.mean():.3f}  "
      f"(isolated, run {sys.argv[4]}, {bi.get_nrun()} runs)")
PY
    done
  done
done
echo ISO-DONE >> "$OUT"
