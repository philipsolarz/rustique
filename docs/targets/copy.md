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
  `return self` (`Enum`) — only their frame's depth check remains; and
  `object.__reduce_ex__(4)` for plain classes (§3).

## 2. What is native and what calls CPython's code

| input | path |
|---|---|
| `str`, `int`, `None`, `float`, `bool` (and the rest of `_atomic_types`) | native |
| exact `list`, `dict`, `tuple`, `frozendict`, bound method | native (original helpers intact) |
| plain class instances, dataclasses (incl. frozen), `__slots__` classes | native reduce + native `_reconstruct` (§3) |
| `Enum` members, any `__deepcopy__`/`__copy__` that is `return self` | native |
| `set`, `frozenset`, `bytearray`, `datetime`, `UUID`, `Decimal`, subclasses of builtins, objects with `__reduce_ex__`/`__getstate__`/`__setstate__`/`__getnewargs__` | the real hook is called; `_reconstruct` emulated natively |
| user `__deepcopy__`, registered copiers | called with the (materialised) memo |
| reduce value not a 2–5 tuple, `_reconstruct` modified | CPython's `_reconstruct` |
| a `copy.py` helper whose code, defaults or local monitoring changed | that helper is called (as a registered copier would be) |

Whole-call fallback (the original function runs; `stats()` reason):
- `args`: a keyword other than `memo`, 3.14's `_nil` argument; `copy.copy` with keywords; `asdict(..., dict_factory=f)` /
  `astuple(..., tuple_factory=f)` with a non-default factory.
- `deps`: a pinned dependency was replaced (deopt-while-unpinned then restores the original
  vectorcall until it is put back): `copy.deepcopy`, `_reconstruct`, `_keep_alive`,
  `_atomic_types`, `_deepcopy_dispatch`, `dispatch_table`, `_copy_atomic_types`,
  `_copy_builtin_containers`, `copy`, `copyreg.__newobj__`, `sys.modules['copyreg']`, and
  every builtin `copy.py`/`dataclasses.py` reads (`type`, `id`, `getattr`, `hasattr`,
  `isinstance`, `issubclass`, `len`, `setattr`, `zip`, `tuple`, `frozendict`, ...) through
  `pin_global` (so a module global shadowing a builtin counts too); also `deepcopy`'s own
  `__code__`/`__defaults__`.
- `tools`: a `PY_START` listener (settrace/setprofile/cProfile/coverage) or local events on
  the function's code.

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
RESULTS_CONFORMANCE

## 7. Measurements

### 7.1 Callgrind, 3.15.1 (Ir per call, `benches/targets/copy_cells.py`, region-instrumented, steady state)

| cell | CPython | Rustique | ratio |
|---|---:|---:|---:|
| deepcopy int | 712 | 496 | 0.696 |
| deepcopy str | 714 | 496 | 0.694 |
| deepcopy record (10 fields) | 24,230 | 4,123 | 0.170 |
| deepcopy list of 5 records | 119,216 | 20,502 | 0.172 |
| deepcopy cfg (3 keys, nested) | 25,442 | 4,510 | 0.177 |
| deepcopy tuple of immutables | 19,354 | 2,367 | 0.122 |
| deepcopy small dataclass | 29,376 | 7,833 | 0.267 |
| deepcopy dataclass tree | 141,272 | 46,009 | 0.326 |
| deepcopy plain object | 35,373 | 8,994 | 0.254 |
| deepcopy slotted object | 38,220 | 9,852 | 0.258 |
| deepcopy enum member | 3,002 | 1,784 | 0.594 |
| deepcopy frozenset | 29,082 | 5,696 | 0.196 |
| deepcopy set | 27,941 | 5,362 | 0.192 |
| deepcopy datetime | 13,104 | 4,193 | 0.320 |
| deepcopy uuid | 27,280 | 11,322 | 0.415 |
| deepcopy Decimal (C __deepcopy__) | 2,836 | 1,313 | 0.463 |
| deepcopy __deepcopy__ hook | 14,042 | 7,155 | 0.510 |
| deepcopy registered copier | 13,128 | 6,695 | 0.510 |
| deepcopy copyreg reductor | 18,958 | 5,390 | 0.284 |
| deepcopy dict subclass | 26,027 | 9,660 | 0.371 |
| deepcopy OrderedDict | 22,509 | 12,763 | 0.567 |
| deepcopy memo=OrderedDict (fallback args) | 28,583 | 28,474 | 0.996 |
| deepcopy explicit {} memo | 25,502 | 8,617 | 0.338 |
| copy int | 645 | 386 | 0.598 |
| copy list100 | 3,123 | 2,686 | 0.860 |
| copy dict100 | 5,146 | 4,620 | 0.898 |
| copy record | 1,688 | 1,242 | 0.735 |
| copy plain object | 12,688 | 4,725 | 0.372 |
| copy small dataclass | 12,796 | 4,764 | 0.372 |
| copy enum member | 1,911 | 1,133 | 0.593 |
| copy __copy__ hook | 2,926 | 2,029 | 0.693 |
| copy set | 1,477 | 1,275 | 0.863 |
| copy datetime | 6,629 | 2,653 | 0.400 |
| copy (fallback args: kw) | 1,840 | 1,840 | 1.000 |
| asdict small dataclass | 12,605 | 4,635 | 0.368 |
| asdict dataclass tree | 76,314 | 27,221 | 0.357 |
| astuple small dataclass | 12,483 | 4,210 | 0.337 |
| replace small dataclass | 10,431 | 7,550 | 0.724 |
| replace no changes | 10,001 | 8,059 | 0.806 |
| asdict (fallback: dict_factory) | 16,547 | 16,528 | 0.999 |

RESULTS_MORE

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
