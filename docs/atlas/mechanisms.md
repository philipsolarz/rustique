STATUS: Phase 1 research (round question 4), 2026-11-01. Prototype code: branch `p1/mechanisms`
(`crates/rustique-py/src/accel/` reusable, `src/proto.rs` throwaway, `benches/mechanisms/`,
`tests/test_mechanisms_proto.py`). Raw data: `~/rustique-research/mechanisms/`.

# Mechanisms: putting Rust behind CPython's existing entry points

How a Rust kernel replaces the work behind an entry point that user code already holds
(`str.split`, `str % x`, `_json`'s encoder, `posixpath.join`, `uuid.UUID.__str__`, …)
without changing the object the user holds. Covers the three kinds of target, what each
mechanism costs, what keeps working, and the draft install / guard / fallback / report rules
for `docs/ACCELERATION.md`.

**How measured.** Instruction counts (Ir) come from callgrind, instrumented only around the
measured region. Each cell runs in its own process, as Ir(2n calls) − Ir(n calls) over n,
statements unrolled 10×, so fixed costs cancel; the noise floor is about ±15 Ir per call.
Builds: python-build-standalone 3.15.1, 3.15.1t and 3.14.9; prototype built with
`--profile dev-release`. Every number is tagged **M** (measured; the script is named) or
**I** (inferred, from the cited source, `Python/bytecodes.c` = B, `Python/specialize.c` =
S, `Python/optimizer_bytecodes.c` = O, `Objects/typeobject.c` = T, 3.15.1 line numbers).

## 0. Recommendations

| kind | mechanism | fallback cost per call | identity, captured refs, introspection | caveats |
|---|---|---|---|---|
| **A** builtin type methods | swap `ml_meth` of the static `PyMethodDef` (existing `takeover.rs` mechanism) | **+2 Ir** (≤ 1.003); +5 Ir with a stats-gated counter **M** | all kept: the same descriptor objects; every call site reads `ml_meth` at call time **M** | 3.15 JIT copies `ml_meth` into traces: call `sys._clear_internal_caches()` after every install and uninstall **M**. Bypassed by `CALL_LIST_APPEND`, `CALL_LEN`, `CALL_ISINSTANCE`, `BUILD_STRING`, `any(genexpr)` (§3). |
| **A** builtin type slots | swap the slot of the static type | **+2 Ir** **M** | all kept | Unreachable for the hottest exact-type operations (`str ==`, `str +`, `s[i]`, `int +`, `k in d`, `f"{s}"`, `for c in s` on 3.15, dict hashing) (§3). Python subclasses and explicit `str.__mod__` keep the original. Never swap a slot the specializer compares by pointer (3.15 dict `mp_subscript`: it de-specializes every `d[k]`). |
| **B** C accelerator functions and methods | swap `ml_meth` (all 26 modules surveyed keep their `PyMethodDef` arrays as static, non-const data, shared by heap types and every module instance) | **+2 Ir** **M** (`struct.pack`, `Pattern.search` with `METH_METHOD`) | all kept | Process-global, including subinterpreters. Patch aliases too (3.15 `Pattern.match` and `Pattern.prefixmatch` are separate entries: `re.match` reaches only `prefixmatch` **M**). C-internal calls bypass (`json.loads` calls `scanstring_unicode` directly **M**). |
| **B** heap-type slots (`_json.make_encoder.tp_call`) | swap the slot of that interpreter's heap type | **+2 Ir** (16 520 Ir call) **M** | all kept | Per interpreter (main only: §6). Instance structs are private: read state through `tp_members` offsets. Types with a per-instance vectorcall (`operator.itemgetter`, `functools.partial`) never reach `tp_call` **M**. |
| **C** pure-Python functions and methods | **`PyFunction_SetVectorcall` on the existing function object**, plus guards; fall back through the saved `_PyFunction_Vectorcall` | **+180–260 Ir** (JIT: +250–350) **M**: callers lose the inlined `CALL_PY_*` frame push. +3 Ir through slots (`str(uuid)` via `slot_tp_str`). | **all kept**: identity, pickling, every captured reference (alias, `from` import, closure, default argument, `partial`, callback dict, bound method), signature, `__code__`, source, tracebacks on fallback **M** | Never slower only for targets whose fallback call costs ≥ ~5k Ir, ≥ ~7k under the JIT. Measured: posixpath.join with 2 args 1.051 (JIT 1.073), with 3 args 1.038 (JIT 1.060); `json.dumps` 1.009. Recursive targets pay the cost per level: `copy.deepcopy` pass-through 1.13 (JIT 1.21). Tools see the original only with the tools guard (§2). |
| all | lazy activation: one dict watcher on `sys.modules`, patching on importlib's post-exec re-insert | 0 per call. `enable()`: 32k Ir (~10 µs) instead of 78M Ir (~19 ms) for eager import of 9 target modules. +0.09% import time for other modules **M**. | – | Never import a target module eagerly. |

Rejected:
- **Module attribute replacement** (`posixpath.join = rust_join`): it has the cheapest fast
  path (0.271 vs 0.293), but references captured earlier keep the original. Static
  analysis finds `copy.deepcopy` captured by `from` import at 34% of call sites and
  `dataclasses` at 68–90% (`static-usage.md`). Pickling a captured original raises
  `PicklingError`, and `inspect.signature`/`getsource` fail on a C replacement.
- **`__code__` swap** to a shim: fallback 1.28×, fast path 0.62, `getsource` returns the
  wrong source, and tracers see the shim's frames. This disagrees with the `__code__`
  suggestion in `static-usage.md`; vectorcall keeps everything the shim keeps and costs
  less.
- **Class `__dict__` replacement**: bound methods captured earlier keep the old function, and
  `UUID.__str__ is` the original turns false.
- **PEP 523**: 1.5–1.9× on every Python call (TAKEOVER §3.5).

## 1. Overheads (Ir per call)

`benches/mechanisms/overhead.py`, **M**. `orig` is untouched CPython. `pt` is a
pass-through kernel that immediately calls the saved original: the floor of any fallback.

### 1.1 Kinds A/B: pointer swaps

| cell | 3.15.1 orig | pt (no counter) | pt + counter, stats off | stats on | interpreter guard | 3.15.1t orig → pt | JIT orig → pt |
|---|---:|---:|---:|---:|---:|---:|---:|
| `s.split()` (`ml_meth`) | 1408 | +2 | +5 | +6 | +30 | 1308 → +2 | 1361 → +2 |
| `fmt % (a, b)` (static slot `nb_remainder`) | 1463 | +2 | +5 | | | 1456 → +2 | 1465 → +2 |
| `repr(12345)` (static slot `tp_repr`) | 808 | +2 | ±15 (noise) | | | 810 → +2 | 720 → +2 |
| `json.dumps(lst)` (heap slot `make_encoder.tp_call`) | 16534 | +2 | | | | 17611 → +2 | 15006 → +2 |
| `rx.search(s)` (`METH_METHOD`) | 1152 | +2 | | | | 959 → +2 | 1137 → +2 |
| `struct.pack('i', 7)` (`FASTCALL`) | 755 | +2 | | | | 791 → +2 | 703 → +2 |

The interpreter guard (`PyInterpreterState_Get() == main`) costs about 30 Ir because it is
an out-of-line call. So kernels on process-global sites should be interpreter-agnostic
instead of checking (§6).

### 1.2 Kind C: pure-Python functions

| cell | orig | vectorcall pass-through | vectorcall fast path | other mechanisms |
|---|---:|---:|---:|---|
| `ident(x)` (`def ident(x): return x`) | 275 | 504 (1.83) | 224 (0.82): the floor of a fast path | after uninstall: 338 (1.23, version cleared); with `func_version` restored: 275 (1.00) |
| `o.m(x)` (method) | 305 | 564 (1.85) | – | |
| `posixpath.join(a, b)` | 3538 | 3717 (**1.051**) | 997 (0.282) with no guards; **1035 (0.293)** with all guards; 1036 with stats on | attribute → C function: 959 (0.271); `__code__` shim: 2210 (0.625) |
| `posixpath.join(a, b, c)` (fallback: argument count) | 5389 | 5593 (1.038) | 5612 (**1.041**) | attribute: 5616 (1.042); `__code__` shim: 6926 (1.285) |
| `posixpath.join(b'/usr', b'lib')` (fallback: type) | 3625 | | 3820 (1.054) | |
| `posixpath.join(a, b)` after `posixpath._get_sep` was replaced (fallback: dependency) | 3620 | | 3829 (1.058) | |
| `str(u)` (`UUID.__str__`, reached via `slot_tp_str`) | 5806 | 5809 (**1.001**) | | |
| `json.dumps(small)` | 18301 | 18469 (1.009) | | |
| `copy.deepcopy(small)` (recursive: ~8 calls per top-level call) | 19165 | 21689 (**1.132**) | | |

The 3.15.1t numbers are within ±0.02 of these ratios: 1.050, 0.281, 1.042, 1.055, 1.057,
1.000, 1.008, 1.127, and ident 1.86 / 0.88 / 1.24 / 1.00.

Why (**I**, S:1765, B `_DO_CALL`): `specialize_py_call` refuses any function whose
`vectorcall != _PyFunction_Vectorcall` (`SPEC_FAIL_CALL_VECTORCALL`), and
`PyFunction_SetVectorcall` clears `func_version` for good (funcobject.c:450). A call that
used to push the callee's frame inline (`CALL_PY_EXACT_ARGS`, ~60 Ir) becomes a generic
`CALL`, then `PyObject_Vectorcall`, then the kernel. When the kernel declines, it calls
`_PyFunction_Vectorcall`, which re-enters the eval loop on a new C frame. Calls that were
never inlined (through `slot_tp_*`, C callers, `functools.partial`) pay only the kernel's
checks (+3 Ir for `str(u)`).

The guards cost 38 Ir in total (1035 − 997): an exact-type and arity check, one relaxed load
of the dependency flag, `PyMonitoring_EnterScope` (34 Ir, an out-of-line call) and the
local-events check on the code object.

### 1.3 Under the JIT (`PYTHON_JIT=1`, 3.15.1, warmed 30 000 iterations)

| cell | orig | pass-through | guarded fast path | attribute → C function | `__code__` shim |
|---|---:|---:|---:|---:|---:|
| `s.split()`, `fmt % args`, `struct.pack`, `rx.search`, `json.dumps(lst)` (A/B) | – | +2 Ir (≤ 1.003) | – | – | – |
| `ident(x)` | 219 | 575 (2.62) | 228 (1.04) | – | – |
| `posixpath.join(a, b)` | 3469 | 3721 (**1.073**) | 1039 (0.299) | 927 (0.267) | 2279 (0.657) |
| `posixpath.join(a, b, c)` (fallback) | 5277 | 5596 (1.060) | 5615 (**1.064**) | 5573 (1.056) | 7007 (1.328) |
| `posixpath.join(pb, qb)` / deps changed (fallback) | 3524 / 3594 | – | 3791 (1.076) / 3814 (1.061) | – | – |
| `str(u)`, `json.dumps(small)` | 5394 / 16743 | 5397 (1.001) / 16875 (1.008) | – | – | – |
| `copy.deepcopy(small)` | 17232 | 20781 (**1.206**) | – | – | – |
| `ident(x)` after uninstall / with version restored | 219 | 421 (1.92) / 219 (1.00) | – | – | – |

The JIT makes an inlined Python-to-Python call cheaper (ident 219 vs 275 Ir), so a
vectorcall fallback costs relatively more: **+250–350 Ir** per call (**M**). Pointer swaps
are unaffected, apart from the stale-trace issue of §4.

## 2. Compatibility matrix

`benches/mechanisms/compat.py` (posixpath.join + `UUID.__str__`; the class `__dict__` column replaces `UUID.__str__` with a `functools.wraps` wrapper, so its tool rows are inferred) and
`tests/test_mechanisms_proto.py` (7 tests). Identical results on 3.15.1, 3.15.1t and 3.14.9
(**M**). ✓ means kept, ✗ means broken.

| property | `ml_meth` swap (A/B) | static slot swap (A) | heap slot swap (B) | **vectorcall + guards (C)** | vectorcall without tools guard | module attribute → C function | `__code__` shim | class `__dict__` replacement |
|---|---|---|---|---|---|---|---|---|
| refs captured before activation (alias, `from` import + closure, default argument, `partial`, callback dict, bound method) | ✓ (descriptors and bound methods read `ml_meth` per call) | ✓ | ✓ | **✓ all 6 accelerated** | ✓ | ✗ all keep the original | ✓ (shim) | ✗ bound methods |
| identity (`f is mod.f`, `pickle.loads(pickle.dumps(f)) is f`) | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ captured original: `PicklingError` | ✓ | ✗ `UUID.__str__ is` original turns false |
| signature, `__doc__`, names, defaults | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ `signature` ValueError, `__doc__` lost | ✓ | ✓ with `functools.wraps` (**I**) |
| `__code__`, `inspect.getsource`, `isfunction` | n/a | n/a | n/a | ✓ | ✓ | ✗ TypeError, not a function | ✗ wrong source and code | ✗ the wrapper's (**I**) |
| traceback on fallback | CPython's | CPython's | CPython's | ✓ original frames only | ✓ | ✓ | ✗ extra shim frame | ✗ extra wrapper frame (**I**) |
| cProfile | same builtin entry (**I**: same object) | invisible (as in CPython) | invisible | ✓ `posixpath:join n=5` (falls back while profiling) | ✗ calls missing | ✗ shows a builtin `<posixpath.join>` | ✗ shows the shim | wrapper and original (**I**) |
| `sys.settrace` / `sys.monitoring` `PY_START` | n/a (C) | n/a | n/a | ✓ | ✗ no events | ✗ | ✗ shim frame | wrapper frame added (**I**) |
| local events on the code object (debugger breakpoint) | n/a | n/a | n/a | ✓ 11 LINE events (`_co_monitoring` check) | ✗ 0 | ✗ 0 | ✗ shim | ✓ original still runs (**I**) |
| specializer | ✓ `CALL_METHOD_DESCRIPTOR_*`/`CALL_BUILTIN_*` keep firing | reached only off the specialized paths (§3) | ✓ | callers lose `CALL_PY_*` inlining (+180–260 Ir); the function's own body stays specialized | same | ✓ `CALL_BUILTIN_FAST` | version cleared | type version bump |
| JIT (3.15) | ✗ stale traces unless `sys._clear_internal_caches()` (§4) | ✓ read per call (except `tp_iternext` `_ITER_NEXT_INLINE`, constant folding) | ✓ | ✓ version guards deopt stale traces (2000/2000 after patch, 0/2000 after unpatch) | ✓ | ✓ | ✓ | ✓ |
| free-threading (§5) | ✓ atomic store, 0.2 µs | ✓ | ✓ | ✓ stop-the-world: 0.2 µs idle, **1.1 ms** under load | ✓ | ✓ | stop-the-world | ✓ |
| subinterpreters (§6) | **process-global**: kernels run in every interpreter | process-global; wrappers of subinterpreters created during the swap keep the kernel | per interpreter: main only | per interpreter: main only | same | per interpreter | per interpreter | per interpreter |
| uninstall | exact restore (CAS) + clear JIT caches | exact restore (CAS) | exact restore (CAS) | ✓ with `func_version` restored (otherwise 1.23× forever on a trivial function) | same | ✓ | version lost | type version bump |
| startup | needs the type or module object: patch on import (§7) | at `enable()` | per import | lazy on import (§7) | | | | |

## 3. Reachability of builtin entry points by a pointer swap

`benches/mechanisms/reach.py` (**M**). It swaps in a counting pass-through and reports the
share of warmed-up executions that reach it. Operands are locals, so the tier-2 optimizer
cannot constant-fold them: with module globals it folds `'a' in s`, `s < t` and `d[k]`, and
the kernel shows 0.00 (**M**). Explanations come from a source read of 3.15.1 and 3.14.9
(**I**, file:line).

**3.15.1t is identical to 3.15.1 in all 79 cells.** JIT columns are explained in §4.

| entry point | swap | 3.15.1 | 3.14.9 | bypassed by (3.15.1) |
|---|---|---:|---:|---|
| `str.split/join/replace/startswith/strip/lower/encode/find/format`, `bytes.decode`, `list.pop/sort/index`, `dict.get`, `set.add`, `Struct.pack`, `datetime.isoformat/strftime` | `ml_meth` | 1.00 | 1.00 | none in tier 1 (B:5111/5191/5262/5333). Tier 2 copies the pointer (O:1863–1935, §4). |
| `f"{s:>5}"`, `f"{x:.2f}"` (`__format__`), `round(f, 2)` (`__round__`) | `ml_meth` | 1.00 | 1.00 | – |
| `list.append` | `ml_meth` | **0.00** | 0.00 | `CALL_LIST_APPEND` checks the callable's identity, then calls `_PyList_AppendTakeRef` (B:5075) |
| `len(l)`, `isinstance(n, int)` | `ml_meth` | **0.00** | 0.00 | `CALL_LEN`, `CALL_ISINSTANCE` (B:5007, B:5033) |
| `repr`, `getattr`, `sum`, `min`, `any(list)`, `math.sqrt`, `struct.pack`, `binascii.hexlify`, `pickle.dumps`, `_json.scanstring` | `ml_meth` | 1.00 | 1.00 | `CALL_BUILTIN_*` read `ml_meth` per call (B:4891) |
| `any(genexpr)` | `ml_meth` | **0.00** | 0.00 | the compiler inlines it behind an identity check (codegen.c:3992) |
| `'%s' % (n,)` (literal format, tuple display) | `str.nb_remainder` | **0.00** | 0.00 | the compiler rewrites it to an f-string (ast_preprocess.c:384) |
| `fmt % args` | `str.nb_remainder` | 1.00 | 1.00 | – |
| `fmt % args` on a `str` subclass defined after the swap | `str.nb_remainder` | **0.00** | 0.00 | `fixup_slot_dispatchers` sets the subclass slot from the wrapper's `d_wrapped`, the original (T:12078) |
| `s + t` | `str.sq_concat` | **0.00** | 0.00 | `BINARY_OP_ADD_UNICODE` calls `PyUnicode_Concat` (B:952) |
| `s == t` / `s < t` | `str.tp_richcompare` | **0.00** / 1.00 | same | `COMPARE_OP_STR`, then `_PyUnicode_Equal` (B:3307); ordering is reached |
| `hash(s)` / `d[s]` | `str.tp_hash` | 1.00 / **0.00** | same | dicts use the cached hash (`_PyObject_HashFast`) |
| `repr(s)`, `f"{s!r}"` | `str.tp_repr` | 1.00 | 1.00 | – |
| `str(s)`, `f"{s}"` | `str.tp_str` | **0.00** | 0.00 | `CALL_STR_1`/`FORMAT_SIMPLE`: an exact str returns itself |
| `for c in s` | `str.tp_iter` | **0.00** | **1.00** | new in 3.15: the private `_tp_iteritem` slot is used before `tp_iter` (ceval.c:1134) |
| `'a' in s` | `str.sq_contains` | 1.00 | 1.00 | – |
| `s[1]` / `s[1:3]` | `str.mp_subscript` | **0.00** / 1.00 | 0.00 / 1.00 | `BINARY_OP_SUBSCR_STR_INT` (B:1188) |
| `b % x`, `b[1]` | bytes slots | 1.00 | 1.00 | – |
| `n + n`, `f + f`, `n == 7` | `int`/`float` slots | **0.00** | 0.00 | `BINARY_OP_ADD_INT/FLOAT`, `COMPARE_OP_INT` |
| `hash(n)`, `repr(n)`, `str(n)`, `f"{n}"`, `repr(f)`, `f"{f}"` | `int`/`float` `tp_hash`/`tp_repr` | 1.00 | 1.00 | – |
| `x in l`, `l == l2`, `hash(t)`, `for k in d` | list/tuple/dict slots | 1.00 | 1.00 | – |
| `l[0]`, `t[0]` | `mp_subscript` | **0.00** | 0.00 | `BINARY_OP_SUBSCR_LIST_INT`/`TUPLE_INT` |
| `l + l2` | `list.sq_concat` | **0.00** | **1.00** | new in 3.15: `BINARY_OP_EXTEND` calls `_PyList_Concat` (S:2313) |
| `d[k]` | `dict.mp_subscript` | 1.00 (**the swap de-specializes `d[k]`**) | **0.00** | 3.15 guards compare `mp_subscript == _PyDict_Subscript` (S:2459, B:1280), so a swap sends every `d[k]` to the generic path; 3.14 guards on `PyDict_CheckExact` only |
| `k in d`, `x in st` | `sq_contains` | **0.00** | 0.00 | `CONTAINS_OP_DICT/SET` (B:3405, B:3387) |
| `json.loads('"abc"')` | `_json.scanstring` | **0.00** | 0.00 | the scanner calls `scanstring_unicode` directly (_json.c:786) |
| `json.dumps('abc')` / `json.dumps(l)` / `json.loads('[1]')` | `encode_basestring_ascii` / `make_encoder.tp_call` / `make_scanner.tp_call` | 1.00 | 1.00 | – |
| `re.match('a', s)` | `Pattern.match` / `Pattern.prefixmatch` | **0.00** / 1.00 | 1.00 / – | in 3.15 `re.match` calls `prefixmatch`, a separate `PyMethodDef` entry |
| `operator.itemgetter(0)(t)` | `tp_call` | **0.00** | 0.00 | per-instance vectorcall |

Rules that follow (**I** from the source read, confirmed by the measurements above):
- Tier 1 re-reads `ml_meth` and `PyCFunction` pointers on every call. A replacement must
  keep `ml_flags`: the descriptor picks its vectorcall wrapper from the flags when it is
  created (descrobject.c:966).
- Generic C-API paths (`PyNumber_*`, `PyObject_RichCompare/GetItem/Repr/Hash/GetIter`,
  `PySequence_Contains`) re-read slots on every call. Exact-type specializations call the
  internal functions directly. Only 3.15's dict subscript and store guards compare the slot
  pointer itself.
- Wrapper descriptors capture the slot when the type is readied. `str.__mod__(a, b)` and
  `super().__mod__` therefore run the original in the main interpreter. A static type's
  wrappers are rebuilt in each new interpreter, so they capture whatever the slot holds at
  that moment (§6).

## 4. Specializer and JIT

| effect | measured (**M**, `reach.py --jit`, `overhead.py --jit`, `jit_vectorcall.py`) |
|---|---|
| 3.15 tier 2 copies `ml_meth` into `_CALL_METHOD_DESCRIPTOR_{O,NOARGS,FAST,FAST_WITH_KEYWORDS}_INLINE` (O:1863–1935) | Traces built before the swap keep calling the **original** (0.00 for 14 methods). Traces built during the swap keep calling the **kernel after uninstall** (1.00). `sys._clear_internal_caches()` invalidates every executor of the interpreter and fixes both (1.00 / 0.00). 3.14.9 does not copy (1.00 / 0.00 without clearing). |
| builtin functions, `METH_VARARGS`/`METH_METHOD` methods, slots | read per call under the JIT (1.00 / 0.00) |
| `_ITER_NEXT_INLINE` copies `tp_iternext` (O:1474) | not measured; same remedy |
| constant folding of pure operations on watched globals | the kernel is never called; this is harmless because the result is CPython's |
| float results of number slots are assumed unique and mutated in place (O:332) | rule: number-slot kernels return fresh floats (**I**) |
| vectorcall-patched functions | Traces that inlined the call fail `_CHECK_FUNCTION_VERSION` and deopt. Patching a warmed loop: 2000/2000 kernel hits. Unpatching: 0/2000. Nothing stale. |
| `func_version` | cleared by `PyFunction_SetVectorcall` and `__code__`/`__defaults__` assignment; never re-assigned by CPython. Uninstall restores it with `func_version = 0; _PyFunction_SetVersion(f, code->co_version)` (exported "for external JIT support"): ident 1.23 → 1.00. |
| dict subscript (3.15) | swapping `dict.mp_subscript` makes `BINARY_OP_SUBSCR_DICT`'s guard fail: every `d[k]` in the process goes generic. `d['a']` costs 176 → 214 Ir (**1.214**) with a pass-through; 3.14.9 is 0.999 (kernel never reached). Do not swap it. |

## 5. Free-threading (3.15.1t)

`benches/mechanisms/ft_probe.py 2 300` (**M**): 2 threads call `s.split()`,
`posixpath.join`, `json.dumps` and `fmt % 1` (215 723 iterations) while the main thread
installs and uninstalls all four kinds of site 300 times, plus 50 idle cycles. **0 wrong
results, no crash.**

| site | install / uninstall, idle | under load: median | under load: max |
|---|---:|---:|---:|
| `ml_meth` `str.split` | 0.2 / 0.2 µs | 1.0 / 0.9 µs | 9.5 µs |
| static slot `str.nb_remainder` | 0.2 / 0.2 µs | 0.3 / 0.3 µs | 7.9 µs |
| heap slot `make_encoder.tp_call` | 0.2 / 0.2 µs | 0.2 / 0.2 µs | 0.7 µs |
| vectorcall `posixpath.join` (stop-the-world) | 0.2 / 0.2 µs | **1101 / 1104 µs** | **6482 µs** |

The GIL build takes 0.1–0.2 µs for every site (max 5.8 µs).

- The pointer stores are atomic. CPython's readers use plain loads: a formal C data race,
  benign for an aligned pointer on x86-64 and aarch64 (**I**).
  `PyFunction_SetVectorcall` and `__code__` assignment stop the world, so their cost grows
  with the number of running threads. Install at import or `enable()`, never in steady state.
- `PyMonitoring_EnterScope` state is shared per target. Concurrent writers store the same
  values, a benign race, as in Cython's use of the same API (**I**).
- Counters: contention numbers in §8.
- Kernel-internal rules (critical sections, references across Python code) stay as in
  TAKEOVER §7.

## 6. Subinterpreters (3.14.9, 3.15.1, 3.15.1t: identical)

`benches/mechanisms/subinterp_probe.py` (**M**):

| question | result |
|---|---|
| import `rustique._rustique` in an isolated subinterpreter (`concurrent.interpreters`) | `ImportError: module rustique._rustique does not support loading in subinterpreters`. PyO3 0.30.1 declares no `Py_mod_multiple_interpreters` slot. |
| the same in a legacy shared-GIL subinterpreter (`_interpreters.create('legacy')`) | **imports**: 3.14.9 fully, 3.15.1 half (`RuntimeError: rustique: init method missing`). PyO3's interpreter-id check does not stop it. PyO3 caches type objects in process statics, so this is unsafe: Rustique must refuse explicitly (`Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED`, or a check in module exec). |
| `ml_meth` swap made in main, called in a subinterpreter (`'a b'.split()` ×100) | **kernel runs** (107 hits: static tables are process-global) |
| static slot swap (`fmt % 1` ×100) | **kernel runs** (121 hits) |
| `ml_meth` swap of multi-phase C module functions and heap-type methods (`_json.encode_basestring_ascii`, `struct.pack`, `_sre` `Pattern.search` with `METH_METHOD`), ×100 each | **kernel runs**: 100 / 100 / 100 hits on 3.15.1 and 3.14.9. Every module instance's function objects and every interpreter's heap-type descriptors point into the same static `PyMethodDef` array. |
| heap-type slot (`json.dumps([1])` ×100) | 0 hits: each interpreter has its own `_json` types |
| vectorcall (`posixpath.join` ×100) | 0 hits: each interpreter has its own function objects |
| subinterpreter created *during* a slot swap: `str.__mod__('%s', 1)` ×10 | 10 kernel hits: its wrapper captured the kernel. Main: 0 (captured at startup). |
| the same after uninstall in main | **10 more kernel hits**: that subinterpreter keeps the kernel for its lifetime |
| shipped `rustique.enable()` kernels (list/tuple + `sum/min/max/sorted/any/all`) in a subinterpreter | correct on 12 checks, including keyword arguments and error messages |

Consequences:
- A kernel on a process-global site must be **interpreter-agnostic**. It holds no
  `PyObject` except immortal or static ones, and takes per-interpreter state from its
  arguments (module `self`, `defining_class`, the receiver). Otherwise it checks the
  interpreter (+30 Ir) and falls back.
- Kernel code and static data must live for the whole process: they can be called after
  `disable()`.
- Per-interpreter targets (Python functions, heap types) are accelerated in the main
  interpreter only, until PyO3 supports per-interpreter modules.

## 7. Install, startup and lazy activation

**Lazy activation without eager imports** (**M**, `test_lazy_patch_on_import_without_eager_imports`):
- importlib inserts a module into `sys.modules` before executing it. After `exec_module` it
  pops the module and inserts it again (`_bootstrap._load_unlocked`, lines 926–927).
- One dict watcher on `sys.modules` therefore sees a fully executed module on the second
  ADDED event for a target name, and patches it then.
- Module dicts are never watched. Non-target imports cost one callback (a string compare).
- Already imported modules are patched at `enable()`.

`benches/mechanisms/startup.py`, 3.15.1 (**M**). Targets: posixpath, json, copy, uuid,
logging, enum, pathlib, `_strptime`, ipaddress.

| cell (callgrind region) | Ir |
|---|---:|
| `enable()`, eager: import the 9 target modules and patch them | 77 951 681 |
| `enable()`, lazy, dict watcher on `sys.modules` (`_proto.lazy`) | **31 975** |
| `enable()`, lazy, `sys.meta_path` finder (Python; imports `importlib.abc`/`importlib.util`) | 4 357 857 |
| import 25 non-target stdlib modules: plain / dict watcher | 117 264 167 / 117 373 455 (**+0.09%**) |
| `import json` (a target): plain / dict watcher (patch included) / meta_path | 23 593 402 / 23 613 699 (+20k) / 23 967 226 (+374k) |

Whole process, `python -c …`, median of 30 interleaved runs (p10–p90), wall time:

| | ms |
|---|---|
| plain | 9.96 (9.54–10.32) |
| `import rustique` | 11.17 (10.66–11.93) |
| `+ lazy(9 targets)` | 11.31 (10.99–12.06) |
| `+ eager import and patch of the 9 targets` | 30.15 (29.45–31.88) |

The meta_path variant's "25 other modules" figure (116.6M) is not comparable: setting up
the finder had already imported some of their dependencies.

Other hooks considered (**I**):
- **Function watchers** (`PyFunction_AddWatcher`): fire on *every* function creation in the
  process (every `def`, lambda and closure, including those in hot loops), and are not told
  about `PyFunction_SetVectorcall`. Not needed: the `sys.modules` signal suffices.
- **Type watchers** (`PyType_Watch`): the right tool for dependencies that are class
  attributes (`JSONEncoder.default` replaced on the class, a patched method of a patched
  class).
- **meta_path finder**: works, but costs 4.4M Ir to set up (importing
  `importlib.abc`/`importlib.util`) and +374k Ir per target import. It also runs Python
  code on every import miss.

Watcher budget per interpreter (**I**, pycore_dict.h:297): 8 dict watcher ids, of which
CPython uses 3 (builtins, globals, module). Type watcher 0 is the optimizer's.
Rustique needs one dict watcher, shared by lazy activation and the dependency guards (§8).

## 8. Guards and reporting

| guard | how | cost |
|---|---|---|
| exact subset | `Py_IS_TYPE` on each argument (never `PyX_Check`), arity, `kwnames == NULL` or known interned names | a few Ir |
| monkeypatched dependencies (`os.fspath`, `posixpath._get_sep`, `json.encoder.c_make_encoder`, `copyreg.dispatch_table`, `time.strptime`) | pin `(dict, key, object)` at install. A dict-watcher callback maintains one `AtomicBool` per target, re-armed when the original object is put back (`mock.patch` exits). | 1 relaxed load (**M**: deps fallback 1.058 measured; flips on assignment and re-arms on restore, tested). Without a watcher, each dependency costs a dict lookup per call (~30–50 Ir, **I**). |
| tools listening (Python targets only) | `PyMonitoring_EnterScope` for global `PY_START` (covers `settrace`, `setprofile`, cProfile, coverage sysmon), plus `code->_co_monitoring` NULL or all local tools 0 (breakpoints) | 34 + ~4 Ir (**M**) |
| other interpreter (process-global sites with interpreter-bound state) | `PyInterpreterState_Get() == main` | 30 Ir (**M**): design kernels not to need it |
| recursion | match CPython's depth check, or fall back before recursing | – |

Reporting:
- Per-target `hits` and `fallback[args | deps | tools | interp | deopt]` counters
  (`accel/stats.rs`).
- Off by default: +3 Ir per call for the relaxed load of the global flag (**M**, 1.004 on
  `s.split()`).
- On: +1 Ir more (relaxed `fetch_add`).
- Contention under free-threading (`benches/mechanisms/contention.py 2`, 3.15.1t, 2
  threads calling `s.split()`, median of 5 runs, wall time, **M**):

  | | ns per call |
  |---|---:|
  | orig | 23.5 |
  | pass-through | 23.3 |
  | stats off | 27.2 (min 23.3: noise) |
  | stats on, one shared counter | **31.1** (+8 ns: cache-line ping-pong) |

  "Stats on" is a diagnostic mode. If it ever needs to be cheap in production, shard the
  counters per thread.

## 9. Version refusal

- **Loader.** An extension built as `cpython-315-x86_64-linux-gnu` loads only into 3.15
  builds with the same ABI flags. Debug builds use the `d` suffix, and `t` marks
  free-threaded builds. PyO3 0.30.1 cannot build for 3.16 at all: its max is 3.15
  (`pyo3-ffi/build.rs:41`), short of abi3 forward compatibility. What reaches run time is
  other 3.15.x patch releases and differently configured 3.15 builds (**I**).
- **Per target, at `enable()`** (`accel/version.rs`, **M**: `supported([14])` on 3.15.1
  returns "CPython 3.15 not validated"):
  1. `Py_Version` major.minor is in the target's validated set, and the releaselevel is
     final (no alphas or RCs).
  2. Not a debug build (`sys.gettotalrefcount` absent).
  3. Layouts calibrated against the running interpreter:
     - `PyFunctionObject.vectorcall` offset equals `tp_vectorcall_offset` (136 / 136);
     - the `PyCodeObject` prefix up to `_co_monitoring` matches the `co_*` attributes;
     - the str, int and float layouts as in TAKEOVER §7.
  4. A **fingerprint of the original**:
     - Python targets: a hash of the canonical code (`co_code`, names, varnames, flags,
       argument counts, recursive consts, exception table; path- and line-independent).
       Measured identical on 3.15.1 and 3.15.1t, different on 3.14.9; 22 µs per function (wall time, one run)
       (`benches/mechanisms/fingerprint.py`).
     - C targets: `ml_name` + `ml_flags` + a `ml_doc` hash, plus a behavioural self-test
       on probe inputs (kernel vs original).
  5. Any mismatch refuses **that target only**, with the reason in `rustique.status()`.
- **Names drift.** `_strptime._strptime_datetime` (named in the brief) does not exist in
  3.14 or 3.15; it is `_strptime_datetime_datetime` (and `_date` / `_time`). `re.match`
  became `prefixmatch` in 3.15. Targets are keyed per minor version.

## 10. Draft rules for docs/ACCELERATION.md

**Install**
1. `enable()` is one idempotent call, and all-or-nothing per target. A target is a set of
   sites plus its guards and its kernel.
2. A site installs with compare-and-swap from the original captured at validation. If
   someone else already changed it, the target is refused.
3. Before installing: run the version gate, layout calibration and fingerprint (§9). A
   failure refuses the target, never the whole of `enable()`.
4. C functions and methods: `ml_meth`, with the same `ml_flags`. Patch every alias entry.
5. Slots: only slots that no specializer compares by pointer.
6. Python functions and methods: `PyFunction_SetVectorcall` on the object in the module or
   class `__dict__`. Never replace attributes, `__code__` or class dict entries.
7. Never import a module to patch it. Patch what is already imported, and the rest on the
   post-exec re-insert into `sys.modules` (one dict watcher).
8. After any `ml_meth` swap, and on `disable()`: `sys._clear_internal_caches()`.
9. On uninstall, restore the pointer (CAS) and the function's `func_version`.
10. Free-threaded builds: install at startup or import. `PyFunction_SetVectorcall` stops
    the world (≈1 ms with busy threads).
11. Refuse to load in any subinterpreter. Process-global kernels must be
    interpreter-agnostic and must live as long as the process.

**Guard.** Decide the exact subset before anything observable happens.
1. Exact types and argument shapes only. For slots, any receiver that is not of the exact
   type goes to the original (C subtypes inherit the kernel).
2. Dependencies the original reads at call time are pinned with the dict watcher; the
   per-call cost is one relaxed load.
3. Python targets: if a tool listens (global `PY_START`, or local events on the code), call
   the original, so tools see CPython's frames.
4. Budget ≤ 40 Ir per call for all guards together. Measured: 38 Ir for `posixpath.join`.

**Fallback**
1. Call the saved original pointer with the unchanged arguments, never the patched object
   again. A restart is allowed only while nothing observable has happened. After Python
   code has run, continue CPython's algorithm from the same state (TAKEOVER §7).
2. The kernel never raises errors of its own. CPython's original raises them, with its
   message and traceback.
3. Never slower:
   - `ml_meth` and slot sites cost +2–5 Ir and are always fine.
   - Vectorcall sites cost +180–260 Ir (+250–350 under the JIT), so a Python target
     qualifies only if its fallback-path calls cost ≥ ~5k Ir (≈ 1.5 µs; ~7k under the JIT)
     or its fast path covers the inputs that make it cheap.
   - Recursive targets must handle recursion inside the kernel: `deepcopy` pays per level.
4. Results: same identity rules as CPython (return the argument or singleton where CPython
   does, `PyUnicode_Concat` order). Number slots return fresh floats.

**Report**
1. Per-target hit and fallback-reason counters, off by default (+3 Ir). Turned on with
   `RUSTIQUE_STATS=1` or `rustique.stats(True)`.
2. `rustique.status()` lists each target: installed or refused (with the reason), the
   fingerprint, and its sites.

## 11. Reproduce

```bash
git -C ~/git/philipsolarz/rustique worktree add ../rustique-wt/p1-mechanisms p1/mechanisms
cd ../rustique-wt/p1-mechanisms
CARGO_TARGET_DIR=$PWD/target-mech maturin develop --profile dev-release   # per venv (3.15, 3.15t, 3.14)
B=benches/mechanisms
python $B/reach.py [--jit] -o reach.json        # §3, §4
python $B/overhead.py [--jit] -o overhead.json  # §1
python $B/compat.py -o compat.json              # §2
python $B/ft_probe.py 2 300                     # §5 (3.15t)
python $B/subinterp_probe.py                    # §6
python $B/startup.py -o startup.json            # §7
python $B/contention.py 2                       # §8 (3.15t)
python $B/fingerprint.py                        # §9
PYTHON_JIT=1 python $B/jit_vectorcall.py pre|post|pre_nested|post_nested   # §4
python -m pytest tests/test_mechanisms_proto.py
```
