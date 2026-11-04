STATUS: wave 2, Phase 1, slice `w2-machinery` (2026-11-02). Branch `w2/p1-machinery`. Tools:
`benches/machinery/`. Prototype kernels (evidence, not product): `crates/rustique-py/src/proto/`,
`python/rustique/_proto_machinery.py`. Raw data: `~/rustique-research/w2/machinery/`.

# Machinery: generated code, stdlib machinery, language-level operations

Question: which CPython-provided machinery do frameworks, libraries and custom applications spend
time in (code Python generates for them, stdlib machinery they call, language operations behind C
entry points), and what can Rustique take exactly?

Tags: **M** measured (tool and box named), **I** inferred (from what). Box = the shared wave-2 dev
box (c7a.8xlarge), CPython 3.15.1 (python-build-standalone), GIL build unless stated.

## 0. Answer

1. **CPython-provided machinery is a small slice of framework and library time; the program's own
   and third-party bytecode dominates.** Over 38 workloads (web 10, devtools 11, frameworks 6,
   libs 6, custom 5), distortion-free perf: each new machinery candidate (generated code, enum,
   functools, contextlib, inspect, typing, abc, weakref, `re` functions) is ≤ 1.3% of mean time;
   email (3.2%) and asyncio (2.8%) are large only in single workloads. The custom family is the
   exception (dataclass `__init__` 7.8%, `singledispatch` 6.9%, contextlib 4.4%, enum 1.8% mean)
   (**M**, §2).
2. **Enum is the best new target.** All its hot entry points are Python functions CPython reaches
   only from C slots (`slot_tp_descr_get`, `slot_tp_call`, `slot_nb_or`, `slot_tp_hash`), so
   fallbacks cost a few Ir, and two of them sit on stdlib *heap types* whose slot can be swapped.
   Prototype: `Color(2)` 0.22, `Flag | Flag` 0.21, `m.value` 0.53, `~flag` 0.56 (callgrind);
   rules_engine 0.964, order_domain 0.980, attrs_cattrs 0.993 of instructions with only `enum` on
   (**M**). Reach 39% of the top-1000 PyPI packages.
3. **Generated code can be recognised exactly and cheaply, but the interpreter hides most of it.**
   A regenerate-and-compare recognizer proves a dataclass `__init__` / namedtuple `__new__` is
   what the generator emits (0 differences in 93 differential cases); lazy recognition at the
   first call costs 0–1.2% at import (eager: +1.7–5.4%, +24% on a 200-dataclass module) (**M**).
   But **`CALL_ALLOC_AND_ENTER_INIT` never consults an `__init__`'s vectorcall** (it inlines the
   code under the class's type version), so positional `Cls(a, b)` calls bypass any `__init__`
   kernel; keyword calls reach it: frozen dataclasses 0.51, non-frozen 1.0 (nothing to gain).
   Namedtuples: `NT(...)` 0.65, `_make` 0.62, `_replace` 0.22 (**M**). Workloads: event_ledger
   0.943, order_domain 0.977 (dataclasses), strawberry_graphql 0.967 (namedtuples) (**M**).
