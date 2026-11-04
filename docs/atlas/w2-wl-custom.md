STATUS: wave 2, Phase 1, slice `w2-wl-custom`, 2026-11-02. Branch `w2/p1-wl-custom`.
Every number is **M** (measured: how and where is given) or **I** (inferred: from what is given).
Raw data: `~/rustique-research/w2/wl-custom/` (profiles `prof/3.15.1{,t}/*.json`,
`summary.md`, `compare-3.15.1{,t}.json`, `machinery-3.15.1.json`).

# Complex custom software: where the time goes, and which CPython machinery it leans on

## 1. The answer in brief

1. **Custom applications spend 40% of their time in their own bytecode and 24% in
   language-level native work** (attribute lookup, dict, tuple/list/set, comparisons/hashing,
   calls, dispatching user dunders from type slots). Another 15% goes to allocation and GC,
   and only 6% to generated code and 15% to the stdlib (M, mean of 10 workloads,
   §4). Exceptions used as control flow cost almost nothing: ≤ 0.8% leaf time, and 0.16% in
   the interpreter that returns through exceptions (M).
2. **Code that Python generates matters in data-model-heavy programs.** It is 5.8% of the
   mean and up to **16.5%** (order_domain). Nearly all of it is **frozen-dataclass
   `__init__`**: a frozen dataclass costs **2.8× the instructions** of the same mutable
   dataclass or a hand-written slotted class (2,774 vs 976 Ir for 2 fields; 9,198 vs 3,898 Ir
   for 6 keyword-only fields) (M, §5.2). Mutable-dataclass `__init__` is already at the
   plain-class floor (976 vs 971 Ir).
3. **Some stdlib machinery costs 3–30× its plain equivalent and dominates the programs that
   use it.** `singledispatchmethod` costs 9,749 Ir per call against 339 Ir for a plain method,
   and is 40% of event_ledger. `@contextmanager` costs 8,089 vs 989 Ir and is 20% of
   rules_engine. `inspect.signature` plus `typing.get_type_hints` (66k and 45k Ir per call)
   are 31% of di_plugins. Enum member lookup, `.value` and `hash()` cost 5–12× a dict or
   slot access. `WeakValueDictionary.get` costs 2.8× `dict.get` (M, §5).
4. **Ruling 15 decides most of what is reachable.** Most of this machinery wraps user code
   (`contextmanager.__enter__/__exit__` resume the user's generator, singledispatch calls the
   implementation, `total_ordering` and `cached_property` call user methods). An exact
   kernel can take only the bookkeeping around those calls: about 50% of contextlib's time
   and 55% of singledispatch's. Enum, dataclass `__eq__`/`__hash__`, weakref containers and
   frozen `__init__` without `__post_init__` are fully reachable (I, §7).
5. **Machinery reached through type slots is the cheapest to accelerate.** Enum
   `__hash__`, `.value` (a descriptor), `EnumType.__call__`, `Flag` operators, dataclass
   `__eq__`/`__hash__`/`__lt__` and `WeakKeyDictionary.__getitem__` are all called from C
   slots. A vectorcall kernel on them loses no inlined frame push, so their fallback costs
   ~+3 Ir, not +180–260 (I, from ACCELERATION §5.2).
6. **Rustique today: 0.90–1.05 by instructions per `run()`, 0.90–1.04 by wall time.** What
   fires is wave-1 targets the programs touch incidentally: `date.strftime`/`isoformat`,
   `copy.copy`, `min`/`max`/`sorted`, `tuple.__contains__`. None of the dominant machinery
   is covered. All 10 digests match plain vs enabled on 3.15.1 and 3.15.1t, and 3.15.1t
   digests equal 3.15.1's (M, §6).
7. **Free-threaded 3.15.1t costs 1.02–1.28× cycles (mean 1.13) on these single-threaded
   programs.** The cost is spread over the interpreter, dict lookups (`_threadsafe`
   variants) and attribute access. Two hotspots stand out: the cyclic GC walking mimalloc
   heaps (`mi_heap_visit_blocks` 9% of rules_engine) and `_decimal` fetching its context
   through `PyContextVar_Get` on every operation (3.5% of order_domain) (M, §8).

**Recommendation** (§9, scores in §7): build one **"generated & data-model machinery"**
workstream with four parts, in this order:

1. enum machinery kernels;
2. a frozen-dataclass `__init__` fast path;
3. the bookkeeping halves of `singledispatch`/`singledispatchmethod` and `contextmanager`;
4. weakref-container operations.

Each is an exact, slot- or vectorcall-reached kernel over stdlib/generated code. Together
they would save an estimated (I) ~5% of the mean custom program and 5–20% of the programs
that lean on one of them. Leave `inspect.signature`/`get_type_hints` to a framework-driven
decision: the payoff is large but the API surface is wide. The bigger prize, own bytecode
plus language-level work at 64%, belongs to w2-interp/w2-machinery.

## 2. The family

