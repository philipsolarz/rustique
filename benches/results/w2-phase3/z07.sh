#!/usr/bin/env bash
# z07.sh: the final code, wave2 07786e2 (= 78751e2 + core's large-sort fix 2e02b1b + test/CI-only
# changes), in its own tree (/work/w2p3z), results in /work/w2p3/results/z07.
#   1. fat-LTO release builds of the four interpreters + pytest, one domain each (untimed)
#   2. timing, one job per L3 domain:
#      domain 1: headline replicate 3 on 3.15.1        domain 2: headline replicate 3 on 3.15.1t
#      CPU 2 / CPU 30: the scoreboard's sort cells at 10k/1M (run 1 / run 2, -p 10 -n 5, alone on
#      the CPU), then core's _wall_big_sort.py (large shapes, wall), then the same sort cells on 3.15.1t
set -uo pipefail
export COMMIT=07786e2 BASE=/work/w2p3z RES=/work/w2p3/results/z07 BUNDLE=/work/w2p3/rustique-z07.bundle
R=/work/w2p3/run_w2phase3.sh
mkdir -p "$BASE/bin" "$RES/sort"; cp -r /work/w2p3/bin/. "$BASE/bin/"
note() { echo "[$(date -u +%FT%TZ)] $*" >> "$RES/steps.log"; }
lane() { local d=$1 b=$2
    CARGO_BUILD_JOBS=8 taskset -c $d $R build $b && taskset -c $d $R wheel $b && taskset -c $d $R status $b
    case $b in 3.15.1|3.15.1t) taskset -c $d $R wlvenv $b ;; esac
    taskset -c $d $R test $b; }
sortcells() { local b=$1 cpu=$2 run=$3
    note "START sort-cells-$b-run$run (CPU $cpu)"
    (cd $BASE/w/$b && $BASE/venv/$b/bin/python benches/scoreboard.py time --op sort --size 1e4,1e6 --shards 1 \
        --affinity $cpu -p 10 -n 5 -o $RES/sort/sort-time-$b-run$run.json) > $RES/sort/sort-time-$b-run$run.log 2>&1
    note "END   sort-cells-$b-run$run rc=$?"; }
bigsort() { local b=$1 cpu=$2 run=$3
    note "START wall-big-sort-$b-run$run (CPU $cpu)"
    (cd $BASE/w/$b && taskset -c $cpu $BASE/venv/$b/bin/python benches/targets/_wall_big_sort.py) > $RES/sort/wall-big-sort-$b-run$run.txt 2>&1
    note "END   wall-big-sort-$b-run$run rc=$?"; }
note "z07 start"
$R setup
lane 0-7 3.15.1 & lane 8-15 3.15.1t & lane 16-23 3.14.9 & lane 24-31 3.14.9t &
wait
note "builds and tests done"
$R workloads 3.15.1 8-15 &
$R workloads 3.15.1t 16-23 &
( sortcells 3.15.1 2 1; bigsort 3.15.1 2 1; sortcells 3.15.1t 2 1 ) &
( sortcells 3.15.1 30 2; bigsort 3.15.1 30 2; sortcells 3.15.1t 30 2 ) &
wait
note "z07 done"
