"""``python -m rustique diff -- <command...>``: prove that nothing changed except speed.

Runs the command plain (``RUSTIQUE=0``: the kill switch also neutralises any
``rustique.enable()`` in the program) and enabled (``RUSTIQUE=1`` with counters on), with
the same ``PYTHONHASHSEED`` (0 unless set), and compares:

- the exit codes;
- stdout and stderr, after normalising what differs between any two runs of a program
  (memory addresses, pytest durations and temporary directories; ``--ignore REGEX`` adds
  patterns, ``--no-normalize`` turns this off);
- for pytest commands (``pytest …``, ``python -m pytest …``): every test's outcome, from a
  JUnit XML file added to the command line (``-p no:cacheprovider`` is added too, so the
  first run cannot steer the second through ``.pytest_cache``).

Output that varies between two *plain* runs of the same command (a test server's access
log, ephemeral ports, threads interleaving their output) proves nothing about Rustique: when
only stdout/stderr differ, ``diff`` runs the plain side once more and reports a stream that
also differs plain-to-plain as *volatile* instead of failing (``--strict-output`` turns this
off). Prints the differences (unified diff, first lines) or ``identical``, the timing of each
side, and the per-target report of the enabled run (which proves that something was
accelerated: an "identical" with no accelerated call proves nothing, and says so).
Exit status: 0 identical, 1 different, 2 usage error or Rustique inactive in the command.
``--repeat N`` runs N plain/enabled pairs (interleaved) for the timing; every run's
outputs are compared.
"""

import os
import re
import sys
import time

NORMALIZE = (
    (r"0x[0-9a-fA-F]{6,16}", "0x<addr>"),
    # wall-clock stamps in logs: ISO 8601 date-times and the common log format (a date alone
    # or a time alone is left alone: those are usually results)
    (r"\b\d{4}-\d\d-\d\d[ T]\d\d:\d\d:\d\d(?:[.,]\d+)?(?:Z|[+-]\d\d:?\d\d)?", "<datetime>"),
    (r"\b\d\d/[A-Z][a-z]{2}/\d{4}[: ]\d\d:\d\d:\d\d(?: [+-]\d{4})?", "<datetime>"),
    # ephemeral ports of local test servers
    (r"\b(127\.0\.0\.1|localhost|0\.0\.0\.0|\[::1\]):\d{4,5}\b", r"\1:<port>"),
    # pytest: "in 1.23s", "in 61.20s (0:01:01)", --durations lines, tmp_path directories
    (r"\bin \d+(\.\d+)?s( \(\d+:\d\d:\d\d\))?", "in <time>"),
    (r"^(\s*)\d+\.\d+s (call|setup|teardown) ", r"\1<time> \2 "),
    (r"pytest-of-[^/\\\s]+([/\\])pytest-\d+", r"pytest-of-<user>\1pytest-<n>"),
    (r"--randomly-seed=\d+", "--randomly-seed=<seed>"),
)
# pytest's progress characters (one per test) carry nothing the per-test outcomes do not, and
# threads (a test server's log) interleave with them differently from run to run: for pytest
# commands they are dropped before stdout is compared.
PYTEST_PROGRESS = ((r"^[.sFEXx]{3,}(?:\s*\[\s*\d+%\])?", ""), (r"\[\s*\d+%\]\s*$", ""))
PYTEST_CACHE_FLAGS = ("--lf", "--last-failed", "--ff", "--failed-first", "--nf", "--new-first", "--sw", "--stepwise",
                      "--sw-skip", "--stepwise-skip", "--cache-show", "--cache-clear")


def is_pytest(cmd: list[str]) -> bool:
    if not cmd:
        return False
    exe = os.path.basename(cmd[0]).lower().removesuffix(".exe")
    if exe in ("pytest", "py.test"):
        return True
    return "-m" in cmd[:-1] and cmd[cmd.index("-m") + 1] == "pytest" and exe.startswith(("python", "pypy"))


def pytest_args(cmd: list[str], xml: str, ids: str | None = None) -> list[str]:
    """The pytest command plus a JUnit file and (``ids``) the plugin's outcomes by node id."""
    extra = [f"--junitxml={xml}", "-o", "junit_family=xunit2"]
    if ids:
        extra += ["-p", "rustique.pytest_plugin", f"--rustique-outcomes={ids}"]
    if not any(a.split("=")[0] in PYTEST_CACHE_FLAGS for a in cmd):
        extra += ["-p", "no:cacheprovider"]
    return [*cmd, *extra]


