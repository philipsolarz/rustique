STATUS: wave 2, opt-in tier `interp` (DECISIONS §10.2). Contract: `docs/ACCELERATION.md` with
the opcode-handler site kind (§A below, proposed text). Code: `crates/rustique-interp/` (generated
handler sets, verification, C overrides), `crates/rustique-py/src/targets/interp/` (targets).
Tests: `tests/interp/`. Benchmarks: `benches/interp/` (`cells_cg.py`, `wl_cg.py`,
`regrtest/`). Research: `docs/atlas/w2-interp.md` (prototype `w2/p1-interp`). Every number
is **M** (measured; tool named) or **I** (inferred; source named).

# The `interp` tier: opcode-handler overrides

An opt-in tier (`TargetSpec.opt_in = Some("interp")`, group `interp`): turned on with
`RUSTIQUE_TIERS=interp` (with `RUSTIQUE=1` or `rustique.enable()`) or
`_accel.enable(tiers=["interp"])`; never on by default. Sites are accel's opcode-handler kind
(`Prepare::opcode_handler`); suspensions go through `Counters::request_suspend`.

| target | handlers swapped | what is faster | validated | free-threaded |
|---|---|---|---|---|
| `interp.isinstance` | `CALL_ISINSTANCE` | negative checks, metaclass checks, tuples | 3.15.1, 3.15.2 | yes |
| `interp.attr` | the `LOAD_ATTR` family (14) | polymorphic and uncacheable attribute loads | 3.15.1, 3.15.2 | yes |
| `interp.setattr` | `CALL_NON_PY_GENERAL` | `object.__setattr__(...)` (frozen dataclasses, attrs) | 3.15.1, 3.15.2 | yes |
| `interp.kwinit` | `CALL_KW_NON_PY` | `C(a, b=1)` for plain Python classes | 3.15.1, 3.15.2 | no (refused) |

## 1. Mechanism

In a tail-calling CPython (python-build-standalone, the builds `uv` installs) every opcode
handler is a `preserve_none` function and dispatch is `musttail` through the writable static
array `instruction_funcptr_handler_table`. The tier writes chosen entries of that table:

- **Handlers are CPython's own code, regenerated.** `tools/gen.py` runs the release's own
  `Tools/cases_generator` on the release's own `Python/bytecodes.c` followed by
  `c/overrides.c`. Each override is `override op(X, ...)`: CPython's body with one helper call
  wrapped by a fast path whose fallback is the original helper with the same arguments. One
  handler set per patch release (`gen/v3_15_1/`, `gen/v3_15_2/`), checked in with the headers
  it reads and the `pyconfig.h` of the verified builds: a wheel build needs clang ≥ 19 and no
  CPython source.
