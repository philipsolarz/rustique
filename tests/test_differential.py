"""Differential testing (ARCHITECTURE §1.4, §7): Hypothesis rule-based state machines
apply the same random operation sequence to a builtin container (the model) and to a
container under test (the subject), and compare every observable result: return values,
exception types, length, iteration order, `to_list()`/`to_dict()`/`to_set()`, `==`.

Subjects (``RUSTIQUE_IMPLS``):

* ``builtin`` -- the builtin container itself. This proves the machines are sound.
* ``untyped`` -- ``List``/``Dict``/``Set``. Element generators exercise every storage
  strategy and every demotion (i64 overflow, float/bool/str/None mixes, lone surrogates).
* ``typed``   -- ``List[T]``/``Dict[K, V]``/``Set[T]``. The model applies the schema's
  coercion (``bool`` -> ``int``, ``int`` -> ``float``) and rejects invalid values with
  ``TypeError``; the subject must do the same and stay unchanged.

Documented, deliberate deviations are excluded from comparison (not hidden):
NaN identity (List searches and comparisons treat every NaN as equal to NaN, §4.1) --
queries never use NaN and ``==`` is only asserted when no NaN is stored; NaN ordering after
``sort`` (only "no crash, same multiset" is specified); the provenance of
equal-but-differently typed elements in set intersections (CPython's choice depends on
operand sizes); and whether a typed ``extend`` rejected part-way keeps the valid prefix.
"""

from __future__ import annotations

import math
import operator
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass, field
from typing import Any

import pytest
from _support import assert_same_items, load_rustique, multiset, same, selected_kinds
from hypothesis import note, settings
from hypothesis import strategies as st
from hypothesis.stateful import (
    RuleBasedStateMachine,
    initialize,
    invariant,
    precondition,
    rule,
    run_state_machine_as_test,
)

I64_MIN, I64_MAX = -(2**63), 2**63 - 1


def is_nan(x: object) -> bool:
    return type(x) is float and math.isnan(x)


def has_nan(xs: Iterable[object]) -> bool:
    return any(is_nan(x) for x in xs)


# --------------------------------------------------------------------------------------
# Element generators: one per storage strategy, plus demotion triggers and mixes.
# --------------------------------------------------------------------------------------

INTS = st.one_of(
    st.integers(-5, 5),
    st.integers(I64_MIN, I64_MAX),
    st.sampled_from([I64_MIN, I64_MAX, 2**31, -(2**31) - 1, 2**53 + 1, 1024, 1025]),
)
BIG_INTS = st.one_of(
    st.integers(-5, 5),
    st.integers(I64_MAX - 2, 2**64 + 2),
    st.integers(-(2**64) - 2, I64_MIN + 2),
    st.sampled_from([2**63, -(2**63) - 1, 2**64, 2**100, -(2**100)]),
)
FLOATS = st.one_of(
    st.floats(),
    st.sampled_from([0.0, -0.0, 1.0, -1.0, 0.5, math.inf, -math.inf, math.nan, 1e308, 5e-324, 2.0**63, 0.1]),
    st.integers(-3, 3).map(float),
)
FLOATS_NO_NAN = FLOATS.filter(lambda x: not math.isnan(x))
FLOAT_DUPS = st.sampled_from([0.0, -0.0, 1.0, -1.0, 0.5, math.inf])
BOOLS = st.booleans()
STRS = st.one_of(st.text(max_size=5), st.sampled_from(["", "a", "é", "😀", "\x00", "A" * 30, "0"]))
SURROGATES = st.sampled_from(["\ud800", "a\udfff", "\udc00b"])
SMALL_EXACT_INTS = st.integers(-(2**53), 2**53)
HASHABLE_MISC = st.one_of(st.none(), st.tuples(st.integers(0, 3)), st.binary(max_size=2), st.frozensets(st.integers(0, 2), max_size=2))
MIXED = st.one_of(INTS, BIG_INTS, FLOATS, BOOLS, STRS, SURROGATES, HASHABLE_MISC, st.lists(st.integers(0, 3), max_size=2))
MIXED_HASHABLE_NO_NAN = st.one_of(INTS, BIG_INTS, FLOATS_NO_NAN, BOOLS, STRS, SURROGATES, HASHABLE_MISC)
UNHASHABLE = st.one_of(st.lists(st.integers(0, 2), max_size=2), st.dictionaries(st.integers(0, 2), st.integers(0, 2), max_size=1))


def _to_int(x: Any) -> int:
    if type(x) is bool or type(x) is int:
        return int(x)
    raise TypeError(f"expects int, got {type(x).__name__}")


def _to_float(x: Any) -> float:
    if type(x) is float:
        return x
    if type(x) is int:
        return float(x)
    raise TypeError(f"expects float, got {type(x).__name__}")


def _to_bool(x: Any) -> bool:
    if type(x) is bool:
        return x
    raise TypeError(f"expects bool, got {type(x).__name__}")


def _to_str(x: Any) -> str:
    if type(x) is str:
        return x
    raise TypeError(f"expects str, got {type(x).__name__}")


def _to_opt_int(x: Any) -> int | None:
    return None if x is None else _to_int(x)


@dataclass(frozen=True)
class ElemKind:
    """Values for one storage family, and how a typed container of it behaves."""

    name: str
    values: Any  # strategy for untyped containers
    typed: Any = None  # schema type expression for the typed variant (None: untyped only)
    typed_values: Any = None  # valid values for the typed variant
    invalid: Any = None  # values the typed variant must reject with TypeError
    coerce: Callable[[Any], Any] = field(default=lambda x: x)
    probes: tuple[Any, ...] = ()  # extra query values (cross-type equality traps)


def _list_kinds() -> list[ElemKind]:
    return [
        ElemKind(
            "int",
            INTS,
            typed=int,
            typed_values=st.one_of(INTS, BOOLS),
            invalid=st.sampled_from(["1", None, 1.5, b"x", (1,)]),
            coerce=_to_int,
            probes=(1.0, True, float(2**63), 2**63, "1", None, 0.5),
        ),
        ElemKind(
            "bigint",
            BIG_INTS,
            typed=int,
            typed_values=BIG_INTS,
            invalid=st.sampled_from(["1", None, 1.5]),
            coerce=_to_int,
            probes=(float(2**64), 2.0**100, 1.0, True),
        ),
        ElemKind(
            "float",
            FLOATS,
            typed=float,
            typed_values=st.one_of(FLOATS, SMALL_EXACT_INTS),
            invalid=st.sampled_from(["1.0", None, b"x"]),
            coerce=_to_float,
            probes=(1, 0, True, 2**63, 2**1000, -0.0, "0.5"),
        ),
        ElemKind(
            # Few distinct values: duplicates and signed zeros expose unstable or
            # total-order (rather than Python-equality) native sorts, min/max and search.
            "float-dups",
            FLOAT_DUPS,
            typed=float,
            typed_values=st.one_of(FLOAT_DUPS, st.integers(-1, 1)),
            invalid=st.sampled_from(["0.0", None]),
            coerce=_to_float,
            probes=(0, -0.0, 0.0, 1, True, False),
        ),
        ElemKind(
            "bool",
            BOOLS,
            typed=bool,
            typed_values=BOOLS,
            invalid=st.sampled_from([0, 1, 2, 1.0, "True", None]),
            coerce=_to_bool,
            probes=(0, 1, 1.0, 0.0, 2, None),
        ),
        ElemKind(
            "str",
            st.one_of(STRS, SURROGATES),
            typed=str,
            typed_values=STRS,
            invalid=st.sampled_from([1, None, b"a", 1.5]),
            coerce=_to_str,
            probes=(1, None, b"a"),
        ),
        ElemKind(
            "int-or-none",
            st.one_of(st.none(), INTS),
            typed=int | None,
            typed_values=st.one_of(st.none(), INTS, BOOLS),
            invalid=st.sampled_from(["1", 1.5, b"x"]),
            coerce=_to_opt_int,
            probes=(1.0, True, None, "x"),
        ),
        ElemKind("mixed", MIXED, probes=(1, 1.0, True, None, "a", 0, -0.0)),
    ]


