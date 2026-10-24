"""Regression tests for the Dict/Set/expression safety review (crashes, deadlocks under
container locks, livelocks, iteration semantics, expression limits).

Scenarios that used to crash or deadlock run in a subprocess with a timeout, so a
regression fails cleanly instead of taking the test run down with it.
"""

from __future__ import annotations

import os
import subprocess
import sys
import textwrap

import pytest

import rustique as rs

FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()


def run_isolated(code: str, *flags: str, timeout: float = 60, env: dict[str, str] | None = None) -> str:
    """Run `code` in a fresh interpreter; fail on a crash, a hang or a non-zero exit."""
    script = "import faulthandler; faulthandler.enable()\n" + textwrap.dedent(code)
    try:
        proc = subprocess.run(
            [sys.executable, *flags, "-c", script],
            capture_output=True,
            text=True,
            timeout=timeout,
            env={**os.environ, **(env or {})},
        )
    except subprocess.TimeoutExpired as e:
        pytest.fail(f"timed out after {timeout}s (deadlock?)\nstdout: {e.stdout}\nstderr: {e.stderr}")
    assert proc.returncode == 0, f"exit {proc.returncode}\nstdout: {proc.stdout}\nstderr: {proc.stderr}"
    return proc.stdout


# --- C1: no conversion that may run Python code inside a source's critical section ----------


@pytest.mark.parametrize("mode", ["dict", "list", "f32"])
def test_update_from_builtin_never_converts_borrowed_subclass_values(mode):
    out = run_isolated(
        f"""
        import rustique as rs

        class Evil(int):
            def __str__(self):
                src.clear()
                return "evil"

            __repr__ = __str__

        class EvilF(float):
            def __repr__(self):
                src.clear()
                return "evilf"

            __str__ = __repr__

        mode = {mode!r}
        if mode == "dict":
            src, d = {{"a": Evil(1000)}}, rs.Dict[str, rs.i8]()
        elif mode == "list":
            src, d = [("a", Evil(1000))], rs.Dict[str, rs.i8]()
        else:
            src, d = {{"a": EvilF(1e300)}}, rs.Dict[str, rs.f32]()
        try:
            d.update(src)
        except OverflowError:
            print("overflow")
        print(len(d))
        """,
        env={"PYTHONMALLOC": "debug"},
    )
    assert out.split() == ["overflow", "0"]


def test_update_from_builtin_still_converts_valid_values():
    d = rs.Dict[str, rs.i8]()
    d.update({"a": 1, "b": -128})
    d.update([("c", 127), ("d", True)])
    assert d.to_dict() == {"a": 1, "b": -128, "c": 127, "d": 1}

    class Small(int):
        pass

    d.update({"e": Small(5)})
    assert d["e"] == 5
    with pytest.raises(OverflowError):
        d.update({"f": 300})
    u = rs.Dict({"x": 1.5, "y": "s", "z": None})
    assert u.to_dict() == {"x": 1.5, "y": "s", "z": None}


# --- H1: Python objects are never released while a container lock is held -------------------


def test_setdefault_releases_an_unused_default_after_the_lock():
    out = run_isolated(
        """
        import rustique as rs

        d = rs.Dict[str, rs.List]()

        class Bomb:
            def __del__(self):
                print("del", len(d), flush=True)  # needs d's lock

        def gen():
            d["k"] = []  # the key appears while the default is being converted
            yield Bomb()

        print("got", d.setdefault("k", gen()), flush=True)
        """,
        timeout=30,
    )
    assert out.split() == ["del", "1", "got", "List([])"]


def test_failed_batch_releases_pending_items_after_the_lock():
    # Under -bb the bytes/str comparison raises; the value in flight and the items not yet
    # inserted are released (finalizers touching the dict run) only after the lock.
    out = run_isolated(
        """
        import rustique as rs

        d = rs.Dict({b"a": 1})

        class Bomb:
            def __del__(self):
                print("del", len(d), flush=True)

        try:
            d.update(iter([("a", Bomb()), ("z", Bomb())]))
        except BytesWarning:
            print("BytesWarning", flush=True)
        print("len", len(d))
        """,
        "-bb",
        timeout=30,
    )
    assert out.split() == ["del", "1", "del", "1", "BytesWarning", "len", "1"]


