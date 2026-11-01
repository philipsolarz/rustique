STATUS: wave 1, groups `copy` and `dataclasses`. Contract: `docs/ACCELERATION.md`.
Code: `crates/rustique-py/src/targets/copy/`. Tests: `tests/targets/test_copy_*.py`.
Benchmarks: `benches/targets/copy_cells.py` (callgrind), `benches/targets/copy_pyperf.py`.

# Targets `copy.deepcopy`, `copy.copy`, `dataclasses.asdict`, `astuple`, `replace`

All five are vectorcall kernels on the existing function objects (contract §3.7): every
captured reference (`from copy import deepcopy`, `partial`, default arguments, the
`deepcopy=deepcopy` defaults inside `copy.py`) reaches the kernel; identity, pickling,
`repr`, signature, `__code__` and source are unchanged.

| target | group | module (lazy install) | validated | free-threaded |
|---|---|---|---|---|
| `copy.deepcopy` | `copy` | `copy` | 3.14, 3.15 | yes |
| `copy.copy` | `copy` | `copy` | 3.14, 3.15 | yes |
| `dataclasses.asdict` | `dataclasses` | `dataclasses` | 3.14, 3.15 | yes |
| `dataclasses.astuple` | `dataclasses` | `dataclasses` | 3.14, 3.15 | yes |
| `dataclasses.replace` | `dataclasses` | `dataclasses` | 3.14, 3.15 | yes |

## 1. How `deepcopy` stays exact

`copy.deepcopy` is recursive: a kernel that fell back once per level would cost 1.12–1.21×
(mechanisms.md §1.2). The kernel therefore runs the **whole** recursion and executes
`Lib/copy.py`'s algorithm step for step:

- **Explicit work stack, no C recursion.** CPython's frames for this recursion live on the
  heap; a structure 50,000 levels deep copies under a high recursion limit there, and does
  here (tested). Hooks that call back into `deepcopy` nest exactly as they nest in CPython.
- **Python-frame depth is emulated on the real thread state.** Every frame `copy.py`
  would push (`deepcopy`, `_deepcopy_list`/`_tuple`/`_dict`/`_frozendict`/`_method`,
  `_keep_alive`, `_reconstruct`, `copyreg.__newobj__`, and on 3.14 the generator
  expression in `_reconstruct`) decrements `tstate->py_recursion_remaining` with
  `_Py_EnterRecursivePy` semantics. `RecursionError` is raised at the same frame, and user
  code called from inside the kernel sees the recursion budget it would see in CPython
  (tested by probing the budget from hooks).
- **The memo.** `deepcopy(x)` keeps the memo natively (insertion-ordered `id → copy`
  entries plus the keep-alive list) until something can see it: a `__deepcopy__(memo)`
  hook, a copier registered in `copy._deepcopy_dispatch`, or CPython's own `_reconstruct`.
  At that moment it materialises the dict with exactly CPython's contents, insertion order
  (the keep-alive list under `id(memo)` at the position of the first `_keep_alive`) and
  keep-alive list growth; from then on, and whenever the caller passes a memo, every memo
  operation is the one `copy.py` performs (so entries hooks add, change or delete are
  honoured). A memo that is not an exact dict (`OrderedDict`, a dict subclass, any mapping)
  gets `copy.py`'s generic operations — `d in memo` then `memo[d]` (3.15) or
  `memo.get(d, _nil)` with `deepcopy`'s own `_nil` (3.14), `memo[k] = v`, `memo[id(memo)]`
  with `except KeyError` — so its methods run with the same arguments in the same order
  (tested with a memo that logs every call), and nested calls never fall back. The native memo releases its references in the order the dict's
  deallocation would.
- **Same operations, same order.** `y[deepcopy(k)] = deepcopy(v)` copies the value first;
  lists are built by appends and dicts by inserts (same `sys.getsizeof`); source lists are
  iterated with `listiter` semantics (size re-read each step) and dicts with `dictiter`
  semantics (`dictionary changed size during iteration` / `keys changed` raised at the same
  step); the tuple rule ("return the original if every item is its own copy", after the
  memo re-check) and the frozendict rule (3.15) are CPython's.
