STATUS: wave 2, Phase 2, workstream `datamodel` (branch `ws2/datamodel`). Contract:
`docs/ACCELERATION.md`. Code: `crates/rustique-py/src/targets/{enum,generated,contextlib,functools}/`
(`generated/common.rs` and `generated/adapter.rs` are shared by the four groups). Tests:
`tests/targets/test_dm_{enum,generated,contextlib,functools}.py` (+ `_dm_support.py`).
Benchmarks: `benches/targets/datamodel_cells.py`, `benches/targets/datamodel_alloc_cells.py`
(callgrind), `benches/datamodel/` (workload instructions `wl_instr.py`, free-threading
`ft_scaling.py`/`ft_create.py`, conformance driver `conformance_all.sh`, profiling and
fingerprint tools). Evidence for the slot/dynamic-site mechanisms: `docs/atlas/w2-machinery.md`.
The tp_alloc routing of positional dataclass calls (proposed contract amendment) is in
`docs/targets/generated.md`.

# Data-model machinery: enum, generated methods, contextmanager, singledispatch

CPython machinery that custom and framework code leans on, written in Python and reached
mostly from C slots: enum member access and lookup, the methods `namedtuple()` and
`@dataclass` generate, `@contextmanager` construction and the `singledispatch` cache. Nine
targets in four groups (plus the `tp_alloc` routing of positional dataclass calls, under review); all on by default, validated on 3.14 and 3.15 (GIL and
free-threaded); 3.16 refuses (`enum.py` changed: version gate).

| target | group | site(s) | reached from | fallback cost |
|---|---|---|---|---|
| `enum.property.__get__` | enum | `tp_descr_get` of the heap type `enum.property` | generic `LOAD_ATTR` (`m.value`, `m.name`) | slot: ~+5 Ir |
| `enum.EnumType.__call__` | enum | `tp_call` of `EnumType` + vectorcall of `EnumType.__call__` | `Color(v)`; metaclass subclasses via `slot_tp_call` | slot / C caller |
| `enum.Enum.__hash__` | enum | vectorcall of `Enum.__hash__` | `slot_tp_hash` (`hash(m)`, `d[m]`) | C caller |
| `enum.Flag.operators` | enum | vectorcall of `Flag.__or__/__and__/__xor__/__invert__/__contains__` | `slot_nb_*`, `slot_sq_contains` | C caller |
| `collections.namedtuple` | generated | hook: vectorcall of `namedtuple`; dynamic: each class's `__new__`, `_make`, `_replace` | `slot_tp_new`; bytecode (`_make`, `_replace`) | C caller / +180–260 Ir |
| `dataclasses.__init__` | generated | hook: `_FuncBuilder.add_fns_to_class`; dynamic: each frozen class's `__init__` | `slot_tp_init` (keyword calls, `kw_only` classes, `super().__init__`) | C caller |
| `dataclasses.__init__.positional` | generated | hook: `_process_class`; the routed class's `tp_alloc` | positional calls (`docs/targets/generated.md`: needs a contract amendment) | — |
| `contextlib.contextmanager` | contextlib | hook: vectorcall of `contextmanager`; dynamic: each `helper` | bytecode (`cm(...)`) | +180–260 Ir |
| `functools.singledispatch` | functools | hook: vectorcall of `singledispatch`; dynamic: each `dispatch` closure | bytecode (`wrapper`, `f.dispatch`) | +180–260 Ir |
| `functools.singledispatchmethod.__get__` | functools | vectorcall of `singledispatchmethod.__get__` | `slot_tp_descr_get` (`h.handle`) | C caller |

