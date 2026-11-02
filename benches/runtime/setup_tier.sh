#!/usr/bin/env bash
# Dev/bench box: wheels of this checkout and workload venvs with the gc tier installed.
#
#   benches/runtime/setup_tier.sh <root> <build>...      e.g. setup_tier.sh /work/w2ws-gc 3.15.1 3.15.1t
#
# Per build: a wheel (PROFILE, default dev-release; `release` for wall-time runs) in
# <root>/dist-<build>, and one venv per workload family in <root>/wl/<build>-<family> (family
# pins conflict; frameworks/libs/services pins are complete: --no-deps) with the wheel
# installed. gcprobe.py then runs with --pythons "<build>=<root>/wl/<build>-{family}/bin/python".
set -euo pipefail
root=${1:?root}; shift
src=$(cd "$(dirname "$0")/../.." && pwd)
export PATH=$HOME/.cargo/bin:$HOME/.local/bin:$PATH CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
for b in "$@"; do
    case $b in *t) req="cpython-${b%t}+freethreaded-linux-x86_64-gnu";; *) req="cpython-$b-linux-x86_64-gnu";; esac
    py=$(uv python find "$req")
    out=$root/dist-$b
    rm -rf "$out"; mkdir -p "$out"
    tools=$root/tools-$b
    [ -x "$tools/bin/maturin" ] || { uv venv -q --python "$req" "$tools"; uv pip install -q --python "$tools/bin/python" maturin==1.15.0; }
    (cd "$src" && CARGO_TARGET_DIR=$root/target-wheel-$b "$tools/bin/maturin" build -q \
        --profile "${PROFILE:-dev-release}" -i "$py" -o "$out" 2>&1 | tail -3)
    whl=$(ls "$out"/*.whl | head -1)
    for fam in web data devtools custom frameworks libs services threads; do
        V=$root/wl/$b-$fam
        [ -x "$V/bin/python" ] || uv venv -q --python "$req" "$V"
        extra=()
        grep -q -- "--no-deps" "$src/benches/workloads/$fam/README.md" 2>/dev/null && extra=(--no-deps)
        if grep -qv '^\s*\(#\|$\)' "$src/benches/workloads/$fam/requirements.txt"; then
            uv pip install -q --python "$V/bin/python" "${extra[@]}" -r "$src/benches/workloads/$fam/requirements.txt" \
                || echo "VENV FAILED $b $fam"
        fi
        uv pip install -q --python "$V/bin/python" --reinstall --no-deps "$whl"
        echo "$V: $("$V/bin/python" -c 'import rustique, sys; print(sys.version.split()[0], rustique.__version__)')"
    done
done
