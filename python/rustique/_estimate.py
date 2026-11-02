"""Will Rustique help this program? Measure it on the program itself.

``python -m rustique estimate -- <command>`` (:func:`main`) runs the command three times:

1. **enabled, with counters** (what ``report`` does): which targets the program reaches, how
   often each takes the fast path or falls back, and the entry points to watch (from
   ``status()``: targets added by later releases are found without a table here);
2. **plain, sampled** (:class:`Sampler`): the run time, and the share of CPU time spent under
   each Python-level target (stack samples on a CPU-time timer: no per-call cost, waiting is
   not sampled, safe on free-threaded builds);
3. **plain, profiled** (:class:`Profiler`, ``sys.monitoring``): calls and time of every target
   (C-level ones included), and a small systematic sample of the arguments of Python-level
   ones. At exit each kept call is *replayed* twice, with CPython's original and then with
   the kernel (``rustique.enable(only=<target>)``), timed in a loop: the per-call ratio and
   the fallback rate on the program's own data.

The prediction: ``ratio = 1 - sum(time under target t x (1 - r_t)) / run time``, where
``r_t`` is the replayed ratio (Python-level targets), the measured per-call ratio of the
target x its hit rate (C-level targets, ``C_RATIO``), or hits x the measured saving per hit
(``in`` on tuples/lists, ``float.__repr__``). Everything outside accelerated entry points is
unchanged: the out-of-reach share is printed too. Opt-in tiers (``interp``, ``gc``) have no
entry point to time: they are counted, not predicted (measure them with ``diff``).

Replays call only targets without side effects on these inputs, after the program finished,
on copies where state matters (``random`` replays on a clone of the generator); ``logging``
calls are never replayed (they emit records).

In-process use (the validation harness, ``benches/dx/estimate_workloads.py``)::

    sampler = Sampler(manifest=m).start(); work(); sampler.stop()
    prof = Profiler(manifest=m).start(); work(); prof.stop()
    data = prof.result(); data["replay"] = replay(prof)
    predict(data, data["replay"], counters, plain_ns, sampler.shares())
"""

import _thread
import os
import random as _random_mod
import sys
import time

TOOL_ID = 4  # 0 debugger, 1 coverage, 2 profiler, 5 optimizer: 3 and 4 are free by convention
FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()
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


# Groups whose calls are never replayed: they write (log records) or are not calls at all.
NO_REPLAY_GROUPS = ("logging", "interp", "gc")


def manifest_from_status(status: dict | None) -> dict:
    """Entry points to watch, from ``rustique.status()`` of an enabled run (each target's
    ``sites`` and ``module``), so that targets added after this file was written are found
    too. Known targets keep the tables above (``strptime`` is timed through its Python
    helper; ``logging`` never replays). ``{"py": {name: (module, [qualnames])}, "c": {name:
    (owners, [methods])}, "slots": [names], "other": [names], "no_replay": [names]}``."""
    py: dict = {}
    c: dict = {}
    slots: list = []
    other: list = []
    no_replay = set(NO_REPLAY_RATIO)
    for t in (status or {}).get("targets", []):
        name = t["name"]
        if t.get("group") in NO_REPLAY_GROUPS or t.get("tier"):
            no_replay.add(name)
        if name in PY_SITES:
            py[name] = PY_SITES[name]
            continue
        if name in C_SITES:
            c[name] = C_SITES[name]
            continue
        quals, meths, owners = [], [], []
        kinds = set()
        for site in t.get("sites") or []:
            label, _, kind = site.rpartition(" (")
            kind = kind.rstrip(")")
            kinds.add(kind)
            if kind == "vectorcall" and t.get("module"):
                quals.append(label)
            elif kind == "ml_meth" and "." in label:
                owner, _, meth = label.rpartition(".")
                head, _, rest = owner.partition(".")
                own = f"builtins:{owner}" if not rest and owner in _builtin_type_names() else (
                    f"{head}:{rest}" if rest else head)
                owners.append(own)
                meths.append(meth)
        if quals:
            py[name] = (t["module"], quals)
        elif meths:
            c[name] = (" ".join(dict.fromkeys(owners)), list(dict.fromkeys(meths)))
        elif "slot" in kinds:
            slots.append(name)
        else:
            other.append(name)
    if not py and not c:  # no status (an interpreter without Rustique?): the built-in tables
        py, c = dict(PY_SITES), dict(C_SITES)
    return {"py": py, "c": c, "slots": slots, "other": other, "no_replay": sorted(no_replay)}


