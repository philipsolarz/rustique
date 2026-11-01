"""Group `logging` (crates/rustique-py/src/targets/logging, docs/targets/logging.md):
mechanics, guards, dependency pins and differential tests against CPython's own code.

Differential tests run the same operation with the logging targets installed and with
them disabled (CPython's original code) in this process, on equal inputs, and compare
results, `record.__dict__` (keys *and* order), exceptions and side effects.
"""

from __future__ import annotations

import copy
import io
import logging
import pickle
import sys
import threading
import time
import types
from unittest import mock

import pytest

from rustique import _rustique
import rustique as rt

A = _rustique._accel
TARGETS = [
    "logging.Logger.level",
    "logging.Logger.findCaller",
    "logging.LogRecord.__init__",
    "logging.Formatter.formatTime",
    "logging.Formatter.format",
]


def settle():
    """Let pending calls (deopt-while-unpinned resyncs) run."""
    for _ in range(2000):
        pass


def installed() -> set[str]:
    return set(A.installed(TARGETS))


@pytest.fixture(autouse=True)
def logging_targets():
    """Every test starts with the logging targets installed and counting."""
    A.disable(["logging"])
    A._logging_reset_caches()
    rt.enable(only=["logging"])
    A.stats(True, True)
    assert installed() == set(TARGETS), rt.status()
    yield
    A.stats(False, True)
    rt.check(None)
    A.disable(["logging"])


def stats(name: str) -> dict:
    return A.stats()[name]


def hits(name: str) -> int:
    return stats(name)["hits"]


class plain:
    """`with plain():` runs the body with the logging targets disabled (CPython's code)."""

    def __enter__(self):
        A.disable(["logging"])

    def __exit__(self, *exc):
        rt.enable(only=["logging"])


def both(fn):
    """`(enabled_result, plain_result)` of `fn()` (called from the same line both times, so
    stacks and tracebacks are comparable)."""
    out = []
    for disabled in (False, True):
        if disabled:
            A.disable(["logging"])
        try:
            out.append(fn())
        finally:
            if disabled:
                rt.enable(only=["logging"])
    return out[0], out[1]


# ---------------------------------------------------------------------------------------
# Installation and introspection
# ---------------------------------------------------------------------------------------


def test_status_installed_with_sites():
    st = {t["name"]: t for t in rt.status()["targets"] if t["group"] == "logging"}
    assert set(st) == set(TARGETS)
    for t in st.values():
        assert t["state"] == "installed", t
    assert len(st["logging.Logger.level"]["sites"]) == 6


def test_introspection_unchanged():
    import inspect

    for f in (logging.Logger.debug, logging.Logger.findCaller, logging.LogRecord.__init__,
              logging.Formatter.format, logging.Formatter.formatTime, logging.Logger.isEnabledFor):
        assert type(f) is types.FunctionType
        assert f.__module__ == "logging"
        assert inspect.signature(f) is not None
        assert "def " + f.__name__ in inspect.getsource(f)
    assert pickle.loads(pickle.dumps(logging.Formatter.format)) is logging.Formatter.format
    assert str(inspect.signature(logging.Logger.findCaller)) == "(self, stack_info=False, stacklevel=1)"


def test_captured_references_are_accelerated():
    from logging import Formatter, LogRecord

    fmt = Formatter("%(levelname)s %(message)s")
    bound = fmt.format
    rec = LogRecord("n", 20, "/x/y.py", 1, "m %s %s", (1, 2), None)
    before = hits("logging.Formatter.format")
    assert bound(rec) == "INFO m 1 2"
    assert hits("logging.Formatter.format") == before + 1
    assert hits("logging.LogRecord.__init__") >= 1


