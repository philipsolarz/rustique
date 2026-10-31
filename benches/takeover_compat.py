"""Boundary compatibility matrix for the takeover candidates (docs/TAKEOVER.md §3.2).

Every check is run by "foreign" (non-opted-in) code against a container produced by each
candidate, and compared with the same check on an exact builtin in plain CPython. Cells:
works / slow (correct, but >1.5x the builtin's wall time when timing is on; timed
interleaved with the reference) / BREAKS
(different result or exception).

Candidates:
  6     exact builtin (plain CPython)
  6+K   exact builtin with the safe-tier kernels installed (what `rustique.enable()` does)
  4a    subclass of the builtin named like it, builtin storage authoritative
  4b    subclass with native (hidden) storage, every Python-visible method overridden
  1/2/3 the rustique type a rewrite (import hook, code-object rewrite, frame hook) emits
  5     the rustique type, materialised to a builtin copy at the boundary

Used by `benches/takeover_overhead.py compat` and `tests/test_takeover_compat.py`.
"""

import array
import bisect
import builtins
import collections
import collections.abc
import copy
import dataclasses
import heapq
import itertools
import json
import marshal
import operator
import pickle
import pprint
import random
import struct
import sys
import time

import numpy as np

import rustique as rs

_list, _dict, _set = builtins.list, builtins.dict, builtins.set


def _named(cls, name):
    cls.__name__ = cls.__qualname__ = name
    cls.__module__ = "builtins"
    return cls


# --- candidate 4a: subclass with CPython layout (base storage authoritative) ----------
@lambda c: _named(c, "list")
class LayoutList(_list):
    __slots__ = ()

    def __reduce_ex__(self, proto):
        return (_list, (_list(self),))


@lambda c: _named(c, "dict")
class LayoutDict(_dict):
    __slots__ = ()

    def __reduce_ex__(self, proto):
        return (_dict, (_dict(self),))


@lambda c: _named(c, "set")
class LayoutSet(_set):
    __slots__ = ()

    def __reduce_ex__(self, proto):
        return (_set, (_list(self),))

    def __repr__(self):
        return _set.__repr__(_set(self))


# --- candidate 4b: subclass with native storage (base storage empty) ------------------
# Best case: every Python-visible method is overridden; only C code that reads the base
# layout directly (after a non-exact PyList_Check / PyDict_Check / PyAnySet_Check) sees
# the empty base object.
def _fwd(name):
    def method(self, *a, **k):
        return getattr(self._d, name)(*a, **k)

    method.__name__ = name
    return method


@lambda c: _named(c, "list")
class NativeList(_list):
    __slots__ = ("_d",)

    def __new__(cls, it=()):
        self = _list.__new__(cls)
        self._d = _list(it)
        return self

    def __init__(self, it=()):
        pass

    def __getitem__(self, i):
        r = self._d[i]
        return NativeList(r) if isinstance(i, slice) else r

    def __eq__(self, o):
        return self._d == (o._d if isinstance(o, NativeList) else o)

    def __ne__(self, o):
        return not self == o

    def __add__(self, o):
        return NativeList(self._d + _list(o))

    def __radd__(self, o):
        return NativeList(_list(o) + self._d)

    def __iadd__(self, o):
        self._d += o
        return self

    def __mul__(self, n):
        return NativeList(self._d * n)

    __rmul__ = __mul__

    def copy(self):
        return NativeList(self._d)

    def __copy__(self):
        return NativeList(self._d)

    def __deepcopy__(self, memo):
        return NativeList(copy.deepcopy(self._d, memo))

    def __reduce_ex__(self, proto):
        return (_list, (self._d,))

    def __repr__(self):
        return repr(self._d)

    __hash__ = None


for _n in ("__len__", "__iter__", "__reversed__", "__contains__", "__setitem__", "__delitem__",
           "__lt__", "__le__", "__gt__", "__ge__", "__imul__", "append", "extend", "insert", "pop",
           "remove", "clear", "index", "count", "sort", "reverse", "__sizeof__"):
    setattr(NativeList, _n, _fwd(_n))


@lambda c: _named(c, "dict")
class NativeDict(_dict):
    __slots__ = ("_d",)

    def __new__(cls, *a, **k):
        self = _dict.__new__(cls)
        self._d = _dict(*a, **k)
        return self

    def __init__(self, *a, **k):
        pass

    def __eq__(self, o):
        return self._d == (o._d if isinstance(o, NativeDict) else o)

    def __ne__(self, o):
        return not self == o

    def __or__(self, o):
        return NativeDict(self._d | _dict(o))

    def __ror__(self, o):
        return NativeDict(_dict(o) | self._d)

    def __ior__(self, o):
        self._d |= o
        return self

    def copy(self):
        return NativeDict(self._d)

    def __copy__(self):
        return NativeDict(self._d)

    def __deepcopy__(self, memo):
        return NativeDict(copy.deepcopy(self._d, memo))

    def __reduce_ex__(self, proto):
        return (_dict, (self._d,))

    def __repr__(self):
        return repr(self._d)

    __hash__ = None