- **Hooks are called where CPython calls them, through the same C-API operation**:
  `getattr(x, "__deepcopy__", None)` is `PyObject_GetOptionalAttr`, `dispatch_table.get`
  and `_deepcopy_dispatch.get` are dict lookups on the live dicts (registrations made at any
  time are honoured, metaclass `__hash__`/`__eq__` run as in CPython), reductors and
  `__reduce_ex__(4)`/`__reduce__` are called, `isinstance(rv, str)` is
  `PyObject_IsInstance`, `_reconstruct` is emulated with `hasattr`/`__setstate__`/
  `isinstance`/`len`/unpacking (`_PyEval_UnpackIterableStackRef`'s messages)/
  `__dict__.update`/`setattr`/`append`/`__setitem__` in its order. A reduce value that is
  not a tuple of 2–5 items goes to CPython's `_reconstruct` itself (with the star-argument
  error message of `LIST_EXTEND`).
- **Natively computed, when provably identical:** atomic types (`_atomic_types`); `list`,
  `tuple`, `dict`, `frozendict` and bound methods through the original helpers;
  `issubclass(cls, type)`; `__deepcopy__`/`__copy__` methods whose code is exactly
  `return self` (`Enum`) — only their frame's depth check remains;
  `object.__reduce_ex__(4)` for plain classes (§3); and `set.__reduce__` for exact
  `set`/`frozenset` (`(type(x), (list(x),), None)`, after the depth check of the
  `copyreg._slotnames(cls)` frame CPython runs on every call there — pinned and
  fingerprinted).

## 2. What is native and what calls CPython's code

| input | path |
|---|---|
| `str`, `int`, `None`, `float`, `bool` (and the rest of `_atomic_types`) | native |
| exact `list`, `dict`, `tuple`, `frozendict`, bound method | native (original helpers intact) |
| plain class instances, dataclasses (incl. frozen), `__slots__` classes | native reduce + native `_reconstruct` (§3) |
| `Enum` members, any `__deepcopy__`/`__copy__` that is `return self` | native |
| exact `set`, `frozenset` | native `set.__reduce__` + native `_reconstruct` |
| `bytearray`, `datetime`, `UUID`, `Decimal`, subclasses of builtins, objects with `__reduce_ex__`/`__getstate__`/`__setstate__`/`__getnewargs__` | the real hook is called; `_reconstruct` emulated natively |
| user `__deepcopy__`, registered copiers | called with the (materialised) memo |
| reduce value not a 2–5 tuple, `_reconstruct` modified | CPython's `_reconstruct` |
| a `copy.py` helper whose code, defaults or local monitoring changed | that helper is called (as a registered copier would be) |

Whole-call fallback (the original function runs; `stats()` reason):
- `args`: a keyword other than `memo`, 3.14's `_nil` argument; `copy.copy` with keywords; `asdict(..., dict_factory=f)` /
  `astuple(..., tuple_factory=f)` with a non-default factory.
- `deps`: a pinned dependency was replaced (deopt-while-unpinned then restores the original
  vectorcall until it is put back): `copy.deepcopy`, `_reconstruct`, `_keep_alive`,
  `_atomic_types`, `_deepcopy_dispatch`, `dispatch_table`, `_copy_atomic_types`,
  `_copy_builtin_containers`, `copy`, `copyreg.__newobj__`, `copyreg._slotnames`,
  `sys.modules['copyreg']`, the `dataclasses` helpers and sentinels, and
  every builtin `copy.py`/`dataclasses.py` reads (`type`, `id`, `getattr`, `hasattr`,
  `isinstance`, `issubclass`, `len`, `setattr`, `zip`, `tuple`, `frozendict`, ...) through
  `pin_global` (so a module global shadowing a builtin counts too); also `deepcopy`'s own
  `__code__`/`__defaults__`.
- `tools`: a `sys.monitoring` tool on any ungrouped event (settrace/setprofile/cProfile/
  coverage, LINE- or RAISE-only tools) or local events on the function's code
  (`accel::tools_quiet`); local events on a `copy.py`/`dataclasses.py` helper make the
  kernel call that helper instead of emulating it.

`copy._deepcopy_dispatch` and `copyreg.dispatch_table` are *not* pinned: they are read
live at every node (their keys are types, which `pin_dict` does not cover), so
registrations never cost a fallback.

## 3. Plain objects: `object.__reduce_ex__(4)` natively

