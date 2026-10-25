STATUS: provisional (Milestone A, 2026-10-25). Decision and layout requirement are backed by
the measurements below; kernel speedups of the chosen tier are being measured next.

# Takeover: how `rs.enable()` makes builtin containers Rustique

Workstream 1 decision record. Every number is tagged **measured** (with the tool) or
**inferred** (reasoned from source or from other numbers). Instruction counts (Ir) come
from `valgrind --tool=callgrind` with client requests around the measured region (CPython
3.15.1 release build from python-build-standalone: PGO+LTO+BOLT, tail-calling interpreter),
rustique built with `--profile dev-release`.

## 1. Decision (provisional)

1. **The objects stay exact builtins.** `rs.enable()` does not replace `list`/`dict`/`set`
   with another type. It accelerates the builtin types *in place*, process-wide: Rust
   kernels installed behind the builtin methods and functions (`list.sort`, `sorted`,
   `list.index`/`count`, `x in list`, `min`/`max`/`sum`, …) that run only when the inputs
   allow an exact-semantics fast path, and otherwise call the saved original C
   implementation directly.
2. Consequences: literals, displays, comprehensions, `[*a]`, `{**d}`, `list(x)` need no
   rewriting (they already build exact builtins); code compiled before `enable()` (the
   `__main__` script, earlier imports, C extensions) is covered automatically; every
   boundary case works because the objects *are* the builtins; every specializer and JIT
   fast path keeps firing.
3. **No foreign container type can meet the parity bar** (ratio ≤ 1.00/1.05 in every
   scoreboard cell) on CPython 3.14/3.15: even a subclass that inherits every C slot of
   `list` and adds zero work runs `x[i]` at 2.46×, `x.append(v)` at 3.09× and
   `a, b, c = x` at 4.53× the instructions of an exact list (measured, §3.1). So no
   foreign type is replaced by default, whatever its per-op cost.
4. Native storage (`rustique.List` etc.) stays available through the explicit types, and
   through an **opt-in, per-module native tier** (import-hook/runner/decorator rewriting
   of literals, displays, comprehensions and constructor calls to Rustique types) for code
   that accepts that its containers are not builtins. This tier is never the default; its
   boundary breakage and per-construct costs are measured below.

Scope: process-wide for the safe tier; per module for the native tier. UX: `rs.enable()`
(safe tier), plus `python -m rustique [--native PATTERN] app.py` and an `@rs.native`-style
decorator for the native tier (being prototyped).

## 2. Layout requirement for List / Dict / Set (and Tuple / FrozenSet)

For the parallel workstreams. This is the part that is no longer provisional in substance:

- **R1. The takeover needs no subclassing and no particular layout from `List`/`Dict`/
  `Set`.** The default takeover keeps exact builtins; Rustique's own types remain distinct
  types, and their internals (raw C slots, native strategies, compact dict, set index) are
  free to change.
- **R2. A Rustique type with native storage must not be a subclass of a builtin
  container.** If a type subclasses `list`/`dict`/`set`/`tuple`/`frozenset`, the builtin's
  own storage (`ob_item`/`ob_size`/`allocated`; `ma_used`/`ma_keys`/`ma_values`; the set
  table) must hold *all* elements *at all times*, because C code reads it directly after a
  non-exact `PyList_Check`/`PyDict_Check`/`PyAnySet_Check` and nothing can intercept that.
  Measured (§3.2): a best-case native-storage subclass that overrides every Python-visible
  method still returns **silently wrong results** in 17 of 72 boundary checks, e.g.
  `json.dumps(d)` → `'{}'`, `heapq.heappush(x, v)` is a no-op, `frozenset(s)` →
  `frozenset()`, `array.array('q', x)` → empty, `type('T', (), d)` gets an empty
  namespace, `{1}.issubset(s)` → `False`. A type that is *not* a subclass fails loudly
  (`TypeError`) on those paths instead, which is strictly better.
- **R3. Keeping the builtin layout authoritative does not buy parity either**: a
  CPython-layout subclass (candidate 4a) still breaks `type(x) is list`, `marshal`,
  `str.maketrans` (exact-type checks), and misses every exact-type specialization (§3.1).
  So a builtin subclass gains nothing over the exact builtin with in-place kernels.
- **Tuple / FrozenSet / frozendict (3.15)**: same rules. The specializer guards on exact
  `tuple` (`UNPACK_SEQUENCE_TUPLE`, `BINARY_OP_SUBSCR_TUPLE_INT`, `FOR_ITER_TUPLE`,
  `CALL_FUNCTION_EX` args) and on `PyAnySet_CheckExact`/`PyAnyDict_CheckExact`
  (inferred from `Python/bytecodes.c`). Immutability would make a subclass with an
  authoritative `ob_item` plus a native shadow *consistent* (no C code may mutate them),
  but it still pays the handicap, so the default stays the exact builtin (inferred).
