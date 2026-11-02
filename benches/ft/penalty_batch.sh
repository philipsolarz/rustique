#!/usr/bin/env bash
# Record steady-state profiles of workloads on the current build (see perfprof.sh), one
# after the other: benches/ft/penalty_batch.sh <outdir> <n> <workload>...
out=$1 n=$2; shift 2
for w in "$@"; do
    "$(dirname "$0")/perfprof.sh" record "$w" "$out" -n "$n" > /dev/null 2>&1 && echo "done $w" || echo "FAILED $w"
done