def _builtin_type_names() -> set:
    import builtins

    return {n for n, v in vars(builtins).items() if isinstance(v, type)}


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

    def __init__(self, samples: int = SAMPLES, manifest: dict | None = None) -> None:
        self.samples = samples
        self.py_sites = (manifest or {}).get("py", PY_SITES)
        self.c_sites = (manifest or {}).get("c", C_SITES)
        self.no_replay = set((manifest or {}).get("no_replay", NO_REPLAY_RATIO))
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
        self.mt = False
        self.clock = NS
        self.kappa_t = None
        self.csites: dict = {}  # (code, CALL offset) -> C-level target
        self.c_child: dict[str, int] = {}
        self.c_child_calls: dict[str, int] = {}

    # resolution (repeated whenever sys.modules grows: targets of modules imported later)
    def _resolve(self) -> None:
        E = sys.monitoring.events
        pending = False
        for name, (modname, quals) in self.py_sites.items():
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
        for name, (owners, meths) in self.c_sites.items():
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
        site = (code, off)
        if site not in self.csites:  # where the program calls a C-level target (for the sampler)
            self.csites[site] = name
        tid = _thread.get_ident()
        st = self.stacks.get(tid)
        if st is None:
            st = self.stacks[tid] = []
        self._push(st, name, c)
        return None

    # Stack entries: [name, key, events inside, t0 (None: a recursive call, folded into the
    # enclosing call of the same target), time of timed children, clock, (longest-call args)].
    # Time is exclusive: a nested call of *another* target is timed on its own and subtracted
    # from its parent (``any`` or ``sorted`` with a key calling ``fnmatch``); a recursive call of
    # the same target stays in the outermost one (``deepcopy``: the kernel replaces the whole
    # recursion). Once the program has several threads, calls are timed in thread CPU time:
    # wall intervals would include the time a thread waits for the GIL.
    def _push(self, st, name, key):
        if not self.mt:
            thr = sys.modules.get("threading")
            if thr is not None and len(thr._active) > 1:  # noqa: SLF001
                self.mt = True
                self.clock = time.thread_time_ns
        timed = None
        for e in reversed(st):
            if e[3] is not None:
                timed = e
                break
        if timed is not None:
            timed[2] += 1  # this callback ran inside the innermost timed call
            for e in st:
                if e[0] == name:
                    st.append([name, key, 0, None, 0, None])
                    return False
        clock = self.clock
        st.append([name, key, 0, clock(), 0, clock])
        return True

    def _pop(self, st, key):
        for i in range(len(st) - 1, -1, -1):
            if st[i][1] is key:
                e = st[i]
                t = e[5]() if e[5] is not None else 0
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
                if len(e) > 6 and e[6] is not None and interval > self.longest.get(name, (0,))[0]:
                    self.longest[name] = (interval, e[6])
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
                    if name in self.c_sites and parent[0] not in self.c_sites:
                        # a C-level target inside a Python-level one: stack samples see this
                        # time under the Python-level target
                        self.c_child[parent[0]] = self.c_child.get(parent[0], 0) + interval
                        self.c_child_calls[parent[0]] = self.c_child_calls.get(parent[0], 0) + 1
                return

    def _creturn(self, code, off, c, arg0):
        self.total_events += 1
        if len(sys.modules) != self.nmods or (self.retry_at is not None and self.total_events > self.retry_at):
            self._resolve()
        st = self.stacks.get(_thread.get_ident())
        if st:
            self._pop(st, c)

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
        replay = name not in self.no_replay
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
            self._pop(st, code)

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
        if self.mt and self.kappa_t is None:
            self.kappa_t = calibrate(thread_clock=True)
        k = dict((self.kappa_t if self.mt else self.kappa) or {})
        scale = 1.0
        if plain_ns and self.total_events and k.get("event"):
            k["observed_scale"] = (self.wall - plain_ns) / self.total_events / k["event"]
        k["scale"] = scale
        out = {}
        for name, ns in self.ns.items():
            kind = "c" if name in self.c_sites else "py"
            per_call = k.get("c" if kind == "c" else "py", 0.0) * scale
            corrected = ns - self.calls[name] * per_call - self.events.get(name, 0) * k.get("event", 0.0) * scale
            co = self.calls_outer.get(name, 0)
            outer = self.ns_outer.get(name, 0) - co * per_call - self.events_outer.get(name, 0) * k.get("event", 0.0) * scale
            cc = self.c_child_calls.get(name, 0)
            c_child = self.c_child.get(name, 0) - cc * k.get("c", 0.0) * scale
            out[name] = {"calls": self.calls[name], "ns_raw": ns, "ns": max(corrected, 0.0), "kind": kind,
                         "ns_outer": max(outer, 0.0), "calls_outer": co, "c_child_ns": max(c_child, 0.0),
                         "nested_events": self.events.get(name, 0),
                         "samples": len(self.kept.get(name, []))}
        sites = [[c.co_filename, c.co_qualname, c.co_firstlineno, off, name]
                 for (c, off), name in self.csites.items()]
        return {"targets": out, "wall_ns_instrumented": self.wall, "kappa": k, "events": self.total_events,
                "thread_clock": self.mt, "c_call_sites": sites}


