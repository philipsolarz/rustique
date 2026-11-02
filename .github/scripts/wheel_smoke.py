"""Smoke test of an installed rustique wheel: CI (`.github/workflows/wheels.yml`) and the
local proof in docs/targets/dx.md §5.

    python .github/scripts/wheel_smoke.py [--expect-installed] [--interp installable|refused]

Run it with the interpreter the wheel was installed into, from a directory that is not the
source tree (so `import rustique` finds the installed package). Standard library only.
Checks: import + enable + a battery of accelerated stdlib calls gives the same output as
plain CPython; RUSTIQUE=1 enables at startup through the shipped .pth, loading only the extension
(top-level `_rustique`, also registered as `rustique._rustique`); RUSTIQUE unset imports nothing;
RUSTIQUE=0 turns enable() into a no-op; `python -m rustique status` and `python -m rustique diff`
work. `--interp installable`: the opt-in `interp` tier is built in (its targets are `disabled`
with the opt-in reason, never refused); `--interp refused`: it refuses ("built without
handlers"). Exit status 0 when every check passes.
"""

import json
import os
import subprocess
import sys

BATTERY = r"""
import copy, datetime, json, posixpath
data = {"a": [3, 1, 2], "b": 1.5, "c": "xé", "d": None, "e": (1, 2)}
xs = sorted([5, 3, 9, 1, 7, 2])
print(xs, max(xs), min(xs), sum(xs), 3 in xs, xs.index(9), (1, 2, 2, 3).count(2), any(xs), all(xs))
print(json.dumps(data, sort_keys=True), json.loads('{"k": [1, 2.5, "s", null]}'))
print(copy.deepcopy(data) == data, copy.copy(xs) == xs, posixpath.join("a", "b", "c.txt"))
print(datetime.datetime.strptime("2026-11-01 12:30", "%Y-%m-%d %H:%M").isoformat(), repr(0.1 + 0.2), 1e22, 5e-324)
"""

ENV_VARS = ("RUSTIQUE", "RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE", "RUSTIQUE_STATS", "RUSTIQUE_REPORT")
failures: list[str] = []


def py(*args: str, **env: str) -> subprocess.CompletedProcess:
    e = {k: v for k, v in os.environ.items() if k not in ENV_VARS}
    e.update(env)
    return subprocess.run([sys.executable, *args], capture_output=True, text=True, env=e, timeout=300)


def check(name: str, ok: bool, detail: str = "") -> None:
    print(f"{'ok  ' if ok else 'FAIL'} {name}{': ' + detail if detail and not ok else ''}")
    if not ok:
        failures.append(name)


