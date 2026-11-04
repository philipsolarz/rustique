"""Prototype GC policy: yield-driven deferral of full collections (public `gc` APIs only).

Idea. A full collection (GIL build: generation 2; free-threaded build: every collection,
which always walks the whole heap) costs time proportional to the whole tracked heap. CPython
runs one when the old generation has grown by 25% since the last full collection
(`long_lived_pending >= long_lived_total / 4`, Python/gc.c:1356; FT: `count >=
long_lived_total / 4`, Python/gc_free_threading.c:2013) — whether or not earlier full
collections ever found garbage. This policy keeps CPython's young collections untouched and
lets the old generation grow by a factor G before the next full collection, where G adapts
to what full collections *find*:

    G = 0.25 * 2**k,  k in 0..kmax         (k = 0 is CPython's rule)
    after each full collection: yield y = collected / examined
        y <  y_low   -> k += 1             (full collections are finding nothing: back off)
        y >= y_high  -> k = 0              (garbage is reaching the old generation: CPython)
        otherwise    -> k -= 1

Worst case without a cap: cyclic garbage that reaches the old generation just after the
policy backed off waits one deferred interval (G * old-generation size promotions), up to
0.25 * 2**kmax * total objects of it (measured: 1.45x peak RSS, docs/atlas/w2-runtime.md 4.3).
The pause cap below removes that case.

Mechanism (GIL): a gc.callbacks entry tracks `total` (survivors of the last full
collection) and `pending` (survivors of generation-1 collections since), from the callback
`info` (3.15: 'candidates'); it holds threshold2 at a large value until pending >= G * total
and then restores the visible threshold2, so CPython's own rule decides the exact point.
(FT): threshold0 = max(visible threshold0, G * total) after a low-yield collection.

Pause cap (cap=1.1): a deferred full collection is released as soon as it would examine
more than `cap` x the objects of the largest *legitimate* full collection so far (GIL
estimate: survivors of the last full collection x growth of sys.getallocatedblocks(); FT:
survivors + net allocations, which FT's young count already is). A collection run under
CPython's rule (k == 0) counts with all it examined, a deferred one only with its survivors,
so garbage the deferral let pile up never raises the cap (no ratchet). Result: no collection
examines more than 1.1x what CPython's rule already ran (criterion c's allowance; collection
size is the deterministic proxy of pause length) and deferred garbage stays below ~10% of the
largest live heap (criterion b).

Stepping aside: the program's own gc.set_threshold / enable / disable / freeze / unfreeze /
set_debug calls end the policy for good: real thresholds become exactly what the program
set, the callback is removed. gc.get_threshold() always reports the visible thresholds.
Prototype only: these hooks replace the `gc` module attributes; a product would swap
`ml_meth` of the gc module's PyMethodDef entries (Modules/gcmodule.c:523) so identity and
`from gc import ...` taken earlier are preserved. gc.collect() is never touched.
"""

from __future__ import annotations

import gc
import sys

_real = {n: getattr(gc, n) for n in ("set_threshold", "get_threshold", "enable", "disable",
                                     "freeze", "unfreeze", "set_debug", "collect")}
FT = not getattr(sys, "_is_gil_enabled", lambda: True)()
BLOCK = 1 << 30


