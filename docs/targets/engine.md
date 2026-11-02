STATUS: wave 2, Phase 2, workstream `engine`, 2026-11-02. Branch `ws2/engine`. Every number is
**M** (measured; how and where) or **I** (inferred; from what). Raw data:
`~/rustique-research/w2/ws-engine/data/` (`before/` = 9e1eb4a; `final2/` = this branch merged with
wave2 9a74739, accel's merge A included; `final/` = the same before accel's merge); on the dev box
`/work/w2ws-engine/cells-{before,final,final2}/`.

# engine: the native types retired, lean init, top-level `_rustique`, startup cells

## Summary

| item | result | evidence |
|---|---|---|
| Opt-in native types (DECISIONS §10.1) | retired: Rust containers, `X`, scalars, C-API capsule, `rustique-api`, examples, `native=`/`@native`/the literal rewrite, 25 test files, native benches; −52k lines. Removed names raise `AttributeError` with a pointer to docs/MIGRATION.md; `enable(native=)` raises `TypeError`. Version 0.3.0; last native commit tagged `legacy-native-types` | §1.1 |
| Extension size (T8: ≤ 2.5 MB) | **6,844,144 → 2,326,640 B** (3.15.1), 6,843,032 → 2,326,232 B (3.14.9) | §2 (M) |
| `RUSTIQUE=1` vs installed-but-off (T8) | Ir **1.0454 → 1.0281** (3.15.1), **1.0633 → 1.0288** (3.14.9), in bare and other-`.pth` venvs alike | §2 (M) |
| Every startup cell ≤ 1.05? | Ir: installed-but-off, the marginal `RUSTIQUE=1` cell, and every cell but one in venvs with another `.pth` are ≤ 1.05 on both versions. Above 1.05: `import rustique; rustique.enable()` everywhere (status 0.84M + enable 0.63M, accel's), and in **bare** venvs the cells that add to CPython's `.pth` floor (3.15.1: `RUSTIQUE=1` 1.0556, `import rustique` 1.0604; 3.14.9: the documented floor) | §2, §4 (M) |
| Release build, test suite | fat-LTO wheel build 87 → 32 s (3.15.1; loaded box; M); `pytest -q` ~300 → 76–120 s per build | §2, §5 (M) |

## 1. What changed

### 1.1 Retired (the list is docs/MIGRATION.md's)

- Rust: `crates/rustique-py/src/{list,dict,set,frozenset,tuple,expr,capi,scalars,typing,storage,schema,objsort,sync}` and their subdirectories; `convert.rs` keeps only `exact_int_i64` (the datetime kernels use it); `crates/rustique-api`; rustique-core's `column`, `dtype`, `expr`, `index`, `kernels`, `slice`, `pyhash` (`pyfmt` stays as `float_repr`'s test oracle, `#[cfg(test)]`); the `compact_str` and `rayon` dependencies. The builtins kernels' native-container paths went with core's 4a2b693.
- Python: `rustique.List` & co. (the lazy API), `native()`, `enable(native=)`, `rustique._takeover._hook`/`_rewrite` (the `_takeover` switch and `_dev` stay: tests and target authors use them), the native half of `__init__.pyi` (995 → 40 lines).
- Tests, benches, examples: the 25 native test files, `tests/_support.py` and the implementation fixtures, `benches/{bench,list_hotpaths,takeover_compat,takeover_overhead}.py`, the `rs.List` cells of `builtins_overhead.py`; `scoreboard.py` defaults to `--impls enabled`. `examples/` (all native).
- Docs: docs/MIGRATION.md; TAKEOVER.md and ARCHITECTURE.md marked as history; README section replaced by a pointer.

Not mine and still to follow (relayed): `_cli.py`'s `--native`/`--native-main` and USAGE.md (dx).
Until dx's change, `rustique._enable(only, exclude, native, ...)` keeps its positional
`native` (must be falsy). accel's merge A removed `_accel._set_native` and the status field.

### 1.2 Lean init (the prototype's parts outside `accel/`; atlas w2-startup §1.5)

