"""Callgrind cells of the `re` group (docs/targets/re.md): instructions per call, plain CPython
vs the group installed, each cell in its own process (region-instrumented, warmed; see
`_callgrind.py`).

- compile: `re._compiler.compile(P, F)` (the kernel's site, no cache) for small, medium and
  large patterns, IGNORECASE (ASCII and Unicode tables), big charsets, verbose patterns; the
  fallback paths (declined patterns: `\\N{...}`, LOCALE, warnings, errors) and `re.compile`
  misses through the real cache (`re.purge()` + `re.compile`).
- module functions: cache hits of every function (flags positional, keyword, `RegexFlag`),
  literal and template `sub`; the fallback paths (callable replacement, positional count,
  misses, str/bytes mismatch) both unsuspended (mixed with hits, 1 fallback per hit) and
  steady state (an all-fallback stream, suspended after 64 calls).

    python benches/targets/re_cells.py [--only SUBSTR] [-j 4] [-o out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import per_call  # noqa: E402

SETUP = r"""
import re, warnings
from re import _compiler
warnings.simplefilter("ignore")
SMALL = r"\d+"
MED = r"(?P<major>[0-9]+)\.(?P<minor>[0-9]+)"
URL = r"^(?:(?P<scheme>[a-zA-Z][a-zA-Z0-9+.-]*):)?(?://(?P<authority>[^/?#]*))?(?P<path>[^?#]*)(?:\?(?P<query>[^#]*))?(?:#(?P<fragment>.*))?$"
LARGE = r'''
    (?P<ws>\s+) | (?P<comment>\#[^\n]*) |
    (?P<number>-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?) |
    (?P<string>"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*') |
    (?P<name>[A-Za-z_][A-Za-z0-9_]*) | (?P<op>==|!=|<=|>=|\*\*|//|->|[-+*/%<>=(){}\[\],.:;@])
    | (?P<date>\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?)
'''
EMAIL = r"(?:[a-z0-9!#$%&'*+/=?^_`{|}~-]+(?:\.[a-z0-9!#$%&'*+/=?^_`{|}~-]+)*|\"(?:[\x01-\x08\x0b\x0c\x0e-\x1f\x21\x23-\x5b\x5d-\x7f]|\\[\x01-\x09\x0b\x0c\x0e-\x7f])*\")@(?:(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)"
UNI_I = r"(?i)[a-zà-ÿĀ-ſ]+\s+straße"
BIG = "[Ѐ-ӿԱ-Ֆא-תؠ-يぁ-ゖ一-丯]+"
ASCII_I = r"(?ai)content-type:\s*([\w/+.-]+)(?:;\s*charset=([\w-]+))?"
BYTES = rb"^(?P<method>[A-Z]+) (?P<path>\S+) HTTP/(?P<v>\d\.\d)\r\n"
S = "version 3.15.1 released"
LINE = "127.0.0.1 - - [02/Nov/2026:21:04:05 +0000] \"GET /api/v1/items?id=42 HTTP/1.1\" 200 512"
def cb(m):
    return "#"
def mismatch():
    try:
        re.match(SMALL, b"4")
    except TypeError:
        pass
class Flag(int):
    pass
IFLAG = Flag(2)
P = re.compile(r"(\d+)")
def bad():
    try:
        _compiler.compile("(a", 0)
    except re.error:
        pass
