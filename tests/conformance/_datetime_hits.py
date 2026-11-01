"""Run CPython test modules with the datetime group enabled and report how often each
kernel took its fast path (proof that conformance exercised the kernels).

    python tests/conformance/_datetime_hits.py test.test_datetime test.test_strptime test.test_time
"""
import io
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from run_target_suites import expose_test_package  # noqa: E402

expose_test_package(os.environ["CPYTHON_SRC"])
import rustique._takeover as rq  # noqa: E402

rq.enable(only=["datetime"])
rq.stats(True, reset=True)
for name in sys.argv[1:]:
    suite = unittest.defaultTestLoader.loadTestsFromName(name)
    r = unittest.TextTestRunner(stream=io.StringIO(), verbosity=0).run(suite)
    print(name, "ran", r.testsRun, "failures", len(r.failures), "errors", len(r.errors))
st = rq.stats()
for k, v in st.items():
    if k.startswith(("datetime.", "time.")):
        print(f"  {k:30s} hits {v['hits']:7d} fallback {dict((a, b) for a, b in v['fallback'].items() if b)}")
from rustique._rustique import _accel  # noqa: E402

print(_accel._datetime_debug())