4. **Ruling 15 caps what dispatchers and protocol helpers can give**: `@contextmanager`'s
   `__enter__`/`__exit__`, the `singledispatch` wrapper and `_singledispatchmethod_get.__call__`,
   `total_ordering` methods, `Mapping.get`-style mixins, `html.parser` (bs4's 21.7%) all call user
   code from a Python frame. Only the parts before the user call qualify: contextmanager
   construction (`cm()` 0.65, rules_engine 0.951) and the `singledispatch` cache lookup
   (`dispatch` 0.58, but a whole `singledispatchmethod` call only 0.88) (**M**).
5. **All four prototype groups together** (enum, dataclass, namedtuple, contextmanager; 29
   workloads, instructions per run, identical digests, **M**): custom 0.962 (rules_engine 0.910,
   event_ledger 0.946, order_domain 0.956), frameworks 0.988 (strawberry_graphql 0.956), web 0.994
   (json_api 0.971), devtools 0.991 (rich_render 0.968), libs 0.996; geometric mean 0.987. Real,
   but a few percent: the bulk of framework time stays in bytecode (§2).
6. **Language-level operations are already C**; Rust redoes the same work at the same cost
   (getattr/hasattr 1.4%, str methods 1.3%, dict/list ops 6.3% mean are not targets). One real
   inefficiency found: a *negative* `isinstance(x, C)` performs a full `__class__` getattr
   (`object_isinstance`), ~60% of isinstance time, 3.9% of sqlglot; no Rustique entry point
   reaches it (handed to w2-interp / upstream).

Ranked recommendation (§6): **(1) enum**, (2) namedtuple (`__new__`, `_make`, `_replace`),
(3) contextmanager construction and `re` module functions (measured 0.71–0.88 per call, 49%
reach, but only ~1.3× on calls whose engine time dominates), (4) frozen-dataclass `__init__`
with an upstream `wrapper_descriptor` fix; `singledispatch` (0.77–0.88 per call, 3% reach) and
`inspect.signature` (not prototyped: large surface) only with a workload that needs them. Every
share and score is in §6.

## 1. Method

| step | how | where |
|---|---|---|
| machinery entry shares | `perf record -F 1999 -g` with `-X perf`, gated to steady-state `run()`s; each sample → innermost *user* frame (own or third-party) → the CPython-provided function it called (inclusive; user code below re-attributes) or the first non-plumbing native entry (`lang:`) | `benches/machinery/profile_machinery.py`, `report.py`; 38 workloads (**M**) |
| distortion check | `-X perf` disables inlined Python calls (shares of Python-heavy code shift; w2-interp measured up to 1.43×). Primary shares are therefore re-measured with w2-attribution's plain-perf profiler (`benches/atlas/layers.py` @ 08867fb, `--modes plain`: dwarf unwinding, owner from the bytecode pointer, interpreter specializing as in production) | `benches/machinery/layers_share.py` over 38 workloads (**M**) |
| per-op cost | callgrind, region-instrumented, Ir(2n)−Ir(n), ~60 cells, fresh process each | `benches/machinery/cells.py` (**M**) |
| workload effect | `perf stat -e instructions:u`, (I(3 runs) − I(1 run))/2 per `run()`, plain vs `proto_enable(...)` before the workload imports, PYTHONHASHSEED=0, digests compared; process-to-process noise ±0.1–0.6% (up to ±1% on flask_marshmallow) | `benches/machinery/wl_instr.py` (**M**) |
| correctness | plain vs prototype in fresh processes, 93 observations (results, types, identities, exception types/messages, traceback function names, side-effect order) | `benches/machinery/check_proto.py`: 0 differences (**M**) |
| reach | wave-1 AST scan of the top-1000 PyPI packages (download-weighted) | `benches/machinery/static_reach.py` (**M**) |

Cross-check (**M**): plain-perf innermost shares vs `-X perf` entry shares agree within ±30% for
leaf machinery; `-X perf` *understates* inlined generated code (dataclass `__init__`: order_domain
21.5% plain vs 16.0%, strawberry 1.3% vs 0.9%) and *overstates* Python-call-heavy entries
(falcon `inspect` 11.4% vs 14.8%). §2 uses plain-perf numbers; §2.3 (language entries) and §2.4
(who calls what) need full stacks and use `-X perf`. cProfile is used only for call counts, never
for generated code (pstats merges all `<string>:1 __init__` entries: w2-wl-custom found 65k
reported vs 1.4M real calls).

## 2. Where the time goes

### 2.1 Distortion-free shares per family (% of steady-state CPU time, mean / max)

`layers_share.py` over w2-attribution's plain profiles (**M**, 3.15.1). Innermost-code-object
inclusive; a candidate that calls other CPython Python code is split (inspect → enum, singledispatch
→ weakref).

| candidate | all 38 | web | devtools | frameworks | libs | custom |
|---|---:|---|---|---|---|---|
| **enum** (all of `enum.py`) | 0.42 | 0.39 / 3.0 json_api | 0.06 / 0.2 | 0.19 / 0.4 | 0.18 / 0.5 attrs_cattrs | 1.81 / 5.0 rules_engine |
| · `.value`/`.name` (`enum.property.__get__`) | 0.13 | 0.33 / 3.0 json_api | 0.02 | 0.01 | 0.11 / 0.5 pydantic_payloads | 0.16 / 0.4 |
| · value lookup (`EnumType.__call__`, `Enum.__new__`) | 0.11 | 0.03 | 0.00 | 0.10 / 0.2 falcon | 0.06 / 0.4 attrs_cattrs | 0.56 / 2.3 order_domain |
| · Flag operators | 0.11 | 0.02 | 0.00 | 0.03 | 0.00 | 0.80 / 2.9 rules_engine |
| · `Enum.__hash__` | 0.03 | 0.01 | 0.03 | 0.04 | 0.00 | 0.14 / 0.6 |
| **dataclass generated `__init__`** | 1.31 | 0.07 / 0.4 | 0.76 / 4.3 black | 0.25 / 1.3 strawberry | 0.00 | 7.82 / 21.5 order_domain |
| dataclass generated `__eq__/__hash__/__repr__`/order/frozen setattr | 0.06 | 0.01 | 0.06 / 0.4 | 0.13 / 0.8 strawberry | 0.00 | 0.12 / 0.6 |
| **namedtuple generated `__new__`** | 0.42 | 0.32 / 1.8 fastapi_pydantic | 0.46 / 3.8 rich | 0.92 / 4.6 strawberry | 0.33 / 1.6 pydantic_payloads | 0.02 |
| namedtuple methods (`_make`/`_replace`/`_asdict`/`__repr__`) | 0.22 | 0.00 | 0.76 / 4.1 pylint (`TokenInfo._make`) | 0.00 | 0.02 | 0.00 |
| total_ordering methods | 0.04 | 0 | 0 | 0 | 0 | 0.31 / 1.6 fulfilment_sim |
| contextlib `@contextmanager` helper + `__init__` | 0.36 | 0.12 / 0.5 | 0.29 / 1.5 pyflakes | 0.13 / 0.4 | 0.02 | 1.70 / 8.5 rules_engine |
| contextlib `__enter__`/`__exit__` | 0.45 | 0.16 / 0.6 | 0.33 / 1.6 | 0.16 / 0.5 | 0.02 | 2.18 / 10.9 rules_engine |
| contextlib ExitStack/other | 0.19 | 0.19 / 1.2 flask | 0.18 / 1.2 | 0.13 / 0.4 | 0.00 | 0.53 / 2.7 |
| functools `singledispatch(method)` | 0.90 | 0 | 0 | 0 | 0 | 6.86 / 34.3 event_ledger |
| functools `cached_property` | 0.07 | 0 | 0.05 / 0.4 | 0 | 0 | 0.38 / 1.9 |
| inspect | 0.67 | 0.36 / 1.3 django_blog | 0.55 / 1.9 pytest | 2.26 / 11.4 falcon | 0.02 | 0.44 / 2.2 |
| typing (run time) | 0.22 | 0.23 / 1.1 | 0.05 | 0.09 | 0.30 / 1.7 bs4 | 0.60 / 2.5 event_ledger |
| abc (`ABCMeta.__instancecheck__` wrapper) | 0.26 | 0.34 / 1.2 | 0.07 | 0.43 / 0.8 | 0.46 / 2.0 jsonschema | 0.11 |
| collections.abc mixins | 0.17 | 0.31 / 1.8 requests | 0.04 | 0.29 / 1.1 falcon | 0.16 / 0.6 | 0 |
| weakref | 0.17 | 0.14 / 1.2 | 0 | 0.33 / 1.3 flask_marshmallow | 0 | 0.62 / 3.1 event_ledger |
| `re` module functions (`_compile` cache) | 0.30 | 0.37 / 1.6 | 0.47 / 2.4 isort | 0.17 / 0.5 | 0.27 / 1.4 jsonschema | 0 |
| uuid | 0.30 | 0.90 / 9.0 json_api | 0 | 0.24 / 1.4 falcon | 0.13 / 0.4 | 0 |
| html (`html.parser`, `escape`) | 0.62 | 0.08 / 0.8 | 0.02 | 0.16 / 0.9 | 3.62 / 21.7 bs4_scrape | 0 |
| email (+ `http.client` 0.27) | 3.22 | 10.98 / 95.3 email_mime | 0.88 / 9.7 pip | 0.13 / 0.4 | 0.34 / 2.0 boto3 | 0 |
| tokenize (glue generator) | 0.41 | 0 | 1.41 / 6.0 pylint | 0 | 0 | 0 |
| os.environ | 0.08 | 0.21 / 1.7 requests | 0.06 | 0.02 | 0.02 | 0 |
| threading/queue | 0.21 | 0.77 / 5.9 fastapi_pydantic | 0.01 | 0.05 | 0 | 0 |
| hmac / base64 / ipaddress / string | 0.07 / 0.01 / 0.02 / 0.01 | | | | | |
| asyncio | 2.81 | 8.28 / 52.6 | 0 | 3.97 / 14.1 fastapi_sqlalchemy | 0 | 0 |
| *wave-1 groups, for scale:* json / copy / logging / urllib.parse / paths | 3.00 / 1.50 / 1.98 / 0.73 / 0.55 | | | | | |

Frameworks+libs+custom mean (17 workloads): dataclass `__init__` 2.39, singledispatch 2.02,
html 1.33, inspect 0.93, contextlib 1.46 (all parts), enum 0.66, namedtuple 0.46 (**M**).

### 2.2 Static reach (top-1000 PyPI, library code, download-weighted)

`static_reach.py` (**M**, wave-1 scan of 937 packages, 132.4 G downloads/30 days):

| feature | reach | pkgs | | feature | reach | pkgs |
|---|---:|---:|---|---|---:|---:|
| `isinstance()` | 0.87 | 805 | | `typing.cast` | 0.43 | 361 |
| f-strings | 0.80 | 717 | | Enum base (any kind) | 0.39 | 310 |
| `getattr()` / `hasattr()` | 0.73 / 0.70 | 648 / 617 | | `contextlib.contextmanager` | 0.38 | 299 |
| `warnings.warn` | 0.62 | 506 | | `functools.wraps` / `lru_cache` | 0.34 / 0.31 | 279 / 243 |
| `re.*` module functions | 0.49 | 459 | | `@dataclass` | 0.29 | 261 |
| `str.format` (literal) / `%` | 0.48 / 0.39 | 440 / 395 | | `typing.NamedTuple` / `namedtuple` | 0.26 / 0.21 | 183 / 188 |
| `os.environ`/`getenv` | 0.46 | 423 | | `inspect.signature` | 0.19 | 183 |
| `functools.partial` | 0.38 | 313 | | `cached_property` | 0.13 | 107 |
| `typing.runtime_checkable` | 0.08 | 61 | | `singledispatch(method)` | 0.03 | 33 |
| `html.escape` | 0.06 | 60 | | `total_ordering` | 0.06 | 46 |

### 2.3 Language-level operations reached from user bytecode (`-X perf`, % mean / max)

`report.py` over 38 `-X perf` profiles (**M**): the first native entry below a user frame.

| operation | all | max |
|---|---:|---|
| dict methods/ops | 3.64 | 8.5 jsonschema |
| list/tuple/set ops (incl. sort) | 2.70 | 7.3 json_api |
| getattr()/hasattr()/setattr() | 1.43 | 13.3 mini_interp |
| re engine (`_sre`) | 1.40 | 22.3 sphinx |
| str methods | 1.25 | 5.1 flask_jinja |
| isinstance/issubclass (C paths) | 0.91 | 7.0 sqlglot (of which `__class__` getattr of negative checks ≈ 60%) |
| `str()`/`repr()` of objects | 0.35 | 4.5 pydantic_payloads |
| `str.format`/`format()` / `str % x` | 0.14 / 0.13 | 1.4 / 0.8 |
| str/bytes codecs, int()/float() from text | 0.11 / 0.09 | 0.6 / 1.5 |
| *(w2-interp)* generic attribute paths + `slot_tp_*`, call machinery | 5.9 + 3.1 | 15.7 drf_api |

### 2.4 Which CPython entry points third-party code spends its time in

`-X perf`, summed over the workloads that use the distribution (**M**; top rows):

| distribution | CPython-provided share (sum over its workloads) | top entries |
|---|---:|---|
| Django | 0.61 | dict ops 0.09, getattr/hasattr 0.08, list ops 0.06, str methods 0.05 |
| pyflakes | 0.61 | `ast.parse`/compile 0.28, dict 0.07, isinstance 0.04, contextlib 0.03 |
| falcon | 0.58 | `wsgiref.validate` 0.35, `inspect.getfullargspec` 0.15 (one call per request) |
| pip | 0.49 | zipfile 0.11, import 0.10, list 0.06, email 0.04 |
| pycodestyle | 0.43 | generators 0.13, list 0.07, tokenize 0.07, `_sre` 0.06 |
| urllib3 | 0.28 | `http.client` 0.21 |
| botocore | 0.27 | stdlib C (json/hashlib) 0.09, dict 0.04, getattr 0.03, email 0.02 |
| Pygments | 0.28 | `_sre` 0.26 |
| beautifulsoup4 | 0.24 | `html` 0.16 |
| pydantic | 0.24 | stdlib C 0.09, dict 0.08, namedtuple `__new__` 0.01 |
| graphql-core | 0.21 | getattr 0.05, dict 0.03, namedtuple `__new__` 0.03 |
| sqlglot | 0.21 | isinstance 0.07, dict 0.06 |

Third-party libraries spend their CPython time in the same generic places as user code (dict,
list, attribute builtins, isinstance); the library-specific stdlib entries are `http.client`
(urllib3), `html.parser` (bs4), `_sre` (Pygments), `inspect` (falcon), namedtuples (graphql-core,
rich, pydantic).

## 3. Per-operation costs (callgrind Ir, 3.15.1, **M**)

`cells.py`; plain CPython; proto = this slice's prototype (§4).

| operation | plain | proto | | operation | plain | proto |
|---|---:|---:|---|---|---:|---:|
| `m.value` / `m.name` | 1,097 | 577 (0.53) | | `cm()` (@contextmanager) | 4,181 | 2,736 (0.65) |
| `Color(2)` / `Color(m)` | 2,347 / 2,193 | 515 (0.22) / 439 (0.20) | | `with cm(): pass` | 8,099 | 6,645 (0.82) |
| `Perm.R \| Perm.W` / `&` | 5,358 / 5,364 | 1,121 (0.21) / 1,141 (0.21) | | `fmt(1)` singledispatch | 3,131 | 2,402 (0.77) |
| `~flag` / `r in rw` | 1,143 / 1,048 | 637 (0.56) / 932 (0.89) | | `f.dispatch(int)` | 1,727 | 998 (0.58) |
| `hash(m)` / `d[m]` | 1,213 / 985 | 968 (0.80) / 754 (0.77) | | `h.handle(1)` singledispatchmethod / `h.handle` | 9,668 / 3,412 | 8,495 (0.88) / 2,969 (0.87) |
| `P(1, 2)` dataclass, positional | 1,365 | 1,356 (0.99, bypassed) | | `C(1).p` cached_property first | 3,732 | – |
| `P(x=1, y=2)` | 2,852 | 2,898 (1.016 → plan refused) | | `a >= b` total_ordering | 1,269 | – |
| `F(1, 2)` frozen, positional | 3,822 | 3,813 (bypassed) | | `isinstance(o, Proto)` hit / miss | 3,348 / **23,568** | – |
| `F(x=1, y=2)` frozen | 5,308 | 2,690 (0.51) | | `isinstance({}, Mapping)` | 1,753 | – |
| frozen+slots+kw_only `E(a=1, b='s')` | 5,337 | 2,730 (0.51) | | `isinstance(a, A)` hit / miss | 171 / 347 | – |
| `S(1, 2)` slots / `L(1)` factory | 1,080 / 1,660 | 1.00 / 0.995 | | `typing.cast(int, x)` | 328 | – |
| `p == q` / `hash(f)` / `repr(p)` | 1,476 / 1,622 / 5,849 | – | | `re.match(p, s)` / `re.match(p, s, re.I)` | 4,552 / 5,018 | 3,451 (0.76) / 3,582 (0.71) |
| | | | | `re.search` / `re.sub` / `re.split` | 6,739 / 6,067 / 10,407 | 0.84 / 0.81 / 0.88 |
| | | | | `re.sub(p, r'<\1>', s)` (template: fallback) | 10,830 | 11,172 (1.032) |
| `NT(1, 2, 3)` / keywords | 2,198 / 3,503 | 1,433 (0.65) / 2,558 (0.73) | | `html.escape(12 chars)` | 3,012 | – |
| `TN(1, 2)` (typing.NamedTuple) | 2,151 | 1,385 (0.64) | | `uuid.UUID(hex)` / `str(u)` | 9,100 / 5,779 | – |
| `NT._make(t)` | 1,988 | 1,239 (0.62) | | `os.environ['HOME']` / `.get` | 3,156 / 3,503 | – |
| `nt._replace(x=2)` | 5,260 | 1,169 (0.22) | | `inspect.getfullargspec(f)` | 106,964 | – |
| `repr(nt)` / `nt._asdict()` | 3,880 / 3,440 | – | | `inspect.signature(f)` / `sig.bind` | 93,675 / 25,999 | – |
| `wkd[k]` WeakKeyDictionary | 1,593 | – | | `base64.b64encode(20 B)` | 2,068 | – |

## 4. Prototypes

All six groups pass the 164-case differential check with 0 differences (**M**,
`check_proto.py`; cases include misses, unhashable values, user `__hash__`/`__eq__` with
side-effect logs, subclasses, metaclass subclasses, `enum.property` subclasses, keyword/positional
errors, mutated `__defaults__`, `tuple.__new__(NT, …)`, `__post_init__`, user `__init__`,
generators raising inside `with`, singledispatch with ABC registration changing the cache token,
classmethod/staticmethod registrations, `re` with RegexFlag/keyword flags, compiled patterns,
template and callable replacements, the positional-`count` DeprecationWarning, `re.purge()`).

### 4.1 enum (strongest result)

Sites (all reached from C, so fallback = a few Ir):

| entry | mechanism | reached from |
|---|---|---|
| `enum.property` `tp_descr_get` (heap type defined in enum.py) | **slot swap**; fallback = saved `slot_tp_descr_get` | `m.value`, `m.name` (generic LOAD_ATTR) |
| `EnumType` `tp_call` | **slot swap**; fallback = saved `slot_tp_call` | `Color(2)` |
| `EnumType.__call__`, `Enum.__hash__`, `Flag.__or__/__and__/__xor__/__contains__/__invert__`, `enum.property.__get__` | `PyFunction_SetVectorcall` | `slot_tp_call` of EnumType subclasses, `slot_tp_hash`, `slot_nb_*`, `slot_sq_contains` (C callers: no lost inlining) |

Exact subset: `Color(value)` with one positional argument; the class's `__new__` is `Enum.__new__`;
value is the member itself or an exact `int`/`str`/`bool` found in `_value2member_map_`, whose keys
are all exact `int`/`str`/`bool`/`float`/`None` (so the dict compares only in C). Flag operators:
both operands of exactly the same class, `_value_` exact ints, `_get_value` not overridden, result
already in `_value2member_map_` (composites are cached there after first creation; first creation
falls back to `_missing_`). `.value`/`.name`: the descriptor is exactly `enum.property` with the
`fget` of `Enum.value`/`Enum.name`; the member's type has the generic getattro and no descriptor
named `_value_`/`_name_`. Everything else, and any listening tool, runs CPython's code.

Lead's questions:

1. **Slot swaps on stdlib heap types.** A subclass of `enum.property` created *after* the swap gets
   `slot_tp_descr_get` from `fixup_slot_dispatchers` (its `__get__` is a Python function), created
   *before* keeps its own; the kernel checks the exact type anyway (**I**, typeobject.c
   `update_one_slot`; **M** behaviourally: `MyProp` cases identical). Metaclasses deriving from
   `EnumType` (Django's `ChoicesType`) get `slot_tp_call` and reach the `EnumType.__call__`
   vectorcall kernel (0.47 in an earlier build, before the slot swap) (**M**). No specialization
   is keyed on these pointers: `classify_descriptor` returns `MUTABLE` for heap-type descriptors
   (no LOAD_ATTR specialization, generic path reads `tp_descr_get` per call, specialize.c:515), and
   calls of classes whose metaclass is not `type` go to `CALL_NON_PY_GENERAL`, which reads
   `tp_call` per call (specialize.c:1681) (**I**, source). Assigning `enum.property.__get__` or
   `EnumType.__call__` re-runs `update_slot`, which replaces the kernel with CPython's slot (a
   correct fallback); `disable()` restores both pointers by CAS and the vectorcalls with
   `_PyFunction_SetVersion` (as wave 1).
2. **Mutable state the kernels read.** `_value2member_map_`: looked up per call; its size is cached
   and a change re-validates the key types (Flag pseudo-members added by `_missing_` take this
   path). `_member_map_`: emptiness re-checked per call (`if cls._member_map_`). `__members__` is a
   read-only proxy of `_member_map_`. Rebinding any class or metaclass attribute changes the type's
   version tag, which invalidates the per-class facts (64-entry direct-mapped cache keyed by class,
   tag and metaclass tag). Instance `_value_`/`_name_`/`_inverted_` are read each call. Residual
   (private state): replacing a key of `_value2member_map_` by an unsafe one at equal size; a dict
   watcher on that dict closes it in production. Free-threading: dict reads use
   `PyDict_GetItemRef` (safe); the prototype's facts cache is a plain static and must become
   per-class atomic state (or recomputed: 5 `_PyType_Lookup`s, ~+120 Ir) on 3.15t.
3. **Per-minor differences.** The source of every original the kernels reproduce is identical in
   3.14.9 and 3.15.1 (line-independent source hash of 16 functions; **M**), so 3.14 needs only its
   own bytecode fingerprints: worth validating. 3.16.0a1 rewrote `Flag.__or__`/`__and__` (same
   semantics, a Python fast path for the `None` checks); `enum.property.__get__`,
   `EnumType.__call__`, `Enum.__new__`, `Enum.__hash__` are unchanged (**M**). Upstream overlap is
   small: CPython keeps enum in Python and tunes its bytecode.
4. **Share across all families** (plain perf, §2.1): 0.42% mean of 38 workloads (custom 1.81, web
   0.39, frameworks 0.19, libs 0.18, devtools 0.06); maxima rules_engine 5.0, json_api 3.0,
   order_domain 4.1 (**M**).

Workload effect, enum group only (instructions per run, **M**): rules_engine **0.964**,
order_domain **0.980**, attrs_cattrs 0.993, flask_marshmallow 0.994, jsonschema_validate 0.997,
strawberry_graphql 0.997, django_admin 0.999, event_ledger 1.000.

### 4.2 Generated code: dataclass `__init__`, namedtuple `__new__`/`_make`/`_replace`

**Recognition** (`python/rustique/_proto_machinery.py`): the function must be the one in the
class's own dict, with the generator's file/qualname; the recognizer re-runs the generator's own
builder for the class's fields (`dataclasses._init_fn` into a fresh `_FuncBuilder`, or the
namedtuple `eval` template), compiles it and compares the canonical code (bytecode, names,
varnames, freevars, flags, argcounts, exception table, consts; line-independent). Only then a
plan (parameter names, store steps, closure-cell indices) is attached to the code object
(`PyUnstable_Code_SetExtra`). Refused: `__post_init__` and `InitVar` (ruling 15 / extra code),
`init=False` fields that the generator initializes, a user `__setattr__` on non-frozen classes,
Python data descriptors named like a field, > 16 parameters, and — after measuring 1.00–1.016 —
non-frozen classes. Per call the kernel re-checks: exact `type(self)`, the class's version tag,
the closure cells (`__dataclass_builtins_object__` is `object`, the factory sentinel, factories
exactly `list`/`dict`/`set`), tools quiet. `_make`/`_replace` share one code object across all
namedtuples, so they need no plan: the kernel checks the code object and the closure cells.

