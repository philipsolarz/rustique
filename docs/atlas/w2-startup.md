STATUS: wave 2, Phase 1, slice `w2-startup`, 2026-11-02. Branch `w2/p1-startup`. Prototype
code: `crates/rustique-py/src/{lib.rs,accel/py.rs,accel/registry.rs,accel/fingerprint.rs}` (lean
init) and the `native` cargo feature (`crates/rustique-py/Cargo.toml`, `lib.rs`, `builtins.rs`).
Tools: `benches/startup/`. Raw data: `~/rustique-research/w2/startup/` and `/work/w2-startup/` on
the dev box. Every number is **M** (measured; how and where) or **I** (inferred; from what).

# w2-startup: startup, import, install cost, and the opt-in native types

## Summary

| question | finding | evidence |
|---|---|---|
| Where does `RUSTIQUE=1 python -c pass` spend its +1.80M Ir (3.15.1) / +2.69M (3.14.9)? | **Compiling the exec'd `.pth` loader string: 32% / 44%**. Then the enable (28% / 20%), dlopen (22% / 18%), module exec (5.5%), the exit hook, importlib's `_load`. Wave 1 put ≈ 1.2M on the dynamic loader. That is wrong: the whole dlopen is 0.40M. | §1.1 (M) |
| What do the fixes buy? | Lean init, a top-level import instead of the exec'd string, and an engine built without the native types take the marginal cost from **1.045 → 1.026 Ir** on 3.15.1 and from **1.063 → 1.027** on 3.14.9. Wall time: §1.6. | §1.2–1.5 (M) |
| Which "obvious" fixes do nothing? | `RTLD_LAZY`: +45k Ir (the `.so` is `BIND_NOW`, and Rust's GOT calls are `GLOB_DAT`). `-Bsymbolic`/visibility: nothing to bind (2 exported symbols). DT_RELR: +17k Ir, same page faults, and it needs glibc ≥ 2.36. A pyc-cached bootstrap module: only −0.22M. | §1.2, §1.4 (M) |
| Can every startup cell be ≤ 1.05? | **Ir, now (M):** the marginal cell (1.026 / 1.027) and the RUSTIQUE=1 vs plain cell in venvs with another `.pth` (1.034 / 1.041, I from M parts). **Bare 3.15.1** gets there with the first-call install (1.054 → 1.048, I). **Bare 3.14.9 is out of reach**: any `.pth` alone costs 1.035–1.048 there (CPython's `utf-8-sig` decode imports a codec). Wall: probably ≤ 1.05 with the first-call install (I). The final cells need the quiet box. | §1.6, §6 |
| Install costs (DECISIONS §5 item 4) | Measured with GC off: `import json` +8.0% (re preloaded), pathlib 1.047, copy 1.050, logging +1.21M. Per install: fingerprints 22%, interning 13%, pins 11%. **Fix: first-call (deferred) validation**. It saves 25–46% of startup install work in real short processes and all of it for groups that are installed but never called. | §2 |
| Real programs' startup/import | Class machinery 22%, unmarshal 16%, importlib 13%, finalize 11%, `re.compile` 7.5%, compiling generated source 8.4% (whole-process Ir, mean of 12 programs). Reachable exactly: **a Rust `re` compiler** (upper bound −6.2% mean, −15% on requests, −42% on `import yaml`), import-system leaves (~9.7% reachable), generated-source compile (upper bound −9.3%). | §3 (M, fork of this slice) |
| Opt-in native types | **60% of the extension's code** and **68% of the test suite's time**. They cost every `RUSTIQUE=1` process +0.12M Ir, 29–34 page faults and ~0.1 ms of dlopen, and 24–28% of release build time. No piece speeds up an existing entry point without a code change. **Retire them all**, keeping the rustique-core modules the engine uses. | §4 |

**Recommendations, ranked** (each prototyped and measured unless marked):
1. **The loader is a top-level import, not an exec'd string**: −0.46M Ir on 3.15.1, −1.16M on 3.14.9. Ship the extension as a top-level module `_rustique` and keep `rustique._rustique` as an alias. Packaging: maturin cannot place a top-level extension next to a Python package (§1.3). Repack the wheel in CI (move one file, regenerate RECORD) or switch to setuptools-rust. **Decision for the lead / w2-dx.**
2. **Retire the opt-in native types** (§4): −0.12M Ir, −30 page faults, a 2.3 MB instead of 6.8 MB `.so`, −24–28% build time, −68% test-suite time, and ~38k lines of Rust, ~12k of tests and 1.2k of docs gone.
3. **Lean init** (done on this branch): `_accel` is created lazily, module attributes go through the dict instead of `PyModule::add`, and site labels and fingerprint hex are formatted lazily. Module exec 99k → 4k Ir; enable −35k.
4. **First-call install** (design + measured simulation, §1.7 / §2.3). At enable, only stubs are installed; each target validates (fingerprints, pins) on its first call. Saves 117–215k Ir per short process at startup and more on lazy installs. This is the structural fix for both startup and install costs.
5. **Framework micro-costs** (I): pre-sized pin vectors, interning keyed by `&'static str`, hashing `co_code_adaptive` without the de-specialised copy where it is safe. Worth 10–20% of install work.
6. **Upstream** (document only): the `.pth` `utf-8-sig` decode (§6.1). It is worth 0.6M Ir per process that has any `.pth`, i.e. most real venvs.

---

## 1. Rustique's own startup

**How measured** (all §1): release wheels (`maturin build --release`, fat LTO;
`benches/startup/build_variants.sh`) `pip install`ed into fresh `python -m venv --without-pip`
venvs on the dev box, against an identical venv without Rustique. `python -c pass`,
`PYTHONHASHSEED=0`.
- Ir: callgrind over the whole process (`--instr-atstart=yes`).
- Parts: `benches/startup/decompose.py`. It uses variant venvs plus `--toggle-collect` on
  `_imp_create_dynamic`, `_imp_exec_dynamic`, `startup()` and the exit hook.
- Counters: `perf stat -r 100…300` (instructions/cycles user and kernel, page faults).
- Wall: median of 100–300 interleaved `posix_spawn` rounds.

The first decomposition ran with the box idle (load 0.2). Its wall numbers match Phase 3's
quiet-box cells: plain 9.49 ms against Phase 3's 9.44. Every later run had load average 14–28
from other slices. **Wall numbers after §1.1 are indicative only**; Ir and page-fault counts
are deterministic.

### 1.1 Decomposition: wave 1 as merged (`main` 7f4c939)

| part | 3.15.1 Ir | share | 3.14.9 Ir | share |
|---|---:|---:|---:|---:|
| `.pth` processing with RUSTIQUE unset (vs plain venv; not in the delta below) | 1,033,226 | | 1,923,062 | |
| **compiling the exec'd loader string** + env test (`RUSTIQUE=off` − unset) | **567,805** | **31.6%** | **1,183,469** | **43.9%** |
| locate + importlib `_load` (`spec_from_file_location`, `isfile`, module attrs) | 139,357 | 7.7% | 229,909 | 8.5% |
| create: dlopen + relocations + symbol binding + module object | 397,272 | 22.1% | 481,180 | 17.9% |
| exec: module exec slot (`_accel` with 15 functions) | 98,899 | 5.5% | 164,582 | 6.1% |
| enable: `_accel.startup()` (23 targets installed, 61 pending) | 499,731 | 27.8% | 528,896 | 19.6% |
| exit hook (restores every site) | 43,543 | 2.4% | 44,708 | 1.7% |
| rest (GC traversal and teardown of what the extension keeps alive, watcher events) | 51,901 | 2.9% | 62,158 | 2.3% |
| **RUSTIQUE=1 − unset** | **1,798,508 (1.0453×)** | | **2,694,902 (1.0634×)** | |

Inside **create** (3.15.1; `--toggle-collect=_imp_create_dynamic`, annotated):
- ~225k is symbol lookup (`do_lookup_x`, `_dl_lookup_symbol_x`, `check_match`, `strcmp`, name hashing): 387 imported symbols at ~580 Ir each (310 Python C-API, 77 libc/libgcc).
- ~120k is relocation processing (`_dl_relocate_object`): 7,581 `R_X86_64_RELATIVE` + 384 `GLOB_DAT`.
- `LD_DEBUG=statistics`: 1,204 − 816 = 388 symbol relocations added by the extension.
- ELF facts: `FLAGS BIND_NOW`, `FLAGS_1 NOW`, 3 `JUMP_SLOT` (no PLT: Rust emits GOT-indirect calls), 6.84 MB file, 4.29 MB `.text`, 145 KB `.data.rel.ro`, 191 KB `.rela.dyn`.

Inside **enable** (3.15.1, inclusive): `posixpath` ≈ 250k (group `shared` 120k + 8 functions),
`float` 37k, `builtins` 34k, `list.sort` 24k. By primitive:
- code fingerprints 67k
- `prepare::interned` 66k
- `format!` 66k (site labels 25k; fingerprint hex 20k)
- pins 34k, and the `sys.monitoring` layout probe 27k

**Wall** (base, idle box, `perf stat -r 300`): RUSTIQUE=1 vs unset is **+0.74 ms (1.076×)**:
- user cycles +1.46M (+6.0%)
- kernel cycles +0.53M (+5.0%)
- page faults +123 (950 → 1,073). The load alone is +66 (`load` − `off`) and enable +49.

3.14.9: +1.07 ms (1.106×), faults +149.

### 1.2 The loader: compile vs search (`benches/startup/loader_variants.py`, `loader_compile.py`)

Compile cost of candidate loader strings, measured in-process (callgrind region,
`loader_compile.py`, 3.15.1 / 3.14.9):

| string | chars | 3.15.1 Ir | 3.14.9 Ir |
|---|---:|---:|---:|
| shipped loader | 398 | 552,841 | 582,929 |
| without the OFF/-E tests | 294 | 409,920 | 427,448 |
| `_imp.create_dynamic(SimpleNamespace(...))` | 152 | 252,992 | 268,572 |
| `f(a,b)` | 6 | 111,604 | 116,099 |
| `pass` | 4 | 72,673 | 74,324 |

CPython's compiler has a ~70k Ir fixed cost and ~1.3k Ir per character after that. No
exec'd string can be cheap. On 3.14.9 the whole-process cost of the string (1.18M) is twice
the in-process figure (I: first compile of the process; not decomposed further).

Whole-process variants, same extension (3.15.1, RUSTIQUE=1 − unset, Ir; M):

| variant (the `.pth` gate is unchanged: `import os; os.environ.get('RUSTIQUE', '0') != '0' and …`) | wave-1 ext | lean ext | lean, no native |
|---|---:|---:|---:|
| shipped: `exec("<loader>")` | +1,799,644 (1.0453) | +1,631,407 (1.0411) | +1,507,896 (1.0380) |
| `lazyflags`: shipped + `sys.setdlopenflags(RTLD_LAZY)` | +1,844,603 (1.0464) | | |
| `bootmod`: `__import__('_rustique_boot')` (pyc-cached module holding the loader) | +1,579,057 (1.0398) | +1,416,281 (1.0357) | +1,293,495 (1.0326) |
| `shortexec`: shortest exec'd loader (env tests in Rust) | +1,503,292 (1.0379) | | |
| `engpkg`: `__import__('_rustique_engine._rustique')` (empty package + extension) | | | +1,388,582 (1.0350) |
| **`toplevel`: `__import__('_rustique').startup()`** | +1,329,297 (1.0335) | **+1,167,192 (1.0294)** | **+1,044,905 (1.0263)** |

The same on 3.14.9, lean + no native: shipped +2,313,277 (1.0544) → toplevel **+1,153,094
(1.0271)**; `bootmod` 1.0323, `engpkg` 1.0343.

- **RTLD_LAZY buys nothing.** The extension is linked `-z now` (rustc's full RELRO default),
  and calls into CPython go through the GOT (`GLOB_DAT`), which the loader binds eagerly in
  any mode. Lazy binding would need PLT calls (`-Z plt=yes`, nightly) and partial RELRO.
- **Symbol visibility / `-Bsymbolic`**: the version script already hides everything but
  `PyInit__rustique` and `PyModExport__rustique`. Intra-library references are bound at link
  time, so nothing is left to gain.
- **Top-level import** moves the env tests into Rust. `startup()` already re-checks the OFF
  values and `-E`/`-I` (`switch.rs`). The only behavioural difference: with an OFF value other
  than `0` (`off`/`false`/`no`/empty), or with RUSTIQUE set under `-E`/`-I`, the extension is
  loaded and nothing is enabled. Today nothing is loaded in those cases (dx ruling). The cost
  in those cases is the load, ~0.3M Ir. Either accept that or keep `!= '0'` as the only gate
  test (recommended: these are rare spellings).

### 1.3 Packaging the top-level extension (M: maturin 1.15 on the dev box)

| layout | result |
|---|---|
| `module-name = "_rustique"`, `python-source = "python"` | error: "python-source is set … but the python module at `python/_rustique` does not exist" |
| pure-Rust layout + `python-packages = ["rustique"]` | builds, but maturin wraps the extension in a generated package `_rustique/__init__.py` + `_rustique/_rustique.so`. Its `include` paths also land under `python/` |
| mixed layout, `module-name = "_rustique_engine._rustique"` + `python-packages = ["rustique"]` | builds the right files (`_rustique_engine/{__init__.py,_rustique.so}`, `rustique/`, `.pth` at the root), but importing through a package costs **+0.34M Ir** over a top-level module (`engpkg` above) |

So maturin cannot ship a top-level single-file extension next to a Python package.

**Options:**
- **(a) Repack the wheel in CI.** Unpack, move `_rustique_engine/_rustique.*` to the root as `_rustique.*`, regenerate RECORD. The `.pth` calls `__import__('_rustique')`. An sdist build without the repack gets maturin's package form, which `__import__('_rustique').startup()` also loads, just slower. One step for every platform in `wheels.yml`, `.pyd` on Windows included.
- **(b) setuptools-rust**: `RustExtension("_rustique", …)` is natively top-level. This changes the build backend, `wheels.yml` and the sdist path.

Either way:
- Compatibility: Rust module exec inserts `sys.modules['rustique._rustique']` and `rustique/__init__.py` uses that module, so `rustique._rustique` stays importable. The `MODULE` static already makes a second load reuse the first module (`lib.rs`).
- Platform: on Windows the top-level `.pyd` is found by the same path finder. Unverified: **w2-dx owns the wheel smoke**.
- Name clash risk: `_rustique` at the root of site-packages. Choose a distinctive name.

### 1.4 Smaller engine: the `native` feature (`--no-default-features`)

The prototype gates the native types' registration (`ensure_core`, scalars) and the
builtins kernels' native-container paths (`if !cfg!(feature = "native")`), so fat LTO drops
the rest (M, `benches/startup/so_size.py`, 3.15.1 release):

| | with native types | engine only | change |
|---|---:|---:|---:|
| `.so` file | 6,844,144 B | 2,314,728 B | −66% |
| `.text` / `.data.rel.ro` / `.eh_frame` | 4.29 MB / 145 KB / 367 KB | 1.48 MB / 43 KB / 108 KB | −65% / −70% / −71% |
| relocations RELATIVE / GLOB_DAT | 7,581 / 384 | 2,673 / 304 | −65% / −21% |
| imported symbols (Python C-API) | 386 (310) | 306 (244) | −80 |
| symbol bytes attributed to native types + rustique-core | 2,553 KB (60.3%) | 88 KB (engine's rustique-core modules: json, datetime, paths, random…) | |
| create (dlopen + module), Ir | 397k (3.15.1) / 481k (3.14.9) | 275k / 360k | −122k / −121k |
| page faults, RUSTIQUE=1 process | 1,072 | 1,038 | −34 |
| wall of the dlopen step (`wall_steps.py`, loaded box) | 395 µs | 281 µs | −29% |
| fat-LTO release build (3.15.1 / 3.14.9) | 81 s / 81 s | 59 s / 62 s | −28% / −24% |

**DT_RELR** (`-C link-arg=-Wl,-z,pack-relative-relocs`) shrinks `.rela.dyn` from 191 KB to 9 KB.
The create cost goes 397k → 415k Ir (lean), 275k → 282k (no native). Page faults: 1,078 →
1,075. Decoding the bitmap costs as much as the RELATIVE loop, and the touched
`.data.rel.ro` pages are the same. It also requires glibc ≥ 2.36 at run time (wheels are
manylinux_2_28). **Rejected** (M).

### 1.5 Lean init (prototype on this branch)

| change | effect (3.15.1, M) |
|---|---|
| `_accel` created on first access; `startup` at the extension's top level; module attributes set through the module dict instead of `PyModule::add`. `add` maintains `__all__`: its first lookup raises AttributeError, and PyO3's `PyErr::take` then creates its `PanicException` type, ~40k Ir. | exec 98,899 → 4,314 Ir; locate+`_load` −36k |
| site labels (`format!("{name} ({kind})")`) built by `status()` only | enable −25k |
| fingerprint hex without `core::fmt` | enable −10k |
| **total** | **1,798,508 → 1,628,587 Ir** (3.14.9: 2,694,902 → 2,435,993) |

Tests: the full suite with the lean build, dev-release, 3.15.1 (M): see §4.2. One regression
was found and fixed: `_record`/`_report_at_exit` keep their underscore names.

### 1.6 The startup cells now

Ir ratios (M, callgrind whole process):

| cell (Ir) | 3.15.1 wave 1 | 3.15.1 lean + toplevel + engine only | 3.14.9 wave 1 | 3.14.9 lean + toplevel + engine only |
|---|---:|---:|---:|---:|
| installed, unset (bare venv) | 1.027 | 1.027 | 1.047 | 1.048 |
| **marginal: RUSTIQUE=1 vs unset** | 1.045 | **1.026** | 1.063 | **1.027** |
| RUSTIQUE=1 vs plain, bare venv | 1.073 | 1.054 | 1.114 | 1.076 |
| RUSTIQUE=1 vs plain, venv with another `.pth` (I: unset 1.008 / 1.014 × marginal) | 1.054 | **1.034** | 1.078 | **1.041** |
| + first-call install (I: simulation §1.7, −215k on `-c pass`) | | 1.048 bare / 1.029 other | | 1.071 bare / 1.036 other |

- **Floor** (M): in a bare venv any `.pth` costs 1.020 (3.15.1) / 1.035 (3.14.9), because
  `site` decodes it as `utf-8-sig`, which imports `encodings.utf_8_sig`. The gate's compile
  brings that to 1.027 / 1.048. On 3.14.9 that leaves ~0.1M Ir for everything RUSTIQUE=1
  does, which is not reachable. Most real venvs already carry a `.pth` (virtualenv's
  `_virtualenv.pth`, editable installs, setuptools' `distutils-precedence.pth`); the
  "other `.pth`" row is the representative one.
- **Wall** (indicative; the box was at load 14–28). Marginal RUSTIQUE=1 vs unset:
  - wave 1 on an idle box: 1.076 (+0.74 ms).
  - lean + toplevel + engine only, loaded box: 1.056–1.064 (3.15.1), 1.076–1.088 (3.14.9).
  - Inferred for a quiet box, scaling the idle-box Ir-to-wall relation and the −35% page-fault
    load cost: ≈ 1.045–1.05 on 3.15.1, ≈ 1.05 on 3.14.9. With the first-call install, ≈ 1.035–1.04.
  - The final cells need the quiet box: `benches/dx/startup.py cells`, plus
    `benches/startup/wall_steps.py` and `decompose.py` for the parts.

**Wall residue by step** (`benches/startup/wall_steps.py`, `perf_counter_ns` between steps in a
fresh process, median of 200, cores 16–23, loaded box; M):

| step | lean, known path | engine only, known path | engine only, top-level search | minor faults (engine only) |
|---|---:|---:|---:|---:|
| find (`spec_from_file_location` / full `sys.meta_path` search) | 24.5 µs | 24.4 µs | 118.9 µs | 0 / 1 |
| dlopen (`_imp.create_dynamic`) | 394.6 µs | 281.0 µs | 287.4 µs | 57 → 28 |
| exec | 21.3 µs | 19.3 µs | 19.5 µs | 6 |
| enable (`startup()`) | 305.4 µs | 267.4 µs | 275.4 µs | 33 |

- The **dlopen is the largest wall item**. It is mostly kernel work: mmap, page faults on the
  pages the relocations dirty, file-backed text faults. Its Ir is only ~0.1 ms of it. Retiring
  the native types is the measured lever (−29%).
- The **top-level search costs ~95 µs** more than a known path:
  - It creates the site-packages `FileFinder` (listdir) and runs importlib's Python code. In a
    real program the site-packages finder is created by the first third-party import anyway,
    so only trivial processes pay it in full.
  - It still beats compiling the loader (~0.15 ms on 3.15.1, ~0.3 ms on 3.14.9; I from Ir).
  - A known-path load without a compile is not possible from a `.pth` line: anything beyond the
    gate is either compiled on every start (unset path) or compiled when set.
- **Syscalls** (strace -c, M): the RUSTIQUE=1 process makes 29 more than plain (+5 mmap,
  +1 mprotect, +3 openat, +5 read, +7 stat, …). That is not where the time goes.
- **The `.pth` file itself** (unset path, M): +1.03M Ir / +0.30 ms on 3.15.1 and +1.92M /
  +0.45 ms on 3.14.9 (idle box), of which the codec import is 0.61M / 0.66M (§6.1).

### 1.7 First-call install (deferred validation): the remaining enable cost

**Which eager targets do real short processes call?** (`benches/startup/eager_use.py`;
RUSTIQUE=1 + counters, the 21 `import_startup` children, engine-only build; M)

| target | processes calling it (of 21) |
|---|---:|
| `builtins.max`, `posixpath.join`, `tuple.__contains__` | 21 |
| `builtins.min`, `builtins.sorted`, `list.__contains__`, `list.sort` | 20 |
| `tuple.index` / `builtins.sum` / `builtins.any` / `list.count` | 19 / 18 / 11 / 11 |
| `posixpath.dirname` / `time.strftime` / `posixpath.isabs` / `posixpath.abspath` | 6 / 6 / 4 / 3 |
| `float.repr`, `list.index`, `posixpath.basename`, `posixpath.splitext` | 1 each |
| `builtins.all`, `tuple.count`, `posixpath.relpath`, `posixpath.split` | 0 |

Even bare `-c pass` calls `posixpath.join` 7×: `site`'s `setcopyright` runs after the `.pth`.
A typical short process calls 8–15 of the 23 eager targets.

**Saving if only the called targets were validated** (`benches/startup/deferral_sim.py`:
`_accel.enable(only=S)` regions, engine-only build, 3.15.1; M):

| process | validation Ir | saving vs all 23 (464,348) |
|---|---:|---:|
| bare `-c pass` (3 called) | 249,160 | **215,188 (46%)** |
| `import requests` (11 called) | 310,386 | 153,962 (33%) |
| `import black` (15 called) | 347,364 | 116,984 (25%) |

**Design sketch (not prototyped):**
- At enable, a target in an already-imported module gets a stub at its sites instead of its
  kernel:
  - vectorcall: one generic stub that finds the target by function pointer;
  - `ml_meth`/slots: one const-generic stub per target.
- On the first call the stub:
  1. restores the original;
  2. runs the target's normal `prepare` (fingerprints, pins: C code only, as inside the
     `sys.modules` watcher today, so ruling 15 holds);
  3. installs the kernel or leaves the original;
  4. dispatches.
- The lazy install of modules imported later works the same way. Most of the logging and
  pathlib install costs (§2) are targets that a given program never calls.
- Status shows `installed (validates on first call)`.

**Risks:**
- A function replaced before its first call keeps the stub until `disable()`/exit (they
  restore it).
- On free-threaded builds two threads can race on the first call; the registry lock
  serialises them and the second dispatches.
- `PyFunction_SetVectorcall` stops the world on the t build: allowed in a call, not in the
  watcher.

**Gain:** 0.12–0.22M Ir per short process at startup (0.3–0.5% of `python -c pass`), plus
most of each lazy install. **Recommended for Phase 2** together with §2.3.

---

## 2. Install-time costs (DECISIONS §5 item 4)

### 2.1 Cells after the lean changes (M)

`benches/startup/install_costs.py`: callgrind regions in fresh processes, **GC disabled in
the region**. Collection timing moved cells by ±0.6M Ir between runs; with GC on, `import copy`
lazy measured *below* plain. 3.15.1 release wheels. "lazy" = the import with every group
pending (`_accel.enable()` first), so the lazy install runs inside the import.

| cell | wave 1 Ir | lean Ir | lean ratio / delta | budget (DECISIONS §5.3) |
|---|---:|---:|---:|---|
| `import json` lazy (`re`, `codecs` preloaded) | 4,738,691 | 4,729,645 | **1.079 (+346,798)** | ≤ 5% → over |
| `import json` lazy (cold: imports `re` too) | 22,602,496 | 22,603,799 | 1.017 (+367,752) | ok |
| `import copy` lazy | 4,257,723 | 4,254,738 | 1.050 (+202,667) | at budget |
| `import logging` lazy | 53,438,376 | 53,381,092 | 1.023 (+1,208,195) | ok by ratio, largest absolute |
| `import random` lazy | 3,621,941 | 3,615,067 | 1.045 (+156,632) | ok |
| `import pathlib` lazy | 9,263,697 | 9,237,460 | 1.047 (+414,928) | ok, barely |
| `import fnmatch` / `textwrap` / `statistics` / `urllib.parse` / `dataclasses` | | | 1.019 / 1.011 / 1.013 / 1.010 / 1.021 | ok |
| `import datetime` lazy | 559,102 | 547,212 | 1.278 (+119,189) | over in ratio (`datetime` imports in 0.43M); +0.12M absolute |
| enable `logging` (imported) | 531,225 | 511,459 | | ≤ 0.1M → over |
| enable `copy` / `posixpath` / `fnmatch` / `pathlib` | 223,092 / 289,389 / 343,320 / 545,284 | 217,939 / 276,021 / 334,738 / 520,206 | | over |
| enable `json` / `random` / `dataclasses` | 305,620 / 218,178 / 224,011 | 299,690 / 210,036 / 219,318 | | over |
| enable all, fresh (= RUSTIQUE=1's enable) | 491,702 | 458,997 | | |

The "enable G (imported)" cells include the first-use costs shared by every group: the
`sys.monitoring` layout probe (27k), the watcher, and the interned keys. They are higher than
wave 1's per-group deltas, which were measured after another group was enabled.

### 2.2 Where an install spends (`import logging` with RUSTIQUE=1; callgrind, inclusive; M)

`registry::activate` totals 1.35M Ir:
- `fingerprint_code` 292k (22%)
- `prepare::interned` 171k (13%)
- `guard::activate` (pins) 154k (11%), including `Vec<PendingPin>` growth 43k
- `Prepare::get_opt` 136k
- `pathlib::shared` 293k. `import logging` imports pathlib's dependencies, and pathlib's group installs.
- the logging group: record 129k, function 105k, shared 87k, level 54k, caller 40k, format 37k
- `textwrap` 104k

The exit hook of that process then costs 147k to restore the ~45 sites.

### 2.3 Fixes, by size

| fix | saving (scope) | status |
|---|---|---|
| **first-call validation** (§1.7) for lazy installs too: an import installs stubs (~1k Ir each); each target validates when first called | every target a program never calls: e.g. 4 of 8 posixpath targets and 4 of 6 builtins in typical processes; likely most of the logging/pathlib groups in programs that only import them (I) | design; simulation measured for the eager set |
| fingerprints: hash without `_PyCode_GetCode`'s de-specialised copy (18k of 67k at startup) where the code has not been specialised yet; parse expected fingerprints once into `u64` | ~25% of fingerprint cost (I) | not prototyped |
| interning: `&'static str` keys (no `String` allocation per lookup); skip `PyUnicode_InternFromString` for keys used only for dict lookups | ~40% of `interned` (I) | not prototyped |
| pins: pre-size `Vec<PendingPin>`/`KeySlot` | ~40k Ir per `import logging` (M: `grow_one` cost) | not prototyped |
| lazy site labels, hex without `core::fmt` | 3–7% (M) | done (§1.5) |
| shared GroupSpecs | wave 1 already did this for paths; logging/pathlib groups already use them | settled |

These overlap with **w2-floors** (per-target work). The lead relayed the logging/json/pathlib
numbers on 2026-11-02.

---

## 3. Startup and import of real programs

Measured by this slice's fork; tools `benches/startup/import_pools.py`, `upper_bounds.py`,
`re_corpus.py`, `compile_repeats.py`; raw data `~/rustique-research/w2/startup/imports/`
(`table-*.md`, `perf-*.json`, `ub-*.json`).

**Method:**
- Plain CPython venvs on the dev box. The 13 programs are listed in the table below.
- `perf record -g` with `-X perf` (`instructions:u` and `cycles`, period 1,000,003, ~12k
  samples per program).
- Each sample is attributed to the first "marker" frame walking out from the innermost Python
  frame: module body, import system, `re`, class machinery, site, metadata.
- Native leaves get a second dimension: unmarshal, source compile, `_sre.compile`, extension
  init.
- Classifier validated (M): 300 `re.compile` calls → 99% in the re pool; 60 dataclasses → 100%
  in class machinery.
- A 20k sampling period was throttled by the VM's counters (only 4% of samples were kept) and
  was discarded.

### 3.1 Pools (3.15.1, % of whole-process instructions; mean over 12 programs, bare excluded; M)

| program | Ir (M) | init+process | finalize | importlib | unmarshal | C-ext init | `re.compile` | class machinery | run: stdlib | run: 3rd-party | (source compile, across pools) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| django `setup()` | 739 | 2.9 | 14.3 | 19.7 | 18.6 | 2.5 | 7.4 | 9.9 | 9.1 | 15.4 | 1.3 |
| flask app | 504 | 4.1 | 13.7 | 16.4 | 18.2 | 3.2 | 11.2 | 16.7 | 8.4 | 7.6 | 2.9 |
| fastapi + pydantic app | 1096 | 2.0 | 8.9 | 9.0 | 11.5 | 1.5 | 5.2 | 31.0 | 4.9 | 24.6 | 10.7 |
| sqlalchemy ORM models | 981 | 2.4 | 11.9 | 11.2 | 15.9 | 2.0 | 3.4 | 18.8 | 5.3 | 28.7 | 6.6 |
| pydantic models | 468 | 4.4 | 10.6 | 8.7 | 14.3 | 3.3 | 3.1 | 41.7 | 5.3 | 5.6 | 16.7 |
| `import requests` | 442 | 4.5 | 12.4 | 19.6 | 14.9 | 5.2 | 18.2 | 11.6 | 8.6 | 4.8 | 1.3 |
| `pytest --co` | 1091 | 2.1 | 8.4 | 11.3 | 25.7 | 1.7 | 2.5 | 26.1 | 7.6 | 12.0 | 8.8 |
| `pip --version` | 548 | 3.8 | 11.8 | 13.7 | 18.3 | 2.5 | 14.0 | 16.2 | 8.6 | 10.7 | 7.2 |
| `black --version` | 648 | 3.1 | 7.7 | 8.9 | 10.6 | 2.2 | 6.0 | 16.5 | 3.7 | 40.5 | 5.6 |
| `python -m json.tool` | 181 | 10.0 | 11.1 | 11.2 | 9.3 | 4.4 | 2.3 | 43.4 | 6.8 | 0.0 | 21.5 |
| typer `--help` | 512 | 4.0 | 11.9 | 16.2 | 17.1 | 1.8 | 11.3 | 15.1 | 6.5 | 15.5 | 7.5 |
| rich print | 256 | 7.4 | 12.6 | 14.2 | 16.2 | 2.9 | 6.0 | 16.6 | 6.3 | 17.3 | 10.2 |
| **mean** | | **4.2** | **11.3** | **13.3** | **15.9** | **2.8** | **7.5** | **22.0** | **6.8** | **15.2** | **8.4** |

- site/`.pth`: 0.3%. `importlib.metadata`: 0.5% (max 2.6%).
- Cycles shares differ: importlib 19.5% (stat/listdir/open in the kernel), `re` 4.4%, class
  machinery 17.2%.
- **`import_startup` workload** (21 processes, weighted by Ir): importlib 14.1, unmarshal 16.7,
  `re` 7.4, class machinery 18.3, finalize 12.2.
- **Class machinery by module**: dataclasses 8.4, enum 3.3, typing 3.0, abc 1.9,
  annotationlib 1.7 (17.3 for `import pydantic`), namedtuple 1.6.
- **importlib by function** (mean % of the process): `_call_with_frames_removed` 1.23,
  `FileFinder.find_spec` 1.12, `_path_join` 0.87, `FileLoader.get_data` 0.63,
  `_find_and_load` 0.54, `_ModuleLock.acquire` 0.51, `_path_stat` 0.49.
- **3.14.9**: similar pools, class machinery 23.9%. `import logging` costs 154M Ir on 3.14 vs
  101M on 3.15: traceback → `_colorize` builds dataclasses; 3.15's `lazy import` removed that.
- **GC** (M, `perf stat`):
  - automatic collections during imports: 6.3% mean (2.7–8.4%);
  - the whole finalization: 13.6% (`os._exit` at the end);
  - `gc.freeze()` at the end: −11.0%.
  - This goes to **w2-runtime** (policy tier). Skipping the shutdown collection is not exact.

### 3.2 Upper-bound prototypes (M: replay of results captured in a prior run of the same program; callgrind whole process; verify mode 0 mismatches over 33 programs)

| program | `re`: patterns | Ir saved if compiling cost nothing | generated sources (dataclass/namedtuple) | Ir saved if compiling them cost nothing |
|---|---:|---:|---:|---:|
| django setup | 93 | 5.8% | 37 | 1.8% |
| flask app | 82 | 8.9% | 40 | 4.4% |
| fastapi app | 78 | 3.9% | 101 | 11.5% |
| pydantic models | 11 | 2.1% | 68 | 15.3% |
| `import requests` | 84 | **15.2%** | 28 | 1.8% |
| `pytest --co` | 61 | 2.1% | 121 | 15.1% |
| `pip --version` | 86 | 12.0% | 54 | 6.0% |
| `python -m json.tool` | 13 | 2.1% | 17 | **34.7%** |
| typer `--help` | 97 | 10.3% | 51 | 6.2% |
| **mean of 12** / Ir-weighted | | **6.2%** / 5.6% | | **9.3%** / 8.2% |
| `import_startup` workload (Ir-weighted) | | 6.1% (`import yaml`: 41.9%) | | 4.6% |

An in-process memo of generated source saves only 0–47% of its compile time (mostly
`ForwardRef` strings): ~1.5% Ir (M, `compile_repeats.py`).

### 3.3 Candidates (brief's format)

| candidate | layer | entry points + mechanism | share mean / max (which) | s | exact subset & fallback | floor vs current | risks | upstream | reach | prototype | evidence |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **Rust `re` compiler** | stdlib | vectorcall on `re._compiler.compile`; the kernel parses and compiles in Rust and calls CPython's own `_sre.compile` with **identical arguments** (code list, flags, groupindex, indexgroup) | 6.2% / 15.2% (requests); 6.1% of `import_startup`; 41.9% `import yaml` | 10 (I: 19 µs median per pattern today) | patterns that error or warn, LOCALE, DEBUG → original. Corpus: 515 patterns from these imports + stdlib, none of those; 52 use Unicode IGNORECASE (case-folding tables) | `_sre.compile` + argument building vs pure-Python `re._parser` + `re._compiler` | pin `re._casefix`/`_sre` tables and the `re._parser` constants; error messages and warnings must stay CPython's (fall back on any error) | none known for 3.16/3.17 | R 0.8: 586 module-level compiles in 42 of 83 packages, 227 in the stdlib | upper bound measured (§3.2); differential corpus tool `re_corpus.py` | fork tables `table-upper-bounds-3.15.1.md`, `re-corpus-all-3.15.1.json` |
| **generated-source compile** (dataclass/namedtuple/`_FuncBuilder`) | generated | (a) a template compiler behind `dataclasses._FuncBuilder.add_fns_to_class`, verified against `compile()` per minor; (b) opt-in persistent code cache | 9.3% / 34.7% (json.tool); 4.6% of `import_startup` | (a) 3 (I) | (a) known templates only; anything else → `exec` | `MAKE_FUNCTION` of cached code vs `compile()` | (a) exact code objects (positions, names); (b) on-disk state needs a contract ruling | dataclasses rewrites in 3.14/3.15 (U 0.3) | R 0.8: 365 `@dataclass` in 34 packages | upper bound measured | fork `ub-dc-3.15.1.json` |
| **import-system leaves** | runtime | vectorcall on frozen `_bootstrap_external` functions (`FileFinder.find_spec`, `get_code`/`get_data`, `_path_join`/`_path_split`/`cache_from_source`, `_validate_timestamp_pyc`, module locks). `_find_and_load`/`exec_module` are excluded: they run module bodies (ruling 15) | ~9.7% reachable (13.3% importlib) | 2.5 (I) | plain `str` paths, default finders/loaders; anything subclassed/hooked → original | the syscalls themselves | they are installed eagerly at RUSTIQUE=1 (adds to §1's enable unless first-call); hooks on `sys.path_hooks`/`meta_path` | PEP 810 lazy imports (U 0.3) | R 1.0 | none (I) | fork `table-core-3.15.1.md` |
| GC during imports | runtime | policy tier | 6.3% | – | – | – | timing is outside the contract; opt-in | – | – | measured (§3.1) | → w2-runtime |

Scores (`I × F × (1 − K) × (1 − U) × (0.5 + R)`; the fork's estimates, I):
- `re` compiler: I = 5.6, F 0.6, K 0.15, U 0.1, R 0.8 → **3.3**
- generated-source compile: (a) F 0.3, K 0.25 → **1.9**; (b) F 0.7, K 0.5 → **3.2** (needs a ruling)
- import-system leaves: I = 5.8, F 0.5, K 0.3, U 0.3, R 1.0 → **2.1**

### 3.4 Out of reach, because…

- **unmarshal** (16%): C (`r_object`, `_PyCode_Quicken`, interning). No entry point exists below `marshal.loads`, and replacing the loader is replacing the import system.
- **Interpreter init and finalization, C-extension init** (OpenSSL, `_ast`): no entry point.
- **Kernel time in imports** (stat/open): only a cross-call stat cache would cut it, and it can go stale (not exact).
- **enum/typing/abc/annotationlib**: metaclass hooks call user code (ruling 15).
- **Third-party module bodies**: not targets. `type_new` is 2.1% of the process; that belongs to w2-machinery.
- **`importlib.metadata`**: 0.5% mean (I: scales with the number of installed distributions).

---

## 4. The opt-in native types

### 4.1 What they cost on the default path (M unless marked)

| cost | measure |
|---|---|
| binary | **60.3%** of the extension's symbol bytes (2,553 KB of 4,234 KB); `.so` 6.84 MB → 2.31 MB without them |
| load (every RUSTIQUE=1 process, and every `import rustique`) | +122k Ir (create 397k vs 275k), +34 page faults, +114 µs of the dlopen step (loaded box). `import rustique`: 1,329,555 vs 1,208,445 Ir |
| first use (opt-in only) | `rustique.List` 1,047,620 Ir (all container types + ABC registrations); `rustique.i64` 214,605 |
| build/CI | fat-LTO release build 81 s → 59 s (3.15.1), 81 → 62 s (3.14.9). Per wheel job × 8 platforms × 4 builds in `wheels.yml`. CI's `python` job also builds `examples/rust_extension` and runs mypy/pyright over the 995-line `__init__.pyi` |
| test-suite time | see §4.2 |
| maintenance surface | Rust: 20,793 lines in rustique-py (list, dict, set, frozenset, tuple, expr, capi, scalars, typing, storage, convert, schema + `list/`, `dict/`). rustique-core's native-only modules: ~8,000 of 14,003 lines (column, dtype, expr*, index, kernels*, slice, pyhash, pyfmt). rustique-api: 607. `examples/rust_extension` and examples: 1,304. Python: `_takeover/` 570, `__init__.pyi` 995, lazy API in `__init__.py`. Tests: 25 files, ~12k lines. Docs: ARCHITECTURE.md 776, TAKEOVER.md 466. In total ≈ **38k lines of Rust** next to the engine's 34k |
| builtins kernels | the `builtins` group carries native-container fast paths. Under `--no-default-features` they compile out, leaving 43 KB of 71 KB of the group |

### 4.2 Tests

`pytest -q --durations=0` of the full suite, lean build (native types on), dev-release,
3.15.1, dev box; `benches/startup/test_time_split.py` (M):

| | native-type tests (25 files) | engine tests (targets, accel, dx, takeover-safe, builtins) |
|---|---:|---:|
| tests (`pytest --co`) | 3,656 (63%) | 2,175 (37%) |
| time (sum of setup+call+teardown) | **201.7 s (68%)** | 96.8 s (32%) |
| largest file | `test_differential.py` 134.0 s (Hypothesis state machines on `rs.*`) | `targets/test_json_loads.py` 35.5 s |

The suite passed: **5,819 passed, 12 skipped** in 302.8 s (lean branch, the `_record` fix
included). Retiring the native types removes about two thirds of the suite's time on every CI
job that runs it (I: CI runs the suite on 4 builds).

### 4.3 Does any piece earn its place under "no new APIs"?

| piece | speeds up an existing entry point without code change? | verdict |
|---|---|---|
| `rs.List/Dict/Set/Tuple/FrozenSet` | no: user code must construct them, and they are not `list`/`dict` (`type(x) is list` fails; C extensions see foreign types) | **retire** |
| `X` expressions, `arange` | no: new API | **retire** |
| fixed-width scalars `i8…f64` | no: new API | **retire** |
| C-API capsule + `crates/rustique-api` + `examples/rust_extension` | no: exists to let Rust extensions consume `rs.*` containers | **retire** with the containers |
| `enable(native=...)`, `--native`, `@rs.native`, `python/rustique/_takeover` (source rewriting) | changes which types user code builds. That is a semantic change, not an exact acceleration, and it contradicts the contract (ACCELERATION.md: objects stay CPython's) | **retire** |
| builtins kernels' native-container paths (`sum`/`min`/`max`/`sorted`/`any`/`all` over `rs.*`) | only for `rs.*` inputs | **retire**; the kernels for builtin containers stay |
| rustique-core modules used by the engine (`json::escape/scan/itoa`, `datetime::*`, `paths::*`, `random`, `stats`, `logging`, `textwrap`, `urlparse`, `float_repr`) | yes: these are wave-1 kernels | **keep** (move into the engine crate or keep rustique-core slimmed) |
| `objsort` (CPython's own `list.sort` on a temporary list) | used by `List.sort` and by `sorted` for builtin lists (`list_sort_descriptor`) | keep the part `builtins.sorted` uses |
| `takeover.rs` (the `containers` group: `list.sort/index/count`, `tuple.*`, `__contains__`) | yes, despite its name: it is part of the engine | **keep** |

### 4.4 Retirement sketch

1. **Tag** the last commit with them (`v0.2-native-types`). Record it in DECISIONS.md: why
   (this table), what replaces them (nothing: the engine), and where the code lives (the tag).
2. **Delete:**
   - Rust: `crates/rustique-py/src/{list.rs,list/,dict.rs,dict/,set.rs,frozenset.rs,tuple.rs,expr.rs,capi.rs,scalars.rs,typing.rs,storage.rs,convert.rs (keep exact_int_i64 for datetime),schema.rs}`, `crates/rustique-api`, the native-only modules of rustique-core, `examples/rust_extension`, the native examples.
   - Python: `python/rustique/_takeover/`, the native half of `__init__.pyi`.
   - Tests: the 25 native-type test files, `benches/takeover_compat.py` and related benches.
   - Docs: TAKEOVER.md and the native parts of ARCHITECTURE.md.
   - Code paths: `lib.rs`' `ensure_core`/scalars, the builtins kernels' `rs_*` paths, `_set_native` and the `process["native"]` status field.
3. **Keep:** the engine. `rustique-core`'s engine modules (or move them into the engine crate). `objsort`'s builtin-list path. `takeover.rs`, renamed `containers`.
4. **Python API:** `rustique.enable(native=...)` raises a `TypeError` naming the removal (one release) or is dropped. `rustique.List` etc. raise `AttributeError` with a pointer to the tag. `rustique.native()` is removed.
5. **Docs/migration note** (USAGE.md, README): "Rustique 0.3 removes the opt-in container types. Programs get the same speedups through CPython's own types; pin `rustique<0.3` if you depend on `rustique.List`." PyPI was never published (brief), so there are no external users to migrate. **I: zero downstream breakage.**
6. Afterwards the `native` cargo feature of this branch is unnecessary. It exists only for the §1.4 measurement.

---

## 5. Settled / out of reach

- **Lazy binding (`RTLD_LAZY`)**: no effect on a Rust cdylib on stable Rust (BIND_NOW + `GLOB_DAT`). Settled, M.
- **`-Bsymbolic`, symbol visibility**: nothing to gain; 2 exported symbols. Settled, M.
- **DT_RELR**: no Ir or fault gain; glibc ≥ 2.36. Rejected, M.
- **pyc-cached bootstrap module, empty engine package**: 0.2–0.34M Ir worse than a top-level extension. Rejected, M.
- **RUSTIQUE=1 ≤ 1.05 against a bare 3.14.9 venv**: the `.pth` floor (1.048 unset) leaves ~0.1M Ir. Out of reach for Rustique. It needs the upstream fix §6.1 or a venv that already has a `.pth`.
- **Splitting extensions** (engine + native types as two extensions or distributions): moot once the native types are retired (§4). The split's only benefit, a smaller engine, is the same as retiring them (§1.4).

## 6. Upstream items (document only; not filed)

1. **`site` decodes every `.pth` as `"utf-8-sig"`**, which imports `encodings.utf_8_sig`
   (`Lib/site.py`: 3.15 `_read_pthstart_file`, 3.14 `addpackage`). Measured in a fresh process
   (callgrind region): `b"#x\n".decode("utf-8-sig")` costs **607,007 Ir (3.15.1) / 656,721
   (3.14.9)**. Stripping a BOM by hand and decoding with the fast-path `"utf-8"` costs 4,458 /
   4,586. Whole process with one comment-only `.pth`: 1.0199× / 1.0355× `python -c pass`
   (docs/targets/dx.md §1.3). Every Python process in a venv that has any `.pth` pays this:
   virtualenv/uv venvs, editable installs, setuptools. Suggested fix: `if raw.startswith(codecs.BOM_UTF8): raw = raw[3:]` then `raw.decode("utf-8")`.
2. **PEP 829 `.start` files** import `pkgutil` and `re`: 1.72× startup (docs/targets/dx.md §1.2; carried over).

## 7. Reproduce

Dev box; the variant venvs are built from release wheels (`benches/startup/build_variants.sh
SNAP OUT PY lean nonative relr nonative-relr`, then `pip install` into fresh venvs).

```
python benches/startup/decompose.py PLAIN_VENV RQ_VENV          # §1.1 table, counters, wall
python benches/startup/loader_variants.py PLAIN_VENV RQ_VENV     # §1.2 variants
python benches/startup/loader_compile.py                          # compile cost of loader strings
python benches/startup/so_size.py path/to/_rustique*.so          # §1.4 size attribution
python benches/startup/wall_steps.py VENV [--toplevel]            # §1.6 wall per step
python benches/startup/cgdiff.py a.out b.out                      # diff two callgrind profiles
RUSTIQUE=1 RUSTIQUE_STATS=1 python benches/startup/eager_use.py   # §1.7 which eager targets run
python benches/startup/deferral_sim.py --python VENV/bin/python   # §1.7 saving of first-call install
python benches/startup/install_costs.py --python VENV/bin/python  # §2 install cells
python benches/startup/import_pools.py …; upper_bounds.py; re_corpus.py   # §3 (docstrings)
python benches/startup/test_time_split.py pytest.log              # §4.2
```

Raw data:
- `/work/w2-startup/dec-*.json`, `lv*-*.json`, `inst-*.json` on the dev box, copied to `~/rustique-research/w2/startup/`.
- The fork's data: `~/rustique-research/w2/startup/imports/`.
