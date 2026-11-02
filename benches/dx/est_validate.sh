#!/usr/bin/env bash
# Validate `python -m rustique estimate` on the 65 workloads (T13): per family, in parallel, one
# core each: `measure` (interleaved plain/enabled pairs, wall + CPU) and `all` (prediction).
#   benches/dx/est_validate.sh ROOT PY OUT [first-core]     (venvs from est_setup.sh)
#   STEPS="est" or "measure" runs one of the two (default both); FAMS="web data" a subset.
set -uo pipefail
root=$1; py=$2; out=$3; core=${4:-4}
src=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$out"
fams=(${FAMS:-web data devtools frameworks libs custom services threads})
steps=${STEPS:-measure est}
i=0
for fam in "${fams[@]}"; do
    v=$root/venvs/$fam-$py/bin/python
    m=$((core + i)); e=$((core + 8 + i))
    case $steps in *measure*)
    ( cd /tmp && taskset -c $m "$v" "$src/benches/dx/estimate_workloads.py" measure "$fam" -o "$out/measured-$fam.json" \
        > "$out/measure-$fam.log" 2>&1 ) & ;; esac
    case $steps in *est*)
    ( cd /tmp && taskset -c $e "$v" "$src/benches/dx/estimate_workloads.py" all "$fam" --build "$py" -o "$out/est-$fam.json" \
        > "$out/est-$fam.log" 2>&1 ) & ;; esac
    i=$((i + 1))
done
wait
echo "done $(date -u +%FT%TZ)" > "$out/DONE"
