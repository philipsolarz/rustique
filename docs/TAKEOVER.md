> **History.** This is the Phase-1 decision record of the takeover work (wave 1). Its
> *native tier* (`enable(native=...)`, `--native`, `@rs.native`, the literal rewrite, the
> Rustique containers) was retired in 0.3 (docs/MIGRATION.md; code at tag
> `legacy-native-types`); its benchmarks `benches/takeover_overhead.py` and
> `takeover_compat.py` went with it. The in-place (safe) tier it chose is the engine, and the
> measurements cited from docs/ACCELERATION.md as T§n still stand as evidence.

STATUS: decided (Phase 1, 2026-10-25). The decision and the layout requirement are final
for Phase 1; §8 lists what would reopen them.

# Takeover: how `rs.enable()` makes builtin containers Rustique

Decision record of workstream 1. Every number is tagged **measured** (tool named) or
**inferred** (reasoned from CPython source or from other numbers). Instruction counts (Ir)
come from callgrind with client requests around exactly the measured region, on the
python-build-standalone CPython 3.15.1 (PGO+LTO+BOLT, tail-calling interpreter) and 3.15.1t
(GIL disabled), rustique built with `--profile dev-release`. Reproduce everything with
`benches/takeover_overhead.py` (§11).

## 1. Decision

1. **`rs.enable()` keeps every container an exact builtin and accelerates the builtin
   types in place, process-wide** (the *safe tier*). Exact-semantics Rust kernels are
   installed behind builtin methods: `list.sort` (and so `sorted`), `list.index`,
   `list.count`, `x in list`, and the same for `tuple`. The builtin *functions* (`sum`,
   `min`, `max`, `sorted`, `any`, `all`) come from workstream 5's `rustique._builtins`,
   which `enable()` installs when present. A kernel runs only where it provably computes
   CPython's result and otherwise calls CPython's own C implementation.
2. Because nothing replaces the objects, literals, displays, comprehensions, `[*a]`,
   `{**d}` and `list(x)` need no rewriting. Code compiled before `enable()` (the `__main__`
   script, earlier imports, C extensions) is covered, every specializer and JIT fast path
   keeps firing, and every boundary case works at full speed (77/77, §3.2).
3. **Parity bar, measured**: the scoreboard's touched list and tuple cells (sizes 0–100) are
   218/218 at ratio ≤ 1.05 on both 3.15.1 and 3.15.1t, fallbacks included (worst 1.043 and
   1.047).
   Untouched operations run unchanged bytecode: ratio 1.00.
4. **No foreign container type can be the default.** Even a subclass that inherits every C
   slot of `list` costs 2.4–4.5× the instructions of `list` on `x[i]`, `x[i] = v`,
   `x.append`, `a, b, c = x` and `if x:` (3–7× under the JIT). That is the floor for
   *any* foreign type, before it does any work of its own (§3.1).
5. **Native storage stays explicit or opt-in.** `rustique.List`/`Dict`/`Set` remain
   distinct types. An opt-in, per-module *native tier* rewrites literals, displays,
   comprehensions and `list()`/`dict()`/`set()` calls of selected modules to them:
   `rs.enable(native="pkg.*")`, `python -m rustique --native 'pkg.*' app.py`, or
   `@rs.native`. It is never the default: it misses the parity bar by construction (§3.3)
   and breaks 40 of 77 boundary checks, 30 more being slow (§3.2).

## 2. Layout requirement for List / Dict / Set (and Tuple / FrozenSet / frozendict)

- **R1. The takeover imposes no layout on `List`/`Dict`/`Set`**, and they need not
  subclass anything. The default tier keeps exact builtins; internals (raw slots, native
  strategies, compact dict, set index) are free.
- **R2. A type with native storage must not subclass a builtin container.** C code reads
  the builtin layout directly after a non-exact `PyList_Check`/`PyDict_Check`/
  `PyAnySet_Check`, and nothing can intercept that. So a subclass's builtin storage
  (`ob_item`/`ob_size`; `ma_keys`/`ma_values`; the set table) would have to hold every
  element at all times. **Measured**: a best-case native-storage subclass that overrides
  every Python-visible method still returns *silently wrong* results in 19 of 77 checks:
  - `json.dumps(d)` gives `'{}'`;
  - `heapq.heappush` does nothing;
  - `frozenset(s)` and `array.array('q', x)` come back empty;
  - `type('T', (), d)` gets an empty namespace;
  - PyO3 `cast::<PyList>()`/`cast::<PyDict>()` sees empty containers.

  A type that is not a subclass fails loudly instead (`TypeError`).
