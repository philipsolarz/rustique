"""jsonschema validating API documents against a Draft 2020-12 schema.

Stands for the reference JSON Schema implementation in Python (jsonschema, a top-50 PyPI
package pulled in by Jupyter, OpenAPI tooling, CI linters and many config loaders) doing
its everyday job: checking incoming documents against a schema with ``$defs``/``$ref``,
``allOf``/``oneOf``/``anyOf``, ``if``/``then``/``else``, ``patternProperties``,
``additionalProperties: false``, ``dependentRequired``, ``prefixItems``, ``uniqueItems``,
``enum``/``const``, numeric and string constraints and the stdlib-backed ``format``
checks (email, ipv4, ipv6, uuid, date, regex). Valid documents go through ``is_valid`` and
``validate``, invalid ones through ``iter_errors`` with ``best_match``, ``json_path`` and
messages (the error path is the expensive one in real use: config linters and API
gateways report every error). The ``referencing`` library resolves ``$ref``s (with
``rpds-py``, Rust, for its immutable maps). 2,500 documents per run (2,000 valid, 500
invalid), each a ~1-2 kB order/fulfilment document.
"""

REQUIRES = ["jsonschema==4.26.0", "jsonschema-specifications==2025.9.1", "referencing==0.37.0",
            "rpds-py==2026.9.1", "attrs==26.1.0", "typing-extensions==4.16.0"]

SCHEMA = {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "$id": "https://schemas.example.com/order.json",
    "title": "Order",
    "type": "object",
    "required": ["id", "customer", "lines", "payment", "status", "created"],
    "additionalProperties": False,
    "properties": {
        "id": {"type": "string", "format": "uuid"},
        "status": {"enum": ["pending", "paid", "packed", "shipped", "delivered", "cancelled"]},
        "created": {"type": "string", "format": "date"},
        "customer": {"$ref": "#/$defs/customer"},
        "lines": {"type": "array", "minItems": 1, "maxItems": 50, "items": {"$ref": "#/$defs/line"}},
        "payment": {"oneOf": [{"$ref": "#/$defs/card"}, {"$ref": "#/$defs/transfer"}, {"$ref": "#/$defs/voucher"}]},
        "shipping": {"$ref": "#/$defs/shipping"},
        "notes": {"type": "array", "items": {"type": "string", "maxLength": 500}, "uniqueItems": True},
        "metadata": {"type": "object", "patternProperties": {"^x-[a-z][a-z0-9-]*$": {"type": ["string", "number", "boolean"]}},
                     "additionalProperties": False},
        "client": {"type": "object", "properties": {"ip": {"anyOf": [{"format": "ipv4"}, {"format": "ipv6"}]},
                                                    "agent": {"type": "string"}}, "required": ["ip"]},
        "coordinates": {"type": "array", "prefixItems": [{"type": "number", "minimum": -90, "maximum": 90},
                                                         {"type": "number", "minimum": -180, "maximum": 180}],
                        "items": False},
    },
    "if": {"properties": {"status": {"enum": ["shipped", "delivered"]}}},
    "then": {"required": ["shipping"], "properties": {"shipping": {"required": ["tracking", "carrier"]}}},
    "dependentRequired": {"coordinates": ["shipping"]},
    "$defs": {
        "money": {"type": "object", "required": ["amount", "currency"], "additionalProperties": False,
                  "properties": {"amount": {"type": "number", "minimum": 0, "maximum": 1000000},
                                 "currency": {"type": "string", "pattern": "^[A-Z]{3}$"}}},
        "address": {"type": "object", "required": ["street", "city", "country"],
                    "properties": {"street": {"type": "string", "minLength": 1, "maxLength": 120},
                                   "city": {"type": "string", "minLength": 1},
                                   "postcode": {"type": "string", "pattern": "^[0-9A-Z -]{3,10}$"},
                                   "country": {"type": "string", "enum": ["DE", "FR", "PL", "US", "GB", "JP", "BR"]}}},
        "customer": {"type": "object", "required": ["id", "email", "name"], "additionalProperties": False,
                     "properties": {"id": {"type": "integer", "minimum": 1},
                                    "email": {"type": "string", "format": "email"},
                                    "name": {"type": "string", "minLength": 2, "maxLength": 80},
                                    "phone": {"type": "string", "pattern": "^\\+[1-9][0-9]{6,14}$"},
                                    "address": {"$ref": "#/$defs/address"},
                                    "segments": {"type": "array", "items": {"enum": ["vip", "b2b", "new", "churn-risk"]},
                                                 "uniqueItems": True}}},
        "line": {"type": "object", "required": ["sku", "qty", "price"], "additionalProperties": False,
                 "properties": {"sku": {"type": "string", "pattern": "^[A-Z]{3}-[0-9]{4,6}$"},
                                "qty": {"type": "integer", "minimum": 1, "maximum": 999},
                                "price": {"$ref": "#/$defs/money"},
                                "discount": {"type": "number", "exclusiveMinimum": 0, "exclusiveMaximum": 1},
                                "attributes": {"type": "object", "additionalProperties": {"type": "string"},
                                               "maxProperties": 8}}},
        "card": {"type": "object", "required": ["method", "brand", "last4"], "additionalProperties": False,
                 "properties": {"method": {"const": "card"}, "brand": {"enum": ["visa", "mastercard", "amex"]},
                                "last4": {"type": "string", "pattern": "^[0-9]{4}$"},
                                "expires": {"type": "string", "pattern": "^(0[1-9]|1[0-2])/[0-9]{2}$"}}},
        "transfer": {"type": "object", "required": ["method", "iban"], "additionalProperties": False,
                     "properties": {"method": {"const": "transfer"},
                                    "iban": {"type": "string", "pattern": "^[A-Z]{2}[0-9]{2}[A-Z0-9]{10,30}$"},
                                    "reference": {"type": "string", "maxLength": 35}}},
        "voucher": {"type": "object", "required": ["method", "code"], "additionalProperties": False,
                    "properties": {"method": {"const": "voucher"}, "code": {"type": "string", "minLength": 6},
                                   "value": {"$ref": "#/$defs/money"}}},
        "shipping": {"type": "object", "required": ["address", "service"],
                     "properties": {"address": {"$ref": "#/$defs/address"},
                                    "service": {"enum": ["standard", "express", "pickup"]},
                                    "carrier": {"type": "string"}, "tracking": {"type": "string", "minLength": 8},
                                    "cost": {"$ref": "#/$defs/money"}}},
    },
}