def test_optimized_mode_installs():
    import subprocess

    code = (
        "import rustique, logging\n"
        "rustique.enable(only=['logging'])\n"
        "st = [t for t in rustique.status()['targets'] if t['group'] == 'logging']\n"
        "assert all(t['state'] == 'installed' for t in st), st\n"
        "print(logging.LogRecord('n', 20, '/p.py', 1, 'm', (), None).threadName)\n"
    )
    out = subprocess.run([sys.executable, "-O", "-c", code], capture_output=True, text=True)
    assert out.returncode == 0, out.stderr
    assert out.stdout.strip() == "MainThread"


def test_disable_restores_and_reenable():
    rec = logging.LogRecord("n", 20, "/x/y.py", 1, "m", (), None)
    A.disable(["logging"])
    assert installed() == set()
    A.stats(True, True)
    logging.Formatter().format(rec)
    assert hits("logging.Formatter.format") == 0
    rt.enable(only=["logging"])
    logging.Formatter().format(rec)
    assert hits("logging.Formatter.format") == 1


# ---------------------------------------------------------------------------------------
# LogRecord.__init__
# ---------------------------------------------------------------------------------------

TIME_KEYS = {"created", "msecs", "relativeCreated"}


def record_view(r):
    d = r.__dict__
    return [(k, v) for k, v in d.items() if k not in TIME_KEYS], list(d)


ARGS_SHAPES = [
    (),
    (1,),
    ("a", 2, 3.5, None, True),
    ({"a": 1},),
    ({},),
    ([1, 2],),
    ((1, 2),),
    ("x",),
    (b"x",),
    (0,),
    (2**80,),
]


@pytest.mark.parametrize("args", ARGS_SHAPES, ids=repr)
@pytest.mark.parametrize("pathname", ["/srv/app/handlers.py", "mod.py", "/a/.hidden", "/a/..x.py", "", "<stdin>", "/x/é.py"])
def test_record_dict_identical(args, pathname):
    def mk():
        return logging.LogRecord("app.http", 20, pathname, 42, "x=%s", args, None, "handle", None)

    a, b = both(mk)
    assert record_view(a) == record_view(b)
    for k in TIME_KEYS:
        assert type(getattr(a, k)) is float
    # args unwrapping keeps identity
    if args and len(args) == 1 and isinstance(args[0], dict) and args[0]:
        assert a.args is args[0]
    else:
        assert a.args is args


def test_record_kernel_hits_and_single_arg_learning():
    A.stats(True, True)
    for _ in range(3):
        logging.LogRecord("n", 20, "/p.py", 1, "m %s", (5,), None)
    s = stats("logging.LogRecord.__init__")
    assert s["hits"] >= 2  # the first single-int call may learn from the original


@pytest.mark.parametrize("level", [0, 5, 10, 20, 25, 30, 40, 50, 99, -1, 2**70])
def test_level_names(level):
    a, b = both(lambda: logging.LogRecord("n", level, "/p.py", 1, "m", (), None))
    assert record_view(a) == record_view(b)


def test_custom_level_names_and_nonint_levels():
    logging.addLevelName(25, "NOTICE")
    try:
        a, b = both(lambda: logging.LogRecord("n", 25, "/p.py", 1, "m", (), None))
        assert a.levelname == b.levelname == "NOTICE"
        a, b = both(lambda: logging.LogRecord("n", "INFO", "/p.py", 1, "m", (), None))
        assert record_view(a) == record_view(b)
        a, b = both(lambda: logging.LogRecord(None, None, "", 0, "", (), None, None))  # makeLogRecord
        assert record_view(a) == record_view(b)
    finally:
        del logging._levelToName[25]
        del logging._nameToLevel["NOTICE"]


def test_record_from_thread_and_dummy_thread():
    out = {}

    def work():
        out["a"], out["b"] = both(lambda: logging.LogRecord("n", 20, "/p.py", 1, "m", (), None))

    t = threading.Thread(target=work, name="worker-7")
    t.start()
    t.join()
    assert record_view(out["a"]) == record_view(out["b"])
    assert out["a"].threadName == "worker-7"


def test_thread_name_changes_are_seen():
    cur = threading.current_thread()
    old = cur.name
    try:
        cur.name = "renamed"
        assert logging.LogRecord("n", 20, "/p.py", 1, "m", (), None).threadName == "renamed"
    finally:
        cur.name = old


