"""Free-threading stress tests for Rustique's switch, pins and exit path.

    python benches/ft/stress.py toggle   [--secs 20] [--workers 6]   # enable/disable under load
    python benches/ft/stress.py patch    [--secs 20] [--workers 6]   # mock.patch of pins from another thread
    python benches/ft/stress.py tables   [--secs 20]                 # watched-dict mutations vs (re)activation
    python benches/ft/stress.py shutdown [--runs 100]                # daemon threads in kernels at exit

Every scenario arms `faulthandler.dump_traceback_later` (a hang prints every thread's stack
and exits with status 1) and exits non-zero on any wrong result.

- `toggle`: workers call accelerated operations and compare with results computed plain
  before `enable()`; one thread loops `rustique.disable()`/`enable()`; one re-imports a
  module (sys.modules mutations go through Rustique's dict watcher); one runs `gc.collect()`
  (stop-the-world on 3.15t).
- `patch`: for each pinned dependency, a patcher thread replaces it with a counting
  wrapper, hands over to a checker thread through an Event (happens-before), which must see
  the wrapper called by the accelerated entry point (as CPython would), then restores it,
  and the checker must see it no longer called; meanwhile hammer threads call the same entry
  point (score updates, suspensions, resumes).
- `tables`: one thread loops `disable()`/`enable()` (every enable re-activates pins under
  the pin table lock), others mutate pinned module dicts (copy's globals, json's default
  encoder `__dict__`, sys.modules) and run `gc.collect()`.
- `shutdown`: child processes whose daemon threads hammer kernels while the main thread
  exits at a random point; any crash or fatal error fails.
"""

from __future__ import annotations

import argparse
import faulthandler
import os
import random
import subprocess
import sys
import threading
import time

OPS_SRC = r'''
import copy, json, posixpath, fnmatch, logging, textwrap, statistics, dataclasses
from datetime import datetime, date
from urllib.parse import quote, urlsplit

@dataclasses.dataclass
class P:
    x: int
    y: list

_fmt = logging.Formatter("%(levelname)s %(name)s: %(message)s")
_rec = logging.LogRecord("a.b", 20, "/x.py", 1, "hi %s", ("ana",), None)
_doc = {"a": [1, 2.5, "x", None, True], "b": {"c": ("d", 3)}, "s": frozenset({"q"})}

OPS = {
    "json.dumps": lambda: json.dumps({"a": [1, 2.5, "é", None], "b": {"c": True}}, sort_keys=True),
    "json.loads": lambda: json.loads('{"a": [1, 2.5, "\\u00e9", null], "b": {"c": true}}'),
    "deepcopy": lambda: repr(copy.deepcopy(_doc)),
    "copy": lambda: repr(copy.copy([1, [2], {3: 4}])),
    "asdict": lambda: dataclasses.asdict(P(1, [2, {"k": 3}])),
    "strptime": lambda: datetime.strptime("2026-11-02 18:21:47", "%Y-%m-%d %H:%M:%S"),
    "isoformat": lambda: datetime(2026, 11, 2, 18, 21, 47, 5).isoformat(),
    "strftime": lambda: date(2026, 11, 2).strftime("%Y/%m/%d %A"),
    "join": lambda: posixpath.join("/srv", "app", "x.css"),
    "fnmatch": lambda: (fnmatch.fnmatch("a/b.py", "*.py"), fnmatch.fnmatch("a/b.pyc", "*.py")),
    "format": lambda: _fmt.format(_rec),
    "wrap": lambda: textwrap.wrap("the quick brown fox jumps over the lazy dog", 12),
    "mean": lambda: statistics.mean([1.5, 2.5, 3.25]),
    "quote": lambda: quote("/a b/é?x=1"),
    "urlsplit": lambda: urlsplit("https://h:8/a?b=1#c"),
    "sorted": lambda: sorted([5, 3, 9, 1, 7, 2]),
    "contains": lambda: (7 in [1, 2, 3, 4, 5, 6, 7], "z" in ("a", "b")),
    "max": lambda: max([1.5, 9.25, 3.0]),
    "repr": lambda: repr(0.1 + 0.2),
}
'''


def ops():
    ns: dict = {}
    exec(OPS_SRC, ns)
    return ns["OPS"]


def arm(secs: float) -> None:
    faulthandler.dump_traceback_later(secs + 60, exit=True)


class Fail:
    def __init__(self):
        self.n = 0
        self.msgs: list[str] = []
        self.lock = threading.Lock()

    def __call__(self, msg: str) -> None:
        with self.lock:
            self.n += 1
            if len(self.msgs) < 20:
                self.msgs.append(msg)


