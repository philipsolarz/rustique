"""Human-readable reports of ``rustique.status()`` and ``rustique.stats()`` (the report at
exit, ``python -m rustique status|report|diff``, the pytest plugin's summary).

Records: each process that reports writes ``{"pid", "argv", "status", "stats", "counters"}``
(``status()["process"]["ppid"]``: the parent's pid if the process was forked after enable).
Processes that end without exit hooks (``os._exit`` in forked pool workers, SIGTERM,
SIGKILL) still leave their *live counter file* next to a JSON report
(``<report>.json.<pid>.cnt``, kept by the extension while counters run; Linux and macOS),
which :func:`collect` reads. A forked child starts with zeroed counters, so every call is
counted once.
"""

import os
import struct
import sys

STATES = ("installed", "armed", "pending-import", "refused", "disabled")
# Refusal categories that only say "not validated here": expected on a test matrix.
EXPECTED_REFUSALS = ("version", "build", "jit", "platform", "unvalidated-patch")

# What a developer can do about each fallback reason (docs/USAGE.md "See what it does").
FALLBACK_ADVICE = {
    "args": "inputs outside the kernel's exact subset (a subclass, an unsupported keyword or "
            "type, a huge input): CPython's code ran. Expected for some calls; the subset is in "
            "docs/targets/<group>.md",
    "deps": "something the target depends on was replaced (mock.patch, a monkeypatch, gevent): "
            "CPython's code ran until it was restored",
    "tools": "a tracer, profiler, debugger or coverage was listening: Python-level targets step "
             "aside so tools see CPython's frames. Measure without coverage (a separate CI job)",
    "interp": "called in a subinterpreter: CPython's code runs there",
    "deopt": "the kernel met something it cannot decide and restarted in CPython's code before "
             "anything observable (it paid only its checks)",
    "continue": "the kernel handed over midway and CPython's algorithm finished from the same "
                "state (no work repeated)",
    "abandon": "the kernel restarted in CPython's code after partial work, which was wasted. "
               "Persistent waste suspends the target",
    "error": "an error path: CPython's code re-ran the call to raise its own exception",
}
SUSPENDED_ADVICE = {
    "fallback": "persistent fallback: too few calls took the fast path, so CPython's original "
                "was restored until the next enable(). Nothing to do; RUSTIQUE_EXCLUDE=<target> "
                "skips the attempts",
    "deps": "a pinned dependency changed: the original runs until it is restored",
    "conflict": "CPython or another tool rewrote the slot the kernel was installed in: the "
                "original runs",
}
REFUSAL_ADVICE = {
    "version": "this CPython version is not one these targets are validated for. CPython's code "
               "runs; nothing to do",
    "unvalidated-patch": "this CPython patch release was not validated by this Rustique release. "
                         "Upgrade rustique, or set RUSTIQUE_UNVALIDATED_PATCH=1 to install the "
                         "targets whose code matches a validated release (reported as unvalidated)",
    "platform": "not validated on this platform (OS, C library, architecture). CPython's code "
                "runs; nothing to do (docs/USAGE.md#platforms)",
    "build": "this build of CPython (debug, or an unvalidated configuration) is not validated",
    "jit": "the JIT is on and these targets are not validated with it (PYTHON_JIT=0 to "
           "accelerate them)",
    "fingerprint": "the standard-library code differs from the validated release (a patched or "
                   "vendored CPython?). CPython's code runs; report it if this is a stock build",
    "conflict": "another tool replaced the entry point first (gevent, an APM agent, a "
                "monkeypatch). CPython's (or that tool's) code runs",
    "deps": "a module the target relies on is missing or not the validated one",
    "layout": "this CPython's object layout differs from the validated one. Please report it "
              "(python -m rustique status --json)",
    "symbol": "a CPython function the target needs is not exported here. Please report it",
    "selftest": "a behavioural self-test failed. Please report it (python -m rustique status --json)",
    "other": "see the reason",
}

