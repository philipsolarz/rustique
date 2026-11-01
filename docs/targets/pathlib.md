STATUS: wave 1, workstream `paths` (2026-11-01). Contract: docs/ACCELERATION.md.

# Group `pathlib`

The pure-path core of `pathlib.PurePosixPath` / `PosixPath` (and so `Path()` on POSIX):
vectorcall sites on the functions in `PurePath.__dict__` (methods, and the `fget` of
properties). Code: `crates/rustique-py/src/targets/pathlib/mod.rs` (str buffers and
builders shared with `posixpath`, `rt.rs`). Tests: `tests/targets/test_pathlib.py`.

## Targets

| target | reached through | exact subset |
|---|---|---|
| `pathlib.PurePath.__new__`, `pathlib.Path.__new__` | `PurePosixPath(...)`, `Path(...)` (`slot_tp_new`: fallback +3 Ir) | `cls` is `PurePath`/`Path` (→ the POSIX class: `os.name` pinned to 'posix') or exactly `PurePosixPath`/`PosixPath`; no keywords |
| `pathlib.PurePath.__init__` | the same calls (`slot_tp_init`) | receiver of those classes; arguments exact str or exact POSIX paths (their `_raw_paths` are extended, as `isinstance(arg, PurePath)` with the same parser does) |
| `pathlib.PurePath.__str__` (3.15: also `__fspath__`, `__vfspath__`, the same function) | `str(p)`, f-strings, `os.fspath(p)` (slot / C callers: fallback +3 Ir) | receiver exactly `PurePosixPath` or `PosixPath` |
| `pathlib.PurePath.__truediv__` | `p / x` (slot) | key an exact `str` or an exact path of those two classes |
| `pathlib.PurePath.__eq__`, `__hash__` | `==`, `hash()`, dict/set (slots) | both operands of those classes |
| `pathlib.PurePath.relative_to` | method call | `other` an exact str or path of those classes; no `walk_up` |
| `pathlib.PurePath.parent`, `name`, `suffix`, `stem`, `parts` | properties | receiver of those classes |

Common to all: the receiver's private slots hold what pathlib itself stores (`_raw_paths`
an exact list of exact str; parsed slots exact str / list; `_drv` empty), else fallback.

## State: filled exactly as CPython fills it

PurePath keeps its parsed state in private `__slots__` that its properties fill lazily.
The kernels compute the same values and store them in the same slots (read and written
under the object's critical section on free-threaded builds):

- `_drv`, `_root`, `_tail_cached` (what `_parse_path(_raw_path)` returns: `_raw_path`
  joins the raw paths with `posixpath.join` semantics, the root follows C `_Py_skiproot`,
  the tail is `[x for x in rel.split('/') if x and x != '.']`);
- `_str` (`_format_parsed_parts(...) or '.'`), `_str_normcase_cached` (the `_str` object
  itself on POSIX), `_hash` (`hash(_str_normcase)`);
- new objects (`Path(...)`, `p / x`, `parent`, `relative_to`) are created as
  `type(self)(...)` creates them (`object.__new__(cls)`, then the slots `__init__` /
  `_from_parsed_parts` / `_from_parsed_string` set): `_raw_paths` (which `pickle`/`copy` expose through
  `__reduce__`), and for derived paths `_str`, `_drv`, `_root`, `_tail_cached`.

Object sharing is reproduced where CPython shares: `_str` is the single tail item of a
one-component relative path, the root object of a root-only path, `'.'` for an empty
path; `_root`/`_drv` are the shared `''`/`'/'` singletons (`'//'` a fresh string);
`parts` returns the tail items themselves; `parent` of a path without tail is the path
itself; `relative_to` also fills the slots CPython's comparisons fill on `self` and on a
path `other` (`_str`, `_str_normcase_cached`, parsed slots). The differential test runs
random operation sequences with the kernels on and off and compares results *and* every
slot of every object involved, including which positions hold the same object.

## Guards and pins

Both exact classes must have the default metaclass and `parser is posixpath`. Every
attribute the originals reach through `self`/`cls` is pinned through the MRO of both
classes (`pin_attr`: `__new__`, `__init__`, `with_segments`, `parser`, `_parse_path`,
`_format_parsed_parts`, `_from_parsed_parts`, `_from_parsed_string`, `_raw_path`,
`drive`, `root`, `_tail`, the targets themselves, `_str_normcase`, `parents`), so patching
`PurePath.with_segments` (the public subclass hook) or any of these on a class suspends the
targets. Also pinned: `posixpath.sep/altsep/join/splitroot/splitdrive/_get_sep`, the
pathlib globals the code reads (`os`, `posixpath`, `PurePath`, `chain`, `_PathParents`)
and its builtins. Fingerprints: all 29 functions involved (per minor; checked once per set
of function objects, since the thirteen targets share them).

Subclasses (including any overriding `with_segments`, `__init__` or `parser`),
`PureWindowsPath`, and all other inputs run CPython's code.

## Private state skipped

None: the kernels fill the same private slots with the same values. Not filled where
CPython would not fill them either.

## Measurements

RESULTS_PLACEHOLDER

## Conformance

CONFORMANCE_PLACEHOLDER

## Version coverage

3.14 and 3.15: the PurePath core is the same except `__fspath__` (3.15: alias of
`__str__`, accelerated with it; 3.14: its own function calling `str(self)`, which reaches
the `__str__` kernel) and `relative_to`'s loop (same result without `walk_up`).
`jit_safe: false`.