for _n in ("__len__", "__iter__", "__reversed__", "__contains__", "__getitem__", "__setitem__",
           "__delitem__", "keys", "values", "items", "get", "pop", "popitem", "setdefault",
           "update", "clear", "__sizeof__"):
    setattr(NativeDict, _n, _fwd(_n))


@lambda c: _named(c, "set")
class NativeSet(_set):
    __slots__ = ("_d",)

    def __new__(cls, it=()):
        self = _set.__new__(cls)
        self._d = _set(it)
        return self

    def __init__(self, it=()):
        pass

    def _o(self, o):
        return o._d if isinstance(o, NativeSet) else o

    def __eq__(self, o):
        return self._d == self._o(o)

    def __ne__(self, o):
        return not self == o

    def __le__(self, o):
        return self._d <= self._o(o)

    def __ge__(self, o):
        return self._d >= self._o(o)

    def __lt__(self, o):
        return self._d < self._o(o)

    def __gt__(self, o):
        return self._d > self._o(o)

    def __or__(self, o):
        return NativeSet(self._d | self._o(o))

    __ror__ = __or__

    def __and__(self, o):
        return NativeSet(self._d & self._o(o))

    __rand__ = __and__

    def __sub__(self, o):
        return NativeSet(self._d - self._o(o))

    def __rsub__(self, o):
        return NativeSet(self._o(o) - self._d)

    def __xor__(self, o):
        return NativeSet(self._d ^ self._o(o))

    __rxor__ = __xor__

    def copy(self):
        return NativeSet(self._d)

    def __copy__(self):
        return NativeSet(self._d)

    def __deepcopy__(self, memo):
        return NativeSet(copy.deepcopy(self._d, memo))

    def __reduce_ex__(self, proto):
        return (_set, (_list(self._d),))

    def __repr__(self):
        return repr(self._d)

    __hash__ = None


for _n in ("__len__", "__iter__", "__contains__", "add", "discard", "remove", "pop", "clear",
           "update", "union", "intersection", "difference", "symmetric_difference", "issubset",
           "issuperset", "isdisjoint", "intersection_update", "difference_update",
           "symmetric_difference_update", "__sizeof__"):
    setattr(NativeSet, _n, _fwd(_n))


def _ident(x):
    return x


def _export(x):
    """Candidate 5: materialise to a builtin at the boundary (a copy)."""
    if isinstance(x, rs.List):
        return x.to_list()
    if isinstance(x, rs.Dict):
        return x.to_dict()
    if isinstance(x, rs.Set):
        return x.to_set()
    return x


CANDIDATES = {
    "6: exact builtin": ({"list": _list, "dict": _dict, "set": _set}, _ident),
    "6+K: exact builtin + kernels": ({"list": _list, "dict": _dict, "set": _set}, _ident),
    "4a: subclass, CPython layout": ({"list": LayoutList, "dict": LayoutDict, "set": LayoutSet}, _ident),
    "4b: subclass, native storage": ({"list": NativeList, "dict": NativeDict, "set": NativeSet}, _ident),
    "1/2/3: rs type (rewrite target today)": ({"list": rs.List, "dict": rs.Dict, "set": rs.Set}, _ident),
    "5: rs + export at boundary": ({"list": rs.List, "dict": rs.Dict, "set": rs.Set}, _export),
}

LDATA = [5, 3, 8, 1, 9, 2, 7]
SDATA = ["b", "a", "c"]
DDATA = {"a": 1, "b": [1, 2], "c": None}
SETDATA = {3, 1, 2}


@dataclasses.dataclass
class Holder:
    v: object


def _err(fn):
    try:
        fn()
    except Exception as e:  # noqa: BLE001
        return f"{type(e).__name__}: {e}"
    return "no error"


def _match_list(x):
    match x:
        case [a, b, *rest]:
            return ("seq", a, b, len(rest))
        case _:
            return "nomatch"


def _match_dict(x):
    match x:
        case {"a": v, **rest}:
            return ("map", v, sorted(rest))
        case _:
            return "nomatch"


def _f(*a, **k):
    return a, k


def _ex(B, x):
    ns = B(x)
    exec("r = a * 2", ns)
    return ns["r"]


