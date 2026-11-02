#!/usr/bin/env bash
# The tables of docs/atlas/w2-phase0.md, generated on the box after the schedule (nothing else
# running): wave 1's analyze_phase3.py (workloads, stats, whole, startup, conformance, junit,
# ppcompare, sbcompare) and this run's analyze_w2phase0.py, against wave 1's Phase 3 raw data
# in the clone ($SRC/benches/results/phase3). Output: $RES/tables/*.md.
set -uo pipefail
BASE=${BASE:-/work/w2p0}
RES=${RES:-$BASE/results}
SRC=$BASE/src
P3=$SRC/benches/results/phase3
T=$RES/tables
PY=$BASE/venv/3.15.1/bin/python          # has pyperf and rustique
A3=$P3/analyze_phase3.py
AW=$BASE/bin/analyze_w2phase0.py
mkdir -p "$T"
tmp=$(mktemp -d)
# Inputs from a decompressed copy of the results (the schedule's `pack` gzips files > 1 MB).
IN=$tmp/res; mkdir -p "$IN"; cp -a "$RES/." "$IN/"; find "$IN" -name '*.gz' -exec gunzip -f {} +
gz() { # gz <subdir> <file>...: decompressed copies of .gz files under $tmp/<subdir> (paths printed)
    local d=$tmp/$1; shift; mkdir -p "$d"
    for f in "$@"; do case $f in *.gz) zcat "$f" >"$d/$(basename "${f%.gz}")"; echo "$d/$(basename "${f%.gz}")" ;; *) echo "$f" ;; esac; done; }
pick() { for f in "$@"; do [[ -e $f ]] && echo "$f"; done; }

$PY $A3 junit $(ls "$IN"/tests/pytest-*.xml* | sort -V) >"$T/pytest.md" 2>&1
$PY $A3 conformance $(pick "$IN"/conformance/all-groups-{3.15.1,3.15.1t,3.14.9,3.15.2,3.14.10}.json) >"$T/conformance.md" 2>&1
$PY $A3 workloads $(pick "$IN"/workloads/compare-3.15.1-{data,devtools,web}.json "$IN"/workloads/compare-3.15.1t-{data,devtools,web}.json) >"$T/workloads.md" 2>&1
$PY $AW --res "$IN" --p3 "$P3" wlcompare >"$T/wlcompare.md" 2>&1
$PY $A3 stats $(pick "$IN"/workloads/stats-run-3.15.1 "$IN"/workloads/stats-run-3.15.1t) --top 3 >"$T/stats-run.md" 2>&1
$PY $A3 whole $(pick "$IN"/whole/whole-3.15.1.json "$IN"/whole/whole-3.15.1t.json) >"$T/whole.md" 2>&1
$PY $A3 whole "$P3"/whole/whole-3.15.1.json "$P3"/whole/whole-3.15.1t.json >"$T/whole-phase3.md" 2>&1
$PY $A3 startup $(pick "$IN"/startup/cells-{3.15.1,3.14.9}-{bare,pth}.json) >"$T/startup.md" 2>&1
$PY $A3 ppcompare --new "$IN/pyperformance" --old "$P3/pyperformance" 3.15.1 3.15.1t >"$T/pyperformance.md" 2>&1
for b in 3.15.1 3.15.1t; do
    new=$(gz new $(pick "$IN/scoreboard/instr-enabled-$b.json" "$IN/scoreboard/instr-enabled-$b.json.gz" "$IN/scoreboard/time-enabled-$b.json".shard*.json "$IN/scoreboard/time-enabled-$b.json".shard*.json.gz))
    old=$(gz old $(pick "$P3/scoreboard/instr-enabled-$b.json.gz" "$P3/scoreboard/time-enabled-$b.json".shard*.json.gz))
    [[ -n $new ]] && (cd "$BASE/w/$b" && "$BASE/venv/$b/bin/python" "$A3" sbcompare --repo . --old $old --new $new) >"$T/sbcompare-$b.md" 2>&1
    rm -rf "${tmp:?}"/new "${tmp:?}"/old
done
for c in ftpenalty gc jit status durations; do $PY $AW --res "$IN" --p3 "$P3" $c >"$T/$c.md" 2>&1; done
rm -rf "$tmp"
ls -la "$T"