def main() -> int:
    expect_installed = "--expect-installed" in sys.argv
    repo = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    r = py("-c", "import rustique, os; print(os.path.dirname(rustique.__file__))")
    check("import rustique", r.returncode == 0, r.stderr)
    if r.returncode:
        return 1
    check("installed package, not the source tree", not r.stdout.strip().startswith(repo + os.sep), r.stdout)

    plain = py("-c", BATTERY)
    enabled = py("-c", "import rustique; st = rustique.enable()\n" + BATTERY)
    check("battery runs", plain.returncode == 0 and enabled.returncode == 0, plain.stderr + enabled.stderr)
    check("enable() leaves results unchanged", plain.stdout == enabled.stdout, f"{plain.stdout!r} != {enabled.stdout!r}")

    probe = ("import json, sys\nloaded = sorted(m for m in sys.modules if 'rustique' in m)\n"
             "out = {'loaded': loaded}\n"
             "ext = sys.modules.get('rustique._rustique')\n"
             "out['one_module'] = sys.modules.get('_rustique', ext) is ext\n"
             "if ext is not None:  # the startup path loads only the extension\n"
             "    st = ext._accel.status()\n"
             "    out['source'] = st['process']['switch']['source']\n"
             "    out['states'] = sorted({t['state'] for t in st['targets']})\n"
             "    out['installed'] = sum(t['state'] == 'installed' for t in st['targets'])\n"
             "print(json.dumps(out))\n")
    unset = json.loads(py("-c", probe).stdout)
    check("RUSTIQUE unset imports nothing at startup", unset["loaded"] == [], str(unset))
    zero = json.loads(py("-c", probe, RUSTIQUE="0").stdout)
    check("RUSTIQUE=0 imports nothing at startup", zero["loaded"] == [], str(zero))
    on = py("-c", probe, RUSTIQUE="1")
    on_out = json.loads(on.stdout) if on.returncode == 0 else {}
    # The extension only: top-level `_rustique` (repacked wheels; the shim of maturin-built ones)
    # registered as `rustique._rustique` too, one module object; never the `rustique` package.
    check("RUSTIQUE=1 enables at startup (.pth), loading only the extension",
          on_out.get("source") == "RUSTIQUE=1" and "rustique._rustique" in on_out.get("loaded", [])
          and set(on_out.get("loaded", [])) <= {"_rustique", "rustique._rustique"} and on_out.get("one_module"),
          on.stdout + on.stderr)
    if expect_installed:
        check("targets installed on this interpreter", on_out.get("installed", 0) > 0, str(on_out))
    startup_battery = py("-c", BATTERY, RUSTIQUE="1")
    check("RUSTIQUE=1 leaves results unchanged", startup_battery.stdout == plain.stdout, startup_battery.stdout)
    killed = py("-c", "import rustique; st = rustique.enable(); "
                      "print(st['process']['switch']['killed'], sum(t['state'] == 'installed' for t in st['targets']))",
                RUSTIQUE="0")
    check("RUSTIQUE=0 turns enable() into a no-op", killed.stdout.split() == ["True", "0"], killed.stdout + killed.stderr)

    # Every refusal on this platform, with its category (informational: the plugin check below
    # fails on categories other than version/build/jit/platform).
    refusals = py("-c", "import json, sys, importlib, rustique\n"
                        "rustique.enable()\n"
                        "for m in ('json', 'copy', 'dataclasses', 'datetime', '_strptime', 'posixpath', 'fnmatch',\n"
                        "          'pathlib', 'logging', 'random', 'textwrap', 'urllib.parse', 'statistics'):\n"
                        "    importlib.import_module(m)\n"
                        "for t in rustique.status()['targets']:\n"
                        "    if t['state'] == 'refused':\n"
                        "        print(f\"{t['name']:32} [{t.get('refusal')}] {t.get('reason')}\")\n")
    print("     refused here:" + ("" if refusals.stdout.strip() else " none"))
    print("\n".join("       " + ln for ln in refusals.stdout.splitlines()))

    interp = sys.argv[sys.argv.index("--interp") + 1] if "--interp" in sys.argv else None
    if interp:
        tier = py("-c", "import json, rustique\nrustique.enable()\n"
                        "print(json.dumps([t for t in rustique.status()['targets'] if t.get('tier') == 'interp']))")
        ts = json.loads(tier.stdout) if tier.returncode == 0 else []
        if interp == "installable":
            ok = bool(ts) and all(t["state"] == "disabled" and "opt-in" in (t.get("reason") or "") for t in ts)
        else:
            ok = bool(ts) and all(t["state"] == "refused" and "built without handlers" in (t.get("reason") or "")
                                  for t in ts)
        check(f"interp tier {interp}", ok, tier.stdout[:800] + tier.stderr[-800:])

    status = py("-m", "rustique", "status")
    check("python -m rustique status", status.returncode == 0 and "targets in" in status.stdout, status.stderr)
    print("\n".join("     " + ln for ln in status.stdout.splitlines()[:6]))
    diff = py("-m", "rustique", "diff", "--quiet", "--", sys.executable, "-c", BATTERY)
    check("python -m rustique diff (identical)", diff.returncode == 0, diff.stdout + diff.stderr)

    if py("-c", "import pytest").returncode == 0:
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, "test_smoke.py"), "w") as f:
                f.write("import json\n\ndef test_json():\n    assert json.loads(json.dumps([1, 2.5])) == [1, 2.5]\n")
            plain_t = subprocess.run([sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", d],
                                     capture_output=True, text=True, cwd=d)
            on_t = subprocess.run([sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", d],
                                  capture_output=True, text=True, cwd=d, env={**os.environ, "RUSTIQUE": "1"})
            plug = subprocess.run([sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", "--rustique", d],
                                  capture_output=True, text=True, cwd=d)
        check("RUSTIQUE=1 pytest: same result, no warning", plain_t.returncode == on_t.returncode == 0
              and "warning" not in on_t.stdout, on_t.stdout[-800:])
        check("pytest --rustique (entry point plugin)", plug.returncode == 0 and "enabled by pytest --rustique" in plug.stdout,
              plug.stdout[-800:])

    print(f"\n{len(failures)} failure(s)" if failures else "\nall checks passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
