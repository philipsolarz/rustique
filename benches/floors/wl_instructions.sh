#!/usr/bin/env bash
# Instructions (user space, hardware counter) per steady-state run() of each workload,
# enabled and plain: perf stat over `run.py one <wl> [-n 1 | -n 4]`, (I4 - I1) / 3.
#   bash benches/floors/wl_instructions.sh <out.tsv> <family/name>...
set -uo pipefail
out=$1; shift
for wl in "$@"; do
  fam=${wl%%/*}; py=${WLROOT:-/work/w2-floors}/wl-3.15.1-$fam/bin/python
  for v in on plain; do
    flag=$([ $v = on ] && echo --enable || echo "")
    i1=$(perf stat -x, -e instructions:u "$py" benches/workloads/run.py one "$wl" $flag -n 1 2>&1 >/dev/null | awk -F, '/instructions/{print $1}' | tail -1)
    i4=$(perf stat -x, -e instructions:u "$py" benches/workloads/run.py one "$wl" $flag -n 4 2>&1 >/dev/null | awk -F, '/instructions/{print $1}' | tail -1)
    echo -e "$wl\t$v\t$i1\t$i4\t$(( (i4 - i1) / 3 ))" >> "$out"
  done
done
