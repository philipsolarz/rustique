"""``python -m rustique run|status|report|diff`` and the pytest plugin (docs/USAGE.md)."""

import json
import os
import re
import subprocess
import sys
import textwrap

import pytest

from rustique import _diff

VARS = ("RUSTIQUE", "RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE", "RUSTIQUE_STATS", "RUSTIQUE_REPORT", "RUSTIQUE_CHECK")


def clean_env(**extra: str) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in VARS and not k.startswith("PYTEST_")}
    env.update(extra)
    return env


def rq(*args: str, env: dict[str, str] | None = None, cwd=None):
    return subprocess.run([sys.executable, "-m", "rustique", *args], capture_output=True, text=True,
                          env=env if env is not None else clean_env(), cwd=cwd, timeout=300)


# --- run -----------------------------------------------------------------------------------

SHOW = "import rustique; sw = rustique.status()['process']['switch']; print(sw['source'], sw['killed'])"


def test_run_command():
    r = rq("-c", SHOW)
    assert r.stdout.split() == ["python", "-m", "rustique", "False"], r.stderr


def test_run_explicit_subcommand_and_args(tmp_path):
    script = tmp_path / "app.py"
    script.write_text("import sys, rustique\nprint(sys.argv[1:], rustique.status()['process']['switch']['source'])\n")
    r = rq("run", str(script), "a", "--b")
    assert r.stdout.strip() == "['a', '--b'] python -m rustique", r.stderr


def test_run_selection_flags():
    r = rq("--exclude", "builtins", "-c",
           "import rustique; print(sorted({t['group'] for t in rustique.status()['targets'] if t['state'] == 'installed'}))")
    assert "builtins" not in r.stdout and r.returncode == 0, r.stderr


def test_run_respects_kill_switch():
    r = rq("-c", SHOW, env=clean_env(RUSTIQUE="0"))
    assert r.stdout.split() == ["None", "True"], r.stderr


def test_run_report_flag():
    r = rq("--report", "-c", "sorted([2, 1])")
    assert "enabled by python -m rustique" in r.stderr


def test_help_and_version():
    assert "usage: python -m rustique" in rq("--help").stdout
    assert rq().returncode == 2
    assert rq("--version").stdout.startswith("rustique ")


# --- status --------------------------------------------------------------------------------


def test_status():
    r = rq("status")
    assert r.returncode == 0, r.stderr
    assert r.stdout.startswith("Python      ") and "targets in" in r.stdout


def test_status_json():
    st = json.loads(rq("status", "--json").stdout)
    assert st["targets"] and st["process"]["executable"] == sys.executable
    assert "startup_files" in st["process"]


def test_status_killed():
    r = rq("status", env=clean_env(RUSTIQUE="0"))
    assert "kill switch" in r.stdout and r.returncode == 0


# --- report --------------------------------------------------------------------------------


def test_report_runs_the_command_and_reports():
    code = "import sys, rustique; rustique.enable(); print(sorted([3, 1])); sys.exit(3)"
    r = rq("report", "--", sys.executable, "-c", code)
    assert r.returncode == 3
    assert r.stdout == "[1, 3]\n"
    assert "1 Python process(es) reported" in r.stderr and "command exit code 3" in r.stderr


def test_report_script_shorthand(tmp_path):
    script = tmp_path / "app.py"
    script.write_text("import rustique\nrustique.enable()\nprint(max([1, 5]))\n")
    r = rq("report", "--json", "--", str(script))
    assert r.stdout == "5\n" and r.returncode == 0
    rec = json.loads(r.stderr)
    assert rec["status"]["targets"] and rec["status"]["process"]["switch"]["stats"]


def test_report_without_python():
    r = rq("report", "--", "true")
    assert "wrote no report" in r.stderr and r.returncode == 0


def test_report_refuses_under_kill_switch():
    r = rq("report", "--", "true", env=clean_env(RUSTIQUE="0"))
    assert r.returncode == 2 and "kill switch" in r.stderr


# --- diff ----------------------------------------------------------------------------------

# Programs enable Rustique themselves here (dev installs have no startup file); the plain run
# sets RUSTIQUE=0, which turns that enable() into a no-op.
ENABLE = "import rustique; rustique.enable(); "


def test_diff_identical():
    r = rq("diff", "--", sys.executable, "-c", ENABLE + "print(sorted([3, 1, 2]), sum([1.5, 2]))")
    assert r.returncode == 0, r.stdout + r.stderr
    assert "identical: exit code 0, stdout 1 lines" in r.stdout
    assert "time: plain" in r.stdout


def test_diff_reports_differences():
    code = ENABLE + "import sys; print(rustique.status()['process']['switch']['killed']); sys.exit(0 if 'x' else 1)"
    r = rq("diff", "--", sys.executable, "-c", code)
    assert r.returncode == 1
    assert "DIFFERENT" in r.stdout and "-False" not in r.stdout and "+False" in r.stdout


