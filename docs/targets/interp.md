STATUS: wave 2, opt-in tier `interp` (DECISIONS §10.2). Contract: `docs/ACCELERATION.md` with
the opcode-handler site kind (§A below, proposed text). Code: `crates/rustique-interp/` (generated
handler sets, verification, C overrides), `crates/rustique-py/src/targets/interp/` (targets).
Tests: `tests/interp/`. Benchmarks: `benches/interp/` (`cells_cg.py`, `wl_instr.py`,
`regrtest/`). Research: `docs/atlas/w2-interp.md` (prototype `w2/p1-interp`). Every number
is **M** (measured; tool named) or **I** (inferred; source named).

# The `interp` tier: opcode-handler overrides

Turned on with `RUSTIQUE_TIERS=interp` or `rustique.enable(tiers=["interp"])` (accel's opt-in
tiers; until they are merged the targets are the hidden group `_interp`, named in
`enable(only=[...])`). Never on by default.

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

### 2.5 Net-benefit suspension (ACCELERATION §5.6 per override group)

Each group counts executions of its overridden path and fast-path hits. After 16 windows of
65,536 executions (~1M) a group whose cumulative hits are below 1/ratio of its steady
executions (cache fills excluded) restores the core's handlers for the process: attribute
family 5, `object.__setattr__` 64, keyword instantiation 64; `isinstance` never (every path is
at most the core's cost). One decision per install, on a horizon long enough to average out
start-up phases. `_accel._t_interp.info()["groups"]` shows executions, hits, fills and
`suspended`; the registry records the suspension in `status()` once accel's merge B handles
every site kind.

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

## 4. Measurements

RESULTS

## 5. Conformance and gates

GATES

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

APPENDIX
