"""Configuration loading for deploy tooling: parse a fleet of service configs written in
INI, TOML, YAML and JSON, layer them (defaults < environment < service), expand
``${VAR}`` references, validate, and dump the merged result.

Stands for what infrastructure scripts, CLI tools and app start-up do with config files:
``configparser`` (with interpolation and ``getint``/``getboolean``), ``tomllib.loads``,
PyYAML ``yaml.safe_load``/``safe_dump`` (pure-Python loader, which is what
``safe_load`` uses unless code opts into the libyaml ``CSafeLoader``), ``json.loads``/
``dumps``, recursive dict merging, ``string.Template``/``re.sub`` substitution and
``isinstance``-based schema checks. Data shape: 4 formats x 240 services, 1-3 KB each.
"""

REQUIRES = ["PyYAML==6.0.3"]

N_SERVICES = 240


def setup():
    import json
    import random

    rng = random.Random(11)
    envs = ["dev", "staging", "prod"]
    texts = {"ini": [], "toml": [], "yaml": [], "json": []}
    for s in range(N_SERVICES):
        name = f"svc-{s:03d}"
        port = 8000 + s
        replicas = rng.randint(1, 12)
        hosts = [f"db-{rng.randint(1, 9)}.internal" for _ in range(rng.randint(1, 4))]
        feats = {f"feature_{k}": rng.random() < 0.5 for k in range(rng.randint(4, 12))}
        # INI (legacy service settings)
        ini = [
            "[DEFAULT]", f"service = {name}", "base_dir = /srv/%(service)s",
            "log_dir = %(base_dir)s/logs", "",
            "[server]", f"port = {port}", f"workers = {replicas * 2}", "debug = no",
            "bind = 0.0.0.0:%(port)s", "",
            "[database]", f"hosts = {','.join(hosts)}", f"pool_size = {rng.randint(5, 50)}",
            "timeout = 30", f"password = ${{DB_PASSWORD_{s}}}", "",
            "[features]"] + [f"{k} = {'yes' if v else 'no'}" for k, v in feats.items()] + [
            "", "[logging]", "level = INFO", "file = %(log_dir)s/app.log", "rotate = true"]
        texts["ini"].append("\n".join(ini))
        # TOML (build/deploy metadata)
        toml = [
            "[service]", f'name = "{name}"', f'owner = "team-{rng.randint(1, 12)}"',
            f"replicas = {replicas}", f'image = "registry.example.com/{name}:1.{s}.0"', "",
            "[service.resources]", f'cpu = "{rng.choice(["250m", "500m", "1", "2"])}"',
            f'memory = "{rng.choice(["256Mi", "512Mi", "1Gi", "2Gi"])}"', "",
            "[[service.ports]]", f"container = {port}", 'protocol = "TCP"', "",
            "[[service.ports]]", f"container = {port + 1000}", 'protocol = "TCP"', "",
            "[env]"] + [f'VAR_{k} = "value-{rng.randint(1, 999)}"' for k in range(rng.randint(5, 20))
                        ] + ["", "[deploy]", 'strategy = "rolling"',
                             f"max_surge = {rng.randint(1, 3)}",
                             f"regions = {json.dumps(rng.sample(['eu', 'us', 'ap', 'sa'], 2))}",
                             f"updated = 2026-10-{rng.randint(1, 28):02d}T12:00:00Z"]
        texts["toml"].append("\n".join(toml))
        # YAML (k8s-style manifest)
        env_list = "\n".join(f"        - name: VAR_{k}\n          value: \"${{ENV_{k}}}\""
                             for k in range(rng.randint(4, 14)))
        yaml_doc = f"""\
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {name}
  labels:
    app: {name}
    tier: {rng.choice(['web', 'worker', 'api'])}
spec:
  replicas: {replicas}
  selector:
    matchLabels:
      app: {name}
  template:
    spec:
      containers:
      - name: {name}
        image: registry.example.com/{name}:1.{s}.0
        ports:
        - containerPort: {port}
        env:
{env_list}
        resources:
          limits: {{cpu: "1", memory: 1Gi}}
          requests: {{cpu: 250m, memory: 256Mi}}
        livenessProbe:
          httpGet: {{path: /healthz, port: {port}}}
          initialDelaySeconds: 10
"""
        texts["yaml"].append(yaml_doc)
        # JSON (feature flags / per-env overrides)
        texts["json"].append(json.dumps({
            "service": name,
            "overrides": {e: {"replicas": replicas + i, "debug": e == "dev",
                              "hosts": hosts[: i + 1]} for i, e in enumerate(envs)},
            "flags": feats,
            "limits": {"rps": rng.randint(100, 5000), "burst": rng.randint(10, 500)},
        }, indent=2))
    environ = {f"DB_PASSWORD_{s}": f"pw{s:03d}" for s in range(N_SERVICES)}
    environ.update({f"ENV_{k}": f"env-value-{k}" for k in range(20)})
    return {"texts": texts, "environ": environ}


