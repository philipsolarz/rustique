#!/usr/bin/env bash
# Real open-source test suites with Rustique, the way a user would run them
# (docs/targets/dx.md §7). Re-run at integration with the merged targets.
#
#   benches/dx/real_projects.sh PYTHON WHEEL_DIR WORK_DIR [project...]
#
# For each project (default: attrs rich sqlglot): a fresh venv of PYTHON, `pip install` of the
# rustique wheel from WHEEL_DIR (so the startup .pth is there, as for users), the project at
# its default branch HEAD (shallow clone) with its test dependencies, then:
#   plain    pytest                                   baseline
#   startup  RUSTIQUE=1 pytest                        the zero-code switch
#   plugin   pytest --rustique                        per-target report in the summary
#   diff     python -m rustique diff -- pytest        exit code, output and per-test outcomes
# Logs and timings go to WORK_DIR/results/<project>/; a summary table to WORK_DIR/results/summary.txt.
set -u
PY=$1; WHEELS=$(cd "$2" && pwd); WORK=$3; shift 3
PROJECTS=("${@:-attrs rich sqlglot}")
# shellcheck disable=SC2206
PROJECTS=(${PROJECTS[*]})
mkdir -p "$WORK/src" "$WORK/results"

declare -A REPO=(
    [attrs]=https://github.com/python-attrs/attrs
    [rich]=https://github.com/Textualize/rich
    [sqlglot]=https://github.com/tobymao/sqlglot
    [requests]=https://github.com/psf/requests
    [networkx]=https://github.com/networkx/networkx
)
# Test dependencies (beyond the project itself) and the pytest arguments.
declare -A DEPS=(
    [attrs]="pytest hypothesis pympler cloudpickle"
    [rich]="pytest attrs"
    [sqlglot]="pytest pytz duckdb pandas"
    [requests]="pytest pytest-httpbin pytest-mock trustme werkzeug"
    [networkx]="pytest numpy scipy"
)
declare -A ARGS=(
    [attrs]="tests --ignore=tests/test_mypy.yml -p no:mypy-plugins"
    [rich]="tests"
    [sqlglot]="tests --ignore=tests/test_sqlglotc.py"
    [requests]="tests"
    [networkx]="networkx"
)

timed() { # timed LOG cmd... -> prints "<rc> <seconds>"
    local log=$1; shift
    local t0 t1 rc
    t0=$(date +%s.%N)
    "$@" > "$log" 2>&1
    rc=$?
    t1=$(date +%s.%N)
    echo "$rc $(echo "$t1 - $t0" | bc)"
}

summary="$WORK/results/summary.txt"
printf '%-10s %-8s %4s %9s  %s\n' project mode rc seconds "last line" > "$summary"
for p in "${PROJECTS[@]}"; do
    out="$WORK/results/$p"; mkdir -p "$out"
    src="$WORK/src/$p"
    [ -d "$src/.git" ] || git clone -q --depth 1 "${REPO[$p]}" "$src"
    (cd "$src" && git log -1 --format='%H %cs' > "$out/commit.txt")
    venv="$WORK/venv-$p"
    if [ ! -x "$venv/bin/python" ]; then
        "$PY" -m venv "$venv"
    fi
    "$venv/bin/python" -m pip install -q --disable-pip-version-check -e "$src" ${DEPS[$p]} > "$out/install.log" 2>&1 \
        || echo "$p: some dependencies failed to install (see $out/install.log)"
    "$venv/bin/python" -m pip install -q --disable-pip-version-check --force-reinstall --no-deps --no-index \
        --find-links "$WHEELS" rustique
    "$venv/bin/python" -m rustique status > "$out/status.txt" 2>&1
    cd "$src" || continue
    # shellcheck disable=SC2086
    set -- "$venv/bin/python" -m pytest -q -p no:cacheprovider ${ARGS[$p]}
    env -u RUSTIQUE "$@" > "$out/warmup.log" 2>&1  # compile the .pyc files once
    for mode in plain startup plugin diff; do
        case $mode in
            plain)   r=$(timed "$out/$mode.log" env -u RUSTIQUE "$@") ;;
            startup) r=$(timed "$out/$mode.log" env RUSTIQUE=1 "$@") ;;
            plugin)  r=$(timed "$out/$mode.log" env -u RUSTIQUE "$@" --rustique) ;;
            diff)    r=$(timed "$out/$mode.log" env -u RUSTIQUE "$venv/bin/python" -m rustique diff --keep "$out/diff-runs" -- "$@") ;;
        esac
        last=$(grep -E "passed|failed|identical|DIFFERENT|error" "$out/$mode.log" | tail -1 | cut -c1-90)
        printf '%-10s %-8s %4s %9.1f  %s\n' "$p" "$mode" ${r} "$last" >> "$summary"
    done
    cd - > /dev/null || exit 1
done
cat "$summary"
