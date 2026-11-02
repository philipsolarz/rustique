STATUS: wave 2, workstream `core`. Free-threaded builds only.

# Group `set`

One target, `set.__reduce__`: the `ml_meth` of the `set.__reduce__` and
`frozenset.__reduce__` method table entries (one C function, two `PyMethodDef`s; both are
patched, all-or-nothing). Code: `crates/rustique-py/src/targets/set/mod.rs`. Tests:
`tests/targets/test_set_reduce.py`.

## Why (the free-threading defect it removes)

On every free-threaded CPython from 3.14 to 3.16a1, `set.__reduce__` is an Argument Clinic
`@critical_section` method: it runs with the set's per-object lock held, and its body calls
`PySequence_List(so)`, whose `_list_extend` takes `Py_BEGIN_CRITICAL_SECTION2(list, so)`.
The two-mutex fast path fails on the set's mutex, held by the same thread, and
`_PyCriticalSection2_BeginSlow` only skips re-locking when the previous section was a
two-mutex section on the same pair: the thread contends with itself (`PyMutex_Lock` spins
40 × `sched_yield`, then parks; detaching suspends the outer section and the lock is taken).
About 34.5 µs per call instead of 0.6 µs (measured, `docs/atlas/w2-threads.md` §1.2).
Reached by `copy.deepcopy` of sets and frozensets (with the `copy` group off; its native
engine emulates `set.__reduce__` itself), `copy.copy` of set subclasses, pickling at
protocol < 4 and of set subclasses, `dataclasses.asdict` leaves, pydantic deep copies.

## Exact subset and fallbacks

All receivers (the method's receiver is always a `set`/`frozenset` instance, subclasses
included): the kernel is CPython's body, call for call, without the clinic wrapper's
critical section:

1. `keys = PySequence_List(so)` (the same list, grown by `list_resize` exactly as the
   original's: same capacity, same `sys.getsizeof`; it takes its own (list, set) critical
   section, now uncontended; a subclass's `__iter__` runs as in CPython);
2. `args = (keys,)`;
3. `state = _PyObject_GetState(so)` (exported by CPython; one `__getstate__` lookup, user
   `__getattribute__`/`__getstate__` included, default state from `__dict__` and slots);
4. `(type(so), args, state)`.

Errors are CPython's own, raised at the same points; the kernel never restarts (user code
may have run). No fallback path exists, so no fallback cell.

Not observable: the set's lock is no longer held while the state is read. It guards only
the set's hash table, which `_PyObject_GetState` does not read (the instance `__dict__` has
its own lock), and code run by a subclass's `__iter__`/`__getstate__` cannot tell whether
the set's lock is held (CPython itself releases it whenever such code blocks).

Private state skipped: none.

## GIL builds: refused by design (category `build`)

With the GIL the clinic critical section is a no-op, so the kernel makes exactly the calls
the original makes: callgrind cells 0.993–1.008 (measured in Phase 1, `w2-threads.md`
§4.1: `set()`, `{1..5}`, `set(range(100))`, `frozenset`, subclass with `__dict__`,
`deepcopy(frozenset)`, `deepcopy(set)`, `pickle.dumps(set, 2)`). No gain, so the target
refuses on GIL builds (`status()`: `refused`, category `build`) instead of adding a site and
install work.

## Install

`module: None` (installed at `enable()`), validated on 3.14 and 3.15 free-threaded builds.
Fingerprint of both entries (`ml_name`, `ml_flags`, `ml_doc`, format v2): `9794b5c54ea800a6`
on 3.14 and 3.15. `_PyObject_GetState` resolved by symbol (`symbol` refusal if missing).
Install self-test: the kernel and the original on instances of a `set` subclass with
`__slots__ = ()` (empty and `{1, 2, 3}`) and of a `frozenset` subclass with an instance
attribute must return equal results. The probe classes cache `__slotnames__ = []`: the
default state of an exact set always enters `copyreg._slotnames`, which would import
`copyreg` at `enable()` (`enable()` loads no module, `tests/test_dx.py`).

When CPython fixes the defect upstream (a one-line change; see `w2-threads.md` §1.2), the
target should be restricted to the patch releases that still have it.

## Measured

T7 (free-threading target, 3.15.1t, dev box, `run.py compare -n 3 -p 3`, wall time,
digests identical), **measured**:

| workload | `--only set` | every group but `copy` | T7 bar |
|---|---:|---:|---:|
| `devtools/isort_stdlib` | 1324.0 → 348.9 ms, **0.263** | **0.261** | ≤ 0.35 |
| `data/state_snapshot` | 1579.4 → 1129.2 ms, **0.715** | **0.699** | ≤ 0.75 |

8-thread CPU per call, enabled ÷ plain (`kernel_scaling.py -t 1,8`): `s.__reduce__()`
0.032 (per-thread sets) / 0.016 (one shared set). Per call (Phase 1, wall ns): `s.__reduce__()` 39,036 → 794
(0.020), `deepcopy(frozenset)` 44,044 → 3,258 (0.074).

## Conformance

`tests/conformance/run_target_suites.py --targets set --modules
test.test_set,test.test_copy,test.test_pickle` (`benches/targets/core_conformance.sh`),
same outcome for every test id with and without the target, **measured**: 3.15.1t
test_set 644 pass, test_copy 84, test_pickle 1039 + 47 skip; 3.14.9t 630 / 81 / 960 + 55
skip; 0 differences. On GIL builds the target is refused (test_set 644 / 630, identical).
