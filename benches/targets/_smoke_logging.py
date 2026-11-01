import io, json, logging, sys, time
import rustique
from rustique._takeover import status as _status, stats as _stats
rustique.status = _status; rustique.stats = _stats
rustique.enable(only=["logging"])
for t in rustique.status()["targets"]:
    if t["group"] == "logging":
        print(t["name"], t["state"], t["reason"])
rustique.stats(True)
sio = io.StringIO()
lg = logging.getLogger("smoke"); lg.propagate = False; lg.setLevel(logging.INFO)
h = logging.StreamHandler(sio); h.setFormatter(logging.Formatter("%(asctime)s %(levelname)-8s [%(name)s] %(module)s:%(funcName)s:%(lineno)d %(message)s")); lg.addHandler(h)
def f():
    lg.debug("nope %s", 1)
    lg.info("x=%s y=%d", "a", 5)
    lg.warning("single %s", 7)
    lg.info("dict %(a)s", {"a": 1})
    lg.error("err", exc_info=False)
f(); f()
print(sio.getvalue())
r = logging.LogRecord("n", 20, "/a/b/c.py", 3, "m %s", (1,), None, "fn", None)
print(list(r.__dict__.items()))
print(json.dumps(rustique.stats(), indent=None))