**Hook and import cost.** `dataclasses._process_class` and `collections.namedtuple` get a
vectorcall kernel that calls the original and then patches the new class's functions
(`typing.NamedTuple` goes through `namedtuple`). Recognition at class creation costs ~40 µs per
dataclass (the recognizer compiles source), so it is done lazily at the first call:

| `import X` (user instructions, whole process, median of 3, **M**) | plain | eager recognition | lazy recognition |
|---|---:|---:|---:|
| 200 dataclasses + 50 namedtuples + 50 contextmanagers (synthetic) | 538.7 M | +23.7% | **+0.2%** |
| `rich.console` / `sqlglot` / `pydantic` | 82.5 / 381.5 / 150.8 M | +4.1 / +1.7 / +2.7% | −0.2 / 0.0 / +0.2% |
| `black` / `fastapi` / `sqlalchemy.orm` / `django.db.models` | 579 / 956 / 897 / 479 M | +4.4 / +5.4 / +1.9 / +1.9% | +0.8 / +1.2 / −0.1 / 0.0% |

(`proto_enable` itself: 2.5 M instructions, importing the recognizer module.) A Rust structural
verifier of the code object (inferred ~1–3 µs per class from the size of the bytecode) would make
eager recognition affordable too.

**The interpreter decides what a kernel can reach** (**M**, source-confirmed):

