#!/usr/bin/env bash
# CPython's full regression suite with an _rqeval variant installed at interpreter startup,
# to compare with a plain run (same harness, same environment).
#   bash benches/interp/rqeval/run_tests.sh <variant|plain> <outdir> [regrtest args...]
# The PBS build ships no Lib/test: a .pth file in the venv's site-packages adds a directory
# holding only a `test` symlink to $CPYTHON_SRC/Lib/test (the stdlib is not shadowed), and
# imports site/rq_autoinstall.py, which installs the variant named by $RQEVAL. A .pth file is
# processed under -I/-E too, so test subprocesses (regrtest workers, `python -I -c ...`)
# inherit both the test package and the variant (the environment is inherited). Every
# process that installed the variant appends its pid to $out/installed.<variant>.
set -uo pipefail
v=${1:?}; out=${2:?}; shift 2
HERE=$(cd "$(dirname "$0")" && pwd)
TP=/work/w2-interp/testpkg
mkdir -p "$TP" "$out"
[ -e "$TP/test" ] || ln -s "$CPYTHON_SRC/Lib/test" "$TP/test"
SP=$(python -c 'import sysconfig; print(sysconfig.get_paths()["purelib"])')
printf '%s\n%s\n' "$TP" \
  "import os, sys; os.environ.get('RQEVAL') and (sys.path.insert(0, '$HERE/site'), __import__('rq_autoinstall'), sys.path.remove('$HERE/site'))" \
  > "$SP/rq_testpkg.pth"
args=${*:-"-j8 -q"}
name=${v//[:,]/_}
rm -f "$out/installed.$name"
if [ "$v" = plain ]; then
  env -u RQEVAL python -m test $args > "$out/$name.log" 2>&1
else
  RQEVAL=$v RQEVAL_MARK="$out/installed.$name" python -m test $args > "$out/$name.log" 2>&1
fi
echo "$v rc=$? installed-in-processes=$( [ -f "$out/installed.$name" ] && wc -l < "$out/installed.$name" || echo 0)"
tail -30 "$out/$name.log"
