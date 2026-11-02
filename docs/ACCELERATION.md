STATUS: contract for wave 1 (2026-11-01), amended at integration with the rulings made
while the targets were built (§3.8, §3.12–14, §4.5, §5.5–8, §8; reasons in `docs/DECISIONS.md` §5),
and for wave 2 (2026-11-03; DECISIONS §10): site kinds for slots of stdlib heap types, dynamic
vectorcall sites and opcode handlers (§1, §3.15–17); validate on first call, fingerprint v3,
recorded patch releases and the platform allowlist (§3.2); opt-in tiers (§3.1); free-threading
rules for the framework (§3.10, §3.18); credits v2 (§5.6); asynchronous exceptions (§5.8);
per-process counters, states and fields (§6); the 8-thread rule (§7.4).
Every accelerated target follows it. Evidence for each rule: `docs/atlas/mechanisms.md`
(cited as M§n), `docs/TAKEOVER.md` (T§n) and the wave-2 slices `docs/atlas/w2-*.md`. Changing a
rule needs a measurement and an entry in `docs/DECISIONS.md`.

# The acceleration contract

Rustique puts Rust behind CPython's *existing* entry points. A user's objects, functions,
modules and types stay CPython's own; only the code that runs behind an entry point
changes, and only when the result is provably the one CPython computes. Otherwise CPython's
original code runs.

## 1. Vocabulary

- **Target**: one accelerated piece of behaviour with a stable dotted name
  (`float.repr`, `json.dumps`, `copy.deepcopy`, `datetime.strptime`). A target belongs to a
  **group** (`float`, `json`, `copy`, `datetime`, `re`, …), the unit users switch on and off.
- **Site**: one pointer Rustique swaps. Four kinds:
  - `ml_meth` of a `PyMethodDef` (builtin methods and functions, C accelerator functions
    and methods, including methods of heap types: their tables stay static) — kinds A and B;
  - a **type slot** (`tp_repr`, `nb_remainder`, …) of a static type, of a C accelerator's
    heap type (`_json.make_encoder.tp_call`), or of a **heap type defined in Python by the
    standard library** whose slot holds CPython's `slot_*` dispatcher (`enum.property`'s
    `tp_descr_get`, `EnumType.tp_call`; §3.15) — kinds A and B;
  - the **`vectorcall`** of a pure-Python function object, set with
    `PyFunction_SetVectorcall` — kind C (module-level functions and methods in a class
    `__dict__`; also **dynamic sites**: functions created after install, such as generated
    `__init__`/`__new__`, patched at their first use, §3.16). A Python function reached only
    from C slots (`slot_tp_call`, `slot_nb_*` calling `__call__`, `__or__`, …) loses no
    specialization when its vectorcall is replaced: its fallback costs ~+3 Ir, as for kinds
    A and B (w2-machinery §5);
  - an **opcode handler** — kind D: one entry of the core interpreter's tail-call dispatch
    table (`instruction_funcptr_handler_table`), replaced by a handler generated from the
    running patch release's own `Python/bytecodes.c` by that release's cases generator with
    a Rustique override file. Only in tail-calling builds whose (release, configuration, ELF
    build-id) is recorded (python-build-standalone). Handlers are process-global (every
    interpreter dispatches through the table). Opt-in tier `interp` (DECISIONS §10.2; §3.17).
- **Kernel**: the Rust function installed at a site.
- **Original**: the pointer the site held at validation; the kernel's fallback.
- **Guard**: the checks a kernel runs before doing anything observable to decide that the
  call is in its **exact subset**.
- **Tier**: an opt-in set of targets (`interp`, `gc`), outside the default selection (§3.1).
- **Armed** target: its sites hold first-call stubs; it validates at its first call (§3.2).

## 2. What "identical" means

Within its exact subset a kernel must be indistinguishable from the original for any
program that does not inspect Rustique itself:

1. **Results**: same value, same type (exact type, not a subclass), and the same identity
   wherever CPython returns an existing object (an unchanged `str` returned as itself,
   `True`/`None`/small-int singletons, an argument returned as is).
2. **Errors**: same exception type, message, `args`, notes, `__cause__`/`__context__`. Rule:
   **a kernel never raises its own errors** — when it detects an error condition before
   anything observable happened, it calls the original, which raises. After observable
   work, it raises exactly what CPython would at that point (prove it with tests).
3. **Side effects and their order**: calls into user code (`__repr__`, `__deepcopy__`,
   `default=`, `__format__`, hooks, `__eq__`, `__hash__`), warnings, mutations of
   arguments, I/O and memo/cache contents that are *public* must happen in CPython's order
   with CPython's arguments. If a kernel cannot guarantee that, its guard must exclude the
   input.