- **R3. A builtin-layout subclass buys nothing**: it still breaks `type(x) is list`,
  `marshal` and `str.maketrans` (exact-type checks), and it pays the full specializer
  handicap (§3.1). It is strictly worse than the exact builtin with kernels.
- **Tuple / FrozenSet / frozendict (new in 3.15)** follow the same rules: their
  specializations guard on exact types (`UNPACK_SEQUENCE_TUPLE`,
  `BINARY_OP_SUBSCR_TUPLE_INT`, `FOR_ITER_TUPLE`, `PyAnySet_CheckExact`,
  `PyAnyDict_CheckExact`; inferred from `Python/bytecodes.c`). Immutability would make a
  builtin-layout subclass with a native shadow *consistent* (C code cannot mutate it), but
  it would still pay the handicap. So the default stays the exact builtin, and the native
  types stay distinct.
- **For the opt-in native tier** the rewriter calls one constructor per kind with a freshly
  built builtin temporary (`__rs_list__([...])`). Display semantics are therefore
  CPython's. The rewriter prefers a `T._from_builtin(tmp)` hook when a type offers one
  (interface request, §9).

## 3. Measurements

### 3.1 Specializer handicap of a foreign container type

Ir per operation (loop and `LOAD_FAST`/`POP_TOP` baseline subtracted), 8 elements (100 for
iteration). `subclass` is `class sub(list): __slots__ = ()`, which inherits every C slot and
adds no work. **measured**, 3.15.1:

| op | exact | subclass | rs today | subclass/exact | rs/exact |
|---|---:|---:|---:|---:|---:|
| `x[i]` | 59 | 144 | 306 | **2.44** | 5.18 |
| `x[i] = v` | 63 | 161 | 440 | **2.55** | 6.97 |
| `x.append(v)` | 114 | 313 | 520 | **2.75** | 4.57 |
| `for v in x` (per item) | 75 | 124 | 241 | **1.66** | 3.23 |
| `a, b, c = x` | 93 | 421 | 1785 | **4.52** | 19.17 |
| `if x:` | 41 | 88 | 142 | **2.18** | 3.51 |
| `len(x)` | 115 | 115 | 172 | 1.00 | 1.49 |
| `v in x` (miss, 8) | 1092 | 1092 | 455 | 1.00 | 0.42 |
| `d[k]` | 142 | 142 | 394 | 1.00 | 2.77 |
| `d[k] = v` | 150 | 150 | 590 | 1.00 | 3.93 |
| `k in d` | 140 | 310 | 310 | **2.21** | 2.21 |
| `d.get(k)` | 264 | 395 | 571 | **1.50** | 2.16 |
| `k in s` | 112 | 289 | 310 | **2.58** | 2.77 |
| `s.add(k)` | 224 | 364 | 580 | **1.62** | 2.59 |
| `for k in d` / `for v in s` | 140 / 157 | 140 / 157 | 280 / 254 | 1.00 | 1.99 / 1.62 |

Under the JIT (`PYTHON_JIT=1`, `takeover_overhead.py handicap --jit`) subclass/exact is
3.04 for `x[i]`, 3.15 for `x[i] = v`, 6.16 for `x.append`, 5.17 for `if x:`, 3.92 for
`a, b, c = x`, 2.24 for `k in d` and 2.37 for `k in s` (an earlier longer-warm-up run gave
7.20 for `append`). On 3.15.1t it matches the GIL
build within ±0.4. Both **measured**.

Why (inferred from `Python/bytecodes.c` 3.15.1): list guards are `PyList_CheckExact`. Set
and dict containment guard on `PyAnySet_CheckExact`/`PyAnyDict_CheckExact`.
`CALL_LIST_APPEND` checks the identity of `list.append`, and `CALL_METHOD_DESCRIPTOR_*`
checks `Py_IS_TYPE(self, descr->d_type)`. Only dict subscripts compare slot pointers
(`mp_subscript == _PyDict_Subscript`), which a native-storage dict cannot satisfy.
**No per-op work inside List/Dict/Set can close this gap; only exact builtins reach 1.00.**