`benches/workloads/custom/` has 10 programs and 300–900 lines each (README.md has one line
per workload). Every program is stdlib only, deterministic, and runs on 3.15.1 and 3.15.1t.

- **Run time**: median `run()` on 3.15.1 on the dev box (M, `run.py compare custom -n 5 -p 3`).
- **Rustique**: enabled/plain. Wall comes from the same compare and has ±10–15% noise
  (box load 25–70 on 32 cores). Instr is user instructions per `run()` (M,
  `perf stat` gated to the steady-state loop, `PYTHONHASHSEED=0`). The instruction counts
  carry ±1% noise, and up to ±5% when a full GC collection falls inside the few measured
  runs.
- **t penalty**: 3.15.1t / 3.15.1 plain, in cycles and wall per `run()` (M).

| workload | stands for | run() | Rustique wall / instr | t penalty cycles / wall |
|---|---|---:|---:|---:|
| order_domain | e-commerce order back office (DDD): frozen `Money` over `Decimal`, `StrEnum`/`IntEnum`/`Flag`, `__post_init__`, `@property` totals, ABC promotions, repositories, domain events, `match` | 1.35 s | 0.967 / 0.995 | 1.116 / 1.102 |
| event_ledger | event-sourced ledger (CQRS): command/event dataclass hierarchies, `match` class patterns, `singledispatchmethod` aggregate rehydrated per command, `singledispatch` projections, replay | 1.58 s | 1.112 / 0.988 | 1.141 / 1.124 |
| fulfilment_sim | SimPy-style discrete-event simulation: `heapq` of `total_ordering` slotted entries, generator processes (`send`/`throw`), resources as context managers, preemption by exceptions | 1.35 s | 1.006 / 0.970 | 1.131 / 1.113 |
| mini_interp | in-house DSL interpreter: `re` lexer, `NamedTuple` tokens, recursive-descent parser, `__slots__` AST, `getattr("visit_"+name)` visitors, closures, return/break as exceptions | 1.14 s | 0.997 / 1.010 ¹ | 1.140 / 1.144 |
| rules_engine | loan underwriting service: decorator rule registry, `runtime_checkable` Protocols, ABC condition DSL with operator overloading, `cached_property`, `lru_cache`, `@contextmanager` audit, `IntFlag`, workflow | 0.94 s | 0.985 / 0.998 | 1.283 / 1.238 |
| orm_mapper | home-grown ORM over `sqlite3`: descriptor fields, metaclass + `__init_subclass__`, expression builder, `copy.copy` clones, `WeakValueDictionary` identity map, `__getattr__` lazy proxies, unit of work | 0.85 s | 0.944 / 0.896 | 1.019 / 1.149 |
| di_plugins | DI container + pluggy-style plugins: `inspect.signature` + `get_type_hints` autowiring, lifetimes/scopes, call-site injection, hook calls, `WeakMethod` bus | 0.85 s | 1.005 / 1.030 ² | 1.125 / 1.157 |
| task_planner | monorepo build/CI planner: namedtuples, `defaultdict(set)` DAG, Kahn/BFS on `deque`, set algebra, `heapq` list scheduling, `sorted(key=)`, `itertools` | 0.53 s | 1.042 / 1.029 ² | 1.172 / 1.027 |
| ecs_arena | entity-component-system game/agent sim: `Vec2` arithmetic dunders, slotted dataclass components, frozen grid cells as dict keys, spatial hash, enum state machine | 1.04 s | 0.991 / 0.987 | 1.096 / 1.241 |
| report_engine | statements from an in-house template engine: templates compiled to closures, getattr→getitem lookups, `ChainMap`, `__format__` value objects, `format_map`, `html.escape` | 0.96 s | 0.899 / 0.918 | 1.104 / 1.161 |

¹ Measured in a controlled repeat (M, the same gated loop, 2/4/8/12 runs, seed 0): the
marginal cost per `run()` was 14.72–14.79 G instructions plain and 14.79–14.92 G enabled,
i.e. **+0.5–1.0% instructions**, while cycles were equal (4.00 vs 4.00–4.04 G). Earlier
single 8-second measurements showed 1.05 (GIL) and 1.12 (t). Those include a full GC
collection that falls on one side only. In whole-process runs with a fixed seed, enabled
equals plain to 1e-5 on 3.15.1t. Only `tuple.__contains__` fires: 9,539 hits per run, no
fallbacks. **Not a never-slower violation, but worth a callgrind check by w2-floors.**
² Same caveat: in whole-process runs with a fixed seed, di_plugins is 0.950 and
task_planner 0.970 (those runs include `setup()`, which the `random` targets speed up). The
gated instruction counts land within ±3% of 1.00.

## 3. Method (how to reproduce)

All runs are on the dev box (c7a.8xlarge, shared) with the python-build-standalone 3.15.1 /
3.15.1t interpreters (JIT built but off), Rustique at this branch (`rbox w2-wl-custom build`).

