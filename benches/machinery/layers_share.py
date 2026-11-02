#!/usr/bin/env python3
"""Distortion-free shares of machinery candidates from w2-attribution's layer profiler.

    python benches/machinery/layers_share.py <layers-profile.json>... [--json out.json]

Input: `benches/atlas/layers.py profile --modes plain` output (w2-attribution, commit
08867fb): plain `perf record --call-graph dwarf` without -X perf (specialized calls stay
inlined), each sample's owner = the innermost Python code object found from the bytecode
pointer. A candidate's share = samples whose innermost code object is one of the
candidate's functions (including the native code below it). For leaf machinery (no other
Python code under it) this equals the inclusive share; machinery that calls other
CPython Python code (inspect -> enum, singledispatch -> weakref) is split across both.
"""

import json
import re
import sys
from collections import Counter, defaultdict

FAMILY = {
    "web": "aiohttp_app asyncio_http django_blog email_mime fastapi_pydantic flask_jinja json_api "
           "logging_service requests_local sqlalchemy_orm",
    "devtools": "black_format coverage_pyflakes_tests isort_stdlib pip_resolve pycodestyle_stdlib "
                "pyflakes_stdlib pylint_stdlib pytest_networkx rich_render sphinx_autodoc sqlglot_transpile",
    "frameworks": "django_admin drf_api falcon_api fastapi_sqlalchemy flask_marshmallow strawberry_graphql",
    "libs": "attrs_cattrs boto3_client bs4_scrape jsonschema_validate pydantic_payloads yaml_config",
    "custom": "event_ledger fulfilment_sim mini_interp order_domain rules_engine",
}
FAM = {w: f for f, ws in FAMILY.items() for w in ws.split()}

CANDS = [
    ("enum: all", r"^enum\.py:"),
    ("enum: .value/.name (property.__get__)", r"^enum\.py:(property\.__get__|Enum\.value|Enum\.name)$"),
    ("enum: value lookup (EnumType.__call__/Enum.__new__)", r"^enum\.py:(EnumType\.__call__|Enum\.__new__)$"),
    ("enum: Flag operators", r"^enum\.py:Flag\.(__or__|__and__|__xor__|__invert__|__contains__|_get_value|__ror__|__rand__)"),
    ("enum: Enum.__hash__", r"^enum\.py:Enum\.__hash__$"),
    ("dataclass generated __init__", r"^<string>:__create_fn__\.<locals>\.__init__$"),
    ("dataclass generated other (__eq__/__hash__/__repr__/order/frozen setattr)", r"^<string>:__create_fn__\.<locals>\.(?!__init__$)"),
    ("namedtuple generated __new__", r"^<string>:<lambda>$"),
    ("namedtuple methods (_make/_replace/_asdict/__repr__)", r"^collections/__init__\.py:namedtuple\.<locals>\."),
    ("total_ordering methods", r"^functools\.py:_(gt|lt|ge|le)_from_"),
    ("contextlib: @contextmanager helper + __init__", r"^contextlib\.py:(contextmanager\.<locals>\.helper|_GeneratorContextManagerBase\.__init__|asynccontextmanager\.<locals>\.helper)$"),
    ("contextlib: __enter__/__exit__ (ruling 15)", r"^contextlib\.py:_(Async)?GeneratorContextManager\.__(a?enter|a?exit)__$"),
    ("contextlib: ExitStack/other", r"^contextlib\.py:"),
    ("functools: singledispatch(method)", r"^functools\.py:.*singledispatch"),
    ("functools: cached_property", r"^functools\.py:cached_property"),
    ("functools: other", r"^functools\.py:"),
    ("inspect", r"^inspect\.py:"),
    ("typing", r"^typing\.py:"),
    ("abc (ABCMeta.__instancecheck__/...)", r"^(<frozen abc>|abc\.py):"),
    ("collections.abc mixins", r"^(<frozen _collections_abc>|_collections_abc\.py):"),
    ("weakref", r"^(weakref|_weakrefset)\.py:"),
    ("re module functions", r"^re/__init__\.py:"),
    ("uuid", r"^uuid\.py:"),
    ("html", r"^html/"),
    ("base64", r"^base64\.py:"),
    ("hmac", r"^hmac\.py:"),
    ("http.client", r"^http/client\.py:"),
    ("email", r"^email/"),
    ("tokenize", r"^tokenize\.py:"),
    ("ipaddress", r"^ipaddress\.py:"),
    ("string (Template/Formatter)", r"^string/"),
    ("os (environ)", r"^(<frozen os>|os\.py):"),
    ("threading/queue", r"^(threading|queue)\.py:"),
    ("asyncio", r"^asyncio/"),
    # wave-1 groups, for scale
    ("[w1] json", r"^json/"),
    ("[w1] copy", r"^copy\.py:"),
    ("[w1] logging", r"^logging/"),
    ("[w1] urllib.parse", r"^urllib/parse\.py:"),
    ("[w1] paths", r"^(posixpath|<frozen posixpath>|genericpath|<frozen genericpath>|pathlib/|fnmatch)"),
    ("[w1] dataclasses asdict/astuple/replace", r"^dataclasses\.py:(asdict|astuple|replace|_asdict_inner|_astuple_inner)"),
]
_RX = [(n, re.compile(r)) for n, r in CANDS]


def shares(path):
    d = json.load(open(path))
    w = d["workload"].split("/")[-1]
    cnt = d["plain"]["counter"]
    tot = sum(v for _, v in cnt)
    code = Counter()
    for key, v in cnt:
        code[key[4]] += v
    out = defaultdict(float)
    # "all" and "other" rows are not exclusive with the specific ones; specific rows are
    # matched first-hit-wins within their family (contextlib/functools/enum blocks)
    for c, v in code.items():
        if c.startswith("enum.py:"):
            out["enum: all"] += v / tot
        for n, rx in _RX:
            if n == "enum: all":
                continue
            if rx.search(c):
                out[n] += v / tot
                break
    return w, dict(out)


def main():
    argv = sys.argv[1:]
    js = None
    if "--json" in argv:
        i = argv.index("--json"); js = argv[i + 1]; del argv[i:i + 2]
    per = dict(shares(p) for p in argv)
    fams = [f for f in FAMILY if any(FAM.get(w) == f for w in per)]
    rows = []
    for n, _ in CANDS:
        row = {"candidate": n, "all_mean": sum(s.get(n, 0) for s in per.values()) / len(per)}
        for f in fams:
            ws = [w for w in per if FAM.get(w) == f]
            vals = [(per[w].get(n, 0.0), w) for w in ws]
            row[f] = (sum(v for v, _ in vals) / len(ws), max(vals))
        rows.append(row)
    print(f"{len(per)} workloads: " + ", ".join(f"{f} {sum(FAM.get(w) == f for w in per)}" for f in fams))
    print("| candidate | all mean | " + " | ".join(f"{f} mean / max" for f in fams) + " |")
    print("|---|---:|" + "---|" * len(fams))
    for r in rows:
        cells = " | ".join(f"{100 * r[f][0]:.2f} / {100 * r[f][1][0]:.1f} ({r[f][1][1]})" if r[f][1][0] >= 0.0005
                           else f"{100 * r[f][0]:.2f}" for f in fams)
        print(f"| {r['candidate']} | {100 * r['all_mean']:.2f} | {cells} |")
    if js:
        json.dump({"per_workload": per, "rows": rows}, open(js, "w"), indent=1)


if __name__ == "__main__":
    main()
