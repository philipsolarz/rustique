"""Counters across processes (docs/atlas/w2-dx.md §3.1; accel/counters_file.rs): a forked
child starts from zero (keeping the inherited kernel state) and reports its parent; with
counters on and `RUSTIQUE_REPORT=<P>.json`, every process keeps its counters in the live file
`<P>.<pid>.cnt`, which survives `os._exit`. Also the credits-v2 reasons in `stats()`."""

from __future__ import annotations

import os
import struct
import subprocess
import sys
import textwrap

import pytest

from rustique import _rustique

A = _rustique._accel
HAS_FORK = hasattr(os, "fork")


@pytest.fixture(autouse=True)
def _cleanup():
    yield
    A.disable(["_selftest"])
    A.stats(False, True)


def test_stats_have_the_split_deopt_reasons():
    A.enable(["_selftest.meth"])
    A.stats(True, True)
    A._probe(1)
    st = A.stats()["_selftest.meth"]
    assert set(st["fallback"]) == {"args", "deps", "tools", "interp", "deopt", "error", "check", "continue", "abandon"}
    assert st["hits"] == 1 and "suspensions" in st


@pytest.mark.skipif(not HAS_FORK, reason="no fork")
def test_forked_child_starts_from_zero_and_names_its_parent():
    A.enable(["_selftest.meth"])
    A.stats(True, True)
    for _ in range(10):
        A._probe(1)
    assert A.status()["process"]["ppid"] is None
    r, w = os.pipe()
    pid = os.fork()
    if pid == 0:  # child
        try:
            before = A.stats()["_selftest.meth"]["hits"]
            for _ in range(3):
                A._probe(1)
            after = A.stats()["_selftest.meth"]["hits"]
            ppid = A.status()["process"]["ppid"]
            still = A._probe(1) == ("kernel", 1)  # inherited kernel state
            os.write(w, f"{before} {after} {ppid} {still}".encode())
        finally:
            os._exit(0)
    os.close(w)
    out = os.read(r, 200).decode().split()
    os.waitpid(pid, 0)
    assert out == ["0", "3", str(os.getpid()), "True"]
    assert A.stats()["_selftest.meth"]["hits"] == 10  # the parent's are untouched


def read_cnt(path: str) -> dict:
    with open(path, "rb") as f:
        data = f.read()
    magic, pid, ppid, n, names_off, counters_off, flags, row = struct.unpack_from("=8s7Q", data, 0)
    names = data[names_off:counters_off].split(b"\0")[:n]
    rows = {}
    for i, name in enumerate(names):
        rows[name.decode()] = struct.unpack_from(f"={row}Q", data, counters_off + i * row * 8)
    return {"magic": magic, "pid": pid, "ppid": ppid, "flags": flags, "row": row, "rows": rows}


@pytest.mark.skipif(sys.platform == "win32" or not HAS_FORK, reason="live counter file: Unix")
def test_live_counter_file_survives_os_exit_and_forks(tmp_path):
    report = tmp_path / "r.json"
    code = textwrap.dedent(
        """
        import os, sys
        from rustique import _rustique
        A = _rustique._accel
        A.switch_enable(only=["_selftest.meth"])
        for _ in range(7):
            A._probe(1)
        A._probe("x")
        pid = os.fork()
        if pid == 0:
            for _ in range(2):
                A._probe(1)
            os._exit(0)
        os.waitpid(pid, 0)
        print(os.getpid(), pid)
        os._exit(0)  # no exit hook, no JSON: the live file has everything
        """
    )
    e = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE") or k == "RUSTIQUE_VALIDATE"}
    e.update(RUSTIQUE_REPORT=str(report), RUSTIQUE_STATS="1")
    r = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, env=e, timeout=60)
    assert r.returncode == 0, r.stderr
    parent, child = (int(x) for x in r.stdout.split())
    p = read_cnt(f"{report}.{parent}.cnt")
    c = read_cnt(f"{report}.{child}.cnt")
    assert p["magic"] == b"RQCNT\0\0\x02" and p["row"] == 11 and p["pid"] == parent and p["ppid"] == 0
    assert p["flags"] & 1 == 0  # the JSON record was never written (os._exit)
    hits, args = p["rows"]["_selftest.meth"][0], p["rows"]["_selftest.meth"][1]
    assert (hits, args) == (7, 1)
    assert c["pid"] == child and c["ppid"] == parent
    assert c["rows"]["_selftest.meth"][0] == 2  # the child's own, from zero
