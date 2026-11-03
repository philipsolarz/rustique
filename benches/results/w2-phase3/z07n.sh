#!/usr/bin/env bash
# z07n.sh: nearly_pyperf.py on 07786e2, 3.15.1 on CPU 10 (domain 1) and 3.15.1t on CPU 20 (domain 2),
# after the headline left those domains; plain then enabled, -p 10 -n 5.
BASE=/work/w2p3z RES=/work/w2p3/results/z07/sort
one() { local b=$1 cpu=$2
    echo "[$(date -u +%FT%TZ)] START nearly-pyperf-$b (CPU $cpu)" >> /work/w2p3/results/z07/steps.log
    for m in plain on; do
        (cd $BASE/w/$b && taskset -c $cpu $BASE/venv/$b/bin/python /work/w2p3/nearly_pyperf.py --mode $m -p 10 -n 5 \
            --affinity $cpu -o $RES/nearly-$b-$m.json) >> $RES/nearly-$b.log 2>&1
    done
    echo "[$(date -u +%FT%TZ)] END   nearly-pyperf-$b" >> /work/w2p3/results/z07/steps.log; }
one 3.15.1 10 & one 3.15.1t 20 & wait
