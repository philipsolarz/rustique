#!/usr/bin/env bash
# GC baseline (untimed lane, domain 0) as soon as Wave B ends, alongside the port re-runs on domain 1.
until grep -q "WAVE-B done" /work/w2p0/results/steps.log; do sleep 5; done
cd /work/w2p0
taskset -c 0-7 ./run_w2phase0.sh gcstats 3.15.1
taskset -c 0-7 ./run_w2phase0.sh gcstats 3.15.1t
