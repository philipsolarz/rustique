"""Loader for the in-package layout: the extension at ``rustique/_rustique<suffix>``.

Release wheels ship the extension itself at the top level of site-packages as ``_rustique``
(``tools/repack_wheel.py`` moves it there and deletes this file). Wheels built from the
sdist and editable installs keep maturin's layout, with the extension inside the package;
there this module stands in for it, so that ``import _rustique`` (the ``.pth`` startup
line, ``import rustique``) works in every layout without importing the ``rustique``
package. The extension registers itself as ``_rustique`` and ``rustique._rustique`` when it
executes, which replaces this module in ``sys.modules``: the import returns the extension.
"""

import _imp
import os
import sys

if "rustique._rustique" in sys.modules:
    sys.modules[__name__] = sys.modules["rustique._rustique"]
else:
    _dir = os.path.join(os.path.dirname(__file__), "rustique")
    for _suffix in _imp.extension_suffixes():
        _path = os.path.join(_dir, "_rustique" + _suffix)
        if os.path.isfile(_path):
            _bootstrap = sys.modules["_frozen_importlib"]
            _bootstrap._load(sys.modules["_frozen_importlib_external"].spec_from_file_location("rustique._rustique", _path))
            break
    else:
        raise ImportError(f"rustique: no extension module in {_dir}", name=__name__)

# pytest marks every top-level module of a distribution that provides a pytest plugin for
# assertion rewriting, and warns about one that is already imported (here: by the startup
# line, under RUSTIQUE=1). The marker in the docstring is pytest's own opt-out; it lands on the
# extension, which holds this name now. Release wheels (the extension file itself) need none.
_ext = sys.modules[__name__]
if "PYTEST_DONT_REWRITE" not in (_ext.__doc__ or ""):
    _ext.__doc__ = "PYTEST_DONT_REWRITE"
