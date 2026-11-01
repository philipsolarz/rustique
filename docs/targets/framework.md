STATUS: wave 1, accel-core workstream (2026-11-01). Implements docs/ACCELERATION.md §3–§6, §9.
Code: `crates/rustique-py/src/accel/`, `crates/rustique-py/src/targets/`,
`python/rustique/_takeover/`. Tests: `tests/test_accel_*.py`, `tests/targets/`.

# The acceleration framework: how to write a target

A target is a `TargetSpec` plus kernels. The registry installs it when the switch is on
(at `enable()` for builtins and static types, when its module finishes importing for module
targets), all-or-nothing, after a version gate and the target's own `prepare`. Everything
below is the stable API (additive changes only).

## 1. Register

```rust
use crate::accel::{self, Counters, DepFlag, Original, Prepare, Reason, Refusal, SlotName, TargetSpec, Tools};

static COUNTERS: Counters = Counters::new();

pub static DUMPS: TargetSpec = TargetSpec {
    name: "json.dumps",            // stable dotted name, unique
    group: "json",                 // the unit users switch (`only=["json"]`); `_x` = hidden
    module: Some("json"),          // install when `json` is imported; None = at enable()
    versions: &[(3, 14), (3, 15)], // validated minors (others refuse: "version")
    free_threaded: true,           // validated on 3.14t/3.15t too
    counters: &COUNTERS,
    prepare: prepare_dumps,
    jit_safe: true,                // false: refused while the JIT is on (vectorcall fallbacks)
    checkable: true,               // exact subset runs no user code: check mode may re-run
    ..TargetSpec::DEFAULT          // always: future fields stay additive
};
```

Add the group's `TARGETS: &[&TargetSpec]` to `targets::ALL` (`crates/rustique-py/src/targets/mod.rs`).

## 2. `prepare`: validate, then declare

`prepare(py, cx: &mut Prepare) -> Result<(), Refusal>`. Nothing is swapped until it returns
`Ok`; then every declared site is compare-and-swapped from the original captured at
declaration, in order, and rolled back if one fails ("conflict"). Pins activate with the
sites.

Module targets may be prepared **inside CPython's `sys.modules` dict watcher** (lazy
activation): no imports, no Python code, no GIL release. Use the plain-dict lookups:

| need | call |
|---|---|
| the module being installed | `cx.module()?` |
| a module global (never `__getattr__`) | `cx.module_attr("dumps")?` |
| another module, only if already imported | `cx.loaded_module("json.encoder")?` (refuses "deps" otherwise) |
| `dict[key]` / optional | `cx.get(&d, "k")?` / `cx.get_opt(&d, "k")?` |
| a class's own `__dict__` entry | `cx.type_attr(&ty, "default")?` |
| builtins as `LOAD_GLOBAL` sees them | `cx.builtins_of(&globals)?` |