4. **Public state**: anything reachable through a documented API must match
   (`functools.lru_cache.cache_info()` of a public cached function, `re.purge()` effects,
   `logging` handler state, `random` stream position). **Private state** (underscore
   names: `_strptime._regex_cache`, `json.encoder.c_make_encoder`'s internal instances,
   per-call temporaries) may differ, provided no public API exposes the difference. Each
   target lists the private state it skips in its doc.
5. **Randomness**: a kernel that consumes randomness consumes the *same source* in the
   *same amounts* (the Mersenne Twister stream position after `random.randint` must be
   CPython's; `uuid4` reads `os.urandom`, and a patched `os.urandom` is honoured through a
   dependency pin).
6. **Types of returned iterators/generators** are part of the result: never replace a
   function that returns a generator; accelerate the helpers it calls instead.
7. **Subclasses and duck types** get CPython's behaviour, normally by falling back. A
   kernel on a slot must send any receiver that is not of the exact type to the original
   (C subtypes inherit the patched slot).
8. **Introspection** of the patched objects is unchanged: identity, `repr`, `__doc__`,
   `__name__`/`__qualname__`/`__module__`, `__text_signature__`, `inspect.signature`,
   `__code__`, `__defaults__`, `inspect.getsource`, pickling by reference, and tracebacks
   (fallbacks run the original's frames).

Anything on by default aims for **zero documented deviations**. A deviation that is
discovered becomes a guard (the input falls back) or a bug.

## 3. Install

1. **One switch, process-wide.** `rustique.enable()` (or the startup switch, §8) is
   idempotent; `rustique.disable()` restores every site. Install is **all-or-nothing per
   target**: either every site of the target is swapped or none is, and a refused target
   never affects the others. **Opt-in tiers** (`TargetSpec::opt_in = Some("interp")`) are not
   in the default selection: a tier's targets install only when the tier is requested
   (`RUSTIQUE_TIERS=interp,gc`, `rustique.enable(tiers=[...])`; the union of both; `none` or
   `0` in the environment turns every tier off even when code asks); `only`/`exclude` still
   apply and accept tier names. An unrequested tier's targets show state `disabled`, reason
   `opt-in tier: RUSTIQUE_TIERS=<name>`.
2. **Gates and validation, before any kernel runs** (M§9).
   - **Process gate**, once: the build's **platform is validated** (64-bit little-endian
     x86-64 or AArch64 on Linux glibc/musl, macOS or Windows: the wheel matrix; anything
     else, such as an sdist built for i686, armv7, s390x or FreeBSD, refuses every target
     with category `platform`); the running CPython's `major.minor` is the one the extension
     was compiled for and a final release (no alpha/RC: `version`); not a debug build
     (`build`); the layouts every target reads are calibrated (`layout`).
   - **Patch-release gate** (DECISIONS §10.5): the running patch release (`3.15.1`,
     `3.15.1t`) is one **recorded** for this build of Rustique: CPython's test modules passed
     with every target that installs there before its fingerprints were recorded
     (`benches/accel/record_patch.py`). An unrecorded release refuses every target with
     category `unvalidated-patch` (fingerprints cannot see C behaviour changes such as
     3.15.2's `tuple_hash` recursion guard); `RUSTIQUE_UNVALIDATED_PATCH=1` installs the
     targets whose fingerprints match a recorded release, reported `"validation":
     "unvalidated"` (else `"recorded"`).
   - **Per target**: its `major.minor` is in the target's validated set; free-threaded
     builds only if validated there; the JIT only if `jit_safe` (§3.8); and the
     **fingerprint of each original** is one recorded for that minor
     (`accel/fingerprints.txt`: a *set* of accepted hashes per target, original and minor):
     - Python originals, format v3: hash of the canonical code object (bytecode, names,
       varnames, flags, argument counts, recursive consts; path- and line-independent),
       **without the docstring** (`co_consts[0]` when `CO_HAS_DOCSTRING`), with the
       **canonical exception table**: the handler (target, depth, lasti) of each instruction
       that can raise a *synchronous* exception (opcode metadata `HAS_ERROR` /
       `HAS_ERROR_NO_POP`, minus `JUMP_BACKWARD`, whose only error is the eval breaker,
       §5.8). Two code objects with equal v3 send every synchronous exception to the same
       handler (w2-floors §5.3);
     - C originals: `ml_name` + `ml_flags` (v3 drops `ml_doc`), the slot's current pointer
       is the address CPython's own type/module reports, plus a behavioural self-test on
       probe inputs (kernel vs original).
     A patch release whose code changed in a way the kernel still covers adds its hash only
     after review (the recorder lists it; until then the target refuses there with
     `fingerprint`). Any mismatch **refuses that target** with a reason visible in
     `rustique.status()`. Unknown interpreter → refuse, never guess.
   - **Validate on first call** (w2-startup §1.7, w2-floors §4). By default a target is
     *armed* at install: `prepare` runs in arming mode (lookups and site declarations only:
     fingerprints, self-tests, memoized validations, pins and layout calibrations are
     skipped) and its sites get **first-call stubs** that forward to the originals. The first
     call of any of its sites validates it in full (`prepare` again, pins, then the kernels
     swapped in by compare-and-swap, or a refusal that restores the originals) and is then
     dispatched to whatever the site holds; while the registry is busy a call runs the
     original and a later call validates. A program pays validation only for the targets it
     calls. Free-threaded builds validate at install (swapping a vectorcall at a first call
     would stop the world in steady state), and so do targets with opcode-handler sites or
     of an arity the stubs do not cover. `RUSTIQUE_VALIDATE=eager` (CI, the test-suite) and
     `_accel._set_validation("eager")` validate at install; `_accel.validate()` validates
     armed targets now. State `armed` in `status()` (§6).
3. **Never import a module to patch it.** Targets whose module is already imported are
   patched at `enable()`; the others when the module finishes executing (one dict watcher
   on `sys.modules` sees importlib's post-exec re-insert, M§7). Static builtin types and
   builtins are patched at `enable()`. On free-threaded builds the watcher only queues the
   module: activation can take other dicts' critical sections and stop the world, and a
   watcher that parks suspends `sys.modules`' critical section in the middle of its
   mutation; the queue is drained by a pending call any thread runs between bytecodes
   (`_PyEval_AddPendingCall`) and by every registry operation.
4. **Compare-and-swap** each site from the original captured at validation. If someone
   else changed a site, refuse the target.
5. **C sites**: replace `ml_meth` only, never `ml_flags` (descriptors chose their
   vectorcall wrapper from the flags). Patch every alias entry (3.15:
   `Pattern.match` and `Pattern.prefixmatch` are separate entries).
6. **Slots**: only slots no specializer compares by pointer. Forbidden on 3.15:
   `dict.mp_subscript`/`mp_ass_subscript` (de-specializes every `d[k]`, M§4). A slot that
   the specializer bypasses for the exact type (`str ==`, `str +`, `s[i]`, `int +`,
   `k in d`, `str(s)`, `for c in s` on 3.15; M§3) brings nothing: don't patch it.
7. **Python functions**: `PyFunction_SetVectorcall` on the function object found in the
   module or class `__dict__`. Never replace module attributes, `__code__`, or class
   `__dict__` entries (M§0: captured references, pickling, introspection break).
8. **JIT**: after installing or uninstalling any `ml_meth` site, call
   `sys._clear_internal_caches()` (3.15's tier 2 copies `ml_meth` into traces, M§4). This
   applies to the existing list/tuple/builtins kernels too. A target that cannot meet §5.2
   under the JIT (vectorcall fallbacks cost +250–350 Ir there) declares `jit_safe: false`
   and refuses with reason `"jit"` when the JIT is enabled in the process.
9. **Uninstall** restores each pointer (CAS) and, for Python functions, the specializer
   version (`_PyFunction_SetVersion(f, code->co_version)`, exported for external JITs;
   if absent, leave it despecialized — correct, slower).
10. **Free-threaded builds**: install at `enable()` or import time, never in steady state
    (`PyFunction_SetVectorcall` stops the world: ~1 ms with busy threads, M§5). A changed
    dependency swaps nothing there: the kernel stays installed, its deps guard routes calls
    to the original, and those fallbacks debit the net-benefit score (§5.6), so a dependency
    that stays changed ends in one suspension (resumed by the next `enable()`). The only
    steady-state swaps are a net-benefit suspension (at most once per target and
    `enable()`) and dynamic sites (§3.16).
11. **Subinterpreters**: the extension module refuses to load in any subinterpreter
    (`Py_mod_multiple_interpreters` = not supported). Sites on process-global tables
    (`ml_meth`, static slots) are reached from every interpreter, so their kernels must be
    **interpreter-agnostic**: they hold no per-interpreter `PyObject`s (only immortal or
    static ones) and take per-interpreter state from their arguments; otherwise they check
    the interpreter (+30 Ir) and fall back. Kernel code and static data live as long as the
    process (they may be called after `disable()`). Per-interpreter targets (Python
    functions, heap types) are accelerated in the main interpreter only.
12. **Install budget** (restated during wave 1; whole-process cells count, not only
    microbenchmarks): enabling a group whose module is already imported costs ≤ 0.1M Ir;
    a lazy install (at the end of the module's import) costs ≤ max(5% of that module's
    import, 0.1M Ir); and `RUSTIQUE=1 python -c pass` stays within §5.2 of plain
    `python -c pass` (wave 1 misses this by 1–3%: an accepted, documented overage,
    `docs/DECISIONS.md` §5). Overages are accepted only by an entry in `docs/DECISIONS.md`.
    Shared declarations (a module's pins and fingerprints used by several targets) go in
    one `GroupSpec`, prepared once. With validation on first call (§3.2) these budgets apply
    to arming; validation is paid at a target's first call (measured with
    `benches/accel/install_costs.py`: lazy installs json +155k, logging +0.69M, pathlib
    +234k Ir on 3.15.1, each within budget; enabling every group at startup 355k).
13. **Exit**: at interpreter exit the registry restores every site, drops every pin and
    stops its watchers before modules are cleared; each target's `release` hook then drops
    the target's own references to Python objects (verified functions, cached callables,
    types), so finalization frees what CPython would free. A kernel reached after its
    release (a daemon thread) declines and calls the original.
14. **Internal symbols and layouts.** A target may use an exported but non-public symbol of
    the interpreter (`_PyObject_GetMethod`, `_PyDict*`, `_PyUnicode_Is*`) through
    `Prepare::symbol`, which refuses the target when the symbol is missing, and may read
    internal struct fields only through a layout calibrated at install for the running
    minor version (refuse on mismatch). One dict watcher id and one type watcher id serve
    every target.

15. **Slots of heap types defined in Python** (`Prepare::heap_slot`, w2-machinery §4.1):
    the type is a mutable heap type whose own `__dict__` defines the slot's dunder as a
    Python function, so the slot holds CPython's `slot_*` dispatcher (cross-checked against
    an independent class defining the same dunder, `same_as`); the site keeps the type
    alive. The kernel sends every receiver whose type is not exactly the patched one to the
    original: a subclass created while the kernel is installed inherits the kernel pointer
    (`inherit_slots`) and keeps it after `disable()` (kernel code lives as long as the
    process, §3.11; the original dispatches correctly for any subclass). Assigning the dunder
    later (on the type or a base) makes CPython rewrite the slot (`update_slot`): the
    original behaviour runs from then on, `status()` shows `"suspended": "conflict"`,
    `disable()` leaves CPython's pointer alone, and the next `enable()` validates afresh.
    The target pins (or fingerprints) the dunder, so a replaced `__get__` is seen at once.
    Nothing in the specializer compares these pointers (generic `LOAD_ATTR` reads
    `tp_descr_get` per call for heap-type descriptors; calls of instances of non-`type`
    metaclasses go through `tp_call`).
16. **Dynamic vectorcall sites** (`accel::dynamic`): functions created after install
    (generated code) get the kernel at their first use or from the target's `on_install`
    hook (an adoption scan of classes created before `enable()`), through
    `PyFunction_SetVectorcall`, only while the target is *live* (installed, validated, not
    suspended). No registry entry per site (it would cost more than the fallback saves on
    cheap entry points): each such kernel checks `Counters::live()` (one relaxed load) and,
    when the target is no longer live (`disable()`, a suspension, exit), gives the function
    CPython's vectorcall and version back (`dynamic::retire`) and calls the original. Until
    its next call a retired function keeps the kernel pointer, which only ever runs
    CPython's code then. `status()` counts the sites (`dynamic_sites`). On free-threaded
    builds each site costs a stop-the-world: a target that cannot meet §7.4 there refuses on
    free-threaded builds.
17. **Opcode handlers** (kind D; the `interp` tier, DECISIONS §10.2; text of the `interp`
    workstream, `docs/targets/interp.md` Appendix A):
    - (a) A target of kind D declares table entries only on a recorded build: the release
      is exactly `Py_Version`, the configuration matches, and the build-id of the object
      holding the interpreter (found through the mapping of `PyEval_EvalCode`) is in the
      target's verified list, which gives the table and per-handler addresses. Every entry
      the target may swap is compared with the recorded core handler before the swap; any
      mismatch refuses (`conflict`); anything else unrecorded refuses (`version`, `build`,
      `platform`).
    - (b) An install-time ABI self-test proves the calling convention in both directions
      (the core's `NOP` handler tail-calls a probe through a private table) before the
      first swap.
    - (c) Never swapped: `INSTRUMENTED_*` opcodes, `ENTER_EXECUTOR`, `JUMP_BACKWARD*`,
      `RESUME_CHECK_JIT`, `TRACE_RECORD`, labels. A build-time check proves no swapped
      handler can reach tier 2 (`__attribute__((error))` on the JIT entry and the trace
      compiler).
    - (d) A generated handler names the core's dispatch and tracing tables
      (`DISPATCH_TABLE`, `TRACING_DISPATCH_TABLE`) and keeps the core's
      `IS_PEP523_HOOKED`, so it re-dispatches and traces exactly as the core's own handler.
    - (e) An override that must see the deopt traffic of specialized forms swaps the whole
      family (deopts jump directly to the generic handler of the same file).
    - (f) Swaps are CAS on aligned pointers (`Prepare::opcode_handler`); `disable()` and
      exit restore the recorded originals; handler code stays mapped for the life of the
      process.
    - (g) The extension must load on every CPython: every interpreter symbol a handler set
      references is weak; non-exported functions are never called (their calling convention
      may have been changed by LTO).
    - (h) One handler set per patch release, generated from that tag; overridden ops and
      copied core helpers are pinned by hash (`overrides.lock`) and generation stops when
      CPython changed one of them.
18. **Free-threading rules for the framework** (DECISIONS §10.9, w2-threads §3.4). No panic
    escapes `enable()` or an import: a panic anywhere in an activation becomes a refusal
    (category `other`, the panic as reason) and whatever was swapped or pinned is rolled
    back. The pin-table lock is a **leaf**: while it is held nothing takes a per-object
    critical section or the type lock, parks or detaches (any contended `PyMutex`, a
    `Py_DECREF` that may queue to another thread or free an object), allocates Python
    objects, runs Python code or stops the world; code that needs both takes the dict's
    critical section first, the order of the watcher callbacks. Pins are read and
    registered under the pinned dict's critical section and dict snapshots taken with
    `PyDict_Next` under it, so no change between the read and the registration is missed;
    class pins keep the MRO tuple they validated. A lock-order check counts violations
    (`_accel._lock_violations()`, asserted zero by the tests). Watcher callbacks never park:
    references they drop are released at the next safe point. The tools guard is one atomic
    word recomputed from a private state array. A suspension is performed by the thread
    whose fallback asks for it (§5.6), not by the main thread's pending calls. `on_disable`
    hooks restore state that is not a site (calibrated internal state, routed slots) when a
    target is disabled or suspended; `on_install` hooks run after install, outside the
    registry lock and every watcher.

## 4. Guard

Decide the exact subset **before anything observable happens**, cheaply:

1. **Exact types and argument shapes**: `Py_IS_TYPE` (never `PyX_Check`), arity, `kwnames`
   NULL or a known set of interned names with values the kernel supports (e.g.
   `json.dumps(obj)` with no keywords, or `indent=int|None`, `sort_keys=bool`…).
2. **Dependencies** the original reads at call time — module globals (`os.fspath`,
   `json.encoder.c_make_encoder`, `copy._deepcopy_dispatch`, `copyreg.dispatch_table`,
   `time.localtime`) and class attributes (`JSONEncoder.default`, `Formatter.converter`) —
   are **pinned** at install: a dict watcher (module dicts) or type watcher (classes)
   flips one `AtomicBool` per target when the pinned object is replaced and re-arms it when
   the original is put back (`mock.patch` exits). Per-call cost: one relaxed load.
3. **Tools** (Python-function targets only): if a tool listens — global `PY_START`
   (`settrace`, `setprofile`, cProfile, coverage) or local monitoring events on the
   original's code object (breakpoints) — call the original, so tools see CPython's frames
   (M§8: 38 Ir for all guards of `posixpath.join`).
4. **Recursion**: a recursive kernel counts depth like CPython (`Py_EnterRecursiveCall`
   semantics) or falls back before recursing past the point where CPython would raise.
5. **Budget**: all guards together ≤ 40 Ir per call, or ≤ 1% of the original's cost when
   that is larger.
6. **Per-class facts** (does this class override a method, what does a member's value map
   to) are cached by the class's version tag (`accel::ClassCache`: tags are never reused in
   a process and reset on any modification of the class or a base); a fact that also
   depends on a dict not owned by the class registers it with
   `guard::watch_dict_invalidates` (weak: no reference held) and is computed under that
   dict's critical section. Opcode-handler overrides (kind D) decide their exact subset
   from the state CPython's own code reads (type version tags, descriptor kinds, dict kinds,
   the callable's identity), before anything observable. Caches are keyed by version tags
   (CPython's method-cache rule) and hold only per-main-interpreter facts; the interpreter
   comes from the handler's `tstate` (`tstate->interp == main`, else CPython's code). On
   free-threaded builds cache entries are single words or seqlocked, hold no dereferenced
   borrowed pointers, and obtain references with the primitives CPython's own generic path
   uses.

## 5. Fallback and "never slower"

1. Fallback = call the **saved original** with the **unchanged** arguments (never the
   patched object, never re-dispatch). A kernel may *restart* in the original only while
   nothing observable has happened; after user code ran, it must continue CPython's
   algorithm from the same state (T§7).
2. **Never slower**: every measured cell ≤ 1.05 — fast paths, fallback paths, small
   inputs, startup. Consequences:
   - `ml_meth` and slot sites cost +2–5 Ir on fallback: always fine.
   - A vectorcall site costs **+180–260 Ir** per fallback call (+250–350 under the JIT):
     callers lose the specializer's inlined frame push (M§1.2). A Python target qualifies
     only if (a) its cheapest realistic fallback call costs ≥ ~5,000 Ir (~7,000 under the
     JIT), or (b) its kernel covers the inputs that make it cheap, so the measured fallback
     cells stay ≤ 1.05. Prefer a C entry point when one exists (`datetime.strptime` is a C
     classmethod that calls `_strptime` — patch the C method: free fallback).
   - Recursive Python targets (`deepcopy`) must handle the recursion inside the kernel —
     a fallback per level costs 1.12–1.21×.
   - Methods reached through type slots (`str(uuid)` via `slot_tp_str`) fall back for
     ~+3 Ir.
   - **The 8-thread rule** (DECISIONS §10.9): on a free-threaded build, CPU time per call
     at 8 threads with the kernel ≤ 1.05× CPython's original at 8 threads, for per-thread
     and shared inputs (§7.4). Global locks, `Arc` clones, heap growth through the C
     allocator and whole-container critical sections on shared arguments are the measured
     ways to break it (w2-threads §3.1).
   - Opcode-handler overrides (kind D): outside the fast path an override runs CPython's
     original op body, verbatim from `bytecodes.c` (with core helpers the core inlines
     copied verbatim); it never re-dispatches. Every cell ≤ 1.05, including each
     override's fallbacks and the specialized forms of a swapped family; whole-run
     instructions ≤ 1.00 on every workload of the reference set.
3. Number-slot kernels return fresh floats (the 3.15 tier-2 optimizer may mutate a
   uniquely referenced float result in place).
4. Kernels read PEP 393 buffers directly; never call `PyUnicode_AsUTF8` on a caller's
   string (it attaches a UTF-8 cache: `sys.getsizeof` changes).
5. **User code from native code** (ruling 15): a kernel never calls user code (hooks,
   overridden methods, `default=`, `__deepcopy__`, `__format__`) at a point where the
   original would have run Python frames in between, because tracebacks, tools, recursion
   depth and the order of side effects would differ. Where the original is Python, the
   kernel restarts in the original *before* anything observable happens when it meets such
   a call (`copy.deepcopy` meeting a `__deepcopy__`). Where the original is C and itself
   calls user code at that point (`_json` calling `default=`), the kernel may do the same.
   An opcode-handler override calls user code only where CPython's C code does, with no
   Python frame in between (a non-data descriptor's `tp_descr_get`,
   `PyObject_GenericSetAttr`); it may push a Python frame inline instead of calling through
   C only as an existing CPython specialized form pushes it for the same kind of callee
   (keyword instantiation = `CALL_ALLOC_AND_ENTER_INIT` + `CALL_KW_PY`): only the C stack
   differs (§5.8).
6. **Net benefit** (credits v2, w2-floors §3). A vectorcall fallback costs the caller its
   inlined frame push (§5.2), so a target that mostly falls back must stop. Each hit
   credits a score by `hit_credit` (set from the instructions a hit saves at the target's
   representative input), each fallback debits `fallback_debit` (the fallback penalty); a
   fallback with reason `abandon` (a restart after kernel work, which is wasted) debits
   `abandon_debit` more. The score is clamped to `[−64 × fallback_debit, 256]`; at its floor,
   once the **decision window** has passed (256 scored calls since the target was armed:
   a burst of fallbacks at startup does not suspend a target that pays off later), the
   target is **suspended** (its original entry points restored, any site kind) until the
   next `enable()`, by the thread whose fallback reached the floor. Defaults 4 / 1 / 1: a
   target stays installed while ≥ 20% of its calls hit. On GIL builds a changed dependency
   (§4.2) suspends a vectorcall target at once and resumes it when every pin re-arms; on
   free-threaded builds it debits like any fallback (§3.10). Consequence for measurement:
   the first 256 calls of an all-fallback stream pay the fallback cost (≤ 256 × 350 Ir once
   per target and process); cells that exceed §5.2 only there are listed in the target's
   doc. Opcode-handler groups (kind D): executions debit, fast-path hits credit (cache
   fills excluded); one decision after ~1M executions; below the group's ratio its original
   handlers are restored for the process and `status()` reports `suspended`.
7. **Degraded**: a validation a target can only do lazily (after install, at first use)
   that fails marks the target `degraded` in `status()` (with the reason) and its kernels
   fall back until it is disabled.
8. **Outside the contract** (not observable through a documented API, or nondeterministic
   in CPython itself): the timing of garbage collection and allocation counts; a tool
   attached by another thread while a kernel runs (the call in flight completes natively);
   the exact C-stack depth at which `RecursionError` is raised for deeply nested input
   (the Python-recursion limit is honoured); private state (§2.4); **asynchronous
   exceptions** (DECISIONS §10.6): signals and `PyThreadState_SetAsyncExc` arriving while a
   kernel runs are delivered when it returns, at the next eval-breaker check: their timing
   is already nondeterministic in CPython (fingerprint v3 ignores the exception-table
   entries of instructions that can only raise them, §3.2).

## 6. Report

1. Each target has counters: `hits` and `fallback[reason]` with reasons `args`
   (outside the exact subset), `deps` (a pinned dependency changed), `tools`, `interp`,
   `deopt` (a restart in the original before any kernel work), `continue` (CPython's
   algorithm finished the call from the kernel's state: nothing done twice), `abandon` (a
   restart after kernel work: wasted, §5.6), `error` (error path re-run by the original),
   `check`; and `suspensions`. Off by default (+3 Ir); on with `RUSTIQUE_STATS=1` or
   `rustique.stats(True)` (+1 Ir).
2. **Per process.** A forked child starts from zero (its counters are reset before any
   Python code runs in it; the kernel state, scores and suspensions, is inherited) and
   `status()["process"]["ppid"]` names its parent. With counters on and
   `RUSTIQUE_REPORT=<P>.json`, every process keeps its counters in a shared mapping of
   `<P>.<pid>.cnt` (Unix), created at the first `enable()` and by each forked child, so
   nothing is lost on `os._exit` or a kill; the layout is documented in
   `accel/counters_file.rs` (magic `RQCNT\0\0\x02`, a 64-byte header with pid, ppid, the
   number of targets, offsets, flags (bit 0: the JSON record was written at exit) and the
   row width, the target names, then one row `[hits, args, deps, tools, interp, deopt,
   error, check, suspensions, continue, abandon]` per target). Windows: the exit-hook JSON
   only.
3. `rustique.status()` returns one entry per target:
   `{"name", "group", "state", "reason": str|None, "tier": str|None, "refusal": str|None,
   "sites": [str], "versions": [str], "checkable", "module"?}`, with `state` one of
   `installed`, `armed` (first-call stubs in place, §3.2), `pending-import`, `refused`,
   `disabled`; `refusal` one of `version`, `build`, `platform`, `unvalidated-patch`, `jit`,
   `fingerprint`, `layout`, `conflict`, `selftest`, `symbol`, `deps`, `other`; plus, when
   they apply, `"validation": "recorded"|"unvalidated"` (installed or armed targets),
   `"suspended": "deps"|"fallback"|"conflict"`, `"degraded"`, `"deps_intact"`, `"shared"`,
   `"dynamic_sites"`. Process facts: CPython version, patch release, build, platform, JIT,
   Rustique version, pid, ppid, the process-level refusal (`interpreter`,
   `interpreter_refusal`), the validation policy (`validate`), the requested tiers, the
   startup switch and the `RUSTIQUE*` variables in effect.
4. Status and counters are data; formatting them for humans (report at exit,
   `python -m rustique status`) is the DX layer's job.

## 7. Obligations of every target (definition of done)

1. **Conformance**: the CPython test modules covering the target (listed in its doc) pass
   with Rustique enabled exactly as without (same pass/fail/skip sets), on 3.15.1, 3.15.1t,
   3.14.9 and 3.14.9t for every validated version. Run them from the matching CPython
   source tree (`CPYTHON_SRC`), with `rustique.enable()` before the tests import the
   modules, and assert from `rustique.status()` that the target was installed.
2. **Differential fuzzing**: Hypothesis tests drive the original and the kernel on the
   same inputs, including inputs outside the exact subset (fallbacks) and hooks with side
   effects, and compare results, exceptions (type, message), side-effect logs, warnings.
3. **Mechanism tests**: captured references (`from m import f` before `enable()`,
   aliases, `partial`, bound methods) are accelerated; identity, pickling, `repr`,
   signature, source unchanged; `disable()` restores; tools see the original
   (Python targets); dependency patching (`mock.patch` of each pinned dependency) falls
   back and re-arms; first-call validation (armed, then installed or refused at the first
   call); free-threaded stress (concurrent calls, enable/disable under load) has no wrong
   result.
4. **Benchmarks**: callgrind cells (region-instrumented) for small realistic inputs, the
   fallback paths and the guard-reject paths; pyperf cells for large inputs; the target's
   share of the workload set (`benches/workloads/`); the **8-thread rule** on 3.15.1t (CPU
   time per call at 8 threads ≤ 1.05× the original, per-thread and shared inputs:
   `python benches/accel/thread_rule.py --targets <group> --cell <name or
   NAME=SETUP::STATEMENT>`, built on `benches/ft/kernel_scaling.py`). Every cell ≤ 1.05.
   Opcode-handler targets (kind D): CPython's full regression suite with the tier on in
   every process equals the same harness without it (per-test outcomes) on each verified
   build; Hypothesis differential suites for the overridden semantics; race tests (table
   writes under threads, enable/disable with frames inside the handlers);
   `sys.monitoring`/`settrace` untouched.
5. **Docs**: `docs/targets/<group>.md` — what is accelerated (exact subset), what falls
   back and why, private state skipped, measured results, version coverage.

## 8. User-facing switches (owned by the DX layer; listed here so targets agree)

- Turn on with no code change: environment variable `RUSTIQUE=1` read at startup by a
  `.pth` line (`_rustique_startup.pth`, on 3.14 and 3.15: a PEP 829 `.start` entry point
  measured 1.72× startup because `site` then imports `pkgutil` and `re`), or
  `python -m rustique app.py`, or `rustique.enable()` in code. With `RUSTIQUE` unset the
  `.pth` line imports nothing; with `RUSTIQUE=1` it loads only the extension.
- Turn off instantly: `RUSTIQUE=0` (also `off`/`false`/`no`/empty) makes every enable path
  a no-op without loading the extension; `rustique.disable()`.
- Fine-grained: `RUSTIQUE_ONLY=json,copy`, `RUSTIQUE_EXCLUDE=re` (groups, target names or
  tiers); same as `enable(only=..., exclude=...)`.
- Opt-in tiers: `RUSTIQUE_TIERS=interp,gc` (in union with `enable(tiers=...)`; `none`/`0`
  turns every tier off).
- Validation: `RUSTIQUE_VALIDATE=eager|first-call` (default `first-call` on GIL builds,
  §3.2); `RUSTIQUE_UNVALIDATED_PATCH=1` installs targets on an unrecorded patch release whose
  fingerprints match a recorded one (§3.2).
- Visibility: `RUSTIQUE_STATS=1`, `RUSTIQUE_REPORT=1` (report at exit), `rustique.status()`,
  `python -m rustique status|report|diff`; `RUSTIQUE_CHECK=1` runs CPython's original next
  to every fast path of a checkable target and compares (CI).

## 9. Code layout

- `crates/rustique-py/src/accel/` — the framework: sites, registry of targets, version,
  platform and patch-release gates and fingerprints (`fingerprints.txt`: the recorded data;
  `opcodes.rs`: generated by `benches/accel/gen_opcode_tables.py`), first-call stubs
  (`stubs.rs`), guards (tools, dependency pins via dict/type watchers, `ClassCache`),
  dynamic sites, counters and the live counter file, lazy activation, status. Owned by the
  accel workstream. Its tools: `benches/accel/` (`record_patch.py` records a patch release,
  `install_costs.py`, `monitoring_tax.py`, `thread_rule.py`).
- `crates/rustique-py/src/targets/<group>/` — one directory per group: its `TargetSpec`s,
  kernels' PyO3/FFI glue, guards. Pure algorithms that need no Python (float formatting,
  JSON escaping, date parsing, regex translation) go to `crates/rustique-core/src/<area>/`
  with `cargo test`s.
