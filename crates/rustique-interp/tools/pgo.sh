#!/usr/bin/env bash
# Train the PGO profile of the handler sets for one configuration (run in the repository root,
# in the build venv, on the dev box):
#   bash crates/rustique-interp/tools/pgo.sh gil|t [llvm-suffix, default -20]
# 1. build the extension with instrumented handler sets (RUSTIQUE_INTERP_PGO=gen);
# 2. run benches/interp/pgo_train.py with the tier on (workloads + micro cells);
# 3. merge into crates/rustique-interp/gen/pgo-<cfg>.proftext (text: readable by any LLVM >= 19);
# 4. rebuild normally (build.rs then compiles with -fprofile-instr-use).
set -euo pipefail
cfg=${1:?gil|t}; sfx=${2:--20}
P=${PGO_DIR:-/tmp/rq-pgo-$cfg}
rm -rf "$P"; mkdir -p "$P"
RUSTIQUE_INTERP_PGO=gen RUSTIQUE_INTERP_CC=clang$sfx maturin develop --uv --profile dev-release 2>&1 | tail -2
LLVM_PROFILE_FILE="$P/%p.profraw" python benches/interp/pgo_train.py
llvm-profdata$sfx merge -o "$P/merged.profdata" "$P"/*.profraw
llvm-profdata$sfx merge --text -o "crates/rustique-interp/gen/pgo-$cfg.proftext" "$P/merged.profdata"
ls -la "crates/rustique-interp/gen/pgo-$cfg.proftext"
RUSTIQUE_INTERP_CC=clang$sfx maturin develop --uv --profile dev-release 2>&1 | tail -2
