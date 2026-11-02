"""Will Rustique help this program? Measure it on the program itself (prototype).

``python -m rustique estimate -- <command>`` (see :func:`main`) runs the command twice:

1. **plain**, with a ``sys.monitoring`` profiler that times every call of every accelerated
   entry point (outermost calls only; the profiler's own cost per event is calibrated and
   subtracted) and keeps a small uniform sample of the arguments of the Python-level ones.
   At exit, each sampled call is *replayed* twice: with CPython's original, then with the
   Rust kernel (``rustique.enable(only=<target>)``), timed in a loop. That gives, on the
   program's own data, the per-call ratio and how often the kernel would fall back.
2. **enabled with counters** (what ``report`` does): hits and fallbacks of the targets that
   cannot be replayed (C-level methods, operators, logging calls that emit records).

The prediction: ``ratio = 1 - sum(time under target t x (1 - r_t)) / total time``, where
``r_t`` is the replayed ratio (Python-level targets), or the per-call ratio of the target's
docs x its hit rate (C-level targets), or hits x the measured saving per hit (``in`` on
tuples/lists, ``float.__repr__``). Everything outside accelerated entry points is assumed
unchanged: the out-of-reach share is printed too.

Replays only call targets that have no side effects on these inputs (they are run after the
program finished, on copies where state matters: ``random`` replays on a clone of the
generator). ``logging`` calls are never replayed.

In-process use (the validation harness, ``benches/dx/estimate_workloads.py``)::

    prof = Profiler(); prof.start(); work(); prof.stop()
    data = prof.result()          # per target: calls, ns (corrected), samples
    data["replay"] = replay(prof)  # per target: plain ns, kernel ns, hits, fallbacks
"""

import _thread
import os
import random as _random_mod
import sys
import time

TOOL_ID = 4  # 0 debugger, 1 coverage, 2 profiler, 5 optimizer: 3 and 4 are free by convention
NS = time.perf_counter_ns
SAMPLES = 24  # argument samples kept per target (reservoir)

# --- the targets' entry points --------------------------------------------------------------

# Python-level targets (their function's vectorcall is the site): module, qualnames.
PY_SITES = {
    "json.dumps": ("json", ["dumps"]),
    "json.loads": ("json", ["loads"]),
    "logging.Logger.level": ("logging", ["Logger.debug", "Logger.info", "Logger.warning", "Logger.error",
                                         "Logger.critical", "Logger.log"]),
    "logging.Logger.findCaller": ("logging", ["Logger.findCaller"]),
    "logging.LogRecord.__init__": ("logging", ["LogRecord.__init__"]),
    "logging.Formatter.formatTime": ("logging", ["Formatter.formatTime"]),
    "logging.Formatter.format": ("logging", ["Formatter.format"]),
    **{f"random.{n}": ("random", [f"Random.{n}"]) for n in ("randint", "randrange", "choice", "shuffle", "sample",
                                                             "choices")},
    **{f"statistics.{n}": ("statistics", [n]) for n in ("mean", "variance", "pvariance", "stdev", "pstdev")},
    **{f"textwrap.{n}": ("textwrap", [n]) for n in ("wrap", "fill", "shorten", "dedent", "indent")},
    "textwrap.TextWrapper.wrap": ("textwrap", ["TextWrapper.wrap"]),
    "textwrap.TextWrapper.fill": ("textwrap", ["TextWrapper.fill"]),
    **{f"urllib.parse.{n}": ("urllib.parse", [n]) for n in ("quote", "quote_plus", "unquote", "unquote_plus",
                                                            "parse_qsl", "parse_qs", "_urlsplit")},
    **{f"posixpath.{n}": ("posixpath", [n]) for n in ("join", "split", "splitext", "basename", "dirname", "isabs",
                                                      "relpath", "abspath")},
    **{f"fnmatch.{n}": ("fnmatch", [n]) for n in ("fnmatch", "fnmatchcase", "filter")},
    **{f"pathlib.PurePath.{n}": ("pathlib", [f"PurePath.{n}"]) for n in (
        "__new__", "__init__", "__str__", "__truediv__", "parent", "name", "suffix", "stem", "parts", "relative_to",
        "__eq__", "__hash__")},
    "pathlib.Path.__new__": ("pathlib", ["Path.__new__"]),
    "copy.deepcopy": ("copy", ["deepcopy"]),
    "copy.copy": ("copy", ["copy"]),
    "dataclasses.asdict": ("dataclasses", ["asdict"]),
    "dataclasses.astuple": ("dataclasses", ["astuple"]),
    # C entry points that run a Python helper: timed and replayed through the helper's frame
    "datetime.datetime.strptime": ("_strptime", ["_strptime_datetime_datetime"]),
    "datetime.date.strptime": ("_strptime", ["_strptime_datetime_date"]),
    "datetime.time.strptime": ("_strptime", ["_strptime_datetime_time"]),
    "time.strptime": ("_strptime", ["_strptime_time"]),
}
# C-level targets (ml_meth sites): (owner module or class path, method names).
C_SITES = {
    "list.sort": ("builtins:list", ["sort"]), "list.index": ("builtins:list", ["index"]),
    "list.count": ("builtins:list", ["count"]), "tuple.index": ("builtins:tuple", ["index"]),
    "tuple.count": ("builtins:tuple", ["count"]),
    **{f"builtins.{n}": ("builtins", [n]) for n in ("sum", "min", "max", "sorted", "any", "all")},
    "datetime.date.strftime": ("datetime:date datetime:datetime", ["strftime"]),
    "datetime.time.strftime": ("datetime:time", ["strftime"]),
    "time.strftime": ("time", ["strftime"]),
    "datetime.datetime.isoformat": ("datetime:datetime", ["isoformat"]),
    "datetime.date.isoformat": ("datetime:date", ["isoformat"]),
    "datetime.time.isoformat": ("datetime:time", ["isoformat"]),
}
# Per-call ratio (enabled / plain) of C-level targets on typical small inputs where a call
# cannot be replayed (CALL events carry no arguments): measured, benches/dx/c_ratios.py on
# the dev box (3.15.1, release wheel, geometric mean of 1-3 statements); see docs/atlas/w2-dx.md
# §5. list.sort/sorted: 0.4-3.2 depending on order and element type (presorted strings are
# slower), so no saving is assumed.
C_RATIO = {
    "list.sort": 1.0, "list.index": 0.45, "list.count": 0.40, "tuple.index": 0.47, "tuple.count": 0.44,
    "builtins.sum": 0.51, "builtins.min": 0.46, "builtins.max": 0.34, "builtins.sorted": 1.0,
    "builtins.any": 0.78, "builtins.all": 0.42,
    "datetime.date.strftime": 0.09, "datetime.time.strftime": 0.13, "time.strftime": 0.36,
    "datetime.datetime.isoformat": 0.15, "datetime.date.isoformat": 0.23, "datetime.time.isoformat": 0.26,
}
# Python-level targets that are never replayed (they emit log records): per-call ratio from
# docs/targets/logging.md (callgrind, one record through a handler).
NO_REPLAY_RATIO = {"logging.Logger.level": 0.60, "logging.Logger.findCaller": 0.45, "logging.LogRecord.__init__": 0.50,
                   "logging.Formatter.formatTime": 0.30, "logging.Formatter.format": 0.50}
