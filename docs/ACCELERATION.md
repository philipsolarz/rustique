STATUS: contract for wave 1 (2026-11-01), amended at integration with the rulings made
while the targets were built (§3.8, §3.12–14, §4.5, §5.5–8, §8; reasons in `docs/DECISIONS.md` §5).
Every accelerated target follows it. Evidence for each rule: `docs/atlas/mechanisms.md`
(cited as M§n) and `docs/TAKEOVER.md` (T§n). Changing a rule needs a measurement and an
entry in `docs/DECISIONS.md`.

# The acceleration contract

Rustique puts Rust behind CPython's *existing* entry points. A user's objects, functions,
modules and types stay CPython's own; only the code that runs behind an entry point
changes, and only when the result is provably the one CPython computes. Otherwise CPython's
original code runs.

## 1. Vocabulary

- **Target**: one accelerated piece of behaviour with a stable dotted name
  (`float.repr`, `json.dumps`, `copy.deepcopy`, `datetime.strptime`). A target belongs to a
  **group** (`float`, `json`, `copy`, `datetime`, `re`, …), the unit users switch on and off.
- **Site**: one pointer Rustique swaps. Three kinds:
  - `ml_meth` of a `PyMethodDef` (builtin methods and functions, C accelerator functions
    and methods, including methods of heap types: their tables stay static) — kinds A and B;
  - a **type slot** (`tp_repr`, `nb_remainder`, …) of a static type, or of a C
    accelerator's heap type (`_json.make_encoder.tp_call`) — kinds A and B;
  - the **`vectorcall`** of a pure-Python function object, set with
    `PyFunction_SetVectorcall` — kind C (module-level functions and methods in a class
    `__dict__`).
- **Kernel**: the Rust function installed at a site.
- **Original**: the pointer the site held at validation; the kernel's fallback.
- **Guard**: the checks a kernel runs before doing anything observable to decide that the
  call is in its **exact subset**.

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
   never affects the others.
2. **Version gate per target**, before any site is touched (M§9): the running CPython's
   `major.minor` is in the target's validated set and is a final release (no alpha/RC);
   not a debug build; free-threaded builds only if the target is validated there; layouts
   it reads are calibrated (str/int/float/function/code as needed); and the **fingerprint
   of each original** matches the one recorded for that minor version:
   - Python originals: hash of the canonical code object (bytecode, names, varnames,
     flags, argument counts, recursive consts, exception table; path- and line-
     independent);
   - C originals: `ml_name` + `ml_flags` (+ `ml_doc` hash), the slot's current pointer is
     the address CPython's own type/module reports, plus a behavioural self-test on probe
     inputs (kernel vs original) at install.
   Any mismatch **refuses that target** with a reason visible in `rustique.status()`.
   Unknown interpreter → refuse, never guess.
3. **Never import a module to patch it.** Targets whose module is already imported are
   patched at `enable()`; the others when the module finishes executing (one dict watcher
   on `sys.modules` sees importlib's post-exec re-insert, M§7). Static builtin types and
   builtins are patched at `enable()`.
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
    (`PyFunction_SetVectorcall` stops the world: ~1 ms with busy threads, M§5).
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
    one `GroupSpec`, prepared once.
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
6. **Net benefit of vectorcall targets.** A vectorcall fallback costs the caller its
   inlined frame push (§5.2), so a target that mostly falls back must stop. Each hit
   credits a score by `hit_credit`, each fallback (other than a dependency change) debits
   `fallback_debit`; at `−64 × fallback_debit` the registry suspends the target (its
   original vectorcall is restored) until the next `enable()`. Defaults 4 / 1: a target
   stays installed while ≥ 20% of its calls hit; a target sets its own ratio from its
   measured savings and penalty. A changed dependency (§4.2) suspends a vectorcall target
   at once and resumes it when every pin re-arms. Consequence for measurement: the first
   64 calls of an all-fallback stream pay the fallback cost; cells that exceed §5.2 only
   there are listed in the target's doc and in `docs/DECISIONS.md`.
7. **Degraded**: a validation a target can only do lazily (after install, at first use)
   that fails marks the target `degraded` in `status()` (with the reason) and its kernels
   fall back until it is disabled.
8. **Outside the contract** (not observable through a documented API, or nondeterministic
   in CPython itself): the timing of garbage collection and allocation counts; a tool
   attached by another thread while a kernel runs (the call in flight completes natively);
   the exact C-stack depth at which `RecursionError` is raised for deeply nested input
   (the Python-recursion limit is honoured); private state (§2.4).

## 6. Report

1. Each target has counters: `hits` and `fallback[reason]` with reasons `args`
   (outside the exact subset), `deps` (a pinned dependency changed), `tools`, `interp`,
   `deopt` (restart), `error` (error path re-run by the original). Off by default
   (+3 Ir); on with `RUSTIQUE_STATS=1` or `rustique.stats(True)` (+1 Ir).
2. `rustique.status()` returns one entry per target:
   `{"name", "group", "state": "installed"|"pending-import"|"refused"|"disabled",
   "reason": str|None, "sites": [str], "versions": [str]}` plus process facts (CPython
   version and build, Rustique version, startup switch source).
3. Status and counters are data; formatting them for humans (report at exit,
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
   back and re-arms; free-threaded stress (concurrent calls, enable/disable under load)
   has no wrong result.
4. **Benchmarks**: callgrind cells (region-instrumented) for small realistic inputs, the
   fallback paths and the guard-reject paths; pyperf cells for large inputs; the target's
   share of the workload set (`benches/workloads/`). Every cell ≤ 1.05.
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
- Fine-grained: `RUSTIQUE_ONLY=json,copy`, `RUSTIQUE_EXCLUDE=re` (groups or target names);
  same as `enable(only=..., exclude=...)`.
- Visibility: `RUSTIQUE_STATS=1`, `RUSTIQUE_REPORT=1` (report at exit), `rustique.status()`,
  `python -m rustique status|report|diff`; `RUSTIQUE_CHECK=1` runs CPython's original next
  to every fast path of a checkable target and compares (CI).

## 9. Code layout

- `crates/rustique-py/src/accel/` — the framework: sites, registry of targets, version
  gate and fingerprints, guards (tools, dependency pins via dict/type watchers), counters,
  lazy activation, status. Owned by the accel-core workstream.
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
    pub counters: &'static Counters,
    pub prepare: PrepareFn,              // validate and declare the sites (all-or-nothing)
    pub jit_safe: bool,                  // §3.8; default true
    pub checkable: bool,                 // check mode may re-run the original; default false
    pub shared: Option<&'static GroupSpec>, // declarations prepared once per module (§3.12)
    pub hit_credit: u8, pub fallback_debit: u8, // §5.6; defaults 4 / 1
    pub release: Option<fn(Python<'_>)>, // §3.13
}
// `Prepare` offers: module()/loaded_module()/module_attr()/type_attr(), ml_meth(), slot(),
// vectorcall(), pin()/pin_s()/pin_global()/pin_attr()/pin_dict()/pin_absent() -> DepFlag,
// fingerprint_code()/fingerprint_c(), self_test(), symbol(), memo().
// Kernels read their originals from statics the registry fills, use `tools_quiet(code)`
// and the target's `DepFlag`s, and count via `counters`.
```

Targets register by adding their specs to `targets::ALL` (`crates/rustique-py/src/targets/
mod.rs`).
