from rustique import _takeover as _T
_T.enable(only=["enum", "generated", "contextlib", "functools"])
import functools, rustique
from rustique._rustique import _accel as A
@functools.singledispatch
def fmt(x):
    return 0
@fmt.register(int)
def _(x):
    return 1
print("orig vc", A.vectorcall_is_original(fmt.dispatch))
rustique.stats(True)
for i in range(5): fmt(1); fmt.dispatch(int)
print({k: v for k, v in rustique.stats().items() if "functools" in k})
print([ (t["name"], t["state"], t["reason"]) for t in rustique.status()["targets"] if t["group"]=="functools"])
