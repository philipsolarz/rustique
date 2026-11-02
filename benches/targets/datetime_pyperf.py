"""datetime group: wall time of bulk parsing/formatting, plain vs Rustique (pyperf).

    python benches/targets/datetime_pyperf.py [-o out.json] [--fast]
    python benches/targets/datetime_pyperf.py --rustique ...   (worker side; internal)

Each benchmark runs in pyperf worker processes; the `--rustique` variant enables the
`datetime` group before the benchmark module imports anything else. Ratios < 0.67 (1.5x)
are the only wall-time claims made (the shared box is noisy; callgrind proves <= 1.05).
"""

import sys

ENABLE = "--rustique" in sys.argv
if ENABLE:
    sys.argv.remove("--rustique")
    import rustique._takeover as rq

    rq.enable(only=["datetime"])

import datetime as D  # noqa: E402
import time  # noqa: E402

import pyperf  # noqa: E402

N = 1000
LOGS = [f"{1 + i % 28:02d}/{['Jan', 'Feb', 'Mar', 'Apr'][i % 4]}/2024:{i % 24:02d}:{i % 60:02d}:{(7 * i) % 60:02d} +0{i % 3}00"
        for i in range(N)]
ISOS = [f"2024-{1 + i % 12:02d}-{1 + i % 28:02d} {i % 24:02d}:{i % 60:02d}:{(7 * i) % 60:02d}" for i in range(N)]
DTS = [D.datetime(2024, 1 + i % 12, 1 + i % 28, i % 24, i % 60, (7 * i) % 60, i * 997 % 1000000,
                  tzinfo=D.timezone(D.timedelta(hours=i % 5))) for i in range(N)]


def strptime_log(loops):
    p = D.datetime.strptime
    t0 = pyperf.perf_counter()
    for _ in range(loops):
        for s in LOGS:
            p(s, "%d/%b/%Y:%H:%M:%S %z")
    return pyperf.perf_counter() - t0


def strptime_iso(loops):
    p = D.datetime.strptime
    t0 = pyperf.perf_counter()
    for _ in range(loops):
        for s in ISOS:
            p(s, "%Y-%m-%d %H:%M:%S")
    return pyperf.perf_counter() - t0


def strftime_log(loops):
    t0 = pyperf.perf_counter()
    for _ in range(loops):
        for d in DTS:
            d.strftime("%a %d %b %Y %H:%M:%S %z")
    return pyperf.perf_counter() - t0


def time_strftime(loops):
    tts = [d.timetuple() for d in DTS]
    f = time.strftime
    t0 = pyperf.perf_counter()
    for _ in range(loops):
        for t in tts:
            f("%Y-%m-%d %H:%M:%S", t)
    return pyperf.perf_counter() - t0


def main():
    runner = pyperf.Runner(add_cmdline_args=lambda cmd, args: cmd.append("--rustique") if ENABLE else None)
    tag = " [rustique]" if ENABLE else ""
    runner.bench_time_func("strptime log x1000" + tag, strptime_log)
    runner.bench_time_func("strptime iso x1000" + tag, strptime_iso)
    runner.bench_time_func("strftime %a %d %b %Y %H:%M:%S %z x1000" + tag, strftime_log)
    runner.bench_time_func("time.strftime x1000" + tag, time_strftime)


if __name__ == "__main__":
    main()
