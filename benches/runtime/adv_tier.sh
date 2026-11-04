#!/usr/bin/env bash
# Adversarial suite (benches/runtime/adversarial.py) for the gc tier on the dev box, both builds:
#   ADV_POLICIES="default;tier;tier:cap=0" ADV_OUT=/work/w2ws-gc/runs WL=/work/w2ws-gc/wl \
#       bash benches/runtime/adv_tier.sh          (from the source root)
# Tables: python3 benches/runtime/advtable.py $ADV_OUT
set -u
out=${ADV_OUT:-/work/w2ws-gc/runs}; wl=${WL:-/work/w2ws-gc/wl}; mkdir -p "$out"
for sc in late_cycles burst steady; do
    for b in 3.15.1 3.15.1t; do
        core=16,17,18; [ $b = 3.15.1t ] && core=19,20,21
        RQ_ADV=$sc python3 benches/runtime/gcprobe.py sweep benches/runtime/adversarial.py \
            --pythons "$b=$wl/$b-data/bin/python" --policies "${ADV_POLICIES:-default;tier;tier:cap=0}" \
            -p "${ADV_P:-3}" -n 4 --cores $core --out "$out/adv-$sc" --keep-cols > "$out/adv-$sc-$b.log" 2>&1 &
    done
    wait
done
echo DONE > "$out/adv.done"
