"""The user-facing switches (docs/USAGE.md, docs/targets/dx.md): the startup hook, the
environment variables, the kill switch, selection, the report at exit.

Selection and kill-switch logic is tested against a fake framework (`rustique._accel`, the
extension's primitives), so these tests hold whatever targets are registered; a few tests
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
from rustique import _report

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PYSRC = os.path.join(ROOT, "python")
VARS = ("RUSTIQUE", "RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE", "RUSTIQUE_STATS", "RUSTIQUE_REPORT", "RUSTIQUE_CHECK")


def clean_env(**extra: str) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in VARS}
    env.update(extra)
    return env


def py(code: str, *args: str, env: dict[str, str] | None = None, flags: tuple[str, ...] = ()):
    return subprocess.run([sys.executable, *flags, "-c", code, *args], capture_output=True, text=True,
                          env=env if env is not None else clean_env(), timeout=120)


# --- fake framework -------------------------------------------------------------------------


class FakeAccel:
    """Stands for `rustique._rustique._accel` (the framework's primitives)."""

    TARGETS = [("json.dumps", "json"), ("json.loads", "json"), ("copy.deepcopy", "copy"), ("re.match", "re")]
    RustiqueCheckError = RuntimeError

    def __init__(self):
        self.calls: list = []
        self.installed: set[str] = set()
        self.mode = "off"
        self.stats_on = False

    def targets(self):
        return [(n, g, None) for n, g in self.TARGETS]

    def enable(self, only=None, exclude=None):
        self.calls.append(("enable", {"only": only, "exclude": exclude}))
        for n, g in self.TARGETS:
            if (only is None or n in only or g in only) and not (n in (exclude or ()) or g in (exclude or ())):
                self.installed.add(n)

    def disable(self, names=None):
        self.calls.append(("disable",))
        self.installed.clear()

    def status(self):
        return {"targets": [{"name": n, "group": g, "state": "installed" if n in self.installed else "disabled",
                             "reason": None, "refusal": None, "sites": [], "versions": ["3.15"]}
                            for n, g in self.TARGETS],
                "process": {"stats": self.stats_on, "check": self.mode}}

    def stats(self, enabled=None, reset=False):
        self.calls.append(("stats", enabled))
        if enabled is not None:
            self.stats_on = enabled
        return {n: {"hits": 1, "fallback": {}} for n in self.installed}

    def check(self, mode):
        self.calls.append(("check", mode))
        prev, self.mode = self.mode, mode
        return prev

    def check_mode(self):
        return self.mode


@pytest.fixture
def fake(monkeypatch):
    fw = FakeAccel()
    monkeypatch.setattr(rustique, "_accel", fw)
    monkeypatch.setattr(rustique, "_state", {"source": None, "only": None, "exclude": (), "killed_calls": 0,
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
    assert enables(fake) == [{"only": ["json", "copy"], "exclude": ["json.loads"]}]
    assert fake.installed == {"json.dumps", "copy.deepcopy"}
    rustique.enable(builtins=False)
    assert enables(fake)[-1] == {"only": None, "exclude": ["builtins"]}


def test_env_only_and_exclude_apply_to_explicit_enable(fake, monkeypatch):
    monkeypatch.setenv("RUSTIQUE_ONLY", "json,copy")
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", "json.loads")
    rustique.enable()
    assert enables(fake)[0]["only"] == ["json", "copy"]
    assert enables(fake)[0]["exclude"] == ["json.loads"]


def test_env_only_intersects_code_only(fake, monkeypatch):
    """The environment narrows, never widens: code says json + copy, env says json."""
    monkeypatch.setenv("RUSTIQUE_ONLY", "json")
    rustique.enable(only=["json.dumps", "copy"])
    assert enables(fake)[0]["only"] == ["json.dumps"]
    assert fake.installed == {"json.dumps"}


def test_env_exclude_adds_to_code_exclude(fake, monkeypatch):
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", "re")
    rustique.enable(exclude="copy")
    assert enables(fake)[0]["exclude"] == ["copy", "re"]
    assert fake.installed == {"json.dumps", "json.loads"}


def test_unknown_env_names_warn_and_are_dropped(fake, monkeypatch, capsys):
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", "jsn,re")
    rustique.enable()
    err = capsys.readouterr().err
    assert "RUSTIQUE_EXCLUDE: no target or group named 'jsn', ignored" in err and "copy, json, re" in err
    assert enables(fake)[-1]["exclude"] == ["re"]  # the framework raises on unknown names
    monkeypatch.delenv("RUSTIQUE_EXCLUDE")
    monkeypatch.setenv("RUSTIQUE_ONLY", "jsn")
    rustique.enable()
    assert enables(fake)[-1]["only"] == []  # narrows to nothing, never to everything


def test_unknown_env_names_never_raise_with_the_real_framework(monkeypatch):
    for k in VARS:
        monkeypatch.delenv(k, raising=False)
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", "no-such-group")
    try:
        st = rustique.enable()
        assert any(t["state"] == "installed" for t in st["targets"])
    finally:
        rustique.disable()
    with pytest.raises(ValueError):
        rustique.enable(exclude=["no-such-group"])


def test_names_parsing():
    assert rustique._names(None) is None
    assert rustique._names("json, copy;re,,json") == ("json", "copy", "re")
    assert rustique._names(["a", " b "]) == ("a", "b")
    with pytest.raises(TypeError):
        rustique._names([1])


def test_stats_switches(fake, monkeypatch):
    rustique.enable()
    assert ("stats", True) not in fake.calls
    monkeypatch.setenv("RUSTIQUE_STATS", "1")
    rustique.enable()
    assert ("stats", True) in fake.calls
    assert rustique.status()["process"]["switch"]["stats"]


def test_source_is_the_first_enable_path(fake):
    rustique._startup()
    rustique.enable()
    assert rustique.status()["process"]["switch"]["source"] == "RUSTIQUE=1"
    rustique.disable()
    assert rustique.status()["process"]["switch"]["source"] is None


@pytest.mark.parametrize(("value", "mode"), [("1", "warn"), ("warn", "warn"), ("RAISE", "raise"), ("0", "off"),
                                             ("off", "off")])
def test_check_env(fake, monkeypatch, value, mode):
    monkeypatch.setenv("RUSTIQUE_CHECK", value)
    rustique.enable()
    assert ("check", mode) in fake.calls


def test_check_env_unset_and_invalid(fake, monkeypatch, capsys):
    rustique.enable()
    assert not [c for c in fake.calls if c[0] == "check"]
    monkeypatch.setenv("RUSTIQUE_CHECK", "maybe")
    rustique.enable()
    assert not [c for c in fake.calls if c[0] == "check"]
    assert "ignoring RUSTIQUE_CHECK='maybe'" in capsys.readouterr().err


def test_check_not_set_when_killed(fake, monkeypatch):
    monkeypatch.setenv("RUSTIQUE", "0")
    monkeypatch.setenv("RUSTIQUE_CHECK", "1")
    rustique.enable()
    assert fake.calls == []


@pytest.mark.parametrize(("value", "expected"), [
    (None, None), ("", None), ("0", None), ("1", "stderr"), ("stderr", "stderr"), ("true", "stderr"),
    ("/tmp/r.json", "/tmp/r.json"),
])
def test_report_target(monkeypatch, value, expected):
    if value is None:
        monkeypatch.delenv("RUSTIQUE_REPORT", raising=False)
    else:
        monkeypatch.setenv("RUSTIQUE_REPORT", value)
    assert rustique._report_target() == expected


def test_report_target_pid(monkeypatch):
    monkeypatch.setenv("RUSTIQUE_REPORT", "/tmp/r-{pid}.txt")
    assert rustique._report_target() == f"/tmp/r-{os.getpid()}.txt"


# --- report formatting --------------------------------------------------------------------


def record(**kw):
    targets = [
        {"name": "json.dumps", "group": "json", "state": "installed", "reason": None},
        {"name": "copy.deepcopy", "group": "copy", "state": "installed", "reason": None},
        {"name": "logging.Formatter.format", "group": "logging", "state": "pending-import", "reason": None},
        {"name": "re.match", "group": "re", "state": "refused", "reason": "CPython 3.14.9 is not validated (3.15)",
         "refusal": "version"},
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
    assert "re.match  [version] CPython 3.14.9 is not validated (3.15)" in text
    assert "not selected: uuid" in text
    assert "check mode" not in text


def test_render_check_mode():
    rec = record()
    rec["status"]["process"]["check"] = "warn"
    rec["status"]["targets"][0]["checkable"] = True
    assert "check mode: warn (1 installed checkable targets" in _report.render(rec)
    assert "check mismatches: none" in _report.render(rec)
    rec["stats"]["json.dumps"]["fallback"]["check"] = 2
    assert _report.mismatches(rec) == {"json.dumps": 2}
    assert "CHECK MISMATCHES: 2 in json.dumps (2)" in _report.render(rec)


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


def test_real_check_env(real, monkeypatch):
    monkeypatch.setenv("RUSTIQUE_CHECK", "1")
    try:
        st = rustique.enable()
        assert st["process"]["check"] == "warn" and st["process"]["switch"]["check"] == "warn"
        assert rustique.check() == "warn"
        assert sorted([3, 1, 2]) == [1, 2, 3]
    finally:
        assert rustique.check(None) == "warn"
    assert rustique.check() in (None, "off")


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
    assert "os.environ.get('RUSTIQUE', '0') != '0'" in lines[0]
    assert not os.path.exists(os.path.join(PYSRC, "_rustique_startup.start"))  # see docs/targets/dx.md


def test_wheel_ships_the_startup_files():
    with open(os.path.join(ROOT, "pyproject.toml"), "rb") as f:
        cfg = tomllib.load(f)
    inc = {i["path"]: i["format"] for i in cfg["tool"]["maturin"]["include"] if isinstance(i, dict)}
    assert inc.get("python/_rustique_startup.py") == ["sdist", "wheel"]
    assert inc.get("python/_rustique_startup.pth") == ["sdist", "wheel"]
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
