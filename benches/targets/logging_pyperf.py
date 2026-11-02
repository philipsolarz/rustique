"""Logging record path, wall time (pyperf), plain vs `rustique.enable(only=["logging"])`.

Each benchmark runs in pyperf worker processes; the enabled variant enables the logging
targets in its setup before `logging` is imported. Wall time on the shared dev box is
noisy: use it for the >= 1.5x claims, callgrind (`logging_cg.py`) for the rest.

    python benches/targets/logging_pyperf.py -o out.json [--fast] [-p 6]
    python -m pyperf compare_to out.json --table    (pairs: <name> plain / <name> enabled)
"""

import pyperf

SETUP = """
import io, json, logging
sio = io.StringIO()
def mk(name, fmt=None, level=logging.INFO, formatter=None):
    lg = logging.getLogger(name)
    lg.handlers.clear(); lg.propagate = False; lg.setLevel(level)
    h = logging.StreamHandler(sio)
    h.setFormatter(formatter or logging.Formatter(fmt))
    lg.addHandler(h)
    return lg
class JsonFormatter(logging.Formatter):
    def format(self, record):
        return json.dumps({"ts": self.formatTime(record), "level": record.levelname, "logger": record.name,
                           "msg": record.getMessage(), "module": record.module, "line": record.lineno})
L_TIME = mk("p.time", "%(asctime)s %(levelname)-8s [%(name)s] %(message)s")
L_BASIC = mk("p.basic", logging.BASIC_FORMAT)
L_JSON = mk("p.json", formatter=JsonFormatter())
L_OFF = mk("p.off", "%(message)s", logging.WARNING)
def burst(lg, n=1000):
    for i in range(n):
        lg.info("request %s took %d ms", "/api/orders", i)
    sio.seek(0); sio.truncate()
def disabled(n=1000):
    for i in range(n):
        L_OFF.debug("request %s took %d ms", "/api/orders", i)
"""

CASES = {
    "1000 records, asctime format": "burst(L_TIME)",
    "1000 records, BASIC_FORMAT": "burst(L_BASIC)",
    "1000 records, JSON formatter": "burst(L_JSON)",
    "1000 disabled debug calls": "disabled()",
}

if __name__ == "__main__":
    runner = pyperf.Runner()
    for name, stmt in CASES.items():
        runner.timeit(f"{name} plain", stmt, setup=SETUP)
        runner.timeit(f"{name} enabled", stmt, setup="import rustique; rustique.enable(only=['logging'])\n" + SETUP)
