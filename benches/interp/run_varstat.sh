#!/usr/bin/env bash
# Whole-workload instructions/cycles per run for the eval-loop variants (dev box).
#   bash benches/interp/run_varstat.sh <outdir> [core]
set -uo pipefail
out=${1:?}; core=${2:-16}
mkdir -p "$out"
EXT=${EXT:-/work/w2-interp/ext}
W=${W:-"benches/workloads/devtools/sqlglot_transpile.py benches/workloads/web/django_blog.py
   benches/workloads/web/flask_jinja.py $EXT/custom/order_domain.py $EXT/frameworks/drf_api.py
   $EXT/custom/rules_engine.py $EXT/custom/di_plugins.py $EXT/custom/orm_mapper.py"}
V=${V:-"--variant plain --variant swap:_rqeval_isinst:CALL_ISINSTANCE --variant swapall:_rqeval_C --variant copy:_rqeval_C"}
for w in $W; do
  n=$(basename "$w" .py)
  python benches/interp/varstat.py "$w" $V -r ${R:-2} --seconds ${SECS:-3} --taskset "$core" -o "$out/$n.json" 2>&1 | grep -v "Events disabled"
done
