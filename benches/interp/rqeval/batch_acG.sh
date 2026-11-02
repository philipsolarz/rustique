#!/usr/bin/env bash
# acG: PGO build of the final override set (acF + per-site negative hint), cells, then the
# whole-run batch for the frameworks + custom families, django_blog and sqlglot.
cd /work/w2-interp/src
bash benches/interp/rqeval/build_pgo.sh _rqeval_acG overrides_attrcache.c -DRQ_ATTR_CACHE -DRQ_INLINE_HELPERS -mllvm -inline-threshold=400 > /work/w2-interp/pgo_acG.log 2>&1 || exit 1
W="$(ls /work/w2-interp/ext/frameworks/*.py) $(ls /work/w2-interp/ext/custom/*.py) benches/workloads/web/django_blog.py benches/workloads/devtools/sqlglot_transpile.py" \
V="--variant swap:_rqeval_acG:CALL_ISINSTANCE,LOAD_ATTR_FAMILY,CALL_NON_PY_GENERAL --variant swap:_rqeval_acG:CALL_ISINSTANCE,LOAD_ATTR_FAMILY" P=9 \
  bash benches/interp/run_wlcg.sh /work/w2-interp/wlcg6 8-19 > /work/w2-interp/wlcg6.log 2>&1