def test_switches_and_time_ns_mock_fall_back():
    with mock.patch.object(logging, "logThreads", False):
        settle()
        r = logging.LogRecord("n", 20, "/p.py", 1, "m", (), None)
        assert r.thread is None and r.threadName is None
    settle()
    with mock.patch("time.time_ns", return_value=1_677_903_920_999_999_900):
        settle()
        r = logging.LogRecord("n", 20, "/p.py", 1, "m", (), None)
        assert r.created == 1_677_903_921.0 and r.msecs == 0.0
    settle()
    A.stats(True, True)
    logging.LogRecord("n", 20, "/p.py", 1, "m", (), None)
    assert hits("logging.LogRecord.__init__") == 1  # re-armed


def test_time_math_matches_python():
    # The kernel's created/msecs/relativeCreated arithmetic, against the original's
    # formulas on edge values (gh-89047 rounding).
    start = logging._startTime
    for ct in [0, 1, 999_999_999, 1_000_000_000, 1_677_903_920_999_998_503, 1_677_903_920_999_999_900,
               1_677_903_920_999_000_000, 1_793_536_309_946_921_000, start, start + 1, 2**62]:
        got = A._logging_times(ct)
        created = ct / 1e9
        msecs = (ct % 1_000_000_000) // 1_000_000 + 0.0
        if msecs == 999.0 and int(created) != ct // 1_000_000_000:
            msecs = 0.0
        assert got == (created, msecs, (ct - start) / 1e6), ct


def test_record_subclass_and_factory():
    class R(logging.LogRecord):
        pass

    a, b = both(lambda: R("n", 20, "/p.py", 1, "m", (), None))
    assert record_view(a) == record_view(b)

    class P(logging.LogRecord):
        @property
        def module(self):
            return "prop"

        @module.setter
        def module(self, v):
            self._m = v

    a, b = both(lambda: P("n", 20, "/p.py", 1, "m", (), None))
    assert record_view(a) == record_view(b)


def test_multiprocessing_process_name():
    import multiprocessing

    A.stats(True, True)
    mk = lambda: logging.LogRecord("n", 20, "/p.py", 1, "m", (), None)  # noqa: E731
    a, b = both(mk)
    assert record_view(a) == record_view(b) and a.processName == "MainProcess"
    proc = multiprocessing.current_process()
    old = proc.name
    try:
        proc.name = "renamed-main"
        a, b = both(mk)
        assert a.processName == b.processName == "renamed-main"
    finally:
        proc.name = old
    with mock.patch.dict(sys.modules, {"multiprocessing": None}):
        a, b = both(mk)
        assert record_view(a) == record_view(b) and a.processName == "MainProcess"
    with mock.patch.object(multiprocessing, "current_process", lambda: types.SimpleNamespace(name="mocked")):
        a, b = both(mk)
        assert a.processName == b.processName == "mocked"
    assert hits("logging.LogRecord.__init__") >= 3


def test_asyncio_task_name():
    import asyncio

    async def main():
        return both(lambda: logging.LogRecord("n", 20, "/p.py", 1, "m", (), None))

    a, b = asyncio.run(main())
    assert record_view(a) == record_view(b)
    assert a.taskName and a.taskName.startswith("Task-")
    a, b = both(lambda: logging.LogRecord("n", 20, "/p.py", 1, "m", (), None))
    assert a.taskName is None


# ---------------------------------------------------------------------------------------
# Level methods
# ---------------------------------------------------------------------------------------


class Capture(logging.Handler):
    def __init__(self):
        super().__init__()
        self.records = []

    def emit(self, record):
        self.records.append((record.levelno, record.getMessage(), record.funcName, record.lineno))


def make_logger(name, level=logging.INFO):
    lg = logging.getLogger(name)
    lg.handlers.clear()
    lg.setLevel(level)
    lg.propagate = False
    h = Capture()
    lg.addHandler(h)
    return lg, h


