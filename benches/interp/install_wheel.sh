#!/usr/bin/env bash
# Build a wheel of this tree for the active venv's interpreter and install it (non-editable)
# into the measurement venvs (/work/w2ws-interp/venvs/<family>-<py>), so measurements never
# touch the build venv's editable install (which a running regrtest uses).
#   bash benches/interp/install_wheel.sh <py-tag e.g. 3.15.1> [family ...]   (default: all)
set -euo pipefail
py=${1:?py tag}; shift
fams=("$@")
W=/work/w2ws-interp/wheels-$py
rm -rf "$W"; mkdir -p "$W"
maturin build --profile dev-release -i "$(command -v python)" -o "$W" 2>&1 | tail -2
whl=$(ls "$W"/*.whl)
for v in /work/w2ws-interp/venvs/*-"$py"; do
  [ -x "$v/bin/python" ] || continue
  if [ ${#fams[@]} -gt 0 ]; then f=$(basename "$v"); f=${f%-"$py"}; [[ " ${fams[*]} " == *" $f "* ]] || continue; fi
  uv pip install -q --python "$v/bin/python" --no-deps --reinstall "$whl"
done
echo "installed $whl"
