#!/usr/bin/env bash
# accel-core final verification on the dev box (all four builds): full pytest and the
# containers/builtins conformance comparison. Usage (on the box, in /work/accel-core/src):
#   nohup bash benches/_accel_final.sh > /work/accel-core/final.log 2>&1 &
set -u
cd /work/accel-core/src
for b in 3.15.1 3.15.1t 3.14.9 3.14.9t; do
  V=/work/accel-core/venv-$b
  export LD_LIBRARY_PATH=$($V/bin/python -c 'import sysconfig; print(sysconfig.get_config_var("LIBDIR"))')
  export CPYTHON_SRC=/work/cpython/v${b%t}
  echo "=== $b pytest"
  taskset -c 4-7 $V/bin/python -m pytest -q -p no:cacheprovider 2>&1 | tail -3
  echo "=== $b conformance"
  taskset -c 4-7 $V/bin/python tests/conformance/run_target_suites.py --targets containers,builtins \
    --modules test.test_list,test.test_tuple,test.test_sort,test.test_builtin \
    --expect-installed list.sort,list.index,list.count,tuple.index,tuple.count,list.__contains__,tuple.__contains__,builtins.sum,builtins.min,builtins.max,builtins.sorted,builtins.any,builtins.all \
    --json /work/accel-core/conf-$b.json 2>&1 | tail -12
done
echo "=== DONE"
