#!/usr/bin/env bash
# final.sh: the stages on wave2 7fec7e4 (the lead's final code commit), in their own tree
# (/work/w2p3f: clones, build venvs, wheels, workload venvs, pyperformance venvs), results in
# /work/w2p3/results/final. The d40e7b9 schedule (run_w2phase3.sh all) still runs its wave-B
# lanes (the opt-in tiers) in domains 1-3; each lane here starts in a domain only once that
# domain's d40e7b9 work has ended (its END line in results/steps.log). At most one timing job
# per domain; untimed work (builds, tests, conformance, callgrind) only in a domain without one.
#
#   A  (domain 0, untimed, at once): stage 1 (builds, 4 test suites, status on every interpreter,
#      Rust checks), conformance (text groups' 15 modules + the container/builtin modules core
#      changed), workload and pyperformance venvs; then the callgrind scoreboard (0-100, both
#      builds), isolation, heap offsets (sb_heap.py)
#   B3 (domain 3 when free): headline replicate 2 on 3.15.1, whole-process runs, startup cells
#   B2 (domain 2 when free): headline replicate 2 on 3.15.1t
#   B0 (domain 0 after A): threads scaling addendum (benches/ft/quietbox.sh)
#   B1 (domain 1 when free): estimator on 3.15.1 and 3.15.1t (untimed)
#   C  (all domains, after A and B*): pyperformance (one run per domain), ports alone, the gc
#      tier's addendum (domains 0+1), scoreboard wall time 10k/1M + isolation + sb_probe
set -uo pipefail
export COMMIT=7fec7e4 BASE=/work/w2p3f RES=/work/w2p3/results/final BUNDLE=/work/w2p3/rustique-final.bundle
R=/work/w2p3/run_w2phase3.sh
M=/work/w2p3/results/steps.log
D0=0-7 D1=8-15 D2=16-23 D3=24-31
mkdir -p "$BASE/bin" "$RES"; cp -r /work/w2p3/bin/. "$BASE/bin/"
note() { echo "[$(date -u +%FT%TZ)] $*" >> "$RES/steps.log"; }
waitfor() { until grep -q -- "$1" "$2" 2>/dev/null; do sleep 15; done; }
waitfile() { until [[ -e $1 ]]; do sleep 15; done; }

laneA() {
    taskset -c $D0 $R setup
    for b in 3.15.1 3.15.1t 3.14.9 3.14.9t; do
        CARGO_BUILD_JOBS=8 taskset -c $D0 $R build $b && taskset -c $D0 $R wheel $b && taskset -c $D0 $R status $b
        taskset -c $D0 $R test $b
    done
    for x in "3.15.2 3.15.1" "3.15.2t 3.15.1t" "3.14.10 3.14.9" "3.14.10t 3.14.9t" "3.15.0 3.15.1" \
             "3.14.0 3.14.9" "3.14.1 3.14.9" "3.14.5 3.14.9" "3.14.6 3.14.9" "3.14.8 3.14.9"; do
        # shellcheck disable=SC2086
        taskset -c $D0 $R statusx $x
    done
    CARGO_BUILD_JOBS=8 taskset -c $D0 $R status316 3.16.0a1
    CARGO_BUILD_JOBS=8 taskset -c $D0 $R rust
    for b in 3.15.1 3.15.1t 3.14.9 3.14.9t; do
        CONF_MODULES=test.test_logging,test.test_strptime,test.test_datetime,test.test_time,test.test_textwrap,test.test_urlparse,test.test_urllib,test.test_posixpath,test.test_genericpath,test.test_fnmatch,test.test_glob,test.test_pathlib,test.test_os,test.test_random,test.test_statistics,test.test_list,test.test_tuple,test.test_sort,test.test_builtin \
            taskset -c $D0 $R conformance $b
    done
    taskset -c $D0 $R wlvenv 3.15.1; taskset -c $D0 $R wlvenv 3.15.1t
    taskset -c $D0 $R pyperf-venv 3.15.1; taskset -c $D0 $R pyperf-venv 3.15.1t
    touch "$RES/A.done"; note "A done (stage 1, conformance, venvs)"
    $R instr 3.15.1 $D0; $R instr 3.15.1t $D0
    $R isolate-instr 3.15.1 $D0; $R isolate-instr 3.15.1t $D0
    $R sbheap 3.15.1 $D0; $R sbheap 3.15.1t $D0
    touch "$RES/A2.done"; note "A2 done (callgrind scoreboard)"
}
laneB3() {
    waitfor "END   tiers-default-gc-3.15.1 " "$M"; waitfile "$RES/A.done"; note "B3 start (domain 3 free)"
    $R workloads 3.15.1 $D3
    $R whole 3.15.1 $D3; $R whole 3.15.1t $D3
    $R startup 3.15.1 30; $R startup 3.14.9 30
    touch "$RES/B3.done"
}
laneB2() {
    waitfor "END   tiers-default-gc-3.15.1t" "$M"; waitfile "$RES/A.done"; note "B2 start (domain 2 free)"
    $R workloads 3.15.1t $D2
    touch "$RES/B2.done"
}
laneB0() {
    waitfile "$RES/A2.done"
    $R ftquiet $D0
    touch "$RES/B0.done"
}
laneB1() {
    waitfor "END   tiers-default-gc+interp-3.15.1" "$M"; waitfile "$RES/A.done"; note "B1 start (domain 1 free)"
    $R est 3.15.1 0; $R est 3.15.1t 0     # est_validate.sh: cores first+8+i = 8-15
    touch "$RES/B1.done"
}
waveC() {
    for x in A2 B0 B1 B2 B3; do waitfile "$RES/$x.done"; done
    note "C start"
    $R pyperformance 3.15.1 plain $D0 & $R pyperformance 3.15.1 enabled $D1 &
    $R pyperformance 3.15.1t plain $D2 & $R pyperformance 3.15.1t enabled $D3 &
    wait
    $R pyperformance-ports 3.15.1 $D1; $R pyperformance-ports 3.15.1t $D1
    $R pyperformance-compare 3.15.1; $R pyperformance-compare 3.15.1t
    $R gcquiet $D0 $D1
    $R time 3.15.1 2,10,20,30; $R isolate-time 3.15.1 2,10,20,30; $R sbprobe 3.15.1 2,10,20,30
    $R report 3.15.1; $R report 3.15.1t
    $R pack
    note "C done"
}
note "final.sh start ($COMMIT)"
laneA & laneB3 & laneB2 & laneB0 & laneB1 & waveC &
wait
note "ALL done"
