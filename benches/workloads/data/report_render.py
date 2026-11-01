"""Fixture generation + plain-text/Markdown report rendering: build fake-but-realistic
records with random data and UUIDs, then render a Markdown status report and a fixed-width
text summary.

Stands for seed-data/fixture scripts and the report generators that end many batch jobs
(nightly status mails, CLI summaries, changelogs): ``random`` (``choice``/``choices``/
``sample``/``gauss``/``getrandbits``), ``uuid.UUID`` construction, parsing and ``str``,
``uuid.uuid4``, ``secrets.token_hex``/``token_urlsafe``, and string building with
f-strings (format specs, alignment), ``str.format``, ``%``-formatting,
``string.Template``, ``textwrap.fill``/``indent``/``shorten``, ``ljust``/``rjust``/
``center`` and ``"".join``. Data shape: 20,000 records rendered into ~4 MB of text.
"""

REQUIRES: list[str] = []

N_RECORDS = 20_000


def setup():
    words = ("alpha beta gamma delta epsilon release deploy rollback latency throughput "
             "customer invoice backlog sprint incident outage database cache queue worker "
             "migration feature flag rollout canary regression benchmark memory leak "
             "timeout retry checkout payment search index").split()
    return {"words": words, "seed": 20261101}


def run(state):
    import random
    import secrets
    import string
    import textwrap
    import uuid
    from collections import Counter, defaultdict

    rng = random.Random(state["seed"])
    words = state["words"]
    teams = ["payments", "search", "platform", "mobile", "data", "growth"]
    severities = ["low", "medium", "high", "critical"]

    # 1. fixtures
    records = []
    for i in range(N_RECORDS):
        rid = uuid.UUID(int=rng.getrandbits(128), version=4)
        records.append({
            "id": str(rid),
            "team": rng.choice(teams),
            "severity": rng.choices(severities, weights=[50, 30, 15, 5])[0],
            "title": " ".join(rng.sample(words, rng.randint(3, 7))).capitalize(),
            "description": " ".join(rng.choices(words, k=rng.randint(15, 80))) + ".",
            "minutes": max(1, int(rng.gauss(90, 60))),
            "cost": round(rng.uniform(0, 25_000), 2),
            "tags": sorted(rng.sample(words, rng.randint(0, 4))),
        })
    # identifiers that are random by design (session tokens, request ids)
    tokens = [secrets.token_hex(16) for _ in range(2_000)]
    request_ids = [uuid.uuid4() for _ in range(2_000)]
    reparsed = sum(uuid.UUID(r["id"]).version == 4 for r in records)

    # 2. markdown report
    by_team = defaultdict(list)
    for r in records:
        by_team[r["team"]].append(r)
    out = []
    head = string.Template("# Incident report: $period\n\n_Generated for $n records._\n")
    out.append(head.substitute(period="2026-W44", n=len(records)))
    out.append("| Team | Incidents | Critical | Minutes | Cost (EUR) |")
    out.append("|:-----|----------:|---------:|--------:|-----------:|")
    for team in sorted(by_team):
        rs = by_team[team]
        crit = sum(r["severity"] == "critical" for r in rs)
        out.append("| {:<10} | {:>9,d} | {:>8d} | {:>7,d} | {:>10,.2f} |".format(
            team, len(rs), crit, sum(r["minutes"] for r in rs), sum(r["cost"] for r in rs)))
    for team in sorted(by_team):
        out.append(f"\n## {team.title()}\n")
        for r in by_team[team]:
            if r["severity"] not in ("high", "critical"):
                continue
            out.append(f"### [{r['severity'].upper():>8}] {r['title']} (`{r['id'][:8]}`)")
            out.append(textwrap.fill(r["description"], width=88, initial_indent="> ",
                                     subsequent_indent="> "))
            meta = "- duration: %d min, cost: %.2f EUR, tags: %s" % (
                r["minutes"], r["cost"], ", ".join(r["tags"]) or "none")
            out.append(meta)
    markdown = "\n".join(out)

    # 3. fixed-width text summary (the e-mail body)
    lines = ["=" * 78, "WEEKLY SUMMARY".center(78), "=" * 78]
    tag_counts = Counter(t for r in records for t in r["tags"])
    for tag, n in tag_counts.most_common(25):
        bar = "#" * (n * 40 // max(tag_counts.values()))
        lines.append(f"{tag.ljust(14)} {str(n).rjust(6)} {bar}")
    lines.append("-" * 78)
    for r in records[:3000]:
        lines.append(f"{r['id']}  {textwrap.shorten(r['title'], 30, placeholder='...'):<30}"
                     f"  {r['minutes']:>5}m  {r['cost']:>10.2f}")
    summary = textwrap.indent("\n".join(lines), "  ")
    return {
        "markdown": markdown,
        "summary": summary,
        "tokens_ok": all(len(t) == 32 for t in tokens),
        "uuid4_ok": all(u.version == 4 for u in request_ids),
        "reparsed": reparsed,
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
