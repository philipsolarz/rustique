"""black reformatting 4 real standard-library modules (~2.3k lines), default mode.

Stands for the auto-formatter every save hook / pre-commit run invokes: black tokenizes
with its own blib2to3 tokenizer (pytokens), parses into a lib2to3 concrete syntax tree,
runs its line-splitting passes and, as a safety check, re-parses source and output with
`ast.parse` and compares the trees (the default "safe" mode). On CPython 3.15 PyPI has no
mypyc-compiled black wheel (black ships `py3-none-any` only for cp315), so pure Python is
what real users run today; on 3.12-3.14 black usually runs mypyc-compiled.
"""

import hashlib
import os
import sysconfig

REQUIRES = ["black==26.10.1"]

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
# Four mid-sized modules from the slice: pure-Python black formats ~1.6k lines/s, so this
# keeps run() near 1.4 s.
PICK = ("gettext", "json/decoder.py", "pprint", "string/__init__.py")


def stdlib_sources(names):
    root = sysconfig.get_path("stdlib")
    out = []
    for name in names:
        rel = name if name.endswith(".py") else name + ".py"
        with open(os.path.join(root, rel), encoding="utf-8") as f:
            out.append((rel, f.read()))
    return out


def setup():
    import black

    return black, black.Mode(), stdlib_sources(PICK)


def run(state):
    black, mode, files = state
    out = []
    for rel, src in files:
        try:
            out.append(black.format_file_contents(src, fast=False, mode=mode))
        except black.NothingChanged:
            out.append(src)
    return out


def digest(result):
    h = hashlib.sha256()
    for text in result:
        h.update(text.encode())
    return h.hexdigest()
