"""Rustique's startup switch: ``RUSTIQUE=1 python app.py`` (docs/USAGE.md).

Importing this module runs the switch. It sits at the top of site-packages, *outside* the
``rustique`` package, next to ``_rustique_startup.pth``, whose one import line imports it
only when ``RUSTIQUE`` is in the environment::

    import os; 'RUSTIQUE' in os.environ and __import__('_rustique_startup')

Importing anything under ``rustique`` loads the extension module, so the decision whether
to do anything at all is taken here, with only ``os`` and ``sys`` (already imported by
``site``). No PEP 829 ``.start`` file yet: on 3.15.1 resolving one imports ``pkgutil`` and
``re`` (+72% on ``python -c pass``, docs/targets/dx.md §1). ``start`` is the entry point a
future ``_rustique_startup.start`` file will name; it is idempotent.

Values of ``RUSTIQUE``: ``1``/``true``/``yes``/``on`` enable at startup; empty and
``0``/``false``/``no``/``off`` do nothing here (``0`` also turns every later
``rustique.enable()`` into a no-op: the kill switch, see ``rustique._switch``). Like the
``PYTHON*`` variables, the ``RUSTIQUE*`` variables are ignored under ``-E`` and ``-I``.
"""

import os
import sys

_ON = ("1", "true", "yes", "on")
_OFF = ("", "0", "false", "no", "off")
_started = False


def start():
    """Run the startup switch once. Never raises: a broken install must not break Python."""
    global _started
    if _started:
        return
    _started = True
    if sys.flags.ignore_environment:
        return
    value = os.environ.get("RUSTIQUE")
    if value is None:
        return
    v = value.strip().lower()
    if v in _OFF:
        return
    if v not in _ON:
        sys.stderr.write(f"rustique: ignoring RUSTIQUE={value!r} (expected 1 or 0)\n")
        return
    try:
        from rustique import _switch
    except Exception as exc:  # e.g. no wheel for this interpreter, a partial install
        sys.stderr.write(f"rustique: RUSTIQUE={value} ignored: cannot import rustique: {exc}\n")
        return
    try:
        _switch.startup()
    except Exception as exc:  # never break the interpreter's startup
        sys.stderr.write(f"rustique: RUSTIQUE={value} ignored: {type(exc).__name__}: {exc}\n")


start()