What stays CPython's (ruling 15: the code calls user code from its own Python frame):
`_GeneratorContextManager.__enter__/__exit__` (they run the user's generator), the
`singledispatch` `wrapper` and `_singledispatchmethod_get.__call__` (they call the
registered implementation), dataclass `__init__` with `__post_init__` (the generated frame
sits between the caller and `__post_init__`), non-frozen dataclass `__init__` (plain
`STORE_ATTR`s already at the floor: a kernel measured 1.00–1.016 in Phase 1),
`total_ordering`, `cached_property`, `inspect.signature` (§6).

## 1. Mechanisms

**Slots of stdlib heap types** (`enum.property.tp_descr_get`, `EnumType.tp_call`): declared
with `Prepare::heap_slot` (reference classes with Python dunders: `types.DynamicClassAttribute`, `enum.verify`). The kernel
handles the exact type only (`enum.property` instances `Enum.__dict__['value'/'name']`,
classes whose metaclass is exactly `EnumType`); everything else calls the saved
`slot_tp_*` dispatcher, which resolves subclasses' own methods. Assigning
`enum.property.__get__` or `EnumType.__call__` makes CPython rewrite the slot (a correct
fallback; the next `enable()` re-arms); `disable()` restores by compare-and-swap.

**Python functions reached only from C slots** (`Enum.__hash__`, the `Flag` operators,
`EnumType.__call__` for metaclass subclasses, `singledispatchmethod.__get__`, the
generated `__new__`/`__init__`): vectorcall sites whose callers are C (`slot_tp_hash`,
`slot_nb_or`, `slot_tp_new`, ...), so a fallback costs a few instructions, not the
+180–260 Ir of a function called from bytecode (w2-machinery §5.1).

**Dynamic sites** (functions created after install): a *hook* — a vectorcall kernel on
the generator (`namedtuple`, `_FuncBuilder.add_fns_to_class`, `contextmanager`,
`singledispatch`) — calls CPython's original and gives the new function its kernel
(`PyFunction_SetVectorcall`, the official API; on free-threaded builds it stops the world:
§5). No per-site registry is kept: a weak reference per site cost ~300 Ir and made
`contextmanager(f)` 1.066 (measured); instead each dynamic kernel checks that its target is
installed (its hook still carries the hook kernel: 3 loads) and, when it is not
(`disable()`, suspension, exit), gives its function CPython's vectorcall and specializer
version back at that call and falls back (accel's `dynamic::install`, merge C, adopted
this design). Classes made by `namedtuple()` before `enable()` get `_make`/`_replace`
(shared code objects, identity with the fingerprinted constants) from a scan of
`tuple.__subclasses__()` at install (GIL builds; e.g. `tokenize.TokenInfo._make`).

**Recognition by construction** (generated code). The brief asked for recognition by
re-running CPython's generator and comparing canonical code; compiling a reference has two
problems, measured/inferred: it costs 13–37 µs per class (5–15% of a class creation:
eager breaks never-slower, lazy breaks "create + call once"), and every compile is a
`compile` audit event — user code (audit hooks) called where CPython calls none. So the
proof uses CPython's own generator output instead of a re-run:
- `namedtuple`: with `namedtuple`'s code fingerprinted (its nested `_make`/`_replace`
  are constants of it) and everything the generated text depends on pinned (`eval`, `map`,
  `tuple`, `type`, `len`, `_sys`, `sys.intern`), the class returned has `__new__ =
  eval(f'lambda _cls, {fields}: _tuple_new(_cls, ({fields}))')` compiled moments before;
  no user code can run between that `eval` and the hook's return. Structural checks of the
  code object (argument counts, flags, `co_localsplusnames == ('_cls', *_fields)`,
  `co_names == ('_tuple_new',)`, no cells) as defence in depth; raw layout reads, ~1 µs.
- dataclass `__init__`: the hook reads the exact source text the generator produced
  (`self.src` of the `_FuncBuilder`, before `add_fns_to_class` compiles it); after it
  returns, the class's own `__init__` *is* the function compiled from that text (metaclass
  with `type`'s `__setattr__`/`__getattribute__`, no `__init__` of the class's own, every
  global the method uses pinned, `recursive_repr` the only object `__create_fn__` calls). A
  strict parser accepts only the frozen form `_init_fn` emits without `__post_init__`
  (`__dataclass_builtins_object__.__setattr__(self,'x',VALUE)` lines; parameters plain, with
  `=__dataclass_dflt_x__` or `=__dataclass_HAS_DEFAULT_FACTORY__`); the plan is read off
  the text and the code object must agree (counts, flags, varnames, free variables).
  `cargo test` covers the parser.

