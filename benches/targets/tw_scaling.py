"""Thread scaling of the `textwrap` and `urllib` kernels (DECISIONS §10.9: CPU time per call at
8 threads ≤ 1.05× CPython's original on 3.15.1t), with `benches/ft/kernel_scaling.py`'s
method (weak scaling, per-thread `local` inputs and one `shared` input, CPU time per call
from the workers' `thread_time()`).

    python benches/targets/tw_scaling.py [-t 1,8] [-r 3] [--cells a,b] -o plain.json
    python benches/targets/tw_scaling.py --enable [-t 1,8] -o enabled.json
    python benches/targets/tw_scaling.py table plain.json enabled.json

Run plain and enabled in fresh processes on the same CPU set.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "ft"))

import kernel_scaling as ks  # noqa: E402

DESC = "' '.join(('alpha beta gamma delta epsilon release deploy rollback latency throughput'.split() * 4)) + '.'"
ks.CELLS.clear()
ks.CELLS.update({
    "textwrap.fill": (f"import textwrap; s = {DESC}", "textwrap.fill(s, width=88, initial_indent='> ', subsequent_indent='> ')"),
    "textwrap.wrap": ("import textwrap; s = 'The quick brown fox jumps over the lazy dog ' * 12", "textwrap.wrap(s, 60)"),
    "textwrap.shorten": ("import textwrap; s = 'Incident report for the payments search platform rollout'",
                         "textwrap.shorten(s, 30, placeholder='...')"),
    "textwrap.dedent": ("import textwrap; s = '\\n'.join('    line %d of some indented source' % i for i in range(20))",
                        "textwrap.dedent(s)"),
    "textwrap.indent": ("import textwrap; s = '\\n'.join('line %d' % i for i in range(30))", "textwrap.indent(s, '  ')"),
    "urllib.quote": ("from urllib.parse import quote; s = '/api/v1/items/caf\\u00e9 list'", "quote(s)"),
    "urllib.quote_plus": ("from urllib.parse import quote_plus; s = 'a b&c=d e'", "quote_plus(s)"),
    "urllib.unquote": ("from urllib.parse import unquote; s = 'a%20b%C3%A9/x%2Fy'", "unquote(s)"),
    "urllib.parse_qsl": ("from urllib.parse import parse_qsl; s = 'page=2&sort=name&q=hello+world&tag=a&tag=b'",
                         "parse_qsl(s)"),
    "urllib.parse_qs": ("from urllib.parse import parse_qs; s = 'page=2&sort=name&q=hello+world&tag=a&tag=b'",
                        "parse_qs(s)"),
    "urllib._urlsplit": ("import urllib.parse as up; s = 'https://user@example.com:8443/api/v1/items?page=2#top'",
                         "up._urlsplit(s)"),
})

if __name__ == "__main__":
    ks.main()
