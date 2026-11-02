STATUS: wave 1, groups `copy` and `dataclasses`; wave 2 (workstream `core`): `uuid.UUID`
copied natively (§7). Contract: `docs/ACCELERATION.md`, ruling 15
(restart before anything observable). Code: `crates/rustique-py/src/targets/copy/`. Tests:
`tests/targets/test_copy_*.py`. Benchmarks: `benches/targets/copy_cells.py` (callgrind),
`copy_pyperf.py`, `copy_install.py`.

# Targets `copy.deepcopy`, `copy.copy`, `dataclasses.asdict`, `dataclasses.astuple`

All four are vectorcall kernels on the existing function objects (contract §3.7): every
captured reference (`from copy import deepcopy`, `partial`, default arguments, the
`deepcopy=deepcopy` defaults inside `copy.py`) reaches the kernel; identity, pickling,
`repr`, signature, `__code__` and source are unchanged.

| target | group | module (lazy install) | validated | free-threaded |
|---|---|---|---|---|
| `copy.deepcopy` | `copy` | `copy` | 3.14, 3.15 | yes |
| `copy.copy` | `copy` | `copy` | 3.14, 3.15 | yes |
| `dataclasses.asdict` | `dataclasses` | `dataclasses` | 3.14, 3.15 | yes |
| `dataclasses.astuple` | `dataclasses` | `dataclasses` | 3.14, 3.15 | yes |

`dataclasses.replace` is not accelerated: its last step is always the dataclass's
`__init__` (user code), which must run under CPython's own frames.

## 1. Design: pure native work, or CPython from scratch

A kernel completes a call only when **no user code runs anywhere in it**. It executes
`Lib/copy.py`'s algorithm (and `dataclasses._asdict_inner`/`_astuple_inner`) step for step
with operations that run no Python code; at the first node that *would* run user code it
abandons everything it built and calls the saved original from scratch with the unchanged
arguments (contract §5.1, restart rule; counted as `fallback["deopt"]`). Nothing it did
before is observable: it ran no user code, the partial copies it drops are builtin
containers and instances of classes without finalizers, and a caller's memo dict is only
written on success. So every hook runs under CPython's own frames (tracebacks,
`sys._getframe`, `inspect.stack`), tools attached by a hook see the rest of the call, and
temporaries are freed by CPython in its order.

Restarts and suspension (net-benefit policy, contract §5.3: hits credit the score,
fallbacks debit it; per target from the measured savings and penalties of §5: a fallback
costs ~420 Ir over CPython — the C call the interpreter cannot inline — while a hit saves
400 Ir to 20k+ in `deepcopy` (credit 2, debit 1: installed while a third of the calls
hit) and 250–420 Ir for the common `copy.copy` hits against a 420–690 Ir fallback
(credit 1, debit 2: two thirds);
`asdict`/`astuple` keep 4/1). A restart that abandoned copies always debits. Every other fallback
debits when it is a direct call, and is only counted when it is a call the original's
helpers (or hooks) make inside a fallback of the same target: those repeat the outer
fallback level by level (a list of 100 objects with their own `__deepcopy__`, a profiler
attached while a structure is copied) and must not suspend the kernel by themselves. While a restarted call runs, the containers on the path from its root to
the node that needed user code go straight to the original when the helpers reach them again
(each would restart at that same node, since no user code ran before it); everything else
the helpers hand to `deepcopy` (siblings, subtrees) is copied natively. A hook at depth *d*
thus costs one wasted native traversal of the prefix, not *d*. The path is a per-thread
stack of addresses; a stale match only calls the original, which is always correct.

Deep recursion through fallbacks. With the kernel installed, every call of `deepcopy`/
`copy` from Python code is a C call (the interpreter inlines only functions whose
vectorcall is its own). A call that falls back (restart, generic memo, ...) runs the
original, whose helpers or hooks call the function again: one C-level recursion per level,
where CPython inlines and recurses as deep as the recursion limit allows. Fallbacks nesting
deeper than 128 on one thread therefore put the original vectorcall back at once (the deeper
levels are inlined exactly as in CPython) and have the registry record a suspension
(resumed by the next `enable()`). So a raised recursion limit reaches as deep as in CPython
— 50,000-level chains with user code at the bottom, linked hooks calling `deepcopy`/`copy`
recursively — with no C stack overflow; the native engine itself never recurses in C.

