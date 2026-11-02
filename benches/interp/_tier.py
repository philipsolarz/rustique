"""Turn the opt-in interp tier on for benches (accel's tiers: `_accel.enable(tiers=["interp"])`,
the same as `RUSTIQUE_TIERS=interp`). With default=True the default groups are enabled too."""
import rustique

acc = rustique._accel


def interp_group():
    (g,) = {grp for name, grp, _ in acc.targets() if name.startswith("interp.")}
    return g


def default_groups():
    return sorted({grp for name, grp, _ in acc.targets()
                   if not grp.startswith("_") and not name.startswith("interp.")})


def enable_tier(default=False):
    if default:
        return acc.enable(tiers=["interp"])
    return acc.enable(only=[interp_group()], tiers=["interp"])


def info():
    return acc._t_interp.info()


def groups():
    return {k: {kk: v[kk] for kk in ("executions", "hits", "fills", "suspended")}
            for k, v in info().get("groups", {}).items()}


def states():
    return {t["name"]: (t["state"], t.get("reason")) for t in acc.status()["targets"]
            if t["name"].startswith("interp.")}
