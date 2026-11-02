"""Record a CPython patch release in the fingerprint data (DECISIONS §10.5; docs/ACCELERATION.md
§3.2): the conformance suites run first, then every original's format-v3 fingerprint is
compared with the accepted ones (crates/rustique-py/src/accel/fingerprints.txt).

    python benches/accel/record_patch.py [--check] [--accept OWNER[:ID] ...] [--write]
        [--data crates/rustique-py/src/accel/fingerprints.txt] [--json out.json]

Run it with the interpreter to record, the current build installed (on the dev box:
`rbox <ws> run --py 3.15.0 -- python benches/accel/record_patch.py --check --write`, with
`CPYTHON_SRC` pointing at that release's sources). The process runs with
`RUSTIQUE_UNVALIDATED_PATCH=1` (the release is not recorded yet) and validates eagerly.

1. `--check`: CPython's own test modules of every group (tests/conformance/
   run_target_suites.py, the 27 modules of benches/floors/conformance_all.sh plus those of the wave-2 groups) with every
   target that installs on this release; any unexplained difference, crash or missing install
   aborts: nothing is recorded. `pytest tests/targets` runs too and is reported (it asserts
   installs, so on a release where some targets refuse it is informative only).
2. Every fingerprint the targets and their shared groups check (`_accel.fingerprint_records`):
   - v3 among the accepted ones of the minor: nothing to do;
   - no accepted entry yet for this (owner, original, minor), and the inline v2 fingerprint the
     target was written against matches: accepted (a newly recorded original);
   - otherwise the code changed: listed for review, and accepted only with `--accept OWNER`
     or `--accept OWNER:ID`, after a human checked that the kernel still covers the new code
     (the target refuses with category `fingerprint` on this release until then).
3. The release is added to the `patch` lines (when the check passed or was skipped with
   `--no-check`), and `--write` rewrites the data file. The build must be rebuilt to use it.

Also prints the targets installed on this release with the data as written (target T10).
"""

from __future__ import annotations

import argparse
import collections
import importlib
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
DATA = os.path.join(ROOT, "crates", "rustique-py", "src", "accel", "fingerprints.txt")
MODULES = (
    "test.test_list,test.test_tuple,test.test_sort,test.test_builtin,test.test_json,test.test_float,"
    "test.test_format,test.test_strtod,test.test_fstring,test.test_copy,test.test_copyreg,"
    "test.test_dataclasses,test.test_strptime,test.test_datetime,test.test_time,test.test_posixpath,"
    "test.test_genericpath,test.test_fnmatch,test.test_glob,test.test_pathlib,test.test_os,"
    "test.test_logging,test.test_random,test.test_textwrap,test.test_urlparse,test.test_urllib,"
    "test.test_statistics,test.test_set,test.test_pickle,test.test_enum,test.test_collections,"
    "test.test_contextlib,test.test_functools"
)
PROBE = r"""
import importlib, json, sys
from rustique import _rustique
A = _rustique._accel
A._set_validation("eager")
for _, _, m in A.targets():
    if m:
        try:
            importlib.import_module(m)
        except ImportError:
            pass
A.enable()
A.validate()
out = {t["name"]: [t["state"], t.get("refusal"), t.get("reason")] for t in A.status()["targets"]}
print(json.dumps(out))
"""


def env(extra: dict[str, str] | None = None) -> dict[str, str]:
    e = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    e.update({"RUSTIQUE_UNVALIDATED_PATCH": "1", "RUSTIQUE_VALIDATE": "eager"})
    e.update(extra or {})
    return e


def probe() -> dict[str, list]:
    r = subprocess.run([sys.executable, "-c", PROBE], capture_output=True, text=True, env=env(), cwd=ROOT)
    if r.returncode:
        sys.exit(f"probe failed:\n{r.stderr[-3000:]}")
    return json.loads(r.stdout.strip().splitlines()[-1])


def parse(path: str) -> tuple[list[str], list[str], dict[tuple[str, str, str], list[str]]]:
    header, patches, accepted = [], [], collections.OrderedDict()
    with open(path) as f:
        for line in f:
            body = line.split("#")[0].strip()
            if not body:
                if line.startswith("#") and not accepted and not patches:
                    header.append(line.rstrip("\n"))
                continue
            parts = body.split()
            if parts[0] == "patch":
                patches.extend(parts[1:])
            else:
                accepted.setdefault((parts[0], parts[1], parts[2]), []).extend(parts[3:])
    return header, patches, accepted


