"""Validate a wheel on the platform it was built for (``wheels.yml`` job ``platform``;
docs/targets/dx.md §5). Standard library only; run it with any Python 3.10+.

    python .github/scripts/platform_tests.py --dist DIST --python PY --out DIR
                                             [--cpython-src DIR | --fetch-cpython]
                                             [--no-tests] [--no-conformance] [--name NAME]

Steps, each with PY (the interpreter the wheel targets):

1. **Install**: a fresh venv, ``pip install --no-index --find-links DIST rustique``, then pytest
   and hypothesis from the index.
2. **Coverage**: enable Rustique, import every target's module, record each target's state
   and refusal category in ``DIR/coverage-<name>-<tag>.json``. A refusal outside the categories
   that only say "not validated here" (``version``, ``build``, ``jit``, ``platform``,
   ``unvalidated-patch``) fails the job: an installed wheel must never meet a CPython it does
   not expect.
3. **Rustique's tests** for the groups installed here: ``tests/targets/test_<group>*.py`` of
   every group whose targets are all installed, run from a copy of ``tests/`` outside the
   source tree (so ``import rustique`` is the installed wheel), plus ``tests/test_dx*.py``
   (not on Windows: they drive POSIX signals and ``fork``).
4. **Conformance**: CPython's own test modules of every installed group, plain against
   enabled (``tests/conformance/run_target_suites.py``), from ``Lib/test`` of the
   interpreter's exact version (``--fetch-cpython`` downloads that source release). A target
   counts as "on" for a platform only once this passes there.

Exit status 0 when every step passed. ``platform_report.py`` turns the coverage files of every
job into the per-platform table and checks the floors in ``.github/platform-floors.json``.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
EXPECTED = ("version", "build", "jit", "platform", "unvalidated-patch")
# CPython test modules per group (tests/conformance/run_target_suites.py --modules). A group
# missing here runs Rustique's own tests only; workstreams add their groups.
CONFORMANCE = {
    "containers": ["test.test_list", "test.test_tuple", "test.test_sort"],
    "builtins": ["test.test_builtin", "test.test_sort"],
    "float": ["test.test_float", "test.test_format", "test.test_strtod", "test.test_fstring"],
    "json": ["test.test_json"],
    "copy": ["test.test_copy", "test.test_copyreg"],
    "dataclasses": ["test.test_dataclasses"],
    "datetime": ["test.test_strptime", "test.test_datetime", "test.test_time"],
    "posixpath": ["test.test_posixpath", "test.test_genericpath"],
    "fnmatch": ["test.test_fnmatch", "test.test_glob"],
    "pathlib": ["test.test_pathlib"],
    "logging": ["test.test_logging"],
    "random": ["test.test_random"],
    "textwrap": ["test.test_textwrap"],
    "urllib": ["test.test_urlparse", "test.test_urllib"],
    "statistics": ["test.test_statistics"],
    "enum": ["test.test_enum"],
    "re": ["test.test_re"],
    "set": ["test.test_set"],
    "functools": ["test.test_functools"],
    "contextlib": ["test.test_contextlib"],
    "generated": ["test.test_dataclasses", "test.test_collections"],
    "inspect": ["test.test_inspect"],
}
MODULES = ("json", "copy", "dataclasses", "datetime", "_strptime", "time", "posixpath", "fnmatch", "pathlib",
           "logging", "random", "textwrap", "urllib.parse", "statistics", "enum", "re", "functools", "contextlib",
           "inspect", "collections")

PROBE = r"""
import importlib, json, platform, sys, sysconfig
import rustique
rustique.enable()  # RUSTIQUE_VALIDATE=eager: every verdict now, not at each target's first call
for m in MODULES:
    try:
        importlib.import_module(m)
    except ImportError:
        pass