"""

COMPILE = {
    "compile small \\d+": "_compiler.compile(SMALL, 0)",
    "compile medium (version)": "_compiler.compile(MED, 0)",
    "compile URL (5 groups)": "_compiler.compile(URL, 0)",
    "compile large verbose tokenizer": "_compiler.compile(LARGE, 64)",
    "compile email (RFC 5322)": "_compiler.compile(EMAIL, 2)",
    "compile unicode IGNORECASE set": "_compiler.compile(UNI_I, 0)",
    "compile BIGCHARSET": "_compiler.compile(BIG, 0)",
    "compile ASCII IGNORECASE": "_compiler.compile(ASCII_I, 0)",
    "compile bytes": "_compiler.compile(BYTES, 0)",
    "compile empty": "_compiler.compile('', 0)",
    "re.compile miss (purge + compile medium)": "re.purge(); re.compile(MED)",
    "re.compile miss (purge + compile small)": "re.purge(); re.compile(SMALL)",
    # fallback paths of the compiler (declined)
    "compile \\N{...} [fallback]": "_compiler.compile(r'\\N{EM DASH}+', 0)",
    "compile bytes LOCALE [fallback]": "_compiler.compile(rb'\\w+', re.L)",
    "compile FutureWarning [fallback]": "_compiler.compile('[[a]', 0)",
    "compile error [fallback]": "bad()",
    "compile flags=RegexFlag [fallback]": "_compiler.compile(SMALL, re.I)",
    "compile str subclass flags [fallback]": "_compiler.compile(SMALL, IFLAG)",
    # unsuspended declines: each mixed with a hit on SMALL (the hit saves the "compile small"
    # difference; the decline's own overhead is the rest)
    "compile error + small [fallback+hit]": "bad(); _compiler.compile(SMALL, 0)",
    "compile FutureWarning + small [fallback+hit]": "_compiler.compile('[[a]', 0); _compiler.compile(SMALL, 0)",
    "compile \\N{...} + small [fallback+hit]": "_compiler.compile(r'\\N{EM DASH}+', 0); _compiler.compile(SMALL, 0)",
}

FUNCS = {
    "re.match hit": "re.match(SMALL, '42 x')",
    "re.match hit, flags=re.I": "re.match(SMALL, '42 x', re.I)",
    "re.match hit, flags=kw": "re.match(SMALL, '42 x', flags=0)",
    "re.match miss (no match)": "re.match(SMALL, 'x 42')",
    "re.fullmatch hit": "re.fullmatch(MED, '3.15')",
    "re.search hit (log line)": "re.search(URL, LINE)",
    "re.search hit (version)": "re.search(MED, S)",
    "re.findall hit": "re.findall(SMALL, LINE)",
    "re.finditer hit": "list(re.finditer(SMALL, S))",
    "re.split hit": "re.split(r'\\s+', S)",
    "re.split hit maxsplit=kw": "re.split(r'\\s+', S, maxsplit=1)",
    "re.sub literal hit": "re.sub(r'\\d', '#', S)",
    "re.sub template hit (\\1)": "re.sub(r'(\\d+)', r'<\\1>', S)",
    "re.sub template hit (\\g<name>)": "re.sub(MED, r'\\g<minor>.\\g<major>', S)",
    "re.subn literal hit": "re.subn(r'\\d', '#', S)",
    "re.sub count=kw hit": "re.sub(r'\\d', '#', S, count=1)",
    "re.compile hit": "re.compile(MED)",
    "re.compile hit flags": "re.compile(MED, re.I)",
    "re.match bytes hit": "re.match(BYTES, b'GET /x HTTP/1.1\\r\\n')",
    "re.match compiled pattern arg": "re.match(P, '42')",
    # fallbacks, unsuspended (one hit per fallback keeps the target installed)
    "re.sub callable [fallback+hit]": "re.sub(SMALL, cb, S); re.sub(SMALL, '#', S)",
    "re.sub positional count [fallback+hit]": "re.sub(SMALL, '#', S, 1); re.sub(SMALL, '#', S)",
    "re.match str/bytes mismatch [fallback+hit]": "mismatch(); re.match(SMALL, '4')",
    "re.match flags=int subclass [fallback+hit]": "re.match(SMALL, '4', IFLAG); re.match(SMALL, '4')",
    "re.search miss of the cache [fallback]": "re.purge(); re.search(SMALL, S)",
    # all-fallback streams (suspended after 64 calls: steady state is CPython's)
    "re.sub callable [fallback stream]": "re.sub(SMALL, cb, S)",
    "re.match flags=int subclass [fallback stream]": "re.match(SMALL, '4', IFLAG)",
}


def measure(name: str, stmt: str, n: int) -> tuple[str, dict[str, float]]:
    plain = per_call(stmt, SETUP, n=n)
    rq = per_call(stmt, SETUP, enable=["re"], n=n)
    return name, {"plain": plain, "rq": rq, "ratio": rq / plain}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--only")
    ap.add_argument("-j", type=int, default=4)
    ap.add_argument("-o")
    a = ap.parse_args()
    cells = {**{k: (v, 40) for k, v in COMPILE.items()}, **{k: (v, 200) for k, v in FUNCS.items()}}
    if a.only:
        cells = {k: v for k, v in cells.items() if a.only in k}
    with ThreadPoolExecutor(a.j) as ex:
        res = dict(ex.map(lambda kv: measure(kv[0], *kv[1]), cells.items()))
    for k in cells:
        r = res[k]
        print(f"{k:52s} plain={r['plain']:>10,.0f}  rq={r['rq']:>10,.0f}  ratio={r['ratio']:.3f}")
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "cells": res}, f, indent=1)


if __name__ == "__main__":
    main()