### 3.2 Boundary compatibility (77 checks run by non-opted-in code)

`benches/takeover_compat.py`, pinned by `tests/test_takeover_compat.py`, **measured** on
3.15.1 (3.15.1t: same columns within ±3 slow/works). A cell counts as *slow* when it is
correct but takes more than 1.5× the exact builtin's wall time, timed interleaved with the
reference and confirmed by a second measurement on an idle machine:

| candidate | works | slow | breaks | how it breaks / why it is slow |
|---|---:|---:|---:|---|
| 6 exact builtin | 77 | 0 | 0 | — |
| 6+K exact builtin + kernels (`rs.enable()`) | 77 | 0 | 0 | — |
| 4a builtin subclass, builtin storage authoritative | 62 | 8 | 7 | breaks: `type(x) is` ×3, `marshal` ×3, `str.maketrans` (loud); slow: pickle and copy go through `__reduce_ex__` instead of the exact-type fast paths (1.6–3.2×) |
| 4b builtin subclass, native storage | 16 | 42 | 19 | **silent wrong results** (R2) + the 4a set; every protocol call goes through Python-level overrides |
| 1/2/3 `rs` type (what any rewrite emits today) | 7 | 30 | 40 | see below; slow: generic protocol paths instead of exact-type fast paths (`==` with builtins 2.1–6.7×, `f(**d)`/`dict(d)`/`{**d}` 4.8×, `f(*x)` 2.0×, `'-'.join` 2.5×, set algebra with builtins 3–5×) |
| 5 `rs` type, copied to a builtin at the boundary | 24 | 45 | 8 | callee mutations lost (heapq, bisect, shuffle, append/sort), identity; slow: an O(n) copy per crossing |

The checks are `isinstance`, `type(x) is`, `repr`/`str`/f-string, `==`, json (C and Python
encoders), pickle round trip and stream, `marshal`, `copy`/`deepcopy`, `pprint`,
`dataclasses.asdict`, `f(*x)`, `f(**d)`, `'-'.join`, `struct.pack(*x)`, `np.array`,
`array.array`, `match` sequence/mapping, `sorted`/`sum`/`min`/`max`,
`tuple`/`list`/`set`/`Counter`, `reversed`, `itertools.chain`, `[0] + x`, `ref.extend`,
slice assignment, `str.format`, `itemgetter`, error messages, heapq, bisect,
`random.shuffle`, callee mutation, identity, `exec` globals, `str.maketrans`, `type()`
namespaces, set algebra with builtins, `frozenset(s)`, and four PyO3 consumers
(`cast::<PyList>()` + `get_item`, `Vec<i64>` extraction, `cast::<PyDict>()` + iteration,
`cast::<PySet>()` + iteration).

The 40 native-tier breaks fall into two groups:
- **Irreducible** for any non-builtin type (28): `isinstance`, `type(x) is`, json,
  `marshal`, heapq, `exec` globals, `str.maketrans`, `type()` namespaces, PyO3 `cast`,
  `pprint`'s dispatch on `type(x).__repr__`, and copy/deepcopy returning `List`.
- **Presentation**, fixable by the List/Dict/Set workstreams (12): `List([...])` repr,
  `'List'` in error messages, pickling to a rustique class, and the json error naming `Set`.

### 3.3 Cost of rewriting constructs (candidates 1 and 2)

Ir per construct, loop subtracted, **measured** 3.15.1. Columns:
- *call floor*: a C function returning its argument, the cheapest any call-based rewrite
  can be;
- *O(1) adopt*: steals the temporary's `ob_item` into a list subclass;
- *native tier*: the real rewriter's output with `rustique.List`/`Dict`/`Set` targets.

