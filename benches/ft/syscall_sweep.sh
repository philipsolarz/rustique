#!/usr/bin/env bash
# Count lock-related syscalls in the steady state of single-threaded workloads.
#
#   benches/ft/syscall_sweep.sh <outfile.csv> [-n N] [--enable] -- <workload>...
#
# A single-threaded program on a free-threaded build should make no `sched_yield` and
# almost no `futex` calls: CPython's PyMutex spins with sched_yield (up to 40 times) and then
# parks on a futex only when a lock is held. Nonzero counts in a one-thread workload mean a
# thread contends with itself (a nested critical section CPython does not recognise as
# recursive, e.g. set.__reduce__ -> list(set) on 3.14t-3.16t) or with a helper thread. Needs
# perf with tracepoint access (sudo on the dev box). Columns: workload, sched_yield, futex,
# context-switches, task-clock ms, run() seconds (median).
set -euo pipefail
out=$1; shift
args=()
while [ $# -gt 0 ] && [ "$1" != -- ]; do args+=("$1"); shift; done
shift
here=$(cd "$(dirname "$0")" && pwd)
py=$(command -v python)
echo "workload,sched_yield,futex,context_switches,task_clock_ms,run_s" > "$out"
for w in "$@"; do
    d=$(mktemp -d); mkfifo "$d/ctl" "$d/ack"; chmod 777 "$d" "$d/ctl" "$d/ack"
    sudo -E perf stat -x, -o "$d/stat" -e syscalls:sys_enter_sched_yield,syscalls:sys_enter_futex,context-switches,task-clock \
        --control "fifo:$d/ctl,$d/ack" -D -1 -- \
        sudo -E -u "$USER" env PATH="$PATH" VIRTUAL_ENV="${VIRTUAL_ENV:-}" "$py" "$here/perfdrive.py" "$w" --ctl "$d/ctl,$d/ack" "${args[@]}" > "$d/times" 2> "$d/err" || { echo "$w FAILED: $(tail -3 "$d/err")" >&2; rm -rf "$d"; continue; }
    sy=$(awk -F, '$3 ~ /sched_yield/ {print $1}' "$d/stat")
    fx=$(awk -F, '$3 ~ /sys_enter_futex/ {print $1}' "$d/stat")
    cs=$(awk -F, '$3 ~ /context-switches/ {print $1}' "$d/stat")
    tc=$(awk -F, '$3 ~ /task-clock/ {print $1}' "$d/stat")
    rs=$(python -c 'import json,sys,statistics; print(round(statistics.median(json.load(open(sys.argv[1]))["times"]),4))' "$d/times")
    echo "$w,$sy,$fx,$cs,$tc,$rs" | tee -a "$out"
    rm -rf "$d"
done
