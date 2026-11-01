"""Human-readable reports of ``rustique.status()`` and ``rustique.stats()`` (the report at
exit, ``python -m rustique status|report``, the pytest plugin's summary)."""

STATES = ("installed", "pending-import", "refused", "disabled")


def merge(records: list[dict]) -> dict:
    """Combine the records of several processes (``RUSTIQUE_REPORT=x.json``): counters add
    up; a target counts as installed if it was in any process, refused if refused in all."""
    if len(records) == 1:
        return records[0]
    rank = {"installed": 0, "pending-import": 1, "refused": 2, "disabled": 3}
    targets: dict[str, dict] = {}
    counters: dict[str, dict] = {}
    for rec in records:
        for t in rec["status"].get("targets", []):
            cur = targets.get(t["name"])
            if cur is None or rank.get(t["state"], 9) < rank.get(cur["state"], 9):
                targets[t["name"]] = dict(t)
        for name, c in (rec.get("stats") or {}).items():
            acc = counters.setdefault(name, {"hits": 0, "fallback": {}})
            acc["hits"] += c.get("hits", 0)
            for reason, n in (c.get("fallback") or {}).items():
                acc["fallback"][reason] = acc["fallback"].get(reason, 0) + n
    first = records[0]
    status = {"targets": list(targets.values()), "process": dict(first["status"].get("process", {}))}
    return {"pid": None, "processes": len(records), "argv": first.get("argv"), "status": status,
            "stats": counters, "counters": any(r.get("counters") for r in records)}


def _num(n: int) -> str:
    return f"{n:,}"


def _fallbacks(c: dict | None) -> str:
    if not c:
        return ""
    fb = {k: v for k, v in (c.get("fallback") or {}).items() if v}
    if not fb:
        return "0"
    total = sum(fb.values())
    detail = ", ".join(f"{k} {_num(v)}" for k, v in sorted(fb.items(), key=lambda kv: -kv[1]))
    return f"{_num(total)} ({detail})"


def header(rec: dict) -> str:
    proc = rec["status"].get("process", {})
    sw = proc.get("switch", {})
    build = "free-threaded" if proc.get("free_threaded") or proc.get("build") == "free-threaded" else "GIL"
    where = f"pid {rec['pid']}" if rec.get("pid") else f"{rec.get('processes', 1)} processes"
    if sw.get("killed"):
        how = "disabled by RUSTIQUE=0 (kill switch)"
    elif sw.get("source"):
        how = f"enabled by {sw['source']}"
    else:
        how = "not enabled"
    return (f"rustique {proc.get('rustique', '?')} on CPython {proc.get('python', '?')} ({build}), "
            f"{where}: {how}")


def render(rec: dict, *, limit: int | None = None, title: bool = True, counters_note: bool = True) -> str:
    """The report: header, one line per target that ran (sorted by hits), then the targets
    waiting for their module, refused (with reasons) and not selected."""
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
    counts = ", ".join(f"{len(by_state[s])} {s}" for s in STATES if by_state.get(s))
    lines.append(f"  {len(targets)} targets: {counts or 'none'}")
    if not counters_note:
        pass
    elif not have_counters:
        lines.append("  counters: not available (this build's kernels have no counters)")
    elif not (proc.get("stats") or sw.get("stats")) and not any(c.get("hits") for c in counters.values()):
        lines.append("  counters: off (set RUSTIQUE_STATS=1, or use `python -m rustique report -- <cmd>`)")
    installed = by_state["installed"]
    if installed:
        def key(t):
            c = counters.get(t["name"], {})
            return (-c.get("hits", 0), -sum((c.get("fallback") or {}).values()), t["name"])

        installed = sorted(installed, key=key)
        shown = installed if limit is None else installed[:limit]
        w = max(len("target"), *(len(t["name"]) for t in shown))
        g = max(len("group"), *(len(t["group"]) for t in shown))
        lines.append("")
        lines.append(f"  {'target':<{w}}  {'group':<{g}}  {'hits':>12}  fallbacks (reason n)")
        for t in shown:
            c = counters.get(t["name"])
            hits = _num(c.get("hits", 0)) if c else "-"
            lines.append(f"  {t['name']:<{w}}  {t['group']:<{g}}  {hits:>12}  {_fallbacks(c)}".rstrip())
        if len(shown) < len(installed):
            lines.append(f"  ... {len(installed) - len(shown)} more installed targets")
    pending = by_state["pending-import"]
    if pending:
        lines.append("")
        lines.append("  pending import (patched when their module is imported): "
                     + ", ".join(t["name"] for t in sorted(pending, key=lambda t: t["name"])))
    refused = by_state["refused"]
    if refused:
        lines.append("")
        lines.append("  refused:")
        w = max(len(t["name"]) for t in refused)
        for t in sorted(refused, key=lambda t: t["name"]):
            lines.append(f"    {t['name']:<{w}}  {t.get('reason') or 'no reason given'}")
    disabled = by_state["disabled"]
    if disabled and sw.get("source"):
        groups = sorted({t["group"] for t in disabled})
        lines.append("")
        lines.append(f"  not selected: {', '.join(groups)}")
    return "\n".join(lines) + "\n"


def render_status(status: dict) -> str:
    """``python -m rustique status``: per group, which targets are installed, waiting for
    their module, refused (with the reason) or not selected."""
    targets = status.get("targets", [])
    lines = []
    by_state: dict[str, int] = {}
    for t in targets:
        by_state[t["state"]] = by_state.get(t["state"], 0) + 1
    groups: dict[str, list[dict]] = {}
    for t in targets:
        groups.setdefault(t["group"], []).append(t)
    counts = ", ".join(f"{by_state[s]} {s}" for s in STATES if by_state.get(s))
    lines.append(f"{len(targets)} targets in {len(groups)} groups: {counts or 'none'}")
    if not targets:
        return "\n".join(lines) + "\n"
    gw = max(len(g) for g in groups)
    for g in sorted(groups):
        ts = sorted(groups[g], key=lambda t: t["name"])
        states = {}
        for t in ts:
            states.setdefault(t["state"], []).append(t["name"])
        summary = ", ".join(f"{len(v)} {k}" for k, v in sorted(states.items(), key=lambda kv: STATES.index(kv[0])
                                                                 if kv[0] in STATES else 9))
        lines.append(f"  {g:<{gw}}  {summary}")
        for state in STATES:
            if states.get(state):
                lines.append(f"  {'':<{gw}}    {state}: {', '.join(states[state])}")
    refused = [t for t in targets if t["state"] == "refused"]
    if refused:
        lines.append("")
        lines.append("refused (CPython's original runs):")
        w = max(len(t["name"]) for t in refused)
        for t in sorted(refused, key=lambda t: t["name"]):
            lines.append(f"  {t['name']:<{w}}  {t.get('reason') or 'no reason given'}")
    return "\n".join(lines) + "\n"
