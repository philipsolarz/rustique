#!/usr/bin/env bash
# Quiet-box run of the gc tier (docs/targets/gc.md; method: docs/atlas/w2-runtime.md §4-§5).
#
#   WL_VENVS=/path/to/wl  OUT=/path/out  [WHEELS=/path/wheels]  benches/runtime/run_quiet.sh [stage...]
#
# WHEELS/<build>/*.whl (release wheels of this branch): installed into every family venv by
# the `venvs` stage. Policies (benches/runtime/gcpolicy.py): `default` (plain CPython) and
# `tier` (Rustique's gc tier alone); the `e2e` stage compares `rustique` (Rustique's default
# targets) with `tier:all=1` (the same plus the gc tier) on the workloads whose GC is > 10%.
#
# WL_VENVS holds one venv per build and family: $WL_VENVS/<build>-<family>/bin/python
# (as benches/results/phase3/run_phase3.sh lays them out). EXTRA_ROOT (optional) is a
# workloads root with the wave-2 families (custom, frameworks, libs, services, threads).
# Each build runs in its own L3 domain, one timing job at a time (one core per domain).
#
# Smoke test: SEL_SET=data/billing_engine SEL_EXTRA=custom/rules_engine SEL_SERVICES=services/job_worker
#   P=1 N=1 ADV_P=1 ADV_SCEN=burst SVC_N=2 run_quiet.sh set extra adv services
# stages: venvs (create missing family venvs with uv; no timing), set (34 workloads),
#         extra (wave-2 families), adv (adversarial programs), services (long runs, -n 40).
#         Default: all, in that order.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
WL_VENVS=${WL_VENVS:?set WL_VENVS}
OUT=${OUT:?set OUT}
EXTRA_ROOT=${EXTRA_ROOT:-$HERE/../workloads}
POLICIES=${POLICIES:-"default;tier"}
WHEELS=${WHEELS:-}
CORE_GIL=${CORE_GIL:-0-7}    # one L3 domain per build, one job at a time (threads workloads use it all)
CORE_FT=${CORE_FT:-8-15}
P=${P:-5}
N=${N:-5}
ADV_P=${ADV_P:-3}
SEL_SET=${SEL_SET:-"web data devtools"}
SEL_EXTRA=${SEL_EXTRA:-"custom frameworks libs threads"}
SEL_SERVICES=${SEL_SERVICES:-services}
SEL_E2E=${SEL_E2E:-"data/billing_engine devtools/pylint_stdlib devtools/pyflakes_stdlib devtools/black_format data/xml_feeds devtools/coverage_pyflakes_tests data/state_snapshot libs/bs4_scrape frameworks/strawberry_graphql threads/request_pool"}
stages=${*:-venvs set extra adv services e2e}
mkdir -p "$OUT"

py() { echo "$1=$WL_VENVS/$1-{family}/bin/python"; }

sweep() { # sweep <build> <core> <outdir> <selectors...> (extra env passed through)
    local b=$1 core=$2 out=$3; shift 3
    python3 "$HERE/gcprobe.py" sweep "$@" --pythons "$(py "$b")" --policies "$POLICIES" \
        -p "$P" -n "$N" --cores "$core" --out "$out" --keep-cols
}

for st in $stages; do
    echo "== $st $(date -u +%FT%TZ)"
    case $st in
        venvs)
            export PATH=$HOME/.cargo/bin:$HOME/.local/bin:$PATH
            for b in 3.15.1 3.15.1t; do
                case $b in *t) req="cpython-3.15.1+freethreaded-linux-x86_64-gnu";; *) req="cpython-3.15.1-linux-x86_64-gnu";; esac
                for f in web data devtools custom frameworks libs services threads; do
                    v=$WL_VENVS/$b-$f
                    if [ ! -x "$v/bin/python" ]; then
                        r=$HERE/../workloads/$f/requirements.txt
                        [ -f "$r" ] || r=$EXTRA_ROOT/$f/requirements.txt
                        [ -f "$r" ] || continue
                        extra=(); grep -q -- "--no-deps" "$(dirname "$r")/README.md" 2>/dev/null && extra=(--no-deps)
                        uv venv -q --python "$req" "$v" && uv pip install -q --python "$v/bin/python" "${extra[@]}" -r "$r" \
                            || echo "VENV FAILED $b $f"
                    fi
                    if [ -n "$WHEELS" ] && ls "$WHEELS/$b"/*.whl >/dev/null 2>&1; then
                        uv pip install -q --python "$v/bin/python" --reinstall --no-deps "$WHEELS/$b"/*.whl || echo "WHEEL FAILED $b $f"
                    fi
                done
            done ;;
        set)
            sweep 3.15.1 "$CORE_GIL" "$OUT/set" $SEL_SET > "$OUT/set-3.15.1.log" 2>&1 &
            sweep 3.15.1t "$CORE_FT" "$OUT/set" $SEL_SET > "$OUT/set-3.15.1t.log" 2>&1 &
            wait ;;
        extra)
            [ -n "$EXTRA_ROOT" ] || { echo "no EXTRA_ROOT"; continue; }
            RQ_WL_ROOT=$EXTRA_ROOT sweep 3.15.1 "$CORE_GIL" "$OUT/extra" $SEL_EXTRA > "$OUT/extra-3.15.1.log" 2>&1 &
            RQ_WL_ROOT=$EXTRA_ROOT sweep 3.15.1t "$CORE_FT" "$OUT/extra" $SEL_EXTRA > "$OUT/extra-3.15.1t.log" 2>&1 &
            wait ;;
        adv)
            for sc in ${ADV_SCEN:-late_cycles burst steady}; do
                for b in 3.15.1 3.15.1t; do
                    core=$CORE_GIL; [ "$b" = 3.15.1t ] && core=$CORE_FT
                    RQ_ADV=$sc python3 "$HERE/gcprobe.py" sweep "$HERE/adversarial.py" \
                        --pythons "$b=$WL_VENVS/$b-data/bin/python" --policies "$POLICIES;tier:cap=0" \
                        -p "$ADV_P" -n 4 --cores "$core" --out "$OUT/adv-$sc" --keep-cols > "$OUT/adv-$sc-$b.log" 2>&1 &
                done
                wait
            done ;;
        services)
            [ -n "$EXTRA_ROOT" ] || { echo "no EXTRA_ROOT"; continue; }
            RQ_WL_ROOT=$EXTRA_ROOT N=${SVC_N:-40} P=2 sweep 3.15.1 "$CORE_GIL" "$OUT/services" $SEL_SERVICES > "$OUT/services-3.15.1.log" 2>&1 &
            RQ_WL_ROOT=$EXTRA_ROOT N=${SVC_N:-40} P=2 sweep 3.15.1t "$CORE_FT" "$OUT/services" $SEL_SERVICES > "$OUT/services-3.15.1t.log" 2>&1 &
            wait ;;
        e2e)
            POLICIES="rustique;tier:all=1" sweep 3.15.1 "$CORE_GIL" "$OUT/e2e" $SEL_E2E > "$OUT/e2e-3.15.1.log" 2>&1 &
            POLICIES="rustique;tier:all=1" sweep 3.15.1t "$CORE_FT" "$OUT/e2e" $SEL_E2E > "$OUT/e2e-3.15.1t.log" 2>&1 &
            wait ;;
    esac
done
echo "== done $(date -u +%FT%TZ)"