| construct | CPython | call floor | O(1) adopt | native tier |
|---|---:|---:|---:|---:|
| `[]` | 115 | 2.12× | 5.58× | 12.33× |
| `[a]` | 267 | 1.49× | 2.97× | 6.72× |
| `[a, …, h]` | 487 | 1.26× | 2.08× | 5.28× |
| `[0, …, 99]` | 9278 | 1.03× | 1.06× | 1.62× |
| `[x for x in r8]` | 1607 | 1.09× | 1.34× | 2.17× |
| `[x for x in r100]` | 16480 | 1.00× | 1.02× | 1.35× |
| `[*l4, *l4]` | 798 | 1.16× | 1.67× | 3.32× |
| `list(l8)` | 766 | 1.17× | 1.69× | 2.62× |
| `{}` | 159 | 1.81× | – | 13.63× |
| `{a: b}` | 492 | 1.26× | – | 9.01× |
| `{x: x for x in r8}` | 2969 | 1.05× | – | 3.82× |
| `{**d4, **d4b}` | 1717 | 1.08× | – | 5.86× |
| `{a}` (set) | 469 | 1.28× | – | 10.46× |
| `set(l8)` | 1952 | 1.07× | – | 2.73× |
| `f(*x)` (receives `List`) | 1226 | – | – | 3.06× |
| `f(**d)` (receives `Dict`) | 4091 | – | – | 4.30× |

The extra call alone (~130 Ir) puts small displays at 1.5–2.1×, so no rewrite can meet
the bar for them, whatever its target (measured). The safe tier changes no construction
bytecode, and the scoreboard's `literal/display` cells are 1.00 on both builds (measured).

### 3.4 Safe-tier kernels

Ir per statement, CPython vs installed kernels in the same process
(`benches/takeover_overhead.py kernels`), **measured**:

| statement | 3.15.1 | 3.15.1t |
|---|---:|---:|
| `y = x.copy(); y.sort()`, 8 ints / floats / strs | 0.63 / 0.68 / 0.76 | 0.66 / 0.71 / 0.77 |
| `sorted(x)`, 100 ints / 100 strs / 1000 ints | 0.46 / 0.58 / 0.44 | 0.46 / 0.58 / 0.44 |
| `v in x`, 8 ints / 8 strs / 100 ints (miss) | 0.36 / 0.28 / 0.23 | 0.41 / 0.41 / 0.20 |
| `v in x` hit at 0 / empty list | 0.81 / 0.88 | 0.81 / 0.86 |
| `x.index(v)` (8 ints) / `x.count(v)` (100 strs) | 0.55 / 0.13 | 0.65 / 0.15 |
| fallback `sorted` of tuples / with `key=` | 1.02 / 1.02 | 1.02 / 1.02 |
| fallback `v in` objects / tuples / 20 ints + 20 objects | 1.01 / 1.01 / 0.86 | 1.03 / 1.01 / 0.89 |
| fallback `x.index(v, 0)` / `count` objects | 1.01 / 1.02 | 1.01 / 1.03 |
| untouched `x[i]`, `append`+`pop` | 1.00 | 1.00 |

Scoreboard (`benches/scoreboard.py instr --impls enabled`, sizes 0–100), **measured**:

| cell | 3.15.1 | 3.15.1t |
|---|---:|---:|
| `x.sort()` int 8 / int 100 / float 100 / str 100 | 0.61 / 0.46 / 0.49 / 0.58 | 0.67 / 0.46 / 0.50 / 0.59 |
| `x.sort()` object 100 (fallback) | 1.00 | 1.00 |
| `sorted(x)` int 100 | 0.51 | 0.52 |
| `hit in x` int 8 / int 100 | 0.43 / 0.24 | 0.53 / 0.21 |
| `miss in x` float 100 / str 100 / object 100 / int 0 | 0.18 / 0.14 / 1.00 / 0.87 | 0.14 / 0.15 / 1.00 / 0.85 |
| `x.index(hit)` int 100, `x.count(hit)` str 100 | 0.24, 0.37 | 0.22, 0.38 |
| tuple `hit in x` int 100, `count` float 100 | 0.24, 0.18 | 0.24, 0.19 |
| worst touched cell | 1.043 (`list miss in x object 1`) | 1.047 (same cell) |

Wall time, pyperf scoreboard cells (`benches/scoreboard.py time --impls enabled`, idle
machine, enabled and plain CPython in separate processes), 3.15.1, **measured**:

