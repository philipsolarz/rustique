"""Access-log and syslog analysis script: regex-parse web server and sshd logs, parse
timestamps, aggregate traffic and security statistics, emit a JSON report.

Stands for the ops/security scripts run over log files every day (goaccess-style
summaries, fail2ban-style auth-failure counting): a compiled ``re`` pattern per line with
named groups, ``datetime.strptime`` on the Apache timestamp (``%d/%b/%Y:%H:%M:%S %z``),
``urllib.parse`` on request paths and query strings, ``ipaddress`` subnet bucketing, and
``Counter``/``defaultdict`` aggregation. Data shape: 45k Apache "combined" lines plus 15k
syslog lines (~10 MB), read from a file.
"""

REQUIRES: list[str] = []

N_ACCESS = 45_000
N_SYSLOG = 15_000


def setup():
    import os
    import random
    import tempfile

    rng = random.Random(42)
    months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov",
              "Dec"]
    paths = ["/", "/index.html", "/api/v1/orders", "/api/v1/orders/{id}", "/api/v1/users/{id}",
             "/static/app.{h}.js", "/static/style.{h}.css", "/search", "/login", "/logout",
             "/products/{id}", "/cart", "/checkout", "/wp-login.php", "/.env", "/robots.txt",
             "/favicon.ico", "/healthz"]
    agents = [
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) "
        "Chrome/128.0.0.0 Safari/537.36",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) "
        "Version/17.6 Safari/605.1.15",
        "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0",
        "Mozilla/5.0 (iPhone; CPU iPhone OS 17_6 like Mac OS X) AppleWebKit/605.1.15 "
        "(KHTML, like Gecko) Version/17.6 Mobile/15E148 Safari/604.1",
        "Googlebot/2.1 (+http://www.google.com/bot.html)",
        "curl/8.9.1",
        "python-requests/2.32.3",
        "Mozilla/5.0 (compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm)",
    ]
    statuses = [200] * 30 + [301, 302, 304, 304, 400, 401, 403, 404, 404, 404, 500, 502]
    referers = ["-", "https://www.example.com/", "https://www.google.com/",
                "https://news.example.org/item?id=1234", "-"]
    ips = [f"{rng.randint(1, 223)}.{rng.randint(0, 255)}.{rng.randint(0, 255)}."
           f"{rng.randint(1, 254)}" for _ in range(3000)]
    lines = []
    for i in range(N_ACCESS):
        sec = i * 3
        day = 1 + (sec // 86400) % 28
        hh, mm, ss = (sec // 3600) % 24, (sec // 60) % 60, sec % 60
        ts = f"{day:02d}/{months[9]}/2026:{hh:02d}:{mm:02d}:{ss:02d} +0000"
        p = rng.choice(paths).format(id=rng.randint(1, 50000), h=rng.getrandbits(32))
        if p in ("/search", "/products/{id}") or rng.random() < 0.15:
            p += f"?q=term{rng.randint(1, 300)}&page={rng.randint(1, 9)}&utm_source=mail"
        method = "POST" if p.startswith("/api") and rng.random() < 0.3 else "GET"
        user = "-" if rng.random() < 0.9 else f"user{rng.randint(1, 99)}"
        size = rng.randint(0, 250_000)
        lines.append(f'{rng.choice(ips)} - {user} [{ts}] "{method} {p} HTTP/1.1" '
                     f'{rng.choice(statuses)} {size} "{rng.choice(referers)}" '
                     f'"{rng.choice(agents)}"')
    hosts = ["web-1", "web-2", "bastion", "db-1"]
    for i in range(N_SYSLOG):
        sec = i * 9
        day = 1 + (sec // 86400) % 28
        ts = f"Oct {day:2d} {(sec // 3600) % 24:02d}:{(sec // 60) % 60:02d}:{sec % 60:02d}"
        host = rng.choice(hosts)
        pid = rng.randint(300, 65000)
        r = rng.random()
        ip = rng.choice(ips)
        if r < 0.45:
            msg = (f"sshd[{pid}]: Failed password for "
                   f"{'invalid user ' if rng.random() < 0.6 else ''}"
                   f"{rng.choice(['root', 'admin', 'ubuntu', 'test', 'deploy', 'oracle'])} "
                   f"from {ip} port {rng.randint(1024, 65535)} ssh2")
        elif r < 0.6:
            msg = (f"sshd[{pid}]: Accepted publickey for deploy from {ip} port "
                   f"{rng.randint(1024, 65535)} ssh2: ED25519 SHA256:{rng.getrandbits(64):x}")
        elif r < 0.8:
            msg = (f"CRON[{pid}]: (root) CMD (/usr/local/bin/backup.sh --incremental "
                   f">/dev/null 2>&1)")
        else:
            msg = (f"kernel: [UFW BLOCK] IN=eth0 OUT= SRC={ip} DST=10.0.0.5 LEN=60 "
                   f"PROTO=TCP SPT={rng.randint(1024, 65535)} DPT={rng.choice([22, 23, 3389, 5432])}")
        lines.append(f"{ts} {host} {msg}")
    tmp = tempfile.TemporaryDirectory(prefix="rq-logs-")
    path = os.path.join(tmp.name, "combined.log")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    return {"tmp": tmp, "path": path}


def run(state):
    import ipaddress
    import json
    import re
    from collections import Counter, defaultdict
    from datetime import datetime
    from urllib.parse import parse_qs, urlsplit

    access_re = re.compile(
        r'(?P<ip>\S+) (?P<ident>\S+) (?P<user>\S+) \[(?P<ts>[^\]]+)\] '
        r'"(?P<method>[A-Z]+) (?P<target>\S+) (?P<proto>[^"]+)" (?P<status>\d{3}) '
        r'(?P<size>\d+|-) "(?P<referer>[^"]*)" "(?P<agent>[^"]*)"')
    syslog_re = re.compile(
        r"^(?P<ts>[A-Z][a-z]{2} [ \d]\d \d\d:\d\d:\d\d) (?P<host>\S+) "
        r"(?P<prog>[\w\-/]+)(?:\[(?P<pid>\d+)\])?: (?P<msg>.*)$")
    failed_re = re.compile(r"Failed password for (invalid user )?(\S+) from (\S+) port (\d+)")
    ufw_re = re.compile(r"SRC=(\S+) .*DPT=(\d+)")
    bot_re = re.compile(r"bot|crawler|spider|curl|python-requests", re.I)
    id_re = re.compile(r"/\d+(?=/|$)")

    status_counts = Counter()
    endpoints = Counter()
    bytes_per_hour = defaultdict(int)
    subnets = Counter()
    search_terms = Counter()
    agents = Counter()
    errors_by_path = Counter()
    latest = None
    failed_users = Counter()
    failed_ips = Counter()
    blocked_ports = Counter()
    unparsed = 0
    with open(state["path"], encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            m = access_re.match(line)
            if m is not None:
                d = m.groupdict()
                ts = datetime.strptime(d["ts"], "%d/%b/%Y:%H:%M:%S %z")
                if latest is None or ts > latest:
                    latest = ts
                status = int(d["status"])
                status_counts[status // 100 * 100] += 1
                parts = urlsplit(d["target"])
                route = id_re.sub("/:id", parts.path)
                endpoints[(d["method"], route)] += 1
                if parts.query:
                    q = parse_qs(parts.query)
                    for term in q.get("q", ()):
                        search_terms[term.lower()] += 1
                size = 0 if d["size"] == "-" else int(d["size"])
                bytes_per_hour[ts.strftime("%Y-%m-%d %H:00")] += size
                net = ipaddress.ip_network(f"{d['ip']}/24", strict=False)
                subnets[str(net)] += 1
                agent = d["agent"]
                kind = "bot" if bot_re.search(agent) else (
                    "mobile" if "Mobile" in agent else "desktop")
                agents[kind] += 1
                if status >= 500 or status == 404:
                    errors_by_path[route] += 1
                continue
            m = syslog_re.match(line)
            if m is None:
                unparsed += 1
                continue
            prog, msg = m.group("prog"), m.group("msg")
            if prog == "sshd":
                fm = failed_re.search(msg)
                if fm:
                    failed_users[fm.group(2)] += 1
                    failed_ips[fm.group(3)] += 1
            elif prog == "kernel" and "UFW BLOCK" in msg:
                um = ufw_re.search(msg)
                if um:
                    blocked_ports[int(um.group(2))] += 1
    busiest = sorted(bytes_per_hour.items(), key=lambda kv: (-kv[1], kv[0]))[:10]
    report = {
        "latest": latest.isoformat(),
        "status": sorted(status_counts.items()),
        "endpoints": [[m, r, n] for (m, r), n in endpoints.most_common(25)],
        "busiest_hours": busiest,
        "subnets": subnets.most_common(15),
        "search": search_terms.most_common(15),
        "agents": sorted(agents.items()),
        "errors": errors_by_path.most_common(10),
        "ssh_failed_users": failed_users.most_common(10),
        "ssh_failed_ips": failed_ips.most_common(10),
        "blocked_ports": sorted(blocked_ports.items()),
        "unparsed": unparsed,
    }
    return json.dumps(report, sort_keys=True)


def digest(result):
    import hashlib

    return hashlib.sha256(result.encode()).hexdigest()
