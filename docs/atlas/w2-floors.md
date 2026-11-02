STATUS: wave 2, Phase 1, slice `w2-floors` (branch `w2/p1-floors`), 2026-11-02. Evidence, not product:
the prototype kernels on this branch exist to measure (Phase 2 rebuilds what is chosen).
All numbers are from the shared dev box (c7a.8xlarge). Tags: **(M)** measured, with how and where;
**(I)** inferred, with what from. Unless stated otherwise: CPython 3.15.1, dev-release build, callgrind
instructions per call (region-instrumented, fresh process, `Ir(2n) − Ir(n)`), and Phase-3 counters
of one steady-state `run()` (`benches/results/phase3/workloads/stats-run-3.15.1`).

# w2-floors: floors, coverage gaps and settled marks of the 84 wave-1 targets

## Summary

1. **Floors.** 48 of the 84 targets are **settled**: at or near their floor, or with no share in real
   workloads. 36 are **open**. In almost every open target, the remaining cost is in one of three
   places (M, per-function callgrind breakdown of every target's headline cell):
   - Rust heap traffic (`malloc`/`free`/`realloc` of `Vec`s): textwrap 45%, urllib 15–30%, relpath ~40%.
   - Format interpreters that re-read the format string on every call: strptime 2.6k Ir, strftime 0.9k Ir.
   - CPython work that a cache can skip: `findCaller`'s line-number decode 79%, type lookups in
     logging/copy/asdict.

   The largest remaining shares of a real run (I, cell remainder × hits per run ÷ instructions per run):

   | target | remainder | share of run | workload |
   |---|---:|---:|---|
   | `textwrap.fill` | 54k Ir/call | 11.3% | `report_render` |
   | `json.dumps` | 4k Ir/call | 5.0% | `csv_etl` |
   | `date.strftime` | 1.1k Ir/call | 3.7% | `tz_events` |
   | `posixpath.relpath` | | 3.2% | `fs_inventory` |
   | `Formatter.format` | | 3.1% | `logging_service` |
   | `deepcopy` | | 2.9% | |
   | `strptime` | | 2.7% | |
   | `findCaller` | | 2.4% | |

2. **Coverage gaps.** Ranked by what they cost real runs. The counters' biggest fallback counts are not
   the biggest costs.

   | # | gap | share | evidence |
   |---|---|---:|---|
   | 1 | `json.dumps` with `default=`/`cls=JSONEncoder` | logging_service **−18.2%**, flask_jinja −1.1% | M, perf, prototype ("lazy default"); 0 digest changes |
   | 2 | `uuid.UUID` met by `deepcopy`/`asdict` | json_api **~11%** | I: asdict with UUID 138.7k Ir/call vs 50.2k without, 5,173 calls/run |
   | 3 | `default=` hooks that are actually called (the `make_encoder` route) | json_api ~4% | I |
   | 4 | the 67,863 class-needle fallbacks/run of pyflakes | 0.26% | M, identity-scan prototype: 0.33–0.54 per call, fallbacks ≤ 1.05, converts 100% |

   `sorted` with `key=` (≤ 0.1%), `sum`/`min`/`max` over generators (no gain possible) and the
   sqlglot/pydantic `deepcopy` restarts (user `__deepcopy__`, ruling 15) are settled.
3. **Credit system: keep it, tune it.** 24 suspensions in the Phase-3 whole-process records (M).
   - Re-run with suspension off: 23 were right; 1 was wrong (`PurePath.__new__` in pytest_networkx,
     suspended by a burst at startup, ~0.1%).
   - The first-64/320-call exceedances cost ≤ 112k Ir per target per process (I).
   - Proposals:
     - cost-weighted credits from each target's measured saving per hit and penalty per fallback;
     - a 256-call decision window;
     - separate "continue" (no waste) from "restart" (waste) in `deopt`;
     - show `suspensions` in reports.
4. **Install costs** (forked analysis, M, release build): every group's first enable costs
   114–493k Ir, against a 0.1M budget.
   - Fingerprints are 20–30% of it (fnmatch 50%); registry bookkeeping is 23–39%.
   - 39% of all install work goes to groups a workload never calls.
   - Fix: **validate on first call**, one mechanism shared with w2-startup's deferral, plus
     fingerprint v3 and allocation hygiene (§4).
5. **Patch releases are a silent cliff** (M; sent to the lead first).
   - 3.15.2 and 3.14.10 keep every target; conformance shows 0 differences in 27 modules on 3.15.2,
     3.15.2t and 3.14.10.
   - Older patches lose many: 3.15.0 installs 43/84, 3.14.0 47/84, 3.14.1–3.14.5 50/84, 3.14.6 72/84.
   - Causes, all behaviour-neutral: docstrings, an exception-table range fix in the compiler, and C
     `ml_doc` tweaks.
   - Fix: fingerprint v3 with a semantics-preserving canonical exception table (proof in §5.3), a
     set of accepted hashes per minor, and a recording CI job (§5).
6. **Never-slower violation in the shipped `list.sort`** (from w2-dx; confirmed with callgrind):
   - str lists of 2–5 elements: 1.11–1.17;
   - presorted str: up to 1.25; two-run str: 1.67;
   - descending int/float n=20: 1.23–1.32.
   Cause in §6. The prototype fix brings **all 90 sort cells to ≤ 1.05** (worst 1.049), keeps the
   random-input wins (str n=1000 0.42), and passes 5,000 identity-level differential cases.

## 0. Method and data

| what | how | where |
|---|---|---|
| per-call cost and its components | `benches/floors/cg_breakdown.py`: callgrind per-function exclusive Ir of one cell, split into **eval** (interpreter, call machinery), **alloc** (object allocation/deallocation), **cpython** (other interpreter helpers), **kernel** (the Rust extension), **libc** (`memcpy`, and `malloc`/`free` from Rust `Vec`s) | 173 cells (84 targets plain and enabled, plus entry floors), `benches/floors/floor_cells.py`; results in `benches/floors/results/floors-3.15.1.{json,txt}` |
| entry floors | `_selftest.same` (an exact identity kernel on a vectorcall site): **264 Ir** for `same(x)`, of which 210 are CALL plus caller bytecodes. A plain METH_O C call (`operator.index(5)`): **180 Ir** | same file |
| instructions per workload `run()` | `perf stat -e instructions:u` over `run.py one <wl> -n 1` and `-n 4`, `(I4 − I1)/3`, enabled and plain | `benches/floors/results/wl_instr.tsv` |
| noise of the per-run counts | same-config repeats differ by ±0.6–1.4%. aiohttp_app, import_startup and pandas_etl are unstable and are left out of the shares | §3 (mini_interp) |
| fallback causes | prototype trace (`RUSTIQUE_PROTO_TRACE=1`): needle, key, spec and root types at every `args`/`deopt` site of the containers, builtins, float, copy and asdict kernels, over one steady-state `run()` of all 34 workloads | `~/rustique-research/w2/floors/res/proto-trace-all/` |
| suspension decisions | `RUSTIQUE_PROTO_NO_SUSPEND=1` (score never suspends), whole-process and one-run counters of the 12 workloads that suspended something in Phase 3 | `res/nosusp-{run,whole}/` |

"Share of a run" = calls per run × Ir saved per call ÷ Ir per enabled run (I). The cell's input stands
in for the workload's inputs.

## 1. Floors of the 84 targets

How to read the table:
- **plain** and **enabled** are measured Ir per call (M, callgrind, 3.15.1).
- **floor** is inferred (I) from the enabled cell's components. It counts the call machinery CPython
  runs anyway, the result objects, the CPython helpers whose work must happen (dict inserts, hashing),
  and a minimal kernel.
- **left** = enabled − floor.
- **hits/run** is the Phase-3 counter (M); the share uses `wl_instr.tsv` (M).
- Reproduce:
  `python benches/floors/floor_table.py benches/floors/results/floors-3.15.1.json benches/floors/results/wl_instr.tsv`.

| target | cell | plain | enabled | ratio | floor (inf.) | left (Ir, % of enabled) | hits/run, top workload | left × hits, % of run | verdict: what moves it |
|---|---|---:|---:|---:|---:|---:|---|---:|---|
| `list.sort` | `ints[:].sort()` (8 ints (copy included)) | 1,931 | 1,264 | 0.655 | 850 | 414 (33%) | 73,832 xml_feeds | 0.57% | **settled**: slice copy + list alloc (~450) + entry; scan+smallsort of 8 ints ~300 Ir; per-call gain ≤ 400 Ir |
| `list.index` | `ints.index(7)` (8 ints) | 778 | 431 | 0.554 | 330 | 101 (23%) | 3,658 pylint_stdlib | 0.01% | **settled**: METH_FASTCALL entry ~220 + scan; ≤ 100 Ir left |
| `list.count` | `ints.count(3)` (8 ints) | 978 | 472 | 0.483 | 360 | 112 (24%) | 40,000 report_render | 0.24% | **settled**: entry ~200 + scan; ≤ 110 Ir left |
| `tuple.index` | `t.index(1)` (4 ints) | 679 | 405 | 0.597 | 320 | 85 (21%) | 240 email_mime | 0.00% | **settled**: as list.index |
| `tuple.count` | `t.count(9)` (4 ints) | 636 | 382 | 0.601 | 300 | 82 (21%) | 0 | 0.00% | **settled**: as list.count |
| `list.__contains__` | `4 in ints` (miss, 8 ints) | 1,142 | 409 | 0.358 | 300 | 109 (27%) | 655,813 pycodestyle_stdlib | 0.59% | **settled**: CONTAINS_OP ~110 + 8-int scan; ~25 Ir/item now, SIMD would save ≤ 100 Ir |
| `tuple.__contains__` | `9 in t` (4 ints) | 380 | 243 | 0.640 | 200 | 43 (18%) | 226,383 isort_stdlib | 0.44% | **settled**: CONTAINS_OP ~110 + 4-int scan |
| `builtins.sum` | `sum(small)` (3 ints) | 659 | 432 | 0.656 | 380 | 52 (12%) | 1,040 backup_archive | 0.00% | **settled**: entry ~200 + 3 adds + int result |
| `builtins.min` | `min(ints)` (8 ints) | 1,762 | 529 | 0.300 | 330 | 199 (38%) | 230,327 billing_engine | 0.67% | **settled**: the 8-item scan runs inside the critical-section closure (210 Ir, ~25 Ir/item); SIMD on compact ints would leave ~100 Ir less; billing_engine's 230k calls make it 0.7% |
| `builtins.max` | `max(ints)` (8 ints) | 1,791 | 529 | 0.295 | 330 | 199 (38%) | 87,192 tz_events | 0.49% | **settled**: as min |
| `builtins.sorted` | `sorted(ints)` (8 ints (sorted -> list.sort kernel)) | 2,273 | 1,633 | 0.718 | 1,200 | 433 (27%) | 0 | 0.00% | **settled**: list(x) copy + sort; as list.sort |
| `builtins.any` | `any(small)` (3 ints) | 443 | 324 | 0.731 | 290 | 34 (11%) | 6,435 pip_resolve | 0.01% | **settled**: entry ~250 |
| `builtins.all` | `all(strs)` (4 strs) | 708 | 373 | 0.527 | 300 | 73 (20%) | 12,680 pip_resolve | 0.02% | **settled**: entry ~250 |
| `float.repr` | `repr(x)` (repr(1.23)) | 2,031 | 758 | 0.373 | 700 | 58 (8%) | 2,536 flask_jinja | 0.01% | **settled**: shortest digits (zmij ~150) + str alloc/free 170 + CALL_BUILTIN_O; at the floor |
| `json.dumps` | `dumps(record)` (10-field record) | 31,080 | 6,557 | 0.211 | 2,600 | 3,957 (60%) | 32,690 csv_etl | 4.96% | **open**: escape scan 1,728 Ir for ~80 chars (SIMD: ~200), buffer copy 763 (write into the final str), dict walk 429 stays |
| `json.loads` | `loads(rec_text)` (10-field record) | 27,799 | 13,275 | 0.478 | 9,000 | 4,275 (32%) | 5,974 asyncio_http | 2.02% | **open**: allocations 3.9k + dict inserts 1.6k + key hashing 1.1k stay; parser 4.4k → ~1.5k (key cache across calls, SIMD string scan) |
| `copy.deepcopy` | `deepcopy(rec)` (10-field dict) | 24,220 | 4,151 | 0.171 | 1,700 | 2,451 (59%) | 26,265 isort_stdlib | 2.89% | **open**: PyDict_SetItem 1,273 + dictresize 222: insert into a presized dict / clone the keys table of a str-keyed dict (~500); memo bookkeeping |
| `copy.copy` | `copy.copy(p)` (small dataclass) | 12,643 | 4,710 | 0.373 | 1,500 | 3,210 (68%) | 1,615 fastapi_pydantic | 0.36% | **open**: type lookups (_PyType_Lookup* 670 Ir) cacheable by type version tag; reduce emulation 600 |
| `dataclasses.asdict` | `dataclasses.asdict(p)` (3-field dataclass) | 12,622 | 5,017 | 0.398 | 1,900 | 3,117 (62%) | 20 state_snapshot | 0.00% | **open**: field reads via GenericGetAttr 1,086 + fields() 284: per-class plan cached by type version |
| `dataclasses.astuple` | `dataclasses.astuple(p)` (3-field dataclass) | 12,489 | 4,529 | 0.363 | 1,800 | 2,729 (60%) | 0 | 0.00% | **open**: as asdict |
| `datetime.datetime.strptime` | `datetime.strptime(LOG, LOGF)` (access-log format) | 62,703 | 4,896 | 0.078 | 1,500 | 3,396 (69%) | 45,000 log_analytics | 2.67% | **open**: generic format interpreter 2.6k Ir (first_match/parse/names): compile the format once per (format, locale) to a matcher |
| `datetime.date.strptime` | `date.strptime('2024-03-15', '%Y-%m-%d')` (ISO date) | 31,495 | 2,354 | 0.075 | 1,100 | 1,254 (53%) | 0 | 0.00% | **open**: as datetime.strptime |
| `datetime.time.strptime` | `D.time.strptime('13:45:30', '%H:%M:%S')` (H:M:S) | 34,940 | 2,321 | 0.066 | 1,100 | 1,221 (53%) | 0 | 0.00% | **open**: as datetime.strptime |
| `time.strptime` | `time.strptime(ISO, ISOF)` (ISO datetime) | 43,473 | 3,736 | 0.086 | 2,200 | 1,536 (41%) | 0 | 0.00% | **open**: struct_time build (~700) stays; matcher as above |
| `datetime.date.strftime` | `dt.strftime('%Y-%m-%d %H:%M:%S')` (datetime, numeric) | 17,145 | 1,857 | 0.108 | 800 | 1,057 (57%) | 122,593 tz_events | 3.69% | **open**: directive interpretation 900 Ir: compiled format cache (format str → plan) would leave ~150 |
| `datetime.time.strftime` | `t0.strftime('%H:%M:%S.%f')` (time) | 11,876 | 1,473 | 0.124 | 750 | 723 (49%) | 0 | 0.00% | **open**: as date.strftime |
| `time.strftime` | `time.strftime('%Y-%m-%d %H:%M:%S', st)` (struct_time) | 5,364 | 1,950 | 0.364 | 1,300 | 650 (33%) | 0 | 0.00% | **settled**: struct_time unpacking ~400 + CALL_NON_PY_GENERAL; ≤ 650 Ir left, no workload calls |
| `datetime.datetime.isoformat` | `dt.isoformat()` (naive, microseconds) | 6,365 | 733 | 0.115 | 600 | 133 (18%) | 32,689 csv_etl | 0.17% | **settled**: eval 210 + str alloc 105 + digits; ≤ 130 Ir left |
| `datetime.date.isoformat` | `d0.isoformat()` (date) | 3,070 | 493 | 0.161 | 450 | 43 (9%) | 721 state_snapshot | 0.00% | **settled**: at the floor |
| `datetime.time.isoformat` | `t0.isoformat()` (time) | 3,899 | 657 | 0.169 | 560 | 97 (15%) | 0 | 0.00% | **settled**: ≤ 100 Ir left |
| `posixpath.join` | `join(d, f)` (two components) | 3,540 | 972 | 0.275 | 560 | 412 (42%) | 24,590 fs_inventory | 1.06% | **open**: k_join 437 Ir for two short strs is ~4x a minimal join (~100): 410 Ir/call, 1% of fs_inventory (cheap fix) |
| `posixpath.split` | `split(p)` | 3,961 | 1,102 | 0.278 | 900 | 202 (18%) | 6 fs_inventory | 0.00% | **settled**: two str + tuple allocs (~500) + entry |
| `posixpath.splitext` | `splitext(f)` | 3,926 | 1,076 | 0.274 | 880 | 196 (18%) | 5,427 fs_inventory | 0.11% | **settled**: as split |
| `posixpath.basename` | `basename(p)` | 2,047 | 603 | 0.295 | 520 | 83 (14%) | 19,180 pip_resolve | 0.04% | **settled**: entry + one str |
| `posixpath.dirname` | `dirname(p)` | 3,395 | 615 | 0.181 | 520 | 95 (16%) | 199 coverage_pyflakes_tests | 0.00% | **settled**: entry + one str |
| `posixpath.isabs` | `isabs(p)` | 1,527 | 309 | 0.203 | 300 | 9 (3%) | 10 tz_events | 0.00% | **settled**: = the vectorcall entry floor (same(x): 264 Ir) |
| `posixpath.relpath` | `relpath(p, root)` | 20,895 | 7,048 | 0.337 | 3,000 | 4,048 (57%) | 7,492 fs_inventory | 3.18% | **open**: Rust heap traffic (malloc/free/realloc ~2.3k) + Vec<&[u32]> building; normpath on code-point vectors |
| `posixpath.abspath` | `abspath(p)` (absolute input) | 3,067 | 2,193 | 0.715 | 1,000 | 1,193 (54%) | 586 pip_resolve | 0.02% | **open**: normpath over a Vec<u32> (855) + UCS4 round trip (320): work on the PEP 393 buffer in place |
| `fnmatch.fnmatch` | `fnmatch_(f, '*.py')` | 4,703 | 628 | 0.134 | 450 | 178 (28%) | 69,193 fs_inventory | 1.29% | **open**: thread-local pattern cache lookup 146 Ir → a per-pattern pointer cache ~30 (cheap fix, 1.3% of fs_inventory) |
| `fnmatch.fnmatchcase` | `fnmatchcase(f, '*.py')` | 3,182 | 628 | 0.197 | 450 | 178 (28%) | 0 | 0.00% | **settled**: as fnmatch |
| `fnmatch.filter` | `ffilter(names, '*.py')` (32 names) | 57,994 | 5,277 | 0.091 | 2,800 | 2,477 (47%) | 0 | 0.00% | **open**: thread-local cache access 1,735 Ir per call(!) inside the loop: hoist it out of the loop |
| `pathlib.PurePath.__new__` | `PurePosixPath(d, f)` (with __init__) | 5,494 | 1,868 | 0.340 | 1,400 | 468 (25%) | 5,212 pip_resolve | 0.06% | **settled**: object + list allocs (~760) + slot_tp_new/init path ~640 |
| `pathlib.Path.__new__` | `Path(p)` (with __init__) | 4,547 | 1,865 | 0.410 | 1,400 | 465 (25%) | 3,833 fs_inventory | 0.19% | **settled**: as PurePath.__new__ |
| `pathlib.PurePath.__init__` | `Path(p)` (same cell as Path.__new__) | 4,547 | 1,865 | 0.410 | 1,400 | 465 (25%) | 3,833 fs_inventory | 0.19% | **settled**: same cell as Path.__new__ |
| `pathlib.PurePath.__str__` | `str(pp)` (cached str) | 609 | 403 | 0.662 | 360 | 43 (11%) | 1,971 fs_inventory | 0.01% | **settled**: cached str: entry only |
| `pathlib.PurePath.__truediv__` | `pp / f` | 6,162 | 1,532 | 0.249 | 1,250 | 282 (18%) | 4,253 pytest_networkx | 0.03% | **settled**: new path object + list allocs (834) |
| `pathlib.PurePath.parent` | `pp.parent` | 9,244 | 2,750 | 0.297 | 1,700 | 1,050 (38%) | 657 pytest_networkx | 0.02% | **open**: format() 551 + list/obj allocs 1,251: build the parent's str directly |
| `pathlib.PurePath.name` | `pp.name` | 715 | 391 | 0.546 | 330 | 61 (16%) | 391 pytest_networkx | 0.00% | **settled**: property entry ~260 |
| `pathlib.PurePath.suffix` | `pp.suffix` | 2,085 | 699 | 0.335 | 520 | 179 (26%) | 200 pytest_networkx | 0.00% | **settled**: entry + str |
| `pathlib.PurePath.stem` | `pp.stem` | 2,208 | 675 | 0.306 | 520 | 155 (23%) | 1,970 fs_inventory | 0.03% | **settled**: entry + str |
| `pathlib.PurePath.parts` | `pp.parts` | 2,488 | 699 | 0.281 | 600 | 99 (14%) | 7,766 fs_inventory | 0.08% | **settled**: tuple of parts |
| `pathlib.PurePath.relative_to` | `pp.relative_to(root)` | 108,661 | 4,974 | 0.046 | 2,600 | 2,374 (48%) | 3,832 fs_inventory | 0.95% | **open**: allocations 2.1k (+473 libc) for one result object: parse once, build the result str directly |
| `pathlib.PurePath.__eq__` | `pp == pq` | 1,807 | 517 | 0.286 | 420 | 97 (19%) | 19,518 pytest_networkx | 0.05% | **settled**: slot_tp_richcompare + compare |
| `pathlib.PurePath.__hash__` | `hash(pp)` (cached) | 835 | 621 | 0.744 | 560 | 61 (10%) | 17,748 pytest_networkx | 0.03% | **settled**: slot_tp_hash path |
| `logging.Logger.level` | `L_OFF.debug('x=%s', v)` (disabled call) | 1,343 | 845 | 0.629 | 520 | 325 (38%) | 12,000 logging_service | 0.21% | **settled**: disabled call: entry 177 + level cache; ~300 Ir left |
| `logging.Logger.findCaller` | `L_TIME.findCaller()` | 12,485 | 4,027 | 0.323 | 900 | 3,127 (78%) | 14,154 logging_service | 2.42% | **open**: _PyCode_CheckLineNumber 3,172 Ir (79%): a (code, instr offset) → line cache leaves ~150 |
| `logging.LogRecord.__init__` | `logging.LogRecord('app.http', 20, '/srv/app/handlers.py', 42, 'x=%s', (v,), None, 'handle', None)` | 19,505 | 7,574 | 0.388 | 4,700 | 2,874 (38%) | 14,154 logging_service | 2.22% | **open**: 20 PyObject_GenericSetAttr stores 3,065 Ir: direct inline-values stores (calibrated layout) ~600 |
| `logging.Formatter.formatTime` | `F_TIME.formatTime(rec)` | 11,848 | 2,507 | 0.212 | 1,500 | 1,007 (40%) | 15,170 logging_service | 0.84% | **settled**: localtime_r + digits; strftime_numeric 326; ≤ 1k Ir left |
| `logging.Formatter.format` | `F_TIME.format(rec)` (asctime format) | 23,654 | 9,675 | 0.409 | 5,500 | 4,175 (43%) | 13,492 logging_service | 3.08% | **open**: type lookups 900 (_PyType_LookupRef, _PyObject_GetMethod) cacheable per type version; percent render |
| `random.randint` | `rng.randint(3, 7)` | 2,279 | 781 | 0.343 | 520 | 261 (33%) | 60,000 report_render | 0.83% | **settled**: entry ~280 + MT word + int; ≤ 260 Ir left |
| `random.randrange` | `rng.randrange(10)` | 2,200 | 757 | 0.344 | 500 | 257 (34%) | 360 email_mime | 0.00% | **settled**: as randint |
| `random.choice` | `rng.choice(L6)` | 1,821 | 540 | 0.297 | 420 | 120 (22%) | 20,000 report_render | 0.13% | **settled**: ≤ 120 Ir left |
| `random.shuffle` | `rng.shuffle(L100)` (100 items) | 175,956 | 6,605 | 0.038 | 4,500 | 2,105 (32%) | 0 | 0.00% | **settled**: 100 swaps + MT draws ~45 Ir/item |
| `random.sample` | `rng.sample(W, 5)` | 17,400 | 2,293 | 0.132 | 1,500 | 793 (35%) | 40,000 report_render | 1.67% | **open**: sample_indices 475 (set/selection bookkeeping) + Rust heap: ~800 Ir/call, 1.7% of report_render |
| `random.choices` | `rng.choices(T4, weights=[50, 30, 15, 5])` | 8,376 | 2,644 | 0.316 | 1,600 | 1,044 (39%) | 40,000 report_render | 2.20% | **open**: weights read + int→float conversions (__floattidf 180) + Rust heap: ~1k Ir/call, 2% of report_render |
| `statistics.mean` | `S.mean(F10)` (10 floats) | 125,861 | 13,772 | 0.109 | 6,000 | 7,772 (56%) | 0 | 0.00% | **settled**: exact rational arithmetic in CPython ints (_PyLong_FromByteArray/x_divrem 7.6k); native 128-bit exact path would halve it, but statistics has 1 call per run in the set |
| `statistics.variance` | `S.variance(F10)` (10 floats) | 296,398 | 42,906 | 0.145 | 15,000 | 27,906 (65%) | 0 | 0.00% | **settled**: as mean (bigint products/GCD 31k) |
| `statistics.pvariance` | `S.pvariance(F10)` (10 floats) | 296,366 | 42,903 | 0.145 | 15,000 | 27,903 (65%) | 0 | 0.00% | **settled**: as variance |
| `statistics.stdev` | `S.stdev(F10)` (10 floats) | 304,220 | 81,315 | 0.267 | 25,000 | 56,315 (69%) | 1 billing_engine | 0.00% | **settled**: as variance + exact sqrt rounding (x_divrem 20k) |
| `statistics.pstdev` | `S.pstdev(I10)` (10 ints) | 124,845 | 23,429 | 0.188 | 10,000 | 13,429 (57%) | 0 | 0.00% | **settled**: as stdev |
| `textwrap.wrap` | `textwrap.wrap('hello world')` (short) | 31,619 | 7,573 | 0.240 | 2,500 | 5,073 (67%) | 0 | 0.00% | **open**: Rust heap traffic + dict lookups for the wrapper's options |
| `textwrap.fill` | `textwrap.fill(DESC, width=88, initial_indent='> ', subsequent_indent='> ')` (~330 chars) | 500,948 | 79,585 | 0.159 | 26,000 | 53,585 (67%) | 4,003 report_render | 11.28% | **open**: Rust heap traffic 36k Ir/call (malloc/free/realloc of word Vecs) + split 15k: arena/reused buffers, word spans into the source buffer |
| `textwrap.shorten` | `textwrap.shorten(TITLE, 30, placeholder='...')` | 108,128 | 18,175 | 0.168 | 7,000 | 11,175 (61%) | 3,000 report_render | 1.76% | **open**: as fill |
| `textwrap.dedent` | `textwrap.dedent(SRC)` (20 lines) | 53,232 | 24,566 | 0.461 | 8,000 | 16,566 (67%) | 248 logging_service | 0.22% | **open**: Vec<u32> building 6.1k + memset 2.9k + UCS4 round trip 5k |
| `textwrap.indent` | `textwrap.indent(LINES, '  ')` (30 lines) | 34,307 | 12,547 | 0.366 | 5,000 | 7,547 (60%) | 515 isort_stdlib | 0.17% | **open**: splitlines into owned Vecs + UCS4 round trip |
| `textwrap.TextWrapper.wrap` | `TW.wrap(T540)` (540 chars) | 1,084,863 | 167,562 | 0.154 | 55,000 | 112,562 (67%) | 0 | 0.00% | **open**: as fill (540 chars) |
| `textwrap.TextWrapper.fill` | `TW.fill(DESC)` | 508,975 | 83,113 | 0.163 | 26,000 | 57,113 (69%) | 0 | 0.00% | **open**: as fill |
| `urllib.parse.quote` | `quote(P)` | 15,345 | 3,168 | 0.206 | 1,400 | 1,768 (56%) | 13,200 flask_jinja | 0.79% | **open**: Rust heap traffic ~900 + input conversion 388 |
| `urllib.parse.quote_plus` | `up.quote_plus('a b&c')` | 11,128 | 1,785 | 0.160 | 1,000 | 785 (44%) | 4,080 flask_jinja | 0.11% | **settled**: ≤ 800 Ir left |
| `urllib.parse.unquote` | `unquote('a%20b%C3%A9')` | 21,720 | 3,678 | 0.169 | 2,000 | 1,678 (46%) | 3,200 fastapi_pydantic | 0.37% | **open**: Rust heap ~700 + UCS4 conversion |
| `urllib.parse.unquote_plus` | `up.unquote_plus('a+b')` | 2,442 | 1,380 | 0.565 | 900 | 480 (35%) | 0 | 0.00% | **settled**: ≤ 500 Ir left |
| `urllib.parse.parse_qsl` | `parse_qsl(Q)` (5 fields) | 38,882 | 13,070 | 0.336 | 7,500 | 5,570 (43%) | 800 fastapi_pydantic | 0.31% | **open**: Rust heap ~3.8k (malloc/calloc/free) + memcmp; tuples/strs stay |
| `urllib.parse.parse_qs` | `parse_qs(Q)` (5 fields) | 44,495 | 14,949 | 0.336 | 8,500 | 6,449 (43%) | 8,743 log_analytics | 0.99% | **open**: as parse_qsl |
| `urllib.parse._urlsplit` | `up._urlsplit(URL)` | 18,333 | 6,753 | 0.368 | 3,300 | 3,453 (51%) | 6,000 requests_local | 1.72% | **open**: Rust heap ~1.8k + Vec<u32> conversion 759: slice the PEP 393 buffer |

Largest remaining shares: textwrap.fill 11.28%, json.dumps 4.96%, datetime.date.strftime 3.69%, posixpath.relpath 3.18%, logging.Formatter.format 3.08%, copy.deepcopy 2.89%, datetime.datetime.strptime 2.67%, logging.Logger.findCaller 2.42%, logging.LogRecord.__init__ 2.22%, random.choices 2.20%, json.loads 2.02%, textwrap.shorten 1.76%

### 1.1 Notes per group

- **containers, builtins (13): all settled.**
  - Enabled calls cost 243–529 Ir, of which 110–250 is CPython's own call or `CONTAINS_OP` path. The
    scans run at ~25 Ir per item.
  - What remains is ≤ 200 Ir per call; the largest share is `min` in billing_engine at 0.7% (230k calls).
  - The open items here are *coverage* (§2) and the **sort regression** (§6), not floors.
- **float (1): at the floor.** 758 Ir = shortest digits (~210, zmij) + str alloc/free (170) +
  `CALL_BUILTIN_O`/`repr` (~180).
- **json (2): open.**
  - `dumps(record)`, 6.6k Ir:
    - the ASCII escape scan costs 1,728 Ir for ~80 characters (~20 Ir/char; a SIMD scan is ~2);
    - the output is copied twice (763 Ir of `memcpy`: Vec, then the str);
    - the floor is ~2.6k.
    - In csv_etl, 32,690 calls/run put this at **5%** of the run.
  - `loads` is mostly object creation: allocs 3.9k, `PyDict_SetItem` 1.6k, key hashing 1.1k. About
    32% is left in the parser.
- **copy, dataclasses (4): open.**
  - `deepcopy` of a 10-key dict spends 1.5k Ir in `PyDict_SetItem` + `dictresize`. A presized result,
    or cloning the keys table of a str-keyed dict, leaves ~1.7k.
  - `copy.copy` and `asdict` pay 0.7–1.1k Ir of type lookups and generic attribute reads. A per-class
    plan cached under the type's version tag removes most of it.
- **datetime (10): six open, four settled.**
  - `strptime` (4.9k) and `strftime` (1.9k) interpret the format string on every call. A plan
    compiled per (format, locale) leaves ~1.5k and ~0.8k.
  - The shares are 3.7% of tz_events (122,593 `strftime`/run) and 2.7% of log_analytics.
  - `isoformat` is settled at ≤ 130 Ir left.
- **posixpath, fnmatch, pathlib (24): mostly settled.**
  - `isabs` is exactly the vectorcall entry floor (309 vs 264 Ir).
  - Open:
    - `relpath`: 57% left, Rust heap plus `Vec<&[u32]>`; 3.2% of fs_inventory.
    - `join`: `k_join` is 437 Ir for two short strings; 1%, a cheap fix.
    - `fnmatch`: a thread-local cache lookup of 146 Ir; `filter` pays it per name, 1,735 Ir/call.
    - `abspath`, `parent`, `relative_to`: allocations.
- **logging (5): open.**
  - `findCaller`: 79% is `_PyCode_CheckLineNumber`. A (code, instruction offset) → line cache leaves
    ~150 Ir; 2.4% of logging_service.
  - `LogRecord.__init__`: 20 generic `setattr` stores cost 3.1k Ir. Stores into the inline values
    through a calibrated layout would cost ~0.6k. Contract-allowed (§3.14), but layout-sensitive.
  - `Formatter.format`: ~900 Ir of type lookups per call.
- **random (6): four settled; `choices` and `sample` open** (~1k Ir/call left; 2.2% and 1.7% of
  report_render).
- **statistics (5): settled by share.**
  - Exact rational arithmetic in CPython ints is 56–69% of the kernel. A native 128-bit exact path
    would roughly halve it.
  - But the workload set calls statistics once per run.
- **textwrap (7): open, and the largest remainder in the set.**
  - `fill` of 330 characters: 79.6k Ir. Of that, **36k is libc `malloc`/`free`/`realloc`** for
    per-word `Vec`s, and 15k is word splitting.
  - Spans into the source buffer and one reused buffer would leave ~26k (I).
  - 4,003 calls/run in report_render ⇒ **11.3%** of that run (I).
- **urllib (7): five open.** Rust heap traffic is 15–30% of every kernel (`_urlsplit` ~1.8k Ir, `parse_qs`
  ~3.8k). `_urlsplit` is 1.7% of requests_local.

**One fix covers a third of the open targets.** Kernels that build `Vec`s per call should take
scratch buffers from a per-thread arena (one allocation per thread, reset per call) and point into
the PEP 393 buffer instead of copying to `Vec<u32>`. This applies to textwrap, urllib, relpath,
abspath, dedent, sample, choices and json. The remainders are inferred from the measured libc
component of each cell.

**`__init__` targets and `CALL_ALLOC_AND_ENTER_INIT`** (w2-machinery's question; M, source + counters):
- The specializer bypasses an `__init__` vectorcall only for "simple" Python `__init__`s
  (`get_init_for_simple_managed_python_class`: `tp_new` is `object.__new__`, `SIMPLE_FUNCTION`).
- Neither wave-1 `__init__` target qualifies, and both are reached on every call:
  - `LogRecord.__init__` takes `**kwargs`. Its hits equal `findCaller`'s (14,154/run in logging_service).
  - `PurePath.__init__` takes `*args`, and the class defines `__new__`.
- A future target on a dataclass-generated `__init__` *would* be bypassed (w2-machinery).

## 2. Coverage gaps in real workloads

### 2.1 What the fallbacks are

Every `args`/`deopt` fallback of one steady-state `run()` of all 34 workloads was traced with the
prototype build (M; `RUSTIQUE_PROTO_TRACE=1`). Counts are per run; cost and share are inferred (I)
from the cells in §2.2 and `wl_instr.tsv`. Ranked by share.

| # | gap (target, what falls back) | calls/run (M) | Ir saved per call | share of run (I) | verdict |
|---:|---|---|---:|---:|---|
| 1 | `json.dumps(obj, default=f)` / `cls=JSONEncoder` where `f` is never called | logging_service 14,154, flask_jinja 960, json_api ~750; job_worker every call (w2-wl-frameworks) | 22–30k (cells: 0.849 → 0.183, 0.195) | **logging_service −18.2% (M)**, flask −1.1% (M) | **prototyped**, open → Phase 2 |
| 2 | `deepcopy`/`asdict` meeting `uuid.UUID` (Python `__getstate__`/`__setstate__`: restart at the UUID, whole `asdict` restarts) | json_api 5,173 asdict + 5,173 deepcopy | ~87k per asdict (138.7k now, 50.2k with an int id) | **json_api ~11%** | open: native reconstruct for pinned stdlib classes |
| 3 | `default=` hooks that *are* called (Decimal, datetime, UUID, Enum in responses) | json_api 9,600 | ~17k (38.8k now, ~22k with the Rust encoder behind `c_make_encoder`'s `tp_call`) | json_api ~4% | open: the `make_encoder` route |
| 4 | `x in list` with **tuple** needles (`(a, b) in pairs`) | black 167,856 | ~600 (list size unknown) | black ~0.8% (I, list length assumed ≈ 5) | open: equality of exact tuples of scalars |
| 5 | `in` with **class/identity** needles (`type(n) in (A, B)`, plain objects) | pyflakes 67,863; pylint 36,019; pytest 9,332; sqlalchemy 6,659; coverage 3,064 | ~430 (cells 0.44–0.54) | pyflakes 0.26%, pylint 0.21% | **prototyped**, open: cheap and broad |
| 6 | `float.__format__` with fixed precision (`.2f`, `,.2f`, `>10.2f`) | flask 8,568; xml_feeds 6,710; report_render 3,000; email 1,316 | ~1,200 (plain 1,805) | flask 0.35%, xml 0.15%, report 0.19% | open: broad reach (f-strings with `.2f` are everywhere) |
| 7 | `json.loads` error paths (invalid documents parse twice) | asyncio_http 852 | ~5k (half of a double parse) | 0.34% | needs a contract change (kernel raises CPython's `JSONDecodeError`); low priority |
| 8 | `in` with `bytes` needles | asyncio_http 14,507; requests_local 7,260 | ~200 | 0.23% / 0.12% | open: trivial (`memcmp`) |
| 9 | `deepcopy(PosixPath)` (Python `__reduce__`) | isort 1,030 | ~5k | 0.23% | open with #2 |
| 10 | `list.count(list)` | pytest_networkx 24,538 | ~200 | 0.14% | open with #4 |
| 11 | `sort(key=itemgetter(1))` on lists of ≤ 1 item; Python key functions | email 17,520; isort 11,528; others < 2k | ≤ 400 | ≤ 0.11% | **settled**: tiny lists, or the Python key dominates (`sorted(pairs, key=lambda)` 6.3k Ir, of which 5.0k are the key calls, M) |
| 12 | `in` with `IntEnum` needles (pytokens/sqlglot `TokenType`, `inspect._ParameterKind`) | black 13,442; sqlglot 6,058; django 2,702; sphinx 1,621 | ~300 | ≤ 0.05% | open with #5 (classify by `tp_richcompare` slot) |
| 13 | pathlib subclasses | sphinx ~2,500; pytest ~600 | ~3k | ≤ 0.08% | open, low (wave-1 plan) |
| — | `sum`/`min`/`max` over generators | email 131,678; billing 40,000; rich 25,000; pip 3,628 | 0 | — | **settled**: the generator's frames are the work; `sum`'s C loop is already C |
| — | `sorted(dict.items())` / `sorted(tuple)` "fallbacks" | xml_feeds 73,836; report 20,000; black 24,496 | 0 | — | **settled, not a gap**: by design `sorted` hands builtin inputs to `list.sort`, whose kernel then hits (73,832 hits in xml_feeds) |
| — | `deepcopy` root-blocked restarts (sqlglot `Expression.__deepcopy__`; pydantic models in fastapi) | sqlglot 10,361; fastapi 1,915 | 0 | — | **settled** (ruling 15: a user hook). After the first restart per class, roots go straight to the original (`ROOTS_DEEPCOPY`), so the remaining cost is the vectorcall penalty |
| — | `Logger.level` "deopt" | logging_service 14,154; every call in w2-wl-frameworks' job_worker, waitress_flask, uvicorn_fastapi | — | — | **not a fallback**: enabled calls pass through by design (the kernel only answers *disabled* calls). Cost +630–780 Ir per record, 1.009–1.014 with only this target on; the logging group as a whole makes records 0.45–0.51 (M). The record path (`_log` → `handle` → handlers) calls user code (w2-machinery). DX: count these as `pass` rather than `deopt` |
| — | `strptime` directives, `statistics` on iterators, logging `{`/`$` styles | 0 in the 34 workloads | — | — | no measured gap |

**What the counters said vs what costs.**
- The biggest counts (sum over generators, 173k; tuple needles in black, 168k; sorted delegations,
  80k) cost little or nothing.
- The biggest costs are hiding behind *suspensions*: json_api's and logging_service's `json.dumps`
  never show up in the one-run counters, because the target was suspended during setup.

**On w2-dx's 14 test suites** (their table):
- black's 3.57M `list.__contains__` `args` fallbacks are the tuple needles of #4.
  - A fallback through the slot costs +2–5 Ir (shipped kernel; M by containers.md).
  - Under the prototype's out-of-line generic path it costs +37 Ir, i.e. 1.033 on a 1.1k-Ir call (M).
- dateutil's and attrs' `min` "deopt" (44–49%) are **continuations, not restarts**: the scan hands
  over at the first non-scalar and CPython's loop continues from there, with no wasted work.
- sqlglot's `deepcopy` 49% "deopt" are root-blocked hook restarts: also no wasted kernel work.
- Fix for the counters: `deopt` mixes three cases (continuation, restart at the root, restart that
  abandons work). They should be separate reasons (DX, and input for §3).

### 2.2 Prototypes and their cells (callgrind, Ir per call, plain → enabled)

**(a) Identity membership** (`takeover.rs`: `tuple_contains_generic`, `list_contains_generic`).
- **Rule.** When the needle's type has `tp_richcompare` ∈ {NULL, `object`'s}, `==` against any item
  whose type is identity-eq, or is exact int/bool/float/str/None, runs no code and ends in identity:
  - `object_richcompare` returns NotImplemented both ways;
  - the scalar types' own slots return NotImplemented for foreign types.
  So the answer is `any(item is needle)`.
- **First other item.** At the first item outside that set, the kernel *continues CPython's own loop
  from that index*. The items before it were unequal without running code, exactly as
  `tuple_contains`/`list_contains` would have found. There is no restart, so no wasted work.
- **Coverage.** GIL-build lists and all tuples. Free-threaded lists are left to Phase 2 (they need
  CPython's `list_get_item_ref` loop).

| cell | plain | enabled | ratio |
|---|---:|---:|---:|
| `type(node) in NODE_TYPES` (5 ast classes, hit at 5th) | 799 | 370 | 0.463 |
| same, list | 853 | 376 | 0.441 |
| `C in (A, B, C)` | 419 | 224 | 0.535 |
| `D in (A, B, C)` (miss) | 561 | 247 | 0.441 |
| `o in [A(), A(), o]` | 455 | 224 | 0.493 |
| `D in (A, 1, 'x', None, C)` | 915 | 303 | 0.331 |
| `type(o) in CLS` (hit at 1st, already a hit before) | 215 | 203 | 0.944 |
| fallback: needle with `__eq__` | 1,119 | 1,156 | 1.033 |
| fallback after 1 item: metaclass `__eq__` (continuation) | 789 | 753 | 0.954 |
| fallback after 1 item: item `__eq__` (continuation) | 789 | 747 | 0.947 |
| `3 in (1, 2, 3)` (scalar path, unchanged) | 383 | 246 | 0.643 |

A first version that *restarted* in the original after a partial scan measured 1.13. The
continuation is what keeps fallbacks ≤ 1.05.

Workload conversions (M, counters of one run; digests identical to the base build on the same box):

| workload | `args` fallbacks, base → prototype |
|---|---|
| pyflakes tuple | 67,863 → **0** |
| pylint tuple / list | 23,318 → 54 / 13,069 → 314 |
| pytest_networkx list | 9,332 → 119 |
| coverage tuple | 3,064 → 0 |
| sqlalchemy tuple / list | 5,890 → 1,698 / 2,467 → 0 |

The run-level gain (~0.2–0.3%) is below the ±0.6% noise of per-run instruction counts, so it stays
inferred.

**(b) Lazy `default=`** (`targets/json/dumps.rs`).
- **Rule.** `default` is called only for an object outside the encoder's subset. There, the kernel
  already stops and restarts in the original before anything observable (ruling 15: the original is
  Python).
- **Accepting.** So `default=<anything>` and `cls=JSONEncoder` can be accepted unchanged. A
  `cls=` subclass that overrides only `default` (DjangoJSONEncoder) is next: check its MRO for
  `__init__`/`encode`/`iterencode` and the separator class attributes.

| cell | plain | before | prototype |
|---|---:|---:|---:|
| `dumps(rec, default=encode)` (hook not needed) | 36,750 | 0.849 | **0.183** |
| `dumps(payload, default=str)` (logging_service's formatter) | 27,807 | ~0.85 | **0.195** |
| `dumps(rec, cls=json.JSONEncoder)` | 36,658 | 0.849 | **0.184** |
| `dumps(rec_dt, default=encode)` (hook needed: restart, then suspended) | 48,332 | 0.784 | 0.785 |

Workloads (M, `perf stat` instructions per enabled `run()`, two repeats each, spread < 0.6%):

| workload | before → prototype | change | counters (M) |
|---|---|---:|---|
| logging_service | 1.843G → 1.508G | **−18.2%** | json.dumps 0 → 14,154 hits/run, no suspension |
| flask_jinja | 2.952G → 2.921G | −1.1% | 960 hits/run |
| json_api | | | 750 more hits, then suspended as before (its hooks are needed): gap #3 |

Every digest is unchanged in the 11 JSON-using workloads.

**(c) `asdict`/`deepcopy` with UUID (not prototyped).**

| cell | plain | enabled | ratio |
|---|---:|---:|---:|
| `asdict(Order)` with a UUID in a nested dataclass (json_api's shape) | 162,669 | 138,726 | 0.853 (restart → Python asdict) |
| the same Order with an int id | 133,242 | 50,218 | 0.377 |
| `deepcopy(UUID)` | 27,290 | 23,484 | 0.861 |

- **Fix.** Reconstruct exact `uuid.UUID` natively, with `__getstate__`/`__setstate__`/
  `__reduce_ex__` pinned and fingerprinted:
  - the result is `object.__new__(UUID)` with `int` and `is_safe` set as `UUID.__setstate__` sets them;
  - the memo contents follow `_reconstruct` (state dict, `_keep_alive`) when the memo is the user's.
- **Same mechanism.** `PurePath.__reduce__` (gap #9) and `Fraction.__deepcopy__` would use it too.

**(d) `make_encoder` route.**
- Not prototyped. Estimate from the cells: Python `dumps` + `JSONEncoder.__init__` (3.9k, M) +
  `encode`/`iterencode` ≈ 9k of Python overhead remain; the C encoder part goes to the Rust encoder.
- Ruling 15 allows calling `default` from the `tp_call` kernel, because `_json` does so at the same
  point.

## 3. The credit system (ACCELERATION §5.6)

**How it works today** (`accel/stats.rs`):
- A vectorcall kernel's score gains `hit_credit` (4) per hit and loses `fallback_debit` (1) per
  non-`deps` fallback.
- The score is clamped to [−64·debit, 256]. At the floor, the original vectorcall is restored until
  the next `enable()`.
- So a fresh target suspends after 64 consecutive fallbacks; a target at its ceiling after 320.
- Every target uses 4/1. `deepcopy` additionally always debits abandoned restarts.

**How often it fires** (M: Phase-3 whole-process records, then the same 12 workloads re-run with
`RUSTIQUE_PROTO_NO_SUSPEND=1`):
- 24 suspensions in 34 workloads. In 23 of them the steady-state traffic, measured without
  suspension, is (almost) all fallbacks, so suspending was right.

| workload | target | Phase 3 | steady state without suspension | right? |
|---|---|---|---|---|
| json_api | json.dumps | 9,600 hits (setup) then 320 `args` → suspended | 0 hits / 9,600 `args` per run (`default=` needed) | yes (fix: gap #1/#3) |
| json_api, isort, pylint, pip | dataclasses.asdict | 64 `deopt`/`args` → suspended | 0 hits / 114–5,173 per run | yes (json_api: gap #2) |
| logging_service, flask, django | json.dumps | 64 `args` → suspended | 0 / 14,154, 0 / 960, 0 / 42 | yes (gap #1 turns these into hits) |
| django_blog | random.choice | 64,387 hits (setup) then 320 `args` | 0 / 2,272 per run (`secrets.choice` → `SystemRandom`) | yes |
| django_blog | copy.copy | 8 hits, 68 `deopt` | 252 / 1,946 (11% hits) | yes |
| pytest_networkx | posixpath.abspath | 9 / 100 | 567 / 4,661 (11%) | yes |
| **pytest_networkx** | **pathlib.PurePath.__new__** | 8 hits, 96 `args` at startup → suspended | **175 / 150 per run (54% hits); 2,734 / 225 over the process** | **no**: a burst of subclass constructions at pytest startup |
| others (fnmatch.filter, textwrap.dedent/indent, urllib.*, posixpath.basename/isabs, Formatter.format, PurePath.__eq__/relative_to) | | 64 fallbacks → suspended | 0–2 hits per run | yes |

**What it costs.**
- *The exceedances* (the fallbacks before the suspension): ≤ 64 (fresh) or 320 (after a hit streak)
  fallbacks × 180–350 Ir ≤ 112k Ir per target per process. That is ≤ 0.01% of any workload run, a
  bounded one-time cost (I).
- *The wrong decision*: pytest_networkx loses 2,734 hits × ~3.6k Ir ≈ 10M Ir per process, ~0.1% (I).
- *Mixed traffic that stays installed while losing*: flask_jinja's `urllib.unquote` hits 20.4% of
  1,200 + 4,680 calls/run. Savings ~530 Ir per hit (its inputs mostly have no `%`) against ~250 Ir
  per fallback ⇒ −0.5M Ir/run, 0.02% (I).
- *Mixed traffic that is right to stay*: sqlglot's `deepcopy` at 49% restarts (w2-dx). These are
  root-blocked restarts: no wasted kernel work, ~250 Ir of penalty on calls of 5–50k Ir, while hits
  save 70–80%. Net strongly positive; 4/1 gets it right here.

**Assessment.** Net benefit is the right criterion. Per-call-site decisions are **not** possible:
the penalty is per *function*. `PyFunction_SetVectorcall` unsets the function version, so no call
site can specialize `CALL_PY_EXACT_ARGS`. Per-site bookkeeping would cost per call and recover
nothing. What is measured to go wrong is small, so: **evolve, do not replace.**

1. **Cost-weighted credits**, set per target from its own cells (the mechanism exists:
   `hit_credit`/`fallback_debit` in `TargetSpec`; no target sets them):
   - credit = Ir saved per hit at the target's representative input;
   - debit = measured fallback penalty (vectorcall ~250 Ir) + wasted kernel work for abandoning
     restarts (the kernel knows how far it got);
   - examples: `unquote` 2/1, `deepcopy` 16/1, `json.dumps` 30/1.
2. **A decision window**: do not suspend before 256 calls have been seen. This fixes the
   pytest_networkx startup burst (it costs ≤ 256 × 250 Ir = 64k Ir once).
3. **Separate the reasons**: `continue` (no waste: builtins scans, contains continuation), `restart`
   (root, no waste), `restart-abandon` (waste). Only the last should be weighted by work. The DX
   report should show `suspensions` (w2-dx: it doesn't).
4. Keep the score-based suspension and its CAS restore. No timer-based re-arming: it would re-pay
   the stop-the-world `SetVectorcall` on free-threaded builds (~1 ms with busy threads, M§5) for a
   gain measured at ≤ 0.1%.

**mini_interp 1.011** (w2-wl-custom; the lead's question; M):
- One steady-state `run()` calls `tuple.__contains__` 9,539 times (0 fallbacks) and `list.sort`/
  `sorted` once. No vectorcall target is reached, so the credit system is not involved.
- Five repeats of `perf stat` per-run instructions: plain 14.68–14.81G and enabled 14.70–14.90G, plus
  one outlier each (16.4G, 19.0G). Medians give 1.005.
- `enable()` adds 73 GC-tracked objects. `gc.freeze`/`gc.disable` variants are all within ±1%.
- The 1.011 is run-to-run noise of per-run instruction counts on the shared box.

## 4. Per-target install costs (DECISIONS §5 item 4)

Measured (M) with callgrind on 3.15.1, release (fat-LTO) build, dev box, by a forked analysis
(`benches/floors/install/{install_profile,fp_cost,lazy_cells}.py`; raw data in
`~/rustique-research/w2/floors/install/raw/`). Ir is attributed by function, inclusive. w2-startup
owns extension load and init; this section covers per-target work.

**Enable with the module already imported** (budget 0.1M; k Ir):

| group | total | fingerprints | interning | pins | target prepare | registry | of which Vec growth / `format!` |
|---|---:|---:|---:|---:|---:|---:|---|
| json | 258 | 51 | 59 | 50 | 22 | 71 | 26 / 12 |
| copy | 176 | 51 | 33 | 22 | 14 | 50 | 10 / 8 |
| dataclasses | 174 | 44 | 38 | 25 | 8 | 55 | 17 / 6 |
| logging | 488 | 98 | 127 | 41 | 45 | 170 | 51 / 32 |
| random | 169 | 39 | 15 | 21 | 28 | 59 | 16 / 10 |
| posixpath | 236 | 46 | 44 | 24 | 26 | 92 | 25 / 29 |
| fnmatch | 296 | 148 (`re._parser._parse` dominates) | 35 | 22 | 20 | 67 | 16 / 23 |
| pathlib | 493 | 99 | 103 | 35 | 69 | 181 | 46 / 40 |

Notes on the table:
- Groups not listed in DECISIONS §5 but also over budget: datetime 114, textwrap 158, urllib 184,
  statistics 115.
- Within budget: float 57, containers 74, builtins 61.
- Self-tests cost ≤ 0.1k in every group.
- `GroupSpec` sharing saves only 76k of the paths groups' 1,025k.

**Lazy install** (GC off, cold), reconciled with w2-startup's cells:

| module | this group only | all groups (`RUSTIQUE=1`) | w2-startup |
|---|---:|---:|---:|
| logging | +563k | +1,265k (+2.4%) | +1.26M |
| json | +311k | +354k | +369k |
| pathlib | +529k | +443k (+4.8%) | +441k |

- **logging's 1.26M is three groups.** `import logging` imports `traceback`, which imports `pathlib`
  and `textwrap` on 3.15.
- **Percentages depend on the import baseline.** Against a cold import, only copy, random and pathlib
  exceed 5%. Against warm dependencies json is +6.8%, reproducing Phase 3's +6.5%.
- **logging's "0.143M" in DECISIONS §5 was a re-enable**: re-enable 161k, first enable 488k.

**Fingerprint economics.**
- A first fingerprint costs 5.5–8k Ir per function (fnmatch ~14k).
- `_PyCode_GetCode` (the de-optimised copy) is 22–48% of that; the rest is the hex `String`, its cache
  and a HashMap insert.
- Docstrings cost 0.5–1.1 Ir per character, so dropping them (v3, §5) is cost-neutral or a small
  gain. The canonical exception table adds ~10 Ir per entry (I).

**Never-called groups.** 39% of all install work goes to groups the workload never calls (I, counters
× costs):
- fnmatch is installed in 34/34 workloads and called in 2;
- statistics is never called in 33/34;
- dataclasses is never called in 15/20.

**Fixes, ranked:**
1. **Validate on first call** (accel-core; one mechanism shared with w2-startup's posixpath/builtins
   deferral).
   - At install: version gate, site resolution and swap, with the target's `DepFlag` set to
     "unvalidated" (kernels already test it, so no per-call cost).
   - At first call: fingerprints, identity checks, self-tests and pins, in a cold path. On failure,
     degrade and restore through the existing resync/suspension path; no `SetVectorcall` at first
     call on free-threaded builds.
   - Effect: install falls to ~15k per group (I), and never-called groups never pay.
   - Contract change: §3.2 "before any site is touched" becomes "before any kernel runs"; `status()`
     gains "installed (pending validation)"; a validate-now API for CI.
2. **Fingerprint v3 implementation**: compare `u64` to `u64` (no hex `String`, no cache); hash
   `co_code_adaptive` through a per-minor de-optimisation table (self-tested) instead of
   `_PyCode_GetCode`; skip docstrings. −40–50% of fingerprint cost (I).
3. **Fingerprint only what the kernel depends on**: fnmatch's `re._parser._parse` becomes a
   behavioural probe of set syntax (−70k).
4. **Registry allocation hygiene**: pre-sized Vecs, lazy site labels, one str-layout calibration
   (json −38k, logging −82k, pathlib −87k; measured function costs).
5. **Interning**: reuse each dict's own key objects (one `PyDict_Next` pass) or a static name table
   interned once (−60%, I).
6. **Pins**: grouped per dict, reusing values prepare already read (−40%, I).
7. **pathlib**: replace `hasattr("fget")`/`hasattr("__func__")` with type checks (−40k).

Outcome:
- Fixes 2–7 without fix 1 (I): json ~142k, copy 106k, random 109k, posixpath 126k, fnmatch 157k,
  pathlib 245k, logging 269k. That meets every lazy-install budget but not the 0.1M enable budget.
- **Fix 1 is needed for the 0.1M budget.**
- Found along the way: `_accel.fingerprint_report(['random'|'textwrap'|'datetime'])` panics ("PyObject
  pointer is null") in record mode.

## 5. Patch releases: per-minor fingerprints are really per patch range

### 5.1 What installs where (M)

`benches/floors/refusals.py`: `status()` after `enable()` and importing every target module. Same
wave-1 build, `cp314`/`cp315` extension, python-build-standalone 20261021/26 interpreters, dev box.

| CPython | targets installed | refused (besides the 5 pathlib properties refused by design on 3.14) | cause |
|---|---:|---|---|
| 3.15.2, 3.15.2t | **84/84** | — | |
| 3.15.1 (recorded) | 84/84 | — | |
| 3.15.0 | **43/84** | posixpath ×8, fnmatch ×3, pathlib ×13, logging ×5, time.strptime/strftime, copy ×2, dataclasses ×2, json.loads, random.choices, statistics.stdev/pstdev, textwrap.dedent, urllib.parse_qsl | exception tables (compiler fix in 3.15.1), `time.strftime` `ml_doc`, real code changes (`copy.deepcopy`, `dataclasses.fields`) |
| 3.14.10, 3.14.10t | 79/84 | — | |
| 3.14.8 | 79/84 | — | |
| 3.14.6 | 72/84 | logging ×5, time.strptime/strftime | `time.strftime` `ml_doc` ("tuple" → "time_tuple", changed in 3.14.8) |
| 3.14.1, 3.14.5 | 50/84 | + list.sort, builtins.sum, json.dumps, posixpath ×8, fnmatch ×3, pathlib ×8 | C `ml_doc` of `list.sort`, `sum`, `posix.fspath`; `json.dumps` docstring |
| 3.14.0 | 47/84 | + json.loads, statistics.stdev/pstdev, pathlib `_raw_path` | docstrings, real code changes |

**Conformance on the new patch releases.** 0 differences in all 27 CPython test modules, all 15
groups at once, every target installed (M, `benches/floors/conformance_all.sh`):
- 3.15.2 and 3.15.2t: 8,129 outcomes each;
- 3.14.10: 7,827 outcomes;
- results in `benches/floors/results/conformance-patch/`.

### 5.2 Causes, and how often patch releases touch target code

- **Docstrings are hashed.**
  - Python: on 3.14+ the docstring is `co_consts[0]`, and the v2 fingerprint hashes all constants.
  - C: the C fingerprint hashes `ml_doc`.
  - A typo fix refuses the target. 3.15.2's only target-module change is a docstring typo in
    `JSONDecoder.__init__`; it is not fingerprinted, by luck.
- **Exception tables move without semantic change.** 3.15.0 → 3.15.1: `posixpath.join`, `textwrap.dedent`,
  `statistics.stdev` and `JSONDecoder.raw_decode` have identical `co_code`, consts, names and flags.
  Only `co_exceptiontable` differs: the compiler stopped leaving holes at `NOT_TAKEN` instructions
  (§5.3).
- **History.** `benches/floors/patch_history.py` over CPython's tags compares function ASTs without
  docstrings and lists C files. Target modules or their dependencies change in **most** patch
  releases:
  - 3.13.x: 14 of 18 patch releases touch a target file (Python or C);
  - 3.14.x: 10 of 10; 3.12.x: 13 of 16;
  - e.g. `json.dumps`/`loads` docstrings (3.13.10, 3.14.1, 3.14.6), `statistics.stdev/pstdev` code
    (3.13.10, 3.14.1), `_strptime` (6 releases), `random.Random.sample`, `urllib.parse.urlsplit`
    security fixes.
  - **3.15.3 will very likely refuse something.**
- **C originals: fingerprints cannot see behaviour.**
  - 3.15.2/3.14.10 add a recursion guard to `tuple_hash`, and 3.14.3 rewrote part of `float_richcompare`
    (float vs big int).
  - No wave-1 kernel re-implements those: kernels call `PyObject_Hash`, and the int/float scans
    compare compact ints only (exact).
  - So nothing changed this time. But nothing would have caught it if a kernel did.

### 5.3 Fingerprint v3, and its exactness (the lead's three questions)

**(1) The canonical exception table.**

Facts (M, 3.15.0 vs 3.15.1):
- The *per-instruction* handler mapping (handler target, stack depth, lasti) differs at 18
  instructions across the four functions: `NOT_TAKEN` ×10, `CACHE` ×4, `POP_JUMP_IF_TRUE`/`FALSE` ×2,
  `JUMP_BACKWARD` ×2.
- The ranges were split differently (6 entries vs 2 for `join`).

Canonical form: hash the per-instruction mapping restricted to instructions that can raise a
**synchronous** exception, i.e. whose opcode metadata (`_PyOpcode_opcode_metadata` flags) has
`HAS_ERROR`/`HAS_ERROR_NO_POP`.

Why it is exact:
- A synchronous exception can only originate at such an instruction (the flags are generated from
  `bytecodes.c`: every `ERROR_IF`/escaping call sets them).
- CPython looks the handler up by the raising instruction's offset (`get_exception_handler`).
- So two code objects whose canonical forms agree send every synchronous exception to the same
  handler, with the same depth and lasti.
- `NOT_TAKEN`, `CACHE` and the conditional jumps (exact bools in 3.15) cannot raise.

The one place where 3.15.0 and 3.15.1 *really* differ:
- `JUMP_BACKWARD` raises only through the eval breaker: signals and `PyThreadState_SetAsyncExc`.
- That is an asynchronous exception, whose timing no native kernel reproduces anyway (a kernel
  completes its call, then pending calls run).
- **Decided (lead, 2026-11-02):** ACCELERATION §5.8 gains "asynchronous exceptions (signals,
  `PyThreadState_SetAsyncExc`) that arrive while a kernel runs are delivered after it returns, at the
  next eval-breaker check; their timing is already nondeterministic in CPython".
- The canonical form therefore excludes eval-breaker-only instructions.

With that, all four 3.15.0 functions match 3.15.1 (I, from the opcode list above). Implementing and
re-checking all 84 targets on 3.15.0/3.14.0–3.14.6 is Phase 2 work.

*Hashing only what the kernel depends on* is weaker for Python originals. The kernel replicates the
function's whole semantics on its subset, so the whole code object matters; v3 = v2 minus
docstrings, plus the canonical table.

**(2) C originals: what the self-test catches.**
- At install, `self_test` compares kernel vs original on a few probe inputs. It catches gross
  changes: a format, an error message, a changed result on the probes.
- It misses anything outside the probes. A recursion guard like `tuple_hash`'s is the typical case: it
  only matters for deeply nested input.
- A per-patch behavioural probe set per C target closes the gap only in **CI**, not at install. The
  probes worth having are CPython's own test module plus the target's Hypothesis corpus, minutes per
  build, too costly for install time.

Policy:
- C fingerprints drop `ml_doc` (it carries no behaviour) and keep `ml_name`/`ml_flags` and the
  slot/`d_wrapped` identity checks.
- Each new patch release passes the recording job (3) before its hashes are added.
- **Decided (lead, 2026-11-02):** an unrecorded patch release **refuses by default**, under its own
  visible refusal category `unvalidated-patch`. `status()` and the report give the reason and how to
  proceed. The rationale is the mission's "unknown version: refuse rather than risk a wrong
  result": v3 fingerprints cannot see C behaviour changes such as `tuple_hash`'s.
- An explicit opt-in (an environment variable; name to be settled in Phase 2) installs the targets
  whose v3 fingerprints match a recorded hash, reported as "unvalidated".
- The recording CI job (3) keeps that window short.
- (This slice had recommended installing by default; the decision above supersedes it.)

**(3) The recording CI job.**
- **Inputs**: the python-build-standalone release list (GitHub releases API,
  `astral-sh/python-build-standalone`), filtered to supported minors × {GIL, free-threaded} ×
  linux-x86_64-gnu. Python fingerprints are platform-independent, and `ml_doc` drops out with v3.
- **Steps per new patch release**:
  1. install the interpreter (`uv python install`);
  2. build/install the current wheel;
  3. `python -m rustique._takeover._dev fingerprint --all` (record mode; fix the `fingerprint_report`
     panic of §4 first);
  4. run `benches/floors/conformance_all.sh` (27 modules, all groups) and the target test suite
     (`pytest tests/targets`).
- **Output**: a PR that appends the new hashes.
- **Where the hashes live**: move the `Fingerprints` consts into one generated data file per minor
  (`crates/rustique-py/src/accel/fingerprints/3.15.rs`, or a TOML read at build time), as a **set** of
  accepted hashes per minor. Today `check_fingerprint` takes `expected_for(...)` = the first entry
  only.
- **Cost per release** (I, from this run's timings on the dev box):
  - build ~1–2 min, conformance ~3 min, `pytest tests/targets` ~4–5 min, for each of 2 builds;
  - ≈ 20 min of one CI runner per CPython patch release, ~1 per month across the two supported minors;
  - plus a Rustique release that carries the new data (a data-only patch version).

## 6. `list.sort` / `sorted`: a never-slower violation in the shipped kernel, and its fix

### 6.1 The report

w2-dx measured wall time (best of 7, dev box core 5, 3.15.1 release wheel):
- `sorted` on str lists: n=20 ascending 1.68, descending 3.21, random 1.38; n=1000 ascending 1.90.
- int n=20 descending: 1.47.

Callgrind confirms it (M, `benches/floors/proto_cells.py sort`, 90 cells: int/float/str ×
n ∈ {2, 5, 20, 100, 1000} × {asc, desc, rand, halves (two runs), nearly (n/50 swaps), dups (3 values)},
plus 5 `min`/`max` cells; `builtins`+`containers` enabled). The shipped kernel exceeds 1.05 in 22 cells:

| case | worst | cause |
|---|---:|---|
| str, any order, n=2–5 | 1.11–1.17 | fixed cost of the str path: a latin scan plus building 48-byte keys, on a sort CPython does in ~600 Ir |
| str presorted / descending, n ≥ 20 | 1.13–1.25 (desc n=20: **1.84**) | keys cost ~70 Ir per element (an 8-iteration byte loop per prefix, plus the str descriptor); timsort needs one `unsafe_latin_compare` per element on a run |
| str halves / nearly sorted, n ≥ 100 | 1.10–**1.67** | as above, and driftsort over 48-byte pairs vs timsort's galloping merge |
| int/float descending, n=20 | 1.23–1.32 | for n ≤ 20, Rust's stable `sort_by` uses insertion sort: quadratic on descending input; no first-run detection |

`min`/`max` on str/int/float are not affected (0.08–0.44 in every cell: a linear scan). The
scoreboard used random data only, so it missed these cases.

### 6.2 The fix (prototype, `takeover.rs` `try_sort` / `adaptive_sort`, `builtins.rs` `rq_sorted`)

1. **Timsort's run detection.**
   - Every keyed sort goes through `adaptive_sort` over `(key, item)` pairs.
   - For n ≤ 32 it does what `count_run` + `binarysort` do: find the first run, reverse it if strictly
     descending, finish with a binary insertion sort (upper bound: stable).
   - For n > 32 it uses Rust's driftsort, which detects ascending and strictly descending runs itself.
2. **str: use the native sort only where it wins.**
   - Lists shorter than 64 strings go straight to the original. This is checked before the guard and
     critical section, as a tail call.
   - Longer lists are probed: 4 evenly spaced adjacent pairs, compared directly. If all 4 go the same
     way, the input is "structured" and goes to the original, where timsort is near-linear.
   - Otherwise: keyed sort with an 8-byte prefix key; ties are compared on the strings.
   - The probe is only a strategy choice: both paths return CPython's result.
3. **`builtins.sorted` on a builtin list** falls back before its Rustique-container test. That test
   cost 53 Ir on every `sorted(list)`, and the target only hands builtin lists on to `list.sort`.
   - Better still, in Phase 2: install `builtins.sorted` only when the opt-in native types are enabled.
4. **The prototype's tracing code is compiled out by default** (`--features proto_trace`). Inlined into
   the fallback paths, it had added ~40 Ir to each.

### 6.3 Cells (M): shipped → prototype

Ratio enabled ÷ plain; **bold** = the shipped kernel was over 1.05.

| kind, n | asc | desc | rand | halves | nearly | dups |
|---|---:|---:|---:|---:|---:|---:|
| int 2 | 0.972 → 0.949 | 0.963 → 0.948 | 0.963 → 0.948 | 0.963 → 0.948 | 0.963 → 0.948 | 0.972 → 0.949 |
| int 5 | 0.919 → 0.913 | 0.913 → 0.865 | 0.789 → 0.845 | 0.819 → 0.880 | 0.800 → 0.908 | 0.828 → 0.895 |
| int 20 | 0.789 → 0.826 | **1.229** → 0.721 | 0.583 → 0.796 | 0.665 → 0.764 | 0.519 → 0.832 | 0.555 → 0.812 |
| int 100 | 0.702 → 0.700 | 0.579 → 0.578 | 0.464 → 0.464 | 0.746 → 0.745 | 0.797 → 0.797 | 0.374 → 0.375 |
| int 1000 | 0.750 → 0.750 | 0.744 → 0.744 | 0.419 → 0.419 | 0.868 → 0.869 | 0.847 → 0.848 | 0.369 → 0.370 |
| float 2 | 0.970 → 0.951 | 0.961 → 0.952 | 0.970 → 0.951 | 0.961 → 0.952 | 0.970 → 0.951 | 0.970 → 0.951 |
| float 5 | 0.926 → 0.929 | 0.924 → 0.881 | 0.816 → 0.937 | 0.848 → 0.917 | 0.826 → 0.944 | 0.852 → 0.927 |
| float 20 | 0.816 → 0.871 | **1.321** → 0.761 | 0.632 → 0.897 | 0.743 → 0.842 | 0.548 → 0.948 | 0.622 → 0.905 |
| float 100 | 0.730 → 0.741 | 0.605 → 0.613 | 0.545 → 0.548 | 0.791 → 0.801 | 0.716 → 0.722 | 0.433 → 0.438 |
| float 1000 | 0.833 → 0.861 | 0.822 → 0.848 | 0.536 → 0.541 | 0.967 → 0.994 | 1.001 → 1.012 | 0.461 → 0.469 |
| str 2 | **1.112** → 1.045 | **1.114** → 1.044 | **1.112** → 1.045 | **1.114** → 1.044 | **1.114** → 1.044 | **1.112** → 1.045 |
| str 5 | **1.121** → 1.034 | **1.173** → 1.032 | 0.938 → 1.027 | 0.938 → 1.027 | 0.953 → 1.028 | 1.015 → 1.028 |
| str 20 | **1.135** → 1.016 | **1.841** → 1.014 | 0.752 → 1.007 | 0.867 → 1.008 | 0.593 → 1.008 | 0.837 → 1.008 |
| str 100 | **1.185** → 1.049 | **1.052** → 1.042 | 0.599 → 0.524 | **1.153** → 1.043 | 0.885 → 1.021 | 0.751 → 0.858 |
| str 1000 | **1.250** → 1.005 | **1.253** → 1.005 | 0.564 → 0.422 | **1.668** → 1.005 | **1.102** → 1.002 | 0.825 → 0.776 |

**Results.**
- **All 90 cells are ≤ 1.05** with the prototype; the worst is str n=100 ascending at 1.049. The
  shipped kernel's worst was 1.841.
- Wins on random input are kept or improved: str n=1000 random 0.564 → 0.422, int n=1000 random 0.419.
- Some small random int/float lists lose part of their gain (int n=20 random 0.583 → 0.796): binary
  insertion over pairs is slower than Rust's small-sort on random data. Phase 2 can use a sorting
  network after the run check.
- `min`/`max` cells are unchanged (0.081–0.441).

**Exactness.**
- `benches/floors/sort_diff.py`: 5,000 random cases (int incl. big, float incl. ±0.0, 1/2/4-byte strs
  with equal values as distinct objects, five orders, `reverse=True/False`, n up to 300). The kernel's
  result is compared with CPython's sort **by identity**, for both `list.sort` and `sorted`: **0
  mismatches**.
- The wave-1 test modules (`test_takeover_safe`, `test_builtins`, `test_objsort`, json/copy/float
  targets, accel registry and runtime): 1,437 passed. The one failure is
  `test_json_dumps.py::test_fallbacks_are_counted`, which asserts the old `args` reason for
  `default=` that prototype (b) changes on purpose.

**Still to do in Phase 2.**
- Confirm in wall time on a quiet box: w2-dx's wall ratios were larger than the Ir ratios, e.g. 3.21
  wall vs 1.84 Ir for str n=20 desc.
- Run the same cells on 3.15.1t and 3.14.
- Decide whether str sorting stays native at all. It wins only on random lists of 64+ strings
  (0.42–0.86).

## 7. Candidates (format of the Phase-1 brief)

Scores are this slice's own estimates. I = mean share × (1 − 1/s), in %; F feasibility; K risk;
U upstream overlap; R reach.

| candidate | layer | entry points + mechanism | share mean / max (which) | s | exact subset & what falls back | floor vs current | risks | upstream | reach | prototype (M) | I / F / K / U / R |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **json.dumps lazy `default=`/`cls=JSONEncoder`** | stdlib (json) | existing vectorcall kernel: accept the kwargs, restart if the hook would be called | 0.6% / 18.2% (logging_service), flask 1.1%; plus w2-wl-frameworks' job_worker | 5 | trees inside the encoder's subset; anything needing the hook restarts | 6.7k vs 31–37k | wasted prefix on restarts (score handles it) | none | `default=` very common (logging formatters, web frameworks) | logging_service −18.2% Ir/run, 0 digest changes | 0.5 / 0.95 / 0.05 / 0 / 0.8 |
| **native reconstruct for pinned stdlib classes in deepcopy/asdict** (`uuid.UUID`, `PurePath`, `Fraction`) | stdlib (copy, dataclasses) | copy kernel: `_reconstruct` emulated for exact classes whose `__reduce_ex__`/`__getstate__`/`__setstate__` are the fingerprinted originals | 0.35% / 11% (json_api), isort 0.2% | 2.7 | exact UUID/PosixPath; user subclasses restart | asdict 138.7k → ~52k | memo/`_keep_alive` contents with a user memo; UUID `is_safe` | none | UUIDs in dataclasses/pydantic payloads: common in web apps | cells only | 0.3 / 0.8 / 0.15 / 0 / 0.6 |
| **json `make_encoder` `tp_call` kernel** | stdlib (json) | slot of `c_make_encoder`'s type; `default` called from Rust where `_json` calls it (ruling 15) | 0.12% / 4% (json_api) | 2.2 | encoder configs the Rust encoder supports; others call the original | ~22k vs 38.8k | hook ordering, `markers`, re-entrancy | none | web frameworks' encoders | not prototyped | 0.1 / 0.7 / 0.2 / 0 / 0.7 |
| **allocation-free kernels** (per-thread arena, slices of the PEP 393 buffer) | stdlib (textwrap, urllib, posixpath.relpath/abspath, random, json) | existing kernels | 0.6% / 11% (report_render: textwrap.fill) | 2–3 | unchanged | fill 79.6k → ~26k; `_urlsplit` 6.8k → 3.3k | none (internal) | none | textwrap/urllib usage moderate | profile only | 0.4 / 0.9 / 0.02 / 0 / 0.5 |
| **compiled format plans** (strptime/strftime) | stdlib (datetime) | existing kernels: cache (format, locale) → plan | 0.25% / 3.7% (tz_events) | 2.3 | unchanged (plan keyed by the exact format str and LC_TIME state) | strftime 1.9k → 0.8k, strptime 4.9k → 1.5k | locale changes (already pinned) | none | dates in logs/CSV: common | profile only | 0.15 / 0.85 / 0.05 / 0 / 0.6 |
| **json.dumps escape + direct str write** | stdlib (json) | existing kernel: SIMD escape scan, encode straight into the final str | 0.2% / 5% (csv_etl) | 2.5 | unchanged | 6.6k → 2.6k | none | none | high | profile only | 0.15 / 0.9 / 0.02 / 0 / 0.9 |
| **findCaller line cache; LogRecord inline stores; Formatter type-lookup cache** | stdlib (logging) | existing kernels | 0.15% / 7% (logging_service, the three together) | 2 | unchanged; inline stores need a calibrated layout | findCaller 4.0k → 0.9k; LogRecord 7.6k → 4.7k | layout calibration (§3.14) | none | logging in 40% of packages | profile only | 0.1 / 0.8 / 0.1 / 0 / 0.8 |
| **membership by richcompare slot** (identity, `IntEnum`/`StrEnum`, exact bytes, exact tuples of scalars) | language (`in`, `count`, `index`) | `sq_contains`, `count`/`index` `ml_meth` (existing) | 0.1% / 0.8% (black: tuple needles), pyflakes 0.26% | 2 | classify needle and items by `tp_richcompare` slot; other items → *continue* CPython's loop | 0.33–0.54 per call | free-threaded lists (Phase 2) | none | `type(x) in (A, B)` idiom: very common in tools | identity part prototyped: fallbacks ≤ 1.033 | 0.05 / 0.95 / 0.05 / 0 / 0.9 |
| **float `__format__` fixed precision** (`.Nf`, grouping, width/align) | language (formatting) | `float.__format__` `ml_meth` (existing site) | 0.05% / 0.35% (flask) | 3 | `[[fill]align][sign][width][,_][.prec]f`; others → the original | ~600 vs 1.8k | correct rounding must equal `dtoa` mode 3 (both are correctly rounded) | none | f-strings `:.2f` everywhere | not prototyped | 0.03 / 0.85 / 0.1 / 0 / 1.0 |
| **sort: timsort-adaptive kernel** (never-slower fix) | language | `list.sort` `ml_meth` | fixes a regression up to 1.67 (Ir) / 3.2 (wall, w2-dx) | — | §6 | §6 | none | none | sorting everywhere | §6 cells | must-do |
| **fingerprint v3 + accepted-hash sets + recording CI** | runtime (accel) | install | keeps 12–41 targets on patch releases | — | §5 | — | canonical exception table (§5.3) | — | every user on a non-recorded patch | refusal matrix (M) | must-do |
| **validate on first call** | runtime (accel) | install | install 114–493k → ~15k per group | — | contract §3.2 wording | — | first-call latency spike | — | every process | fork measurements | must-do (with w2-startup) |
| **cost-weighted credits + 256-call window** | runtime (accel) | §5.6 | ≤ 0.1% today | — | — | — | — | — | — | no-suspend runs (M) | low priority |

## 8. Recommendations (ranked)

1. **Fix `list.sort` now** (§6). It is a shipped never-slower violation, and the prototype's cells
   show the way.
2. **Fingerprint v3 + accepted-hash sets + the recording CI job** (§5), before the next CPython patch
   release.
3. **json lazy `default=`** (an hour of work, −18% on a logging service), then **native reconstruct for
   UUID/PurePath** (json_api ~11%), then the `make_encoder` route.
4. **Validate on first call + install hygiene** (§4, with w2-startup).
5. **Floor work on the open targets**, in share order:
   - allocation-free textwrap/urllib/relpath (report_render 11%);
   - json escape and direct write (csv_etl 5%);
   - strftime/strptime plans (tz_events 3.7%, log_analytics 2.7%);
   - logging caches (logging_service ~7%).
6. **Membership by richcompare slot, and `float.__format__` precision**: cheap, broad reach, small
   shares.
7. **Credit system**: cost-weighted credits, a 256-call window, split `deopt` reasons, report
   suspensions. Low priority (≤ 0.1% measured).

**Settled, with reason:**
- containers/builtins scans, `float.repr`, `isoformat`, the pathlib constructors, properties and
  hashes: ≤ 20% left or no share;
- statistics: no share;
- `sorted(key=)` on tiny lists and Python keys: the key calls are the work;
- `sum`/`min`/`max` over generators: the generator is the work;
- user `__deepcopy__` restarts: ruling 15;
- `Logger.level` on enabled calls: a pass-through by design. It costs +630–780 Ir per record when it
  is the only logging target on (1.009–1.014), while the logging group makes the same records
  0.45–0.51 (M). This explains w2-wl-frameworks' "deopt on every call" in job_worker, waitress_flask
  and uvicorn_fastapi. Those services' `json.dumps(default=)` is gap #1.

**Unfinished (marked):**
- The canonical exception table is specified and argued, not implemented. Re-checking all targets on
  3.15.0/3.14.x with v3 is Phase 2.
- The UUID reconstruct and `make_encoder` route are estimated from cells, not prototyped.
- The tuple-needle size in black is assumed, not measured.
- The identity prototype covers free-threaded lists only by falling back.
- Free-threaded and 3.14 cells of the prototypes were not run.

## 9. Reproduce, files

- Build (dev box): `RBOX_SRC=<worktree> rbox w2-floors-proto build`; base kernels: `rbox w2-floors build`.
- Floors: `python benches/floors/floor_cells.py > cells.json && python benches/floors/cg_breakdown.py
  --cells cells.json -o floors.json -j 6`, then `floor_table.py floors.json wl_instr.tsv`.
- Instructions per run: `bash benches/floors/wl_instructions.sh out.tsv <family/name>...` (workload
  venvs with a `.pth` to the build).
- Counters/traces: `WLSTATS=benches/floors/wlstats_run2.py RUSTIQUE_PROTO_TRACE=1 bash
  benches/floors/wlstats.sh run <outdir> <workloads>`. The trace needs `--features proto_trace`; it
  is compiled out otherwise, because its code inflated the fallback paths (§6).
- No-suspend runs: `RUSTIQUE_PROTO_NO_SUSPEND=1 bash benches/floors/wlstats.sh whole|run ...`.
- Prototype cells: `python benches/floors/proto_cells.py contains|keys|json_default|asdict|sort [-k substr]`.
- Patch releases: `python benches/floors/refusals.py` in any interpreter's venv with a `.pth` to the
  build; `python benches/floors/patch_history.py <bare cpython.git>`;
  `bash benches/floors/conformance_all.sh out.json`.
- Install: `benches/floors/install/{install_profile,fp_cost,lazy_cells}.py` (release build).
- Results committed: `benches/floors/results/`. Raw data: `~/rustique-research/w2/floors/`.