LIST_KINDS = _list_kinds()


def _subject_kinds() -> list[str]:
    return list(selected_kinds())


def _params(kinds: list[ElemKind]) -> list[Any]:
    out = []
    for sk in _subject_kinds():
        for ek in kinds:
            if sk == "typed" and ek.typed is None:
                continue
            out.append(pytest.param(sk, ek, id=f"{sk}-{ek.name}"))
    return out


# --------------------------------------------------------------------------------------
# Outcome comparison helpers
# --------------------------------------------------------------------------------------

_OK = object()


def outcome(f: Callable[[], Any]) -> tuple[Any, BaseException | None]:
    try:
        return f(), None
    except Exception as e:  # noqa: BLE001 -- we compare exception types
        return None, e


def plain(x: Any) -> Any:
    """Normalise a rustique container result into a builtin for comparison."""
    name = type(x).__name__
    if name == "List" and type(x).__module__.startswith("rustique"):
        return list(x)
    if name == "Dict" and type(x).__module__.startswith("rustique"):
        return dict(x.items())
    if name == "Set" and type(x).__module__.startswith("rustique"):
        return set(x)
    return x


def assert_same_outcome(ctx: str, m: tuple[Any, BaseException | None], s: tuple[Any, BaseException | None]) -> None:
    (mv, me), (sv, se) = m, s
    if me is None:
        assert se is None, f"{ctx}: subject raised {se!r}; builtin returned {mv!r}"
        sv = plain(sv)
        if isinstance(mv, list):
            assert isinstance(sv, list), f"{ctx}: {sv!r} vs {mv!r}"
            assert_same_items(sv, mv)
        else:
            assert same(sv, mv), f"{ctx}: subject returned {sv!r} ({type(sv).__name__}); builtin {mv!r} ({type(mv).__name__})"
    else:
        assert se is not None, f"{ctx}: builtin raised {me!r}; subject returned {sv!r}"
        assert type(se) is type(me), f"{ctx}: subject raised {se!r}; builtin raised {me!r}"


def failing_iter(items: list[Any], exc: type[Exception] = RuntimeError) -> Iterator[Any]:
    yield from items
    raise exc("iterator failed")


def as_form(items: list[Any], form: str, make_subject: Callable[[list[Any]], Any]) -> Any:
    if form == "list":
        return list(items)
    if form == "tuple":
        return tuple(items)
    if form == "gen":
        return (x for x in items)
    if form == "subject":
        return make_subject(items)
    raise AssertionError(form)


FORMS = st.sampled_from(["list", "tuple", "gen", "subject"])
SMALL_INDEX = st.integers(-12, 12)
OPT_INDEX = st.one_of(st.none(), SMALL_INDEX)
STEPS = st.one_of(st.none(), st.integers(-3, 3))
SORT_KEYS: dict[str, Callable[[Any], Any] | None] = {
    "none": None,
    "repr": repr,
    "const": lambda x: 0,
    "typename": lambda x: type(x).__name__,
}

LIST_STRATEGIES = {"empty", "bool", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "str", "object"}


def native_strategy_fits(strategy: str, x: Any) -> bool:
    t = type(x)
    if strategy == "bool":
        return t is bool
    if strategy == "i64":
        return t is int and I64_MIN <= x <= I64_MAX
    if strategy == "f64":
        return t is float
    if strategy == "str":
        if t is not str:
            return False
        try:
            x.encode("utf-8")
        except UnicodeEncodeError:
            return False
        return True
    return strategy == "object"


def tightest_strategy(items: list[Any]) -> str:
    if not items:
        return "empty"
    for s in ("bool", "i64", "f64", "str"):
        if all(native_strategy_fits(s, x) for x in items):
            return s
    return "object"


# --------------------------------------------------------------------------------------
# List machine
# --------------------------------------------------------------------------------------