`lib.rs`: the module exec sets four entries through the module dict (`__version__`,
`startup`, `__getattr__`, `__dir__`) instead of `PyModule::add`; `_accel`, `_builtins` and
`_takeover` are created on first access. Module exec: **99,090 → 8,917 Ir** (3.15.1, M).
The prototype's parts inside `accel/` (the dict registration of `_accel`, lazy site labels,
fingerprint hex) were accel's; its merge A brought the first. Before it, `PyModule::add` was
the visible cost of `_accel`'s creation (M, callgrind inclusive, 3.15.1): `add_function` ×16
74k, of which PyO3's `PanicException` type, created by `PyErr::take` after the first
`__all__` lookup fails, 39k; `._accel` went 161k → 84k whole-process (§4).

### 1.3 Top-level `_rustique` (DECISIONS §10.7)

The shipped `.pth` line: `import os; os.environ.get('RUSTIQUE', '0') != '0' and __import__('_rustique').startup()`.

| layout | where | what `import _rustique` finds |
|---|---|---|
| release wheel (repacked by `tools/repack_wheel.py` in CI) | `site-packages/_rustique.<abi>.so` | the extension itself |
| maturin's in-package layout: a wheel built from the sdist, editable installs, the source tree | `site-packages/rustique/_rustique.<abi>.so` + `site-packages/_rustique.py` | the shim, which loads the extension by path (`_frozen_importlib._load`), without importing the package |

- The extension registers itself as `sys.modules['_rustique']` and `['rustique._rustique']`
  at exec, whichever name it was loaded under. A second load of the file (the other name,
  `importlib` by spec) returns the first module: exec puts it back in `sys.modules`, which
  importlib returns. `rustique/__init__.py` imports `_rustique` (with
  `from rustique import _rustique` as the last resort).
- `startup()` sits at the extension's top level. `RUSTIQUE` unset or `0`: nothing is
  imported (the gate). OFF values other than `0` (`off`/`false`/`no`/empty) and `-E`/`-I`:
  the extension is imported and takes itself out of `sys.modules` again, so "nothing is
  loaded" still holds from Python's view (cost: +0.47M Ir in those spellings only; M). ON
  values enable; other values warn.
- `tools/repack_wheel.py WHEEL... [-o DIR]`: moves `rustique/_rustique.<abi>.{so,pyd}` to
  the root, drops the shim, rewrites `RECORD` (sha256, sizes; written last), keeps every
  other entry and its attributes; idempotent; refuses wheels with grafted libraries
  (`*.libs/`, `.dylibs/`: their run-time path is relative to the extension). Standard
  library only. dx runs it in `wheels.yml` after maturin on every platform.
- Proof on the dev box (M): the repacked release wheels `pip install`ed into fresh venvs
  have `_rustique.cpython-31x-x86_64-linux-gnu.so` at the root and no shim; `RUSTIQUE=1`
  loads `['_rustique', 'rustique._rustique']` (one object, `__name__` `_rustique`) and
  installs 23 targets without importing the package. A wheel built by `pip install` from
  the `maturin sdist` installs the shim layout (`rustique/_rustique.<abi>.so` +
  `_rustique.py`): `RUSTIQUE=1` loads the same two names (`__name__` `rustique._rustique`)
  and `import rustique; rustique.enable()` works. §5.

## 2. Startup cells (release wheels, before → after)

**How measured** (M): `benches/startup/release_cells.sh` on the dev box: `maturin build
--release` (fat LTO) of the base commit 9e1eb4a (before) and of this branch merged with
wave2 9a74739 (after),
repacked; `pip`/`uv` installs into fresh `python -m venv --without-pip` venvs, a plain twin
for each; "other `.pth`" = both venvs carry a comment-only `.pth` (what an editable install,
setuptools or virtualenv brings). `benches/dx/startup.py cells`: callgrind whole-process Ir
(median of 3, `PYTHONHASHSEED=0`) and wall medians of 200 interleaved rounds, every ratio
against `python -c pass` in the plain venv. The box was shared (load 14–58 during these runs): **wall
numbers are noise-dominated** (the same cell moved 0.98–1.20 between runs); Ir is deterministic. Python-build-standalone 3.15.1 and 3.14.9.

