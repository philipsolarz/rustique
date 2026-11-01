"""Differential fuzzing of the `datetime` group against CPython (docs/ACCELERATION.md §7.2).

- strptime: the accelerated C entry points against the Python code their originals call
  (`_strptime._strptime_datetime_datetime` & co.): results (type, repr, tzinfo identity and
  fields), exceptions (type, message) and warnings, on random formats over the supported
  directives (plus unsupported ones, flags and stray `%`) and on inputs rendered from
  random datetimes, mutated near-misses and random strings.
- strftime: check mode ("raise"): every accelerated result is compared with the
  original's on the same arguments; plus explicit comparisons with the group disabled.
"""

from __future__ import annotations

import contextlib
import datetime as D
import time
import warnings

import _strptime
import pytest
from hypothesis import assume, given, settings
from hypothesis import strategies as st

rq = pytest.importorskip("rustique._takeover")

GROUP = "datetime"

try:
    import zoneinfo

    ZONES = [zoneinfo.ZoneInfo(k) for k in ("Europe/Berlin", "America/New_York", "Asia/Kolkata", "UTC")]
except Exception:  # noqa: BLE001
    ZONES = []


@pytest.fixture(autouse=True, scope="module")
def enabled():
    rq.enable(only=[GROUP])
    rq.stats(True, reset=True)
    yield
    rq.stats(False)
    rq.disable()


SUPPORTED = ["%Y", "%y", "%m", "%d", "%H", "%I", "%M", "%S", "%f", "%j", "%b", "%B", "%a", "%A", "%p", "%z", "%%"]
EXTRA_315 = ["%:z"]
OTHER = ["%U", "%W", "%w", "%u", "%e", "%Z", "%c", "%x", "%X", "%G", "%V", "%-d", "%Ey", "%Od", "%", "%T", "%F", "%D"]
LITERALS = ["-", "/", ":", " ", "  ", "\t", "T", ".", ",", "x", "'", "Z", "+", "\\", "(", "|", "é"]

directive = st.sampled_from(SUPPORTED + EXTRA_315)
token = st.one_of(directive, directive, st.sampled_from(OTHER), st.sampled_from(LITERALS))
formats = st.lists(token, min_size=0, max_size=8).map("".join)

offsets = st.one_of(
    st.none(),
    st.just(D.timezone.utc),
    st.builds(
        lambda h, m, s, us, neg: D.timezone((-1 if neg else 1) * D.timedelta(hours=h, minutes=m, seconds=s, microseconds=us)),
        st.integers(0, 23), st.integers(0, 59), st.sampled_from([0, 0, 0, 1, 59]), st.sampled_from([0, 0, 0, 1, 999999]),
        st.booleans(),
    ),
)
datetimes = st.builds(
    lambda dt, tz: dt.replace(tzinfo=tz),
    st.one_of(
        st.datetimes(min_value=D.datetime(1, 1, 1), max_value=D.datetime(9999, 12, 31, 23, 59, 59, 999999)),
        st.sampled_from([D.datetime(1, 1, 1), D.datetime(999, 12, 31, 23, 59, 59), D.datetime(1000, 1, 1),
                         D.datetime(9999, 12, 31, 23, 59, 59, 999999), D.datetime(2024, 2, 29, 12, 0, 0, 1)]),
    ),
    offsets,
)


def outcome(f, *args):
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        try:
            r = f(*args)
            tz = getattr(r, "tzinfo", None)
            facts = None if tz is None else (type(tz), tz is D.timezone.utc, tz.utcoffset(None), tz.tzname(None))
            extra = (r.tm_zone, r.tm_gmtoff) if isinstance(r, time.struct_time) else None
            res = ("ok", type(r), repr(r), facts, extra)
        except Exception as e:  # noqa: BLE001
            res = ("err", type(e), str(e))
    return res, [(type(x.message), str(x.message)) for x in w]


PAIRS = [
    (D.datetime.strptime, lambda s, f: _strptime._strptime_datetime_datetime(D.datetime, s, f)),
    (D.date.strptime, lambda s, f: _strptime._strptime_datetime_date(D.date, s, f)),
    (D.time.strptime, lambda s, f: _strptime._strptime_datetime_time(D.time, s, f)),
    (time.strptime, _strptime._strptime_time),
]


def check_strptime(s, fmt):
    for kernel, python in PAIRS:
        # Cold `_regex_cache`: both sides compile (and may warn).
        _strptime._regex_cache.clear()
        got = outcome(kernel, s, fmt)
        _strptime._regex_cache.clear()
        want = outcome(python, s, fmt)
        assert got == want, ("cold", kernel, s, fmt)
        # Warm: the kernel's fast path.
        outcome(python, s, fmt)
        got = outcome(kernel, s, fmt)
        want = outcome(python, s, fmt)
        assert got == want, ("warm", kernel, s, fmt)


