"""Alias of the pytest plugin, which lives in the top-level module ``_rustique_pytest`` so that
pytest sessions that do not pass ``--rustique`` never import this package (see that module).
``-p rustique.pytest_plugin`` resolves to the entry point of that name, or to this module."""

from _rustique_pytest import *  # noqa: F401,F403