def run(state):
    import configparser
    import json
    import re
    import string
    import tomllib

    import yaml

    environ = state["environ"]
    var_re = re.compile(r"\$\{([A-Z0-9_]+)\}")

    def expand(value):
        if isinstance(value, str):
            if "${" in value:
                return var_re.sub(lambda m: environ.get(m.group(1), m.group(0)), value)
            return value
        if isinstance(value, dict):
            return {k: expand(v) for k, v in value.items()}
        if isinstance(value, list):
            return [expand(v) for v in value]
        return value

    def merge(base, override):
        out = dict(base)
        for k, v in override.items():
            if isinstance(v, dict) and isinstance(out.get(k), dict):
                out[k] = merge(out[k], v)
            else:
                out[k] = v
        return out

    texts = state["texts"]
    merged = {}
    errors = []
    for i in range(len(texts["ini"])):
        cp = configparser.ConfigParser()
        cp.read_string(texts["ini"][i])
        ini = {
            "server": {"port": cp.getint("server", "port"),
                       "workers": cp.getint("server", "workers"),
                       "debug": cp.getboolean("server", "debug"),
                       "bind": cp.get("server", "bind")},
            "database": {"hosts": cp.get("database", "hosts").split(","),
                         "pool_size": cp.getint("database", "pool_size"),
                         "password": cp.get("database", "password")},
            "features": {k: cp.getboolean("features", k) for k in cp.options("features")
                         if k.startswith("feature_")},
            "logging": dict(cp.items("logging")),
        }
        toml = tomllib.loads(texts["toml"][i])
        manifest = yaml.safe_load(texts["yaml"][i])
        flags = json.loads(texts["json"][i])
        name = toml["service"]["name"]
        for env in ("dev", "staging", "prod"):
            cfg = merge(ini, {"service": toml["service"], "env": toml["env"],
                              "deploy": toml["deploy"]})
            cfg = merge(cfg, {"k8s": manifest["spec"]})
            cfg = merge(cfg, {"features": flags["flags"], "limits": flags["limits"]})
            cfg = merge(cfg, flags["overrides"][env])
            cfg = expand(cfg)
            if not isinstance(cfg["server"]["port"], int) or cfg["server"]["port"] < 1024:
                errors.append(f"{name}/{env}: bad port")
            if cfg["replicas"] > 10 and env != "prod":
                errors.append(f"{name}/{env}: too many replicas for {env}")
            merged[f"{name}/{env}"] = cfg
    banner = string.Template("# generated for $count services by $tool\n").substitute(
        count=len(merged), tool="deployctl")
    as_json = json.dumps(merged, sort_keys=True, indent=2, default=str)
    as_yaml = yaml.safe_dump({k: merged[k] for k in list(merged)[:20]}, sort_keys=True)
    return banner + as_json, as_yaml, errors


def digest(result):
    import hashlib

    banner_json, as_yaml, errors = result
    h = hashlib.sha256(banner_json.encode())
    h.update(as_yaml.encode())
    h.update("\n".join(errors).encode())
    return h.hexdigest()