def build_list_machine(subject_kind: str, ek: ElemKind) -> type[RuleBasedStateMachine]:
    typed = subject_kind == "typed"
    rs = load_rustique() if subject_kind != "builtin" else None
    coerce: Callable[[Any], Any] = ek.coerce if typed else (lambda x: x)
    valid = ek.typed_values if typed else ek.values
    values = st.one_of(valid, valid, valid, ek.invalid) if typed else valid
    queries = st.one_of(values, st.sampled_from(ek.probes) if ek.probes else values).filter(lambda x: not is_nan(x))
    clean_values = valid.filter(lambda x: not is_nan(x))

    def make(items: list[Any]) -> Any:
        if subject_kind == "builtin":
            return list(items)
        assert rs is not None
        if typed:
            return rs.List[ek.typed](items)
        return rs.List(items)

    class ListMachine(RuleBasedStateMachine):
        model: list[Any]
        subject: Any

        @initialize(init=st.lists(valid, max_size=10), form=st.sampled_from(["list", "tuple", "gen"]))
        def init(self, init: list[Any], form: str) -> None:
            self.model = [coerce(x) for x in init]
            self.subject = make(as_form(init, form, make))
            self.was_object = False
            if subject_kind == "untyped":
                assert self.subject.strategy == tightest_strategy(self.model), (
                    f"fresh List{self.model!r} picked {self.subject.strategy!r}"
                )

        # ---- helpers -------------------------------------------------------------------

        def mutate(
            self,
            ctx: str,
            model_op: Callable[[list[Any]], Any],
            subject_op: Callable[[Any], Any],
            *,
            partial_ok: bool = False,
            also_ok: tuple[type[BaseException], ...] = (),
        ) -> None:
            """Apply an operation to both. ``also_ok``: further exception types the subject
            may raise instead of the model's when an input is invalid in two ways (e.g. a
            wrong-typed value at an out-of-range index: which check runs first is free)."""
            before = list(self.model)
            m = outcome(lambda: model_op(self.model))
            s = outcome(lambda: subject_op(self.subject))
            note(f"{ctx}: builtin -> {m!r}; subject -> {s!r}")
            if m[1] is not None and s[1] is not None and isinstance(s[1], also_ok):
                assert_same_items(self.subject, before)
                return
            if m[1] is not None and partial_ok and s[1] is not None and type(s[1]) is type(m[1]):
                # Appending operations that fail part-way: builtin `list` keeps the consumed
                # prefix, a validate-then-commit implementation keeps nothing. Accept both.
                got = list(self.subject)
                assert len(got) >= len(before), f"{ctx}: items lost: {got!r} vs {before!r}"
                assert_same_items(got[: len(before)], before)
                self.model[:] = got
                return
            assert_same_outcome(ctx, m, s)

        def coerce_all(self, xs: Iterable[Any]) -> list[Any]:
            return [coerce(x) for x in xs]

        # ---- growth --------------------------------------------------------------------

        @rule(x=values)
        def append(self, x: Any) -> None:
            self.mutate(f"append({x!r})", lambda m: m.append(coerce(x)), lambda s: s.append(x))

        @rule(i=SMALL_INDEX, x=values)
        def insert(self, i: int, x: Any) -> None:
            self.mutate(f"insert({i}, {x!r})", lambda m: m.insert(i, coerce(x)), lambda s: s.insert(i, x))

        @rule(xs=st.lists(values, max_size=5), form=FORMS)
        def extend(self, xs: list[Any], form: str) -> None:
            if form == "subject":
                xs = [x for x in xs if outcome(lambda x=x: coerce(x))[1] is None]
            self.mutate(
                f"extend({form} {xs!r})",
                lambda m: m.extend(self.coerce_all(xs)),
                lambda s: s.extend(as_form(xs, form, make)),
                partial_ok=True,
            )

        @rule(xs=st.lists(values, max_size=5), form=FORMS)
        def iadd(self, xs: list[Any], form: str) -> None:
            if form == "subject":
                xs = [x for x in xs if outcome(lambda x=x: coerce(x))[1] is None]

            def subject_op(s: Any) -> None:
                before = s
                s += as_form(xs, form, make)
                assert s is before, "+= must return the same object"

            def model_op(m: list[Any]) -> None:
                m += self.coerce_all(xs)

            self.mutate(f"+= {form} {xs!r}", model_op, subject_op, partial_ok=True)

        @precondition(lambda self: len(self.model) <= 24)
        @rule()
        def extend_self(self) -> None:
            self.mutate("extend(self)", lambda m: m.extend(list(m)), lambda s: s.extend(s))

        @rule(xs=st.lists(valid, max_size=3))
        def extend_failing_iterator(self, xs: list[Any]) -> None:
            def model_op(m: list[Any]) -> None:
                m.extend(self.coerce_all(xs))
                raise RuntimeError("iterator failed")

            # CPython parity: the items yielded before the iterator raised stay appended.
            self.mutate("extend(failing)", model_op, lambda s: s.extend(failing_iter(xs)))

        @precondition(lambda self: len(self.model) <= 24)
        @rule(n=st.integers(-1, 3))
        def imul(self, n: int) -> None:
            def subject_op(s: Any) -> None:
                before = s
                s *= n
                assert s is before

            def model_op(m: list[Any]) -> None:
                m *= n

            self.mutate(f"*= {n}", model_op, subject_op)

        # ---- removal -------------------------------------------------------------------

        @rule(i=OPT_INDEX)
        def pop(self, i: int | None) -> None:
            args = () if i is None else (i,)
            self.mutate(f"pop{args}", lambda m: m.pop(*args), lambda s: s.pop(*args))

        @rule(q=queries)
        def remove(self, q: Any) -> None:
            self.mutate(f"remove({q!r})", lambda m: m.remove(q), lambda s: s.remove(q))

        @precondition(lambda self: len(self.model) > 0)
        @rule(data=st.data())
        def remove_existing(self, data: Any) -> None:
            q = data.draw(st.sampled_from(self.model))
            if is_nan(q):
                return
            self.mutate(f"remove(existing {q!r})", lambda m: m.remove(q), lambda s: s.remove(q))

        @rule(i=SMALL_INDEX)
        def delitem(self, i: int) -> None:
            self.mutate(f"del [{i}]", lambda m: m.__delitem__(i), lambda s: s.__delitem__(i))

        @rule(start=OPT_INDEX, stop=OPT_INDEX, step=STEPS)
        def delslice(self, start: int | None, stop: int | None, step: int | None) -> None:
            sl = slice(start, stop, step)
            self.mutate(f"del [{sl}]", lambda m: m.__delitem__(sl), lambda s: s.__delitem__(sl))

        @rule()
        def clear(self) -> None:
            self.mutate("clear()", lambda m: m.clear(), lambda s: s.clear())

        # ---- assignment ----------------------------------------------------------------

        @rule(i=SMALL_INDEX, x=values)
        def setitem(self, i: int, x: Any) -> None:
            def model_op(m: list[Any]) -> None:
                v = coerce(x)
                m[i] = v

            index_error = not (-len(self.model) <= i < len(self.model))
            self.mutate(f"[{i}] = {x!r}", model_op, lambda s: s.__setitem__(i, x), also_ok=(IndexError, TypeError) if index_error else ())

        @rule(start=OPT_INDEX, stop=OPT_INDEX, xs=st.lists(values, max_size=5), form=st.sampled_from(["list", "tuple", "gen"]))
        def setslice(self, start: int | None, stop: int | None, xs: list[Any], form: str) -> None:
            sl = slice(start, stop)

            def model_op(m: list[Any]) -> None:
                m[sl] = self.coerce_all(xs)

            self.mutate(f"[{sl}] = {form} {xs!r}", model_op, lambda s: s.__setitem__(sl, as_form(xs, form, make)))

        @rule(start=OPT_INDEX, stop=OPT_INDEX, step=st.sampled_from([-3, -2, -1, 2, 3]), extra=st.sampled_from([0, 0, 0, 1, -1]), data=st.data())
        def setslice_extended(self, start: int | None, stop: int | None, step: int, extra: int, data: Any) -> None:
            sl = slice(start, stop, step)
            n = max(0, len(range(*sl.indices(len(self.model)))) + extra)
            xs = data.draw(st.lists(values, min_size=n, max_size=n))

            def model_op(m: list[Any]) -> None:
                m[sl] = self.coerce_all(xs)

            self.mutate(f"[{sl}] = {xs!r}", model_op, lambda s: s.__setitem__(sl, xs), also_ok=(ValueError, TypeError) if extra else ())

        @precondition(lambda self: len(self.model) <= 24)
        @rule(start=OPT_INDEX, stop=OPT_INDEX)
        def setslice_self(self, start: int | None, stop: int | None) -> None:
            sl = slice(start, stop)

            def model_op(m: list[Any]) -> None:
                m[sl] = list(m)

            self.mutate(f"[{sl}] = self", model_op, lambda s: s.__setitem__(sl, s))

        # ---- reordering ----------------------------------------------------------------

        @rule(key=st.sampled_from(sorted(SORT_KEYS)), reverse=st.booleans())
        def sort(self, key: str, reverse: bool) -> None:
            k = SORT_KEYS[key]
            before = list(self.model)
            m = outcome(lambda: self.model.sort(key=k, reverse=reverse))
            s = outcome(lambda: self.subject.sort(key=k, reverse=reverse))
            note(f"sort(key={key}, reverse={reverse}): builtin -> {m!r}; subject -> {s!r}")
            if m[1] is not None or (k is None and has_nan(before)):
                # Failed sorts leave "some permutation"; NaN order is unspecified.
                assert (s[1] is None) == (m[1] is None) or has_nan(before), f"sort outcome {s!r} vs {m!r}"
                if m[1] is not None and s[1] is not None:
                    assert type(s[1]) is type(m[1])
                got = list(self.subject)
                assert multiset(got) == multiset(before), f"sort lost items: {got!r} vs {before!r}"
                self.model[:] = got
                return
            assert_same_outcome("sort", m, s)

        @rule()
        def reverse(self) -> None:
            self.mutate("reverse()", lambda m: m.reverse(), lambda s: s.reverse())

        # ---- queries (no mutation) -----------------------------------------------------

        @rule(i=SMALL_INDEX)
        def getitem(self, i: int) -> None:
            assert_same_outcome(f"[{i}]", outcome(lambda: self.model[i]), outcome(lambda: self.subject[i]))

        @rule(start=OPT_INDEX, stop=OPT_INDEX, step=STEPS)
        def getslice(self, start: int | None, stop: int | None, step: int | None) -> None:
            sl = slice(start, stop, step)
            s = outcome(lambda: self.subject[sl])
            if s[1] is None and subject_kind != "builtin":
                assert type(s[0]) is type(self.subject)
                assert s[0].schema_type is self.subject.schema_type
            assert_same_outcome(f"[{sl}]", outcome(lambda: self.model[sl]), s)

        @rule(q=queries, start=OPT_INDEX, stop=OPT_INDEX)
        def index(self, q: Any, start: int | None, stop: int | None) -> None:
            args: tuple[Any, ...] = (q,) if start is None else ((q, start) if stop is None else (q, start, stop))
            assert_same_outcome(f"index{args!r}", outcome(lambda: self.model.index(*args)), outcome(lambda: self.subject.index(*args)))

        @rule(q=queries)
        def count_and_contains(self, q: Any) -> None:
            assert_same_outcome(f"count({q!r})", outcome(lambda: self.model.count(q)), outcome(lambda: self.subject.count(q)))
            assert_same_outcome(f"{q!r} in", outcome(lambda: q in self.model), outcome(lambda: q in self.subject))

        @precondition(lambda self: subject_kind != "builtin")
        @rule()
        def kernels(self) -> None:
            # Python semantics, exactly: compensated float `sum`, first-wins `min`/`max`
            # (also with NaN), exact int sums beyond i64, TypeError/ValueError cases.
            for name, builtin_f in (("sum", sum), ("min", min), ("max", max)):
                m = outcome(lambda: builtin_f(self.model))
                s = outcome(lambda: getattr(self.subject, name)())
                assert_same_outcome(f"{name}()", m, s)

        @rule()
        def copy(self) -> None:
            c = self.subject.copy()
            assert type(c) is type(self.subject)
            assert c is not self.subject
            assert_same_items(c, self.model)
            if len(c):
                c.pop()
                assert len(self.subject) == len(self.model)

        @rule(xs=st.lists(clean_values, max_size=4), reflected=st.booleans())
        def add(self, xs: list[Any], reflected: bool) -> None:
            other = self.coerce_all(xs)
            if reflected:
                m = outcome(lambda: other + self.model)
                s = outcome(lambda: other + self.subject)
            else:
                m = outcome(lambda: self.model + other)
                s = outcome(lambda: self.subject + other)
            if s[1] is None:
                assert type(s[0]) is type(self.subject)
            assert_same_outcome("+", m, s)

        @precondition(lambda self: len(self.model) <= 24)
        @rule(n=st.integers(-1, 3), reflected=st.booleans())
        def mul(self, n: int, reflected: bool) -> None:
            if reflected:
                assert_same_outcome("n*", outcome(lambda: n * self.model), outcome(lambda: n * self.subject))
            else:
                assert_same_outcome("*n", outcome(lambda: self.model * n), outcome(lambda: self.subject * n))

        @precondition(lambda self: not has_nan(self.model))
        @rule(
            op=st.sampled_from([operator.eq, operator.ne, operator.lt, operator.le, operator.gt, operator.ge]),
            variant=st.sampled_from(["copy", "prefix", "longer", "changed", "random", "subject"]),
            xs=st.lists(clean_values, max_size=4),
            i=st.integers(0, 20),
        )
        def compare(self, op: Callable[[Any, Any], Any], variant: str, xs: list[Any], i: int) -> None:
            other = list(self.model)
            if variant == "prefix":
                other = other[: i % (len(other) + 1)]
            elif variant == "longer":
                other = other + self.coerce_all(xs)
            elif variant == "changed" and other and xs:
                other[i % len(other)] = coerce(xs[0])
            elif variant == "random":
                other = self.coerce_all(xs)
            operand = make(other) if variant == "subject" else other
            assert_same_outcome(f"{op.__name__} {variant}", outcome(lambda: op(self.model, other)), outcome(lambda: op(self.subject, operand)))
            assert_same_outcome(f"{op.__name__} {variant} (reflected)", outcome(lambda: op(other, self.model)), outcome(lambda: op(operand, self.subject)))

        @rule(k=st.integers(0, 4), x=clean_values)
        def iterate_then_mutate(self, k: int, x: Any) -> None:
            it_m, it_s = iter(self.model), iter(self.subject)
            for _ in range(k):
                assert_same_outcome("next", outcome(lambda: next(it_m)), outcome(lambda: next(it_s)))
            self.model.append(coerce(x))
            self.subject.append(x)
            assert_same_items(it_s, list(it_m))

        # ---- invariants ----------------------------------------------------------------

        @invariant()
        def agree(self) -> None:
            assert len(self.subject) == len(self.model)
            assert_same_items(iter(self.subject), self.model)
            assert_same_items(reversed(self.subject), self.model[::-1])
            if subject_kind == "builtin":
                return
            assert_same_items(self.subject.to_list(), self.model)
            if not has_nan(self.model):
                assert self.subject == self.model
                assert not (self.subject != self.model)
            strategy = self.subject.strategy
            assert strategy in LIST_STRATEGIES, strategy
            if strategy not in ("object", "empty"):
                bad = [x for x in self.model if not native_strategy_fits(strategy, x)]
                assert not bad, f"strategy {strategy!r} cannot hold {bad!r}"
            if strategy == "empty":
                assert not self.model
            if typed:
                pinned = {int: ("i64", "object"), float: ("f64",), bool: ("bool",), str: ("str",)}.get(ek.typed, ("object",))
                assert strategy in pinned, f"typed {ek.typed} uses strategy {strategy!r}"
            else:
                if tightest_strategy(self.model) == "object":
                    assert strategy == "object", f"{self.model!r} needs object storage, got {strategy!r}"
                if self.was_object:
                    assert strategy == "object", "strategies never re-promote (not even after clear())"
                self.was_object = strategy == "object"

    ListMachine.__name__ = f"ListMachine_{subject_kind}_{ek.name}"
    return ListMachine


