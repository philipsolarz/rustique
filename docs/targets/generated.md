STATUS: wave 2, Phase 2, workstream `datamodel`. Group `generated` is described in
`docs/targets/datamodel.md`; this file holds the evidence for one mechanism that needs a
contract amendment before it ships in the default tier: routing positional calls of frozen
dataclasses to the `__init__` kernel through the class's `tp_alloc` (lead's gates, ruling
request of 2026-11-02). Code: `crates/rustique-py/src/targets/generated/dataclass.rs`
(`k_alloc`, `k_process_class`, `on_disable`). Cells: `benches/targets/datamodel_alloc_cells.py`.

# Positional calls of frozen dataclasses: `tp_alloc` routing

## 1. The problem (measured)

A frozen dataclass `__init__` sets each field with `object.__setattr__` (a
`wrapper_descriptor` call without vectorcall): `F(1, 2)` costs 2,827 Ir against ~1,060 for
a plain class. The `__init__` kernel (vectorcall on the generated function) makes it 0.63 —
but only for calls that reach the function's vectorcall. CPython's specializer turns
`Cls(a, b)` into `CALL_ALLOC_AND_ENTER_INIT` for every class whose `tp_new` is
`object.__new__`, `tp_alloc` is `PyType_GenericAlloc` and `__init__` a "simple" function
(no `*args`, `**kwargs`, keyword-only parameters): it allocates and pushes `__init__`'s
frame inline and never reads the function's vectorcall (specialize.c
`get_init_for_simple_managed_python_class`; 3.14 and 3.15 no longer require inline values,
so `__slots__` classes are included). Keyword calls (`CALL_KW`), `kw_only` classes and
`super().__init__` reach the kernel; positional calls of the common shape do not:

| cell (callgrind, Ir/call, 3.15.1) | plain | kernel, no routing | kernel + routing |
|---|---:|---:|---:|
| `F(x=1, y=2)` frozen, keywords | 4,335 | 2,676 (0.617) | 2,733 (0.630) |
| `F(1, 2)` frozen, positional | 2,832 | 2,818 (0.995) | 1,840 (0.651) |
| `S(1, 2)` frozen + slots, positional | 2,774 | 2,765 (0.997) | 1,765 (0.636) |
| `L(1)` frozen, builtin factory, positional | 3,449 | 3,435 (0.996) | 2,213 (0.642) |

In the custom family the hot frozen classes are called positionally (report_engine
`Money(a, c)` 7.4% of `run()`, rules_engine `Outcome(...)` 5.4%, ecs_arena `Cell(x, y)` 3.5%).

## 2. The mechanism

When the dataclass hook gives a class's generated `__init__` its kernel, the
`_process_class` hook (after the class is final: `slots=True` builds a new class) swaps
the class's `tp_alloc` from `PyType_GenericAlloc` to `k_alloc` (compare-and-swap, an
atomic pointer store) and calls `PyType_Modified(cls)` (a call site specialized meanwhile
by user code running inside `_process_class` — a base's `__init_subclass__` on a slots class
— deopts instead of reaching `_ALLOCATE_OBJECT`'s `tp_alloc == PyType_GenericAlloc`
assertion). `k_alloc(type, n)` is `PyType_GenericAlloc(type, n)` — the same allocation,
under another address — plus one check: if `type`'s `__init__` does not carry the kernel,
or a tool is listening (the kernel would fall back), `type` gets `PyType_GenericAlloc` back
(compare-and-swap) so its call sites specialize again. The specializer then fails
(`SPEC_FAIL_OVERRIDDEN`) and specializes the site as `CALL_NON_PY_GENERAL`: `type_call` →
`object_new` → `k_alloc` → `slot_tp_init` → the kernel. `disable()` (`on_disable`, a
weak-reference registry of the routed classes: ~300 Ir on a ~2.8M-Ir class creation)
restores `PyType_GenericAlloc` on every routed class.

