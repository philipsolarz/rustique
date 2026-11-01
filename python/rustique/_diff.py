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

Prints the differences (unified diff, first lines) or ``identical``, the timing of each
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
    # pytest: "in 1.23s", "in 61.20s (0:01:01)", --durations lines, tmp_path directories
    (r"\bin \d+(\.\d+)?s( \(\d+:\d\d:\d\d\))?", "in <time>"),
    (r"^(\s*)\d+\.\d+s (call|setup|teardown) ", r"\1<time> \2 "),
    (r"pytest-of-[^/\\\s]+([/\\])pytest-\d+", r"pytest-of-<user>\1pytest-<n>"),
    (r"--randomly-seed=\d+", "--randomly-seed=<seed>"),
)
PYTEST_CACHE_FLAGS = ("--lf", "--last-failed", "--ff", "--failed-first", "--nf", "--new-first", "--sw", "--stepwise",
                      "--sw-skip", "--stepwise-skip", "--cache-show", "--cache-clear")


def is_pytest(cmd: list[str]) -> bool:
    if not cmd:
        return False
    exe = os.path.basename(cmd[0]).lower().removesuffix(".exe")
    if exe in ("pytest", "py.test"):
        return True
    return "-m" in cmd[:-1] and cmd[cmd.index("-m") + 1] == "pytest" and exe.startswith(("python", "pypy"))


def pytest_args(cmd: list[str], xml: str) -> list[str]:
    extra = [f"--junitxml={xml}", "-o", "junit_family=xunit2"]
    if not any(a.split("=")[0] in PYTEST_CACHE_FLAGS for a in cmd):
        extra += ["-p", "no:cacheprovider"]
    return [*cmd, *extra]


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


def compare(plain: dict, fast: dict, patterns, max_lines: int) -> list[str]:
    """Human-readable differences between two runs (empty: identical)."""
    diffs = []
    if plain["rc"] != fast["rc"]:
        diffs.append(f"exit code: plain {plain['rc']}, rustique {fast['rc']}")
    for stream in ("stdout", "stderr"):
        a, b = normalize(plain[stream], patterns), normalize(fast[stream], patterns)
        if a != b:
            diffs.append(f"{stream} differs:")
            diffs += ["  " + ln for ln in unified(a, b, stream, max_lines)]
    if "tests" in plain:
        ta, tb = plain["tests"], fast["tests"]
        changed = sorted(k for k in ta.keys() & tb.keys() if ta[k] != tb[k])
        only_a, only_b = sorted(ta.keys() - tb.keys()), sorted(tb.keys() - ta.keys())
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


def main(argv: list[str]) -> int:
    import argparse
    import statistics
    import tempfile

    from rustique import _report, _switch
    from rustique._cli import NO_RECORDS, read_records, resolve_command, split_command

    opts_argv, cmd = split_command(argv)
    p = argparse.ArgumentParser(prog="python -m rustique diff",
                                description="Run a command plain and with RUSTIQUE=1; report any difference.")
    p.add_argument("--repeat", type=int, default=1, metavar="N", help="plain/enabled pairs to run (timing: medians)")
    p.add_argument("--ignore", action="append", default=[], metavar="REGEX",
                   help="mask matches of REGEX in stdout/stderr before comparing (repeatable)")
    p.add_argument("--no-normalize", action="store_true", help="compare outputs byte for byte")
    p.add_argument("--max-lines", type=int, default=40, help="diff lines to show per stream")
    p.add_argument("--timeout", type=float, default=None, help="seconds per run")
    p.add_argument("--keep", metavar="DIR", help="save every run's stdout/stderr/report in DIR")
    p.add_argument("--quiet", "-q", action="store_true", help="no per-target report")
    opts = p.parse_args(opts_argv)
    if not cmd:
        p.error("missing command (python -m rustique diff -- <command...>)")
    if _switch.killed():
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
    base = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    base.setdefault("PYTHONHASHSEED", "0")
    for k in ("RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE"):  # the selection applies to the enabled side
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
                xml = os.path.join(tmp, f"{side}-{i}.xml")
                full = pytest_args(cmd, xml) if pytest else cmd
                try:
                    r = run_once(full, env, opts.timeout)
                except FileNotFoundError as exc:
                    sys.stderr.write(f"rustique diff: {exc}\n")
                    return 2
                if pytest:
                    r["tests"] = outcomes(xml)
                runs[side].append(r)
                if opts.keep:
                    os.makedirs(opts.keep, exist_ok=True)
                    for stream in ("stdout", "stderr"):
                        with open(os.path.join(opts.keep, f"{side}-{i}.{stream}"), "w") as f:
                            f.write(r[stream])
        records = read_records(report.format(0))  # the first enabled run's processes
        if opts.keep and records:
            import shutil

            shutil.copy(report.format(0), os.path.join(opts.keep, "report.json"))
    diffs: list[str] = []
    for i in range(opts.repeat):
        d = compare(runs["plain"][i], runs["rustique"][i], patterns, opts.max_lines)
        if d:
            diffs += ([f"pair {i + 1}:"] if opts.repeat > 1 else []) + d
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
    out.append(f"  time: plain {tp:.3f} s, rustique {tr:.3f} s, ratio {tr / tp if tp else float('nan'):.3f}"
               f"{f' (median of {opts.repeat})' if opts.repeat > 1 else ''}")
    rc = 1 if diffs else 0
    if not records:
        out.append("  " + NO_RECORDS.strip())
        rc = rc or 2
    else:
        rec = _report.merge(records)
        hits = sum(c.get("hits", 0) for c in (rec.get("stats") or {}).values())
        if rec.get("counters") and not hits:
            out.append("  note: no accelerated call happened in the rustique run: an identical result proves little.")
        if not opts.quiet:
            out.append("")
            out.append(_report.render(rec, limit=15).rstrip())
    print("\n".join(out))
    return rc