- **Layers**: `benches/workloads/custom/_profile_custom.py <wl> --out p.json --stat-enable`
  (built on wave 1's `_profiler.py` driver and classifier).
  1. Plain `perf record -e cpu-clock -F 1999 -g`, gated to steady-state `run()` calls,
     ≥ 11.6k samples per workload. This gives the **undistorted leaf-native-component split**.
  2. The same with `-X perf`, ≥ 11.7k samples. Each sample's innermost Python frame gets
     an owner by **code-object provenance**. `<string>`/`__create_fn__` is
     generated:dataclass; `<string>`/`<lambda>` and `collections/__init__.py:namedtuple.<locals>`
     are generated:namedtuple; `functools._*_from_*` is generated:total_ordering;
     `enum.py` is generated:enum ("enum machinery acting on user enums"); the reprlib wrapper
     of dataclass `__repr__` is generated:dataclass; stdlib paths are stdlib:<module>; the
     workload file is own.
  3. The owner × component matrix is **re-weighted per component** so its component
     marginal equals the plain profile's. The trampoline's own samples are folded into
     "interpreter" first.

  `-X perf` inflates the `calls` component about **2×** (re-weight 0.50, range 0.12–0.71)
  and leaves interpreter/attr/memory near 1.0 (1.01 / 1.02 / 1.06). Raw and re-weighted
  layers differ by ≤ 3 points. Python-function names (top-function lists, inclusive
  shares) are taken from `-X perf` and say so.
  - Layer rules: runtime = memory/gc/import/kernel components, or any sample under a GC
    frame, for any owner. generated / stdlib = everything else under those owners. Under
    own frames, interpreter components count as own, protocol/builtin components as
    language, and other C modules (`_decimal`, `_heapq`, sqlite, …) as stdlib.
  - Caveat: orm_mapper's "other" component (8.3%) is SQLite static functions (`pcache1Fetch`,
    `btreeParseCellPtr`, `balance`, `likeFunc`, …) that the classifier does not name. §4
    moves it from language to stdlib.
- **cProfile per code object**: the `cprofile` step reads `_lsprof` entries per code
  object. pstats merges entries that share a (file, line, name) label, which undercounts
  exec-generated methods by orders of magnitude: 65k vs 1.6M dataclass calls in
  order_domain. Generated methods are named after their class, and frozen `__init__`
  methods are tagged.
- **Instructions**: the `stat` step runs `perf stat -e instructions:u,cycles:u` gated the
  same way, with `PYTHONHASHSEED=0`; all counters ran 100% of the time.
- **Machinery floors**: `_machinery_bench.py` measures user instructions per operation in a
  hot function loop (`perf stat`, n = 1M, min of 3, loop and process subtracted), each
  case next to its plain-Python floor. Output: `machinery-3.15.1.json`.
- **Summary tables**: `_summarize_custom.py --prof prof/3.15.1 --prof-t prof/3.15.1t
  --compare compare-3.15.1.json --compare-t compare-3.15.1t.json`.
- **Reach**: wave 1's corpus of the top 1000 PyPI packages
  (`~/rustique-research/static/results/scan`, download-weighted share of packages whose
  library code uses the feature). Frozen dataclasses come from a grep for
  `dataclass(...frozen=True` on one line, which undercounts.
- **Perf caveats**: perf 7 right-aligns the tid column, so wave 1's `PerfParser` silently
  collects 0 samples on the dev box (fixed by the `Parser` subclass here; lead notified).
  The w2-startup `/tmp/perf-*.map` deletion incident did not affect these profiles: every
  `-X perf` sample resolved a Python frame (0.0% `<no-python-frame>`).

## 4. Layers per workload (M, 3.15.1)

| workload | own | generated | language | stdlib | thirdparty | runtime (of which GC) |
|---|---:|---:|---:|---:|---:|---:|
| order_domain | 35.1% | **16.5%** | 18.5% | 8.8% | 0 | 21.1% (8.4%) |
| event_ledger | 30.0% | 3.6% | 17.7% | **35.3%** | 0 | 13.4% (2.9%) |
| fulfilment_sim | **59.9%** | 1.4% | 20.4% | 6.4% | 0 | 11.8% (2.4%) |
| mini_interp | **58.7%** | 0.0% | 32.2% | 0.2% | 0 | 8.9% (0.2%) |
| rules_engine | 25.9% | 14.3% | 13.7% | 21.4% | 0 | **24.7%** (10.6%) |
| orm_mapper ³ | 23.0% | 2.1% | 24.9% | **36.3%** | 0 | 13.6% (3.9%) |
| di_plugins | 34.0% | 3.6% | 19.8% | 27.2% | 0 | 15.5% (0.0%) |
| task_planner | 41.6% | 0.4% | **41.0%** | 5.9% | 0 | 11.1% (1.4%) |
| ecs_arena | 54.3% | 7.9% | 24.6% | 1.0% | 0 | 12.2% (0.0%) |
| report_engine | 36.2% | 8.6% | 25.9% | 13.3% | 0 | 16.0% (0.0%) |
| **mean** | **39.9%** | **5.8%** | **23.9%** | **15.5%** | 0 | **14.8% (3.0%)** |