def workers_loop(stop: threading.Event, fail: Fail, expected: dict, opsd: dict, counter: list, i: int) -> None:
    names = list(opsd)
    r = random.Random(i)
    while not stop.is_set():
        n = r.choice(names)
        try:
            got = opsd[n]()
        except Exception as e:  # noqa: BLE001
            fail(f"{n}: raised {e!r}")
            continue
        if got != expected[n]:
            fail(f"{n}: {got!r} != {expected[n]!r}")
        counter[i] += 1


def scenario_toggle(a) -> int:
    import gc
    import importlib

    opsd = ops()
    expected = {k: f() for k, f in opsd.items()}
    import rustique

    rustique.enable()
    stop = threading.Event()
    fail = Fail()
    counter = [0] * (a.workers + 8)
    toggles = [0]

    enable_errors: dict = {}

    def toggler():
        while not stop.is_set():
            rustique.disable()
            try:
                rustique.enable()
            except BaseException as e:  # noqa: BLE001  (pyo3's PanicException is a BaseException)
                enable_errors[repr(e)] = enable_errors.get(repr(e), 0) + 1
            toggles[0] += 1

    def importer():
        while not stop.is_set():
            sys.modules.pop("colorsys", None)
            importlib.import_module("colorsys")

    def collector():
        while not stop.is_set():
            gc.collect()
            time.sleep(0.01)

    ts = [threading.Thread(target=workers_loop, args=(stop, fail, expected, opsd, counter, i)) for i in range(a.workers)]
    ts += [threading.Thread(target=toggler), threading.Thread(target=importer), threading.Thread(target=collector)]
    for t in ts:
        t.start()
    time.sleep(a.secs)
    stop.set()
    for t in ts:
        t.join()
    print(f"toggle: {sum(counter)} calls, {toggles[0]} disable/enable cycles, {fail.n} wrong results, "
          f"enable() errors {enable_errors}")
    for m in fail.msgs:
        print("  ", m)
    return 1 if fail.n else 0


def scenario_patch(a) -> int:
    import json
    import logging
    import os as os_mod
    import posixpath
    from unittest import mock

    import copy

    opsd = ops()
    expected = {k: f() for k, f in opsd.items()}
    import rustique

    rustique.enable()

    # (name, target to patch, attribute, entry point call that must reach the patched object)
    calls = {"n": 0}
    checker_id = [None]

    def counting(orig):
        def w(*args, **kw):
            if threading.get_ident() == checker_id[0]:  # hammer threads may call it too
                calls["n"] += 1
            return orig(*args, **kw)
        return w

    import _strptime

    cases = [
        ("os.fspath via posixpath.join", os_mod, "fspath", lambda: posixpath.join("/srv", "x")),
        ("json.encoder.encode_basestring_ascii via json.dumps", json.encoder, "encode_basestring_ascii",
         lambda: json.dumps({"a": "b"})),
        ("_strptime._strptime_datetime_datetime via datetime.strptime", _strptime, "_strptime_datetime_datetime",
         lambda: __import__("datetime").datetime.strptime("2026-11-02", "%Y-%m-%d")),
        ("logging.Formatter.converter via formatTime", logging.Formatter, "converter",
         lambda: logging.Formatter("%(asctime)s").format(logging.LogRecord("n", 20, "p", 1, "m", None, None))),
    ]
    # copy._deepcopy_dispatch contents (a dict pinned by snapshot): replace the list entry.
    disp = copy._deepcopy_dispatch

    stop = threading.Event()
    fail = Fail()
    counter = [0] * (a.workers + 1)
    hammer = [threading.Thread(target=workers_loop, args=(stop, fail, expected, opsd, counter, i))
              for i in range(a.workers)]
    for t in hammer:
        t.start()
    go, ack = threading.Event(), threading.Event()
    box: dict = {}
    tally: dict = {}

    def checker():
        checker_id[0] = threading.get_ident()
        while True:
            go.wait()
            go.clear()
            job = box.get("job")
            if job is None:
                return
            name, call, want_called = job
            before = calls["n"]
            try:
                call()
            except Exception as e:  # noqa: BLE001
                fail(f"{name}: raised {e!r}")
            called = calls["n"] > before
            if called != want_called:
                fail(f"{name}: patched object {'not ' if want_called else ''}called (want {want_called})")
            tally[(name, want_called, called)] = tally.get((name, want_called, called), 0) + 1
            ack.set()

    ct = threading.Thread(target=checker)
    ct.start()

    def handoff(name, call, want):
        box["job"] = (name, call, want)
        go.set()
        ack.wait()
        ack.clear()

    deadline = time.monotonic() + a.secs
    rounds = 0
    while time.monotonic() < deadline:
        for name, obj, attr, call in cases:
            orig = getattr(obj, attr)
            wrapper = counting(orig)
            with mock.patch.object(obj, attr, wrapper if attr != "converter" else staticmethod(wrapper)):
                handoff(name, call, True)
            handoff(name, call, False)
        orig_list = disp[list]
        disp[list] = counting(orig_list)
        handoff("copy._deepcopy_dispatch[list] via deepcopy", lambda: copy.deepcopy([1, [2]]), True)
        disp[list] = orig_list
        handoff("copy._deepcopy_dispatch[list] via deepcopy", lambda: copy.deepcopy([1, [2]]), False)
        rounds += 1
    box["job"] = None
    go.set()
    ct.join()
    stop.set()
    for t in hammer:
        t.join()
    print(f"patch: {rounds} rounds x {len(cases) + 1} pins, {sum(counter)} hammer calls, {fail.n} failures")
    for (name, want, got), k in sorted(tally.items()):
        print(f"   {name:60s} want_called={want!s:5s} called={got!s:5s} x{k}")
    for m in fail.msgs:
        print("  ", m)
    return 1 if fail.n else 0


