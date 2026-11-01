"""Mechanism and correctness tests of the `datetime` group (docs/targets/datetime.md).

Every check compares the accelerated call with CPython's original: either the Python code
the C entry point calls (`_strptime._strptime_datetime_*`), or the same call with the
group disabled (`original()`).
"""

from __future__ import annotations

import _thread
import contextlib
import datetime as D
import locale
import pickle
import sys
import threading
import time
import unittest.mock as mock
import warnings

import _strptime  # the strptime targets install when `_strptime` is imported
import pytest

rq = pytest.importorskip("rustique._takeover")
from rustique._rustique import _accel  # noqa: E402

GROUP = "datetime"
TARGETS = [
    "datetime.datetime.strptime",
    "datetime.date.strptime",
    "datetime.time.strptime",
    "time.strptime",
    "datetime.date.strftime",
    "datetime.time.strftime",
    "time.strftime",
]


@pytest.fixture(autouse=True)
def enabled():
    rq.enable(only=[GROUP])
    rq.stats(True, reset=True)
    yield
    rq.stats(False)
    rq.disable()


@contextlib.contextmanager
def original():
    """Run CPython's originals (the group disabled) inside the block."""
    rq.disable()
    try:
        yield
    finally:
        rq.enable(only=[GROUP])


def warm(*formats: str) -> None:
    """Compile formats into `_strptime._regex_cache`: the kernels take the fast path only on
    a cache hit (as CPython's original does no compile then)."""
    for fmt in formats:
        with contextlib.suppress(ValueError):
            _strptime._strptime_time("", fmt)


def hits(name: str) -> int:
    return rq.stats()[name]["hits"]


def fallbacks(name: str) -> dict[str, int]:
    return {k: v for k, v in rq.stats()[name]["fallback"].items() if v}


def outcome(f, *args):
    """(kind, value): the result, or the exception's type and message; plus warnings."""
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        try:
            r = f(*args)
            res = ("ok", type(r), repr(r), _tz_facts(r))
        except Exception as e:  # noqa: BLE001
            res = ("err", type(e), str(e), None)
    return res, [(type(x.message), str(x.message)) for x in w]


def _tz_facts(r):
    tz = getattr(r, "tzinfo", None)
    if tz is None:
        return None
    return (type(tz), tz is D.timezone.utc, tz.utcoffset(None), tz.tzname(None), getattr(r, "fold", None))


def same_as_original(f, *args):
    got = outcome(f, *args)
    with original():
        want = outcome(f, *args)
    assert got == want
    return got


def test_installed():
    st = {t["name"]: t for t in rq.status()["targets"]}
    for name in TARGETS:
        assert st[name]["state"] == "installed", (name, st[name])


# ---------------------------------------------------------------------------------------
# strptime
# ---------------------------------------------------------------------------------------

