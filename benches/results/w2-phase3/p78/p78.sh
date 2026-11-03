#!/usr/bin/env bash
# p78.sh A|B: text's posixpath/pathlib/fnmatch fix (wave2 78751e2 = 9d0abae + b2a12b1 + the fix), in its
# own tree (/work/w2p3p), results in /work/w2p3/results/p78. Wave C waits for both parts.
#   A (domain 1): builds of the four interpreters (untimed), the paths groups' conformance modules
#     on 4 builds, data/devtools venvs; then (timing) fs_inventory + pytest_networkx, run.py compare
#     -n 5 -p 5 on 3.15.1
#   B (domain 3, timing): kernel_scaling.py plain and --enable on 3.15.1t (-r 5, the whole table),
#     then the same short compare on 3.15.1t
set -uo pipefail
export COMMIT=78751e2 BASE=/work/w2p3p RES=/work/w2p3/results/p78 BUNDLE=/work/w2p3/rustique-p78.bundle
R=/work/w2p3/run_w2phase3.sh
D1=8-15 D3=24-31
mkdir -p "$BASE/bin" "$RES/ftq" "$RES/workloads"; cp -r /work/w2p3/bin/. "$BASE/bin/"
note() { echo "[$(date -u +%FT%TZ)] $*" >> "$RES/steps.log"; }
compare() { # compare <b> <cpus>
    local b=$1 c=$2 w
    for w in data/fs_inventory devtools/pytest_networkx; do
        (cd $BASE/w/$b && taskset -c $c $BASE/wl/$b-${w%%/*}/bin/python benches/workloads/run.py compare $w -n 5 -p 5 \
            -o $RES/workloads/compare-$b-${w%%/*}.json) >> $RES/logs/short-compare-$b.log 2>&1
    done
}
case ${1:?A or B} in
A)
    note "p78 A start"
    taskset -c $D1 $R setup
    for b in 3.15.1t 3.15.1 3.14.9 3.14.9t; do
        CARGO_BUILD_JOBS=8 taskset -c $D1 $R build $b && taskset -c $D1 $R wheel $b && taskset -c $D1 $R status $b
        [[ $b == 3.15.1t ]] && touch $RES/built-3.15.1t
    done
    for b in 3.15.1 3.15.1t 3.14.9 3.14.9t; do
        CONF_MODULES=test.test_posixpath,test.test_genericpath,test.test_fnmatch,test.test_glob,test.test_pathlib,test.test_os \
            taskset -c $D1 $R conformance $b
    done
    FAMILIES="data devtools" taskset -c $D1 $R wlvenv 3.15.1; FAMILIES="data devtools" taskset -c $D1 $R wlvenv 3.15.1t
    touch $RES/A.venvs
    note "START short-compare-3.15.1"; compare 3.15.1 $D1; note "END   short-compare-3.15.1"
    note "p78 A done"
    ;;
B)
    until [[ -e $RES/built-3.15.1t ]]; do sleep 10; done
    note "START kernel_scaling 3.15.1t (domain 3)"
    cd $BASE/w/3.15.1t
    taskset -c $D3 $BASE/venv/3.15.1t/bin/python benches/ft/kernel_scaling.py -r 5 -o $RES/ftq/ks-ft-plain.json > $RES/ftq/ks-ft-plain.log 2>&1
    taskset -c $D3 $BASE/venv/3.15.1t/bin/python benches/ft/kernel_scaling.py --enable -r 5 -o $RES/ftq/ks-ft-enabled.json > $RES/ftq/ks-ft-enabled.log 2>&1
    $BASE/venv/3.15.1t/bin/python benches/ft/kernel_scaling.py table $RES/ftq/ks-ft-plain.json $RES/ftq/ks-ft-enabled.json > $RES/ftq/kernels.md
    note "END   kernel_scaling 3.15.1t"
    until [[ -e $RES/A.venvs ]]; do sleep 10; done
    note "START short-compare-3.15.1t"; compare 3.15.1t $D3; note "END   short-compare-3.15.1t"
    note "p78 B done"
    ;;
esac