def test_failed_eq_mid_batch_keeps_prefix_and_releases_rest():
    class BadEq:
        def __hash__(self):
            return hash("x")

        def __eq__(self, other):
            raise ValueError("no eq")

    released = []

    class Value:
        def __init__(self, n):
            self.n = n

        def __del__(self):
            released.append((self.n, len(d)))

    d = rs.Dict({"x": 0})
    with pytest.raises(ValueError, match="no eq"):
        d.update([("a", Value(1)), (BadEq(), Value(2)), ("b", Value(3))])
    assert list(d) == ["x", "a"]
    assert sorted(released) == [(2, 2), (3, 2)]


# --- L1: bytes are never compared with str/int under a lock ---------------------------------


def test_bytes_str_comparison_warns_outside_the_lock():
    out = run_isolated(
        """
        import warnings
        import rustique as rs

        s = rs.Set([b"a"])
        d = rs.Dict({b"a": 1})
        t = rs.Set([(b"a", 1)])

        def hook(message, category, filename, lineno, file=None, line=None):
            print(category.__name__, len(s), len(d), len(t), flush=True)  # needs the locks

        warnings.showwarning = hook
        warnings.simplefilter("always")
        print("a" in s, flush=True)
        s.add("a")
        d["a"] = 2
        t.add(("a", 1))
        print(sorted(map(repr, s)), d.to_dict() == {b"a": 1, "a": 2}, len(t), flush=True)
        print(b"a" in s, 97 in rs.Set([b"a"]), flush=True)
        """,
        "-b",
        timeout=30,
    )
    lines = out.splitlines()
    assert lines[0] == "BytesWarning 1 1 1"
    assert lines[1] == "False"
    assert "[\"'a'\", \"b'a'\"] True 2" in lines
    assert lines[-1] == "True False"
    assert sum(line.startswith("BytesWarning") for line in lines) >= 4


def test_bytes_str_comparison_raises_under_bb_like_builtins():
    out = run_isolated(
        """
        import rustique as rs

        for container in (set, rs.Set):
            s = container([b"a"])
            try:
                "a" in s
            except BytesWarning:
                print(container.__name__, "raised")
        """,
        "-bb",
        timeout=30,
    )
    assert out.split() == ["set", "raised", "Set", "raised"]


# --- H2: lookups and intersection_update make progress under concurrent writers ------------

LOOKUP_UNDER_CHURN = """
import sys, threading, time
import rustique as rs

kind, eq_work = sys.argv[1], sys.argv[2]

class Key:
    def __init__(self, v):
        self.v = v
    def __hash__(self):
        return hash(self.v)
    def __eq__(self, other):
        if eq_work == "sleep":
            time.sleep(0.0005)
        else:
            sum(range(2000))
        return isinstance(other, Key) and self.v == other.v

stored = Key(1)
c = rs.Dict({stored: "x"}) if kind == "dict" else rs.Set([stored])
add = (lambda k: c.__setitem__(k, 1)) if kind == "dict" else c.add
rem = (lambda k: c.__delitem__(k)) if kind == "dict" else c.discard
stop = False

def mutator():
    i = 2
    while not stop:
        add(i)  # unrelated keys
        rem(i)
        i += 1

threading.Thread(target=mutator, daemon=True).start()
probe, done, deadline = Key(1), 0, time.monotonic() + 1.5
while time.monotonic() < deadline:
    assert probe in c
    done += 1
stop = True
print(done)
"""


@pytest.mark.parametrize("kind", ["dict", "set"])
@pytest.mark.parametrize("eq_work", ["sleep", "cpu"])
def test_object_key_lookups_progress_under_unrelated_writes(kind, eq_work):
    out = run_isolated(f"import sys; sys.argv[1:] = [{kind!r}, {eq_work!r}]\n" + LOOKUP_UNDER_CHURN, timeout=60)
    # Every comparison used to be restarted by the writer: zero lookups completed.
    assert int(out) >= 20


def test_intersection_update_finishes_under_concurrent_writes():
    out = run_isolated(
        """
        import threading, time
        import rustique as rs

        for n, make in ((200_000, lambda xs: xs), (20_000, lambda xs: [(x,) for x in xs])):
            s = rs.Set(make(range(n)))
            keep = rs.Set(make(range(0, n, 2)))
            stop = False

            def mutator():
                i = -1
                while not stop:
                    s.add(make([i])[0])
                    s.discard(make([i])[0])
                    i -= 1

            t = threading.Thread(target=mutator, daemon=True)
            t.start()
            time.sleep(0.05)
            s.intersection_update(keep)
            stop = True
            t.join()
            assert all(x in keep for x in s), "kept an element outside the intersection"
            assert len([x for x in s if x in keep]) == n // 2
            print("ok", n)
        """,
        timeout=120,
    )
    assert out.split() == ["ok", "200000", "ok", "20000"]


