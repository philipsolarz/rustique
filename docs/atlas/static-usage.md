# Atlas: static usage, demand signals, upstream overlap

Phase 1 research, 2026-11-01. Question: which builtins and stdlib entry points do popular packages call, how do
they bind them, where do people already pay for faster replacements, and what upstream will cover anyway.
Static usage measures how often code is written, not where runtime goes; the profiling atlas answers that.
Read the two together. Tags: **measured** (how) and **inferred** (from what).

## Key findings

1. **Reach of replaceable library entry points** (share of top-1000 downloads whose library code uses it; measured, AST scan):
   `re` 66% (599 packages), `os.path` 49%, logging calls 40%, `json.dumps/dump` 32% (338), `json.loads/load` 28% (329),
   `base64/binascii` 28%, `copy.copy` 25%, `copy.deepcopy` 22% (241), datetime formatting 22%, `inspect.signature` 19%,
   `uuid` 19%, datetime parsing 14% (150). Builtin str work is everywhere: method names `split/join/replace/strip/
   startswith/...` reach 87%, f-strings 80% (175k sites), `%`/`.format` on literals 59% (58k sites).
2. **Binding style decides the install mechanism** (measured): 40% of stdlib call/decorator/base-class sites use a name
   captured by `from X import f`. It differs by module. `json.*`, `re.*`, `pickle.*`, `os.path.*`, `time.*` are
   called as module attributes at ≥95% of sites. `copy.deepcopy` is captured at 34% of sites, `uuid.uuid4` 17%,
   `hashlib.md5` 41%, `urllib.parse.*` 63–90%, `dataclasses.*` 68–90%. Datetime parsing and formatting run 100% through
   the class or instance, so the type has to be patched. Swapping the module attribute in `enable()` misses every
   reference captured before `enable()`. Replacing the implementation in place catches every site: `ml_meth` for C
   functions, as Rustique does today, and `__code__` (or the C function the Python wrapper calls) for Python functions.
