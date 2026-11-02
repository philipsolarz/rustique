#!/usr/bin/env bash
# CPython's own test modules with and without the `re` group (ACCELERATION §7.1), on the
# build whose venv is active (rbox run --py B). Writes results/re-conformance-<B>.json.
#   bash benches/targets/re_conformance.sh <B> <outdir>
set -uo pipefail
B=$1; OUT=${2:-/work/w2ws-re/results}
mkdir -p "$OUT"
MODS=test.test_re,test.test_email,test.test_tokenize,test.test_configparser,test.test_fnmatch,test.test_textwrap,test.test_http_cookies,test.test_difflib,test.test_string,test.test_shlex,test.test_argparse,test.test_gettext,test.test_glob,test.test_csv,test.test_urllib,test.test_mailbox
EXPECT=re._compiler.compile,re.match,re.fullmatch,re.search,re.sub,re.subn,re.split,re.findall,re.finditer,re.compile
# Run from a directory outside the synced source tree: CPython's tests create temporary
# directories in the working directory, and a concurrent `rbox` sync (rsync --delete) of the
# source tree would remove them mid-test.
SRC=$(pwd)
# Every target validated at install (not armed until its first call): the kernels run from the
# first call of every test module.
export RUSTIQUE_VALIDATE=eager
mkdir -p /work/w2ws-re/conf-cwd-$B && cd /work/w2ws-re/conf-cwd-$B
python "$SRC/tests/conformance/run_target_suites.py" --targets re --modules "$MODS" --expect-installed "$EXPECT" \
    --json "$OUT/re-conformance-$B.json" --timeout 2400 > "$OUT/re-conformance-$B.log" 2>&1
echo "exit $?" >> "$OUT/re-conformance-$B.log"
