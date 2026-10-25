"""Import hook of the opt-in native tier: modules whose name matches a pattern are compiled
through :mod:`rustique._takeover._rewrite` (docs/TAKEOVER.md §5).

Rewritten bytecode is cached next to the normal one under its own optimisation tag
(``mod.cpython-315.opt-rsnative<N>.pyc``), so a module imported with and without the hook
never sees the other's bytecode, and the rewrite runs once per source change.
"""

from __future__ import annotations

import fnmatch
import importlib.abc
import importlib.machinery
import importlib.util
import marshal
import sys
import threading
from typing import Any

from rustique._takeover import _rewrite

#: Optimisation tag of rewritten bytecode (bumped with the rewriter version).
CACHE_TAG = f"rsnative{_rewrite.VERSION}"

#: Never rewritten, whatever the patterns say.
_NEVER = ("rustique", "rustique.*", "importlib", "importlib.*", "encodings", "encodings.*")

_lock = threading.Lock()
_patterns: tuple[str, ...] = ()
_finder: NativeFinder | None = None


def matches(fullname: str) -> bool:
    if any(fnmatch.fnmatchcase(fullname, p) for p in _NEVER):
        return False
    return any(fnmatch.fnmatchcase(fullname, p) for p in _patterns)


class RewritingLoader(importlib.machinery.SourceFileLoader):
    """A source loader that applies the native-tier rewrite and binds its helpers."""

    def source_to_code(self, data: Any, path: Any, *, _optimize: int = -1) -> Any:  # type: ignore[override]
        return _rewrite.compile_rewritten(data, path, optimize=_optimize)

    def get_code(self, fullname: str) -> Any:
        from importlib import _bootstrap_external as bx  # private, present in 3.14/3.15

        source_path = self.get_filename(fullname)
        bytecode_path = importlib.util.cache_from_source(source_path, optimization=CACHE_TAG)
        st = self.path_stats(source_path)
        mtime, size = int(st["mtime"]), st["size"]
        try:
            data = self.get_data(bytecode_path)
            exc_details = {"name": fullname, "path": bytecode_path}
            flags = bx._classify_pyc(data, fullname, exc_details)
            if flags == 0:
                bx._validate_timestamp_pyc(data, mtime, size, fullname, exc_details)
                return marshal.loads(memoryview(data)[16:])
        except (OSError, ImportError, EOFError, ValueError, TypeError):
            pass
        source = self.get_data(source_path)
        code = self.source_to_code(source, source_path)
        if not sys.dont_write_bytecode:
            try:
                self._cache_bytecode(source_path, bytecode_path, bx._code_to_timestamp_pyc(code, mtime, size))
            except (OSError, NotImplementedError):
                pass
        return code

    def exec_module(self, module: Any) -> None:
        for name, value in _rewrite.helpers().items():
            module.__dict__.setdefault(name, value)
        super().exec_module(module)


class NativeFinder(importlib.abc.MetaPathFinder):
    """Finds matching modules with the regular path finder and swaps in the rewriting
    loader for plain source files (extension modules, frozen and zip-imported modules are
    left as they are)."""

    def find_spec(self, fullname: str, path: Any = None, target: Any = None) -> Any:
        if not matches(fullname):
            return None
        spec = importlib.machinery.PathFinder.find_spec(fullname, path, target)
        if spec is None or type(spec.loader) is not importlib.machinery.SourceFileLoader:
            return spec
        spec.loader = RewritingLoader(spec.loader.name, spec.loader.path)
        if spec.origin:
            spec.cached = importlib.util.cache_from_source(spec.origin, optimization=CACHE_TAG)
        return spec


def install(patterns: tuple[str, ...]) -> None:
    """Add module-name patterns (``fnmatch`` syntax) and make sure the finder is first on
    ``sys.meta_path``."""
    global _patterns, _finder
    with _lock:
        _patterns = tuple(dict.fromkeys(_patterns + tuple(patterns)))
        if _finder is None:
            _finder = NativeFinder()
        if _finder in sys.meta_path:
            sys.meta_path.remove(_finder)
        sys.meta_path.insert(0, _finder)
    importlib.invalidate_caches()


def uninstall() -> None:
    global _patterns
    with _lock:
        _patterns = ()
        if _finder is not None and _finder in sys.meta_path:
            sys.meta_path.remove(_finder)


def patterns() -> tuple[str, ...]:
    return _patterns
