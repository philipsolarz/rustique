"""The user-facing switches (docs/USAGE.md, docs/targets/dx.md): the startup hook, the
environment variables, the kill switch, selection, the report at exit.

Selection and kill-switch logic is tested against a fake framework (what reaches
`rustique._framework`), so these tests hold whatever targets are registered; a few tests
run the real framework end to end in subprocesses.
"""

import json
import os
import shutil
import subprocess
import sys
import tomllib

import pytest

import rustique
from rustique import _report, _switch

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PYSRC = os.path.join(ROOT, "python")
VARS = ("RUSTIQUE", "RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE", "RUSTIQUE_STATS", "RUSTIQUE_REPORT")


def clean_env(**extra: str) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in VARS}
    env.update(extra)
    return env


def py(code: str, *args: str, env: dict[str, str] | None = None, flags: tuple[str, ...] = ()):
    return subprocess.run([sys.executable, *flags, "-c", code, *args], capture_output=True, text=True,
                          env=env if env is not None else clean_env(), timeout=120)


# --- fake framework -------------------------------------------------------------------------


class FakeFramework:
    NATIVE_API = True
    TARGETS = [
        {"name": "json.dumps", "group": "json"},
        {"name": "json.loads", "group": "json"},
        {"name": "copy.deepcopy", "group": "copy"},
        {"name": "re.match", "group": "re"},
    ]

    def __init__(self):
        self.calls: list = []
        self.installed: set[str] = set()

    def enable(self, *, only=None, exclude=None, native=None, builtins=True):
        self.calls.append(("enable", {"only": only, "exclude": exclude, "native": native, "builtins": builtins}))
        for t in self.TARGETS:
            if (only is None or t["name"] in only or t["group"] in only) and not (
                    t["name"] in (exclude or ()) or t["group"] in (exclude or ())):
                self.installed.add(t["name"])
        return self.status()

    def disable(self):
        self.calls.append(("disable",))
        self.installed.clear()
        return self.status()

    def status(self):
        return {"targets": [{**t, "state": "installed" if t["name"] in self.installed else "disabled",
                             "reason": None, "sites": [], "versions": ["3.15"]} for t in self.TARGETS],
                "process": {}}

    def stats(self, enabled=None, reset=False):
        self.calls.append(("stats", enabled))
        return {n: {"hits": 1, "fallback": {}} for n in self.installed}

    def counters_available(self):
        return True


@pytest.fixture
def fake(monkeypatch):
    fw = FakeFramework()
    monkeypatch.setattr(_switch, "_fw", fw)
    monkeypatch.setattr(_switch, "_state", {"source": None, "only": None, "exclude": (), "killed_calls": 0,
                                            "report": None})
    for k in VARS:
        monkeypatch.delenv(k, raising=False)
    return fw


def enables(fw):
    return [c[1] for c in fw.calls if c[0] == "enable"]


# --- kill switch --------------------------------------------------------------------------


@pytest.mark.parametrize("value", ["0", "false", "OFF", "no", " 0 "])
def test_kill_switch_makes_enable_a_noop(fake, monkeypatch, value):
    monkeypatch.setenv("RUSTIQUE", value)
    st = rustique.enable(only=["json"])
    assert enables(fake) == []
    assert not fake.installed
    sw = st["process"]["switch"]
    assert sw["killed"] and sw["killed_calls"] == 1 and sw["source"] is None


def test_kill_switch_native_returns_the_object(fake, monkeypatch):
    monkeypatch.setenv("RUSTIQUE", "0")

    def f():
        return [1, 2]

    assert rustique.native(f) is f
    assert rustique.native()(f) is f


@pytest.mark.parametrize("value", ["1", "", "yes", "anything"])
def test_only_zero_like_values_kill(fake, monkeypatch, value):
    monkeypatch.setenv("RUSTIQUE", value)
    rustique.enable()
    assert len(enables(fake)) == 1


# --- selection ----------------------------------------------------------------------------


def test_enable_passes_selection(fake):
    rustique.enable(only="json, copy", exclude=["json.loads"])
    assert enables(fake) == [{"only": ("json", "copy"), "exclude": ("json.loads",), "native": None, "builtins": True}]
    assert fake.installed == {"json.dumps", "copy.deepcopy"}


def test_env_only_and_exclude_apply_to_explicit_enable(fake, monkeypatch):
    monkeypatch.setenv("RUSTIQUE_ONLY", "json,copy")
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", "json.loads")
    rustique.enable()
    assert enables(fake)[0]["only"] == ("json", "copy")
    assert enables(fake)[0]["exclude"] == ("json.loads",)


