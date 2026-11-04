"""Validation case: cyclic garbage collection over a large live heap (expect runtime:gc)."""

import gc

REQUIRES = []


class Node:
    __slots__ = ("other", "payload")


def setup():
    # a large live graph that every full collection has to traverse
    return [[i, [i], {"k": i}] for i in range(400_000)]


def run(heap):
    for _ in range(3):
        a = Node()
        b = Node()
        a.other, b.other = b, a
        gc.collect()
    return len(heap)


def digest(result):
    return str(result)