def test_diff_exit_code_difference():
    code = ENABLE + "import sys; sys.exit(rustique.status()['process']['switch']['killed'])"
    r = rq("diff", "--", sys.executable, "-c", code)
    assert r.returncode == 1 and "exit code: plain 1, rustique 0" in r.stdout


def test_diff_normalizes_addresses_and_ignore():
    code = ENABLE + "import time; print(object()); print('t', time.time())"
    r = rq("diff", "--ignore", r"t \d+\.\d+", "--", sys.executable, "-c", code)
    assert r.returncode == 0, r.stdout
    r = rq("diff", "--no-normalize", "--", sys.executable, "-c", code)
    assert r.returncode == 1


def test_diff_without_rustique_in_the_command():
    r = rq("diff", "--", "echo", "hi")
    assert r.returncode == 2 and "wrote no report" in r.stdout


@pytest.fixture
def suite(tmp_path):
    (tmp_path / "test_a.py").write_text(textwrap.dedent("""
        import os
        import pytest
        import rustique
        rustique.enable()

        def test_sort():
            assert sorted([3, 1, 2]) == [1, 2, 3]

        @pytest.mark.parametrize("n", [1, 2])
        def test_param(n):
            assert max([n, 0]) == n

        @pytest.mark.skip(reason="never")
        def test_skipped():
            pass

        @pytest.mark.xfail(reason="known")
        def test_xfail():
            assert False

        def test_tmp(tmp_path):
            print(tmp_path)

        def test_differs():
            assert os.environ.get("FLIP") != "1" or not rustique.status()["process"]["switch"]["killed"]
    """))
    return tmp_path


def test_diff_pytest_outcomes(suite):
    r = rq("diff", "--", sys.executable, "-m", "pytest", "-q", "-s", "-p", "no:randomly", str(suite), cwd=suite)
    assert r.returncode == 0, r.stdout + r.stderr
    assert "7 test outcomes" in r.stdout and "identical" in r.stdout
    assert not (suite / ".pytest_cache").exists()


def test_diff_pytest_outcome_change(suite):
    r = rq("diff", "--", sys.executable, "-m", "pytest", "-q", "-p", "no:randomly", str(suite), cwd=suite,
           env=clean_env(FLIP="1"))
    assert r.returncode == 1
    assert "test outcomes differ (1 changed" in r.stdout
    assert "test_a::test_differs: plain failed, rustique passed" in r.stdout


def test_diff_refuses_plugin_flag(suite):
    r = rq("diff", "--", sys.executable, "-m", "pytest", "--rustique", str(suite))
    assert r.returncode == 2 and "drop --rustique" in r.stderr


def test_is_pytest():
    assert _diff.is_pytest(["pytest", "-q"])
    assert _diff.is_pytest(["/venv/bin/py.test"])
    assert _diff.is_pytest(["python3.15", "-X", "dev", "-m", "pytest"])
    assert not _diff.is_pytest(["python", "-m", "pytest_foo"])
    assert not _diff.is_pytest(["python", "app.py", "-m"])
    assert _diff.pytest_args(["pytest", "--lf"], "x.xml")[-1] == "junit_family=xunit2"
    assert _diff.pytest_args(["pytest"], "x.xml")[-2:] == ["-p", "no:cacheprovider"]


def test_outcomes_parse(tmp_path):
    xml = tmp_path / "j.xml"
    xml.write_text("""<testsuites><testsuite>
      <testcase classname="m" name="a"/>
      <testcase classname="m" name="b"><failure message="x"/></testcase>
      <testcase classname="m" name="c"><error message="x"/></testcase>
      <testcase classname="m" name="d"><skipped type="pytest.skip" message="x"/></testcase>
      <testcase classname="m" name="e"><skipped type="pytest.xfail" message="x"/></testcase>
      <testcase classname="m" name="a"/>
    </testsuite></testsuites>""")
    assert _diff.outcomes(str(xml)) == {"m::a": "passed", "m::b": "failed", "m::c": "error", "m::d": "skipped",
                                        "m::e": "xfailed", "m::a#2": "passed"}


# --- pytest plugin -------------------------------------------------------------------------


def pytest_run(path, *args, **env):
    return subprocess.run([sys.executable, "-m", "pytest", "-p", "rustique.pytest_plugin", "-p", "no:cacheprovider",
                           "-p", "no:randomly", *args, str(path)],
                          capture_output=True, text=True, env=clean_env(**env), cwd=path, timeout=300)


@pytest.fixture
def plain_suite(tmp_path):
    (tmp_path / "test_b.py").write_text("def test_sort():\n    assert sorted([2, 1]) == [1, 2]\n")
    return tmp_path


def test_plugin_inactive_without_flag(plain_suite):
    r = pytest_run(plain_suite)
    assert r.returncode == 0 and not re.search(r"^=+ rustique =+$", r.stdout, re.M)