## 2. Exact subsets

- **`.value`/`.name`**: the descriptor is `Enum.__dict__['value']`/`['name']` (identity),
  its `fget` (an instance attribute, read per call) the fingerprinted function with
  unchanged code; the member's class has the generic attribute protocol and no descriptor
  named `_value_`/`_name_` (per-class facts, below). Result: `PyObject_GenericGetAttr`.
- **`Color(value)`**: exactly one positional argument; per-class facts: `_member_map_` a
  non-empty exact dict, `_value2member_map_` an exact dict whose keys all have builtin
  immutable types (int, str, bool, float, None, tuple, bytes, frozenset, complex), `__new__`
  is `Enum.__new__`; the metaclass leaves `_member_map_`/`_value2member_map_`/`__new__` to
  the class and its `__call__` is `EnumType.__call__`; the value is the member itself or an
  exact int/str/bool/float (then a dict lookup compares in C only). A miss (`_missing_`,
  pseudo-member creation, unhashable values, the functional API) runs CPython's.
- **`hash(member)`**: `hash(self._name_)` for an exact `str` (or None) name, `hash`
  builtin not shadowed in `enum` (pinned).
- **`Flag` operators**: both operands of exactly the same class, `_value_` exact ints,
  `_get_value` and `__class__` CPython's, result already in `_value2member_map_`
  (composites are cached there after their first creation); `~` returns the cached
  `_inverted_`; `in` compares the values. Mixed `IntFlag | int` and first creations fall
  back.
- **Per-class facts** (`ClassCache`: 2-way sets indexed by a multiplicative hash of the
  type's address, keyed by `tp_version_tag`, which changes on any attribute change of the
  class or a base; tags are never reused). `_member_map_` and `_value2member_map_` are
  watched (a dict watcher; accel's `watch_dict_invalidates` in merge C): any mutation clears
  the class's facts, and they are recomputed under the dict's critical section.
- **namedtuple `__new__`**: arguments bind (positional, keywords by identity with the
  interned field names, `__defaults__`), the lambda's globals still map `_tuple_new` to
  `tuple.__new__`, `tuple.__new__`'s checks would pass (`cls` a tuple subtype whose first
  base with a C `tp_new` is `tuple`); built as `tuple_subtype_new` does (`tp_alloc`, items,
  hash-cache reset, GC tracking).
- **`_make(iterable)`**: an exact tuple, or an exact list (copied with `PyList_AsTuple`,
  locked on free-threaded builds), of exactly `len(_fields)` items; cells `tuple_new`,
  `_len`, `num_fields` unchanged. Iterators and generators fall back (they may run code).
- **`_replace(**kw)`**: every keyword a field (identity), `self` iterates as a tuple, has
  no instance dict, `self._make` the generated `_make`.
- **frozen dataclass `__init__`**: arguments bind (new references held: a finalizer that
  replaces `__defaults__` cannot free them); `self` is not a class; for `type(self)` (cached
  per tag, 4 tags per plan, so subclass instances via `super().__init__` qualify) every
  field store runs no Python code: the name resolves to nothing (instance dict present), a
  writable member descriptor of a base (`__slots__`), or a plain value whose type has no
  `__set__` (re-checked per call when that type is mutable); the closure cells hold
  `object`, the factory sentinel, and for each factory the call needs exactly `list`,
  `dict` or `set` — all checked before the first store. Classes with any other
  `default_factory` get no kernel (it would fall back on every defaulted call).