# Slot targets (no call event at all): time saved per accelerated call, ns (benches/dx/c_ratios.py:
# `in` on a 4-element tuple 7-16 ns, on a 20-element list 51-106 ns; operands of `in` in real
# code are mostly short, so both use the short-container value).
SLOT_SAVING_NS = {"tuple.__contains__": 12.0, "list.__contains__": 12.0, "float.repr": 188.0}


def _lookup(modname: str, qual: str):
    mod = sys.modules.get(modname)
    if mod is None:
        return None
    obj = mod
    for part in qual.split("."):
        d = getattr(obj, "__dict__", None)
        obj = d.get(part) if d is not None and part in d else getattr(obj, part, None)
        if obj is None:
            return None
    return obj


def _code_of(obj):
    if isinstance(obj, (staticmethod, classmethod)):
        obj = obj.__func__
    if isinstance(obj, property):
        obj = obj.fget
    return getattr(obj, "__code__", None)


# --- the profiler -----------------------------------------------------------------------------


class Profiler:
    """Times calls of the targets' entry points with ``sys.monitoring`` (plain runs only:
    Rustique's Python-level kernels step aside while a tool listens)."""

    def __init__(self, samples: int = SAMPLES) -> None:
        self.samples = samples
        self.codes: dict = {}  # code object -> target name
        self.funcs: dict = {}  # code object -> function (for replay)
        self.cfuncs: dict = {}  # (owner, name) -> target name
        self.ns: dict[str, int] = {}
        self.calls: dict[str, int] = {}
        self.events: dict[str, int] = {}  # monitoring events inside each target's calls
        self.ns_outer: dict[str, int] = {}  # the same for calls made outside any other target
        self.events_outer: dict[str, int] = {}
        self.calls_outer: dict[str, int] = {}
        self.kept: dict[str, list] = {}
        self.longest: dict[str, tuple] = {}  # target -> (ns, call): its longest outermost call
        self.stride: dict[str, int] = {}
        self.seen: dict[str, int] = {}
        self.stacks: dict[int, list] = {}
        self.nmods = -1
        self.retry_at = None
        self.misses: dict = {}
        self.wall0 = 0
        self.wall = 0
        self.total_events = 0
        self.kappa = None
        self.rng = _random_mod.Random(1234)

    # resolution (repeated whenever sys.modules grows: targets of modules imported later)
    def _resolve(self) -> None:
        E = sys.monitoring.events
        pending = False
        for name, (modname, quals) in PY_SITES.items():
            for q in quals:
                obj = _lookup(modname, q)
                code = _code_of(obj) if obj is not None else None
                if code is None:
                    # module imported but still running its body (we are called from inside
                    # the import): look again later
                    pending = pending or modname in sys.modules
                elif code not in self.codes:
                    self.codes[code] = name
                    self.funcs[code] = obj.fget if isinstance(obj, property) else obj
                    sys.monitoring.set_local_events(TOOL_ID, code, E.PY_START | E.PY_RETURN)
        self.retry_at = self.total_events + 2000 if pending else None
        for name, (owners, meths) in C_SITES.items():
            for owner in owners.split():
                modname, _, cls = owner.partition(":")
                o = _lookup(modname, cls) if cls else sys.modules.get(modname)
                if o is None:
                    continue
                for m in meths:
                    self.cfuncs[(id(o), m)] = name
        self.nmods = len(sys.modules)

    def _ckey(self, c):
        t = type(c).__name__
        if t == "method_descriptor":
            return (id(c.__objclass__), c.__name__)
        if t == "builtin_function_or_method":
            s = c.__self__
            if s is None or isinstance(s, type) or type(s).__name__ == "module":
                return (id(s), c.__name__)
            return (id(type(s)), c.__name__)
        return None

    # callbacks
    def _call(self, code, off, c, arg0):
        self.total_events += 1
        if len(sys.modules) != self.nmods or (self.retry_at is not None and self.total_events > self.retry_at):
            self._resolve()
        k = self._ckey(c)
        name = self.cfuncs.get(k) if k is not None else None
        if name is None:
            key = (code, off)
            n = self.misses.get(key, 0) + 1
            if n >= 3:  # three misses: not a site of a C-level target
                self.misses.pop(key, None)
                return sys.monitoring.DISABLE
            self.misses[key] = n
            return None
        tid = _thread.get_ident()
        st = self.stacks.get(tid)
        if st is None:
            st = self.stacks[tid] = []
        self._push(st, name, c)
        return None

    # Stack entries: [name, key, events inside, t0 (None: a recursive call, folded into the
    # enclosing call of the same target), time of timed children]. Time is exclusive: a nested
    # call of *another* target is timed on its own and subtracted from its parent (``any`` or
    # ``sorted`` with a key calling ``fnmatch``); a recursive call of the same target stays in
    # the outermost one (``deepcopy``: the kernel replaces the whole recursion).
    def _push(self, st, name, key):
        timed = None
        for e in reversed(st):
            if e[3] is not None:
                timed = e
                break
        if timed is not None:
            timed[2] += 1  # this callback ran inside the innermost timed call
            for e in st:
                if e[0] == name:
                    st.append([name, key, 0, None, 0])
                    return False
        st.append([name, key, 0, NS(), 0])
        return True

    def _pop(self, st, key, t):
        for i in range(len(st) - 1, -1, -1):
            if st[i][1] is key:
                e = st[i]
                del st[i:]
                parent = None
                for p in reversed(st):
                    if p[3] is not None:
                        parent = p
                        break
                if e[3] is None:  # folded recursion
                    if parent is not None:
                        parent[2] += 1
                    return
                interval = t - e[3]
                name = e[0]
                if len(e) > 5 and e[5] is not None and interval > self.longest.get(name, (0,))[0]:
                    self.longest[name] = (interval, e[5])
                self.ns[name] = self.ns.get(name, 0) + interval - e[4]
                self.calls[name] = self.calls.get(name, 0) + 1
                self.events[name] = self.events.get(name, 0) + e[2]
                if parent is None and not st:  # not inside any other target: what a stack sample cannot see
                    self.ns_outer[name] = self.ns_outer.get(name, 0) + interval - e[4]
                    self.events_outer[name] = self.events_outer.get(name, 0) + e[2]
                    self.calls_outer[name] = self.calls_outer.get(name, 0) + 1
                if parent is not None:
                    parent[4] += interval
                    parent[2] += 2  # the child's own start/end callbacks, outside its interval
                return

    def _creturn(self, code, off, c, arg0):
        self.total_events += 1
        if len(sys.modules) != self.nmods or (self.retry_at is not None and self.total_events > self.retry_at):
            self._resolve()
        st = self.stacks.get(_thread.get_ident())
        if st:
            self._pop(st, c, NS())

    def _start(self, code, off):
        self.total_events += 1
        if len(sys.modules) != self.nmods or (self.retry_at is not None and self.total_events > self.retry_at):
            self._resolve()
        name = self.codes.get(code)
        if name is None:
            return
        tid = _thread.get_ident()
        st = self.stacks.get(tid)
        if st is None:
            st = self.stacks[tid] = []
        if any(e[0] == name for e in st):
            self._push(st, name, code)
            return
        n = self.seen.get(name, 0) + 1
        self.seen[name] = n
        replay = name not in NO_REPLAY_RATIO
        if replay and n % self.stride.get(name, 1) == 0:
            self._keep(name, code, sys._getframe(1), n)
        big = None
        if replay and not st and (n <= 16 or self.longest.get(name, (0,))[0] > 200_000):
            # outermost calls of a target whose calls can be long: keep the args of the longest
            # one too (a few huge calls can hold most of the time: one deepcopy of a big graph);
            # captured before the clock starts
            big = self._capture(code, sys._getframe(1))
        self._push(st, name, code)
        if big is not None:
            st[-1].append(big)

    def _keep(self, name, code, frame, n):
        call = self._capture(code, frame)
        if call is None:
            return
        kept = self.kept.setdefault(name, [])
        kept.append(call)
        if len(kept) >= 2 * self.samples:  # systematic sample: double the stride, keep every other call
            del kept[::2]
            self.stride[name] = self.stride.get(name, 1) * 2

    def _capture(self, code, frame):
        loc = frame.f_locals
        npos = code.co_argcount
        nkw = code.co_kwonlyargcount
        names = code.co_varnames
        func = self.funcs[code]
        defaults = getattr(func, "__defaults__", None) or ()
        kwdefaults = getattr(func, "__kwdefaults__", None) or {}
        try:
            args = [loc[v] for v in names[:npos]]
            # pass what the caller most likely passed: drop trailing defaults, keyword-only defaults
            nd = len(defaults)
            while nd and args and len(args) > npos - len(defaults) and args[-1] is defaults[nd - 1]:
                args.pop()
                nd -= 1
            kwargs = {v: loc[v] for v in names[npos:npos + nkw] if not (v in kwdefaults and loc[v] is kwdefaults[v])}
            i = npos + nkw
            if code.co_flags & 0x04:  # *args
                args += list(loc[names[i]])
                i += 1
            if code.co_flags & 0x08:  # **kwargs
                kwargs.update(loc[names[i]])
        except KeyError:
            return None
        return (func, tuple(args), kwargs)

    def _end(self, code, off, val):
        self.total_events += 1
        st = self.stacks.get(_thread.get_ident())
        if st and code in self.codes:
            self._pop(st, code, NS())

    # control
    def start(self) -> "Profiler":
        if self.kappa is None:
            self.kappa = calibrate()
        mon = sys.monitoring
        E = mon.events
        mon.use_tool_id(TOOL_ID, "rustique-estimate")
        mon.register_callback(TOOL_ID, E.CALL, self._call)
        mon.register_callback(TOOL_ID, E.C_RETURN, self._creturn)
        mon.register_callback(TOOL_ID, E.C_RAISE, self._creturn)
        mon.register_callback(TOOL_ID, E.PY_START, self._start)
        mon.register_callback(TOOL_ID, E.PY_RETURN, self._end)
        mon.register_callback(TOOL_ID, E.PY_UNWIND, self._end)
        self._resolve()
        mon.set_events(TOOL_ID, E.CALL | E.C_RETURN | E.C_RAISE | E.PY_UNWIND)
        self.wall0 = NS()
        return self

    def stop(self) -> "Profiler":
        self.wall += NS() - self.wall0
        mon = sys.monitoring
        mon.set_events(TOOL_ID, 0)
        for code in self.codes:
            mon.set_local_events(TOOL_ID, code, 0)
        for ev in (mon.events.CALL, mon.events.C_RETURN, mon.events.C_RAISE, mon.events.PY_START,
                   mon.events.PY_RETURN, mon.events.PY_UNWIND):
            mon.register_callback(TOOL_ID, ev, None)
        mon.free_tool_id(TOOL_ID)
        mon.restart_events()
        self.stacks.clear()
        return self

    def result(self, plain_ns: float | None = None) -> dict:
        """Per target: calls, time (ns) with the profiler's own cost subtracted. The cost per
        event is calibrated on a loop (:func:`calibrate`); with ``plain_ns`` (the same work
        timed without the profiler) it is rescaled so that all events together explain the
        measured slowdown of this run."""
        k = dict(self.kappa or {})
        scale = 1.0
        if plain_ns and self.total_events and k.get("event"):
            k["observed_scale"] = (self.wall - plain_ns) / self.total_events / k["event"]
        k["scale"] = scale
        out = {}
        for name, ns in self.ns.items():
            kind = "c" if name in C_SITES else "py"
            per_call = k.get("c" if kind == "c" else "py", 0.0) * scale
            corrected = ns - self.calls[name] * per_call - self.events.get(name, 0) * k.get("event", 0.0) * scale
            co = self.calls_outer.get(name, 0)
            outer = self.ns_outer.get(name, 0) - co * per_call - self.events_outer.get(name, 0) * k.get("event", 0.0) * scale
            out[name] = {"calls": self.calls[name], "ns_raw": ns, "ns": max(corrected, 0.0), "kind": kind,
                         "ns_outer": max(outer, 0.0), "calls_outer": co,
                         "nested_events": self.events.get(name, 0),
                         "samples": len(self.kept.get(name, []))}
        return {"targets": out, "wall_ns_instrumented": self.wall, "kappa": k, "events": self.total_events}


