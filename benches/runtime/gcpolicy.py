"""GC policies for the w2-runtime experiments (public `gc` APIs only).

Specs (combine with '+'):
  default            nothing
  off                gc.disable() (lower bound on GC time, upper bound on memory)
  thr:T0[,T1[,T2]]   gc.set_threshold(...) at launch
  freeze@setup       oracle: gc.freeze() after the workload's setup() (imports + inputs)
  freeze@warmup      oracle: gc.freeze() after setup() and one warm-up run()
  adaptive[:k=v,...] the prototype launch-time policy (gcadapt.AdaptivePolicy), no oracle
  tier[:k=v,...]     the product: Rustique's opt-in `gc` tier (crates/rustique-py/src/targets/gc),
                     enabled alone; k=v: y_low, y_high, kmax, cap, step_aside (research
                     parameters, `_t_gc.set_params`), all=1 (the rest of Rustique too)
  rustique           Rustique's default targets, no tier (the baseline of `tier:all=1`)

"Oracle" policies use the probe's knowledge of where setup ends; they bound what a policy
could get, a launch-time policy cannot know those points.
"""

from __future__ import annotations

import gc


class Policy:
    name = "default"

    def install(self):
        pass

    def after_setup(self):
        pass

    def after_warmup(self):
        pass

    def uninstall(self):
        pass

    def report(self):
        return {}


class Off(Policy):
    def install(self):
        gc.disable()

    def uninstall(self):
        gc.enable()


class Threshold(Policy):
    def __init__(self, arg):
        self.t = tuple(int(x) for x in arg.split(","))

    def install(self):
        self._old = gc.get_threshold()
        gc.set_threshold(*self.t)

    def uninstall(self):
        gc.set_threshold(*self._old)


class Freeze(Policy):
    def __init__(self, when):
        self.when = when
        self.frozen = 0

    def _freeze(self):
        gc.freeze()
        self.frozen = gc.get_freeze_count()

    def after_setup(self):
        if self.when == "setup":
            self._freeze()

    def after_warmup(self):
        if self.when == "warmup":
            self._freeze()

    def report(self):
        return {"frozen": self.frozen}


class Combo(Policy):
    def __init__(self, parts):
        self.parts = parts

    def install(self):
        for p in self.parts:
            p.install()

    def after_setup(self):
        for p in self.parts:
            p.after_setup()

    def after_warmup(self):
        for p in self.parts:
            p.after_warmup()

    def uninstall(self):
        for p in reversed(self.parts):
            p.uninstall()

    def report(self):
        out = {}
        for p in self.parts:
            out.update(p.report())
        return out


class Tier(Policy):
    """Rustique's `gc` tier (installed before the workload is imported)."""

    def __init__(self, kw, tier=True):
        self.all = bool(kw.pop("all", 0))
        if "step_aside" in kw:
            kw["step_aside"] = bool(kw["step_aside"])
        self.kw = kw
        self.tier = tier

    def install(self):
        import os

        import rustique

        if self.tier:
            os.environ["RUSTIQUE_TIERS"] = "gc"
            if self.kw:
                rustique._accel._t_gc.set_params(**self.kw)
        if self.all:
            rustique.enable()
        if self.tier:
            rustique.enable(only=["gc"])
            st = {t["name"]: t for t in rustique.status()["targets"]}["gc.adaptive"]
            if st["state"] != "installed":
                raise SystemExit(f"gc tier not installed: {st['reason']}")

    def uninstall(self):
        import rustique

        self._report = self.report()
        rustique.disable()

    def report(self):
        if getattr(self, "_report", None) is not None:
            return self._report
        if not self.tier:
            return {}
        import rustique

        r = rustique._accel._t_gc.report()
        keep = ("active", "armed", "k", "fulls", "deferred", "max_full", "history", "stepped_aside", "first_full",
                "params")
        return {k: r.get(k) for k in keep}


def _kw(spec):
    kw = {}
    if ":" in spec:
        for kv in spec.split(":", 1)[1].split(","):
            k, _, v = kv.partition("=")
            kw[k] = float(v) if "." in v else int(v)
    return kw


def _one(spec):
    if spec in ("default", ""):
        return Policy()
    if spec == "off":
        return Off()
    if spec.startswith("thr:"):
        return Threshold(spec[4:])
    if spec.startswith("freeze@"):
        return Freeze(spec[7:])
    if spec.startswith("tier"):
        return Tier(_kw(spec))
    if spec == "rustique":
        return Tier({"all": 1}, tier=False)
    if spec.startswith("adaptive"):
        import gcadapt

        kw = {}
        if ":" in spec:
            for kv in spec.split(":", 1)[1].split(","):
                k, _, v = kv.partition("=")
                kw[k] = float(v) if "." in v else int(v)
        return gcadapt.ProbePolicy(**kw)
    raise SystemExit(f"unknown policy {spec!r}")


def make(spec):
    parts = [_one(s) for s in spec.split("+")]
    return parts[0] if len(parts) == 1 else Combo(parts)
