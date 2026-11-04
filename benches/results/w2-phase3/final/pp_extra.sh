#!/usr/bin/env bash
# pyperformance cells to re-run alone (7fec7e4), in domain 2 while the gc addendum runs in
# domains 0-1: sphinx on 3.15.1t (its plain run collided on the shared data directory with the
# concurrent enabled run), and the two cells above 1.05 in the full runs (unpack_sequence on
# 3.15.1, coroutines on 3.15.1t) on both builds; plain then enabled, then enabled then plain.
export COMMIT=7fec7e4 BASE=/work/w2p3f RES=/work/w2p3/results/final
R=/work/w2p3/run_w2phase3.sh
until grep -q "START gcquiet" $RES/steps.log; do sleep 15; done
PP_BENCHMARKS=sphinx $R pyperformance 3.15.1t plain 16-23 -sphinx
PP_BENCHMARKS=sphinx $R pyperformance 3.15.1t enabled 16-23 -sphinx
for b in 3.15.1 3.15.1t; do
  PP_BENCHMARKS=unpack_sequence,coroutines $R pyperformance $b plain 16-23 -solo1
  PP_BENCHMARKS=unpack_sequence,coroutines $R pyperformance $b enabled 16-23 -solo1
  PP_BENCHMARKS=unpack_sequence,coroutines $R pyperformance $b enabled 16-23 -solo2
  PP_BENCHMARKS=unpack_sequence,coroutines $R pyperformance $b plain 16-23 -solo2
done
echo "[$(date -u +%FT%TZ)] pp_extra done" >> $RES/steps.log
