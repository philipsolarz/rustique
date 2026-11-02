STATUS: Phase 1 research, wave 2, slice `w2-interp` (2026-11-02). Branch `w2/p1-interp`.
Prototypes and benches: `benches/interp/` (tools), `benches/interp/rqeval/` (eval-loop copy and
handler swap). Raw data: `~/rustique-research/w2/interp/`. Every number is **M** (measured; tool
named) or **I** (inferred; source named). Builds: python-build-standalone 3.15.1 / 3.15.1t
(PGO+LTO+BOLT, clang 23, tail-calling interpreter, `--enable-experimental-jit=yes-off`).

# Reaching the interpreter layer

## 0. The answer

1. **The ~46% "out of reach" interpreter layer splits into a floor and a reachable part.**
   - Handler self time is 43–52% of cycles: dispatch plus the specialized work inlined in the
     opcode handlers (frame push/pop, attribute loads, locals, compares). Nothing behind an
     entry point can take it. It is the floor (§2).
   - The rest is the generic paths that end in C. They are slow precisely because the
     specializer gave up:
     - polymorphic or uncacheable `LOAD_ATTR`: 5.6% of frameworks (7.9% in w2-attribution's 60);
     - `isinstance` against metaclass or negative cases;
     - `object.__setattr__` in frozen dataclasses (2.2% of custom);
     - C→Python call machinery: 3–4%;
     - `getattr`/`hasattr`.
2. **A new mechanism, M2: opcode handler swap.** It reaches those paths and costs nothing
   elsewhere (§3.2, **M**).
   - In tail-calling builds every opcode handler is a function, and the dispatch table
     (`instruction_funcptr_handler_table`) is a writable static array.
   - Rustique generates its own handlers from the *same* `Python/bytecodes.c` with CPython's
     cases generator plus an override file. It writes chosen entries into the core's table.
   - Unswapped opcodes keep CPython's handlers: cost exactly 0. Overrides fall back to
     CPython's own code for everything outside their fast path.
   - Measured on PBS 3.15.1 and 3.15.1t: 229/229 handlers swap correctly, and threads race
     table writes with 0 errors.
3. **Four exact overrides built and measured** (§4). Cells are Ir ratios; whole runs are exact
   instruction counts of one steady-state run (callgrind):

   | override | fast cases (cells) | fallbacks |
   |---|---|---|
   | `isinstance` | negative 0.59, metaclass 0.30–0.36, tuples 0.79 | ≤ 1.04 |
   | megamorphic attribute cache (`LOAD_ATTR` family) | polymorphic `o.x` 0.78–0.81, inherited `__slots__` 0.68–0.71, materialized `__dict__` 0.60–0.67, `cached_property` 0.57–0.66 | specialized forms 0.96–1.00; AttributeError path 1.03 |
   | `object.__setattr__` (frozen dataclasses) | `F(1, 'x', 2.0)` 0.545, the call 0.40 | ≤ 1.04 |
   | keyword instantiation `C(a, b=1)` | 0.50 | 1.00 |

   Whole runs (§4.4), final build `_rqeval_acL`:
   - frameworks geomean 0.980, custom 0.948, sqlglot 0.876, django_blog 0.990;
   - geomean of 18 workloads 0.957; worst task_planner 1.0005 (per-group net-benefit
     suspension).

   The full regression suite (549 files, every process with the variant installed) found
   three real override bugs, all fixed: nested-tuple recursion, a
   descriptor's type mutated after caching, and subinterpreters reaching process-global
   handlers. It found no difference caused by the mechanism itself, even with all 229
   handlers swapped (§5.1).
4. **Steelmanned rejected approaches** (§3):
   - **PEP 523 is no longer 1.56×.** 3.15's
     `_PyInterpreterState_SetEvalFrameAllowSpecialization` plus a generated copy of the loop
     (M1) keeps every inlined call.
   - **M1's floor is the copy's compile quality.** PBS's own test copy costs 1.15–1.19 per
     call. Ours, with PGO and local helper copies, costs 0.98–0.99 per call, but 1.02–1.04 on
     whole workloads. It lacks BOLT and the core's LTO inlining.
   - **CPython's JIT** makes 5 of these 6 workloads slower (cycles 1.03–1.35); not a policy
     candidate.
   - **A vectorcall "Rust JIT"** for user functions stays out of reach.
5. **Recommendation** (§9): an opt-in `interp` tier in wave 2 on verified PBS/uv tail-calling
   builds: M2 with the four exact overrides and per-group suspension. Default only after the
   gates below pass on every verified build. M1 stays research.
   - Gates: full regrtest equal to the noop harness (already true for the final builds),
     Hypothesis differential suites, cells ≤ 1.05, whole-run ≤ 1.00.
   - Reach is the limit: tail-calling is opt-in in 3.15. Distro, Docker-official, pyenv and
     Homebrew builds use the computed-goto loop, where M2 is impossible.
   - Cost: one generated build per CPython patch release.
6. **Methodology finding.** `-X perf` installs a PEP 523 hook (`perf_trampoline.c:552`). That
   turns off every inlined Python-to-Python call: up to 1.27× on web workloads, 1.43× on
   config_merge. Interpreter shares from `-X perf` profiles overstate call costs. Relayed to
   w2-attribution, which now samples without it.

## 1. Method and tools (all in `benches/interp/`)

| tool | what it measures |
|---|---|
| `opprof.py record/show/refold` | Per-opcode native profile with plain `perf record -e cycles:u -g` (PBS keeps frame pointers), no `-X perf`. Each sample goes to the nearest `_TAIL_CALL_<op>` handler. A leaf in the handler counts as handler self; a leaf below it is attributed to the "deep entry" (first callee past generic call plumbing), flagged when the chain re-enters the eval loop (C→Python call machinery). |
| `opsummary.py` | Buckets per handler group and per deep-entry kind (table §2). |
| `spec_census.py` | Adaptive-bytecode census: specialized vs generic sites, backoff counters. `--diff` compares plain vs `rustique.enable()`. |
| `attrprobe.py` + `rqeval/rq_probe.h` | Analysis override: counts every execution of the generic `LOAD_ATTR` path by (type, name, method-load) with instance-dict facts, then resolves each statically (§2.3). |
| `hook_cg.py`, `isinstance_cg.py`, `attr_cg.py`, `setattr_cg.py`, `callpaths_cg.py`, `cg_annotate_cell.py` | Callgrind cells (Ir per operation, region-instrumented, `benches/targets/_callgrind.py`). |
| `varstat.py` | `perf stat` instructions/cycles per steady-state `run()` under variants. Cycles are unreliable on the shared box (load 10–50). |
| `wl_cg.py` / `run_wlcg.sh` | Exact callgrind Ir of one steady-state `run()` under variants. |
| `rqeval/` | The copy and swap prototype: `build.sh`, `build_pgo.sh`, overrides, `rqpatch.py`, `swaptest.py`, `ft_race.py`, `run_tests.sh` (full regrtest with a variant installed in every process). |

Workloads: the six wave-1/2 representatives (django_blog, flask_jinja, pylint_stdlib,
sqlglot_transpile, custom/order_domain, frameworks/drf_api), plus the frameworks family (5 of 6;
strawberry_graphql did not finish) and the custom family (10). w2-attribution's 60-workload
handler tables (`~/rustique-research/w2/attribution/share/handlers-60.md`) cross-check.

## 2. Where interpreter time goes (M, `opprof.py`, plain perf, 3.15.1)

### 2.1 By handler group and by what handlers call (mean % of cycles)

| bucket | 6 representative | custom (10) | frameworks (5) | reachable? |
|---|---:|---:|---:|---|
| **handler self, total** | **44.6** | **52.3** | **43.2** | floor (dispatch + inlined specialized work) |
| · frame push/pop + inlined calls | 9.9 | 11.4 | 9.7 | floor |
| · attribute (specialized + generic handler self) | 8.6 | 11.7 | 8.6 | floor, except generic misses (§2.3) |
| · locals/stack/consts | 6.3 | 7.3 | 6.4 | floor |
| · call dispatch to C | 6.3 | 6.9 | 4.3 | floor |
| · compare/branch/bool, globals, arith, build, iteration | 12.8 | 15.6 | 13.4 | floor |
| below: generic attribute/name lookup | 5.8 | 3.3 | **7.7** | **M2: attribute cache** |
| below: C→Python call machinery (nested eval entry/exit, frame init) | 3.4 | 2.9 | **4.1** | M2: inline frame pushes (§4.5) |
| below: instantiation (`type_call`, builtin type calls) | 2.0 | 1.8 | 2.7 | M2 (kwargs `__init__`, §4.5) |
| below: bound methods / partial / slot wrappers | 1.6 | 2.5 | 2.1 | M2: `object.__setattr__` (§4.3) |
| below: isinstance/issubclass generic | 2.5 | 0.8 | 1.1 | **M2: isinstance (§4.1)** |
| below: builtin functions (`getattr`, `hasattr`, `sorted`, …) | 4.1 | 3.8 | 2.2 | `ml_meth` (exists) + shared attribute cache |
| below: user dunders via `slot_*` | 0.4 | 1.1 | 0.4 | M2 (inline frame push), small |
| below: dealloc / alloc / cyclic GC | 16.9 | 10.1 | 8.6 | runtime slice |
| below: other C (sqlite, `_json`, `_sre`, extensions) | 8.7 | 9.5 | 17.6 | entry points (other slices) |
| below: dict, str, list, number protocol, formatting | 6.8 | 10.4 | 7.2 | entry points / specialized |

Exceptions are small in every family: handler self plus `_PyErr_*`/traceback/unwind is
0.5–0.8%. Generators: `RETURN_GENERATOR`/`_Py_MakeCoro` plus `gen_dealloc` is 0.3–0.7%;
FOR_ITER_GEN and SEND_GEN are already inlined.

### 2.2 Top generic paths into C (handler → deep entry, mean % / max %)

| path | frameworks | custom | 60 (attribution) | what reaches it |
|---|---|---|---|---|
| `LOAD_ATTR` → `PyObject_GenericGetAttr` | 3.96 / 6.98 | 2.05 / 5.61 | 1.65 | attribute cache (M2) |
| `LOAD_ATTR` → `_PyObject_GetMethodStackRef` | 1.60 / 2.32 | 0.37 / 1.92 | 0.62 | attribute cache (M2) |
| `STORE_ATTR` → `PyObject_GenericSetAttr` | 1.35 / 2.45 | 0.38 / 1.62 | 0.40 | store-side cache (not built) |
| `CALL_ISINSTANCE` → `object_recursive_isinstance` | 0.92 / 1.36 | 0.74 / 1.73 | 1.09 | isinstance override (M2) |
| `CALL_BUILTIN_FAST` → `builtin_getattr` / `builtin_hasattr` | 0.99 / 0.62 | 1.98 (16.0 max) / 0.28 | 0.91 / 0.27 | `ml_meth` kernel + shared cache |
| `CALL_NON_PY_GENERAL` → `wrapperdescr_call` (`object.__setattr__`) | – | 2.24 / 5.87 | (custom 2.0) | setattr override (M2) |
| `CALL_NON_PY_GENERAL` / `CALL_KW_NON_PY` / `CALL_EX_NON_PY_GENERAL` → `type_call` | 1.04 + 0.47 + 0.34 C→Py + 0.32 | 0.93 + 0.38 + 0.21 | – | kwargs-`__init__` inline (proposed) |
| `CALL_EX_NON_PY_GENERAL` → `method_vectorcall` [C→Py] | 0.82 | – | – | bound-method `f(*a, **k)` inline (proposed) |
| `CALL_NON_PY_GENERAL` → `partial_vectorcall` | 0.87 / 4.04 | – | – | partial inline (proposed) |
| `COMPARE_OP` → `do_richcompare`; `BINARY_OP` → `PyNumber_*` | 0.24; 0.55 | 0.77; 1.55 | 0.40; 0.65 | mostly Decimal (`_decimal` slots) or user dunders: little dispatch to save |

### 2.3 Why `LOAD_ATTR` stays generic (M, `attrprobe.py`, 12 workloads, share of generic executions)

| resolution of the attribute at the generic site | mean | where it dominates |
|---|---:|---|
| data descriptor: `__slots__` members at polymorphic sites (one inherited slot, many subclasses), `getset` (`function.__doc__`/`__module__` read by `functools.wraps`), Python descriptor classes | 33.2% | sqlglot 66% (`Expression.args`), fulfilment_sim 71%, task_planner 99% (a field-descriptor class), event_ledger 67% |
| instance attribute, no class attribute (polymorphic site: `field.attname` over ForeignKey/CharField/…) | 24.2% | rules_engine 60%, orm_mapper 45%, django_admin 41% |
| method load, class function, not in the instance (polymorphic `node.render()`) | 18.1% | django 25%, falcon 29% |
| plain class attribute, not in the instance | 7.7% | falcon 36% |
| owner is a class object (metaclass: `Model._meta`, `cls.x`) | 4.9% | django_blog 3% |
| instance attribute shadowing a non-data descriptor (`cached_property`) | 3.9% | drf 12%, django 9–13% |
| non-data descriptor get (classmethod/staticmethod/`cached_property` first access) | 2.2% | event_ledger 15% |
| `__getattr__` / `__getattribute__` hooks; absent (AttributeError path) | 3.2% | django LazyObject |

So the site is generic because it is **polymorphic** (single-entry inline caches, guarded by
exact type version). The second reason is a resolution the specializer refuses: Python
non-data descriptors, materialized `__dict__`, getsets. Deopt churn and type-version
invalidation are small: `_Py_Specialize_LoadAttr` is 0.1%. The census
(`spec_census.py`) finds 300–540 hot generic `LOAD_ATTR` sites per framework workload.

### 2.4 Wave-1 Rustique and the specializer (M, `spec_census.py --diff`)

`rustique.enable()` changes 19–30 call sites per workload from `CALL_PY_EXACT_ARGS` to
`CALL`/`CALL_NON_PY_GENERAL`: the callers of vectorcall-patched stdlib functions (known
+180–260 Ir per fallback call, mechanisms.md §1.2). Nothing else de-specializes. There are no
type-version bumps on user types and no dict-watcher effects on globals. Other form differences
between the two censuses are run-to-run (code objects alive at census time).

## 3. Approaches, steelmanned

### 3.1 PEP 523: classic hook vs the 3.15 generated-copy hook (M1)

New in 3.15: `_PyInterpreterState_SetEvalFrameAllowSpecialization(interp, 1)`
(`Python/pystate.c:3131`) lets the specializer keep working while a hook is installed.
CPython's main loop still deopts its inlined calls under any hook
(`IS_PEP523_HOOKED = eval_frame != NULL`), so the hook must be a *copy* of the loop that
redefines that macro. CPython ships one as a test: `Modules/_testinternalcapi/interpreter.c`,
generated from `bytecodes.c` plus `testbytecodes.c` overrides, built into the PBS binary as
`_testinternalcapi.set_eval_frame_interp()`.

Ir per operation, ratio to no hook (M, `hook_cg.py`, 3.15.1):

| cell | classic hook (`record`) | PBS test copy | our copy A (clang-20 -O3) | B (+ local helpers) | **C (+ PGO)** | C, all 229 handlers swapped (M2) |
|---|---:|---:|---:|---:|---:|---:|
| `ident(1)` | 6.84 | 1.19 | 1.27 | 1.15 | **0.988** | 0.980 |
| `o.m(1)` | 6.28 | 1.17 | 1.24 | 1.13 | **0.983** | 0.976 |
| `kw(1, c=4)` | – | 1.05 | 1.07 | 1.007 | **0.996** | 0.991 |
| closure call | – | 1.17 | 1.24 | 1.12 | **0.992** | 0.987 |
| `fib(10)` | 4.46 | 1.15 | 1.23 | 1.11 | **1.032** | 1.028 |
| instantiation `P(1, 2)` | – | 1.11 | 1.15 | 1.09 | **1.078** | 1.058 |
| user `__eq__` (C→Py entry) | – | 1.15 | 1.15 | 1.09 | **1.066** | 1.015 |
| generator, 100 items | 6.62 | 0.97 | 0.98 | 0.98 | **0.967** | 0.952 |
| coroutine, 20 awaits | – | 1.05 | 1.05 | 1.04 | **1.032** | 1.026 |
| comprehension, 100 items | 1.00 | 1.04 | 1.05 | 1.04 | **1.041** | 1.038 |
| `sum(genexpr)` (C resumes a generator) | – | 1.12 | 1.09 | 1.09 | **1.063** | 0.961 |
| raise + except | – | 1.08 | 1.07 | 1.08 | **1.064** | 1.060 |

Whole workloads with the PGO copy (M, `varstat.py`, instructions per run): M1 1.029–1.042;
all handlers swapped 1.017–1.035.

Why the copy is not free (M, `cg_annotate_cell.py`). The core's handlers inline, through
LTO+PGO, helpers an extension can only call: `_PyEval_FrameClearAndPop`,
`_PyFrame_ClearExceptCode`, `_PyCompactLong_*`, `_PyFrame_PushUnchecked`. BOLT
(`-inline-all -indirect-call-promotion=all`) also folds hot successor handlers into their
predecessors: `RESUME_CHECK` disappears into `CALL_PY_EXACT_ARGS`. B restores the first group
with local copies of those helpers (`rq_inline.h`, common case only, core call otherwise). PGO
fixes inlining and layout. BOLT on our `.so` would be the remaining step. PBS's own copy
lacks all three.

Verdict: **M1 works and is exact by construction, but costs 2–4% on whole programs before
any override pays.** It is only worth it on builds M2 can't reach and only if its overrides
save more than ~4%. Research tier, not default.

### 3.2 M2: per-opcode handler swap (new)

Mechanism (M, `rqeval/rqpatch.py`, `rqeval_mod.c: rq_patch`):
- `TARGET(op)` is `Py_NO_INLINE PyObject *Py_PRESERVE_NONE_CC _TAIL_CALL_op(TAIL_CALL_PARAMS)`.
  Dispatch is `musttail` through the table pointer that every handler receives
  (`ceval_macros.h:104`).
- The table is `static py_tail_call_funcptr instruction_funcptr_handler_table[256]`: not
  const, in `.data` (PBS 3.15.1: `0x14cdbf0`; 3.15.1t: `0x1493a90`).
- Our build generates the whole loop (labels, every handler) from the running release's
  `bytecodes.c` plus an override file. It compiles with the core's exact `-D` set
  (`PY_CFLAGS_NODIST`: `-D_Py_TIER2=3 -D_Py_JIT`) and clang ≥ 19.
- We CAS a chosen entry from the verified core handler address (`_TAIL_CALL_<op>` from the
  symbol table) to ours.
- Our handler runs with the core's frame, stack and tstate. It dispatches onward through the
  table it was passed, i.e. back into core handlers. Its labels (`error`, `exception_unwind`)
  and deopt targets are our verbatim copies.
- Restore = store the saved pointer.

Properties (M unless marked):
- **Cost of unswapped opcodes: 0** (same table, same code). A swapped handler costs its own
  compile quality on its own paths (§3.1 last column).
- **ABI:** handlers built by clang-20 run inside the clang-23 PBS binary (preserve_none
  x86-64 argument registers unchanged since LLVM 19). It must be checked per platform; on
  AArch64 it is untested.
- **BOLT indirect-call promotion is safe:** it compares the loaded target before jumping
  directly (I, BOLT ICP).
- **Direct jumps bypass the table** (deopt `JUMP_TO_PREDICTED`, labels). Overriding a
  *generic* handler therefore misses the deopt traffic of the core's specialized forms,
  unless the whole family is swapped (our copies of the specialized forms cost 0.96–1.00,
  §4.2).
- **Free-threading (3.15.1t):** the table is in `.data`, and all 229 handlers swap.
  - `ft_race.py`: 4 threads run attribute loads, isinstance, exceptions, calls and
    generators while the main thread swaps and restores 1 or 229 handlers 300–1000 times.
    0 wrong results, 0 crashes, on both builds.
  - Pointer stores are atomic release stores of aligned pointers. Readers see the old or the
    new handler, both correct.
  - Override-side caches must be atomic or per-thread on t (prototype caches are GIL-only).
- **JIT (`PYTHON_JIT=1`):** tier 1 dispatches through the table, so swapped handlers run in
  tier 1. Traces use their own uops, so overrides don't apply inside traces: correct, no gain.
  Swapped handlers must not include `ENTER_EXECUTOR`/`JUMP_BACKWARD*`; we never swap them.
  Measured with `PYTHON_JIT=1` and the final override set (`_rqeval_acJ`): `swaptest.py` is
  correct, `kwtest.py` is identical to plain, and `ft_race.py` (4 threads, 200 swap/restore
  cycles) has 0 errors.
- **sys.monitoring / settrace:** instrumentation rewrites instructions to `INSTRUMENTED_*`
  opcodes, which we don't swap. Instrumented code therefore runs CPython's handlers and the
  original semantics (I, `instrumentation.c`; regrtest §5).
- **Subinterpreters:** the table is process-global, so swapped handlers run in every
  interpreter. Overrides must be interpreter-agnostic or check `tstate->interp` (free: tstate
  is a handler argument). The prototype's caches check `interp == main`.
- **Code layout is the residual cost.** A swapped handler leaves the BOLT-optimized layout of
  the core.
  - Verbatim copies of the `LOAD_ATTR` family cost +0.8% instructions on task_planner;
    swapping all 229 handlers costs 1.017–1.035 on whole programs.
  - So swap only what an override needs, and suspend a group that does not pay for itself
    (§4.4).
- **Other PEP 523 users** (debuggers such as pydevd, `-X perf`, profilers): M2 installs no
  hook. Under a hook the core's loop still dispatches through the table, so the overrides
  keep working. Their builds must use the core's `IS_PEP523_HOOKED` definition; the
  prototype shares M1's relaxed one, which only matters under another copy-based hook.
- **Native tools** (perf, gdb, py-spy native mode): swapped handlers appear as
  `_TAIL_CALL_<op>` symbols of Rustique's extension instead of the python binary.
  `_PyEval_EvalFrameDefault` stays on the C stack (unlike M1, §5.1).

### 3.3 Tier-2 executors as a Rustique entry

`ENTER_EXECUTOR` hands the live frame to an executor's `jit_code` (preserve_none,
`_PyJIT_Entry`, `Tools/jit/shim.c`). The API is exported: `_Py_ExecutorDetach`,
`_Py_Executor_DependsOn`, `_Py_Executors_InvalidateDependency`. It would give a per-code-object
hook that costs nothing elsewhere and that CPython invalidates on monitoring and type changes.
Two blockers:
- `_Py_ExecutorInit` is not exported, so executors can't be linked into invalidation;
- free-threaded builds have no executors (`optimizer.c` is `#ifndef Py_GIL_DISABLED`).
More fundamentally, it needs a compiler (a trace JIT) to beat tier 1 on framework code.
**Superseded by M2** for tier-1 overrides. A JIT is out of scope (U high: CPython's own JIT).

### 3.4 CPython's JIT as a Rustique policy (M, `opprof.py --jit`, cycles per run, loaded box)

| workload | JIT / no JIT |
|---|---:|
| django_blog | 1.35 |
| flask_jinja | 1.16 |
| pylint_stdlib | 1.08 |
| sqlglot_transpile | 1.03 |
| order_domain | 1.17 |
| drf_api | 0.91 |

Under the JIT, 7–23% of cycles are in JIT code, and frame push/pop runs out of line
(`_PyEvalFramePushAndInit`, `_PyEval_FrameClearAndPop` are top leaves). The JIT also makes
vectorcall-site fallbacks dearer (mechanisms.md §1.3). It can only be enabled at startup
(`PYTHON_JIT`). Verdict: **not a Rustique policy on 3.15.1.** Phase 0's quiet-box numbers
should confirm. M1 is incompatible with it (it refuses); M2 is compatible (tier 1 only).

### 3.5 Vectorcall replacement of *user* functions by compiled kernels ("Rust JIT")

- The caller penalty is +180–260 Ir per call (+250–350 under the JIT; mechanisms.md §1.2),
  against inlined calls of 270–460 Ir for typical small framework functions (§3.1). Only
  long-running pure functions could pay that back.
- Their share is small:
  - handler self time in "arith/subscript" is 1.1–3.3%;
  - loops whose bodies make no Python calls and no polymorphic attribute access do not show
    up as a distinct bucket in any framework profile.
- Exactness needs frames on demand: tracebacks, `sys._getframe`, `f_lineno`, monitoring,
  signals.
- Verdict: **settled, out of reach.** M2 gives the same reach (the interpreter's own frames,
  zero caller penalty) without a compiler.

### 3.6 Other ideas considered

| idea | finding | verdict |
|---|---|---|
| Swap `tp_getattro` of heap types | `specialize.c` compares `tp_getattro == PyObject_GenericGetAttr` by pointer; a swap de-specializes every attribute load on the type | rejected |
| `type.__instancecheck__` `ml_meth` swap | runs after the lookup, binding and call that cost the time | rejected; M2 override instead |
| Inject `BINARY_OP_EXTEND` descriptors | the only cache with function pointers; generic BINARY_OP's own overhead is small next to the slot it calls | not worth it |
| Exceptions | 0.5–0.8% of cycles in these families (§2.1) | not a target |
| Generators | `FOR_ITER_GEN`/`SEND_GEN` inline; creation and dealloc 0.3–0.7% | not a target |
| `CALL_ALLOC_AND_ENTER_INIT` (w2-machinery) | checks only the class's type version, so vectorcall kernels on `__init__` (dataclass-generated) are bypassed for positional calls | an M2 override can route to machinery's `__init__` kernels; joint item |
| namedtuple `tp_new` (w2-machinery) | `tp_new_wrapper` compares `tp_new` with `slot_tp_new` by pointer, so it can't be swapped | M2 override of the `CALL` path if needed |

## 4. Overrides (exact fast path; core code for everything else)

### 4.1 `CALL_ISINSTANCE` (`rq_support.h: rq_isinstance_fast`)

Exactness: the fast path replays `object_recursive_isinstance` / `object_isinstance`
(`Objects/abstract.c:2603–2700`):
- **Metaclass shortcut.** If `type(cls)` resolves `__instancecheck__` to `type`'s own method,
  CPython binds and calls it, and it computes `object_isinstance(inst, cls)`. No Python frame
  runs in either case.
- **Negative subtype check.** CPython then looks up `inst.__class__`. When `type(inst)` uses
  `PyObject_GenericGetAttr` and its MRO resolves `__class__` to `object`'s data descriptor,
  that lookup returns `type(inst)`: an instance dict cannot shadow a data descriptor.
  Otherwise the lookup is done exactly as `object_isinstance` does it.
- **Caching.** Both facts are cached by `tp_version_tag` (CPython's method-cache rule).
- **Tuples:** items in order.
- **Everything else:** `PyObject_IsInstance` unchanged.
- **Recursion.** The C-stack recursion guard of the original is outside the contract
  (ACCELERATION §5.8).

Ir per call (M, `isinstance_cg.py`; only `CALL_ISINSTANCE` swapped). Columns: the first
build (`_rqeval_isinst`, no PGO) and the final build (`_rqeval_acF`, PGO; type path inlined,
tuple helper out of line):

| case | plain | first | **final** |
|---|---:|---:|---:|
| exact | 171 | 0.854 | **0.842** |
| subclass, positive | 194 | 0.954 | **0.974** |
| **negative, plain class** | 347 | 0.597 | **0.591** |
| negative, int instance | 356 | 0.601 | **0.596** |
| **metaclass, positive / negative** | 562 / 720 | 0.347 / 0.302 | **0.358 / 0.302** |
| tuple `(Other, A)` | 631 | 1.052 | **0.789** |
| `__getattr__` class, negative (fallback) | 522 | 1.015 | **1.040** |
| ABCMeta negative (fallback) | 1658 | 1.020 | **1.025** |

An intermediate build that recursed into tuples hit 1.077 on the positive case, from a
non-inlined call. It also failed test_isinstance on deep nesting (§5.1). Both are fixed in the
final build.

### 4.2 Megamorphic attribute cache in generic `LOAD_ATTR` (`rq_attrcache.h`)

Keyed by `(tp_version_tag, name)`. On a hit it replays `_PyObject_GenericGetAttrWithDict`
(attribute loads) and `_PyObject_GetMethodStackRef` (method loads) for the cached
resolution, in their order:
1. **Type MRO lookup.** Cached, valid while the version tag holds.
2. **Data descriptors** go to the generic code, except a `__slots__` member: `Py_T_OBJECT_EX`,
   no audit, plain offset, `descr_check` guaranteed by the MRO. That is read like
   `PyMember_GetOne`; a NULL slot goes to the generic code, which raises.
3. **Instance attribute:** inline values (`_PyObject_TryGetInstanceAttribute` logic), or the
   instance dict through `PyDict_GetItemRef`. Unicode-keyed dicts only, so no key `__eq__`
   runs.
4. **Not in the instance:** a method descriptor gives (descr, self) for method loads, or its
   `tp_descr_get` for attribute loads; a plain class attribute is returned as is.

Everything else runs the generic code: absent names (AttributeError, suggestions), other
non-data descriptors without an instance value, hooks, class and module owners, a version
tag of 0, and other interpreters. Negative entries cache uncacheable resolutions. Names are
held as strong references; descriptors are borrowed under the version tag.

Ir per operation (M, `attr_cg.py`, PGO build `_rqeval_acD`). Two columns: only the generic
`LOAD_ATTR` handler swapped, or the whole `LOAD_ATTR` family swapped:

| cell | plain | generic only | family |
|---|---:|---:|---:|
| polymorphic `o.x` (3 types, per 4 loads) | 1068 | 1068 (1.000) | 833 (**0.780**) |
| polymorphic `o.m()` (per 4 calls) | 1907 | 1907 (1.000) | 1768 (0.927) |
| polymorphic `__slots__` `o.args` (inherited slot, 3 types, per 4 loads) | 1232 | 1232 (1.000) | 832 (**0.675**) |
| instance attribute, materialized `__dict__` (generic site) | 225 | 136 (0.604) | 136 (**0.604**) |
| `cached_property`, cached (generic site) | 238 | 136 (0.571) | 136 (**0.571**) |
| specialized `a.x` / `a.m()` / `math.pi` (our family copies) | 82 / 270 / 93 | unchanged | 0.963 / 0.996 / 0.957 |
| `__getattr__` class, attribute found | 82 | 82 | 79 (0.963) |
| AttributeError path (miss → generic code) | 3872 | 3996 (1.032) | 3996 (1.032) |

With the descriptor-type guard and the per-site hint (`_rqeval_acG`, family):
- polymorphic `o.x` 0.806, `o.m()` 0.951, inherited slot 0.709;
- materialized-dict attribute 0.666, `cached_property` 0.655;
- specialized forms 0.957–0.996.
The two extra checks cost ~15 Ir on hits at generic sites. That is the price of the two
exactness and regression fixes.

**Polymorphic sites need the family swap.** A polymorphic site is specialized for one type,
and executions for the other types deopt with a direct `musttail` jump into the core's generic
handler, which bypasses the table. Swapping only the generic entry therefore reaches only
sites that are generic in the bytecode. w2-attribution measured the split directly on 65
workloads (bpf sampler, `frame->instr_ptr` checked against the end-of-run adaptive
bytecode):

| family | generic `LOAD_ATTR` | site still generic at end of run | specialized site deopting (lower bound) |
|---|---:|---:|---:|
| all 65 | 3.66% | 2.34 | 1.19 |
| frameworks | 7.92 | 5.45 | 2.26 |
| custom | 4.31 | 3.21 | 0.98 |
| libs / devtools | 4.07 / 3.40 | 1.97 / 1.59 | 2.03 / 1.54 |

"Still generic at end of run" includes polymorphic sites caught in adaptive backoff.

Hit rate on real code (M, `rqeval/ac_hits.py`, `_rqeval_acC` family, before slots and
negative caching): 65–88% of generic executions (drf 65%, django_admin 74%, django_blog 79%,
rules_engine 88%). Slots (`RQ_K_SLOT`) add the 33% data-descriptor share of §2.3 where it is
`__slots__`.

### 4.3 `object.__setattr__` through `CALL_NON_PY_GENERAL` (`rq_support.h: rq_call_non_py`)

`object.__setattr__(self, name, value)` is called by frozen dataclass `__init__` and
`__post_init__`, attrs and pydantic-style code. CPython's path:
1. builds a 3-tuple (`_PyObject_MakeTpCall`);
2. slices it (`wrapperdescr_call`);
3. unpacks it (`wrap_setattr`: `PyArg_UnpackTuple`);
4. runs `hackcheck`, a no-op unless `self` is a type;
5. calls `d_wrapped`, i.e. `PyObject_GenericSetAttr`.

The fast path calls `d_wrapped` directly for exactly that descriptor object, 3 positional
arguments, and `self` not a type. It returns `None` or the exception `d_wrapped` raised.

| cell (M, `setattr_cg.py`, only `CALL_NON_PY_GENERAL` swapped) | plain | swapped | ratio |
|---|---:|---:|---:|
| frozen dataclass `F(1, 'x', 2.0)` (3 fields) | 3826 | 2087 | **0.545** |
| `object.__setattr__(o, 'x', 1)` | 954 | 381 | **0.400** |
| `object.__setattr__(T, 'x', 1)` (type → fallback, raises) | 3490 | 3585 | 1.027 |
| `dict.__setitem__(d, 'a', 1)` (other wrapper → fallback) | 905 | 939 | 1.038 |
| `str.join('-', …)` via descriptor | 727 | 727 | 1.000 |

### 4.4 Whole-run instruction counts (M, `wl_cg.py`, exact Ir of one steady-state `run()`)

Exact Ir of one steady-state `run()` (fresh process, setup and two warm-up runs, then
client-request-bracketed callgrind), ratio to plain. Build `_rqeval_acD` (PGO):
- **A** = isinstance + attribute cache (whole `LOAD_ATTR` family swapped);
- **B** = A + `object.__setattr__`.

| workload | family | plain (G Ir) | A | B |
|---|---|---:|---:|---:|
| django_admin | frameworks | 6.69 | 0.9799 | 0.9835 |
| drf_api | frameworks | 6.14 | 0.9940 | 0.9964 |
| falcon_api | frameworks | 3.96 | 0.9957 | 0.9948 |
| fastapi_sqlalchemy | frameworks | 4.06 | 0.9793 | 0.9795 |
| flask_marshmallow | frameworks | 3.41 | 0.9592 | 0.9643 |
| strawberry_graphql | frameworks | 8.28 | 0.9849 | 0.9771 |
| **frameworks geomean** | | | **0.9821** | **0.9826** |
| di_plugins | custom | 8.67 | 1.0015 | 0.9889 |
| ecs_arena | custom | 12.45 | 0.9957 | 0.9524 |
| event_ledger | custom | 14.64 | 0.9909 | 0.9591 |
| fulfilment_sim | custom | 16.92 | 0.9849 | 0.9856 |
| mini_interp | custom | 14.80 | 1.0054 | 1.0048 |
| order_domain | custom | 15.23 | 0.9955 | 0.8694 |
| orm_mapper | custom | 8.25 | 0.9755 | 0.9426 |
| report_engine | custom | 11.86 | 1.0001 | 0.9121 |
| rules_engine | custom | 9.64 | 0.9834 | 0.8918 |
| task_planner | custom | 6.26 | 1.0270 | 1.0272 |
| **custom geomean** | | | **0.9959** | **0.9522** |
| django_blog | other | 3.09 | 0.9859 | 0.9896 |
| sqlglot_transpile | other | 8.05 | 0.8944 | 0.8951 |
| **other geomean** | | | **0.9390** | **0.9412** |

Generic-only `LOAD_ATTR` swap (first build `_rqeval_acC`, isinstance + generic handler only):
falcon_api 0.9980, fastapi_sqlalchemy 0.9935, flask_marshmallow 0.9698, strawberry_graphql 1.0016. Without the family swap, polymorphic sites are not reached (§4.2).

Readings:
- **The attribute cache and isinstance** give 0.96–0.99 on frameworks, 0.98–0.995 on most
  custom code, and 0.894 on sqlglot (inherited-slot polymorphism plus metaclass isinstance).
- **`object.__setattr__`** carries the custom family: order_domain 0.869, rules_engine 0.892,
  report_engine 0.912. That is what frozen dataclasses cost today.
- **Two regressions:**
  - task_planner 1.027: 99% of its generic `LOAD_ATTR` is a Python descriptor class with
    `__get__`/`__set__` (`Target.cpu`). The override adds a cache probe (negative entry) to
    every execution and our copies of the family's handlers run instead of the core's.
  - mini_interp 1.005.
  The build with an inlined probe (`_rqeval_acF`) is re-measured below.

**Build `_rqeval_acG`** (acD + descriptor-type guard + per-site negative hint for generic
sites; full regrtest equal to the noop baseline, §5.1). Same A/B sets:

| workload | family | A (acG) | B (acG) |
|---|---|---:|---:|
| django_admin | frameworks | 0.9804 | 0.9839 |
| drf_api | frameworks | 0.9963 | 0.9985 |
| falcon_api | frameworks | 0.9958 | 0.9951 |
| fastapi_sqlalchemy | frameworks | 0.9763 | 0.9844 |
| flask_marshmallow | frameworks | 0.9728 | 0.9732 |
| strawberry_graphql | frameworks | 0.9757 | 0.9804 |
| **frameworks geomean** | | **0.9828** | **0.9859** |
| di_plugins | custom | 0.9987 | 0.9861 |
| ecs_arena | custom | 0.9992 | 0.9560 |
| event_ledger | custom | 0.9908 | 0.9585 |
| fulfilment_sim | custom | 0.9787 | 0.9795 |
| mini_interp | custom | 1.0022 | 1.0026 |
| order_domain | custom | 0.9911 | 0.8649 |
| orm_mapper | custom | 0.9773 | 0.9446 |
| report_engine | custom | 0.9985 | 0.9113 |
| rules_engine | custom | 0.9867 | 0.8964 |
| task_planner | custom | 1.0214 | 1.0215 |
| **custom geomean** | | **0.9944** | **0.9510** |
| django_blog | other | 0.9863 | 0.9902 |
| sqlglot_transpile | other | 0.8856 | 0.8860 |
| **other geomean** | | **0.9346** | **0.9366** |

The hint takes task_planner from 1.027 to 1.021 and mini_interp from 1.005 to 1.003. The
rest is the cost of our copies of the family's handlers: 0.8% on task_planner with *no*
override, measured with the verbatim copies `_rqeval_C`.

`object.__setattr__` costs frameworks 0.3–0.8% (B vs A on fastapi, strawberry, drf). The
override handler copy is on every `CALL_NON_PY_GENERAL`, a very hot handler (10% of cycles
in w2-attribution's 60), and frameworks rarely hit it.

**Final build `_rqeval_acJ`:** all four overrides, plus net-benefit suspension per override
group. ACCELERATION §5.6 is applied to handlers: a group whose fast path hits less than
1/ratio of its executions in a 65,536-execution window restores the core's handlers for that
process. Ratios: attribute family 5, `object.__setattr__` 64, keyword instantiation 64.
`ac_hits.py` on task_planner with the quick build: suspended=1 after the first window.

**`_rqeval_acJ`, all four overrides with per-group suspension** (exact Ir of one steady-state
run, ratio to the same plain runs). The suspension rule here is per single 65,536-execution
window; the long-horizon rule follows below.

| workload | family | all four overrides (`_rqeval_acJ`) |
|---|---|---:|
| django_admin | frameworks | 0.9854 |
| drf_api | frameworks | 0.9931 |
| falcon_api | frameworks | 0.9890 |
| fastapi_sqlalchemy | frameworks | 0.9819 |
| flask_marshmallow | frameworks | 0.9861 |
| strawberry_graphql | frameworks | 0.9685 |
| **frameworks geomean** | | **0.9840** |
| di_plugins | custom | 0.9714 |
| ecs_arena | custom | 0.9595 |
| event_ledger | custom | 0.9824 |
| fulfilment_sim | custom | 0.9791 |
| mini_interp | custom | 0.9934 |
| order_domain | custom | 0.8655 |
| orm_mapper | custom | 0.9476 |
| report_engine | custom | 0.9143 |
| rules_engine | custom | 0.8987 |
| task_planner | custom | 1.0002 |
| **custom geomean** | | **0.9502** |
| django_blog | other | 0.9844 |
| sqlglot_transpile | other | 0.8734 |
| **other geomean** | | **0.9272** |
| **all 18 geomean** | | **0.9587** |

[exited with code 0]

- **No workload is slower:** worst task_planner 1.0002, where the attribute group suspended
  itself.
- Frameworks −1.6%, custom −5.0%, sqlglot −12.7%, django_blog −1.6%; geomean of all 18 0.959.
- Keyword instantiation adds to acG mainly on di_plugins (0.986 → 0.971), mini_interp
  (1.003 → 0.993), strawberry (0.980 → 0.969) and sqlglot (0.886 → 0.873).
- The single-window rule suspended groups too eagerly during start-up:
  - flask_marshmallow's attribute group (0.973 under acG → 0.986);
  - event_ledger's `object.__setattr__` group (0.959 → 0.982).
  The final rule decides once, on cumulative counts over ~1M executions, with cache fills
  excluded. `groups_probe.py` confirms the corrected decisions:
  - event_ledger: nothing suspended;
  - flask_marshmallow: only the setattr group (it builds no frozen dataclasses);
  - task_planner: only the attribute group.

**Final build `_rqeval_acL`**: all four overrides, main-interpreter guard, long-horizon
suspension. The full regression suite equals the noop baseline (§5.1).

| workload | family | all four overrides (`_rqeval_acL`) |
|---|---|---:|
| django_admin | frameworks | 0.9785 |
| drf_api | frameworks | 0.9963 |
| falcon_api | frameworks | 0.9893 |
| fastapi_sqlalchemy | frameworks | 0.9731 |
| flask_marshmallow | frameworks | 0.9704 |
| strawberry_graphql | frameworks | 0.9696 |
| **frameworks geomean** | | **0.9795** |
| di_plugins | custom | 0.9715 |
| ecs_arena | custom | 0.9543 |
| event_ledger | custom | 0.9531 |
| fulfilment_sim | custom | 0.9800 |
| mini_interp | custom | 0.9940 |
| order_domain | custom | 0.8690 |
| orm_mapper | custom | 0.9490 |
| report_engine | custom | 0.9165 |
| rules_engine | custom | 0.9004 |
| task_planner | custom | 1.0005 |
| **custom geomean** | | **0.9480** |
| django_blog | other | 0.9899 |
| sqlglot_transpile | other | 0.8758 |
| **other geomean** | | **0.9311** |
| **all 18 geomean** | | **0.9565** |

- **Final package:** frameworks −2.1%, custom −5.2%, sqlglot −12.4%, django_blog −1.0%;
  18-workload geomean 0.957.
- **Compared with the single-window rule (acJ):** flask_marshmallow 0.986 → 0.970,
  event_ledger 0.982 → 0.953, fastapi 0.982 → 0.973.
- **Worst case:** task_planner 1.0005 (its attribute group suspends). The `object.__setattr__`
  and keyword groups stay installed there until their own ~1M-execution decision, costing
  0.05%. That is within the ≤ 1.05 cell bar and 0.05% above the ≤ 1.00 whole-run bar
  proposed in §8. Phase 2 should cut the probe cost or shorten the horizon for low-traffic
  groups.

Projection beyond these 18 (I): the reachable pools are generic `LOAD_ATTR` 3.7% and
isinstance 1.6% (mean of w2-attribution's 65), plus `object.__setattr__` and keyword
instantiation where code uses them. The realized fraction of the targeted share is ~25% in
frameworks and ~65% in custom code. That suggests about −1% to −2% on the 65-workload mean,
−2% to −5% on framework and OO-heavy custom code, and up to −12% on polymorphic-slot code like
sqlglot.

### 4.5 C→Python call machinery (one override built: keyword instantiation; the others costed)

What a Python call costs when the interpreter pushes the frame inline, versus through C
(M, `callpaths_cg.py`, Ir per call):

| call | inline form | through C | re-entry overhead |
|---|---:|---:|---:|
| `f(*args)` vs bound method `bm(*args)` | 432 | 587 | +155 |
| `f(*args, **kw)` vs `o.m(*args, **kw)` | 1381 | 1935 | +554 |
| `P(1)` (`CALL_ALLOC_AND_ENTER_INIT`) vs `P(1, b=2)` (`CALL_KW` → `type_call`) | 973 | 2304 | **+1331** |
| `P(1)` vs `P(*args)` | 973 | 1213 | +240 |
| `o.m(1)` vs `o(1)` (`__call__`, `slot_tp_call`) | 389 | 731 | +342 |
| `f(1, 2)` vs `functools.partial(f, 1)(2)` | 295 | 630 | +335 |

Exact fast paths exist for each, as overrides that push the callee's frame inline with the
core's helpers (`_PyEvalFramePushAndInit[_Ex]`, the `_Py_InitCleanup` shim for `__init__`):
- **`CALL_KW` to a class:** metaclass `type`, `tp_new == object_new`, `tp_init == slot_tp_init`
  with a plain Python `__init__`. Same allocation; `__init__` must return None (shim).
  `object_new`'s excess-argument rule is not triggered because `tp_init` is overridden.
- **`CALL_EX` of a bound method:** a new args tuple with self prepended, then as `CALL_EX_PY`.
- **`__call__` of instances:** as `CALL_BOUND_METHOD_*`, guarded by the type version.
- **`functools.partial` of a Python function:** the C struct is fixed, so prepend the
  partial's args.

The observable difference is C-stack depth only (outside the contract). Tracebacks,
monitoring events and recursion counting match: the frames are the same Python frames.

**Built: keyword instantiation** (`rq_support.h: rq_kw_init_for / rq_push_kw_init`,
override of `_CALL_KW_NON_PY`).
- Scope: for exactly the classes CPython itself pushes inline for positional calls (metaclass
  `type`, heap type, not abstract, `tp_new == object_new`, `tp_alloc == PyType_GenericAlloc`,
  `__init__` a Python function with the default vectorcall, no PEP 523 hook, stack space
  and recursion headroom).
- What it does: `CALL_ALLOC_AND_ENTER_INIT`'s sequence (allocate, `_Py_InitCleanup` shim
  with `EXIT_INIT_CHECK`, push `__init__`), with the keywords passed to
  `_PyEvalFramePushAndInit` as `CALL_KW_PY` does.
- Measured:
  - `P(1, b=2)`: 2304 → 1156 Ir (**0.502**, quick build).
  - `P(1)` 0.995 and unrelated calls 1.000: the handler is not reached.
  - `kwtest.py`, plain vs swapped, is byte-identical over 12 shapes: defaults, `*args`,
    `**kw`, keyword-only, `__init__` returning non-None, raising, unexpected, missing and
    duplicate keywords, `__slots__`, subclasses, and RecursionError from deep recursion.
    The comparison covers results, messages and traceback frames.
- Final-build numbers: §4.4.

The other three (bound-method `f(*a, **k)`, `__call__`, `partial`) follow the same pattern.
Projected (I, shares §2.2 × savings above): frameworks ≈ 1–1.5% more, custom ≈ 0.5%. Each is
about a week in Phase 2 with tests.

## 5. Exactness and safety plan

| obligation | how | status |
|---|---|---|
| same code as CPython for non-overridden behaviour | generate from the running release's `bytecodes.c` with CPython's cases generator; overrides are declared `override op(...)` blocks whose fallback is CPython's original body | M (prototype) |
| version gate | exact `Py_Version` (patch level). 3.15.1 → 3.15.2 changes `bytecodes.c` (one instruction's error path: a refcount fix), `generated_cases.c.h`, `pycore_opcode_metadata.h`, `cpython/code.h` (M, diff on the box). So **one build per patch release**; an unknown patch level refuses. | M |
| build-configuration gate | compile-time `Py_GIL_DISABLED`, `_Py_TIER2`, `_Py_JIT`, `_Py_TAIL_CALL_INTERP`, `Py_DEBUG` must equal the running build's (`sysconfig` `PY_CFLAGS_NODIST` + `pyconfig.h`) | M (build.sh takes them from sysconfig) |
| table verification | before each CAS, every swapped entry must equal the address of `_TAIL_CALL_<op>`. Ship a table of (ELF build-id → table offset, per-op handler offsets) for each known PBS build; verify the build-id at install; refuse unknown binaries. Prototype: `nm` once, cached per binary (0.5 s → JSON). Install cost: loading the 160 KB override extension costs +0.58 M Ir per process (callgrind, `python -S -c 'import _rqeval_acF'` vs `python -S -c 'import sys'`: 27.58 M vs 27.00 M; about 0.15 ms). The prototype harness adds +14 ms of Python (json, subprocess, rqpatch). A product links the handlers into Rustique's own extension and ships the offsets, so the install is the CAS loop (< 0.01 ms, I), lazy at `enable(interp=True)`. | M (prototype), I (product) |
| ABI | preserve_none argument order checked per platform by a self-test at install: swap a probe opcode, run a probe function, compare, restore | I |
| full regression suite, plain vs variant, 0 differences | `rqeval/run_tests.sh`: the variant is installed in every process through a `.pth` (also under `-I`); `cmp_regrtest.py` compares failed-file sets against plain and against a `noop` variant (harness footprint only) | see §5.1 |
| differential fuzzing | Hypothesis over attribute-resolution shapes (descriptors, slots, `__dict__` materialization, class mutation between loads) and isinstance shapes | Phase 2 |
| sys.monitoring / settrace | `INSTRUMENTED_*` opcodes are never swapped | I + regrtest (test_monitoring, test_sys_settrace) |
| JIT on | tier-1 only; never swap `ENTER_EXECUTOR`/`JUMP_BACKWARD*` | I |
| FT threads racing table writes | atomic release stores; override caches atomic or per-thread on t | M (`ft_race.py`: 0 errors) |
| `disable()` while frames run inside our handlers | handlers are re-entrant code with no state of their own. A frame inside our handler finishes it and dispatches through the table, then reaches core handlers. The `.so` is never unloaded (ACCELERATION §3.11). | I; M (`ft_race.py` swaps and restores under load) |
| subinterpreters | interpreter-agnostic overrides or `tstate->interp` check | M (prototype checks main) |

### 5.1 Full regression suite (M, `rqeval/run_tests.sh`, `cmp_regrtest.py`, 3.15.1, `-j8`)

Harness and baseline:
- 549 test files run; ~3,800 Python processes per run had the variant installed (counted by
  the harness).
- Plain baseline: 47,182 tests, 60 failures in 20 files. All are environmental: PBS install
  paths, perf, `-I` subprocesses. They are identical in every run.

| variant | failed files beyond plain | cause |
|---|---|---|
| isinstance + generic `LOAD_ATTR` cache + `object.__setattr__` (first build) | test_isinstance, test_opcache, test_datetime, test_faulthandler, test_lazy_import, test_repl, test_subprocess, test_io.test_general | **2 real bugs, fixed**, plus harness artifacts (below) |
| `LOAD_ATTR` family alone (first build) | the 5 harness files + test_datetime + test_opcache | test_opcache: bug 2 below; test_datetime: harness (a `.pth` import in subinterpreters, fixed in the harness) |
| **swap-all: all 229 handlers replaced by our generated copies (`_rqeval_C`)** | test_io.test_general, test_datetime, test_faulthandler, test_lazy_import, test_repl, test_subprocess | **harness only: 0 differences caused by the mechanism.** These are the noop variant's files plus test_datetime (subinterpreter harness) |
| M1 copy (PEP 523, `_rqeval_C` installed as the eval-frame function) | test_io.test_general, test_faulthandler, test_subprocess (harness) + **test_c_stack_unwind** (hung; killed after 5 min) | **one real difference.** Native unwinders expect `_PyEval_EvalFrameDefault` frames on the C stack, and under M1 the loop is `Rq_EvalFrame` in our `.so`. The same applies to py-spy-style native profilers and gdb helpers. M2 does not change the eval-frame function. |
| `noop` (harness footprint: extension loaded, `rqpatch` + `json` + `subprocess` imported at startup, nothing swapped) | test_lazy_import, test_repl, test_subprocess, test_io.test_general, test_faulthandler (same 12 failures) | harness, not the mechanism. Tests check startup-imported modules, the "Extension modules:" list in faulthandler output, and child stderr |
| **`_rqeval_acF`** (isinstance + attribute-cache family + `object.__setattr__`, bugs fixed) | test_io.test_general, test_faulthandler, test_lazy_import, test_repl, test_subprocess: **exactly the noop set; same counts (47,182 run, 73 failures, 25 files)** | **0 differences from the overrides** |
| **`_rqeval_acG`** (acF + per-site negative hint) | the noop set, same counts | **0 differences** |
| `_rqeval_acJ` (acG + keyword instantiation + per-group net-benefit suspension) | the noop set + test_interpreter_pool, test_datetime (SIGSEGV in a subinterpreter thread), test_struct | **bug 3 below, fixed** |
| **`_rqeval_acK`** (all four overrides + main-interpreter guard) | **exactly the noop set; same counts (47,182 run, 73 failures, 25 files)** | **0 differences from the overrides** |
| **`_rqeval_acL`** (final: acK + long-horizon suspension rule) | **exactly the noop set; same counts (47,182 run, 73 failures, 25 files)** | **0 differences** (`kwtest.py` also identical) |

The three real bugs show the gate works:
1. **Unbounded recursion on nested tuples** (`test_isinstance`): the first isinstance fast
   path recursed into nested tuples itself, without CPython's recursion guard. Fixed: nested
   non-type items go to `PyObject_IsInstance`, so the fast path never recurses.
2. **Descriptor kind changed without a class version change** (`test_opcache`
   `test_descriptor_added_after_optimization`): adding `__get__`/`__set__` to the class
   attribute's *own type* does not change the owner's version tag. Fixed: the cache also
   guards `type(descr)`'s identity and version tag when that type is mutable. CPython's
   specializer refuses the same case.
3. **Interpreter-unaware caches in process-global handlers** (test_interpreter_pool and
   test_datetime crashed in a subinterpreter). The dispatch table is a static of the binary,
   so swapped handlers also run in subinterpreters. Caches keyed by type version tags can
   then match another interpreter's heap type: the keyword-instantiation cache handed a
   subinterpreter the main interpreter's `__init__`. Fixed: every override fast path checks
   `tstate->interp == main` (free, since tstate is a handler argument). The previously
   failing tests pass, and test_interpreters fails identically in plain runs.

## 6. Build and maintenance

- **Per patch release:** run the cases generator on that tag's `bytecodes.c` plus our
  override file, then compile one `.so` per (patch, GIL/t, platform).
  - The override file must apply: `override op(X)` errors if X changed shape.
  - The generated code is ~145–160 KB per build (prototype `.so`).
  - A wheel for 3.15.x × {GIL, t} × {x86_64, aarch64} with three patch releases is ~12 builds,
    ~2 MB (I).
  - CI job per new CPython patch release: fetch the tag, generate, build with PGO (training:
    a fixed set of workloads, ~5 min), run the self-tests and the full regrtest (~3 min with
    `-j8`), record PBS build-ids and table offsets.
- **Compiler:** clang ≥ 19 (preserve_none and musttail) on Linux and macOS, matching ABI; the
  prototype used clang-20 against PBS's clang-23. Windows tail-calling builds use MSVC's
  `__preserve_none`, and ABI compatibility with a clang-built `.so` is unverified.
- **What a new patch costs us:** usually nothing manual (regenerate, rebuild, test). An
  override must be re-proven only when the generator reports that its op changed.

## 7. Reach (which CPython builds M2 can serve)

Tail-calling is opt-in in 3.15: `configure --with-tail-call-interp`, default off
(`configure.ac:7486`); Windows `build.bat --tail-call-interp`.

| distribution | interpreter | M2 | evidence |
|---|---|---|---|
| python-build-standalone (uv, rye, pdm, hatch, mise) Linux x86_64 3.15.1 / 3.15.1t | clang, tail-calling, non-PIE, unstripped, table in `.data` | **yes** | M (both builds) |
| PBS Linux aarch64, macOS x86_64/arm64 | same build recipe | expected | I: needs an ABI self-test per platform, and macOS `__DATA` writability |
| PBS Windows | MSVC | unknown | I: needs a matching `__preserve_none` ABI |
| Debian/Ubuntu/Fedora system Python, Docker official images, pyenv, Homebrew, actions/setup-python, conda-forge (Linux) | GCC or default configure: computed-goto loop | **no** | I: the label table can't point outside `_PyEval_EvalFrameDefault` |
| python.org macOS / Windows installers | opt-in flag | to check | I |

Where M2 can't go, M1 (the PEP 523 copy) is possible with matching internal headers. Its cost
is §3.1 (whole programs 1.02–1.04 on PBS), so there it is research only.

## 8. Proposed contract amendment (for ACCELERATION §1/§3/§5; needs a DECISIONS entry)

- **§1 Site:** add a fourth kind, **opcode handler**: one entry of the core interpreter's
  tail-call dispatch table, replaced by a handler generated from the running release's
  `Python/bytecodes.c` by CPython's cases generator with a Rustique override file. Kind D.
- **§3 Install** (new items):
  - (a) Only on builds whose (CPython patch version, build configuration, ELF build-id)
    appear in the target's verified list. The table offset and per-opcode handler offsets
    come from that list. Every entry is compared with the expected core handler before the
    CAS. Any mismatch refuses the whole `interp` group.
  - (b) An install-time self-test swaps a probe opcode, runs probe code and restores it,
    proving the calling convention.
  - (c) Never swap instrumented opcodes, `ENTER_EXECUTOR`, `JUMP_BACKWARD*`, or labels.
  - (d) Swap the whole family when an override must also catch the deopt traffic of
    specialized forms.
  - (e) Restore on `disable()` and at exit; the code stays mapped for the process lifetime.
- **§4 Guard:**
  - An override's fast path decides exactness from the same state CPython's code reads:
    type version tags, descriptor kinds, dict kinds.
  - Caches are keyed by version tags (CPython's method-cache rule).
  - Interpreter identity comes from the handler's `tstate`.
- **§5 Fallback:**
  - Outside the fast path, an override runs CPython's original op body, verbatim from
    `bytecodes.c`. It never re-dispatches.
  - Never slower: every cell ≤ 1.05, including the override's fallbacks and the specialized
    forms of a swapped family. A whole-run instruction count ≤ 1.00 on every workload of the
    reference set.
- **§5.5 (ruling 15) applies unchanged.** An override may call user code only where CPython's
  C code does, with no Python frame in between. Examples: a non-data descriptor's
  `tp_descr_get`, `PyObject_GenericSetAttr`.
- **§5.5 extension (frame pushes):** an override may push a Python frame inline instead of
  calling through C only in the way an existing CPython specialized form pushes it for the
  same kind of callee. Example: keyword instantiation follows `CALL_ALLOC_AND_ENTER_INIT` +
  `CALL_KW_PY`. Only the C-stack depth differs, which is already outside the contract (§5.8).
- **§5.6 applies per override group:** hits credit, executions debit. Below the group's
  ratio in a window, the group's original handlers are restored for the process, and
  `status()` reports it as `suspended`.

## 9. Candidates (brief format) and recommendation

**Measured package (all four overrides, final `_rqeval_acL`, exact whole-run Ir; §4.4):**
frameworks geomean 0.980, custom 0.948, sqlglot 0.876, django_blog 0.990; 18 workloads 0.957;
worst 1.0005. Full regrtest equal to the noop baseline.

Score per IMPACT_ATLAS §2: `I × F × (1 − K) × (1 − U) × (0.5 + R)`. Shares are means over
w2-attribution's 60–65 workloads where available (frameworks / custom in brackets); `s` is the
conservative speedup of the targeted path from the cells. R is code reach (how common the
pattern is in real code), not build reach; build reach (PBS-only) is folded into F.

| candidate | layer | entry points + mechanism | share mean / max (which) | s | exact subset & what falls back | floor vs current | risks | upstream overlap | reach | prototype result (M) | evidence | I | F | K | U | R | score |
|---|---|---|---|---:|---|---|---|---|---|---|---|---:|---:|---:|---:|---:|---:|
| **interp/attr-cache**: megamorphic attribute cache | language (attr) | M2 opcode handlers: `LOAD_ATTR` family (generic carries the cache; specialized forms verbatim so deopts reach it) | 3.7 / 13.8 (all 65; frameworks 7.9, custom 4.3; drf max) | 1.6 | non-data resolutions + `__slots__` members, by (type version, name); data descriptors other than slots, hooks, class/module owners, AttributeError → generic code | poly load 1068 → 833 Ir per 4; dict attr 225 → 136; slot 1232 → 832 per 4 | new site kind; family swap moves hot handlers out of BOLT layout; FT caches must be atomic | medium (CPython may add polymorphic caches in tier 1/2: 3.16+) | everywhere: polymorphic attribute access in all OO code | with isinstance (acG A, exact Ir): frameworks 0.983, custom 0.994, sqlglot 0.886; hit rate 65–88% | §2.3, §4.2 | 1.39 | 0.45 | 0.4 | 0.35 | 0.9 | **0.34** |
| **interp/isinstance**: exact isinstance fast path | language | M2: `CALL_ISINSTANCE` | 1.6 / 8.9 (sqlglot) | 2 | metaclass with type's own `__instancecheck__`; negative with plain `__class__`; tuples; ABCs and hooks → original | negative 347 → 225, metaclass 562–720 → 195–217 | as above | high: a 10-line upstream fix | everywhere | sqlglot 0.929 alone (perf stat); cells 0.30–0.60 | §4.1 | 0.80 | 0.6 | 0.3 | 0.5 | 0.9 | **0.24** |
| **interp/obj-setattr**: `object.__setattr__` direct | generated (frozen dataclass, attrs) | M2: `CALL_NON_PY_GENERAL` | 0.4 (custom 2.2, order_domain 6.5) | 2 | exactly `object.__setattr__`, 3 positional args, self not a type → `d_wrapped`; everything else → original | `F(...)` 3826 → 2087; call 954 → 381 | small | medium (wrapper-descriptor vectorcall upstream) | frozen dataclasses, attrs, pydantic v1 | whole runs (acG A→B): order_domain 0.991→0.865, rules_engine 0.987→0.896, report_engine 0.999→0.911, ecs_arena 0.999→0.956 | §4.3 | 0.20 | 0.7 | 0.25 | 0.4 | 0.4 | **0.06** |
| **interp/inline-calls**: C→Python frame pushes (kwargs `__init__`, bound-method `f(*a, **k)`, `__call__`, `partial`) | language (calls) | M2: `CALL_KW_NON_PY`, `CALL_EX_NON_PY_GENERAL`, `CALL_NON_PY_GENERAL` | ~2.5 (frameworks 4.1 incl. instantiation) | 1.8 | plain Python callee, `type` metaclass, `object_new` + Python `__init__`; else original | +155 to +1331 Ir per call over the inline form | frame-push overrides are the hardest to prove; recursion/C stack | medium (CPython keeps adding inlined call forms) | everywhere | keyword instantiation built: `P(1, b=2)` 0.53; whole runs: di_plugins 0.986→0.971, strawberry 0.980→0.969, sqlglot 0.886→0.873 | §4.5 | 1.1 | 0.35 | 0.4 | 0.35 | 0.8 | **0.20** |
| **getattr/hasattr** with the shared cache | language | `ml_meth` of `builtins.getattr/hasattr` (existing mechanism, all builds) | 1.2 / 16 (mini_interp) | 1.4 | same resolution as attr-cache; misses → original | ~150–250 Ir per call | low | low | Django, DRF, plugin systems | not built | §2.2 | 0.34 | 0.7 | 0.2 | 0.2 | 0.8 | **0.20** |
| **interp/store-attr** cache | language | M2: `STORE_ATTR` family | 0.7 (frameworks 1.35) | 1.4 | instance dict/inline stores of non-data names | – | as attr-cache | medium | everywhere | not built | §2.2 | 0.20 | 0.4 | 0.4 | 0.35 | 0.9 | **0.04** |
| M1: generated loop as PEP 523 hook (non-PBS builds) | – | `_PyInterpreterState_SetEvalFrameAllowSpecialization` + generated copy | – | 0.96–0.98 (a 2–4% loss) | everything; overrides as in M2 | whole programs 1.02–1.04 | perf/debugger hook conflicts | – | – | §3.1 | §3.1 | <0 | | | | | **no** |

**Settled / out of reach, with the reason:**
- Handler self time (dispatch plus inlined specialized work, 43–52%): no entry point and no
  override can beat CPython's own BOLT-laid-out handler on its own fast path. Our copies
  reach 0.96–1.00 at best.
- The PEP 523 classic hook (4.5–6.8× per call), and a full copy as default (whole programs
  1.02–1.04).
- CPython's JIT as policy (slower on 5 of 6).
- Vectorcall kernels for user functions (caller penalty, exactness, tiny share).
- Executors (no compiler to put behind them).
- `tp_getattro` swaps (de-specialize).
- Exceptions and generators (≤ 0.8%).

**Recommendation:**
1. **Wave 2: an opt-in `interp` tier** (`rustique.enable(interp=True)` / `RUSTIQUE_INTERP=1`)
   on verified PBS/uv builds (3.15.x, GIL and t; x86_64 first, then aarch64 and macOS after
   the ABI self-test). It is M2 with the four overrides: isinstance, the attribute cache over
   the `LOAD_ATTR` family, `object.__setattr__`, and keyword instantiation.
   - **Default-eligible on verified builds** once, per build:
     - full regrtest equal to plain (the `noop` harness footprint excepted);
     - Hypothesis differential suites for attribute resolution, isinstance shapes and call
       binding;
     - every cell ≤ 1.05;
     - whole-run instructions ≤ 1.00 on every workload of the reference set.
   - task_planner-like code (Python data-descriptor classes at generic sites) is the case to
     watch for the last bar.
2. **Not default on other builds.** M2 is impossible on computed-goto builds; M1 costs 2–4%
   and breaks native unwinders (§5.1).
3. **Phase 2 work order:**
   - (a) a build-id table and table verification, the ABI self-test, and the CAS install and
     restore in Rustique's own extension (no Python harness);
   - (b) the four overrides hardened for free-threading (atomic or per-thread caches) and
     subinterpreters;
   - (c) the remaining frame-push overrides (bound-method `f(*a, **k)`, `__call__`, `partial`)
     and a `STORE_ATTR` cache;
   - (d) with w2-machinery: route `CALL_ALLOC_AND_ENTER_INIT` / keyword instantiation to
     dataclass `__init__` kernels;
   - (e) CI per CPython patch release (§6).
4. **Upstream in parallel.** Each override is also a small CPython patch: isinstance
   (`object_isinstance`), a polymorphic attribute cache, `object.__setattr__` vectorcall, and
   `CALL_KW` to classes. That bounds U at ~0.3–0.5 for 3.16/3.17. Rustique ships them for
   3.15 on PBS first.

## 10. Reproduce

```bash
R=~/rustique-research/w2/devbox/rbox            # dev box; clang-20 + lld-20 + llvm-20 installed there
# per-opcode profiles, census, attribute probe
$R w2-interp run -- bash benches/interp/run_opprof.sh /work/w2-interp/out plain 8-11
python benches/interp/opsummary.py <profiles...>
$R w2-interp run -- bash benches/interp/run_census.sh /work/w2-interp/census
bash benches/interp/rqeval/build.sh _rqeval_attrprobe overrides_attrprobe.c -DRQ_ATTR_PROBE -DRQ_INLINE_HELPERS
python benches/interp/attrprobe.py <workload.py> -o out.json
# M1 / M2 builds: verbatim copy A, B (helpers), C (PGO); override builds (final: acJ)
bash benches/interp/rqeval/build.sh _rqeval_none overrides_none.c
bash benches/interp/rqeval/build_pgo.sh _rqeval_C overrides_none.c -DRQ_INLINE_HELPERS -mllvm -inline-threshold=400
bash benches/interp/rqeval/build_pgo.sh _rqeval_acJ overrides_attrcache.c -DRQ_ATTR_CACHE -DRQ_INLINE_HELPERS -mllvm -inline-threshold=400
# cells
python benches/interp/hook_cg.py --rq /work/w2-interp/rqeval:_rqeval_C --swapall /work/w2-interp/rqeval:_rqeval_C
python benches/interp/isinstance_cg.py --swap /work/w2-interp/rqeval:_rqeval_acG
python benches/interp/attr_cg.py --swap /work/w2-interp/rqeval:_rqeval_acG [--generic-only]
python benches/interp/setattr_cg.py --swap /work/w2-interp/rqeval:_rqeval_acG
python benches/interp/callpaths_cg.py --swap /work/w2-interp/rqeval:_rqeval_acJ:CALL_KW_NON_PY
python benches/interp/rqeval/kwtest.py; python benches/interp/rqeval/kwtest.py _rqeval_acJ   # diff
# whole runs (exact Ir), full regrtest, races, hit rates
W="..." V="--variant plain --variant swap:_rqeval_acJ:CALL_ISINSTANCE,LOAD_ATTR_FAMILY,CALL_NON_PY_GENERAL,CALL_KW_NON_PY" \
  bash benches/interp/run_wlcg.sh out 8-19
bash benches/interp/rqeval/run_tests.sh plain out; bash benches/interp/rqeval/run_tests.sh noop:_rqeval_acJ out
bash benches/interp/rqeval/run_tests.sh swap:_rqeval_acJ:CALL_ISINSTANCE,LOAD_ATTR_FAMILY,CALL_NON_PY_GENERAL,CALL_KW_NON_PY out
python benches/interp/rqeval/cmp_regrtest.py out
python benches/interp/rqeval/ft_race.py _rqeval_tnone ALL --dir /work/w2-interp/rqeval-t   # 3.15.1t venv
RQMOD=_rqeval_acJ python benches/interp/rqeval/ac_hits.py <workload.py> --family
# end-to-end batches used for this doc: benches/interp/rqeval/batch_ac{F,G,J}.sh
```
