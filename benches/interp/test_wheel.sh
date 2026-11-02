#!/usr/bin/env bash
# Build a wheel for the active venv's interpreter, install it into a separate test venv
# (/work/w2ws-interp/venv-test-<py>) and run tests/interp there: lets the tests run while the
# build venv's editable install is in use (regrtest).
#   bash benches/interp/test_wheel.sh <py-tag> [pytest args...]
set -euo pipefail
py=${1:?}; shift
T=/work/w2ws-interp/venv-test-$py
W=/work/w2ws-interp/wheels-test-$py
[ -x "$T/bin/python" ] || { uv venv -q --python "$(command -v python)" "$T"; uv pip install -q --python "$T/bin/python" pytest==9.1.1 hypothesis==6.170.9; }
rm -rf "$W"; mkdir -p "$W"
maturin build --profile dev-release -i "$(command -v python)" -o "$W" 2>&1 | grep -E "error|Built" | tail -2
uv pip install -q --python "$T/bin/python" --no-deps --reinstall "$W"/*.whl
cd /tmp && "$T/bin/python" -m pytest -q -p no:cacheprovider /work/w2ws-interp/src/tests/interp "$@" 2>&1 | tail -15