def test_disabled_calls_are_kernel_hits_and_enabled_calls_log():
    lg, h = make_logger("t.level")
    lg.isEnabledFor(10), lg.isEnabledFor(5)  # fill the level cache (a miss takes the lock: CPython's path)
    A.stats(True, True)
    lg.debug("no %s", 1)
    lg.debug("no %s", 1, extra={"x": 1})
    lg.log(5, "no")
    s = stats("logging.Logger.level")
    assert s["hits"] == 3
    lg.info("yes %s", 2)
    lg.warning("w")
    lg.log(logging.ERROR, "e %d", 3)
    assert [r[:2] for r in h.records] == [(20, "yes 2"), (30, "w"), (40, "e 3")]


def test_level_keyword_errors_match():
    lg, _ = make_logger("t.kw")

    def call(f):
        try:
            f()
        except TypeError as e:
            return str(e)

    for f in (lambda: lg.debug("x", msg="y"), lambda: lg.debug(msg="y"), lambda: lg.log(5, "x", level=1),
              lambda: lg.debug(), lambda: lg.log("5", "x")):
        a, b = both(lambda: call(f))
        assert a == b


def test_is_enabled_for_matches_hierarchy_and_disable():
    import random

    rnd = random.Random(7)
    names = ["h", "h.a", "h.a.b", "h.c", "h.c.d.e"]
    for trial in range(30):
        for n in names:
            logging.getLogger(n).setLevel(rnd.choice([0, 0, 10, 20, 30, 40]))
            logging.getLogger(n).disabled = rnd.random() < 0.1
        dis = rnd.choice([0, 0, 0, 15, 35])
        logging.disable(dis)
        try:
            levels = [5, 10, 15, 20, 25, 30, 45]
            got = both(lambda: [[logging.getLogger(n).isEnabledFor(lv) for lv in levels] for n in names])
            assert got[0] == got[1], trial
            # twice: the second time from the cache (kernel hits)
            got = both(lambda: [[logging.getLogger(n).isEnabledFor(lv) for lv in levels] for n in names])
            assert got[0] == got[1], trial
        finally:
            logging.disable(logging.NOTSET)
    for n in names:
        logging.getLogger(n).setLevel(0)
        logging.getLogger(n).disabled = False


def test_instance_and_class_overrides_of_is_enabled_for():
    lg, h = make_logger("t.ovr", logging.WARNING)
    with mock.patch.object(lg, "isEnabledFor", return_value=True):
        lg.debug("forced")
    assert h.records[-1][:2] == (10, "forced")

    class L(logging.Logger):
        def isEnabledFor(self, level):
            return True

    sub = L("t.sub")
    sub.setLevel(logging.WARNING)
    sub.addHandler(h)
    sub.debug("sub")
    assert h.records[-1][:2] == (10, "sub")
    lg.disabled = True
    lg.error("dropped")
    assert h.records[-1][1] == "sub"


def test_weird_cache_keys_fall_back():
    lg, h = make_logger("t.cache", logging.WARNING)

    calls = []

    class Eq:
        def __hash__(self):
            return hash(10)

        def __eq__(self, other):
            calls.append(other)
            return False

    def run():
        lg._cache.clear()
        lg.isEnabledFor(10)
        lg._cache[Eq()] = True
        calls.clear()
        r = (lg.debug("x"), lg.isEnabledFor(10), lg.isEnabledFor(30))
        return r, list(calls)

    a, b = both(run)
    assert a == b
    lg._cache.clear()


# ---------------------------------------------------------------------------------------
# findCaller
# ---------------------------------------------------------------------------------------


def _caller_cases(lg):
    def inner(stacklevel):
        return lg.findCaller(False, stacklevel)

    def outer(stacklevel):
        return inner(stacklevel)

    out = []
    for sl in (1, 2, 3, 50):
        out.append(inner(sl))
        out.append(outer(sl))
    out.append(lg.findCaller())
    out.append(lg.findCaller(None))
    return out


