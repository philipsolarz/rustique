#!/usr/bin/env bash
# The startup slot for 445de19: after the d40e7b9 startup cells in domain 3, and after the
# 445de19 stage-1 re-run has built its wheels.
until grep -q "END   startup-3.14.9" /work/w2p3/results/steps.log && grep -q "TEXTFIX STAGE1 DONE" /work/w2p3/textfix.log; do sleep 10; done
/work/w2p3/textfix.sh 445de19 x 30 > /work/w2p3/textfix-startup.log 2>&1
