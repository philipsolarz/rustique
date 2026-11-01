import posixpath, os, sys, types
import rustique
cases = [("join", ("/usr", "lib")), ("join", ("a/", "")), ("join", ("", "")), ("join", ("/a", "b", "/c", "d")), ("join", (b"/a", b"b")),
         ("join", ("/jürgen", "你\U0001f600")), ("split", ("/a/b//c",)), ("split", (b"//x",)), ("splitext", ("a/.b.c",)),
         ("basename", ("/a/b",)), ("dirname", ("a//b",)), ("isabs", (b"/x",)), ("relpath", ("/a/b/c", "/a/x")), ("relpath", ("x",)),
         ("relpath", (b"/a/b", b"/a")), ("abspath", ("x/../y",)), ("abspath", (b"/a/./b",))]
ns = dict(posixpath.__dict__)
for k, v in list(ns.items()):
    if isinstance(v, types.FunctionType):
        ns[k] = types.FunctionType(v.__code__, ns, v.__name__, v.__defaults__, v.__closure__)
before = [(n, ns[n](*a)) for n, a in cases]
rustique.enable(only=["posixpath"])
from rustique._takeover import stats, status; stats(True)
print([(t["name"], t["state"], t["reason"]) for t in status()["targets"] if t["group"] == "posixpath"])
after = [(n, getattr(posixpath, n)(*a)) for n, a in cases]
bad = [(n, a, x, y) for (n, x), (_, y), (_, a) in zip(before, after, cases) if not (x == y and type(x) is type(y))]
print("BAD", bad)
print({k: v for k, v in stats().items() if k.startswith("posixpath")})
