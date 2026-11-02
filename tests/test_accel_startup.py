"""The `RUSTIQUE=1` startup path that loads only the extension (docs/ACCELERATION.md §8;
the environment semantics are dx's, tested in tests/test_dx*.py): the `.pth` line imports
the top-level `_rustique` without importing the `rustique` package and calls its
`startup()`.

Each case runs a fresh interpreter that executes the shipped line exactly as `site` does for
a `.pth` import line (`exec(line)` in a frame with a local `sitedir`). In the source tree
`_rustique` is the in-package layout's shim (`python/_rustique.py`); tests/test_engine.py
covers the release wheel's top-level layout.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import textwrap

import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# The line shipped in `_rustique_startup.pth`. `site` compiles every `.pth` import line at
# each start, so it stays one short statement: RUSTIQUE unset or 0 stops at one getenv;
# anything else imports the extension, whose `startup()` decides (OFF values and -E/-I
# leave nothing loaded, ON values enable, others warn).
with open(os.path.join(ROOT, "python", "_rustique_startup.pth")) as _f:
    PTH_LINE = _f.read().strip()

SITEDIR = os.path.dirname(os.path.dirname(importlib.util.find_spec("rustique").origin))


def run(body: str, env: dict[str, str | None], *flags: str, sitedir: str = SITEDIR) -> subprocess.CompletedProcess[str]:
    """Run the `.pth` line as site would, then `body`, in a fresh interpreter."""
    prologue = (
        "def _site(sitedir):\n"
        f"    exec({PTH_LINE!r})\n"
        f"_site({sitedir!r})\n"
    )
    e = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    e.update({k: v for k, v in env.items() if v is not None})
    return subprocess.run(
        [sys.executable, *flags, "-c", prologue + textwrap.dedent(body)],
        capture_output=True,
        text=True,
        env=e,
        timeout=120,
    )


def test_the_line_is_one_import_line():
    assert "\n" not in PTH_LINE and PTH_LINE.startswith("import ")


@pytest.mark.parametrize("value", [None, "0", "off", "", " No ", "FALSE", "Off"])
def test_unset_and_off_values_load_nothing(value):
    r = run(
        "import sys\nnames = sorted(m for m in sys.modules if 'rustique' in m)\nprint(names)",
        {"RUSTIQUE": value},
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() == "[]", r.stdout
    assert r.stderr == ""


@pytest.mark.parametrize("value", ["0", "", "off", "No"])
def test_off_values_are_the_kill_switch(value):
    r = run(
        """
        import rustique._rustique as ext
        ext._accel.switch_enable()
        st = ext._accel.status()
        print(st['process']['enabled'], st['process']['switch']['killed'], st['process']['switch']['killed_calls'])
        """,
        {"RUSTIQUE": value},
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.split() == ["False", "True", "1"]


def test_bad_value_warns_and_stays_off():
    r = run(
        """
        import sys
        st = sys.modules['rustique._rustique']._accel.status()
        print(st['process']['enabled'], st['process']['switch']['killed'])
        """,
        {"RUSTIQUE": "maybe"},
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.split() == ["False", "False"]
    assert r.stderr.strip() == "rustique: ignoring RUSTIQUE='maybe' (expected 1 or 0)"


def test_startup_loads_only_the_extension_and_enables():
    r = run(
        """
        import sys
        assert 'rustique' not in sys.modules, sorted(sys.modules)
        ext = sys.modules['_rustique']
        assert sys.modules['rustique._rustique'] is ext
        st = ext._accel.status()
        installed = sorted(t['name'] for t in st['targets'] if t['state'] == 'installed')
        print(st['process']['switch']['source'], len(installed))
        # The package imported afterwards reuses the loaded extension.
        import rustique
        assert rustique._rustique is sys.modules['rustique._rustique'] is ext
        assert rustique.status()['process']['enabled']
        assert [3, 1, 2].index(2) == 2 and sum([1, 2]) == 3
        """,
        {"RUSTIQUE": "1"},
    )
    assert r.returncode == 0, r.stderr
    assert r.stderr == ""
    source, n = r.stdout.split()
    assert source == "RUSTIQUE=1" and int(n) >= 13


@pytest.mark.parametrize("flag", ["-E", "-I"])
def test_ignore_environment_imports_nothing(flag):
    r = run("import sys; print([m for m in sys.modules if 'rustique' in m])", {"RUSTIQUE": "1"}, flag)
    assert r.returncode == 0 and r.stdout.strip() == "[]", (r.stdout, r.stderr)


def test_selection_and_warnings_from_the_environment():
    r = run(
        """
        import sys
        st = sys.modules['rustique._rustique']._accel.status()
        sw = st['process']['switch']
        installed = sorted(t['name'] for t in st['targets'] if t['state'] == 'installed')
        print(json.dumps([sw['only'], sw['exclude'], installed, sw['stats'], sw['check']]))
        """.replace("import sys", "import sys, json"),
        {"RUSTIQUE": "1", "RUSTIQUE_ONLY": "containers; nope", "RUSTIQUE_EXCLUDE": "list.sort",
         "RUSTIQUE_STATS": "yes", "RUSTIQUE_CHECK": "raise"},
    )
    assert r.returncode == 0, r.stderr
    only, exclude, installed, stats_on, check = json.loads(r.stdout)
    assert only == ["containers"] and exclude == ["list.sort"]
    assert "list.sort" not in installed and "list.index" in installed and "builtins.sum" not in installed
    assert stats_on is True and check == "raise"
    assert r.stderr.startswith("rustique: RUSTIQUE_ONLY: no target or group named 'nope', ignored (groups: ")


def test_only_with_every_name_unknown_installs_nothing():
    r = run(
        """
        import sys
        st = sys.modules['rustique._rustique']._accel.status()
        print(sum(t['state'] == 'installed' for t in st['targets']))
        """,
        {"RUSTIQUE": "1", "RUSTIQUE_ONLY": "nope"},
    )
    assert r.returncode == 0 and r.stdout.strip() == "0", (r.stdout, r.stderr)


def test_report_at_exit_imports_formatting_only_then(tmp_path):
    out = tmp_path / "report-{pid}.json"
    r = run(
        """
        import sys
        assert 'rustique._report' not in sys.modules and 'rustique' not in sys.modules
        [1, 2, 3].index(2)
        """,
        {"RUSTIQUE": "1", "RUSTIQUE_REPORT": str(out)},
    )
    assert r.returncode == 0, r.stderr
    files = list(tmp_path.glob("report-*.json"))
    assert len(files) == 1, (files, r.stderr)
    rec = json.loads(files[0].read_text().splitlines()[0])
    assert rec["status"]["process"]["switch"]["source"] == "RUSTIQUE=1"
    assert rec["stats"]["list.index"]["hits"] >= 1


@pytest.mark.skipif(not os.path.isfile(os.path.join(SITEDIR, "_rustique.py")),
                    reason="the extension is installed top-level here (a release wheel)")
def test_in_package_layout_loads_through_the_shim():
    # The source tree (and wheels built from the sdist): `_rustique` is the shim, which loads
    # `rustique/_rustique<suffix>` by path, still without importing the package.
    r = run(
        """
        import sys, os, importlib.util
        ext = sys.modules['_rustique']
        package_dir = os.path.dirname(importlib.util.find_spec('rustique').origin)
        print(ext.__name__, os.path.dirname(ext.__file__) == package_dir, 'rustique' in sys.modules)
        """,
        {"RUSTIQUE": "1"},
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.split() == ["rustique._rustique", "True", "False"], r.stdout


# --- interpreter exit -------------------------------------------------------------------

EXIT_MODULE = """
DEP = object()
SEP = "/"
class K:
    attr = 1000
