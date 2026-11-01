"""Roll the Phase 1 profiles up into per-candidate time shares (for docs/IMPACT_ATLAS.md).

A candidate's share in a workload = the share of samples whose innermost Python frame is in
one of the candidate's stdlib modules (this includes native code those modules call), plus
the share of the candidate's native components/symbols under *other* owners (C entry points
called directly from user/third-party code). Candidates may overlap (json contains float
formatting); the table says so.

Usage: python3 rollup.py [--md]   (reads ~/rustique-research/profiles/*/*.json)
"""

import glob
import json
import os
import re
import statistics
import sys

PROFILES = os.path.expanduser("~/rustique-research/profiles")

# name: (owner modules, native components counted outside those owners, symbol regex counted
#        outside those owners and components)
CANDIDATES = {
    "json": (["json"], ["json-accel"], None),
    "float->text": ([], [], r"^(_Py_dg_dtoa|float_repr|PyOS_double_to_string|format_float_short|_PyFloat_FormatAdvancedWriter|format_float_internal|lshift|mult|multadd|pow5mult|Balloc|Bfree|quorem|i2b|diff|_Py_dg_freedtoa)$"),
    "copy (deepcopy/copy)": (["copy", "copyreg"], [], None),
    "datetime text (strptime/strftime/iso)": (["_strptime", "_pydatetime", "zoneinfo"], ["datetime", "time", "locale"], None),
    "re (module glue)": (["re"], [], None),
    "re (engine, all owners)": ([], ["re-engine"], None),
    "logging": (["logging"], [], None),
    "paths (posixpath/pathlib/fnmatch/glob/os/shutil)": (["posixpath", "genericpath", "pathlib", "fnmatch", "glob", "os", "shutil", "ntpath"], [], None),
    "email + http headers": (["email", "http"], [], None),
    "urllib.parse": (["urllib", "urllib.parse"], [], None),
    "uuid": (["uuid"], [], None),
    "enum": (["enum"], [], None),
    "dataclasses (asdict/replace, runtime)": (["dataclasses"], [], None),
    "xml.etree": (["xml.etree", "xml"], ["elementtree", "expat"], None),
    "csv": (["csv"], ["csv-accel"], None),
    "textwrap": (["textwrap"], [], None),
    "random": (["random"], [], None),
    "ipaddress": (["ipaddress"], [], None),
    "tomllib": (["tomllib"], [], None),
    "configparser": (["configparser"], [], None),
    "zlib/gzip/zipfile/tarfile": (["gzip", "zipfile", "tarfile", "zlib"], ["zlib"], None),
    "asyncio (loop/streams)": (["asyncio", "selectors"], ["asyncio-accel"], None),
    "contextlib": (["contextlib"], [], None),
    "typing/abc/inspect": (["typing", "abc", "_collections_abc", "inspect", "annotationlib"], ["isinstance/abc", "abc"], None),
    "collections (Counter/namedtuple/OrderedDict)": (["collections"], ["collections-accel"], None),
    "functools": (["functools"], ["functools-accel"], None),
    "tokenize/ast": (["tokenize", "ast", "token"], ["parser", "ast-objects", "compiler"], None),
    "statistics/fractions/decimal": (["statistics", "fractions", "decimal", "_pydecimal"], ["decimal"], None),
    "importlib (import system)": (["importlib", "importlib._bootstrap", "importlib._bootstrap_external", "<frozen>"], ["import"], None),
    "pickle": (["pickle"], ["pickle"], None),
    "threading/queue": (["threading", "queue"], ["locks", "thread"], None),
    "traceback/linecache": (["traceback", "linecache"], [], None),
    "html": (["html"], [], None),
    "base64/binascii": (["base64"], ["binascii"], None),
    "hashlib/hmac": (["hashlib", "hmac"], ["hashlib", "openssl"], None),
    "str methods (all owners)": ([], ["str"], None),
    "int (all owners)": ([], ["int"], None),
    "dict (all owners)": ([], ["dict"], None),
    "memory/gc (all owners)": ([], ["memory/gc"], None),
    "interpreter+calls+attr (all owners)": ([], ["interpreter", "calls", "attr/type"], None),
}


def owner_module(owner: str) -> str | None:
    return owner[len("stdlib:"):] if owner.startswith("stdlib:") else None


def shares(d: dict) -> dict[str, float]:
    out = {}
    for name, (mods, comps, symre) in CANDIDATES.items():
        mods_set = set(mods)
        s = sum(v for o, v in d["python_owner"].items()
                if (m := owner_module(o)) and (m in mods_set or m.split(".")[0] in mods_set))
        if comps:
            for cell in d.get("matrix", []):
                m = owner_module(cell["python_owner"]) or ""
                if cell["native_component"] in comps and not (m in mods_set or m.split(".")[0] in mods_set):
                    s += cell["share"]
            if not mods:  # component-only candidates: use the full component share
                s = sum(d["native_component"].get(c, 0.0) for c in comps)
        if symre:
            rx = re.compile(symre)
            s += sum(x["share"] for x in d.get("top_native", []) if rx.match(x["symbol"]))
        out[name] = s
    return out


def main() -> None:
    rows = []
    for f in sorted(glob.glob(f"{PROFILES}/*/*.json")):
        d = json.load(open(f))
        fam = os.path.basename(os.path.dirname(f))
        if "pandas" in os.path.basename(f):
            pass
        rows.append((fam, os.path.basename(f)[:-5], shares(d)))
    fams = sorted({r[0] for r in rows})
    md = "--md" in sys.argv
    hdr = ["candidate", "mean all", *[f"mean {f}" for f in fams], "max (workload)", "n≥5%"]
    print("| " + " | ".join(hdr) + " |")
    print("|" + "---|" * len(hdr))
    table = []
    for name in CANDIDATES:
        vals = [(fam, w, s[name]) for fam, w, s in rows]
        allm = statistics.mean(v for _, _, v in vals)
        fm = [statistics.mean(v for f2, _, v in vals if f2 == f) for f in fams]
        mx = max(vals, key=lambda x: x[2])
        n5 = sum(1 for _, _, v in vals if v >= 0.05)
        table.append((allm, name, fm, mx, n5))
    for allm, name, fm, mx, n5 in sorted(table, key=lambda t: -t[0]):
        print(f"| {name} | {allm:.1%} | " + " | ".join(f"{x:.1%}" for x in fm)
              + f" | {mx[2]:.0%} ({mx[0]}/{mx[1]}) | {n5} |")
    print(f"\nworkloads: {len(rows)} ({', '.join(f'{f}: {sum(1 for r in rows if r[0]==f)}' for f in fams)})")
    if "--per-workload" in sys.argv:
        for fam, w, s in rows:
            top = sorted(s.items(), key=lambda kv: -kv[1])[:6]
            print(f"{fam}/{w}: " + ", ".join(f"{k} {v:.0%}" for k, v in top if v >= 0.02))


if __name__ == "__main__":
    main()