def node_outcomes(path: str) -> dict[str, str] | None:
    """``{node id: outcome}`` written by the plugin's ``--rustique-outcomes``, or None."""
    import json

    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


# Options of the original pytest command that a re-run of single tests keeps (they select
# plugins, configuration and the root, not tests).
_RERUN_OPTS = ("-p", "-o", "-c", "-W", "--rootdir", "--confcutdir", "--import-mode", "--override-ini", "--config-file")


def rerun_command(cmd: list[str], nodeids: list[str]) -> list[str]:
    """The command's interpreter and pytest, its plugin/config options, and these tests."""
    exe = os.path.basename(cmd[0]).lower().removesuffix(".exe")
    if exe in ("pytest", "py.test"):
        head, rest = cmd[:1], cmd[1:]
    else:
        i = cmd.index("-m")
        head, rest = cmd[: i + 2], cmd[i + 2 :]
    keep: list[str] = []
    i = 0
    while i < len(rest):
        a = rest[i]
        if a in _RERUN_OPTS and i + 1 < len(rest):
            keep += rest[i : i + 2]
            i += 2
            continue
        if any(a.startswith(o + ("=" if o.startswith("--") else "")) and a != o for o in _RERUN_OPTS):
            keep.append(a)
        i += 1
    return [*head, *keep, *nodeids]


def test_differences(ta: dict[str, str], tb: dict[str, str]) -> tuple[list[str], list[str], list[str]]:
    """Test ids whose outcome changed, ids only in the plain run, ids only in the enabled run.
    Ids that differ only in digits (a parametrization with now() or a random value) are the
    same tests if, digits masked, ids and outcomes pair up one to one."""
    changed = sorted(k for k in ta.keys() & tb.keys() if ta[k] != tb[k])
    only_a, only_b = sorted(ta.keys() - tb.keys()), sorted(tb.keys() - ta.keys())
    if only_a and len(only_a) == len(only_b):
        mask = lambda k: re.sub(r"\d+", "#", k)  # noqa: E731
        if sorted((mask(k), ta[k]) for k in only_a) == sorted((mask(k), tb[k]) for k in only_b):
            only_a, only_b = [], []
    return changed, only_a, only_b


def outcomes(xml_path: str) -> dict[str, str]:
    """``{test id: passed|failed|error|skipped|xfailed}`` from a JUnit XML file."""
    import xml.etree.ElementTree as ET

    out: dict[str, str] = {}
    if not os.path.exists(xml_path):
        return out
    for case in ET.parse(xml_path).getroot().iter("testcase"):
        tid = f"{case.get('classname', '')}::{case.get('name', '')}"
        kinds = [c.tag for c in case]
        if "failure" in kinds:
            res = "failed"
        elif "error" in kinds:
            res = "error"
        elif "skipped" in kinds:
            sk = case.find("skipped")
            res = "xfailed" if sk is not None and (sk.get("type") or "").endswith("xfail") else "skipped"
        else:
            res = "passed"
        n, key = 2, tid
        while key in out:  # duplicate ids (e.g. same-named parametrizations)
            key = f"{tid}#{n}"
            n += 1
        out[key] = res
    return out


def normalize(text: str, patterns: list[tuple[str, str]]) -> str:
    for pat, rep in patterns:
        text = re.sub(pat, rep, text, flags=re.M)
    return text


def run_once(cmd: list[str], env: dict[str, str], timeout: float | None) -> dict:
    import subprocess

    t0 = time.perf_counter()
    p = subprocess.run(cmd, env=env, capture_output=True, timeout=timeout)
    return {"rc": p.returncode, "stdout": p.stdout.decode("utf-8", "replace"),
            "stderr": p.stderr.decode("utf-8", "replace"), "seconds": time.perf_counter() - t0}


def unified(a: str, b: str, name: str, max_lines: int) -> list[str]:
    import difflib

    lines = list(difflib.unified_diff(a.splitlines(), b.splitlines(), f"{name} (plain)", f"{name} (rustique)",
                                      lineterm="", n=2))
    if len(lines) > max_lines:
        lines = lines[:max_lines] + [f"... {len(lines) - max_lines} more diff lines"]
    return lines