class Sampler:
    """Share of wall time under each Python-level target, by sampling the stack of one thread
    (``sys._current_frames``) every ``interval`` seconds: no per-call cost, so nested and
    recursive targets (``deepcopy``) are measured as accurately as flat ones. The outermost
    target frame on the stack gets the sample."""

    def __init__(self, interval: float = 0.0005, thread_id: int | None = None) -> None:
        self.interval = interval
        self.tid = thread_id
        self.codes: dict = {}
        self.counts: dict[str, int] = {}
        self.total = 0
        self.nmods = -1
        self.pending = False
        self._stop = False
        self._thread = None
        self._switch = None
        self.disabled = None

    def _resolve(self) -> None:
        pending = False
        for name, (modname, quals) in PY_SITES.items():
            for q in quals:
                obj = _lookup(modname, q)
                code = _code_of(obj) if obj is not None else None
                if code is not None:
                    self.codes[code] = name
                else:
                    pending = pending or modname in sys.modules
        self.pending = pending
        self.nmods = len(sys.modules)

    def _loop(self) -> None:
        codes = self.codes
        get = sys._current_frames
        loops = 0
        while not self._stop:
            time.sleep(self.interval)
            loops += 1
            if len(sys.modules) != self.nmods or (self.pending and loops % 64 == 0):
                self._resolve()
            f = get().get(self.tid)
            outer = None
            while f is not None:
                n = codes.get(f.f_code)
                if n is not None:
                    outer = n
                f = f.f_back
            self.total += 1
            if outer is not None:
                self.counts[outer] = self.counts.get(outer, 0) + 1

    def start(self) -> "Sampler":
        import threading

        if not getattr(sys, "_is_gil_enabled", lambda: True)():
            # Free-threaded build: the frames sys._current_frames() returns keep running after
            # it restarts the world, and walking another running thread's f_back segfaults
            # (measured: 1 in 3 runs of data/xml_feeds on 3.15.1t). No stack samples there.
            self.disabled = "free-threaded build"
            return self
        self.tid = self.tid or threading.get_ident()
        self._resolve()
        self._switch = sys.getswitchinterval()
        sys.setswitchinterval(min(self._switch, self.interval / 5))
        self._thread = threading.Thread(target=self._loop, name="rustique-estimate-sampler", daemon=True)
        self._thread.start()
        return self

    def stop(self) -> "Sampler":
        self._stop = True
        if self._thread is not None:
            self._thread.join()
        if self._switch is not None:
            sys.setswitchinterval(self._switch)
        return self

    def shares(self) -> dict:
        out = {"samples": self.total, "shares": {n: c / self.total for n, c in self.counts.items()} if self.total else {}}
        if self.disabled:
            out["disabled"] = self.disabled
        return out