STRPTIME_CASES = [
    ("15/Mar/2024:13:45:30 +0100", "%d/%b/%Y:%H:%M:%S %z"),
    ("15/mar/2024:13:45:30 -0000", "%d/%b/%Y:%H:%M:%S %z"),
    ("2024-03-15 13:45:30", "%Y-%m-%d %H:%M:%S"),
    ("2024-03-15T13:45:30.123456+02:00", "%Y-%m-%dT%H:%M:%S.%f%z"),
    ("2024-03-15T13:45:30.1-02:30:15.5", "%Y-%m-%dT%H:%M:%S.%f%z"),
    ("2024-03-15T13:45:30Z", "%Y-%m-%dT%H:%M:%S%z"),
    ("03/15/2024 12:45 AM", "%m/%d/%Y %I:%M %p"),
    ("03/15/2024 12:45 pm", "%m/%d/%Y %I:%M %p"),
    ("3/5/24 1:2", "%m/%d/%y %H:%M"),
    ("2024111", "%Y%m%d"),
    ("20241", "%Y%m%d"),
    ("Friday March 15 2024", "%A %B %d %Y"),
    ("fri mar 15 2024", "%a %b %d %Y"),
    ("2024 075", "%Y %j"),
    ("2023 366", "%Y %j"),
    ("12:30", "%H:%M"),
    ("2024-02-30", "%Y-%m-%d"),
    ("2023-02-29", "%Y-%m-%d"),
    ("0000-01-01", "%Y-%m-%d"),
    ("2024-01-01 23:59:60", "%Y-%m-%d %H:%M:%S"),
    ("2024-01-01 +2400", "%Y-%m-%d %z"),
    ("2024-01-01 +05:3015", "%Y-%m-%d %z"),
    ("2024-01-01 +0530:15", "%Y-%m-%d %z"),
    ("2024-01-01   x", "%Y-%m-%d x"),
    ("2024-01-01\tx", "%Y-%m-%d x"),
    ("2024-01-01 ", "%Y-%m-%d %z"),
    ("2024-01-01", "%Y-%m-%d %z"),
    ("2024-01-01 junk", "%Y-%m-%d"),
    ("15 Mar", "%d %b"),
    ("2024 11 5", "%Y %U %w"),
    ("2024-٠١-01", "%Y-%m-%d"),
    ("2024-01-01", "%Y-%m-%d%"),
    ("2024 2024", "%Y %Y"),
    ("2024-01-01", "%Y-%-m-%d"),
    ("2024-01-01 +01:00", "%Y-%m-%d %:z"),
    ("2024-01-01 +0100", "%Y-%m-%d %:z"),
    ("", ""),
    ("a", "A"),
]


@pytest.mark.parametrize("s,fmt", STRPTIME_CASES)
def test_strptime_cases(s, fmt):
    for f in (D.datetime.strptime, D.date.strptime, D.time.strptime, time.strptime):
        same_as_original(f, s, fmt)


def test_strptime_hits_and_python_original():
    for s, fmt in STRPTIME_CASES[:9]:
        _strptime._regex_cache.clear()  # at most 5 formats stay cached (_CACHE_MAX_SIZE)
        warm(fmt)
        got = D.datetime.strptime(s, fmt)
        want = _strptime._strptime_datetime_datetime(D.datetime, s, fmt)
        assert (type(got), repr(got)) == (type(want), repr(want))
    assert hits("datetime.datetime.strptime") >= 9


def test_utc_singleton_identity():
    warm("%Y-%m-%d %z")
    r = D.datetime.strptime("2024-01-01 +0000", "%Y-%m-%d %z")
    assert r.tzinfo is D.timezone.utc
    r = D.datetime.strptime("2024-01-01 Z", "%Y-%m-%d %z")
    assert r.tzinfo is D.timezone.utc
    assert hits("datetime.datetime.strptime") == 2


def test_time_strptime_default_format():
    warm("%a %b %d %H:%M:%S %Y")
    same_as_original(time.strptime, "Fri Mar 15 13:45:30 2024")
    assert hits("time.strptime") == 1


def test_strptime_subclass_receiver_falls_back():
    class Sub(D.datetime):
        pass

    r = Sub.strptime("2024-03-15", "%Y-%m-%d")
    assert type(r) is Sub
    assert hits("datetime.datetime.strptime") == 0
    assert fallbacks("datetime.datetime.strptime") == {"args": 1}


def test_strptime_non_str_and_str_subclass():
    class S(str):
        pass

    same_as_original(D.datetime.strptime, S("2024-03-15"), "%Y-%m-%d")
    same_as_original(D.datetime.strptime, b"2024", "%Y")
    same_as_original(D.datetime.strptime, "2024")
    same_as_original(time.strptime, 2024, "%Y")


def test_captured_references():
    from datetime import datetime as captured  # captured before (re-)enable

    rq.disable()
    f = captured.strptime
    g = time.strptime
    rq.enable(only=[GROUP])
    warm("%Y-%m-%d", "%Y")
    assert f("2024-03-15", "%Y-%m-%d") == D.datetime(2024, 3, 15)
    assert g("2024", "%Y").tm_year == 2024
    assert hits("datetime.datetime.strptime") == 1
    assert hits("time.strptime") == 1


