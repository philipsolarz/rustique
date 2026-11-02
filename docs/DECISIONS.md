# Decision record

Two programs so far. §1–§4: making the builtins Rustique (parity program, October 2026).
§5–§9: wave 1 of the acceleration program, from fast builtins to faster real programs
(November 2026). Each part is a short record of what was decided, what was achieved and what
was not, and what comes next.

# Part 1: making the builtins Rustique (parity program, October 2026)

Evidence: `docs/TAKEOVER.md` (the mechanism, with every candidate measured) and
`docs/BENCHMARKS.md` (the parity scoreboard, pyperformance, real applications). Every number
here is **measured** unless marked *inferred*. Measurements: callgrind instruction counts
for containers of 0–100 elements, pyperf wall time for 10k/1M elements, pyperformance and
three applications, on a dedicated AWS c7a.8xlarge (AMD EPYC 9R14, 32 cores without SMT),
CPython 3.15.1 and 3.15.1t; tests also on 3.14.9 and 3.14.9t.

## 1. The takeover mechanism: accelerate the exact builtins in place

**Decision.** `rustique.enable()` keeps every `list`, `tuple`, `dict`, `set` and
`frozenset` an *exact builtin* and swaps, process-wide, the C function pointers CPython
calls for `list.sort` (hence `sorted`), `list`/`tuple` `.index`/`.count`/`in`, and the
builtin functions `sum`, `min`, `max`, `sorted`, `any`, `all` for Rust kernels. A kernel
runs only where it provably computes CPython's result (exact `int`/`float`/`str`/`None`/
`tuple` elements, no key function, no unusual arguments) and calls CPython's original C
function otherwise. Rustique's own native-storage types (`List`, `Tuple`, `Dict`, `Set`,
`FrozenSet`) stay explicit, with an opt-in per-module rewrite of literals and
comprehensions (`rs.enable(native="pkg.*")`, `python -m rustique --native`, `@rs.native`).

**Why** (TAKEOVER.md §3):

- CPython's specializing interpreter (and the 3.14+ JIT) fast-paths only exact builtin
  types. Even a subclass of `list` that inherits every C slot and does no work costs 2.4×
  the instructions of `list` for `x[i]`, 2.75× for `append`, 4.5× for `a, b, c = x`, and
  2.2–2.6× for `k in d`/`k in s` (3–7× under the JIT). No foreign type can reach ratio
  1.00 on per-element operations, whatever it does internally.
- Rewriting literals to another type costs at least one call per construct: `[]` 2.12×,
  `[a]` 1.49× even with a target that does no work.
- A PEP 523 frame hook costs 1.56× on *every* Python call, process-wide.
- A Rustique type crossing into code that did not opt in breaks 40 of 77 boundary checks
  (`isinstance(x, list)`, `json`, `exec`/`type()` namespaces, `heapq`, PyO3 extraction…);
  a native-storage *subclass* of a builtin returns silently wrong results in 19 (C code
  reads the builtin layout directly). Exact builtins with in-place kernels pass 77/77.
- Exact builtins keep every interpreter fast path, so untouched operations run unchanged
  bytecode: ratio 1.00 by construction, and code compiled before `enable()` (the main
  script, earlier imports, C extensions) is covered automatically.

**Scope and UX.** Process-wide, one call: `import rustique; rustique.enable()`
(`rustique.disable()` restores everything). Native storage remains a per-module opt-in
that can never be the default.

**Risks accepted** (TAKEOVER.md §8): the pointer swaps rely on CPython's static method
and slot tables being writable data and on its calling conventions and object layouts
(all checked at install: `enable()` refuses rather than miscomputes); call it before
starting threads; one private-but-exported symbol (`_PySet_NextEntry`) and a doc-hidden
PyO3 trampoline are used by `Set`.

## 2. What is at parity

**The default tier (`rs.enable()`), against plain CPython:**

| | 3.15.1 | 3.15.1t |
|---|---|---|
| scoreboard, 0–100 elements (instructions) | 3810/3814 cells ≤ 1.05, geometric mean 0.93 | 3814/3814 cells ≤ 1.05, geometric mean 0.93 |
| scoreboard, 10k/1M elements (wall time) | 1741/1846 cells ≤ 1.05, geometric mean 0.88 | 1766/1846 cells ≤ 1.05, geometric mean 0.88 |
| pyperformance, geometric mean of ~120 benchmarks | 1.00× | 1.01× |
| real applications (pyflakes lint, csv/json etl, networkx graph) | 0.963, 0.987, 1.005 | 0.998, 0.993, 1.006 |

