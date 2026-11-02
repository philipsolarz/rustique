#!/usr/bin/env bash
# CPython's full regression suite with the interp tier on in every Python process, to compare
# with the same harness without it (the gate of docs/targets/interp.md).
#   bash benches/interp/regrtest/run_tests.sh <plain|noop|tier> <outdir> [regrtest args...]
# The PBS build ships no Lib/test: a .pth file in the venv's site-packages adds a directory that
# holds only a `test` symlink to $CPYTHON_SRC/Lib/test (the stdlib is not shadowed), and loads
# site/rq_tier_autoload.py when RQ_TIER_TEST is set:
#   noop  imports rustique and verifies the binary (the harness's own footprint), swaps nothing;
#   tier  also enables the interp tier (its own targets only).
# A .pth file is processed under -I/-E too, so test subprocesses inherit the variant (the
# environment is inherited); only -S processes run plain. Every process that applied the
# variant appends its pid to $out/installed.<variant>.
set -uo pipefail
v=${1:?plain|noop|tier}; out=${2:?outdir}; shift 2
HERE=$(cd "$(dirname "$0")" && pwd)
TP=${TP:-/work/w2ws-interp/testpkg-$(python -c 'import sys; print(sys.version_info[1], sys.abiflags, sep="")')}
mkdir -p "$TP" "$out"
[ -e "$TP/test" ] || ln -s "$CPYTHON_SRC/Lib/test" "$TP/test"
SP=$(python -c 'import sysconfig; print(sysconfig.get_paths()["purelib"])')
printf '%s\n%s\n' "$TP" \
  "import os, sys; os.environ.get('RQ_TIER_TEST') and (sys.path.insert(0, '$HERE/site'), __import__('rq_tier_autoload'), sys.path.remove('$HERE/site'))" \
  > "$SP/rq_testpkg.pth"
args=${*:-"-j8 -q"}
args="$args --junit-xml $out/$v.xml"
rm -f "$out/installed.$v"
if [ "$v" = plain ]; then
  env -u RQ_TIER_TEST python -m test $args > "$out/$v.log" 2>&1
else
  RQ_TIER_TEST=$v RQ_TIER_MARK="$out/installed.$v" python -m test $args > "$out/$v.log" 2>&1
fi
echo "$v rc=$? processes-with-variant=$( [ -f "$out/installed.$v" ] && wc -l < "$out/installed.$v" || echo 0)"
tail -25 "$out/$v.log"
