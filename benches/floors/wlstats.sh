#!/usr/bin/env bash
# Counters of real workloads (Phase 3's two record kinds), for w2-floors' analyses.
#   bash benches/floors/wlstats.sh run   <outdir> <family/name>...   # one steady-state run() (wlstats_run.py)
#   bash benches/floors/wlstats.sh whole <outdir> <family/name>...   # whole process, RUSTIQUE_REPORT (setup+warm-up+run)
# Env: WL=/work/w2-floors/wl-3.15.1-<family> venvs (workload pins + a .pth to a Rustique build);
#      RQ_SRC=<src/python of the Rustique build to use> (prepended to PYTHONPATH); any RUSTIQUE_* knobs.
set -uo pipefail
mode=$1; out=$2; shift 2
mkdir -p "$out"
for wl in "$@"; do
  fam=${wl%%/*}; py=${WLROOT:-/work/w2-floors}/wl-3.15.1-$fam/bin/python
  f="$out/${wl/\//__}.json"
  if [ "$mode" = run ]; then
    PYTHONPATH=${RQ_SRC:-} timeout 900 "$py" ${WLSTATS:-benches/results/phase3/wlstats_run.py} "$wl" "$f" >/dev/null 2>"$f.err" || echo "FAILED $wl"
  else
    PYTHONPATH=${RQ_SRC:-} RUSTIQUE_REPORT="$f" timeout 900 "$py" benches/workloads/run.py one "$wl" --enable -n 1 >/dev/null 2>"$f.err" || echo "FAILED $wl"
  fi
  echo "done $wl"
done