| op (list) | int 10k / 1M | float 10k / 1M | str 10k / 1M | object 10k / 1M |
|---|---:|---:|---:|---:|
| `hit in x` | 0.20 / 0.27 | 0.13 / 0.28 | 0.46 / 0.56 | 0.80 / 1.00 |
| `miss in x` | 0.23 / 0.31 | 0.15 / 0.22 | 0.19 / 0.50 | 1.16 / 0.95 |
| `x.index(hit)` | 0.23 / 0.29 | 0.09 / 0.22 | 0.38 / 0.59 | 0.97 / 1.00 |
| `x.count(hit)` | 0.22 / 0.35 | 0.20 / 0.37 | 0.43 / 0.50 | 0.79 / 1.13 |
| `sorted(x)` | 0.26 / 0.24 | 0.30 / 0.26 | 0.55 / 0.33 | 1.17 / 1.03 |
| `x.sort()` | 0.26 / 0.15 | 0.32 / 0.20 | 0.58 / 0.33 | 1.08 / 0.93 |

Tuples behave the same (`in`/`index`/`count`: 0.12–0.51).

**Noise.** On this WSL2 VM, pyperf's per-cell coefficient of variation has a median of 18%
and a maximum of 46%, so wall time cannot resolve a ≤ 1.05 bar. The object column is
delegated wholesale to CPython's own code, and its 0.79–1.17 spread is cross-process
noise: callgrind on the same cells at 10k elements gives 1.000 for all six object cells
(16–389 Ir of difference on 1.2–25 M), 1.000 for the untouched `for v in x`, and
0.19–0.41 for the int cells (**measured**, `benches/results/enabled-instr-3.15.1-10k.json`).

Why the gains: CPython compares every element through `PyObject_RichCompareBool`, about
130 Ir per element on 3.15.1 (measured: `v in x`, 8 ints, 1092 Ir). A kernel compares
exact ints, floats and strings by value inline, and sorts (key, pointer) pairs with Rust's
stable sort instead of timsort over boxed objects.

### 3.5 PEP 523 frame-evaluation hook (candidate 3)

A pass-through hook that only calls `_PyEval_EvalFrameDefault`, **measured** (Ir per
unit, ratio to no hook):

| workload | 3.15.1 | 3.15.1 JIT | 3.15.1t |
|---|---:|---:|---:|
| `fib(15)` (per Python call) | 1.56 | 1.88 | 1.56 |
| `o.m(i)` method call | 1.78 | 1.72 | 1.79 |
| generator iteration (per item) | 1.54 | 1.48 | 1.47 |
| loop without calls / inlined comprehension | 1.00 / 1.00 | 1.00 / 1.00 | 1.00 / 0.98 |

With a hook installed, the interpreter stops inlining Python-to-Python calls
(`IS_PEP523_HOOKED` guards), so every call and every generator resume goes through the hook
on a new C frame. The cost is process-wide and hits code that never opted in, and a hook
that did real work would cost more. It also cannot change what `BUILD_LIST` builds: it could
only swap code objects, which is the rewrite of §3.3 with a higher fixed cost.
**Rejected.**

### 3.6 Native tier: compile and import cost

Wall time, best of 7 or 5, **measured** 3.15.1:
- **Compile**: parse + rewrite + compile of 8 stdlib modules (~16k lines) takes 356 ms
  against 74 ms for `compile()` alone (4.8×). About 35 ms per 1000 lines, once per source
  change.
- **Import of a copy of `argparse` (3000 lines)**: cold 20.5 ms plain vs 62.8 ms native;
  warm (bytecode cache hit) 1.11 ms vs 1.21 ms. Rewritten bytecode is cached under its own
  tag (`mod.cpython-315.opt-rsnative1.pyc`).
- **Startup**: `import rustique` costs 1.1 ms. The first `enable(native=...)` adds 2.3 ms
  (the `ast` module and the hook).

### 3.7 Native tier against CPython's own test suites

`tests/test_takeover_conformance.py` imports CPython 3.15.1 test modules *through the
native-tier hook*. It covers `test_list`, `test_tuple`, `test_dict` (with `list_tests`,
`seq_tests` and `mapping_tests` rewritten too), `test_set`, `test_dictviews`,
`test_grammar`, `test_scope`, `test_super`, `test_class`, `test_named_expressions`,
`test_copy` and `test_sort`. **measured**:

