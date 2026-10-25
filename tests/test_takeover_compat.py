"""Boundary compatibility of the takeover tiers (docs/TAKEOVER.md §3.2).

The matrix itself lives in `benches/takeover_compat.py` (it is also a benchmark section).
It runs ~76 checks of how non-opted-in code (stdlib C and Python code, numpy, PyO3
extensions) treats the containers each candidate produces, against exact builtins.

* The safe tier (exact builtins, with or without the kernels) must pass every check.
* The native tier (`rustique.List`/`Dict`/`Set` from rewritten code) breaks a known set of
  checks. Each is pinned here, split into *irreducible* (no non-builtin type can pass:
  exact-type checks in C, `isinstance`) and *presentation* (fixable by the List/Dict/Set
  workstreams: builtin-identical repr, error messages and pickling). A check that starts
  passing fails this test on purpose: update the lists and TAKEOVER.md.
* A native-storage *subclass* of the builtins (the layout rule R2 forbids) breaks silently.
"""

from __future__ import annotations

import importlib.util
import os

import pytest

_spec = importlib.util.spec_from_file_location(
    "takeover_compat", os.path.join(os.path.dirname(__file__), os.pardir, "benches", "takeover_compat.py")
)
tc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(tc)

SAFE = ("6: exact builtin", "6+K: exact builtin + kernels")
NATIVE = "1/2/3: rs type (rewrite target today)"
NATIVE_SUBCLASS = "4b: subclass, native storage"

IRREDUCIBLE = {
    ("list", "isinstance(x, list)"), ("list", "type(x) is list"), ("list", "json.dumps (C encoder)"),
    ("list", "json.dumps(indent=1) (Py encoder)"), ("list", "marshal.dumps"), ("list", "copy.copy"),
    ("list", "copy.deepcopy"), ("list", "pprint.pformat(width=10)"), ("list", "mutation: heapq.heappush"),
    ("list", "mutation: heapq.heapify"), ("list", "PyO3: cast::<PyList>() + get_item"),
    ("dict", "isinstance(d, dict)"), ("dict", "type(d) is dict"), ("dict", "json.dumps (C encoder)"),
    ("dict", "json.dumps(indent, sort_keys)"), ("dict", "marshal.dumps"), ("dict", "copy.copy / deepcopy"),
    ("dict", "pprint.pformat(width=10)"), ("dict", "exec(code, d) globals"), ("dict", "str.maketrans(d)"),
    ("dict", "type('T', (), d) namespace"), ("dict", "PyO3: cast::<PyDict>() + iter"),
    ("set", "isinstance(s, set)"), ("set", "type(s) is set"), ("set", "marshal.dumps"), ("set", "copy.copy / deepcopy"),
    ("set", "pprint.pformat"), ("set", "PyO3: cast::<PySet>() + iter"),
}
PRESENTATION = {
    ("list", "repr(x) / str / f-string"), ("list", "pickle round trip"), ("list", "pickle stream is builtin-only"),
    ("list", "hash(x) error"), ("list", "x.nope error"),
    ("dict", "repr(d)"), ("dict", "pickle round trip"), ("dict", "hash(d) error"),
    ("set", "repr(s)"), ("set", "pickle round trip"), ("set", "json.dumps error"), ("set", "hash(s) error"),
}


@pytest.fixture(scope="module")
def matrix():
    names, results = tc.run(time_it=False, candidates=[*SAFE, NATIVE, NATIVE_SUBCLASS])
    return results


def test_safe_tier_passes_every_boundary_check(matrix):
    failures = [(key, name, row[name]) for key, row in matrix.items() for name in SAFE if row[name][0] != "works"]
    assert not failures


def test_matrix_covers_the_boundaries_of_the_brief(matrix):
    labels = " ".join(label for _, label in matrix)
    for needle in ("isinstance", "type(x) is", "json", "pickle", "copy.deepcopy", "pprint", "dataclasses.asdict",
                   "f(*x)", "f(**d)", "join", "np.array", "PyO3", "match"):
        assert needle in labels, needle


@pytest.mark.parametrize("key", sorted(IRREDUCIBLE | PRESENTATION))
def test_native_tier_known_breaks_are_still_broken(matrix, key):
    status, info = matrix[key][NATIVE]
    assert status == "BREAKS", f"{key} now works for the native tier: update the lists here and docs/TAKEOVER.md"


def test_native_tier_everything_else_works(matrix):
    unexpected = [(key, row[NATIVE]) for key, row in matrix.items()
                  if key not in IRREDUCIBLE | PRESENTATION and row[NATIVE][0] != "works"]
    assert not unexpected


def test_native_storage_subclass_corrupts_silently(matrix):
    """Why R2 forbids hiding native storage behind a builtin subclass: C code that checks
    `PyList_Check`/`PyDict_Check` reads the empty builtin layout and returns wrong data
    without raising."""
    silent = {key: row[NATIVE_SUBCLASS][1] for key, row in matrix.items()
              if row[NATIVE_SUBCLASS][0] == "BREAKS" and "raised" not in row[NATIVE_SUBCLASS][1]}
    for key in [("dict", "json.dumps (C encoder)"), ("list", "mutation: heapq.heappush"), ("list", "array.array('q', x)"),
                ("set", "frozenset(s), ref.update(s)"), ("dict", "type('T', (), d) namespace"),
                ("list", "PyO3: cast::<PyList>() + get_item"), ("dict", "PyO3: cast::<PyDict>() + iter")]:
        assert key in silent, key