# --- live counter files (written by the extension; layout agreed with accel) -----------------

CNT_SUFFIX = ".cnt"
_MAGIC = b"RQCNT\x00\x00"
# Row columns per layout version (the magic's last byte); the header's word at offset 56 gives
# the row length (0 in v1 files: 9).
_COLUMNS = {
    1: ("hits", "args", "deps", "tools", "interp", "deopt", "error", "check", "suspensions"),
    2: ("hits", "args", "deps", "tools", "interp", "deopt", "error", "check", "suspensions", "continue", "abandon"),
}


def read_counter_file(path: str) -> dict | None:
    """``{"pid", "ppid", "exit_record": bool, "stats": {name: {"hits", "fallback", "suspensions"}}}``
    from a live counter file, or None if it is not one (or is still being created)."""
    try:
        with open(path, "rb") as f:
            data = f.read()
    except OSError:
        return None
    if len(data) < 64 or data[:7] != _MAGIC:
        return None
    version = data[7]
    pid, ppid, n, names_off, counters_off, flags, row_words = struct.unpack_from("=7Q", data, 8)
    cols = _COLUMNS.get(version) or _COLUMNS[max(_COLUMNS)]
    row_words = row_words or len(cols)
    if not (64 <= names_off <= counters_off <= len(data)) or counters_off + n * row_words * 8 > len(data):
        return None
    names = data[names_off:counters_off].split(b"\0")[:n]
    stats = {}
    for i, raw in enumerate(names):
        row = struct.unpack_from(f"={row_words}Q", data, counters_off + i * row_words * 8)
        vals = dict(zip(cols, row))
        fb = {k: v for k, v in vals.items() if k not in ("hits", "suspensions")}
        if vals.get("hits") or any(fb.values()) or vals.get("suspensions"):
            stats[raw.decode("utf-8", "replace")] = {"hits": vals.get("hits", 0), "fallback": fb,
                                                     "suspensions": vals.get("suspensions", 0)}
    return {"pid": pid, "ppid": ppid or None, "exit_record": bool(flags & 1), "stats": stats}


def read_json_records(path: str) -> list[dict]:
    """The JSON records appended to ``path`` (one per line; a torn last line is skipped)."""
    import json

    out = []
    try:
        f = open(path)
    except OSError:
        return out
    with f:
        for line in f:
            line = line.strip()
            if line:
                try:
                    out.append(json.loads(line))
                except ValueError:
                    pass  # a process killed mid-write
    return out


def collect(path: str) -> list[dict]:
    """Every process's record for a JSON report ``path``: the records written at exit, plus a
    counters-only record (``"status": None``, ``"exit_record": False``) for every process that
    left a live counter file and no record (it ended in ``os._exit`` or was killed)."""
    import glob

    records = read_json_records(path)
    seen = {r.get("pid") for r in records}
    for cnt in sorted(glob.glob(glob.escape(path) + ".*" + CNT_SUFFIX)):
        c = read_counter_file(cnt)
        if c is None or c["pid"] in seen or c["exit_record"]:
            continue
        seen.add(c["pid"])
        records.append({"pid": c["pid"], "ppid": c["ppid"], "argv": None, "status": None, "stats": c["stats"],
                        "counters": True, "exit_record": False})
    return records


# --- writing ------------------------------------------------------------------------------------


def write(target: str, record) -> None:
    """The report at exit (``RUSTIQUE_REPORT``): text on stderr (``"stderr"``), or appended
    to a file, as text or (``*.json``) as one JSON record. Never fails the program's exit."""
    try:
        rec = record()
        if target.endswith(".json"):
            import json

            text = json.dumps(rec, default=list) + "\n"
        else:
            text = render(rec)
        if target == "stderr":
            sys.stderr.write(text)
            sys.stderr.flush()
        else:
            fd = os.open(target, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o644)
            try:
                os.write(fd, text.encode())
            finally:
                os.close(fd)
    except Exception as exc:
        sys.stderr.write(f"rustique: report failed: {type(exc).__name__}: {exc}\n")


