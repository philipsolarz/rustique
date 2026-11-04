STATUS: wave 1, group `logging` (branch `ws/logging`), 2026-11-01; wave 2 (workstream `text`,
2026-11-02: §0). Validated on CPython 3.14.9, 3.14.9t, 3.15.1, 3.15.1t. Code: `crates/rustique-py/src/targets/logging/`,
`crates/rustique-core/src/logging/`. Tests: `tests/targets/test_logging*.py`. Benchmarks:
`benches/targets/logging_cg.py`, `benches/targets/logging_suite_stats.py`.

# Group `logging`: the default record path, piecewise

## 0. Wave 2 (2026-11-02)

With wave 1 on, logging was still 53% of `web/logging_service`, 21% of `threads/shared_cache`
and 2–10% of the services (w2-attribution §6). What changed (callgrind, 3.15.1,
`benches/targets/logging_cg.py`, Ir per statement; workloads: `benches/targets/text_wl.py`,
user-space instructions per steady-state `run()`, all digests identical):

| change | mechanism | cell before → after (CPython) |
|---|---|---|
| `findCaller` line numbers | the wave-1 memo hashed only the low bits of `lasti` (unrolled call sites thrashed it: 79% of the call was `_PyCode_CheckLineNumber`) and was off on free-threaded builds; now a per-thread table keyed by `(code, co_version, lasti)` with a multiplicative hash, every build | 4,027 → 868 (12,491) |
| `LogRecord.__init__` stores | the 21 attributes go straight into a fresh record's inline values (slot + insertion-order byte, as `store_instance_attr_lock_held` does when there is no `__dict__`), GIL builds; verified once per process against `PyObject_GenericSetAttr` on the first real record (slots, order array, managed-dict offset), otherwise off | 7,574 → 5,037 (19,673) |
| `Formatter.format` checks | instance-shadowing checks (`usesTime`/`formatTime`/`formatMessage`/`getMessage`/the style's methods) read the inline values instead of `_PyObject_GetMethod`; class attributes cached per type version (GIL builds); `usesTime` answered once per format plan; record attributes read through the validated class; no buffer zeroing | 9,674 → 7,704 (23,760) |
| format-plan cache | the global `parking_lot` mutex (`FMT_CACHE`, the 8-thread cliff: CPU ×3.25) becomes an insert-only lock-free table keyed by the `_fmt` object (64 entries, kept for the process; a full table parses per call) | 8-thread CPU per call 1.01 → 0.45 of CPython's |
| C `Decimal` arguments | an exact `decimal.Decimal` of a loaded `_decimal` (immutable C type; `str`/`repr`/`int`/`float` are C, reading the thread's decimal context as CPython's own `str()` does) is a plain value for `%`-formatting and the `Mapping` answer of `LogRecord`; `shared_cache`'s `Formatter.format` had been suspended by them | `info('total %s', Decimal)` 69.1k → 23.8k |
| `Logger.level` enabled calls | an enabled call costs the kernel's checks + the vectorcall fallback (~+800 Ir), a disabled one saves ~500: enabled calls now debit the score (credit 5 / debit 7), so a program logging mostly enabled records (services) suspends the target and pays nothing; GIL builds only (8 threads, below) | services: 0 overhead after 64 calls |
| `Handler.handle` (new target) | `StreamHandler` → exact `io.StringIO`, no filters, exact `_thread.RLock`, a formatter `Formatter.format`'s kernel handles: filter/lock/emit/format/write/flush/unlock natively (§0.1) | `_HS.handle(rec)` 29.6k → 10.1k; a record to StringIO 70.7k → 23.1k |

Records end to end (StreamHandler to StringIO): `info` with `%(asctime)s …` **0.327** (wave 1
0.453), `%(message)s` 0.348, `LoggerAdapter` 0.402, a file stream 0.408. Fallback cells
(steady/alternating) ≤ 1.016; the `Handler.handle` fallback for a file stream is 0.533
steady (the other targets still run) and 0.456 alternating with a StringIO handler.

Workloads (3.15.1, all groups on, ratio of instructions, wave 1 → wave 2):
`web/logging_service` 0.737 → **0.709**, `services/job_worker` 0.638 → **0.612**,
`threads/shared_cache` 0.614 → **0.532**.

**8 threads** (3.15.1t, `benches/targets/text_scaling.py -t 1,8`, CPU time per call at 8
threads, enabled ÷ CPython, local / shared inputs; a loaded shared box, so ±0.2):
`Formatter.format` 0.56 / 0.51 (wave 1: 1.010, the `FMT_CACHE` mutex), `LogRecord` 0.45 /
0.38, `findCaller` 0.21 / 0.27, a record to StringIO (with `Handler.handle`) 0.53 / 0.53, a
Decimal argument 0.63 / 0.43. **`Logger.level` is not installed on free-threaded builds**:
its `isEnabledFor` shadowing check (`_PyObject_GetMethod`) takes a reference to the shared
function object on every call, which CPython's specialized call avoids, and a disabled
`debug()` cost 2.1–3.2× CPython's CPU per call at 8 threads with per-thread loggers (three
runs), for ~200 Ir saved at one thread. Every installed logging kernel meets the ≤ 1.05
rule.