def scenario_tables(a) -> int:
    import copy
    import gc
    import importlib
    import json

    import rustique

    rustique.enable()
    stop = threading.Event()
    n = {"toggle": 0, "mut": 0, "imp": 0, "gc": 0}

    errors: dict = {}

    def toggler():
        while not stop.is_set():
            rustique.disable()
            try:
                rustique.enable()
            except BaseException as e:  # noqa: BLE001
                errors[repr(e)] = errors.get(repr(e), 0) + 1
            n["toggle"] += 1

    def mutate_copy():
        g = copy.__dict__
        i = 0
        while not stop.is_set():
            g["_stress_key"] = i
            del g["_stress_key"]
            i += 1
            n["mut"] += 1

    def mutate_json():
        d = json._default_encoder.__dict__
        while not stop.is_set():
            d["_stress"] = 1
            del d["_stress"]

    def importer():
        while not stop.is_set():
            sys.modules.pop("colorsys", None)
            importlib.import_module("colorsys")
            n["imp"] += 1

    def collector():
        while not stop.is_set():
            gc.collect()
            n["gc"] += 1

    def caller():
        x = {"a": [1, {"b": frozenset({2})}]}
        while not stop.is_set():
            copy.deepcopy(x)
            json.dumps({"k": [1, 2]})

    ts = [threading.Thread(target=f) for f in (toggler, mutate_copy, mutate_copy, mutate_json, importer, collector,
                                               caller, caller)]
    for t in ts:
        t.start()
    time.sleep(a.secs)
    stop.set()
    for t in ts:
        t.join()
    print(f"tables: no hang; {n}; enable() errors {errors}")
    return 2 if errors else 0


CHILD = r'''
import os, random, sys, threading
sys.path.insert(0, os.environ["STRESS_DIR"])
import stress
import rustique
rustique.enable()
opsd = stress.ops()
def loop(i):
    names = list(opsd)
    r = random.Random(i)
    while True:
        opsd[r.choice(names)]()
for i in range(6):
    threading.Thread(target=loop, args=(i,), daemon=True).start()
import time
time.sleep(random.Random(int(sys.argv[1])).uniform(0.0, 0.05))
if int(sys.argv[1]) % 2:
    sys.exit(0)
'''


def scenario_shutdown(a) -> int:
    env = dict(os.environ, STRESS_DIR=os.path.dirname(os.path.abspath(__file__)))
    bad = 0
    for i in range(a.runs):
        r = subprocess.run([sys.executable, "-c", CHILD, str(i)], capture_output=True, text=True, env=env, timeout=120)
        if r.returncode != 0 or "Fatal" in r.stderr or "Segmentation" in r.stderr:
            bad += 1
            if bad <= 5:
                print(f"run {i}: exit {r.returncode}\n{r.stderr[-1500:]}")
    print(f"shutdown: {a.runs} runs, {bad} bad exits")
    return 1 if bad else 0


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("scenario", choices=["toggle", "patch", "tables", "shutdown"])
    ap.add_argument("--secs", type=float, default=20)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--runs", type=int, default=100)
    a = ap.parse_args()
    arm(a.secs if a.scenario != "shutdown" else a.runs * 2)
    print(f"{sys.version.split()[0]} gil={getattr(sys, '_is_gil_enabled', lambda: True)()} scenario={a.scenario}", flush=True)
    rc = {"toggle": scenario_toggle, "patch": scenario_patch, "tables": scenario_tables,
          "shutdown": scenario_shutdown}[a.scenario](a)
    sys.exit(rc)


if __name__ == "__main__":
    main()
