#!/usr/bin/env bash
# Profile the steady state of one workload with perf (setup and warm-up excluded).
#
#   benches/ft/perfprof.sh record <workload> <outdir> [-n N] [--enable]   # perf record -g + report
#   benches/ft/perfprof.sh stat   <workload> <outdir> [-n N] [--enable]   # hardware counters
#
# Uses the current interpreter (`python`). Plain `perf record -g` (python-build-standalone
# keeps frame pointers); `PERF_PY=1` adds `-X perf` for Python frame names, but its PEP 523
# trampoline disables inlined Python-to-Python calls and inflates interpreter/call shares
# (w2-interp: up to 1.27-1.43x), so never use it for t-vs-GIL attribution. Output:
# <outdir>/<tag>.data (record), <tag>.self.txt (self-time symbols), <tag>.children.txt,
# <tag>.stat.txt, where tag = <workload with / -> _>-<build>[-enabled].
set -euo pipefail
mode=$1 w=$2 out=$3; shift 3
mkdir -p "$out"
build=$(python -c 'import sys,sysconfig; v=sys.version_info; print(f"{v.major}.{v.minor}.{v.micro}"+("t" if sysconfig.get_config_var("Py_GIL_DISABLED") else ""))')
tag="${w//\//_}-$build"
for x in "$@"; do [ "$x" = --enable ] && tag="$tag-enabled"; done
d=$(mktemp -d); mkfifo "$d/ctl" "$d/ack"
here=$(cd "$(dirname "$0")" && pwd)
case $mode in
  record)
    perf record -F 1999 -g -o "$out/$tag.data" --control "fifo:$d/ctl,$d/ack" -D -1 -- \
        python ${PERF_PY:+-X perf} "$here/perfdrive.py" "$w" --ctl "$d/ctl,$d/ack" "$@" > "$out/$tag.times.json" 2> "$out/$tag.record.log"
    perf report -i "$out/$tag.data" --no-children --sort dso,symbol --percent-limit 0.3 --stdio -g none 2>/dev/null \
        > "$out/$tag.self.txt" || true
    ;;
  stat)
    perf stat -e cycles,instructions,cache-references,cache-misses,branch-misses,L1-dcache-load-misses,task-clock \
        -x, -o "$out/$tag.stat.txt" --control "fifo:$d/ctl,$d/ack" -D -1 -- \
        python "$here/perfdrive.py" "$w" --ctl "$d/ctl,$d/ack" "$@" > "$out/$tag.times.json"
    ;;
esac
rm -rf "$d"
cat "$out/$tag.times.json"
