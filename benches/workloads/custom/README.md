# Workloads: `custom` family (complex in-house applications)

Programs that read like the in-house code most Python is: classes, data models, closures and
protocols, built on what CPython generates and provides (dataclasses, enums, `functools`,
`typing`, descriptors, user dunders) rather than on one stdlib module. Shared format
(`REQUIRES`, `setup()`, `run(state)`, `digest(result)`); stdlib only; inputs generated
deterministically in `setup()`; digests stable across runs, `PYTHONHASHSEED` values and
3.15.1t. Run time: median `run()` on CPython 3.15.1 on the shared dev box (*measured*,
`run.py compare custom -n 5 -p 3`, box load 25-70 on 32 cores: ±10-15%). Profiles and findings: `docs/atlas/w2-wl-custom.md`.

| workload | stands for | run() |
|---|---|---|
| `order_domain` | e-commerce order back office (DDD): frozen/slotted/`order=True` dataclass `Money` over `Decimal`, `StrEnum`/`IntEnum`/`Flag`, `__post_init__` validation, computed `@property` totals, ABC promotions, dict-indexed repositories, domain events on a bus, `match` | 1.35 s |
| `event_ledger` | event-sourced banking ledger (CQRS): command/event dataclass hierarchies, `match` class patterns with guards, `singledispatchmethod` aggregate rehydrated from snapshot + stream tail per command, `singledispatch` projections, replay, exceptions as rejections | 1.58 s |
| `fulfilment_sim` | discrete-event simulation (SimPy-style engine written in-house): `heapq` of `total_ordering` `__slots__` entries (user `__lt__`/`__eq__`, generated `__ge__`), generator processes resumed by `send`/`throw`, resources with `with req:` and preemption by exceptions, closures, `partial`, Welford/`bisect` monitors; 3 scenarios, ~40k orders | 1.35 s |
| `mini_interp` | in-house DSL / scripting-language interpreter: `re` master-pattern tokenizer, `NamedTuple` tokens, recursive-descent parser into `__slots__` AST classes, `getattr("visit_" + name)` visitors, constant folding, linked-dict environments, closures, `return`/`break` as exceptions; 9 programs, ~4M node visits | 1.14 s |
| `rules_engine` | loan underwriting decision service: decorator rule registry, `@runtime_checkable` `Protocol` `isinstance`, ABC condition DSL with `&`/`\|`/`~` and `attrgetter` paths, `cached_property` facts, `lru_cache`, `partial`, `@contextmanager` audit sections, `ExitStack`, `IntFlag`, workflow state machine; 12k applications, ~264k rule evaluations | 0.94 s |
| `orm_mapper` | home-grown ORM over `sqlite3` driving a helpdesk: field descriptors (`__set_name__`/`__get__`/`__set__` validation + dirty tracking), metaclass + `__init_subclass__`, operator-overloaded column expressions compiled to SQL, `copy.copy` query cloning, `WeakValueDictionary` identity map, `__getattr__` lazy FK proxies, unit of work; 33k inserts, ~5.5k statements, ~51k rows hydrated | 0.85 s |
| `di_plugins` | DI container + pluggy-style plugin kernel: `inspect.signature` + `typing.get_type_hints` autowiring (`X \| None`, `list[T]`), scoped/singleton/transient lifetimes, call-site injection, `__init_subclass__` plugin registry, hook calls with per-impl argument selection, `WeakMethod` event bus, `WeakKeyDictionary`; 50 boots, 24k requests | 0.85 s |
| `task_planner` | monorepo build/CI planner: `namedtuple`/`NamedTuple` records, `defaultdict(set)` DAG, Kahn sort on `deque` + `Counter`, iterative-DFS cycle report, closure + transitive reduction with set algebra, BFS impact queries, per-team `heapq` list scheduling, LPT test sharding, `itertools` reports; 3.2k targets, 1.5k impact queries, 4 plans | 0.53 s |
| `ecs_arena` | entity-component-system arena game/agent simulation: `__slots__` `Vec2` with arithmetic dunders, slotted dataclass components, frozen dataclass grid cells as dict keys, `dict`-of-`set` spatial hash, `Enum` AI state machine with `match`, event `deque`, `Counter` inventories; 240 units, 240 ticks | 1.04 s |
| `report_engine` | statements/notifications from an in-house template engine: `re`-tokenized templates compiled to closures, getattr→getitem dotted lookups, decorator filter registry, `ChainMap` scopes, `loop` properties, `__format__`/`__str__`/`__repr__` value objects, `str.format_map` + `__missing__`, `ljust`/`rjust` tables, `html.escape`; 2,200 statements, ~9 MB of text | 0.96 s |

Tools (not workloads; leading underscore):

- `_profile_custom.py <workload.py> --out p.json [--stat-enable]`: plain `perf record -g`
  native components, `-X perf` owners by code-object provenance (own / generated /
  stdlib) re-weighted to the plain component split, the six layers, type-slot dispatch,
  cProfile per code object (exec-generated methods named after their class), and
  `perf stat` instructions per `run()` (optionally with `rustique.enable()`).
- `_summarize_custom.py --prof <dir> [--prof-t <dir>] [--compare c.json]`: the tables of
  the atlas doc.
- `_machinery_bench.py`: instructions per operation of the machinery (dataclass, enum,
  functools, contextlib, typing/abc, weakref, inspect) next to its plain-Python floor.
