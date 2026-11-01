"""The shared mechanics helpers (`_mechanics.py`) against framework self-test targets with
exact kernels: `_selftest.same` (a Python function: vectorcall site, pinned global, tools
guard) and the `containers` group (ml_meth and slot sites)."""

from __future__ import annotations

import importlib
import sys
import textwrap
from unittest import mock

import pytest
from _mechanics import (
    A,
    assert_disable_restores,
    assert_introspection_unchanged,
    assert_patch_falls_back,
    assert_references_accelerated,
    assert_tools_see_original,
    enabled,
    ft_stress,
    references,
)

MOD = "_rustique_selftest"
SOURCE = textwrap.dedent(
    """
    SEP = "/"

    def same(x):
        "Return x."
        return x
    """
)


@pytest.fixture(scope="module")
def m(tmp_path_factory):
    d = tmp_path_factory.mktemp("selftest")
    (d / f"{MOD}.py").write_text(SOURCE)
    sys.path.insert(0, str(d))
    sys.modules.pop(MOD, None)
    try:
        yield importlib.import_module(MOD)
    finally:
        sys.path.remove(str(d))
        sys.modules.pop(MOD, None)
        A.disable(["_selftest"])


def test_python_function_target(m):
    refs = references(m.same)
    with enabled("_selftest.same"):
        assert_references_accelerated("_selftest.same", refs, (7,))
    assert_introspection_unchanged(m.same, "_selftest.same")
    assert_disable_restores("_selftest.same", m.same, lambda: m.same(3))
    assert_tools_see_original("_selftest.same", m.same, lambda: m.same(3))
    assert_patch_falls_back("_selftest.same", mock.patch.object(m, "SEP", "\\\\"), lambda: m.same(3), 3)
    assert ft_stress(["_selftest.same"], lambda: m.same(5), 5) > 0


def test_method_and_slot_targets():
    data = [3, 1, 2]
    with enabled("list.index", "list.__contains__"):
        assert_references_accelerated("list.index", references(data.index), (2,))
    assert_introspection_unchanged(list.index, "list.index")
    assert_introspection_unchanged(list.__contains__, "list.__contains__")
    assert_disable_restores("list.index", list.index, lambda: data.index(2))
    assert_disable_restores("list.__contains__", None, lambda: 2 in data)
    assert ft_stress(["list.index", "list.count"], lambda: (data.index(2), data.count(1)), (2, 1)) > 0