- `python/rustique/` — Python API (`enable`, `disable`, `status`, `stats`), startup hooks.
- Tests: `tests/targets/test_<group>*.py`; conformance runner
  `tests/conformance/run_target_suites.py`. Benchmarks: `benches/targets/<group>.py`.

### Target registration (Rust interface, implemented by accel-core)

The authoritative definition is `crates/rustique-py/src/accel/spec.rs` (`TargetSpec`,
`GroupSpec`) and `prepare.rs` (`Prepare`). In short:

```rust
pub struct TargetSpec {
    pub name: &'static str,              // "json.dumps"
    pub group: &'static str,             // "json"; "_x" groups are hidden (tests only)
    pub module: Option<&'static str>,    // import that triggers install; None = at enable()
    pub versions: &'static [(u8, u8)],   // validated minors, e.g. &[(3, 14), (3, 15)]
    pub free_threaded: bool,             // validated on free-threaded builds too
    pub opt_in: Option<&'static str>,    // opt-in tier ("interp", "gc"), §3.1; default None
    pub counters: &'static Counters,
    pub prepare: PrepareFn,              // validate and declare the sites (all-or-nothing)
    pub jit_safe: bool,                  // §3.8; default true
    pub checkable: bool,                 // check mode may re-run the original; default false
    pub shared: Option<&'static GroupSpec>, // declarations prepared once per module (§3.12)
    pub hit_credit: u8, pub fallback_debit: u8, pub abandon_debit: u8, // §5.6; 4 / 1 / 1
    pub release: Option<fn(Python<'_>)>, // §3.13
    pub on_disable: Option<fn(Python<'_>)>, // restore non-site state (disable, suspension)
    pub on_install: Option<fn(Python<'_>)>, // after install, outside locks (adoption scans)
}
// `Prepare` offers: module()/loaded_module()/module_attr()/type_attr(), ml_meth(), slot(),
// heap_slot() (§3.15), vectorcall(), opcode_handler() (§3.17),
// pin()/pin_s()/pin_global()/pin_attr()/pin_dict()/pin_absent() -> DepFlag,
// fingerprint_code()/fingerprint_c() (v3 against fingerprints.txt), self_test(), symbol(),
// memo(), arming() (true while only arming, §3.2).
// Kernels read their originals from statics the registry fills, use `tools_quiet(code)`
// and the target's `DepFlag`s, and count via `counters` (`hit_streak`/`fallback_streak`
// for net benefit; `live()` for dynamic sites, `accel::dynamic`). Per-class facts:
// `accel::ClassCache`.
```

Targets register by adding their specs to `targets::ALL` (`crates/rustique-py/src/targets/
mod.rs`).
