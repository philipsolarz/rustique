#!/usr/bin/env bash
# re9.sh A|B: the re group's first-call rework (wave2 9d0abae = 7fec7e4 + re), in its own tree
# (/work/w2p3r), results in /work/w2p3/results/re9. Steady-state stages stay on 7fec7e4.
#   A (domain 0, untimed; holds the threads addendum until done): builds, the four test suites,
#     status on every interpreter (T10, the 3.15.0 re line), the re group's 16 conformance
#     modules, workload venvs (for the whole-process runs), install costs (T9), re degradation probe
#   B (domain 3, timing; after 7fec7e4's 3.15.1 headline there and after A): whole-process runs,
#     startup cells and decomposition (T8)
set -uo pipefail
export COMMIT=9d0abae BASE=/work/w2p3r RES=/work/w2p3/results/re9 BUNDLE=/work/w2p3/rustique-re9.bundle
R=/work/w2p3/run_w2phase3.sh
D0=0-7 D3=24-31
mkdir -p "$BASE/bin" "$RES/startup"; cp -r /work/w2p3/bin/. "$BASE/bin/"
note() { echo "[$(date -u +%FT%TZ)] $*" >> "$RES/steps.log"; }
case ${1:?A or B} in
A)
    note "re9 A start"
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
    for b in 3.15.1 3.15.1t 3.14.9 3.14.9t; do
        CONF_MODULES=test.test_re,test.test_email,test.test_tokenize,test.test_configparser,test.test_fnmatch,test.test_textwrap,test.test_http_cookies,test.test_difflib,test.test_string,test.test_shlex,test.test_argparse,test.test_gettext,test.test_glob,test.test_csv,test.test_urllib,test.test_mailbox \
            taskset -c $D0 $R conformance $b
    done
    taskset -c $D0 $R wlvenv 3.15.1; taskset -c $D0 $R wlvenv 3.15.1t
    (cd "$BASE/w/3.15.1" && taskset -c $D0 "$BASE/venv/3.15.1/bin/python" benches/accel/install_costs.py \
        -o "$RES/startup/install-costs-3.15.1.json" > "$RES/logs/install-costs-3.15.1.log" 2>&1)
    (cd "$BASE/w/3.14.9" && taskset -c $D0 "$BASE/venv/3.14.9/bin/python" benches/accel/install_costs.py \
        -o "$RES/startup/install-costs-3.14.9.json" > "$RES/logs/install-costs-3.14.9.log" 2>&1)
    (cd "$BASE/w/3.15.1" && taskset -c $D0 "$BASE/venv/3.15.1/bin/python" "$BASE/bin/install_split.py" > "$RES/startup/install-split-3.15.1.txt" 2>&1)
    for v in 3.15.0 3.15.2 3.14.1 3.14.10; do (cd /tmp && taskset -c $D0 "$BASE/sv/$v/bin/python" "$BASE/bin/re_probe.py"); done \
        > "$RES/status/degraded-re-probe.txt" 2>&1
    touch "$RES/A.done"; note "re9 A done"
    ;;
B)
    until grep -q "END   workloads-3.15.1 " /work/w2p3/results/final/steps.log 2>/dev/null && [[ -e $RES/A.done ]]; do sleep 15; done
    note "re9 B start (domain 3)"
    $R whole 3.15.1 $D3; $R whole 3.15.1t $D3
    $R startup 3.15.1 30; $R startup 3.14.9 30
    touch "$RES/B.done"; note "re9 B done"
    ;;
esac
