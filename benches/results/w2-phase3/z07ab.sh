#!/usr/bin/env bash
# z07ab.sh: the scoreboard's str sort cells, 7fec7e4 (Phase 3's tree /work/w2p3f) against 07786e2
# (/work/w2p3z), 3.15.1, alone on CPU 10 (domain 1), A B A B; -p 10 -n 5. Why the random-str cells
# measured 1.5-2x faster enabled in z07 than in Phase 3 although 2e02b1b leaves random input alone.
RES=/work/w2p3/results/z07/sort
for rep in 1 2; do for t in w2p3f w2p3z; do
    echo "[$(date -u +%FT%TZ)] START str-ab-$t-$rep" >> /work/w2p3/results/z07/steps.log
    (cd /work/$t/w/3.15.1 && taskset -c 10 /work/$t/venv/3.15.1/bin/python benches/scoreboard.py time --type list --op "sorted(x)" \
        --storage str --size 1e4,1e6 --shards 1 --affinity 10 -p 10 -n 5 -o $RES/str-ab-$t-$rep.json) > $RES/str-ab-$t-$rep.log 2>&1
done; done
echo "[$(date -u +%FT%TZ)] END   str-ab" >> /work/w2p3/results/z07/steps.log
