"""pip's dependency resolver: three offline `pip install --dry-run --report` resolutions.

Stands for `pip install -r requirements.txt` / `pip lock` resolution, which every CI job,
Docker build and virtualenv refresh runs. The index is a local `--find-links` directory of
1030 small wheels generated in `setup()` from the real dependency metadata of 103 popular
distributions (data/pip_index.json: names, versions, Requires-Dist with markers and extras,
Requires-Python), ten versions per project. The newest release of 15 projects declares an
unsatisfiable dependency, so resolvelib must backtrack, as with real-world conflicts. Each
`run()` resolves a 26-package application, the same with 8 upper-bound pins (older
candidates), and a 9-package data/notebook-style stack.
pip runs in-process (`pip._internal.cli.main.main`) with `--no-index --ignore-installed`:
directory scan, wheel-filename and tag parsing, `packaging` version/specifier/marker
evaluation, METADATA reading from the zip files (email header parsing), resolvelib.
No network.
"""

import hashlib
import json
import os
import tempfile
import zipfile

REQUIRES = ["pip==26.2.2"]

ROOT_REQUIREMENTS = (
    "django", "flask", "fastapi", "requests", "boto3", "pandas", "sqlalchemy", "sphinx",
    "pytest", "black", "pylint", "mypy", "rich", "click", "pydantic", "httpx", "aiohttp",
    "jinja2", "matplotlib", "scipy", "networkx", "sqlglot", "coverage", "hypothesis",
    "typer", "isort",
)
PINNED = ROOT_REQUIREMENTS + ("urllib3<2.5", "numpy<2.5", "click<8.3", "attrs<26",
                               "pygments<2.20", "packaging<26", "idna<3.16", "certifi<2026.6")
DATA_STACK = ("pandas", "scipy", "matplotlib", "networkx", "sqlalchemy", "pydantic", "rich",
              "requests", "pyyaml")
# Their newest version depends on a project that does not exist.
# (typing-extensions is left out on purpose: breaking it sends resolvelib into minutes of
# backtracking, a real pip pathology but not a steady workload.)
BROKEN_NEWEST = ("urllib3", "certifi", "idna", "anyio", "attrs", "packaging", "pygments",
                 "markupsafe", "numpy", "click", "platformdirs", "six", "pyyaml", "starlette",
                 "sqlalchemy")
OLDER = 9


def _older_versions(version):
    parts = version.split(".")
    major = int("".join(ch for ch in parts[0] if ch.isdigit()) or 0)
    minor = int("".join(ch for ch in (parts[1] if len(parts) > 1 else "0") if ch.isdigit()) or 0)
    out = []
    for k in range(1, OLDER + 1):
        if minor - k >= 0:
            out.append(f"{major}.{minor - k}.0")
        else:
            out.append(f"{max(major - 1, 0)}.{9 - k}.{k}")
    return out


def _wheel(directory, name, version, info, broken):
    dist = name.replace("-", "_")
    fname = f"{dist}-{version}-py3-none-any.whl"
    lines = ["Metadata-Version: 2.1", f"Name: {name}", f"Version: {version}"]
    if info["requires_python"]:
        lines.append(f"Requires-Python: {info['requires_python']}")
    for extra in info["provides_extra"]:
        lines.append(f"Provides-Extra: {extra}")
    for req in info["requires_dist"]:
        lines.append(f"Requires-Dist: {req}")
    if broken:
        lines.append(f"Requires-Dist: {name}-native-ext>=1")  # no such project
    di = f"{dist}-{version}.dist-info"
    files = {
        f"{dist}/__init__.py": "",
        f"{di}/METADATA": "\n".join(lines) + "\n",
        f"{di}/WHEEL": "Wheel-Version: 1.0\nGenerator: devtools-bench\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
    }
    files[f"{di}/RECORD"] = "".join(f"{p},,\n" for p in files) + f"{di}/RECORD,,\n"
    with zipfile.ZipFile(os.path.join(directory, fname), "w") as z:
        for path, text in files.items():
            z.writestr(zipfile.ZipInfo(path, date_time=(2026, 1, 1, 0, 0, 0)), text)


def setup():
    from pip._internal.cli.main import main  # noqa: F401  (warm pip's imports)

    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, "data", "pip_index.json")) as f:
        index = json.load(f)
    root = tempfile.mkdtemp(prefix="devtools-pip-")
    links = os.path.join(root, "wheels")
    os.makedirs(links)
    for name, info in index.items():
        _wheel(links, name, info["version"], info, broken=name in BROKEN_NEWEST)
        for v in _older_versions(info["version"]):
            _wheel(links, name, v, info, broken=False)
    return root, links


def _resolve(root, links, requirements):
    import contextlib
    import io
    from pip._internal.cli.main import main

    report = os.path.join(root, "report.json")
    sink = io.StringIO()
    with contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
        code = main(["install", "--dry-run", "--ignore-installed", "--no-index", "--isolated",
                     "--disable-pip-version-check", "--no-cache-dir", "--quiet",
                     "--find-links", links, "--report", report, *requirements])
    if code:
        raise RuntimeError(sink.getvalue())
    with open(report) as f:
        data = json.load(f)
    return sorted(f"{i['metadata']['name']}=={i['metadata']['version']}" for i in data["install"])


def run(state):
    root, links = state
    return [_resolve(root, links, reqs) for reqs in (ROOT_REQUIREMENTS, PINNED, DATA_STACK)]


def digest(result):
    return hashlib.sha256(repr(result).encode()).hexdigest()