class SelfRemoving:
    """Equal to everything; comparing it removes the stored key it is compared with."""

    def __init__(self, container, victim):
        self.container, self.victim = container, victim

    def __hash__(self):
        return hash(self.victim)

    def __eq__(self, other):
        if isinstance(self.container, dict) or isinstance(self.container, rs.Dict):
            self.container.pop(self.victim, None)
        else:
            self.container.discard(self.victim)
        return True


class Tag:
    def __init__(self, name):
        self.name = name

    def __hash__(self):
        return 7

    def __eq__(self, other):
        return self.name == other.name if isinstance(other, Tag) else NotImplemented


@pytest.mark.parametrize("make", [dict.fromkeys, rs.Dict.fromkeys, set, rs.Set])
def test_matched_key_removed_during_comparison_is_not_found(make):
    victim = Tag("v")
    c = make([victim, 1])
    assert (SelfRemoving(c, victim) in c) is False
    assert list(c) == [1]


@pytest.mark.parametrize("make", [set, rs.Set])
def test_key_added_during_comparison_is_compared_too(make):
    first, second = Tag("a"), Tag("b")
    c = make([first])

    class Probe:
        def __hash__(self):
            return 7

        def __eq__(self, other):
            if other is first:
                c.add(second)
                return False
            return other is second

    assert Probe() in c
    assert len(c) == 2


def test_intersection_update_keeps_concurrent_insertions_and_element_sources():
    s = rs.Set([1.0, 2, "a", (1,)])
    ref = {1.0, 2, "a", (1,)}
    s.intersection_update([1, "a", (1,)])
    ref.intersection_update([1, "a", (1,)])
    assert sorted(map(repr, s)) == sorted(map(repr, ref))

    class Sneaky:
        def __init__(self, v):
            self.v = v

        def __hash__(self):
            return hash(self.v)

        def __eq__(self, other):
            if self.v == 3:
                s.add("late")  # an insertion while the intersection is being computed
            return isinstance(other, Sneaky) and self.v == other.v

    items = [Sneaky(i) for i in range(5)]
    s = rs.Set(items)
    s.intersection_update([Sneaky(1), Sneaky(3)])
    assert "late" in s and items[1] in s and items[3] in s and len(s) == 3
    t = rs.Set[int]([1, 2, 3])
    t.intersection_update(rs.Set[int]([2, 3, 4]), [3])
    assert t == {3}


# --- M1: probes of str/int tables by arbitrary objects are not O(n) -------------------------


def test_identity_compared_probes_never_scan_native_tables():
    import time

    keys = [f"key{i}" for i in range(200_000)]
    tables = (rs.Set(keys), rs.Dict.fromkeys(keys), rs.Set(range(200_000)))

    class Plain:
        pass

    probes = [object(), Plain(), Plain, len]
    start = time.perf_counter()
    for _ in range(200):
        for table in tables:
            for probe in probes:
                assert probe not in table
    # Each lookup used to hash every stored str under the lock (~30 ms for 1M keys).
    assert time.perf_counter() - start < 5


class StrLike:
    """Equal to one str through its own __eq__."""

    def __init__(self, text):
        self.text = text

    def __hash__(self):
        return hash(self.text)

    def __eq__(self, other):
        return other == self.text if isinstance(other, str) else NotImplemented


def typed_str_dict(keys):
    return rs.Dict[str, int]((k, 0) for k in keys)


@pytest.mark.parametrize("make", [set, rs.Set, rs.Set[str], dict.fromkeys, rs.Dict.fromkeys, typed_str_dict])
def test_custom_eq_probes_of_str_tables(make):
    c = make(f"k{i}" for i in range(100))
    assert StrLike("k5") in c and StrLike("nope") not in c
    remove = c.discard if isinstance(c, (set, rs.Set)) else (lambda k: c.pop(k, None))
    for i in range(60):  # enough removals to compact
        remove(f"k{i}")
    assert StrLike("k5") not in c and StrLike("k77") in c
    if isinstance(c, (set, rs.Set)):
        c.add("k5")
    else:
        c["k5"] = 0
    assert StrLike("k5") in c
    popped = c.pop() if isinstance(c, (set, rs.Set)) else c.popitem()[0]
    assert StrLike(popped) not in c
    c.clear()
    assert StrLike("k77") not in c


def test_str_subclass_with_identity_eq_still_equals_stored_str():
    class Weird(str):
        __eq__ = object.__eq__
        __hash__ = str.__hash__

    for c in ({"abc"}, rs.Set(["abc"]), rs.Dict.fromkeys(["abc"])):
        assert Weird("abc") in c
        assert Weird("abd") not in c


