"""Smoke check of the interp tier: verification, install, a few results, disable."""
import json
import sys

import rustique

acc = rustique._accel if hasattr(rustique, "_accel") else None
if acc is None:
    from rustique import _rustique
    acc = _rustique._accel
info = acc._t_interp.info()
print(json.dumps({k: v for k, v in info.items() if k != "groups"}, indent=1))


class A:
    def __init__(self, x):
        self.x = x

    def m(self):
        return self.x


class B(A):
    pass


class Meta(type):
    pass


class M(metaclass=Meta):
    pass


class S:
    __slots__ = ("a",)

    def __init__(self):
        self.a = 5


def work():
    out = []
    objs = [A(1), B(2), M(), S(), 3, "s"]
    for _ in range(3):
        for o in objs:
            out.append((isinstance(o, A), isinstance(o, M), isinstance(o, (int, A)),
                        getattr(o, "x", None), getattr(o, "a", None)))
        for o in objs[:2] * 10:
            out.append(o.x + o.m())
        out.append(A(x=7).x)
    return out


before = work()
st = rustique.enable(only=["_interp"]) if "only" in rustique.enable.__code__.co_varnames else None
for t in acc.status()["targets"]:
    if t["name"].startswith("interp."):
        print(t["name"], t["state"], t.get("reason"))
after = work()
print("identical:", before == after)
print(json.dumps(acc._t_interp.info().get("groups"), indent=1))
rustique.disable()
print("after disable swapped:", {k: v["swapped"] for k, v in acc._t_interp.info().get("groups", {}).items()})
