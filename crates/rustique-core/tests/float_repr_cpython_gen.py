"""Record CPython's `repr(float)` for the oracle test `float_repr_cpython.rs`.

Each category draws values from the same xorshift64* stream as the Rust test (or walks a
fixed sequence), and CPython's `repr` of every value is folded into one FNV-1a 64 hash of
`repr + "\\n"`. Only the hashes are embedded in the Rust file; a mismatch there points at
the category, and `--dump CATEGORY` prints the reprs to find the value.

    python crates/rustique-core/tests/float_repr_cpython_gen.py > /tmp/oracle.rs

and replace everything after `// BEGIN GENERATED` in `float_repr_cpython.rs` with it.
"""

import math
import struct
import sys

MASK = (1 << 64) - 1
FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3


class Rng:
    def __init__(self, seed):
        self.state = seed

    def next(self):
        s = self.state
        s ^= s >> 12
        s ^= (s << 25) & MASK
        s ^= s >> 27
        self.state = s
        return (s * 0x2545F4914F6CDD1D) & MASK


def from_bits(b):
    return struct.unpack("<d", struct.pack("<Q", b & MASK))[0]


def to_bits(x):
    return struct.unpack("<Q", struct.pack("<d", x))[0]


def pow2(e):
    """2**e for -1022 <= e <= 1023, built from bits like the Rust side."""
    return from_bits((e + 1023) << 52)


POW10 = {j: float(f"1e{j}") for j in range(-30, 31)}


def neighbours(x):
    b = to_bits(x)
    out = []
    for d in (-2, -1, 0, 1, 2):
        c = b + d
        if 0 <= c < 0x7FF0000000000000:
            out.append(from_bits(c))
    return out


def gen(kind, seed, n):
    r = Rng(seed)
    if kind == "bits":
        k = 0
        while k < n:
            x = from_bits(r.next())
            if math.isfinite(x):
                k += 1
                yield x
    elif kind == "subnormal":
        for _ in range(n):
            v = r.next()
            x = from_bits(v & ((1 << 52) - 1))
            yield -x if v >> 63 else x
    elif kind == "pow2":
        for e in range(-1074, 1024):
            x = math.ldexp(1.0, e)
            for y in neighbours(x):
                yield y
                yield -y
    elif kind == "pow10":
        for e in range(-325, 309):
            for y in neighbours(float(f"1e{e}")):
                yield y
                yield -y
    elif kind == "dyadic":
        for _ in range(n):
            m = (r.next() >> 11) >> (r.next() % 53)
            e = r.next() % 160 - 80
            x = m * pow2(e)
            yield -x if r.next() & 1 else x
    elif kind == "decimal":
        for _ in range(n):
            digits = 1 + r.next() % 17
            d = r.next() % (10**digits)
            e = r.next() % 660 - 340
            yield float(f"{d}e{e}")
    elif kind == "unit":
        for _ in range(n):
            u = (r.next() >> 11) * 2.0**-53
            j = r.next() % 61 - 30
            yield u * POW10[j]
    elif kind == "int":
        for _ in range(n):
            bits = 53 + r.next() % 12
            yield float(r.next() >> (64 - bits))
    elif kind == "cutoff":
        # Around the layout cut-offs 1e16 and 1e-4 / 1e-5, both signs.
        for base in (1e16, 1e-4, 1e-5, 1e15, 1e17, 1e-3, 1e-6):
            b = to_bits(base)
            for d in range(-n, n + 1):
                x = from_bits(b + d)
                yield x
                yield -x
    else:
        raise ValueError(kind)


CATEGORIES = [
    # (kind, seed, n)
    ("bits", 0x9E3779B97F4A7C15, 400_000),
    ("subnormal", 0xD1B54A32D192ED03, 50_000),
    ("pow2", 0, 0),
    ("pow10", 0, 0),
    ("dyadic", 0x94D049BB133111EB, 300_000),
    ("decimal", 0xBF58476D1CE4E5B9, 200_000),
    ("unit", 0x2545F4914F6CDD1D, 200_000),
    ("int", 0x632BE59BD9B4E019, 100_000),
    ("cutoff", 0, 2_000),
]


def digest(kind, seed, n):
    h = FNV_OFFSET
    count = 0
    for x in gen(kind, seed, n):
        for c in (repr(x) + "\n").encode():
            h = ((h ^ c) * FNV_PRIME) & MASK
        count += 1
    return h, count


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--dump":
        kind = sys.argv[2]
        for k, seed, n in CATEGORIES:
            if k == kind:
                for x in gen(k, seed, n):
                    print(f"{to_bits(x):016x} {x!r}")
        return
    print(f"// CPython {sys.version.split()[0]}: (kind, seed, n, values, FNV-1a 64 of repr + newline)")
    print("const ORACLE: &[(&str, u64, usize, usize, u64)] = &[")
    for kind, seed, n in CATEGORIES:
        h, count = digest(kind, seed, n)
        print(f'    ("{kind}", {seed:#x}, {n}, {count}, {h:#018x}),')
    print("];")


if __name__ == "__main__":
    main()