def setup():
    import copy
    import random
    import uuid

    from jsonschema import Draft202012Validator, FormatChecker

    validator = Draft202012Validator(SCHEMA, format_checker=FormatChecker())
    Draft202012Validator.check_schema(SCHEMA)
    rng = random.Random(4)
    statuses = ["pending", "paid", "packed", "shipped", "delivered", "cancelled"]

    def money(lo, hi):
        return {"amount": rng.randint(lo, hi) / 100, "currency": rng.choice(["EUR", "USD", "PLN"])}

    def address():
        return {"street": f"{rng.randint(1, 300)} Main Street", "city": rng.choice(["Berlin", "Lyon", "Gdansk"]),
                "postcode": f"{rng.randint(10000, 99999)}", "country": rng.choice(["DE", "FR", "PL", "US"])}

    def doc(i):
        st = statuses[i % 6]
        d = {"id": str(uuid.UUID(int=rng.getrandbits(128), version=4)), "status": st,
             "created": f"2025-0{1 + i % 9}-{1 + i % 28:02d}",
             "customer": {"id": 1 + i, "email": f"user{i}@example.com", "name": f"Customer {i}",
                          "phone": f"+4930{rng.randint(1000000, 9999999)}", "address": address(),
                          "segments": rng.sample(["vip", "b2b", "new", "churn-risk"], rng.randint(0, 2))},
             "lines": [{"sku": f"{rng.choice(['ABC', 'XYZ', 'QRS'])}-{rng.randint(1000, 999999)}",
                        "qty": rng.randint(1, 5), "price": money(100, 50000),
                        **({"discount": 0.15} if rng.random() < 0.2 else {}),
                        **({"attributes": {"color": "red", "size": "M"}} if rng.random() < 0.3 else {})}
                       for _ in range(rng.randint(1, 8))],
             "payment": [{"method": "card", "brand": "visa", "last4": f"{rng.randint(0, 9999):04d}", "expires": "09/27"},
                         {"method": "transfer", "iban": f"DE{rng.randint(10, 99)}{rng.randint(10 ** 17, 10 ** 18 - 1)}",
                          "reference": f"INV-{i}"},
                         {"method": "voucher", "code": f"GIFT{i:06d}", "value": money(500, 5000)}][i % 3],
             "notes": [f"note {j}" for j in range(rng.randint(0, 3))],
             "metadata": {"x-source": "web", "x-ab-bucket": i % 4, "x-beta": i % 2 == 0},
             "client": {"ip": f"10.{i % 256}.{(i * 7) % 256}.{(i * 13) % 256}" if i % 4 else f"2001:db8::{i % 9999:x}",
                        "agent": "Mozilla/5.0"}}
        if st in ("shipped", "delivered") or i % 5 == 0:
            d["shipping"] = {"address": address(), "service": rng.choice(["standard", "express"]),
                             "carrier": "dhl", "tracking": f"JD{i:012d}", "cost": money(0, 2000)}
            d["coordinates"] = [round(rng.uniform(-90, 90), 4), round(rng.uniform(-180, 180), 4)]
        return d

    valid = [doc(i) for i in range(2000)]
    invalid = []
    for j in range(500):
        d = copy.deepcopy(valid[(j * 7) % 2000])
        m = j % 10
        if m == 0:
            d["customer"]["email"] = "not-an-email"
        elif m == 1:
            d["lines"][0]["qty"] = 0
            d["lines"][0]["sku"] = "bad"
        elif m == 2:
            d["payment"] = {"method": "card", "iban": "DE00"}
        elif m == 3:
            d["status"] = "shipped"
            d.pop("shipping", None)
        elif m == 4:
            d["unknown"] = 1
            d["metadata"]["Bad-Key"] = "x"
        elif m == 5:
            d["client"]["ip"] = "999.1.1.1"
        elif m == 6:
            d["notes"] = ["a", "a"]
            d["coordinates"] = [100, 0, 3]
        elif m == 7:
            d["lines"] = []
        elif m == 8:
            d["created"] = "2025-13-45"
            d["id"] = "1234"
        else:
            d["lines"][0]["price"]["amount"] = -1.005
            d["customer"].pop("name")
        invalid.append(d)
    return {"validator": validator, "valid": valid, "invalid": invalid}


def run(state):
    from jsonschema import ValidationError
    from jsonschema.exceptions import best_match

    v = state["validator"]
    ok = 0
    for i, d in enumerate(state["valid"]):
        if i % 2:
            ok += v.is_valid(d)
        else:
            try:
                v.validate(d)
                ok += 1
            except ValidationError:
                pass
    reports = []
    for d in state["invalid"]:
        errs = sorted(v.iter_errors(d), key=lambda e: e.json_path)
        best = best_match(v.iter_errors(d))
        reports.append((len(errs), [(e.json_path, e.validator, e.message[:80]) for e in errs],
                        best.json_path if best else None))
    return ok, reports


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