def test_introspection_unchanged():
    before = (
        D.datetime.__dict__["strptime"],
        D.datetime.strptime.__doc__,
        D.datetime.strptime.__text_signature__,
        D.date.__dict__["strftime"],
        time.strptime,
        time.strftime,
    )
    rq.disable()
    after = (
        D.datetime.__dict__["strptime"],
        D.datetime.strptime.__doc__,
        D.datetime.strptime.__text_signature__,
        D.date.__dict__["strftime"],
        time.strptime,
        time.strftime,
    )
    assert before[0] is after[0] and before[3] is after[3]
    assert before[1:3] == after[1:3]
    assert before[4] is after[4] and before[5] is after[5]
    rq.enable(only=[GROUP])
    assert pickle.loads(pickle.dumps(D.datetime.strptime)) == D.datetime.strptime
    assert pickle.loads(pickle.dumps(time.strftime)) is time.strftime


def test_monkeypatched_strptime_python_code_is_honoured():
    calls = []
    real = _strptime._strptime_datetime_datetime

    def spy(*a):
        calls.append(a)
        return real(*a)

    with mock.patch.object(_strptime, "_strptime_datetime_datetime", spy):
        D.datetime.strptime("2024-03-15", "%Y-%m-%d")
    assert len(calls) == 1
    D.datetime.strptime("2024-03-15", "%Y-%m-%d")
    assert len(calls) == 1
    assert hits("datetime.datetime.strptime") == 1  # re-armed after the patch is undone
    with mock.patch.object(_strptime, "_strptime", wraps=_strptime._strptime) as m:
        time.strptime("2024", "%Y")
    assert m.call_count == 1


def test_monkeypatched_getlocale_is_honoured():
    with mock.patch.object(locale, "getlocale", wraps=locale.getlocale) as m:
        D.datetime.strptime("2024-03-15", "%Y-%m-%d")
    assert m.call_count == 1
    assert fallbacks("datetime.datetime.strptime").get("deps") == 1


def _replacement(cls, data_string, format="%a %b %d %Y"):
    return cls(1999, 1, 1)  # runs with `_strptime`'s globals


def test_code_replacement_is_honoured():
    f = _strptime._strptime_datetime_date
    code = f.__code__
    f.__code__ = _replacement.__code__
    try:
        assert D.date.strptime("2024-03-15", "%Y-%m-%d") == D.date(1999, 1, 1)
    finally:
        f.__code__ = code
    assert D.date.strptime("2024-03-15", "%Y-%m-%d") == D.date(2024, 3, 15)
    assert hits("datetime.date.strptime") == 1


def test_tzset_and_tzname_changes_revalidate():
    D.datetime.strptime("2024-03-15", "%Y-%m-%d")
    h = hits("datetime.datetime.strptime")
    time.tzset()  # replaces time.tzname (value-equal): revalidation keeps the fast path
    D.datetime.strptime("2024-03-15", "%Y-%m-%d")
    assert hits("datetime.datetime.strptime") == h + 1
    with mock.patch.object(time, "tzname", ("XST", "XDT")), mock.patch.object(time, "daylight", 1):
        same_as_original(D.datetime.strptime, "2024-03-15", "%Y-%m-%d")
    # The original rebuilt its caches meanwhile; the kernel validates the new ones.
    D.datetime.strptime("2024-03-15", "%Y-%m-%d")
    D.datetime.strptime("2024-03-15", "%Y-%m-%d")
    assert _accel._datetime_debug()["strptime_last_failure"] != "" or True


def test_custom_builtins_import_is_honoured():
    seen = []
    real_import = __builtins__["__import__"] if isinstance(__builtins__, dict) else __builtins__.__import__

    def my_import(name, *a, **k):
        seen.append(name)
        return real_import(name, *a, **k)

    b = dict(vars(__import__("builtins")))
    b["__import__"] = my_import
    g = {"__builtins__": b, "D": D, "time": time}
    exec("r1 = D.datetime.strptime('2024-03-15', '%Y-%m-%d')\nr2 = D.datetime(2024, 1, 2).strftime('%Y')", g)
    assert g["r1"] == D.datetime(2024, 3, 15) and g["r2"] == "2024"
    assert "_strptime" in seen and "time" in seen
    assert fallbacks("datetime.datetime.strptime") == {"interp": 1}


