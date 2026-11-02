#!/usr/bin/env bash
# Venvs for validating `python -m rustique estimate` on the 65 workloads (benches/dx/README in
# docs/targets/dx.md §5): one venv per workload family (their pins conflict), each with the
# family's requirements and a Rustique wheel of this checkout.
#
#   benches/dx/est_setup.sh ROOT PY [WHEEL]     e.g. est_setup.sh /work/w2ws-dx/est 3.15.1
#
# Without WHEEL, builds a release wheel of this checkout (maturin build --release) into
# ROOT/wheels first. Run from the source tree (rbox: `rbox <ws> run -- bash benches/dx/est_setup.sh ...`).
set -euo pipefail
root=$1; py=$2; wheel=${3:-}
here=$(cd "$(dirname "$0")" && pwd)
src=$(cd "$here/../.." && pwd)
mkdir -p "$root/wheels"
if [ -z "$wheel" ]; then
    case $py in *t) req="cpython-${py%t}+freethreaded-linux-x86_64-gnu";; *) req="cpython-$py-linux-x86_64-gnu";; esac
    interp=$(uv python find "$req")
    (cd "$src" && maturin build --release -i "$interp" -o "$root/wheels/$py" >/dev/null)
    wheel=$(ls -t "$root/wheels/$py"/rustique-*.whl | head -1)
fi
echo "wheel: $wheel"
for fam in web data devtools frameworks libs custom services threads; do
    bash "$src/benches/atlas/setup_venvs.sh" "$root/venvs" "$py" "$fam" "$src/benches/workloads"
    uv pip install -q --python "$root/venvs/$fam-$py/bin/python" --reinstall --no-deps "$wheel"
done