Where the kernels apply the gains are large. At 1M elements (wall time, 3.15.1):
`max(floats)` 0.13×, `min(ints)` 0.17×, `ints.sort()` 0.18×, `sorted(ints)` 0.29×,
`x in ints` 0.26×, `ints.count(v)` 0.25×, `sum(ints)` 0.37×; over all 328 cells whose code
path the kernels change, the geometric mean is 0.46× (3.15.1t: 0.48×). The other 1518
cells at 10k/1M run unchanged CPython code: geometric mean 1.004 (3.15.1t: 0.999), median
1.000, with as many cells below 0.95 as above 1.05, i.e. wall-time noise. Every scoreboard
result is checked against plain CPython's (3814/3814 identical per build). pyperformance
and the applications were measured at `2789558`; later commits only reduced startup cost
and routed large builtin sets to CPython (see BENCHMARKS.md).

**Rustique's own types against the builtins** (the explicit / opt-in native tier),
geometric mean of the ratio, baseline → final:

| | 0–100 elements (instructions) | 10k/1M elements (wall time) |
|---|---|---|
| `List` | 2.10 → 1.21 | 1.44 → 0.75 |
| `Tuple` (new) | 1.32 | 0.95 |
| `Dict` | 2.73 → 1.70 | 3.84 → 1.18 |
| `Set` | 2.41 → 1.63 | 4.23 → 0.89 |
| `FrozenSet` (new) | 1.49 | 0.95 |

CPython's own container test suites against the Rustique types: list 61/69 → 67/69, tuple
37/38, dict 34/36 → 35/36, set 43/54 → 52/54, frozenset 31/33. Every remaining failure is
a documented deviation (below).

## 3. What is not at parity, and why

- **Per-element operations on Rustique types** (`x[i]`, iteration, `d[k]`, `k in s`,
  unpacking): bounded below by the interpreter handicap of §1. `List` is now at
  1.0–1.16× of that floor for `x[i]`, `x[i] = v`, `len`, `if x`, iteration; `append` is
  below it (0.7×, `CALL_METHOD_DESCRIPTOR_O` fires for our type). Native storage must
  also box values on reads (an element cache absorbs repeated reads).
- **Small Rustique containers** (0–8 elements): construction, `copy` and `clear` of
  `Dict`/`Set` pay three allocations per hash table (~1000 instructions); 2–5×.
- **Startup.** `import rustique; rustique.enable()` adds 1.7 ms (3.15.1) / 2.2 ms
  (3.15.1t) to a process (`python_startup` 1.17× / 1.18×, down from 1.39× / 1.42× before
  the startup fix): loading an extension module and creating its types. Paid once per
  process; not avoidable short of shipping inside CPython.
- **Garbage collection.** `create_gc_cycles` is 1.03–1.04× slower: the imported module's
  objects add to what the collector traverses (*inferred*). The only other significant
  pyperformance slowdowns are 1.02–1.04× with overlapping noise.
- **`str` ingest into native storage**: `List(list_of_strs)` is 2.3–6.4× at 100–10k
  elements (each string is re-encoded into native storage); object storage would avoid it.
- **Wall-time cells above 1.05 at 10k/1M** (105 / 80 per build): all but 5 run unchanged
  CPython code (noise, see above); the 5 are within 1–2σ of their measured noise. Two real
  regressions were found and fixed during measurement: walking a large builtin set in hash
  order lost to CPython's iterator for `all()` and for `min`/`max` over str/tuple elements
  (up to 1.68×), so sets of ≥ 2048 elements now use the kernels only for numbers; and the
  startup cost above.
- **Instruction-count cells above 1.05** (4 of 3814, 3.15.1 only, all at size 0): they run
  no Rustique code; re-measured in isolation they are 0.93–1.00 (allocator state left by
  the previous cell in the same worker process).
- **Documented deviations of the Rustique types**: `repr` is `List([...])`; they are not
  builtin subclasses, so `json.dumps`, `type(x) is list`, `isinstance(x, list)` and C
  code needing a real `list` reject them (by design, §1); native storage has no element
  identity (`x[0] is x[0]` only after the element cache warms); no instance `__dict__` on
  `Set`; NaN equals NaN in native float columns.