³ SQLite statics moved from language to stdlib (§3). The SQLite engine totals ~26%.

Native components (plain perf, mean): interpreter 49.6%, memory/gc 14.6%, attr/type 10.9%,
dict 6.3%, list/tuple/set 2.9%, calls 2.8%, str 1.8%, sqlite 1.7%, decimal 1.7% (M,
`summary.md`). Compared with wave 1's stdlib-heavy set (46% interpreter + calls + attribute,
16% alloc + GC), custom software has the same runtime share and a **larger** interpreter +
language share (64% own + language). The stdlib shrinks from wave 1's "reachable third"
to 15%, and generated code appears as a new 6% layer.

GC is 0% in ecs_arena, di_plugins and report_engine even though they allocate millions of
objects. 3.15's young-generation trigger counts net growth, so short-lived temporaries
(`Vec2`, signatures, format strings) never start a collection. GC appears where programs
keep growing object graphs (audit trails, event logs): rules_engine 10.6%, order_domain
8.4% (M).

## 5. Which CPython machinery dominates

### 5.1 Per workload (M, exclusive share of `run()`; owners re-weighted, language buckets = plain-perf leaf time)

"cp" marks a cProfile tottime share. cProfile is per code object; its overhead per call
inflates cheap, frequently called functions somewhat, compared with the perf shares.

| workload | top CPython-provided machinery |
|---|---|
| order_domain | **frozen dataclass `__init__` 13.4% cp** (`Money`, 1.40M calls/run, has `__post_init__`), `_decimal` 8.7%, GC 8.4% + alloc 12.2%, user-dunder slot dispatch 5.0% (`slot_nb_add`/`slot_nb_multiply`: 41% inclusive), enum 2.6% (`Flag.__and__` 42k, `EnumType.__call__` 68k, `.value` 90k calls) |
| event_ledger | **`functools` 33.6% + `weakref` 6.2%**: `_singledispatchmethod_get.__call__` 15.2%, `.__init__` 7.4%, `WeakKeyDictionary.__getitem__` 5.6%, `dispatch` 3.8%, `singledispatchmethod.__get__` 3.7% (-X perf self); 3.6M functools calls/run; frozen-dataclass init 2.4% (cProfile) |
| fulfilment_sim | heap comparisons through `slot_tp_richcompare` (40.5% inclusive, comparison/hash leaf 6.6%), `random` distributions 3.6% (`normalvariate`, `expovariate`, `lognormvariate`, `paretovariate`), `_heapq` 2.4%, `total_ordering` `_ge_from_lt` 1.4% (400k calls) |
| mini_interp | dynamic-name `getattr(self, "visit_" + name, None)`: `PyObject_GetOptionalAttr` 10.9% leaf + string hashing (`pysiphash`) 3.6% + `PyUnicode_Concat` 3.9%; dict 5.1%; no generated or stdlib machinery |
| rules_engine | **`@contextmanager` 20.0%** (1.47M contextlib calls/run), **frozen dataclass `__init__` (`Outcome`) 5.4% cp / 13.1% gen layer**, GC 10.6%, enum 4.4% (`Flag.__or__`/`__contains__`, `Enum.__hash__`), `inspect.getattr_static` via `runtime_checkable` isinstance 2.4%, `cached_property` 1.7% |
| orm_mapper | SQLite ~26%, **`WeakValueDictionary` 6.4%** (`get`, `__setitem__`, `KeyedRef.__new__/__init__`), enum 2.1% (`Status(value)` on hydration), `copy.copy` 1.5% |
| di_plugins | **`inspect` 20.6% + `typing` 8.8% + `annotationlib` 1.8%** (signature/type-hint analysis per registration and per injected call), enum 1.6% (`.value` 190k calls), frozen `Dependency` init 1.0% |
| task_planner | language only: list/tuple/set 12.1%, dict 11.7%, comparison/hash 10.0% (tuple keys in heaps/sorts), `_heapq` 3.9%, `Counter` methods 1.2% |
| ecs_arena | frozen grid `Cell`: `__init__` 3.5% + `__hash__` 2.2% + `__eq__` 1.0% (cProfile; generated layer 7.9%), dict 9.8%, `slot_nb_subtract` (user `Vec2.__sub__`) 10.6% inclusive |
| report_engine | **frozen `Money.__init__` 7.4% cp** (902k calls), `_decimal` 7.7%, `ChainMap.__getitem__` 2.2%, `html.escape` 1.3%, str 5.0% |

**Family mean** (M): alloc/dealloc 11.6%, dict ops 6.3%, generated dataclass code 5.7%,
generic attribute lookup 5.4%, `functools` 3.5%, cyclic GC 3.0%, list/tuple/set 2.9%,
user-dunder slot dispatch 2.7%, calls 2.3%, `inspect` 2.3%, comparison/hash 2.2%,
`contextlib` 2.0%, descriptors/property 1.8%, str 1.8%, `_decimal` 1.6%, `weakref` 1.4%,
enum 1.1%, `typing` 0.9%; exceptions 0.2%, isinstance (specialized) 0.2%.

