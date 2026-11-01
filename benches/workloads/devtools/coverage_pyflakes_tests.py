"""`coverage run --branch -m unittest` of pyflakes' own test suite, then `coverage report`.

Stands for the coverage-measured test job in CI (`coverage run -m pytest/unittest` +
`coverage report`/`xml`): coverage.py's tracer records every line and branch arc of the
measured package while 754 real unit tests run (pyflakes is re-imported under
measurement each run), then the reporter re-parses the measured
sources (`ast`/`tokenize`) to compute statements, branches and missing lines. On 3.14+ the
default core is `sysmon` (PEP 669 `sys.monitoring` callbacks in Python) for both lines and
branches; the C tracer is not used. Data is kept in memory (`data_file=None`). The suite
ships inside the pyflakes wheel, so nothing is bundled.
"""

import hashlib
import io

REQUIRES = ["coverage==7.17.0", "pyflakes==4.1.0"]

TEST_MODULES = (
    "pyflakes.test.test_imports", "pyflakes.test.test_other", "pyflakes.test.test_undefined_names",
    "pyflakes.test.test_type_annotations", "pyflakes.test.test_builtin", "pyflakes.test.test_match",
    "pyflakes.test.test_dict", "pyflakes.test.test_is_literal", "pyflakes.test.test_code_segment",
    "pyflakes.test.test_doctests", "pyflakes.test.test_custom_builtins",
    "pyflakes.test.test_lazy_imports",
)


def setup():
    import unittest  # noqa: F401
    import coverage  # noqa: F401

    return TEST_MODULES


def run(state):
    import sys
    import unittest
    import coverage

    # As in a fresh `coverage run`, pyflakes and its tests are imported while measured.
    for name in [n for n in sys.modules if n == "pyflakes" or n.startswith("pyflakes.")]:
        del sys.modules[name]
    cov = coverage.Coverage(data_file=None, branch=True, source=["pyflakes"],
                            omit=["*/pyflakes/test/*"], config_file=False)
    cov.start()
    try:
        suite = unittest.defaultTestLoader.loadTestsFromNames(state)
        result = unittest.TextTestRunner(stream=io.StringIO(), verbosity=0).run(suite)
    finally:
        cov.stop()
    report = io.StringIO()
    total = cov.report(file=report, show_missing=True)
    lines = [ln for ln in report.getvalue().splitlines() if ln.strip()]
    # normalise the absolute site-packages prefix away
    lines = [ln[ln.find("pyflakes"):] if "pyflakes" in ln else ln for ln in lines]
    return result.testsRun, len(result.failures), len(result.errors), round(total, 3), lines


def digest(result):
    return hashlib.sha256(repr(result).encode()).hexdigest()
