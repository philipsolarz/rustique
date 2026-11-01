import sys, time, datetime as D, zoneinfo
import rustique._takeover as rustique
from rustique._rustique import _accel
rustique.enable(only=["datetime"])
import _strptime  # trigger strptime targets
st = rustique.status()
for t in st["targets"]:
    if t["group"] == "datetime":
        print(t["name"], t["state"], t.get("reason"), t.get("refusal"))
rustique.stats(True)
from datetime import datetime, date, timezone, timedelta
cases = [
    lambda: datetime.strptime("15/Mar/2024:13:45:30 +0100", "%d/%b/%Y:%H:%M:%S %z"),
    lambda: datetime.strptime("2024-03-15 13:45:30", "%Y-%m-%d %H:%M:%S"),
    lambda: datetime.strptime("2024-03-15T13:45:30.123456+02:00", "%Y-%m-%dT%H:%M:%S.%f%z"),
    lambda: datetime.strptime("03/15/2024 01:45 PM", "%m/%d/%Y %I:%M %p"),
    lambda: date.strptime("2024-03-15", "%Y-%m-%d"),
    lambda: D.time.strptime("13:45:30", "%H:%M:%S"),
    lambda: time.strptime("2024-03-15 13:45:30", "%Y-%m-%d %H:%M:%S"),
    lambda: time.strptime("Fri Mar 15 13:45:30 2024"),
    lambda: datetime(2024,3,15,13,45,30,123456).strftime("%Y-%m-%d %H:%M:%S.%f %a %A %b %B %p %j %Z%z"),
    lambda: datetime(2024,3,15,13,45,30,tzinfo=timezone.utc).strftime("%Y-%m %Z %z %:z"),
    lambda: datetime(2024,3,15,13,45,30,tzinfo=timezone(timedelta(hours=-5, minutes=-30))).strftime("%Z|%z|%:z"),
    lambda: datetime(2024,3,15,13,45,30,tzinfo=zoneinfo.ZoneInfo("Europe/Berlin")).strftime("%a %d %b %Y %H:%M %Z %z"),
    lambda: date(987,1,2).strftime("%Y|%F|%C|%G"),
    lambda: D.time(1,2,3,4).strftime("%H:%M:%S.%f %j %a"),
    lambda: time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(1710510330)),
    lambda: time.strftime("%a %c", (2024,3,15,13,45,30,4,75,0)),
    lambda: f"{datetime(2024,3,15):%Y/%m/%d}",
]
res = [repr(c()) for c in cases]
st = rustique.stats()
print({k: (v['hits'], {r: n for r, n in v['fallback'].items() if n}) for k, v in st.items() if 'time' in k})
print(_accel._datetime_debug())
rustique.disable()
ref = [repr(c()) for c in cases]
for a, b in zip(res, ref):
    print("OK " if a == b else "BAD", a, "" if a == b else b)
