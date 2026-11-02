"""``python -m rustique estimate`` (rustique/_estimate.py): the sampler, the profiler, the
attribution model and the manifest of entry points (docs/targets/dx.md §5)."""

import json
import os
import subprocess
import sys
import textwrap
import threading
import time

import pytest

from rustique import _estimate as E

FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()


def busy(seconds: float) -> None:
    end = time.process_time() + seconds
    while time.process_time() < end:
        pass


# --- the manifest of entry points ------------------------------------------------------------


def test_manifest_from_status_finds_new_targets():
    status = {"targets": [
        {"name": "json.dumps", "group": "json", "module": "json", "sites": ["dumps (vectorcall)"]},
        {"name": "re.compile_new", "group": "re", "module": "re._compiler", "sites": ["compile (vectorcall)"]},
        {"name": "set.__reduce__", "group": "set", "module": None, "sites": ["set.__reduce__ (ml_meth)"]},
        {"name": "datetime.date.weekday2", "group": "datetime", "module": "_datetime",
         "sites": ["datetime.date.weekday (ml_meth)"]},
        {"name": "enum.EnumType.__call__", "group": "enum", "module": "enum", "sites": ["EnumType.tp_call (heap slot)"]},
        {"name": "list.__contains__", "group": "containers", "module": None, "sites": ["list.sq_contains (slot)"]},
        {"name": "logging.Logger.level", "group": "logging", "module": "logging", "sites": []},
        {"name": "interp.isinstance", "group": "interp", "tier": "interp", "sites": ["ISINSTANCE (handler)"]},
    ]}
    m = E.manifest_from_status(status)
    assert m["py"]["json.dumps"] == E.PY_SITES["json.dumps"]  # the table wins for known targets
    assert m["py"]["re.compile_new"] == ("re._compiler", ["compile"])
    assert m["c"]["set.__reduce__"] == ("builtins:set", ["__reduce__"])
    assert m["c"]["datetime.date.weekday2"] == ("datetime:date", ["weekday"])
    assert m["slots"] == ["list.__contains__"]
    assert set(m["other"]) == {"enum.EnumType.__call__", "interp.isinstance"}
    assert {"logging.Logger.level", "interp.isinstance"} <= set(m["no_replay"])
    assert json.loads(json.dumps(m))  # it travels to the plain runs as JSON


def test_manifest_without_status_uses_the_tables():
    m = E.manifest_from_status(None)
    assert m["py"] == E.PY_SITES and m["c"] == E.C_SITES


# --- the attribution model -------------------------------------------------------------------


def test_chains_go_to_the_kernel_that_takes_the_call():
    sampled = {"chains": {"logging.Logger.level>logging.Formatter.format": 0.4, "pathlib.PurePath.relative_to>"
                          "pathlib.PurePath.__init__": 0.2, "json.dumps": 0.1}}
    counters = {"logging.Logger.level": {"hits": 0, "fallback": {"deopt": 10}},
                "pathlib.PurePath.relative_to": {"hits": 10, "fallback": {}},
                "logging.Formatter.format": {"hits": 5, "fallback": {}}}
    shares = E.assign_chains(sampled, counters, {})
    # Logger.level always falls back: its time is the formatter's; relative_to always hits: its
    # kernel replaces the __init__ calls under it
    assert shares == {"logging.Logger.level": 0.0, "logging.Formatter.format": 0.4,
                      "pathlib.PurePath.relative_to": 0.2, "json.dumps": 0.1}
    counters["logging.Logger.level"] = {"hits": 1, "fallback": {"deopt": 3}}
    shares = E.assign_chains(sampled, counters, {})
    assert shares["logging.Logger.level"] == pytest.approx(0.1) and shares["logging.Formatter.format"] == pytest.approx(0.3)


def test_predict_counts_cpu_time_and_waiting():
    profile = {"targets": {"json.dumps": {"calls": 10, "ns": 0, "kind": "py", "nested_events": 0},
                           "builtins.min": {"calls": 100, "ns": 50e6, "kind": "c"}}}
    replayed = {"json.dumps": {"ratio": 0.5, "samples": 5, "hits": 5, "fallbacks": 0}}
    counters = {"builtins.min": {"hits": 100, "fallback": {}}}
    # half of the wall time is waiting: the CPU-time share of json.dumps is of the CPU half
    sampled = {"samples": 1000, "mode": "signal", "cpu_ns": 500e6, "wall_ns": 1000e6, "shares": {"json.dumps": 0.4},
               "chains": {"json.dumps": 0.4}}
    pred = E.predict(profile, replayed, counters, 1000e6, sampled)
    saved = 0.4 * 500e6 * 0.5 + 50e6 * (1 - E.C_RATIO["builtins.min"])
    assert pred["ratio"] == pytest.approx(1 - saved / 1000e6)
    rows = {r["target"]: r for r in pred["rows"]}
    assert rows["json.dumps"]["source"].startswith("replayed 5 calls")
    assert rows["builtins.min"]["source"].startswith("table")


