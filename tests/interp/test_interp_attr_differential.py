"""Differential fuzzing of attribute resolution and isinstance under the interp tier.

Hypothesis builds a class hierarchy (descriptors of every kind, __slots__, __getattr__,
__getattribute__, metaclasses), instances (inline values, materialized and replaced
__dict__s, non-str keys), and a program of polymorphic attribute loads, method calls and
isinstance checks with mutations of classes, descriptor types and instances between rounds.
The program runs twice from freshly built classes and freshly compiled code, once plain and
once with the tier on; results, exception types and messages, and the log of every
user-visible side effect (descriptor __get__/__set__, __getattr__, __getattribute__,
__instancecheck__, __class__ getters) must be identical.
"""

from __future__ import annotations

import abc
import functools
from typing import Any

from hypothesis import given, settings
from hypothesis import strategies as st

from _interp import requires_tier, tier

pytestmark = requires_tier

NAMES = ("a", "b", "m", "d", "s")
KINDS = ("absent", "value", "method", "property", "datadesc", "nondatadesc", "classmethod",
         "staticmethod", "cached", "none")
MUTATIONS = ("set_class_value", "del_class", "add_get_to_type", "add_set_to_type",
             "set_instance", "del_instance", "materialize", "replace_dict", "general_keys",
             "swap_class", "set_meta_attr", "noop")

class_spec = st.fixed_dictionaries({
    "attrs": st.dictionaries(st.sampled_from(NAMES), st.sampled_from(KINDS), max_size=4),
    "slots": st.sampled_from([None, (), ("s",), ("a", "s")]),
    "getattr": st.booleans(),
    "getattribute": st.booleans(),
})
spec_strategy = st.fixed_dictionaries({
    "meta": st.sampled_from(["type", "plain", "attrs", "instancecheck", "abc"]),
    "classes": st.lists(class_spec, min_size=1, max_size=3),   # a chain: C0 <- C1 <- C2
    "other_slots": st.booleans(),
    "objs": st.lists(st.tuples(st.integers(0, 3), st.dictionaries(st.sampled_from(NAMES), st.integers(0, 9), max_size=3),
                               st.sampled_from(["plain", "materialize", "replace", "general"])),
                     min_size=1, max_size=5),
    "class_prop": st.booleans(),
    "mutations": st.lists(st.tuples(st.sampled_from(MUTATIONS), st.integers(0, 3), st.sampled_from(NAMES),
                                    st.integers(0, 9)), max_size=6),
    "rounds": st.integers(2, 4),
})


