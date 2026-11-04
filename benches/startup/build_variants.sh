#!/usr/bin/env bash
# Build the release-wheel variants measured in docs/atlas/w2-startup.md §2 (dev box).
#   benches/startup/build_variants.sh SNAPSHOT_DIR OUT_DIR PY_TAG [variant...]
# SNAPSHOT_DIR is a copy of the source tree (so that rsyncs of the worktree during the builds
# cannot change it); each variant gets its own target dir under OUT_DIR. Variants:
#   lean           default features (engine + opt-in native types)
#   nonative       --no-default-features (the engine alone)
#   relr           lean + `-z pack-relative-relocs` (DT_RELR; needs glibc >= 2.36 at run time)
#   nonative-relr  both
set -euo pipefail
snap=$1; out=$2; py=$3; shift 3
cd "$snap"
for v in "$@"; do
    feats=(); flags=""
    case $v in
        lean) ;;
        nonative) feats=(--no-default-features) ;;
        relr) flags="-C link-arg=-Wl,-z,pack-relative-relocs" ;;
        nonative-relr) feats=(--no-default-features); flags="-C link-arg=-Wl,-z,pack-relative-relocs" ;;
        *) echo "unknown variant $v" >&2; exit 2 ;;
    esac
    echo "== $v $(date +%T)"
    CARGO_TARGET_DIR=$out/target-$v-$py RUSTFLAGS="$flags" maturin build --release "${feats[@]}" \
        --skip-auditwheel -i python -o "$out/wheels/$v-$py" 2>&1 | grep -E "^error|warning: unused|Finished|Built wheel"
done
echo "== done $(date +%T)"