### 5.2 What each piece of machinery costs against its floor (M, instructions per operation, 3.15.1)

| machinery | current Ir | plain-Python floor | Ir | ratio |
|---|---:|---|---:|---:|
| frozen+slots dataclass, 2 fields: `F2(1,'y')` | 2,774 | same as mutable dataclass / slotted class | 976 / 971 | 2.84× |
| frozen+slots dataclass, 6 kw fields | 9,198 | mutable dataclass, 6 kw fields | 3,898 | 2.36× |
| frozen dataclass without slots, 2 fields | 2,851 | — | — | — |
| dataclass `__eq__` / `__hash__` / `order=True __lt__` | 1,236 / 1,491 / 1,800 | hand-written `__eq__`/`__hash__`; inline tuple compare | 1,320 / 1,491; 829 | ≈1 vs hand-written |
| `C(2)` (enum by value), `S('b')` | 2,376 / 2,381 | dict lookup | 211 | 11× |
| `member.value` | 1,150 | slot attribute | 94 | 12× |
| `hash(member)` / `d[member]` | 1,231 / 1,057 | `d[int]` | 207 | 5.1× |
| `Flag` `\|` / `&` / `in` | 5,506 / 5,472 / 1,134 | int ops | ~100 (I) | 11–55× |
| `singledispatch` call | 3,189 | plain function call | 298 | 10.7× |
| `singledispatchmethod` call | 9,749 | plain method call | 339 | 28.8× |
| `with @contextmanager` | 8,089 | class-based CM | 989 | 8.2× |
| `isinstance(x, runtime_checkable Protocol)` | 3,394 | ABC subclass / plain class | 1,702 / 217 | 15.6× |
| `cached_property` first access (+ new instance) | 3,275 | `property` (+ new instance) | 858 | 3.8× |
| `total_ordering` `__ge__` | 1,315 | direct `__lt__` | 770 | 1.7× |
| `WeakValueDictionary.get` / `WeakKeyDictionary[k]` | 908 / 1,658 | `dict.get` / `dict[k]` | 326 / 191 | 2.8× / 8.7× |
| `ChainMap[k]` (3 maps, hit in the last) | 3,790 | `dict[k]` | 191 | 20× |
| `inspect.signature(f)` / `typing.get_type_hints(f)` | 66,420 / 45,467 | — | — | — |

## 6. Type-slot dispatch of user dunders (M, inclusive, plain-perf stacks)

User dunders called from C type slots carry much of the time:

| workload | dispatch through type slots (inclusive) |
|---|---|
| order_domain | `slot_nb_multiply` 20.5%, `slot_nb_add` 20.3% (`Money` arithmetic) |
| fulfilment_sim | `slot_tp_richcompare` 40.5% (`heapq` comparing `__lt__` entries) |
| orm_mapper | `slot_tp_iter` 50.6% (`Query.__iter__`), `slot_tp_descr_set` 6.3% (field descriptors) |
| event_ledger | `slot_tp_descr_get` 12.2% (`singledispatchmethod.__get__`) |
| ecs_arena | `slot_nb_subtract` 10.6% |
| di_plugins, mini_interp | `slot_tp_call`: user `__call__` objects |

The dispatch overhead itself is the leaf time in `slot_*`, `vectorcall_method` and
`lookup_maybe_method`: **2.7% mean, 5.0% max** (order_domain). CPython 3.15 does not
specialize `BINARY_OP`/`COMPARE_OP` for Python-defined dunders, so every `Money + Money`
goes through `slot_nb_add` and a method lookup. This is language/interpreter territory
(w2-machinery/w2-interp). §7 lists it with its numbers.

## 7. Candidates

Scored as `I × F × (1 − K) × (1 − U) × (0.5 + R)`, with I = mean share × (1 − 1/s) in
percent. Shares are re-weighted (M); s, F, K, U are my estimates (I) with reasons in the
row; R is download-weighted library reach (M, wave-1 corpus).

