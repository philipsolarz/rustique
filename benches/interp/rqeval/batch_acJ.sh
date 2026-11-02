#!/usr/bin/env bash
# acJ: acH + per-group net-benefit suspension. PGO build of all four overrides (isinstance, attribute cache, object.__setattr__,
# keyword instantiation), differential check, cells, full regrtest, whole runs.
cd /work/w2-interp/src
OPS=CALL_ISINSTANCE,LOAD_ATTR_FAMILY,CALL_NON_PY_GENERAL,CALL_KW_NON_PY
bash benches/interp/rqeval/build_pgo.sh _rqeval_acJ overrides_attrcache.c -DRQ_ATTR_CACHE -DRQ_INLINE_HELPERS -mllvm -inline-threshold=400 > /work/w2-interp/pgo_acJ.log 2>&1 || exit 1
( cd benches/interp/rqeval && python kwtest.py > /work/w2-interp/kwJ_plain.txt 2>&1; python kwtest.py _rqeval_acJ > /work/w2-interp/kwJ_swap.txt 2>&1; diff /work/w2-interp/kwJ_plain.txt /work/w2-interp/kwJ_swap.txt > /work/w2-interp/kwJ_diff.txt && echo "kwtest IDENTICAL" > /work/w2-interp/kwJ_diff.txt )
taskset -c 2-3 python benches/interp/callpaths_cg.py --swap /work/w2-interp/rqeval:_rqeval_acJ:CALL_KW_NON_PY -o /work/w2-interp/out/callpaths_acJ.json > /work/w2-interp/callpaths_acJ.log 2>&1 &
true
taskset -c 20-23,4-7 bash benches/interp/rqeval/run_tests.sh swap:_rqeval_acJ:$OPS /work/w2-interp/regrtest3 -j8 -q > /work/w2-interp/tests8.log 2>&1
W="$(ls /work/w2-interp/ext/frameworks/*.py) $(ls /work/w2-interp/ext/custom/*.py) benches/workloads/web/django_blog.py benches/workloads/devtools/sqlglot_transpile.py" \
V="--variant swap:_rqeval_acJ:$OPS" P=9 bash benches/interp/run_wlcg.sh /work/w2-interp/wlcg9 8-19 > /work/w2-interp/wlcg9.log 2>&1