def test_find_caller_matches():
    lg = logging.getLogger("t.fc")
    a, b = both(lambda: _caller_cases(lg))
    assert a == b
    assert a[0][2] == "<lambda>" or a[0][2] == "inner"


def test_find_caller_through_logging_call_and_stacklevel():
    lg, h = make_logger("t.fc2")

    def helper(msg):
        lg.info(msg, stacklevel=2)

    def user():
        helper("via helper")
        lg.info("direct")

    def run():
        h.records.clear()
        user()
        return list(h.records)

    a, b = both(run)
    assert a == b
    assert a[0][2] == "user" and a[1][2] == "user"


def test_find_caller_fallbacks():
    lg = logging.getLogger("t.fc3")
    a, b = both(lambda: lg.findCaller(True))
    assert a[:3] == b[:3] and a[3].startswith("Stack (most recent call last):")
    a, b = both(lambda: [lg.findCaller(False, 0), lg.findCaller(False, -1)])
    assert a == b
    assert a[0][2] == "findCaller"
    with mock.patch.object(logging, "_srcfile", None):
        settle()
        a = lg.findCaller()
    settle()
    with plain(), mock.patch.object(logging, "_srcfile", None):
        b = lg.findCaller()
    assert (a[0], a[2], a[3]) == (b[0], b[2], b[3])


def test_find_caller_line_numbers_in_big_function():
    lg = logging.getLogger("t.fc4")
    src = "def big(lg):\n" + "    x = 1\n" * 300 + "    r1 = lg.findCaller()\n" + "    y = 2\n" * 50 + "    return r1, lg.findCaller()\n"
    ns = {}
    exec(compile(src, "/tmp/big_module.py", "exec"), ns)
    a, b = both(lambda: [ns["big"](lg) for _ in range(3)])
    assert a == b
    assert a[0][0][1] == 302 and a[0][1][1] == 353


# ---------------------------------------------------------------------------------------
# Formatter.formatTime / format
# ---------------------------------------------------------------------------------------


def fixed_record(created=1_793_536_309.946921, msg="x=%s", args=(5,), **kw):
    r = logging.LogRecord(kw.pop("name", "app.http"), kw.pop("level", 20), kw.pop("pathname", "/srv/app/h.py"),
                          kw.pop("lineno", 42), msg, args, kw.pop("exc_info", None), kw.pop("func", "handle"),
                          kw.pop("sinfo", None))
    r.created = created
    r.msecs = float(int((created - int(created)) * 1000))
    r.relativeCreated = 1234.5678
    r.__dict__.update(kw)
    return r


def format_both(fmt_factory, record_factory):
    def run():
        f = fmt_factory()
        r = record_factory()
        try:
            out = f.format(r)
            err = None
        except Exception as e:  # noqa: BLE001
            out, err = None, (type(e), str(e))
        return out, err, list(r.__dict__.items())

    return both(run)


DATEFMTS = [None, "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%H:%M:%S", "%d/%m/%y %j %%", "%b %d %H:%M",
            "%Y-%m-%dT%H:%M:%S%z", "", "é %Y"]


@pytest.mark.parametrize("datefmt", DATEFMTS)
@pytest.mark.parametrize("created", [1_793_536_309.946921, 0.0, 1e9 + 0.9995, 86399.999, -1.5, 253402300799.0, 4e12])
def test_format_time_matches(datefmt, created):
    def run():
        f = logging.Formatter("%(asctime)s %(message)s", datefmt)
        r = fixed_record(created)
        try:
            return f.formatTime(r, datefmt), f.formatTime(r)
        except Exception as e:  # noqa: BLE001
            return type(e), str(e)

    a, b = both(run)
    assert a == b


