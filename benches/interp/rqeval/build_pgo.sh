#!/usr/bin/env bash
# PGO build of an _rqeval variant (benches/interp/rqeval): instrumented build, training run
# with the copy installed as the eval-frame function, merge, optimized build.
#   bash benches/interp/rqeval/build_pgo.sh <modname> <overrides.c> [extra cflags...]
set -euo pipefail
mod=${1:?}; ovr=${2:?}; shift 2
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${OUT:-/work/w2-interp/rqeval}
P=$OUT/pgo-$mod
mkdir -p "$P"
find "$P" -name '*.profraw' -delete
bash "$HERE/build.sh" "${mod}_gen" "$ovr" "$@" -fprofile-instr-generate
# the link step needs the profile runtime too
G=$OUT/${mod}_gen
SUFFIX=$(python -c 'import sysconfig; print(sysconfig.get_config_var("EXT_SUFFIX"))')
clang-20 -shared -fuse-ld=lld-20 -fprofile-instr-generate -o "$OUT/${mod}_gen$SUFFIX" "$G/${mod}_gen.o"
EXT=${EXT:-/work/w2-interp/ext}
LLVM_PROFILE_FILE="$P/%p.profraw" python "$HERE/pgo_train.py" "${mod}_gen" \
    "$HERE/../../workloads/web/flask_jinja.py" "$HERE/../../workloads/devtools/sqlglot_transpile.py" \
    "$EXT/custom/order_domain.py" "$HERE/../../workloads/web/django_blog.py"
llvm-profdata-20 merge -o "$P/merged.profdata" "$P"/*.profraw
bash "$HERE/build.sh" "$mod" "$ovr" "$@" -fprofile-instr-use="$P/merged.profdata"