- **A swapped handler runs inside the core's loop**, with the core's frame, stack and thread
  state, and dispatches onward through the table it was passed, i.e. back into the core's
  handlers. Three changes from CPython's own test copy (`Modules/_testinternalcapi/
  interpreter.c`) make it behave exactly as the core's handler: `DISPATCH_TABLE` and
  `TRACING_DISPATCH_TABLE` name the *core's* tables (re-dispatch after specialization, JIT
  trace recording), `IS_PEP523_HOOKED` keeps the core's definition, and the JIT entry and trace
  compiler are declared `__attribute__((error))`, so the build fails if any swapped handler can
  reach tier 2.
- **Unswapped opcodes cost 0**: same table, same code (cells, §4).
- **Verified binaries only.** `src/builds.rs` lists (release, configuration, ELF build-id) with
  the addresses of the dispatch table, the tracing table and every handler the tier may swap,
  recorded by `tools/record_build.py`. At install the tier finds the object holding the
  interpreter (`dl_iterate_phdr`, the mapping of `PyEval_EvalCode`), reads its build-id, and
  refuses unless the triple is recorded; it then compares every entry it may swap (and `NOP`)
  with the recorded core handler, runs the ABI self-test, and only then declares the entries as
  sites; the registry swaps them with compare-and-swap, all-or-nothing per target, and restores
  them on `disable()` and at exit. A stripped variant of a build has the same build-id and
  addresses. The handler code stays mapped for the life of the process.
- **ABI self-test.** The set calls the core's `NOP` handler with sentinel arguments and a
  private dispatch table whose entry for the next code unit is a probe of ours; the probe
  checks all six arguments, in both directions of the `preserve_none` convention (clang-20
  handlers inside a clang-23 binary).
- **Loads everywhere, swaps only where verified.** Every interpreter symbol the handlers
  reference is a weak reference (`gen/<release>/rq_weak.h`), so the extension imports on any
  CPython; the build-id check guarantees the symbols exist where the tier runs. Non-exported
  core functions are never called: under LTO their calling convention may have been changed
  (the first free-threaded build that called `_PyType_LookupStackRefAndVersion` by address
  never filled its cache; M, `tests/interp`).
- **Local copies of helpers the core inlines.** The core's handlers have
  `_Py_VectorCall_StackRefSteal`, `PyObject_Vectorcall` and `_PyObject_GetAttrStackRef`
  inlined; their exported copies are laid out as cold code (+29 to +180 Ir per call, M,
  callgrind per function). `c/rq_inline.h` holds verbatim copies (the last restricted to its
  common case), and `tools/gen.py` refuses a release whose originals differ from
  `c/overrides.lock`.
- **PGO**: `tools/pgo.sh` trains on the custom workloads and the micro cells with the tier on;
  one text profile per configuration serves every release (`gen/pgo-{gil,t}.proftext`).

## 2. The overrides (exact subset; everything else is CPython's code)

### 2.1 `CALL_ISINSTANCE` (`c/rq_isinstance.h`)

Replays `object_recursive_isinstance`/`object_isinstance` (`Objects/abstract.c`):
- `type(inst) is cls` → True (CPython's first test).
- `cls` a type whose metaclass is `type` or resolves `__instancecheck__` to `type`'s own
  method: CPython binds and calls it, and it computes `object_isinstance(inst, cls)` with no
  Python frame in between; the fast path computes that directly.
- `object_isinstance`: subtype check; a negative result looks up `inst.__class__`, skipped
  when `type(inst)` uses `PyObject_GenericGetAttr` and its MRO resolves `__class__` to
  `object`'s data descriptor (its getter returns `type(inst)`, no side effect, an instance dict
  cannot shadow it); otherwise the lookup runs exactly as CPython's.
- an exact tuple: items in order; non-type items go to `PyObject_IsInstance` (with its own
  recursion guard), so the fast path never recurses.
- **Falls back** (original `PyObject_IsInstance`): unions, tuple subclasses, ABCs and every
  user `__instancecheck__`, other interpreters.
- Differs only in the C-stack recursion guard of the original (outside the contract, §5.8).
- Caches: the two per-type facts by `tp_version_tag`, one 64-bit word per entry
  (`version << 8 | fact`), stored only when the tag read before and after computing the fact is
  the same non-zero tag.

### 2.2 The attribute cache (`c/rq_attrcache.h`, `LOAD_ATTR` family)

Keyed by (type version tag, name); a hit replays `_PyObject_GenericGetAttrWithDict`
(attribute loads) and `_PyObject_GetMethodStackRef` (method loads) in their order: the cached
MRO lookup; data descriptors run the generic code except a plain `__slots__` member
(`Py_T_OBJECT_EX`, no audit, read as `PyMember_GetOne`; NULL runs the generic code, which
raises); the instance attribute (inline values or an exact, unicode-keyed instance dict); a
method descriptor (`(descr, self)` or its `tp_descr_get`) or a plain class attribute.
**Falls back**: absent names (AttributeError and its suggestions), other descriptors when the
instance lacks the name, `__getattr__`/`__getattribute__`, class and module owners, dicts to be
materialized, non-immortal names, version tag 0, other interpreters. Uncacheable resolutions
are cached as negative entries, and a site that is generic in the bytecode records a negative
hint (type version) in its own unused inline-cache entries (private state: the specializer
overwrites them, `co_code` reports caches as zero).

The whole family is swapped: a specialized site that deopts jumps directly (not through the
table) to the generic handler of the same file, so only a family swap reaches polymorphic
sites. Chosen by whole-run instructions (M, prototype `_rqeval_acC` generic-only vs `_acD`
family): fastapi_sqlalchemy 0.9935 vs 0.979, flask_marshmallow 0.970 vs 0.959,
strawberry_graphql 1.0016 vs 0.985, falcon_api 0.998 vs 0.996.

A class attribute's kind can change without the owner's tag changing (its own mutable type
gains `__get__`/`__set__`): entries also hold `type(descr)` and its version tag.

### 2.3 `object.__setattr__` (`c/rq_calls.h`, `CALL_NON_PY_GENERAL`)

For exactly the descriptor `object.__dict__['__setattr__']`, three positional arguments and
`self` not a type, calls its `d_wrapped` (`PyObject_GenericSetAttr`) directly: CPython builds a
3-tuple, slices and unpacks it, runs `hackcheck` (a no-op unless `self` is a type) and calls
the same function. Everything else: the original call. No cache.

### 2.4 Keyword instantiation (`c/rq_calls.h`, `CALL_KW_NON_PY`)

`C(a, b=1)` runs `type_call → object_new → slot_tp_init →` a C-level call of `__init__`,
while CPython pushes `C(a, b)` inline (`CALL_ALLOC_AND_ENTER_INIT`). For exactly the classes the
specializer accepts for that form (metaclass `type`, heap type, not abstract, `tp_new` =
`object_new`, `tp_alloc` = `PyType_GenericAlloc`, `__init__` a Python function with the default
vectorcall, no PEP 523 hook, stack and recursion headroom) the override does what
`CALL_ALLOC_AND_ENTER_INIT` does and passes the keywords as `CALL_KW_PY` does: same object,
same argument binding and errors, the same `__init__() should return None` check, the same
Python frames. Only the C stack differs (outside the contract, §5.8). GIL builds only: its
cache holds `__init__` borrowed under the class's version tag, as the specializer's own init
cache does; the free-threaded build refuses it (category `build`).

### 2.5 Not shipped: `getattr` / `hasattr` (`targets/interp/getattr.rs`, `rq_getattr_fast`)

Built and measured, not registered: `ml_meth` kernels (the existing site kind: fallback costs a few Ir) on `builtins.getattr` and
`builtins.hasattr` that resolve through the attribute cache. `getattr(o, name)` is
`PyObject_GetAttr`, `getattr(o, name, default)` and `hasattr(o, name)` are
`PyObject_GetOptionalAttr`; for a type with `PyObject_GenericGetAttr` both are
`_PyObject_GenericGetAttrWithDict` (suppress 0 or 1), whose resolution a hit replays. An
attribute absent from the MRO and the instance is a hit only for the suppressing forms
(CPython returns the default, or False, without creating the AttributeError). Everything
else (other argument counts, non-str or non-immortal names, hooks, data descriptors, class
owners, misses) calls the original with the unchanged arguments. Differential tests passed
(Hypothesis, 200 examples, GIL and t), but the cells fail the never-slower rule (M, callgrind):
hits gain little because `builtin_getattr` is already cheap around the lookup (`getattr(o, 'x')`
0.962, polymorphic 0.970, `hasattr` 0.973), while fallbacks pay the probe plus a second
dispatch (data descriptor 1.126, `__getattr__` 1.142, class owner 1.355). Next step if wanted:
reject non-generic owners and negative entries before the probe in the kernel itself.

### 2.6 Net-benefit suspension (ACCELERATION §5.6 per override group)

Each group counts executions of its overridden path and fast-path hits (attribute-cache
fills excluded) and decides twice: **early**, after the first 65,536 executions, when its hits
are below a quarter of break-even (its cost is visible already); **final**, after 16 windows
(~1M executions), at break-even, a horizon long enough to average out start-up phases for
borderline groups, with every cache fill debited 16 times (fills that keep coming — types
created or modified per run, conflicting entries — are a cost, not a warm-up). Break-even
ratios: attribute family 5, `object.__setattr__` 64, keyword instantiation 64, `isinstance` 4.
`isinstance` counts only paths that save or cost: negative and metaclass answers are credited,
the slow `__class__` lookup and fallbacks (ABCs, hooks, unions) debited; exact and plain
positive answers are not counted (no counter on the cheapest paths). A
suspension asks the registry (`Counters::request_suspend`), which restores the core's handlers
for the process and reports the target `suspended`; a later `enable()` reinstalls it and the
group counts afresh. The early decision fixed the T1 misses of the first measurement
(task_planner 1.0008, email_mime 1.0023, billing_engine 1.0013, pandas_etl 1.0016 with the
tier alone): groups that never hit there (no frozen dataclasses, few keyword
instantiations) kept their handler copies until ~1M executions. `_accel._t_interp.info()["groups"]` shows executions, hits, fills and
`suspended`; the registry records the suspension in `status()` once accel's merge B handles
every site kind. `rustique.stats()` shows no per-call counts for these targets (the handlers
count in C, always on, for the suspension rule): reporting tools read
`_accel._t_interp.info()`.

## 3. Free-threading, subinterpreters, tools, the JIT

- **Free-threaded builds (3.15.1t):** the table is in `.data`; entries are aligned pointers
  swapped with atomic CAS, so a dispatching thread sees the old or the new handler, both
  correct. Group counters use relaxed atomic loads and stores (lost updates only). Attribute
  entries are seqlocked and hold only integers and identity-compared pointers; a hit that
  needs the class attribute gets a strong reference from the exported `_PyType_LookupRef`
  bracketed by two acquire loads of the type's tag (a class mutation resets the tag before it
  changes the type's dict, `type_update_dict`); instance values and slots are read with
  `_Py_TryIncrefCompare`/`_Py_TryXGetRef` (CPython's lock-free reads), and any contention runs
  the generic code. `interp.kwinit` is refused.
- **Subinterpreters:** the table is process-global, so swapped handlers run in every
  interpreter; every fast path first checks `tstate->interp == main` (free: `tstate` is a
  handler argument) and runs CPython's code elsewhere.
- **sys.monitoring, settrace, setprofile, coverage, debuggers:** instrumented code executes
  `INSTRUMENTED_*` opcodes, which are never swapped; tools see CPython's handlers
  (`tests/interp/test_interp_mechanics.py::test_tools_see_cpython_handlers`). PEP 523 users
  (`-X perf`, pydevd) leave the core's loop in place, so the overrides keep working.
- **JIT (`PYTHON_JIT=1`):** tier 1 dispatches through the table, so overrides run there;
  traces use their own micro-ops (correct, no gain). Never swapped: `ENTER_EXECUTOR`,
  `JUMP_BACKWARD*`, `RESUME_CHECK_JIT`, `TRACE_RECORD`, labels. With the core's tracing table
  named by `TRACING_DISPATCH_TABLE`, trace recording sees our handlers as the core's
  (`test_jit_process`). Decision: allowed (`jit_safe`), evidence §5.
- **Native tools** (perf, gdb, py-spy native): swapped handlers appear as
  `_TAIL_CALL_<op>` of the Rustique extension; `_PyEval_EvalFrameDefault` stays on the stack.

### 3.1 Caches beside accel's ClassCache

accel's ClassCache (merge C) is the shared FT-safe per-class cache for Rust kernels. The
interp caches are separate because they are probed inside generated C handlers on every
generic `LOAD_ATTR`/`isinstance` execution, where a cross-language call (and the register
saves a `preserve_none` caller does not do) would cost ~15–25 Ir, the whole gain of a
polymorphic site (lead's ruling, 2026-11-02).

| | interp attribute cache | interp isinstance facts | interp keyword-init cache | ClassCache (accel) |
|---|---|---|---|---|
| key | (type `tp_version_tag`, interned immortal name) | type `tp_version_tag` | class `tp_version_tag` | type version tag |
| holds | kind, dict kind, shared-keys index; descriptor borrowed (GIL) or identity only (t); `type(descr)` + its tag | one bit | `__init__` borrowed (GIL only) | per accel |
| invalidation | any change to the type or a base changes the tag (CPython's method-cache rule; tags never reused); the descriptor's type tag is re-checked on every hit; entries for dead tags are never matched | same | same, and the vectorcall is re-checked on every hit | same rule |
| free-threading | seqlocked entries; references through `_PyType_LookupRef` between two acquire loads of the tag, `_Py_TryIncrefCompare` | one 64-bit word per entry | refused on t | FT-safe by construction (accel) |
| interpreters | main only (`tstate->interp`) | main only | main only | per accel |

## 4. Measurements

### 4.1 Cells (M, `benches/interp/cells_cg.py`, callgrind Ir per operation, fresh process per
cell, 3.15.1, PGO build; "per 4": a loop over 4 objects of 3 types)

| override | cell | plain | tier | ratio |
|---|---|---:|---:|---:|
| isinstance | exact type | 178 | 151 | 0.848 |
| | subclass, positive | 271 | 266 | 0.981 |
| | **negative, plain class** | 354 | 219 | **0.619** |
| | negative, int instance | 358 | 221 | 0.618 |
| | **metaclass, negative** | 727 | 236 | **0.325** |
| | tuple `(Other, A)` | 630 | 495 | 0.786 |
| | `__getattr__` class, negative (fallback) | 529 | 541 | 1.022 |
| | ABCMeta negative (fallback) | 1665 | 1710 | 1.027 |
| | union `int \| str` (fallback) | 3171 | 3217 | 1.014 |
| attribute cache | **polymorphic `o.x`** (per 4) | 1068 | 888 | **0.831** |
| | polymorphic `o.m()` (per 4) | 2316 | 2055 | 0.887 |
| | **polymorphic inherited `__slots__`** (per 4) | 1114 | 839 | **0.754** |
| | materialized `__dict__` attribute | 225 | 159 | 0.706 |
| | `cached_property`, cached | 238 | 167 | 0.701 |
| | specialized `a.x` (our family copy) | 82 | 79 | 0.963 |
| | specialized `a.m()` | 310 | 306 | 0.987 |
| | specialized class attribute | 78 | 78 | 1.000 |
| | specialized module attribute `math.pi` | 93 | 89 | 0.957 |
| | class attribute on a class `C.x` | 78 | 75 | 0.962 |
| | Python data descriptor (fallback, negative hint) | 668 | 686 | 1.027 |
| | `__getattr__` hook (fallback) | 737 | 754 | 1.023 |
| | AttributeError raised (fallback) | 4246 | 4319 | 1.017 |
| `object.__setattr__` | **frozen dataclass `F(1, 'x', 2.0)`** | 3826 | 2136 | **0.558** |
| | `object.__setattr__(o, 'x', 1)` | 954 | 399 | 0.418 |
| | `object.__setattr__(type, ...)` (fallback, raises) | 3786 | 3886 | 1.026 |
| | `dict.__setitem__` (other wrapper, fallback) | 905 | 935 | 1.033 |
| | `str.join` via descriptor (fallback) | 814 | 811 | 0.996 |
| keyword instantiation | **`P(1, b=2)`** | 2413 | 1253 | **0.519** |
| | `P(1)` (`CALL_ALLOC_AND_ENTER_INIT`, not ours) | 1068 | 1054 | 0.987 |
| | `dict(a=1)` (builtin class, fallback) | 748 | 781 | 1.044 |
| | `f(1, b=2)` (`CALL_KW_PY`, not ours) | 456 | 456 | 1.000 |
| none | `ident(1)`, `len(l3)`, `sum(genexpr)` (unswapped opcodes) | 270 / 156 / 31603 | same | 1.000 |

Every cell ≤ 1.05 (worst: `dict(a=1)` 1.044, a keyword call of a builtin class through the
swapped `CALL_KW_NON_PY`). Two fallbacks exceeded 1.05 in the first product build (Python data
descriptor 1.073, `dict.__setitem__` 1.056): the exported copies of `_PyObject_GetAttrStackRef`
and `_Py_VectorCall_StackRefSteal` that a swapped handler calls are laid out as cold code in the
PBS binary (the core inlines them); `c/rq_inline.h` fixed both.

WHOLE


## 5. Conformance and gates

### 5.1 CPython's full regression suite (M, `benches/interp/regrtest/`, `-j8`)

Every Python process of the run has the variant applied at startup through a `.pth` file (also
under `-I`; only `-S` processes run plain); `noop` imports Rustique and verifies the binary
without swapping anything (the harness's own footprint), `tier` also enables the tier.
`cmp_regrtest.py` compares failed files and per-test outcomes (`--junit-xml`).

| build | run | tests | failures | skipped | failed files | processes with the variant | tier vs noop |
|---|---|---:|---:|---:|---:|---:|---|
| 3.15.1 | plain | 47,182 | 60 | 3,000 | 20 | – | – |
| 3.15.1 | noop | 47,182 | 64 | 3,000 | 23 | 3,806 | – |
| 3.15.1 | tier | 47,182 | 64 | 3,000 | 23 | 3,804 | **0 differences** (files and 47,041 per-test outcomes) |
| 3.15.1t | noop | 47,230 | 64 | 2,968 | 23 | 3,804 | – |
| 3.15.1t | tier | 47,230 | 64 | 2,968 | 23 | 3,812 | **0 differences** (files and 47,091 per-test outcomes) |
| 3.15.2 | noop / tier | 47,274 | 65 / 65 | 2,998 / 3,000 | 23 / 23 | 3,813 / 3,817 | 0 file differences; per-test: `test_os` `test_closerange` ×2 skipped with the tier (it skips when no range of free file descriptors is available; on 3.15.2t it went the other way) |
| 3.15.2t | noop / tier | 47,323 | 66 / 65 | 2,968 / 2,966 | 24 / 23 | 3,819 / 3,812 | 0 failing only with the tier; per-test: the same two `test_closerange` (skipped in noop) and `test_multiprocessing_fork` `test_put` (failed in noop) |
| 3.15.1, `PYTHON_JIT=1` | 20 files (isinstance, descr, class, dataclasses, functools, inspect, opcache, monitoring, sys_settrace, generators, coroutines, typing, enum, abc, property, super, types, unittest, json, ast) | 5,551 | 49 | 71 | 3 | 123 | **0 differences** (5,510 outcomes) |

Final build (`0d73921`, wave2 with this tier): 3.15.1t tier vs noop 0 differences (47,091
outcomes, 3,803 processes); 3.15.1: 2 differences, `test_os` `TimerfdTests.test_timerfd_interval`
and `test_timerfd_select` failed with the tier at load average ~60 on the shared box; both pass
with the tier 3 times out of 3 when rerun (and with the harness alone): timer-interval tests
under load, not the tier.

The plain failures are environmental (python-build-standalone ships no `_testcapi`, no `Lib/test`
of its own, no `idlelib.idle_test`; perf; `-I` subprocesses); the noop harness adds three
(startup-imported modules, the faulthandler module list, child stderr). The same suite also
passed with `interp.getattr`/`interp.hasattr` registered (0 differences, both builds). The
prototype's suite found three real bugs (nested-tuple recursion, a descriptor's type mutated
after caching, subinterpreters reaching process-global handlers), all fixed there and covered
here by the differential and mechanism tests.

### 5.2 Rustique's tests

- `tests/interp/` (20 tests; skip with the tier's own reason where it refuses): mechanics
  (verification, install all-or-nothing, `disable()` restores every entry, refusal reasons,
  opt-in only, tools see CPython's handlers for settrace / setprofile / sys.monitoring,
  subinterpreters, a `PYTHON_JIT=1` process, net-benefit suspension), Hypothesis differential
  suites (attribute resolution and isinstance over generated class hierarchies with descriptors
  of every kind, `__slots__`, `__getattr__`, `__getattribute__`, metaclasses with and without
  `__instancecheck__`, a `__class__` property, materialized / replaced / non-str-keyed
  `__dict__`s, and class, descriptor-type and instance mutations between rounds, 300 examples;
  `object.__setattr__` shapes; keyword instantiation shapes incl. errors, frames and
  RecursionError), races (table writes while 4 threads run the swapped handlers; class mutation
  racing the cache).
- Passing on 3.15.1, 3.15.1t, 3.15.2, 3.15.2t (tier verified), 3.14.9, 3.14.9t (tier refused,
  tests skipped). Full suite (`pytest -q tests`): 3.15.1 2199 passed / 5 skipped, 3.15.1t 2199 / 5,
  3.14.9 2183 / 20, 3.14.9t 2183 / 21 (M, dev box).

## 6. Reach per distribution

Tail-calling is opt-in in 3.15 (`--with-tail-call-interp`); the tier needs it and a recorded
build-id. Everything else refuses with its reason in `status()` and
`_accel._t_interp.info()`.

| distribution | interpreter loop | tier |
|---|---|---|
| python-build-standalone x86_64 Linux 3.15.1 / 3.15.1t / 3.15.2 / 3.15.2t (uv, rye, pdm, hatch, mise) | clang, tail-calling | **yes** (recorded; M) |
| PBS microarchitecture variants (x86_64_v2/v3/v4), musl | tail-calling | refused until recorded (`build`) |
| PBS aarch64 Linux, macOS | tail-calling | refused (`platform`): ABI self-test and `__DATA` writability to validate |
| PBS Windows | MSVC `__preserve_none` | refused (`platform`) |
| Debian/Ubuntu/Fedora, Docker official images, pyenv, Homebrew, conda-forge, actions/setup-python | computed goto | refused (`build`: unverified binary) |
| CPython 3.14.x, 3.15.0, unreleased 3.15.x | – | refused (`version`) |

## 7. Maintenance per patch release

1. `tools/gen.py --src <tag> --pbs-gil <PBS> --pbs-t <PBS t> [--base <previous>]` regenerates
   the set; it stops when an overridden op or a copied helper changed (`c/overrides.lock`):
   re-prove that override, then `--update-lock`.
2. `tools/record_build.py --write` for each PBS build of the release (GIL and t, each
   platform): build-id, table and handler addresses, checked in the running process.
3. CI: build, `tests/interp`, the full regression suite with the tier vs the harness without it
   (`benches/interp/regrtest/`), then commit the records. Typical cost when nothing an override
   touches changed: zero manual work.

## 8. Private state

The attribute cache, the isinstance facts and the keyword-instantiation cache are private
(never observable through a documented API). The per-site negative hint lives in the unused
inline-cache entries of generic `LOAD_ATTR` instructions: visible only through the private
`code._co_code_adaptive`; the specializer overwrites them when it specializes the site.

## Appendix A: proposed contract text (ACCELERATION §1/§3/§4/§5/§6)

**§1 Site — a fourth kind, the opcode handler** (kind D): one entry of the core interpreter's
tail-call dispatch table (`instruction_funcptr_handler_table`), replaced by a handler generated
from the running patch release's own `Python/bytecodes.c` by that release's cases generator
with a Rustique override file. Only in tail-calling builds whose (release, configuration, ELF
build-id) is recorded (python-build-standalone). Handlers are process-global (every
interpreter dispatches through the table).

**§3 Install** (additions):
- (a) A target of kind D declares table entries only on a recorded build: the release is
  exactly `Py_Version`, the configuration matches, and the build-id of the object holding the
  interpreter (found through the mapping of `PyEval_EvalCode`) is in the target's verified list,
  which gives the table and per-handler addresses. Every entry the target may swap is compared
  with the recorded core handler before the swap; any mismatch refuses (`conflict`); anything
  else unrecorded refuses (`version`, `build`, `platform`).
- (b) An install-time ABI self-test proves the calling convention in both directions (the
  core's `NOP` handler tail-calls a probe through a private table) before the first swap.
- (c) Never swapped: `INSTRUMENTED_*` opcodes, `ENTER_EXECUTOR`, `JUMP_BACKWARD*`,
  `RESUME_CHECK_JIT`, `TRACE_RECORD`, labels. A build-time check proves no swapped handler can
  reach tier 2 (`__attribute__((error))` on the JIT entry and the trace compiler).
- (d) A generated handler names the core's dispatch and tracing tables (`DISPATCH_TABLE`,
  `TRACING_DISPATCH_TABLE`) and keeps the core's `IS_PEP523_HOOKED`, so it re-dispatches and
  traces exactly as the core's own handler.
- (e) An override that must see the deopt traffic of specialized forms swaps the whole family
  (deopts jump directly to the generic handler of the same file).
- (f) Swaps are CAS on aligned pointers; `disable()` and exit restore the recorded originals;
  handler code stays mapped for the life of the process.
- (g) The extension must load on every CPython: every interpreter symbol a handler set
  references is weak; non-exported functions are never called (their calling convention may
  have been changed by LTO).
- (h) One handler set per patch release, generated from that tag; overridden ops and copied
  core helpers are pinned by hash (`overrides.lock`) and generation stops when CPython changed
  one of them.

**§4 Guard** (additions): an override decides its exact subset from the state CPython's own
code reads (type version tags, descriptor kinds, dict kinds, the callable's identity), before
anything observable. Caches are keyed by version tags (CPython's method-cache rule) and hold
only per-main-interpreter facts; the interpreter comes from the handler's `tstate`
(`tstate->interp == main`, else CPython's code). On free-threaded builds cache entries are
single words or seqlocked, hold no dereferenced borrowed pointers, and obtain references with
the primitives CPython's own generic path uses.

**§5 Fallback** (additions):
- Outside the fast path an override runs CPython's original op body, verbatim from
  `bytecodes.c` (with core helpers the core inlines copied verbatim); it never re-dispatches.
- Never slower: every cell ≤ 1.05, including each override's fallbacks and the specialized
  forms of a swapped family; whole-run instructions ≤ 1.00 on every workload of the reference
  set.
- §5.5 (ruling 15) applies unchanged: an override calls user code only where CPython's C code
  does, with no Python frame in between (a non-data descriptor's `tp_descr_get`,
  `PyObject_GenericSetAttr`).
- §5.5 extension (frame pushes): an override may push a Python frame inline instead of calling
  through C only as an existing CPython specialized form pushes it for the same kind of callee
  (keyword instantiation = `CALL_ALLOC_AND_ENTER_INIT` + `CALL_KW_PY`); only the C stack
  differs (§5.8).
- §5.6 per override group: executions debit, fast-path hits credit (cache fills excluded); one
  decision after ~1M executions; below the group's ratio its original handlers are restored for
  the process and `status()` reports `suspended`.

**§6 Report**: sites are named `opcode <NAME>` (kind `opcode-handler`); per group executions,
hits, cache fills and suspension (`_accel._t_interp.info()`).

**§7 Definition of done** for kind D: CPython's full regression suite with the tier on in every
process equals the same harness without it (per-test outcomes) on each verified build;
Hypothesis differential suites for the overridden semantics; race tests (table writes under
threads, enable/disable with frames inside the handlers); sys.monitoring/settrace untouched.
