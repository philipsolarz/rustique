"""sqlglot parsing, transpiling and optimizing 100 generated analytics queries.

Stands for SQL tooling in data-engineering dev loops (SQLMesh/dbt-style lineage and
dialect translation, SQL linters/formatters built on sqlglot): tokenize + recursive-descent
parse each PostgreSQL query into an expression tree, generate it for DuckDB, BigQuery and
Snowflake, and run the full optimizer (qualify columns against a schema, normalize,
push down predicates, simplify, ...) on every 4th query. Queries are generated
deterministically in `setup()` (CTEs, joins, GROUP BY/HAVING, window functions, CASE,
IN/EXISTS subqueries, date and string functions), ~80 kB of SQL. sqlglot is pure Python
here (its optional Rust tokenizer `sqlglotrs` is not installed).
"""

import hashlib
import random

REQUIRES = ["sqlglot==30.27.0"]

SCHEMA = {
    "orders": {"o_id": "INT", "o_cust": "INT", "o_date": "DATE", "o_total": "DECIMAL(12,2)",
               "o_status": "VARCHAR", "o_region": "VARCHAR"},
    "customers": {"c_id": "INT", "c_name": "VARCHAR", "c_email": "VARCHAR", "c_region": "VARCHAR",
                  "c_created": "TIMESTAMP", "c_tier": "INT"},
    "lineitems": {"l_order": "INT", "l_product": "INT", "l_qty": "INT", "l_price": "DECIMAL(12,2)",
                  "l_discount": "DOUBLE", "l_shipdate": "DATE"},
    "products": {"p_id": "INT", "p_name": "VARCHAR", "p_category": "VARCHAR", "p_cost": "DOUBLE"},
}
ALIAS = {"orders": "o", "customers": "c", "lineitems": "l", "products": "p"}
JOIN = {("orders", "customers"): "o.o_cust = c.c_id", ("orders", "lineitems"): "o.o_id = l.l_order",
        ("lineitems", "products"): "l.l_product = p.p_id", ("customers", "lineitems"): "o.o_id = l.l_order"}


def _query(rng, i):
    tables = rng.choice([["orders", "customers"], ["orders", "lineitems"],
                         ["orders", "lineitems", "products"], ["orders", "customers", "lineitems"]])
    cols = [f"{ALIAS[t]}.{c}" for t in tables for c in SCHEMA[t]]
    num = [f"{ALIAS[t]}.{c}" for t in tables for c, ty in SCHEMA[t].items()
           if ty.startswith(("INT", "DEC", "DOU"))]
    txt = [f"{ALIAS[t]}.{c}" for t in tables for c, ty in SCHEMA[t].items() if ty == "VARCHAR"]
    keys = rng.sample(txt, min(2, len(txt)))
    total = f"SUM({rng.choice(num)}) AS total_{i}"
    aggs = [f"COUNT(DISTINCT {rng.choice(cols)}) AS n_{i}",
            f"AVG(CASE WHEN {rng.choice(num)} > {rng.randint(1, 500)} THEN {rng.choice(num)} ELSE 0 END) AS a_{i}",
            f"MAX(COALESCE({rng.choice(num)}, 0)) AS m_{i}"]
    frm = f"{tables[0]} AS {ALIAS[tables[0]]}"
    for a, b in zip(tables, tables[1:]):
        kind = rng.choice(["JOIN", "LEFT JOIN", "INNER JOIN"])
        frm += f" {kind} {b} AS {ALIAS[b]} ON {JOIN[(a, b)]}"
    where = [f"{rng.choice(num)} BETWEEN {rng.randint(0, 50)} AND {rng.randint(100, 900)}",
             f"UPPER({rng.choice(txt)}) LIKE '%{rng.choice('ABCDE')}%'",
             f"{rng.choice(txt)} IN ({', '.join(repr(f'v{k}') for k in range(rng.randint(2, 6)))})",
             f"o.o_date >= DATE '2024-0{rng.randint(1, 9)}-01' + INTERVAL '{rng.randint(1, 30)} days'",
             f"EXISTS (SELECT 1 FROM customers AS c2 WHERE c2.c_id = o.o_cust AND c2.c_tier > {rng.randint(1, 4)})"]
    body = (f"SELECT {', '.join(keys)}, {total}, {', '.join(rng.sample(aggs, 2))} FROM {frm} "
            f"WHERE {' AND '.join(rng.sample(where, 3))} GROUP BY {', '.join(keys)} "
            f"HAVING COUNT(*) > {rng.randint(1, 20)}")
    win = (f"SELECT *, ROW_NUMBER() OVER (PARTITION BY {keys[0].split('.')[1]} ORDER BY total_{i} DESC) AS rk, "
           f"SUM(total_{i}) OVER (ORDER BY {keys[0].split('.')[1]} ROWS BETWEEN 2 PRECEDING AND CURRENT ROW) AS roll, "
           f"CONCAT(LOWER(CAST({keys[0].split('.')[1]} AS VARCHAR)), '-', '{i}') AS label "
           f"FROM base_{i}")
    return (f"WITH base_{i} AS ({body}), ranked_{i} AS ({win}) SELECT * FROM ranked_{i} "
            f"WHERE rk <= {rng.randint(3, 10)} ORDER BY roll DESC NULLS LAST LIMIT {rng.randint(10, 100)}")


def setup():
    import logging
    import sqlglot  # noqa: F401
    from sqlglot.optimizer import optimize  # noqa: F401

    logging.getLogger("sqlglot").setLevel(logging.ERROR)  # unsupported-translation warnings

    rng = random.Random(1234)
    return [_query(rng, i) for i in range(100)]


def run(state):
    import sqlglot
    from sqlglot.optimizer import optimize

    out = []
    for i, sql in enumerate(state):
        tree = sqlglot.parse_one(sql, read="postgres")
        for dialect in ("duckdb", "bigquery", "snowflake"):
            out.append(tree.sql(dialect=dialect))
        if i % 4 == 0:
            out.append(optimize(tree, schema=SCHEMA, dialect="postgres").sql(dialect="duckdb"))
    return out


def digest(result):
    return hashlib.sha256("\n".join(result).encode()).hexdigest()
