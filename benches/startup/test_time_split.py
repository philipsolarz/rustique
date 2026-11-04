"""Split a `pytest --durations=0` log into test time of the opt-in native types vs the engine
(docs/atlas/w2-startup.md §5).

    python benches/startup/test_time_split.py pytest.log
"""
import collections
import re
import sys

NATIVE = {
    "test_buffer", "test_capi", "test_dict", "test_dict_fast_paths", "test_dict_threads", "test_differential",
    "test_expr", "test_frozenset", "test_hardening", "test_hardening_dictset", "test_interop", "test_list_core",
    "test_list_parity", "test_list_slots", "test_objsort", "test_scalars", "test_set", "test_threads", "test_tuple",
    "test_typed", "test_typing", "test_takeover_rewrite", "test_takeover_compat", "test_takeover_conformance",
    "test_regressions",
}
LINE = re.compile(r"^([\d.]+)s (call|setup|teardown)\s+(\S+?)::")


def main():
    per_file = collections.Counter()
    counts = collections.Counter()
    for line in open(sys.argv[1]):
        m = LINE.match(line)
        if m:
            per_file[m.group(3)] += float(m.group(1))
            if m.group(2) == "call":
                counts[m.group(3)] += 1
    split = collections.Counter()
    tests = collections.Counter()
    for f, t in per_file.items():
        name = f.rsplit("/", 1)[-1].removesuffix(".py")
        kind = "native" if f.startswith("tests/test_") and name in NATIVE else "engine"
        split[kind] += t
        tests[kind] += counts[f]
    total = sum(split.values())
    for k in ("native", "engine"):
        print(f"{k:<7} {split[k]:8.1f} s ({100 * split[k] / total:.0f}%)  {tests[k]} tests")
    for f, t in per_file.most_common(15):
        print(f"  {t:7.1f} s  {f}")


if __name__ == "__main__":
    main()
