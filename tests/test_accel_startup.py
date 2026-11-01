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

# The line shipped in `_rustique_startup.pth` (one logical line; no nested scopes: it runs
# through exec() in site's frame, where only `sitedir`, `os` and `sys` are guaranteed).
PTH_LINE = (
    "import os, sys, _imp; "
    "os.environ.get('RUSTIQUE', '0').strip().lower() not in ('', '0', 'false', 'no', 'off') "
    "and not sys.flags.ignore_environment "
    "and 'rustique._rustique' not in sys.modules "
    "and (p := next(filter(os.path.isfile, map(os.path.join, [sitedir, *sys.path], "
    "[os.path.join('rustique', '_rustique' + _imp.extension_suffixes()[0])] * (len(sys.path) + 1))), None)) "
    "and (s := sys.modules['_frozen_importlib_external'].spec_from_file_location('rustique._rustique', p)) "
    "and (m := sys.modules['_frozen_importlib'].module_from_spec(s)) "
    "and not s.loader.exec_module(m) "
    "and not sys.modules.__setitem__('rustique._rustique', m) "
    "and m._accel.startup()"
)

SITEDIR = os.path.dirname(os.path.dirname(importlib.util.find_spec("rustique").origin))


def run(body: str, env: dict[str, str | None], *flags: str) -> subprocess.CompletedProcess[str]:
    """Run the `.pth` line as site would, then `body`, in a fresh interpreter."""
    prologue = (
        "def _site(sitedir):\n"
        f"    exec({PTH_LINE!r})\n"
        f"_site({SITEDIR!r})\n"
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


@pytest.mark.parametrize("value", [None, "0", "off", ""])
def test_unset_or_off_imports_nothing(value):
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
