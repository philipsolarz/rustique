"""Install an _rqeval variant at interpreter startup (imported from a .pth file; see
run_tests.sh). RQEVAL=copy:MOD | swapall:MOD | swap:MOD:OP[,OP] (OP may be FAM_FAMILY);
RQEVAL_DIR holds the builds. sys.path is restored afterwards so tests that inspect it see
what they would see plainly; RQEVAL_MARK=<file> appends this pid to <file> (proof of install).
"""
import os, sys

def _is_main_interpreter():
    try:
        import _interpreters
        return _interpreters.get_current()[0] == _interpreters.get_main()[0]
    except Exception:
        return True


_v = os.environ.get("RQEVAL")
# The extension refuses subinterpreters (ACCELERATION 3.11); swapped handlers are process-wide
# anyway, and the overrides fall back outside the main interpreter.
if _v and _is_main_interpreter():
    _saved = list(sys.path)
    _d = os.environ.get("RQEVAL_DIR", "/work/w2-interp/rqeval")
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    sys.path.insert(0, _d)
    import rqpatch as _rp
    _kind, _, _rest = _v.partition(":")
    if _kind == "copy":
        __import__(_rest).install()
    elif _kind == "noop":
        __import__(_rest)          # import and core_symbols() only: the harness's own footprint
        _rp.core_symbols()
    elif _kind == "swapall":
        _rp.swap_all(_d, _rest)
    elif _kind == "swap":
        _m, _ops = _rest.split(":")
        _names = []
        for _op in _ops.split(","):
            if _op.endswith("_FAMILY"):
                import _opcode_metadata as _om
                _names += [_op[:-7]] + list(_om._specializations[_op[:-7]])
            else:
                _names.append(_op)
        _rp.swap(_d, _m, _names)
    sys.path[:] = _saved
    _mark = os.environ.get("RQEVAL_MARK")
    if _mark:
        with open(_mark, "a") as _f:
            _f.write(f"{os.getpid()}\n")
