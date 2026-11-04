#!/usr/bin/env bash
# Re-measure kernel_scaling's "Pattern.match (compiled)" cell (not a Rustique target) on 3.15.1t,
# alone in domain 3 while the gc addendum runs in domains 0-1: 7fec7e4, 9d0abae, 78751e2, 3 rounds
# of plain then --enable each (-r 5), plus "re.match (module cache)" as a target reference.
OUT=/work/w2p3/results/p78/pm; mkdir -p $OUT
until grep -q "START gcquiet" /work/w2p3/results/final/steps.log; do sleep 15; done
echo "[$(date -u +%FT%TZ)] START pm_probe (domain 3)" >> /work/w2p3/results/p78/steps.log
for round in 1 2 3; do
  for t in w2p3f:7fec7e4 w2p3r:9d0abae w2p3p:78751e2; do
    B=/work/${t%%:*}; c=${t##*:}
    cd $B/w/3.15.1t
    for m in plain enabled; do
      en=(); [[ $m == enabled ]] && en=(--enable)
      taskset -c 24-31 $B/venv/3.15.1t/bin/python benches/ft/kernel_scaling.py "${en[@]}" -r 5 \
        --cells "Pattern.match (compiled)|re.match (module cache)" -o $OUT/ks-$c-$m-$round.json > $OUT/ks-$c-$m-$round.log 2>&1
    done
    $B/venv/3.15.1t/bin/python benches/ft/kernel_scaling.py table $OUT/ks-$c-plain-$round.json $OUT/ks-$c-enabled-$round.json > $OUT/table-$c-$round.md
  done
done
echo "[$(date -u +%FT%TZ)] END   pm_probe" >> /work/w2p3/results/p78/steps.log