def write(path: str, header: list[str], patches: list[str], accepted: dict) -> None:
    def key(p: str) -> tuple:
        t = p.endswith("t")
        return tuple(int(x) for x in p.rstrip("t").split(".")) + (t,)

    lines = list(header) + [""]
    by_minor: dict[str, list[str]] = collections.defaultdict(list)
    for p in sorted(set(patches), key=key):
        by_minor[".".join(p.split(".")[:2])].append(p)
    for minor in sorted(by_minor, key=lambda m: tuple(int(x) for x in m.split("."))):
        lines.append(f"patch {' '.join(by_minor[minor])}")
    lines.append("")
    for (minor, owner, oid) in sorted(accepted, key=lambda k: (tuple(int(x) for x in k[0].split(".")), k[1], k[2])):
        hashes = sorted(set(accepted[(minor, owner, oid)]))
        lines.append(f"{minor} {owner} {oid} {' '.join(hashes)}")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="run the conformance suites first (required to record)")
    ap.add_argument("--no-check", action="store_true", help="record without the conformance run (not for releases)")
    ap.add_argument("--accept", action="append", default=[], help="OWNER or OWNER:ID whose changed code was reviewed")
    ap.add_argument("--data", default=DATA)
    ap.add_argument("--write", action="store_true")
    ap.add_argument("--json")
    a = ap.parse_args()

    os.environ.update({"RUSTIQUE_UNVALIDATED_PATCH": "1", "RUSTIQUE_VALIDATE": "eager"})
    from rustique import _rustique

    A = _rustique._accel
    recorded, running = A.patch_releases()
    minor = ".".join(running.rstrip("t").split(".")[:2])
    print(f"CPython {running} (recorded: {', '.join(recorded) or 'none'})")
    report: dict = {"release": running}

    states = probe()
    installs = sorted(n for n, (st, _, _) in states.items() if st == "installed")
    report["installed_before"] = len(installs)
    checked = False
    if a.check:
        groups = sorted({g for _, g, _ in A.targets() if not g.startswith("_")})
        cmd = [sys.executable, "tests/conformance/run_target_suites.py", "--targets", ",".join(groups),
               "--modules", MODULES, "--expect-installed", ",".join(installs), "--timeout", "3600",
               "--json", "/tmp/rq-record-conformance.json"]
        print("conformance:", len(installs), "targets installed,", len(MODULES.split(",")), "modules ...", flush=True)
        r = subprocess.run(cmd, env=env(), cwd=ROOT, capture_output=True, text=True)
        tail = "\n".join(r.stdout.splitlines()[-40:])
        report["conformance"] = {"ok": r.returncode == 0, "tail": tail}
        print(tail)
        if r.returncode:
            print("conformance FAILED: nothing recorded")
            if a.json:
                json.dump(report, open(a.json, "w"), indent=1)
            sys.exit(1)
        r = subprocess.run([sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", "tests/targets"],
                           env=env(), cwd=ROOT, capture_output=True, text=True)
        report["pytest_targets"] = r.stdout.strip().splitlines()[-1] if r.stdout.strip() else r.stderr[-500:]
        print("pytest tests/targets:", report["pytest_targets"])
        checked = True

    for _, _, m in A.targets():
        if m:
            try:
                importlib.import_module(m)
            except ImportError:
                pass
    header, patches, accepted = parse(a.data)
    added, review, notes = [], [], []
    for rec in A.fingerprint_records():
        if not rec["v3"]:
            notes.append(f"{rec['owner']}: {rec['id']}")
            continue
        k = (minor, rec["owner"], rec["id"])
        have = accepted.get(k)
        ok_review = rec["owner"] in a.accept or f"{rec['owner']}:{rec['id']}" in a.accept
        if have and rec["v3"] in have:
            continue
        if not have and rec["v2_expected"] is not None and rec["v2"] == rec["v2_expected"]:
            accepted.setdefault(k, []).append(rec["v3"])
            added.append(f"{rec['owner']} {rec['id']} {rec['v3']} (new; v2 confirms)")
        elif ok_review:
            accepted.setdefault(k, []).append(rec["v3"])
            added.append(f"{rec['owner']} {rec['id']} {rec['v3']} (accepted after review)")
        else:
            review.append(f"{rec['owner']} {rec['id']}: v3 {rec['v3']} not accepted "
                          f"(accepted {have}; v2 {rec['v2']} vs inline {rec['v2_expected']})")
    if checked or a.no_check:
        if running not in patches:
            patches.append(running)
    report.update(added=added, review=review, notes=notes, patches=patches)
    print(f"\n{len(added)} fingerprints added, {len(review)} to review")
    for x in added:
        print("  +", x)
    for x in review:
        print("  REVIEW", x)
    for x in notes:
        print("  note", x)
    if a.write:
        write(a.data, header, patches, accepted)
        print(f"wrote {a.data} (rebuild to use it)")
    refused = collections.Counter(f"{st}:{cat}" for st, cat, _ in states.values() if st != "installed")
    print(f"\ninstalled on {running} before this recording: {len(installs)}; others: {dict(refused)}")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(report, f, indent=1)


if __name__ == "__main__":
    main()