- **Identity mapping** (rewrite on, targets mapped back to the builtins): 1288 tests,
  **0 outcomes differ from the unrewritten run.** This is the strongest evidence that the
  rewrite preserves semantics: scoping, cells, evaluation order, positions, annotations,
  generators, classes.
- **Rustique mapping** (literals become `rustique` types):

  | module | pass |
  |---|---|
  | `test_list` | 63/69 |
  | `test_tuple` | 38/38 |
  | `test_dict` | 118/144 |
  | `test_set` | 625/644 |
  | `test_dictviews` | 13/16 |
  | `test_grammar` | 71/75 |
  | `test_scope` | 39/41 |
  | `test_super` | 38/39 |
  | `test_copy` | 81/84 |
  | `test_sort` | 17/21 |
  | `test_named_expressions` | 42/62 |
  | `test_class` | fails at import |

  `test_class` executes `type("AllTests", (object,), d)` with a rewritten dict, and
  `exec()`, `type()` and `__getnewargs_ex__` require an exact `dict`. That is the
  irreducible class of §3.2 hitting an opted-in module's *own* code. Everything else is a
  type deviation for workstreams 2–4: reprs, view reprs, iterator pickling, error-message
  type names, `setdefault`/`update` atomicity details, and `rs.List.sort` not keeping
  float/str element identity (native storage).

## 4. Candidates

| # | mechanism | verdict | deciding numbers |
|---|---|---|---|
| 1 | import-hook AST rewriting | **kept as the opt-in native tier** | +130 Ir per construct floor (§3.3); 0 differences on 1288 CPython tests under identity mapping (§3.7); 4.8× compile time (§3.6) |
| 2 | code-object rewriting of compiled functions | **kept as `@rs.native`** (re-compiles from source; closures, `__class__`, line numbers kept) | same runtime as 1; raw bytecode patching rejected: same emitted call, fragile across 3.14/3.15 bytecode (inferred) |
| 3 | PEP 523 frame-eval hook | **rejected** | 1.47–1.88× on every Python call process-wide (§3.5) |
| 4 | builtin subclassing | **rejected** for list/dict/set | 1.5–4.5× handicap (3–7× JIT) (§3.1); native storage behind it corrupts silently (§3.2) |
| 5 | lazy materialisation at boundaries | **rejected** | loses callee mutations and identity (8 breaks); O(n) per crossing; boundaries cannot be found statically |
| 6 | exact builtins accelerated in place | **chosen: the default** | 77/77 boundaries, none slow; 218/218 touched cells at parity on both builds; fast paths at 0.13–0.8 |

## 5. Scope and UX

- `import rustique as rs; rs.enable()` turns on the safe tier, process-wide. It is
  idempotent, and `rs.disable()` restores the originals. Call it early, before starting
  threads.
- `rs.enable(native=["myapp.*"])` also opts the matching modules into the native tier when
  they are imported later (`fnmatch` patterns; `rustique`, `importlib` and `encodings` are
  never rewritten).
- `python -m rustique [--native PATTERN]... [--native-main] script.py|-m mod|-c cmd` enables
  the safe tier before the program is compiled. With `--native`, it also rewrites the main
  program, which `enable()` cannot do for a `__main__` that is already running.
- `@rs.native` rewrites a function, or the methods of a class, from source. Statements of a
  class body have already run and are not affected.
- The scoreboard's `--impls enabled` uses `rustique.enable()` as is.

## 6. Interop

**Safe tier**: the objects *are* the builtins, so `type(x) is list`, `isinstance`, json,
pickle (by value, loadable without rustique), `copy`, `repr`, `marshal`, C extensions and
PyO3 `downcast` behave exactly as on CPython (77/77, none slower, measured).

The patched callables keep their identity, `repr`, `__doc__`, `__text_signature__` and
pickling (`pickle.loads(pickle.dumps(list.sort)) is list.sort`). The mechanism:
- CPython reaches these methods through the `ml_meth` pointer of a static `PyMethodDef`,
  which every descriptor, bound method and specialized `CALL_METHOD_DESCRIPTOR_*` reads at
  call time, and `in` goes through `sq_contains`. The safe tier swaps exactly those
  pointers.
