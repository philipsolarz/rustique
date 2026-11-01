import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from utils_cg import helper  # noqa
h = helper()
import rustique
g, m = sys.argv[1], sys.argv[2]
for k in [k for k in sys.modules if k == m]:
    del sys.modules[k]
if m in sys.modules:
    h.start()
    rustique.enable(only=[g])
else:
    rustique.enable(only=[g])
    h.start()
    __import__(m)
h.dump("x")
h.stop()