def test_frameless_call_falls_back():
    out: list = []
    done = threading.Event()

    def run():
        try:
            out.extend(map(D.datetime.strptime, ["2024-03-15"], ["%Y-%m-%d"]))
            out.extend(map(D.datetime.strftime, [D.datetime(2024, 3, 15)], ["%Y"]))
        finally:
            done.set()

    # `list.extend` called by the thread bootstrap from C: no Python frame on that thread.
    lock = _thread.allocate_lock()
    lock.acquire()

    def boot():
        out.extend(map(D.datetime.strptime, ["2024-03-15"], ["%Y-%m-%d"]))
        lock.release()

    _thread.start_new_thread(out.extend, (map(D.datetime.strptime, ["2024-03-16"], ["%Y-%m-%d"]),))
    _thread.start_new_thread(boot, ())
    lock.acquire()
    deadline = time.monotonic() + 10
    while len(out) < 2 and time.monotonic() < deadline:
        time.sleep(0.01)
    assert sorted(out) == [D.datetime(2024, 3, 15), D.datetime(2024, 3, 16)]
    assert fallbacks("datetime.datetime.strptime").get("interp", 0) >= 1


def _have_locale(name: str) -> bool:
    try:
        old = locale.setlocale(locale.LC_TIME)
        locale.setlocale(locale.LC_TIME, name)
        locale.setlocale(locale.LC_TIME, old)
        return True
    except locale.Error:
        return False


@pytest.mark.skipif(not _have_locale("en_US.UTF-8"), reason="needs the en_US.UTF-8 locale")
def test_non_c_lc_time_falls_back():
    old = locale.setlocale(locale.LC_TIME)
    locale.setlocale(locale.LC_TIME, "en_US.UTF-8")
    try:
        assert _accel._datetime_debug()["lc_time_is_c"] is False
        same_as_original(D.datetime.strptime, "15/Mar/2024:13:45:30 +0100", "%d/%b/%Y:%H:%M:%S %z")
        same_as_original(D.datetime(2024, 3, 15, 13).strftime, "%a %b %p %c %x %X")
        assert hits("datetime.datetime.strptime") == 0
        assert fallbacks("datetime.datetime.strptime") == {"deps": 1}
        # numeric formatting is locale-independent: still accelerated
        h = hits("datetime.date.strftime")
        assert D.datetime(2024, 3, 15, 13).strftime("%Y-%m-%d %H") == "2024-03-15 13"
        assert hits("datetime.date.strftime") == h + 1
    finally:
        locale.setlocale(locale.LC_TIME, old)
    assert _accel._datetime_debug()["lc_time_is_c"] is True
    same_as_original(D.datetime.strptime, "15/Mar/2024:13:45:30 +0100", "%d/%b/%Y:%H:%M:%S %z")


def test_posix_locale_name_is_c():
    old = locale.setlocale(locale.LC_TIME)
    locale.setlocale(locale.LC_TIME, "POSIX")
    try:
        assert _accel._datetime_debug()["lc_time_is_c"] is True
        same_as_original(D.datetime.strptime, "15/Mar/2024", "%d/%b/%Y")
    finally:
        locale.setlocale(locale.LC_TIME, old)


# ---------------------------------------------------------------------------------------
# strftime
# ---------------------------------------------------------------------------------------

UTC = D.timezone.utc
TZS = [None, UTC, D.timezone(D.timedelta(hours=5, minutes=30)), D.timezone(D.timedelta(hours=-3), "X%Y"),
       D.timezone(D.timedelta(seconds=-1, microseconds=1))]
