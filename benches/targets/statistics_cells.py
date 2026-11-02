"""Callgrind cells for the `statistics` group (benches/targets/utils_cg.py statistics_cells)."""

COMMON = """
import statistics as S
from decimal import Decimal
F10 = [x / 7 for x in range(10)]
F1000 = [x / 7 for x in range(1000)]
I10 = list(range(10))
I1000 = list(range(1000))
D10 = [Decimal(x) / 7 for x in range(10)]
"""

INSTALL = """
import rustique
rustique.enable(only=["statistics"])
"""

CELLS = {
    "mean(10 floats)": "S.mean(F10)",
    "mean(1000 floats)": "S.mean(F1000)",
    "mean(10 ints)": "S.mean(I10)",
    "mean(1000 ints)": "S.mean(I1000)",
    "variance(10 floats)": "S.variance(F10)",
    "pvariance(1000 floats)": "S.pvariance(F1000)",
    "stdev(10 floats)": "S.stdev(F10)",
    "stdev(1000 floats)": "S.stdev(F1000)",
    "pstdev(10 ints)": "S.pstdev(I10)",
    "variance(10 ints)": "S.variance(I10)",
    # fallbacks
    "mean(10 Decimals) [fallback]": "S.mean(D10)",
    "stdev(10 Decimals) [fallback]": "S.stdev(D10)",
    "mean(iterator) [fallback]": "S.mean(iter(F10))",
    "variance(floats, xbar) [fallback]": "S.variance(F10, 0.5)",
}

N = {"mean(1000 floats)": 20, "mean(1000 ints)": 20, "pvariance(1000 floats)": 20, "stdev(1000 floats)": 20}