def _dummy_py(a, b=1):
    return a


def calibrate(n: int = 20000) -> dict:
    """Cost of the profiler itself: ns added to one measured Python-level call (``py``), one
    C-level call (``c``), and one nested monitoring event (``event``)."""
    p = Profiler.__new__(Profiler)
    Profiler.__init__(p, samples=0)
    p.kappa = {}
    code = _dummy_py.__code__
    mon = sys.monitoring
    E = mon.events
    f = _dummy_py
    c = abs
    loop_py = "for _ in r: f(1)"
    loop_c = "for _ in r: c(-1)"
    g = {"f": f, "c": c, "r": range(n)}
    t0 = NS(); exec(loop_py, g); u_py = (NS() - t0) / n  # noqa: E702
    t0 = NS(); exec(loop_c, g); u_c = (NS() - t0) / n  # noqa: E702
    mon.use_tool_id(TOOL_ID, "rustique-estimate-calibration")
    try:
        p.codes = {code: "_py"}
        p.funcs = {code: f}
        p.cfuncs = {p._ckey(c): "_c"}
        p.nmods = len(sys.modules)
        for ev, cb in ((E.CALL, p._call), (E.C_RETURN, p._creturn), (E.C_RAISE, p._creturn), (E.PY_START, p._start),
                       (E.PY_RETURN, p._end)):
            mon.register_callback(TOOL_ID, ev, cb)
        mon.set_local_events(TOOL_ID, code, E.PY_START | E.PY_RETURN)
        mon.set_events(TOOL_ID, E.CALL | E.C_RETURN | E.C_RAISE)
        p.seen = {"_py": 10**9}  # no samples kept
        t0 = NS(); exec(loop_py, g); m_py = (NS() - t0) / n  # noqa: E702
        t0 = NS(); exec(loop_c, g); m_c = (NS() - t0) / n  # noqa: E702
    finally:
        mon.set_events(TOOL_ID, 0)
        mon.set_local_events(TOOL_ID, code, 0)
        for ev in (E.CALL, E.C_RETURN, E.C_RAISE, E.PY_START, E.PY_RETURN):
            mon.register_callback(TOOL_ID, ev, None)
        mon.free_tool_id(TOOL_ID)
        mon.restart_events()
    meas_py = p.ns.get("_py", 0) / max(p.calls.get("_py", 1), 1)
    meas_c = p.ns.get("_c", 0) / max(p.calls.get("_c", 1), 1)
    # measured interval - true call time; a nested event costs what one callback costs
    return {"py": max(meas_py - u_py, 0.0), "c": max(meas_c - u_c, 0.0), "event": max((m_py - u_py) / 2, 0.0),
            "loop_overhead_py": m_py - u_py, "loop_overhead_c": m_c - u_c}


