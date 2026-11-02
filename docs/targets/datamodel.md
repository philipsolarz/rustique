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
with `Prepare::slot` (accel's `heap_slot` lands in its merge C; same semantics). The kernel
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

## 3. Measured results (callgrind, Ir per call, 3.15.1, region-instrumented, fresh process)

See `benches/targets/datamodel_cells.py` (full table in §3.1 of the final report below).

## 4. Conformance

`benches/datamodel/conformance_all.sh` runs test_enum, test_dataclasses, test_collections,
test_typing, test_contextlib, test_contextlib_async, test_functools, test_copy,
test_inspect, test_weakref, test_pickle plain and enabled (all four groups enabled before
the module is imported) and compares every test id's outcome.

## 5. Free-threading

All kernels are validated on 3.14t/3.15t: dict scans under the dict's critical section,
verdict caches in atomics (a class modified while a verdict is computed is never cached
under the old tag), new references taken before anything can run. Dynamic sites use
`PyFunction_SetVectorcall` (stop-the-world) per the lead's ruling.

## 6. Not in this wave: `inspect.signature` / `getfullargspec`

See §6 of the final report: per call 66–107k Ir (Phase 1); the exact subset a kernel can
cover excludes annotated functions (`get_annotations` calls the compiler-generated
`__annotate__`, a Python frame: ruling 15), which is di_plugins' whole use (DI autowiring
reads type hints); falcon's per-request `getfullargspec` of unannotated responders would
qualify, but `Signature`/`Parameter` construction, validation and `follow_wrapped` /
`__signature__` / `__text_signature__` rules are ~400 lines of `inspect.py` to reproduce
exactly per minor version. Next wave, with a dedicated prototype.