| call form | path in 3.15 | kernel reached? | measured |
|---|---|---|---|
| `P(1, 2)` (`tp_new` is `object_new`, Python `__init__`) | `CALL_ALLOC_AND_ENTER_INIT`: inlines `__init__`'s code under the class's type version, never reads the function's vectorcall or version (specialize.c:1626–1705, `_PyType_CacheInitForSpecialization`) | **no** | 0.99–1.00 |
| `P(x=1, y=2)`, `P(*a)`, `super().__init__(…)` from C callers | `type_call` → `slot_tp_init` → vectorcall | yes | frozen 0.51, frozen+slots+kw_only 0.51, non-frozen 1.00–1.016 |
| `NT(1, 2, 3)` | `type_call` → `slot_tp_new` → vectorcall (C caller: +3 Ir fallback) | yes | 0.65 (keywords 0.73) |
| `NT._make(t)`, `nt._replace(x=…)` | bound classmethod / method from bytecode: vectorcall replaces an inlined call (+180–260 Ir per fallback) | yes | 0.62, 0.22; `_make(iterator)` falls back (non tuple/list): measured +371 Ir (`_replace` through `_make(map(...))` was 1.071 before it got its own kernel), so iterator inputs need the §5.6 credit or exact builtin-iterator support |
| swap a namedtuple class's `tp_new` | — | **not allowed**: `tuple.__new__`'s safety check (`tp_new_wrapper`) walks bases while `tp_new == slot_tp_new`; a swapped pointer makes `tuple.__new__(NT, …)` and `_make` raise | – |

