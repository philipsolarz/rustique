"""Turn the interp tier on for benches and tests, whatever the switch API of the build is:
`rustique.enable(tiers=["interp"])` (accel's opt-in tiers) or, until they land, the hidden
group `_interp` named in `only=`. With default=True the default groups are enabled too."""
import rustique

acc = rustique._accel


def interp_group():
    groups = {t["group"] for t in acc.status()["targets"] if t["name"].startswith("interp.")}
    (g,) = groups
    return g


def default_groups():
    return sorted({t["group"] for t in acc.status()["targets"]
                   if not t["group"].startswith("_") and not t["name"].startswith("interp.")})


def enable_tier(default=False):
    g = interp_group()
    try:
        if default:
            return rustique.enable(tiers=["interp"])
        return rustique.enable(only=[g], tiers=["interp"])
    except TypeError:
        pass
    return rustique.enable(only=(default_groups() if default else []) + [g])


def info():
    return acc._t_interp.info()


def groups():
    return {k: {kk: v[kk] for kk in ("executions", "hits", "fills", "suspended")}
            for k, v in info().get("groups", {}).items()}


def states():
    return {t["name"]: (t["state"], t.get("reason")) for t in acc.status()["targets"]
            if t["name"].startswith("interp.")}
