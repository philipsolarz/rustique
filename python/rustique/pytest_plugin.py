"""pytest plugin: run a test session with Rustique enabled and show what it did.

Registered automatically (entry point ``pytest11: rustique``) and inactive unless asked::

    pytest --rustique                       # enable for the session, report in the summary
    pytest --rustique --rustique-only json,copy --rustique-exclude re
    pytest -p rustique.pytest_plugin --rustique   # when plugin autoloading is off

With ``--rustique`` the plugin enables Rustique (and its counters) before collection, so
the code under test is accelerated from its first import; at the end it prints the
per-target report (hits, fallbacks by reason, refusals) and **fails the session if a
selected target was refused unexpectedly**: refused for another reason than "this
interpreter is not one the target is validated for" (a fingerprint mismatch, a site
patched by someone else, a failed self-test). ``--rustique-allow-refused NAMES`` accepts
those too. Under ``pytest-xdist`` each worker enables Rustique and the controller adds up
their counters. ``RUSTIQUE=0`` (kill switch) wins: the session runs plain and the summary
says so.
"""

import pytest

# Refusal reasons that only say "not validated on this interpreter": expected, never fatal.
_EXPECTED = ("version", "not validated", "free-threaded", "debug build", "pre-release", "prerelease",
             "unsupported", "subinterpreter")


def pytest_addoption(parser: pytest.Parser) -> None:
    g = parser.getgroup("rustique", "Rustique (docs/USAGE.md)")
    g.addoption("--rustique", action="store_true", default=False,
                help="enable Rustique for the session and report what it accelerated")
    g.addoption("--rustique-only", metavar="G,..", default=None, help="enable only these groups or targets")
    g.addoption("--rustique-exclude", metavar="G,..", default=None, help="do not enable these groups or targets")
    g.addoption("--rustique-allow-refused", metavar="NAMES", default="",
                help="targets or groups whose refusal is accepted (comma-separated, or 'all')")
    g.addoption("--rustique-report-limit", type=int, default=20, metavar="N",
                help="installed targets shown in the summary (default 20)")


def expected_refusal(t: dict) -> bool:
    """Refused only because this interpreter is not one the target is validated for."""
    kind = t.get("refusal")  # a machine-readable category, when the framework provides one
    if kind is not None:
        return kind in ("version", "build")
    reason = (t.get("reason") or "").lower()
    return any(word in reason for word in _EXPECTED)


class _Session:
    def __init__(self, config: pytest.Config) -> None:
        self.config = config
        self.workers: list[dict] = []

    def unexpected(self, st: dict) -> list[dict]:
        allow = {n.strip() for n in self.config.getoption("rustique_allow_refused").split(",") if n.strip()}
        if "all" in allow:
            return []
        return [t for t in st.get("targets", []) if t["state"] == "refused" and not expected_refusal(t)
                and t["name"] not in allow and t["group"] not in allow]


def pytest_configure(config: pytest.Config) -> None:
    if not config.getoption("rustique"):
        return
    from rustique import _switch

    _switch.stats(enabled=True)
    _switch.enable(only=config.getoption("rustique_only"), exclude=config.getoption("rustique_exclude"),
                   _source="pytest --rustique")
    config.stash[_KEY] = _Session(config)


_KEY = pytest.StashKey[_Session]()


def _record() -> dict:
    from rustique import _switch

    return _switch.record()


@pytest.hookimpl(trylast=True)
def pytest_sessionfinish(session: pytest.Session, exitstatus: int) -> None:
    state = session.config.stash.get(_KEY, None)
    if state is None:
        return
    rec = _record()
    workeroutput = getattr(session.config, "workeroutput", None)
    if workeroutput is not None:  # an xdist worker: hand the counters to the controller
        import json

        workeroutput["rustique"] = json.dumps(rec, default=list)
        return
    bad = state.unexpected(rec["status"])
    if bad and session.exitstatus == 0:
        session.exitstatus = pytest.ExitCode.TESTS_FAILED


@pytest.hookimpl(optionalhook=True)
def pytest_testnodedown(node, error) -> None:  # pytest-xdist: a worker finished
    state = node.config.stash.get(_KEY, None)
    out = getattr(node, "workeroutput", {}) or {}
    if state is not None and "rustique" in out:
        import json

        state.workers.append(json.loads(out["rustique"]))


def pytest_terminal_summary(terminalreporter, exitstatus: int, config: pytest.Config) -> None:
    state = config.stash.get(_KEY, None)
    if state is None or getattr(config, "workeroutput", None) is not None:
        return
    from rustique import _report

    rec = _record()
    if state.workers:  # xdist: the controller ran no tests; its own record only adds statuses
        rec = _report.merge([rec, *state.workers])
        rec["processes"] = len(state.workers)
    tr = terminalreporter
    tr.write_sep("=", "rustique")
    for line in _report.render(rec, limit=config.getoption("rustique_report_limit")).rstrip().splitlines():
        tr.write_line(line)
    bad = state.unexpected(rec["status"])
    if bad:
        tr.write_line("")
        tr.write_line(f"rustique: {len(bad)} target(s) refused unexpectedly (session marked failed): "
                      + ", ".join(t["name"] for t in bad), red=True, bold=True)
        tr.write_line("  accept with --rustique-allow-refused=NAMES, or exclude with --rustique-exclude=NAMES")
