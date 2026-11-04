"""The `re` patterns real programs compile at import, as a corpus for a native `re` compiler
(docs/atlas/w2-startup.md §3).

Input: the marshal tables written by `upper_bounds.py --exp re` in capture mode (key
(is_str, pattern, flags) -> the exact `_sre.compile` arguments). Output: corpus size, the
flags and syntax features in use (what a native compiler must cover, and what it would leave
to CPython's compiler: patterns that warn, LOCALE, DEBUG), and the cost of
`re._compiler.compile` per pattern (median of --runs, ns) with the heaviest patterns.

    python benches/startup/re_corpus.py TABLE.marshal [TABLE ...] [-o corpus.json] [--dump patterns.marshal]
"""

from __future__ import annotations

import argparse
import json
import marshal
import re
import statistics
import time
import warnings
from collections import Counter
from re import _compiler, _constants as C, _parser


def features(p, flags) -> set[str]:
    out = set()
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        try:
            tree = _parser.parse(p, flags)
        except re.error:
            return {"error"}
        if w:
            out.add("warns")
    fl = tree.state.flags
    for name, bit in (("I", C.SRE_FLAG_IGNORECASE), ("M", C.SRE_FLAG_MULTILINE), ("S", C.SRE_FLAG_DOTALL),
                      ("X", C.SRE_FLAG_VERBOSE), ("A", C.SRE_FLAG_ASCII), ("L", C.SRE_FLAG_LOCALE),
                      ("U", C.SRE_FLAG_UNICODE), ("DEBUG", C.SRE_FLAG_DEBUG)):
        if fl & bit:
            out.add("flag:" + name)
    if fl & C.SRE_FLAG_IGNORECASE and not fl & C.SRE_FLAG_ASCII and isinstance(p, str):
        out.add("unicode-ignorecase")

    def walk(sub):
        for op, av in sub:
            out.add("op:" + str(op))
            if op in (C.SUBPATTERN,):
                walk(av[-1])
            elif op in (C.BRANCH,):
                for s in av[1]:
                    walk(s)
            elif op in (C.MAX_REPEAT, C.MIN_REPEAT, C.POSSESSIVE_REPEAT):
                walk(av[2])
            elif op in (C.ASSERT, C.ASSERT_NOT):
                walk(av[1])
            elif op is C.ATOMIC_GROUP:
                walk(av)
            elif op is C.GROUPREF_EXISTS:
                walk(av[1])
                if av[2]:
                    walk(av[2])
    walk(tree)
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("tables", nargs="+")
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("-o", "--output")
    ap.add_argument("--dump", help="write the deduplicated corpus {(is_str, pattern, flags): args}")
    args = ap.parse_args()
    corpus = {}
    for t in args.tables:
        try:
            with open(t, "rb") as f:
                tab = marshal.load(f)
        except (EOFError, ValueError):  # an empty table (a program that compiled nothing)
            continue
        # dcexec tables share the directory: keep (is_str, pattern, flags) keys only
        corpus.update({k: v for k, v in tab.items() if len(k) == 3 and isinstance(k[0], bool)})
    feats = Counter()
    costs = []
    for (is_str, p, flags), _a in corpus.items():
        fs = features(p, flags)
        feats.update(fs)
        ts = []
        for _ in range(args.runs):
            t0 = time.perf_counter_ns()
            _compiler.compile(p, flags)
            ts.append(time.perf_counter_ns() - t0)
        costs.append((statistics.median(ts), p, flags))
    costs.sort(key=lambda x: -x[0])
    total = sum(c for c, _p, _f in costs)
    res = {
        "patterns": len(corpus),
        "bytes_patterns": sum(1 for k in corpus if not k[0]),
        "total_compile_ms": total / 1e6,
        "median_us": statistics.median(c for c, _p, _f in costs) / 1e3 if costs else 0,
        "top10_share": sum(c for c, _p, _f in costs[:10]) / total if total else 0,
        "features": dict(feats.most_common()),
        "heaviest": [[round(c / 1e3, 1), (p[:120] if isinstance(p, str) else repr(p[:120])), f]
                     for c, p, f in costs[:15]],
    }
    print(json.dumps({k: v for k, v in res.items() if k != "heaviest"}, indent=1))
    for c, p, f in res["heaviest"]:
        print(f"{c:9.1f} us  flags={f}  {p!r}")
    if args.output:
        with open(args.output, "w") as f:
            json.dump(res, f, indent=1)
    if args.dump:
        with open(args.dump, "wb") as f:
            marshal.dump(corpus, f)


if __name__ == "__main__":
    main()
