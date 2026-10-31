"""Import hook of the opt-in native tier: modules whose name matches a pattern are compiled
through :mod:`rustique._takeover._rewrite` (docs/TAKEOVER.md §5).

Rewritten bytecode is cached next to the normal one under its own optimisation tag
(``mod.cpython-315.opt-rsnative<N>.pyc``), so a module imported with and without the hook
never sees the other's bytecode, and the rewrite runs once per source change.
"""

import importlib.machinery
import importlib.util
import marshal
import _thread
import sys

from rustique._takeover import _rewrite

#: Optimisation tag of rewritten bytecode (bumped with the rewriter version).
CACHE_TAG = f"rsnative{_rewrite.VERSION}"

#: Never rewritten, whatever the patterns say: rustique, importlib and encodings packages.

_lock = _thread.allocate_lock()
_patterns: tuple[str, ...] = ()
_targets: dict | None = None  # None: rustique's types (see _rewrite.default_targets)
_finder: NativeFinder | None = None


def _match(name: str, pattern: str) -> bool:
    if not any(c in pattern for c in "*?["):
        return name == pattern
    import fnmatch  # imports `re`: only when a pattern needs it

    return fnmatch.fnmatchcase(name, pattern)


def matches(fullname: str) -> bool:
    if fullname.split(".", 1)[0] in ("rustique", "importlib", "encodings"):
        return False
    return any(_match(fullname, p) for p in _patterns)


class RewritingLoader(importlib.machinery.SourceFileLoader):
    """A source loader that applies the native-tier rewrite and binds its helpers."""

    def source_to_code(self, data, path, *, _optimize: int = -1) :  # type: ignore[override]
        return _rewrite.compile_rewritten(data, path, optimize=_optimize)

    def get_code(self, fullname: str) :
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

    def exec_module(self, module) -> None:
        for name, value in _rewrite.helpers(_targets).items():
            module.__dict__.setdefault(name, value)
        super().exec_module(module)


class NativeFinder:
    """Finds matching modules with the regular path finder and swaps in the rewriting
    loader for plain source files (extension modules, frozen and zip-imported modules are
    left as they are)."""

    def find_spec(self, fullname: str, path = None, target = None) :
        if not matches(fullname):
            return None
        spec = importlib.machinery.PathFinder.find_spec(fullname, path, target)
        if spec is None or type(spec.loader) is not importlib.machinery.SourceFileLoader:
            return spec
        spec.loader = RewritingLoader(spec.loader.name, spec.loader.path)
        if spec.origin:
            spec.cached = importlib.util.cache_from_source(spec.origin, optimization=CACHE_TAG)
        return spec


def install(patterns: tuple[str, ...], targets: dict | None = None) -> None:
    """Add module-name patterns (``fnmatch`` syntax) and make sure the finder is first on
    ``sys.meta_path``. ``targets`` overrides the builtin -> type mapping (e.g. the builtins
    themselves, to validate the rewrite in isolation)."""
    global _patterns, _finder, _targets
    with _lock:
        _patterns = tuple(dict.fromkeys(_patterns + tuple(patterns)))
        if targets is not None:
            _targets = dict(targets)
        if _finder is None:
            _finder = NativeFinder()
        if _finder in sys.meta_path:
            sys.meta_path.remove(_finder)
        sys.meta_path.insert(0, _finder)


def uninstall() -> None:
    global _patterns, _targets
    with _lock:
        _patterns = ()
        _targets = None
        if _finder is not None and _finder in sys.meta_path:
            sys.meta_path.remove(_finder)


def patterns() -> tuple[str, ...]:
    return _patterns