@pytest.mark.parametrize("subject_kind,ek", _params(LIST_KINDS))
def test_list_differential(subject_kind: str, ek: ElemKind) -> None:
    run_state_machine_as_test(build_list_machine(subject_kind, ek), settings=settings())


# --------------------------------------------------------------------------------------
# Dict machine
# --------------------------------------------------------------------------------------


@dataclass(frozen=True)
class KeyKind:
    name: str
    keys: Any  # strategy for untyped keys
    typed: Any = None
    typed_keys: Any = None
    invalid: Any = None
    coerce: Callable[[Any], Any] = field(default=lambda x: x)
    probes: Any = None  # query strategy beyond the keys themselves


def _dict_key_kinds() -> list[KeyKind]:
    return [
        KeyKind(
            "int",
            INTS,
            typed=int,
            typed_keys=st.one_of(INTS, BOOLS),
            invalid=st.sampled_from(["1", 1.5, None, (1,)]),
            coerce=_to_int,
            probes=st.sampled_from(["1", None, (1,), b"1"]),
        ),
        KeyKind("bigint", BIG_INTS, typed=int, typed_keys=BIG_INTS, invalid=st.sampled_from(["1", 2.5]), coerce=_to_int, probes=st.sampled_from(["x", None])),
        KeyKind(
            "str",
            st.one_of(STRS, SURROGATES),
            typed=str,
            typed_keys=STRS,
            invalid=st.sampled_from([1, None, b"a"]),
            coerce=_to_str,
            probes=st.sampled_from([1, None, b"a", 0.5]),
        ),
        KeyKind(
            "float",
            FLOATS_NO_NAN,
            typed=float,
            typed_keys=st.one_of(FLOATS_NO_NAN, SMALL_EXACT_INTS),
            invalid=st.sampled_from(["1.0", None]),
            coerce=_to_float,
            probes=st.sampled_from([1, 0, -0.0, 2**53, "x", None]),
        ),
        KeyKind(
            "float-dups",
            FLOAT_DUPS,
            typed=float,
            typed_keys=st.one_of(FLOAT_DUPS, st.integers(-1, 1)),
            invalid=st.sampled_from(["0.0", None]),
            coerce=_to_float,
            probes=st.sampled_from([0, 1, -1, "x", None]),
        ),
        KeyKind("bool", BOOLS, typed=bool, typed_keys=BOOLS, invalid=st.sampled_from([0, 1, "x", None]), coerce=_to_bool, probes=st.sampled_from(["x", None, 0.5])),
        KeyKind("numeric-mix", st.one_of(st.integers(-3, 3), st.integers(-3, 3).map(float), BOOLS, st.sampled_from([0.5, -0.0, 2**64]))),
        KeyKind("mixed", MIXED_HASHABLE_NO_NAN),
    ]