def build(spec: dict[str, Any]) -> tuple[list[Any], list[Any], list[str], Any]:
    """Fresh classes and objects for `spec`; returns (objects, classes, log, mutate)."""
    log: list[str] = []

    class DataDesc:
        def __init__(self, tag: str) -> None:
            self.tag = tag

        def __get__(self, obj: Any, owner: Any = None) -> Any:
            log.append(f"get {self.tag} {type(obj).__name__}")
            return f"dd:{self.tag}"

        def __set__(self, obj: Any, value: Any) -> None:
            log.append(f"set {self.tag} {value!r}")

    class NonDataDesc:
        def __init__(self, tag: str) -> None:
            self.tag = tag

        def __get__(self, obj: Any, owner: Any = None) -> Any:
            log.append(f"ndget {self.tag} {type(obj).__name__}")
            return f"nd:{self.tag}"

    class Plain:
        """A class attribute whose type may later gain __get__/__set__."""

        def __init__(self, tag: str) -> None:
            self.tag = tag

        def __repr__(self) -> str:
            return f"Plain({self.tag})"

    def make_attr(kind: str, tag: str) -> Any:
        if kind == "value":
            return Plain(tag)
        if kind == "method":
            def m(self: Any, *a: Any) -> Any:
                return f"m:{tag}:{type(self).__name__}"
            m.__name__ = tag
            return m
        if kind == "property":
            def g(self: Any) -> Any:
                log.append(f"prop {tag}")
                return f"p:{tag}"
            return property(g)
        if kind == "datadesc":
            return DataDesc(tag)
        if kind == "nondatadesc":
            return NonDataDesc(tag)
        if kind == "classmethod":
            return classmethod(lambda cls, *a: f"cm:{tag}:{cls.__name__}")
        if kind == "staticmethod":
            return staticmethod(lambda *a: f"sm:{tag}")
        if kind == "cached":
            def c(self: Any) -> Any:
                log.append(f"cached {tag}")
                return f"c:{tag}"
            cp = functools.cached_property(c)
            return cp
        if kind == "none":
            return None
        raise AssertionError(kind)

    if spec["meta"] == "type":
        meta: type = type
    elif spec["meta"] == "abc":
        meta = type("AbcMeta", (abc.ABCMeta,), {})   # fresh: mutations must not leak
    else:
        mns: dict[str, Any] = {}
        if spec["meta"] == "attrs":
            mns["a"] = "meta-a"
            mns["m"] = lambda cls: f"meta-m:{cls.__name__}"
        if spec["meta"] == "instancecheck":
            def ic(cls: Any, inst: Any) -> bool:
                log.append(f"instancecheck {cls.__name__} {type(inst).__name__}")
                return type.__instancecheck__(cls, inst)
            mns["__instancecheck__"] = ic
        meta = type("Meta", (type,), mns)

    classes: list[Any] = []
    base: tuple[type, ...] = ()
    for i, cs in enumerate(spec["classes"]):
        ns: dict[str, Any] = {}
        for name, kind in cs["attrs"].items():
            if kind == "absent":
                continue
            if cs["slots"] is not None and name in cs["slots"]:
                continue   # a slot member with that name is the attribute
            if kind == "cached" and cs["slots"] is not None:
                kind = "value"   # cached_property needs an instance __dict__
            ns[name] = make_attr(kind, f"{name}{i}")
            if kind == "cached":
                ns[name].__set_name__(None, name)
        if cs["slots"] is not None:
            parent_slots = set()
            for c in classes:
                parent_slots |= set(getattr(c, "__slots__", ()))
            ns["__slots__"] = tuple(s for s in cs["slots"] if s not in parent_slots)
        if cs["getattr"]:
            def ga(self: Any, name: str, _i: int = i) -> Any:
                log.append(f"getattr{_i} {name}")
                if name == "b":
                    raise AttributeError(name)
                return f"ga{_i}:{name}"
            ns["__getattr__"] = ga
        if cs["getattribute"]:
            def gat(self: Any, name: str, _i: int = i) -> Any:
                if name == "d":
                    log.append(f"getattribute{_i} {name}")
                    return f"gat{_i}"
                return object.__getattribute__(self, name)
            ns["__getattribute__"] = gat
        if spec["class_prop"] and i == 0:
            def cls_get(self: Any) -> Any:
                log.append("__class__")
                return int
            ns["__class__"] = property(cls_get)
        try:
            c = meta(f"C{i}", base, ns)
        except TypeError:
            c = meta(f"C{i}x", base, {k: v for k, v in ns.items() if k != "__slots__"})
        classes.append(c)
        base = (c,)

    oslots = {"__slots__": ("s", "b")} if spec["other_slots"] else {}
    other = type("Other", (), {"m": lambda self: "other-m", "a": Plain("other-a"), **oslots})
    pool = classes + [other]

    objs: list[Any] = []
    for k, attrs, how in spec["objs"]:
        cls = pool[k % len(pool)]
        try:
            o = cls()
        except TypeError:
            o = object.__new__(cls)
        for name, v in attrs.items():
            try:
                setattr(o, name, v)
            except AttributeError as e:
                log.append(f"setattr-error {type(e).__name__}")
        if hasattr(type(o), "__dictoffset__") and type(o).__dictoffset__ != 0:
            try:
                if how == "materialize":
                    o.__dict__
                elif how == "replace":
                    o.__dict__ = dict(o.__dict__)
                elif how == "general":
                    o.__dict__[1] = "int-key"
            except (AttributeError, TypeError):
                pass
        objs.append(o)
    # classes themselves are owners too (metaclass attributes, class attributes)
    objs += classes[:1]

    def mutate(m: tuple[str, int, str, int]) -> None:
        kind, k, name, v = m
        cls = pool[k % len(pool)]
        try:
            if kind == "set_class_value":
                setattr(cls, name, Plain(f"set{v}"))
            elif kind == "del_class":
                delattr(cls, name)
            elif kind == "add_get_to_type":
                Plain.__get__ = lambda self, obj, owner=None: f"now-get:{self.tag}"
            elif kind == "add_set_to_type":
                NonDataDesc.__set__ = lambda self, obj, value: log.append("now-set")
                Plain.__set__ = lambda self, obj, value: log.append("plain-set")
            elif kind == "set_instance":
                setattr(objs[k % len(objs)], name, v)
            elif kind == "del_instance":
                delattr(objs[k % len(objs)], name)
            elif kind == "materialize":
                objs[k % len(objs)].__dict__
            elif kind == "replace_dict":
                o = objs[k % len(objs)]
                o.__dict__ = {name: v}
            elif kind == "general_keys":
                objs[k % len(objs)].__dict__[v] = "g"
            elif kind == "swap_class":
                o = objs[k % len(objs)]
                o.__class__ = pool[(k + 1) % len(pool)]
            elif kind == "set_meta_attr":
                setattr(type(cls), name, f"meta-set{v}")
        except (AttributeError, TypeError) as e:
            log.append(f"mutate-error {type(e).__name__}")

    return objs, pool, log, mutate


