"""Run CPython's test_logging in-process and record per-test outcomes.

    python benches/targets/logging_suite_stats.py plain  -o plain.json
    python benches/targets/logging_suite_stats.py rearm  -o rearm.json   # enable(only=logging) before every test
    python benches/targets/logging_suite_stats.py compare plain.json rearm.json

`rearm` re-enables the logging targets before each test (re-arming targets that the
fallback streak or a broken pin suspended in an earlier test), so every test runs with
the kernels in place; it prints per-target hits / fallbacks at the end. The shared
conformance runner (tests/conformance/run_target_suites.py) enables once per module."""
import json
import os
import sys
import tempfile
import unittest

def main():
    global rt, _rustique
    mode = sys.argv[1]
    if mode == "compare":
        a, b = (json.load(open(p)) for p in sys.argv[2:4])
        diff = {k: (a.get(k), b.get(k)) for k in set(a) | set(b) if a.get(k) != b.get(k)}
        counts = {}
        for v in b.values():
            counts[v] = counts.get(v, 0) + 1
        print("tests", len(b), counts, "differences", len(diff))
        for k, v in sorted(diff.items()):
            print("  ", k, v)
        sys.exit(1 if diff else 0)

    out = sys.argv[sys.argv.index("-o") + 1] if "-o" in sys.argv else None
    lib_test = os.path.join(os.environ["CPYTHON_SRC"], "Lib", "test")
    tmp = tempfile.mkdtemp()
    os.symlink(lib_test, os.path.join(tmp, "test"))
    sys.path.insert(0, tmp)

    if mode == "rearm":
        from rustique import _rustique
        import rustique as rt

        rt.enable(only=["logging"])
        _rustique._accel.stats(True, True)


    class Rec(unittest.TextTestResult):
        def __init__(self, *a, **k):
            super().__init__(*a, **k)
            self.outcomes = {}

        def startTest(self, test):
            if mode == "rearm":
                rt.enable(only=["logging"])
            super().startTest(test)

        def addSuccess(self, t):
            super().addSuccess(t)
            self.outcomes.setdefault(t.id(), "pass")

        def addFailure(self, t, e):
            super().addFailure(t, e)
            self.outcomes[t.id()] = "fail"

        def addError(self, t, e):
            super().addError(t, e)
            self.outcomes[t.id()] = "error"

        def addSkip(self, t, r):
            super().addSkip(t, r)
            self.outcomes[t.id()] = "skip"

        def addSubTest(self, t, st, e):
            super().addSubTest(t, st, e)
            if e is not None:
                self.outcomes[t.id()] = "fail"


    mod = __import__("test.test_logging", fromlist=["*"])
    suite = unittest.defaultTestLoader.loadTestsFromModule(mod)
    runner = unittest.TextTestRunner(stream=open(os.devnull, "w"), verbosity=0, resultclass=Rec)
    res = runner.run(suite)
    if os.getpid() and out:
        with open(out, "w") as f:
            json.dump(res.outcomes, f, indent=0)
    print("tests", res.testsRun, "failures", len(res.failures), "errors", len(res.errors), "skipped", len(res.skipped))
    if mode == "rearm":
        st = _rustique._accel.stats()
        for t in rt.status()["targets"]:
            if t["group"] == "logging":
                s = st[t["name"]]
                fb = {k: v for k, v in s["fallback"].items() if v}
                print(f"{t['name']:32s} {t['state']:10s} hits={s['hits']} fallback={fb} suspensions={s.get('suspensions')}")


if __name__ == "__main__":
    main()
