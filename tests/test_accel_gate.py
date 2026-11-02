"""The process gate (docs/ACCELERATION.md §3.2; DECISIONS §10.5, §10.8): the platform
allowlist, the patch-release gate with its `unvalidated-patch` category and explicit opt-in,
and fingerprint format v3 (docstrings out, exception tables canonical)."""

from __future__ import annotations

import os
import subprocess
import sys
import textwrap
import types

import pytest

from rustique import _rustique

A = _rustique._accel
pytestmark = pytest.mark.refused_ok


@pytest.fixture
def supported():
    """The interpreter itself is supported (only the patch release may be unrecorded)."""
    r = A.process_refusal()
    if r is not None and r[0] != "unvalidated-patch":
        pytest.skip(f"interpreter refused [{r[0]}]: {r[1]}")


def test_platform_and_patch_are_reported():
    p = A.status()["process"]
    assert p["platform"] and p["patch"]
    recorded, running = A.patch_releases()
    assert running == p["patch"]
    refusal = A.process_refusal()
    if refusal is None:
        assert running in recorded or os.environ.get("RUSTIQUE_UNVALIDATED_PATCH")
    else:
        assert refusal[0] in {"version", "platform", "build", "layout", "unvalidated-patch"}
        assert p["interpreter"] == refusal[1] and p["interpreter_refusal"] == refusal[0]


def test_unrecorded_patch_refuses_unless_opted_in(supported):
    recorded, _ = A.patch_releases()
    assert recorded, "the build carries recorded patch releases"
    some = recorded[0]
    assert A._patch_decision(some, False) == ("recorded", None)
    minor = ".".join(some.rstrip("t").split(".")[:2])
    unknown = f"{minor}.99" + ("t" if some.endswith("t") else "")
    ok, refusal = A._patch_decision(unknown, False)
    assert ok is None and refusal[0] == "unvalidated-patch"
    assert unknown in refusal[1] and "RUSTIQUE_UNVALIDATED_PATCH=1" in refusal[1]
    assert some in refusal[1] or "validated:" in refusal[1]
    assert A._patch_decision(unknown, True) == ("unvalidated", None)


def test_installed_targets_report_their_validation(supported):
    if A.process_refusal():
        pytest.skip("interpreter refused")
    A.enable(["_selftest.meth"])
    try:
        t = next(t for t in A.status()["targets"] if t["name"] == "_selftest.meth")
        assert t["validation"] == "recorded"
    finally:
        A.disable(["_selftest"])


def _f(src: str) -> types.FunctionType:
    ns: dict = {}
    exec(textwrap.dedent(src), ns)
    return ns["f"]


def test_fingerprint_v3_ignores_docstrings(supported):
    a = _f('''
        def f(x):
            "one docstring"
            return x + 1
    ''')
    b = _f('''
        def f(x):
            "another, longer docstring"
            return x + 1
    ''')
    c = _f('''
        def f(x):
            "one docstring"
            return x + 2
    ''')
    assert A.fingerprint3(a)[1] == A.fingerprint3(b)[1] != A.fingerprint3(c)[1]
    assert A.fingerprint(a) != A.fingerprint(b)  # v2 hashed the docstring


def _varint(v: int, start: bool = False) -> bytes:
    chunks = []
    while True:
        chunks.append(v & 63)
        v >>= 6
        if not v:
            break
    chunks.reverse()
    out = bytearray(c | 64 for c in chunks[:-1]) + bytes([chunks[-1]])
    if start:
        out[0] |= 128
    return bytes(out)


def _entries(table: bytes) -> list[tuple[int, int, int, int]]:
    out, i = [], 0

    def rd():
        nonlocal i
        b = table[i]
        i += 1
        v = b & 63
        while b & 64:
            b = table[i]
            i += 1
            v = (v << 6) | (b & 63)
        return v

    while i < len(table):
        start, size, target, dl = rd(), rd(), rd(), rd()
        out.append((start, size, target, dl))
    return out


def _encode(entries) -> bytes:
    return b"".join(_varint(s, True) + _varint(n) + _varint(t) + _varint(dl) for s, n, t, dl in entries)


def test_fingerprint_v3_canonical_exception_table(supported):
    f = _f('''
        def f(d, k):
            try:
                v = d[k]
                w = v + 1
            except KeyError:
                return None
            return w
    ''')
    code = f.__code__
    entries = _entries(code.co_exceptiontable)
    s, n, t, dl = max(entries, key=lambda e: e[1])
    assert n >= 2
    # The same per-instruction mapping, split into two entries: v3 equal, v2 differs.
    split = [e for e in entries if e != (s, n, t, dl)] + [(s, 1, t, dl), (s + 1, n - 1, t, dl)]
    split.sort()
    g = types.FunctionType(code.replace(co_exceptiontable=_encode(split)), {})
    assert A.fingerprint3(g)[1] == A.fingerprint3(f)[1]
    assert A.fingerprint(g) != A.fingerprint(f)
    # A different handler for the protected range: v3 differs.
    moved = [(a, b, c + 1 if (a, b, c, e) == (s, n, t, dl) else c, e) for a, b, c, e in entries]
    h = types.FunctionType(code.replace(co_exceptiontable=_encode(moved)), {})
    assert A.fingerprint3(h)[1] != A.fingerprint3(f)[1]


def test_unvalidated_opt_in_reaches_the_gate(supported):
    code = (
        "from rustique import _rustique as r; import json; "
        "print(r._accel.process_refusal() is None, r._accel.status()['process']['patch'])"
    )
    e = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    e["RUSTIQUE_UNVALIDATED_PATCH"] = "1"
    r = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, env=e, timeout=60)
    assert r.returncode == 0, r.stderr
    assert r.stdout.split()[0] == "True"
