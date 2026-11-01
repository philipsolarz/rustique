"""pyflakes linting 61 real standard-library modules (~99k lines) in one go.

Stands for the lint step every editor-on-save hook, pre-commit run and CI job performs
(pyflakes is also the core of flake8's F-codes): parse each file with `ast.parse`, then walk
the tree in Python to build scopes and report unused/undefined names. The inputs are the
running interpreter's own `Lib/` sources (fixed module list, read in `setup()`), so no data
is bundled and the input is identical for 3.15.1 and 3.15.1t.
"""

import hashlib
import os
import sysconfig

REQUIRES = ["pyflakes==4.1.0"]

# A fixed, diverse slice of CPython's Lib/ (shared by the devtools lint/format workloads).
STDLIB_SLICE = (
    "argparse ast asyncio/base_events.py calendar collections/__init__.py configparser csv "
    "dataclasses _pydatetime _pydecimal difflib email/message.py email/_header_value_parser.py "
    "enum fractions ftplib functools gettext glob gzip heapq http/client.py http/server.py "
    "inspect ipaddress json/encoder.py json/decoder.py logging/__init__.py mailbox os "
    "pathlib/_local.py pickle pickletools pprint pydoc random re/_parser.py re/_compiler.py "
    "shlex shutil smtplib statistics string/__init__.py subprocess tarfile tempfile textwrap "
    "threading tokenize traceback typing unittest/case.py urllib/parse.py urllib/request.py "
    "uuid xml/dom/minidom.py zipfile/__init__.py zoneinfo/_zoneinfo.py "
    "importlib/_bootstrap_external.py tomllib/_parser.py sysconfig/__init__.py"
).split()


def stdlib_sources(count=None):
    root = sysconfig.get_path("stdlib")
    out = []
    for name in STDLIB_SLICE[:count]:
        rel = name if name.endswith(".py") else name + ".py"
        with open(os.path.join(root, rel), encoding="utf-8") as f:
            out.append((rel, f.read()))
    return out


class _Collect:
    """A pyflakes reporter that keeps the messages instead of printing them."""

    def __init__(self):
        self.lines = []

    def unexpectedError(self, filename, msg):
        self.lines.append(f"{filename}: error: {msg}")

    def syntaxError(self, filename, msg, lineno, offset, text):
        self.lines.append(f"{filename}:{lineno}: syntax: {msg}")

    def flake(self, message):
        self.lines.append(str(message))


def setup():
    import pyflakes.api  # noqa: F401

    return stdlib_sources()


def run(state):
    import pyflakes.api

    reporter = _Collect()
    for rel, source in state:
        pyflakes.api.check(source, rel, reporter)
    return reporter.lines


def digest(result):
    return hashlib.sha256("\n".join(result).encode()).hexdigest()
