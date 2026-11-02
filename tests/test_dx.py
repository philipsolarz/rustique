"""The user-facing switches (docs/USAGE.md, docs/targets/dx.md): the startup hook, the
environment variables, the kill switch, selection, the report at exit.

These tests are the contract of the switches: they go through the public API, the shipped
`.pth` line and environment variables only, so they hold whichever layer implements the
environment handling (Python, or the extension's `_accel`).
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
VARS = ("RUSTIQUE", "RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE", "RUSTIQUE_STATS", "RUSTIQUE_REPORT", "RUSTIQUE_CHECK",
        "RUSTIQUE_TIERS", "RUSTIQUE_UNVALIDATED_PATCH")
LIVE = ("installed", "armed")  # "armed": sites swapped, validated at the first call


def clean_env(**extra: str) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in VARS}
    env.update(extra)
    return env


def py(code: str, *args: str, env: dict[str, str] | None = None, flags: tuple[str, ...] = ()):
    return subprocess.run([sys.executable, *flags, "-c", code, *args], capture_output=True, text=True,
                          env=env if env is not None else clean_env(), timeout=120)


# --- the switch semantics: the contract (docs/USAGE.md "Reference: environment variables") ----
#
# Behaviour through the public API and the real framework, whatever implements the
# environment handling (Python today, `_accel` in Rust per the lead's plan). Groups of the
# registry used here: `containers` (list/tuple methods) and `builtins` (sum/min/max/...).

CONTAINERS = {"list.sort", "list.index", "list.count", "tuple.index", "tuple.count", "list.__contains__",
              "tuple.__contains__"}


@pytest.fixture
def real(monkeypatch):
    for k in VARS:
        monkeypatch.delenv(k, raising=False)
    was = installed()
    yield
    rustique.disable()
    rustique.check(None)
    if was:
        rustique.enable()


# Assertions look at the wave-0 groups only: target groups whose module is already imported
# (pytest imports logging, json, ...) also install on a bare enable().
WAVE0 = {"containers", "builtins"}


def installed(st=None) -> set[str]:
    st = st or rustique.status()
    return {t["name"] for t in st["targets"] if t["state"] in LIVE and t["group"] in WAVE0}


def groups(st=None) -> set[str]:
    st = st or rustique.status()
    return {t["group"] for t in st["targets"] if t["state"] in LIVE and t["group"] in WAVE0}


@pytest.mark.parametrize("value", ["0", "false", "OFF", "no", " 0 ", ""])
def test_kill_switch_makes_enable_a_noop(real, monkeypatch, value):
    rustique.disable()
    monkeypatch.setenv("RUSTIQUE", value)
    calls = rustique.status()["process"]["switch"]["killed_calls"]
    monkeypatch.setenv("RUSTIQUE_CHECK", "1")
    monkeypatch.setenv("RUSTIQUE_STATS", "1")
    st = rustique.enable(only=["containers"])
    assert installed(st) == set()
    sw = st["process"]["switch"]
    assert sw["killed"] and sw["killed_calls"] == calls + 1 and sw["source"] is None
    assert rustique.check() in (None, "off") and not st["process"]["stats"]  # nothing else switched on


@pytest.mark.parametrize("value", ["1", "yes", "anything"])
def test_only_off_values_kill(real, monkeypatch, value):
    monkeypatch.setenv("RUSTIQUE", value)
    assert installed(rustique.enable()) >= CONTAINERS


def test_enable_selection(real):
    st = rustique.enable(only="containers, builtins", exclude=["list.sort"])
    assert installed(st) >= CONTAINERS - {"list.sort"} and "list.sort" not in installed(st)
    assert st["process"]["switch"]["exclude"] == ("list.sort",)
    rustique.disable()
    assert groups(rustique.enable(builtins=False)) == {"containers"}


def test_code_unknown_names_raise(real):
    with pytest.raises(ValueError):
        rustique.enable(exclude=["no-such-group"])
    assert installed() == set()


def test_env_only_and_exclude_apply_to_explicit_enable(real, monkeypatch):
    monkeypatch.setenv("RUSTIQUE_ONLY", "containers")
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", "list.sort")
    assert installed(rustique.enable()) == CONTAINERS - {"list.sort"}


def test_env_only_intersects_code_only(real, monkeypatch):
    """The environment narrows, never widens: code says list.sort + builtins, env says containers."""
    monkeypatch.setenv("RUSTIQUE_ONLY", "containers")
    st = rustique.enable(only=["list.sort", "builtins"])
    assert installed(st) == {"list.sort"}
    assert st["process"]["switch"]["only"] == ("list.sort",)


def test_env_exclude_adds_to_code_exclude(real, monkeypatch):
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", "builtins")
    st = rustique.enable(exclude="list.sort")
    assert installed(st) == CONTAINERS - {"list.sort"}
    assert st["process"]["switch"]["exclude"] == ("list.sort", "builtins")


def test_unknown_env_names_warn_and_are_dropped(real, monkeypatch, capfd):
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", "jsn;builtins")
    assert groups(rustique.enable()) == {"containers"}  # never a ValueError from the environment
    err = capfd.readouterr().err
    assert "rustique: RUSTIQUE_EXCLUDE: no target or group named 'jsn', ignored (groups: builtins, containers" in err
    rustique.disable()
    monkeypatch.delenv("RUSTIQUE_EXCLUDE")
    monkeypatch.setenv("RUSTIQUE_ONLY", "jsn")
    assert installed(rustique.enable()) == set()  # narrows to nothing, never to everything
    assert "RUSTIQUE_ONLY: no target or group named 'jsn'" in capfd.readouterr().err


def test_stats_env(real, monkeypatch):
    rustique.stats(False)
    rustique.enable()
    assert not rustique.status()["process"]["stats"]
    monkeypatch.setenv("RUSTIQUE_STATS", "1")
    st = rustique.enable()
    assert st["process"]["stats"] and st["process"]["switch"]["stats"]
    rustique.stats(False)


def test_source_is_the_first_enable_path(real):
    rustique.disable()
    rustique.enable()
    rustique.enable(only="containers")
    assert rustique.status()["process"]["switch"]["source"] == "rustique.enable()"
    assert rustique.disable()["process"]["switch"]["source"] is None


@pytest.mark.parametrize(("value", "mode"), [("1", "warn"), ("warn", "warn"), ("RAISE", "raise"), ("0", "off"),
                                             ("off", "off")])
def test_check_env(real, monkeypatch, value, mode):
    rustique.check("warn" if mode == "off" else None)
    monkeypatch.setenv("RUSTIQUE_CHECK", value)
    st = rustique.enable()
    assert rustique.check() in ((None, "off") if mode == "off" else (mode,))
    assert st["process"]["switch"]["check"] == st["process"]["check"]


def test_check_env_unset_and_invalid(real, monkeypatch, capfd):
    rustique.check("raise")
    rustique.enable()
    assert rustique.check() == "raise"  # unset: left as is
    monkeypatch.setenv("RUSTIQUE_CHECK", "maybe")
    rustique.enable()
    assert rustique.check() == "raise"
    assert "rustique: ignoring RUSTIQUE_CHECK='maybe' (expected 1, raise or 0)" in capfd.readouterr().err


def test_switch_state_shape(real, monkeypatch):
    monkeypatch.setenv("RUSTIQUE_STATS", "1")
    sw = rustique.enable(only="containers")["process"]["switch"]
    assert {"source", "only", "exclude", "killed_calls", "report", "killed", "stats", "check", "env"} <= sw.keys()
    assert sw["env"] == {"RUSTIQUE_STATS": "1"} and sw["only"] == ("containers",) and sw["exclude"] == ()
    rustique.stats(False)


def test_names_are_comma_or_semicolon_separated(real, monkeypatch):
    monkeypatch.setenv("RUSTIQUE_EXCLUDE", " builtins ;; list.sort,builtins ")
    assert installed(rustique.enable()) == CONTAINERS - {"list.sort"}


# --- report formatting --------------------------------------------------------------------


def record(**kw):
    targets = [
        {"name": "json.dumps", "group": "json", "state": "installed", "reason": None},
        {"name": "copy.deepcopy", "group": "copy", "state": "installed", "reason": None},
        {"name": "copy.copy", "group": "copy", "state": "installed", "reason": None},
        {"name": "logging.Formatter.format", "group": "logging", "state": "pending-import", "reason": None,
         "module": "logging"},
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


def test_render_lists_called_targets_by_hits():
    text = _report.render(record())
    lines = text.splitlines()
    assert lines[0] == "rustique 0.2.0 on CPython 3.15.1 (GIL), pid 42: enabled by RUSTIQUE=1"
    assert ("6 targets: 2 called, 1 installed but not called, 1 waiting for their module, 1 refused, "
            "1 not selected") in text
    body = [ln.split() for ln in lines if ln.startswith(("  copy.", "  json."))]
    assert [b[0] for b in body] == ["copy.deepcopy", "json.dumps"]  # copy.copy was not called
    assert body[0][2:4] == ["2,000", "100%"]
    assert body[1][2:] == ["10", "71%", "4", "(args", "3,", "deps", "1)"]
    assert "(1 installed targets were not called)" in text
    assert "1 targets wait for their module (logging): installed when it is imported" in text
    assert "    [version] 1: re.match" in text and "reason: CPython 3.14.9 is not validated (3.15)" in text
    assert "not selected: uuid" in text
    assert "check mode" not in text


def test_render_explains_fallbacks_and_suspensions():
    rec = record()
    rec["stats"]["copy.deepcopy"] = {"hits": 5, "fallback": {"abandon": 900, "deopt": 3}, "suspensions": 2}
    rec["status"]["targets"][1]["suspended"] = "fallback"
    text = _report.render(rec)
    row = next(ln for ln in text.splitlines() if ln.startswith("  copy.deepcopy"))
    assert row.endswith("[suspended 2x; suspended now (fallback)]")
    assert "what the fallbacks mean here:" in text
    assert "    abandon (copy.deepcopy 900): the kernel restarted in CPython's code after partial work" in text
    assert "    args (json.dumps 3): inputs outside the kernel's exact subset" in text
    assert "suspended (copy.deepcopy): persistent fallback" in text


def test_render_verbose_lists_everything():
    text = _report.render(record(), verbose=True)
    assert "  copy.copy" in text and "waiting for their module (installed when it is imported): " \
        "logging.Formatter.format" in text


def test_render_refusal_categories_with_advice():
    rec = record()
    ts = rec["status"]["targets"]
    ts.append({"name": "x.a", "group": "x", "state": "refused", "reason": "CPython 3.15.3 is not a recorded patch release",
               "refusal": "unvalidated-patch"})
    ts.append({"name": "x.b", "group": "x", "state": "refused", "reason": "a.b: fingerprint 1 != expected 2",
               "refusal": "fingerprint"})
    text = _report.render(rec)
    # unexpected categories first
    assert text.index("[fingerprint] 1: x.b") < text.index("[unvalidated-patch] 1: x.a") < text.index("[version]")
    assert "RUSTIQUE_UNVALIDATED_PATCH=1" in text and "report it if this is a stock build" in text


def test_render_tiers_armed_and_unvalidated():
    rec = record()
    ts = rec["status"]["targets"]
    ts[0]["state"], ts[0]["validation"] = "armed", "unvalidated"
    ts.append({"name": "interp.isinstance", "group": "interp", "state": "disabled", "tier": "interp",
               "reason": "opt-in tier: RUSTIQUE_TIERS=interp"})
    ts.append({"name": "gc.policy", "group": "gc", "state": "installed", "tier": "gc", "reason": None})
    rec["status"]["process"]["switch"]["tiers"] = ["gc"]
    text = _report.render(rec)
    assert "opt-in tiers: gc on (1 of 1 targets active), interp off (0 of 1 targets active); " \
        "RUSTIQUE_TIERS=interp turns it on" in text
    assert "1 installed as unvalidated (RUSTIQUE_UNVALIDATED_PATCH=1)" in text
    assert "[armed: validates at its first call; unvalidated]" in text
    assert "not selected: uuid" in text  # a tier that is off is not "not selected"
    assert "[opt-in tier: interp]" in _report.render_status(rec["status"])


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
    a, b = record(), record(pid=43)
    b["status"]["targets"][3]["state"] = "installed"
    b["status"]["process"] = dict(b["status"]["process"], ppid=42)
    m = _report.merge([a, b])
    assert m["processes"] == 2 and m["pid"] is None and m["forked"] == 1
    assert m["stats"]["json.dumps"] == {"hits": 20, "fallback": {"args": 6, "deps": 2}, "suspensions": 0}
    states = {t["name"]: t["state"] for t in m["status"]["targets"]}
    assert states["logging.Formatter.format"] == "installed"
    assert "2 processes (1 forked)" in _report.render(m).splitlines()[0]


def counter_file(path, pid, rows, *, ppid=0, version=2, exit_record=False, row_words=None):
    """A live counter file in the layout agreed with accel (docs/targets/dx.md §3)."""
    import struct

    cols = 11 if version == 2 else 9
    row_words = row_words if row_words is not None else (cols if version == 2 else 0)
    names = b"".join(n.encode() + b"\0" for n in rows)
    names += b"\0" * (-len(names) % 8)
    names_off = 64
    counters_off = names_off + len(names)
    head = b"RQCNT\x00\x00" + bytes([version])
    head += struct.pack("=7Q", pid, ppid, len(rows), names_off, counters_off, int(exit_record), row_words)
    body = b"".join(struct.pack(f"={cols}Q", *vals) + b"\0" * 8 * ((row_words or cols) - cols)
                    for vals in rows.values())
    with open(path, "wb") as f:
        f.write(head + names + body)


def test_collect_reads_live_counter_files(tmp_path):
    rep = str(tmp_path / "r.json")
    with open(rep, "w") as f:
        f.write(json.dumps(record()) + "\n" + '{"torn')
    # pid 42 wrote its record: its counter file is ignored; 50 (os._exit) and 51 (v1 layout) count
    counter_file(f"{rep}.42.cnt", 42, {"json.dumps": [99] + [0] * 10})
    counter_file(f"{rep}.50.cnt", 50, {"json.dumps": [7, 1, 0, 0, 0, 0, 0, 0, 1, 0, 2], "copy.copy": [0] * 11},
                 ppid=42)
    counter_file(f"{rep}.51.cnt", 51, {"copy.deepcopy": [3, 0, 0, 0, 0, 4, 0, 0, 0]}, version=1)
    counter_file(f"{rep}.52.cnt", 52, {"copy.deepcopy": [1000] + [0] * 10}, exit_record=True)
    (tmp_path / "r.json.53.cnt").write_bytes(b"RQCNT")  # being created: skipped
    recs = _report.collect(rep)
    assert [r["pid"] for r in recs] == [42, 50, 51]
    assert recs[1] == {"pid": 50, "ppid": 42, "argv": None, "status": None, "counters": True, "exit_record": False,
                       "stats": {"json.dumps": {"hits": 7, "suspensions": 1, "fallback": {
                           "args": 1, "deps": 0, "tools": 0, "interp": 0, "deopt": 0, "error": 0, "check": 0,
                           "continue": 0, "abandon": 2}}}}
    assert recs[2]["stats"]["copy.deepcopy"]["fallback"]["deopt"] == 4
    m = _report.merge(recs)
    assert m["stats"]["json.dumps"]["hits"] == 17 and m["stats"]["copy.deepcopy"]["hits"] == 2003
    head = _report.render(m).splitlines()[0]
    assert "3 processes (1 forked, 2 ended without exit hooks, counted from their live counters)" in head


def test_counter_file_longer_rows(tmp_path):
    # a newer layout with more columns than this reader knows: the known ones are read
    p = str(tmp_path / "x.cnt")
    counter_file(p, 9, {"a.b": [5] + [0] * 10}, row_words=13)
    assert _report.read_counter_file(p)["stats"]["a.b"]["hits"] == 5


def test_render_status_groups():
    text = _report.render_status(record()["status"])
    assert text.startswith("6 targets in 5 groups: 4 available here (3 active now, 1 activate when their module is "
                           "imported), 1 refused, 1 disabled")
    assert "refused (CPython's code runs):" in text and "what to do: this CPython version" in text


# --- real framework, in-process -----------------------------------------------------------


def test_real_status_shape(real):
    st = rustique.enable()
    assert st["targets"], "the framework reports no target"
    for t in st["targets"]:
        assert {"name", "group", "state", "reason", "refusal", "sites", "versions"} <= t.keys()
        assert t["state"] in ("installed", "armed", "pending-import", "refused", "disabled")
    proc = st["process"]
    assert proc["python"] == sys.version.split()[0] and proc["rustique"] == rustique.__version__
    assert proc["switch"]["source"] == "rustique.enable()"
    assert all(t["state"] != "installed" for t in rustique.disable()["targets"])


# --- the startup hook (subprocesses) ------------------------------------------------------


@pytest.fixture
def sitedir(tmp_path):
    """A site directory laid out like an installed release wheel: the `.pth` line and the
    extension at the top level as `_rustique<suffix>` (a link to the extension under test).
    The probe puts it first on sys.path, ahead of the source tree's `_rustique.py` shim."""
    d = tmp_path / "site"
    d.mkdir()
    shutil.copy(os.path.join(PYSRC, "_rustique_startup.pth"), d / "_rustique_startup.pth")
    ext = rustique._rustique.__file__
    os.symlink(ext, d / os.path.basename(ext))
    return str(d)