- **For the opt-in native tier** the rewrite calls one constructor per type with a freshly
  built builtin temporary (`List(<list display>)`, `Dict(<dict display>)`, …), so the
  semantics of the display (evaluation order, duplicate keys keep the first key object and
  the last value) are CPython's. Interface requests are in §6.

## 3. Measurements so far

### 3.1 Specializer handicap: exact builtin vs slot-inheriting subclass vs `rs` type

Ir per operation (loop and `LOAD_FAST`/`POP_TOP` baseline subtracted), container of 8
elements (100 for iteration), after warm-up. `subclass` is `class sub(list): __slots__ = ()`,
which inherits every C slot of the builtin and adds no work: it is the floor for *any*
foreign type. **measured** (callgrind), CPython 3.15.1:

| op | exact | subclass | rs today | subclass/exact | rs/exact |
|---|---:|---:|---:|---:|---:|
| `x[i]` | 59 | 145 | 307 | **2.46** | 5.20 |
| `x[i] = v` | 63 | 161 | 440 | **2.55** | 6.98 |
| `x.append(v)` | 100 | 308 | 523 | **3.09** | 5.24 |
| `for v in x` (per item) | 75 | 124 | 241 | **1.66** | 3.23 |
| `a, b, c = x` | 93 | 421 | 1780 | **4.53** | 19.14 |
| `if x:` | 41 | 89 | 143 | **2.16** | 3.47 |
| `len(x)` | 115 | 115 | 172 | 1.00 | 1.50 |
| `v in x` (hit at 0) | 99 | 99 | 435 | 1.00 | 4.39 |
| `v in x` (miss, len 8) | 1092 | 1092 | 455 | 1.00 | 0.42 |
| `t += x[j]` loop (per item) | 295 | 381 | 543 | 1.29 | 1.84 |
| `d[k]` (int key) | 142 | 142 | 379 | 1.00 | 2.67 |
| `d[k]` (str key) | 124 | 124 | 506 | 1.00 | 4.08 |
| `d[k] = v` | 150 | 150 | 590 | 1.00 | 3.93 |
| `k in d` | 140 | 310 | 295 | **2.21** | 2.11 |
| `d.get(k)` | 265 | 396 | 556 | **1.49** | 2.10 |
| `for k in d` (per item) | 140 | 140 | 280 | 1.00 | 1.99 |
| `k in s` | 112 | 289 | 295 | **2.58** | 2.63 |
| `s.add(k)` (present) | 225 | 365 | 581 | **1.62** | 2.58 |
| `for v in s` (per item) | 157 | 157 | 255 | 1.00 | 1.62 |

With the JIT on (`PYTHON_JIT=1`, 3.15.1) the gaps widen: subclass/exact is 3.09 for
`x[i]`, 7.20 for `x.append(v)`, 5.29 for `if x:`, 2.24 for `k in d`, 2.37 for `k in s`
(measured). 3.15.1t (free-threaded) matches the GIL build within ±0.4 (`x[i]` 2.40,
`append` 2.27, unpack 4.16, `k in s` 2.34; measured).

Why: list guards are `PyList_CheckExact` (`_GUARD_NOS_LIST`/`_GUARD_TOS_LIST`), set and
dict containment guard on `PyAnySet_CheckExact`/`PyAnyDict_CheckExact`, `CALL_LIST_APPEND`
on the identity of `list.append`, and `CALL_METHOD_DESCRIPTOR_*` on
`Py_IS_TYPE(self, descr->d_type)`. Only dict subscripts (`_GUARD_NOS_DICT_SUBSCRIPT`
compares `mp_subscript == _PyDict_Subscript`) accept subclasses that inherit the slot,
which a native-storage dict cannot do (inferred from `Python/bytecodes.c` 3.15.1).

**Conclusion (measured):** parity for a foreign type is missed by 1.5–4.5× per operation
in the interpreter and up to 7.2× under the JIT, before the type does any work of its own.
No per-op optimisation inside `List`/`Dict`/`Set` can close this; only exact builtins
reach ratio 1.00.

### 3.2 Boundary compatibility matrix (72 checks run by non-opted-in code)

**measured** with the candidate stand-ins (`benches/takeover_overhead.py --compat` once
committed): 6 = exact builtin; 4a = `list`/`dict`/`set` subclass named like the builtin,
builtin storage authoritative, reprs/pickles as the builtin; 4b = subclass with native
(hidden) storage, every Python-visible method overridden; 1/2/3 = the rewrite target
today (`rs.List`/`Dict`/`Set`); 5 = `rs` value materialised to a builtin copy at the
boundary.

| candidate | works | breaks | how it breaks |
|---|---:|---:|---|
| 6 exact builtin (in-place kernels) | 72 | 0 | — |
| 4a subclass, CPython layout | 65 | 7 | `type(x) is list/dict/set`, `marshal` ×3, `str.maketrans` (exact checks, loud) |
| 4b subclass, native storage | 55 | 17 | **silent wrong results** (json `'{}'`, heapq no-op, `frozenset(s)` empty, `array.array` empty, `issubset`, `type()` namespace, pprint) + the 4a set |
| 1/2/3 `rs` type | 35 | 37 | `isinstance`, `repr`, json, pickle/copy type, `exec` globals, heapq, error-message type names, … |
| 5 `rs` + export at boundary | 64 | 8 | callee mutations lost (`heapq`, `bisect.insort`, `random.shuffle`, `x.append`), identity |