DICT_KINDS = _dict_key_kinds()
DICT_VALUES = st.one_of(INTS, FLOATS_NO_NAN, STRS, st.none(), BOOLS, st.lists(st.integers(0, 2), max_size=2))
TYPED_DICT_VALUES = st.one_of(INTS, INTS, BOOLS, st.sampled_from(["v", None, 1.5]))


SET_OPS = {
    "|": operator.or_,
    "&": operator.and_,
    "-": operator.sub,
    "^": operator.xor,
}
SET_METHODS = ["union", "intersection", "difference", "symmetric_difference"]
SET_UPDATES = ["update", "intersection_update", "difference_update", "symmetric_difference_update"]
SET_IOPS = {"|=": operator.ior, "&=": operator.iand, "-=": operator.isub, "^=": operator.ixor}
SET_PREDICATES = ["issubset", "issuperset", "isdisjoint"]
SET_CMPS = [operator.eq, operator.ne, operator.lt, operator.le, operator.gt, operator.ge]


def build_dict_machine(subject_kind: str, kk: KeyKind) -> type[RuleBasedStateMachine]:
    typed = subject_kind == "typed"
    rs = load_rustique() if subject_kind != "builtin" else None
    kco: Callable[[Any], Any] = kk.coerce if typed else (lambda x: x)
    vco: Callable[[Any], Any] = _to_int if typed else (lambda x: x)
    valid_keys = kk.typed_keys if typed else kk.keys
    keys = st.one_of(valid_keys, valid_keys, valid_keys, kk.invalid) if typed else valid_keys
    vals = TYPED_DICT_VALUES if typed else DICT_VALUES
    valid_vals = INTS if typed else DICT_VALUES
    # Lookups: valid keys, unambiguous foreign probes and unhashables. Never a value that is
    # numerically equal to a key but rejected by the key schema (unspecified, see test_typed).
    queries = st.one_of(valid_keys, kk.probes if kk.probes is not None else valid_keys, UNHASHABLE)

    def make(arg: Any = (), **kwargs: Any) -> Any:
        if subject_kind == "builtin":
            return dict(arg, **kwargs)
        assert rs is not None
        if typed:
            return rs.Dict[kk.typed, int](arg, **kwargs)
        return rs.Dict(arg, **kwargs)

    def items_of(d: Any) -> list[tuple[Any, Any]]:
        return list(d.items())

    def check_dict_result(ctx: str, got: Any, expected: dict[Any, Any], lenient: bool) -> None:
        got_items = items_of(got)
        exp_items = list(expected.items())
        assert len(got_items) == len(exp_items), f"{ctx}: {got_items!r} vs {exp_items!r}"
        if lenient:
            assert dict(got_items) == expected, f"{ctx}: {got_items!r} vs {exp_items!r}"
        else:
            assert_same_items([k for k, _ in got_items], [k for k, _ in exp_items])
            assert_same_items([v for _, v in got_items], [v for _, v in exp_items])

    def valid_pair(k: Any, v: Any) -> bool:
        return outcome(lambda: (kco(k), vco(v)))[1] is None

    class DictMachine(RuleBasedStateMachine):
        model: dict[Any, Any]
        subject: Any

        @initialize(init=st.lists(st.tuples(valid_keys, valid_vals), max_size=8), form=st.sampled_from(["dict", "pairs", "kwargs", "subject"]))
        def init(self, init: list[tuple[Any, Any]], form: str) -> None:
            if form == "kwargs" and (not typed or kk.typed is str):
                init = [(k, v) for k, v in init if type(k) is str and k.isidentifier()]
                self.subject = make(**dict(init))
            elif form == "pairs":
                self.subject = make(list(init))
            elif form == "subject":
                self.subject = make(make(init))
            else:
                self.subject = make(dict(init))
            self.model = {}
            for k, v in (init if form == "pairs" else dict(init).items()):
                self.model[kco(k)] = vco(v)

        def mutate(
            self,
            ctx: str,
            model_op: Callable[[dict[Any, Any]], Any],
            subject_op: Callable[[Any], Any],
            also_ok: tuple[type[BaseException], ...] = (),
        ) -> None:
            before = list(self.model.items())
            m = outcome(lambda: model_op(self.model))
            s = outcome(lambda: subject_op(self.subject))
            note(f"{ctx}: builtin -> {m!r}; subject -> {s!r}")
            if s[1] is not None and isinstance(s[1], also_ok):
                assert_same_items(list(self.subject.items()), before)
                return
            assert_same_outcome(ctx, m, s)

        # ---- writes --------------------------------------------------------------------

        @rule(k=keys, v=vals)
        def setitem(self, k: Any, v: Any) -> None:
            def model_op(m: dict[Any, Any]) -> None:
                ck, cv = kco(k), vco(v)
                m[ck] = cv

            self.mutate(f"[{k!r}] = {v!r}", model_op, lambda s: s.__setitem__(k, v))

        @precondition(lambda self: len(self.model) > 0)
        @rule(data=st.data(), v=vals)
        def overwrite_existing(self, data: Any, v: Any) -> None:
            k = data.draw(st.sampled_from(list(self.model)))
            self.mutate(f"[existing {k!r}] = {v!r}", lambda m: m.__setitem__(k, vco(v)), lambda s: s.__setitem__(k, v))

        @rule(v=vals)
        def setitem_unhashable(self, v: Any) -> None:
            bad: list[Any] = [1]
            self.mutate("[[1]] = v", lambda m: m.__setitem__(bad, v), lambda s: s.__setitem__(bad, v))

        @rule(k=st.one_of(valid_keys, kk.probes if kk.probes is not None else valid_keys), v=valid_vals)
        def setdefault(self, k: Any, v: Any) -> None:
            def model_op(m: dict[Any, Any]) -> Any:
                if k in m:
                    return m[k]
                ck, cv = kco(k), vco(v)
                m[ck] = cv
                return cv

            self.mutate(f"setdefault({k!r}, {v!r})", model_op, lambda s: s.setdefault(k, v))

        @rule(pairs=st.lists(st.tuples(keys, vals), max_size=4), form=st.sampled_from(["dict", "pairs", "subject", "kwargs"]))
        def update(self, pairs: list[tuple[Any, Any]], form: str) -> None:
            if form == "kwargs":
                kw = {k: v for k, v in pairs if type(k) is str and k.isidentifier()}
                items = list(kw.items())

                def call(s: Any) -> None:
                    s.update(**kw)

            elif form == "pairs":
                items = list(pairs)

                def call(s: Any) -> None:
                    s.update(list(pairs))

            else:
                if form == "subject":
                    pairs = [(k, v) for k, v in pairs if valid_pair(k, v)]
                items = list(dict(pairs).items())
                operand = dict(pairs) if form == "dict" else make(pairs)

                def call(s: Any) -> None:
                    s.update(operand)

            before = dict(self.model)

            def model_op(m: dict[Any, Any]) -> None:
                validated = [(kco(k), vco(v)) for k, v in items]  # validate all, then apply
                for k, v in validated:
                    m[k] = v

            m = outcome(lambda: model_op(self.model))
            s = outcome(lambda: call(self.subject))
            note(f"update({form} {pairs!r}): builtin -> {m!r}; subject -> {s!r}")
            if m[1] is not None and s[1] is not None and type(m[1]) is type(s[1]):
                # A typed update rejected part-way may have applied a prefix: resync.
                got = dict(items_of(self.subject))
                assert all(k in got for k in before), f"update lost keys: {got!r} vs {before!r}"
                self.model = got
                return
            assert_same_outcome(f"update({form})", m, s)

        @rule(pairs=st.lists(st.tuples(valid_keys, valid_vals), max_size=3), form=st.sampled_from(["dict", "subject"]))
        def ior(self, pairs: list[tuple[Any, Any]], form: str) -> None:
            operand = dict(pairs) if form == "dict" else make(pairs)

            def subject_op(s: Any) -> None:
                before = s
                s |= operand
                assert s is before, "|= must update in place"

            def model_op(m: dict[Any, Any]) -> None:
                for k, v in dict(pairs).items():
                    m[kco(k)] = vco(v)

            self.mutate(f"|= {form} {pairs!r}", model_op, subject_op)

        # ---- removal -------------------------------------------------------------------

        @rule(q=queries)
        def delitem(self, q: Any) -> None:
            self.mutate(f"del [{q!r}]", lambda m: m.__delitem__(q), lambda s: s.__delitem__(q))

        @rule(q=queries, with_default=st.booleans())
        def pop(self, q: Any, with_default: bool) -> None:
            args = (q, "dflt") if with_default else (q,)
            # CPython quirk: `{}.pop(unhashable)` short-cuts on the empty dict (KeyError or
            # the default) before hashing; raising TypeError there is equally fine.
            quirk = (TypeError,) if not self.model and outcome(lambda: hash(q))[1] is not None else ()
            self.mutate(f"pop{args!r}", lambda m: m.pop(*args), lambda s: s.pop(*args), also_ok=quirk)

        @precondition(lambda self: len(self.model) > 0)
        @rule(data=st.data(), with_default=st.booleans())
        def pop_existing(self, data: Any, with_default: bool) -> None:
            k = data.draw(st.sampled_from(list(self.model)))
            args = (k, "dflt") if with_default else (k,)
            self.mutate(f"pop{args!r}", lambda m: m.pop(*args), lambda s: s.pop(*args))

        @rule()
        def popitem(self) -> None:
            self.mutate("popitem()", lambda m: m.popitem(), lambda s: s.popitem())

        @rule()
        def clear(self) -> None:
            self.mutate("clear()", lambda m: m.clear(), lambda s: s.clear())

        # ---- reads ---------------------------------------------------------------------

        @rule(q=queries)
        def lookups(self, q: Any) -> None:
            for ctx, f in (
                ("getitem", lambda d: d[q]),
                ("contains", lambda d: q in d),
                ("get", lambda d: d.get(q)),
                ("get-default", lambda d: d.get(q, "dflt")),
                ("keys-contains", lambda d: q in d.keys()),
            ):
                assert_same_outcome(f"{ctx}({q!r})", outcome(lambda: f(self.model)), outcome(lambda: f(self.subject)))

        @precondition(lambda self: len(self.model) > 0)
        @rule(data=st.data())
        def lookup_existing(self, data: Any) -> None:
            k = data.draw(st.sampled_from(list(self.model)))
            assert_same_outcome(f"[{k!r}]", outcome(lambda: self.model[k]), outcome(lambda: self.subject[k]))
            v = self.model[k]
            assert_same_outcome("(k, v) in items()", outcome(lambda: (k, v) in self.model.items()), outcome(lambda: (k, v) in self.subject.items()))
            assert_same_outcome("v in values()", outcome(lambda: v in self.model.values()), outcome(lambda: v in self.subject.values()))

        @rule(xs=st.lists(valid_keys, max_size=4), op=st.sampled_from(["&", "|", "-", "^"]))
        def keys_view_set_ops(self, xs: list[Any], op: str) -> None:
            other = set(kco(x) for x in xs)
            f = SET_OPS[op]
            expected = f(self.model.keys(), other)
            got = f(self.subject.keys(), other)
            lenient = cross_type_equal(list(self.model) + list(other))
            got_list = list(got)
            assert len(got_list) == len(expected), f"keys() {op}: {got_list!r} vs {expected!r}"
            if lenient:
                assert set(got_list) == expected
            else:
                assert multiset(got_list) == multiset(expected), f"keys() {op}: {got_list!r} vs {expected!r}"

        @rule()
        def copy(self) -> None:
            c = self.subject.copy()
            assert c is not self.subject
            assert type(c) is type(self.subject)
            check_dict_result("copy", c, self.model, lenient=False)
            c.clear()
            assert len(self.subject) == len(self.model)

        @rule(pairs=st.lists(st.tuples(valid_keys, valid_vals), max_size=3), form=st.sampled_from(["dict", "subject"]), reflected=st.booleans())
        def or_(self, pairs: list[tuple[Any, Any]], form: str, reflected: bool) -> None:
            pairs = [(kco(k), vco(v)) for k, v in pairs]
            other_model = dict(pairs)
            operand = dict(pairs) if form == "dict" else make(pairs)
            if reflected:
                expected = dict(other_model)
                expected.update(self.model)
                got = outcome(lambda: operand | self.subject)
            else:
                expected = dict(self.model)
                expected.update(other_model)
                got = outcome(lambda: self.subject | operand)
            assert got[1] is None, f"| raised {got[1]!r}"
            if subject_kind != "builtin":
                assert type(got[0]) is type(self.subject), type(got[0])
            check_dict_result("|", got[0], expected, cross_type_equal(list(self.model) + list(other_model)))

        @rule(
            variant=st.sampled_from(["copy", "changed", "missing", "extra", "reordered"]),
            form=st.sampled_from(["dict", "subject"]),
            k=valid_keys,
            v=valid_vals,
        )
        def compare(self, variant: str, form: str, k: Any, v: Any) -> None:
            other = dict(self.model)
            if variant == "changed" and other:
                k0 = next(iter(other))
                other[k0] = vco(v)
            elif variant == "missing" and other:
                del other[next(iter(other))]
            elif variant == "extra":
                ck = kco(k)
                if ck in other:
                    return
                other[ck] = vco(v)
            elif variant == "reordered":
                other = dict(reversed(list(other.items())))
            operand = make(other) if form == "subject" else other
            for op in (operator.eq, operator.ne, operator.lt):
                assert_same_outcome(f"{op.__name__} {variant}", outcome(lambda: op(self.model, other)), outcome(lambda: op(self.subject, operand)))
                assert_same_outcome(f"{op.__name__} {variant} reflected", outcome(lambda: op(other, self.model)), outcome(lambda: op(operand, self.subject)))
            for foreign in (list(self.model.items()), None, 1):
                assert_same_outcome("== foreign", outcome(lambda: self.model == foreign), outcome(lambda: self.subject == foreign))

        @precondition(lambda self: len(self.model) > 0)
        @rule(k=valid_keys, v=valid_vals, change=st.sampled_from(["add", "delete", "overwrite"]))
        def iterate_then_mutate(self, k: Any, v: Any, change: str) -> None:
            it_m, it_s = iter(self.model), iter(self.subject)
            assert_same_outcome("next", outcome(lambda: next(it_m)), outcome(lambda: next(it_s)))
            first = next(iter(self.model))
            if change == "add":
                ck = kco(k)
                if ck in self.model:
                    return
                self.model[ck] = vco(v)
                self.subject[k] = v
            elif change == "delete":
                del self.model[first]
                del self.subject[first]
            else:
                self.model[first] = vco(v)
                self.subject[first] = v
            assert_same_outcome(f"iterate after {change}", outcome(lambda: list(it_m)), outcome(lambda: list(it_s)))

        @invariant()
        def agree(self) -> None:
            assert len(self.subject) == len(self.model)
            assert_same_items(list(self.subject), list(self.model))
            assert_same_items(list(self.subject.keys()), list(self.model.keys()))
            assert_same_items(list(self.subject.values()), list(self.model.values()))
            assert_same_items(list(self.subject.items()), list(self.model.items()))
            assert len(self.subject.keys()) == len(self.subject.values()) == len(self.subject.items()) == len(self.model)
            if subject_kind != "builtin":
                d = self.subject.to_dict()
                assert type(d) is dict
                assert_same_items(list(d.items()), list(self.model.items()))
                assert self.subject == self.model
                assert self.model == self.subject
                assert not (self.subject != self.model)

    DictMachine.__name__ = f"DictMachine_{subject_kind}_{kk.name}"
    return DictMachine


