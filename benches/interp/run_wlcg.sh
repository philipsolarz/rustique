#!/usr/bin/env bash
# Exact whole-run instruction counts (wl_cg.py) for many workloads in parallel (dev box).
#   W="..." V="--variant plain --variant ..." P=6 bash benches/interp/run_wlcg.sh <outdir> <cpus>
set -uo pipefail
out=${1:?}; cpus=${2:-12-19}
mkdir -p "$out"
export V out cpus
printf '%s\n' $W | xargs -P ${P:-6} -I{} bash -c 'n=$(basename {} .py); taskset -c $cpus python benches/interp/wl_cg.py {} $V -o $out/$n.json > $out/$n.log 2>&1; cat $out/$n.log'
