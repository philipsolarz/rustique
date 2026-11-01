STATUS: wave 1, groups `copy` and `dataclasses`. Contract: `docs/ACCELERATION.md`, ruling 15
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
fallbacks debit it). A restart that abandoned copies always debits. Every other fallback
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

- `args`: a memo that is not `None` or an exact `dict`; a keyword other than `memo`; 3.14's
  `_nil` argument; `copy.copy` with keywords; non-default factories.
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

Private state not reproduced: none (`copy.py` has none; the kernels add no caches besides
the code objects recognised as `return self`).

## 4. Conformance (`tests/conformance/run_target_suites.py`)

`--targets copy,dataclasses --modules test.test_copy,test.test_copyreg,test.test_dataclasses
--expect-installed copy.deepcopy,copy.copy,dataclasses.asdict,dataclasses.astuple`: identical
per-test outcomes plain vs enabled on 3.15.1, 3.15.1t, 3.14.9, 3.14.9t (3.15: test_copy 84
pass, test_copyreg 6, test_dataclasses 284 + 1 skip; 3.14: 81 / 6 / 279 + 1 skip), 0
differences. Full pytest on the four builds (after merging wave1 b162e61): all pass except
`tests/test_expr.py::test_filter_matches_python` on 3.15.1t, 3.14.9 and 3.14.9t, a
Hypothesis case of the expression engine (`X + (X / X == c)` at `X = 0` loses the
`ZeroDivisionError`), unrelated to these targets.

## 5. Measurements

### 5.1 Callgrind cells (`benches/targets/copy_cells.py`, Ir per call, enabled / plain)

| cell | plain Ir (3.15.1) | 3.15.1 | 3.14.9 | 3.15.1t |
|---|---|---|---|---|
| deepcopy int | 712 | 0.429 | 0.563 | 0.430 |
| deepcopy record (10 fields) | 24,230 | 0.170 | 0.166 | 0.209 |
| deepcopy list of 5 records | 119,142 | 0.163 | 0.154 | 0.208 |
| deepcopy tuple of immutables | 19,354 | 0.131 | 0.131 | 0.131 |
| deepcopy small dataclass | 29,328 | 0.259 | 0.241 | 0.270 |
| deepcopy dataclass tree (datetimes, enums) | 141,431 | 0.257 | 0.232 | 0.271 |
| deepcopy plain / slotted object | 35,404 / 38,163 | 0.249 / 0.229 | 0.231 / 0.215 | 0.275 / 0.240 |
| deepcopy enum member | 3,002 | 0.453 | 0.531 | 0.448 |
| deepcopy set / frozenset | 27,987 / 29,122 | 0.186 / 0.191 | 0.173 / 0.174 | 0.200 / 0.203 |
| deepcopy datetime | 13,104 | 0.308 | 0.292 | 0.297 |
| deepcopy Decimal | 2,847 | 0.353 | 0.405 | 0.353 |
| deepcopy explicit `{}` memo | 25,459 | 0.296 | 0.290 | 0.357 |
| copy int / record / set | 645 / 1,688 / 1,477 | 0.610 / 0.753 / 0.708 | 0.655 / 0.748 / 0.943 | 0.589 / 0.703 / 0.670 |
| copy list100 / dict100 | 3,123 / 5,076 | 0.869 / 0.922 | 0.869 / 0.917 | 0.840 / 0.890 |
| copy plain object / dataclass | 12,714 / 12,636 | 0.369 / 0.372 | 0.392 / 0.386 | 0.373 / 0.370 |
| copy enum / datetime | 1,884 / 6,629 | 0.424 / 0.359 | 0.690 / 0.421 | 0.395 / 0.334 |
| asdict small dataclass / tree | 12,622 / 76,330 | 0.397 / 0.367 | 0.380 / 0.363 | 0.449 / 0.414 |
| astuple small dataclass | 12,478 | 0.362 | 0.362 | 0.411 |

