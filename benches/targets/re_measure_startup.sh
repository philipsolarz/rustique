#!/usr/bin/env bash
# The re group's startup measurements on the dev box (3.15.1): install cost (venv with the
# editable release build) and whole-process imports (devtools venv, repacked release wheel).
#   bash benches/targets/re_measure_startup.sh <outdir> <tag>
set -uo pipefail
OUT=${1:-/work/w2ws-re/results}; TAG=${2:-final}; mkdir -p "$OUT"
DV=/work/w2ws-re/venvs/devtools-3.15.1/bin/python
WH=/work/w2ws-re/wheels-$TAG-$(date +%s)
mkdir -p "$WH"
python benches/targets/re_install.py > "$OUT/re-install-$TAG.log" 2>&1
maturin build --release -i "$DV" -o "$WH" > "$OUT/wheel-$TAG.log" 2>&1
python tools/repack_wheel.py "$WH"/*.whl >> "$OUT/wheel-$TAG.log" 2>&1
uv pip install -q --python "$DV" --force-reinstall --no-deps "$WH"/*.whl >> "$OUT/wheel-$TAG.log" 2>&1
mkdir -p /work/w2ws-re/impwd
python benches/targets/re_imports.py --python "$DV" --workdir /work/w2ws-re/impwd -j 4 \
    -o "$OUT/re-imports-$TAG.json" > "$OUT/re-imports-$TAG.log" 2>&1
echo done >> "$OUT/re-imports-$TAG.log"