def test_format_time_converters_and_msec_formats():
    def run():
        out = []
        f = logging.Formatter()
        r = fixed_record()
        out.append(f.formatTime(r))
        f.converter = time.gmtime
        out.append(f.formatTime(r))
        f.default_msec_format = "%s.%03d"
        out.append(f.formatTime(r))
        f.default_msec_format = None
        out.append(f.formatTime(r))
        f.default_time_format = "%H:%M"
        out.append(f.formatTime(r))
        f.converter = lambda t: time.gmtime(t + 3600)
        out.append(f.formatTime(r))
        r.msecs = 7
        f2 = logging.Formatter()
        out.append(f2.formatTime(r))
        r.msecs = 1234.5
        out.append(f2.formatTime(r))
        return out

    a, b = both(run)
    assert a == b
    with mock.patch.object(logging.Formatter, "converter", time.gmtime):
        a, b = both(lambda: logging.Formatter().formatTime(fixed_record()))
        assert a == b


def test_format_time_mocked_strftime_falls_back():
    with mock.patch("time.strftime", return_value="MOCK"):
        settle()
        assert logging.Formatter().formatTime(fixed_record()) == "MOCK,946"
    settle()


FORMATS = [
    "%(message)s",
    logging.BASIC_FORMAT,
    "%(asctime)s %(levelname)-8s [%(name)s] %(message)s",
    "%(asctime)s.%(msecs)03d %(process)d %(thread)d %(threadName)s %(module)s:%(funcName)s:%(lineno)d %(message)s",
    "%(created)f %(relativeCreated)d %(levelno)s %(pathname)s %(filename)s",
    "%(name)10s|%(levelname)-10.3s|%(lineno)05d|%(lineno)-5d|%(message).4s|%(msecs)d",
    "%(missing)s",
    "%(name)d",
    "%(message)r %(args)s",
    "%(taskName)s %(stack_info)s %(exc_text)s",
    "100%% %(message)s",
    "%(name)s %",
    "plain text",
    "é %(message)s",
]


@pytest.mark.parametrize("fmt", FORMATS)
@pytest.mark.parametrize("datefmt", [None, "%H:%M:%S"])
def test_format_matches(fmt, datefmt):
    a, b = format_both(lambda: logging.Formatter(fmt, datefmt, validate=False), fixed_record)
    assert a == b


MESSAGES = [
    ("plain", ()),
    ("x=%s", (5,)),
    ("x=%s y=%d z=%.2f w=%r", ("a", 2, 3.14159, "q")),
    ("%s %s", (None, True)),
    ("%5s|%-5s|%05d|%-05d|%x", ("ab", "cd", -42, 7, 255)),
    ("%(a)s %(b)d", ({"a": "A", "b": 3},)),
    ("%s", ({"a": 1},)),
    ("%d", ("notint",)),
    ("%s %s", (1,)),
    ("%s", (1, 2)),
    ("%s", ([1, 2],)),
    ("%s", (object(),)),
    ("%d", (3.9,)),
    ("%d", (float("nan"),)),
    ("%s", (2**100,)),
    ("é %s", ("ü",)),
    ("%s", ("€",)),
    ("100%%", ()),
    ("100%%", (1,)),
    (12345, ()),
    (None, ()),
    ("%(x)s", ({"x": object()},)),
]


@pytest.mark.parametrize("msg,args", MESSAGES, ids=repr)
def test_format_messages_match(msg, args):
    a, b = format_both(lambda: logging.Formatter("%(levelname)s %(message)s"), lambda: fixed_record(msg=msg, args=args))
    assert a == b


def test_format_exc_and_stack_info_fall_back_identically():
    try:
        1 / 0
    except ZeroDivisionError:
        ei = sys.exc_info()
    a, b = format_both(logging.Formatter, lambda: fixed_record(exc_info=ei))
    assert a[0] == b[0]
    a, b = format_both(logging.Formatter, lambda: fixed_record(sinfo="Stack (most recent call last):\n  x"))
    assert a == b


