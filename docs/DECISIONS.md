# Decision record: making the builtins Rustique (parity program, October 2026)

Short record of what was decided, what is at parity and what is not, and what comes next.
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