PROBE = """
import site, sys
sys.path.insert(0, sys.argv[1])
site.addsitedir(sys.argv[1])
loaded = sorted(m for m in sys.modules if 'rustique' in m)
out = {'loaded': loaded}
ext = sys.modules.get('rustique._rustique')
if ext is not None:
    st = ext._accel.status()
    out['source'] = st['process']['switch']['source']
    out['killed'] = st['process']['switch']['killed']
    out['installed'] = sorted(t['name'] for t in st['targets'] if t['state'] == 'installed')
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
    # one short statement, compiled at every start (docs/targets/dx.md §1.3): the gate, then
    # a plain import of the top-level extension (docs/DECISIONS.md §10.7)
    assert lines[0] == "import os; os.environ.get('RUSTIQUE', '0') != '0' and __import__('_rustique').startup()"
    assert not os.path.exists(os.path.join(PYSRC, "_rustique_startup.start"))  # see docs/targets/dx.md


def test_wheel_ships_the_startup_files():
    with open(os.path.join(ROOT, "pyproject.toml"), "rb") as f:
        cfg = tomllib.load(f)
    inc = {i["path"]: i["format"] for i in cfg["tool"]["maturin"]["include"] if isinstance(i, dict)}
    assert inc.get("python/_rustique_startup.pth") == ["sdist", "wheel"]
    assert inc.get("python/_rustique_pytest.py") == ["sdist", "wheel"]
    assert inc.get("python/_rustique.py") == ["sdist", "wheel"]  # the in-package layout's loader
    assert cfg["project"]["entry-points"]["pytest11"] == {"rustique.pytest_plugin": "_rustique_pytest"}


def test_startup_unset_imports_nothing(sitedir):
    out, err = probe(sitedir)
    assert out["loaded"] == [] and err == ""


def test_startup_kill_switch_imports_nothing(sitedir):
    out, err = probe(sitedir, RUSTIQUE="0")
    assert out["loaded"] == [] and err == ""


@pytest.mark.parametrize("value", ["off", "FALSE", "no", "", " Off "])
def test_startup_off_values_load_nothing(sitedir, value):
    """Every OFF value is the kill switch, like 0: the startup line loads nothing."""
    out, err = probe(sitedir, RUSTIQUE=value)
    assert out["loaded"] == [] and err == ""


@pytest.mark.parametrize("value", ["1", "true", "ON"])
def test_startup_enables(sitedir, value):
    out, err = probe(sitedir, RUSTIQUE=value)
    assert out["source"] == "RUSTIQUE=1" and out["installed"] and err == ""


def test_startup_honours_selection(sitedir):
    out, _ = probe(sitedir, RUSTIQUE="1", RUSTIQUE_EXCLUDE="builtins")
    assert out["installed"] and not any(n.startswith("builtins.") for n in out["installed"])


def test_startup_invalid_value_warns(sitedir):
    out, err = probe(sitedir, RUSTIQUE="maybe")
    assert out.get("source") is None and not out.get("installed")
    assert "ignoring RUSTIQUE='maybe'" in err


@pytest.mark.parametrize("flag", ["-E", "-I"])
def test_startup_ignores_environment_under_E_and_I(sitedir, flag):
    out, err = probe(sitedir, flags=(flag,), RUSTIQUE="1")
    # the .pth line reads os.environ (imports rustique); _startup() honours the flag
    assert out.get("source") is None and not out.get("installed") and err == ""


def test_kill_switch_ignored_under_E():
    r = py("import rustique; print(rustique.enable()['process']['switch']['killed'])", flags=("-E",),
           env=clean_env(RUSTIQUE="0"))
    assert r.stdout.strip() == "False", r.stderr


def test_startup_survives_a_broken_install(sitedir, tmp_path):
    # no loadable extension anywhere: the line's import fails
    for f in os.listdir(sitedir):
        if f.startswith("_rustique."):
            os.remove(os.path.join(sitedir, f))
    broken = tmp_path / "broken"
    broken.mkdir()
    (broken / "_rustique.py").write_text("raise ImportError('no wheel for this interpreter')\n")
    code = f"import sys; sys.path.insert(0, {str(broken)!r})\n" + PROBE
    # -S: an interpreter with the wheel installed would have loaded the real extension through
    # its own startup line already (the platform job runs this suite against installed wheels)
    r = py(code, sitedir, env=clean_env(RUSTIQUE="1"), flags=("-S",))
    assert r.returncode == 0  # site reports the failing line and carries on
    assert "ImportError: no wheel for this interpreter" in r.stderr


def test_startup_and_import_load_no_other_module(sitedir):
    """The module budget (docs/targets/dx.md §1.3): `import rustique; enable()` loads the
    package and the extension, nothing else; the RUSTIQUE=1 startup path no more than that."""
    code = ("import sys; before = set(sys.modules); import rustique; rustique.enable();"
            "print(sorted(set(sys.modules) - before))")
    assert py(code).stdout.strip() == "['_rustique', 'rustique', 'rustique._rustique']"
    out, err = probe(sitedir, RUSTIQUE="1")
    assert out["loaded"] == ["_rustique", "rustique._rustique"] and out["source"] == "RUSTIQUE=1", err  # no package
    r = py("import site, sys; sys.path.insert(0, sys.argv[1]); site.addsitedir(sys.argv[1]);"
           "ext = sys.modules['_rustique']; import rustique;"
           "print(rustique._rustique is ext is sys.modules['rustique._rustique'],"
           "rustique.status()['process']['switch']['source'])",
           sitedir, env=clean_env(RUSTIQUE="1"))
    assert r.stdout.split() == ["True", "RUSTIQUE=1"], r.stderr  # a later import reuses it


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


@pytest.mark.parametrize(("value", "where"), [("stderr", "stderr"), ("TRUE", "stderr"), ("0", None), ("", None)])
def test_report_values(value, where):
    r = py("import rustique; rustique.enable()", env=clean_env(RUSTIQUE_REPORT=value))
    assert ("enabled by rustique.enable()" in r.stderr) == (where == "stderr"), r.stderr


def test_report_text_file(tmp_path):
    path = tmp_path / "report.txt"
    for _ in range(2):
        r = py("import rustique; rustique.enable()", env=clean_env(RUSTIQUE_REPORT=f" {path} "))
        assert r.returncode == 0 and r.stderr == ""
    assert path.read_text().count("enabled by rustique.enable()") == 2  # appended


def test_no_report_without_enable():
    r = py("import rustique", env=clean_env(RUSTIQUE_REPORT="1"))
    assert r.stderr == ""