def test_predict_without_model_promises_nothing():
    profile = {"targets": {"new.c_target": {"calls": 10, "ns": 5e6, "kind": "c"}}}
    pred = E.predict(profile, {}, {"new.c_target": {"hits": 10, "fallback": {}}}, 10e6)
    assert pred["ratio"] == 1.0 and pred["rows"][0]["source"].startswith("no model")


# --- the sampler -----------------------------------------------------------------------------


def _target_code():
    import posixpath

    def work():
        end = time.process_time() + 0.25
        while time.process_time() < end:
            posixpath.join("a", "b", "c")
    return work


def test_signal_sampler_samples_cpu_not_waiting():
    if not hasattr(__import__("signal"), "setitimer"):
        pytest.skip("no setitimer")
    work = _target_code()
    s = E.Sampler(interval=0.001).start()
    try:
        work()
        time.sleep(0.25)  # waiting: no CPU, no samples
    finally:
        s.stop()
    sh = s.shares()
    assert sh["mode"] == "signal" and sh["samples"] > 50
    # most CPU samples are under posixpath.join; the sleep is not sampled
    assert sh["shares"].get("posixpath.join", 0) > 0.3, sh
    assert sh["cpu_ns"] < 0.8 * sh["wall_ns"]


def test_signal_sampler_is_safe_with_threads():
    """Other threads' stacks are read only where it is safe (GIL builds; on free-threaded
    builds their CPU is unattributed): no crash, every sample accounted for."""
    if not hasattr(__import__("signal"), "setitimer"):
        pytest.skip("no setitimer")
    work = _target_code()
    stop = threading.Event()

    def spin():
        import json

        while not stop.is_set():
            json.dumps({"a": [1, 2, 3]})

    threads = [threading.Thread(target=spin) for _ in range(4)]
    s = E.Sampler(interval=0.0005).start()
    try:
        for t in threads:
            t.start()
        work()
    finally:
        s.stop()
        stop.set()
        for t in threads:
            t.join()
    sh = s.shares()
    assert sh["samples"] >= 3  # signals coalesce while the main thread waits for the GIL
    if FREE_THREADED:
        assert sh["unattributed"] > 0
    else:
        assert sum(sh["shares"].values()) <= 1.0 + 1e-9


def test_sampler_restores_the_signal_handler():
    import signal

    if not hasattr(signal, "setitimer"):
        pytest.skip("no setitimer")
    before = signal.getsignal(signal.SIGPROF)
    E.Sampler().start().stop()
    assert signal.getsignal(signal.SIGPROF) == before
    assert signal.getitimer(signal.ITIMER_PROF) == (0.0, 0.0)


def test_sampler_steps_aside_for_a_program_using_sigprof():
    import signal

    if not hasattr(signal, "setitimer"):
        pytest.skip("no setitimer")
    mine = lambda *a: None  # noqa: E731
    old = signal.signal(signal.SIGPROF, mine)
    try:
        s = E.Sampler().start()
        s.stop()
        assert s.mode != "signal" and signal.getsignal(signal.SIGPROF) is mine
    finally:
        signal.signal(signal.SIGPROF, old)


# --- the profiler ----------------------------------------------------------------------------


def test_profiler_times_targets_and_c_children():
    import json

    def default(o):
        return sorted(o)  # a C-level target inside a Python-level one

    p = E.Profiler().start()
    try:
        for i in range(200):
            json.dumps({"s": {3, 1, 2}, "i": i}, default=default)
    finally:
        p.stop()
    r = p.result()["targets"]
    assert r["json.dumps"]["calls"] == 200 and r["builtins.sorted"]["calls"] == 200
    assert r["json.dumps"]["c_child_ns"] > 0  # samples would see sorted's time under dumps
    assert not p.result()["thread_clock"]


def test_profiler_switches_to_thread_cpu_time_with_threads():
    import posixpath

    t = threading.Thread(target=busy, args=(0.2,))
    p = E.Profiler().start()
    try:
        t.start()
        for _ in range(100):
            posixpath.join("a", "b")
    finally:
        p.stop()
        t.join()
    res = p.result()
    assert res["thread_clock"] and res["targets"]["posixpath.join"]["calls"] == 100


# --- the command -----------------------------------------------------------------------------


def test_estimate_command_on_a_waiting_program(tmp_path):
    prog = tmp_path / "prog.py"
    prog.write_text(textwrap.dedent("""
        import json, time
        for i in range(20000):
            json.dumps({"a": [i, i + 1], "b": "x" * 20})
        time.sleep(0.3)
    """))
    env = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    r = subprocess.run([sys.executable, "-m", "rustique", "estimate", "--json", "--", sys.executable, str(prog)],
                       capture_output=True, text=True, env=env, cwd=tmp_path, timeout=600)
    assert r.returncode == 0, r.stderr
    out = json.loads(r.stdout)
    pred = out["prediction"]
    row = next(x for x in pred["rows"] if x["target"] == "json.dumps")
    assert row["calls"] == 20000 and 0 < row["ratio"] < 1
    if out["sampler"]["mode"] == "signal":
        assert any("waited" in n for n in out["notes"]), out["notes"]
