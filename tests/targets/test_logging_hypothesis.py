"""Group `logging`: Hypothesis differential tests. Every example runs once with the logging
targets installed and once with CPython's original code (targets disabled), on equal
inputs, and compares outputs, `record.__dict__` (keys in order, values), exceptions and
side-effect logs. Time is frozen per record (`created`/`msecs` set explicitly) so the
values are comparable; the `time.time_ns` path of `LogRecord.__init__` has its own
arithmetic test in test_logging.py.
"""

from __future__ import annotations

import decimal
import io
import logging
import string
import sys

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _rustique
import rustique as rt

A = _rustique._accel


@pytest.fixture(autouse=True, scope="module")
def logging_targets():
    A.disable(["logging"])
    rt.enable(only=["logging"])
    import sysconfig

    assert len(A.installed(["logging"])) == (5 if sysconfig.get_config_var("Py_GIL_DISABLED") else 6)
    A.stats(True, True)
    yield
    A.stats(False, True)
    A.disable(["logging"])


def both(fn):
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


ATTRS = ["name", "levelno", "levelname", "pathname", "filename", "module", "lineno", "funcName", "created",
         "asctime", "msecs", "relativeCreated", "thread", "threadName", "process", "processName", "taskName",
         "message", "exc_text", "stack_info", "args", "msg", "custom", "missing"]
CONVS = "sdirafegxc"

directive = st.builds(
    lambda key, flags, width, prec, conv: f"%({key}){flags}{width}{prec}{conv}",
    st.sampled_from(ATTRS),
    st.sampled_from(["", "-", "0", "-0", "+", " ", "#"]),
    st.sampled_from(["", "1", "5", "8", "12"]),
    st.sampled_from(["", ".0", ".2", ".5"]),
    st.sampled_from(list(CONVS)),
)
literal = st.text(alphabet=string.ascii_letters + " :[]|-%é€", max_size=6).map(lambda t: t.replace("%", "%%"))
fmt_strategy = st.lists(st.one_of(directive, literal), min_size=1, max_size=6).map("".join)

datefmt_strategy = st.one_of(
    st.none(),
    st.just(""),
    st.lists(st.sampled_from(["%Y", "%m", "%d", "%H", "%M", "%S", "%y", "%j", "%%", "%b", "%a", "%z", "T", "-", ":", " ", "é"]),
             max_size=8).map("".join),
)

scalar = st.one_of(
    st.none(), st.booleans(), st.integers(min_value=-(2**70), max_value=2**70),
    st.floats(allow_nan=True, allow_infinity=True), st.text(max_size=8),
    # C Decimals (wave 2: plain values for %-formatting), incl. specials and exponents
    st.decimals(allow_nan=True, allow_infinity=True, places=None),
    st.builds(lambda m, e: decimal.Decimal(m).scaleb(e), st.integers(-10**12, 10**12), st.integers(-30, 30)),
)
msg_args = st.one_of(
    st.tuples(st.sampled_from(["plain", "x=%s", "%d", "%s %s", "%5s|%-5s", "%05d", "%r", "%(a)s %(b)d", "%.2f", "100%%", "%s%%"]),
              st.one_of(st.tuples(), st.tuples(scalar), st.tuples(scalar, scalar),
                        st.tuples(st.dictionaries(st.sampled_from(["a", "b", "c"]), scalar, max_size=3)),
                        st.tuples(st.lists(scalar, max_size=2)))),
    st.tuples(scalar, st.just(())),
)
created_strategy = st.one_of(
    st.floats(min_value=-1e9, max_value=4e10, allow_nan=False),
    st.sampled_from([1_793_536_309.946921, 0.0, 1e9 + 0.9995, 86399.999, 253402300799.0]),
)


def make_record(msg, args, created, extra):
    r = logging.LogRecord("app.http", 20, "/srv/app/h.py", 42, msg, args, None, "handle", None)
    r.created = created
    r.msecs = float(int(abs(created - int(created)) * 1000) % 1000)
    r.relativeCreated = 1234.5678
    r.thread, r.process = 140001, 4242
    r.__dict__.update(extra)
    return r


def outcome(f, r):
    try:
        out, err = f.format(r), None
    except Exception as e:  # noqa: BLE001
        out, err = None, (type(e).__name__, str(e))
    return out, err, [(k, repr(v)) for k, v in r.__dict__.items()]


@settings(max_examples=300)
@given(fmt=fmt_strategy, datefmt=datefmt_strategy, ma=msg_args, created=created_strategy,
       extra=st.dictionaries(st.sampled_from(["custom", "asctime"]), scalar, max_size=2),
       defaults=st.one_of(st.none(), st.just({}), st.dictionaries(st.sampled_from(["custom", "missing"]), scalar, max_size=2)))
def test_formatter_format(fmt, datefmt, ma, created, extra, defaults):
    msg, args = ma

    def run():
        f = logging.Formatter(fmt, datefmt, validate=False, defaults=defaults)
        return outcome(f, make_record(msg, args, created, extra))

    a, b = both(run)
    assert a == b


