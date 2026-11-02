"""Calendar/scheduling batch job: parse event timestamps in several formats, convert
between IANA time zones, expand weekly recurrences across DST changes, detect room
double-bookings, and render agenda lines.

Stands for the datetime-heavy back-office scripts (booking exports, shift planners,
reminder schedulers, billing-period cutoffs): ``datetime.fromisoformat`` and
``datetime.strptime`` on mixed legacy formats, ``zoneinfo.ZoneInfo`` attach/convert
(``astimezone``), ``timedelta`` arithmetic, ``isocalendar``/``weekday`` bucketing and
``strftime``/``isoformat`` output. Data shape: 24k events in 10 zones over 2025-2026,
each expanded into a few occurrences (~120k datetimes handled).
"""

REQUIRES = ["tzdata==2026.6"]

N_EVENTS = 24_000


def setup():
    import random
    import zoneinfo

    # Use the pinned tzdata package, not the host's /usr/share/zoneinfo, so the result
    # does not depend on the machine.
    zoneinfo.reset_tzpath(to=[])
    rng = random.Random(7)
    zones = ["America/New_York", "America/Los_Angeles", "America/Sao_Paulo", "Europe/London",
             "Europe/Berlin", "Europe/Warsaw", "Asia/Tokyo", "Asia/Kolkata",
             "Australia/Sydney", "UTC"]
    rooms = [f"room-{i:02d}" for i in range(40)]
    events = []
    for i in range(N_EVENTS):
        y = rng.choice([2025, 2026])
        mo = rng.randint(1, 12)
        d = rng.randint(1, 28)
        h = rng.randint(6, 20)
        mi = rng.choice([0, 15, 30, 45])
        fmt = rng.random()
        if fmt < 0.5:
            start = f"{y}-{mo:02d}-{d:02d}T{h:02d}:{mi:02d}:00"
        elif fmt < 0.75:
            start = f"{y}-{mo:02d}-{d:02d} {h:02d}:{mi:02d}"
        elif fmt < 0.9:
            start = f"{mo:02d}/{d:02d}/{y} {h % 12 or 12:02d}:{mi:02d} {'AM' if h < 12 else 'PM'}"
        else:
            start = f"{d:02d}.{mo:02d}.{y} {h:02d}:{mi:02d}"
        events.append({
            "id": i,
            "start": start,
            "tz": rng.choice(zones),
            "minutes": rng.choice([15, 30, 45, 60, 90, 120]),
            "room": rng.choice(rooms),
            "repeat_weeks": rng.choice([0, 0, 0, 1, 3, 6, 12]),
            "viewer_tz": rng.choice(zones),
        })
    return {"events": events}


def run(state):
    import functools
    from collections import Counter, defaultdict
    from datetime import UTC, datetime, timedelta
    from zoneinfo import ZoneInfo

    # ZoneInfo(key) keeps only 8 zones strongly cached; with 10 zones in rotation the
    # rest are re-read from disk every few calls (measured: ~40% of this run). Scripts
    # that care memoise the lookup, as here.
    get_zone = functools.lru_cache(maxsize=None)(ZoneInfo)

    def parse(s):
        if "T" in s:
            return datetime.fromisoformat(s)
        if "/" in s:
            return datetime.strptime(s, "%m/%d/%Y %I:%M %p")
        if "." in s[:6]:
            return datetime.strptime(s, "%d.%m.%Y %H:%M")
        return datetime.strptime(s, "%Y-%m-%d %H:%M")

    by_room = defaultdict(list)
    per_week = Counter()
    per_weekday = Counter()
    off_hours = 0
    dst_shifted = 0
    agenda = []
    for ev in state["events"]:
        tz = get_zone(ev["tz"])
        viewer = get_zone(ev["viewer_tz"])
        local = parse(ev["start"]).replace(tzinfo=tz)
        dur = timedelta(minutes=ev["minutes"])
        first_offset = local.utcoffset()
        for k in range(ev["repeat_weeks"] + 1):
            occ = local + timedelta(weeks=k)  # wall-clock recurrence, like calendar apps
            if occ.utcoffset() != first_offset:
                dst_shifted += 1
            start_utc = occ.astimezone(UTC)
            end_utc = start_utc + dur
            by_room[ev["room"]].append((start_utc, end_utc, ev["id"]))
            iso = start_utc.isocalendar()
            per_week[f"{iso.year}-W{iso.week:02d}"] += 1
            seen = start_utc.astimezone(viewer)
            per_weekday[seen.strftime("%A")] += 1
            if not 8 <= seen.hour < 18 or seen.weekday() >= 5:
                off_hours += 1
            if k == 0:
                agenda.append(
                    f"{seen.strftime('%a %d %b %Y %H:%M %Z')} "
                    f"({seen.isoformat(timespec='minutes')}) "
                    f"#{ev['id']} {ev['room']} {int(dur.total_seconds() // 60)}min")
    conflicts = 0
    busiest_gap = timedelta(0)
    for room, slots in sorted(by_room.items()):
        slots.sort()
        last_end = None
        for start, end, _ in slots:
            if last_end is not None:
                if start < last_end:
                    conflicts += 1
                else:
                    busiest_gap = max(busiest_gap, start - last_end)
            if last_end is None or end > last_end:
                last_end = end
    agenda.sort()
    return {
        "conflicts": conflicts,
        "dst_shifted": dst_shifted,
        "off_hours": off_hours,
        "max_gap_h": busiest_gap.total_seconds() / 3600,
        "weeks": sorted(per_week.items())[:20],
        "weekdays": sorted(per_weekday.items()),
        "agenda_head": agenda[:50],
        "agenda_len": len(agenda),
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
