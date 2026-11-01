"""Cold-process startup: `python -c "import X"` for 20 popular packages plus bare `python`.

Stands for the fixed cost every CLI invocation, test run, pre-commit hook, serverless
cold start and `python -m tool` pays before doing any work: interpreter init, `site`, then
the import system (frozen `importlib._bootstrap*` finders/loaders, `os.stat`/`listdir`,
pyc unmarshalling) and module bodies (class creation, `enum`/`dataclass`/`typing`
machinery, module-level `re.compile`, decorators). Each `run()` spawns 21 fresh processes
of the running interpreter sequentially with warm OS page cache and existing `.pyc`
files (the steady state of a developer machine). Whole-process by nature: run() measures
children. `DEVTOOLS_CHILD_PRELUDE` (env) is prepended to every child's code, e.g.
`import rustique; rustique.enable(); ` for the accelerated variant or a profiler hook.
"""

import hashlib
import os
import subprocess
import sys

REQUIRES = [
    "requests==2.34.2", "django==6.1.2", "flask==3.1.3", "fastapi==0.162.0", "pandas==3.1.0",
    "sqlalchemy==2.1.6", "boto3==1.43.123", "rich==15.0.0", "click==8.5.0", "pydantic==2.14.2",
    "numpy==2.5.4", "httpx==0.28.1", "aiohttp==3.14.5", "jinja2==3.1.6", "pyyaml==6.0.3",
    "attrs==26.1.0", "typer==0.27.3", "pytest==9.2.0", "black==26.10.1", "pip==26.2.2",
]

IMPORTS = (
    "", "requests", "django.db.models", "flask", "fastapi", "pandas", "sqlalchemy.orm", "boto3",
    "rich.console", "click", "pydantic", "numpy", "httpx", "aiohttp", "jinja2", "yaml", "attrs",
    "typer", "pytest", "black", "pip._internal.cli.main",
)


def setup():
    env = dict(os.environ)
    return env


def run(state):
    prelude = state.get("DEVTOOLS_CHILD_PRELUDE", "")
    out = []
    for mod in IMPORTS:
        code = prelude + (f"import {mod}; " if mod else "") + (
            "import sys; print(len(sys.modules))")
        proc = subprocess.run([sys.executable, "-c", code], env=state, capture_output=True,
                              text=True, check=True)
        out.append((mod or "<bare>", int(proc.stdout.split()[-1])))
    return out


def digest(result):
    return hashlib.sha256(repr(result).encode()).hexdigest()
