#!/usr/bin/env bash
# All four builds, sequentially, on the dev box (detached): build, conformance of the utils
# groups, tests/targets and the full test suite. Results in $1 (default /work/utils/matrix2).
out=${1:-/work/utils/matrix2}
mkdir -p "$out"
src=$(pwd)
for py in ${BUILDS:-3.15.1 3.15.1t 3.14.9 3.14.9t}; do
  (
    V=/work/utils/venv-$py
    export CARGO_TARGET_DIR=/work/utils/target-$py CPYTHON_SRC=/work/cpython/v${py%t} VIRTUAL_ENV=$V
    export PATH=$V/bin:$HOME/.cargo/bin:$HOME/.local/bin:$PATH PYO3_PYTHON=$V/bin/python
    export LD_LIBRARY_PATH=$($V/bin/python -c 'import sysconfig; print(sysconfig.get_config_var("LIBDIR"))')
    cd "$src"
    maturin develop --uv --profile dev-release > "$out/build_$py.log" 2>&1
    echo "build $py: exit $?" >> "$out/summary_all.txt"
    FULL=1 bash benches/targets/utils_matrix.sh "$out" random textwrap urllib statistics
  )
done
echo "ALL DONE" >> "$out/summary_all.txt"