# --- merging ------------------------------------------------------------------------------------

_RANK = {s: i for i, s in enumerate(STATES)}


def _ppid(rec: dict) -> int | None:
    return rec.get("ppid") or ((rec.get("status") or {}).get("process") or {}).get("ppid")


def merge(records: list[dict]) -> dict:
    """Combine the records of several processes: counters add up (each process counts its own
    calls: a forked child starts from zero); a target counts as installed if it was in any
    process, refused if refused in all. Counters-only records (no exit record) add counters."""
    full = [r for r in records if r.get("status")]
    if len(records) == 1 and full:
        rec = dict(records[0])
        rec.setdefault("processes", 1)
        return rec
    targets: dict[str, dict] = {}
    counters: dict[str, dict] = {}
    for rec in full:
        for t in rec["status"].get("targets", []):
            cur = targets.get(t["name"])
            if cur is None or _RANK.get(t["state"], 9) < _RANK.get(cur["state"], 9):
                targets[t["name"]] = dict(t)
    for rec in records:
        for name, c in (rec.get("stats") or {}).items():
            acc = counters.setdefault(name, {"hits": 0, "fallback": {}, "suspensions": 0})
            acc["hits"] += c.get("hits", 0)
            acc["suspensions"] += c.get("suspensions", 0) or 0
            for reason, n in (c.get("fallback") or {}).items():
                acc["fallback"][reason] = acc["fallback"].get(reason, 0) + n
    first = full[0] if full else records[0]
    status = {"targets": list(targets.values()),
              "process": dict((first.get("status") or {}).get("process", {}))}
    status["process"].pop("ppid", None)
    return {"pid": None, "processes": len(records), "argv": first.get("argv"), "status": status,
            "stats": counters, "counters": any(r.get("counters") for r in records),
            "forked": sum(1 for r in records if _ppid(r)),
            "counters_only": sum(1 for r in records if not r.get("status"))}


# --- rendering ----------------------------------------------------------------------------------


def _num(n: int) -> str:
    return f"{n:,}"


def _fallback_total(c: dict | None) -> int:
    return sum((c.get("fallback") or {}).values()) if c else 0


def _fallbacks(c: dict | None) -> str:
    if not c:
        return ""
    fb = {k: v for k, v in (c.get("fallback") or {}).items() if v}
    if not fb:
        return "0"
    detail = ", ".join(f"{k} {_num(v)}" for k, v in sorted(fb.items(), key=lambda kv: -kv[1]))
    return f"{_num(sum(fb.values()))} ({detail})"


def header(rec: dict) -> str:
    proc = rec["status"].get("process", {})
    sw = proc.get("switch", {})
    build = "free-threaded" if proc.get("free_threaded") or proc.get("build") == "free-threaded" else "GIL"
    if rec.get("pid"):
        where = f"pid {rec['pid']}"
    else:
        n = rec.get("processes", 1)
        extra = []
        if rec.get("forked"):
            extra.append(f"{rec['forked']} forked")
        if rec.get("counters_only"):
            extra.append(f"{rec['counters_only']} ended without exit hooks, counted from their live counters")
        where = f"{n} processes" + (f" ({', '.join(extra)})" if extra else "")
    if sw.get("killed"):
        how = "disabled by RUSTIQUE=0 (kill switch)"
    elif sw.get("source"):
        how = f"enabled by {sw['source']}"
    else:
        how = "not enabled"
    return (f"rustique {proc.get('rustique', '?')} on CPython {proc.get('python', '?')} ({build}), "
            f"{where}: {how}")


def mismatches(rec: dict) -> dict[str, int]:
    """Check-mode mismatches per target (``fallback["check"]``)."""
    out = {}
    for name, c in (rec.get("stats") or {}).items():
        n = (c.get("fallback") or {}).get("check", 0)
        if n:
            out[name] = n
    return out


