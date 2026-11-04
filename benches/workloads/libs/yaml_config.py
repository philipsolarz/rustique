"""PyYAML loading and dumping configuration: Kubernetes manifests, compose files, app config.

Stands for one of the most-downloaded Python packages (PyYAML, top-10 on PyPI) in its
everyday role: reading and writing configuration in deployment tools, CI systems, Ansible,
pre-commit, MkDocs, Kubernetes tooling and application settings. ``yaml.safe_load`` and
``yaml.safe_load_all`` use the pure-Python ``SafeLoader`` (reader -> scanner -> parser ->
composer -> constructor) unless a program explicitly asks for the libyaml ``CSafeLoader``,
so this is the path most code takes. Per run: a 120-document Kubernetes manifest stream
(Deployments, Services, ConfigMaps with block scalars, Ingresses), 40 docker-compose style
files with anchors, aliases and ``<<`` merge keys, and 80 nested application configs with
timestamps, floats, booleans and flow collections are loaded, validated/transformed and
dumped back with ``yaml.safe_dump`` (emitter + representer), ~0.44 MB of YAML in total.
PyYAML 6.0.3 has no cp315 wheel; it installs from the sdist as pure Python
(``yaml.__with_libyaml__`` is False), which is what users of 3.15 get.
"""

REQUIRES = ["PyYAML==6.0.3"]

COMPOSE = """\
version: "3.9"
x-defaults: &defaults
  restart: unless-stopped
  logging:
    driver: json-file
    options: {{max-size: "10m", max-file: "3"}}
  environment: &env
    TZ: UTC
    LOG_LEVEL: info
    FEATURE_FLAGS: "a,b,c"
x-healthcheck: &health
  interval: 30s
  timeout: 5s
  retries: 3
services:
{services}
volumes:
  pgdata: {{}}
  cache: {{driver: local}}
networks:
  front: {{}}
  back: {{internal: true}}
"""

SERVICE = """\
  {name}:
    <<: *defaults
    image: "registry.example.com/{name}:{tag}"
    command: ["gunicorn", "app:create_app()", "--workers", "{workers}", "--bind", "0.0.0.0:{port}"]
    ports:
      - "{port}:{port}"
    environment:
      <<: *env
      SERVICE_NAME: {name}
      DATABASE_URL: postgres://app:secret@db:5432/{name}
      CACHE_TTL: {ttl}
      DEBUG: {debug}
    depends_on: [db, cache]
    healthcheck:
      <<: *health
      test: ["CMD", "curl", "-f", "http://localhost:{port}/health"]
    deploy:
      resources:
        limits: {{cpus: "{cpus}", memory: {mem}M}}
      labels:
        team: {team}
        tier: backend
"""


