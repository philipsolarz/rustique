#!/usr/bin/env bash
# Instructions per steady-state run() of workloads, plain and enabled (all groups), with
# digests, on the dev box: perf stat over `run.py one <wl> [--enable] -n 1 | -n 4`,
# per-run = (I4 - I1) / 3 (w2-floors' method). Workload venvs: /work/w2-runtime/wl/<B>-<family>,
# this build's package through PYTHONPATH.
#   bash benches/targets/core_workloads.sh <ws> <B> <out.tsv> <family/name>...
set -uo pipefail
ws=$1; b=$2; out=$3; shift 3
export PYTHONPATH=/work/$ws/src/python
for wl in "$@"; do
  fam=${wl%%/*}; py=/work/w2-runtime/wl/$b-$fam/bin/python
  for v in plain on; do
    flag=$([ $v = on ] && echo --enable || echo "")
    i1=$(perf stat -x, -e instructions:u "$py" benches/workloads/run.py one "$wl" $flag -n 1 2>&1 >/dev/null | awk -F, '/instructions/{print $1}' | tail -1)
    res=$(perf stat -x, -e instructions:u -o /tmp/$ws-perf.$$ "$py" benches/workloads/run.py one "$wl" $flag -n 4 2>/dev/null | tail -1)
    i4=$(awk -F, '/instructions/{print $1}' /tmp/$ws-perf.$$ | tail -1)
    dig=$(printf '%s' "$res" | python3 -c 'import json,sys; d=json.loads(sys.stdin.read() or "{}"); print(d.get("digest","?"))' 2>/dev/null)
    echo -e "$b\t$wl\t$v\t$i1\t$i4\t$(( (i4 - i1) / 3 ))\t$dig" >> "$out"
  done
done
rm -f /tmp/$ws-perf.$$
