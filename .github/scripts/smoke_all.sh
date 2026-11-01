#!/usr/bin/env bash
# Install the matching wheel from DIST into a fresh venv of each interpreter with pip, and
# run wheel_smoke.py there (outside the source tree).
#
#   smoke_all.sh DIST PY [PY...]      PY: an interpreter command or path
#
# An interpreter that is not available is skipped with a warning; a missing wheel, a failed
# install or a failed check fails the run.
set -u
here=$(cd "$(dirname "$0")" && pwd)
dist=$(cd "$1" && pwd); shift
work=$(mktemp -d)
status=0
for py in "$@"; do
    if ! command -v "$py" >/dev/null 2>&1; then
        echo "::warning::smoke: interpreter $py not available, skipped"
        continue
    fi
    tag=$("$py" -c 'import sys; print(f"cp{sys.version_info[0]}{sys.version_info[1]}" + ("t" if not sys._is_gil_enabled() else ""))')
    echo "=== $tag ($("$py" -c 'import sys; print(sys.version.split()[0], sys.platform)'))"
    venv="$work/venv-$tag"
    "$py" -m venv "$venv" || { echo "::error::smoke $tag: venv failed"; status=1; continue; }
    vpy="$venv/bin/python"
    [ -x "$vpy" ] || vpy="$venv/Scripts/python.exe"
    if ! "$vpy" -m pip install --disable-pip-version-check -q --no-index --find-links "$dist" rustique; then
        echo "::error::smoke $tag: pip install of the wheel failed"
        status=1
        continue
    fi
    (cd "$work" && "$vpy" "$here/wheel_smoke.py" --expect-installed) || { echo "::error::smoke $tag failed"; status=1; }
done
exit $status
