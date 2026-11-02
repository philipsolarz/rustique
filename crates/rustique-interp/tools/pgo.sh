#!/usr/bin/env bash
# Train the PGO profile of the handler sets for one configuration (run in the repository root
# with the build venv active, on the dev box):
#   bash crates/rustique-interp/tools/pgo.sh gil|t [llvm-suffix, default -20]
# 1. build a wheel with instrumented handler sets (RUSTIQUE_INTERP_PGO=gen) and install it into
#    a training venv of its own (the build venv's editable install is left alone);
# 2. run benches/interp/pgo_train.py there with the tier on (workloads + micro cells);
# 3. merge into crates/rustique-interp/gen/pgo-<cfg>.proftext (text: readable by any LLVM >= 19).
# The next build compiles the sets with -fprofile-instr-use.
set -euo pipefail
cfg=${1:?gil|t}; sfx=${2:--20}
P=${PGO_DIR:-/tmp/rq-pgo-$cfg}
tag=$(python -c 'import sys; print("%d.%d.%d" % sys.version_info[:3] + ("t" if not getattr(sys, "_is_gil_enabled", lambda: True)() else ""))')
T=${PGO_VENV:-/work/w2ws-interp/venv-train-$tag}
W=$P/wheel
rm -rf "$P"; mkdir -p "$P" "$W"
[ -x "$T/bin/python" ] || uv venv -q --python "$(command -v python)" "$T"
RUSTIQUE_INTERP_PGO=gen RUSTIQUE_INTERP_CC=clang$sfx maturin build --profile dev-release -i "$(command -v python)" -o "$W" 2>&1 | tail -1
uv pip install -q --python "$T/bin/python" --no-deps --reinstall "$W"/*.whl
LLVM_PROFILE_FILE="$P/%p.profraw" "$T/bin/python" benches/interp/pgo_train.py
llvm-profdata$sfx merge -o "$P/merged.profdata" "$P"/*.profraw
llvm-profdata$sfx merge --text -o "crates/rustique-interp/gen/pgo-$cfg.proftext" "$P/merged.profdata"
ls -la "crates/rustique-interp/gen/pgo-$cfg.proftext"
