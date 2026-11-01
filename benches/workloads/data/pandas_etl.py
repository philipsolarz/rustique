"""pandas ETL: load an orders CSV and a customer table, parse dates, clean strings, join,
group, pivot and export, with one row-wise ``apply`` the way real notebooks and jobs do.

Included to represent reality: a large share of data-processing Python runs through
pandas, whose work is mostly C/Cython/NumPy, so it bounds what interpreter- or
stdlib-level acceleration can reach for this population. Uses ``pd.read_csv``,
``pd.to_datetime(format=...)``, ``.str`` accessor methods, ``merge``, ``groupby().agg``,
``pivot_table``, ``Series.apply``/``map`` with Python callables, ``to_json``/
``to_csv``. Data shape: 240k orders x 10 columns (~19 MB CSV) + 5k customers.
"""

REQUIRES = ["pandas==3.1.0", "numpy==2.5.4", "python-dateutil==4.0.0", "tzdata==2026.6"]

N_ROWS = 240_000


def setup():
    import io
    import random

    import pandas as pd  # noqa: F401  (import cost belongs to setup)

    rng = random.Random(31)
    countries = ["US", "DE", "FR", "GB", "PL", "NL", "SE", "ES", "IT", "JP"]
    channels = ["web", "ios", "android", "marketplace", "phone"]
    buf = io.StringIO()
    buf.write("order_id,created_at,customer_id,sku,quantity,unit_price,discount,status,"
              "channel,note\n")
    for i in range(N_ROWS):
        buf.write(
            f"{i},2025-{rng.randint(1, 12):02d}-{rng.randint(1, 28):02d} "
            f"{rng.randint(0, 23):02d}:{rng.randint(0, 59):02d}:{rng.randint(0, 59):02d},"
            f"C{rng.randrange(5000):05d},SKU-{rng.choice('ABCDEFGH')}{rng.randrange(250):04d},"
            f"{rng.randint(1, 12)},{rng.uniform(0.5, 400):.2f},"
            f"{rng.choice(['', '', '', '5%', '10%', '15%'])},"
            f"{rng.choice(['PAID', 'paid', ' Paid ', 'refunded', 'cancelled'])},"
            f"{rng.choice(channels)},"
            f"{rng.choice(['', '', 'gift wrap', 'leave at door', 'call before delivery'])}\n")
    cust = io.StringIO()
    cust.write("customer_id,name,email,country,signup\n")
    for c in range(5000):
        cust.write(f"C{c:05d},Customer {c},  USER{c}@Example.COM ,{rng.choice(countries)},"
                   f"20{rng.randint(15, 25)}-{rng.randint(1, 12):02d}-{rng.randint(1, 28):02d}\n")
    return {"orders_csv": buf.getvalue(), "customers_csv": cust.getvalue()}


def run(state):
    import io

    import pandas as pd

    orders = pd.read_csv(io.StringIO(state["orders_csv"]), dtype={"discount": "string",
                                                                 "note": "string"})
    customers = pd.read_csv(io.StringIO(state["customers_csv"]))
    orders["created_at"] = pd.to_datetime(orders["created_at"], format="%Y-%m-%d %H:%M:%S")
    orders["status"] = orders["status"].str.strip().str.lower()
    orders = orders[orders["status"].isin(["paid", "refunded"])].copy()
    disc = orders["discount"].fillna("").str.rstrip("%")
    orders["disc_pct"] = pd.to_numeric(disc, errors="coerce").fillna(0.0) / 100
    orders["net"] = (orders["quantity"] * orders["unit_price"] * (1 - orders["disc_pct"]))
    orders.loc[orders["status"] == "refunded", "net"] *= -1
    # the row-wise Python callable that real jobs nearly always contain somewhere
    orders["size_class"] = orders["net"].apply(
        lambda v: "large" if v > 1000 else "medium" if v > 100 else "small" if v > 0 else "refund")
    orders["family"] = orders["sku"].map(lambda s: s.split("-", 1)[1][0])
    customers["email"] = customers["email"].str.strip().str.lower()
    customers["signup"] = pd.to_datetime(customers["signup"])
    df = orders.merge(customers, on="customer_id", how="left")
    df["month"] = df["created_at"].dt.to_period("M").astype(str)
    df["tenure_days"] = (df["created_at"] - df["signup"]).dt.days
    by_country = (df.groupby(["country", "month"])
                    .agg(orders=("order_id", "count"), revenue=("net", "sum"),
                         avg_qty=("quantity", "mean"))
                    .round(4).reset_index())
    pivot = df.pivot_table(index="family", columns="channel", values="net", aggfunc="sum",
                           fill_value=0).round(2)
    top_customers = (df.groupby("email")["net"].sum().sort_values(ascending=False)
                     .head(50).round(2))
    notes = df["note"].fillna("").str.contains("door").sum()
    return (by_country.to_csv(index=False), pivot.to_json(orient="split"),
            top_customers.to_json(), int(notes), int(df["tenure_days"].median()))


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
