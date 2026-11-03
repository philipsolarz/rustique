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

# Part 3: wave 2, from stdlib-heavy programs to all Python programs (November 2026)

Evidence: `docs/IMPACT_ATLAS.md` Part 2 (§7–§14, the wave-2 atlas), `docs/atlas/w2-*.md` (one
per Phase-1 slice) and `docs/atlas/w2-phase0.md` (ground truth). Tags: (M) measured, (I)
inferred. §10 records the Phase-1 decisions; results follow in Phase 3.

## 10. What was decided at the end of Phase 1, and why

**Workstreams and targets.** Nine Phase-2 workstreams (`engine`, `accel`, `core`, `text`,
`datamodel`, `re`, `interp`, `gc`, `dx`) and thirteen measurable targets (T1–T13), chosen and
justified in IMPACT_ATLAS §14. The headline is the speedup on real workloads per family,
framework-heavy and custom software included.

**Rulings** (each with its evidence in the slice doc named):

1. **Opt-in native types are retired** (`w2-startup.md` §4): `List`/`Dict`/`Set`/`Tuple`/
   `FrozenSet`, `X` expressions, fixed-width scalars, the C-API capsule and `rustique-api`,
   `enable(native=...)`, `--native`, `@rs.native`. They are 60% of the extension's code, 68% of
   test time and a quarter of build time; none speeds up an existing entry point without a code
   change; the extension shrinks from 6.84 to 2.31 MB, which is a third of the startup fix. The
   in-place kernels that came from the same program (`list.sort`, `sum`/`min`/`max`/…,
   containers' `in`/`index`/`count`) are the new way of working and stay. The last commit that
   contains the native tier is tagged before deletion, with a migration note.
2. **A new site kind, the opcode handler (M2), ships as an opt-in `interp` tier**
   (`w2-interp.md` §8–§9): on verified tail-calling builds (python-build-standalone), handlers
   generated from the running patch release's `bytecodes.c`, every table entry verified before
   the swap. Default-eligible only after its gates hold (full regrtest 0 differences on each
   verified build, differential suites, every cell ≤ 1.05, whole runs ≤ 1.00). M1 (a copied
   loop as a PEP 523 hook) stays research: 1.02–1.04 on whole programs (M).
3. **The adaptive GC policy ships as an opt-in `gc` tier, not as a default**
   (`w2-runtime.md` §5). Criterion (a), "documented `gc` APIs report nothing different", is
   defined as in `w2-runtime.md` §5.1: values that depend on collection timing (`get_count`,
   `get_stats`, the number of callback invocations, `collect()` return values) are outside the
   contract, as they already are for every Rustique kernel that allocates less than CPython
   (ACCELERATION §5.8). The prototype's two declared deviations — `get_threshold()` reports
   the configured thresholds rather than the ones in force, and `gc.callbacks` holds one extra
   entry — are acceptable in an opt-in tier and **block default status** on both builds.
   Memory budget: peak RSS ≤ 1.05× per workload and per adversarial scenario.
4. **"Fast exit" is not shipped**, not even opt-in (`w2-runtime.md` §6): skipping the
   exit-time collections loses the finalization of cyclic garbage (buffered data in files only
   reachable from cycles), which changes semantics, not just timing.
5. **Unrecorded CPython patch releases refuse by default** (`w2-floors.md` §5): fingerprints
   cannot see C behaviour changes (3.15.2's `tuple_hash` recursion guard). Fingerprint v3
   drops docstrings and canonicalizes exception tables; accepted hashes are recorded per patch
   release by a CI job that runs the conformance suite first; an unrecorded patch release shows
   the refusal category `unvalidated-patch`, and an explicit opt-in installs targets whose v3
   fingerprints match a recorded hash, reported as "unvalidated".
6. **Asynchronous exceptions are outside the contract** (ACCELERATION §5.8 amendment,
   `w2-floors.md` §5.3): signals and `PyThreadState_SetAsyncExc` arriving while a kernel runs
   are delivered when it returns, at the next eval-breaker check; their timing is already
   nondeterministic in CPython.
7. **Packaging**: the extension becomes top-level `_rustique`, loaded directly by the `.pth`
   line, by repacking the wheels in CI (`w2-dx.md` §7, `w2-startup.md` §1); the loader falls
   back to the in-package path for sdist and editable installs. setuptools-rust was rejected
   (rebuilding the proven wheel matrix for 0.46M instructions).
8. **Platforms**: the process gate gets an allowlist of the validated platforms; anything
   else refuses with category `platform`, and Rustique's own test suite skips on `version` and
   `platform` refusals (`w2-dx.md` §2).
9. **Free-threading**: the framework is hardened (no panic escapes `enable()`; dict snapshots
   outside the registry lock; the registry lock is a leaf; suspension decided on the thread that
   falls back) before any target pins a class attribute (`w2-threads.md` §3). A kernel scaling
   rule joins the never-slower bar: CPU time per call at 8 threads ≤ 1.05× CPython's original.

**Rulings made while the workstreams built** (Phase 2; evidence in the target doc named):

10. **Opcode-handler sites** (`docs/targets/interp.md` App. A, now ACCELERATION §1/§3–§7): a site
    kind for the `interp` tier only — an entry of the tail-calling dispatch table, replaced by a
    handler generated from the running patch release's `bytecodes.c`, on recorded
    (release, configuration, ELF build-id) builds only, every entry verified before a
    compare-and-swap, never on instrumented opcodes, `ENTER_EXECUTOR` or JIT paths; per-group
    net-benefit suspension. A frame-pushing override may push a Python frame inline only the way
    an existing CPython specialized form pushes it for the same kind of callee.
11. **`tp_alloc` routing of recognized frozen dataclasses** (`docs/targets/generated.md` §4, the
    one exception to ACCELERATION §3.6): a class whose generated `__init__` carries a kernel gets a
    `tp_alloc` that returns exactly `PyType_GenericAlloc(type, n)`, set before the class reaches
    user code and followed by `PyType_Modified`, so `CALL_ALLOC_AND_ENTER_INIT` does not bypass
    the kernel; subclasses get `PyType_GenericAlloc` from `type_new`; while a tool listens, or for
    a type whose `__init__` lacks the kernel, the function hands `PyType_GenericAlloc` back;
    `on_disable` restores. Default tier; every cell ≤ 1.05 (M).
12. **The `gc` tier writes calibrated collector state** (`docs/targets/gc.md`): deferral through
    `gcstate->long_lived_total` (CPython's own 25% rule), the observer through a private list
    that `gcstate->callbacks` points to and that forwards every collection to the program's own
    list exactly as `invoke_gc_callback` does. Both §10.3 deviations are gone (thresholds and
    `gc.callbacks` read back unchanged). This is an opt-in-tier mechanism, not a site; it does not
    qualify the tier for default status by itself (memory and pause evidence first). Its free-
    threaded pause cap is 1.05× (1.1× on GIL builds); an explicit `gc.collect()` of an empty
    generation costs 1.059× (+76 instructions) with the tier on — a listed opt-in exceedance.
13. **The `interp` tier ships opt-in and is not yet default-eligible**: every cell ≤ 1.05 and every
    whole workload ≤ 1.0043 (inside T1), CPython's full regression suite 0 per-test differences on
    3.15.1t and 3.15.2/3.15.2t and two load-dependent timer flakes on 3.15.1 (pass 3 of 3 when
    re-run), but its own gate (whole runs ≤ 1.00) misses on pycodestyle (1.0043, genuine
    copy cost) and on four 0.07–0.25% residuals. Next: never swap the specialized family members
    on programs without polymorphic sites, and a multi-run measurement protocol.
14. **Validate on first call** (accel D): on GIL builds a target's fingerprints, self-tests and pins
    run at its first call (state `armed` until then); free-threaded builds validate eagerly; a
    target that must act from install on declares `TargetSpec.eager` (the `gc` tier).
    `RUSTIQUE_VALIDATE=eager` restores eager validation (the test suite sets it).
15. **Credits v2** (ACCELERATION §5.6): cost-weighted credits per target, a 256-call window for
    mixed traffic, and an early exit — a target with no hit since it was armed suspends at 64
    fallbacks; `deopt` is split into restart / continue / abandon; suspensions are reported.
16. **Patch releases** (§10.5 implemented): fingerprint v3, recorded sets for 3.14.1–3.14.10 and
    3.15.0–3.15.2 (conformance run before each recording); 3.14.0 stays unrecorded because its
    conformance run failed on a textwrap helper the kernel reimplemented without fingerprinting —
    the gate working as intended; every target now fingerprints the helpers it reimplements.
17. **`not-needed`** is a status category distinct from refusals (e.g. `set.__reduce__` on GIL
    builds, where the CPython bug it fixes does not exist).
18. **Accepted overages** (each measured, each bounded): enabling an already-imported group
    costs 155k–408k instructions at first call (budget 0.1M; the targets' own prepare; every lazy
    install is within budget); `re` declines cost ≤ 1.036 for their first 64 calls and an
    all-fallback stream ≤ 1.046 after suspension (the restored call site stays unspecialized);
    the enum call kernel on a class whose private value map holds a user-typed key 1.033.
19. **Default-path hygiene**: a default `enable()` leaves no Python object behind that plain
    CPython would not have (the fork hook is registered only for live counter files), and no
    framework probe may touch `sys.monitoring` (its tool-id release bumped the global
    instrumentation version and taxed every later code object ~930 instructions; found by the
    datamodel workstream, fixed in accel).

## 11. Speedup on real workloads, per family, against the targets

Measured in Phase 3 (`docs/atlas/w2-phase3.md`) on the dedicated AWS c7a.8xlarge: the 65
workloads (`benches/workloads/`, 8 families), plain CPython against the **default tier**
(`rustique.enable()`, every default group, no opt-in tier), fresh processes, interleaved
(`run.py compare -n 5 -p 5`), fat-LTO release builds. Replicate 2 on `7fec7e4` (the final
steady-state code), replicate 1 on `d40e7b9` (agrees within 0.006 per family). Every
workload's result digest is identical plain and enabled, in both replicates on both builds (M).

| family (n) | 3.15.1 | 3.15.1t | target (IMPACT_ATLAS §14.4) |
|---|---:|---:|---|
| **all 65** | **0.870** (1.15× faster) | **0.872** (1.15×) | T3 ≤ 0.88 / ≤ 0.84: **met / not met** |
| wave 1's 34 (web, data, devtools) | 0.858 | 0.826 | T2 ≤ 0.85 / ≤ 0.80: **not met** (wave 1: 0.877 / 0.843) |
| the 31 new | 0.884 | 0.925 | — |
| framework apps (6) | 0.940 | 0.955 | ≤ 0.98: **met** |
| custom applications (10) | 0.936 | 0.973 | ≤ 0.95: **met** (3.15.1) |
| long-running services (4) | 0.848 | 0.888 | ≤ 0.85: **met** (3.15.1) |
| third-party-heavy code (6) | 0.961 | 0.966 | ≤ 0.95: **not met** |
| multi-threaded, 8 threads (5) | 0.685 | 0.792 | ≤ 0.78 on 3.15.1t: **not met** (by 0.012) |
| web / data / devtools | 0.882 / 0.752 / 0.957 | 0.903 / 0.735 / 0.861 | — |
| best | `report_render` 0.357 | `isort_stdlib` 0.201 | — |
| worst | `mini_interp` 1.006 | `mini_interp` 1.018 | T1 ≤ 1.02: **met** |

- **Framework-heavy and custom software moved for the first time.** After wave 1, Django,
  Flask, FastAPI, SQLAlchemy apps sat at 0.97–1.01; the six new framework apps are 0.940 by
  default (falcon 0.795, Django admin 0.968), and 0.933 with the `interp` tier. Custom domain
  software is 0.936 by default and 0.920 with `interp`; with both tiers the rules engine is
  0.820 and the order domain 0.893. The machinery that gets them there:
  enum, generated namedtuple/dataclass code, `contextmanager`/`singledispatch` bookkeeping,
  `re`, logging, json `default=`, UUID copying, and the opcode-handler overrides.
- **Wave 1's stdlib-heavy programs gained again**: `report_render` 0.438 → 0.357,
  `logging_service` 0.813 → 0.686, `json_api` 0.835 → 0.717, `flask_jinja` 0.997 → 0.958,
  `tz_events`, `fs_inventory`, `log_analytics` 1–3 points each (3.15.1).
- **Whole processes** (`RUSTIQUE=1` through the `.pth`, interpreter start and imports included,
  8 workloads, `9d0abae`): 0.743 / 0.742; `import_startup` (21 child processes) 0.990 — it
  was 1.022 in wave 1: the `re` compiler now pays for the startup cost.
- **pyperformance 1.14.0** (122 benchmarks, `7fec7e4`): **0.911 / 0.936** (T12 ≤ 0.93 / ≤ 0.94:
  **met**; wave 1 0.942 / 0.955); `regex_compile` 0.044 / 0.122, `logging_format` 0.51 / 0.64.
- **Opt-in tiers** (`d40e7b9`, against plain / against the default tier, all 65): `interp`
  0.859 / 0.987; `gc` 0.868 / 0.998 (3.15.1), 0.866 / 0.992 (3.15.1t); both (frameworks, custom,
  devtools) 0.985 of the default tier; no tier slows a workload by more than 1.8% against the
  default tier.
- **Free-threading** (T7): with the copy group off, `isort_stdlib` 0.260 and `state_snapshot`
  0.668 on 3.15.1t (**met**: the `set.__reduce__` kernel alone removes CPython's
  self-contention). The threads family scales 1.9–7.5× at 8 threads with Rustique on.

**Against all thirteen targets** (`w2-phase3.md` Summary has the evidence per row):

| # | target | result | |
|---|---|---|---|
| T1 | never slower | workloads ≤ 1.018; 0–100 cells 3814/3814; **`sorted()` of a 1M-element set/frozenset of int/float 1.23–1.28** (see §13); `RUSTIQUE=1` wall 1.059 / 1.075; first call of `random` +0.29M Ir; pyperformance `coroutines` 1.07 on 3.15.1t (unexplained) | **not met** |
| T2 | wave 1's 34 ≤ 0.85 / 0.80 | 0.858 / 0.826 | not met |
| T3 | all 65 ≤ 0.88 / 0.84 | 0.870 / 0.872 | met / not met |
| T4 | families | custom, frameworks, services met; libs 0.961, threads (t) 0.792 not | 3 of 5 |
| T5 | `interp`: frameworks ≤ 0.96, custom ≤ 0.90, full regrtest 0 differences | 0.933 / 0.920; regrtest (dev box, interp workstream) 0 differences on 3.15.1t, two load-dependent timer flakes on 3.15.1; not re-run on the bench box | frameworks met, custom not |
| T6 | `gc` tier | GC-heavy geomean 0.982 / 0.952 (target 0.87); RSS ≤ 1.038 (met); largest pause `state_snapshot` 1.30 (3.15.1) | not met |
| T7 | free-threading fix | 0.260 / 0.668 | met |
| T8 | startup ≤ 1.05, extension ≤ 2.5 MB | instructions 1.028 / 1.029 (met); wall 1.059 / 1.075; 3.63 MB | partly |
| T9 | install budgets | every lazy install within budget on `9d0abae` | met |
| T10 | reach | 3.15.x 108–110 targets (104 on t), 3.14.x 105 (99 on t), 3.14.0 refuses `unvalidated-patch`, 3.16.0a1 refuses `version` | met (Linux; Windows/macOS/musl floors are CI's) |
| T11 | correctness | CPython's test modules: 0 differences in 18,210 outcomes per 3.15 build (50 modules, every default group at once); 14 real projects' suites: 0 differences in 34 runs (dx, `d40e7b9`) | met |
| T12 | pyperformance | 0.911 / 0.936 | met |
| T13 | estimator ±0.03 on ≥ 80% | 69% (3.15.1), 80% (3.15.1t) | not met on 3.15.1 |

## 12. Per-target results (wave 2)

21 default groups, 111 targets on 3.15.1 (110 installed, `set.__reduce__` not needed on GIL
builds), plus the two opt-in tiers (`interp` 4 targets, `gc` 1). Per call: callgrind
instructions, enabled ÷ plain, small realistic inputs, 3.15.1 (M; each workstream's
`docs/targets/<group>.md`). Conformance: the CPython test modules of every group, every group
on at once, 0 differences on 4 builds (M).

| group (new or extended in wave 2) | per call | where it shows |
|---|---|---|
| containers / builtins | sort grid every cell 0.37–0.94 (the first kernel reached 1.84 on ordered and short-string input; tiny inputs fixed in `7fec7e4`), `sorted` a real kernel, `any`/`all` 0.53–0.73; `x in list`/`index`/`count` without the list's lock on free-threaded builds | devtools, data |
| set (new, free-threaded only) | `set.__reduce__` without CPython's self-contention | isort 4.0× → 0.20 on t |
| json / copy / dataclasses | `dumps(rec, default=hook)` when the hook is not needed 0.183; `asdict(Order)` with a UUID 0.341; `deepcopy` of records holding a UUID 0.544 | json_api 0.717, logging_service |
| logging | one record 0.327 (wave 1: 0.453), `findCaller` 4,027 → 868 Ir, `Handler.handle` for `StringIO` streams | logging_service 0.686, job_worker, shared_cache |
| datetime | format plans (`strftime` 1.85k → 1.46k Ir), one-pass `strptime` | tz_events, log_analytics |
| textwrap / urllib / paths / random | allocation-free textwrap (`fill` 0.038) and urllib (`quote`'s CPU per call from 1 to 8 threads ×19.6 → ×1.01), `relpath` 6,993 → 1,635 Ir, cached pathlib paths in `join`; `randint` 0.22–0.41 | report_render 0.357, fs_inventory |
| enum (new) | `Color(2)` 0.265, `IntFlag` `\|` 0.180, `m.value` 0.496 | custom family, json_api |
| generated (new) | namedtuple `_replace` 0.265, `_make` 0.702, `NT(...)` 0.726; frozen dataclass `__init__` 0.63–0.64 (positional calls through `tp_alloc` routing, §10.11) | custom family, pylint (`TokenInfo`) |
| contextlib / functools (new) | `cm()` 0.644, `singledispatch` `dispatch` 0.669 | rules_engine, event_ledger |
| re (new) | a compile miss 0.146–0.275 (exact `_sre.compile` arguments; 99.7% of 8,522 PyPI patterns), `re.match` 0.679, `re.compile` hit 0.449 | `import yaml` 0.61, `regex_compile` 0.04 |
| `interp` tier (opt-in) | `isinstance` metaclass negative 0.330, polymorphic attribute 0.754–0.831, frozen dataclass `F(...)` 0.558, `P(1, b=2)` 0.519 | frameworks 0.933, custom 0.920 |
| `gc` tier (opt-in) | GC work 0.95 (GIL) / 0.82 (t) | billing 0.87 / 0.91, request_pool 0.85 (t) |

## 13. What fell back, and why; what is open

The fallback classes of §8 stand (user code between native steps, subclasses and duck types,
inputs outside the validated grammar, state the kernel cannot see, error paths). Wave 2 adds:

- **Ruling 15 keeps the hot halves of dispatchers in CPython**: `contextmanager`'s
  `__enter__`/`__exit__`, `singledispatch`'s wrapper, frozen dataclasses with `__post_init__`,
  `html.parser` handlers, `inspect.signature` on annotated functions (`__annotate__`).
- **Generated code on free-threaded builds**: every per-function site would stop the world; those
  targets refuse on t this wave (the enum slots and the `set` kernel do not need it).
- **The `interp` tier** runs only on recorded python-build-standalone builds (tail-calling
  interpreter, verified build-id); everywhere else it refuses with the reason.
- **Unrecorded patch releases** refuse (`unvalidated-patch`), and 3.15.0's `copy` targets
  refuse because CPython's code really changed there.
- **Open at the end of the wave** (each measured, each listed):
  - `sorted()` of a 1M-element set or frozenset of int/float: 1.23–1.28 (102.5M → 127.3M Ir per
    call; long nearly ordered runs, where the wave-2 sort strategy loses to timsort). Fix in
    progress at the time of writing; see §17 for its status.
  - the `gc` tier's largest pause on `state_snapshot` (3.15.1): 1.30× the default's in
    milliseconds. The tier's bound — objects examined ≤ 1.1× (GIL) / 1.05× (t) of the largest
    full collection the default policy made — held (1.00×): the deferred collection examined
    295,026 objects where the default's largest examined 321,648, but it landed in the run's last
    phase, where traversal costs 254 ns per object instead of 175–184 (that phase is as costly
    under both policies). A trigger policy cannot see per-object cost in advance; bounding
    milliseconds means not deferring. Known limit of the opt-in tier; see §16.
  - pyperformance `coroutines` on 3.15.1t: 1.07–1.08 under pyperf only (pure Python, no group
    alone causes it, 0 GC collections, not reproducible outside the pyperf worker): inferred
    code/stack layout, not decomposed.
  - startup wall time with `RUSTIQUE=1`: 1.059 (3.15.1) / 1.075 (3.14.9), instructions 1.028 /
    1.029; extension 3.63 MB (the new kernels — `re`, datamodel, the interp handler sets —
    brought it back up from 2.33 MB).
  - first-call costs over the install budget: `random` +0.29M Ir, `json` and `pathlib` by
    3–12k; enabling an already-imported group 155k–408k (§10.18).

## 14. What is settled, and why

In addition to IMPACT_ATLAS §12 (the floors and the rejected approaches):

- **The specialized interpreter fast paths** are a floor (our generated copies reach
  0.96–1.00 at best); only the generic paths are reachable, and only on tail-calling builds.
- **Language operations already in C** (str methods, dict/list ops, formatting, `getattr`):
  no Rust re-implementation beats them at real input sizes.
- **Allocator switching** (`mimalloc` 1.051), **static GC thresholds and freeze policies**,
  **fast exit**, **CPython's JIT as a policy** (1.015 on the workloads, slower on 5 of 6
  interpreter-bound ones), **a PEP 523 copy of the loop as a default** (1.02–1.04 on whole
  programs), **a vectorcall "Rust JIT"** of user functions.
- **48 of the 84 wave-1 targets** are at or near their floor (`docs/atlas/w2-floors.md` §1).
- **The scoreboard's non-kernel cells above 1.05** (tuple iteration, dict copy, set equality,
  pickle at 1M): not Rustique — enabled ÷ imported ≤ 1.026 at the best heap offset; pymalloc pool
  position and process layout move plain CPython by the same amounts.

## 15. The opt-in native types

Retired in 0.3 (§10.1): `List`/`Dict`/`Set`/`Tuple`/`FrozenSet`, `X`, the scalars, the C-API
capsule and `rustique-api`, `enable(native=...)`/`--native`/`@rs.native`. About 52,000 lines and
25 test files removed; the extension went from 6.84 MB to 2.33 MB at the time of the removal
(wave 2's new kernels brought it back to 3.63 MB) and the test suite from ~300 s to ~150 s per
build; accessing a removed name raises `AttributeError` that
points to `docs/MIGRATION.md`; tag `legacy-native-types` marks the last commit with them. The
in-place kernels they led to (`list.sort`, `sum`/`min`/`max`/`any`/`all`, `in`/`index`/`count`)
are the new way of working and stay, at zero cost when not called.

## 16. Runtime-policy tiers: the evidence

- **`gc` tier (opt-in)**: defers full collections while they keep finding nothing, through
  CPython's own 25% rule; steps aside when the program tunes the GC; explicit `gc.collect()` is
  never touched. Measured on the quiet box (`w2-phase3.md` §9.1): GC-heavy workloads 0.982 /
  0.952 (T6's 0.87 not met: most of their full collections find real garbage), peak RSS ≤ 1.032 /
  1.038 (budget 1.05: met), adversarial programs ≤ 1.03× RSS with the pause cap. **Criteria for
  default status**: (a) documented `gc` APIs report nothing different — met now (thresholds and
  `gc.callbacks` read back unchanged; timing-derived values are outside the contract); (b) peak
  memory within 1.05 — met; (c) pauses no longer — **not met**: the bound the tier can guarantee
  is in objects examined (≤ 1.1× / 1.05× of the largest legitimate full collection; held, 1.02 /
  1.06 largest collection over the GC-heavy set), and pause milliseconds are a measured
  indicator with one exceedance (`state_snapshot` on 3.15.1, 1.30×: same size, a costlier phase,
  §13);
  (d) steps aside when the program tunes the GC — met (ml_meth hooks on every `gc` setter).
  Verdict: **stays opt-in**. A default would need (c) in milliseconds, which a deferral policy
  cannot guarantee; the next step is evidence that pause placement costs nothing on the
  workloads people run (or a cost model of traversal per object), not a tighter trigger. The two
  "others" above 1.02 against plain (`pydantic_payloads` 1.021, range 1.00–1.02, GC share < 1%;
  `fastapi_pydantic` 1.037 on 3.15.1t) include loading the extension; against loaded-but-off the
  tier measured ≤ 1.005 in instructions on the dev box (the quiet box had no loaded baseline).
- **`interp` tier (opt-in)**: §10.13; default-eligible per verified build once its own gate
  (whole runs ≤ 1.00) holds.
- **Fast exit**: not shipped (§10.4).

## 17. Ranked next waves

Ranked by measured headroom and what wave 2 left, discounted by risk:

1. **Close the never-slower items** of §13: the large-set sort, the `gc` tier's pause placement,
   startup wall time (smaller engine: strip symbols, split the interp handler sets into a
   lazily loaded object), the first-call costs over budget, the `coroutines` layout effect.
2. **`interp` tier to default on verified builds**: never swap the specialized family members on
   programs without polymorphic sites (pycodestyle 1.0043), a multi-run measurement protocol,
   the remaining C→Python frame pushes (bound-method `f(*a, **k)`, `__call__`, `partial`: ~1–1.5%
   on frameworks), a `STORE_ATTR` cache; aarch64 and macOS python-build-standalone builds.
3. **Third-party-heavy code** (libs 0.961): what libraries make CPython do — generic attribute
   lookups (5.9%), allocation (12.6%), dict operations — reached through the `interp` tier and the
   runtime layer; `getattr`/`hasattr` with the attribute cache once its fallback cells fit.
4. **email/http header parsing** (59% of `email_mime`), the **`xml.etree` serializer** (50% of
   `xml_feeds`), the json `make_encoder` route (~4% of `json_api`), `inspect.signature`,
   weakref containers.
5. **Free-threading**: generated-code targets on t without stop-the-world installs; the
   `isinstance(x, ABC)` cache; threaded programs in the estimator.
6. **Platform reach**: Windows (48 targets planned) and macOS/musl (77) proven by the
   per-platform CI job; recording new patch releases as they ship.
7. **Upstream reports** (documented, not filed): `set.__reduce__`'s nested critical section,
   `isinstance`'s `__class__` lookup, the `utf-8-sig` decode every `.pth` costs, a vectorcall for
   `wrapper_descriptor`, remote-frame sampling on 3.15t.