def ident(x):
    return ("orig", x)
def same(x):
    return x
"""


def _exit_run(tmp_path, body):
    (tmp_path / "_rustique_selftest.py").write_text(EXIT_MODULE)
    code = f"import sys; sys.path.insert(0, {str(tmp_path)!r})\n" + textwrap.dedent(body)
    e = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    return subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, env=e, timeout=120)


def test_exit_hook_without_importing_atexit(tmp_path):
    r = _exit_run(
        tmp_path,
        """
        import _rustique_selftest as m
        from rustique._rustique import _accel as A
        A.enable(["_selftest.vectorcall", "_selftest.same", "containers"])
        print("atexit" in sys.modules, A.exit_hook_registered(), A._selftest_vectorcall(m.ident))
        """,
    )
    assert r.returncode == 0 and r.stdout.split() == ["False", "True", "kernel"], (r.stdout, r.stderr)
    assert r.stderr == ""


def test_exit_hook_runs_after_later_atexit_callbacks(tmp_path):
    # Callbacks run last in, first out: one registered before the first enable() runs after
    # the hook (originals restored), one registered after it runs before (kernels).
    r = _exit_run(
        tmp_path,
        """
        import atexit
        import _rustique_selftest as m
        from rustique._rustique import _accel as A
        atexit.register(lambda: print("before", A._selftest_vectorcall(m.ident), m.ident(1)))
        A.enable(["_selftest.vectorcall"])
        atexit.register(lambda: print("after", A._selftest_vectorcall(m.ident), m.ident(1)))
        """,
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.splitlines() == ["after kernel ('kernel', 1)", "before original ('orig', 1)"], r.stdout


def test_kernels_on_daemon_threads_survive_exit(tmp_path):
    r = _exit_run(
        tmp_path,
        """
        import threading, time
        import _rustique_selftest as m
        from rustique._rustique import _accel as A
        A.enable(["_selftest.vectorcall", "_selftest.same", "containers", "builtins"])
        def spin():
            xs = [3, 1, 2]
            while True:
                m.ident(1); m.same(2); xs.index(2); sorted(xs); sum(xs)
                A._selftest_call_kernel(m.ident, 1)
        for _ in range(4):
            threading.Thread(target=spin, daemon=True).start()
        time.sleep(0.05)
        print("exiting")
        """,
    )
    assert r.returncode == 0 and r.stdout == "exiting\n", (r.stdout, r.stderr)


def test_finalization_without_the_hook(tmp_path):
    # `atexit._clear()` drops the hook: the dict watcher stops at Py_IsFinalizing instead.
    r = _exit_run(
        tmp_path,
        """
        import atexit
        import _rustique_selftest as m
        from rustique._rustique import _accel as A
        A.enable(["_selftest.vectorcall", "_selftest.same"])
        atexit._clear()
        class Late:
            def __del__(self):
                print("late", m.ident(1) if callable(m.ident) else None)
        m.LATE = Late()
        """,
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() in ("late ('orig', 1)", "late None", ""), r.stdout