## 4. Ranked next steps toward replacing every builtin

Ranked by expected gain × (1 − risk). All follow the same pattern: exact builtins stay,
Rust kernels go behind the existing C entry points, CPython's code runs otherwise.

1. **`str` methods** (`find`/`count`/`split`/`replace`/`join`, ASCII `upper`/`lower`,
   `startswith`): SIMD search and case mapping on PEP 393 buffers; *inferred* 2–8× on
   long strings (the archaeology measured 7× for ASCII `upper`), parity by construction
   elsewhere. Medium risk: PEP 393 kinds, exact Unicode semantics.
2. **`bytes`/`bytearray` methods** (`find`/`count`/`split`/`replace`/`hex`/`fromhex`/
   `translate`): `memchr`/`memmem`-class kernels; *inferred* 2–8× on large inputs. Low risk.
3. **Large `int` arithmetic** behind `nb_multiply`/`nb_floor_divide`/`nb_power` and
   `int`↔`str` conversion, only above a size threshold: subquadratic algorithms; the
   archaeology measured 4–27× with GMP. Small ints never reach the slot (the specializer
   calls CPython's int functions directly; *inferred* from `Python/bytecodes.c`), so their
   cost is untouched. Medium risk.
4. **`float` → text** (`repr`/`str`/`format`) with shortest-round-trip printing
   (`rustique-core` already has a CPython-exact `pyfmt`): *inferred* 2–3× for
   float-heavy `json.dumps`/printing. Low risk.
5. **Remaining container kernels**: `list ==`/`<`, `list.remove`, `key=` fast paths for
   `min`/`max`/`sorted`, a faster table walk for builtin sets. Expected 1.2–3× on those
   operations; low risk.
6. **Rustique types**: single-allocation hash tables for `Dict`/`Set` (sizes 0–8 from
   2–5× toward ~1.5×), a cheaper iterator `next`, lock-free reads on 3.15t. Low–medium
   risk; benefits the explicit / native tier only.
7. **Native storage as the default** needs an interpreter that specializes foreign
   containers (a specialization protocol or type-version hooks for third-party types),
   i.e. an upstream CPython change. Until then, replacing `list`/`dict` *objects* cannot
   meet the parity bar (§1).

# Part 2: wave 1, faster real programs (November 2026)

Evidence: `docs/IMPACT_ATLAS.md` (where real programs spend their time, the scored
candidates), `docs/ACCELERATION.md` (the contract every target follows),
`docs/targets/<group>.md` (one per group: exact subset, fallbacks, measurements),
`docs/atlas/phase0.md` (the ground truth before wave 1) and `docs/atlas/phase3.md` (the
integrated measurements). Tags: (M) measured, (I) inferred.

## 5. What was decided, and why

**Target real programs, not operations.** The parity program made builtins fast, but a
profile of 35 real workloads (web services, data processing, developer tools) put two
thirds of their time where no in-place kernel can reach: the bytecode interpreter (46%),
memory management and the garbage collector (16%) and dict operations (5.5%) (M,
IMPACT_ATLAS §3). Most of the reachable third is *pure-Python standard library* code
running on *small* inputs, not C builtins on large ones. Wave 1 therefore went after the
stdlib modules with the highest score (time share × achievable speedup × feasibility, minus
risk and overlap with the upstream "Rust for CPython" effort): copy, json, paths, datetime
text, logging and a set of utilities (random, textwrap, urllib.parse, statistics).

**One mechanism family, one contract.** Every target is CPython's existing entry point with
a Rust kernel behind it: the `ml_meth` of a C function or method, a type slot, or the
`vectorcall` of a pure-Python function (`PyFunction_SetVectorcall`). Objects, modules,
functions and types stay CPython's own, so identity, pickling, introspection and captured
references are unchanged, and code imported before `enable()` is covered. A kernel runs only
inside its *exact subset* and otherwise calls the saved original with unchanged arguments;
it never raises its own errors. The rules are `docs/ACCELERATION.md`; a shared framework
(`crates/rustique-py/src/accel/`) implements them once: all-or-nothing install per target,
a version gate and code fingerprints per CPython minor version (unknown interpreter →
refuse), lazy install when a module finishes importing (never import to patch),
dependency pins through one dict watcher and one type watcher (a `mock.patch` of anything
a kernel relies on makes it fall back, and re-arms when restored), a tools guard (tracers,
profilers, coverage and breakpoints see CPython's frames), counters and status, check mode,
JIT cache clearing, subinterpreter refusal, and release of everything at exit.

**Default on, per group, with an instant off.** `RUSTIQUE=1` (a `.pth` line that imports
nothing when the variable is unset) or `rustique.enable()` turns on every group;
`RUSTIQUE_ONLY`/`RUSTIQUE_EXCLUDE` select groups or single targets; `RUSTIQUE=0` makes every
enable path a no-op; `RUSTIQUE_CHECK=1` runs CPython's original next to every checkable fast
path and compares (`python -m rustique diff` for CI). See `docs/USAGE.md`.

**Rulings made while building the targets** (now in `docs/ACCELERATION.md`):

1. **User code from native code** (§5.5, "ruling 15"). A kernel never calls a user hook at
   a point where the original would run Python frames in between: tracebacks, tools,
   recursion depth and the order of side effects would differ. Where the original is
   Python, the kernel restarts in the original before anything observable (copy meets a
   `__deepcopy__`: restart, and the restart is paid for by the share of calls that hit).
   Where the original is C and itself calls user code there (`_json` calling `default=`),
   the kernel may too; json's `default=`/`cls=` users are wave 2's first item.
2. **Net benefit of vectorcall targets** (§5.6). A fallback through a replaced vectorcall
   costs the caller 180–350 instructions (the specializer can no longer inline the frame
   push). Instead of a fixed streak limit, each target keeps a score (hit +4, fallback −1 by
   default, tunable per target from its measured savings and penalty) and is suspended,
   original restored, when the score reaches −64× the debit, i.e. when fewer than ~20% of
   its calls hit. Mixed traffic that still nets a gain keeps the kernel.
3. **Install budget restated** (§3.12). The original "≤ 1.05 on startup" bar could not be
   applied per microbenchmark to install work, which happens once. Restated: ≤ 0.1M
   instructions to enable a group whose module is already imported; a lazy install costs
   ≤ max(5% of that module's import, 0.1M); and whole-process cells (`RUSTIQUE=1 python -c
   pass`, the workload set) count. Shared declarations moved into one `GroupSpec` per
   module, which cut paths' enable cost from 1.98M + 1.92M to 0.22M + 0.28M instructions.
4. **Accepted overages** (each measured on 3.15.1 with callgrind, each with a wave-2 fix):
   - startup with `RUSTIQUE=1`, against the same venv with it unset: 1.045× instructions /
     1.078× wall on 3.15.1 and 1.063× / 1.108× on 3.14.9 (+0.76 / +1.09 ms on
     `python -c pass`, Phase 3); unset ≤ 1.05 everywhere (1.008–1.047). Of the ≈ 1.8M
     instructions (3.15.1, callgrind), ≈ 1.2M is the dynamic loader relocating the 6.8 MB
     extension, mostly binding its 384 imported symbols (`GLOB_DAT`, resolved at load
     whatever the binding mode; 7,580 relative relocations), and ≈ 0.5M is enabling the
     targets of modules imported at startup. The extension also carries the opt-in
     container types; fix: split the engine into its own, smaller extension
     (needs setuptools-rust or a second distribution, since maturin builds one extension per
     wheel) and a precompiled loader. Short-lived processes pay it without gaining
     (`import_startup` 1.022);
   - `import json` +6.5% (budget 5%); `logging` warm enable 0.143M; `copy` 0.162M per group
     and +0.159M on `import copy`; `random` lazy install +0.142M; paths enable posixpath
     0.22M, fnmatch 0.28M, pathlib 0.46M and `import pathlib` +5.5%. The remaining cost is
     code fingerprints and string interning; fix: deferred fingerprinting and cheaper
     interning;
   - cells of an all-fallback stream before the net-benefit score suspends the target
     (the first ~64 calls): `json.loads(invalid)` 1.092, `posixpath.basename`/`isabs` on
     a `PathLike` up to 1.105, `pathlib` `name`/`suffix` on a subclass 1.27–1.28,
     `random` with a Python override up to 1.181, `textwrap`/`urllib` 1.046. The cost is
     bounded in absolute terms (≤ 64 fallbacks × ~300 instructions ≈ 20k instructions,
     a few microseconds, once per target and process) and every steady-state cell is
     ≤ 1.05. Fix where it matters: cover the input in the kernel (pathlib subclasses are
     wave 2).
5. **Outside the contract** (§5.8): garbage-collection timing and allocation counts; a tool
   attached by another thread while a kernel is running; the exact C-stack depth at which
   deeply nested input raises `RecursionError` (the Python recursion limit is honoured);
   private, underscore-named state (`_strptime._regex_cache`, internal encoder instances).
6. **JIT policy** (§3.8). Kernels are re-validated under 3.15's JIT; a target that cannot
   keep its fallback cells ≤ 1.05 there declares `jit_safe: false` and refuses when the JIT
   is on. All wave-1 targets are `jit_safe` (`tuple.count` was shaved from 1.062 to 1.049).
7. **Internal symbols** (§3.14). Exported-but-private CPython functions
   (`_PyObject_GetMethod`, `_PyDictKeys_StringLookup`, `_PyUnicode_Is*`) and calibrated
   internal layouts are allowed, resolved at install and refused when missing or different.
8. **Exit** (§3.13). An exit hook restores every site and drops every pin before modules are
   cleared, and each group releases its own references; without it, kept-alive module
   dictionaries made `RUSTIQUE=1` process teardown cost 0.75M instructions (paths alone).
9. **No engine split in wave 1.** The `RUSTIQUE=1` path loads only the extension (env
   handling lives in Rust); splitting the opt-in types into their own extension is wave 2.

## 6. Speedup on real workloads

Measured in Phase 3 (`docs/atlas/phase3.md`) at `wave1` `11bad29`, fat-LTO release wheels,
on the dedicated AWS c7a.8xlarge: the workload set (`benches/workloads/`, 34 programs in
three families, each a real library or stdlib-heavy task), plain CPython against all 15
groups enabled, in fresh processes, interleaved (`run.py compare all -n 5 -p 5`). Every
workload's result digest is identical plain and enabled (M).

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| **all 34 workloads, geometric mean of time ratios** | **0.880** (1.14× faster) | **0.843** (1.19×) |
| data processing (12) | 0.773 | 0.756 |
| developer tools (12) | 0.963 | 0.863 |
| web services (10) | 0.922 | 0.933 |
| median workload | 0.980 | 0.986 |
| workloads ≤ 0.95 / > 1.05 | 14 / 0 | 13 / 0 |
| best | `report_render` 0.436 (2.3×) | `isort_stdlib` 0.196 (5.1×) |
| worst | `config_merge` 1.013 | `xml_feeds` 1.005 |

The programs that gain are the stdlib-heavy ones (3.15.1): `report_render` 0.436,
`tz_events` 0.591, `log_analytics` 0.594, `csv_etl` 0.625, `state_snapshot` 0.651,
`fs_inventory` 0.743, `isort_stdlib` 0.771, `asyncio_http` 0.791, `logging_service` 0.817,
`json_api` 0.835, `aiohttp_app` 0.910, `requests_local` 0.941, `pytest_networkx` 0.948,
`pip_resolve` 0.949. Framework-heavy programs (Django, Flask, FastAPI, SQLAlchemy, black,
pylint, pyflakes, sphinx, rich, pandas) stay at 0.97–1.01: their time is their own bytecode.
On 3.15.1t, `isort_stdlib` (0.196) and `state_snapshot` (0.476) gain more because CPython's
own `copy.deepcopy` is 2–4× slower on the free-threaded build than on the GIL build, and
the Rust kernel is not (cause in CPython not investigated; M).

A second replicate, run after pyperformance, reproduces the first (M):

- geometric mean 0.879 / 0.843;
- every workload within 2% of its first ratio, 29 / 33 of 34 within 1%;
- `config_merge` at 1.003, so its 1.013 was noise;
- worst workloads, averaged over both replicates: 1.008 / 1.006.

Whole processes, startup and imports included (`RUSTIQUE=1` through the shipped `.pth`,
`pyperf command`, 6 representative workloads): geometric mean **0.756** (3.15.1) and
**0.712** (3.15.1t); `log_analytics` 0.639, `report_render` 0.494, `state_snapshot` 0.749,
`json_api` 0.794, `pytest_networkx` 0.972, and `import_startup` 1.022 (21 child processes,
each paying the startup cost below).

Against the atlas' prediction (IMPACT_ATLAS §1, *inferred* there): "stdlib-heavy programs
gain 1.3–2×" → measured 1.2–2.3×; "the mean workload spends ~10% less time" → 12% (3.15.1);
"framework-heavy web apps ~1.0–1.05×" → 1.00–1.10×.

**pyperformance 1.14.0** (full default suite, ~120 benchmarks, plain vs `rustique.enable()`
through `sitecustomize`, two replicate pairs per build): geometric mean **0.942 / 0.940**
on 3.15.1 and **0.956 / 0.957** on 3.15.1t (Phase 0 at `5abd54c`: 1.002), median 0.998.
Averaged per benchmark over both pairs (122 benchmarks, port benchmarks from their solo
re-run): **0.942** and **0.957**, against Phase 0's 1.002 / 1.002 computed the same way.
Gains: `deepcopy_memo` 0.13–0.18, `deepcopy` 0.23–0.28, `json_dumps` 0.25–0.33,
`json_loads` 0.57–0.59, `logging_*` 0.61–0.79, `sympy_integrate` 0.83, `meteor_contest`,
`hexiom`, `pathlib` 0.93–0.95. Above 1.05 in both replicates: only `python_startup`
(1.08–1.10, the startup cost of `enable()`; Phase 0: 1.18); `fannkuch` 1.114 in one replicate only
(≤ 1.03 in the other: noise); 1.03–1.05 on 3.15.1t: `xdsl_constant_fold` 1.033 in both,
others in one replicate only (M).

**Startup** (§5 item 4; `python -c pass`, callgrind and wall, release wheel, Phase 3):
installed but off (`RUSTIQUE` unset) 1.027 instructions / 1.032 wall in a bare venv on
3.15.1 (1.008 / 1.011 where another `.pth` exists), 1.047 / 1.048 on 3.14.9; `RUSTIQUE=1`
against unset 1.045 / 1.078 on 3.15.1 and 1.063 / 1.108 on 3.14.9 (+0.76 ms and +1.09 ms).

**Scoreboard at the integrated commit** (M, `docs/atlas/phase3.md` §7):

- **0–100 elements:** 3811 / 3814 cells ≤ 1.05 after isolation re-runs (the other 3 are
  fixed since, §5).
- **10k/1M wall time:** kernel-path geometric mean 0.462 / 0.482, the same as Phase 0.
  - Cells cross the 1.05 bar in both directions in equal numbers, which is noise at
    3 processes.
  - The 8 kernel-path cells that read above 1.05 measure 0.967–1.040 alone at
    10 processes (twice per build, on both builds).

**Correctness at the integrated commit** (M): CPython's own test modules of every group, all
15 groups enabled at once: 8,125 test outcomes on 3.15.1 and 3.15.1t, 7,823 on 3.14.9, **0
differences**; the full Rustique suite passes on 3.14.9, 3.14.9t, 3.15.1 and 3.15.1t; the
scoreboard's 3,814 instruction cells and 1,846 wall-time cells per build return results
identical to plain CPython's.

**Found after Phase 3 and fixed** (the first CI run of the integrated branch): a
use-after-free in the `json.loads` kernel for documents that repeat a key within one object
(its key memo held borrowed pointers; the replaced value's keys were freed mid-parse). It
crashed two CI jobs and was the "unreproduced crash" of Phase 2; memcheck located it, the
memo now owns its keys, and Hypothesis now generates documents with repeated keys
(`docs/targets/json.md`, open issues).

Phase 3's repeated test runs hit the same bug once in 9 runs (M, `docs/atlas/phase3.md`
§2):

- Where: a segfault in the json round-trip property test on 3.15.1.
- How the key repeats: the strategy builds dicts with both `'😀'` (two lone
  surrogates) and `'\U0001F600'` as keys. They are different keys in Python, but
  `json.dumps` writes both as `"😀"`.
- Memcheck shows an invalid read on the measured commit and is clean on the fix.

Lesson for every target: per-call caches own what they hold, unless the input itself
provably keeps it alive.

## 7. Per-target results

84 targets in 15 groups. All 84 install on 3.15.1 and 3.15.1t; on 3.14.9 and 3.14.9t, 79
install and the five pathlib property targets (`parent`, `name`, `suffix`, `stem`, `parts`)
refuse by design ("CPython 3.14 not validated": 3.14's specializer inlines property getters
without checking the function version) (M, f5090f9). Per-call ratios are callgrind instructions, enabled / plain, on small realistic
inputs, 3.15.1, steady state (M, measured by each workstream, `docs/targets/<group>.md`).
"Workloads" are wall-time ratios of the workload set with only that group enabled (M,
Phase 2, 3.15.1). Conformance: the CPython test modules of each group pass with the group
enabled exactly as without, on all four builds (0 differences, M).

| group (targets) | per call (enabled / plain) | workloads (only this group on) | install (budget) |
|---|---|---|---|
| float (1): `float.__repr__`/`__format__('')` | 0.035–0.389 | via json | 45k enable (ok) |
| json (2): `dumps`, `loads` | dumps 0.055–0.291, loads 0.218–0.563 | asyncio_http 0.789, csv_etl 0.799, aiohttp 0.906, json_api 0.916; geomean 0.878 | `import json` +6.5% (over) |
| copy (2) + dataclasses (2) | deepcopy 0.13–0.46, copy 0.36–0.93, asdict/astuple 0.36–0.40 | state_snapshot 0.675, isort 0.763, json_api 0.961, sqlglot 0.971; geomean 0.833 | ~0.16M per group, `import copy` +0.16M (over) |
| datetime (10): `strptime`, `strftime`, `isoformat` | strptime 0.068–0.086, strftime 0.085–0.20, isoformat 0.12–0.29 | log_analytics 0.617, tz_events 0.593, csv_etl 0.823; geomean 0.670 | 96k enable, +83k import (ok) |
| posixpath (8), fnmatch (3), pathlib (13) | posixpath 0.16–0.32 (abspath 0.45–0.71), fnmatch 0.09–0.23, pathlib 0.05–0.76 | fs_inventory 0.742, pytest_networkx 0.908, pip_resolve 0.949 | 0.22M / 0.28M / 0.46M enable, `import pathlib` +5.5% (over) |
| logging (5) | one record 0.45–0.51, disabled `debug()` 0.62 | logging_service 0.830 | lazy +1.3% (ok), warm enable 0.143M (over) |
| random (6), textwrap (7), urllib (7), statistics (5) | random 0.05–0.54, textwrap 0.15–0.47, urllib 0.16–0.67, statistics 0.035–0.27 | report_render 0.449, requests_local 0.930, log_analytics 0.975, billing 0.997 (the four groups together) | random +142k (over); others ok |
| containers (7), builtins (6) (from the parity program) | 1M elements: `max(floats)` 0.13, `min(ints)` 0.17, `ints.sort()` 0.18, `x in ints` 0.26 (wall) | (spread thinly) | at `enable()` |

Re-measured at the integrated commit with every group enabled (Phase 3, `docs/atlas/phase3.md`
§8): each group's own callgrind bench reproduces its headline cells within ±0.01–0.02 and
every cell is ≤ 1.05 (json 43 cells, copy 52, datetime 39, paths 99, logging 48, random 41).

## 8. What falls back, and why

The rule is the same everywhere: a kernel runs only where it provably computes CPython's
result with CPython's side effects; everything else runs CPython's own code. The classes of
fallback, and why each exists:

- **User code would run between native steps** (ruling 15): `json.dumps(default=/cls=)`,
  `copy.deepcopy` meeting `__deepcopy__`/`__reduce_ex__` overrides/copiers, logging
  handlers, filters and overridden methods, an argument's `__str__`/`__fspath__`,
  `random` subclasses with Python methods. Running these from Rust would change tracebacks,
  what tools see and recursion depth. Wave 2 turns the most common ones (json `default=`)
  into C-entry kernels where CPython itself calls the hook from C.
- **Subclasses and duck types** (contract §2.7): subclass receivers of C types inherit the
  patched slot, so kernels check the exact type; pathlib subclasses and `str`/`bytes`
  subclasses of path arguments fall back (plain pathlib subclasses: wave 2).
- **Inputs outside the validated grammar**: `strptime` directives outside the subset,
  non-C `LC_TIME` locales where names are used, tzinfo other than `timezone`/C `ZoneInfo`,
  years < 1000, `{`/`$` logging styles, non-UTF-8 codecs in `urllib.parse`, iterators
  (which a kernel would consume) in `statistics`, Decimal/Fraction/NaN/big ints.
- **State the kernel cannot see safely**: a monkeypatched dependency (`mock.patch` of any
  pinned global or class attribute: falls back, re-arms when restored); a tracer,
  profiler, coverage tool or breakpoint listening (Python targets: tools see CPython's
  frames); a non-main interpreter.
- **Error paths**: when a kernel detects an error before anything observable, CPython's
  original re-runs the call and raises its own exception.
- **Not accelerated on purpose**: `dataclasses.replace` (always ends in the user's
  `__init__`), `json.dump` (its writes are observable), `JSONEncoder.encode` called
  directly, `fromisoformat` (already C, no headroom), `posixpath.normcase` (a fallback
  would cost 1.4×), `random.uniform` (its fallback would exceed 1.05),
  `logging.getMessage`/`isEnabledFor` (fallbacks 1.10, 1.36 on free-threaded builds),
  `urllib.parse.urlsplit`'s wrapper (its `lru_cache` statistics are public), pathlib
  properties on 3.14 (the 3.14 specializer inlines property getters without checking the
  function version), `ipaddress` (deferred to wave 2).
- **Platforms**: every wheel (8 platforms × 3.14/3.14t/3.15/3.15t) installs and runs
  correctly, but targets install only where validated: all 84 on Linux/glibc (3.15), 74 on
  macOS and musl (the `datetime` text targets rely on glibc's locale and `strftime`
  behaviour), 30 on Windows (POSIX-only paths groups; no run-time lookup of CPython
  symbols such as `_PyObject_GetMethod`; the free-threaded build's string layout under
  MSVC). Category `platform` in `status()`; wave 2 widens it.
- **Where the time is not**: two thirds of typical CPU time (interpreter, allocation, GC,
  dict operations) is out of reach of any in-place kernel (IMPACT_ATLAS §3).

## 9. Ranked next waves

Ranked by measured headroom (IMPACT_ATLAS §4) and what Phase 3's counters showed real
workloads fall back on (`docs/atlas/phase3.md` §4.2), discounted by risk:

1. **Startup and install cost** (every process pays it): split the engine into its own
   extension so `RUSTIQUE=1` does not load the opt-in container types (setuptools-rust or
   a second distribution), a precompiled loader, deferred fingerprinting and cheaper
   interning (the install overages of §5), a lazily built status for the public
   `enable()` (≈ 0.7M instructions today). Goal: every startup cell ≤ 1.05.
2. **json with `default=`/`cls=`** behind `_json.make_encoder`'s `tp_call`, where CPython
   itself calls the hook from C (ruling 15 permits it): the encoders of web frameworks.
3. **Membership with class needles and `key=` paths.** `tuple.__contains__` falls back for
   needles that are classes (`type(node) in (A, B)`: 67,863 fallbacks per run of
   `pyflakes_stdlib`, 23,318 in `pylint_stdlib`, 14,507 in `asyncio_http`), and
   `sorted`/`list.sort`/`min`/`max` with `key=` fall back (73,836 `sorted` calls per run of
   `xml_feeds`). Kernels: an identity scan when needle and items are exactly `type`
   instances (class equality is identity there), and `key=` fast paths for exact-type keys.
4. **HTTP header and e-mail parsing** (`http.client.parse_headers`, the HTTP subset of
   `email.feedparser`, then header classes): atlas score 0.38; `aiohttp_app`,
   `requests_local` and `email_mime` are at 0.91–1.00 today.
5. **`re`**: module-function fast path (`re.match(p, s)` cache lookups), then a DFA
   start-finder for long subjects.
6. **Deeper coverage of wave-1 targets**: `copy.deepcopy` restarts on 12–22% of calls in
   `sqlglot_transpile`, `json_api` and `fastapi_pydantic` (reductors and `__deepcopy__` met
   mid-copy); plain pathlib subclasses; `json.loads` restarts in `asyncio_http`.
7. **Platform coverage**: run-time lookup of CPython symbols on Windows
   (`GetProcAddress` on the Python DLL), a layout-independent read of `PyASCIIObject.state`
   (json on free-threaded Windows), and validating the `datetime` text targets on macOS and
   musl libc (74 → 84 targets there; 30 → ~70 on Windows).
8. **Bulk text kernels** behind a length guard (`str`/`bytes` find/split/replace), the
   `xml.etree` serializer, `tokenize`, the `csv` writer, `ipaddress`, `uuid` (3.14/3.15).
9. **zlib/crc32** only if the upstream Rust zlib (3.16) slips.
10. **Upstream reports**: PEP 829 `.start` files cost 1.72× startup (`site` imports `pkgutil`
   and `re`); `copy.deepcopy` is 2–4× slower on free-threaded 3.15.1t than on the GIL build.