def test_env_only_intersects_code_only(fake, monkeypatch):
    """The environment narrows, never widens: code says json + copy, env says json."""
    monkeypatch.setenv("RUSTIQUE_ONLY", "json")
    rustique.enable(only=["json.dumps", "copy"])
    assert enables(fake)[0]["only"] == ("json.dumps",)
    assert fake.installed == {"json.dumps"}


def test_env_exclude_adds_to_code_exclude(fake, monkeypatch):
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", "re")
    rustique.enable(exclude="copy")
    assert enables(fake)[0]["exclude"] == ("copy", "re")
    assert fake.installed == {"json.dumps", "json.loads"}


def test_unknown_env_names_warn(fake, monkeypatch, capsys):
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", "jsn")
    rustique.enable()
    err = capsys.readouterr().err
    assert "RUSTIQUE_EXCLUDE: no target or group named 'jsn'" in err and "copy, json, re" in err


def test_names_parsing():
    assert _switch.names(None) is None
    assert _switch.names("json, copy;re,,json") == ("json", "copy", "re")
    assert _switch.names(["a", " b "]) == ("a", "b")
    with pytest.raises(TypeError):
        _switch.names([1])


def test_stats_switches(fake, monkeypatch):
    rustique.enable()
    assert ("stats", True) not in fake.calls
    monkeypatch.setenv("RUSTIQUE_STATS", "1")
    rustique.enable()
    assert ("stats", True) in fake.calls
    assert rustique.status()["process"]["switch"]["stats"]


def test_source_is_the_first_enable_path(fake):
    _switch.startup()
    rustique.enable()
    assert rustique.status()["process"]["switch"]["source"] == "RUSTIQUE=1"
    rustique.disable()
    assert rustique.status()["process"]["switch"]["source"] is None


@pytest.mark.parametrize(("value", "expected"), [
    (None, None), ("", None), ("0", None), ("1", "stderr"), ("stderr", "stderr"), ("true", "stderr"),
    ("/tmp/r.json", "/tmp/r.json"),
])
def test_report_target(monkeypatch, value, expected):
    if value is None:
        monkeypatch.delenv("RUSTIQUE_REPORT", raising=False)
    else:
        monkeypatch.setenv("RUSTIQUE_REPORT", value)
    assert _switch.report_target() == expected


def test_report_target_pid(monkeypatch):
    monkeypatch.setenv("RUSTIQUE_REPORT", "/tmp/r-{pid}.txt")
    assert _switch.report_target() == f"/tmp/r-{os.getpid()}.txt"


# --- report formatting --------------------------------------------------------------------


def record(**kw):
    targets = [
        {"name": "json.dumps", "group": "json", "state": "installed", "reason": None},
        {"name": "copy.deepcopy", "group": "copy", "state": "installed", "reason": None},
        {"name": "logging.Formatter.format", "group": "logging", "state": "pending-import", "reason": None},
        {"name": "re.match", "group": "re", "state": "refused", "reason": "CPython 3.14.9 is not validated (3.15)"},
        {"name": "uuid.uuid4", "group": "uuid", "state": "disabled", "reason": None},
    ]
    rec = {"pid": 42, "counters": True,
           "status": {"targets": targets, "process": {"python": "3.15.1", "rustique": "0.2.0", "free_threaded": False,
                                                      "switch": {"source": "RUSTIQUE=1", "stats": True}}},
           "stats": {"json.dumps": {"hits": 10, "fallback": {"args": 3, "deps": 1}},
                     "copy.deepcopy": {"hits": 2000, "fallback": {}}}}
    rec.update(kw)
    return rec


def test_render_sorts_by_hits_and_lists_refusals():
    text = _report.render(record())
    lines = text.splitlines()
    assert lines[0] == "rustique 0.2.0 on CPython 3.15.1 (GIL), pid 42: enabled by RUSTIQUE=1"
    assert "5 targets: 2 installed, 1 pending-import, 1 refused, 1 disabled" in text
    body = [ln.split() for ln in lines if ln.startswith("  copy.") or ln.startswith("  json.")]
    assert [b[0] for b in body] == ["copy.deepcopy", "json.dumps"]
    assert body[0][2] == "2,000" and body[1][2:] == ["10", "4", "(args", "3,", "deps", "1)"]
    assert "pending import (patched when their module is imported): logging.Formatter.format" in text
    assert "re.match  CPython 3.14.9 is not validated (3.15)" in text
    assert "not selected: uuid" in text


