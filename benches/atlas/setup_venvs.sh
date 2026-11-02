#!/usr/bin/env bash
# Create one venv per workload family and interpreter on the dev box (family pins conflict).
#   benches/atlas/setup_venvs.sh <root> <py-build> <family> [<workloads-dir>]
# e.g. setup_venvs.sh /work/w2-attribution/venvs 3.15.1t web
set -euo pipefail
root=$1; py=$2; fam=$3; wdir=${4:-$(cd "$(dirname "$0")/../workloads" && pwd)}
case $py in *t) req="cpython-${py%t}+freethreaded-linux-x86_64-gnu";; *) req="cpython-$py-linux-x86_64-gnu";; esac
V=$root/$fam-$py
[ -x "$V/bin/python" ] || uv venv -q --python "$req" "$V"
extra=()
if [ -n "${NO_DEPS:-}" ] || grep -q -- "--no-deps" "$wdir/$fam/README.md" 2>/dev/null; then extra=(--no-deps); fi
if grep -qv '^\s*\(#\|$\)' "$wdir/$fam/requirements.txt"; then
  uv pip install -q --python "$V/bin/python" "${extra[@]}" -r "$wdir/$fam/requirements.txt"
fi
echo "$V ok: $("$V/bin/python" -c 'import sys; print(sys.version.split()[0])')"
