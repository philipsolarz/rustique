STATUS: wave 1, group `logging` (branch `ws/logging`), 2026-11-01. Validated on CPython 3.14.9,
3.14.9t, 3.15.1, 3.15.1t. Code: `crates/rustique-py/src/targets/logging/`,
`crates/rustique-core/src/logging/`. Tests: `tests/targets/test_logging*.py`. Benchmarks:
`benches/targets/logging_cg.py`, `benches/targets/logging_suite_stats.py`.

# Group `logging`: the default record path, piecewise

`logging` is in 40% of popular packages and owns 66% of the `web/logging_service` workload
(`docs/atlas/workloads-web.md`). Its hook surface is the widest of wave 1 (record
factories, logger classes, formatter subclasses, converters, filters, handlers, module
switches), so the group accelerates the default record path **piece by piece**, each piece
a pure-Python function whose vectorcall is swapped (contract §1, kind C), and only where
no Python code runs below the kernel.

## 1. Targets

| target | sites | the kernel |
|---|---|---|
| `logging.Logger.level` | `Logger.isEnabledFor`, `.debug`, `.info`, `.warning`, `.error`, `.critical`, `.log` | decides `self.isEnabledFor(level)` from `self.disabled` and the level cache; a **disabled** call returns `None`; an enabled call runs CPython's method |
| `logging.Logger.findCaller` | `Logger.findCaller` | the frame walk with `_is_internal_frame` semantics, `(co_filename, f_lineno, co_name, None)` |
| `logging.LogRecord.__init__` | `LogRecord.__init__` | every record attribute, stored in CPython's order |
| `logging.Formatter.formatTime` | `Formatter.formatTime` | `converter(created)` + `strftime` + `default_msec_format` |
| `logging.Formatter.format` | `Formatter.format` | `getMessage()` + `usesTime()` + `formatTime()` + `PercentStyle.format()` |

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
5. **Dependencies are pinned** (dict-watcher pins of the registry, `pin`/`pin_global`):
   `logging.time/os/sys/threading/collections`, `getLevelName`, `_levelToName`,
   `_nameToLevel`, `_srcfile`, `currentframe`, `_is_internal_frame`, `_startTime`,
   `logThreads`/`logProcesses`/`logMultiprocessing`/`logAsyncioTasks`, the level constants,
   `time.time_ns`/`time.strftime`, `os.path`, `os.getpid`, `os.fspath`,
   `posixpath.basename/splitext/normcase/_get_sep/os/genericpath`, `genericpath._splitext`,
   `threading.get_ident/current_thread/_active`, `sys.modules`, `sys._getframe`,
   `collections.abc.Mapping`, and the builtins the code reads (`isinstance`, `len`,
   `hasattr`, `int`, `str`, through `pin_global`, so a shadowing module global counts).
   A replaced dependency (`mock.patch("time.time_ns")`, `logging._srcfile = None`)
   suspends the target until it is restored. `Formatter.converter` is not pinned: it is
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
7. **Tools**: any `sys.monitoring` tool listening to *any* global event (not only
   `PY_START`: line-only, raise-only tools too), or locally on any of those code objects
   (breakpoints; 16 local events on 3.15, 11 on 3.14), makes every kernel fall back, so
   tracers, profilers, coverage and debuggers see CPython's frames.

### Per target

**`Logger.level`.** Exact subset: `self` of a validated logger class, `level` an exact
`int` (the module constant for `debug`..`critical`), no keyword naming a positional
parameter (CPython raises "multiple values"), `self.isEnabledFor` CPython's, `disabled` a
plain truth value, `_cache` an exact dict with exact-int keys (so the lookup runs no
`__eq__`) holding the level. Disabled → `None` (no frame, no `*args`/`**kwargs` objects).
`isEnabledFor` returns the cached object itself. A cache miss (CPython takes
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
`relativeCreated = (ct - _startTime) / 1e6`; `getLevelName` (scan of `_levelToName`
verifying exact-int keys, `_nameToLevel` exact-str keys, `"Level %s" % level`);
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
- `Task.get_name()` and `PyFrame_GetLineNumber` are called exactly when CPython calls them.

## 4. Measured

### callgrind (instructions per call, region-instrumented, 3.15.1, `benches/targets/logging_cg.py`)

MEASUREMENTS

### Workload

WORKLOAD

## 5. Conformance

CONFORMANCE

## 6. Versions

Fingerprints recorded for 3.14 and 3.15 (identical on the free-threaded builds); any other
minor refuses. The inline-value reads (GIL builds) and the `dk_kind` check are calibrated
at install against `PyObject_GenericGetAttr` on `logging.root` and probe dicts; a failed
calibration only turns those fast paths off. `python -O` changes `Thread.name`'s bytecode
(the `assert` is stripped): `LogRecord.__init__` then refuses (fingerprint).