def test_render_killed():
    rec = record()
    rec["status"]["process"]["switch"] = {"killed": True, "killed_calls": 2}
    text = _report.render(rec)
    assert "disabled by RUSTIQUE=0 (kill switch)" in text and "2 enable call(s) ignored" in text


def test_merge_adds_counters():
    a, b = record(), record()
    b["status"]["targets"][2]["state"] = "installed"
    m = _report.merge([a, b])
    assert m["processes"] == 2 and m["pid"] is None
    assert m["stats"]["json.dumps"] == {"hits": 20, "fallback": {"args": 6, "deps": 2}}
    states = {t["name"]: t["state"] for t in m["status"]["targets"]}
    assert states["logging.Formatter.format"] == "installed"
    assert "2 processes" in _report.render(m).splitlines()[0]


def test_render_status_groups():
    text = _report.render_status(record()["status"])
    assert text.startswith("5 targets in 5 groups")
    assert "refused (CPython's original runs):" in text


# --- real framework, in-process -----------------------------------------------------------


@pytest.fixture
def real(monkeypatch):
    for k in VARS:
        monkeypatch.delenv(k, raising=False)
    was = {t["name"] for t in rustique.status()["targets"] if t["state"] == "installed"}
    yield
    rustique.disable()
    if was:
        rustique.enable()


def test_real_status_shape(real):
    st = rustique.enable()
    assert st["targets"], "the framework reports no target"
    for t in st["targets"]:
        assert {"name", "group", "state", "reason", "sites", "versions"} <= t.keys()
        assert t["state"] in ("installed", "pending-import", "refused", "disabled")
    proc = st["process"]
    assert proc["python"] == sys.version.split()[0] and proc["rustique"] == rustique.__version__
    assert proc["switch"]["source"] == "rustique.enable()"
    assert all(t["state"] != "installed" for t in rustique.disable()["targets"])


def test_real_exclude_a_group(real):
    st = rustique.enable(exclude=["builtins"])
    states = {t["name"]: t["state"] for t in st["targets"] if t["group"] == "builtins"}
    assert states and all(s != "installed" for s in states.values())
    assert sum(1 for x in [1, 2] if x) == 2


def test_real_kill_switch(real, monkeypatch):
    monkeypatch.setenv("RUSTIQUE", "0")
    st = rustique.enable()
    assert all(t["state"] != "installed" for t in st["targets"])


# --- the startup hook (subprocesses) ------------------------------------------------------


@pytest.fixture
def sitedir(tmp_path):
    """A site directory holding what the wheel puts at the root of site-packages."""
    d = tmp_path / "site"
    d.mkdir()
    for name in ("_rustique_startup.pth", "_rustique_startup.py"):
        shutil.copy(os.path.join(PYSRC, name), d / name)
    return str(d)


PROBE = """
import site, sys
site.addsitedir(sys.argv[1])
loaded = sorted(m for m in sys.modules if 'rustique' in m)
out = {'loaded': loaded}
if 'rustique' in sys.modules:
    sw = sys.modules['rustique'].status()['process']['switch']
    out['source'] = sw['source']
    out['killed'] = sw['killed']
    out['installed'] = sorted(t['name'] for t in sys.modules['rustique'].status()['targets'] if t['state'] == 'installed')
print(__import__('json').dumps(out))
"""


def probe(sitedir, flags=(), **env):
    r = py(PROBE, sitedir, env=clean_env(**env), flags=flags)
    assert r.returncode == 0, r.stderr
    return json.loads(r.stdout), r.stderr


def test_pth_is_one_cheap_import_line():
    with open(os.path.join(PYSRC, "_rustique_startup.pth")) as f:
        lines = [ln for ln in f.read().splitlines() if ln.strip() and not ln.startswith("#")]
    assert len(lines) == 1 and lines[0].startswith("import ")  # site exec()s only `import ` lines
    assert "'RUSTIQUE' in os.environ" in lines[0]
    assert not os.path.exists(os.path.join(PYSRC, "_rustique_startup.start"))  # see docs/targets/dx.md