For an instance whose class has the metatype `type`, generic attribute access, no item
size, a `tp_new`, and whose `__reduce_ex__`, `__reduce__` and `__getstate__` are
`object`'s (not shadowed in the instance dict for the first and last), no
`__getnewargs_ex__`/`__getnewargs__`, and an already cached `__slotnames__`, the kernel
builds `(copyreg.__newobj__, (cls,), state, None, None)` itself — what `reduce_newobj`
returns — with `state` the instance `__dict__` (materialised exactly when
`object_getstate_default` materialises it: emptiness is read with the public
`PyObject_VisitManagedDict`, never by creating the dict) or `None`, paired with the slot
values in `__slotnames__` order. All eligibility checks are type-cache lookups and
`_PyObject_GetMethod` (exported, `cx.symbol`), which run no user code, so an ineligible
object takes CPython's path with nothing done twice. `copyreg.__newobj__(cls)` is then
`cls->tp_alloc(cls, 0)` when `cls.__new__` is `object.__new__`, the metatype is `type`
and the class is not abstract (whose error CPython raises).

## 4. `dataclasses`

- `asdict(obj)` / `astuple(obj)` with the default factory: `_is_dataclass_instance`,
  `fields(obj)` (same `getattr(obj, '__dataclass_fields__')`, same `_field_type is _FIELD`
  filter, its frames emulated), `getattr(obj, f.name)` per field (twice for `f.name`, as
  the comprehension reads it), exact `list`/`dict`/`tuple` natively, leaves through
  `copy.deepcopy` (resolved like `LOAD_GLOBAL copy`, including 3.15's lazy import
  reification through `_PyLazyImport_Reify`; run on the copy engine directly when the
  `copy.deepcopy` kernel is installed). Subclasses of `tuple`/`list`/`dict`/`frozendict`
  (namedtuples, defaultdicts) and anything beyond 200 native levels go to CPython's
  `_asdict_inner`/`_astuple_inner` (a list inside `astuple` too: `list(genexpr)`'s
  allocation pattern is CPython's own then).
- `replace(obj, **changes)`: `_replace`'s loop natively — same `f._field_type`, `f.init`,
  `f.name` reads (three per copied field, as CPython), same `changes` insertion order, the
  same `init=False` and `InitVar` errors (f-string formatting via `PyObject_Format`/`Repr`)
  — then `obj.__class__(**changes)` (`__init__`, `__post_init__` run as usual). Classes
  whose `__dataclass_fields__` is not a class-level dict, or whose attribute access is not
  generic, fall back (`args`); an instance-level shadow hands the loop to `_replace`.

## 5. Private state and known limits

- Private state not reproduced: none of `copy.py`'s module state exists; the kernels create
  no caches. The native memo is never visible (it becomes the real dict first).
- Tracebacks of exceptions raised inside a kernel (by hooks or by `copy.py`'s own checks)
  lack the `copy.py` frames CPython would show (type, message, `args`, cause and context
  are identical); frame-walking from hooks (`sys._getframe`, `inspect.stack`) sees no
  `copy.py` frames. Tools that attach *during* a call (a hook enabling `sys.settrace`) see
  the rest of that call's native steps as nothing; calls made after attaching fall back.
- Finalizer timing of temporaries: objects whose last reference is a temporary of the
  reduce protocol (`rv`, a state tuple) are released when the emulated `deepcopy` call ends,
  as in CPython; the order among several such temporaries of one call is not reproduced.
- `copy.Error` is looked up at raise time (`LOAD_GLOBAL`); a module subclass installed as
  `copyreg`'s `__class__` with its own `__getattribute__` is not detected (the pin covers
  `sys.modules['copyreg']` and `copyreg.__newobj__`).

## 6. Conformance (`tests/conformance/run_target_suites.py`)

| build | `test_copy` | `test_copyreg` | `test_dataclasses` | differences |
|---|---|---|---|---|
| 3.15.1 | 84 pass | 6 pass | 284 pass, 1 skip | 0 |
| 3.15.1t | 84 pass | 6 pass | 284 pass, 1 skip | 0 |
| 3.14.9 | 81 pass | 6 pass | 279 pass, 1 skip | 0 |
| 3.14.9t | 81 pass | 6 pass | 279 pass, 1 skip | 0 |

Same pass/fail/skip sets with and without Rustique; every target asserted `installed`.
Full `pytest -q`: 4140 passed, 11 skipped (3.15.1, 3.14.9); 4143 passed, 8 skipped (3.15.1t, 3.14.9t).

## 7. Measurements

### 7.1 Callgrind (Ir per call, `benches/targets/copy_cells.py`, region-instrumented, steady state)

| cell | 3.15.1 CPython | 3.15.1 Rustique | ratio | 3.14.9 CPython | 3.14.9 Rustique | ratio |
|---|---:|---:|---:|---:|---:|---:|
| deepcopy int | 712 | 493 | 0.692 | 753 | 622 | 0.826 |
| deepcopy str | 714 | 493 | 0.690 | 757 | 624 | 0.825 |
| deepcopy record (10 fields) | 24,230 | 4,126 | 0.170 | 25,844 | 4,297 | 0.166 |
| deepcopy list of 5 records | 119,215 | 20,493 | 0.172 | 128,790 | 20,833 | 0.162 |
| deepcopy cfg (3 keys, nested) | 25,373 | 4,506 | 0.178 | 26,625 | 4,705 | 0.177 |
| deepcopy tuple of immutables | 19,354 | 2,371 | 0.122 | 20,479 | 2,512 | 0.123 |
| deepcopy small dataclass | 29,384 | 7,756 | 0.264 | 32,631 | 7,906 | 0.242 |
| deepcopy dataclass tree | 141,114 | 46,111 | 0.327 | 160,581 | 46,675 | 0.291 |
| deepcopy plain object | 35,316 | 8,970 | 0.254 | 38,984 | 9,134 | 0.234 |
| deepcopy slotted object | 38,272 | 9,819 | 0.257 | 41,960 | 10,262 | 0.245 |
| deepcopy enum member | 3,067 | 1,737 | 0.566 | 3,140 | 1,983 | 0.632 |
| deepcopy frozenset | 29,082 | 5,707 | 0.196 | 31,998 | 5,790 | 0.181 |
| deepcopy set | 28,007 | 5,355 | 0.191 | 31,029 | 5,620 | 0.181 |
| deepcopy datetime | 13,104 | 4,189 | 0.320 | 15,951 | 4,681 | 0.293 |
| deepcopy uuid | 27,324 | 11,267 | 0.412 | 30,435 | 11,794 | 0.388 |
| deepcopy Decimal (C __deepcopy__) | 2,816 | 1,299 | 0.461 | 2,806 | 1,471 | 0.524 |
| deepcopy __deepcopy__ hook | 13,993 | 7,073 | 0.505 | 14,339 | 7,410 | 0.517 |
| deepcopy registered copier | 13,218 | 6,631 | 0.502 | 13,493 | 6,982 | 0.517 |
| deepcopy copyreg reductor | 18,958 | 5,346 | 0.282 | 21,716 | 5,573 | 0.257 |
| deepcopy dict subclass | 26,095 | 9,661 | 0.370 | 29,629 | 10,069 | 0.340 |
| deepcopy OrderedDict | 22,489 | 12,854 | 0.572 | 23,927 | 13,593 | 0.568 |
| deepcopy memo=OrderedDict (generic memo) | 28,583 | 10,782 | 0.377 | 30,695 | 11,832 | 0.385 |
| deepcopy explicit {} memo | 25,433 | 8,548 | 0.336 | 26,969 | 8,986 | 0.333 |
| copy int | 645 | 385 | 0.596 | 654 | 448 | 0.685 |
| copy list100 | 3,123 | 2,686 | 0.860 | 3,138 | 2,709 | 0.863 |
| copy dict100 | 5,096 | 4,665 | 0.915 | 5,048 | 4,584 | 0.908 |
| copy record | 1,688 | 1,242 | 0.735 | 1,661 | 1,219 | 0.734 |
| copy plain object | 12,722 | 4,725 | 0.371 | 13,297 | 4,671 | 0.351 |
| copy small dataclass | 12,704 | 4,728 | 0.372 | 13,215 | 4,656 | 0.352 |
| copy enum member | 1,938 | 1,136 | 0.586 | 2,120 | 1,233 | 0.581 |
| copy __copy__ hook | 2,834 | 2,032 | 0.717 | 2,788 | 2,107 | 0.756 |
| copy set | 1,477 | 1,275 | 0.863 | 1,529 | 1,303 | 0.852 |
| copy datetime | 6,629 | 2,635 | 0.397 | 7,664 | 3,000 | 0.391 |
| copy (fallback args: kw) | 1,840 | 1,840 | 1.000 | 1,795 | 1,794 | 1.000 |
| asdict small dataclass | 12,605 | 4,637 | 0.368 | 12,620 | 4,631 | 0.367 |
| asdict dataclass tree | 76,328 | 27,269 | 0.357 | 78,865 | 27,720 | 0.351 |
| astuple small dataclass | 12,483 | 4,212 | 0.337 | 12,576 | 4,194 | 0.333 |
| replace small dataclass | 10,431 | 7,545 | 0.723 | 10,125 | 7,497 | 0.740 |
| replace no changes | 10,001 | 8,054 | 0.805 | 9,715 | 7,875 | 0.811 |
| asdict (fallback: dict_factory) | 16,547 | 16,611 | 1.004 | 16,630 | 16,550 | 0.995 |

Every cell is ≤ 1.05 (`asdict(..., dict_factory=f)`, a whole-call fallback, is 1.004 / 0.995: noise floor ±15 Ir).

Free-threaded 3.15.1t (same script): every cell ≤ 1.0 — deepcopy int 0.666, record 0.208,
list of 5 records 0.216, tuple 0.124, small dataclass 0.275, plain object 0.279,
frozenset 0.209, enum 0.564, `__deepcopy__` hook 0.556, explicit `{}` memo 0.395;
`copy.copy` plain object 0.380, list100 0.834; asdict 0.429, replace 0.790; whole-call
fallbacks 0.999 / 1.000; pairs 0.431 / 0.995 / 0.760.

### 7.2 Under the JIT (3.15.1, `PYTHON_JIT=1`, warmed 30,000 iterations; subset)

| cell | CPython | Rustique | ratio |
|---|---:|---:|---:|
| deepcopy int | 575 | 496 | 0.862 |
| deepcopy enum member | 2,379 | 1,807 | 0.760 |
| deepcopy record (10 fields) | 21,256 | 4,129 | 0.194 |
| deepcopy list of 5 records | 102,423 | 20,496 | 0.200 |
| deepcopy small dataclass | 26,680 | 7,745 | 0.290 |
| copy int | 419 | 387 | 0.924 |
| copy record | 1,450 | 1,244 | 0.858 |
| copy enum member | 1,437 | 1,203 | 0.837 |
| copy small dataclass | 11,226 | 4,716 | 0.420 |
| asdict small dataclass | 10,588 | 4,639 | 0.438 |
| astuple small dataclass | 10,759 | 4,214 | 0.392 |
| replace small dataclass | 9,524 | 7,530 | 0.791 |

All steady-state cells measured under the JIT stay below 1 (the kernels' fast paths replace
calls the JIT cannot inline into anyway), so the five targets keep `jit_safe: true`.

### 7.3 Fallbacks before adaptive suspension

Steady-state fallback cells (§7.1) measure ~1.00 because a target that falls back 64 times
in a row is suspended (the original vectorcall is restored). The first 64 calls pay the
vectorcall fallback. Measured with each fallback followed by a hit that resets the streak
(fallback overhead = pair difference + the hit's measured saving):

| pair cell | CPython | Rustique | ratio | fallback overhead |
|---|---:|---:|---:|---:|
| `deepcopy(cfg, OrderedDict()); deepcopy(7)` (generic memo: no fallback any more) | 29,289 | 11,268 | 0.385 | none (was +3,490 Ir: one fallback per level) |
| `copy.copy(x=rec); copy.copy(7)` | 2,479 | 2,502 | 1.009 | +283 Ir on a 1,840 Ir call (1.15 for the first 64 calls) |
| same, JIT | 2,208 | 2,362 | 1.070 | +186 Ir |
| same, 3.14.9 | 2,438 | 2,502 | 1.027 | +270 Ir |
| `asdict(p, dict_factory=OrderedDict); asdict(p)` | 29,144 | 21,529 | 0.739 | ~+200 Ir on a 16,500 Ir call (1.01) |

The only remaining whole-call fallbacks are keyword forms nobody writes (`copy.copy(x=…)`,
`deepcopy(x=…)`), 3.14's private `_nil` argument, non-default factories, patched
dependencies (suspended immediately by deopt-while-unpinned) and tools.

### 7.4 pyperf, large inputs (3.15.1, `benches/targets/copy_pyperf.py --fast`; shared, loaded box: only ≥ 1.5× is claimed)

| cell | CPython | Rustique | ratio |
|---|---:|---:|---:|
| `deepcopy` 10k records | 32.27 ms | 10.94 ms | 0.339 |
| `deepcopy` 1k dataclass orders (items, tuple, datetime, enum) | 16.63 ms | 8.18 ms | 0.492 |
| `deepcopy` object tree (3^7 plain objects) | 9.58 ms | 3.38 ms | 0.353 |
| `deepcopy` config (500 sections) | 1.18 ms | 0.22 ms | 0.183 |
| `copy.copy` list 100k | 0.14 ms | 0.14 ms | 1.007 (same `list.copy`; noise) |
| `asdict` 1k orders | 6.70 ms | 2.83 ms | 0.422 |
| `astuple` 1k orders | 7.14 ms | 4.95 ms | 0.693 (not claimed) |

### 7.5 Workloads (3.15.1, `benches/workloads/run.py compare … -n 5 -p 3 --only copy,dataclasses`; wall time, loaded box)

| workload | plain | enabled | ratio | digests |
|---|---:|---:|---:|---|
| data/state_snapshot | 665.2 ms | 538.5 ms | 0.809 | same |
| devtools/isort_stdlib | 282.8 ms | 211.6 ms | 0.748 | same |
| web/json_api | 397.4 ms | 376.3 ms | 0.947 | same |
| devtools/sqlglot_transpile | 876.3 ms | 849.0 ms | 0.969 | same |
| geometric mean | | | 0.863 | |


### 7.6 Install cost (3.15.1, `benches/targets/copy_install.py`, callgrind regions)

| cell | Ir |
|---|---:|
| `rustique.enable(only=['copy'])`, copy imported | 1,192,743 |
| `rustique.enable(only=['dataclasses'])`, imported | 1,434,513 |
| `rustique.enable(only=['copy', 'dataclasses'])`, imported | 1,976,981 |
| `import copy`: plain / with the lazy install | 538,725 / 1,163,674 (+116%) |
| `import dataclasses` (copy imported): plain / lazy install | 5,545,192 / 6,386,106 (+15.2%) |

Over the 0.1M Ir / 5% budget (item 13). Trimmed from 2.39M / 4.39M Ir: the `return self`
reference code is compiled on first use instead of at install (0.69M), the group's second
target reuses the first one's validated state instead of re-fingerprinting (copy: 10
functions, dataclasses: 8 functions plus copy's 10 before decoupling), copy's globals are
one `pin_dict` snapshot instead of 20 pins, each dataclasses target pins only what its
emulated helpers read. What remains, inside `registry::enable` for the copy group
(0.63M Ir of the 1.19M; the rest is the Python-level `enable()`): fingerprinting the 10
`copy.py`/`copyreg` functions the kernels emulate 0.28M, vectorcall-site setup 0.15M (a
one-time `py.eval` in `function_vectorcall`), state validation 0.06M, pins 0.12M (each of
the two targets declares its own: disabling one target must not unprotect the other). The
fingerprints are required (every emulated function is checked) and accel-core is making
`fingerprint_code` cheaper; the per-target pins could be shared if the registry kept a
shared flag's pins alive while any target using it is installed.

## 8. Tests

- `tests/targets/test_copy_differential.py`: Hypothesis graphs (shared references,
  cycles, tuples of immutables, sets, frozendicts, plain/slotted/dataclass objects,
  `__deepcopy__` hooks that inspect, mutate, delete from, or corrupt the memo or raise,
  every `__reduce_ex__` shape including `listiter`/`dictiter`/state tuples/bad shapes,
  `__setstate__`, `__getattr__`, copyreg reductors, registered copiers, metaclass
  `__hash__`, sources mutated during the copy), run against fresh unpatched copies of
  `copy.py`/`dataclasses.py`; compared: result structure and identities, `getsizeof` of
  built containers, each hook call with the labelled memo it saw and its recursion budget,
  exceptions. Also recursion limits (same `RecursionError` level; 50,000 levels under a
  high limit), plain-object reduce edge cases, `replace`/`asdict` errors.
- `tests/targets/test_copy_mechanics.py`: captured references, introspection, `disable()`,
  `sys.setprofile` events identical to CPython's, local `sys.monitoring` events on a
  helper, every pinned dependency patched with `mock.patch` (honoured, re-armed), helper
  defaults replaced, 3.14's mutable type sets, free-threaded stress (concurrent copies,
  concurrent source mutation, enable/disable under load).