# Each check: (kind, name, fn(x, B) -> observation, mutation?)
CHECKS = [
    ("list", "isinstance(x, list)", lambda x, B: isinstance(B(x), list)),
    ("list", "type(x) is list", lambda x, B: type(B(x)) is list),
    ("list", "isinstance(x, abc.MutableSequence)", lambda x, B: isinstance(B(x), collections.abc.MutableSequence)),
    ("list", "repr(x) / str / f-string", lambda x, B: (repr(B(x)), str(B(x)), f"{B(x)}")),
    ("list", "x == ref, ref == x", lambda x, B: (B(x) == LDATA, LDATA == B(x))),
    ("list", "json.dumps (C encoder)", lambda x, B: json.dumps(B(x))),
    ("list", "json.dumps(indent=1) (Py encoder)", lambda x, B: json.dumps(B(x), indent=1)),
    ("list", "pickle round trip", lambda x, B: (pickle.loads(pickle.dumps(B(x))), type(pickle.loads(pickle.dumps(B(x)))).__name__)),
    ("list", "pickle stream is builtin-only", lambda x, B: b"rustique" not in pickle.dumps(B(x)) and b"__main__" not in pickle.dumps(B(x))),
    ("list", "marshal.dumps", lambda x, B: marshal.loads(marshal.dumps(B(x)))),
    ("list", "copy.copy", lambda x, B: (copy.copy(B(x)), type(copy.copy(B(x))).__name__)),
    ("list", "copy.deepcopy", lambda x, B: (copy.deepcopy(B(x)), type(copy.deepcopy(B(x))).__name__)),
    ("list", "pprint.pformat(width=10)", lambda x, B: pprint.pformat(B(x), width=10)),
    ("list", "dataclasses.asdict", lambda x, B: dataclasses.asdict(Holder(B(x)))),
    ("list", "f(*x)", lambda x, B: _f(*B(x))),
    ("list", "'-'.join(x) (strs)", lambda x, B: "-".join(B(type(x)(map(str, x))))),
    ("list", "struct.pack('7q', *x)", lambda x, B: struct.pack("7q", *B(x))),
    ("list", "np.array(x)", lambda x, B: np.array(B(x)).tolist()),
    ("list", "array.array('q', x)", lambda x, B: array.array("q", B(x)).tolist()),
    ("list", "match x: case [a, b, *rest]", lambda x, B: _match_list(B(x))),
    ("list", "sorted/sum/min/max", lambda x, B: (sorted(B(x)), sum(B(x)), min(B(x)), max(B(x)))),
    ("list", "tuple/list/set/Counter", lambda x, B: (tuple(B(x)), list(B(x)), sorted(set(B(x))), collections.Counter(B(x)))),
    ("list", "reversed(x), itertools.chain", lambda x, B: (list(reversed(B(x))), list(itertools.chain(B(x), B(x))))),
    ("list", "[0] + x", lambda x, B: [0] + B(x)),
    ("list", "ref.extend(x); ref[1:1] = x", lambda x, B: (lambda r: (r.extend(B(x)), r.__setitem__(slice(1, 1), B(x)), r)[2])([0])),
    ("list", "'{0[1]}'.format(x), itemgetter", lambda x, B: ("{0[1]}".format(B(x)), operator.itemgetter(1, 2)(B(x)))),
    ("list", "hash(x) error", lambda x, B: _err(lambda: hash(B(x)))),
    ("list", "x[99] error", lambda x, B: _err(lambda: B(x)[99])),
    ("list", "x.nope error", lambda x, B: _err(lambda: B(x).nope)),
    ("list", "x.index(99) error", lambda x, B: _err(lambda: B(x).index(99))),
    ("list", "mutation: heapq.heappush", lambda x, B: (heapq.heappush(B(x), -1), list(x))[1], True),
    ("list", "mutation: heapq.heapify", lambda x, B: (heapq.heapify(B(x)), list(x))[1], True),
    ("list", "mutation: bisect.insort", lambda x, B: (bisect.insort(B(x), 4), list(x))[1], True),
    ("list", "mutation: random.shuffle", lambda x, B: (random.Random(1).shuffle(B(x)), list(x))[1], True),
    ("list", "mutation: callee x.append / x.sort", lambda x, B: (lambda y: (y.append(0), y.sort(), list(x))[2])(B(x)), True),
    ("list", "identity: callee sees same object", lambda x, B: B(x) is x),
    ("dict", "isinstance(d, dict)", lambda x, B: isinstance(B(x), dict)),
    ("dict", "type(d) is dict", lambda x, B: type(B(x)) is dict),
    ("dict", "repr(d)", lambda x, B: repr(B(x))),
    ("dict", "d == ref, ref == d", lambda x, B: (B(x) == DDATA, DDATA == B(x))),
    ("dict", "json.dumps (C encoder)", lambda x, B: json.dumps(B(x))),
    ("dict", "json.dumps(indent, sort_keys)", lambda x, B: json.dumps(B(x), indent=1, sort_keys=True)),
    ("dict", "pickle round trip", lambda x, B: (pickle.loads(pickle.dumps(B(x))), type(pickle.loads(pickle.dumps(B(x)))).__name__)),
    ("dict", "marshal.dumps", lambda x, B: marshal.loads(marshal.dumps(B(x)))),
    ("dict", "copy.copy / deepcopy", lambda x, B: (copy.copy(B(x)), type(copy.copy(B(x))).__name__, copy.deepcopy(B(x)))),
    ("dict", "pprint.pformat(width=10)", lambda x, B: pprint.pformat(B(x), width=10)),
    ("dict", "dataclasses.asdict", lambda x, B: dataclasses.asdict(Holder(B(x)))),
    ("dict", "f(**d)", lambda x, B: _f(**B(x))),
    ("dict", "dict(d), {**d}, ref.update(d)", lambda x, B: (dict(B(x)), {**B(x)}, (lambda r: (r.update(B(x)), r)[1])({}))),
    ("dict", "ref | d", lambda x, B: {"z": 0} | B(x)),
    ("dict", "match d: case {'a': v}", lambda x, B: _match_dict(B(x))),
    ("dict", "'%(a)s' % d, format_map", lambda x, B: ("%(a)s" % B(x), "{a}".format_map(B(x)))),
    ("dict", "exec(code, d) globals", lambda x, B: _ex(B, x)),
    ("dict", "str.maketrans(d)", lambda x, B: str.maketrans(B(x))),
    ("dict", "type('T', (), d) namespace", lambda x, B: sorted(k for k in list(vars(type("T", (), B(x)))) if not k.startswith("__"))),
    ("dict", "hash(d) error", lambda x, B: _err(lambda: hash(B(x)))),
    ("dict", "d['zz'] error", lambda x, B: _err(lambda: B(x)["zz"])),
    ("dict", "mutation: callee d[k] = v", lambda x, B: (B(x).__setitem__("q", 1), sorted(x))[1], True),
    ("set", "isinstance(s, set)", lambda x, B: isinstance(B(x), set)),
    ("set", "type(s) is set", lambda x, B: type(B(x)) is set),
    ("set", "repr(s)", lambda x, B: repr(B(x))),
    ("set", "s == ref, ref == s", lambda x, B: (B(x) == SETDATA, SETDATA == B(x))),
    ("set", "ref | s, ref & s, ref - s, ref ^ s", lambda x, B: ({0, 1} | B(x), {0, 1} & B(x), {0, 1} - B(x), {0, 1} ^ B(x))),
    ("set", "ref <= s, ref.issubset(s)", lambda x, B: ({1} <= B(x), {1}.issubset(B(x)), {9}.isdisjoint(B(x)))),
    ("set", "frozenset(s), ref.update(s)", lambda x, B: (frozenset(B(x)), (lambda r: (r.update(B(x)), r)[1])(set()))),
    ("set", "pickle round trip", lambda x, B: (pickle.loads(pickle.dumps(B(x))), type(pickle.loads(pickle.dumps(B(x)))).__name__)),
    ("set", "marshal.dumps", lambda x, B: marshal.loads(marshal.dumps(B(x)))),
    ("set", "copy.copy / deepcopy", lambda x, B: (copy.copy(B(x)), type(copy.copy(B(x))).__name__, copy.deepcopy(B(x)))),
    ("set", "pprint.pformat", lambda x, B: pprint.pformat(B(x))),
    ("set", "json.dumps error", lambda x, B: _err(lambda: json.dumps(B(x)))),
    ("set", "sorted/sum/max", lambda x, B: (sorted(B(x)), sum(B(x)), max(B(x)))),
    ("set", "hash(s) error", lambda x, B: _err(lambda: hash(B(x)))),
    ("set", "mutation: callee s.add", lambda x, B: (B(x).add(0), sorted(x))[1], True),
]