Frozen dataclasses are slow because each field is `object.__setattr__(self, 'x', x)`: a
`wrapper_descriptor` call without vectorcall (`wrapperdescr_call` builds an args tuple, then
`wrap_setattr` → `PyObject_GenericSetAttr`): 3,822 Ir positional vs 1,365 non-frozen. Giving
`wrapper_descriptor` a vectorcall (or `__setattr__` slot wrappers a fast path) upstream would give
every frozen class most of the kernel's gain, positional calls included (**I**).

Workload effect (instructions per run, one group on, **M**; this run still had plans for
non-frozen classes): dataclass kernel: event_ledger **0.943** (frozen slots kw_only events),
order_domain **0.977** (its frozen kw_only events; the hottest class, `Money`, has
`__post_init__`: refused), rules_engine 0.995; namedtuple `__new__` (before `_make`/`_replace` existed):
strawberry_graphql **0.967** (327k `NT(...)` per run, graphql-core), flask_marshmallow 0.993,
django_admin 0.998. Fallback counters (`proto_stats`): strawberry 26k dataclass calls per run with
`type(self)` a subclass (falls back: plan is per class).

### 4.3 contextmanager construction

`helper(*args, **kwds)` (the closure `@contextmanager` returns; all share one code object) is
done natively: create the generator by calling the user's generator function (binding only — no
user code runs before the first `next()`, and the tools guard covers profilers), then allocate a
`_GeneratorContextManager` and set `gen, func, args, kwds, __doc__` in CPython's order. Requires
an exact pure-Python generator function with CPython's vectorcall and an unchanged
`_GeneratorContextManager` (version tag). `__enter__`/`__exit__` stay CPython's (they run the
generator: ruling 15). Hook: vectorcall kernel on `contextlib.contextmanager` patches each helper.

