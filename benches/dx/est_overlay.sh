#!/usr/bin/env bash
# Copy this checkout's dx files (python/rustique/{_cli,_diff,_report,_estimate}.py, the plugin) over
# the installed wheel in every venv under ROOT/venvs: iterate on the CLI/estimator without
# rebuilding the extension.   benches/dx/est_overlay.sh ROOT
set -euo pipefail
root=$1
src=$(cd "$(dirname "$0")/../.." && pwd)
for v in "$root"/venvs/*/; do
    sp=$("$v/bin/python" -c 'import sysconfig; print(sysconfig.get_paths()["purelib"])')
    cp "$src"/python/rustique/{_cli,_diff,_report,_estimate,__main__}.py "$sp/rustique/"
    cp "$src"/python/_rustique_pytest.py "$sp/"
done
echo "overlaid $(ls -d "$root"/venvs/*/ | wc -l) venvs"
