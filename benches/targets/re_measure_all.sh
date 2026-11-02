#!/usr/bin/env bash
# The re group's workload and startup measurements on the dev box (3.15.1, release build):
# rebuild the devtools-venv wheel, then steady-state workloads and whole-process imports.
#   bash benches/targets/re_measure_all.sh <outdir>
set -uo pipefail
OUT=${1:-/work/w2ws-re/results}; mkdir -p "$OUT"
DV=/work/w2ws-re/venvs/devtools-3.15.1/bin/python
rm -rf /work/w2ws-re/wheels
maturin build --release -i "$DV" -o /work/w2ws-re/wheels > "$OUT/wheel.log" 2>&1
python tools/repack_wheel.py /work/w2ws-re/wheels/*.whl >> "$OUT/wheel.log" 2>&1
uv pip install -q --python "$DV" --force-reinstall --no-deps /work/w2ws-re/wheels/*.whl >> "$OUT/wheel.log" 2>&1
python benches/targets/re_workloads.py web/email_mime data/log_analytics data/text_corpus -r 3 \
    -o "$OUT/re-workloads-stdlib-3.15.1.json" > "$OUT/re-workloads-stdlib-3.15.1.log" 2>&1
"$DV" benches/targets/re_workloads.py devtools/pycodestyle_stdlib devtools/sphinx_autodoc devtools/rich_render -r 3 \
    -o "$OUT/re-workloads-devtools-3.15.1.json" > "$OUT/re-workloads-devtools-3.15.1.log" 2>&1
echo done >> "$OUT/re-workloads-devtools-3.15.1.log"
mkdir -p /work/w2ws-re/impwd
python benches/targets/re_imports.py --python "$DV" --workdir /work/w2ws-re/impwd -j 4 \
    -o "$OUT/re-imports2-3.15.1.json" > "$OUT/re-imports2-3.15.1.log" 2>&1
echo done >> "$OUT/re-imports2-3.15.1.log"
