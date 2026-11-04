#!/usr/bin/env bash
# Per-opcode profiles of the interp slice's workload set (run on the dev box via rbox).
#   bash benches/interp/run_opprof.sh <outdir> [plain|jit|enable] [cores]
set -euo pipefail
out=${1:?outdir}; mode=${2:-plain}; cores=${3:-8-11}
mkdir -p "$out"
EXT=${EXT:-/work/w2-interp/ext}
W=${W:-"benches/workloads/web/django_blog.py benches/workloads/web/flask_jinja.py
   benches/workloads/devtools/pylint_stdlib.py benches/workloads/devtools/sqlglot_transpile.py
   $EXT/custom/order_domain.py $EXT/frameworks/drf_api.py"}
flag=""
case $mode in jit) flag=--jit;; enable) flag=--enable;; esac
for w in $W; do
  n=$(basename "$w" .py)
  python benches/interp/opprof.py record "$w" -o "$out/$n.$mode.json" --seconds ${SECS:-8} --taskset "$cores" --keep $flag
done
