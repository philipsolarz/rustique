#!/usr/bin/env python3
"""Where real programs spend time in CPython-provided machinery (wave 2, slice w2-machinery).

    profile_machinery.py profile <workload.py> --python <interp> --out <m.json> [--seconds 4]
    profile_machinery.py rollup <m.json>... --out <rollup.json>

`profile` runs the workload in fresh processes of <interp>:
  perf   `perf record -e cpu-clock -F 1999 -g` with `-X perf`, gated (control FIFO) to
         steady-state run() calls after one warm-up; symbolized with the wave-1 BOLT-aware
         symbolizer (benches/workloads/data/_profiler.py: ElfSyms, Classifier, PerfParser).
  cprof  cProfile around one run(), with callers, to count calls and per-call costs.

Attribution ("machinery entry", inclusive). For each sample, take the innermost *user*
Python frame U (the workload's own files, third-party packages): the program decided to
call something there. Then:
  - if Python frames inside U exist, they are all CPython-provided: the sample belongs to
    the outermost of them, the function U called (`std:<module>:<qualname>` for stdlib,
    `gen:<kind>:<qualname>` for code CPython generated: dataclass methods (`<string>`,
    `__create_fn__`), namedtuple `__new__` (`<string>`, `<lambda>`));
  - otherwise the sample is native code U reached from bytecode: the first native frame
    below the interpreter that is not call plumbing (`lang:<symbol>`, the C entry point:
    a builtin method, a type slot, a C module function), or `interp:<owner-kind>` when the
    leaf is the interpreter itself (bytecode of U);
  - samples under the cyclic GC are `rt:gc`; samples with no user frame are
    `driven:<module>` (stdlib loops such as asyncio or socketserver running their own code).
A kernel replacing a machinery entry would replace all of its inclusive time, so these are
upper bounds of what each entry point could save (the native floor inside is reported too:
`inner` splits each entry's time into Python glue vs the native entries it reaches).
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
WL = os.path.join(os.path.dirname(HERE), "workloads")


def _prof():
    spec = importlib.util.spec_from_file_location("_wl_profiler",
                                                  os.path.join(WL, "data", "_profiler.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


# ------------------------------------------------------------------------------------------
# driver (inside the profiled interpreter)
# ------------------------------------------------------------------------------------------

def _ctl(cmd):
    ctl = os.environ.get("RQ_PERF_CTL")
    if not ctl:
        return
    with open(ctl, "w") as f:
        f.write(cmd + "\n")
    with open(os.environ["RQ_PERF_ACK"]) as f:
        f.read(5)


def _load(path):
    name = "rq_wl_" + os.path.splitext(os.path.basename(path))[0]
    sys.path.insert(0, os.path.dirname(path))
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def driver_main(a):
    import sysconfig

    try:
        import importlib.metadata as md

        pkgdist = {k: v[0] for k, v in md.packages_distributions().items()}
    except Exception:
        pkgdist = {}
    out = {"env": {"python": sys.version.split()[0], "stdlib": sysconfig.get_paths()["stdlib"],
                   "pkgdist": pkgdist}, "pid": os.getpid()}
    if a.enable:
        import rustique

        rustique.enable()
    mod = _load(a.workload)
    state = mod.setup()
    ref = mod.digest(mod.run(state))
    if a.mode == "perf":
        walls = []
        _ctl("enable")
        t_start = time.perf_counter()
        while True:
            t0 = time.perf_counter()
            res = mod.run(state)
            walls.append(time.perf_counter() - t0)
            if walls and time.perf_counter() - t_start >= a.seconds and len(walls) >= 3:
                break
        _ctl("disable")
        out.update(walls=walls, digest_ok=mod.digest(res) == ref)
        # keep our own copy of the perf map: other jobs on a shared box may clean /tmp,
        # but the interpreter keeps the file open, so read it through /proc/self/fd
        for fd in os.listdir("/proc/self/fd"):
            try:
                tgt = os.readlink(f"/proc/self/fd/{fd}")
            except OSError:
                continue
            if tgt.startswith(f"/tmp/perf-{os.getpid()}.map"):
                with open(f"/proc/self/fd/{fd}", "rb") as src, open(a.out + ".map", "wb") as dst:
                    dst.write(src.read())
                break
    else:
        import cProfile
        import pstats

        t0 = time.perf_counter()
        res = mod.run(state)
        out["wall_plain"] = time.perf_counter() - t0
        prof = cProfile.Profile()
        t0 = time.perf_counter()
        prof.enable()
        res = mod.run(state)
        prof.disable()
        out["wall_cprofile"] = time.perf_counter() - t0
        out["digest_ok"] = mod.digest(res) == ref
        rows = []
        for (fn, line, name), (cc, nc, tt, ct, callers) in pstats.Stats(prof).stats.items():
            rows.append([fn, line, name, nc, cc, tt, ct,
                         [[c[0], c[1], c[2], v[0], v[3]] for c, v in callers.items()]])
        out["rows"] = rows
    with open(a.out, "w") as f:
        json.dump(out, f)


# ------------------------------------------------------------------------------------------
# attribution
# ------------------------------------------------------------------------------------------

def make_owner(env, workload, own_roots):
    stdlib = env["stdlib"].rstrip("/") + "/"
    pkgdist = env.get("pkgdist", {})
    roots = [os.path.abspath(r).rstrip("/") + "/" for r in own_roots]
    wl = os.path.abspath(workload)

    def owner(fname, qual=""):
        """-> (kind, label); kind in own, thirdparty, stdlib, gen, gen3p, tool, other."""
        if fname == wl or any(fname.startswith(r) for r in roots):
            if fname.endswith("profile_machinery.py"):
                return "tool", "tool"
            return "own", "own"
        if fname.startswith("<") and not fname.startswith("<frozen "):
            if fname == "<string>":
                if qual.startswith("__create_fn__"):
                    return "gen", "dataclasses"
                if qual == "<lambda>":
                    return "gen", "namedtuple?"
                return "gen", "exec"
            return "gen3p", fname.split()[0].strip("<>")[:40]
        if fname.startswith("<frozen "):
            return "stdlib", fname[8:-1].replace(".", "/") + ".py"
        if fname.startswith(stdlib) and "/site-packages/" not in fname:
            return "stdlib", fname[len(stdlib):]
        if "/site-packages/" in fname:
            top = fname.split("/site-packages/", 1)[1].split("/")[0].removesuffix(".py")
            return "thirdparty", pkgdist.get(top, top)
        return "other", os.path.basename(fname)

    return owner


USER = ("own", "thirdparty", "gen3p", "other")


def attribute(samples, P, cls, owner):
    total = 0
    entry = Counter()
    entry_caller = defaultdict(Counter)
    inner = defaultdict(Counter)
    lang_comp = {}
    for period, frames in samples:
        total += period
        py = []  # (index, kind, label, qual, fname)
        for i, (k, s, _d) in enumerate(frames):
            if k == "py":
                qual, _, fname = s.partition(":")
                kind, label = owner(fname, qual)
                py.append((i, kind, label, qual, fname))
        # cyclic GC anywhere below the innermost Python frame
        lim = py[0][0] if py else len(frames)
        in_gc = any(k == "n" and s is not None and P.base_name(s).startswith(P.GC_FRAMES)
                    for k, s, _d in frames[:lim])
        if in_gc:
            entry["rt:gc"] += period
            continue
        if not py:
            entry["nopy"] += period
            continue
        u = next((j for j, p in enumerate(py) if p[1] in USER), None)
        if u is None:
            outer = py[-1]
            key = f"driven:{outer[2].split('/')[0]}"
            entry[key] += period
            continue
        ucaller = py[u][1] if py[u][1] != "thirdparty" else "3p:" + py[u][2]
        if u > 0:
            _, kind, label, qual, _fn = py[u - 1]
            if kind == "stdlib":
                key = f"std:{label}:{qual}"
            elif kind == "gen":
                key = f"gen:{label}:{qual}"
            else:
                key = f"{kind}:{label}:{qual}"
            entry[key] += period
            entry_caller[key][ucaller] += period
            # inner split: native entry reached from the innermost Python frame
            ent, comp = _native_entry(frames, py[0][0], P, cls)
            inner[key][ent if ent else "(py glue)"] += period
            continue
        ent, comp = _native_entry(frames, py[0][0], P, cls)
        if ent is None:
            key = f"interp:{py[u][1]}"
        else:
            key = f"lang:{ent}"
            lang_comp[key] = comp
            inner[key][_second_entry(frames, _native_entry.last, P)
                       if ent != "(call machinery)" else "(self)"] += period
        entry[key] += period
        entry_caller[key][ucaller] += period
    return total, entry, entry_caller, inner, lang_comp


def _native_entry(frames, pyidx, P, cls):
    """First native frame below the innermost interpreter frame that is not call plumbing
    (None if the leaf is the interpreter itself)."""
    boundary = None
    for i in range(pyidx):
        k, s, d = frames[i]
        if cls.component(s, d) in ("interpreter", "perf-trampoline"):
            boundary = i
            break
    if boundary is None:
        boundary = pyidx
    if boundary == 0:
        return None, None
    for j in range(boundary - 1, -1, -1):
        k, s, d = frames[j]
        if k == "py" or s is None:
            continue
        b = P.base_name(s)
        if b.startswith(P.CALL_WRAPPERS):
            continue
        _native_entry.last = j
        return b, cls.component(s, d)
    return "(call machinery)", "calls"


def _load_map(path):
    import bisect

    rows = []
    try:
        with open(path, errors="replace") as f:
            for line in f:
                a, sz, name = line.rstrip("\n").split(" ", 2)
                rows.append((int(a, 16), int(sz, 16), name))
    except OSError:
        return None
    rows.sort()
    starts = [r[0] for r in rows]

    def look(ip):
        i = bisect.bisect_right(starts, ip) - 1
        if i >= 0 and ip < rows[i][0] + rows[i][1]:
            return rows[i][2]
        return None

    return look


def parse_perf(parser, P, data_file, map_copy=None):
    """PerfParser.parse, but perf 7 indents the sample header line too: a header is any
    line not starting with a tab. Python frames are resolved from our own copy of the perf
    map when perf could not (the /tmp file was removed)."""
    look = _load_map(map_copy) if map_copy else None
    proc = subprocess.Popen(["perf", "script", "-i", data_file, "-F",
                             "tid,period,ip,sym,dso,dsoff"],
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                            errors="replace")
    samples = []
    cur = None
    for line in proc.stdout:
        line = line.rstrip("\n")
        if not line.strip():
            if cur is not None and cur[1]:
                samples.append(cur)
            cur = None
            continue
        if not line.startswith("\t"):
            p = line.split()
            period = int(p[1]) if len(p) > 1 and p[1].isdigit() else 1
            cur = (period, [])
            continue
        m = P._LINE_RE.match(line)
        if not m or cur is None:
            continue
        ip, sym, dsopart = m.groups()
        if look is not None and "/tmp/perf-" in dsopart and not sym.startswith("py::"):
            sym = look(int(ip, 16)) or sym
        if sym.startswith("py::"):
            cur[1].append(("py", sym[4:], None))
            continue
        dso, _, off = dsopart.rpartition("+0x")
        if not dso:
            dso, off = dsopart, "0"
        leaf = not cur[1]
        o = int(off, 16)
        s = parser.sym(dso, o if leaf else max(o - 1, 0))
        cur[1].append(("n", s, dso))
    if cur is not None and cur[1]:
        samples.append(cur)
    proc.wait()
    return samples


def _second_entry(frames, j, P):
    """The next non-plumbing native frame leaf-ward of the entry at index j (what the entry
    spent its time in), or '(self)'."""
    ent = P.base_name(frames[j][1])
    for i in range(j - 1, -1, -1):
        k, s, _d = frames[i]
        if k != "n" or s is None:
            continue
        b = P.base_name(s)
        if b.startswith(P.CALL_WRAPPERS) or b == ent:
            continue
        return b
    return "(self)"


def cprof_summary(rows, owner, total_tt):
    """Per stdlib/generated function: calls from user frames, tottime, cumtime."""
    out = {}
    for fn, line, name, nc, cc, tt, ct, callers in rows:
        if fn == "~":
            kind, label = "builtin", "builtin"
            key = f"bi:{name}"
        else:
            kind, label = owner(fn, name)
            if kind == "stdlib":
                key = f"std:{label}:{name}@{line}"
            elif kind == "gen":
                key = f"gen:{label}:{name}"
            else:
                continue
        ucalls = 0
        for cf, cl, cn, cnc, _cct in callers:
            ck, _cl = owner(cf, cn) if cf != "~" else ("builtin", "")
            if ck in USER:
                ucalls += cnc
        rec = out.setdefault(key, [0, 0, 0.0, 0.0])
        rec[0] += nc
        rec[1] += ucalls
        rec[2] += tt
        rec[3] += ct
    return {k: {"calls": v[0], "user_calls": v[1], "tottime_share": round(v[2] / total_tt, 5),
                "cumtime_share": round(v[3] / total_tt, 5)}
            for k, v in sorted(out.items(), key=lambda kv: -kv[1][2]) if v[2] / total_tt > 2e-4
            or v[1] > 1000}


def profile_main(a):
    P = _prof()
    py = a.python
    wl = os.path.abspath(a.workload)
    name = os.path.splitext(os.path.basename(wl))[0]
    tmp = tempfile.mkdtemp(prefix=f"rqm-{name}-", dir=a.tmp)
    own_roots = [os.path.dirname(wl)] + (a.own_root or [])
    log = lambda *x: print(f"[{name}]", *x, file=sys.stderr, flush=True)
    res = {"workload": name, "family": os.path.basename(os.path.dirname(wl))}
    drv = lambda mode, out, extra=(): ([py] + list(extra) + [os.path.abspath(__file__), "driver",
                                                            wl, "--mode", mode, "--out", out,
                                                            "--seconds", str(a.seconds)]
                                       + (["--enable"] if a.enable else []))
    # perf
    ctl, ack = os.path.join(tmp, "ctl"), os.path.join(tmp, "ack")
    for p in (ctl, ack):
        os.mkfifo(p)
    data = os.path.join(tmp, "perf.data")
    out = os.path.join(tmp, "drv-perf.json")
    env = dict(os.environ, RQ_PERF_CTL=ctl, RQ_PERF_ACK=ack)
    subprocess.run(["perf", "record", "-q", "-e", "cpu-clock", "-F", "1999", "-g", "-D", "-1",
                    "--control", f"fifo:{ctl},{ack}", "-o", data, "--"]
                   + drv("perf", out, ["-X", "perf"]), check=True, env=env,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    with open(out) as f:
        d = json.load(f)
    owner = make_owner(d["env"], wl, own_roots)
    cls = P.Classifier(a.cpython_src, a.cache)
    parser = P.PerfParser(cls, lambda f: "x", lambda f, q: q)
    samples = parse_perf(parser, P, data, out + ".map")
    try:
        os.unlink(f"/tmp/perf-{d['pid']}.map")
    except OSError:
        pass
    total, entry, entry_caller, inner, lang_comp = attribute(samples, P, cls, owner)
    sh = lambda v: round(v / total, 5)
    res.update(python=d["env"]["python"], samples=len(samples),
               run_s_under_perf=round(statistics.median(d["walls"]), 4),
               digest_ok=d["digest_ok"])
    res["entries"] = {k: sh(v) for k, v in entry.most_common() if v / total >= 2e-4}
    res["entry_callers"] = {k: {c: sh(w) for c, w in entry_caller[k].most_common(5)}
                            for k, v in entry.most_common(150) if k in entry_caller}
    res["entry_inner"] = {k: {c: sh(w) for c, w in inner[k].most_common(6)}
                          for k, v in entry.most_common(150) if k in inner}
    res["lang_component"] = lang_comp
    log("perf", len(samples), "samples")
    # cProfile
    out = os.path.join(tmp, "drv-cprof.json")
    subprocess.run(drv("cprof", out), check=True)
    with open(out) as f:
        c = json.load(f)
    tot_tt = sum(r[5] for r in c["rows"]) or 1.0
    res["run_s_plain"] = round(c["wall_plain"], 4)
    res["run_s_cprofile"] = round(c["wall_cprofile"], 4)
    res["cprofile"] = cprof_summary(c["rows"], owner, tot_tt)
    log("cprofile", len(c["rows"]), "functions")
    with open(a.out, "w") as f:
        json.dump(res, f, indent=1)
    shutil.rmtree(tmp, ignore_errors=True)


def rollup_main(a):
    profs = [json.load(open(p)) for p in a.profiles]
    keys = Counter()
    for p in profs:
        for k, v in p["entries"].items():
            keys[k] += v
    rows = []
    n = len(profs)
    for k, _ in keys.most_common():
        vals = [p["entries"].get(k, 0.0) for p in profs]
        mx = max(range(n), key=lambda i: vals[i])
        rows.append({"entry": k, "mean": round(sum(vals) / n, 5), "max": vals[mx],
                     "max_in": profs[mx]["workload"], "n_ge_1pct": sum(v >= 0.01 for v in vals)})
    with open(a.out, "w") as f:
        json.dump({"workloads": [p["workload"] for p in profs], "rows": rows}, f, indent=1)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("driver")
    d.add_argument("workload")
    d.add_argument("--mode", required=True, choices=["perf", "cprof"])
    d.add_argument("--out", required=True)
    d.add_argument("--seconds", type=float, default=4.0)
    d.add_argument("--enable", action="store_true")
    p = sub.add_parser("profile")
    p.add_argument("workload")
    p.add_argument("--python", default=sys.executable)
    p.add_argument("--out", required=True)
    p.add_argument("--seconds", type=float, default=4.0)
    p.add_argument("--enable", action="store_true", help="rustique.enable() first")
    p.add_argument("--own-root", action="append", help="extra directories of own code")
    p.add_argument("--cpython-src", default=os.environ.get("CPYTHON_SRC",
                                                           os.path.expanduser("~/src/cpython-3.15.1")))
    p.add_argument("--cache", default=os.path.expanduser("~/.cache/rqm"))
    p.add_argument("--tmp", default=None)
    r = sub.add_parser("rollup")
    r.add_argument("profiles", nargs="+")
    r.add_argument("--out", required=True)
    a = ap.parse_args()
    if a.cmd == "profile":
        os.makedirs(a.cache, exist_ok=True)
    {"driver": driver_main, "profile": profile_main, "rollup": rollup_main}[a.cmd](a)


if __name__ == "__main__":
    main()