Sites (the kernel's C arity is checked against `ml_flags` / the slot):

| kind | call | kernel falls back with |
|---|---|---|
| `ml_meth` (builtin methods/functions, C accelerators; patch every alias) | `cx.ml_meth(&obj, kernel, &ORIG)?` or `unsafe { cx.ml_meth_def(what, def, kernel, &ORIG) }` | `ORIG.get()(…)` (`static ORIG: Original<ffi::PyCFunctionFastWithKeywords>`) |
| type slot | `cx.slot(&ty, SlotName::TpRepr, kernel, &ORIG)?` (refuses `dict` `mp_(ass_)subscript`; checks the wrapper descriptor) | `ORIG.get()(…)` |
| vectorcall of a pure-Python function (module or class `__dict__`) | `cx.vectorcall(&func, kernel, &accel::FUNCTION_VECTORCALL)?` | `accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw)` |

Validation:

- `cx.fingerprint_code(&func, &[(3, 14, "…"), (3, 15, "…")])?` — canonical code hash of a
  Python original (or a dependency); `cx.fingerprint_c(&obj, …)?`,
  `unsafe { cx.fingerprint_def(what, def, …) }` — `ml_name`/`ml_flags`/`ml_doc` of a C
  original. Missing entry for the running minor → "version"; mismatch → "fingerprint".
  Get the values with `python -m rustique._takeover._dev fingerprint posixpath.join`, or
  `… _dev report --only json --import json` (runs your `prepare` in record mode).
- `cx.self_test("what", |py| Ok(kernel_result == original_result))?` → "selftest".
- `cx.symbol(c"_PyObject_GetMethod", &CELL)?` resolves an exported internal symbol (refuses
  "symbol" when missing; validate the signature per minor yourself).
- Errors: `Err(refuse!(Layout: "…{x}"))`, `Refusal::with(Category::Deps, "…")`,
  `Refusal::new("…")` / `?` on a `PyResult` ("other"). Categories: version, build, jit,
  fingerprint, layout, conflict, selftest, symbol, deps, other (`status()[…]["refusal"]`).

Dependency pins (one relaxed load per call: `if !DEPS.ok() { fallback(Reason::Deps) }`):

| what the original reads | pin |
|---|---|
| a module global | `cx.pin(&g, "_get_sep", &DEPS)?` |
| a global that must stay absent | `cx.pin_absent(&g, "len", &DEPS)?` |
| what `LOAD_GLOBAL name` resolves (global, else builtin) | `cx.pin_global(&g, "len", &DEPS)?` |
| a class attribute along the MRO | `cx.pin_attr(&ty, "default", &DEPS)?` |
| a whole dict (instance attributes incl. absent keys, a class dict) | `cx.pin_dict(&obj_dict, &DEPS)?` |
| a value legitimately replaced at run time | `accel::guard::watch(&d, "key", &GEN)` (an `AtomicU64` generation: revalidate by value when it moves) |
| a fact derived from a small dict (any key) | `accel::guard::watch_dict_generation(&d, &GEN)` |
| "does this subclass override any of these names?" | `static C: accel::guard::OverrideCache` (`set_base`, `overrides(ty)`; keyed by `tp_version_tag`) |

The target must validate what it pins (fingerprint, identity with a known object): a pin
records the current object. Flags re-arm when the original state comes back
(`mock.patch` exiting). One dict watcher id and one type watcher id serve all of Rustique.

## 3. Kernels

```rust
unsafe extern "C" fn k_join(f: O, args: *const O, nargsf: usize, kw: O) -> O {
    unsafe {
        if !(kw.is_null() && nargs(nargsf) == 2 && exact_str(*args) && exact_str(*args.add(1))) {
            C.fallback_streak(Reason::Args);
        } else if !DEPS.ok() {
            C.fallback_streak(Reason::Deps);
        } else if !accel::tools_quiet(&TOOLS, f) {     // Python targets only (~40 Ir)
            C.fallback_streak(Reason::Tools);
        } else {
            let r = join2(*args, *args.add(1));
            if C.hit_check_streak() {                   // one byte load when stats/check are off
                return accel::check::verify(&JOIN, r, &[*args, *args.add(1)],
                                            || accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw));
            }
            return r;
        }
        accel::FUNCTION_VECTORCALL.get()(f, args, nargsf, kw)
    }
}
```

- Counters: `hit()`, `fallback(reason)` (+3 Ir off); vectorcall kernels use
  `hit_streak()` / `hit_check_streak()` / `fallback_streak(reason)`: 64 consecutive
  fallbacks (any reason but `Deps`) suspend the target (original vectorcall and specializer
  version restored, `status()` shows `"suspended": "fallback"`) until the next `enable()`.
- A pin that drops suspends a vectorcall target the same way (`"suspended": "deps"`) until
  it re-arms, so a patched dependency costs CPython's own call; >16 suspensions refuse it.
  Both happen in a coalesced `Py_AddPendingCall` (never inside the dict watcher).
- Check mode (`checkable` targets): `hit_check()` / `check::verify(...)`; mismatches count
  as `fallback["check"]` and return the original's result (`"warn"`) or raise
  `RustiqueCheckError` (`"raise"`).
- Tools guard (`accel::tools_quiet`): falls back when any `sys.monitoring` tool listens
  globally to any ungrouped event (PY_START … STOP_ITERATION, RAISE, EXCEPTION_HANDLED,
  PY_UNWIND, PY_THROW, RERAISE; `settrace`/`setprofile`/cProfile/coverage are tools too;
  the verdict is cached per monitoring version) or locally on the original's code object
  (`_co_monitoring`: 11 local events on 3.14, 16 on 3.15, calibrated at `enable()`).
- `accel::in_main_interpreter()` (~30 Ir) only for process-global sites that hold
  per-interpreter objects; better: be interpreter-agnostic (the extension refuses to load in
  subinterpreters, but static `ml_meth`/slot sites are reached from them).

## 4. What the registry does for you

- Version gate: final release, the compiled minor, not a debug build, validated minors and
  free-threading per target; JIT on + `jit_safe: false` → "jit".
- Lazy activation: one dict watcher on `sys.modules` sees importlib's post-exec re-insert
  (DELETE then ADD of the same module object) and installs the module's pending targets;
  already-imported modules are installed at `enable()`; a module still executing waits.
  Nothing is imported to patch it.
- `sys._clear_internal_caches()` after any `ml_meth` install or restore (3.15's tier 2
  copies `ml_meth` into traces; `tests/test_accel_runtime.py` shows the defect without it).
- `disable()` restores every site by compare-and-swap, gives Python functions their
  specializer version back (`_PyFunction_SetVersion`), drops pins.
- The registry lock is a CPython `PyMutex` (detaches while waiting); the `sys.modules`
  watcher only try-locks and queues.

## 5. Python API

The switch primitives live in the extension, `rustique._rustique._accel`, so the startup
path imports only the package and the extension; `rustique.enable/disable/status/stats/
check` (dx) and `rustique._takeover` (native tier, compatibility) are thin wrappers:

- `enable(only=None, exclude=None, builtins=True, quiet=False) -> status() | None` —
  additive, idempotent; names are groups or targets (list or `"a,b"`); unknown names →
  `ValueError` before anything changes; `quiet=True` skips building the status dict (the
  startup hook). Reads no environment variables, imports nothing.
- `disable(names=None, quiet=False)`; `status() -> {"targets": [{name, group, state, reason,
  refusal, sites, versions, checkable, [module, deps_intact, suspended]}], "process":
  {python, hexversion, build, jit, rustique, interpreter, enabled, stats, check, only,
  exclude, native}}`; `stats(enabled=None, reset=False) -> {name: {"hits", "fallback":
  {args, deps, tools, interp, deopt, error, check}, "suspensions"}}`; `check(mode)` /
  `check_mode()`; `RustiqueCheckError`.
- The `RUSTIQUE*` switch (semantics: dx, docs/targets/dx.md; implementation:
  `accel/switch.rs`): `startup()` (the `.pth` path; never raises),
  `switch_enable(only=None, exclude=None, builtins=True, source=...)` (env-aware quiet
  enable behind `rustique.enable()`), `killed()`, `report_to(target)`;
  `status()["process"]["switch"]` = {source, only, exclude, killed_calls, report, killed,
  stats, check, env}, plus `process["free_threaded"]`.
- Diagnostics: `installed(names)`, `targets()`, `fingerprint(obj)`,
  `fingerprint_report(only)`, `pins(name)`, `func_version(f)`, `vectorcall_is_original(f)`.

## 6. Test and benchmark helpers

- `tests/targets/_mechanics.py`: `enabled`, `counting`, `references` +
  `assert_references_accelerated`, `assert_introspection_unchanged`,
  `assert_disable_restores`, `assert_tools_see_original`, `assert_patch_falls_back`,
  `ft_stress` (see `tests/targets/test_accel_mechanics.py`).
- `tests/conformance/run_target_suites.py --targets json --modules test.test_json`:
  per-test-id outcomes plain vs enabled, and the targets installed.
- `benches/targets/_callgrind.py`: `per_call(stmt, setup, enable=[...], jit=False)` (Ir per
  call, region-instrumented, fresh process), `region(stmt, setup)` (one-shot cost).

## 7. Measured

**Per-call costs** (docs/atlas/mechanisms.md §1, unchanged): `ml_meth`/slot fallback +2 Ir,
counters +3 Ir when off, tools guard 34 + ~4 Ir, a dependency flag one relaxed load, a
vectorcall fallback +180–260 Ir (+250–350 under the JIT; hence deopt-while-unpinned and
the fallback streak). Retrofitted kernels: docs/targets/containers.md.

**`enable()`**, callgrind regions, 3.15.1 (`benches/startup_breakdown.py`): 0.36M Ir for
the 13 `containers`+`builtins` targets including `status()` (0.14M; `quiet=True` skips
it); a second `enable()` 0.14M. Lazy activation: one `sys.modules` callback per import
event (a flag load when nothing is pending; a length/bytes compare per pending module name
otherwise).

**Startup** (`benches/startup.py`, whole process, `python -c pass` = 39.7M Ir / 9.8 ms,
3.15.1, fat-LTO release build, CPU-pinned, 40 interleaved runs):

| | callgrind | wall | page faults |
|---|---:|---:|---:|
| wave1 before (eager package and extension), dev-release | import 1.150, +enable 1.168 | 1.145 / 1.161 | |
| release, extension lazy, package `__init__` eager (current dx file) | 1.157 / 1.167 | 1.151 / 1.163 | +144 / +154 |
| release, extension lazy, lean package `__init__` (patch sent to dx) | **1.038 / 1.047** (quiet 1.046) | **1.060 / 1.070** | +66 / +84 |

**The `RUSTIQUE=1` path loads only the extension.** The `.pth` line (exec'd by `site`;
`sitedir` is `addpackage`'s parameter on 3.14 and injected by 3.15's `_exec_imports`)
loads `sitedir/rustique/_rustique<ABI suffix>` by file spec (falling back to `sys.path`
for editable layouts), registers it as `sys.modules["rustique._rustique"]` without
importing the `rustique` package, and calls `_accel.startup()`; OFF values and `-E`/`-I`
load nothing. A later `import rustique` reuses the loaded extension. The extension never
imports itself by name (`PyImport_Import` uses an empty fromlist, which imports the
parent package): it keeps its module object in a static. Tests:
`tests/test_accel_startup.py`. The loader is a string exec'd only when `RUSTIQUE` is set
(`site` compiles every `.pth` line at each start; 3.15's compiler costs ~140k Ir even for
one call, 0.43M for this loader, 1.0M for the first, longer version).

`benches/startup_switch.py`, release wheel, fresh venvs (`uv venv`), 3.15.1, `python -c
pass`, CPU-pinned, 40 interleaved runs, against the same venv without rustique,
**measured**:

| | Ir | wall | page faults |
|---|---:|---:|---:|
| bare venv, rustique installed, `RUSTIQUE` unset | 1.026 | 1.028 | +11 |
| bare venv, `RUSTIQUE=1` | **1.060** | **1.084** | +86 |
| venv with another `.pth`, unset | 1.005 | 1.009 | +6 |
| venv with another `.pth`, `RUSTIQUE=1` | **1.035** | **1.062** | +80 |

The `RUSTIQUE=1` increment (≈1.3M Ir, ≈0.55 ms): compiling the loader 0.43M, dlopen +
`PyInit` of the 5.4 MB extension ≈0.3M plus most of the page faults, module exec ≈0.1M,
`startup()` (environment, 13 targets) ≈0.3M.

What remains in `import rustique` (lean): importlib's finder/loader floor for the package
and an extension (a tiny two-module pure-Python package costs 0.65M Ir), dlopen of the
5.4 MB `.so` (5756 relative + 322 symbol relocations, 0.36M Ir with the extension's
`PyInit`), the extension's exec 0.2M; wall time adds the page faults of mapping and
touching it.

**Next step if the startup bar needs more (ranked; not done in wave 1).** Measured: loading
our 6.8 MB extension costs +0.38 ms / +66 page faults / 0.37M Ir (dlopen + `PyInit`), a
0.8 MB PyO3 extension +0.24 ms / +24 faults / 0.18M Ir; the opt-in engine (containers,
typing, expressions, scalars, C-API; `rustique_core` containers) is ≈ 70% of the code.
Splitting the engine into a second extension loaded on first access of the opt-in API
would save ≈ 0.10–0.14 ms (1–1.4 points of `python -c pass`). Options:
1. **setuptools-rust** as the build backend: several Rust extensions in one wheel natively;
   cost: the dev/CI/`rbox` workflow moves from `maturin develop` to `pip install -e`.
2. **Two distributions** (`rustique` = package + switch extension, `rustique-types` = engine,
   a dependency): clean with maturin on every platform; cost: lockstep versions and a
   versioned capsule ABI (engine type pointers and the builtins kernels' Rustique-container
   entry points registered into the switch when the engine loads; zero cost on builtin
   inputs, one indirect call on Rustique inputs).
3. **An in-tree PEP 517 backend wrapping maturin** that also builds the second cdylib:
   works for native builds, fragile for cross/manylinux/universal2 and sdist builds.