# --- replay -----------------------------------------------------------------------------------


def _clone_args(name, args, kwargs):
    """Copies where replaying would change state the program could still observe."""
    if name.startswith("random.") and args:
        # replayed on the program's own generator (after the program ran: its later draws are
        # not observed); a fresh instance per call would time the kernel's first-use path
        if name == "random.shuffle" and len(args) > 1 and type(args[1]) is list:
            args = (args[0], list(args[1]), *args[2:])
    return args, kwargs


def _replayable(name):
    return name not in NO_REPLAY_RATIO and name not in C_SITES


def _time_call(f, args, kwargs, name, budget_ns=40_000, max_reps=2000):
    """ns per call of f(*args, **kwargs), repeated until budget_ns; None if it raises."""
    a, kw = _clone_args(name, args, kwargs)
    try:
        f(*a, **kw)  # warm-up: first calls pay lazy initialisation and cold caches
    except BaseException:
        return None
    reps = 0
    total = 0
    while (total < budget_ns or reps < 3) and reps < max_reps:
        a, kw = _clone_args(name, args, kwargs)
        t0 = NS()
        try:
            f(*a, **kw)
        except BaseException:
            return None
        total += NS() - t0
        reps += 1
    return total / reps, reps


def _entry_for(name, func, args):
    """What to call when replaying: strptime helpers replay through the C entry point."""
    if name == "time.strptime":
        import time as _t

        return _t.strptime, args
    if name.endswith(".strptime") and args:
        return args[0].strptime, args[1:]
    return func, args


