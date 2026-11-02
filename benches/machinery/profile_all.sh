#!/usr/bin/env bash
# Profile workloads with profile_machinery.py, N at a time.
#   profile_all.sh <outdir> <parallel> <workload.py>...
set -u
out=$1; par=$2; shift 2
mkdir -p "$out"
here=$(cd "$(dirname "$0")" && pwd)
printf '%s\n' "$@" | xargs -P "$par" -I{} bash -c '
  w={}; n=$(basename "$w" .py)
  python '"$here"'/profile_machinery.py profile "$w" --out '"$out"'/$n.json --seconds ${SECONDS_PER:-4} \
     --cache /work/w2-machinery/cache ${EXTRA:-} > '"$out"'/$n.log 2>&1 || echo "FAILED $n" >> '"$out"'/failed.txt'
