#!/usr/bin/env bash
# acF: PGO build of the final override set, its cells, then the regrtest runs (after tests3).
cd /work/w2-interp/src
bash benches/interp/rqeval/build_pgo.sh _rqeval_acF overrides_attrcache.c -DRQ_ATTR_CACHE -DRQ_INLINE_HELPERS -mllvm -inline-threshold=400 > /work/w2-interp/pgo_acF.log 2>&1 || exit 1
taskset -c 4-7 python benches/interp/isinstance_cg.py --swap /work/w2-interp/rqeval:_rqeval_acF -o /work/w2-interp/out/isinstance_acF.json > /work/w2-interp/isinstance_acF.log 2>&1
taskset -c 4-7 python benches/interp/setattr_cg.py --swap /work/w2-interp/rqeval:_rqeval_acF -o /work/w2-interp/out/setattr_acF.json > /work/w2-interp/setattr_acF.log 2>&1
taskset -c 4-7 python benches/interp/attr_cg.py --swap /work/w2-interp/rqeval:_rqeval_acF -o /work/w2-interp/out/attr_acF.json > /work/w2-interp/attr_acF.log 2>&1
until [ "$(grep -c rc= /work/w2-interp/tests3.log)" -ge 5 ]; do sleep 20; done
for v in noop:_rqeval_acF swap:_rqeval_acF:CALL_ISINSTANCE,LOAD_ATTR_FAMILY,CALL_NON_PY_GENERAL; do
  taskset -c 20-23,4-7 bash benches/interp/rqeval/run_tests.sh $v /work/w2-interp/regrtest3 -j8 -q
done