st = rustique.status()
proc = st["process"]
print(json.dumps({
    "python": platform.python_version() + ("t" if sysconfig.get_config_var("Py_GIL_DISABLED") else ""),
    "platform": sysconfig.get_platform(), "system": platform.system(), "machine": platform.machine(),
    "libc": " ".join(platform.libc_ver()).strip(), "rustique": proc.get("rustique"),
    "interpreter": proc.get("interpreter"),
    "targets": [{k: t.get(k) for k in ("name", "group", "state", "refusal", "reason", "tier", "validation")}
                for t in st["targets"]],
}))
"""


def venv_python(venv: str) -> str:
    for p in (os.path.join(venv, "bin", "python"), os.path.join(venv, "Scripts", "python.exe")):
        if os.path.exists(p):
            return p
    raise SystemExit(f"no interpreter in {venv}")


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    shown = [a if len(a) < 120 else a[:60].replace("\n", " ") + " ..." for a in cmd]
    print("+ " + " ".join(shown), flush=True)
    return subprocess.run(cmd, **kw)


def clean_env() -> dict[str, str]:
    """No RUSTIQUE* variable, except eager validation: a target's verdict (installed or refused)
    at enable/import, as the coverage and the expect-installed checks need, instead of at its
    first call (the default on GIL builds; tests/conftest.py sets the same)."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE") and k != "PYTHONPATH"}
    env["RUSTIQUE_VALIDATE"] = "eager"
    return env


