#!/usr/bin/env bash
# Conformance + target tests of the utils groups on the current build (run on the dev box
# through `rbox utils run --py B -- bash benches/targets/utils_matrix.sh <outdir> [groups]`).
set -u
out=${1:-/tmp/utils-matrix}; shift || true
groups=${*:-random textwrap urllib statistics}
mkdir -p "$out"
b=$(python -c 'import sys,sysconfig; print(sys.version.split()[0] + ("t" if sysconfig.get_config_var("Py_GIL_DISABLED") else ""))')
declare -A MODS=([random]=test.test_random [textwrap]=test.test_textwrap [urllib]=test.test_urlparse,test.test_urllib [statistics]=test.test_statistics [ipaddress]=test.test_ipaddress)
for g in $groups; do
    python tests/conformance/run_target_suites.py --targets "$g" --modules "${MODS[$g]}" --json "$out/conf_${g}_$b.json" > "$out/conf_${g}_$b.txt" 2>&1
    echo "conformance $g $b: exit $?" >> "$out/summary_$b.txt"
done
python -m pytest -q -p no:cacheprovider tests/targets > "$out/pytest_targets_$b.txt" 2>&1
echo "pytest targets $b: exit $? $(tail -1 "$out/pytest_targets_$b.txt")" >> "$out/summary_$b.txt"
if [ -n "${FULL:-}" ]; then
    python -m pytest -q -p no:cacheprovider > "$out/pytest_full_$b.txt" 2>&1
    echo "pytest full $b: exit $? $(tail -1 "$out/pytest_full_$b.txt")" >> "$out/summary_$b.txt"
fi
