#!/usr/bin/env bash
# CPython test modules of the data-model groups, plain vs enabled (tests/conformance/
# run_target_suites.py), one JSON report per module: benches/datamodel/conformance_all.sh OUTDIR
set -u
out=${1:?outdir}
mkdir -p "$out"
T=enum,generated,contextlib,functools
E_ENUM=enum.EnumType.__call__,enum.Enum.__hash__,enum.Flag.operators
# enum.property.__get__ is not validated on free-threaded builds this wave.
python -c 'import sys; sys.exit(0 if sys._is_gil_enabled() else 1)' && E_ENUM=enum.property.__get__,$E_ENUM
run() { # module expect
  python tests/conformance/run_target_suites.py --targets $T --modules "$1" --expect-installed "$2" \
    --json "$out/${1#test.}.json" --timeout 3000 > "$out/${1#test.}.log" 2>&1
  echo "$1 exit=$?"
}
run test.test_enum "$E_ENUM"
run test.test_dataclasses dataclasses.__init__
run test.test_collections collections.namedtuple
run test.test_typing collections.namedtuple,dataclasses.__init__
run test.test_contextlib contextlib.contextmanager
run test.test_contextlib_async contextlib.contextmanager
run test.test_functools functools.singledispatch,functools.singledispatchmethod.__get__
run test.test_copy functools.singledispatch
run test.test_inspect functools.singledispatch
run test.test_weakref functools.singledispatch
run test.test_pickle collections.namedtuple