- **`cm(*args, **kw)`** (`@contextmanager`): the `func` cell an exact generator function
  with CPython's vectorcall (calling it only binds and creates the generator), no tool on
  `helper`, `_GeneratorContextManagerBase.__init__` or `func`; `_GeneratorContextManager`
  unchanged (tag, metaclass tag): `object.__new__`, the fingerprinted `__init__`, generic
  `__setattr__`, no descriptors for the five attributes. Binding errors fall back (CPython
  raises them from its frames). Same order as CPython: generator, `args`, `kwds`,
  allocation, `gen`, `func`, `args`, `kwds`, `__doc__`.
- **`dispatch(cls)`** (`singledispatch`): a hit of `dispatch_cache[cls]` only: `cache_token`
  `None` or equal to `abc.get_cache_token()`; `dispatch_cache` an exact
  `WeakKeyDictionary` (class unchanged, `__getitem__` fingerprinted on first use since
  `weakref` is imported by `singledispatch()` itself, `weakref.ref` unchanged); `cls` a class
  whose metaclass hashes and compares like `type`; every key of `data` a weak reference
  that is dead or whose cached hash is its referent's pointer hash (so a lookup compares
  only with `cls`'s own entry, in C; verdict per dict, watched).
- **`singledispatchmethod.__get__`**: exact `singledispatchmethod` whose `dispatcher` and
  `func` are plain functions; `_singledispatchmethod_get` unchanged (tag); attributes set in
  CPython's order (3.15 adds `_dispatch_arg_index`, version-specific).
- **Everywhere**: the code objects reproduced are the fingerprinted ones (identity per call
  for the static ones, fingerprints of the generators for the dynamic ones) and no tool
  listens, globally or locally on any of them; otherwise CPython's code runs.

**Private state skipped** (§2.4): none of the targets keeps state CPython would; the
per-class/per-dict verdict caches and the plans on code objects (`co_extra`, freed with the
code) are Rustique's own.

**Residuals** (documented, same as wave-1 precedent): a non-`str` key with a colliding hash
stored in an enum member's or a dataclass instance's own `__dict__` would have its `__eq__`
called by `PyObject_GenericGetAttr`/`SetAttr` without the Python frame CPython would have
on the stack (wave 1 treats instance dicts the same way); a kernel elides the Python
frame(s) it replaces, so the exact recursion depth at which `RecursionError` is raised
differs by those frames (as for every wave-1 vectorcall target).

## 3. Measured results

### 3.1 Cells (callgrind, Ir per call, 3.15.1, region-instrumented, fresh process; `benches/targets/datamodel_cells.py`)