Restarts (user code reached; CPython's call from scratch). A restart costs the wasted
native prefix plus the C-call entry of every call the original's helpers make into the
function (they are not inlined while the kernel is installed); the helpers' calls for
subtrees without user code run natively, so a restarted call is still cheaper than plain:

| cell | plain Ir | 3.15.1 | 3.14.9 | 3.15.1t |
|---|---|---|---|---|
| `__deepcopy__` hook at the root (its children copied natively) | 13,993 | 0.716 | 0.713 | 0.769 |
| 5 records then a hook (restart at the end) | 131,181 | 0.493 | 0.470 | 0.582 |
| 5 records then a `uuid.UUID` (Python `__getstate__`) | 144,310 | 0.542 | 0.526 | 0.618 |
| hook last, 4 levels deep | 150,529 | 0.581 | 0.562 | 0.658 |
| same structure without the hook (no restart) | 138,172 | 0.169 | 0.160 | 0.210 |
| registered copier / copyreg reductor / dict subclass | 13,207 / 19,053 / 26,053 | 0.649 / 0.779 / 0.849 | 0.655 / 0.802 / 0.872 | 0.713 / 0.821 / 0.872 |
| `OrderedDict` (C `__reduce__`, not recognised) | 22,489 | 1.030 | 1.020 | 1.029 |
| pair: restart, hook at the root + deepcopy int | 14,700 | 0.709 | 0.705 | 0.753 |
| pair: restart, hook last 4 levels deep + deepcopy int | 151,179 | 0.582 | 0.564 | 0.657 |
| pair: `copy.copy` of a `__copy__` hook + copy int | 3,495 | 1.152 | 1.331 | 1.151 |
| `copy.copy` of a `__copy__` hook (suspended by the net-benefit policy) | 2,824 | 0.983 | 0.992 | 1.000 |

Fallbacks before any work: `memo=OrderedDict` 0.991 (alone; suspended) and 1.113 paired
with a hit (every nested call of the original's helpers is a C call); `copy.copy(x, kw)`
paired 1.025; `asdict(dict_factory=...)` 0.998.

### 5.2 Install cost (`benches/targets/copy_install.py`, quiet `_accel.enable`, Ir)

Each group declares its validation, state and pins once in a `GroupSpec` (`copy`,
`dataclasses`); the targets only check the group's state and declare their vectorcall
site. "warm": after another group's imported vectorcall target took the process's one-time
costs (calibration), as the budget is stated.

| | 3.15.1 | 3.14.9 |
|---|---|---|
| `enable(['copy'])`, cold (first enable of the process) | 204,843 | 202,707 |
| `enable(['copy'])`, warm | 161,624 | 162,768 |
| `enable(['dataclasses'])`, warm | 162,103 | 157,220 |
| `enable(['copy', 'dataclasses'])`, warm | 303,961 | 302,569 |
| `import copy` plain / lazy install (warm) | 1,362,881 / 1,521,977 (+159,096, +11.7%) | 559,976 / 720,974 (+160,998, +28.8%) |
| `import dataclasses` plain / lazy install, group `dataclasses` | 5,580,101 / 5,742,220 (+2.9%) | 25,794,376 / 25,961,927 (+0.6%) |

Budget: per-group enable ≤ 0.1M Ir — both groups are over (~160k). Lazy install ≤ max(5%
of the import, 0.1M): `dataclasses` is within (162k ≤ 279k on 3.15, ≤ 1.29M on 3.14);
`copy` is over (159k vs 100k: `import copy` is a small module). The cost is dominated by
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
records 0.328, 1k dataclass orders 0.271, object tree (3^7 nodes) 0.335, config (500
sections) 0.177; `copy.copy` of a 100k list 1.013 (noise: `list.copy` is the whole call);
`asdict` 1k orders 0.432; `astuple` 1k orders 0.418.

Workloads (`benches/workloads/run.py compare ... -n 5 -p 3 --only copy,dataclasses`,
3.15.1, outputs identical): `data/state_snapshot` 0.654, `devtools/isort_stdlib` 0.764,
`web/json_api` 0.960, `devtools/sqlglot_transpile` 0.971; geometric mean 0.826.


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
