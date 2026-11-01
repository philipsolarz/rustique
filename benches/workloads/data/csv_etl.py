"""Nightly sales-export ETL: read an order CSV, clean and type-convert each row, aggregate,
and write JSON lines plus a summary CSV.

Stands for the most common Python batch job there is: a ``csv.DictReader`` loop that
normalises strings (strip/lower/split), converts numbers and ISO timestamps
(``datetime.fromisoformat``), filters, groups with ``defaultdict``/``Counter`` and emits
``json.dumps`` records and a ``csv.writer`` report. Data shape: 40k orders, 13 columns,
~4.5 MB of CSV read from a file, as an export from a shop or CRM would look.
"""

REQUIRES: list[str] = []

N_ROWS = 40_000


def setup():
    import os
    import random
    import tempfile

    rng = random.Random(1234)
    countries = ["US", "DE", "FR", "GB", "PL", "NL", "SE", "ES", "IT", "JP", "BR", "CA"]
    currencies = {"US": "USD", "CA": "CAD", "GB": "GBP", "JP": "JPY", "BR": "BRL",
                  "SE": "SEK", "PL": "PLN"}
    statuses = ["paid"] * 8 + ["refunded", "cancelled", "pending"]
    channels = ["web", "ios", "android", "marketplace", "phone"]
    first = ["anna", "ben", "carla", "dmitri", "eva", "farid", "greta", "hiro", "ines",
             "jonas", "kemal", "lena", "mateo", "nora", "olivia", "piotr"]
    domains = ["example.com", "mail.example.org", "corp.example.net", "example.de"]
    skus = [f"SKU-{c}{n:04d}" for c in "ABCDEFGH" for n in range(0, 250)]
    tmp = tempfile.TemporaryDirectory(prefix="rq-csv-etl-")
    path = os.path.join(tmp.name, "orders.csv")
    header = ["order_id", "created_at", "customer_email", "customer_name", "country",
              "currency", "channel", "sku", "quantity", "unit_price", "discount",
              "status", "tags"]
    lines = [",".join(header)]
    for i in range(N_ROWS):
        country = rng.choice(countries)
        month = rng.randint(1, 12)
        day = rng.randint(1, 28)
        ts = (f"2025-{month:02d}-{day:02d}T{rng.randint(0, 23):02d}:"
              f"{rng.randint(0, 59):02d}:{rng.randint(0, 59):02d}"
              f"{rng.choice(['+00:00', '+01:00', '-05:00', '+09:00', 'Z'])}")
        name = f"{rng.choice(first).title()} {rng.choice(first).title()}son"
        email = f"  {name.split()[0]}.{rng.randint(1, 999)}@{rng.choice(domains)} "
        if rng.random() < 0.3:
            email = email.upper()
        price = f"{rng.uniform(0.5, 400):.2f}"
        discount = rng.choice(["", "", "", "5%", "10%", "15%", "2.50"])
        tags = ";".join(rng.sample(["gift", "promo", "b2b", "repeat", "vip", "new"],
                                   rng.randint(0, 3)))
        name_field = f'"{name}, Jr."' if rng.random() < 0.05 else name
        lines.append(",".join([
            f"ORD{i:07d}", ts, email, name_field, country,
            currencies.get(country, "EUR"), rng.choice(channels), rng.choice(skus),
            str(rng.randint(1, 12)), price, discount, rng.choice(statuses), tags,
        ]))
    with open(path, "w", newline="") as f:
        f.write("\r\n".join(lines) + "\r\n")
    fx = {"USD": 0.92, "CAD": 0.68, "GBP": 1.17, "JPY": 0.0062, "BRL": 0.17,
          "SEK": 0.088, "PLN": 0.23, "EUR": 1.0}
    return {"tmp": tmp, "path": path, "fx": fx}


def run(state):
    import csv
    import io
    import json
    from collections import Counter, defaultdict
    from datetime import datetime, timezone

    fx = state["fx"]
    out = io.StringIO()
    by_country_month = defaultdict(lambda: [0, 0.0])
    sku_units = Counter()
    channel_rev = defaultdict(float)
    customers = defaultdict(set)
    rejected = 0
    with open(state["path"], newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            status = row["status"].strip().lower()
            if status not in ("paid", "refunded"):
                rejected += 1
                continue
            created = datetime.fromisoformat(row["created_at"]).astimezone(timezone.utc)
            qty = int(row["quantity"])
            price = float(row["unit_price"])
            disc = row["discount"].strip()
            if disc.endswith("%"):
                net = price * qty * (1 - float(disc[:-1]) / 100)
            elif disc:
                net = max(price * qty - float(disc), 0.0)
            else:
                net = price * qty
            eur = round(net * fx[row["currency"]], 2)
            if status == "refunded":
                eur = -eur
            email = row["customer_email"].strip().lower()
            local, _, domain = email.partition("@")
            name = row["customer_name"].replace(", Jr.", "").strip()
            tags = [t for t in row["tags"].split(";") if t]
            month = created.strftime("%Y-%m")
            agg = by_country_month[(row["country"], month)]
            agg[0] += 1
            agg[1] += eur
            sku_units[row["sku"]] += qty if status == "paid" else -qty
            channel_rev[row["channel"]] += eur
            customers[row["country"]].add(email)
            record = {
                "id": row["order_id"],
                "ts": created.isoformat(),
                "customer": {"email": email, "domain": domain, "name": name,
                             "first": name.split(" ", 1)[0]},
                "country": row["country"],
                "sku": row["sku"],
                "qty": qty,
                "eur": eur,
                "tags": tags,
                "refund": status == "refunded",
            }
            out.write(json.dumps(record, ensure_ascii=False, sort_keys=True))
            out.write("\n")
    summary = io.StringIO()
    w = csv.writer(summary, lineterminator="\n")
    w.writerow(["country", "month", "orders", "revenue_eur"])
    for (country, month), (n, rev) in sorted(by_country_month.items()):
        w.writerow([country, month, n, f"{rev:.2f}"])
    report = {
        "rejected": rejected,
        "top_skus": sku_units.most_common(20),
        "channels": {k: round(v, 2) for k, v in sorted(channel_rev.items())},
        "unique_customers": {k: len(v) for k, v in sorted(customers.items())},
    }
    return out.getvalue(), summary.getvalue(), json.dumps(report, sort_keys=True)


def digest(result):
    import hashlib

    h = hashlib.sha256()
    for part in result:
        h.update(part.encode())
        h.update(b"\0")
    return h.hexdigest()