def compare(plain: dict, fast: dict, patterns, max_lines: int, skip=(), volatile_tests=()) -> list[str]:
    """Human-readable differences between two runs (empty: identical); ``skip``: streams
    not compared; ``volatile_tests``: tests whose outcome varies without Rustique."""
    diffs = []
    rc_explained = False
    if "tests" in plain and volatile_tests and plain["rc"] in (0, 1) and fast["rc"] in (0, 1):
        # pytest exits 1 when a test fails: a flaky test alone explains a 0/1 difference
        rest = test_differences(plain["tests"], fast["tests"])
        rc_explained = not any(k not in volatile_tests for ks in rest for k in ks)
    if plain["rc"] != fast["rc"] and not rc_explained:
        diffs.append(f"exit code: plain {plain['rc']}, rustique {fast['rc']}")
    for stream in ("stdout", "stderr"):
        if stream in skip:
            continue
        a, b = normalize(plain[stream], patterns), normalize(fast[stream], patterns)
        if a != b:
            diffs.append(f"{stream} differs:")
            diffs += ["  " + ln for ln in unified(a, b, stream, max_lines)]
    if "tests" in plain:
        ta, tb = plain["tests"], fast["tests"]
        changed, only_a, only_b = test_differences(ta, tb)
        changed = [k for k in changed if k not in volatile_tests]
        only_a = [k for k in only_a if k not in volatile_tests]
        only_b = [k for k in only_b if k not in volatile_tests]
        if changed or only_a or only_b:
            diffs.append(f"test outcomes differ ({len(changed)} changed, {len(only_a)} only plain, "
                         f"{len(only_b)} only rustique):")
            for k in changed[:max_lines]:
                diffs.append(f"  {k}: plain {ta[k]}, rustique {tb[k]}")
            for k in only_a[:max_lines]:
                diffs.append(f"  {k}: only in the plain run ({ta[k]})")
            for k in only_b[:max_lines]:
                diffs.append(f"  {k}: only in the rustique run ({tb[k]})")
    return diffs


def confirm(cmd: list[str], runs: dict, base: dict, opts, tmp: str) -> dict[str, str]:
    """Re-run, alone, the tests whose outcome differs between the first plain and enabled runs,
    ``opts.confirm`` times on each side (interleaved). Returns the tests whose outcome varies
    without Rustique (flaky: ``{id: "plain a/b/c, rustique d/e/f"}``); the others stay
    differences. Needs the plugin's node ids (``--rustique-outcomes``)."""
    a, b = runs["plain"][0].get("tests") or {}, runs["rustique"][0].get("tests") or {}
    changed, only_a, only_b = test_differences(a, b)
    ids = [k for k in changed + only_a + only_b if k.split("::")[0].endswith(".py")]  # node ids, not JUnit ids
    if not ids or len(ids) > 50:
        return {}
    seen: dict[str, dict[str, list[str]]] = {k: {"plain": [a.get(k, "absent")], "rustique": [b.get(k, "absent")]}
                                             for k in ids}
    path = os.path.join(tmp, "confirm.json")
    xml = os.path.join(tmp, "confirm.xml")
    for _ in range(opts.confirm):
        for side in ("plain", "rustique"):
            env = dict(base, RUSTIQUE="0") if side == "plain" else dict(base, RUSTIQUE="1")
            if os.path.exists(path):
                os.remove(path)
            try:
                run_once(pytest_args(rerun_command(cmd, ids), xml, path), env, opts.timeout)
            except Exception:  # noqa: BLE001 (a re-run that cannot run confirms nothing)
                return {}
            got = node_outcomes(path) or {}
            for k in ids:
                seen[k][side].append(got.get(k, "absent"))
    return {k: f"plain {'/'.join(v['plain'])}, rustique {'/'.join(v['rustique'])}"
            for k, v in seen.items() if len(set(v["plain"])) > 1}