class AdaptivePolicy:
    def __init__(self, y_low=0.01, y_high=0.05, kmax=3, hooks=1, cap=1.1):
        self.y_low, self.y_high, self.kmax = y_low, y_high, int(kmax)
        self.hooks = bool(hooks)
        # pause cap: never defer a full collection past the size of the largest full
        # collection seen so far (criterion c: no collection longer than before)
        self.cap = float(cap)  # 0 = no cap; else the factor over the largest full collection
        self.max_full = 0
        self.blocks_at_full = 1
        self.k = 0
        self.total = -1
        self.pending = 0
        self.blocked = False
        self.active = False
        self.visible = None
        self.fulls = 0
        self.deferred_gen1 = 0
        self.history = []  # (k, yield) per full collection
        self.stepped_aside = None
        self._ft_t0 = None

    # -- life cycle ----------------------------------------------------------------
    def install(self):
        if not gc.isenabled():
            return
        self.visible = tuple(_real["get_threshold"]())
        if self.visible[0] == 0:
            return
        self.active = True
        gc.callbacks.insert(0, self._cb)
        if self.hooks:
            self._hook()

    def uninstall(self):
        self._step_aside("uninstall")

    def _step_aside(self, why):
        if not self.active:
            return
        self.active = False
        self.stepped_aside = why
        try:
            gc.callbacks.remove(self._cb)
        except ValueError:
            pass
        _real["set_threshold"](*self.visible)

    def _hook(self):
        pol = self

        def get_threshold():
            return pol.visible if pol.active else _real["get_threshold"]()

        def set_threshold(*args):
            r = _real["set_threshold"](*args)  # same validation and errors as CPython
            if pol.active:
                # the call changed only the thresholds it named: apply them over the
                # *visible* vector (the real one holds the policy's value)
                new = list(pol.visible)
                new[:len(args)] = args
                pol.visible = tuple(new)
            pol._step_aside("set_threshold")
            return r

        def wrap(name):
            f = _real[name]

            def w(*args, **kw):
                pol._step_aside(name)
                return f(*args, **kw)

            w.__name__ = name
            return w

        gc.get_threshold = get_threshold
        gc.set_threshold = set_threshold
        for n in ("enable", "disable", "freeze", "unfreeze", "set_debug"):
            setattr(gc, n, wrap(n))

    # -- the policy ------------------------------------------------------------------
    def _G(self):
        return 0.25 * (1 << self.k)

    def _cb(self, phase, info):
        if phase != "stop" or not self.active:
            return
        gen = info["generation"]
        cand = info.get("candidates", 0)
        col = info["collected"] + info["uncollectable"]
        if FT:
            # every FT collection is a whole-heap collection
            self._full(cand, col)
            t0 = self.visible[0]
            if self.k:
                t0 = max(t0, int(self._G() * self.total))
                if self.cap:
                    # the next collection walks ~ total + t0 objects
                    t0 = max(self.visible[0], min(t0, int(self.cap * self.max_full) - self.total))
            t0 = min(t0, BLOCK)
            if t0 != self._ft_t0:
                # set_threshold stops the world on FT (Modules/gcmodule.c:167): only on change
                _real["set_threshold"](t0, *self.visible[1:])
                self._ft_t0 = t0
            return
        if gen == 2:
            self._full(cand, col)
        elif gen == 1:
            self.pending += max(0, cand - col)
        else:
            return
        allow = self.k == 0 or self.total < 0 or self.pending >= self._G() * self.total
        if not allow and self.cap and gen == 1:
            # size of the next full collection ~ survivors of the last one scaled by the
            # growth of pymalloc's live blocks since (sys.getallocatedblocks: O(arenas));
            # `pending` alone overestimates it (it never subtracts promoted objects that died)
            est = self.total * sys.getallocatedblocks() / max(1, self.blocks_at_full)
            allow = est >= self.cap * self.max_full
        if allow and self.blocked:
            _real["set_threshold"](*self.visible)
            self.blocked = False
        elif not allow and not self.blocked:
            _real["set_threshold"](self.visible[0], self.visible[1], BLOCK)
            self.blocked = True
        if not allow and gen == 1:
            self.deferred_gen1 += 1

    def _full(self, cand, col):
        y = col / cand if cand > 0 else 0.0
        # the cap's reference size: a collection CPython's own rule ran (k == 0) counts with
        # everything it examined; a collection the policy deferred counts only with what
        # survived (live objects), so garbage the deferral let pile up never raises the cap
        # (no ratchet: without this, each deferred collection could be 1.1x the previous)
        self.max_full = max(self.max_full, cand if self.k == 0 else cand - col)
        self.fulls += 1
        self.history.append((self.k, round(y, 4)))
        if y < self.y_low:
            self.k = min(self.k + 1, self.kmax)
        elif y >= self.y_high:
            self.k = 0
        elif self.k:
            self.k -= 1
        self.total = max(0, cand - col)
        self.pending = 0
        if not FT:
            self.blocks_at_full = sys.getallocatedblocks()

    def report(self):
        return {"k": self.k, "fulls": self.fulls, "deferred_gen1": self.deferred_gen1,
                "history": self.history[-40:], "stepped_aside": self.stepped_aside,
                "active": self.active, "ft": FT}


class ProbePolicy(AdaptivePolicy):
    """gcpolicy adapter (no oracle hooks)."""

    def after_setup(self):
        pass

    def after_warmup(self):
        pass


def install(**kw):
    """Launch-time entry point (sitecustomize / .pth): install the policy once."""
    p = AdaptivePolicy(**kw)
    p.install()
    return p
