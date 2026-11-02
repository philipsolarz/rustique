#!/usr/bin/env bash
# Workstream `core` verification on the dev box: CPython's own test modules for every group
# the workstream owns, plain vs enabled, same outcome per test id (docs/ACCELERATION.md §7.1).
# Usage (on the box, in /work/<ws>/src):
#   bash benches/targets/core_conformance.sh <ws> [builds...] > /work/<ws>/out/conformance.log 2>&1
set -u
ws=${1:-w2ws-core}; shift || true
builds=${*:-3.15.1 3.15.1t 3.14.9 3.14.9t}
out=/work/$ws/out
mkdir -p "$out"
for b in $builds; do
  V=/work/$ws/venv-$b
  export LD_LIBRARY_PATH=$($V/bin/python -c 'import sysconfig; print(sysconfig.get_config_var("LIBDIR"))')
  export CPYTHON_SRC=/work/cpython/v${b%t}
  run() { # name targets modules expect
    echo "=== $b $1"
    $V/bin/python tests/conformance/run_target_suites.py --targets "$2" --modules "$3" \
      ${4:+--expect-installed "$4"} --json "$out/conf-$b-$1.json" 2>&1 | tail -8
  }
  run containers containers,builtins test.test_list,test.test_tuple,test.test_sort,test.test_builtin,test.test_contains \
    list.sort,list.index,list.count,tuple.index,tuple.count,list.__contains__,tuple.__contains__,builtins.sum,builtins.min,builtins.max,builtins.sorted,builtins.any,builtins.all
  run json json,float test.test_json,test.test_float,test.test_format,test.test_strtod json.dumps,json.loads,float.repr
  run copy copy,dataclasses test.test_copy,test.test_copyreg,test.test_dataclasses,test.test_uuid \
    copy.deepcopy,copy.copy,dataclasses.asdict,dataclasses.astuple
  case $b in
    *t) run set set test.test_set,test.test_copy,test.test_pickle set.__reduce__ ;;
    *) run set set test.test_set ;;
  esac
done
echo "=== DONE"