def replay(prof: Profiler, *, budget_ns: int = 40_000) -> dict:
    """Replay the sampled calls, original then kernel; per target: plain ns, kernel ns
    (sums over samples, per call), hits and fallbacks of the replayed kernel calls."""
    import rustique

    out: dict = {}
    names = [n for n in set(prof.kept) | set(prof.longest) if _replayable(n)]
    if not names:
        return out
    plan = []
    for n in names:
        for func, args, kwargs in prof.kept.get(n, []):
            f, a = _entry_for(n, func, args)
            plan.append((n, f, a, kwargs))
        if n in prof.longest and prof.ns.get(n):
            ns, (func, args, kwargs) = prof.longest[n]
            f, a = _entry_for(n, func, args)
            plan.append((n + "#longest", f, a, kwargs))
    plain = []
    for n, f, a, kw in plan:
        plain.append(_time_call(f, a, kw, n, budget_ns))
    rustique.stats(True)
    rustique.enable(only=names)
    try:
        for (n, f, a, kw), p in zip(plan, plain):
            if p is None:
                continue
            rustique.stats(reset=True)
            r = _time_call(f, a, kw, n, budget_ns)
            st = rustique.stats().get(n.split("#")[0], {})
            if r is None:
                continue
            o = out.setdefault(n, {"plain_ns": 0.0, "kernel_ns": 0.0, "samples": 0, "hits": 0, "fallbacks": 0,
                                   "fallback_reasons": {}})
            o["plain_ns"] += p[0]
            o["kernel_ns"] += r[0]
            o["samples"] += 1
            o["hits"] += st.get("hits", 0)
            for reason, k in (st.get("fallback") or {}).items():
                if k:
                    o["fallbacks"] += k
                    o["fallback_reasons"][reason] = o["fallback_reasons"].get(reason, 0) + k
    finally:
        rustique.disable()
    for o in out.values():
        o["ratio"] = o["kernel_ns"] / o["plain_ns"] if o["plain_ns"] else 1.0
    for n in [k for k in out if k.endswith("#longest")]:
        base = n.split("#")[0]
        lo = out.pop(n)
        w = min(prof.longest[base][0] / prof.ns[base], 1.0)  # its share of the target's time
        o = out.setdefault(base, dict(lo))
        if o is not lo:
            o["ratio"] = w * lo["ratio"] + (1 - w) * o["ratio"]
            o["samples"] += 1
        o["longest_share"] = w
    return out


# --- the prediction ---------------------------------------------------------------------------