| candidate | layer | entry points + mechanism | share mean / max | s | exact subset & falls back | floor vs current | risks | upstream | reach | prototype | evidence |
|---|---|---|---|---:|---|---|---|---|---|---|---|
| **C1 enum machinery** | generated | `Enum.__hash__`, `enum.property.__get__` (+ `Enum.value`/`name`), `EnumType.__call__` + `Enum.__new__` value hit, `Flag.__or__/__and__/__xor__/__invert__/__contains__`/`_get_value`; vectorcall on the stdlib functions, **all reached from C slots** (no inlining lost) | 1.1% / 4.4% (rules), 2.6% order, 2.1% orm, 1.6% di | 3 | members of enums with no override of the touched dunder, `__new__`, `_missing_`; value hashable by C (int/str/tuple/…); `Flag` results already in `_value2member_map_`; falls back: subclass overrides, pseudo-member creation, unhashable/user-hashed values | 211 / 94 / 207 Ir vs 2,376 / 1,150 / 1,057 Ir (§5.2) | enum.py internals change per version (fingerprint code objects, refuse on mismatch); `_value2member_map_` mutation (aliases, `_add_member_`); FT: read class dicts with the safe lookup API | 0.2 (no C enum planned; enum.py is tuned occasionally) | 38.5% (enum classes) | — | §5.1/5.2; cProfile calls: rules 685k, order 629k, di 517k enum calls/run |
| **C2 frozen-dataclass `__init__`** | generated | the class's generated `__init__` (vectorcall on the function, or its `tp_init`): set fields with `PyObject_GenericSetAttr` exactly as `object.__setattr__` does, apply defaults | 3.0% eligible / 10.3% (rules); report 9.1, ecs 4.6, event 3.8 (all frozen init 4.3% / 13.4%) | 2.0 | code object byte-identical to what `dataclasses` generates for the class's fields (regenerate and compare at install); **no `__post_init__`** (ruling 15: the generated frame sits between caller and `__post_init__`), no `InitVar`, defaults = values or C-type factories; any binding error → original raises; classes outside the subset are never installed (no fallback cost) | ~1,150–1,400 Ir (mutable 976 + lost `CALL_ALLOC_AND_ENTER_INIT` inlining) vs 2,774 Ir (2 fields); 9,198 Ir (6 kw fields) | losing the specializer's inlined init on installed classes (+180–260 Ir, inside the floor estimate); `__setattr__` hackcheck equivalence; class redefinition/`__init__` replacement (pin) | 0.3 (a C helper for frozen init is a natural upstream fix) | ~20% (frozen; grep) / 28.6% (`@dataclass`) | — | §5.2 microbench; cProfile frozen-init shares |
| **C3 singledispatch(method) bookkeeping** | stdlib | `singledispatch.<locals>.dispatch` (incl. the `WeakKeyDictionary` cache hit and the `cache_token` check), `_singledispatchmethod_get.__init__`, `singledispatchmethod.__get__` (slot-reached); vectorcall. `wrapper` and `__call__` **stay Python** (they call the implementation: ruling 15) | 4.0% / 39.7% (event) | 1.8 | dispatch on `args[0].__class__` = exact type (no `__class__` override), cache hit, token unchanged; plain-function implementations; falls back: misses, ABC-registered changes | kernelable part ≈ 55% of the functools+weakref time; whole call 9,749 → ~3,500 Ir (I) | the `dispatch_cache` stays the public `WeakKeyDictionary`: the kernel must read it, not shadow it; `register()` after first call; FT | 0.4 (singledispatchmethod perf actively worked on: GH-143535) | 3.2% | — | event_ledger -X perf self list (§5.1) |
| **C4 contextmanager bookkeeping** | stdlib | `contextmanager.<locals>.helper` + `_GeneratorContextManagerBase.__init__` (vectorcall); `__enter__`/`__exit__` **stay Python** (resume the user generator) | 2.0% / 20.0% (rules) | 1.6 | `func` is a plain generator function (creating the generator runs no frames), no `__doc__` oddities; falls back: non-generator callables, subclasses | ~49% of contextlib time kernelable; `with` 8,089 → ~5,000 Ir (I) | helper is called from bytecode (inlined): +180–260 Ir per call against ~3,000 Ir saved | 0.2 | **38.1%** | — | rules_engine -X perf self: `__exit__` 5.9, `__init__` 5.7, `__enter__` 3.0, `helper` 2.9 |
| **C5 `inspect.signature` / `typing.get_type_hints`, plain functions** | stdlib | `inspect.signature`, `Signature.from_callable`, `typing.get_type_hints`; vectorcall (calls ≥ 45k Ir: qualifies under §5.2a) | 3.1% / 31.2% (di) | 3 | `FunctionType` / bound methods without `__signature__`, `__wrapped__`, `__text_signature__`; annotations already materialized (no `__annotate__` call: ruling 15) and made of classes / `None` / `X \| None` / `list[T]`; Parameter/Signature objects built exactly (same classes, same slots); falls back: strings, ForwardRefs, `eval_str`, partials, classes | ~15–20k Ir (object construction stays) vs 66k / 45k Ir (I) | big API surface (annotation formats, validation, `follow_wrapped`); every new CPython version | 0.2 | 18.9% (signature); frameworks call it per route/fixture/hook | — | di_plugins §5.1; machinery bench |
| **C6 weakref containers** | stdlib | `WeakValueDictionary.get/__getitem__/__setitem__/__contains__`, `WeakKeyDictionary.__getitem__/__setitem__/get`, `KeyedRef.__new__/__init__`; vectorcall (`[]` is slot-reached; `.get` loses inlining) | 1.4% / 6.4% (orm; event's 6.2% is inside C3) | 2 | exact types, `_pending_removals` empty, keys hashed by C (str/int/tuple/types); falls back: user `__hash__`/`__eq__` keys, subclasses, iteration-in-progress guard | dict.get 326 vs 908 Ir; dict[k] 191 vs 1,658 Ir | callback/removal ordering (`_remove` closures), FT (`_IterationGuard`) | 0.1 | 23.1% | — | orm/event -X perf self |
| C7 dataclass `__eq__`/`__hash__`/`__lt__` | generated | the generated methods, slot-reached; vectorcall | 0.5% / 4.2% (ecs) | 2.5 | field values of exact builtin types; else falls back (~+3 Ir) | ~400–500 Ir (I) vs 1,236 / 1,491 / 1,800 Ir | field types with Python `__eq__` (ruling 15 → fall back) | 0.2 | 28.6% | — | ecs cProfile: `Cell.__hash__` 467k, `__eq__` 304k calls |
| C8 user-dunder slot dispatch | language | per-class fast slot (`nb_add`, `tp_richcompare`, …) caching the Python dunder by type version; or interpreter specialization | 2.7% / 5.0% (order) leaf | 1.7 | heap types whose dunder is a plain function; falls back on any attribute change (type version) | — | `update_slot` interplay, reflected operands, subclass priority | 0.5 (natural specializer work) | ~all OO code | — | §6 |
| C9 `getattr` with a fresh (non-interned) name | language | `builtins.getattr` (`ml_meth` swap): map the name to its interned twin so `_PyType_Lookup` hits the type cache | ~1.6% / ~11% (mini) | 1.3 (I, unprototyped) | exact `str` names; same lookup otherwise | — | none known; benefit unproven | 0.3 | `getattr` 86.6% (with isinstance/hasattr) | — | mini_interp leaf `PyObject_GetOptionalAttr` 10.9% |
| C10 `ChainMap.__getitem__` | stdlib | vectorcall, slot-reached | 0.3% / 2.8% (report) | 5 | exact ChainMap over exact dicts with C-hashed keys | 191 vs 3,790 Ir | low | 0.1 | low (ChainMap rare) | — | report_engine |

Scores (I):

| candidate | I | F | K | U | R | score |
|---|---:|---:|---:|---:|---:|---:|
| C2 frozen `__init__` | 1.50 | 0.70 | 0.25 | 0.3 | 0.20 | **0.39** |
| C5 signature / type hints | 2.08 | 0.50 | 0.35 | 0.2 | 0.19 | **0.37** |
| C1 enum machinery | 0.73 | 0.80 | 0.25 | 0.2 | 0.385 | **0.31** |
| C3 singledispatch | 1.78 | 0.75 | 0.30 | 0.4 | 0.03 | **0.30** |
| C4 contextmanager | 0.75 | 0.75 | 0.25 | 0.2 | 0.38 | **0.30** |
| C6 weakref containers | 0.70 | 0.80 | 0.30 | 0.1 | 0.23 | **0.26** |
| C8 slot dispatch | 1.08 | 0.50 | 0.40 | 0.5 | 0.6 | 0.18 |
| C9 dynamic getattr | 0.37 | 0.60 | 0.20 | 0.3 | 0.87 | 0.17 |
| C7 dataclass eq/hash/lt | 0.30 | 0.80 | 0.20 | 0.2 | 0.29 | 0.12 |
| C10 ChainMap | 0.24 | 0.90 | 0.10 | 0.1 | 0.05 | 0.10 |

On this family alone, C1 + C2 + C3 + C4 + C6 + C7 would save about 5% of the mean
`run()` (5.4%). They would save 5–20% of the programs that lean on one of them:
event_ledger ~20%, rules_engine ~15%, report_engine ~5%, ecs_arena ~5%, orm_mapper ~5%,
di_plugins ~2.5% without C5 (I, from shares × (1 − 1/s)).

**Hand-offs**:

- **w2-floors** (coverage gap): `random` distributions (`normalvariate`, `expovariate`,
  `lognormvariate`, `paretovariate`) are 3.6% of fulfilment_sim. They are uncovered;
  exactness needs the same libm calls.
- **w2-threads**: `_decimal`'s `PyContextVar_Get` per operation on 3.15.1t.
- **w2-runtime**: GC policy, since GC is 10.6% of rules_engine (13.1% on t).

## 8. Free-threaded 3.15.1t (M, plain perf × cycles per run, gated `perf stat`)

| workload | cycles t/GIL | instr t/GIL | where the extra cycles go (Gcycles per run) |
|---|---:|---:|---|
| rules_engine | **1.283** | 1.117 | memory/gc +0.38 (`mi_heap_visit_blocks` 9.0% of t time: the FT GC walks mimalloc heaps), interpreter +0.13, dict +0.12, `_Py_TryIncrefCompareStackRef` 3.7% |
| task_planner | 1.172 | 1.142 | list/tuple/set +0.09, memory/gc +0.06, dict +0.05 |
| event_ledger | 1.141 | 1.122 | interpreter +0.35, dict +0.19, attr/type +0.14 (`PyMember_GetOne` 4.5%) |
| mini_interp | 1.140 | 1.133 | interpreter +0.41, dict +0.25 (`_Py_dict_lookup_threadsafe*` 7.9%) |
| fulfilment_sim | 1.131 | 1.084 | interpreter +0.26, attr/type +0.10 |
| di_plugins | 1.125 | 1.124 | interpreter +0.15, attr/type +0.07 |
| order_domain | 1.116 | 1.096 | interpreter +0.19, other-objects +0.19 (**`PyContextVar_Get` 3.5%**: `_decimal` looks up its context per operation on t), attr/type +0.11 |
| report_engine | 1.104 | 1.077 | attr/type +0.15 (`_decimal` context lookup 2.3% again) |
| ecs_arena | 1.096 | 1.114 | list/tuple/set +0.08, attr/type +0.07 |
| orm_mapper | 1.019 | 0.999 | memory/gc +0.13 (SQLite time does not change) |
| **mean** | **1.133** | 1.101 | |

Single-threaded custom programs pay 2–28% on t. The penalty is spread thin, except GC
(rules_engine) and `_decimal` contexts. Rustique on t: digests equal. Single gated
instruction ratios range 0.92–1.12 (`prof/3.15.1t/*.json`). Whole-process, fixed-seed
counts (setup + 3 runs) do not reproduce the two values above 1.05: mini_interp 1.000,
rules_engine 0.979. Those two are GC-timing noise of the gated method, not a measured
slowdown. Wall ratios are 0.88–1.02 (`compare-3.15.1t.json`, noisy).

## 9. Settled / out of reach, because …

- **Own bytecode, 40% of the mean** (59–60% in mini_interp and fulfilment_sim): the
  program's own interpreter time is out of reach of entry-point kernels (w2-interp's
  question). mini_interp is the limiting case: an interpreter written in Python leaves
  nothing CPython-generated to take, apart from C9.