def check_lines(rec: dict) -> list[str]:
    proc = rec["status"].get("process", {})
    mode = proc.get("check")
    bad = mismatches(rec)
    if mode in (None, "off") and not bad:
        return []
    checkable = sum(1 for t in rec["status"].get("targets", [])
                    if t.get("checkable") and t["state"] in ("installed", "armed"))
    lines = [f"  check mode: {mode or 'off'} ({checkable} installed checkable targets re-run CPython's original "
             "after each fast path and compare)"]
    if bad:
        lines.append(f"  CHECK MISMATCHES: {sum(bad.values()):,} in "
                     + ", ".join(f"{k} ({v:,})" for k, v in sorted(bad.items(), key=lambda kv: -kv[1]))
                     + " -- CPython's result was used; please report this")
    else:
        lines.append("  check mismatches: none")
    return lines


def tier_lines(status: dict) -> list[str]:
    """Opt-in tiers: which exist, which are on, and the switch."""
    tiers: dict[str, list[dict]] = {}
    for t in status.get("targets", []):
        if t.get("tier"):
            tiers.setdefault(t["tier"], []).append(t)
    sw = status.get("process", {}).get("switch", {})
    on = list(sw.get("tiers") or ())
    if not tiers and not on:
        return []
    parts = []
    for name in sorted(set(tiers) | set(on)):
        ts = tiers.get(name, [])
        state = "on" if name in on else "off"
        if ts:
            active = sum(t["state"] in ("installed", "armed", "pending-import") for t in ts)
            parts.append(f"{name} {state} ({active} of {len(ts)} targets active)")
        else:
            parts.append(f"{name} {state}")
    off = sorted(n for n in tiers if n not in on)
    hint = f"; RUSTIQUE_TIERS={','.join(off)} turns {'it' if len(off) == 1 else 'them'} on" if off else ""
    return [f"  opt-in tiers: {', '.join(parts)}{hint}"]


def refusal_lines(refused: list[dict], *, verbose: bool) -> list[str]:
    """Refusals grouped by category, each with what to do about it."""
    if not refused:
        return []
    by: dict[str, list[dict]] = {}
    for t in refused:
        by.setdefault(t.get("refusal") or "other", []).append(t)
    order = sorted(by, key=lambda c: (c in EXPECTED_REFUSALS, -len(by[c]), c))
    lines = ["", "  refused (CPython's code runs):"]
    for cat in order:
        ts = sorted(by[cat], key=lambda t: t["name"])
        names = [t["name"] for t in ts]
        shown = names if verbose or len(names) <= 6 else names[:5] + [f"... {len(names) - 5} more"]
        lines.append(f"    [{cat}] {len(ts)}: {', '.join(shown)}")
        lines.append(f"      what to do: {REFUSAL_ADVICE.get(cat, REFUSAL_ADVICE['other'])}")
        reasons = list(dict.fromkeys(t.get("reason") for t in ts if t.get("reason")))
        for r in reasons[: (len(reasons) if verbose else 2)]:
            lines.append(f"      reason: {r}")
        if not verbose and len(reasons) > 2:
            lines.append(f"      ({len(reasons) - 2} more reasons: python -m rustique status)")
    return lines


def advice_lines(counters: dict, status: dict) -> list[str]:
    """For each fallback reason that occurred, and for suspensions: what it means, and the
    targets it hit most."""
    by_reason: dict[str, list[tuple[int, str]]] = {}
    for name, c in counters.items():
        for reason, n in (c.get("fallback") or {}).items():
            if n and reason != "check":
                by_reason.setdefault(reason, []).append((n, name))
    lines = []
    for reason in sorted(by_reason, key=lambda r: -sum(n for n, _ in by_reason[r])):
        top = sorted(by_reason[reason], reverse=True)[:3]
        who = ", ".join(f"{name} {_num(n)}" for n, name in top)
        lines.append(f"    {reason} ({who}): {FALLBACK_ADVICE.get(reason, 'see docs/USAGE.md')}")
    suspended = {n for n, c in counters.items() if c.get("suspensions")}
    now = {t["name"]: t["suspended"] for t in status.get("targets", []) if t.get("suspended")}
    if suspended or now:
        names = sorted(suspended | set(now))
        kinds = sorted(set(now.values())) or ["fallback"]
        lines.append(f"    suspended ({', '.join(names[:5])}{', ...' if len(names) > 5 else ''}): "
                     + " / ".join(SUSPENDED_ADVICE.get(k, k) for k in kinds))
    return ["", "  what the fallbacks mean here:", *lines] if lines else []


