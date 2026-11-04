#!/usr/bin/env bash
# Final 8-thread curves on a quiet machine, end to end (w2-threads addendum).
#
#   PY_GIL=/path/venv-3.15.1/bin/python PY_FT=/path/venv-3.15.1t/bin/python \
#   OUT=/path/out CORES=24-31 benches/ft/quietbox.sh [kernels|workloads|services|all]
#
# Both interpreters must have Rustique installed (maturin develop or the wheel) and run from
# this checkout's root. CORES: one L3 domain of >= 8 cores, nothing else running there
# (check with `ps -eo psr,pcpu,args`). Steps (all = in this order):
#   kernels    kernel_scaling.py plain and --enable on PY_FT (1/2/4/8 threads, wall and CPU
#              time per call), then the comparison table          ~12 min
#   workloads  scaling.py on the threads family, both builds, plain vs enable,
#              1/2/4/8 threads, p=5 n=3, then the tables          ~60 min
#   services   uvicorn_fastapi (sync and async dependency) t vs GIL, 5 processes x 5 runs,
#              when benches/ft/_ext/ holds the services workloads   ~5 min
# Every result lands in $OUT (JSON + .md tables); progress in $OUT/quietbox.log.
# QUICK=1: smoke test of the whole script (1 repeat, 1 and 8 threads, short cells), ~6 min.
set -euo pipefail
: "${PY_GIL:?}" "${PY_FT:?}" "${OUT:?}" "${CORES:=24-31}"
step=${1:-all}
mkdir -p "$OUT"
if [ -n "${QUICK:-}" ]; then
    KS=(-t 1,8 -r 1 --secs 0.02); SC=(-t 1,8 -p 1 -n 1); NSVC=1
else
    KS=(-r 5); SC=(-t 1,2,4,8 -p 5 -n 3); NSVC=5
fi
log() { echo "$(date -u +%H:%M:%SZ) $*" | tee -a "$OUT/quietbox.log"; }
pin() { taskset -c "$CORES" "$@"; }
for p in "$PY_GIL" "$PY_FT"; do
    "$p" -c 'import rustique, sys; rustique.enable(); assert not getattr(sys, "_is_gil_enabled", lambda: False)() or "t" not in sys.abiflags' \
        || { echo "rustique missing or GIL re-enabled under $p" >&2; exit 2; }
done

if [ "$step" = kernels ] || [ "$step" = all ]; then
    log "kernels plain"
    pin "$PY_FT" benches/ft/kernel_scaling.py "${KS[@]}" -o "$OUT/ks-ft-plain.json" > "$OUT/ks-ft-plain.log"
    log "kernels enabled"
    pin "$PY_FT" benches/ft/kernel_scaling.py --enable "${KS[@]}" -o "$OUT/ks-ft-enabled.json" > "$OUT/ks-ft-enabled.log"
    "$PY_FT" benches/ft/kernel_scaling.py table "$OUT/ks-ft-plain.json" "$OUT/ks-ft-enabled.json" > "$OUT/kernels.md"
    log "kernels done"
fi

if [ "$step" = workloads ] || [ "$step" = all ]; then
    for p in "$PY_FT" "$PY_GIL"; do
        b=$("$p" -c 'import sys, sysconfig; v=sys.version_info; print(f"{v.major}.{v.minor}.{v.micro}" + ("t" if sysconfig.get_config_var("Py_GIL_DISABLED") else ""))')
        log "workloads $b"
        pin "$p" benches/ft/scaling.py run threads "${SC[@]}" -o "$OUT/scaling-$b.json" > "$OUT/scaling-$b.log"
    done
    "$PY_FT" benches/ft/scaling.py table "$OUT"/scaling-*.json > "$OUT/workloads.md"
    log "workloads done"
fi

if { [ "$step" = services ] || [ "$step" = all ]; } && [ -f benches/ft/_ext/uvicorn_fastapi.py ]; then
    for p in "$PY_GIL" "$PY_FT"; do
        for w in uvicorn_fastapi uvicorn_fastapi_async; do
            for _ in $(seq "$NSVC"); do
                pin "$p" benches/workloads/run.py one "../ft/_ext/$w" -n 5 >> "$OUT/services-$(basename "$(dirname "$(dirname "$p")")").jsonl"
            done
        done
    done
    log "services done"
fi
log "end"