# --- L5: iteration over a mutated Dict/Set follows CPython's rules -------------------------


def drive(container, body, view=iter):
    out = []
    try:
        for k in view(container):
            out.append(k)
            body(container, k[0] if isinstance(k, tuple) else k)
            if len(out) > 60:
                out.append("runaway")
                break
    except RuntimeError as e:
        out.append(f"RuntimeError: {e}")
    return out


DICT_BODIES = {
    "insert": lambda d, k: d.__setitem__(100 + k, 0) if k < 100 else None,
    "delete": lambda d, k: d.pop(k),
    "del+insert same size": lambda d, k: (d.__delitem__(k), d.__setitem__(k + 10, 0)),
    "overwrite value": lambda d, k: d.__setitem__(k, 99),
    "clear+refill": lambda d, k: (d.clear(), [d.__setitem__(i, 0) for i in range(3)]),
    "pop then reinsert same key": lambda d, k: (d.pop(k), d.__setitem__(k, 1)),
    "two keys churn": lambda d, k: (d.pop(k), d.__setitem__(k + 10, 0)) if k < 10 else None,
}


@pytest.mark.parametrize("name", sorted(DICT_BODIES))
@pytest.mark.parametrize("size", [2, 3, 6, 40])
def test_dict_iteration_under_mutation_matches_dict(name, size):
    body = DICT_BODIES[name]
    for view in (iter, reversed, lambda d: d.items(), lambda d: reversed(d.items())):
        src = {i: i for i in range(size)}
        want = drive(dict(src), body, view)
        got = drive(rs.Dict(src), body, view)
        assert got == want


def test_dict_iterator_errors_are_sticky_like_dict():
    for make in (dict, rs.Dict):
        d = make({0: 0, 1: 1})
        it = iter(d)
        next(it)
        d[5] = 5
        with pytest.raises(RuntimeError, match="changed size"):
            next(it)
        del d[5]
        with pytest.raises(RuntimeError, match="changed size"):
            next(it)
        d = make({0: 0, 1: 1})
        it = iter(d)
        for k in (0, 1):
            assert next(it) == k
            del d[k]
            d[k + 10] = 0
        with pytest.raises(RuntimeError, match="keys changed"):
            next(it)
        with pytest.raises(StopIteration):
            next(it)


def test_iterator_length_hints_match_builtins():
    import operator

    for make_d, make_s in ((dict, set), (rs.Dict, rs.Set)):
        d, s = make_d({i: i for i in range(5)}), make_s(range(5))
        di, si = iter(d), iter(s)
        next(di), next(si)
        assert operator.length_hint(di) == 4 and operator.length_hint(si) == 4
        d[9] = 9
        s.add(9)
        assert operator.length_hint(di) == 0 and operator.length_hint(si) == 0


SET_BODIES = {
    "add": lambda s, k: s.add(100 + k) if k < 100 else None,
    "readd same": lambda s, k: (s.discard(k), s.add(k)),
    "clear+refill": lambda s, k: (s.clear(), s.update(range(3))),
}


@pytest.mark.parametrize("name", sorted(SET_BODIES))
def test_set_iteration_under_mutation_matches_set(name):
    body = SET_BODIES[name]
    want = drive({0, 1, 2}, body)
    got = drive(rs.Set([0, 1, 2]), body)
    assert got == want


def test_set_iteration_with_same_size_churn_is_bounded():
    # CPython yields elements re-added into later hash slots (layout dependent); an
    # insertion-ordered Set yields at most its size at creation and never raises.
    for start in (3, 40):
        s = rs.Set(range(start))
        out = drive(s, lambda s, k: (s.discard(k), s.add(k + 1000)))
        assert len(out) <= start and all(isinstance(k, int) for k in out)


def test_dict_growth_compaction_keeps_values_aligned():
    d, ref = rs.Dict(), {}
    for step in range(3000):
        k = step % 97
        for target in (d, ref):
            if k in target and step % 3:
                del target[k]
            else:
                target[k] = step
    assert d.to_dict() == ref and list(d) == list(ref) and list(d.values()) == list(ref.values())
    d = rs.Dict[str, rs.List]()
    for i in range(200):
        d[str(i)] = [i]
        if i % 2:
            del d[str(i - 1)]
    assert [list(v) for v in d.values()] == [[i] for i in range(1, 200, 2)]


# --- M2: expressions sharing subexpressions cannot blow up exponentially ---------------------


