import os, sys
sys.path.insert(0, os.path.expanduser("~/.cache/rustique-lg-bench"))
import _lg_bench_v1 as h
import rustique, logging
h.start()
rustique.enable(only=['logging'])
h.dump("x")
h.stop()