def test_format_defaults_and_styles():
    a, b = format_both(lambda: logging.Formatter("%(custom)s %(message)s", defaults={"custom": "D"}), fixed_record)
    assert a == b
    a, b = format_both(lambda: logging.Formatter("%(custom)s %(message)s", defaults={"custom": "D"}),
                       lambda: fixed_record(custom="R"))
    assert a == b
    for style, fmt in (("{", "{asctime} {levelname} {message}"), ("$", "$asctime $message")):
        a, b = format_both(lambda: logging.Formatter(fmt, style=style), fixed_record)
        assert a == b


def test_format_subclasses_and_instance_overrides():
    class OwnTime(logging.Formatter):
        def formatTime(self, record, datefmt=None):
            return "T"

    class Plain(logging.Formatter):
        pass

    class Style(logging.PercentStyle):
        def _format(self, record):
            return "S"

    for factory in (lambda: OwnTime("%(asctime)s %(message)s"), lambda: Plain("%(asctime)s %(message)s")):
        a, b = format_both(factory, fixed_record)
        assert a == b

    def styled():
        f = logging.Formatter("%(message)s")
        f._style = Style("%(message)s")
        return f

    a, b = format_both(styled, fixed_record)
    assert a == b and a[0] == "S"

    def shadowed():
        f = logging.Formatter("%(asctime)s %(message)s")
        f.usesTime = lambda: False
        return f

    a, b = format_both(shadowed, fixed_record)
    assert a == b

    def changed_fmt():
        f = logging.Formatter("%(message)s")
        f._style._fmt = "%(levelname)s:%(message)s"
        return f

    a, b = format_both(changed_fmt, fixed_record)
    assert a == b and a[0] == "INFO:x=5"


def test_format_record_overrides():
    def rec_with_getmessage():
        r = fixed_record()
        r.getMessage = lambda: "overridden"
        return r

    a, b = format_both(logging.Formatter, rec_with_getmessage)
    assert a[:2] == b[:2] and a[0] == "overridden"
    assert [k for k, _ in a[2]] == [k for k, _ in b[2]]

    def rec_nonstr_key():
        r = fixed_record()
        r.__dict__[1] = "one"
        return r

    a, b = format_both(logging.Formatter, rec_nonstr_key)
    assert a == b

    class Rec(logging.LogRecord):
        def getMessage(self):
            return "sub"

    a, b = format_both(logging.Formatter, lambda: Rec("n", 20, "/p.py", 1, "m", (), None))
    assert a[0] == b[0] == "sub"


def test_format_user_str_runs_once_and_errors_identical():
    calls = []

    class S:
        def __str__(self):
            calls.append(1)
            return "S!"

    class Bad:
        def __str__(self):
            raise ValueError("bad str")

    a, b = format_both(logging.Formatter, lambda: fixed_record(args=(S(),)))
    assert a[:2] == b[:2] and a[0] == "x=S!" and len(calls) == 2  # once per run
    assert [k for k, _ in a[2]] == [k for k, _ in b[2]]
    a, b = format_both(logging.Formatter, lambda: fixed_record(args=(Bad(),)))
    assert a[:2] == b[:2] and a[1] == (ValueError, "bad str")


# ---------------------------------------------------------------------------------------
# End to end: handlers, errors, tools
# ---------------------------------------------------------------------------------------


def run_scenario():
    sio = io.StringIO()
    lg = logging.getLogger("t.e2e")
    lg.handlers.clear()
    lg.propagate = False
    lg.setLevel(logging.DEBUG)
    h = logging.StreamHandler(sio)
    h.setFormatter(logging.Formatter("%(levelname)-8s %(name)s %(module)s:%(funcName)s:%(lineno)d %(message)s"))
    lg.addHandler(h)
    lg.debug("d %s", 1)
    lg.info("i %s %d", "a", 2)
    lg.warning("w %(k)s", {"k": "v"})
    lg.error("e", extra={"user": "u"})
    try:
        {}["k"]
    except KeyError:
        lg.exception("boom %s", "x")
    lg.info("%d", "bad")  # formatting error -> handleError
    return sio.getvalue()


