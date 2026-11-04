"""The `gc` tier's forwarder against CPython's `invoke_gc_callback` (docs/targets/gc.md §3).

With the tier, `gcstate->callbacks` points at a private `[observer]` list and the observer
calls the program's list itself. Every program here runs twice in fresh interpreters,
plain and with the tier (and once more with the policy kept armed while the program tunes
the GC, `step_aside=False`, so that the policy and the forwarder run together), and the
logs must be identical: which callbacks run, in which order, with which phase string and
info dict (identity shared across the callbacks of one invocation, mutations visible to
the later ones), what happens when a callback mutates the list during the loop, raises, is
not callable, or calls `gc.collect()`, and what `sys.unraisablehook` receives.

Collections are explicit (`gc.collect(g)`), and automatic collection is disabled, so the
logs do not depend on collection timing.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import textwrap

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

rustique = pytest.importorskip("rustique")

SKIP = 77
EXPECTED = ("version", "build", "jit", "platform", "symbol")

HEAD = """
import gc, sys, json
LOG = []
def log(*a): LOG.append(a)
def unraisable(u):
    tb = []
    t = u.exc_traceback
    while t is not None:
        tb.append(t.tb_frame.f_code.co_name)
        t = t.tb_next
    log("unraisable", u.err_msg, repr(u.object), u.exc_type.__name__, str(u.exc_value), tb)
sys.unraisablehook = unraisable
gc.disable()
gc.collect()
"""

TIER = """
import rustique
ONLY = __import__("os").environ.get("RQ_GC_ONLY", "gc").split(",")
T = rustique._accel._t_gc
if {armed}:
    T.set_params(step_aside=False)
rustique.enable(only=ONLY)
_st = {{t["name"]: t for t in rustique.status()["targets"]}}["gc.adaptive"]
if _st["state"] != "installed":
    print(json.dumps({{"refused": _st.get("refusal"), "reason": _st["reason"]}}))
    sys.exit(77)
"""

TAIL = """
gc.collect()
print(json.dumps(LOG, default=repr))
"""


def _env(tier: bool) -> dict:
    env = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    if tier:
        env["RUSTIQUE_TIERS"] = os.environ.get("RQ_GC_TIERS", "gc")
    return env


def _run(body: str, mode: str) -> str:
    """mode: plain | tier | armed (tier, policy kept armed through gc.disable())."""
    tier = mode != "plain"
    pre = TIER.format(armed=mode == "armed") if tier else ""
    # The tier is enabled before the program disables automatic collection (as a launch
    # switch would be).
    src = pre + HEAD + textwrap.dedent(body) + TAIL
    r = subprocess.run([sys.executable, "-c", src], capture_output=True, text=True, env=_env(tier), timeout=300)
    if r.returncode == SKIP:
        import json

        info = json.loads(r.stdout.strip().splitlines()[-1])
        if info["refused"] in EXPECTED:
            pytest.skip(f"gc tier refused here: {info['reason']}")
        pytest.fail(f"gc tier refused unexpectedly: {info}")
    assert r.returncode == 0, r.stderr[-3000:]
    # object addresses differ between processes
    return re.sub(r"0x[0-9a-f]+", "0x?", r.stdout + r.stderr)


def same_everywhere(body: str) -> str:
    plain = _run(body, "plain")
    assert _run(body, "tier") == plain
    assert _run(body, "armed") == plain
    return plain


CYCLES = """
class C:
    def __init__(self): self.me = self
def garbage(n):
    for _ in range(n):
        C()