class Sampler:
    """Share of CPU time under each Python-level target, from stack samples: no per-call cost,
    so nested and recursive targets (``deepcopy``) are measured as accurately as flat ones.
    The innermost target frame on a stack gets the sample.

    ``signal`` mode (POSIX): a CPU-time timer (``setitimer(ITIMER_PROF)``) fires every
    ``interval`` seconds of process CPU time; the handler runs in the main thread and walks
    the main thread's *own* frames, which is safe on free-threaded builds too. Samples are
    uniform in CPU time, so time a program spends waiting (I/O, sleeps, locks) is not
    sampled, and samples do not cluster at the points where the interpreter releases the GIL
    (which biased a sampler thread toward system calls). With several threads, each sample is
    split by every thread's CPU time since the previous one; other threads' stacks come from
    ``sys._current_frames()`` on GIL builds (consistent: they are parked) and are counted as
    unattributed on free-threaded builds, where walking a running thread's frames is a data
    race (it segfaulted 3.15.1t).

    ``thread`` mode (no ``setitimer``: Windows): a sampler thread reads the main thread's
    stack every ``interval`` seconds of wall time (GIL builds only). Otherwise: no samples,
    and the profiler's per-call times are used."""

    def __init__(self, interval: float = 0.0005, thread_id: int | None = None, manifest: dict | None = None) -> None:
        self.interval = interval
        self.tid = thread_id
        self.py_sites = (manifest or {}).get("py", PY_SITES)
        # where the program calls C-level targets, as the profiled run saw it: (file, qualname,
        # first line) -> {CALL offset: target}. A frame stopped at such a CALL is inside that
        # target (the timer's handler runs at the check after the call: same instruction).
        self.csite_index: dict = {}
        for f, q, line, off, name in (manifest or {}).get("c_call_sites", []):
            self.csite_index.setdefault((f, q, line), {})[off] = name
        self.csite_cache: dict = {}
        self.codes: dict = {}
        self.weights: dict[str, float] = {}
        self.total = 0.0
        self.samples = 0
        self.unattributed = 0.0
        self.nmods = -1
        self.pending = False
        self.mode = None
        self.disabled = None
        self._stop = False
        self._thread = None
        self._switch = None
        self._old = None
        self._busy = False
        self._clocks: dict[int, int] = {}
        self._cpu_last: dict[int, int] = {}
        self.cpu_ns = 0
        self.wall_ns = 0
        self._t0 = (0, 0)

    def _resolve(self) -> None:
        pending = False
        for name, (modname, quals) in self.py_sites.items():
            for q in quals:
                obj = _lookup(modname, q)
                code = _code_of(obj) if obj is not None else None
                if code is not None:
                    self.codes[code] = name
                else:
                    pending = pending or modname in sys.modules
        self.pending = pending
        self.nmods = len(sys.modules)

    def _outer(self, f) -> tuple | None:
        """The distinct targets on the stack, outermost first (``("logging.Logger.level",
        "logging.Formatter.format")``): :func:`predict` gives the time to the outermost one
        whose kernel takes the call, as often as it does, and the rest to the targets inside
        it. A recursive target (``deepcopy``) appears once."""
        codes = self.codes
        index = self.csite_index
        cache = self.csite_cache
        chain = []
        while f is not None:
            code = f.f_code
            if index:
                sites = cache.get(code, False)
                if sites is False:
                    sites = cache[code] = index.get((code.co_filename, code.co_qualname, code.co_firstlineno))
                if sites:
                    c = sites.get(f.f_lasti)
                    if c is not None and c not in chain:
                        chain.append(c)
            n = codes.get(code)
            if n is not None and n not in chain:
                chain.append(n)
            f = f.f_back
        return tuple(reversed(chain)) if chain else None

    def _add(self, chain: tuple | None, w: float) -> None:
        self.total += w
        if chain is not None:
            self.weights[chain] = self.weights.get(chain, 0.0) + w

    # signal mode ---------------------------------------------------------------------------
    def _thread_cpu(self) -> dict[int, int]:
        """CPU ns of every live thread since the previous sample."""
        import threading

        out = {}
        for ident in list(threading._active):  # noqa: SLF001 (no lock: a racy snapshot is fine)
            clk = self._clocks.get(ident)
            try:
                if clk is None:
                    clk = self._clocks[ident] = time.pthread_getcpuclockid(ident)
                now = time.clock_gettime_ns(clk)
            except (OSError, AttributeError, ValueError):
                continue
            out[ident] = now - self._cpu_last.get(ident, now)
            self._cpu_last[ident] = now
        return out

    def _on_signal(self, signum, frame) -> None:
        if self._busy:  # a timer signal while the handler runs: drop it
            return
        self._busy = True
        try:
            self._sample(frame)
        finally:
            self._busy = False

    def _sample(self, frame) -> None:
        self.samples += 1
        if len(sys.modules) != self.nmods or (self.pending and self.samples % 64 == 0):
            self._resolve()
        import threading

        if len(threading._active) <= 1:  # noqa: SLF001
            self._add(self._outer(frame), 1.0)
            return
        deltas = self._thread_cpu()
        total = sum(d for d in deltas.values() if d > 0)
        main = threading.main_thread().ident
        if total <= 0:
            self._add(self._outer(frame), 1.0)
            return
        others = None
        for ident, d in deltas.items():
            if d <= 0:
                continue
            w = d / total
            if ident == main:
                self._add(self._outer(frame), w)
            elif FREE_THREADED:
                self.total += w
                self.unattributed += w
            else:
                if others is None:
                    others = sys._current_frames()  # noqa: SLF001 (GIL build: the other threads are parked)
                self._add(self._outer(others.get(ident)), w)

    # thread mode ---------------------------------------------------------------------------
    def _loop(self) -> None:
        get = sys._current_frames  # noqa: SLF001
        while not self._stop:
            time.sleep(self.interval)
            self.samples += 1
            if len(sys.modules) != self.nmods or (self.pending and self.samples % 64 == 0):
                self._resolve()
            self._add(self._outer(get().get(self.tid)), 1.0)

    def start(self) -> "Sampler":
        import threading

        self._resolve()
        self._t0 = (time.process_time_ns(), time.perf_counter_ns())
        signal = None
        try:
            import signal
        except ImportError:
            pass
        if (signal is not None and hasattr(signal, "setitimer") and threading.current_thread() is threading.main_thread()
                and signal.getsignal(signal.SIGPROF) in (signal.SIG_DFL, None)):
            self.mode = "signal"
            self._thread_cpu()
            self._old = signal.signal(signal.SIGPROF, self._on_signal)
            signal.setitimer(signal.ITIMER_PROF, self.interval, self.interval)
            return self
        if FREE_THREADED:
            # No CPU timer and no safe way to read another running thread's frames.
            self.disabled = "free-threaded build without setitimer"
            return self
        self.mode = "thread"
        self.tid = self.tid or threading.get_ident()
        self._switch = sys.getswitchinterval()
        sys.setswitchinterval(min(self._switch, self.interval / 5))
        self._thread = threading.Thread(target=self._loop, name="rustique-estimate-sampler", daemon=True)
        self._thread.start()
        return self

    def stop(self) -> "Sampler":
        if self.mode == "signal":
            import signal

            signal.setitimer(signal.ITIMER_PROF, 0, 0)
            signal.signal(signal.SIGPROF, self._old if self._old is not None else signal.SIG_DFL)
        self._stop = True
        if self._thread is not None:
            self._thread.join()
        if self._switch is not None:
            sys.setswitchinterval(self._switch)
        self.cpu_ns += time.process_time_ns() - self._t0[0]
        self.wall_ns += time.perf_counter_ns() - self._t0[1]
        return self

    def shares(self) -> dict:
        """``shares``: of CPU (signal mode) or wall time, per innermost target; ``chains``: per
        stack of distinct targets (``"outer>inner"``)."""
        inner: dict[str, float] = {}
        for chain, w in self.weights.items():
            inner[chain[-1]] = inner.get(chain[-1], 0.0) + w
        t = self.total or 1.0
        out = {"samples": self.samples, "mode": self.mode, "cpu_ns": self.cpu_ns, "wall_ns": self.wall_ns,
               "c_sites": bool(self.csite_index),
               "shares": {n: w / t for n, w in inner.items()} if self.total else {},
               "chains": {">".join(c): w / t for c, w in self.weights.items()} if self.total else {},
               "unattributed": self.unattributed / self.total if self.total else 0.0}
        if self.disabled:
            out["disabled"] = self.disabled
        return out