def test_wheel_ships_the_startup_files():
    with open(os.path.join(ROOT, "pyproject.toml"), "rb") as f:
        cfg = tomllib.load(f)
    inc = {i["path"]: i["format"] for i in cfg["tool"]["maturin"]["include"] if isinstance(i, dict)}
    assert inc.get("python/_rustique_startup.py") == "wheel"
    assert inc.get("python/_rustique_startup.pth") == "wheel"
    assert cfg["project"]["entry-points"]["pytest11"] == {"rustique.pytest_plugin": "rustique.pytest_plugin"}


def test_startup_unset_imports_nothing(sitedir):
    out, err = probe(sitedir)
    assert out["loaded"] == [] and err == ""


@pytest.mark.parametrize("value", ["0", "off", ""])
def test_startup_off_values_do_not_import_rustique(sitedir, value):
    out, err = probe(sitedir, RUSTIQUE=value)
    assert "rustique" not in out["loaded"] and err == ""


@pytest.mark.parametrize("value", ["1", "true", "ON"])
def test_startup_enables(sitedir, value):
    out, err = probe(sitedir, RUSTIQUE=value)
    assert out["source"] == "RUSTIQUE=1" and out["installed"] and err == ""


def test_startup_honours_selection(sitedir):
    out, _ = probe(sitedir, RUSTIQUE="1", RUSTIQUE_EXCLUDE="builtins")
    assert out["installed"] and not any(n.startswith("builtins.") for n in out["installed"])


def test_startup_invalid_value_warns(sitedir):
    out, err = probe(sitedir, RUSTIQUE="maybe")
    assert "rustique" not in out["loaded"]
    assert "ignoring RUSTIQUE='maybe'" in err


@pytest.mark.parametrize("flag", ["-E", "-I"])
def test_startup_ignores_environment_under_E_and_I(sitedir, flag):
    out, err = probe(sitedir, flags=(flag,), RUSTIQUE="1")
    assert "rustique" not in out["loaded"] and err == ""


def test_kill_switch_ignored_under_E():
    r = py("import rustique; print(rustique.enable()['process']['switch']['killed'])", flags=("-E",),
           env=clean_env(RUSTIQUE="0"))
    assert r.stdout.strip() == "False", r.stderr


def test_startup_survives_a_broken_install(sitedir, tmp_path):
    broken = tmp_path / "broken" / "rustique"
    broken.mkdir(parents=True)
    (broken / "__init__.py").write_text("raise ImportError('no wheel for this interpreter')\n")
    code = f"import sys; sys.path.insert(0, {str(broken.parent)!r})\n" + PROBE
    r = py(code, sitedir, env=clean_env(RUSTIQUE="1"))
    assert r.returncode == 0
    assert "RUSTIQUE=1 ignored: cannot import rustique: no wheel for this interpreter" in r.stderr


def test_hook_start_is_idempotent(sitedir):
    r = py("import site, sys; site.addsitedir(sys.argv[1]); import _rustique_startup as h, rustique;"
           "h.start(); h.start(); print(rustique.status()['process']['switch']['source'])", sitedir,
           env=clean_env(RUSTIQUE="1"))
    assert r.stdout.strip() == "RUSTIQUE=1", r.stderr


# --- report at exit (subprocesses) --------------------------------------------------------


def test_report_at_exit_stderr():
    r = py("import rustique; rustique.enable(); print(sorted([3, 1, 2]))", env=clean_env(RUSTIQUE_REPORT="1"))
    assert r.returncode == 0 and r.stdout == "[1, 2, 3]\n"
    first = r.stderr.splitlines()[0]
    assert first.startswith(f"rustique {rustique.__version__} on CPython") and "enabled by rustique.enable()" in first


def test_report_at_exit_json(tmp_path):
    path = tmp_path / "r-{pid}.json"
    r = py("import rustique; rustique.enable(); print(sum([1, 2]))", env=clean_env(RUSTIQUE_REPORT=str(path)))
    assert r.returncode == 0 and r.stderr == ""
    (f,) = list(tmp_path.glob("r-*.json"))
    rec = json.loads(f.read_text())
    assert rec["pid"] == int(f.stem[2:]) and rec["status"]["targets"]
    assert rec["status"]["process"]["switch"]["source"] == "rustique.enable()"


def test_report_says_killed():
    r = py("import rustique; rustique.enable()", env=clean_env(RUSTIQUE="0", RUSTIQUE_REPORT="1"))
    assert "disabled by RUSTIQUE=0 (kill switch)" in r.stderr and "1 enable call(s) ignored" in r.stderr


def test_no_report_without_enable():
    r = py("import rustique", env=clean_env(RUSTIQUE_REPORT="1"))
    assert r.stderr == ""
