"""Validation case: a loop of dataclass constructions (expect the generated __init__ to own
the time: generated:dataclasses)."""

from dataclasses import dataclass

REQUIRES = []


@dataclass
class Point:
    x: int
    y: int
    z: int
    label: str
    weight: float = 1.0
    flag: bool = False


def setup():
    return list(range(300_000))


def run(xs):
    P = Point
    n = 0
    for i in xs:
        p = P(i, i, i, "p", 2.0, True)
        p = P(i, i, i, "q", 3.0, False)
        p = P(i, i, i, "r", 4.0, True)
        n += 1
    return n + p.x


def digest(result):
    return str(result)