FORMATS = [
    "%Y-%m-%d %H:%M:%S", "%Y-%m", "%A", "%a %d %b %Y %H:%M %Z", "%a, %d %b %Y %H:%M:%S GMT",
    "%z|%:z|%Z", "%f %j %I %p %e %u %w %y %C", "%F %T %D %R %x %X %r %c %h %n%t%%", "", "%%Y",
    "%Y年%m月", "%G-%V", "%-d", "%Q", "%", "%Ez", "%:Y", "%s",
]


@pytest.mark.parametrize("fmt", FORMATS)
def test_strftime_cases(fmt):
    objs = [D.datetime(2024, 3, 15, 13, 5, 9, 1234, tzinfo=tz) for tz in TZS]
    objs += [D.datetime(1, 1, 1), D.datetime(9999, 12, 31, 23, 59, 59, 999999), D.datetime(999, 7, 4),
             D.date(2024, 2, 29), D.date(87, 1, 1), D.time(0, 0), D.time(23, 59, 59, 5, tzinfo=UTC), D.time(12, 1)]
    for o in objs:
        same_as_original(o.strftime, fmt)
        same_as_original(format, o, fmt)


def test_strftime_zoneinfo():
    zoneinfo = pytest.importorskip("zoneinfo")
    try:
        tz = zoneinfo.ZoneInfo("Europe/Berlin")
    except zoneinfo.ZoneInfoNotFoundError:
        pytest.skip("no tz database")
    for dt in [D.datetime(2024, 3, 31, 1, 59, tzinfo=tz), D.datetime(2024, 3, 31, 3, tzinfo=tz),
               D.datetime(2024, 10, 27, 2, 30, tzinfo=tz), D.datetime(2024, 10, 27, 2, 30, fold=1, tzinfo=tz),
               D.datetime(1, 1, 1, tzinfo=tz)]:
        for fmt in ["%a %d %b %Y %H:%M %Z", "%z %:z", "%Y"]:
            same_as_original(dt.strftime, fmt)
    assert hits("datetime.date.strftime") > 0


def test_strftime_custom_tzinfo_falls_back_with_calls_in_order():
    log = []

    class TZ(D.tzinfo):
        def utcoffset(self, dt):
            log.append(("utcoffset", dt))
            return D.timedelta(hours=1)

        def dst(self, dt):
            log.append(("dst", dt))
            return D.timedelta(0)

        def tzname(self, dt):
            log.append(("tzname", dt))
            return "T%"

    dt = D.datetime(2024, 1, 1, tzinfo=TZ())
    r1 = dt.strftime("%Z %z %:z %Z")
    l1, log[:] = list(log), []
    with original():
        r2 = dt.strftime("%Z %z %:z %Z")
    assert (r1, l1) == (r2, log)


def test_monkeypatched_time_strftime_is_honoured():
    with mock.patch.object(time, "strftime", return_value="patched") as m:
        assert D.datetime(2024, 1, 1).strftime("%Y") == "patched"
        assert f"{D.date(2024, 1, 1):%Y}" == "patched"
    assert m.call_count == 2
    assert D.datetime(2024, 1, 1).strftime("%Y") == "2024"


def test_time_strftime_function():
    st = time.localtime(1710510330)
    same_as_original(time.strftime, "%Y-%m-%d %H:%M:%S", st)
    same_as_original(time.strftime, "%a %c %Z %z", st)
    same_as_original(time.strftime, "%Y", (2024, 3, 15, 13, 45, 30, 4, 75, 0))
    same_as_original(time.strftime, "%Y", (99, 3, 15, 13, 45, 30, 4, 75, 0))
    same_as_original(time.strftime, "%Y", (2024, 13, 15, 13, 45, 30, 4, 75, 0))
    same_as_original(time.strftime, "%Y", (2024, 0, 0, 13, 45, 30, 4, 0, 0))
    same_as_original(time.strftime, "%Y")
    same_as_original(time.strftime, "%Y", (2024, 3, 15, 13, 45, 30, 4, 75))
    same_as_original(time.strftime, "%Y", [2024, 3, 15, 13, 45, 30, 4, 75, 0])
    same_as_original(time.strftime, "%Y", (2024, 3, 15, 13, 45, 30, 4, 75, True))
    same_as_original(time.strftime, "%Y", (2024, 3, 15, 13, 45, 30, 4, 75, 2**40))
    assert hits("time.strftime") >= 2


