#!/usr/bin/env bash
# Release wheels of this checkout for the real-project runs (docs/targets/dx.md §7), repacked to a
# top-level `_rustique` like wheels.yml, interp handlers required.   rp_wheels.sh OUTDIR PY...
set -euo pipefail
out=$1; shift
src=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$out"
for py in "$@"; do
    case $py in *t) req="cpython-${py%t}+freethreaded-linux-x86_64-gnu";; *) req="cpython-$py-linux-x86_64-gnu";; esac
    interp=$(uv python find "$req")
    (cd "$src" && RUSTIQUE_INTERP_REQUIRE=1 maturin build --release -i "$interp" -o "$out") 2>&1 | grep -E "Built wheel|error|warning: rustique" || true
done
python3 "$src/tools/repack_wheel.py" "$out"/*.whl
python3 "$src/.github/scripts/check_wheels.py" --layout top "$out"/*.whl
