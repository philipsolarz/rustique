#!/usr/bin/env bash
# The startup cells on release wheels (target T8; docs/atlas/w2-startup.md §1, docs/MIGRATION.md).
#
#   benches/startup/release_cells.sh SRC OUT PY [PY...]
#
# SRC: a source tree (this checkout, or `git archive` of another commit for a "before" run).
# OUT: work directory. PY: an interpreter (path) per build to measure, e.g. a
# python-build-standalone 3.15.1 and 3.14.9. For each PY:
#   1. `maturin build --release` (fat LTO) a wheel from SRC; tools/repack_wheel.py moves the
#      extension to the top level when SRC has it (0.3+);
#   2. four fresh `python -m venv --without-pip` venvs: plain, plain + another .pth,
#      Rustique installed (`uv pip install --no-deps` of the wheel), the same + another .pth;
#   3. benches/dx/startup.py cells for both pairs (bare venv; venv with another .pth) and
#      benches/startup/decompose.py on the bare pair. JSON and text land in OUT.
# Run on the dev box (valgrind, perf). Wall numbers there are indicative (shared box);
# instruction counts are deterministic.
set -euo pipefail
src=$(cd "$1" && pwd); out=$2; shift 2
mkdir -p "$out"; out=$(cd "$out" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
rounds=${ROUNDS:-200}; perf_runs=${PERF_RUNS:-200}
for py in "$@"; do
    tag=$("$py" -c 'import sys; print(f"{sys.version.split()[0]}{sys.abiflags}")')
    echo "== $tag $(date +%T)"
    whl="$out/wheels-$tag"
    rm -rf "$whl"
    (cd "$src" && CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$out/target-$tag} maturin build --release --skip-auditwheel \
        -i "$py" -o "$whl" 2>&1 | grep -E "^error|Finished|Built wheel")
    if [ -f "$src/tools/repack_wheel.py" ]; then "$py" "$src/tools/repack_wheel.py" "$whl"/*.whl; fi
    "$py" -c 'import sys, zipfile; z = zipfile.ZipFile(sys.argv[1])
for i in z.infolist():
    if i.filename.rsplit("/", 1)[-1].startswith("_rustique.") and i.filename.endswith(".so"):
        print(f"extension: {i.filename} {i.file_size:,} bytes")' "$whl"/*.whl
    for v in plain plain-other rq rq-other; do
        d="$out/venv-$tag-$v"; rm -rf "$d"
        "$py" -m venv --without-pip "$d"
        sp=$("$d/bin/python" -c 'import sysconfig; print(sysconfig.get_path("purelib"))')
        case $v in *-other) echo "# stands for any other .pth (an editable install, setuptools, virtualenv)" > "$sp/aaa_other.pth" ;; esac
        case $v in rq*) uv pip install -q --python "$d/bin/python" --no-deps "$whl"/*.whl
                        env -u RUSTIQUE "$d/bin/python" -c 'import rustique' ;; esac  # warm .pyc
    done
    "$py" "$here/../dx/startup.py" cells "$out/venv-$tag-plain" "$out/venv-$tag-rq" -r "$rounds" \
        -o "$out/cells-$tag-bare.json" | tee "$out/cells-$tag-bare.txt"
    "$py" "$here/../dx/startup.py" cells "$out/venv-$tag-plain-other" "$out/venv-$tag-rq-other" -r "$rounds" \
        -o "$out/cells-$tag-other.json" | tee "$out/cells-$tag-other.txt"
    "$py" "$here/decompose.py" "$out/venv-$tag-plain" "$out/venv-$tag-rq" --perf-runs "$perf_runs" \
        --wall-rounds "$rounds" -o "$out/decompose-$tag.json" | tee "$out/decompose-$tag.txt"
done
echo "== done $(date +%T)"