def predict(profile: dict, replayed: dict, counters: dict | None, total_ns: float,
            sampled: dict | None = None, mode: str = "samples") -> dict:
    """Per target: time under it (ns), the ratio used and where it comes from, ns saved;
    the predicted end-to-end ratio and the share of time under accelerated entry points.
    ``sampled`` (:meth:`Sampler.shares`): time of Python-level targets from stack samples
    instead of the profiler's corrected per-call times."""
    rows = []
    saved = 0.0
    under = 0.0
    counters = counters or {}
    targets = {n: dict(p) for n, p in profile["targets"].items()}
    if sampled is not None and not sampled.get("samples"):
        sampled = None  # no stack samples (free-threaded build): the profiler's times only
    if sampled is not None:
        # The profiler's per-call correction is exact enough where its own events are few;
        # where a call contains many of them (recursion: deepcopy, asdict) the stack sample
        # is used instead.
        # mode "samples": every Python-level target's time from the stack samples (no per-call
        # cost; biased against compute when the program mostly waits in GIL-releasing calls),
        # C-level targets from the profiler outside Python-level ones. Mode "hybrid": samples
        # only where the profiler's own events are a large part of a call (recursion).
        shares = sampled.get("shares", {})
        for n, p in targets.items():
            if p["kind"] == "py" and (mode == "samples" or p.get("nested_events", 0) > 4 * max(p["calls"], 1)):
                p["ns"] = shares.get(n, 0.0) * total_ns
                p["from_samples"] = True
            elif mode == "samples" and "ns_outer" in p:
                p["ns"] = p["ns_outer"]
        for n, sh in shares.items():  # seen by the sampler only (imported after the profiler looked)
            targets.setdefault(n, {"calls": 0, "ns": sh * total_ns, "kind": "py", "from_samples": True})
    for name, p in targets.items():
        t = p["ns"]
        under += t
        if name in replayed and replayed[name]["samples"]:
            r, src = replayed[name]["ratio"], f"replayed {replayed[name]['samples']} calls"
        else:
            base = C_RATIO.get(name, NO_REPLAY_RATIO.get(name))
            c = counters.get(name) or {}
            hits = c.get("hits", 0)
            fb = sum((c.get("fallback") or {}).values())
            h = hits / (hits + fb) if hits + fb else (1.0 if not counters else 0.0)
            if base is None:
                r, src = 1.0, "no model"
            else:
                r, src = 1 - h * (1 - base), f"table {base:.2f} x hit rate {h:.2f}"
        s = t * (1 - r)
        saved += s
        if p.get("from_samples"):
            src += "; time from stack samples"
        rows.append({"target": name, "calls": p["calls"], "ns": t, "share": t / total_ns, "ratio": r,
                     "source": src, "saved_ns": s})
    for name, per_hit in SLOT_SAVING_NS.items():
        c = counters.get(name) or {}
        if c.get("hits"):
            s = c["hits"] * per_hit
            saved += s
            rows.append({"target": name, "calls": c["hits"], "ns": None, "share": None, "ratio": None,
                         "source": f"{per_hit:g} ns saved per hit x hits", "saved_ns": s})
    rows.sort(key=lambda r: -r["saved_ns"])
    return {"ratio": (total_ns - saved) / total_ns if total_ns else 1.0, "under_targets": under / total_ns if total_ns
            else 0.0, "saved_ns": saved, "total_ns": total_ns, "rows": rows}


def render(pred: dict, limit: int = 12) -> str:
    rng = f", range {pred['range'][0]:.2f}-{pred['range'][1]:.2f}" if pred.get("range") else ""
    lines = [f"predicted time with Rustique: {pred['ratio']:.3f} x plain "
             f"({(1 / pred['ratio'] if pred['ratio'] else 0):.2f}x faster{rng})",
             f"  {pred['under_targets'] * 100:.1f}% of the run is under accelerated entry points; "
             f"the other {100 - pred['under_targets'] * 100:.1f}% is unchanged by Rustique",
             "",
             f"  {'target':30} {'calls':>9} {'share':>7} {'ratio':>6} {'saves':>7}  from"]
    for r in pred["rows"][:limit]:
        share = f"{r['share'] * 100:6.1f}%" if r["share"] is not None else "      -"
        ratio = f"{r['ratio']:6.2f}" if r["ratio"] is not None else "     -"
        lines.append(f"  {r['target']:30} {r['calls']:>9,} {share} {ratio} {r['saved_ns'] / pred['total_ns'] * 100:6.1f}%"
                     f"  {r['source']}")
    return "\n".join(lines) + "\n"


# --- the command ------------------------------------------------------------------------------

_HOOK = '''\
import atexit, json, os, sys, time
_mode = os.environ.get("RUSTIQUE_ESTIMATE_MODE")
_p = None
try:
    from rustique import _estimate as _e
    _p = (_e.Sampler() if _mode == "sample" else _e.Profiler()).start()
except Exception as exc:
    sys.stderr.write(f"rustique estimate: profiler not started: {exc}\\n")
_t0 = time.perf_counter_ns()
def _finish():
    if _p is None:
        return
    _p.stop()
    rec = {"pid": os.getpid(), "argv": sys.argv, "mode": _mode, "wall_ns": time.perf_counter_ns() - _t0}
    if _mode == "sample":
        rec.update(_p.shares())
    else:
        rec.update(_p.result())
        try:
            rec["replay"] = _e.replay(_p)
        except Exception as exc:
            rec["replay_error"] = repr(exc)
    with open(os.environ["RUSTIQUE_ESTIMATE_OUT"], "a") as f:
        f.write(json.dumps(rec, default=str) + "\\n")
atexit.register(_finish)
# run a sitecustomize further down the path, if any (this file shadows it)
import importlib.machinery, importlib.util
_spec = importlib.machinery.PathFinder.find_spec(
    "sitecustomize", [p for p in sys.path if os.path.abspath(p or ".") != os.path.dirname(os.path.abspath(__file__))])
if _spec is not None:
    _m = importlib.util.module_from_spec(_spec)
    _spec.loader.exec_module(_m)
'''


