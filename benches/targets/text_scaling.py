"""Thread scaling of the `text` workstream's kernels (logging, datetime): CPU time per call
at 1..8 threads, plain vs Rustique (DECISIONS §10.9: at 8 threads ≤ 1.05× CPython's
original on 3.15.1t). Same harness and options as benches/ft/kernel_scaling.py, with the
cells below added (`--cells` selects among all of them):

    python benches/targets/text_scaling.py -t 1,8 -o ks-plain.json --cells <list>
    python benches/targets/text_scaling.py -t 1,8 --enable -o ks-enabled.json --cells <list>
    python benches/targets/text_scaling.py table ks-plain.json ks-enabled.json
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "ft"))
import kernel_scaling as ks  # noqa: E402

# A logger per thread for `local` (its own handler and stream), one for `shared`.
_LOGGER = ("import io, logging, threading\n"
           "L = logging.getLogger('ks.' + str(threading.get_ident()))\n"
           "L.propagate = False\nL.setLevel(logging.INFO)\nL.handlers.clear()\n"
           "h = logging.StreamHandler(io.StringIO())\n"
           "h.setFormatter(logging.Formatter('%(asctime)s %(levelname)-8s [%(name)s] %(message)s'))\n"
           "L.addHandler(h)\nL.isEnabledFor(10)\n")

ks.CELLS.update({
    "posixpath.join (user PathLike: fallback)": (
        "import posixpath\nclass P:\n    def __fspath__(self):\n        return '/srv/app'\np = P()",
        "posixpath.join(p, 'static')"),
    "logging.findCaller": ("import logging; L = logging.getLogger('svc.api')", "L.findCaller()"),
    "logging.info (StringIO)": (_LOGGER, "L.info('user %s logged in', 'ana')"),
    "logging.debug (disabled)": (_LOGGER, "L.debug('user %s logged in', 'ana')"),
    "logging.format (Decimal arg)": (
        "import decimal, logging; f = logging.Formatter('%(asctime)s %(levelname)s %(name)s: %(message)s'); "
        "r = logging.LogRecord('svc.api', 20, '/srv/app.py', 10, 'total %s', (decimal.Decimal('12.50'),), None)",
        "f.format(r)"),
    "strftime ZoneInfo names": (
        "from datetime import datetime; import zoneinfo; "
        "x = datetime(2026, 11, 2, 18, 21, tzinfo=zoneinfo.ZoneInfo('Europe/Berlin'))",
        "x.strftime('%a %d %b %Y %H:%M %Z')"),
    "strftime numeric": ("from datetime import datetime; x = datetime(2026, 11, 2, 18, 21, 47)",
                         "x.strftime('%Y-%m-%d %H:%M:%S')"),
    "strptime access log": ("from datetime import datetime; s = '02/Nov/2026:18:21:47 +0100'",
                            "datetime.strptime(s, '%d/%b/%Y:%H:%M:%S %z')"),
    "isoformat ZoneInfo": (
        "from datetime import datetime; import zoneinfo; "
        "x = datetime(2026, 11, 2, 18, 21, tzinfo=zoneinfo.ZoneInfo('Europe/Berlin'))",
        "x.isoformat(timespec='minutes')"),
})

TEXT_CELLS = ("logging.format,logging.LogRecord,logging.findCaller,logging.info (StringIO),"
              "logging.debug (disabled),logging.format (Decimal arg),datetime.strptime,datetime.isoformat,"
              "date.strftime,strftime ZoneInfo names,strftime numeric,strptime access log,isoformat ZoneInfo")

if __name__ == "__main__":
    if "--cells" not in sys.argv and (len(sys.argv) < 2 or sys.argv[1] != "table"):
        sys.argv += ["--cells", TEXT_CELLS]
    ks.main()