**Install** (`benches/targets/logging_install_cost.py`, 3.15.1): `import logging` with the
group pending 27.65M vs 27.92M plain (accel's deferred activation; within the 5% budget);
`_accel.enable(['logging'])` with `logging` imported 0.64M cold / 0.22M warm (wave 1:
0.51M / 0.14M): the sixth target adds five fingerprinted functions and eight captured
objects. The warm per-group cost stays the documented wave-1 overage (framework-side
lever: validate on first call).

### 0.1 `logging.Handler.handle`

Exact subset (anything else runs CPython's `Handler.handle`, before anything observable):
the handler's class resolves `filter`/`emit`/`format`/`flush` to `Filterer.filter`,
`StreamHandler.emit`, `Handler.format`, `StreamHandler.flush` (fingerprinted; no instance
shadows them); `filters` an exact empty list; `lock` an exact `_thread.RLock`; `stream` an
exact `_io.StringIO` with no instance `__dict__`; `terminator` an exact str; the formatter
(or `_defaultFormatter`, pinned) a class whose `format` is `Formatter.format` and whose
instances are always true, and the record one `Formatter.format`'s kernel formats natively
(`isinstance(record, LogRecord)` with `LogRecord` pinned and of metaclass `type`).

Order of effects, as CPython's: the lock is taken with `acquire(False)` (never waits, so no
signal handler can run below the kernel; a lock held by another thread falls back and
CPython blocks in its own frames), the record's `message`/`asctime` are stored, the line is
written with `StringIO.write`, `flush` re-enters the lock and calls `IOBase.flush`, both
locks are released, the record is returned. A format the kernel declines after the lock
was taken, or a failed `write` (a closed stream: StringIO checks before writing), releases
the lock and restarts in CPython's original, which repeats the stores with equal values and
reports the error through `handleError` from its own frames. `flush` can only fail if
another thread calls `IOBase.close(stream)` concurrently (there was no instance `__dict__`
holding `__IOBase_closed`): that close is ordered after the flush.

**Not accelerated:** file and console streams (`TextIOWrapper`): an `OSError` in their
`write`/`flush` can happen after bytes reached the OS, and `handleError` prints a traceback
through `emit`'s and `flush`'s frames (and walks `tb_frame.f_back`), which a kernel cannot
reproduce; handlers with filters (user code); `Logger.handle`/`callHandlers` (they call
every handler's `handle`, which may be user code). In `logging_service` and `job_worker`
every handler writes to a file or has a user filter/formatter: `Handler.handle` falls back
there and suspends after 64 calls. The remaining logging time there is the flush syscall per
record (CPython's semantics), `os.getpid()` (a syscall per record in CPython too) and the
user filters/formatters (ruling 15).


`logging` is in 40% of popular packages and owns 66% of the `web/logging_service` workload
(`docs/atlas/workloads-web.md`). Its hook surface is the widest of wave 1 (record
factories, logger classes, formatter subclasses, converters, filters, handlers, module
switches), so the group accelerates the default record path **piece by piece**, each piece
a pure-Python function whose vectorcall is swapped (contract §1, kind C), and only where
no Python code runs below the kernel.

## 1. Targets

| target | sites | the kernel |
|---|---|---|
| `logging.Logger.level` | `Logger.debug`, `.info`, `.warning`, `.error`, `.critical`, `.log` | decides `self.isEnabledFor(level)` from `self.disabled` and the level cache; a **disabled** call returns `None`; an enabled call runs CPython's method |
| `logging.Logger.findCaller` | `Logger.findCaller` | the frame walk with `_is_internal_frame` semantics, `(co_filename, f_lineno, co_name, None)` |
| `logging.LogRecord.__init__` | `LogRecord.__init__` | every record attribute, stored in CPython's order |
| `logging.Formatter.formatTime` | `Formatter.formatTime` | `converter(created)` + `strftime` + `default_msec_format` |
| `logging.Formatter.format` | `Formatter.format` | `getMessage()` + `usesTime()` + `formatTime()` + `PercentStyle.format()` |
| `logging.Handler.handle` (wave 2) | `Handler.handle` | filter + lock + emit + format + write + flush for a `StreamHandler` to `io.StringIO` (§0.1) |

All five are module targets (`module: "logging"`: installed when `logging` is imported,
lazily), validated on 3.14 and 3.15 including free-threaded builds, `jit_safe`.
`level`, `findCaller` and `formatTime` are `checkable` (their exact subset has no side
effects); `__init__` and `format` store into the record.

**Not accelerated, on purpose:** `Logger._log`, `makeRecord`, `handle`, `callHandlers`,
`Filterer.filter`, `Handler.handle`/`emit`/`format`/`flush`, `LoggerAdapter`,
`StrFormatStyle`/`StringTemplateStyle`, `formatException`, `formatStack`,
`LogRecord.getMessage` as a separate site. Each of them calls user code (filters,
handlers, streams, factories, formatter overrides, `__str__` of arguments): replacing it
would run user code below a kernel, without CPython's frames on the stack, so stack
inspection, `findCaller`-style depth counting and the tracebacks `Handler.handleError`
prints could differ. `getMessage` on its own would cost +200 Ir per fallback on a ~2k Ir
call (1.10) whenever an argument is not plain.

## 2. Exactness: the rules every kernel follows

1. **No Python code below a kernel.** A kernel reads attributes through the generic
   protocol of *validated* classes, does dict lookups whose keys cannot run `__eq__`,
   calls C functions only (`time.localtime/gmtime/strftime`, `_asyncio.current_task`,
   `getpid`, `localtime_r`), and formats only *plain* values (exact `str`, `int`, `bool`,
   `float`, `None`). Anything else falls back to CPython's original **before anything
   observable happened**, so the original's frames run the user code (and produce the
   tracebacks `handleError` prints, which tests compare).
2. **Errors re-run the original.** A `%`-format error, a missing key (`ValueError:
   Formatting field not found`), a `strftime` error: the kernel saw that no user code ran,
   discards its work and calls the original, which raises with CPython's message and
   traceback. The only stores a kernel may have made by then are `record.message` /
   `record.asctime` (format), which the original repeats with equal values (same keys,
   same order).
3. **Validated classes** (`TypeCache`): a class is accepted when its MRO resolves every
   name the kernel relies on to CPython's own function (`isEnabledFor`, `getMessage`,
   `usesTime`, `formatTime`, `formatMessage`, `PercentStyle.format/_format/usesTime`,
   `Thread.name`, `BaseProcess.name`), every attribute it reads or stores is absent from
   the class or a plain value (no descriptor), and `__getattribute__`/`__setattr__` are
   the generic ones. Subclasses that override nothing relevant are accepted (a
   `class MyFormatter(Formatter): pass`); the others fall back. The verdict is cached per
   `(type, tp_version_tag)`: any class modification (attribute assignment such as
   `Formatter.converter = time.gmtime`, `__bases__`) revalidates.
4. **Instances may shadow methods**: `self.formatTime`, `self.usesTime`,
   `self._style.format`, `record.getMessage`, `logger.isEnabledFor` are checked with
   `_PyObject_GetMethod` (what `self.x(...)` calls); an instance attribute
   (`mock.patch.object(logger, "isEnabledFor")`, `record.getMessage = ...` via `extra`)
   falls back.
5. **Dependencies are pinned** (dict-watcher pins of the registry; dependencies several
   targets share are declared once in the group's `GroupSpec`):
   `logging.time/os/sys/threading/collections`, `getLevelName`, `_levelToName`,
   `_nameToLevel`, `_srcfile`, `currentframe`, `_is_internal_frame`, `_startTime`,
   `logThreads`/`logProcesses`/`logMultiprocessing`/`logAsyncioTasks`, the level constants,
   `time.time_ns`/`time.strftime`, `os.path`, `os.getpid`, `os.fspath`,
   `posixpath.basename/splitext/normcase/_get_sep/os/genericpath`, `genericpath._splitext`,
   `threading.get_ident/current_thread/_active`, `sys.modules`, `sys._getframe`,
   `collections.abc.Mapping`, and the builtins the code reads (`isinstance`, `len`,
   `hasattr`, `int`, `str`, through `pin_global`, so a shadowing module global counts).
   A replaced dependency (`mock.patch("time.time_ns")`, `logging._srcfile = None`)
   suspends the target until it is restored; one declared in the group's `GroupSpec`
   (`logging.time/os/sys`, `os.path`, `os.fspath`, `posixpath.os`, `time.strftime`,
   `isinstance`, `int`) suspends all five (CPython's code, 1.00). `Formatter.converter` is not pinned: it is
   read per call and accepted when it *is* `time.localtime` or `time.gmtime` (by identity),
   so `Formatter.converter = time.gmtime` stays accelerated and a custom converter falls
   back.
6. **Every function a kernel stands in for** (the patched one and the helpers it inlines:
   `getLevelName`, `posixpath.basename/splitext/normcase`, `genericpath._splitext`,
   `threading.current_thread`, `Thread.name`, `_is_internal_frame`, `currentframe`,
   `usesTime`, `formatMessage`, `PercentStyle.*`, `getMessage`,
   `multiprocessing.process.current_process`, `BaseProcess.name`) is fingerprinted per
   minor version (canonical code hash; C functions by `ml_name/ml_flags/ml_doc`), and its
   `__code__`/`__defaults__` are checked per call (assignment falls back).
7. **Tools**: any `sys.monitoring` tool listening to *any* global event (the framework's
   `accel::Tools`: line-only, raise-only tools too), or locally on any of those code
   objects (`accel::code_locally_quiet` on each: breakpoints; 16 local events on 3.15, 11
   on 3.14), makes every kernel fall back, so tracers, profilers, coverage and debuggers
   see CPython's frames.
8. **Ruling 15** (restart before user code): no kernel calls user code. Every input that
   would reach user code (a handler, filter, stream, factory, formatter or style override,
   an argument's `__str__`, a converter function) is recognised before anything observable
   and runs CPython's original from the start.
9. **Attribute reads** of validated objects use `PyObject_GenericGetAttr`; GIL builds read
   the instance's inline values directly (`_PyObject_InlineValues`, index in the type's
   shared keys, falling back to the class's plain value), as
   `_PyObject_TryGetInstanceAttribute` does — the layout (pycore_dict.h, pycore_object.h,
   identical in 3.14 and 3.15) is calibrated at install against `PyObject_GenericGetAttr`
   on `logging.root`, and a failed calibration only turns the fast reads off. Stores use
   `PyObject_GenericSetAttr`.

### Per target

**`Logger.level`.** Exact subset: `self` of a validated logger class, `level` an exact
`int` (the module constant for `debug`..`critical`), no keyword naming a positional
parameter (CPython raises "multiple values"), `self.isEnabledFor` CPython's, `disabled` a
plain truth value, `_cache` an exact dict whose keys are all exact ints (scanned: a level
cache holds a handful of entries; so the lookup runs no `__eq__`) holding the level. Disabled → `None` (no frame, no `*args`/`**kwargs` objects).
`Logger.isEnabledFor` itself is not a site (≈600 Ir when called directly: 0.97 on GIL
builds, 1.36 on free-threaded ones, where attribute reads cannot use inline values). A cache miss (CPython takes
`logging._lock` and computes `getEffectiveLevel`) and every enabled call run the original;
both are counted as non-streak fallbacks (an enabled call is not a failure: the kernel
decided, and costs ≈ 1% of the record).

**`Logger.findCaller`.** Exact subset: `stack_info` `False`/`None`, `stacklevel` an exact
int in 1..2³⁰ (`stacklevel < 1` returns findCaller's own frame in CPython, which the
kernel does not have), a Python frame below the call. The walk starts at the caller's
frame (what `sys._getframe(1).f_back` is in CPython), materialises the same frame objects
(`PyFrame_GetBack`), compares `co_filename` with `_srcfile` and looks for
`importlib`/`_bootstrap`. `f_lineno` is `PyFrame_GetLineNumber`; for untraced frames
(`f_lineno` field 0) GIL builds memoise `PyCode_Addr2Line(code, lasti)` per
`(code, co_version, lasti)` — `co_version` is unique per code object, so a reused address
cannot alias. `stack_info=True` (traceback formatting) runs the original.

**`LogRecord.__init__`.** Exact subset: positional arguments only (8–10), `self` of a
validated record class, `args` an exact tuple, `level` an exact int or `None`
(`makeLogRecord`), `pathname` an exact str. Values: `time.time_ns()` → `PyTime_Time`
(what `time_ns` calls), `created = ct / 1e9` (int → double correctly rounded, as
`PyLong_AsDouble`), `msecs = (ct % 10**9) // 10**6 + 0.0` with the gh-89047 reset,
`relativeCreated = (ct - _startTime) / 1e6`; `getLevelName` (`_levelToName.get(level)`
once every key is verified to be an exact int — re-verified only when the dict changes,
through the registry's whole-dict generation watch —, `_nameToLevel` with exact-str keys,
`"Level %s" % level`);
`basename`/`splitext` on the str (`rustique_core::logging::split_path`, result identity
kept: `filename is pathname` when there is no `/`); `threading.get_ident()` →
`PyThread_get_thread_ident`; `current_thread()` → `threading._active[ident]` (a thread
not registered there falls back: CPython would create a `_DummyThread`), `.name` → the
validated `Thread.name` property's `_name`; `processName`: `'MainProcess'`, or with
`multiprocessing` imported `multiprocessing.process._current_process._name` once
`current_process` and `BaseProcess.name` are validated (fingerprints, identities);
`process` → `getpid()`; `taskName`: with asyncio imported, `_asyncio._get_running_loop()`
(no loop: CPython's `current_task()` raises and the exception is swallowed → `None`),
`_asyncio.current_task()` and `Task.get_name()` for an exact `_asyncio.Task`.
`isinstance(args[0], collections.abc.Mapping)` for a single argument is **never computed
by the kernel** (the ABC machinery can run user `__subclasshook__`s): it answers from what
CPython answered for that argument type at the current `abc.get_cache_token()` (CPython's
answer is stable while the token is: positive and negative ABC caches), learned by running
the original once per (type, token) and reading back `record.args`. The attributes are
stored with `PyObject_GenericSetAttr` in CPython's order, so `record.__dict__` has the same
keys in the same order (JSON formatters dump it).

**`Formatter.formatTime`.** Exact subset: `self` of a class whose `converter`,
`default_time_format`, `default_msec_format` are plain values; record class validated;
`converter` is `time.localtime` or `time.gmtime`; `record.created` an exact float;
`datefmt` `None` or a str. The broken-down time comes from the same `localtime_r` /
`gmtime_r` call (floor to `time_t`, as `_PyTime_ObjectToTime_t(ROUND_FLOOR)`). Formats
made of `%Y %m %d %H %M %S %y %j %%` and ASCII literals (years 1000–9999) are produced
natively (`rustique_core::logging::strftime_numeric`; these directives are
locale-independent zero-padded numbers in glibc and BSD libc); any other format calls
`time.localtime`/`time.strftime` themselves. `'%s,%03d' % (s, msecs)` is native for a
float `msecs` in [0, 1000), otherwise `PyUnicode_Format`.

**`Formatter.format`.** Exact subset: validated formatter (`usesTime`, `formatTime`,
`formatMessage` CPython's, plain `converter`/`default_*`/`_style`/`datefmt`), `_style`
of a class validated against `PercentStyle` (so `{` and `$` styles fall back), record
validated, `exc_info`, `exc_text` and `stack_info` all false, `record.msg` a plain value,
`record.args` empty, a tuple of plain values or a dict with str keys and plain values,
every value the format reads plain, `_defaults` `None` or a dict. `record.__dict__` is
materialised first (CPython does it in `PercentStyle._format`; earlier is unobservable)
and the record's attributes are read from it; a dict whose keys are not all exact str
(checked with `dk_kind`, calibrated) falls back. `%`-formatting: a native renderer
(`rustique_core::logging::{parse_format, render}`) for `%s` and `%d/%i/%u` with `-`/`0`
flags, width and (for `%s`) precision, Latin-1 text — byte-for-byte
`unicode_format_arg_output`; everything else `PyUnicode_Format`, CPython's own function.

## 3. Private state that may differ

- `record.__dict__` is materialised earlier inside `Formatter.format` (same content and
  order).
- The level cache is read, never written, by the kernels; misses go to CPython.
- `abc` caches: the kernel does not call `isinstance(args[0], Mapping)`, so CPython's ABC
  negative/positive caches are not consulted by it (they hold the same answer).
- `func_version` of the patched functions (the specializer stops inlining calls to them;
  restored on `disable()`), and the kernels' own caches (validated types, parsed format
  strings, line numbers, ABC answers).
- Wave 2: the format-plan table (≤ 64 `_fmt` strings kept for the process with their key
  strings), the per-thread line-number table, the remembered `_decimal.Decimal` types, the
  per-type cached class attributes and shared-keys indices. The first record of a process
  gets its `__dict__` materialized once (verification of the inline-value stores: an
  allocation, not a value). `Handler.handle` takes the handler's lock with `acquire(False)`
  where CPython's `with` uses `acquire()` (same lock state; it never waits).
- `PyFrame_GetLineNumber` is skipped on a line-number memo hit (for a frame whose
  `f_lineno` field is 0 it has no side effect); `Task.get_name()` is called when CPython
  calls it (it caches the generated `Task-N` name, as in CPython).

## 4. Measured

### callgrind (instructions per call, region-instrumented, 3.15.1, `benches/targets/logging_cg.py`)

`benches/targets/logging_cg.py` (orig: plain CPython; enabled: `rustique.enable(only=
["logging"])` before `import logging`), Ir per statement, (Ir(2n) − Ir(n)) / n with 10×
unrolling, 3.15.1, dev-release build. Records go through a `StreamHandler` to an
`io.StringIO` from a caller function with ~40 locals. JIT columns: `PYTHON_JIT=1`, warmed
30 000 statements (subset). "[fallback, steady]" cells run a fallback path 1000s of times
in a row: after 64 consecutive fallbacks the target suspends itself (CPython's code at
1.00); "[fallback, alternating]" cells interleave the fallback with a fast call so the
target never suspends (the first-64-calls cost). "[deps]": a pinned dependency replaced
(the targets that pin it are suspended).

| cell | CPython | enabled | ratio | JIT CPython | JIT enabled | JIT ratio |
|---|---:|---:|---:|---:|---:|---:|
| info %(message)s | 54,428 | 27,844 | **0.512** | 46,279 | 26,507 | 0.573 |
| info BASIC_FORMAT | 57,093 | 28,875 | **0.506** | | | |
| info asctime fmt | 71,258 | 32,269 | **0.453** | 61,696 | 30,582 | 0.496 |
| info asctime fmt, child logger | 70,902 | 32,555 | **0.459** | 62,437 | 31,070 | 0.498 |
| info asctime fmt, 3 args | 71,223 | 34,995 | **0.491** | 62,751 | 33,725 | 0.537 |
| info asctime fmt, no args | 66,768 | 30,496 | **0.457** | 58,395 | 28,870 | 0.494 |
| info asctime fmt, extra= | 76,930 | 36,313 | **0.472** | 68,445 | 34,818 | 0.509 |
| info JSON formatter (subclass) | 83,513 | 48,505 | **0.581** | | | |
| info JSON __dict__ formatter | 117,985 | 82,824 | **0.702** | | | |
| info {-style | 79,550 | 44,293 | **0.557** | | | |
| info, time.strftime replaced [deps] (a shared pin: the whole group suspends) | 71,607 | 71,795 | **1.003** | | | |
| info Formatter subclass [fallback] | 70,766 | 32,199 | **0.455** | | | |
| info obj arg [fallback] | 71,260 | 47,546 | **0.667** | | | |
| info %(message)s, obj arg [fallback] | 54,653 | 40,194 | **0.735** | 46,613 | 34,369 | 0.737 |
| info stack_info=True [fallback] | 349,389 | 330,462 | **0.946** | | | |
| exception [fallback] | 714,290 | 678,561 | **0.950** | | | |
| adapter info | 87,864 | 44,037 | **0.501** | | | |
| logging.info root (disabled) | 3,846 | 3,132 | **0.815** | | | |
| debug disabled | 1,343 | 836 | **0.623** | 1,314 | 839 | 0.638 |
| info disabled | 1,343 | 836 | **0.623** | | | |
| debug disabled, no args | 1,172 | 803 | **0.685** | 1,150 | 806 | 0.700 |
| debug disabled, kwargs | 1,932 | 957 | **0.495** | 1,897 | 960 | 0.506 |
| isEnabledFor (not a site) | 612 | 612 | **1.000** | 689 | – | – |
| LogRecord() | 19,339 | 7,685 | **0.397** | 15,744 | 7,687 | 0.488 |
| makeRecord | 19,802 | 8,106 | **0.409** | | | |
| findCaller | 12,491 | 4,100 | **0.328** | 11,585 | 4,103 | 0.354 |
| getMessage | 2,049 | 2,062 | **1.007** | | | |
| getMessage obj arg | 2,253 | 2,266 | **1.006** | | | |
| format %(message)s | 7,441 | 5,776 | **0.776** | 6,819 | 5,746 | 0.843 |
| format BASIC_FORMAT | 10,094 | 6,743 | **0.668** | | | |
| format asctime | 23,704 | 9,674 | **0.408** | 22,830 | 9,667 | 0.423 |
| format datefmt | 18,780 | 8,234 | **0.438** | | | |
| format {-style [fallback] | 32,143 | 22,638 | **0.704** | | | |
| format formatTime overridden [fallback, steady] | 11,932 | 11,959 | **1.002** | 11,254 | 11,230 | 0.998 |
| format formatTime overridden + plain [fallback, alternating] | 36,703 | 22,403 | **0.610** | 34,272 | 21,627 | 0.631 |
| formatTime custom converter [fallback, steady] | 11,813 | 11,908 | **1.008** | 11,672 | 11,642 | 0.997 |
| formatTime custom converter + plain [fallback, alternating] | 23,804 | 14,877 | **0.625** | 23,549 | 14,631 | 0.621 |
| debug disabled, isEnabledFor overridden [fallback] | 1,110 | 1,110 | **1.000** | 985 | 985 | 1.000 |
| LogRecord(keywords) [fallback] | 19,104 | 19,256 | **1.008** | | | |
| LogRecord(keywords) + plain [fallback, alternating] | 35,929 | 26,153 | **0.728** | 33,736 | 25,559 | 0.758 |
| findCaller(stack_info=True) [fallback] | 286,477 | 286,946 | **1.002** | 259,768 | 260,036 | 1.001 |
| format subclass [fallback] | 23,669 | 9,682 | **0.409** | | | |
| format obj arg [fallback] | 23,911 | 14,399 | **0.602** | | | |
| format %(message)s, obj arg [fallback, steady] | 7,650 | 7,650 | **1.000** | 7,122 | 7,108 | 0.998 |
| format %(message)s, obj arg + plain [fallback, alternating] | 14,211 | 13,894 | **0.978** | 12,896 | 13,346 | 1.035 |
| formatTime | 11,926 | 2,506 | **0.210** | 11,777 | 2,509 | 0.213 |
| formatTime datefmt | 9,346 | 2,094 | **0.224** | 9,315 | 2,097 | 0.225 |
| formatTime subclass | 11,806 | 2,505 | **0.212** | 11,769 | 2,508 | 0.213 |

Readings:
- An enabled record through a `StreamHandler` costs **0.45–0.51** of CPython's
  instructions (0.47–0.49 with `extra=`/several arguments, 0.58 with a JSON formatter
  subclass, 0.50 through a `LoggerAdapter`); under the JIT 0.50–0.57.
- A **disabled** `logger.debug(...)` costs **0.62** (0.50 with keyword arguments, 0.69
  without arguments); `logging.info()` through the root logger 0.82.
- Pieces: `LogRecord(...)` 0.40, `findCaller` 0.33 (line numbers memoised; CPython
  recomputes them from the line table on every call), `Formatter.format` 0.41–0.78,
  `formatTime` 0.21–0.22.
- Every fallback cell is ≤ 1.008 (steady) and ≤ 0.98 when alternating (≤ 1.035 under the
  JIT); `getMessage` is not a site: 1.007 is +13 Ir, inside the ±15 Ir noise floor.
- Where a target falls back, the pieces below it still run their kernels (a formatter with
  arguments that are not plain: `format` falls back, its `formatTime` call does not → 0.60).

### Workload

`python benches/workloads/run.py compare web/logging_service -n 5 -p 3 --only logging`
(3.15.1, fresh processes, plain and enabled interleaved, median of steady-state runs, wall
time on the shared dev box): **282.0 ms → 234.2 ms, 0.830**, digests identical. Logging
is 66% of this workload; the rest (its JSON-lines formatter's `json.dumps`, a
`{`-style handler, `logger.exception` tracebacks, the request simulation) is not this
group's.

### pyperf (wall time, 3.15.1, `benches/targets/logging_pyperf.py --fast`)

| benchmark | CPython | enabled | ratio |
|---|---:|---:|---:|
| 1000 records, asctime format → StreamHandler(StringIO) | 6109 µs | 3691 µs | **0.604** |
| 1000 records, BASIC_FORMAT | 4670 µs | 3186 µs | 0.682 |
| 1000 records, JSON formatter (Formatter subclass) | 7359 µs | 5139 µs | 0.698 |
| 1000 disabled `debug` calls | 84.3 µs | 53.1 µs | 0.630 |

### Free-threaded spot check (3.15.1t, callgrind)

| cell | CPython | enabled | ratio |
|---|---:|---:|---:|
| info asctime fmt | 75,741 | 46,930 | 0.620 |
| info asctime fmt, `extra=` | 82,411 | 52,897 | 0.642 |
| debug disabled / no args / kwargs | 1,437 / 1,259 / 2,058 | 1,188 / 1,153 / 1,302 | 0.826 / 0.915 / 0.632 |
| `LogRecord(...)` | 21,319 | 9,689 | 0.454 |
| `findCaller()` | 12,899 | 9,085 | 0.704 |
| format asctime / `%(message)s` | 24,059 / 7,922 | 11,160 / 6,751 | 0.464 / 0.852 |
| formatTime | 11,640 | 2,922 | 0.251 |
| fallback cells (steady / alternating) | | | 0.999–1.000 / 0.650–1.035 |

Free-threaded builds read attributes through `PyObject_GenericGetAttr` (no inline-value
reads: another thread may replace a value concurrently) and do not memoise line numbers,
so the gains are smaller.

### Install cost (contract items 13/16, callgrind regions, `benches/targets/logging_install_cost.py`)

| region (3.15.1) | Ir |
|---|---:|
| `import logging`, plain | 27,947,128 |
| `import logging` with the group pending (lazy install of the 5 targets) | 28,303,814 (**+1.3%**) |
| `_accel.enable(["logging"])` (the registry call) with `logging` imported, cold process | 510,187 |
| the same after `enable` + `disable` (warm: one-time process facts done) | **143,439** |
| public `rustique.enable(only=["logging"])` with `logging` imported | 806,260 |
| public `rustique.enable(only=["logging"])` before `import logging` (nothing of this group runs) | 1,092,431 |

The lazy install is within the 5% budget. Per group, `enable()` costs ≈0.14M warm and
≈0.5M cold; the cold excess is one-time process work done by whichever group installs
first (monitoring and code-layout calibration, this group's attribute-layout and dict-kind
calibration, interning its ≈75 attribute names). What remains is 30 fingerprinted
functions (the patched ones and every helper a kernel inlines), 10 sites and ≈55 pins; the
module identities, `time.strftime`, the time functions and the builtins shared by several
targets are declared once in the group's `GroupSpec`. The public `enable()` adds ≈0.3–0.6M
of its own (inferred: the pending-only call, which runs nothing of this group, costs 1.09M).

## 5. Conformance

CPython's `test_logging`, run from the matching source tree with
`tests/conformance/run_target_suites.py --targets logging --modules test.test_logging`
(the five targets asserted installed), and again in-process with the targets re-enabled
before every test (`benches/targets/logging_suite_stats.py rearm`, so targets that a broken
pin or a fallback streak suspended in one test run again in the next):

| build | tests | plain | enabled (runner) | enabled, re-armed per test | kernel hits in the re-armed run |
|---|---:|---|---|---|---|
| 3.15.1 | 287 | 283 pass, 4 skip | identical (0 differences) | identical | level 370, findCaller 672, `__init__` 3789, formatTime 2503, format 2035 |
| 3.15.1t | 287 | 283 pass, 4 skip | identical | identical | same |
| 3.14.9 | 277 | 274 pass, 3 skip | identical | identical | same |
| 3.14.9t | 277 | 274 pass, 3 skip | identical | identical | same |

`test_logging` monkeypatches heavily (`logThreads`/`logProcesses`/`logMultiprocessing`/
`logAsyncioTasks`, `time.time_ns`, `raiseExceptions`, record factories, logger classes,
formatter subclasses, `Formatter.converter`), which exercises every pin and fallback.

Own tests (`tests/targets/test_logging.py`, `test_logging_hypothesis.py`, 245 tests, all 4
builds): introspection and captured references; `record.__dict__` (keys in order, values)
against the original for argument shapes, paths, levels, custom level names, threads,
`multiprocessing` (renamed process, mocked `current_process`, `sys.modules` entry `None`),
asyncio tasks, subclasses with and without descriptors; the time arithmetic on edge values
(gh-89047); level methods (keyword clashes, logger hierarchies, `logging.disable`, instance
and class overrides of `isEnabledFor`, colliding cache keys); `findCaller` (nesting,
`stacklevel`, `stack_info`, `_srcfile` replaced, line numbers in a 350-line function);
`formatTime` (datefmts incl. locale names and `%z`, converters, msec formats, mocked
`strftime`) and `format` (14 formats, 22 message/argument shapes incl. errors, user
`__str__` side effects and exceptions, `exc_info`, `stack_info`, `defaults`, `{`/`$`
styles, subclasses, instance overrides, `_style._fmt` changed, records with a
`getMessage` attribute or a non-str key); end to end through `StreamHandler` including the
`handleError` report on stderr (traceback compared); tools (`sys.settrace` sees every
logging frame); check mode; fallback-streak suspension; `python -O`; free-threaded stress
(8 threads logging, enable/disable under load). Hypothesis (`test_logging_hypothesis.py`,
1000 examples per run): random `%`-format strings over every record attribute with every
flag/width/precision/conversion, datefmts, message/argument shapes (incl. NaN, big ints,
non-Latin-1 text, dicts, lists), `defaults`, frozen times (incl. negative and year > 9999),
converters and msec formats, random logger hierarchies/levels/`disabled`/`propagate`/
`logging.disable` with every level method, `findCaller` depth × stacklevel, and
`LogRecord` constructor arguments; each compared with CPython's code on the same input.

## 6. Versions and internal layouts

Fingerprints (format v2) are recorded for 3.14 and 3.15 (identical on the free-threaded
builds); any other minor refuses. `python -O` strips the `assert` in `Thread.name`'s
getter: both bytecodes are recorded, so the group installs under `-O` too (tested).

**CPython internals the kernels rely on** (all identical in 3.14.9 and 3.15.1; each is
calibrated or cross-checked at install, and a failed check only turns that fast path off):

| internal | where | used for | check / fallback |
|---|---|---|---|
| `struct _dictvalues` (`capacity`, `valid`, `values[]`) at `obj + tp_basicsize` (`_PyObject_InlineValues`), `PyHeapTypeObject.ht_cached_keys`, `PyDictKeysObject` (`dk_log2_index_bytes` at 9, `dk_nentries` at 24, unicode entries after the indices) | `common.rs` `TypeCache::read` | reading instance attributes of validated objects (GIL builds only) | calibrated against `PyObject_GenericGetAttr` on `logging.root`; otherwise `PyObject_GenericGetAttr` |
| `PyDictKeysObject.dk_kind` (offset 10) through `PyDictObject.ma_keys` | `common.rs` `str_keyed` | "every key of this dict is an exact str" | calibrated on probe dicts; otherwise a key scan |
| `PyFrameObject.f_lineno` (after `f_back`, `f_frame`, `f_trace`) and `PyCodeObject.co_version` | `caller.rs` `line_number` | memoising line numbers of untraced frames (GIL builds only) | layout identical in both minors (source check); `co_version` via the framework's calibrated `RawCode` |
| `PyCodeObject._co_monitoring` | framework `code_locally_quiet` | local tools on each inlined helper | calibrated by the framework |
| `_PyObject_GetMethod`, `_PyType_LookupRef`, `PyUnstable_Type_AssignVersionTag`, `PyTime_Time`, `PyFrame_GetLasti` | exported functions | method shadowing checks, class validation, time | resolved at install (`cx.symbol`) or linked |

## 7. Install-cost status (wave 1)

The lazy install on `import logging` (+1.3%) meets the budget. The warm per-group
`enable()` (0.143M Ir, paid only when `logging` is already imported at `enable()` time) is
a documented wave-1 overage: what remains is 30 fingerprints, 10 sites and ≈55 pins (shared
ones declared once in the group's `GroupSpec`), and the remaining lever is framework-side
(cheaper or deferred fingerprints), not fewer pins. The cost of the public
`rustique.enable()` wrapper itself (≈0.3–0.6M, inferred) is outside this group.