What restarts (`deepcopy`): a `__deepcopy__` other than a recognised no-op (a Python
function whose code is exactly `return self`, i.e. `Enum.__deepcopy__`); a copier registered
in `copy._deepcopy_dispatch` that is not one of `copy.py`'s own helpers; any reductor in
`copyreg.dispatch_table`; an object whose reduce protocol is not `object`'s default
(`__reduce_ex__`, `__reduce__`, `__getstate__`, `__setstate__`, `__getnewargs[_ex]__`, a
`__new__` other than `object.__new__`, abstract classes, a `__dict__` that is not the
standard descriptor, slots that are not member descriptors, a `__getattr__`/
`__getattribute__`, a metatype other than `type`); copies of classes with `__del__`/a
finalizer (they would be discarded on a later restart); a dict key or set element whose
`__hash__`/`__eq__` is not builtin (`object`'s for instances); a metatype with its own
`__hash__`; a modified `copy.py` helper or one with local monitoring events (a debugger
breakpoint); the depth at which CPython would raise `RecursionError`; any error (CPython
raises it itself). On 3.14, a `copy._atomic_types` (a mutable set) that no longer holds
exactly the validated types.

Native (`deepcopy`): `copy`'s atomic types; exact `list`, `tuple`, `dict`, `frozendict`,
bound methods through the original helpers; `issubclass(cls, type)`; `return self` hooks
(Python functions whose code is `return self`, ignoring 3.14's `CO_METHOD` flag);
`_decimal.Decimal` (its C `__deepcopy__` returns `self`); the `_datetime` types `date`,
`datetime`, `time`, `timedelta`, `timezone` (their C `__reduce_ex__`, or `tzinfo.__reduce__`
emulated for `timezone` because CPython's enters `copyreg._slotnames`, then `_reconstruct`
calling the C constructor with the copied arguments). These C types are recognised lazily
and never imported: exact, immutable types that are the very objects in
`sys.modules["_datetime"]`/`["_decimal"]` (`known.rs`);
plain objects and dataclasses (incl. frozen and `__slots__` ones): `object.__reduce_ex__(4)`
computed natively (`reduce.rs`; the first copy of a class runs `copyreg._slotnames`'
computation natively and caches `cls.__slotnames__` exactly as CPython does:
`(copyreg.__newobj__, (cls,), state)` with `state` the
instance `__dict__` — materialised exactly when `object_getstate_default` materialises it,
emptiness read with `PyObject_VisitManagedDict` — and the slot values), then `_reconstruct`
(`object.__new__` allocation, `y.__dict__.update`, member-descriptor `setattr`); exact
`set`/`frozenset` (`set.__reduce__` emulated, including the frame CPython enters in
`copyreg._slotnames`).

The algorithm is CPython's in every detail that could become observable: memo entries and
the keep-alive list in `copy.py`'s order (on success with a caller's exact dict memo they
are written there in that order: new keys in insertion order, the keep-alive list created at
its position or appended to); `y[deepcopy(k)] = deepcopy(v)` copies the value first; lists
built by appends and dicts by inserts (same `sys.getsizeof`); the tuple rule ("the original
if every item is its own copy", after the memo re-check); the frozendict rule (3.15); the
recursion budget counted per emulated Python frame (`deepcopy`, the `_deepcopy_*` helper,
`_keep_alive`, `_reconstruct`, `copyreg.__newobj__`, `copyreg._slotnames`, 3.14's
generator expression in `_reconstruct`), so a structure that CPython copies, the kernel
copies — 50,000 levels deep under a high recursion limit included — and one that CPython
cannot, CPython gets to fail.

`copy.copy` native: atomic types, `list`/`dict`/`set`/`bytearray` (`cls.copy(x)`, the C
methods), `issubclass(cls, type)`, a `return self` `__copy__` (Enum) or `Decimal`'s, plain
objects (`object.__reduce_ex__(4)` natively, then `y.__dict__.update(x.__dict__)` when its
keys are strings, and the slots), the `_datetime` types (`cls(*args)`). Anything else
restarts (a direct call debits the net-benefit score; nested in a fallback of `copy`, counted only).

`dataclasses.asdict`/`astuple` native: atomic values, dataclass instances (`fields(obj)` over
`Field` objects, `getattr(obj, f.name)` when it is an instance attribute, a slot, or a class
attribute without a Python `__get__`), exact `list`/`dict`/`tuple` (`astuple`: `tuple` and
`dict`; `list(genexpr)` restarts), and leaves through the copy engine when `copy` is
imported and its `deepcopy` is the installed kernel. A property, a subclass of a builtin
container (namedtuple, defaultdict), a lazily imported `copy` still to be imported, a leaf
whose copy restarts, more than 200 levels: restart.

## 2. Whole-call fallbacks before any work (`stats()` reasons)

- `args`: a memo that is not `None` or an exact `dict` (after the atomic check, which
  CPython makes before touching the memo: atomic roots are native with any memo); a
  keyword other than `memo`; 3.14's `_nil` argument; `copy.copy` with keywords;
  non-default factories.
- `deopt` without any work (root pre-check): a root whose class itself needs user code —
  a `__deepcopy__`/`__copy__` hook of the class, a registered copier (not one of
  `copy.py`'s helpers) or a `copyreg` reductor — is remembered by `(type,
  tp_version_tag)` and the generation of `copy._deepcopy_dispatch`/`copyreg.dispatch_table`
  (dict watchers); later roots of that class go straight to the original. Any change to the
  class or to a registration re-decides; conditions that are not the class's (local
  monitoring events on a hook or helper, a modified helper, the recursion limit, an
  error) are never remembered. An entry can only send a call to the original, which is
  always correct.
- `deps`: a pinned dependency replaced (deopt-while-unpinned then restores the original
  vectorcall until it is put back): copy's globals (one `pin_dict` snapshot: every binding
  the kernels use and the absence of any global shadowing a builtin), the builtins
  `copy.py` reads, `copyreg.__newobj__`/`_slotnames`, `sys.modules['copyreg']`, the
  `dataclasses` helpers, sentinels, `Field`, factories' defaults and builtins; also
  `deepcopy`'s/`copy`'s own `__code__`/`__defaults__`.
- `tools`: a `sys.monitoring` tool on any ungrouped event or local events on the function
  (`accel::tools_quiet`).

`copy._deepcopy_dispatch` and `copyreg.dispatch_table` are read live at every node (their
keys are types, which `pin_dict` does not cover), so registrations never need a fallback:
they restart exactly the calls that reach a registered type.

## 3. Outside the contract (ruled, not deviations)

- GC and allocation timing: native copying allocates differently (no frames, no
  temporaries), so a GC pass may trigger at another moment.
- A *concurrent* thread attaching a tool while a kernel runs sees the rest of that call as
  native (timing-dependent in CPython too). A tool attached by user code *inside* the call
  is not an issue: no user code runs inside a kernel.

Private state not reproduced: none (`copy.py` has none). The kernels' own caches: the code
objects recognised as `return self`, the recognised `_datetime`/`_decimal` types, the
root pre-check entries above, and per-thread fallback nesting / restart paths.

### Internal layouts relied upon (validated at install or by construction)

| what | used for | validation |
|---|---|---|
| `PyThreadState.py_recursion_remaining` / `py_recursion_limit` offsets (48/52 on 3.14, 52/56 on 3.15) | recursion budget of the emulated frames | install: the pair read there must equal `sys.getrecursionlimit()` and a sane remaining count, else refused ("layout") |
| `PySetObject` header (`fill`, `used`, `mask`, `table`), `setentry {key, hash}`, `set_do_lookup` linear probing over `LINEAR_PROBES` = 9 slots (3.14 only: copy's type sets are mutable sets) | membership of the hot types decided from the slots CPython's lookup inspects; snapshot compare otherwise | install self-test (`calibrate_set_probing`): a crafted set's table must show the header, the entry order, a colliding key placed in the next slot, and the mirrored lookup must answer as `PySet_Contains` for present, colliding and absent keys, else refused ("layout") |
| `PyFunctionObject` (`func_code`, `func_defaults`, `vectorcall`), `PyCodeObject` (`co_flags`, `co_argcount`, `co_kwonlyargcount`, `co_nlocalsplus`, `Py_SIZE`) | helper identity checks, `return self` recognition | public `cpython/` headers; the reference `return self` code is compiled by the running interpreter |
| `tp_version_tag` | root pre-check entries | public field; 0 (unassigned) never cached |
| `_PyObject_GetMethod` (exported) | `__reduce_ex__`/`__getstate__` not shadowed by the instance | resolved by symbol at install, else refused ("symbol") |
| `PyObject_VisitManagedDict`, `PyObject_GenericGetDict`, `PyType_GetDict`, `PyFunction_SetVectorcall` (public C API) | instance state as `object_getstate_default` reads it; deep fallback bound | public API |

## 4. Conformance (`tests/conformance/run_target_suites.py`)

`--targets copy,dataclasses --modules test.test_copy,test.test_copyreg,test.test_dataclasses
--expect-installed copy.deepcopy,copy.copy,dataclasses.asdict,dataclasses.astuple`: identical
per-test outcomes plain vs enabled on 3.15.1, 3.15.1t, 3.14.9, 3.14.9t (3.15: test_copy 84
pass, test_copyreg 6, test_dataclasses 284 + 1 skip; 3.14: 81 / 6 / 279 + 1 skip), 0
differences. Full pytest on the four builds (after merging wave1 b162e61): all pass except
`tests/test_expr.py::test_filter_matches_python`, a Hypothesis case of the expression
engine, unrelated to these targets (comparing an `X` expression with a complex number built
`(X + False)` instead of raising); fixed on wave1 in 672009b (the comparison now raises
`TypeError`, as Python does), after which the full suites pass on all four builds.

## 5. Measurements

### 5.1 Callgrind cells (`benches/targets/copy_cells.py`, Ir per call, enabled / plain)

Every measured cell is ≤ 1.05 on every build (maximum 1.044).

| cell | plain Ir (3.15.1) | 3.15.1 | 3.14.9 | 3.15.1t |
|---|---|---|---|---|
| deepcopy int | 712 | 0.436 | 0.578 | 0.438 |
| deepcopy record (10 fields) | 24,230 | 0.171 | 0.167 | 0.209 |
| deepcopy list of 5 records | 119,142 | 0.164 | 0.154 | 0.209 |
| deepcopy tuple of immutables | 19,354 | 0.132 | 0.132 | 0.132 |
| deepcopy small dataclass | 29,328 | 0.260 | 0.241 | 0.271 |
| deepcopy dataclass tree (datetimes, enums) | 141,872 | 0.257 | 0.232 | 0.271 |
| deepcopy plain / slotted object | 35,404 / 38,219 | 0.250 / 0.230 | 0.232 / 0.215 | 0.276 / 0.241 |
| deepcopy enum member | 3,002 | 0.464 | 0.542 | 0.456 |
| deepcopy set / frozenset | 27,987 / 29,122 | 0.187 / 0.193 | 0.174 / 0.175 | 0.200 / 0.204 |
| deepcopy datetime | 13,104 | 0.311 | 0.294 | 0.299 |
| deepcopy Decimal | 2,847 | 0.363 | 0.414 | 0.360 |
| deepcopy explicit `{}` memo | 25,459 | 0.296 | 0.290 | 0.357 |
| copy int / record / set | 645 / 1,688 / 1,477 | 0.655 / 0.771 / 0.729 | 0.698 / 0.765 / 0.962 | 0.628 / 0.721 / 0.688 |
| copy list100 / dict100 | 3,123 / 5,076 | 0.879 / 0.928 | 0.878 / 0.957 | 0.850 / 0.896 |
| copy plain object / dataclass | 12,714 / 12,636 | 0.372 / 0.374 | 0.394 / 0.388 | 0.375 / 0.373 |
| copy enum / datetime | 1,884 / 6,629 | 0.440 / 0.364 | 0.705 / 0.426 | 0.408 / 0.338 |
| asdict small dataclass / tree | 12,622 / 76,339 | 0.398 / 0.367 | 0.398 / 0.363 | 0.449 / 0.414 |
| astuple small dataclass | 12,478 | 0.363 | 0.362 | 0.412 |

Restarts and fallbacks. A restart costs the wasted native prefix plus the C-call entry of
every call the original's helpers make into the function (the interpreter cannot inline
them while the kernel is installed); the helpers' calls for subtrees without user code
are native, so a restarted call is still cheaper than plain. A root whose class needs user
code goes straight to the original (root pre-check), so its cost is the C call alone:

| cell | plain Ir | 3.15.1 | 3.14.9 | 3.15.1t |
|---|---|---|---|---|
| `__deepcopy__` hook at the root (its children copied natively) | 13,993 | 0.649 | 0.639 | 0.704 |
| restart at the end: 5 records then a hook | 131,181 | 0.484 | 0.461 | 0.573 |
| 5 records then a `uuid.UUID` (Python `__getstate__`) | 144,310 | 0.544 | 0.528 | 0.620 |
| hook last, 4 levels deep | 150,529 | 0.576 | 0.555 | 0.652 |
| same structure without the hook (no restart) | 138,150 | 0.169 | 0.161 | 0.211 |
| registered copier / copyreg reductor / dict subclass | 13,207 / 19,053 / 26,053 | 0.615 / 0.737 / 0.854 | 0.609 / 0.761 / 0.871 | 0.679 / 0.781 / 0.877 |
| `OrderedDict` (C `__reduce__`, not recognised) | 22,489 | 1.033 | 1.044 | 1.034 |
| generic memo (`memo=OrderedDict()`) | 28,664 | 0.941 | 0.956 | 0.941 |
| pair: restart, hook at the root + deepcopy int | 14,700 | 0.642 | 0.635 | 0.691 |
| pair: restart at the end (5 records then a hook) + deepcopy int | 131,844 | 0.486 | 0.462 | 0.572 |
| pair: restart, hook last 4 levels deep + deepcopy int | 151,179 | 0.576 | 0.556 | 0.651 |
| pair: generic memo + deepcopy int | 29,370 | 0.929 | 0.948 | 0.929 |
| pair: `OrderedDict` + deepcopy int | 23,168 | 1.016 | 1.024 | 1.015 |
| pair: `copy.copy` of a `__copy__` hook + copy int (suspends) | 3,495 | 0.983 | 1.013 | 1.000 |
| pair: `copy.copy(x=...)` + copy int (suspends) | 2,479 | 1.000 | 1.000 | 1.000 |
| `copy.copy` of a `__copy__` hook alone (suspends) | 2,824 | 0.983 | 0.992 | 1.000 |
| `asdict(dict_factory=...)` alone / paired with a hit | 16,548 / 29,129 | 0.998 / 0.750 | 0.996 / 0.754 | 1.001 / 0.770 |

Net-benefit policy cells (§1): `copy.copy` with one int per three hook objects — every
fallback costs more than a hit saves — is suspended and runs at 0.995 / 1.000 / 1.000. A
`deepcopy` with one int per three `OrderedDict`s stays installed (1.026 / 1.041 / 1.028):
each restarted `OrderedDict` call copies its items through native hits, which credit the
score.

### 5.2 Install cost (`benches/targets/copy_install.py`, quiet `_accel.enable`, Ir)

Each group declares its validation, state and pins once in a `GroupSpec` (`copy`,
`dataclasses`); the targets only check the group's state and declare their vectorcall
site. "warm": after another group's imported vectorcall target took the process's one-time
costs (calibration), as the budget is stated.

| | 3.15.1 | 3.14.9 |
|---|---|---|
| `enable(['copy'])`, cold (first enable of the process) | 207,845 | 214,424 |
| `enable(['copy'])`, warm | 164,358 | 175,002 (incl. the set-probing self-test) |
| `enable(['dataclasses'])`, warm | 161,745 | 157,280 |
| `enable(['copy', 'dataclasses'])`, warm | 306,871 | 315,067 |
| `import copy` plain / lazy install (warm) | 1,363,467 / 1,525,655 (+162,188, +11.9%) | 560,313 / 731,432 (+171,119, +30.5%) |
| `import dataclasses` plain / lazy install, group `dataclasses` | 5,578,870 / 5,742,836 (+2.9%) | 25,797,866 / 25,960,881 (+0.6%) |

Budget: per-group enable ≤ 0.1M Ir — both groups are over (~160–175k), accepted as a
documented wave-1 overage. Lazy install ≤ max(5% of the import, 0.1M): `dataclasses` is
within (164k ≤ 279k on 3.15, ≤ 1.29M on 3.14); `copy` is over (162–171k vs 100k:
`import copy` is a small module). The cost is dominated by
the code fingerprints of every function the kernels execute instead of CPython — 11 for
`copy` (`deepcopy`, `copy`, five `_deepcopy_*` helpers, `_keep_alive`, `_reconstruct`,
`copyreg.__newobj__`, `copyreg._slotnames`) ≈ 69k at ≈6.3k each, 6 for `dataclasses` ≈ 38k
— then the pins (`copy`: one snapshot of the module globals plus 14 keys; `dataclasses`:
~30 keys, since a snapshot of its globals would deviate on 3.15 when the lazy `import copy`
reifies), module lookups and the two vectorcall sites. Reaching 0.1M would take validating
the rarely used helpers lazily (first use, `accel::degrade` on mismatch) instead of at
enable.

### 5.3 pyperf and workloads

`benches/targets/copy_pyperf.py --fast` (3.15.1, wall time, enabled / plain): deepcopy 10k
records 0.344, 1k dataclass orders 0.275, object tree (3^7 nodes) 0.339, config (500
sections) 0.191; `copy.copy` of a 100k list 1.039 (noise: `list.copy` of 100k items is the
whole call; 1.001–1.013 in other runs); `asdict` 1k orders 0.432; `astuple` 1k orders 0.416.

Workloads (`benches/workloads/run.py compare ... -n 5 -p 3 --only copy,dataclasses`,
3.15.1, outputs identical): `data/state_snapshot` 0.675, `devtools/isort_stdlib` 0.763,
`web/json_api` 0.961, `devtools/sqlglot_transpile` 0.971; geometric mean 0.833.

## 6. Tests

- `tests/targets/test_copy_differential.py`: Hypothesis graphs (shared references, cycles,
  tuples of immutables, sets, frozendicts, plain/slotted/dataclass objects, `__deepcopy__`
  hooks that inspect, mutate, delete from or corrupt the memo or raise, every
  `__reduce_ex__` shape incl. `listiter`/`dictiter`/state tuples/bad shapes,
  `__setstate__`, `__getattr__`, copyreg reductors, registered copiers, metaclass
  `__hash__`, sources mutated during the copy, explicit and logging memos) against fresh
  unpatched copies of `copy.py`/`dataclasses.py`; compared: result structure and
  identities, `getsizeof` of built containers, every hook call with the labelled memo it
  saw and its recursion budget, exceptions. Also: the stack a hook sees and the traceback
  of an exception raised by a hook are CPython's, a tracer attached by a hook sees the rest
  of the call as in CPython, `RecursionError` at the same level and 50,000 levels under a
  high limit, plain-object reduce edge cases, `asdict`/`astuple` errors.
- `tests/targets/test_copy_mechanics.py`: captured references, introspection, `disable()`,
  `sys.setprofile` events, the shared tools-guard checks (settrace, setprofile, cProfile,
  monitoring PY_START/LINE/RAISE, local events), local monitoring on a helper, every
  pinned dependency patched with `mock.patch` (honoured, re-armed), helper defaults
  replaced, 3.14's mutable type sets, dataclasses installed before `copy` is imported (3.15
  lazy import), free-threaded stress.

## 7. Wave 2 (workstream `core`): `uuid.UUID` in `deepcopy` and `asdict`

json_api's `asdict(order)` restarted at every customer id (a `uuid.UUID`: Python
`__getstate__`/`__setstate__`), ~11% of the run (`w2-floors.md` §2). The engine now emulates
those two methods (`crates/rustique-py/src/targets/copy/uuidk.rs`), as it emulates
`copy.py`'s and `copyreg`'s helpers: they are standard-library code verified by fingerprint,
not user code.

- **Recognised class**: `sys.modules['uuid'].UUID` (resolved lazily, never imported, C-level
  reads only), metatype `type`, MRO `(UUID, object)`, `__getstate__`/`__setstate__`
  functions with the recorded code fingerprints (3.14: `4aed2bf10b533995` /
  `ac3a32a4fe7a35af`; 3.15: `34553904bd4f8992` / `1f178d7c0bb94b3c`), every other copy hook
  `object`'s or absent, no finalizer, slots `int`/`is_safe` as member descriptors;
  `uuid.SafeUUID` an enum class whose metatype does plain type lookup and whose `==` is
  `object`'s. Per call: the class's version tag and both methods' code objects unchanged,
  `uuid.SafeUUID` (module global) still the validated class and its version tag unchanged.
- **Exact subset per object**: exact `UUID`, `x.int` an exact `int`, `x.is_safe is
  SafeUUID.unknown` (what `uuid4()`, `UUID(str)`, `UUID(int=)` produce). Subclasses and
  `is_safe` `safe`/`unsafe` (whose `__setstate__` calls the enum) restart as before.
- **Emulated steps**, in CPython's order and frames: `object.__reduce_ex__(4)` →
  `UUID.__getstate__` (one frame against the recursion budget) → `{'int': x.int}` →
  `_reconstruct` (the engine's existing `copyreg.__newobj__` / memo / `deepcopy(state)`
  steps, so a caller's memo receives CPython's entries and keep-alive list) →
  `UUID.__setstate__` (one frame) → the `int` and `is_safe` slots stored as
  `object.__setattr__` stores them.
- Tools: as for every input, a listening tool sends the whole call to the original
  (`tests/targets/test_copy_uuid.py::test_tracer_sees_cpythons_frames`).

callgrind, Ir per call, 3.15.1, plain → `enable(only=["copy", "dataclasses", "datetime"])`
(`benches/targets/core_cells.py uuid`), **measured**:

| cell | plain | enabled | ratio | before |
|---|---:|---:|---:|---:|
| `asdict(Order)`, customer id a UUID (json_api's shape) | 162,375 | 55,387 | **0.341** | 0.853 |
| `asdict(Order)`, customer id an int | 132,897 | 50,072 | 0.377 | 0.377 |
| `asdict(Customer)` with a UUID | 44,129 | 11,584 | 0.262 | — |
| `deepcopy(UUID)` | 27,610 | 5,223 | **0.189** | 0.861 |
| `deepcopy([UUID, UUID])` | 35,014 | 6,683 | 0.191 | — |
| `deepcopy(UUID, memo={})` | 27,641 | 7,432 | 0.269 | — |

Tests: `tests/targets/test_copy_uuid.py` (results, identity of shared copies, caller memo
contents, counters, subclasses and `safe`/`unsafe` restarting with CPython's result, a
patched `__getstate__` running CPython's code, tracers). Not covered: `copy.copy(UUID)`
(shallow; not on a measured hot path), `PurePath` (isort 0.23%, `w2-floors.md` §2 gap #9).

### 7.1 Free-threaded scaling and the "4.4× on 3.15.1t" signal

- 8-thread CPU per call, enabled ÷ plain (3.15.1t, `benches/ft/kernel_scaling.py -t 1,8`,
  local / shared inputs, **measured**): `deepcopy(dict)` 0.239 / 0.215, `deepcopy` of a
  dict holding a UUID 0.213 / 0.174, `deepcopy` of a frozenset 0.020 / 0.023, `asdict`
  0.358 / 0.445: within DECISIONS §10.9 (≤ 1.05); the extra reference-count traffic on
  shared inputs costs part of the 1-thread gain, never more than the original.
- w2-dx's replay of sqlglot's `deepcopy` calls (kernel/original 4.4 on 3.15.1t) **does not
  reproduce as a slowdown**: replaying the 2,691 outermost calls of one `sqlglot_transpile`
  run (`benches/ft/deepcopy_replay.py`, CPU time per call, same process, best of 3–7) gave
  0.77, 0.91 and 1.08 in three runs on the loaded dev box (3.15.1: 0.84); callgrind on the
  same replay counts about half the instructions enabled (see the workstream report for the
  GC-free counts). The calls are root-blocked restarts (sqlglot's `Expression.__deepcopy__`,
  67,950 of 289,692 calls) whose inner `deepcopy`s run natively; no lock, `sched_yield` or
  park shows in the profile. The earlier 4.4 is consistent with CPU-time noise on a box at
  load 40–60, not with a kernel effect.