def cross_type_equal(items: Iterable[Any]) -> bool:
    """True if two of ``items`` are equal (same hash bucket) but not identical in type or
    float sign -- the cases where which representative survives is an implementation
    detail of a set/dict operation."""
    rep: dict[Any, Any] = {}
    for x in items:
        try:
            if x in rep:
                if not same(rep[x], x):
                    return True
            else:
                rep[x] = x
        except TypeError:
            continue
    return False


@pytest.mark.parametrize("subject_kind,kk", [pytest.param(sk, kk, id=f"{sk}-{kk.name}") for sk in _subject_kinds() for kk in DICT_KINDS if sk != "typed" or kk.typed is not None])
def test_dict_differential(subject_kind: str, kk: KeyKind) -> None:
    run_state_machine_as_test(build_dict_machine(subject_kind, kk), settings=settings())


# --------------------------------------------------------------------------------------
# Set machine
# --------------------------------------------------------------------------------------

def build_set_machine(subject_kind: str, kk: KeyKind) -> type[RuleBasedStateMachine]:
    typed = subject_kind == "typed"
    rs = load_rustique() if subject_kind != "builtin" else None
    co: Callable[[Any], Any] = kk.coerce if typed else (lambda x: x)
    valid = kk.typed_keys if typed else kk.keys
    elems = st.one_of(valid, valid, valid, kk.invalid) if typed else valid
    queries = st.one_of(valid, kk.probes if kk.probes is not None else valid, UNHASHABLE)

    def make(items: Iterable[Any] = ()) -> Any:
        if subject_kind == "builtin":
            return set(items)
        assert rs is not None
        if typed:
            return rs.Set[kk.typed](items)
        return rs.Set(items)

    def as_operand(items: list[Any], form: str) -> Any:
        if form == "set":
            return set(items)
        if form == "frozenset":
            return frozenset(items)
        if form == "list":
            return list(items)
        return make(items)

    def check_set(ctx: str, got: Any, expected: set[Any], lenient: bool) -> None:
        got_items = list(got)
        assert len(got_items) == len(expected), f"{ctx}: {got_items!r} vs {expected!r}"
        if lenient:
            assert set(got_items) == expected, f"{ctx}: {got_items!r} vs {expected!r}"
        else:
            assert multiset(got_items) == multiset(expected), f"{ctx}: {got_items!r} vs {expected!r}"

    class SetMachine(RuleBasedStateMachine):
        model: set[Any]
        subject: Any

        @initialize(init=st.lists(valid, max_size=8), form=st.sampled_from(["list", "set", "gen", "subject"]))
        def init(self, init: list[Any], form: str) -> None:
            self.model = set()
            for x in init:
                self.model.add(co(x))
            src: Any = {"list": lambda: list(init), "set": lambda: set(init), "gen": lambda: iter(init), "subject": lambda: make(init)}[form]()
            self.subject = make(src)
            if form == "set":  # set(init) may keep a different representative of equal items
                self.resync_if_lenient(cross_type_equal(init))

        def resync_if_lenient(self, lenient: bool) -> None:
            if lenient:
                check_set("resync", self.subject, self.model, lenient=True)
                self.model = set(self.subject)

        def mutate(self, ctx: str, model_op: Callable[[set[Any]], Any], subject_op: Callable[[Any], Any]) -> None:
            m = outcome(lambda: model_op(self.model))
            s = outcome(lambda: subject_op(self.subject))
            note(f"{ctx}: builtin -> {m!r}; subject -> {s!r}")
            assert_same_outcome(ctx, m, s)

        @rule(x=elems)
        def add(self, x: Any) -> None:
            self.mutate(f"add({x!r})", lambda m: m.add(co(x)), lambda s: s.add(x))

        @rule(q=queries)
        def discard(self, q: Any) -> None:
            self.mutate(f"discard({q!r})", lambda m: m.discard(q), lambda s: s.discard(q))

        @rule(q=queries)
        def remove(self, q: Any) -> None:
            self.mutate(f"remove({q!r})", lambda m: m.remove(q), lambda s: s.remove(q))

        @rule(q=queries)
        def contains(self, q: Any) -> None:
            assert_same_outcome(f"{q!r} in", outcome(lambda: q in self.model), outcome(lambda: q in self.subject))

        @rule()
        def pop(self) -> None:
            if not self.model:
                assert_same_outcome("pop()", outcome(self.model.pop), outcome(self.subject.pop))
                return
            x = self.subject.pop()
            match = [y for y in self.model if y == x]
            assert len(match) == 1, f"pop() returned {x!r}, not in {self.model!r}"
            assert same(match[0], x), f"pop() returned {x!r}, stored {match[0]!r}"
            self.model.discard(x)

        @rule()
        def clear(self) -> None:
            self.mutate("clear()", lambda m: m.clear(), lambda s: s.clear())

        @rule(xs=st.lists(valid, max_size=5), name=st.sampled_from(SET_UPDATES), form=st.sampled_from(["set", "list", "subject", "frozenset"]))
        def update_method(self, xs: list[Any], name: str, form: str) -> None:
            cx = [co(x) for x in xs]
            operand = as_operand(xs, form)
            # Which of two equal-but-differently-typed items survives an intersection is a
            # CPython implementation detail (it depends on operand sizes).
            lenient = name == "intersection_update" and cross_type_equal(list(self.model) + cx)
            if form in ("set", "frozenset", "subject"):
                lenient = lenient or cross_type_equal(xs)
            self.mutate(f"{name}({form} {xs!r})", lambda m: getattr(m, name)(cx), lambda s: getattr(s, name)(operand))
            self.resync_if_lenient(lenient)

        @rule(x=elems, name=st.sampled_from(["update", "symmetric_difference_update"]))
        def update_with_invalid(self, x: Any, name: str) -> None:
            self.mutate(f"{name}([{x!r}])", lambda m: getattr(m, name)([co(x)]), lambda s: getattr(s, name)([x]))

        @rule(xs=st.lists(valid, max_size=5), op=st.sampled_from(sorted(SET_IOPS)), form=st.sampled_from(["set", "subject", "frozenset"]))
        def inplace_operator(self, xs: list[Any], op: str, form: str) -> None:
            cx = [co(x) for x in xs]
            lenient = cross_type_equal(xs) or (op == "&=" and cross_type_equal(list(self.model) + cx))
            f = SET_IOPS[op]
            self.model = f(self.model, set(cx))
            before = self.subject
            self.subject = f(self.subject, as_operand(xs, form))
            assert self.subject is before, f"{op} must update in place"
            self.resync_if_lenient(lenient)

        @rule(xs=st.lists(valid, max_size=5), op=st.sampled_from(sorted(SET_OPS)), form=st.sampled_from(["set", "subject", "frozenset"]), reflected=st.booleans())
        def binary_operator(self, xs: list[Any], op: str, form: str, reflected: bool) -> None:
            cx = set(co(x) for x in xs)
            f = SET_OPS[op]
            operand = as_operand(xs, form)
            if reflected:
                expected = f(cx, self.model)
                got = outcome(lambda: f(operand, self.subject))
            else:
                expected = f(self.model, cx)
                got = outcome(lambda: f(self.subject, operand))
            assert got[1] is None, f"{op} raised {got[1]!r}"
            # A typed Set combined with a builtin operand may yield an untyped result that
            # keeps the operand's own elements (the List `+` rule): compare by equality.
            lenient = cross_type_equal(list(self.model) + list(xs)) or (typed and form != "subject")
            check_set(op, got[0], expected, lenient)
            if subject_kind != "builtin":
                assert type(got[0]) is type(self.subject), f"{op}: result type {type(got[0])}"

        @rule(xs=st.lists(valid, max_size=5), name=st.sampled_from(SET_METHODS), form=st.sampled_from(["set", "list", "subject"]), n_args=st.integers(0, 2))
        def method(self, xs: list[Any], name: str, form: str, n_args: int) -> None:
            if name == "symmetric_difference":
                n_args = 1  # takes exactly one argument
            cx = [co(x) for x in xs]
            halves = [cx[: len(cx) // 2], cx[len(cx) // 2 :]][:n_args]
            raw = [xs[: len(xs) // 2], xs[len(xs) // 2 :]][:n_args]
            expected = getattr(self.model, name)(*halves)
            got = getattr(self.subject, name)(*[as_operand(r, form) for r in raw])
            check_set(name, got, expected, cross_type_equal(list(self.model) + list(xs)) or (typed and form != "subject"))
            assert type(got) is type(self.subject)

        @rule(xs=st.lists(valid, max_size=5), name=st.sampled_from(SET_PREDICATES), form=st.sampled_from(["set", "list", "subject"]))
        def predicate(self, xs: list[Any], name: str, form: str) -> None:
            cx = [co(x) for x in xs]
            assert_same_outcome(name, outcome(lambda: getattr(self.model, name)(cx)), outcome(lambda: getattr(self.subject, name)(as_operand(xs, form))))

        @rule(variant=st.sampled_from(["copy", "subset", "superset", "random"]), xs=st.lists(valid, max_size=3), form=st.sampled_from(["set", "frozenset", "subject"]))
        def compare(self, variant: str, xs: list[Any], form: str) -> None:
            cx = [co(x) for x in xs]
            if variant == "copy":
                other = set(self.model)
            elif variant == "subset":
                other = set(list(self.model)[: len(cx)])
            elif variant == "superset":
                other = self.model | set(cx)
            else:
                other = set(cx)
            operand = as_operand(list(other), form)
            for op in SET_CMPS:
                assert_same_outcome(f"{op.__name__} {variant}", outcome(lambda: op(self.model, other)), outcome(lambda: op(self.subject, operand)))
                assert_same_outcome(f"{op.__name__} {variant} reflected", outcome(lambda: op(other, self.model)), outcome(lambda: op(operand, self.subject)))
            for foreign in ([1], (1,), None):
                assert_same_outcome("== foreign", outcome(lambda: self.model == foreign), outcome(lambda: self.subject == foreign))
                assert_same_outcome("<= foreign", outcome(lambda: self.model <= foreign), outcome(lambda: self.subject <= foreign))

        @rule()
        def copy(self) -> None:
            c = self.subject.copy()
            assert c is not self.subject and type(c) is type(self.subject)
            check_set("copy", c, self.model, lenient=False)
            c.clear()
            assert len(self.subject) == len(self.model)

        @invariant()
        def agree(self) -> None:
            assert len(self.subject) == len(self.model)
            check_set("state", self.subject, self.model, lenient=False)
            if subject_kind != "builtin":
                s = self.subject.to_set()
                assert type(s) is set
                check_set("to_set", s, self.model, lenient=False)
                assert self.subject == self.model
                assert self.model == self.subject
                assert self.subject == frozenset(self.model)
                assert not (self.subject != self.model)

    SetMachine.__name__ = f"SetMachine_{subject_kind}_{kk.name}"
    return SetMachine


@pytest.mark.parametrize("subject_kind,kk", [pytest.param(sk, kk, id=f"{sk}-{kk.name}") for sk in _subject_kinds() for kk in DICT_KINDS if sk != "typed" or kk.typed is not None])
def test_set_differential(subject_kind: str, kk: KeyKind) -> None:
    run_state_machine_as_test(build_set_machine(subject_kind, kk), settings=settings())
