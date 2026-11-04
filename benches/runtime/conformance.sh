#!/usr/bin/env bash
# CPython's own test modules with the gc tier (docs/targets/gc.md §6), in the rbox venv of a
# build (run from the source root on the dev box: `rbox <ws> run --py B -- bash benches/runtime/conformance.sh OUT`).
#
#   1. benches/runtime/policy_suites.py --policy tier: plain vs RUSTIQUE_TIERS=gc +
#      rustique.enable(only=["gc"]) before the test module is imported (the gate of
#      tests/conformance/run_target_suites.py, whose `_takeover.enable` reads no environment
#      and so cannot request a tier); the policy steps aside where the modules tune the GC,
#      the forwarder stays.
#   2. benches/runtime/policy_suites.py --policy tier:step_aside=0: the same modules with the
#      policy kept armed through their own GC tuning (research: exercises deferral + forwarder
#      under test_gc, test_weakref, test_finalization, ...).
# Three parallel groups each; results in OUT/{conf,armed}-<build>-g<i>.{json,log}.
set -uo pipefail
out=${1:?out dir}; mkdir -p "$out"
b=$(python -c 'import sys; print(sys.version.split()[0] + ("t" if not sys._is_gil_enabled() else ""))')
G1=test.test_gc,test.test_weakref,test.test_finalization,test.test_sys,test.test_copy,test.test_deque
G2=test.test_descr,test.test_dict,test.test_list,test.test_set,test.test_generators,test.test_coroutines,test.test_class,test.test_ordered_dict
G3=test.test_functools,test.test_exceptions,test.test_contextlib,test.test_scope,test.test_json,test.test_dataclasses,test.test_tracemalloc
[ -f "$CPYTHON_SRC/Lib/test/test_gc_stats.py" ] && G1=$G1,test.test_gc_stats
case $b in *t) G3=$G3,test.test_free_threading.test_gc;; esac
i=0
for G in $G1 $G2 $G3; do
    python benches/runtime/policy_suites.py --policy tier --modules "$G" \
        --json "$out/conf-$b-g$i.json" --timeout 2400 > "$out/conf-$b-g$i.log" 2>&1 &
    python benches/runtime/policy_suites.py --policy tier:step_aside=0 --modules "$G" \
        --json "$out/armed-$b-g$i.json" --timeout 2400 > "$out/armed-$b-g$i.log" 2>&1 &
    i=$((i + 1))
done
wait
echo "DONE $b"