3. **Most json calls are exact-fast-path friendly** (measured): 80% of `json.dumps` calls pass no keyword arguments,
   and only 9.8% pass `default=`/`cls=`/a `**` splat (111 packages; about 5% without semgrep's generated code).
   98.6% of `json.loads` calls pass no keywords.
   `indent` 5.3% and `sort_keys` 3.5% are the next most common.
4. **`re` patterns are static** (measured): 77% of `re.compile` patterns are string or bytes literals, and 82–84% for
   `re.match`/`re.sub`. Flags are static or absent at 99% of calls. The corpus has 10,881 literal sites and 7,815
   unique patterns (`~/rustique-research/static/re_patterns.jsonl`). 94% of literal sites use no lookaround,
   backreference or conditional.
5. **Demand concentrates on JSON, but nobody ships an exact drop-in** (measured on 3.15.1):
   - orjson has 218M downloads a month and jiter 393M (mostly transitive). ujson, simplejson, msgspec and rapidjson add about 120M.
   - Every fast encoder changes output bytes: float exponents (`1e16` vs `1e+16`, `1e-7` vs `1e-07`), `\/` escaping, separators and `ensure_ascii`. orjson also raises on integers beyond 64 bits.
   - Headroom shown by non-drop-ins: `dumps` 0.13–0.17× (orjson), `loads` 0.33× (orjson, msgspec).
6. **Exact or near-exact drop-ins show large gaps** (measured):
   - copium `deepcopy` 0.095× (3.14).
   - ciso8601 0.016× of `strptime` (3.14). The stdlib's own C `fromisoformat` is already 0.036× of `strptime`.
   - `uuid_utils.uuid4` 0.085×.
   - mypyc-compiled tomli 0.53× of `tomllib`, rtoml 0.09×.
   - quicktions `Fraction` 0.27×.
   - propcache `cached_property` 0.55×.
   - pybase64 0.077× for 64 KB encode, but 0.94× for 48 B.
   - `regex` and `google-re2` are **slower** than `re` (1.8–22×): people adopt them for features (regex) and linear-time guarantees (re2), not for speed.
7. **Upstream (sourced; see §4)**:
   - "Rust for CPython" has no PEP yet; submission is aimed at Dec 2026/Jan 2027. It plans only an optional Rust zlib for 3.16 (Oct 2027). io, json, xml, memoryview and the parser are aspirations for 3.17 (Oct 2028), and Rust becomes required no earlier than 3.18.
   - CPython itself lands C `uuid` in 3.16 (`uuid4` 15×) and a C `strptime` for numeric formats in 3.16.
   - `base64` got 2–3× faster in 3.15, and json is getting incremental C fast paths.
   - The C `deepcopy` was rejected, and nobody plans a new `re` engine.
8. **Priority signal** (inferred from 1–7):
   - Strongest: **json** (`dumps`+`loads`), **re** (headroom still unmeasured) and **copy.deepcopy/copy.copy** (no upstream overlap).
   - Next: **logging** record and format path, `urllib.parse`, and `strptime`/`strftime`. The latter matter for 3.14/3.15 and for non-numeric formats on 3.16.
   - Deprioritize `uuid` (fully covered by 3.16), `base64` (mostly covered by 3.15; large payloads only), zlib (compressed output not identical, R4C), and hashlib/pickle/struct/csv (already C).

## 1. Method

| item | value |
|---|---|
| package list | hugovk top-pypi-packages, 30-day downloads, snapshot `2026-11-01 06:41`; top 1000 |
| artifact per package | latest release: pure-Python wheel (814), else smallest linux-x86_64 wheel (183), else sdist (3); wheels ≥ 25 MB read by HTTP Range (only `.py` members) |
| corpus | 94,307 `.py` files (1.1 GB); 937 packages with parsed library code; 62 have no `.py` (stub-only `types-*`, `nvidia-*`, meta-packages); pyspark skipped (456 MB sdist) |
| parser | CPython 3.15.1 `ast`; 2 parse errors (Python 2 files); `.py` > 5 MB skipped (1 file) |
| categories | `vendored` (path has `_vendor`/`vendor`/`third_party`/`extern`…), `tests` (`test(s)`/`testing` dirs, `test_*.py`, `*_test.py`, `conftest.py`), `other` (docs/examples/benchmarks, sdist `setup.py`), else `lib`. Wheels rarely ship tests (170 packages have any) |
| denominator | 937 packages, 132.4 G downloads per 30 days |
| **reach** | download-weighted package share = Σ downloads of packages with ≥ 1 use in library code / 132.4 G |
| **#pkgs, sites** | unweighted package count and call-site count (library code unless stated) |

**Name resolution.** A per-file alias map is built from every `import`/`from … import` in the file (any scope).
It handles `import json; json.dumps`, `import json as j`, `from json import dumps as d`, `import os.path as osp`,
`from os import path` and `from xml.etree import ElementTree as ET`. When a name is bound both to a third-party
module and to a stdlib one (`try: import simplejson as json except ImportError: import json`), the stdlib binding
wins. Each resolved reference gets a context: call, decorator, base class or other reference. It also gets a
**mode**:
- `from`: the name was captured by `from X import f`.
- `attr`: fetched from a module object at the moment of use.
- `obj`: fetched from a captured class or object, as in `datetime.strptime` after `from datetime import datetime`.

**Approximations.**
- Builtin *method* counts are name-based. Any `.join(...)` on a receiver that is not a resolved import counts, so `.get` includes `session.get` and `.join` includes `Thread.join`. The `literal receiver` column is certain.
- Local shadowing is ignored: `copy = x.copy()` after `import copy` produces a few junk `copy.*` names.
- Logging calls are `logging.<level>()` plus `<…log…>.<level>()` by receiver name.
- Annotation-only `typing` references count as references.
- Call sites are skewed by generated code: kubernetes alone has 33% of `json.dumps` sites, and mlflow/mlflow-skinny ship the same code. Prefer reach and #pkgs over sites.
- Static counts say nothing about how often a call runs or how big its input is.

## 2. Ranked usage

### 2.1 Stdlib modules imported (library code)

`__future__` removed (it does nothing at runtime). Measured.

| # | module | reach | #pkgs | import stmts | #pkgs (tests) | #pkgs (vendored) |
|---|---|---|---|---|---|---|
| 1 | `sys` | 79.4% | 659 | 6181 | 112 | 25 |
| 2 | `collections` | 76.1% | 650 | 15067 | 86 | 18 |
| 3 | `typing` | 73.5% | 666 | 52811 | 81 | 19 |
| 4 | `re` | 67.7% | 610 | 7033 | 61 | 20 |
| 5 | `os` | 67.5% | 626 | 7339 | 118 | 16 |
| 6 | `warnings` | 63.9% | 527 | 4548 | 54 | 21 |
| 7 | `functools` | 61.3% | 531 | 4510 | 58 | 18 |
| 8 | `contextlib` | 48.0% | 383 | 2671 | 71 | 11 |
| 9 | `importlib` | 47.9% | 375 | 1979 | 47 | 15 |
| 10 | `types` | 46.3% | 348 | 2489 | 48 | 20 |
| 11 | `io` | 42.9% | 378 | 1722 | 66 | 17 |
| 12 | `logging` | 41.5% | 407 | 7245 | 48 | 11 |
| 13 | `enum` | 41.3% | 339 | 2243 | 21 | 11 |
| 14 | `datetime` | 40.6% | 372 | 3979 | 52 | 10 |
| 15 | `inspect` | 40.0% | 352 | 2172 | 45 | 12 |
| 16 | `itertools` | 39.8% | 347 | 2154 | 46 | 16 |
| 17 | `threading` | 39.2% | 340 | 1982 | 54 | 7 |
| 18 | `json` | 38.9% | 403 | 6765 | 53 | 8 |
| 19 | `time` | 38.2% | 364 | 2857 | 70 | 7 |
| 20 | `urllib` | 34.4% | 317 | 2396 | 26 | 8 |
| 21 | `abc` | 33.5% | 299 | 2116 | 17 | 7 |
| 22 | `copy` | 33.0% | 309 | 2816 | 47 | 10 |
| 23 | `pathlib` | 32.9% | 319 | 2875 | 63 | 7 |
| 24 | `math` | 31.1% | 261 | 2063 | 37 | 8 |
| 25 | `dataclasses` | 31.0% | 285 | 4706 | 34 | 7 |
| 26 | `operator` | 29.6% | 215 | 1025 | 34 | 11 |
| 27 | `base64` | 25.7% | 243 | 1103 | 18 | 7 |
| 28 | `subprocess` | 25.5% | 208 | 813 | 66 | 7 |
| 29 | `platform` | 25.4% | 225 | 488 | 38 | 6 |
| 30 | `hashlib` | 23.8% | 232 | 954 | 22 | 4 |
| 31 | `weakref` | 23.3% | 175 | 613 | 29 | 8 |
| 32 | `tempfile` | 21.9% | 199 | 841 | 62 | 5 |
| 33 | `struct` | 21.7% | 184 | 661 | 18 | 13 |
| 34 | `textwrap` | 21.6% | 187 | 505 | 45 | 8 |
| 35 | `argparse` | 20.9% | 210 | 655 | 18 | 6 |
| 36 | `random` | 20.4% | 208 | 728 | 43 | 5 |
| 37 | `codecs` | 20.2% | 133 | 255 | 4 | 9 |
| 38 | `string` | 20.0% | 178 | 413 | 28 | 6 |
| 39 | `asyncio` | 19.8% | 204 | 1866 | 33 | 3 |
| 40 | `socket` | 19.5% | 155 | 644 | 28 | 4 |

Ranks 41–60 by reach: `ctypes` 15.9%, `stat` 15.9%, `builtins` 15.6%, `ssl` 15.5%, `decimal` 15.5%, `ast` 15.1%,
`concurrent` 14.9%, `binascii` 13.7%, `email` 13.7%, `ipaddress` 12.8%, `http` 12.5%, `unicodedata` 12.1%,
`numbers` 12.0%, `shlex` 11.1%, `pickle` 10.8%, `contextvars` 10.6%, `pprint` 10.5%, `bisect` 10.4%, `queue` 10.3%,
`atexit` 10.0%. Further down: `zlib` 9.4%, `html` 9.2%, `glob` 9.0%.

### 2.2 Stdlib functions, classes and decorators (calls + decorator uses + base classes; library code)

Top 60 by reach (measured). The full top 120 is in `results/tables.md`.

| # | name | reach | #pkgs | sites | from / attr / obj % | sites (tests) |
|---|---|---|---|---|---|---|
| 1 | `warnings.warn` | 62.1% | 506 | 8287 | 12 / 88 / 0 | 248 |
| 2 | `re.compile` | 61.0% | 509 | 6702 | 1 / 99 / 0 | 294 |
| 3 | `typing.TypeVar` | 46.4% | 352 | 4350 | 91 / 9 / 0 | 85 |
| 4 | `typing.cast` | 42.6% | 361 | 21509 | 87 / 13 / 0 | 326 |
| 5 | `logging.getLogger` | 40.1% | 383 | 6999 | 1 / 99 / 0 | 142 |
| 6 | `contextlib.contextmanager` | 38.1% | 299 | 2252 | 62 / 38 / 0 | 332 |
| 7 | `functools.partial` | 37.9% | 315 | 3959 | 58 / 42 / 0 | 1242 |
| 8 | `os.environ.get` | 35.6% | 317 | 4146 | 0 / 0 / 100 | 298 |
| 9 | `os.path.join` | 35.6% | 320 | 7766 | 2 / 98 / 0 | 1852 |
| 10 | `typing.overload` | 33.4% | 248 | 11129 | 92 / 8 / 0 | 85 |
| 11 | `functools.wraps` | 33.4% | 278 | 1796 | 47 / 53 / 0 | 189 |
| 12 | `enum.Enum` | 32.9% | 275 | 3475 | 73 / 27 / 0 | 54 |
| 13 | `re.match` | 31.0% | 278 | 2825 | 0 / 100 / 0 | 187 |
| 14 | `abc.abstractmethod` | 30.4% | 270 | 7686 | 70 / 30 / 0 | 61 |
| 15 | `io.BytesIO` | 30.1% | 244 | 1413 | 51 / 49 / 0 | 1089 |
| 16 | `json.dumps` | 29.7% | 320 | 10423 | 1 / 99 / 0 | 290 |
| 17 | `os.path.dirname` | 29.4% | 260 | 1793 | 2 / 98 / 0 | 422 |
| 18 | `dataclasses.dataclass` | 28.6% | 261 | 15308 | 90 / 10 / 0 | 174 |
| 19 | `re.sub` | 28.6% | 276 | 1839 | 0 / 100 / 0 | 125 |
| 20 | `functools.lru_cache` | 27.5% | 206 | 1482 | 68 / 32 / 0 | 27 |
| 21 | `pathlib.Path` | 27.0% | 266 | 4756 | 89 / 11 / 0 | 933 |
| 22 | `typing.Protocol` | 26.7% | 195 | 1811 | 97 / 3 / 0 | 19 |
| 23 | `typing.TypedDict` | 26.5% | 177 | 2200 | 97 / 3 / 0 | 5 |
| 24 | `time.time` | 26.3% | 243 | 3903 | 5 / 95 / 0 | 250 |
| 25 | `typing.NamedTuple` | 25.9% | 185 | 1260 | 94 / 6 / 0 | 28 |
| 26 | `collections.defaultdict` | 25.9% | 249 | 2770 | 85 / 15 / 0 | 111 |
| 27 | `os.path.exists` | 25.7% | 226 | 2517 | 2 / 98 / 0 | 277 |
| 28 | `importlib.import_module` | 25.5% | 216 | 1312 | 34 / 66 / 0 | 84 |
| 29 | `json.loads` | 24.9% | 297 | 8124 | 1 / 99 / 0 | 400 |
| 30 | `copy.copy` | 24.8% | 194 | 2007 | 17 / 83 / 0 | 103 |
| 31 | `threading.Lock` | 24.3% | 212 | 1297 | 16 / 84 / 0 | 64 |
| 32 | `os.path.basename` | 22.7% | 202 | 1198 | 1 / 99 / 0 | 76 |
| 33 | `abc.ABC` | 22.5% | 218 | 1767 | 65 / 35 / 0 | 22 |
| 34 | `copy.deepcopy` | 22.2% | 241 | 2896 | 34 / 66 / 0 | 323 |
| 35 | `datetime.timedelta` | 22.0% | 202 | 1469 | 66 / 34 / 0 | 979 |
| 36 | `sys.exit` | 22.0% | 214 | 1348 | 0 / 100 / 0 | 118 |
| 37 | `re.search` | 21.8% | 230 | 1368 | 1 / 99 / 0 | 232 |
| 38 | `io.StringIO` | 21.3% | 178 | 594 | 59 / 41 / 0 | 1684 |
| 39 | `itertools.chain` | 21.1% | 188 | 1291 | 59 / 41 / 0 | 82 |
| 40 | `os.path.isfile` | 20.5% | 177 | 1097 | 1 / 99 / 0 | 98 |
| 41 | `os.path.abspath` | 20.5% | 207 | 1333 | 3 / 97 / 0 | 212 |
| 42 | `collections.namedtuple` | 20.5% | 188 | 885 | 78 / 22 / 0 | 113 |
| 43 | `argparse.ArgumentParser` | 20.3% | 199 | 618 | 10 / 90 / 0 | 18 |
| 44 | `os.path.isdir` | 19.9% | 174 | 1038 | 1 / 99 / 0 | 54 |
| 45 | `re.escape` | 19.6% | 147 | 587 | 1 / 99 / 0 | 554 |
| 46 | `collections.deque` | 19.6% | 168 | 933 | 79 / 21 / 0 | 170 |
| 47 | `inspect.signature` | 18.9% | 183 | 1208 | 12 / 88 / 0 | 128 |
| 48 | `contextlib.suppress` | 18.8% | 154 | 1340 | 43 / 57 / 0 | 40 |
| 49 | `collections.OrderedDict` | 18.7% | 180 | 1464 | 91 / 9 / 0 | 197 |
| 50 | `time.sleep` | 18.3% | 195 | 1154 | 9 / 91 / 0 | 355 |
| 51 | `dataclasses.field` | 18.0% | 155 | 5635 | 90 / 10 / 0 | 110 |
| 52 | `os.getenv` | 17.2% | 182 | 2763 | 1 / 99 / 0 | 73 |
| 53 | `base64.b64encode` | 17.2% | 172 | 833 | 15 / 85 / 0 | 66 |
| 54 | `os.remove` | 16.8% | 125 | 523 | 2 / 98 / 0 | 124 |
| 55 | `textwrap.dedent` | 16.6% | 141 | 682 | 47 / 53 / 0 | 554 |
| 56 | `re.split` | 16.5% | 132 | 284 | 1 / 99 / 0 | 12 |
| 57 | `sys.exc_info` | 16.1% | 140 | 707 | 2 / 98 / 0 | 111 |
| 58 | `datetime.datetime.now` | 16.0% | 208 | 1778 | 0 / 0 / 100 | 109 |
| 59 | `enum.IntEnum` | 15.9% | 111 | 524 | 67 / 33 / 0 | 8 |
| 60 | `os.path.expanduser` | 15.8% | 134 | 549 | 3 / 97 / 0 | 15 |

### 2.3 Binding style per entry point, and what it means for installation

Over all 316,531 resolved stdlib call/decorator/base sites in library code: **attr 55.7%, from 40.3%, obj 4.0%**.
The code has 224,082 `from X import name` statements binding 2,924 distinct stdlib names, and 85,379 `import X`
statements. Measured.

| name | sites | from-import | module attr | via class/object | `from X import f` stmts |
|---|---|---|---|---|---|
| `json.dumps` | 10423 | 0.6% | 99.4% | 0.0% | 40 |
| `json.loads` | 8124 | 0.7% | 99.3% | 0.0% | 38 |
| `copy.deepcopy` | 2896 | 34.0% | 66.0% | 0.0% | 463 |
| `copy.copy` | 2007 | 16.9% | 83.1% | 0.0% | 174 |
| `re.compile` | 6702 | 0.6% | 99.4% | 0.0% | 22 |
| `re.match` | 2825 | 0.1% | 99.9% | 0.0% | 5 |
| `re.sub` | 1839 | 0.4% | 99.6% | 0.0% | 8 |
| `re.search` | 1368 | 0.9% | 99.1% | 0.0% | 9 |
| `uuid.uuid4` | 1446 | 17.3% | 82.7% | 0.0% | 162 |
| `uuid.UUID` | 216 | 39.4% | 60.6% | 0.0% | 142 |
| `datetime.datetime.strptime` | 261 | 0.0% | 0.0% | 100.0% | 0 |
| `datetime.datetime.fromisoformat` | 229 | 0.0% | 0.0% | 100.0% | 0 |
| `time.strftime` | 96 | 4.2% | 95.8% | 0.0% | 3 |
| `base64.b64encode` | 833 | 15.2% | 84.8% | 0.0% | 87 |
| `base64.b64decode` | 629 | 12.6% | 87.4% | 0.0% | 61 |
| `hashlib.sha256` | 645 | 11.6% | 88.4% | 0.0% | 71 |
| `hashlib.md5` | 249 | 41.4% | 58.6% | 0.0% | 66 |
| `pickle.dumps` | 637 | 1.7% | 98.3% | 0.0% | 5 |
| `struct.unpack` | 1278 | 24.1% | 75.9% | 0.0% | 38 |
| `urllib.parse.urlparse` | 1458 | 62.6% | 37.4% | 0.0% | 564 |
| `urllib.parse.quote` | 656 | 66.2% | 33.8% | 0.0% | 253 |
| `urllib.parse.urlencode` | 1021 | 89.8% | 10.2% | 0.0% | 227 |
| `os.path.join` | 7766 | 1.7% | 98.3% | 0.0% | 45 |
| `dataclasses.asdict` | 355 | 68.2% | 31.8% | 0.0% | 172 |
| `dataclasses.replace` | 893 | 74.2% | 25.8% | 0.0% | 231 |
| `dataclasses.dataclass` | 15308 | 89.8% | 10.2% | 0.0% | 3592 |
| `inspect.signature` | 1208 | 12.1% | 87.9% | 0.0% | 99 |
| `functools.wraps` | 1796 | 46.9% | 53.1% | 0.0% | 497 |
| `functools.lru_cache` | 1482 | 67.7% | 32.3% | 0.0% | 510 |
| `functools.partial` | 3959 | 58.1% | 41.9% | 0.0% | 930 |
| `functools.cached_property` | 1281 | 61.0% | 39.0% | 0.0% | 282 |
| `collections.defaultdict` | 2770 | 84.7% | 15.3% | 0.0% | 1175 |
| `itertools.chain` | 1291 | 58.9% | 41.1% | 0.0% | 493 |
| `textwrap.dedent` | 682 | 47.1% | 52.9% | 0.0% | 116 |
| `shlex.quote` | 286 | 15.0% | 85.0% | 0.0% | 21 |
| `html.escape` | 283 | 29.3% | 70.7% | 0.0% | 45 |
| `zlib.decompress` | 65 | 0.0% | 100.0% | 0.0% | 0 |
| `difflib.SequenceMatcher` | 26 | 65.4% | 34.6% | 0.0% | 14 |
| `xml.etree.ElementTree.fromstring` | 77 | 5.2% | 94.8% | 0.0% | 5 |
| `csv.writer` | 67 | 3.0% | 97.0% | 0.0% | 2 |
| `typing.get_type_hints` | 161 | 73.3% | 26.7% | 0.0% | 91 |

Implications (inferred from the table and from how CPython binds names):
- **Module-attribute swap is enough** for `json`, `re`, `pickle`, `os.path`, `time`, `zlib`, `csv` and `xml.etree` (≥ 93% attr).
- **The implementation must be swapped in place** wherever capture is common: `copy.deepcopy` (34% from), `uuid.uuid4/UUID` (17–39%), `hashlib.md5` (41%), `urllib.parse` (63–90%), `dataclasses` (68–90%), `functools` (47–68%).
- For C functions this is the existing `PyMethodDef.ml_meth` swap. For pure-Python entry points (`copy.deepcopy`, `urllib.parse.*`, `dataclasses.asdict/replace`, `base64.*`, `uuid.uuid4` before 3.16), replace the function's `__code__` with a shim that calls the kernel. That keeps the function's identity, so captured references see the change. The alternative is enabling from a `.pth` before user imports (startup cost and policy: **needs a decision**).
- `datetime.strptime/fromisoformat/strftime/isoformat` are reached 100% through the class or instance. That needs type-level method-table swaps (static types, as for `list.sort` today).

### 2.4 Builtin functions called (library code; by call sites)

Measured. Exception constructors appear because `raise ValueError(...)` is a call.

| # | builtin | reach | #pkgs | sites |
|---|---|---|---|---|
| 1 | `isinstance` | 86.6% | 805 | 181272 |
| 2 | `len` | 83.3% | 788 | 108697 |
| 3 | `ValueError` | 76.7% | 685 | 63134 |
| 4 | `getattr` | 72.7% | 648 | 60901 |
| 5 | `super` | 72.9% | 654 | 58630 |
| 6 | `str` | 76.3% | 711 | 38897 |
| 7 | `dict` | 59.6% | 527 | 28977 |
| 8 | `type` | 69.3% | 608 | 27894 |
| 9 | `list` | 72.0% | 639 | 25279 |
| 10 | `range` | 61.5% | 537 | 24302 |
| 11 | `int` | 69.6% | 634 | 23153 |
| 12 | `tuple` | 61.3% | 541 | 21255 |
| 13 | `set` | 69.2% | 593 | 21245 |
| 14 | `hasattr` | 69.6% | 617 | 19932 |
| 15 | `enumerate` | 58.0% | 505 | 13669 |
| 16 | `NotImplementedError` | 49.4% | 396 | 13569 |
| 17 | `TypeError` | 63.9% | 537 | 13126 |
| 18 | `callable` | 37.6% | 359 | 11626 |
| 19 | `print` | 41.9% | 378 | 11167 |
| 20 | `AssertionError` | 19.8% | 150 | 10843 |
| 21 | `sorted` | 60.9% | 510 | 10073 |
| 22 | `any` | 58.4% | 464 | 9483 |
| 23 | `max` | 51.1% | 448 | 9482 |
| 24 | `zip` | 41.2% | 391 | 9314 |
| 25 | `float` | 41.8% | 365 | 8104 |
| 26 | `RuntimeError` | 48.9% | 385 | 8087 |
| 27 | `setattr` | 48.3% | 391 | 7664 |
| 28 | `bool` | 57.4% | 483 | 7124 |
| 29 | `min` | 43.2% | 392 | 6639 |
| 30 | `frozenset` | 41.5% | 305 | 6530 |

### 2.5 Builtin-type methods by name (library code; by call sites)

Name-based, so receiver types are approximate (see §1). memoryview-only names (`cast`, `tolist`, `release`, …) are
dropped because their counts are dominated by torch/numpy objects. Measured.

| # | method | builtin types with it | reach | #pkgs | sites | literal receiver |
|---|---|---|---|---|---|---|
| 1 | `.get` | dict | 78.4% | 729 | 152733 | 313 |
| 2 | `.append` | bytearray, list | 81.9% | 744 | 102566 | 0 |
| 3 | `.items` | dict | 73.7% | 679 | 37768 | 126 |
| 4 | `.format` | str, memoryview | 57.8% | 546 | 32190 | 25588 |
| 5 | `.pop` | bytearray, list, dict, set | 67.5% | 602 | 31577 | 1 |
| 6 | `.join` | str, bytes, bytearray | 78.3% | 710 | 29587 | 12505 |
| 7 | `.update` | dict, set | 65.4% | 592 | 18513 | 0 |
| 8 | `.split` | str, bytes, bytearray | 72.4% | 652 | 16883 | 397 |
| 9 | `.add` | set | 59.2% | 508 | 15338 | 0 |
| 10 | `.startswith` | str, bytes, bytearray | 65.9% | 593 | 14440 | 8 |
| 11 | `.extend` | bytearray, list | 60.4% | 495 | 13057 | 0 |
| 12 | `.lower` | str, bytes, bytearray | 61.7% | 548 | 11590 | 50 |
| 13 | `.decode` | bytes, bytearray | 60.9% | 552 | 11587 | 57 |
| 14 | `.strip` | str, bytes, bytearray | 61.7% | 548 | 11004 | 124 |
| 15 | `.replace` | str, bytes, bytearray | 60.1% | 554 | 10949 | 87 |
| 16 | `.values` | dict | 59.5% | 489 | 10346 | 38 |
| 17 | `.encode` | str | 58.9% | 514 | 9382 | 352 |
| 18 | `.keys` | dict | 53.4% | 476 | 8416 | 15 |
| 19 | `.copy` | bytearray, list, dict, set | 48.7% | 419 | 7630 | 1 |
| 20 | `.setdefault` | dict | 43.8% | 367 | 5344 | 1 |
| 21 | `.endswith` | str, bytes, bytearray | 56.7% | 456 | 5217 | 1 |
| 22 | `.clear` | bytearray, list, dict, set | 42.6% | 352 | 3761 | 0 |
| 23 | `.rstrip` | str, bytes, bytearray | 46.3% | 370 | 3124 | 49 |
| 24 | `.remove` | bytearray, list, set | 41.3% | 334 | 3056 | 0 |
| 25 | `.upper` | str, bytes, bytearray | 43.3% | 348 | 2987 | 18 |
| 26 | `.find` | str, bytes, bytearray | 35.2% | 310 | 2289 | 2 |
| 27 | `.index` | str, bytes, bytearray, list | 32.6% | 296 | 2223 | 33 |
| 28 | `.insert` | bytearray, list | 41.4% | 306 | 2184 | 0 |
| 30 | `.sort` | list | 33.1% | 269 | 1736 | 0 |
| 32 | `.splitlines` | str, bytes, bytearray | 32.4% | 252 | 1561 | 7 |
| 34 | `.lstrip` | str, bytes, bytearray | 36.8% | 298 | 1519 | 19 |
| 35 | `.count` | str, bytes, bytearray, list | 29.4% | 238 | 1488 | 47 |
| 36 | `.partition` | str, bytes, bytearray | 28.9% | 212 | 1149 | 1 |
| 37 | `.fromkeys` | dict | 17.9% | 152 | 1077 | 3 |
| 38 | `.discard` | set | 22.6% | 166 | 1011 | 0 |
| 39 | `.union` | set, frozenset | 11.5% | 123 | 1000 | 17 |
| 40 | `.rsplit` | str, bytes, bytearray | 28.7% | 237 | 970 | 0 |
| 41 | `.isdigit` | str, bytes, bytearray | 23.5% | 211 | 963 | 0 |
| 43 | `.format_map` | str | 2.1% | 21 | 781 | 748 |
| 44 | `.intersection` | set, frozenset | 13.9% | 124 | 708 | 20 |

Target-relevant names on any receiver (name-based; measured):
- datetime: `.strftime` 771 sites in 142 packages; `.isoformat` 892/158; `.timestamp` 671/101.
- compiled-pattern methods: `.match` 4,441/407, `.group` 6,497/392, `.sub` 1,415/224, `.search` 1,773/281, `.groupdict` 1,403/124.
- hash objects: `.hexdigest` 977/182.
- pathlib: `.resolve` 1,606/186, `.exists` 1,732/192, `.read_text` 534/115.

### 2.6 String formatting and logging style (library code)

| construct | reach | #pkgs | sites |
|---|---|---|---|
| fstring | 79.5% | 717 | 175298 |
| percent_str | 38.8% | 395 | 30290 |
| percent_bytes | 10.0% | 49 | 297 |
| format_strlit | 47.6% | 440 | 27586 |
| format_other | 40.7% | 358 | 4604 |
| concat_strlit | 66.7% | 647 | 32472 |
| tstring | 0.0% | 0 | 0 |

f-strings are 75% of literal formatting sites (fstring vs `%` on a str literal vs `.format` on a str literal). The
interpreter compiles f-strings to `FORMAT_SIMPLE`/`FORMAT_WITH_SPEC`/`BUILD_STRING`, so there is no method to swap.
The only levers are `int/float/str.__format__` and `PyUnicode_Format`/`str.format` (inferred from CPython's compiler).
`%`- and `.format`-formatting still cover 58k sites in 557 packages.

Logging message style (`logging.x()` and `<…log…>.x()` calls):

| style | reach | #pkgs | sites |
|---|---|---|---|
| lazy_percent | 32.7% | 304 | 19285 |
| plain | 29.9% | 299 | 15257 |
| fstring | 12.0% | 157 | 12667 |
| other_expr | 20.0% | 220 | 2488 |
| eager_format | 4.0% | 73 | 612 |
| eager_percent | 5.3% | 56 | 392 |
| concat | 4.8% | 56 | 247 |
| plain_with_pct | 0.5% | 6 | 14 |
| nomsg | 0.3% | 7 | 8 |

38% of logging calls defer `%` formatting (`lazy_percent`) and 25% build an f-string eagerly even when the level is
off (measured). The hot path in libraries is therefore `Logger.isEnabledFor` plus, when the call is enabled,
`LogRecord.__init__` and `Formatter.format`/`LogRecord.getMessage`. All of that is pure Python in `logging/__init__.py`
(inferred from source).

### 2.7 json calls and their keyword arguments (library code)

| func | calls | #pkgs | no kwargs | hook/cls/** | indent | ** | sort_keys | separators | ensure_ascii | default | cls | allow_nan | exclude_readonly | strict | object_hook | object_pairs_hook | parse_constant | parse_float |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `json.dumps` | 10423 | 320 | 80.2% | 9.8% (111 pkgs) | 5.3% | 5.2% | 3.5% | 3.4% | 3.3% | 2.7% | 2.0% | 0.6% | 0.4% |  |  |  |  |  |
| `json.dump` | 252 | 87 | 41.3% | 16.3% (23 pkgs) | 43.7% | 4.0% | 7.5% | 2.8% | 6.0% | 4.0% | 8.7% | 0.8% |  |  |  |  |  |  |
| `json.loads` | 8124 | 297 | 98.6% | 1.0% (49 pkgs) |  | 0.4% |  |  |  |  | 0.3% |  |  | 0.4% | 0.1% | 0.1% | 0.1% | 0.1% |
| `json.load` | 542 | 137 | 96.9% | 3.0% (12 pkgs) |  | 1.7% |  |  |  |  | 0.4% |  |  |  | 0.7% | 0.9% |  | 0.7% |

Measured. 92% of the `**` splats are semgrep's generated code (497 of 539 sites); without semgrep, hook/cls/** is
about 5% of `json.dumps` calls. `default=` occurs in 61 packages (litellm has 39% of those sites) and `cls=` in 24.
`exclude_readonly` (azure-keyvault-*, azure-monitor) is passed through to a `cls=` encoder. In
tests (170 packages), `json.dumps` has 290 sites and `json.loads` 400.

### 2.8 `re` calls, pattern literals and flags (library code)

| func | calls | literal str | literal bytes | f-string | name/attr | other expr |
|---|---|---|---|---|---|---|
| `re.compile` | 6702 | 73.9% | 3.4% | 4.6% | 7.7% | 10.4% |
| `re.match` | 2825 | 82.2% | 1.0% | 1.3% | 12.3% | 3.3% |
| `re.sub` | 1839 | 83.8% | 0.6% | 2.1% | 9.4% | 4.1% |
| `re.search` | 1368 | 65.9% | 1.2% | 3.5% | 24.8% | 4.7% |
| `re.escape` | 587 | 4.8% | 0.2% | 0.5% | 80.7% | 13.8% |
| `re.findall` | 460 | 68.9% | 0.4% | 5.2% | 22.6% | 2.8% |
| `re.split` | 284 | 81.7% | 2.8% | 1.1% | 12.3% | 2.1% |
| `re.fullmatch` | 276 | 60.1% | 1.1% | 1.1% | 34.4% | 3.3% |
| `re.finditer` | 217 | 51.2% | 1.4% | 4.6% | 35.5% | 7.4% |
| `re.subn` | 11 | 81.8% | 0.0% | 0.0% | 18.2% | 0.0% |
| flag | sites | #pkgs |
|---|---|---|
| `re.IGNORECASE` | 677 | 171 |
| `re.VERBOSE` | 355 | 119 |
| `re.I` | 324 | 74 |
| `re.DOTALL` | 298 | 100 |
| `re.MULTILINE` | 270 | 82 |
| `re.UNICODE` | 117 | 57 |
| `re.ASCII` | 89 | 35 |
| `re.M` | 89 | 26 |
| `re.X` | 52 | 19 |
| `re.S` | 36 | 19 |
| `re.U` | 33 | 16 |
| `re.A` | 3 | 1 |
| `re.RegexFlag` | 1 | 1 |
Flags argument: dynamic 126, none 11843, static 2013

Literal-pattern corpus, measured with `re._parser` on 3.15.1 (`scripts/re_corpus.py`); library sites only:

| metric | value |
|---|---|
| sites with a literal pattern (all categories / library) | 12,190 / 10,881 |
| unique library patterns | 7,815; length p50 21, p90 76, max 18,644 chars |
| no lookahead/lookbehind/backreference/conditional/atomic/possessive | 10,272 (94.4%) |
| lookahead / lookbehind / backreference | 384 / 240 / 101 |
| named groups / alternation / lazy quantifier | 1,960 / 1,900 / 1,831 |
| `\s` / `\d` / `\w` (Unicode semantics for str) | 1,841 / 1,675 / 772 |
| IGNORECASE / ASCII flag / bytes pattern | 706 / 79 / 301 |
| pure literal (could be `str.replace`/`in`) | 260 |

Each corpus row (`~/rustique-research/static/re_patterns.jsonl`) holds `pattern`, `bytes`, `func`, `flags`,
`flags_static`, `pkg`, `downloads`, `category`, `file`, `line`, `features`, `groups`. Patterns are kept per site,
so they can be weighted by downloads.

### 2.9 Requested modules: drill-down (library code)

`#pkgs referencing` counts any resolved reference (calls, decorators, bases, attribute reads). Each entry in the
top-names column is `name #pkgs/sites`. Measured.

| module | #pkgs referencing | reach | top names (#pkgs / sites) |
|---|---|---|---|
| `json` | 400 | 38.6% | `dumps` 330/10489, `loads` 310/8242, `load` 138/544, `JSONDecodeError` 109/922, `dump` 87/253, `JSONEncoder` 64/117, `decoder.JSONDecodeError` 38/289, `JSONDecoder` 20/32, `JSONEncoder.default` 16/23, `get` 5/17 |
| `re` | 609 | 67.7% | `compile` 513/6725, `match` 279/2837, `sub` 276/1842, `search` 232/1376, `IGNORECASE` 186/839, `escape` 152/648, `Pattern` 138/823, `split` 132/285, `VERBOSE` 127/385, `DOTALL` 110/415 |
| `copy` | 309 | 33.0% | `deepcopy` 242/2908, `copy` 198/2015, `error` 1/3, `Error` 1/1, `set_source_expressions` 1/2, `as_sql` 1/1, `get_source_expressions` 1/1, `__file__` 1/1, `_get` 1/1, `_execute` 1/1 |
| `pickle` | 105 | 10.8% | `dumps` 60/645, `loads` 46/152, `dump` 41/100, `load` 39/101, `HIGHEST_PROTOCOL` 30/65, `PicklingError` 25/88, `Pickler` 16/26, `UnpicklingError` 15/75, `Unpickler` 12/19, `DEFAULT_PROTOCOL` 10/11 |
| `datetime` | 370 | 40.6% | `datetime` 275/9496, `timedelta` 229/4697, `datetime.now` 213/1799, `timezone.utc` 200/1585, `date` 159/2936, `datetime.fromtimestamp` 136/398, `time` 105/621, `datetime.strptime` 91/264, `datetime.fromisoformat` 75/235, `tzinfo` 60/336 |
| `time` | 363 | 38.2% | `time` 247/3989, `sleep` 198/1185, `monotonic` 104/1650, `perf_counter` 63/568, `localtime` 47/104, `strftime` 41/97, `gmtime` 37/94, `mktime` 36/66, `time_ns` 26/189, `struct_time` 19/38 |
| `uuid` | 212 | 18.6% | `uuid4` 163/1459, `UUID` 103/1503, `uuid1` 14/29, `uuid5` 12/28, `getnode` 3/3, `SafeUUID.unknown` 3/3, `NAMESPACE_OID` 3/13, `uuid7` 2/2, `_generate_time_safe` 2/2, `_UuidCreate` 2/2 |
| `logging` | 406 | 41.5% | `getLogger` 386/7009, `DEBUG` 172/7689, `INFO` 126/435, `Logger` 109/898, `StreamHandler` 107/178, `Formatter` 87/174, `WARNING` 85/263, `basicConfig` 85/142, `ERROR` 69/159, `NullHandler` 59/86 |
| `pathlib` | 317 | 32.8% | `Path` 303/12738, `PurePath` 64/280, `Path.cwd` 61/225, `Path.home` 50/170, `PurePosixPath` 36/410, `PureWindowsPath` 19/40, `PosixPath` 9/14, `WindowsPath` 9/14, `Path.open` 4/9, `Path.is_file` 4/5 |
| `os.path` | 462 | 49.0% | `join` 323/7788, `dirname` 262/1800, `exists` 228/2533, `abspath` 209/1352, `basename` 204/1210, `isfile` 180/1118, `isdir` 177/1059, `expanduser` 134/554, `splitext` 132/680, `normpath` 122/499 |
| `enum` | 339 | 41.3% | `Enum` 299/4037, `IntEnum` 115/550, `auto` 85/1867, `IntFlag` 32/61, `unique` 30/94, `EnumMeta` 25/51, `Flag` 16/33, `StrEnum` 15/46, `Enum.__repr__` 3/3, `Enum.__new__` 2/2 |
| `dataclasses` | 283 | 30.9% | `dataclass` 261/15318, `field` 155/5711, `fields` 79/335, `asdict` 75/370, `replace` 67/894, `is_dataclass` 60/254, `MISSING` 23/102, `InitVar` 18/40, `Field` 17/54, `field.name` 14/30 |
| `typing` | 661 | 72.6% | `Any` 575/193340, `TYPE_CHECKING` 399/17642, `Union` 387/133624, `cast` 361/21517, `TypeVar` 354/4508, `Optional` 330/245528, `Literal` 304/17752, `Callable` 298/22144, `Tuple` 257/42588, `Dict` 254/67288 |
| `urllib.parse` | 288 | 30.2% | `urlparse` 189/1460, `quote` 126/680, `unquote` 114/647, `urlencode` 97/1023, `urlsplit` 95/395, `parse_qs` 84/244, `urlunparse` 79/247, `urljoin` 76/262, `urlunsplit` 55/115, `parse_qsl` 50/103 |
| `base64` | 243 | 25.7% | `b64encode` 172/837, `b64decode` 142/640, `urlsafe_b64encode` 69/148, `urlsafe_b64decode` 48/104, `encodebytes` 25/52, `decodebytes` 21/52, `standard_b64encode` 18/29, `standard_b64decode` 14/23, `b32encode` 8/10, `encodestring` 6/6 |
| `hashlib` | 230 | 22.4% | `sha256` 154/788, `sha1` 88/280, `md5` 84/289, `sha512` 34/60, `new` 26/52, `sha384` 16/30, `blake2b` 15/23, `_Hash` 10/15, `pbkdf2_hmac` 7/12, `algorithms_available` 7/9 |
| `struct` | 183 | 21.7% | `unpack` 129/1305, `pack` 129/1420, `calcsize` 50/158, `error` 35/133, `Struct` 34/184, `unpack_from` 27/206, `pack_into` 6/28, `iter_unpack` 2/2 |
| `csv` | 71 | 4.8% | `reader` 42/56, `writer` 35/67, `DictReader` 16/23, `DictWriter` 14/27, `QUOTE_MINIMAL` 8/22, `QUOTE_NONE` 6/10, `Error` 6/7, `Sniffer` 4/5, `excel` 4/4, `register_dialect` 3/3 |
| `xml.etree.ElementTree` | 67 | 6.0% | `Element` 47/909, `fromstring` 34/78, `tostring` 28/47, `SubElement` 26/251, `ParseError` 21/32, `register_namespace` 15/48, `ElementTree` 13/41, `parse` 11/14, `TreeBuilder` 7/9, `XMLParser` 5/8 |
| `tomllib` | 37 | 3.7% | `load` 20/25, `loads` 19/32, `TOMLDecodeError` 10/11, `dumps` 1/2 |
| `difflib` | 63 | 6.5% | `get_close_matches` 31/43, `unified_diff` 23/28, `SequenceMatcher` 21/29, `ndiff` 4/6, `SequenceMatcher.get_matching_blocks` 1/1, `context_diff` 1/1, `Match` 1/4, `SequenceMatcher.__init__` 1/1 |
| `ipaddress` | 88 | 12.8% | `ip_address` 59/127, `IPv6Address` 56/178, `IPv4Address` 49/166, `IPv6Network` 32/116, `IPv4Network` 28/153, `ip_network` 17/44, `IPv4Interface` 13/40, `IPv6Interface` 13/40, `AddressValueError` 13/25, `ip_interface` 4/4 |
| `textwrap` | 186 | 21.5% | `dedent` 141/682, `indent` 55/164, `wrap` 38/72, `fill` 29/62, `TextWrapper` 16/19, `shorten` 3/3, `TextWrapper._split` 1/1, `TextWrapper.__init__` 1/1 |
| `shlex` | 95 | 11.1% | `split` 67/196, `quote` 45/294, `join` 19/70, `shlex` 11/14, `_ShlexInstream` 1/1 |
| `fnmatch` | 89 | 8.3% | `fnmatch` 59/152, `filter` 23/29, `translate` 19/26, `fnmatchcase` 11/26 |
| `glob` | 94 | 9.0% | `glob` 83/311, `iglob` 19/28, `escape` 11/23, `has_magic` 4/26, `glob.has_magic` 1/1, `glob1` 1/1, `encode` 1/1 |
| `inspect` | 350 | 39.8% | `signature` 184/1226, `isclass` 126/523, `iscoroutinefunction` 101/437, `isfunction` 88/246, `ismethod` 70/178, `Parameter.POSITIONAL_OR_KEYWORD` 59/175, `getfullargspec` 59/92, `Parameter.VAR_KEYWORD` 56/129, `isawaitable` 56/259, `currentframe` 55/123 |
| `functools` | 528 | 60.6% | `partial` 323/4216, `wraps` 279/1802, `lru_cache` 207/1489, `reduce` 116/690, `cached_property` 107/1316, `cache` 92/690, `update_wrapper` 65/152, `total_ordering` 46/93, `singledispatch` 28/70, `cmp_to_key` 21/46 |
| `collections` | 645 | 75.9% | `abc.Iterable` 276/5605, `abc.Mapping` 273/15066, `abc.Sequence` 267/17016, `abc.Callable` 264/15723, `defaultdict` 255/3257, `abc.Iterator` 216/3584, `OrderedDict` 204/1859, `namedtuple` 190/894, `deque` 183/1446, `abc.Generator` 139/1416 |
| `itertools` | 344 | 39.0% | `chain` 190/1313, `chain.from_iterable` 119/621, `islice` 112/358, `count` 93/471, `product` 83/432, `zip_longest` 74/181, `groupby` 64/185, `repeat` 47/187, `cycle` 47/124, `tee` 34/61 |
| `asyncio` | 201 | 18.9% | `get_running_loop` 104/671, `sleep` 100/629, `CancelledError` 87/953, `gather` 78/543, `create_task` 70/760, `AbstractEventLoop` 66/497, `get_event_loop` 66/302, `TimeoutError` 64/392, `Future` 63/923, `wait_for` 59/337 |
| `html` | 89 | 9.0% | `escape` 60/293, `parser.HTMLParser` 21/27, `unescape` 18/36, `entities.name2codepoint` 7/14, `parser.HTMLParser.__init__` 6/8, `entities.html5.items` 4/4, `entities.codepoint2name.items` 2/2, `entities.codepoint2name` 2/3, `entities.name2codepoint.get` 2/2, `parser.HTMLParser.reset` 2/2 |
| `zlib` | 92 | 8.9% | `compress` 45/69, `decompress` 40/71, `decompressobj` 35/62, `error` 31/70, `MAX_WBITS` 31/56, `compressobj` 21/29, `crc32` 15/28, `DEFLATED` 9/10, `Z_SYNC_FLUSH` 5/5, `adler32` 5/9 |

### 2.10 Library vs tests vs vendored

- **Tests.** Only 170 packages ship tests in their wheel or sdist (8,267 files). Tests favour `os.path.join` (1,852
  sites), `io.StringIO` (1,684), `functools.partial` (1,242), `warnings.catch_warnings` (1,239), `io.BytesIO`
  (1,089), `datetime.timedelta` (979) and `datetime.datetime` (6,519), so they lean much harder on
  datetime/io/warnings than library code does (measured).
- **Vendored.** 31 packages vendor code (1,163 files). The largest are pip (250 files), debugpy (207), setuptools
  (90), torch `third_party` (85), scikit-learn `externals` (75) and poetry-core (72). Common vendored libraries are
  packaging, tomli and platformdirs. Their module mix matches library code (`#pkgs (vendored)` column in §2.1).

## 3. Demand signals: third-party accelerators

### 3.1 Downloads and drop-in status

Downloads are for 30 days, from the top-pypi dataset (2026-11-01); entries marked `p` come from pypistats.org
`last_month`. Many large numbers are transitive (requests→charset-normalizer, jinja2→markupsafe,
aiohttp→multidict/yarl/frozenlist/propcache, httpx→h11, openai/anthropic→jiter, botocore→python-dateutil).
`imports` = number of top-1000 packages whose library code imports the module (measured).

| package | downloads/mo | competes with | drop-in? | speed vs stdlib | imports |
|---|---|---|---|---|---|
| jiter | 393.4M | `json.loads` | parse-only API (bytes in); same objects out on test doc | loads 0.38–0.41× (m) | 2 |
| regex | 367.9M | `re` | near-superset API; V0 mode aims at `re` compatibility, differs in edge cases | **1.8–3.0× slower** (m) | 10 |
| uvloop | 281.0M | asyncio event loop | via policy / `uvloop.run`; scheduling details differ | claimed 2–4× (README) | 9 |
| tomli | 273.8M | `tomllib` | same code as tomllib, mypyc-compiled wheels | loads 0.53× (m) | 34 |
| orjson | 218.5M | `json.dumps/loads` | no: bytes out, compact, no `ensure_ascii`, `1e16`/`1e-7` float form, raises on > 64-bit ints | dumps 0.13–0.17×, loads 0.33× (m) | 20 |
| markupsafe | 816.3M | `html.escape` | no: returns `Markup`, `&#34;`/`&#39;` instead of `&quot;`/`&#x27;` | 0.40× (2 KB), 0.80× (120 B) (m) | 9 |
| propcache | 519.2M | `functools.cached_property` | near (C descriptor; same value on test; edge semantics not checked) | 0.55× new instance + first access (m) | 3 |
| python-dateutil | 1126.1M | `datetime.strptime/fromisoformat` | no (lenient parser) | `isoparse` 1.16× (slower than `strptime`) (m) | 43 |
| lxml | 413.5M | `xml.etree.ElementTree` | mostly API-compatible, differs in details | not measured | 27 |
| yarl | 540.1M | `urllib.parse` | no (URL objects) | not measured | 18 |
| rapidfuzz | 133.8M | `difflib.SequenceMatcher.ratio`, `get_close_matches` | no (InDel metric, different scores) | 0.007× (m) | 5 |
| xxhash / blake3 | 160.9M / 17.3M | `hashlib` | no (different hash functions) | 0.065× / 0.52× of sha256 at 64 KB (m) | 10 / 0 |
| uuid-utils | 84.9M | `uuid.uuid4`, `UUID` | `uuid_utils` has its own UUID type; `uuid_utils.compat` returns stdlib `UUID` | uuid4 0.085× (own type), 0.35× (compat); `UUID(str)` 0.17× (m) | 4 |
| fastuuid | 80.5M | `uuid` | own types | uuid4 0.33×, `UUID(str)` 0.13× (m, 3.14) | 1 |
| uuid6 + uuid7 | 21.9M | `uuid.uuid7` (stdlib since 3.14) | API-compatible | not measured | 0 |
| pendulum / arrow | 56.7M / 71.1M | `datetime` | no (subclass / own type) | `pendulum.parse` 0.82× of `strptime` (m) | 4 / 3 |
| simplejson | 49.2M | `json` | yes (json's ancestor) | **2.2–2.8× slower** dumps, 0.93–1.36× loads (m) | 5 |
| ijson | 46.6M | none (streaming JSON) | — | — | 1 |
| msgspec | 37.3M | `json`, `dataclasses` | no (bytes, `1e16` floats, typed structs) | encode 0.17×, decode 0.34× (m) | 4 |
| ujson | 30.1M | `json` | no (`\/` escaping, `1e-7`, defaults) | dumps 0.44×, loads 0.52–0.54× (m) | 5 |
| pybase64 | 21.3M | `base64.b64encode/b64decode` | yes (same API and bytes) | encode 0.077× at 64 KB, 0.94× at 48 B; decode 0.76× at 64 KB (m) | 1 |
| google-re2 | 21.0M | `re` | no (RE2 subset, no backreferences or lookaround) | **2.8× slower** findall, 22× slower short match (m, 3.14) | 2 |
| ciso8601 | 6.6M | `datetime.fromisoformat` / `strptime` | yes for ISO 8601 input | 0.016× of `strptime`, 0.65× of `fromisoformat` (m, 3.14) | 1 |
| python-rapidjson | 4.1M | `json` | close: identical bytes on the test doc with `ensure_ascii=False`; defaults differ | dumps 0.60–0.96× (m) | 1 |
| zlib-ng | 3.2M | `zlib` | same API; **compressed bytes differ**; decompress identical | compress 0.88×, decompress 0.42× (m, 3.14) | 0 |
| isal | 2.8M | `zlib`, `gzip` | same API; different levels and compressed bytes | compress 0.065× (isal level 2 vs zlib level 6, not comparable), decompress 0.63× (m, 3.14) | 1 |
| stringzilla | 2.3M | str search/split | no (own `Str` type) | not measured | 0 |
| ada-url | 0.7M | `urllib.parse` | no (WHATWG semantics) | not measured | 0 |
| fastnumbers | 0.2M | `int()`/`float()` | partly | not measured | 1 |
| cydifflib | 0.18M | `difflib` | yes (Cython-compiled difflib) | no cp314/cp315 wheels | 0 |
| picologging | 13k (p) | `logging` | near drop-in | claimed 4–17× (README); no cp314/cp315 wheels | 0 |
| quicktions | 4k (p) | `fractions.Fraction` | yes | 0.27× (m) | 0 |
| copium | 3.6k (p) | `copy.deepcopy` | yes (claims identical semantics, autopatch) | 0.095× (m, 3.14; no cp315 wheel); claimed 4–28× | 0 |

No stdlib counterpart, listed for completeness:
- charset-normalizer 1450.7M, chardet 170.3M and cchardet 18.3M (encoding detection).
- PyYAML 1150.1M, ruamel.yaml 175.5M, tomlkit 565.0M (style-preserving, slower), toml 145.1M and tomli-w 84.0M.
- pydantic-core 1046.5M, attrs 844.2M, cattrs 37.9M and mashumaro 27.8M (vs `dataclasses`; not drop-ins).
- h11 892.3M and httptools 275.4M (HTTP parsing).
- multidict 556.0M, frozenlist 490.8M and sortedcontainers 248.4M.
- msgpack 232.8M, cbor2 74.7M, cloudpickle 203.3M and dill 163.3M (vs `pickle`, for features).
- zstandard 185.7M and backports.zstd 58.8M (`compression.zstd` is in the stdlib since 3.14), brotli 109.2M, lz4 86.3M and cramjam 32.9M.
- cachetools 269.5M and cachebox 13.2M (vs `functools.lru_cache`, for policies).
- cytoolz 10.6M and toolz 69.5M, more-itertools 256.1M, structlog 113.7M and loguru 78.6M.

Grouped by stdlib entry point, from strongest demand down:
- **json**: about 780M/month, of which jiter 393M is mostly transitive. 20+5+5+4 top packages import orjson/ujson/simplejson/msgspec.
- **datetime parsing**: about 1.5G/month (dateutil, isodate, arrow, pendulum, dateparser, ciso8601…), dominated by transitive dateutil at 1.1G.
- **uuid**: about 190M/month (uuid-utils, fastuuid, uuid6, uuid7).
- **regex features**: about 390M/month.
- **cached_property** via propcache: about 520M/month.
- **zlib-compatible codecs**: about 6M/month.
- **base64**: about 21M/month.
- **deepcopy, logging, fractions, difflib drop-ins**: under 0.2M/month each.

Demand is high where a non-drop-in API is acceptable (bytes output, new types). The exact drop-ins that Rustique
resembles (copium, cydifflib, picologging, quicktions, pybase64) have little uptake. Inferred: drop-in speed alone does
not drive installs, so a single `pip install rustique` that covers many entry points is the differentiator.

### 3.2 Measured speed of accelerators vs stdlib

How: `scripts/bench_accel.py`, `timeit` with autorange loop sizes, 7 repeats, median per call. Run on the shared
8-CPU WSL box (load average about 4), so spreads are 5–90%. Treat only ratios below 0.67 or above 1.5 as signal.
`identical` compares the accelerator's return value with the stdlib's on the benchmark input (`n/a` for random UUIDs
or different algorithms). Interpreters: python-build-standalone 3.15.1 (venv), and 3.14.9 for packages without cp315
wheels.

| entry point | case | stdlib ns/call | replacement | ns/call | ratio | spread | identical | Python |
|---|---|---|---|---|---|---|---|---|
| `json.dumps` | small (1 record, ~200 B) | 1,997 | orjson.dumps | 262 | 0.131 | 46% | **no** (bytes, compact, floats) | 3.15.1 |
| `json.dumps` | small (1 record, ~200 B) | 1,997 | ujson.dumps | 879 | 0.440 | 41% | **no** (`\/`, compact, floats) | 3.15.1 |
| `json.dumps` | small (1 record, ~200 B) | 1,997 | msgspec.json.encode | 330 | 0.165 | 4% | **no** (bytes, compact, floats) | 3.15.1 |
| `json.dumps` | small (1 record, ~200 B) | 1,997 | simplejson.dumps | 5,559 | 2.783 | 75% | yes | 3.15.1 |
| `json.dumps` | small (1 record, ~200 B) | 1,997 | rapidjson.dumps | 1,191 | 0.596 | 12% | **no** (compact separators) | 3.15.1 |
| `json.loads` | small (1 record, ~200 B) | 1,929 | orjson.loads | 643 | 0.333 | 119% | yes | 3.15.1 |
| `json.loads` | small (1 record, ~200 B) | 1,929 | ujson.loads | 1,046 | 0.542 | 47% | yes | 3.15.1 |
| `json.loads` | small (1 record, ~200 B) | 1,929 | msgspec.json.decode | 666 | 0.346 | 19% | yes | 3.15.1 |
| `json.loads` | small (1 record, ~200 B) | 1,929 | simplejson.loads | 2,629 | 1.363 | 30% | yes | 3.15.1 |
| `json.loads` | small (1 record, ~200 B) | 1,929 | jiter.from_json | 725 | 0.376 | 31% | yes | 3.15.1 |
| `json.dumps` | medium (200 records, ~40 KB) | 210,264 | orjson.dumps | 35,787 | 0.170 | 19% | **no** (bytes, compact, floats) | 3.15.1 |
| `json.dumps` | medium (200 records, ~40 KB) | 210,264 | ujson.dumps | 159,131 | 0.757 | 68% | **no** (`\/`, compact, floats) | 3.15.1 |
| `json.dumps` | medium (200 records, ~40 KB) | 210,264 | msgspec.json.encode | 46,753 | 0.222 | 32% | **no** (bytes, compact, floats) | 3.15.1 |
| `json.dumps` | medium (200 records, ~40 KB) | 210,264 | simplejson.dumps | 460,334 | 2.189 | 40% | yes | 3.15.1 |
| `json.dumps` | medium (200 records, ~40 KB) | 210,264 | rapidjson.dumps | 202,347 | 0.962 | 5% | **no** (compact separators) | 3.15.1 |
| `json.loads` | medium (200 records, ~40 KB) | 325,321 | orjson.loads | 106,028 | 0.326 | 93% | yes | 3.15.1 |
| `json.loads` | medium (200 records, ~40 KB) | 325,321 | ujson.loads | 169,821 | 0.522 | 53% | yes | 3.15.1 |
| `json.loads` | medium (200 records, ~40 KB) | 325,321 | msgspec.json.decode | 111,080 | 0.341 | 62% | yes | 3.15.1 |
| `json.loads` | medium (200 records, ~40 KB) | 325,321 | simplejson.loads | 301,279 | 0.926 | 51% | yes | 3.15.1 |
| `json.loads` | medium (200 records, ~40 KB) | 325,321 | jiter.from_json | 134,183 | 0.412 | 25% | yes | 3.15.1 |
| `datetime.strptime(%Y-%m-%dT%H:%M:%S.%f)` | ISO 8601 '2026-10-21T17:48:02.123456' | 3,461 | datetime.fromisoformat (stdlib C) | 124 | 0.036 | 54% | yes | 3.15.1 |
| `datetime.strptime(%Y-%m-%dT%H:%M:%S.%f)` | ISO 8601 '2026-10-21T17:48:02.123456' | 3,461 | dateutil.parser.isoparse | 4,019 | 1.161 | 28% | yes | 3.15.1 |
| `datetime.strptime(%Y-%m-%dT%H:%M:%S.%f)` | ISO 8601 '2026-10-21T17:48:02.123456' | 3,461 | pendulum.parse | 2,832 | 0.818 | 41% | value only (own subclass) | 3.15.1 |
| `datetime.strptime(%Y-%m-%dT%H:%M:%S.%f)` | ISO 8601 '2026-10-21T17:48:02.123456' | 3,461 | iso8601.parse_date | 3,290 | 0.951 | 37% | yes | 3.15.1 |
| `datetime.strftime` | strftime('%Y-%m-%d %H:%M:%S') | 1,485 | datetime.isoformat (stdlib C) | 458 | 0.308 | 58% | yes | 3.15.1 |
| `uuid.uuid4` | uuid4() | 939 | uuid_utils.uuid4 | 80 | 0.085 | 50% | n/a | 3.15.1 |
| `uuid.uuid4` | uuid4() | 939 | uuid_utils.compat.uuid4 | 326 | 0.348 | 42% | n/a | 3.15.1 |
| `uuid.UUID(s)` | UUID(str) | 897 | uuid_utils.UUID(s) | 153 | 0.171 | 56% | value only (own type) | 3.15.1 |
| `copy.deepcopy` | 20 JSON-like records | 58,482 | orjson round-trip | 18,243 | 0.312 | 25% | yes | 3.15.1 |
| `re (compiled).findall` | findall email pattern in 14 KB | 529,909 | regex (compiled).findall | 1,473,135 | 2.780 | 41% | yes | 3.15.1 |
| `re (compiled).match` | match short anchored date | 315 | regex (compiled).match | 772 | 2.448 | 20% | yes | 3.15.1 |
| `base64.b64encode` | b64encode 48 B | 222 | pybase64.b64encode | 209 | 0.942 | 37% | yes | 3.15.1 |
| `base64.b64encode` | b64encode 64 KB | 33,623 | pybase64.b64encode | 2,586 | 0.077 | 33% | yes | 3.15.1 |
| `base64.b64decode` | b64decode 64 KB | 40,352 | pybase64.b64decode | 30,616 | 0.759 | 16% | yes | 3.15.1 |
| `html.escape` | 2 KB markup | 10,034 | markupsafe.escape | 3,994 | 0.398 | 12% | **no** (`&#34;`/`&#39;` entities, `Markup` type; equal after mapping) | 3.15.1 |
| `html.escape` | 120 B, nothing to escape | 427 | markupsafe.escape | 343 | 0.803 | 55% | **no** (`&#34;`/`&#39;` entities, `Markup` type; equal after mapping) | 3.15.1 |
| `difflib.SequenceMatcher.ratio` | SequenceMatcher(a,b).ratio() 43 chars | 34,624 | rapidfuzz.fuzz.ratio/100 (InDel, not identical) | 256 | 0.007 | 27% | **no** | 3.15.1 |
| `hashlib.sha256().hexdigest` | sha256 64 KB | 31,167 | xxhash.xxh3_64 (different hash) | 2,024 | 0.065 | 14% | n/a | 3.15.1 |
| `hashlib.sha256().hexdigest` | sha256 64 KB | 31,167 | blake3 (different hash) | 16,170 | 0.519 | 50% | n/a | 3.15.1 |
| `functools.cached_property` | new instance + first access | 301 | propcache.cached_property | 165 | 0.546 | 48% | yes | 3.15.1 |
| `fractions.Fraction` | Fraction(3,7)+Fraction(5,11) | 1,078 | quicktions.Fraction | 292 | 0.271 | 26% | value only (own type) | 3.15.1 |
| `tomllib.loads` | loads 50 tables (~3 KB) | 1,068,866 | tomli.loads | 563,696 | 0.527 | 44% | yes | 3.15.1 |
| `tomllib.loads` | loads 50 tables (~3 KB) | 1,068,866 | rtoml.loads | 91,512 | 0.086 | 30% | yes | 3.15.1 |
| `datetime.strptime(%Y-%m-%dT%H:%M:%S.%f)` | ISO 8601 '2026-10-21T17:48:02.123456' | 5,311 | ciso8601.parse_datetime | 86 | 0.016 | 44% | yes | 3.14.9 |
| `uuid.uuid4` | uuid4() | 1,424 | fastuuid.uuid4 | 474 | 0.333 | 33% | n/a | 3.14.9 |
| `uuid.UUID(s)` | UUID(str) | 1,034 | fastuuid.UUID(s) | 133 | 0.129 | 34% | value only (own type) | 3.14.9 |
| `copy.deepcopy` | 20 JSON-like records | 69,250 | copium.deepcopy | 6,607 | 0.095 | 28% | yes | 3.14.9 |
| `re (compiled).findall` | findall email pattern in 14 KB | 433,876 | re2 (compiled).findall | 1,226,337 | 2.826 | 35% | yes | 3.14.9 |
| `re (compiled).match` | match short anchored date | 385 | re2 (compiled).match | 8,436 | 21.892 | 24% | yes | 3.14.9 |
| `zlib.compress` | compress 56 KB text level 6 | 912,034 | zlib_ng.zlib_ng.compress | 805,073 | 0.883 | 24% | **no** (different compressed bytes) | 3.14.9 |
| `zlib.compress` | compress 56 KB text level 6 | 912,034 | isal.isal_zlib.compress | 59,634 | 0.065 | 30% | **no** (isal level 2 vs zlib 6; different bytes) | 3.14.9 |
| `zlib.decompress` | decompress 56 KB text | 46,632 | zlib_ng.zlib_ng.decompress | 19,699 | 0.422 | 41% | yes | 3.14.9 |
| `zlib.decompress` | decompress 56 KB text | 46,632 | isal.isal_zlib.decompress | 29,132 | 0.625 | 39% | yes | 3.14.9 |

Also measured on 3.15.1: rapidjson, ujson and msgspec encode the same data as compact `json.dumps(...,
ensure_ascii=False)` with float differences at 321–564 of 2,000 random floats (`1e16` vs `1e+16`, `1e-7` vs
`1e-07`). rapidjson matched exactly. orjson raised `TypeError` on `2**64`.

## 4. Upstream overlap

Sources: `Doc/whatsnew/3.14.rst` (3.14.9) and `3.15.rst` (3.15.1) read locally. 3.16 comes from python/cpython `main`
`Doc/whatsnew/3.16.rst` (fetched 2026-11-01); PRs 139123 (C uuid, merged 2026-10-21) and 158253 (C strptime, merged
2026-10-23) were verified with `gh`. R4C (Rust for CPython) comes from discuss.python.org/t/106895,
github.com/Rust-for-CPython and blog.python.org/2026/09/language-summit-2026-rust-for-cpython. Raw notes with URLs:
`~/rustique-research/static/upstream/notes.md`. Release dates: 3.15.0 on 2026-10-09; 3.16.0 planned for 2027-10-05
(beta 1 on 2027-05-04).

| area | upstream item | version / status | overlap with a Rust in-place kernel |
|---|---|---|---|
| R4C overall | Pre-PEP; no PEP number or submission yet (target Dec 2026–Jan 2027); Rust optional; private Rust API; Cargo dependencies called a possible "showstopper" | 3.16: optional Rust zlib backend only; 3.17 (Oct 2028): io, json, xml, memoryview, parser as aspirations; Rust required ≥ 3.18 | — |
| json encode | ASCII strings that need no escaping up to 2.2× faster; faster small `dumps`; open PRs for a float fast path and no-escape escapers | landed 3.15; in 3.16a1; open; R4C json ≥ 3.17 (speculative) | **partial**, eroding each release |
| json decode | Arrays and objects built without an extra refcount round trip; open PR for numbers | 3.16a1; open | partial (small) |
| re | `\d`/`\s`/`\w` compile to a single CATEGORY opcode; open ASCII `\w` PR; no engine work | 3.16a1 | none–small |
| datetime parse | C `strptime` fast path for numeric formats (`%Y-%m-%d`, `%Y-%m-%d %H:%M:%S`); other formats stay in Python `_strptime`; PR to extend formats open | merged for 3.16 (2026-10-23) | **partial → large on 3.16+** for numeric formats; none on 3.14/3.15 |
| datetime format | `strftime` lookups cached; `date.today` 5× | 3.15 | small |
| uuid | 3.14: `uuid4` about 30% faster. 3.16: `UUID`, `uuid4`, `uuid7` in C (uuid4 15×, uuid7 30×, `UUID(str)` 6.8×, `str()` 6.7×) | 3.14 landed; merged for 3.16 | **full on 3.16+**; 3.14/3.15 window only |
| base64 / binascii | 3.14: b16decode 6×. 3.15: base64 encode 2×, decode 3×; Base32/Base85 in C (about 100×); hex SIMD | landed | **mostly full**; SIMD headroom remains at ≥ 64 KB only (measured: pybase64 0.077× encode on 3.15) |
| zlib / compression | `compression.zstd` (PEP 784); zlib-ng in the Windows build (3.14); `--with-zlib=zlib-rs` build option (3.16); R4C Rust zlib | 3.14; 3.16 option merged; R4C 3.16 proposed | partial now; full if distros adopt it. Compressed bytes differ across zlib/zlib-ng/zlib-rs, so only decompress and crc32/adler32 can be exact |
| io / text decoding | 3.14: fewer syscalls, 15% faster small reads. 3.15: `readall` 4×, line iteration 49%. 3.16: `readline` uses memchr. R4C io ≥ 3.17 | landed; speculative | partial |
| xml | none before 3.17 (R4C aspiration) | speculative | none near term |
| copy / deepcopy | C deepcopy PR closed unmerged (2026-03-26); gh-150815 closed "not planned" (2026-09-23) | rejected | **none** |
| pickle | minor | — | none |
| str / bytes methods | `str.count` 2× (3.14); `bytes.translate` 2×, hex SIMD (3.15); `unicodedata.normalize` up to 2× (3.16) | landed / 3.16a1 | partial, method by method |
| int / float formatting | none found | — | none |
| hashlib | HACL* Blake2 (3.14); hexdigest SIMD (3.15) | landed | none (already C/asm) |
| logging | lock-free `getLogger` fast path, faster `dictConfig` (3.16) | 3.16a1 | small (the record/format path is untouched) |
| textwrap / difflib / pathlib | `dedent` 2.4× (3.14); `IS_LINE_JUNK` 2× (3.14); `Path.read_bytes` 9–17% (3.14) | landed | small |
| asyncio | native task list: 10–20% on asyncio benchmarks (3.14) | landed | partial |
| startup | PEP 810 explicit lazy imports (opt-in); `tomllib` import 10× faster | 3.15 | orthogonal |
| interpreter | tail-calling interpreter 3–5% (3.14); JIT still experimental: 7–8% x86-64 Linux, 11–12% AArch64 macOS (3.15); 3.16 JIT goal about 10% | landed / goal | orthogonal: the JIT speeds up bytecode, not C kernels (inferred). Free-threading costs 5–10% single-threaded (3.14), and kernels must stay FT-safe |
| frozendict | PEP 814: builtin `frozendict`, accepted by json/copy/pickle | 3.15 | compatibility requirement: kernels must handle it or fall back |

Other projects:
- PyPy 7.3.23 (May 2026) targets 3.11, GraalPy 25.x targets 3.12, and Pyston is unmaintained.
- CinderX is a pip-installable JIT/Static Python package with cp314-only wheels, experimental for outside users.
- mypyc: no proposal to compile the stdlib. tomli shows the potential; compiled tomllib runs at 0.53× (measured).
- The Faster CPython team lost Microsoft sponsorship in 2025, and the JIT is now community-run.
- No project accelerates several stdlib modules process-wide with exactness guarantees. copium (deepcopy, autopatch)
  is the closest.

## 5. Signals by candidate

Reach and #pkgs come from library code (measured, `results/group_reach.json`). Demand is accelerator downloads per
month (measured). Upstream overlap is sourced (§4). The headroom column holds the best measured exact or
near-exact gap (§3.2). Signal is the inferred priority from these columns alone; profiling decides the final order.

| candidate | reach / #pkgs / sites | binding (from-import share) | accelerator demand | measured headroom (exact?) | upstream overlap | signal |
|---|---|---|---|---|---|---|
| `json.dumps`/`dump` | 31.8% / 338 / 10.8k | 0.6% | very high (orjson 218M, ujson, msgspec…) | 0.13–0.17× (not exact) | partial, eroding; R4C ≥ 3.17 | **high** |
| `json.loads`/`load` | 27.6% / 329 / 8.7k | 0.7% | very high (jiter 393M, orjson) | 0.33× (orjson/msgspec give the same objects) | small | **high** |
| `re` (compile/match/search/sub/findall/split) | 65.9% / 599 / 14.0k | ≤ 1.4% | features only (regex 368M) | none known: every alternative measured slower | none–small | **high usage, headroom unknown** (needs the headroom agent) |
| `copy.deepcopy` | 22.2% / 241 / 2.9k | 34% | tiny (copium) | **0.095× exact** (3.14) | none (rejected upstream) | **high** |
| `copy.copy` | 24.8% / 194 / 2.0k | 17% | none | not measured | none | medium |
| str methods (split/join/replace/strip/startswith/lower…) | 86.5% / 825 / 107k (name-based) | n/a | none (stringzilla 2.3M) | not measured; already C | partial, method by method | medium (profiling decides) |
| `%` / `str.format` | 59.4% / 557 / 58k | n/a | none | not measured | none | medium (profiling decides) |
| f-strings | 79.5% / 717 / 175k | n/a (bytecode) | none | — (only `__format__` reachable) | JIT/interpreter | low via swaps |
| logging (record + format) | 40.4% / 408 / 51k calls | 1% (`getLogger`) | low (picologging 13k; structlog/loguru are alternatives) | claimed 4–17× (picologging) | small (3.16 `getLogger`) | **medium-high** |
| `os.path` | 48.8% / 456 / 20.7k | 2% | none | not measured; mostly syscalls and short strings | none | low-medium |
| `pathlib` | 32.7% / 315 / 13.5k | 89% (`Path`) | none | not measured | small | low-medium |
| `urllib.parse` | 29.2% / 283 / 5.3k | 63–90% | yarl 540M (transitive), ada-url 0.7M | not measured | none found | **medium** |
| datetime parse (`strptime`, `fromisoformat`) | 14.3% / 150 / 605 | 0% (via class) | dateutil 1.1G (transitive), ciso8601 6.6M | `strptime` → 0.016× (ciso8601, exact on ISO input) | 3.16 C `strptime` (numeric formats) | **medium** (3.14/3.15 and named formats) |
| datetime format (`strftime`, `isoformat`) | 21.8% / 241 / 1.8k | 0% (via instance) | none | `isoformat` is 0.31× of `strftime` (stdlib C vs format parser) | small (3.15 caching) | medium |
| `uuid` (uuid4 / UUID) | 18.6% / 210 / 3.0k | 17–39% | uuid-utils 85M, fastuuid 80M | 0.085× (own type), 0.35× (compat) | **full in 3.16** | low (3.14/3.15 only) |
| `base64` / `binascii` | 27.7% / 262 / 2.4k | 13–15% | pybase64 21M | 0.077× at 64 KB, 0.94× at 48 B (exact) | mostly full (3.15) | low (large payloads only) |
| `dataclasses.asdict/replace/fields` | 13.6% / 129 / 1.6k | 68–74% | pydantic/attrs/msgspec (not drop-ins) | not measured | none found | medium-low |
| `@dataclass`, enum classes, `inspect.signature` | 28.6% / 39.1% / 18.9% | mostly from | — | class creation (startup) | none | low (startup only) |
| `functools.cached_property` | 11.9% / 101 / 1.3k | 61% | propcache 519M (transitive) | 0.55× (exact for plain use) | none | low-medium |
| `functools.lru_cache`, `partial` | 31.3% / 38.5% | 58–68% | cachetools/cachebox (features) | already C | none | low |
| zlib / gzip / bz2 / lzma | 13.0% / 122 / 472 | 0–7% | isal / zlib-ng about 6M | decompress 0.42× (exact output); compress output differs | R4C Rust zlib 3.16 (optional), zlib-rs build option | low |
| hashlib | 21.2% / 217 / 1.2k | 12–41% | xxhash/blake3 (different hashes) | none (OpenSSL/HACL*) | — | none |
| pickle / struct / csv | 9.3% / 21.3% / 4.8% | 2–24% | msgpack, cloudpickle (features) | already C | none | low |
| `difflib` | 6.2% / 62 / 106 | 65% (`SequenceMatcher`) | rapidfuzz 134M (different results) | `SequenceMatcher.ratio` 34.6 µs pure Python; cydifflib is exact | small | low-medium (niche, big per-call cost) |
| `textwrap` / `shlex` / `fnmatch` / `glob` | 20.2% / 11.1% / 13.5% | 15–47% | none | not measured | `dedent` 2.4× (3.14) | low |
| `xml.etree` | 5.3% / 65 / 724 | 5% | lxml 413M | not measured | R4C ≥ 3.17 (speculative) | low |
| `tomllib` | 3.6% / 34 / 54 | — | tomli 274M | 0.53× (mypyc, exact), 0.09× (rtoml) | import 10× faster (3.15) | low (config is parsed once) |
| `html.escape` | 6.8% / 72 / 319 | 29% | markupsafe 816M (transitive; different entities) | 0.40× (not exact) | none | low |
| `ipaddress` / `fractions` / `decimal` | 10.9% / 2.6% / 9.7% | — | quicktions 4k | `Fraction` 0.27× (exact) | none | low |
| `asyncio` loop | 16.9% / 193 / 6.0k | — | uvloop 281M | claimed 2–4× (scheduling not identical) | 3.14 task list 10–20% | low (exactness of scheduling order) |
| `isinstance` / `getattr` / `hasattr`, `len` | 90.0% / 845 / 262k | builtins | none | already C and specialized by the interpreter | — | none at this layer |

## 6. Reproduce

All scripts are in `~/rustique-research/static/scripts/` and read untrusted downloads only as data (`python -I`;
nothing is imported or executed).

| step | command | output |
|---|---|---|
| fetch | `python3.15 -I scripts/fetch_top.py dl/top.json dl/pkgs 1000 8` (top.json = `https://hugovk.dev/top-pypi-packages/top-pypi-packages.min.json`) | `dl/pkgs/<rank>_<name>/{meta.json,files/}` (about 1 min, 1.4 GB) |
| scan | `python3.15 -I scripts/scan.py dl/pkgs results/scan 2` | per-package counters + regex rows (about 6 min with 2 processes) |
| tables | `python3.15 -I scripts/aggregate.py results/scan results` | `results/tables.md` (full tables), `results/summary.json`, `results/re_patterns.jsonl` |
| regex corpus | `python3.15 -I scripts/re_corpus.py results/re_patterns.jsonl re_patterns.jsonl results/re_corpus_stats.json` | annotated corpus |
| groups | `python3.15 -I scripts/group_reach.py results/scan` | `results/group_reach.json` (§5 reach) |
| drill / top contributors | `python3.15 -I scripts/query.py results/scan 'ref:json.dumps\|call'` | which packages dominate a key |
| demand | `python3.15 -I scripts/accel_downloads.py dl/top.json results/accel_downloads.json` | downloads per accelerator |
| bench | `venv/bin/python -I scripts/bench_accel.py results/bench_accel_315.json` (and `venv314/`) | §3.2 |