def test_plugin_without_flag_imports_nothing(tmp_path):
    # The pytest11 entry point (which `-p rustique.pytest_plugin` resolves to) is the
    # top-level `_rustique_pytest`: without --rustique, no session imports the package.
    (tmp_path / "test_m.py").write_text(
        "import sys\n\ndef test_m():\n    assert not [m for m in sys.modules if m.split('.')[0] == 'rustique']\n")
    r = pytest_run(tmp_path)
    assert r.returncode == 0, r.stdout + r.stderr


def test_plugin_enables_and_reports(plain_suite):
    (plain_suite / "test_c.py").write_text(
        "import rustique\n\ndef test_on():\n    assert rustique.status()['process']['switch']['source'] == 'pytest --rustique'\n")
    r = pytest_run(plain_suite, "--rustique")
    assert r.returncode == 0, r.stdout + r.stderr
    assert re.search(r"^=+ rustique =+$", r.stdout, re.M)
    assert "enabled by pytest --rustique" in r.stdout


def test_plugin_selection(plain_suite):
    (plain_suite / "test_c.py").write_text(
        "import rustique\n\ndef test_sel():\n"
        "    st = rustique.status()\n"
        "    assert not any(t['group'] == 'builtins' and t['state'] == 'installed' for t in st['targets'])\n")
    r = pytest_run(plain_suite, "--rustique", "--rustique-exclude", "builtins")
    assert r.returncode == 0, r.stdout


def test_plugin_kill_switch(plain_suite):
    r = pytest_run(plain_suite, "--rustique", RUSTIQUE="0")
    assert r.returncode == 0 and "disabled by RUSTIQUE=0 (kill switch)" in r.stdout


REFUSE = """
import rustique

class Accel:
    def __init__(self, inner):
        self._inner = inner
    def __getattr__(self, name):
        return getattr(self._inner, name)
    def status(self):
        st = self._inner.status()
        st["targets"].append({{"name": "fake.target", "group": "fake", "state": "refused", "reason": "test",
                              "refusal": {refusal!r}, "sites": [], "versions": []}})
        return st

rustique._accel = Accel(rustique._accel)
"""


@pytest.mark.parametrize(("refusal", "args", "fails"), [
    ("fingerprint", (), True),
    ("conflict", (), True),
    ("fingerprint", ("--rustique-allow-refused", "fake"), False),
    ("version", (), False),
    ("build", (), False),
    ("jit", (), False),
    ("platform", (), False),
])
def test_plugin_unexpected_refusal_fails_the_session(plain_suite, refusal, args, fails):
    (plain_suite / "conftest.py").write_text(REFUSE.format(refusal=refusal))
    r = pytest_run(plain_suite, "--rustique", *args)
    assert (r.returncode == 1) == fails, r.stdout
    assert ("refused unexpectedly" in r.stdout) == fails
    assert "fake.target" in r.stdout


def test_plugin_check_mode(plain_suite):
    (plain_suite / "test_c.py").write_text(
        "import rustique\n\ndef test_check():\n    assert rustique.status()['process']['check'] == 'warn'\n")
    r = pytest_run(plain_suite, "--rustique", "--rustique-check")
    assert r.returncode == 0, r.stdout
    assert "check mode: warn" in r.stdout and "check mismatches: none" in r.stdout


MISMATCH = """
import rustique

class Accel:
    def __init__(self, inner):
        self._inner = inner
    def __getattr__(self, name):
        return getattr(self._inner, name)
    def stats(self, enabled=None, reset=False):
        out = self._inner.stats(enabled, reset)
        out["list.sort"] = {"hits": 3, "fallback": {"check": 2}}
        return out

rustique._accel = Accel(rustique._accel)
"""


def test_plugin_check_mismatch_fails_the_session(plain_suite):
    (plain_suite / "conftest.py").write_text(MISMATCH)
    r = pytest_run(plain_suite, "--rustique", "--rustique-check")
    assert r.returncode == 1, r.stdout
    assert "CHECK MISMATCHES: 2 in list.sort (2)" in r.stdout and "check mode found 2 mismatch(es)" in r.stdout


def test_run_check_flag():
    r = rq("--check", "-c", "import rustique; print(rustique.status()['process']['check'])")
    assert r.stdout.strip() == "warn", r.stderr
    r = rq("--check-raise", "-c", "import rustique; print(rustique.status()['process']['check'])")
    assert r.stdout.strip() == "raise", r.stderr


def test_diff_check():
    r = rq("diff", "--check", "--", sys.executable, "-c", ENABLE + "print(sorted([3, 1, 2]))")
    assert r.returncode == 0, r.stdout
    assert "check mode: warn" in r.stdout


def test_plugin_xdist(plain_suite):
    pytest.importorskip("xdist")
    r = pytest_run(plain_suite, "--rustique", "-n", "2")
    assert r.returncode == 0, r.stdout
    assert "2 processes" in r.stdout