PROGRAM = '''
import re as _re

def canon(r, o):
    if callable(r) and hasattr(r, "__func__"):
        return ("bound", r.__func__.__name__, r.__self__ is o)
    if callable(r) and not isinstance(r, type):
        return ("callable", type(r).__name__, getattr(r, "__name__", None))
    if isinstance(r, type):
        return ("type", r.__name__)
    return ("v", _re.sub(r" at 0x[0-9a-f]+", "", repr(r)))

def exc(e):
    return ("exc", type(e).__name__, _re.sub(r" at 0x[0-9a-f]+", "", str(e)))

''' + "".join(f'''
def load_{n}(objs):
    out = []
    for o in objs:
        try:
            out.append(canon(o.{n}, o))
        except Exception as e:
            out.append(exc(e))
    return out

def call_{n}(objs):
    out = []
    for o in objs:
        try:
            out.append(canon(o.{n}(), o))
        except Exception as e:
            out.append(exc(e))
    return out
''' for n in NAMES) + '''
def checks(objs, classes):
    out = []
    for o in objs:
        for k in classes:
            try:
                out.append(isinstance(o, k))
            except Exception as e:
                out.append(exc(e))
        try:
            out.append(isinstance(o, (int, classes[-1], (str, classes[0]))))
            out.append(isinstance(o, tuple(classes)))
        except Exception as e:
            out.append(exc(e))
    return out
'''


def run(spec: dict[str, Any]) -> tuple[list[Any], list[str]]:
    objs, pool, log, mutate = build(spec)
    ns: dict[str, Any] = {}
    exec(compile(PROGRAM, "<program>", "exec"), ns)
    out: list[Any] = []
    muts = list(spec["mutations"])
    for r in range(spec["rounds"]):
        for _ in range(20):   # warm the specializer: sites go polymorphic / generic
            for n in NAMES:
                ns[f"load_{n}"](objs)
            ns["checks"](objs, pool)
        for n in NAMES:
            out.append(ns[f"load_{n}"](objs))
            out.append(ns[f"call_{n}"](objs))
        out.append(ns["checks"](objs, pool))
        if muts:
            mutate(muts.pop(0))
    return out, log


@settings(max_examples=300)
@given(spec_strategy)
def test_attribute_resolution_and_isinstance(spec: dict[str, Any]) -> None:
    plain = run(spec)
    with tier():
        accelerated = run(spec)
    assert accelerated[0] == plain[0]
    assert accelerated[1] == plain[1]


def test_differential_reaches_the_fast_paths() -> None:
    """Guard against a vacuous differential: a fixed scenario hits the attribute cache."""
    from _interp import info

    spec = {"meta": "attrs", "classes": [{"attrs": {"a": "value", "m": "method", "b": "cached"}, "slots": None,
                                          "getattr": False, "getattribute": False},
                                         {"attrs": {"d": "nondatadesc"}, "slots": ("s",), "getattr": False,
                                          "getattribute": False}],
            "other_slots": True, "objs": [(0, {"a": 1}, "plain"), (1, {"b": 2}, "materialize"), (2, {"s": 3}, "plain"),
                                          (3, {}, "plain")],
            "class_prop": False, "mutations": [("set_class_value", 0, "a", 1)], "rounds": 2}
    plain = run(spec)
    with tier():
        accelerated = run(spec)
        g = info()["groups"]["interp.attr"]
    assert accelerated == plain
    assert g["hits"] > 100, g

