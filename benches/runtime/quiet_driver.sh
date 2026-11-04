#!/usr/bin/env bash
# quiet_driver.sh <bench-host> [stages...]: the gc tier's quiet-box run (docs/targets/gc.md §8).
#
#   1. release wheels of this checkout for 3.15.1 and 3.15.1t, built on the dev box through
#      rbox (SKIP_WHEELS=1 reuses $LOCAL/wheels);
#   2. deploys benches/runtime, benches/workloads and the wheels to the bench box;
#   3. runs benches/runtime/run_quiet.sh there under nohup (stages: venvs set extra adv
#      services e2e; default all), venvs in $R/wl with the wheel installed;
#   4. pulls results every 2 minutes into $LOCAL until "== done".
# Tables afterwards: gctable.py scoreboard $LOCAL/out/set --python 3.15.1 --base default, ...
set -uo pipefail
HOST=${1:?bench host (user@ip)}; shift
KEY=~/.ssh/rustique-bench
SSH=(ssh -i "$KEY" -o ConnectTimeout=15 "$HOST")
WT=$(cd "$(dirname "$0")/../.." && pwd)
WS=$(basename "$WT")
RBOX=${RBOX:-$HOME/rustique-research/w2/devbox/rbox}
R=${R:-/work/w2-gc-quiet}                     # on the bench box
LOCAL=${LOCAL:-$HOME/rustique-research/w2/ws-gc/quiet}
mkdir -p "$LOCAL/wheels"
if [ -z "${SKIP_WHEELS:-}" ]; then
    for b in 3.15.1 3.15.1t; do
        "$RBOX" "$WS" run --py "$b" -- bash -c "rm -rf .qwheels-$b && maturin build --release -q -o .qwheels-$b" \
            || { echo "wheel build failed for $b"; exit 1; }
        rm -rf "$LOCAL/wheels/$b"
        "$RBOX" "$WS" pull ".qwheels-$b/" "$LOCAL/wheels/$b/"
    done
fi
"${SSH[@]}" "mkdir -p $R/src/benches $R/wheels $R/out" < /dev/null
rsync -a --delete -e "ssh -i $KEY" --exclude __pycache__ "$WT/benches/runtime" "$WT/benches/workloads" "$HOST:$R/src/benches/"
rsync -a --delete -e "ssh -i $KEY" "$LOCAL/wheels/" "$HOST:$R/wheels/"
"${SSH[@]}" "cd $R && WL_VENVS=\${WL_VENVS:-$R/wl} OUT=$R/out WHEELS=$R/wheels EXTRA_ROOT=$R/src/benches/workloads \
    setsid nohup src/benches/runtime/run_quiet.sh $* > $R/run.log 2>&1 < /dev/null & disown; echo started" < /dev/null
while true; do
    sleep 120
    rsync -a -e "ssh -i $KEY" "$HOST:$R/out/" "$LOCAL/out/" 2>/dev/null
    rsync -a -e "ssh -i $KEY" "$HOST:$R/run.log" "$LOCAL/" 2>/dev/null
    tail -n 1 "$LOCAL/run.log" 2>/dev/null
    grep -q "^== done" "$LOCAL/run.log" 2>/dev/null && break
done
echo "quiet run finished; results in $LOCAL/out"
