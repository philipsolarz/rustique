#!/usr/bin/env python3
"""Static reach of CPython-provided machinery across the top-1000 PyPI packages.

    static_reach.py [SCAN_DIR] [--json out.json]

Reads the wave-1 AST scan (`~/rustique-research/static/results/scan/NNNN_<pkg>.json`, method:
docs/atlas/static-usage.md §1). Each package file has library-code counters such as
`ref:dataclasses.dataclass|deco|from` (module.name | context call/deco/base/ref | binding
mode attr/from/obj). For each feature (a set of counter-key prefixes) this prints:
reach = download-weighted share of the 937 packages with parsed library code that use it at
least once, the package count, and the site count.
"""

import glob
import json
import os
import sys

FEATURES = {
    # generated code
    "dataclasses: @dataclass": ["ref:dataclasses.dataclass|"],
    "dataclasses: fields()/replace()/asdict()": ["ref:dataclasses.fields|call", "ref:dataclasses.replace|call",
                                                 "ref:dataclasses.asdict|call"],
    "collections.namedtuple": ["ref:collections.namedtuple|call"],
    "typing.NamedTuple (base)": ["ref:typing.NamedTuple|base", "ref:typing_extensions.NamedTuple|base"],
    "typing.TypedDict (base)": ["ref:typing.TypedDict|base"],
    "functools.total_ordering": ["ref:functools.total_ordering|"],
    # enum
    "enum: any Enum base": ["ref:enum.Enum|base", "ref:enum.IntEnum|base", "ref:enum.StrEnum|base",
                            "ref:enum.Flag|base", "ref:enum.IntFlag|base"],
    "enum.Enum (base)": ["ref:enum.Enum|base"],
    "enum.IntEnum/StrEnum (base)": ["ref:enum.IntEnum|base", "ref:enum.StrEnum|base"],
    "enum.Flag/IntFlag (base)": ["ref:enum.Flag|base", "ref:enum.IntFlag|base"],
    # typing at run time
    "typing.cast": ["ref:typing.cast|call"],
    "typing.runtime_checkable": ["ref:typing.runtime_checkable|", "ref:typing_extensions.runtime_checkable|"],
    "typing.Protocol (base)": ["ref:typing.Protocol|base", "ref:typing_extensions.Protocol|base"],
    "typing.NewType": ["ref:typing.NewType|call"],
    "typing.get_type_hints": ["ref:typing.get_type_hints|call"],
    "typing.Generic (base)": ["ref:typing.Generic|base"],
    # functools / contextlib
    "functools.cached_property": ["ref:functools.cached_property|"],
    "functools.singledispatch(method)": ["ref:functools.singledispatch|", "ref:functools.singledispatchmethod|"],
    "functools.wraps": ["ref:functools.wraps|"],
    "functools.lru_cache/cache": ["ref:functools.lru_cache|", "ref:functools.cache|"],
    "functools.partial": ["ref:functools.partial|call"],
    "contextlib.contextmanager": ["ref:contextlib.contextmanager|"],
    "contextlib.asynccontextmanager": ["ref:contextlib.asynccontextmanager|"],
    "contextlib.suppress": ["ref:contextlib.suppress|call"],
    # inspect / weakref
    "inspect.signature": ["ref:inspect.signature|call"],
    "inspect.isclass/isfunction/...": ["ref:inspect.isclass|call", "ref:inspect.isfunction|call",
                                       "ref:inspect.iscoroutinefunction|call", "ref:inspect.ismethod|call"],
    "inspect.getmembers": ["ref:inspect.getmembers|call"],
    "weakref.WeakValueDictionary": ["ref:weakref.WeakValueDictionary|call"],
    "weakref.WeakKeyDictionary": ["ref:weakref.WeakKeyDictionary|call"],
    "weakref.WeakSet": ["ref:weakref.WeakSet|call"],
    "weakref.finalize": ["ref:weakref.finalize|call"],
    "weakref.ref/proxy": ["ref:weakref.ref|call", "ref:weakref.proxy|call"],
    # re module functions
    "re.match/search/fullmatch": ["ref:re.match|call", "ref:re.search|call", "ref:re.fullmatch|call"],
    "re.sub/subn": ["ref:re.sub|call", "ref:re.subn|call"],
    "re.split/findall/finditer": ["ref:re.split|call", "ref:re.findall|call", "ref:re.finditer|call"],
    "re.escape": ["ref:re.escape|call"],
    "re.* module functions (any)": ["ref:re.match|call", "ref:re.search|call", "ref:re.fullmatch|call",
                                    "ref:re.sub|call", "ref:re.subn|call", "ref:re.split|call",
                                    "ref:re.findall|call", "ref:re.finditer|call"],
    # text / misc stdlib
    "string.Template": ["ref:string.Template|"],
    "html.escape": ["ref:html.escape|call"],
    "uuid (any)": ["ref:uuid."],
    "ipaddress (any)": ["ref:ipaddress."],
    "os.environ / getenv": ["ref:os.environ", "ref:os.getenv|call"],
    "threading.Event/Condition": ["ref:threading.Event|call", "ref:threading.Condition|call"],
    "queue.Queue": ["ref:queue.Queue|call", "ref:queue.SimpleQueue|call"],
    "traceback.format_*": ["ref:traceback.format_exc|call", "ref:traceback.format_exception|call",
                           "ref:traceback.print_exc|call"],
    "warnings.warn": ["ref:warnings.warn|call"],
    "gettext": ["ref:gettext."],
    "http.cookies": ["ref:http.cookies."],
    "email.utils": ["ref:email.utils."],
    "base64 (any)": ["ref:base64."],
    "hmac (any)": ["ref:hmac."],
    "shlex (any)": ["ref:shlex."],
    "argparse.ArgumentParser": ["ref:argparse.ArgumentParser|call"],
    "http.client (any)": ["ref:http.client."],
    "email.parser/message_from_*": ["ref:email.parser.", "ref:email.message_from_"],
    "xml.etree (any)": ["ref:xml.etree."],
    "tokenize (any)": ["ref:tokenize."],
    "csv.DictWriter/DictReader": ["ref:csv.DictWriter|call", "ref:csv.DictReader|call"],
    "csv.writer/reader": ["ref:csv.writer|call", "ref:csv.reader|call"],
    "copy.copy": ["ref:copy.copy|call"],
    # language-level operations (builtins)
    "getattr()": ["bcall:getattr"],
    "hasattr()": ["bcall:hasattr"],
    "setattr()": ["bcall:setattr"],
    "isinstance()": ["bcall:isinstance"],
    "vars()": ["bcall:vars"],
    "str.format (literal receiver)": ["methlit:format|"],
    "% on str literal": ["fmt:percent_str"],
    "f-strings": ["fmt:fstring"],
}


