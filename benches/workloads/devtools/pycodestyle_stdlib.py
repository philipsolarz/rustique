"""pycodestyle (PEP 8 checker, flake8's E/W codes) over 18 real standard-library modules.

Stands for style linting in pre-commit/CI/editors. pycodestyle is the token- and
regex-heavy linter: it runs `tokenize` over every file, rebuilds logical lines and applies
~70 regex/str checks per physical and logical line, plus one `ast` check. Inputs are the
running interpreter's own `Lib/` sources (fixed module list, read in `setup()`); all
default checks enabled except the default ignore list, max line length 79.
"""

import hashlib
import os
import sysconfig

REQUIRES = ["pycodestyle==2.15.0"]

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
COUNT = 18


def stdlib_sources(count=None):
    root = sysconfig.get_path("stdlib")
    out = []
    for name in STDLIB_SLICE[:count]:
        rel = name if name.endswith(".py") else name + ".py"
        with open(os.path.join(root, rel), encoding="utf-8") as f:
            out.append((rel, f.read()))
    return out


def setup():
    import pycodestyle

    files = [(rel, src.splitlines(keepends=True)) for rel, src in stdlib_sources(COUNT)]
    return pycodestyle, files


def run(state):
    pycodestyle, files = state

    class Report(pycodestyle.BaseReport):
        def __init__(self, options):
            super().__init__(options)
            self.found = []

        def error(self, line_number, offset, text, check):
            code = super().error(line_number, offset, text, check)
            if code:
                self.found.append(f"{self.filename}:{line_number}:{offset + 1}: {text}")
            return code

    style = pycodestyle.StyleGuide(quiet=True, reporter=Report)
    report = style.options.report
    for rel, lines in files:
        style.input_file(rel, lines=list(lines))
    return report.found


def digest(result):
    return hashlib.sha256("\n".join(result).encode()).hexdigest()