Ir ratios (wall in parentheses):

| cell | 3.15.1 bare | 3.15.1 other `.pth` | 3.14.9 bare | 3.14.9 other `.pth` |
|---|---|---|---|---|
| installed, unset | 1.0269 → **1.0267** (1.021) | 1.0081 → **1.0079** (0.975) | 1.0472 → **1.0468** (1.065) | 1.0140 → **1.0135** (1.015) |
| `RUSTIQUE=1` | 1.0735 → 1.0556 (1.072) | 1.0539 → **1.0362** (1.082) | 1.1135 → 1.0770 (1.107) | 1.0779 → **1.0426** (1.070) |
| `import rustique` | 1.0672 → 1.0604 (1.048) | 1.0476 → **1.0409** (1.036) | 1.0847 → 1.0771 (1.010) | 1.0502 → **1.0428** (1.104) |
| `import rustique; rustique.enable()` | 1.1040 → 1.0984 (1.081) | 1.0838 → 1.0782 (1.070) | 1.1196 → 1.1132 (1.103) | 1.0840 → 1.0777 (1.129) |
| **`RUSTIQUE=1` vs unset, same venv (T8)** | 1.0454 → **1.0281** (1.050) | 1.0454 → **1.0281** (1.110) | 1.0633 → **1.0288** (1.040) | 1.0631 → **1.0287** (1.055) |

Bold: ≤ 1.05 in Ir. Page faults, `RUSTIQUE=1` − unset (`perf stat -r 200`, M): +123 → +75
(3.15.1), +150 → +94 (3.14.9).

## 3. Decomposition of `RUSTIQUE=1` − unset (before → after)

`benches/startup/decompose.py` (now reads both line formats): callgrind on variant venvs and
`--toggle-collect` on `_imp_create_dynamic`, `_imp_exec_dynamic`, `startup()` and the exit
hook (M):

| part | 3.15.1 before | 3.15.1 after | 3.14.9 before | 3.14.9 after | owner |
|---|---:|---:|---:|---:|---|
| compiling the exec'd loader string | 569,288 | **0** | 1,184,291 | **0** | engine (gone) |
| locate + import (`_load` by path → finder search for top-level `_rustique`) | 138,734 | 216,678 | 224,232 | 212,281 | engine (§4) |
| create: dlopen + relocations + module object | 397,403 | **277,046** | 480,552 | **350,179** | engine (native types gone) |
| exec: module exec slot | 99,090 | **8,917** | 164,362 | **8,742** | engine (lean init) |
| enable: `startup()` | 500,443 | 526,172 | 529,391 | 556,120 | accel (first-call install) |
| exit hook | 44,001 | 44,969 | 44,340 | 45,296 | accel |
| rest: GC, teardown, watcher | 53,671 | 43,022 | 66,008 | 53,641 | accel |
| **total** | **1,802,630 (1.0454)** | **1,116,804 (1.0281)** | **2,693,176 (1.0633)** | **1,226,259 (1.0288)** | |

Before accel's merge A the total was 1,090,226 / 1,199,958 (enable 502,370 / 532,549; `final/`).
The `.pth` file itself (unset − plain): 1.04M (3.15.1) / 1.92M (3.14.9) before, 1.03M /
1.90M after. Most of it is CPython's `utf-8-sig` decode of any `.pth` (0.61M / 0.66M,
atlas w2-startup §6.1).

## 4. What is still above 1.05, and what it would take

**Where `import rustique; rustique.enable()` spends** (`benches/startup/import_parts.py`,
whole-process differences, 3.15.1, other-`.pth` venv; 3.14.9 within 5%; M):

| step | Ir | owner |
|---|---:|---|
| `import _rustique` (search, dlopen 277k, exec 9k, teardown) | 644,536 | engine |
| `._accel` (accel's lean init: 161k before its merge A) | 83,615 | accel |
| `import rustique` (the package; an empty package costs 407k here) | 575,623 | engine (≈ 170k above the floor) |
| `_accel.switch_enable()` (the quiet enable) | 625,716 | accel (first-call install: −215k on `-c pass`, atlas §1.7) |
| the status dict `enable()` returns | 843,232 | accel (`registry::status`) |
| **total over installed-but-off** | **2,772,722** | |

- **`import rustique; rustique.enable()`** (1.078 other `.pth`, 1.098 bare on 3.15.1): to
  reach 1.05 in a venv with another `.pth` it needs −1.1M. `status()` and the enable are
  1.47M of it: a status built from cached per-target parts and the first-call install
  (accel) are the levers; the engine's own share is the package (≈ 0.16M
  above an empty package) and the extension load.
