#!/usr/bin/env bash
# quiet_driver.sh <host> [stages...]: deploy benches/runtime + workloads + wave-2 families to the
# bench box, run benches/runtime/run_quiet.sh there under nohup, and pull results back every
# 2 minutes until it finishes. Results: ~/rustique-research/w2/runtime/quiet/
set -uo pipefail
HOST=${1:?host}; shift
KEY=~/.ssh/rustique-bench
SSH=(ssh -i $KEY -o ConnectTimeout=15 $HOST)
WT=/home/cairon/git/philipsolarz/rustique-wt/w2-runtime
R=/work/w2-runtime-quiet           # on the bench box
LOCAL=~/rustique-research/w2/runtime/quiet
mkdir -p $LOCAL
"${SSH[@]}" "mkdir -p $R/src/benches $R/extra $R/out" < /dev/null
rsync -a --delete -e "ssh -i $KEY" --exclude __pycache__ $WT/benches/runtime $WT/benches/workloads $HOST:$R/src/benches/
rsync -a --delete -e "ssh -i $KEY" --exclude __pycache__ ~/rustique-research/w2/runtime/extra/ $HOST:$R/extra/
"${SSH[@]}" "cd $R && WL_VENVS=\${WL_VENVS:-$R/wl} OUT=$R/out EXTRA_ROOT=$R/extra setsid nohup src/benches/runtime/run_quiet.sh $* > $R/run.log 2>&1 < /dev/null & disown; echo started" < /dev/null
while true; do
  sleep 120
  rsync -a -e "ssh -i $KEY" $HOST:$R/out/ $HOST:$R/run.log $LOCAL/ 2>/dev/null
  tail -n 1 $LOCAL/run.log 2>/dev/null
  grep -q "^== done" $LOCAL/run.log 2>/dev/null && break
done
echo "quiet run finished; results in $LOCAL"
