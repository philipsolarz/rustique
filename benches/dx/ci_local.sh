#!/usr/bin/env bash
# The Linux x86_64 cell of .github/workflows/wheels.yml, run locally (dev box): a release wheel
# of this checkout, the repack to a top-level `_rustique`, the layout check, the smoke test and
# the platform job's coverage/tests/conformance (docs/targets/dx.md §6).
#
#   benches/dx/ci_local.sh OUT PY [CPYTHON_SRC]     e.g. ci_local.sh /work/w2ws-dx/ci 3.15.1 /work/cpython/v3.15.1
set -uo pipefail
out=$1; py=$2; cpy=${3:-}
src=$(cd "$(dirname "$0")/../.." && pwd)
case $py in *t) req="cpython-${py%t}+freethreaded-linux-x86_64-gnu";; *) req="cpython-$py-linux-x86_64-gnu";; esac
interp=$(uv python find "$req")
mkdir -p "$out/dist-$py"
rm -f "$out/dist-$py"/*.whl
step() { echo; echo "=== $*"; }
step "maturin build --release ($py)"
(cd "$src" && maturin build --release -i "$interp" -o "$out/dist-$py") 2>&1 | tail -2
step "repack + layout"
"$interp" "$src/tools/repack_wheel.py" "$out/dist-$py"/*.whl
"$interp" "$src/.github/scripts/check_wheels.py" --layout top "$out/dist-$py"/*.whl; echo "layout exit $?"
step "smoke"
bash "$src/.github/scripts/smoke_all.sh" "$out/dist-$py" "$interp"; echo "smoke exit $?"
step "platform job"
args=(--dist "$out/dist-$py" --python "$interp" --out "$out/coverage" --name linux-x86_64)
if [ -n "$cpy" ]; then args+=(--cpython-src "$cpy"); else args+=(--no-conformance); fi
"$interp" "$src/.github/scripts/platform_tests.py" "${args[@]}"; echo "platform exit $?"
step "platform report"
"$interp" "$src/.github/scripts/platform_report.py" "$out/coverage" "$src/.github/platform-floors.json"; echo "report exit $?"
