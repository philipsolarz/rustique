"""Validation case: plain Python method calls and attribute access (expect own bytecode with
specialized calls CALL_PY_EXACT_ARGS / CALL_BOUND_METHOD_EXACT_ARGS in plain mode)."""

REQUIRES = []


class Acc:
    def __init__(self):
        self.total = 0
        self.count = 0

    def add(self, v):
        self.total += v
        self.count += 1
        return self.total


def helper(a, b):
    return a + b if a < b else a - b


def setup():
    return list(range(400_000))


def run(xs):
    acc = Acc()
    s = 0
    for x in xs:
        s = helper(s & 0xFFFF, x)
        acc.add(x)
    return s + acc.total + acc.count


def digest(result):
    return str(result)