- **Bare 3.15.1, `RUSTIQUE=1`: 1.0556**, 0.006 over. CPython's `.pth` floor takes 1.0267 of
  it, leaving 0.9M for everything `RUSTIQUE=1` does; it does 1.12M. Accel's first-call
  install (−0.215M, atlas §1.7) brings it to ≈ 1.050 (I).
- **Bare 3.15.1, `import rustique`: 1.0604.** Out of reach while the floor stands: the
  floor (1.0267) plus an empty package (0.41M) plus the dlopen (0.28M) is already 1.045;
  the rest is importlib's search and the package body. In venvs with another `.pth` it is
  1.041.
- **Bare 3.14.9: every cell is above 1.05 by the `.pth` floor** (installed-but-off 1.0468;
  w2-startup §1.5/§6.1); the marginal cell (1.028) and the other-`.pth` cells (≤ 1.045 but
  the enable cell) are within.
- **Wall** (indicative): the marginal cell measured 1.040–1.110 on the loaded box (load up
  to 58; cycles ratios of the same cell swing 0.99–1.16 between runs), with
  instructions 1.028 and kernel-side faults +75/+94. The atlas's idle-box scaling put
  1.026 Ir at ≈ 1.045–1.05 wall (I). Final wall cells need the quiet box (Phase 3).

Settled here (M, atlas §1.2/§1.4): `RTLD_LAZY`, `-Bsymbolic`, DT_RELR, a pyc-cached bootstrap
module and an engine package (+0.34M over a top-level module) buy nothing or cost more. The
`.pth` gate line is within 0.1% of every cheaper spelling (docs/targets/dx.md §1.3). An extra
`__all__` set up front in `lib.rs` to dodge the `PanicException` path saved 11k
whole-process (M); accel's dict registration (merge A) since took `._accel` from 161k to
84k.

## 5. Tests

- `tests/test_engine.py` (new): the 0.3 API and the pointers for all 23 removed names, the
  extension's surface, `_takeover` without the native tier, the two names being one module,
  a second load returning the first module, `RUSTIQUE=1` and the OFF values in both wheel
  layouts (run without `site` so an installed `.pth` cannot interfere), the shim's error,
  and the repack script (move, `RECORD`, attributes, idempotence, `-o`, `.pyd`, refusals).
- `tests/test_accel_startup.py`, `tests/test_dx.py`: the new line; `_rustique` and
  `rustique._rustique` loaded, never the package; OFF values and `-E`/`-I` leave nothing.
- Full `pytest -q` (M, dev box, dev-release, this branch merged with wave2 9a74739): 3.15.1
  2181 passed / 5 skipped (92 s), 3.15.1t 2181 / 5 (177 s), 3.14.9 2183 / 3 (107 s),
  3.14.9t 2183 / 3 (152 s) (times on a loaded box; ~300 s per build before); clippy
  `-D warnings`, `cargo fmt --check`, `cargo test --workspace` clean.
- The whole suite also passes **against the installed, repacked release wheels** (fat LTO,
  `pip install` into fresh venvs, tests run from a copy outside the source tree; M):
  3.15.1 2180 passed / 6 skipped, 3.14.9 2182 / 4 (the extra skip is the shim-layout test).
  It found one bug, fixed: `import rustique` masked the extension's own `ImportError` (a
  refused subinterpreter) behind its package-relative fallback.

## 6. Reproduce

```
benches/startup/release_cells.sh SRC OUT PY [PY...]     # wheels, venvs, cells, decomposition
python benches/startup/import_parts.py RQ_VENV [PLAIN]  # §4
python tools/repack_wheel.py dist/*.whl                  # what wheels.yml runs
```
