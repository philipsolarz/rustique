"""Apply a regrtest variant at interpreter startup (imported from a .pth file; see
run_tests.sh). RQ_TIER_TEST=noop: import rustique and verify the binary; tier: enable the
interp tier too. Subinterpreters are left alone (the extension refuses to load there; the
swapped handlers fall back to CPython's code in them). sys.path is restored afterwards so
tests that inspect it see what they would see plainly; RQ_TIER_MARK=<file> appends this pid to
<file> when the variant was applied (proof of install)."""
import os
import sys


def _is_main_interpreter():
    try:
        import _interpreters
        return _interpreters.get_current()[0] == _interpreters.get_main()[0]
    except Exception:
        return True


_v = os.environ.get("RQ_TIER_TEST")
if _v and _is_main_interpreter():
    _saved = list(sys.path)
    _ok = False
    try:
        import rustique as _r
        _acc = _r._accel
        _info = _acc._t_interp.info()
        if _v == "tier" and _info["verified"]:
            _g = next(grp for name, grp, _ in _acc.targets() if name.startswith("interp."))
            _acc.enable(only=[_g], tiers=["interp"])
            _ok = any(t["state"] == "installed" for t in _acc.status()["targets"]
                      if t["name"].startswith("interp."))
        elif _v == "noop":
            _ok = True
    finally:
        sys.path[:] = _saved
    _mark = os.environ.get("RQ_TIER_MARK")
    if _mark and _ok:
        with open(_mark, "a") as _f:
            _f.write(f"{os.getpid()}\n")