`cm()` 0.65, `with cm(): pass` 0.82 (**M**); rules_engine **0.951**, strawberry 0.996 (**M**,
ctx group only). The helper is called from bytecode, so a fallback costs +180–260 Ir; in the
workloads 0 fallbacks were counted (`proto_stats`).

### 4.4 singledispatch: the cache lookup and `singledispatchmethod.__get__`

Ruling 15 leaves two pieces: (a) the `dispatch(cls)` closure (cells `cache_token`,
`dispatch_cache`, `registry`), on a hit of `dispatch_cache[cls]`: exact `WeakKeyDictionary` with
an unchanged class, `weakref.ref` unchanged, the class's metaclass hashes and compares in C (then
the lookup is identity on the basic weakref), ABC cache token unchanged (re-read through the C
`get_cache_token` when one is set); a miss runs `_find_impl` in the original. Hook on
`functools.singledispatch` patches each new `dispatch`. (b) `singledispatchmethod.__get__` (reached
from `slot_tp_descr_get`) builds `_singledispatchmethod_get` natively when `func` is a plain
function (its `__init__` runs no user code then): same seven attributes in CPython's order. The
`wrapper` and `_singledispatchmethod_get.__call__` stay CPython's (they call the user's impl).

`f.dispatch(int)` 0.58, `fmt(1)` 0.77, `h.handle(1)` 0.88 (`__get__` alone 0.87: building a
seven-attribute instance natively is not much cheaper than the bytecode `__init__`) (**M**).
Workloads (sd group only, instructions per run, **M**): event_ledger **0.925**,
flask_marshmallow 1.003 (noise band ±0.5% on that workload).

### 4.5 `re` module functions

`re.prefixmatch/match/fullmatch/search/findall/finditer/sub/subn/split`: the kernel reproduces
`_compile`'s fast path against CPython's own `_cache2` (key `(type, pattern, flags)`, flags exact
int, `_ZeroSentinel`, or `RegexFlag` via `_value_`; exact `str`/`bytes`/`Pattern`), then calls the
`Pattern` method. Using CPython's cache itself keeps `re.purge()` and cache contents identical; a
miss (compilation) runs the original. `sub`/`subn` only with a literal replacement (no backslash:
templates are compiled by Python code, callables are user code); positional `count`/`maxsplit`
(DeprecationWarning path) falls back.

`re.match` 0.76, with `re.I` 0.71, `search` 0.84, `sub` 0.81, `split` 0.88; the template
fallback costs 1.032 (lost inlining; 1.049 before the replacement was checked first) (**M**). The prototype reads `_compile`/`_cache2` from
the module dict per call (~60 Ir); dependency pins (one relaxed load) would save that.
Workloads (re group only, instructions per run, **M**): fastapi_pydantic 0.985 (httpx),
django_admin 0.995, black 0.996, jsonschema_validate 0.997, boto3_client 0.999, isort 1.001,
pycodestyle 1.001, **email_mime 1.004**: `email` calls `re.sub` with templates/callables, each a
fallback (+500 Ir in that build, +342 Ir after checking the replacement first) — the
never-slower rule needs the §5.6 credit (or template support) before this ships.

### 4.6 enum, dataclass, namedtuple and contextmanager together

`wl_instr.py`, instructions per `run()`, plain vs `proto_enable("all")` before the workload
imports, digests identical everywhere (**M**; noise ±0.1–0.6%, flask_marshmallow ±1%; see
`~/rustique-research/w2/machinery/instr3/`):

| family (geo mean) | workloads, instructions proto / plain |
|---|---|
| custom (**0.962**) | rules_engine **0.910**, event_ledger **0.946**, order_domain **0.956**, fulfilment_sim 1.000, mini_interp 1.002 |
| frameworks (**0.988**) | strawberry_graphql **0.956**, fastapi_sqlalchemy 0.986, flask_marshmallow 0.994, falcon_api 0.995, django_admin 0.998, drf_api 0.999 |
| libs (**0.996**) | bs4_scrape 0.989, attrs_cattrs 0.993, pydantic_payloads 0.993, boto3_client 0.999, jsonschema_validate 0.999, yaml_config 1.002 |
| web, 6 of 10 (**0.994**) | json_api **0.971**, requests_local 0.996, fastapi_pydantic 0.997, flask_jinja 0.999, sqlalchemy_orm 1.000, django_blog 1.002 |
| devtools, 6 of 11 (**0.991**) | rich_render **0.968**, pycodestyle_stdlib 0.988, pyflakes_stdlib 0.994, pylint_stdlib 0.998, black_format 0.998, isort_stdlib 1.000 |
| all 29 | geometric mean **0.987** |

