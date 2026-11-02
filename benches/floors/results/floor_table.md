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