def main(argv: list[str]) -> int:
    import argparse
    import statistics
    import tempfile

    import rustique
    from rustique import _report
    from rustique._cli import NO_RECORDS, add_tiers, resolve_command, split_command

    opts_argv, cmd = split_command(argv)
    p = argparse.ArgumentParser(prog="python -m rustique diff",
                                description="Run a command plain and with RUSTIQUE=1; report any difference.")
    p.add_argument("--repeat", type=int, default=1, metavar="N", help="plain/enabled pairs to run (timing: medians)")
    p.add_argument("--ignore", action="append", default=[], metavar="REGEX",
                   help="mask matches of REGEX in stdout/stderr before comparing (repeatable)")
    p.add_argument("--no-normalize", action="store_true", help="compare outputs byte for byte")
    p.add_argument("--strict-output", action="store_true",
                   help="fail on any stdout/stderr difference, even in a stream that varies between plain runs")
    p.add_argument("--max-lines", type=int, default=40, help="diff lines to show per stream")
    p.add_argument("--timeout", type=float, default=None, help="seconds per run")
    p.add_argument("--keep", metavar="DIR", help="save every run's stdout/stderr/report in DIR")
    p.add_argument("--quiet", "-q", action="store_true", help="no per-target report")
    p.add_argument("--check", action="store_true",
                   help="also run check mode in the enabled run (RUSTIQUE_CHECK=1): mismatches count as differences")
    p.add_argument("--tiers", metavar="T,..", help="opt-in tiers to turn on in the enabled run too (RUSTIQUE_TIERS)")
    p.add_argument("--confirm", type=int, default=2, metavar="N",
                   help="pytest: re-run tests whose outcome differs N more times on each side, alone; a test whose "
                        "outcome also varies without Rustique is flaky, not a difference (default 2; 0: off)")
    opts = p.parse_args(opts_argv)
    if not cmd:
        p.error("missing command (python -m rustique diff -- <command...>)")
    if rustique._killed():
        sys.stderr.write("rustique diff: RUSTIQUE=0 (kill switch) is set; unset it to compare.\n")
        return 2
    cmd = resolve_command(cmd)
    if "--rustique" in cmd:
        sys.stderr.write("rustique diff: drop --rustique from the command: diff enables Rustique itself "
                         "(the plugin's summary would differ between the runs).\n")
        return 2
    patterns = [] if opts.no_normalize else list(NORMALIZE)
    patterns += [(rx, "<ignored>") for rx in opts.ignore]
    pytest = is_pytest(cmd)
    if pytest and not opts.no_normalize:
        patterns += list(PYTEST_PROGRESS)
    base = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    base.setdefault("PYTHONHASHSEED", "0")
    add_tiers(opts.tiers)
    # the selection and opt-ins apply to the enabled side (RUSTIQUE=0 turns them off in the plain one)
    for k in ("RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE", "RUSTIQUE_TIERS", "RUSTIQUE_UNVALIDATED_PATCH"):
        if (v := os.environ.get(k)) is not None:
            base[k] = v
    runs: dict[str, list[dict]] = {"plain": [], "rustique": []}
    with tempfile.TemporaryDirectory(prefix="rustique-diff-") as tmp:
        report = os.path.join(tmp, "report-{}.json")
        for i in range(opts.repeat):
            for side in ("plain", "rustique"):
                env = dict(base)
                if side == "plain":
                    env["RUSTIQUE"] = "0"
                else:
                    env.update(RUSTIQUE="1", RUSTIQUE_STATS="1", RUSTIQUE_REPORT=report.format(i))
                    if opts.check:
                        env["RUSTIQUE_CHECK"] = "1"
                # one path for every run: pytest prints it ("generated xml file: ...") at
                # some verbosities, so a per-side name would differ by construction
                xml = os.path.join(tmp, "junit.xml")
                ids = os.path.join(tmp, "outcomes.json")
                for f in (xml, ids):
                    if os.path.exists(f):
                        os.remove(f)
                full = pytest_args(cmd, xml, ids) if pytest else cmd
                try:
                    r = run_once(full, env, opts.timeout)
                except FileNotFoundError as exc:
                    sys.stderr.write(f"rustique diff: {exc}\n")
                    return 2
                if pytest:
                    nodes = node_outcomes(ids)
                    r["tests"] = nodes if nodes is not None else outcomes(xml)
                runs[side].append(r)
                if opts.keep:
                    os.makedirs(opts.keep, exist_ok=True)
                    for stream in ("stdout", "stderr"):
                        with open(os.path.join(opts.keep, f"{side}-{i}.{stream}"), "w") as f:
                            f.write(r[stream])
        flaky: dict[str, str] = {}
        if pytest and opts.confirm > 0:
            flaky = confirm(cmd, runs, base, opts, tmp)
        records = _report.collect(report.format(0))  # the first enabled run's processes
        if opts.keep and records:
            import json

            with open(os.path.join(opts.keep, "report.json"), "w") as f:
                f.write("".join(json.dumps(r, default=list) + "\n" for r in records))
    diffs: list[str] = []
    volatile: list[str] = []
    pairs = [(runs["plain"][i], runs["rustique"][i]) for i in range(opts.repeat)]
    output_only = all(not compare(dict(a, stdout="", stderr=""), dict(b, stdout="", stderr=""), [], 0,
                                  volatile_tests=flaky) for a, b in pairs)
    unstable = set()
    if flaky and output_only and not opts.strict_output:
        unstable = {"stdout", "stderr"}  # the flaky tests' failure reports
    elif output_only and not opts.strict_output and any(compare(a, b, patterns, 0) for a, b in pairs):
        # Only stdout/stderr differ: is that the program, or Rustique? Compare plain with plain
        # (the runs of --repeat, else one more plain run).
        plains = list(runs["plain"])
        if len(plains) < 2:
            env = dict(base, RUSTIQUE="0")
            xml = os.path.join(tempfile.mkdtemp(prefix="rustique-diff-"), "junit.xml")
            plains.append(run_once(pytest_args(cmd, xml) if pytest else cmd, env, opts.timeout))
        for stream in ("stdout", "stderr"):
            texts = {normalize(r[stream], patterns) for r in plains}
            if len(texts) > 1:
                unstable.add(stream)
    for i, (a, b) in enumerate(pairs):
        d = compare(a, b, patterns, opts.max_lines, skip=unstable, volatile_tests=flaky)
        if d:
            diffs += ([f"pair {i + 1}:"] if opts.repeat > 1 else []) + d
    for stream in sorted(unstable):
        if any(normalize(a[stream], patterns) != normalize(b[stream], patterns) for a, b in pairs):
            volatile.append(f"{stream} differs, but it also differs between two plain runs (volatile output: "
                            f"not compared; --strict-output to compare, --ignore REGEX to mask)")
    tp = statistics.median(r["seconds"] for r in runs["plain"])
    tr = statistics.median(r["seconds"] for r in runs["rustique"])
    first = runs["plain"][0]
    out = [f"rustique diff: {' '.join(cmd)}"]
    what = f"exit code {first['rc']}, stdout {len(first['stdout'].splitlines())} lines, " \
           f"stderr {len(first['stderr'].splitlines())} lines"
    if pytest:
        what += f", {len(first['tests'])} test outcomes"
    if diffs:
        out.append(f"  DIFFERENT ({what} in the plain run)")
        out += ["  " + d for d in diffs]
    else:
        out.append(f"  identical: {what}")
    out += ["  note: " + v for v in volatile]
    if flaky:
        out.append(f"  note: {len(flaky)} test(s) changed outcome, but also without Rustique when re-run "
                   "(flaky: not compared):")
        out += [f"    {k}: {v}" for k, v in sorted(flaky.items())[: opts.max_lines]]
    out.append(f"  time: plain {tp:.3f} s, rustique {tr:.3f} s, ratio {tr / tp if tp else float('nan'):.3f}"
               f"{f' (median of {opts.repeat})' if opts.repeat > 1 else ''}")
    rc = 1 if diffs else 0
    if not records or not any(r.get("status") for r in records):
        out.append("  " + NO_RECORDS.strip())
        rc = rc or 2
    else:
        rec = _report.merge(records)
        bad = _report.mismatches(rec)
        if bad:
            out.insert(2, f"  CHECK MISMATCHES in the rustique run: {', '.join(f'{k} ({v})' for k, v in bad.items())}")
            rc = 1
        hits = sum(c.get("hits", 0) for c in (rec.get("stats") or {}).values())
        if rec.get("counters") and not hits:
            out.append("  note: no accelerated call happened in the rustique run: an identical result proves little.")
        if not opts.quiet:
            out.append("")
            out.append(_report.render(rec, limit=15).rstrip())
    print("\n".join(out))
    return rc