def main(argv: list[str]) -> int:
    """``python -m rustique estimate [--json] -- <command...>``: three runs of the command
    (plain with the stack sampler: total time and shares; plain with the profiler and the
    replay; enabled with counters), then the prediction. Only the main thread of each Python
    process is sampled; every process the command starts through this interpreter reports."""
    import argparse
    import json
    import subprocess
    import tempfile

    from rustique import _report
    from rustique._cli import read_records, resolve_command, split_command

    opts_argv, cmd = split_command(argv)
    p = argparse.ArgumentParser(prog="python -m rustique estimate",
                                description="Predict what Rustique does for a command's run time, from the command itself.")
    p.add_argument("--json", action="store_true", help="print the prediction as JSON")
    p.add_argument("--limit", type=int, default=12)
    p.add_argument("--mode", default="samples", choices=("samples", "hybrid", "profile"))
    opts = p.parse_args(opts_argv)
    if not cmd:
        p.error("missing command (python -m rustique estimate -- <command...>)")
    cmd = resolve_command(cmd)
    base = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    walls = {}
    with tempfile.TemporaryDirectory(prefix="rustique-estimate-") as tmp:
        with open(os.path.join(tmp, "sitecustomize.py"), "w") as f:
            f.write(_HOOK)
        pypath = os.pathsep.join([tmp, *filter(None, [base.get("PYTHONPATH")])])
        outs = {}
        for mode in ("sample", "profile"):
            outs[mode] = os.path.join(tmp, f"{mode}.json")
            env = dict(base, RUSTIQUE_ESTIMATE_OUT=outs[mode], RUSTIQUE_ESTIMATE_MODE=mode, PYTHONPATH=pypath)
            t0 = time.perf_counter()
            subprocess.call(cmd, env=env, stdout=subprocess.DEVNULL)
            walls[mode] = time.perf_counter() - t0
        rep = os.path.join(tmp, "report.json")
        t0 = time.perf_counter()
        subprocess.call(cmd, env=dict(base, RUSTIQUE="1", RUSTIQUE_STATS="1", RUSTIQUE_REPORT=rep),
                        stdout=subprocess.DEVNULL)
        walls["enabled"] = time.perf_counter() - t0
        samples = read_records(outs["sample"])
        profiles = read_records(outs["profile"])
        records = read_records(rep)
    if not profiles or not samples:
        sys.stderr.write("rustique estimate: no Python process reported a profile (is rustique installed in the "
                         "interpreter the command runs?)\n")
        return 2
    total_ns = sum(r["wall_ns"] for r in samples)  # CPU-serial sum over the processes
    shares: dict = {}
    nsamp = sum(r.get("samples", 0) for r in samples)
    for r in samples:  # weight each process's shares by its wall time
        for n, sh in r.get("shares", {}).items():
            shares[n] = shares.get(n, 0.0) + sh * r["wall_ns"] / total_ns
    merged = {"targets": {}}
    replayed: dict = {}
    for pr in profiles:
        for n, d in pr["targets"].items():
            m = merged["targets"].setdefault(n, {"calls": 0, "ns": 0.0, "ns_raw": 0, "ns_outer": 0.0, "calls_outer": 0,
                                                 "nested_events": 0, "kind": d["kind"], "samples": 0})
            for k in ("calls", "ns", "ns_raw", "ns_outer", "calls_outer", "nested_events", "samples"):
                m[k] += d.get(k, 0)
        for n, d in (pr.get("replay") or {}).items():
            r = replayed.setdefault(n, {"plain_ns": 0.0, "kernel_ns": 0.0, "samples": 0, "hits": 0, "fallbacks": 0})
            w = d.get("ratio", 1.0)
            r["kernel_ns"] += w * d["plain_ns"]
            r["plain_ns"] += d["plain_ns"]
            for k in ("samples", "hits", "fallbacks"):
                r[k] += d[k]
    for r in replayed.values():
        r["ratio"] = r["kernel_ns"] / r["plain_ns"] if r["plain_ns"] else 1.0
    counters = _report.merge(records)["stats"] if records else {}
    sampled = {"samples": nsamp, "shares": shares}
    pred = predict(merged, replayed, counters, total_ns, sampled, mode=opts.mode)
    # the other time source gives the range: stack samples under-see compute in programs that
    # mostly wait in GIL-releasing calls (I/O), per-call times over-count in nested calls
    alt = predict(merged, replayed, counters, total_ns, sampled, mode="hybrid" if opts.mode != "hybrid" else "samples")
    pred["range"] = (min(pred["ratio"], alt["ratio"]), max(pred["ratio"], alt["ratio"]))
    if opts.json:
        print(json.dumps({"prediction": pred, "runs_s": walls, "processes": len(samples)}, indent=1, default=str))
    else:
        sys.stdout.write(f"rustique estimate: {' '.join(cmd)}\n")
        sys.stdout.write(render(pred, opts.limit))
        sys.stdout.write(f"\n  {len(samples)} Python process(es); runs: sampled {walls['sample']:.2f} s, profiled "
                         f"{walls['profile']:.2f} s, enabled {walls['enabled']:.2f} s. To measure instead of predict: "
                         f"python -m rustique diff --repeat 5 -- <command>\n")
    return 0
