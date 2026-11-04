#!/usr/bin/env bash
# The estimator (benches/dx/est_validate.sh STEPS=est) on 7fec7e4 in domain 0, which the threads
# addendum freed at 06:06Z; the schedule's est steps (domain 1, later) are skipped by markers.
RES=/work/w2p3/results/final; B=/work/w2p3f
log() { echo "[$(date -u +%FT%TZ)] $*" | tee -a $RES/steps.log; }
for b in 3.15.1 3.15.1t; do
    mkdir -p $B/est/venvs $RES/est
    for f in web data devtools frameworks libs custom services threads; do ln -sfn $B/wl/$b-$f $B/est/venvs/$f-$b; done
    t0=$(date +%s); log "START est-$b (domain 0, est_d0.sh)"
    (cd $B/w/$b && STEPS=est bash benches/dx/est_validate.sh $B/est $b $RES/est/$b -8) > $RES/logs/est-$b.log 2>&1
    log "END   est-$b rc=$? $(( $(date +%s) - t0 ))s"
done