### 3.3 Per-construct cost of rewriting (candidates 1/2)

Ir per construct (loop baseline subtracted); a rewrite emits `T(<builtin display>)`.
`ident-call` is a C function returning its argument: the floor for *any* call-based
rewrite. `adopt-subclass` steals the temporary's `ob_item` in O(1) into a list subclass.
**measured**, 3.15.1:

| construct | plain | ident-call | adopt-subclass | subclass copy | rs today |
|---|---:|---:|---:|---:|---:|
| `[]` | 115 | 244 (2.12×) | 654 (5.68×) | 946 (8.22×) | 1408 (12.2×) |
| `[a]` | 267 | 397 (1.49×) | 807 (3.02×) | 1224 (4.58×) | 1785 (6.68×) |
| `[a, …, h]` (8) | 487 | 616 (1.26×) | 1026 (2.11×) | 1570 (3.22×) | 2315 (4.75×) |
| `[0, …, 99]` | 9326 | 9481 (1.02×) | 9891 (1.06×) | 12200 (1.31×) | 14996 (1.61×) |
| `[x for x in r8]` | 1608 | 1757 (1.09×) | 2167 (1.35×) | 2711 (1.69×) | 3456 (2.15×) |
| `[*l4, *l4]` | 784 | 913 (1.16×) | 1323 (1.69×) | 1867 (2.38×) | 2612 (3.33×) |
| `list(l8)` | 776 | 905 (1.17×) | 1315 (1.69×) | 1859 (2.40×) | 2604 (3.36×) |
| `{}` | 159 | 288 (1.81×) | – | 1036 (6.51×) | 2156 (13.6×) |
| `{a: b}` | 497 | 626 (1.26×) | – | 1655 (3.33×) | 4444 (8.94×) |
| `{8 pairs}` | 2114 | 2243 (1.06×) | – | 3537 (1.67×) | 10494 (4.96×) |
| `{x: x for x in r8}` | 2979 | 3130 (1.05×) | – | 4424 (1.49×) | 11381 (3.82×) |
| `{**d4, **d4b}` | 1727 | 1856 (1.07×) | – | 3150 (1.82×) | 10107 (5.85×) |
| `{a}` (set) | 467 | 597 (1.28×) | – | 1456 (3.12×) | 4879 (10.5×) |
| `{a, …, h}` (set) | 1608 | 1737 (1.08×) | – | 3145 (1.96×) | 8722 (5.42×) |
| `set(l8)` | 1957 | 2086 (1.07×) | – | 3488 (1.78×) | 9071 (4.64×) |

The extra call alone costs ~130 Ir per construct, so a rewrite cannot meet the bar for
small displays (`[]` 2.1×, `{}` 1.8×, `[a]` 1.5×) even with a free target (measured).
The safe tier changes no construction bytecode: ratio 1.00 by construction (inferred;
to be confirmed by the benchmark script).

## 4. Candidates (status)

| # | candidate | status |
|---|---|---|
| 1 | import-hook AST rewriting | prototype for the opt-in native tier; construct overhead §3.3 |
| 2 | code-object rewriting (incl. already-compiled functions, via re-compiling from source) | prototype for the decorator / `__main__` path; same runtime cost as 1 |
| 3 | PEP 523 frame-eval hook | to measure: process-wide cost of a pass-through hook |
| 4 | subclassing the builtins | rejected for list/dict/set: §3.1 handicap, §3.2 breakage |
| 5 | lazy materialization at boundaries | rejected: breaks mutation/identity (§3.2), O(n) per crossing |
| 6 | exact builtins accelerated in place | **chosen** default tier; kernels being prototyped |

## 5. Open items (being worked on)

- Safe-tier kernels in `crates/rustique-py/src/takeover.rs`: measured speedups and the
  fallback cost (must be ≤ 1.05 when the fast path does not apply).
- PEP 523 cost, import/compile-time overhead of the rewrite, pyperf wall times,
  `benches/takeover_overhead.py`, tests, final interface requests.

## 6. Interface requests (provisional)

- None are required by the default tier.
- Native tier (opt-in): `List`/`Dict`/`Set` constructors that accept a fresh builtin
  temporary cheaply (O(1) adoption of a builtin `list`'s items for the object strategy;
  today `rs.List([a])` costs 6.7× the display, §3.3), and a builtin-compatible
  presentation mode for takeover-created values (`repr` without the `List(...)` wrapper,
  `'list'` in error messages, pickling to builtins) so that values crossing into
  non-opted-in code fail only where `isinstance`/exact-type checks are involved.