def main():
    argv = sys.argv[1:]
    out = None
    if "--json" in argv:
        i = argv.index("--json")
        out = argv[i + 1]
        del argv[i:i + 2]
    scan = argv[0] if argv else os.path.expanduser("~/rustique-research/static/results/scan")
    pk = []
    for p in sorted(glob.glob(os.path.join(scan, "[0-9]*.json"))):
        d = json.load(open(p))
        lib = d["cats"].get("lib")
        if lib:
            pk.append((d["meta"]["name"], d["meta"]["downloads"], lib))
    tot = sum(dl for _, dl, _ in pk)
    rows = []
    for feat, prefs in FEATURES.items():
        npk = sites = dl = 0
        top = []
        for name, d, lib in pk:
            s = sum(v for k, v in lib.items() if k.startswith(tuple(prefs)))
            if s:
                npk += 1
                sites += s
                dl += d
                top.append((s, name))
        top.sort(reverse=True)
        rows.append({"feature": feat, "reach": round(dl / tot, 3), "packages": npk, "sites": sites,
                     "top": [n for _, n in top[:4]]})
    print(f"{len(pk)} packages with library code, {tot / 1e9:.1f} G downloads / 30 days")
    print(f"{'feature':44} {'reach':>6} {'pkgs':>5} {'sites':>7}  top")
    for r in rows:
        print(f"{r['feature']:44} {r['reach']:6.3f} {r['packages']:5d} {r['sites']:7d}  {', '.join(r['top'])}")
    if out:
        json.dump(rows, open(out, "w"), indent=1)


if __name__ == "__main__":
    main()