Subclasses do not inherit `k_alloc`: `type_new` sets `tp_alloc = PyType_GenericAlloc` on
every class it creates (typeobject.c `type_new_alloc`; `inherit_slots` copies `tp_alloc`
only when it is NULL). A C extension type made with `PyType_FromSpec` on such a base
without `Py_tp_alloc` would inherit it; `k_alloc` gives it `PyType_GenericAlloc` back at its
first allocation (its `__init__` is not the kernel's), the same allocator its base had.

## 3. Gates (lead, 2026-11-02) and measurements

### 3.1 Never slower (callgrind, Ir per call, 3.15.1; `benches/targets/datamodel_alloc_cells.py`)

| cell | plain | on | ratio |
|---|---:|---:|---:|
| `F(1, 2)` frozen positional | 2,827 | 1,840 | 0.651 |
| subclass with its own `__init__` (`super().__init__`) — the "window" case: does not exist, the subclass gets `PyType_GenericAlloc` from `type_new` | 3,469 | 2,214 | 0.638 |
| subclass inheriting the generated `__init__` (not routed: specialized as in CPython) | 2,847 | 2,838 | 0.997 |
| one site alternating a routed class and a plain class | 4,851 | 3,408 | 0.702 |
| non-frozen dataclass (no kernel, not routed) | 1,063 | 1,054 | 0.991 |
| a site specialized while enabled, after `disable()` | 3,075 | 3,066 | 0.997 |
| `disable()` then `enable()` again (classes are not re-routed) | 3,075 | 3,066 | 0.997 |
| under `sys.monitoring` LINE events (tool: the class is given back `GenericAlloc`) | 4,510 | 4,510 | 1.000 |
| under `sys.setprofile` | 6,979 | 7,068 | 1.013 |
| frozen class with a user `default_factory` (no kernel at all since the fix: it would fall back on every defaulted call; before: 1.289) | 3,379 | 3,365 | 0.996 |
| class creation `@dataclass(frozen=True)` + one call (hooks, capture, plan, routing) | 2,816,580 | 2,842,462 | 1.009 |

Classes created before `enable()` are neither given the kernel nor routed (no adoption of
dataclasses this wave): unchanged. Workloads: see the final report (custom, frameworks,
rich_render, pylint_stdlib, strawberry_graphql, json_api, instructions per `run()`).

### 3.2 Exactness: who reads `tp_alloc` (audit of CPython 3.15.1 and 3.14.9 sources)

- `object_new` → `type->tp_alloc(type, 0)` (typeobject.c:2613): `k_alloc` returns what
  `PyType_GenericAlloc` returns (it calls it). Pickling (`copyreg.__newobj__`, `object.__reduce_ex__`),
  `copy`, `cls.__new__(cls)`: the same path.
- `specialize.c:1629` (`get_init_for_simple_managed_python_class`): the intended effect
  (specialization fails, `SPEC_FAIL_OVERRIDDEN`).
- `bytecodes.c:4773` (`_ALLOCATE_OBJECT`): `assert(tp_alloc == PyType_GenericAlloc)` for
  sites already specialized — prevented by `PyType_Modified` at routing time (and asserts
  are compiled out in release builds; debug builds are refused by the version gate).
- `typeobject.c:5201` `check_basicsize_includes_size_and_offsets`: only in
  `PyType_FromMetaclass` (types from specs), never on an existing class.
- `typeobject.c:9048` `inherit_slots` `COPYSLOT(tp_alloc)`: only when the subtype's
  `tp_alloc` is NULL (types from specs without `Py_tp_alloc`): handled by `k_alloc`'s reset.
- `_PyType_NewManagedObject` asserts `GenericAlloc`: no caller in CPython.
- `__sizeof__`, `sys.getsizeof`, `gc`: use `tp_basicsize`/`tp_itemsize`, not `tp_alloc`.
- Not visible from Python: no attribute, `__dict__` entry or `repr` exposes `tp_alloc`.
- Extensions: Cython `cdef class`es cannot subclass a Python class; pybind11/nanobind/PyO3
  classes derive from their own bases (their own `tp_alloc`); C code calling
  `Py_TYPE(o)->tp_alloc` on a dataclass type gets `PyType_GenericAlloc`'s result.

### 3.3 Free-threading

The slot store is a compare-and-swap (`AtomicUsize` on the field): a concurrent reader sees
either function, and both allocate identically. `on_disable` (also run on a suspension,
possibly inside a call: it only swaps pointers and drops weak references) restores by
compare-and-swap. This wave the target is not validated on free-threaded builds: it
depends on the dataclass `__init__` kernel, a dynamic site, and dynamic sites refuse there
(every `PyFunction_SetVectorcall` stops the world; `docs/targets/datamodel.md` §5).

## 4. Proposed amendment to ACCELERATION §3.6 (for DECISIONS)

> 6. **Slots**: only slots no specializer compares by pointer. [...] *Exception (wave 2,
> generated code):* a target may set the `tp_alloc` of a heap type it created or
> recognized at creation (today: a dataclass whose generated `__init__` carries a kernel)
> to a function that returns exactly `PyType_GenericAlloc(type, n)`, so that the
> specializer's `CALL_ALLOC_AND_ENTER_INIT` (which inlines `__init__` and bypasses its
> vectorcall) does not apply and calls reach the kernel through `type_call`. Conditions:
> set only before the class is returned to user code, followed by `PyType_Modified`; the
> function gives any type whose `__init__` does not carry the kernel, and any class while a
> tool listens, `PyType_GenericAlloc` back; `on_disable` restores every routed class;
> cells for subclasses, polymorphic sites, tools and disable/re-enable are part of the
> target's never-slower evidence.

The routing is its own target, `dataclasses.__init__.positional` (group `generated`): until
the amendment is recorded it can be left out with `RUSTIQUE_EXCLUDE=dataclasses.__init__.positional`
(keyword calls keep the kernel); the measured gap without it is the "kernel, no routing"
column above.