- **Mutable-dataclass `__init__`**: at the floor already (976 vs 971 Ir for a hand-written
  slotted class).
- **Dataclass `__eq__`/`__hash__` vs hand-written ones**: identical cost (1,236 vs 1,320;
  1,491 vs 1,491 Ir). Only a native kernel (C7) beats them, and the share is small.
- **`total_ordering` methods, `cached_property.__get__`, `contextmanager.__enter__/__exit__`,
  singledispatch `wrapper`/`__call__`**: each calls user code from its own frame. Ruling 15
  forbids a native kernel there, and the bookkeeping around the call is all that is
  reachable (C3, C4). `total_ordering` costs +545 Ir per derived comparison (1.4% of
  fulfilment_sim): settled.
- **Frozen dataclasses with `__post_init__`** (order_domain's `Money`, the family's largest
  single generated-code cost at 13.4%): the generated frame must stay between caller and
  `__post_init__` (ruling 15). Out of reach unless the contract changes. A kernel that only
  sets the fields and then calls `__post_init__` would drop one `<string>` frame from
  tracebacks.
- **Exceptions as control flow** (mini_interp's return/break, rules_engine's
  `HardDecline`): ≤ 0.8% leaf time (exceptions machinery; 0.16% in mini_interp). 3.11+
  zero-cost `try` already removed the cost: settled.