def test_exponential_expression_dags_are_rejected():
    e = rs.X
    for _ in range(15):
        e = e + e
    # 2**16 - 1 nodes: fine, and evaluated natively once per shared subexpression.
    assert list(rs.List([1, 2, 3]).map(e)) == [2**15, 2**16, 3 * 2**15]
    assert len(repr(e)) < 2_000_000
    with pytest.raises(ValueError, match="more than 100000 nodes"):
        e + e
    with pytest.raises(ValueError, match="more than 100000 nodes"):
        e * e
    deep = rs.X
    for _ in range(511):
        deep = deep + 1
    with pytest.raises(RecursionError):
        deep + 1


def test_dag_blowup_probe_fails_fast():
    out = run_isolated(
        """
        import time
        import rustique as rs

        start = time.perf_counter()
        e = rs.X
        try:
            for _ in range(40):
                e = e + e
        except ValueError:
            print("rejected", time.perf_counter() - start < 1)
        """,
        timeout=60,
    )
    assert out.split() == ["rejected", "True"]


@pytest.mark.skipif(not hasattr(__import__("signal"), "setitimer"), reason="needs signal.setitimer")
def test_python_fallback_evaluation_is_interruptible():
    out = run_isolated(
        """
        import signal, time
        import rustique as rs

        e = rs.X
        for _ in range(15):
            e = e + e
        lst = rs.List([1] * 5000 + [2**70])  # object storage: evaluated with Python operators
        assert lst.strategy == "object"

        def interrupt(signum, frame):
            raise KeyboardInterrupt

        signal.signal(signal.SIGALRM, interrupt)  # like Ctrl-C, delivered by the OS
        signal.setitimer(signal.ITIMER_REAL, 0.3)
        start = time.perf_counter()
        try:
            lst.map(e)
            print("finished")
        except KeyboardInterrupt:
            print("interrupted", time.perf_counter() - start < 5)
        """,
        timeout=120,
    )
    assert out.split() == ["interrupted", "True"]


# --- L3: fixed-width shifts in the engine match the rustique scalars --------------------------

FIXED = ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"]


def outcome(f):
    try:
        return ("ok", f())
    except (ValueError, OverflowError, TypeError, ZeroDivisionError) as e:
        return ("err", type(e).__name__, str(e))


@pytest.mark.parametrize("name", FIXED)
def test_fixed_shifts_engine_matches_scalars(name):
    cls = getattr(rs, name)
    bits = cls.BITS
    values = sorted({int(cls.MIN), -1 if int(cls.MIN) < 0 else 0, 0, 1, 5, int(cls.MAX)})
    lst = rs.List[cls](values)
    for n in range(-2, bits + 3):
        for op, expr in (("<<", rs.X << n), (">>", rs.X >> n)):

            def scalar():
                return [int(cls(v) << n) if op == "<<" else int(cls(v) >> n) for v in values]

            assert outcome(lambda: [int(x) for x in lst.map(expr)]) == outcome(scalar), (name, op, n)
            assert outcome(lambda: [int(expr(cls(v))) for v in values]) == outcome(scalar), (name, op, n)
        if 0 <= n <= int(cls.MAX):
            # Reflected: the shifted value is converted to the type, the count is the element.
            counts = rs.List[cls]([c for c in (0, 1, bits - 1, bits) if c <= int(cls.MAX)])
            assert outcome(lambda: [int(x) for x in counts.map(n << rs.X)]) == outcome(
                lambda: [int(n << cls(c)) for c in counts]
            ), (name, n)


def test_python_int_shifts_keep_exact_semantics():
    ints = rs.List([1, -5, 2**62])
    assert list(ints.map(rs.X >> 100)) == [0, -1, 0]
    assert list(ints.map(rs.X << 70)) == [x << 70 for x in (1, -5, 2**62)]
    with pytest.raises(ValueError, match="negative shift count"):
        ints.map(rs.X << -1)
    assert list(rs.List[rs.i8]([127]).map(rs.X << 1)) == [-2]
    assert (rs.X << 1)(rs.i8(127)) == rs.i8(-2)


# --- L4: large native filters ------------------------------------------------------------------


def test_large_native_filter():
    n = 300_000
    lst = rs.List[int](range(n))
    kept = lst.filter(rs.X % 3 == 0)
    assert len(kept) == n // 3 and kept[0] == 0 and kept[-1] == n - 3
    strs = rs.List([str(i) for i in range(140_000)])
    assert len(strs.filter(rs.X < "2")) == sum(1 for i in range(140_000) if str(i) < "2")
