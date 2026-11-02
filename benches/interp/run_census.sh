#!/usr/bin/env bash
# Specialization census of the interp slice's workload set, plain and with rustique.enable().
set -euo pipefail
out=${1:?outdir}; cores=${2:-12-15}
mkdir -p "$out"
EXT=${EXT:-/work/w2-interp/ext}
W="benches/workloads/web/django_blog.py benches/workloads/web/flask_jinja.py
   benches/workloads/devtools/pylint_stdlib.py benches/workloads/devtools/sqlglot_transpile.py
   $EXT/custom/order_domain.py $EXT/frameworks/drf_api.py"
for w in $W; do
  n=$(basename "$w" .py)
  taskset -c "$cores" python benches/interp/spec_census.py "$w" -o "$out/$n.plain.census.json"
  taskset -c "$cores" python benches/interp/spec_census.py "$w" --enable -o "$out/$n.enable.census.json"
done
