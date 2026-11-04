"""The 84-row floor table of docs/atlas/w2-floors.md §1.

Inputs (all produced by benches/floors/*):
  - cells: cg_breakdown.py over floor_cells.py (plain / enabled Ir per call and components)
  - counters: Phase 3's one-run counters (benches/results/phase3/workloads/stats-run-3.15.1)
  - Ir per run: wl_instructions.sh (perf stat, enabled runs)

    python benches/floors/floor_table.py floors-3.15.1.json wl_instr.tsv > table.md

FLOOR below is the *inferred* irreducible cost of the enabled cell for the current site kind:
the call machinery CPython runs anyway (eval), the result objects (alloc), CPython helpers
whose work must happen (dict inserts, hashing, ...), plus a minimal kernel; the note says
what makes up the difference. Remaining = enabled - floor; its workload share = hits per run x
remaining / Ir per run (inferred: the workload's inputs differ from the cell's).
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
P3 = os.path.join(HERE, "..", "results", "phase3", "workloads", "stats-run-3.15.1")

# target -> (floor Ir, verdict, note). Verdict: settled / open.
FLOOR = {
    "list.sort": (850, "settled", "slice copy + list alloc (~450) + entry; scan+smallsort of 8 ints ~300 Ir; per-call gain ≤ 400 Ir"),
    "list.index": (330, "settled", "METH_FASTCALL entry ~220 + scan; ≤ 100 Ir left"),
    "list.count": (360, "settled", "entry ~200 + scan; ≤ 110 Ir left"),
    "tuple.index": (320, "settled", "as list.index"),
    "tuple.count": (300, "settled", "as list.count"),
    "list.__contains__": (300, "settled", "CONTAINS_OP ~110 + 8-int scan; ~25 Ir/item now, SIMD would save ≤ 100 Ir"),
    "tuple.__contains__": (200, "settled", "CONTAINS_OP ~110 + 4-int scan"),
    "builtins.sum": (380, "settled", "entry ~200 + 3 adds + int result"),
    "builtins.min": (330, "settled", "the 8-item scan runs inside the critical-section closure (210 Ir, ~25 Ir/item); SIMD on compact ints would leave ~100 Ir less; billing_engine's 230k calls make it 0.7%"),
    "builtins.max": (330, "settled", "as min"),
    "builtins.sorted": (1200, "settled", "list(x) copy + sort; as list.sort"),
    "builtins.any": (290, "settled", "entry ~250"),
    "builtins.all": (300, "settled", "entry ~250"),
    "float.repr": (700, "settled", "shortest digits (zmij ~150) + str alloc/free 170 + CALL_BUILTIN_O; at the floor"),
    "json.dumps": (2600, "open", "escape scan 1,728 Ir for ~80 chars (SIMD: ~200), buffer copy 763 (write into the final str), dict walk 429 stays"),
    "json.loads": (9000, "open", "allocations 3.9k + dict inserts 1.6k + key hashing 1.1k stay; parser 4.4k → ~1.5k (key cache across calls, SIMD string scan)"),
    "copy.deepcopy": (1700, "open", "PyDict_SetItem 1,273 + dictresize 222: insert into a presized dict / clone the keys table of a str-keyed dict (~500); memo bookkeeping"),
    "copy.copy": (1500, "open", "type lookups (_PyType_Lookup* 670 Ir) cacheable by type version tag; reduce emulation 600"),
    "dataclasses.asdict": (1900, "open", "field reads via GenericGetAttr 1,086 + fields() 284: per-class plan cached by type version"),
    "dataclasses.astuple": (1800, "open", "as asdict"),
    "datetime.datetime.strptime": (1500, "open", "generic format interpreter 2.6k Ir (first_match/parse/names): compile the format once per (format, locale) to a matcher"),
    "datetime.date.strptime": (1100, "open", "as datetime.strptime"),
    "datetime.time.strptime": (1100, "open", "as datetime.strptime"),
    "time.strptime": (2200, "open", "struct_time build (~700) stays; matcher as above"),
    "datetime.date.strftime": (800, "open", "directive interpretation 900 Ir: compiled format cache (format str → plan) would leave ~150"),
    "datetime.time.strftime": (750, "open", "as date.strftime"),
    "time.strftime": (1300, "settled", "struct_time unpacking ~400 + CALL_NON_PY_GENERAL; ≤ 650 Ir left, no workload calls"),
    "datetime.datetime.isoformat": (600, "settled", "eval 210 + str alloc 105 + digits; ≤ 130 Ir left"),
    "datetime.date.isoformat": (450, "settled", "at the floor"),
    "datetime.time.isoformat": (560, "settled", "≤ 100 Ir left"),
    "posixpath.join": (560, "open", "k_join 437 Ir for two short strs is ~4x a minimal join (~100): 410 Ir/call, 1% of fs_inventory (cheap fix)"),
    "posixpath.split": (900, "settled", "two str + tuple allocs (~500) + entry"),
    "posixpath.splitext": (880, "settled", "as split"),
    "posixpath.basename": (520, "settled", "entry + one str"),
    "posixpath.dirname": (520, "settled", "entry + one str"),
    "posixpath.isabs": (300, "settled", "= the vectorcall entry floor (same(x): 264 Ir)"),
    "posixpath.relpath": (3000, "open", "Rust heap traffic (malloc/free/realloc ~2.3k) + Vec<&[u32]> building; normpath on code-point vectors"),
    "posixpath.abspath": (1000, "open", "normpath over a Vec<u32> (855) + UCS4 round trip (320): work on the PEP 393 buffer in place"),
    "fnmatch.fnmatch": (450, "open", "thread-local pattern cache lookup 146 Ir → a per-pattern pointer cache ~30 (cheap fix, 1.3% of fs_inventory)"),
    "fnmatch.fnmatchcase": (450, "settled", "as fnmatch"),
    "fnmatch.filter": (2800, "open", "thread-local cache access 1,735 Ir per call(!) inside the loop: hoist it out of the loop"),
    "pathlib.PurePath.__new__": (1400, "settled", "object + list allocs (~760) + slot_tp_new/init path ~640"),
    "pathlib.Path.__new__": (1400, "settled", "as PurePath.__new__"),
    "pathlib.PurePath.__init__": (1400, "settled", "same cell as Path.__new__"),
    "pathlib.PurePath.__str__": (360, "settled", "cached str: entry only"),
    "pathlib.PurePath.__truediv__": (1250, "settled", "new path object + list allocs (834)"),
    "pathlib.PurePath.parent": (1700, "open", "format() 551 + list/obj allocs 1,251: build the parent's str directly"),
    "pathlib.PurePath.name": (330, "settled", "property entry ~260"),
    "pathlib.PurePath.suffix": (520, "settled", "entry + str"),
    "pathlib.PurePath.stem": (520, "settled", "entry + str"),
    "pathlib.PurePath.parts": (600, "settled", "tuple of parts"),
    "pathlib.PurePath.relative_to": (2600, "open", "allocations 2.1k (+473 libc) for one result object: parse once, build the result str directly"),
    "pathlib.PurePath.__eq__": (420, "settled", "slot_tp_richcompare + compare"),
    "pathlib.PurePath.__hash__": (560, "settled", "slot_tp_hash path"),
    "logging.Logger.level": (520, "settled", "disabled call: entry 177 + level cache; ~300 Ir left"),
    "logging.Logger.findCaller": (900, "open", "_PyCode_CheckLineNumber 3,172 Ir (79%): a (code, instr offset) → line cache leaves ~150"),
    "logging.LogRecord.__init__": (4700, "open", "20 PyObject_GenericSetAttr stores 3,065 Ir: direct inline-values stores (calibrated layout) ~600"),
    "logging.Formatter.formatTime": (1500, "settled", "localtime_r + digits; strftime_numeric 326; ≤ 1k Ir left"),
    "logging.Formatter.format": (5500, "open", "type lookups 900 (_PyType_LookupRef, _PyObject_GetMethod) cacheable per type version; percent render"),
    "random.randint": (520, "settled", "entry ~280 + MT word + int; ≤ 260 Ir left"),
    "random.randrange": (500, "settled", "as randint"),
    "random.choice": (420, "settled", "≤ 120 Ir left"),
    "random.shuffle": (4500, "settled", "100 swaps + MT draws ~45 Ir/item"),
    "random.sample": (1500, "open", "sample_indices 475 (set/selection bookkeeping) + Rust heap: ~800 Ir/call, 1.7% of report_render"),
    "random.choices": (1600, "open", "weights read + int→float conversions (__floattidf 180) + Rust heap: ~1k Ir/call, 2% of report_render"),
    "statistics.mean": (6000, "settled", "exact rational arithmetic in CPython ints (_PyLong_FromByteArray/x_divrem 7.6k); native 128-bit exact path would halve it, but statistics has 1 call per run in the set"),
    "statistics.variance": (15000, "settled", "as mean (bigint products/GCD 31k)"),
    "statistics.pvariance": (15000, "settled", "as variance"),
    "statistics.stdev": (25000, "settled", "as variance + exact sqrt rounding (x_divrem 20k)"),
    "statistics.pstdev": (10000, "settled", "as stdev"),
    "textwrap.wrap": (2500, "open", "Rust heap traffic + dict lookups for the wrapper's options"),
    "textwrap.fill": (26000, "open", "Rust heap traffic 36k Ir/call (malloc/free/realloc of word Vecs) + split 15k: arena/reused buffers, word spans into the source buffer"),
    "textwrap.shorten": (7000, "open", "as fill"),
    "textwrap.dedent": (8000, "open", "Vec<u32> building 6.1k + memset 2.9k + UCS4 round trip 5k"),
    "textwrap.indent": (5000, "open", "splitlines into owned Vecs + UCS4 round trip"),
    "textwrap.TextWrapper.wrap": (55000, "open", "as fill (540 chars)"),
    "textwrap.TextWrapper.fill": (26000, "open", "as fill"),
    "urllib.parse.quote": (1400, "open", "Rust heap traffic ~900 + input conversion 388"),
    "urllib.parse.quote_plus": (1000, "settled", "≤ 800 Ir left"),
    "urllib.parse.unquote": (2000, "open", "Rust heap ~700 + UCS4 conversion"),
    "urllib.parse.unquote_plus": (900, "settled", "≤ 500 Ir left"),
    "urllib.parse.parse_qsl": (7500, "open", "Rust heap ~3.8k (malloc/calloc/free) + memcmp; tuples/strs stay"),
    "urllib.parse.parse_qs": (8500, "open", "as parse_qsl"),
    "urllib.parse._urlsplit": (3300, "open", "Rust heap ~1.8k + Vec<u32> conversion 759: slice the PEP 393 buffer"),
}


def main():
    cells = json.load(open(sys.argv[1]))
    ir_run = {}
    for line in open(sys.argv[2]):
        wl, v, _a, _b, per = line.split("\t")
        # aiohttp_app / import_startup / pandas_etl: per-run instructions not stable
        # (event-loop polling, child processes, numpy threads): left out of the shares.
        if v == "on" and wl not in ("web/aiohttp_app", "devtools/import_startup", "data/pandas_etl"):
            ir_run[wl] = int(per)
    hits = {}
    for f in os.listdir(P3):
        d = json.load(open(os.path.join(P3, f)))
        wl = f[:-5].replace("__", "/")
        for t, s in d["stats"].items():
            if s["hits"]:
                hits.setdefault(t, {})[wl] = s["hits"]
    sys.path.insert(0, HERE)
    from floor_cells import CELLS
    rows = []
    print("| target | cell | plain | enabled | ratio | floor (inf.) | left (Ir, % of enabled) | hits/run, top workload | left × hits, % of run | verdict: what moves it |")
    print("|---|---|---:|---:|---:|---:|---:|---|---:|---|")
    for t, (groups, _s, stmt, what) in CELLS.items():
        if t.startswith("_"):
            continue
        p = cells[f"{t} [plain]"]["total"]
        e = cells[f"{t} [on]"]["total"]
        fl, verdict, note = FLOOR[t]
        fl = min(fl, e)
        left = e - fl
        h = hits.get(t, {})
        best = ""
        share = 0.0
        if h:
            h = {k: v for k, v in h.items() if k in ir_run} or h
            wl, n = max(h.items(), key=lambda kv: kv[1] * left / max(ir_run.get(kv[0], 1), 1))
            share = 100 * n * left / ir_run[wl] if wl in ir_run else 0.0
            best = f"{n:,} {wl.split('/')[1]}"
        rows.append((t, share))
        cell = f"`{stmt}`" + (f" ({what})" if what else "")
        print(f"| `{t}` | {cell} | {p:,.0f} | {e:,.0f} | {e / p:.3f} | {fl:,} | {left:,.0f} ({100 * left / e:.0f}%) | {best or '0'} | {share:.2f}% | **{verdict}**: {note} |")
    print()
    print("Largest remaining shares:", ", ".join(f"{t} {s:.2f}%" for t, s in sorted(rows, key=lambda r: -r[1])[:12]))


if __name__ == "__main__":
    main()