DATA = {"list": LDATA, "dict": DDATA, "set": SETDATA}


def _probe_extra():
    """Probes emulating PyO3 extension code (`crates/rustique-py/src/takeover.rs`)."""
    t = getattr(rs._rustique, "_takeover", None)
    if t is None or not hasattr(t, "_pyo3_probe_list"):
        return []
    return [
        ("list", "PyO3: cast::<PyList>() + get_item", lambda x, B: t._pyo3_probe_list(B(x))),
        ("list", "PyO3: Vec<i64>::extract", lambda x, B: t._pyo3_probe_vec(B(x))),
        ("dict", "PyO3: cast::<PyDict>() + iter", lambda x, B: t._pyo3_probe_dict(B(x))),
        ("set", "PyO3: cast::<PySet>() + iter", lambda x, B: t._pyo3_probe_set(B(x))),
    ]


def _loops(fn, budget=0.005):
    n = 1
    while True:
        t0 = time.perf_counter()
        for _ in range(n):
            fn()
        if time.perf_counter() - t0 > budget:
            return n
        n *= 2


def _ratio(ref, cand, rounds=7):
    """Wall-time ratio cand/ref, timed interleaved (ref, cand, ref, cand, ...) and taking
    the best of each, so that drifting machine load cancels."""
    n_ref, n_cand = _loops(ref), _loops(cand)
    best_ref = best_cand = float("inf")
    for _ in range(rounds):
        t0 = time.perf_counter()
        for _ in range(n_ref):
            ref()
        best_ref = min(best_ref, (time.perf_counter() - t0) / n_ref)
        t0 = time.perf_counter()
        for _ in range(n_cand):
            cand()
        best_cand = min(best_cand, (time.perf_counter() - t0) / n_cand)
    return best_cand / best_ref