def mutate(s: str, data) -> str:
    ops = data.draw(st.lists(st.sampled_from(["del", "ins", "case", "dup", "space"]), max_size=2))
    for op in ops:
        if not s:
            break
        i = data.draw(st.integers(0, len(s) - 1))
        if op == "del":
            s = s[:i] + s[i + 1:]
        elif op == "ins":
            s = s[:i] + data.draw(st.sampled_from(list("0159 :+-Z"))) + s[i:]
        elif op == "case":
            s = s.swapcase()
        elif op == "dup":
            s = s[:i] + s[i] + s[i:]
        else:
            s = s[:i] + " " + s[i:]
    return s


@settings(max_examples=400)
@given(fmt=formats, dt=datetimes, data=st.data())
def test_strptime_round_trip_and_near_misses(fmt, dt, data):
    try:
        s = dt.strftime(fmt.replace("%:z", "%:z"))
    except Exception:  # noqa: BLE001
        s = data.draw(st.text("0123456789 :/-.+TZamp", max_size=24))
    check_strptime(s, fmt)
    check_strptime(mutate(s, data), fmt)


@settings(max_examples=300)
@given(fmt=formats, s=st.text("0123456789 :/-.+TZamp\tJFMASONDjfmasondyuebrlgptcv", max_size=30))
def test_strptime_random_strings(fmt, s):
    check_strptime(s, fmt)


@settings(max_examples=200)
@given(parts=st.lists(st.sampled_from(["%Y", "%m", "%d", "%H", "%M", "%S", "%f", "%j", "%y", "%I"]), min_size=1, max_size=4,
                      unique=True),
       digits=st.text("0123456789", min_size=1, max_size=20))
def test_strptime_adjacent_numeric_fields(parts, digits):
    """Formats like %Y%m%d / %H%M%S%f: sre's backtracking order decides the split."""
    check_strptime(digits, "".join(parts))


# ---------------------------------------------------------------------------------------
# strftime
# ---------------------------------------------------------------------------------------

F_SUPPORTED = ["%Y", "%m", "%d", "%H", "%M", "%S", "%y", "%j", "%I", "%p", "%a", "%A", "%b", "%B", "%h", "%e", "%u", "%w",
               "%F", "%T", "%D", "%R", "%C", "%n", "%t", "%c", "%x", "%X", "%r", "%%", "%f", "%z", "%:z", "%Z"]
F_OTHER = ["%G", "%V", "%U", "%W", "%s", "%k", "%-d", "%Ey", "%", "%:", "%Q"]
f_token = st.one_of(st.sampled_from(F_SUPPORTED), st.sampled_from(F_SUPPORTED), st.sampled_from(F_OTHER),
                    st.sampled_from(LITERALS + ["年", "\x00", "%%Y"]))
f_formats = st.lists(f_token, max_size=8).map("".join)
tzinfos = st.one_of(offsets, st.sampled_from(ZONES) if ZONES else st.none())


@contextlib.contextmanager
def check_raise():
    rq.check("raise")
    try:
        yield
    finally:
        rq.check(None)


@settings(max_examples=400)
@given(fmt=f_formats, dt=datetimes, tz=tzinfos)
def test_strftime_check_mode(fmt, dt, tz):
    dt = dt.replace(tzinfo=tz)
    with check_raise():
        for o in (dt, dt.date(), dt.timetz() if not ZONES or tz not in ZONES else dt.time()):
            with contextlib.suppress(ValueError, UnicodeEncodeError, OverflowError):
                o.strftime(fmt)
            with contextlib.suppress(ValueError, UnicodeEncodeError, OverflowError):
                format(o, fmt)


@settings(max_examples=300)
@given(fmt=f_formats, dt=datetimes, isdst=st.integers(-2, 2))
def test_time_strftime_check_mode(fmt, dt, isdst):
    tt = dt.timetuple()[:8] + (isdst,)
    with check_raise():
        with contextlib.suppress(ValueError, UnicodeEncodeError, OverflowError):
            time.strftime(fmt, tt)
        with contextlib.suppress(ValueError, UnicodeEncodeError, OverflowError):
            time.strftime(fmt, time.struct_time(tt))


@settings(max_examples=60)
@given(fmt=f_formats, dt=datetimes, tz=tzinfos)
def test_strftime_against_disabled(fmt, dt, tz):
    dt = dt.replace(tzinfo=tz)

    def run():
        return [outcome(o.strftime, fmt) for o in (dt, dt.date())]

    got = run()
    rq.disable()
    try:
        want = run()
    finally:
        rq.enable(only=[GROUP])
    assume(True)
    assert got == want


def test_zz_fast_paths_were_exercised():
    st = rq.stats()
    for name in ["datetime.datetime.strptime", "datetime.date.strptime", "datetime.time.strptime", "time.strptime",
                 "datetime.date.strftime", "datetime.time.strftime", "time.strftime"]:
        assert st[name]["hits"] > 20, (name, st[name])