def render(rec: dict, *, limit: int | None = None, title: bool = True, counters_note: bool = True,
           verbose: bool = False) -> str:
    """The report: header, one line per target that ran (sorted by hits), what the fallbacks
    and suspensions mean here, then the targets waiting for their module, refused (by
    category, with what to do) and not selected. ``verbose``: every installed target and every
    name."""
    status = rec["status"]
    counters = rec.get("stats") or {}
    have_counters = bool(rec.get("counters"))
    proc = status.get("process", {})
    sw = proc.get("switch", {})
    targets = status.get("targets", [])
    by_state: dict[str, list[dict]] = {s: [] for s in STATES}
    for t in targets:
        by_state.setdefault(t["state"], []).append(t)
    lines = [header(rec)] if title else []
    if sw.get("killed"):
        n = sw.get("killed_calls", 0)
        lines.append(f"  nothing installed; {n} enable call(s) ignored. Unset RUSTIQUE (or set RUSTIQUE=1) to enable.")
        return "\n".join(lines) + "\n"
    if proc.get("interpreter"):
        lines.append(f"  refused on this interpreter: {proc['interpreter']}")
    live = by_state["installed"] + by_state["armed"]
    called = [t for t in live if (counters.get(t["name"]) or {}).get("hits") or _fallback_total(counters.get(t["name"]))]
    counts = []
    if have_counters and live:
        counts.append(f"{len(called)} called")
        if len(live) > len(called):
            counts.append(f"{len(live) - len(called)} installed but not called")
    elif live:
        counts.append(f"{len(live)} installed")
    counts += [f"{len(by_state[s])} {label}" for s, label in (("pending-import", "waiting for their module"),
                                                                ("refused", "refused"), ("disabled", "not selected"))
               if by_state.get(s)]
    lines.append(f"  {len(targets)} targets: {', '.join(counts) or 'none'}")
    unvalidated = [t for t in live if t.get("validation") == "unvalidated"]
    if unvalidated:
        lines.append(f"  {len(unvalidated)} installed as unvalidated (RUSTIQUE_UNVALIDATED_PATCH=1): their code matches "
                     "a validated release, but this patch release was not validated")
    lines += tier_lines(status)
    if not counters_note:
        pass
    elif not have_counters:
        lines.append("  counters: not available (this build's kernels have no counters)")
    elif not (proc.get("stats") or sw.get("stats")) and not any(c.get("hits") for c in counters.values()):
        lines.append("  counters: off (set RUSTIQUE_STATS=1, or use `python -m rustique report -- <cmd>`)")
    lines += check_lines(rec)
    rows = called if have_counters and not verbose else live
    if rows:
        def key(t):
            c = counters.get(t["name"]) or {}
            return (-c.get("hits", 0), -_fallback_total(c), t["name"])

        rows = sorted(rows, key=key)
        shown = rows if limit is None else rows[:limit]
        w = max(len("target"), *(len(t["name"]) for t in shown))
        g = max(len("group"), *(len(t["group"]) for t in shown))
        lines.append("")
        lines.append(f"  {'target':<{w}}  {'group':<{g}}  {'hits':>12}  {'fast':>5}  fallbacks (reason n)")
        for t in shown:
            c = counters.get(t["name"])
            hits = _num(c.get("hits", 0)) if c else "-"
            total = (c.get("hits", 0) + _fallback_total(c)) if c else 0
            fast = f"{100 * c.get('hits', 0) / total:4.0f}%" if total else "-"
            notes = []
            if c and c.get("suspensions"):
                notes.append(f"suspended {c['suspensions']}x")
            if t.get("suspended"):
                notes.append(f"suspended now ({t['suspended']})")
            if t.get("state") == "armed":
                notes.append("armed: validates at its first call")
            if t.get("validation") == "unvalidated":
                notes.append("unvalidated")
            tail = f"  [{'; '.join(notes)}]" if notes else ""
            lines.append(f"  {t['name']:<{w}}  {t['group']:<{g}}  {hits:>12}  {fast:>5}  {_fallbacks(c)}{tail}".rstrip())
        if len(shown) < len(rows):
            lines.append(f"  ... {len(rows) - len(shown)} more")
        if have_counters and not verbose and len(live) > len(called):
            lines.append(f"  ({len(live) - len(called)} installed targets were not called)")
    lines += advice_lines(counters, status)
    pending = by_state["pending-import"]
    if pending:
        lines.append("")
        if verbose:
            lines.append("  waiting for their module (installed when it is imported): "
                         + ", ".join(t["name"] for t in sorted(pending, key=lambda t: t["name"])))
        else:
            mods = sorted({t.get("module") or t["group"] for t in pending})
            lines.append(f"  {len(pending)} targets wait for their module ({', '.join(mods[:8])}"
                         f"{', ...' if len(mods) > 8 else ''}): installed when it is imported")
    lines += refusal_lines(by_state["refused"], verbose=verbose)
    disabled = by_state["disabled"]
    if disabled and sw.get("source"):
        groups = sorted({t["group"] for t in disabled if not t.get("tier")})
        if groups:
            lines.append("")
            lines.append(f"  not selected: {', '.join(groups)}")
    return "\n".join(lines) + "\n"


