#!/usr/bin/env bash
# After batch_acF.sh's regrtest runs: cells and the full regrtest for _rqeval_acG.
cd /work/w2-interp/src
taskset -c 4-7 python benches/interp/attr_cg.py --swap /work/w2-interp/rqeval:_rqeval_acG -o /work/w2-interp/out/attr_acG.json > /work/w2-interp/attr_acG.log 2>&1
taskset -c 4-7 python benches/interp/isinstance_cg.py --swap /work/w2-interp/rqeval:_rqeval_acG -o /work/w2-interp/out/isinstance_acG.json > /work/w2-interp/isinstance_acG.log 2>&1
taskset -c 4-7 python benches/interp/setattr_cg.py --swap /work/w2-interp/rqeval:_rqeval_acG -o /work/w2-interp/out/setattr_acG.json > /work/w2-interp/setattr_acG.log 2>&1
until [ "$(grep -c rc= /work/w2-interp/tests4.log)" -ge 2 ]; do sleep 20; done
taskset -c 20-23,4-7 bash benches/interp/rqeval/run_tests.sh swap:_rqeval_acG:CALL_ISINSTANCE,LOAD_ATTR_FAMILY,CALL_NON_PY_GENERAL /work/w2-interp/regrtest3 -j8 -q