- Type dictionaries and type version tags are untouched. `list`, `tuple`, `dict` and `set`
  have fixed version tags that the specializer and the tier-2 optimiser rely on, and
  `type_modified_unlocked` asserts that static builtin types are never modified.

**Native tier**: §3.2 lists what non-opted-in code sees. Inside an opted-in module the
following deviations apply, each pinned by tests:
- `isinstance(x, list)` is false for its own literals;
- `exec`/`type()`/`eval` reject rewritten dicts as namespaces;
- type names in error messages say `List`;
- starred *assignment* targets (`a, *b = x`) stay builtin lists (built by `UNPACK_EX`);
- tuple displays are never rewritten (needed by `except`, `isinstance`, `%`,
  `__match_args__`);
- displays in unobservable positions keep CPython's code: `x in [..]`, `for x in [..]`,
  `f(*[..])`, `f(**{..})`.

## 7. Safe-tier kernel rules (deviations: none known)

- **Fast paths** apply only to exact `int`/`bool` (compact, or `i64` for sorting),
  `float`, `str` and `None`, with no key function. `sort` accepts only `key=None` and an
  exact bool or int `reverse`.
- **Sorting** is stable, so the result is the unique stable permutation CPython's timsort
  also produces. -0.0 and 0.0 are equivalent and keep their order. Float lists containing
  NaN are sorted by CPython (its order with NaN is algorithm-specific). Keys sit on the
  stack up to 128 elements.
- **Scans** (`in`, `index`, `count`) follow CPython's order: size re-read each step,
  identity first, item on the left. Exact scalars compare by value (`1 == 1.0 == True`,
  exact for compact ints). Anything else uses `PyObject_RichCompareBool`.
- **Deoptimization**: a scan that meets an element needing the generic comparison before
  any Python code ran, within its first 16 elements, restarts in the original. This is
  unobservable, because all comparisons so far were pure.
- **Error messages** are captured from the originals at install time (`list.index(x): x not
  in list`).
- **Free-threading**: `sort` holds the list's critical section like CPython's `list.sort`.
  Scans of 4 or more elements run the pure part under the critical section and leave it,
  holding a strong reference, for a generic comparison. Shorter lists take a reference per
  element. A lock-free prelude handles the empty, identity and foreign-type cases; it only
  ever compares pointers or reads an object header, under the same QSBR guarantee as
  CPython's lock-free `list_get_item_ref`.
- **Calibration**: `str` kind and data pointer are read inline through a layout calibrated
  against `PyUnicode_KIND`/`PyUnicode_DATA` at install. The int and float layouts are
  self-checked, and all-or-nothing installation verifies the `PyMethodDef` flags and the
  writability of every site.

## 8. Risks

- **Process-global pointer swaps.** They rely on CPython's static method tables and slot
  tables being writable data (checked against `/proc/self/maps` on Linux; assumed on macOS
  and Windows) and on the calling conventions (checked). The kernels also run in
  subinterpreters; they hold no per-interpreter Python objects. Call `enable()` before
  starting threads: each pointer swap is atomic, but a thread may run the original and a
  kernel within the same operation sequence.
- **Already-copied slots and cached traces.** Subclasses of `list`/`tuple` created before
  `enable()` copied the original `sq_contains` and are not accelerated (still correct).
  JIT traces that inline a C function pointer (`_CALL_METHOD_DESCRIPTOR_O_INLINE`) keep the
  original (correct, slower). `PyList_Sort()` from C keeps CPython's sort.
- **Version coupling.** A future CPython that bakes `ml_meth` into specialized
  instructions, or changes `PyLongObject`/`PyASCIIObject` layouts, disables kernels or
  makes `install()` refuse (checked) rather than miscompute.
- **Native tier.** It can never be the default. Its value depends on Rustique types
  winning on bulk operations by more than they lose per element: today 2–19× per op
  (§3.1) and 5–14× per small construct (§3.3).

## 9. What must improve elsewhere, and interface requests