def render_status(status: dict) -> str:
    """``python -m rustique status``: per group, which targets are installed, waiting for
    their module, refused (with the reason and what to do) or not selected; opt-in tiers."""
    targets = status.get("targets", [])
    lines = []
    by_state: dict[str, int] = {}
    for t in targets:
        by_state[t["state"]] = by_state.get(t["state"], 0) + 1
    groups: dict[str, list[dict]] = {}
    for t in targets:
        groups.setdefault(t["group"], []).append(t)
    live = by_state.get("installed", 0) + by_state.get("armed", 0)
    pending = by_state.get("pending-import", 0)
    lines.append(f"{len(targets)} targets in {len(groups)} groups: {live + pending} available here ({live} active now, "
                 f"{pending} activate when their module is imported)"
                 + "".join(f", {by_state[s]} {s}" for s in ("refused", "disabled") if by_state.get(s)))
    lines += [ln[2:] for ln in tier_lines(status)]
    if not targets:
        return "\n".join(lines) + "\n"
    gw = max(len(g) for g in groups)
    for g in sorted(groups):
        ts = sorted(groups[g], key=lambda t: t["name"])
        states: dict[str, list[str]] = {}
        for t in ts:
            states.setdefault(t["state"], []).append(t["name"])
        summary = ", ".join(f"{len(v)} {k}" for k, v in sorted(states.items(), key=lambda kv: STATES.index(kv[0])
                                                                 if kv[0] in STATES else 9))
        tier = next((t["tier"] for t in ts if t.get("tier")), None)
        lines.append(f"  {g:<{gw}}  {summary}{f'  [opt-in tier: {tier}]' if tier else ''}")
        for state in STATES:
            if states.get(state):
                lines.append(f"  {'':<{gw}}    {state}: {', '.join(states[state])}")
    lines += [ln[2:] for ln in refusal_lines([t for t in targets if t["state"] == "refused"], verbose=True)]
    return "\n".join(lines) + "\n"