| cell | plain | on | ratio |
|---|---:|---:|---:|
| `m.value` / `m.name` | 1,097 | 544 / 550 | 0.496 / 0.501 |
| `Color(2)` / `Color(m)` / `Num(2)` (IntEnum) | 2,348 / 2,193 / 2,348 | 623 / 487 / 623 | 0.265 / 0.222 / 0.265 |
| `Perm.R \| Perm.W` / `rw & r` / `rw ^ r` | 5,449 / 5,364 / 5,369 | 1,123 / 1,049 / 1,051 | 0.206 / 0.196 / 0.196 |
| `~r` / `r in rw` / `Bits.A \| Bits.B` (IntFlag) | 1,143 / 1,048 / 6,636 | 524 / 781 / 1,195 | 0.459 / 0.745 / 0.180 |
| `hash(m)` / `d[m]` | 1,213 / 985 | 945 / 731 | 0.779 / 0.742 |
| `M(1)` (metaclass subclass of EnumType) | 2,348 | 752 | 0.320 |
| fallback: `Color(99)` miss → ValueError | 12,937 | 13,266 | 1.025 |
| fallback: `Bits.A \| 2` (IntFlag \| int) | 8,795 | 7,169 | 0.815 |
| fallback: user `enum.property` | 1,097 | 1,129 | 1.029 |
| fallback: value map with a user-typed key (`Bad(1)`) | 2,348 | 2,511 | 1.069 (see below) |
| `NT(1, 2, 3)` / keywords / `typing.NamedTuple` | 2,209 / 3,514 / 2,065 | 1,604 / 2,750 / 1,500 | 0.726 / 0.783 / 0.726 |
| `NT._make(tuple)` / `(list)` / subclass | 1,988 / 2,160 / 2,013 | 1,395 / 1,588 / 1,420 | 0.702 / 0.735 / 0.706 |
| `nt._replace(b=5)` | 5,355 | 1,420 | 0.265 |
| fallbacks: `_make(iter(t))`, `_replace` on a subclass with `__dict__` | 2,665 / 5,294 | 2,665 / 5,289 | 1.000 / 0.999 |
| `F(x=1, y=2)` frozen keywords / `E(a=1, b='t')` slots kw_only | 4,335 / 4,254 | 2,775 / 2,722 | 0.640 / 0.640 |
| `F(1, 2)` / `S(1, 2)` slots / `L(1)` factory, positional (routing, `generated.md`) | 2,832 / 2,774 / 3,449 | 1,849 / 1,789 / 2,255 | 0.653 / 0.645 / 0.654 |
| untouched: user factory, non-frozen, `__post_init__` | 3,379 / 2,572 / 3,312 | 3,374 / 2,567 / 3,326 | 0.999 / 0.998 / 1.004 |
| `cm()` / `with cm(): pass` / `cm2(1, k=2)` | 4,181 / 8,085 / 5,525 | 2,695 / 6,591 / 3,391 | 0.644 / 0.815 / 0.614 |
| fallback: `@contextmanager` on a non-generator | 4,342 | 4,342 | 1.000 |
| `fmt(1)` / `fmt.dispatch(int)` (singledispatch) | 3,130 / 1,947 | 2,485 / 1,303 | 0.794 / 0.669 |
| `h.handle(1)` / `h.handle` (singledispatchmethod) | 9,672 / 3,417 | 8,405 / 2,796 | 0.869 / 0.818 |
| fallbacks: metaclass with `__eq__`, classmethod implementation | 4,454 / 10,501 | 4,436 / 10,293 | 0.996 / 0.980 |
| decorate: `contextmanager(f)` / `singledispatch(f)` | 12,904 / 34,743 | 13,200 / 35,138 | 1.023 / 1.011 |
| create `namedtuple` + 1 call / frozen dataclass + 1 call / enum (5 members) | 312,831 / 2,816,580 / 420,352 | 314,126 / 2,842,462 / 422,583 | 1.004 / 1.009 / 1.005 |

`Bad(1)` (1.069) is an enum whose `_value2member_map_` holds a key of a user class (only
possible by writing to the private map): every lookup falls back after the class facts
said "unsafe keys"; the +163 Ir are `tp_call` kernel + `slot_tp_call` + the vectorcall
kernel's early exit. Not a realistic program state; listed as the one cell above 1.05.
Creation cells measured after merging accel's fix of the monitoring-version tax its layout
probe caused (wave2 a000743; before it: 1.012 / 1.023 / 1.033, ~930 Ir per new code object).

### 3.2 Workloads (instructions per `run()`, `perf stat`, dev box, 3.15.1, digests identical; `benches/datamodel/wl_instr.py`)

"all" = every group enabled; "base" = every group except this workstream's; all/base is
this workstream's effect. Custom family (k=3, r=2, spread ≤ ±0.0034):

| workload | all/plain | all/base |
|---|---:|---:|
| rules_engine | 0.8005 | 0.8009 |
| report_engine | 0.8410 | 0.9158 |
| event_ledger | 0.8553 | 0.8658 |
| orm_mapper | 0.9462 | 0.9714 |
| ecs_arena | 0.9582 | 0.9659 |
| order_domain | 0.9604 | 0.9604 |
| di_plugins | 0.9609 | 0.9577 |
| task_planner | 0.9969 | 0.9963 |
| fulfilment_sim | 0.9959 | 0.9979 |
| mini_interp | 0.9994 | 1.0019 |
| **geometric mean** | **0.929** | **0.944** |