def _dummy_py(a, b=1):
    return a


def calibrate(n: int = 20000, thread_clock: bool = False) -> dict:
    """Cost of the profiler itself: ns added to one measured Python-level call (``py``), one
    C-level call (``c``), and one nested monitoring event (``event``), with the wall clock or
    (``thread_clock``) the thread CPU-time clock that multi-threaded programs are timed with."""
    p = Profiler.__new__(Profiler)
    Profiler.__init__(p, samples=0)
    p.kappa = {}
    p.mt = True  # the clock below, whatever threads exist
    p.clock = time.thread_time_ns if thread_clock else NS
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


def _replayable(name, prof=None):
    if prof is not None:
        return name not in prof.no_replay and name not in prof.c_sites
    return name not in NO_REPLAY_RATIO and name not in C_SITES


def _time_call(f, args, kwargs, name, budget_ns=40_000, max_reps=2000, batches=5):
    """ns per call of f(*args, **kwargs): the fastest of ``batches`` batch means, each batch
    repeated until ``budget_ns / batches`` (at least once): interference from other processes
    only ever adds time. None if a call raises."""
    a, kw = _clone_args(name, args, kwargs)
    try:
        f(*a, **kw)  # warm-up: first calls pay lazy initialisation and cold caches
    except BaseException:
        return None
    best = None
    reps_all = 0
    per = budget_ns / batches
    for _ in range(batches):
        reps = 0
        total = 0
        while (total < per or reps < 1) and reps < max_reps // batches + 1:
            a, kw = _clone_args(name, args, kwargs)
            t0 = NS()
            try:
                f(*a, **kw)
            except BaseException:
                return None
            total += NS() - t0
            reps += 1
        mean = total / reps
        best = mean if best is None else min(best, mean)
        reps_all += reps
    return best, reps_all


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
    names = [n for n in set(prof.kept) | set(prof.longest) if _replayable(n, prof)]
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

    ``total_ns``: wall time of one plain run. ``sampled`` (:meth:`Sampler.shares`): time of
    Python-level targets from stack samples instead of the profiler's corrected per-call
    times. Samples in ``signal`` mode are shares of CPU time: a target's time is its share of
    the run's CPU time (``total_ns`` x CPU/wall of the sampled runs), and the saving is
    divided by ``max(wall, CPU)``, so waiting (I/O) dilutes it and parallel threads share it.
    ``mode``: ``samples`` (every Python-level target from samples), ``hybrid`` (samples only
    where the profiler's own events are a large part of a call: recursion), ``profile``."""
    rows = []
    saved = 0.0
    under = 0.0
    counters = counters or {}
    targets = {n: dict(p) for n, p in profile["targets"].items()}
    if sampled is not None and not sampled.get("samples"):
        sampled = None  # no stack samples: the profiler's times only
    if mode == "profile":
        sampled = None
    basis = total_ns  # what Python-level shares are shares of
    denom = total_ns  # what the saving is divided by
    if sampled is not None and sampled.get("mode") == "signal" and sampled.get("wall_ns"):
        cpu_frac = sampled.get("cpu_ns", 0) / sampled["wall_ns"]
        basis = total_ns * cpu_frac
        denom = max(total_ns, basis)
    mt = bool(profile.get("thread_clock")) and sampled is not None and mode == "samples"
    if mt:
        # Several threads: stack samples of the threads other than the main one are taken
        # where they gave up the GIL (system calls), not uniformly; the profiler's per-call
        # times are in thread CPU time there, so they are used instead.
        mode = "hybrid"
    if sampled is not None:
        shares = assign_chains(sampled, counters, replayed) if sampled.get("chains") else sampled.get("shares", {})
        c_sampled = bool(sampled.get("c_sites")) and mode == "samples"
        for n, p in targets.items():
            if p["kind"] == "c" and c_sampled:
                # the sampler saw the calls of C-level targets at their call sites: no
                # per-call profiler cost to subtract (it is larger than many of these calls)
                p["ns"] = shares.get(n, 0.0) * basis
                p["from_samples"] = True
            elif p["kind"] == "py" and (mode == "samples" or p.get("nested_events", 0) > 4 * max(p["calls"], 1)):
                # without C call sites, samples see C-level targets' time under the innermost
                # Python-level target: the profiler timed those calls, so they count there once
                p["ns"] = max(shares.get(n, 0.0) * basis - (0.0 if c_sampled else p.get("c_child_ns", 0.0)), 0.0)
                p["from_samples"] = True
        if mode == "samples":
            for n, sh in shares.items():  # seen by the sampler only (imported after the profiler looked)
                targets.setdefault(n, {"calls": 0, "ns": sh * basis, "kind": "py", "from_samples": True})
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
                r, src = 1.0, "no model (counted, not timed)"
            else:
                r, src = 1 - h * (1 - base), f"table {base:.2f} x hit rate {h:.2f}"
        s = t * (1 - r)
        saved += s
        if p.get("from_samples"):
            src += "; time from stack samples"
        rows.append({"target": name, "calls": p["calls"], "ns": t, "share": t / denom if denom else 0.0, "ratio": r,
                     "source": src, "saved_ns": s})
    for name, per_hit in SLOT_SAVING_NS.items():
        c = counters.get(name) or {}
        if c.get("hits"):
            s = c["hits"] * per_hit
            saved += s
            rows.append({"target": name, "calls": c["hits"], "ns": None, "share": None, "ratio": None,
                         "source": f"{per_hit:g} ns saved per hit x hits", "saved_ns": s})
    rows.sort(key=lambda r: -r["saved_ns"])
    return {"ratio": (denom - saved) / denom if denom else 1.0, "under_targets": under / denom if denom else 0.0,
            "saved_ns": saved, "total_ns": denom, "wall_ns": total_ns, "cpu_ns": basis, "rows": rows}


def hit_fraction(name: str, counters: dict, replayed: dict) -> float:
    """How often a target's kernel takes the call: from the enabled run's counters, else from
    the replay, else 1."""
    c = counters.get(name) or {}
    n = c.get("hits", 0) + sum((c.get("fallback") or {}).values())
    if n:
        return c.get("hits", 0) / n
    r = replayed.get(name) or {}
    n = r.get("hits", 0) + r.get("fallbacks", 0)
    return r.get("hits", 0) / n if n else 1.0


def assign_chains(sampled: dict, counters: dict, replayed: dict) -> dict[str, float]:
    """Share of time per target from the sampled stacks of targets: each stack's time goes to
    its outermost target in the proportion of calls its kernel takes (a kernel that takes the
    call replaces everything under it), the rest to the next target inside, and so on. A
    ``Logger.info`` that always falls back leaves its time to the logging targets inside it;
    ``relative_to``, which always hits, keeps the ``PurePath`` calls inside it."""
    out: dict[str, float] = {}
    for key, w in sampled["chains"].items():
        chain = key.split(">")
        for i, t in enumerate(chain):
            h = 1.0 if i == len(chain) - 1 else hit_fraction(t, counters, replayed)
            out[t] = out.get(t, 0.0) + w * h
            w *= 1 - h
            if w <= 0:
                break
    return out


def render(pred: dict, limit: int = 12, notes: list[str] | None = None) -> str:
    """The prediction for a developer: the ratio and its range, how much of the run Rustique
    can reach at all, the targets that save the most and how each number was obtained."""
    ratio = pred["ratio"]
    rng = f"; range {pred['range'][0]:.2f}-{pred['range'][1]:.2f} by the other time source" if pred.get("range") else ""
    under = pred["under_targets"] * 100
    lines = [f"  predicted: {ratio:.3f} x the plain run time ({(1 / ratio if ratio else 0):.2f}x faster){rng}"]
    if ratio > 0.98:
        lines.append(f"  verdict: no noticeable change. {100 - under:.0f}% of the run is outside the entry points Rustique "
                     "accelerates (your code, third-party code, the interpreter itself).")
    else:
        lines.append(f"  {under:.1f}% of the run is under accelerated entry points; the other {100 - under:.1f}% is "
                     "unchanged by Rustique.")
    shown = [r for r in pred["rows"] if r["saved_ns"] > 0][:limit]
    if shown:
        lines += ["", f"  {'target':32} {'calls':>11} {'share':>7} {'ratio':>6} {'saves':>7}  from"]
        for r in shown:
            share = f"{r['share'] * 100:6.1f}%" if r["share"] is not None else "      -"
            rr = f"{r['ratio']:6.2f}" if r["ratio"] is not None else "     -"
            lines.append(f"  {r['target']:32} {r['calls']:>11,} {share} {rr} "
                         f"{r['saved_ns'] / pred['total_ns'] * 100:6.1f}%  {r['source']}")
        rest = [r for r in pred["rows"] if r["saved_ns"] > 0][limit:]
        if rest:
            lines.append(f"  ... {len(rest)} more targets save {sum(r['saved_ns'] for r in rest) / pred['total_ns'] * 100:.1f}%")
        lines += ["", "  share: of the run's time under the target; ratio: kernel / CPython's original on your program's "
                  "calls (fallbacks included); saves: of the whole run"]
    for n in notes or []:
        lines.append(f"  note: {n}")
    return "\n".join(lines) + "\n"


# --- the command ------------------------------------------------------------------------------

_HOOK = """\
import atexit, json, os, sys, time
_mode = os.environ.get("RUSTIQUE_ESTIMATE_MODE")
_p = None
if _mode == "enabled":  # like RUSTIQUE=1, also where no startup file is installed (dev installs)
    try:
        import rustique
        rustique._enable(source="python -m rustique estimate")
    except Exception as exc:
        sys.stderr.write(f"rustique estimate: enable failed: {exc}\\n")
    _mode = None
try:
    if _mode is None:
        raise StopIteration
    from rustique import _estimate as _e
    _man = None
    if os.environ.get("RUSTIQUE_ESTIMATE_MANIFEST"):
        with open(os.environ["RUSTIQUE_ESTIMATE_MANIFEST"]) as _f:
            _man = json.load(_f)
    _p = (_e.Sampler(manifest=_man) if _mode == "sample" else _e.Profiler(manifest=_man)).start()
except StopIteration:
    pass
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
        os.environ.pop("RUSTIQUE", None)  # the program ran plain (kill switch); the replay enables
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
"""


def combine(samples: list[dict], profiles: list[dict], records: list[dict]) -> tuple:
    """The inputs of :func:`predict` from the records of every process of the three runs:
    ``(profile, replayed, counters, total_ns, sampled)``."""
    from rustique import _report

    total_ns = sum(r["wall_ns"] for r in samples)  # serial sum over the processes
    shares: dict = {}
    cpu = sum(r.get("cpu_ns", 0) for r in samples)
    sampled_wall = sum(r.get("wall_ns", 0) for r in samples)
    chains: dict = {}
    for r in samples:  # weight each process's shares by its CPU (signal mode) or wall time
        w = (r.get("cpu_ns", 0) / cpu) if cpu and r.get("mode") == "signal" else (
            r["wall_ns"] / total_ns if total_ns else 0)
        for n, sh in r.get("shares", {}).items():
            shares[n] = shares.get(n, 0.0) + sh * w
        for n, sh in r.get("chains", {}).items():
            chains[n] = chains.get(n, 0.0) + sh * w
    modes = {r.get("mode") for r in samples if r.get("samples")}
    sampled = {"samples": sum(r.get("samples", 0) for r in samples), "shares": shares, "chains": chains,
               "mode": "signal" if modes == {"signal"} else ("thread" if modes else None),
               "cpu_ns": cpu, "wall_ns": sampled_wall}
    merged = {"targets": {}, "thread_clock": any(pr.get("thread_clock") for pr in profiles)}
    replayed: dict = {}
    for pr in profiles:
        for n, d in pr["targets"].items():
            m = merged["targets"].setdefault(n, {"calls": 0, "ns": 0.0, "ns_raw": 0, "ns_outer": 0.0, "calls_outer": 0,
                                                 "nested_events": 0, "kind": d["kind"], "samples": 0})
            for k in ("calls", "ns", "ns_raw", "ns_outer", "calls_outer", "nested_events", "samples", "c_child_ns"):
                m[k] = m.get(k, 0) + d.get(k, 0)
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
    return merged, replayed, counters, total_ns, sampled


def main(argv: list[str]) -> int:
    """``python -m rustique estimate [--json] -- <command...>``: three runs of the command,
    then the prediction.

    1. **enabled, with counters** (``report``): which targets the program reaches, their hit
       and fallback counts, and the entry points to watch (``status()``'s sites);
    2. **plain, sampled**: run time, and the share of CPU time under each Python-level target;
    3. **plain, profiled**: calls and time of every target, a sample of their arguments,
       replayed at exit with CPython's original and with the kernel.

    Every Python process the command starts (through an interpreter with Rustique) reports."""
    import argparse
    import json
    import subprocess
    import tempfile

    from rustique import _report
    from rustique._cli import add_tiers, read_records, resolve_command, split_command

    opts_argv, cmd = split_command(argv)
    p = argparse.ArgumentParser(prog="python -m rustique estimate",
                                description="Predict what Rustique does for a command's run time, from the command itself.")
    p.add_argument("--json", action="store_true", help="print the prediction as JSON")
    p.add_argument("--limit", type=int, default=12, help="targets shown")
    p.add_argument("--mode", default="samples", choices=("samples", "hybrid", "profile"),
                   help="time source for Python-level targets (default: stack samples)")
    p.add_argument("--tiers", metavar="T,..", help="opt-in tiers to count in (their targets are counted, not timed)")
    opts = p.parse_args(opts_argv)
    if not cmd:
        p.error("missing command (python -m rustique estimate -- <command...>)")
    cmd = resolve_command(cmd)
    add_tiers(opts.tiers)
    base = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    walls = {}
    notes = []
    with tempfile.TemporaryDirectory(prefix="rustique-estimate-") as tmp:
        with open(os.path.join(tmp, "sitecustomize.py"), "w") as f:
            f.write(_HOOK)
        pypath = os.pathsep.join([tmp, *filter(None, [base.get("PYTHONPATH")])])
        rep = os.path.join(tmp, "report.json")
        env = dict(base, RUSTIQUE="1", RUSTIQUE_STATS="1", RUSTIQUE_REPORT=rep, RUSTIQUE_ESTIMATE_MODE="enabled",
                   PYTHONPATH=pypath)
        for k in ("RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE", "RUSTIQUE_TIERS", "RUSTIQUE_UNVALIDATED_PATCH"):
            if k in os.environ:
                env[k] = os.environ[k]
        t0 = time.perf_counter()
        try:
            subprocess.call(cmd, env=env, stdout=subprocess.DEVNULL)
        except FileNotFoundError as exc:
            sys.stderr.write(f"rustique estimate: {exc}\n")
            return 2
        walls["enabled"] = time.perf_counter() - t0
        records = _report.collect(rep)
        full = [r for r in records if r.get("status")]
        if not full:
            sys.stderr.write("rustique estimate: the command reported nothing: is rustique installed in the "
                             "interpreter it runs? (python -m rustique status with that interpreter)\n")
            return 2
        manifest = manifest_from_status(_report.merge(full)["status"])
        man_path = os.path.join(tmp, "manifest.json")
        with open(man_path, "w") as f:
            json.dump(manifest, f)
        outs = {}
        for mode in ("profile", "sample"):  # the profiled run finds where C-level targets are called
            outs[mode] = os.path.join(tmp, f"{mode}.json")
            # RUSTIQUE=0: a rustique.enable() in the program is a no-op, so these runs are plain
            env = dict(base, RUSTIQUE="0", RUSTIQUE_ESTIMATE_OUT=outs[mode], RUSTIQUE_ESTIMATE_MODE=mode,
                       PYTHONPATH=pypath, RUSTIQUE_ESTIMATE_MANIFEST=man_path)
            t0 = time.perf_counter()
            subprocess.call(cmd, env=env, stdout=subprocess.DEVNULL)
            walls[mode] = time.perf_counter() - t0
            if mode == "profile":
                manifest["c_call_sites"] = [s for r in read_records(outs["profile"]) for s in r.get("c_call_sites", [])]
                with open(man_path, "w") as f:
                    json.dump(manifest, f)
        samples = read_records(outs["sample"])
        profiles = read_records(outs["profile"])
    if not profiles or not samples:
        sys.stderr.write("rustique estimate: no Python process reported a profile (the command's interpreter "
                         "must be able to import rustique)\n")
        return 2
    merged, replayed, counters, total_ns, sampled = combine(samples, profiles, records)
    pred = predict(merged, replayed, counters, total_ns, sampled, mode=opts.mode)
    # The other time source gives the range: per-call times over-count nested calls, samples
    # can under-see short programs.
    alt = predict(merged, replayed, counters, total_ns, sampled, mode="hybrid" if opts.mode != "hybrid" else "samples")
    pred["range"] = (min(pred["ratio"], alt["ratio"]), max(pred["ratio"], alt["ratio"]))
    if sampled["mode"] is None:
        why = next((r.get("disabled") for r in samples if r.get("disabled")), "no samples")
        notes.append(f"no stack samples ({why}): Python-level targets are timed per call, which over-counts "
                     "recursive targets")
    elif sampled["mode"] == "signal" and sampled["wall_ns"] and sampled["cpu_ns"] < 0.7 * sampled["wall_ns"]:
        notes.append(f"the program waited {100 - 100 * sampled['cpu_ns'] / sampled['wall_ns']:.0f}% of its run time "
                     "(I/O, sleeps, locks): Rustique only shortens the rest")
    unattr = sum(r.get("unattributed", 0) * r.get("cpu_ns", 0) for r in samples) / (sampled["cpu_ns"] or 1)
    if unattr > 0.05:
        notes.append(f"{unattr * 100:.0f}% of the CPU time ran in threads whose stacks cannot be sampled safely on "
                     "this free-threaded build: not counted (the prediction under-promises)")
    tiered = [t for t in _report.merge(full)["status"].get("targets", []) if t.get("tier")]
    if tiered and not os.environ.get("RUSTIQUE_TIERS"):
        names = sorted({t["tier"] for t in tiered})
        notes.append(f"opt-in tiers not included ({', '.join(names)}): their effect is measured, not predicted: "
                     f"python -m rustique diff --repeat 5 --tiers {','.join(names)} -- <command>")
    fb = sorted(((sum((c.get("fallback") or {}).values()), n, c) for n, c in counters.items()), reverse=True)
    for n_fb, name, c in fb[:3]:
        tot = n_fb + c.get("hits", 0)
        if tot and n_fb / tot >= 0.3 and n_fb >= 100:
            top = max((c.get("fallback") or {}).items(), key=lambda kv: kv[1])[0]
            notes.append(f"{name} fell back on {100 * n_fb / tot:.0f}% of {tot:,} calls (mostly {top}: "
                         f"{_report.FALLBACK_ADVICE.get(top, top).split(':')[0]})")
    if opts.json:
        print(json.dumps({"prediction": pred, "notes": notes, "runs_s": walls, "processes": len(samples),
                          "sampler": {k: sampled[k] for k in ("mode", "samples", "cpu_ns", "wall_ns")}},
                         indent=1, default=str))
    else:
        sys.stdout.write(f"rustique estimate: {' '.join(cmd)}\n")
        sys.stdout.write(render(pred, opts.limit, notes))
        sys.stdout.write(f"\n  {len(samples)} Python process(es); runs: enabled {walls['enabled']:.2f} s, sampled "
                         f"{walls['sample']:.2f} s, profiled {walls['profile']:.2f} s. To measure instead of predict: "
                         f"python -m rustique diff --repeat 5 -- <command>\n")
    return 0
