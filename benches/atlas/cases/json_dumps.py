"""Validation case: json.dumps of a big list (expect >95% of time under _json's encoder)."""

import hashlib
import json

REQUIRES = []


def setup():
    return [{"id": i, "name": f"item-{i}", "tags": ["a", "b", str(i % 7)], "price": i * 0.25,
             "ok": bool(i % 2), "none": None} for i in range(60_000)]


def run(data):
    return json.dumps(data)


def digest(result):
    return hashlib.sha256(result.encode()).hexdigest()[:16]
