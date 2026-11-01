"""The `RUSTIQUE=1` startup path that loads only the extension (docs/ACCELERATION.md §8;
the environment semantics are dx's, tested in tests/test_dx*.py): the `.pth` line loads
`rustique._rustique` by file without importing the `rustique` package and calls
`_accel.startup()`.

Each case runs a fresh interpreter that executes the line exactly as `site` does for a
`.pth` import line (`exec(line)` in a frame with a local `sitedir`), with `sitedir` set to
the directory holding the `rustique` package.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import textwrap

import pytest

# The line shipped in `_rustique_startup.pth` (dx's file; this is its exact text). `site`
# compiles every `.pth` import line at each start, so the line itself stays short
# (RUSTIQUE unset or 0: one getenv); the loader is a string literal, compiled by exec()
# only when RUSTIQUE is set. It runs through exec() in site's frame, where `sitedir` is
# defined (3.14: addpackage's parameter; 3.15: injected by _exec_imports for
# compatibility); the inner exec sees the same locals; it uses no nested scopes.
# Compiled only when RUSTIQUE is set (and not "0"); kept small: 3.15's compiler costs ~140k
# Ir for a one-call statement, 0.43M for this one, 1.0M for an earlier walrus/filter chain.
# `_frozen_importlib._load` is importlib's own spec loader (sys.modules insertion, module
# attributes, cleanup on failure). Wheel installs: `sitedir/rustique/`; other layouts
# (editable installs, where the package is on sys.path): a normal import. Other OFF values
# ("off", "false", "no", "") load the extension and `startup()` enables nothing.
LOADER = (
    "import sys,_imp;p=sitedir+'/rustique/_rustique'+_imp.extension_suffixes()[0];"
    "sys.flags.ignore_environment or (sys.modules['_frozen_importlib']._load("
    "sys.modules['_frozen_importlib_external'].spec_from_file_location('rustique._rustique',p)) "
    "if os.path.isfile(p) else __import__('rustique._rustique')._rustique)._accel.startup()"
)
PTH_LINE = "import os; os.environ.get('RUSTIQUE', '0') != '0' and exec(" + repr(LOADER) + ")"

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


@pytest.mark.parametrize("value", [None, "0"])
def test_unset_or_0_imports_nothing(value):
    r = run(
        "import sys\nnames = sorted(m for m in sys.modules if m.startswith('rustique'))\nprint(names)",
        {"RUSTIQUE": value},
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() == "[]", r.stdout
    assert r.stderr == ""


def test_startup_loads_only_the_extension_and_enables():
    r = run(
        """
        import sys
        assert 'rustique' not in sys.modules, sorted(sys.modules)
        assert 'rustique._rustique' in sys.modules
        ext = sys.modules['rustique._rustique']
        st = ext._accel.status()
        installed = sorted(t['name'] for t in st['targets'] if t['state'] == 'installed')
        print(st['process']['switch']['source'], len(installed))
        # The package imported afterwards reuses the loaded extension.
        import rustique
        assert rustique._rustique is sys.modules['rustique._rustique'] is ext
        assert rustique.status()['process']['enabled']
        assert type(rustique.List([1, 2])).__name__ == 'List'   # lazy opt-in API
        assert [3, 1, 2].index(2) == 2 and sum([1, 2]) == 3
        """,
        {"RUSTIQUE": "1"},
    )
    assert r.returncode == 0, r.stderr
    assert r.stderr == ""
    source, n = r.stdout.split()
    assert source == "RUSTIQUE=1" and int(n) >= 13


@pytest.mark.parametrize("value", ["off", "", "No"])
def test_other_off_values_enable_nothing(value):
    r = run(
        """
        import sys
        assert 'rustique' not in sys.modules
        st = sys.modules['rustique._rustique']._accel.status()
        print(st['process']['enabled'], st['process']['switch']['source'])
        """,
        {"RUSTIQUE": value},
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.split() == ["False", "None"] and r.stderr == ""


def test_ignore_environment_imports_nothing():
    r = run("import sys; print([m for m in sys.modules if m.startswith('rustique')])", {"RUSTIQUE": "1"}, "-E")
    assert r.returncode == 0 and r.stdout.strip() == "[]", (r.stdout, r.stderr)


def test_kill_switch_and_bad_value():
    r = run("import sys; print('rustique._rustique' in sys.modules)", {"RUSTIQUE": "maybe"})
    assert r.returncode == 0
    assert r.stderr.strip() == "rustique: ignoring RUSTIQUE='maybe' (expected 1 or 0)"
    r = run(
        """
        import rustique
        st = rustique.enable()
        print(st['process']['enabled'], st['process']['switch']['killed'], st['process']['switch']['killed_calls'])
        """,
        {"RUSTIQUE": "false"},
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.split() == ["False", "True", "1"]


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


def test_other_layouts_fall_back_to_a_normal_import(tmp_path):
    # The `.pth` sits in a site dir without the package (editable installs): the extension
    # comes through the package on sys.path.
    r = run(
        """
        import sys
        st = sys.modules['rustique._rustique']._accel.status()
        print(st['process']['switch']['source'], 'rustique' in sys.modules)
        """,
        {"RUSTIQUE": "1"},
        sitedir=str(tmp_path),
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.split() == ["RUSTIQUE=1", "True"]
