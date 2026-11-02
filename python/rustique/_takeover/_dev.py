"""Developer tools for target authors (not imported by ``rustique``).

    python -m rustique._takeover._dev fingerprint posixpath.join str.split json.encoder.JSONEncoder.default
        Print the fingerprint of each original (Python function: canonical code hash; C
        function / method descriptor: ml_name + ml_flags + ml_doc) as a Rust literal to
        paste into `Prepare::fingerprint_code` / `fingerprint_c`:
            (3, 15, "1f0e9d...") // posixpath.join

    python -m rustique._takeover._dev report [--only json,copy] [--import json,copy]
        Run every selected target's `prepare` in record mode (nothing is installed) and
        print each fingerprint it checks: observed on this interpreter, and the recorded
        value for this version (or "missing"). `--import` imports modules first (module
        targets need their module).

    python -m rustique._takeover._dev status [--only ...]
        Enable (optionally a subset) and print each target's state, reason and sites.

Run it once per validated version (3.14, 3.15; free-threaded builds give the same
fingerprints) and record the values per `(major, minor)`.
"""

from __future__ import annotations

import argparse
import importlib
import sys

from rustique import _rustique

A = _rustique._accel


def resolve(dotted: str) -> object:
    """`a.b.C.meth` -> the object, importing the longest importable module prefix."""
    parts = dotted.split(".")
    for i in range(len(parts), 0, -1):
        try:
            obj = importlib.import_module(".".join(parts[:i]))
        except ImportError:
            continue
        for p in parts[i:]:
            # Read class attributes raw (a method in a class dict, not a bound method).
            d = getattr(obj, "__dict__", {})
            obj = d[p] if isinstance(obj, type) and p in d else getattr(obj, p)
        return obj
    try:
        import builtins

        obj = builtins
        for p in parts:
            obj = getattr(obj, p)
        return obj
    except AttributeError:
        raise SystemExit(f"cannot resolve {dotted}") from None


def main(argv: list[str] | None = None) -> None:
    ap = argparse.ArgumentParser(prog="python -m rustique._takeover._dev", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    f = sub.add_parser("fingerprint")
    f.add_argument("names", nargs="+")
    r = sub.add_parser("report")
    r.add_argument("--only")
    r.add_argument("--import", dest="imports")
    s = sub.add_parser("status")
    s.add_argument("--only")
    a = ap.parse_args(argv)
    major, minor = sys.version_info[:2]
    if a.cmd == "fingerprint":
        for n in a.names:
            print(f'({major}, {minor}, "{A.fingerprint(resolve(n))}"), // {n}')
    elif a.cmd == "report":
        for m in (a.imports or "").split(","):
            if m.strip():
                importlib.import_module(m.strip())
        only = a.only.split(",") if a.only else None
        for target, what, observed, expected in A.fingerprint_report(only):
            if not observed:
                print(f"{target}: {what}")
                continue
            mark = "ok" if observed == expected else ("MISSING" if expected is None else f"DIFFERS (recorded {expected})")
            print(f'{target}: {what}: ({major}, {minor}, "{observed}") {mark}')
    else:
        from rustique import _takeover

        st = _takeover.enable(only=a.only.split(",") if a.only else None)
        for t in st["targets"]:
            print(f"{t['name']:32s} {t['state']:15s} {t['refusal'] or '':12s} {t['reason'] or ''}")
            for site in t["sites"]:
                print(f"    {site}")


if __name__ == "__main__":
    main()