- **`isinstance` on plain classes**: specialized (`CALL_ISINSTANCE`), 0.2%. Protocol and ABC
  `isinstance` only matter where used per object: rules_engine 2.4%, not a priority.
- **namedtuple / `NamedTuple` generated code**: ≤ 0.7% (task_planner).

## 10. Recommendation (opinionated)

1. **Phase 2: one "data-model machinery" workstream** (C1, C2, C7 first, then C3, C4,
   C6). All are exact, stdlib- or generated-code kernels behind existing entry points.
   Most are slot-reached, so fallbacks are nearly free, and enum and dataclasses have the
   reach (38.5% / 28.6%). The vectorcall mechanism needs one extension: kernels for
   *generated* functions are installed per class at class creation (a hook on
   `dataclasses` processing, or a lazy install on the first call through the slot).
   Exactness is proven by regenerating the expected code object.
2. **Decide C5 with w2-wl-frameworks' data.** `inspect.signature`/`get_type_hints` are 31%
   of a DI-heavy program but have a wide, version-sensitive surface. If FastAPI/pydantic/
   pytest/typer show them per request, they become a top-3 candidate.
3. **Tell the lead/w2-machinery that user-dunder dispatch through type slots (C8)** is the
   largest language-level item that is specific to custom software: up to 41% inclusive,
   2.7% mean overhead. It is a specializer-shaped problem.
4. **Contract question for the lead**: ruling 15 puts frozen dataclasses with
   `__post_init__` and the hot halves of contextmanager/singledispatch out of reach. Those
   are the single largest items in three of ten programs (13.4%, 9.5%, 15.2% -X perf
   self). An exception that let a kernel call user code while synthesizing the intervening
   frame is the only way in. I do not recommend it without a design that keeps tracebacks
   identical.
