#!/usr/bin/env bash
# textfix.sh <commit> <cpus> [startup-cpu]: text's fingerprint follow-up, re-measured in its own
# tree (/work/w2p3t) so that the d40e7b9 builds the timing stages use stay untouched:
# stage 1 (builds, tests, status table on every interpreter), the text groups' conformance
# modules, and (only with a startup CPU, nothing else timing in its domain) the startup cells.
set -uo pipefail
export COMMIT=${1:?commit} BASE=/work/w2p3t RES=/work/w2p3/results/textfix BUNDLE=/work/w2p3/rustique-text.bundle
CPUS=${2:?cpus}; SU=${3:-}
export CONF_MODULES=test.test_logging,test.test_strptime,test.test_datetime,test.test_time,test.test_textwrap,test.test_urlparse,test.test_urllib,test.test_posixpath,test.test_genericpath,test.test_fnmatch,test.test_glob,test.test_pathlib,test.test_os,test.test_random,test.test_statistics
R=/work/w2p3/run_w2phase3.sh
mkdir -p "$BASE/bin" "$RES"; cp -r /work/w2p3/bin/. "$BASE/bin/"
if [[ -z $SU ]]; then
  taskset -c "$CPUS" $R setup
  for b in 3.15.1 3.15.1t 3.14.9 3.14.9t; do
    CARGO_BUILD_JOBS=8 taskset -c "$CPUS" $R build $b && taskset -c "$CPUS" $R wheel $b && taskset -c "$CPUS" $R status $b
    taskset -c "$CPUS" $R test $b
  done
  for x in "3.15.2 3.15.1" "3.15.2t 3.15.1t" "3.14.10 3.14.9" "3.14.10t 3.14.9t" "3.15.0 3.15.1" "3.14.0 3.14.9" "3.14.1 3.14.9" "3.14.5 3.14.9" "3.14.6 3.14.9" "3.14.8 3.14.9"; do
    taskset -c "$CPUS" $R statusx $x
  done
  CARGO_BUILD_JOBS=8 taskset -c "$CPUS" $R status316 3.16.0a1
  for b in 3.15.1 3.15.1t 3.14.9 3.14.9t; do taskset -c "$CPUS" $R conformance $b; done
  echo "TEXTFIX STAGE1 DONE"
else
  $R startup 3.15.1 "$SU"; $R startup 3.14.9 "$SU"
  echo "TEXTFIX STARTUP DONE"
fi