def _make(T, kind):
    d = DATA[kind]
    if kind == "dict":
        return T(d)
    return T(d)


def run(time_it=True, candidates=None):
    """Run every check for every candidate: (names, {(kind, label): {name: (status, info)}})."""
    checks = CHECKS + _probe_extra()
    names = list(candidates or CANDIDATES)
    if names[0] != "6: exact builtin":
        names.insert(0, "6: exact builtin")
    kernels = rs._rustique._takeover
    was_installed = bool(kernels.installed())
    kernels.uninstall()
    try:
        return names, _run(checks, names, time_it, kernels)
    finally:
        (kernels.install if was_installed else kernels.uninstall)()


def _run(checks, names, time_it, kernels):
    results = {}
    for check in checks:
        kind, label, fn = check[:3]
        ref_types, _ = CANDIDATES[names[0]]
        try:
            ref = fn(_make(ref_types[kind], kind), _ident)
        except Exception as e:  # noqa: BLE001
            ref = ("raised", type(e).__name__, str(e))
        ref_fn = lambda: fn(_make(ref_types[kind], kind), _ident)  # noqa: E731
        row = {}
        for cname in names:
            types, B = CANDIDATES[cname]
            T = types[kind]
            if cname.startswith("6+K"):
                kernels.install()
            try:
                got = fn(_make(T, kind), B)
            except Exception as e:  # noqa: BLE001
                got = ("raised", type(e).__name__, str(e))
            finally:
                kernels.uninstall()
            if got != ref:
                row[cname] = ("BREAKS", f"{got!r}"[:90])
                continue
            if time_it and cname != names[0]:
                if cname.startswith("6+K"):
                    kernels.install()
                try:
                    r = _ratio(ref_fn, lambda: fn(_make(T, kind), B))
                    if r > 1.5:  # confirm before calling it slow (sub-microsecond checks are noisy)
                        r = min(r, _ratio(ref_fn, lambda: fn(_make(T, kind), B)))
                finally:
                    kernels.uninstall()
                row[cname] = ("slow", f"{r:.1f}x") if r > 1.5 else ("works", f"{r:.2f}x")
            else:
                row[cname] = ("works", "")
        results[(kind, label)] = row
    return results


def table(names, results, detail=False):
    out = ["| kind | boundary | " + " | ".join(names) + " |", "|---|---|" + "---|" * len(names)]
    for (kind, label), row in results.items():
        cells = []
        for n in names:
            status, info = row[n]
            if status == "BREAKS":
                cells.append("**BREAKS**" + (f" `{info}`" if detail else ""))
            elif status == "slow":
                cells.append(f"slow ({info})")
            else:
                cells.append("works")
        out.append(f"| {kind} | {label} | " + " | ".join(cells) + " |")
    summary = []
    for n in names:
        c = collections.Counter(row[n][0] for row in results.values())
        summary.append(f"{n}: works {c['works']}, slow {c['slow']}, BREAKS {c['BREAKS']}")
    return "\n".join(out) + "\n\n" + "\n".join(f"- {s}" for s in summary)


if __name__ == "__main__":
    names, results = run(time_it="--no-time" not in sys.argv)
    print(table(names, results, detail="--detail" in sys.argv))