@settings(max_examples=300)
@given(datefmt=datefmt_strategy, created=created_strategy, gm=st.booleans(),
       msf=st.sampled_from([None, "", "%s,%03d", "%s.%03d", "%s"]),
       dtf=st.sampled_from(["%Y-%m-%d %H:%M:%S", "%H:%M:%S", "%d %b %Y", ""]),
       msecs=st.one_of(st.none(), st.floats(min_value=-5, max_value=2000), st.integers(-5, 2000)))
def test_format_time(datefmt, created, gm, msf, dtf, msecs):
    import time

    def run():
        f = logging.Formatter()
        if gm:
            f.converter = time.gmtime
        f.default_msec_format = msf
        f.default_time_format = dtf
        r = make_record("m", (), created, {})
        if msecs is not None:
            r.msecs = msecs
        res = []
        for d in (datefmt, None):
            try:
                res.append(f.formatTime(r, d))
            except Exception as e:  # noqa: BLE001
                res.append((type(e).__name__, str(e)))
        return res

    a, b = both(run)
    assert a == b


level_strategy = st.sampled_from([0, 5, 10, 15, 20, 25, 30, 40, 50])


@settings(max_examples=150)
@given(levels=st.lists(level_strategy, min_size=4, max_size=4),
       disabled=st.lists(st.booleans(), min_size=4, max_size=4),
       propagate=st.lists(st.booleans(), min_size=4, max_size=4),
       manager_disable=st.sampled_from([0, 0, 15, 35]),
       calls=st.lists(st.tuples(st.integers(0, 3), st.sampled_from(["debug", "info", "warning", "error", "critical", "log"]),
                                level_strategy, st.booleans()), min_size=1, max_size=12))
def test_logger_hierarchy_calls(levels, disabled, propagate, manager_disable, calls):
    names = ["hy", "hy.a", "hy.a.b", "hy.c"]

    def run():
        sio = io.StringIO()
        root = logging.getLogger("hy")
        root.handlers.clear()
        h = logging.StreamHandler(sio)
        h.setFormatter(logging.Formatter("%(levelname)s:%(name)s:%(funcName)s:%(message)s"))
        root.addHandler(h)
        root.propagate = False
        for n, lv, dis, pr in zip(names, levels, disabled, propagate):
            lg = logging.getLogger(n)
            lg.setLevel(lv)
            lg.disabled = dis
            if n != "hy":
                lg.propagate = pr
        logging.disable(manager_disable)
        enabled = []
        try:
            for i, meth, lv, kw in calls:
                lg = logging.getLogger(names[i])
                enabled.append(lg.isEnabledFor(lv))
                kwargs = {"extra": {"k": 1}} if kw else {}
                if meth == "log":
                    lg.log(lv, "m %s", lv, **kwargs)
                else:
                    getattr(lg, meth)("m %s", meth, **kwargs)
        finally:
            logging.disable(logging.NOTSET)
        return sio.getvalue(), enabled

    a, b = both(run)
    assert a == b


def _depth(n, lg, stacklevel):
    if n:
        return _depth(n - 1, lg, stacklevel)
    return lg.findCaller(False, stacklevel)


@settings(max_examples=100)
@given(depth=st.integers(0, 6), stacklevel=st.integers(-1, 10))
def test_find_caller(depth, stacklevel):
    lg = logging.getLogger("hy.fc")
    a, b = both(lambda: _depth(depth, lg, stacklevel))
    assert a == b


@settings(max_examples=150)
@given(name=st.text(max_size=5), level=st.one_of(level_strategy, st.integers(-3, 70)),
       pathname=st.one_of(st.text(alphabet="/._-ab", max_size=12), st.sampled_from(["", "/a/b.py", "x.tar.gz", "/é/ü.py"])),
       lineno=st.integers(0, 10**6), args=st.one_of(st.tuples(), st.tuples(scalar), st.tuples(scalar, scalar),
                                                     st.tuples(st.dictionaries(st.text(max_size=2), scalar, max_size=2))),
       func=st.one_of(st.none(), st.text(max_size=4)), sinfo=st.one_of(st.none(), st.text(max_size=4)))
def test_log_record_init(name, level, pathname, lineno, args, func, sinfo):
    def run():
        r = logging.LogRecord(name, level, pathname, lineno, "m", args, None, func, sinfo)
        d = r.__dict__
        return [(k, repr(v) if k not in ("created", "msecs", "relativeCreated") else type(v)) for k, v in d.items()], r.args is args

    a, b = both(run)
    assert a == b


def test_kernels_were_exercised():
    """The examples above reached every kernel's fast path (and its fallbacks)."""
    s = A.stats()
    import sysconfig

    names = ["logging.Logger.findCaller", "logging.LogRecord.__init__", "logging.Formatter.formatTime",
             "logging.Formatter.format"]
    if not sysconfig.get_config_var("Py_GIL_DISABLED"):
        names.append("logging.Logger.level")  # not installed on free-threaded builds
    for name in names:
        assert s[name]["hits"] > 50, (name, s[name])
        assert sum(s[name]["fallback"].values()) > 0, (name, s[name])
