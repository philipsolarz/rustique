#!/usr/bin/env bash
# Build the _rqeval prototype: a generated copy of CPython's interpreter loop as a PEP 523
# eval-frame function (see rqeval_mod.c).
#
#   bash benches/interp/rqeval/build.sh <modname> <overrides.c> [extra cflags...]
#
# Needs: CPYTHON_SRC (the exact source tree of the running release), clang-20, the venv
# python. Output: $OUT/<modname>.cpython-315-x86_64-linux-gnu.so (OUT defaults to
# /work/w2-interp/rqeval). Hidden (non-exported) symbols of the python binary that the loop
# needs are bound with --defsym to their addresses: python-build-standalone's python is a
# non-PIE executable with a full symbol table (a prototype shortcut; a product would resolve
# and verify them at load time, or carry its own copies).
set -euo pipefail
mod=${1:?modname}; ovr=${2:?overrides.c}; shift 2
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${OUT:-/work/w2-interp/rqeval}
CC=${CC:-clang-20}
mkdir -p "$OUT/$mod"
G="$OUT/$mod"
SRC=${CPYTHON_SRC:?}
PY=$(command -v python)
PYBIN=$(readlink -f "$(python -c 'import sys; print(sys.executable)')")
INC=$(python -c 'import sysconfig; print(sysconfig.get_paths()["include"])')
SUFFIX=$(python -c 'import sysconfig; print(sysconfig.get_config_var("EXT_SUFFIX"))')

# 1. generate the loop from the running release's bytecodes.c + overrides
python "$SRC/Tools/cases_generator/tier1_generator.py" -o "$G/rq_cases.c.h" \
    "$SRC/Python/bytecodes.c" "$HERE/$ovr"
python "$SRC/Tools/cases_generator/target_generator.py" -o "$G/rq_targets.h" \
    "$SRC/Python/bytecodes.c" "$HERE/$ovr"

# 2. compile (same flags as the PBS build: -O3, frame pointers, tail-calling via pyconfig.h)
# The core is built with -D_Py_TIER2=3 -D_Py_JIT (PY_CFLAGS_NODIST, not in pyconfig.h):
# take every -D of the running build so the copy compiles the same configuration.
CORE_DEFS=$(python -c 'import sysconfig; print(" ".join(f for f in sysconfig.get_config_var("PY_CFLAGS_NODIST").split() if f.startswith("-D")))')
CFLAGS="$CORE_DEFS -fno-semantic-interposition -O3 -fno-omit-frame-pointer -mno-omit-leaf-frame-pointer -fno-strict-overflow -fPIC -fno-plt \
 -DNDEBUG -DPy_BUILD_CORE_MODULE -DRQ_MODNAME=$mod -DRQ_MODNAME_STR=\"$mod\" -std=gnu11 -w"
$CC $CFLAGS "$@" -I"$G" -I"$HERE" -I"$SRC/Python" -I"$INC" -I"$INC/internal" \
    -I"$INC/internal/mimalloc" -c "$HERE/rqeval_mod.c" -o "$G/$mod.o"

# 3. bind the binary's hidden symbols
nm -D --defined-only "$PYBIN" | awk '{print $3}' | sort -u > "$G/exported.txt"
nm -u "$G/$mod.o" | awk '{print $2}' | sort -u > "$G/undef.txt"
nm "$PYBIN" | awk 'NF==3 {print $3, $1}' | sort -k1,1 -u > "$G/allsyms.txt"
DEFS=()
: > "$G/hidden.txt"
while read -r s; do
    if ! grep -qx "$s" "$G/exported.txt"; then
        a=$(awk -v s="$s" '$1==s {print $2; exit}' "$G/allsyms.txt")
        if [ -n "$a" ]; then DEFS+=("-Wl,--defsym,$s=0x$a"); echo "hidden: $s 0x$a" >> "$G/hidden.txt";
        else echo "unresolved: $s" >> "$G/hidden.txt"; fi
    fi
done < "$G/undef.txt"
$CC -shared -fuse-ld=lld-20 -o "$OUT/$mod$SUFFIX" "$G/$mod.o" "${DEFS[@]}" 2>&1 | grep -v "^$" || true
[ -f "$OUT/$mod$SUFFIX" ] || { echo "link failed"; exit 1; }
echo "built $OUT/$mod$SUFFIX ($(stat -c %s "$OUT/$mod$SUFFIX") bytes); hidden symbols: ${#DEFS[@]}"
grep unresolved "$G/hidden.txt" || true