"""


def test_two_callbacks_every_generation():
    out = same_everywhere(CYCLES + textwrap.dedent("""
        def a(p, i): log("a", p, i["generation"], i["collected"], i["uncollectable"], sorted(i))
        def b(p, i): log("b", p, i["generation"], i["collected"])
        gc.callbacks.append(a); gc.callbacks.append(b)
        for g in (0, 1, 2, 2):
            garbage(50)
            gc.collect(g)
    """))
    assert '"a", "start", 0' in out and '"b", "stop", 2' in out


def test_info_and_phase_are_shared_across_one_invocation():
    same_everywhere("""
        seen = {}
        def a(p, i):
            i["mark"] = len(LOG)
            seen["phase"], seen["info"] = p, i
        def b(p, i):
            log("b", p, i.get("mark"), p is seen["phase"], i is seen["info"], type(p).__name__)
        gc.callbacks.extend([a, b])
        gc.collect(0); gc.collect(2)
    """)


def test_mutation_during_the_loop():
    same_everywhere("""
        def late(p, i): log("late", p)
        def adder(p, i):
            log("adder", p)
            if p == "start" and late not in gc.callbacks:
                gc.callbacks.append(late)      # called in this same loop
        def remover(p, i):
            log("remover", p)
            gc.callbacks.remove(remover)       # the next entry shifts into this index
        def after(p, i): log("after", p)
        def front(p, i):
            log("front", p)
            if p == "stop" and len(LOG) < 40:
                gc.callbacks.insert(0, lambda p, i: log("inserted", p))  # by-index loop
        gc.callbacks.extend([adder, remover, after, front])
        gc.collect(0); gc.collect(1); gc.collect(2)
        def clearer(p, i):
            log("clearer", p)
            gc.callbacks.clear()
        gc.callbacks[:] = [clearer, after]
        gc.collect(0); gc.collect(0)
    """)


def test_raising_and_uncallable_callbacks_are_reported_and_skipped():
    out = same_everywhere("""
        class Boom(Exception): pass
        def bad(p, i):
            raise Boom(p)
        def good(p, i): log("good", p)
        gc.callbacks.extend([bad, 42, good])
        gc.collect(0)
        gc.callbacks[:] = []
        gc.collect(2)
    """)
    assert "Exception ignored while calling GC callback" in out


def test_rebinding_the_module_attribute_changes_nothing():
    same_everywhere("""
        def a(p, i): log("a", p)
        def x(p, i): log("x", p)
        gc.callbacks.append(a)
        orig = gc.callbacks
        gc.callbacks = [x]          # CPython keeps calling its own list
        gc.collect(0)
        gc.callbacks = orig
        gc.collect(0)
    """)


def test_collect_and_set_threshold_inside_a_callback():
    same_everywhere(CYCLES + textwrap.dedent("""
        def inner(p, i):
            garbage(5)
            log("inner", p, gc.collect())   # a collection is running: returns 0
            log("thr", gc.get_threshold(), gc.isenabled())
        gc.callbacks.append(inner)
        gc.collect(1)
    """))


def test_callbacks_identity_length_and_refcount():
    same_everywhere("""
        cb = gc.callbacks
        log(len(cb), sys.getrefcount(cb), len(gc.get_referrers(cb)), type(cb).__name__)
        def a(p, i): pass
        cb.append(a)
        log(len(gc.callbacks), gc.callbacks is cb, gc.callbacks[0] is a)
        log(sum(1 for o in gc.get_objects() if type(o) is list and len(o) == 1 and type(o[0]).__name__ == "builtin_function_or_method"))
    """)


# ---------------------------------------------------------------------------------------
# Hypothesis: random programs of list mutations, callbacks with side effects, collections
# ---------------------------------------------------------------------------------------

BEHAVIOURS = ["record", "raise", "append_other", "remove_self", "mark_info", "clear", "insert_front"]

ops = st.lists(
    st.one_of(
        st.tuples(st.just("append"), st.integers(0, 5)),
        st.tuples(st.just("insert0"), st.integers(0, 5)),
        st.tuples(st.just("remove"), st.integers(0, 5)),
        st.tuples(st.just("clear"), st.just(0)),
        st.tuples(st.just("collect"), st.integers(0, 2)),
        st.tuples(st.just("garbage"), st.integers(0, 40)),
    ),
    min_size=1,
    max_size=14,
)


def program(behaviours: list[str], steps: list[tuple[str, int]]) -> str:
    lines = [CYCLES, "CBS = []", "budget = [60]"]
    for i, b in enumerate(behaviours):
        lines.append(textwrap.dedent(f"""
            def cb{i}(p, i):
                log({i}, p, i["generation"], i.get("mark"))
                if budget[0] <= 0:
                    return
                budget[0] -= 1
                b = {b!r}
                if b == "raise":
                    raise ValueError({i})
                if b == "append_other":
                    gc.callbacks.append(CBS[({i} + 1) % len(CBS)])
                if b == "remove_self" and cb{i} in gc.callbacks:
                    gc.callbacks.remove(cb{i})
                if b == "mark_info":
                    i["mark"] = {i}
                if b == "clear" and p == "stop":
                    gc.callbacks.clear()
                if b == "insert_front":
                    gc.callbacks.insert(0, CBS[({i} + 2) % len(CBS)])
            CBS.append(cb{i})
        """))
    for op, n in steps:
        k = n % len(behaviours)
        if op == "append":
            lines.append(f"gc.callbacks.append(CBS[{k}])")
        elif op == "insert0":
            lines.append(f"gc.callbacks.insert(0, CBS[{k}])")
        elif op == "remove":
            lines.append(f"CBS[{k}] in gc.callbacks and gc.callbacks.remove(CBS[{k}])")
        elif op == "clear":
            lines.append("gc.callbacks.clear()")
        elif op == "collect":
            lines.append(f"log('collect', gc.collect({n}))")
        else:
            lines.append(f"garbage({n})")
    lines.append("gc.callbacks.clear()")
    return "\n".join(lines) + "\n"


@settings(max_examples=int(os.environ.get("RQ_GC_EXAMPLES", "50")), deadline=None)
@given(behaviours=st.lists(st.sampled_from(BEHAVIOURS), min_size=1, max_size=5), steps=ops)
def test_differential_random_programs(behaviours, steps):
    src = program(behaviours, steps)
    plain = _run(src, "plain")
    if any(op == "collect" for op, _ in steps):
        assert '"collect"' in plain, plain
    assert _run(src, "tier") == plain