def test_end_to_end_output_and_handle_error(capsys):
    def run():
        out = run_scenario()
        return out, capsys.readouterr().err

    (a, err_a), (b, err_b) = both(run)
    assert a == b
    # handleError's report (traceback through emit/format/getMessage) is CPython's
    norm = lambda s: "\n".join(l for l in s.splitlines() if "line " not in l)  # noqa: E731
    assert "--- Logging error ---" in err_a
    assert norm(err_a) == norm(err_b)


def test_tools_see_cpython_frames():
    seen = []

    def tracer(frame, event, arg):
        if event == "call" and frame.f_code.co_filename == logging.__file__:
            seen.append(frame.f_code.co_name)
        return None

    lg, h = make_logger("t.tools")
    A.stats(True, True)
    sys.settrace(tracer)
    try:
        lg.info("traced")
        logging.Formatter().format(fixed_record())
    finally:
        sys.settrace(None)
    assert {"info", "findCaller", "__init__", "format", "getMessage"} <= set(seen)
    assert stats("logging.Logger.findCaller")["hits"] == 0


def test_check_mode_agrees():
    rt.check("raise")
    lg, h = make_logger("t.check", logging.WARNING)
    for _ in range(5):
        lg.debug("x")
        lg.isEnabledFor(10)
        lg.findCaller()
        logging.Formatter().formatTime(fixed_record())
        logging.Formatter().formatTime(fixed_record(), "%H:%M:%S")
    rt.check(None)


def test_streak_suspension_for_always_fallback():
    class OwnTime(logging.Formatter):
        def formatTime(self, record, datefmt=None):
            return "T"

    f = OwnTime("%(asctime)s %(message)s")
    r = fixed_record()
    for _ in range(100):
        assert f.format(r) == "T x=5"
    settle()
    st = {t["name"]: t for t in rt.status()["targets"]}
    assert st["logging.Formatter.format"].get("suspended") == "fallback"


# ---------------------------------------------------------------------------------------
# Free-threaded stress
# ---------------------------------------------------------------------------------------


def test_concurrent_logging_identical_lines():
    sio = io.StringIO()
    lg = logging.getLogger("t.ft")
    lg.handlers.clear()
    lg.propagate = False
    lg.setLevel(logging.INFO)
    h = logging.StreamHandler(sio)
    h.setFormatter(logging.Formatter("%(levelname)s %(threadName)s %(message)s"))
    lg.addHandler(h)
    n_threads, n = 8, 400
    barrier = threading.Barrier(n_threads)

    def work(i):
        barrier.wait()
        for j in range(n):
            lg.debug("drop %d", j)
            lg.info("t%d n=%d", i, j)

    ts = [threading.Thread(target=work, args=(i,), name=f"w{i}") for i in range(n_threads)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    lines = sio.getvalue().splitlines()
    assert len(lines) == n_threads * n
    expect = {f"INFO w{i} t{i} n={j}" for i in range(n_threads) for j in range(n)}
    assert set(lines) == expect


def test_enable_disable_under_load():
    sio = io.StringIO()
    lg = logging.getLogger("t.ft2")
    lg.handlers.clear()
    lg.propagate = False
    lg.setLevel(logging.INFO)
    h = logging.StreamHandler(sio)
    h.setFormatter(logging.Formatter("%(levelname)s %(message)s"))
    lg.addHandler(h)
    stop = threading.Event()
    count = [0] * 4

    def work(i):
        while not stop.is_set():
            lg.debug("x")
            lg.info("v%d", i)
            count[i] += 1

    ts = [threading.Thread(target=work, args=(i,)) for i in range(4)]
    for t in ts:
        t.start()
    try:
        for _ in range(20):
            A.disable(["logging"])
            rt.enable(only=["logging"])
    finally:
        stop.set()
        for t in ts:
            t.join()
    lines = sio.getvalue().splitlines()
    assert len(lines) == sum(count)
    assert all(l.startswith("INFO v") for l in lines)
