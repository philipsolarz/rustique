import posixpath, os, sys
from rustique._rustique import _t_posixpath as T
cases = [("join", ("/usr", "lib")), ("join", ("a/", "")), ("join", ("", "")), ("join", ("/a", "b", "/c", "d")), ("join", (b"/a", b"b")),
         ("join", ("/jürgen", "你\U0001f600")), ("split", ("/a/b//c",)), ("split", (b"//x",)), ("splitext", ("a/.b.c",)),
         ("basename", ("/a/b",)), ("dirname", ("a//b",)), ("isabs", (b"/x",)), ("relpath", ("/a/b/c", "/a/x")), ("relpath", ("x",)),
         ("relpath", (b"/a/b", b"/a")), ("abspath", ("x/../y",)), ("abspath", (b"/a/./b",))]
ref = {k: getattr(posixpath, k) for k in set(c[0] for c in cases)}
import types
ns = dict(posixpath.__dict__)
for k, v in list(ns.items()):
    if isinstance(v, types.FunctionType):
        ns[k] = types.FunctionType(v.__code__, ns, v.__name__, v.__defaults__, v.__closure__)
before = [(n, ns[n](*a)) for n, a in cases]
print(T.install())
T.stats(True)
after = [(n, getattr(posixpath, n)(*a)) for n, a in cases]
for (n, x), (_, y), (_, a) in zip(before, after, cases):
    print("OK " if x == y and type(x) is type(y) else "BAD", n, a, x, y)
print(T.counters())
T.uninstall()
print(T.status())