def setup():
    import datetime as dt
    import random

    import yaml

    rng = random.Random(12)
    words = "api web worker billing search auth gateway media report notify ingest export".split()

    manifests = []
    for i in range(120):
        app = f"{rng.choice(words)}-{i}"
        kind = ["Deployment", "Service", "ConfigMap", "Ingress"][i % 4]
        labels = {"app.kubernetes.io/name": app, "app.kubernetes.io/part-of": "shop", "team": rng.choice(words)}
        if kind == "Deployment":
            spec = {"replicas": rng.randint(1, 6), "selector": {"matchLabels": labels},
                    "template": {"metadata": {"labels": labels, "annotations": {"prometheus.io/scrape": "true"}},
                                 "spec": {"containers": [{
                                     "name": app, "image": f"registry.example.com/{app}:1.{i}.0",
                                     "ports": [{"containerPort": 8000 + i, "protocol": "TCP"}],
                                     "env": [{"name": f"VAR_{j}", "value": f"value-{rng.randint(0, 999)}"} for j in range(12)],
                                     "resources": {"requests": {"cpu": "100m", "memory": "128Mi"},
                                                   "limits": {"cpu": "1", "memory": "512Mi"}},
                                     "readinessProbe": {"httpGet": {"path": "/ready", "port": 8000 + i},
                                                        "periodSeconds": 10, "failureThreshold": 3}}]}}}
        elif kind == "Service":
            spec = {"type": "ClusterIP", "selector": labels,
                    "ports": [{"name": "http", "port": 80, "targetPort": 8000 + i}]}
        elif kind == "ConfigMap":
            spec = None
        else:
            spec = {"ingressClassName": "nginx", "rules": [{"host": f"{app}.example.com", "http": {"paths": [
                {"path": f"/{p}", "pathType": "Prefix", "backend": {"service": {"name": app, "port": {"number": 80}}}}
                for p in words[:6]]}}]}
        doc = {"apiVersion": "apps/v1" if kind == "Deployment" else "v1", "kind": kind,
               "metadata": {"name": app, "namespace": "prod", "labels": labels}}
        if spec is None:
            doc["data"] = {"app.ini": "\n".join(f"key{j} = {rng.random():.6f}" for j in range(30)) + "\n",
                           "nginx.conf": "server {\n  listen 80;\n  location / { proxy_pass http://app; }\n}\n"}
        else:
            doc["spec"] = spec
        manifests.append(doc)
    class BlockDumper(yaml.SafeDumper):  # multi-line strings as block scalars, as in hand-written manifests
        pass

    def str_presenter(dumper, data):
        return dumper.represent_scalar("tag:yaml.org,2002:str", data, style="|" if "\n" in data else None)

    BlockDumper.add_representer(str, str_presenter)
    manifest_text = yaml.dump_all(manifests, Dumper=BlockDumper, sort_keys=False, default_flow_style=False)

    composes = []
    for c in range(40):
        services = "".join(SERVICE.format(name=f"{words[(c + s) % 12]}{s}", tag=f"2025.{c}.{s}", workers=2 + s % 4,
                                          port=8000 + s, ttl=60 * (s + 1), debug=str(s % 2 == 0).lower(),
                                          cpus=f"0.{1 + s % 9}", mem=128 * (1 + s % 4), team=words[s % 12])
                           for s in range(8))
        composes.append(COMPOSE.format(services=services))

    configs = []
    for c in range(80):
        cfg = {"service": {"name": f"svc-{c}", "version": f"{c // 10}.{c % 10}.0", "started": dt.datetime(2025, 1, 1, c % 24, 0),
                           "release_date": dt.date(2025, 1 + c % 12, 1 + c % 28)},
               "server": {"host": "0.0.0.0", "port": 8000 + c, "workers": 4, "timeout": 30.5, "keepalive": True,
                          "trusted_proxies": [f"10.0.{j}.0/24" for j in range(6)]},
               "database": {"url": f"postgresql://db{c}.internal/app", "pool": {"min": 2, "max": 20, "recycle": 3600},
                            "replicas": [{"host": f"replica{j}.internal", "weight": round(rng.random(), 3)} for j in range(3)]},
               "features": {f"flag_{j}": rng.random() < 0.5 for j in range(25)},
               "limits": {k: {"rate": rng.randint(10, 1000), "burst": rng.randint(1, 50), "window": "1m"}
                          for k in ("login", "search", "upload", "export", "api")},
               "i18n": {"default": "en", "available": ["en", "de", "fr", "pl", "ja"], "fallbacks": {"de": "en", "pl": "en"}},
               "logging": {"version": 1, "handlers": {"console": {"class": "logging.StreamHandler", "level": "INFO"}},
                           "root": {"level": "INFO", "handlers": ["console"]}}}
        configs.append(yaml.safe_dump(cfg, sort_keys=True, default_flow_style=None, width=100))
    return {"manifests": manifest_text, "composes": composes, "configs": configs,
            "libyaml": yaml.__with_libyaml__}


def run(state):
    import hashlib

    import yaml

    h = hashlib.sha256()
    summary = []
    docs = list(yaml.safe_load_all(state["manifests"]))
    kinds = {}
    for d in docs:
        kinds[d["kind"]] = kinds.get(d["kind"], 0) + 1
        d["metadata"].setdefault("annotations", {})["deploy/revision"] = "42"
        if d["kind"] == "Deployment":
            for ctr in d["spec"]["template"]["spec"]["containers"]:
                ctr["image"] = ctr["image"].rsplit(":", 1)[0] + ":2.0.0"
    h.update(yaml.safe_dump_all(docs, sort_keys=False).encode())
    summary.append(sorted(kinds.items()))
    for text in state["composes"]:
        c = yaml.safe_load(text)
        ports = sorted(p for s in c["services"].values() for p in s["ports"])
        envs = sum(len(s["environment"]) for s in c["services"].values())
        summary.append((len(c["services"]), ports[0], envs, c["services"][next(iter(c["services"]))]["logging"]["options"]))
        h.update(yaml.safe_dump(c, sort_keys=True).encode())
    for text in state["configs"]:
        c = yaml.safe_load(text)
        c["server"]["port"] += 1
        enabled = sorted(k for k, v in c["features"].items() if v)
        summary.append((c["service"]["name"], c["service"]["started"].isoformat(), len(enabled)))
        h.update(yaml.safe_dump(c, default_flow_style=False).encode())
    return h.hexdigest(), summary


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
