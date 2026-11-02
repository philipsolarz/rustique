#!/usr/bin/env bash
# acK: acJ + main-interpreter guard on every override fast path. PGO build of all four overrides (isinstance, attribute cache, object.__setattr__,
# keyword instantiation), differential check, cells, full regrtest, whole runs.
cd /work/w2-interp/src
OPS=CALL_ISINSTANCE,LOAD_ATTR_FAMILY,CALL_NON_PY_GENERAL,CALL_KW_NON_PY
bash benches/interp/rqeval/build_pgo.sh _rqeval_acK overrides_attrcache.c -DRQ_ATTR_CACHE -DRQ_INLINE_HELPERS -mllvm -inline-threshold=400 > /work/w2-interp/pgo_acK.log 2>&1 || exit 1
( cd benches/interp/rqeval && python kwtest.py > /work/w2-interp/kwK_plain.txt 2>&1; python kwtest.py _rqeval_acK > /work/w2-interp/kwK_swap.txt 2>&1; diff /work/w2-interp/kwK_plain.txt /work/w2-interp/kwK_swap.txt > /work/w2-interp/kwK_diff.txt && echo "kwtest IDENTICAL" > /work/w2-interp/kwK_diff.txt )
taskset -c 2-3 python benches/interp/callpaths_cg.py --swap /work/w2-interp/rqeval:_rqeval_acK:CALL_KW_NON_PY -o /work/w2-interp/out/callpaths_acK.json > /work/w2-interp/callpaths_acK.log 2>&1 &
true
taskset -c 20-23,4-7 bash benches/interp/rqeval/run_tests.sh swap:_rqeval_acK:$OPS /work/w2-interp/regrtest3 -j8 -q > /work/w2-interp/tests9.log 2>&1