def fetch_cpython(version: str, dest: str) -> str:
    """The source release of ``version`` (``3.15.1``; a ``t`` suffix is ignored) from python.org."""
    import tarfile
    import urllib.request

    v = version.rstrip("t")
    url = f"https://www.python.org/ftp/python/{v}/Python-{v}.tgz"
    path = os.path.join(dest, f"Python-{v}.tgz")
    print(f"fetching {url}", flush=True)
    urllib.request.urlretrieve(url, path)
    with tarfile.open(path) as tf:
        members = [m for m in tf.getmembers() if m.name.startswith(f"Python-{v}/Lib/")]
        tf.extractall(dest, members=members, filter="data")
    return os.path.join(dest, f"Python-{v}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dist", required=True, help="directory holding the wheel(s)")
    ap.add_argument("--python", required=True, help="the interpreter to validate (command or path)")
    ap.add_argument("--out", required=True, help="directory for coverage-*.json and conformance-*.json")
    ap.add_argument("--name", default="", help="platform name for the report (wheels.yml matrix name)")
    ap.add_argument("--cpython-src", default=os.environ.get("CPYTHON_SRC"))
    ap.add_argument("--fetch-cpython", action="store_true", help="download the interpreter's source release")
    ap.add_argument("--no-tests", action="store_true")
    ap.add_argument("--no-conformance", action="store_true")
    ap.add_argument("--timeout", type=int, default=2400, help="seconds per conformance module")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="rq-platform-")
    failures: list[str] = []

    # 1. install
    venv = os.path.join(work, "venv")
    if run([a.python, "-m", "venv", venv]).returncode:
        print("::error::venv failed")
        return 1
    py = venv_python(venv)
    pip = [py, "-m", "pip", "install", "--disable-pip-version-check", "-q"]
    if run([*pip, "--no-index", "--find-links", os.path.abspath(a.dist), "rustique"]).returncode:
        print("::error::pip install of the wheel failed")
        return 1
    have_pytest = run([*pip, "pytest", "hypothesis"]).returncode == 0

    # 2. coverage
    probe = run([py, "-c", f"MODULES = {MODULES!r}\n" + PROBE], capture_output=True, text=True, cwd=work,
                env=clean_env())
    if probe.returncode:
        print(probe.stderr)
        print("::error::coverage probe failed")
        return 1
    cov = json.loads(probe.stdout.strip().splitlines()[-1])
    cov["name"] = a.name
    tag = cov["python"]
    targets = cov["targets"]
    installed = [t for t in targets if t["state"] == "installed"]
    refused = [t for t in targets if t["state"] == "refused"]
    unexpected = [t for t in refused if t.get("refusal") not in EXPECTED]
    print(f"\n{a.name or cov['platform']} {tag}: {len(installed)} of {len(targets)} targets installed")
    for t in sorted(refused, key=lambda t: (t.get("refusal") or "", t["name"])):
        print(f"  refused [{t.get('refusal')}] {t['name']}: {t.get('reason')}")
    for t in unexpected:
        failures.append(f"unexpected refusal [{t.get('refusal')}] {t['name']}: {t.get('reason')}")
    groups: dict[str, list[dict]] = {}
    for t in targets:
        groups.setdefault(t["group"], []).append(t)
    full = sorted(g for g, ts in groups.items() if all(t["state"] == "installed" for t in ts))
    partial = sorted(g for g, ts in groups.items() if g not in full and any(t["state"] == "installed" for t in ts))
    cov["groups_full"], cov["groups_partial"] = full, partial
    with open(os.path.join(a.out, f"coverage-{a.name or cov['platform']}-{tag}.json"), "w") as f:
        json.dump(cov, f, indent=1)

    # 3. Rustique's own tests, from a copy of tests/ outside the source tree
    if not a.no_tests and have_pytest:
        tests = os.path.join(work, "tests")
        shutil.copytree(os.path.join(REPO, "tests"), tests, ignore=shutil.ignore_patterns("__pycache__"))
        # the switch tests read the shipped startup files and pyproject.toml (not importable:
        # `python/` is not on sys.path, so `import rustique` stays the installed wheel)
        os.makedirs(os.path.join(work, "python"), exist_ok=True)
        for f in ("_rustique_startup.pth", "_rustique_pytest.py"):
            shutil.copy(os.path.join(REPO, "python", f), os.path.join(work, "python", f))
        shutil.copy(os.path.join(REPO, "pyproject.toml"), os.path.join(work, "pyproject.toml"))
        files = sorted({f for g in full for f in glob.glob(os.path.join(tests, "targets", f"test_{g}*.py"))})
        if os.name != "nt":
            files += [os.path.join(tests, "test_dx.py"), os.path.join(tests, "test_dx_cli.py")]
        files = [f for f in files if os.path.exists(f)]
        if files:
            r = run([py, "-m", "pytest", "-q", "-p", "no:cacheprovider", "-x", "--rootdir", tests, *files],
                    cwd=work, env=dict(clean_env(), HYPOTHESIS_PROFILE="quick"))
            if r.returncode:
                failures.append(f"Rustique's tests failed (exit {r.returncode})")
    elif not a.no_tests:
        print("::warning::no pytest (offline?): Rustique's tests skipped")

    # 4. conformance of the installed targets
    if not a.no_conformance:
        src = a.cpython_src
        if not src and a.fetch_cpython:
            try:
                src = fetch_cpython(tag, work)
            except Exception as exc:  # noqa: BLE001
                failures.append(f"could not fetch the CPython {tag} sources: {exc}")
        if src:
            names = [t["name"] for t in installed]
            mods = list(dict.fromkeys(m for g in full + partial for m in CONFORMANCE.get(g, [])))
            if names and mods:
                out = os.path.join(a.out, f"conformance-{a.name or cov['platform']}-{tag}.json")
                r = run([py, os.path.join(REPO, "tests", "conformance", "run_target_suites.py"), "--cpython-src", src,
                         "--targets", ",".join(names), "--modules", ",".join(mods), "--expect-installed",
                         ",".join(names), "--timeout", str(a.timeout), "--json", out], cwd=work, env=clean_env())
                if r.returncode:
                    failures.append(f"conformance differs or a target was not installed (see {os.path.basename(out)})")
        elif not a.fetch_cpython:
            print("::warning::no CPython sources (--cpython-src / --fetch-cpython): conformance skipped")

    shutil.rmtree(work, ignore_errors=True)
    for msg in failures:
        print(f"::error::{a.name} {tag}: {msg}")
    print(f"\n{a.name} {tag}: {'all steps passed' if not failures else f'{len(failures)} failure(s)'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