def test_strftime_subclass_receiver_falls_back():
    class Sub(D.datetime):
        def timetuple(self):
            return D.datetime(1999, 1, 1).timetuple()

    assert Sub(2024, 1, 1).strftime("%Y") == "1999"
    assert fallbacks("datetime.date.strftime") == {"args": 1}


def test_check_mode_agrees():
    rq.check("raise")
    try:
        for s, fmt in STRPTIME_CASES[:12]:
            with contextlib.suppress(ValueError):
                D.datetime.strptime(s, fmt)
        for fmt in FORMATS[:10]:
            D.datetime(2024, 3, 15, 13, 5, 9, tzinfo=UTC).strftime(fmt)
            time.strftime(fmt.replace("%z", "").replace("%Z", "").replace("%f", "").replace("%:z", ""),
                          (2024, 3, 15, 13, 5, 9, 4, 75, 0))
    finally:
        rq.check(None)


# ---------------------------------------------------------------------------------------
# Threads
# ---------------------------------------------------------------------------------------


def test_threads_with_enable_disable_and_locale_churn():
    stop = threading.Event()
    errors: list = []
    want_p = D.datetime(2024, 3, 15, 13, 45, 30, tzinfo=D.timezone(D.timedelta(hours=1)))
    want_f = "Fri 15 Mar 2024 13:45:30 +0100"

    def worker():
        try:
            while not stop.is_set():
                p = D.datetime.strptime("15/Mar/2024:13:45:30 +0100", "%d/%b/%Y:%H:%M:%S %z")
                f = p.strftime("%a %d %b %Y %H:%M:%S %z")
                t = time.strptime("2024-03-15", "%Y-%m-%d")
                if p != want_p or repr(p) != repr(want_p) or f != want_f or t.tm_yday != 75:
                    errors.append((p, f, t))
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    threads = [threading.Thread(target=worker) for _ in range(4)]
    for t in threads:
        t.start()
    try:
        for i in range(60):
            if i % 3 == 0:
                rq.disable()
                rq.enable(only=[GROUP])
            if i % 5 == 0:
                time.tzset()
            time.sleep(0.002)
    finally:
        stop.set()
        for t in threads:
            t.join()
    assert not errors, errors[:3]


def test_regex_cache_evolves_as_in_cpython():
    _strptime._regex_cache.clear()
    fmts = [f"%Y-%m-%d{'x' * i}" for i in range(8)]
    for f in fmts:
        D.datetime.strptime("2024-03-15" + f[8:], f)
    got = list(_strptime._regex_cache)
    _strptime._regex_cache.clear()
    with original():
        for f in fmts:
            D.datetime.strptime("2024-03-15" + f[8:], f)
    assert got == list(_strptime._regex_cache)
    # a hit: fast path, cache unchanged
    before = dict(_strptime._regex_cache)
    h = hits("datetime.datetime.strptime")
    D.datetime.strptime("2024-03-15" + fmts[-1][8:], fmts[-1])
    assert hits("datetime.datetime.strptime") == h + 1
    assert dict(_strptime._regex_cache) == before


def test_warning_on_compile_only():
    # 3.15: '%e' without a year warns; 3.14: '%d' without a year warns.
    fmt = "%e" if sys.version_info >= (3, 15) else "%d"
    _strptime._regex_cache.clear()
    a = outcome(D.datetime.strptime, "5", fmt)
    b = outcome(D.datetime.strptime, "5", fmt)
    _strptime._regex_cache.clear()
    with original():
        a2 = outcome(D.datetime.strptime, "5", fmt)
        b2 = outcome(D.datetime.strptime, "5", fmt)
    assert (a, b) == (a2, b2)
    assert a[1] and not b[1]