Workloads at 1.002 (yaml_config, mini_interp, django_blog) had no kernel fallbacks (yaml_config no
kernel activity at all, `proto_stats`): their +0.2% is the noise floor of whole-run instruction
counts (importing the recognizer shifts allocation and GC timing, **I**). Classes created before
`proto_enable` are not hooked (e.g. `tokenize.TokenInfo` when `tokenize` was already imported:
pylint 0.998 although `TokenInfo._make` is 4.1% of it); a production install at startup, or a
one-time adoption scan of `sys.modules`, would cover them.

## 5. Mechanism facts (for ACCELERATION / other slices)

1. **Python functions reached only from C slots behave like `ml_meth` sites**: `slot_tp_*` /
   `slot_nb_*` call them through `vectorcall`, so a kernel's fallback costs ~+3 Ir, not +180–260.
   Most enum, namedtuple-construction and dataclass dunder machinery is of this kind (**M**).
2. **Slots of stdlib heap types defined in Python** (`enum.property`, `EnumType`) can be swapped
   like static slots: `update_slot` replaces the kernel when the class is changed, subclasses get
   `slot_*` from `fixup_slot_dispatchers`, and no specialization compares them (**M**/**I** §4.1).
   Not every slot: `tp_new` of tuple subclasses is compared by pointer in `tp_new_wrapper` (§4.2).
3. **`CALL_ALLOC_AND_ENTER_INIT` bypasses vectorcall kernels on `__init__`** for positional calls
   (no function-version guard): relevant to any target that patches an `__init__` (wave-1 logging
   `LogRecord.__init__` — passed to w2-floors by the lead) (**M**).
4. **Generated code is provable**: re-running the generator's builder and comparing canonical
   code costs ~40 µs per class in Python (eager: too costly at import; lazy: ~free) (**M**).
5. **Negative `isinstance(x, C)`** costs a full `__class__` attribute lookup
   (`object_isinstance` → `PyObject_GetOptionalAttr`), 347 vs 171 Ir for a hit, ~60% of isinstance
   native time in the workloads (sqlglot 3.9%, pytest 1.6%, pyflakes 1.5%). No Rustique entry point
   (CALL_ISINSTANCE → `PyObject_IsInstance`; the slot belongs to the user's type); an upstream fast
   path (`tp_getattro` generic and `__class__` not overridden → `Py_TYPE(inst)`) would fix it (**M**,
   handed to w2-interp).
6. **Tooling**: wave-1's `_profiler.py` parser collects 0 samples with perf 7 (the sample header
   line is indented now; fixed in `profile_machinery.py`), and a shared box may delete
   `/tmp/perf-PID.map` mid-run: `profile_machinery.py` copies its own map through
   `/proc/self/fd` (**M**).

## 6. Candidates (brief format)

Score = I × F × (1−K) × (1−U) × (0.5+R), I = mean share (all 38, plain perf) × (1 − 1/s). The
"F+L+C" column is the frameworks+libs+custom mean (17 workloads).

| candidate | layer | entry points + mechanism | share all / F+L+C / max | s (on the part) | exact subset & fallback | floor vs current | risks | upstream | reach | prototype (M) | I | F | K | U | R | score |
|---|---|---|---|---:|---|---|---|---|---:|---|---:|---:|---:|---:|---:|---:|
| **enum** value lookup, Flag ops, `.value/.name`, `__hash__` | generated | 2 stdlib heap-type slot swaps + 6 C-reached vectorcalls | 0.42 / 0.66 / 5.0 rules_engine | 2.3 (op 1.9–4.8) | §4.1; misses, non-int/str values, user overrides, tools → original | lookup 515 vs 2,347 Ir; floor ≈ generic LOAD_ATTR/type call (~400 Ir) | private `_value2member_map_` tampering; FT facts cache | none (3.16 tunes Python) | 0.39 | 0.20–0.56 per op; rules_engine 0.964 | 0.24 | 0.9 | 0.15 | 0.1 | 0.39 | **0.15** |
| **namedtuple** `__new__`, `_make`, `_replace` | generated | hook on `namedtuple` + vectorcall (C-reached `__new__`; bytecode-reached `_make/_replace`) | 0.64 / 0.46 / 4.6 strawberry, 4.1 pylint | 1.6 (`_replace` 4.5) | lazy regenerate-and-compare; non tuple/list `_make` input, subclass overrides → original | 1,433 vs 2,198 Ir; floor = type_call + alloc | `_make(iterator)` fallbacks need §5.6 credit | none | 0.35 | 0.62–0.73, `_replace` 0.22; strawberry 0.967 | 0.24 | 0.8 | 0.15 | 0 | 0.35 | **0.14** |
| `inspect.getfullargspec`/`signature` for plain functions | stdlib | vectorcall | 0.67 / 0.93 / 11.4 falcon | ≥5 (**I**) | exact functions, no `__signature__`/`__wrapped__`/annotations → original | 106,964 / 93,675 Ir per call | `Signature`/`Parameter` are Python classes; annotations run user code | none | 0.19 | – | 0.54 | 0.3 | 0.35 | 0 | 0.19 | **0.07** |
| contextmanager construction | stdlib | hook on `contextmanager` + vectorcall on each helper | 0.36 / 0.55 / 8.5 rules_engine | 1.5 | generator functions with CPython vectorcall; `__enter__/__exit__` stay | 2,736 vs 4,181 Ir | helper fallbacks +180–260 Ir | none | 0.38 | `cm()` 0.65; rules_engine 0.951 | 0.13 | 0.75 | 0.2 | 0 | 0.38 | **0.07** |
| `re` module functions (`_compile` cache hit) | stdlib | vectorcall on 8 functions (called from bytecode) | 0.30 / 0.15 / 2.4 isort (incl. engine time) | 1.3 (measured) | §4.5; cache miss, templates/callables in `sub`, positional count → original | 3,451 vs 4,552 Ir; floor = engine + match object | template fallback 1.032; email_mime 1.004 (needs the §5.6 credit) | none | 0.49 | `re.match` 0.76, `re.I` 0.71, `sub` 0.81; fastapi_pydantic 0.985, django_admin 0.995, black 0.996, email_mime 1.004 (fallbacks) | 0.07 | 0.85 | 0.1 | 0 | 0.49 | **0.05** |
| `singledispatch` cache + `singledispatchmethod.__get__` | stdlib | hook on `singledispatch` + vectorcall on each `dispatch` (bytecode-called) + `__get__` (C-reached) | 0.90 / 2.02 / 34.3 event_ledger | 1.2 on the call (measured) | §4.4; `__call__`/`wrapper` stay (ruling 15) | `fmt(1)` 2,402 vs 3,131; `h.handle(1)` 8,495 vs 9,668 Ir | weakref state, ABC cache token | none | 0.03 | `dispatch` 0.58, `fmt(1)` 0.77, `h.handle(1)` 0.88; event_ledger 0.925 | 0.15 | 0.6 | 0.25 | 0 | 0.03 | **0.04** |
| frozen dataclass `__init__` (keyword calls) | generated | hook on `_process_class` + vectorcall | 1.31 / 2.39 / 21.5 (reachable ≈ 30%: frozen + keyword, **I**) | 2.0 | §4.2; positional calls bypassed | 2,690 vs 5,308 Ir | positional bypass; per-class plans miss subclasses | upstream wrapper_descriptor vectorcall would cover it | 0.29 | 0.51; event_ledger 0.943 | 0.19 | 0.5 | 0.25 | 0.3 | 0.29 | **0.04** |
| email / `http.client.parse_headers` (wave-1 #1 next) | stdlib | vectorcall | 3.22+0.27 / 0.17 / 95 email_mime | 5 (wave 1) | wave 1 | 27.5 µs per response (wave 1) | policies, header classes | none | 0.08 | – | 2.8 | 0.3 | 0.5 | 0 | 0.08 | **0.24** (one workload) |

Below 0.03 (measured, not worth a workstream now): `abc` `__instancecheck__` wrapper (0.26%,
one Python frame of ~1,750 Ir; `_abc_instancecheck` may call user `__subclasshook__`),
`typing._ProtocolMeta.__instancecheck__` (0.07%; a miss costs 23,568 Ir — big per call, rare),
uuid (0.30%, C `uuid` in 3.16: U = 0.7), `html.escape` (0.05%), `os.environ` (0.08%), base64/hmac
(≤ 0.07%, thin wrappers over C).

## 7. Settled / out of reach, and why

| item | share (M) | why |
|---|---:|---|
| `_GeneratorContextManager.__enter__/__exit__`, `_singledispatchmethod_get.__call__`, `singledispatch` `wrapper`, `total_ordering` methods, `cached_property.__get__`, `collections.abc` mixins (`Mapping.get`…), `html.parser.HTMLParser.goahead/parse_starttag` | 0.45, ~0.5, 0.04, 0.07, 0.17, 0.6 | each calls user code (generator, registered impl, user `__lt__`, user getter, user `__getitem__`, user `handle_*`) from its Python frame: a kernel would remove that frame from tracebacks and `sys._getframe` (ruling 15). Only parts before the user call are reachable (§6). |
| dataclass `__init__` positional calls | most of 1.31 | `CALL_ALLOC_AND_ENTER_INIT` inlines the code; no entry point is consulted (§4.2). |
| non-frozen dataclass `__init__` | – | specialized STORE_ATTR_INSTANCE_VALUE is already at the floor: kernel 1.00–1.016 (**M**). |
| dataclass `__eq__/__hash__/__repr__`, order methods | 0.06 | small share; `__eq__`/`__lt__` compare field values that may be user objects (ruling 15 unless all fields are builtin) |
| `__post_init__` dataclasses | (order_domain's `Money`) | the generated `__init__` calls user code last (ruling 15). |
| getattr/hasattr/setattr, str methods, dict/list ops, `str.format`/`%`, `int()/float()` | 1.43, 1.25, 6.3, 0.27, 0.09 | already C; a Rust copy does the same lookups (wave 1: str kernels ~1.0× at real sizes). |
| negative `isinstance` | ~0.55 | no entry point (§5.5): upstream / w2-interp. |
| `typing.cast`, `NewType` instances | ≤ 0.03 | a 328-Ir call; a kernel saves ~50 Ir. |
| tokenize glue | 0.41 | it is a generator (never replaced, ACCELERATION §2.6); its `TokenInfo._make` is taken by the namedtuple kernel. |
| asyncio | 2.81 | scheduling order is observable (wave 1, K = 0.6). |
| import-time machinery (dataclass/enum creation, `re.compile`) | – | w2-startup. |

## 8. Recommendation

1. **Enum group (Phase 2)**: the measured kernels plus `IntFlag`/`StrEnum` value paths, a dict
   watcher on `_value2member_map_`, FT-safe per-class facts, fingerprints for 3.14 and 3.15. Cheap
   fallbacks, 39% reach, 3.6% on the enum-heaviest workload.
2. **Generated-code group**: namedtuple `__new__`/`_make`/`_replace` now; frozen-dataclass
   `__init__` behind the same hooks with a Rust structural verifier, and propose the
   `wrapper_descriptor` vectorcall upstream (it would also speed up positional frozen calls, which
   no kernel can reach).
3. **Contextmanager construction** and **`re` module functions** as small additions to the
   stdlib-utilities workstream (both prototyped: `cm()` 0.65, `re.match` 0.71–0.76; `re` with
   dependency pins and the replacement check moved first). The **singledispatch cache** only if
   the workload set grows dispatch-heavy programs (3% reach); `inspect.signature` only if a
   framework needs it (falcon spends 15% in one `getfullargspec` per request).
4. Change to the contract to consider: a sentence in ACCELERATION §3 that Python functions
   reached only from C slots, and Python-defined stdlib heap-type slots, are first-class sites
   with `ml_meth`-like fallback cost, and that `__init__` kernels are bypassed by
   `CALL_ALLOC_AND_ENTER_INIT`.

## 9. Reproduce

```bash
R=~/rustique-research/w2/devbox/rbox
$R w2-machinery build
$R w2-machinery run -- python benches/machinery/profile_machinery.py profile <workload.py> --out m.json
python benches/machinery/report.py prof*/*.json                 # §2.3, §2.4
python benches/machinery/layers_share.py layers/*.json          # §2.1 (w2-attribution layers.py plain output)
python benches/machinery/static_reach.py                        # §2.2
$R w2-machinery run -- python benches/machinery/cells.py --variants plain,proto   # §3
$R w2-machinery run -- python benches/machinery/check_proto.py                     # 0 differences
$R w2-machinery run -- python benches/machinery/wl_instr.py <workload.py> --proto enum   # §4 (groups: enum, dc, nt, ctx, sd, re, all)
$R w2-machinery run -- python benches/machinery/import_cost.py                     # §4.2
```