mini_interp re-measured with 6 runs × 3 repeats: all/base 1.0001 (±0.0005; 1,984 namedtuple
hits per run, no fallback: noise).

Other families (all/base; k=8 runs × 2 repeats, spread ≤ ±0.0033; json_api k=3):

| workload | all/base | | workload | all/base |
|---|---:|---|---|---:|
| strawberry_graphql | 0.9555 | | json_api | 0.9630 |
| rich_render | 0.9759 | | fastapi_sqlalchemy | 0.9906 |
| django_admin | 0.9972 | | pylint_stdlib | 0.9972 |
| drf_api | 0.9992 | | flask_marshmallow | 0.9996 |
| falcon_api | 1.0012 (±0.0018) | | | |

Frameworks geometric mean 0.990.

## 4. Conformance

`benches/datamodel/conformance_all.sh` runs test_enum, test_dataclasses, test_collections,
test_typing, test_contextlib, test_contextlib_async, test_functools, test_copy,
test_inspect, test_weakref, test_pickle plain and enabled (all four groups enabled before
the module is imported) and compares every test id's outcome.

## 5. Free-threading

On 3.14t/3.15t this wave: `enum.EnumType.__call__`, `enum.Enum.__hash__`,
`enum.Flag.operators` and `functools.singledispatchmethod.__get__` (static sites,
installed at enable/import). Refused there (`build`):
- the dynamic-site targets (`collections.namedtuple`, `dataclasses.__init__`,
  `dataclasses.__init__.positional`, `contextlib.contextmanager`,
  `functools.singledispatch`): per the lead's ruling they must use the official
  `PyFunction_SetVectorcall`, which stops the world for every generated function; with
  busy threads each stop waits until every thread reaches a safe point
  (`benches/datamodel/ft_create.py`: creating 1,000 namedtuples next to 8 busy threads
  did not finish in minutes enabled on the loaded dev box, against well under a second
  plain);
- `enum.property.__get__`: its per-call `fget` check takes a reference to a shared function
  object that CPython's own path never touches (deferred reference counting): `m.value`
  1.139× CPU per call at 8 threads (`ft_scaling.py`).
Kernels compare cached objects by pointer without taking references (`cell_is`,
`dict_item_is`) so they add no reference-count traffic on shared objects; dict scans run
under the dict's critical section; verdict caches are atomics (a class modified while a
verdict is computed is never cached under the old tag); arguments get new references
before anything can run.

8-thread rule (`benches/datamodel/ft_scaling.py`, 3.15.1t, CPU time per call over 8
threads, minimum of 5 interleaved repeats; the shared dev box ran at load ~50 on 32 cores,
so ops whose targets are refused there — and therefore run CPython's code on both sides —
read 0.97–1.19: that is the noise band): `Color(2)` 0.844, `Perm.R | Perm.W` 0.304,
`hash(m)` 0.789, `h.handle(1)` 0.978. A clean stop-the-world cost could not be measured on
the loaded box (`ft_stw.py`: the main thread itself is descheduled for milliseconds); it is
the quiet-box item for Phase 3 if dynamic sites are to be enabled on free-threaded builds.

## 6. Not in this wave: `inspect.signature` / `getfullargspec`

See §6 of the final report: per call 66–107k Ir (Phase 1); the exact subset a kernel can
cover excludes annotated functions (`get_annotations` calls the compiler-generated
`__annotate__`, a Python frame: ruling 15), which is di_plugins' whole use (DI autowiring
reads type hints); falcon's per-request `getfullargspec` of unannotated responders would
qualify, but `Signature`/`Parameter` construction, validation and `follow_wrapped` /
`__signature__` / `__text_signature__` rules are ~400 lines of `inspect.py` to reproduce
exactly per minor version. Next wave, with a dedicated prototype.
