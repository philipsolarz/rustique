"""Sphinx HTML build of an autodoc API reference for 8 standard-library modules.

Stands for `sphinx-build -b html` on a project's API docs (the docs job in CI and on Read
the Docs): Sphinx imports the documented modules, autodoc introspects them (`inspect`
signatures, docstrings, `typing` annotations), docutils parses the reStructuredText
docstrings (with napoleon off: plain rST), the HTML writer and Jinja2 templates render the
pages, Pygments highlights code blocks, and the search index is built. The project
(conf.py + one page per module) is generated in `setup()` into a temp dir; each `run()` is
a fresh, clean build (`freshenv`, new output dir), as in CI. Documented modules come from
the running interpreter, so the output depends only on the Python version.
"""

import hashlib
import os
import re
import shutil
import tempfile

REQUIRES = ["sphinx==9.2.0", "docutils==0.23"]

MODULES = ("textwrap", "shlex", "fractions", "difflib", "pprint", "json", "argparse", "string")

CONF = """\
project = "stdlib-api"
extensions = ["sphinx.ext.autodoc", "sphinx.ext.viewcode"]
autodoc_default_options = {"members": True, "undoc-members": True, "show-inheritance": True}
html_theme = "alabaster"
html_last_updated_fmt = None
html_copy_source = False
suppress_warnings = ["autodoc", "ref", "docutils"]
"""

PAGE = """\
{title}
{underline}

.. automodule:: {mod}

Example
-------

.. code-block:: python

   import {mod}
   help({mod})
"""


def setup():
    import sphinx.application  # noqa: F401

    root = tempfile.mkdtemp(prefix="devtools-sphinx-")
    src = os.path.join(root, "src")
    os.makedirs(src)
    with open(os.path.join(src, "conf.py"), "w") as f:
        f.write(CONF)
    toc = "API\n===\n\n.. toctree::\n   :maxdepth: 1\n\n" + "".join(f"   {m}\n" for m in MODULES)
    with open(os.path.join(src, "index.rst"), "w") as f:
        f.write(toc)
    for mod in MODULES:
        title = f"``{mod}``"
        with open(os.path.join(src, mod + ".rst"), "w") as f:
            f.write(PAGE.format(title=title, underline="=" * len(title), mod=mod))
    return root


def run(state):
    import io
    from sphinx.application import Sphinx

    src = os.path.join(state, "src")
    out = os.path.join(state, "out")
    shutil.rmtree(out, ignore_errors=True)
    warnings = io.StringIO()
    app = Sphinx(src, src, os.path.join(out, "html"), os.path.join(out, "doctrees"), "html",
                 status=None, warning=warnings, freshenv=True, verbosity=0, parallel=0)
    app.build(force_all=True)
    pages = {}
    for mod in ("index", *MODULES):
        with open(os.path.join(out, "html", mod + ".html"), encoding="utf-8") as f:
            pages[mod] = f.read()
    return pages


def digest(result):
    h = hashlib.sha256()
    for name in sorted(result):
        # strip the per-build cache-busting query strings on static assets
        h.update(re.sub(r"\?v=[0-9a-f]+", "", result[name]).encode())
    return h.hexdigest()
