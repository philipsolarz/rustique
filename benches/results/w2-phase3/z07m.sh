#!/usr/bin/env bash
# z07m.sh: custom/mini_interp on 3.15.1t (07786e2), domain 2 (CPUs 16-23, as the headline), mini_probe.py
# 5 rounds: plain / import / enabled / exclude each half of the 21 default groups; then the halving step ($1 = stage).
cd /work/w2p3z/w/3.15.1t
P=/work/w2p3z/wl/3.15.1t-custom/bin/python OUT=/work/w2p3/results/z07/mini
mkdir -p $OUT
A=builtins,containers,contextlib,copy,dataclasses,datetime,enum,float,fnmatch,functools
B=generated,json,logging,pathlib,posixpath,random,re,set,statistics,textwrap,urllib
stage=${1:-1}; shift
echo "[$(date -u +%FT%TZ)] START mini-probe-$stage" >> /work/w2p3/results/z07/steps.log
if [ "$stage" = 1 ]; then
    taskset -c 16-23 $P /work/w2p3/mini_probe.py custom/mini_interp 5 plain import enabled excl=$A excl=$B > $OUT/mini-3.15.1t-1.txt 2>&1
else
    taskset -c 16-23 $P /work/w2p3/mini_probe.py custom/mini_interp 5 plain enabled "$@" > $OUT/mini-3.15.1t-$stage.txt 2>&1
fi
echo "[$(date -u +%FT%TZ)] END   mini-probe-$stage" >> /work/w2p3/results/z07/steps.log
