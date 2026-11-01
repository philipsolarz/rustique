"""pytest running a slice of networkx's own test suite in-process (~600 tests).

Stands for the developer's test loop (`pytest` on save / in CI) on a real pure-Python
library with a pytest-native suite: collection of modules and classes, parametrize,
fixtures, plain-`assert` rewriting, unittest-style setup methods, per-test reporting and
warnings capture, plus the library code under test (networkx graph classes, views and
functions). networkx ships its tests inside the wheel, so no data is bundled. `run()` is
one `pytest.main()` call; the test modules are dropped from `sys.modules` first so every
run re-imports them through pytest's assertion-rewriting import hook (from its cached
rewritten pyc, as a repeat `pytest` invocation does), since networkx's tests mutate their
classes in `setup_class`. Imports of pytest/networkx themselves stay warm.
"""

import hashlib
import io
import os
import sys
import contextlib

REQUIRES = ["pytest==9.2.0", "networkx==3.7"]

TESTS = (
    "classes/tests/test_graph.py",
    "classes/tests/test_function.py",
    "classes/tests/test_reportviews.py",
    "classes/tests/test_coreviews.py",
    "utils/tests/test_misc.py",
)


class _Outcomes:
    def __init__(self):
        self.results = []

    def pytest_runtest_logreport(self, report):
        if report.when == "call" or report.outcome != "passed":
            self.results.append((report.nodeid, report.when, report.outcome))


def setup():
    import networkx
    import pytest  # noqa: F401

    root = os.path.dirname(networkx.__file__)
    return [os.path.join(root, t) for t in TESTS]


def run(state):
    import pytest

    for name in [n for n in sys.modules if n.startswith("networkx.") and ".tests." in n]:
        del sys.modules[name]
    plugin = _Outcomes()
    sink = io.StringIO()
    with contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
        code = pytest.main(
            ["-q", "-p", "no:cacheprovider", "-W", "ignore::DeprecationWarning",
             "--rootdir", os.path.dirname(os.path.dirname(os.path.dirname(state[0]))), *state],
            plugins=[plugin],
        )
    return int(code), sorted(plugin.results)


def digest(result):
    code, results = result
    return hashlib.sha256(repr((code, results)).encode()).hexdigest()