- **For the default tier**: nothing is required from List/Dict/Set. Further in-place
  kernels to consider:
  - `list ==`/`<` via `tp_richcompare` (~130 Ir per element today);
  - `list.remove`;
  - `max`/`min`/`sum`/`sorted` with `key=` (workstream 5);
  - `set`/`dict` operations: no headroom, since CPython's hash-table loops already work on
    the same `PyObject*` tables (inferred).
- **Workstream 5** (`rustique._builtins`): `enable()` calls `install()`/`uninstall()`.
  Swapping `ml_meth` of the builtin functions' `PyMethodDef` (see `takeover.rs`) keeps
  `min is builtins.min`, `repr`, pickling and `CALL_BUILTIN_*` specialization, and needs no
  `builtins` dict mutation. Prototype numbers from this workstream, callgrind 3.15.1,
  before handing over:
  - `min(8 ints)` 0.27, `max(8 floats)` 0.24, `min(8 strs)` 0.48, `sum(8 ints)` 0.44,
    `sum(8 floats)` 0.38, `min(1000 ints)` 0.14, `sum(1000 ints)` 0.26;
  - fallbacks: `min(a, b)` 1.043 (the floor of one extra call layer), `max(key=f)` 1.003,
    `sum(x, 0)` 1.017, `min(tuples)` 1.025.

  Exactness traps:
  - the first minimal element wins;
  - NaN replicates `item < best`;
  - `sum` of floats is CPython's Neumaier `cs_add`/`cs_to_double`, started from `0 + x0`;
    ints accumulate while they fit `isize`.
- **Workstreams 2/3/4, for the opt-in native tier only:**
  - `List._from_builtin(list)`, `Dict._from_builtin(dict)`, `Set._from_builtin(set)`: build
    from a fresh builtin temporary. Adopting the object strategy should be O(1). Today
    `rs.List([a])` costs 6.7× the display (§3.3). The rewriter already uses these hooks
    when they exist.
  - Vectorcall constructors: `rs.List(x)` goes through `tp_call`/`tp_new`.
  - Optionally, a builtin-identical presentation for takeover-created values (repr, error
    messages, pickling to builtins), which removes the 12 presentation breaks of §3.2.
  - The conformance deviations listed in §3.7: `DictIterator` pickling, view reprs,
    `update`/`setdefault` atomicity, reflexive `deepcopy`, sort identity.
- **Lead, at integration:** add `enable`, `disable` and `native` to `rustique.__all__` and
  to the expected set in `tests/test_interop.py::test_public_api`. The names are exported
  as attributes today.
- **When `Tuple`/`FrozenSet` land:** `_rewrite.default_targets()` already maps `tuple` and
  `frozenset` to `rs.Tuple`/`rs.FrozenSet` when they exist, and the rewriter can route
  `tuple(...)`/`frozenset(...)` calls to them (tested with stand-in types). Turning that
  on is two lines in `python/rustique/_takeover/_rewrite.py`:
  - add the kinds to `DEFAULT_KINDS`;
  - bump `VERSION`, which invalidates cached rewritten bytecode.

  Tuple *displays* stay unrewritten (see §6).

## 10. Files

- `crates/rustique-py/src/takeover.rs`: the safe-tier kernels and installer, plus the
  PyO3-consumer probes.
- `python/rustique/_takeover/`: `enable`/`disable`/`status`/`native`, plus `_rewrite.py`
  (the rewriter and decorator) and `_hook.py` (the import hook).
- `python/rustique/__main__.py`: the runner.
- Tests: `tests/test_takeover_safe.py`, `test_takeover_rewrite.py`,
  `test_takeover_compat.py`, `test_takeover_conformance.py`.
- Benchmarks: `benches/takeover_overhead.py`, `benches/takeover_compat.py`.

## 11. Reproduce

```bash
.venv/bin/python benches/takeover_overhead.py -o results.json      # handicap, construct, kernels, pep523, compile, compat
.venv/bin/python benches/takeover_overhead.py handicap pep523 --jit  # under the JIT
.venv/bin/python benches/takeover_overhead.py report results.json
.venv/bin/python benches/scoreboard.py instr --impls enabled --type list,tuple --op 'in x,x.index,x.count,sort' --shards 2 -o e.json
CPYTHON_SRC=<cpython 3.15.1 checkout> .venv/bin/python tests/test_takeover_conformance.py --json conf.json
```
