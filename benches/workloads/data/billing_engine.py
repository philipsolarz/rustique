"""Monthly invoicing run: rate metered usage against tiered price plans with ``Decimal``
money math, prorate mid-cycle plan changes with ``Fraction``, apply discounts and
regional tax, convert currencies, and summarise the run with ``statistics``.

Stands for billing, payroll, accounting and pricing scripts, where floats are not
allowed: ``decimal.Decimal`` construction from strings, arithmetic under a local
context, ``quantize(ROUND_HALF_UP)``; ``fractions.Fraction`` proration; ``enum.Enum``/
``IntFlag`` and frozen/slotted ``dataclasses`` for the domain; ``statistics`` (mean,
median, stdev, quantiles) over the invoice totals; money formatting with
``f"{x:,.2f}"``. Data shape: 40,000 customers x 1-6 meters, ~140k usage lines.
"""

import enum
from dataclasses import dataclass, field
from decimal import Decimal

REQUIRES: list[str] = []

N_CUSTOMERS = 40_000


class Region(enum.Enum):
    EU = "EU"
    US = "US"
    UK = "UK"
    CH = "CH"
    APAC = "APAC"


class Flag(enum.IntFlag):
    TAX_EXEMPT = 1
    NONPROFIT = 2
    LEGACY = 4
    INVOICE_BY_POST = 8


@dataclass(frozen=True, slots=True)
class Tier:
    up_to: Decimal | None
    unit: Decimal


@dataclass(slots=True)
class Line:
    meter: str
    quantity: Decimal
    amount: Decimal


@dataclass(slots=True)
class Invoice:
    customer: str
    currency: str
    lines: list = field(default_factory=list)
    subtotal: Decimal = Decimal("0")
    discount: Decimal = Decimal("0")
    tax: Decimal = Decimal("0")
    total: Decimal = Decimal("0")


def setup():
    import random

    rng = random.Random(77)
    customers = []
    for c in range(N_CUSTOMERS):
        meters = []
        for _ in range(rng.randint(1, 6)):
            meters.append((rng.choice(["api_calls", "storage_gb", "seats", "egress_gb",
                                       "builds"]),
                           f"{rng.uniform(0, 250_000):.3f}"))
        customers.append({
            "id": f"C{c:05d}",
            "plan": rng.choice(["free", "starter", "team", "business", "enterprise"]),
            "region": rng.choice(["EU", "EU", "US", "UK", "CH", "APAC"]),
            "currency": rng.choice(["EUR", "USD", "GBP", "CHF"]),
            "change_day": rng.choice([None] * 6 + list(range(2, 29))),
            "new_plan": rng.choice(["starter", "team", "business"]),
            "coupon": rng.choice([None, None, None, "WELCOME10", "PARTNER25", "FIXED50"]),
            "flags": rng.getrandbits(4),
            "usage": meters,
        })
    return {"customers": customers}


def run(state):
    import statistics
    from decimal import ROUND_HALF_UP, Decimal, localcontext
    from fractions import Fraction

    D = Decimal
    CENT = D("0.01")
    base_fee = {"free": D("0"), "starter": D("19.00"), "team": D("99.00"),
                "business": D("499.00"), "enterprise": D("2490.00")}
    tiers = {
        "api_calls": [Tier(D("10000"), D("0")), Tier(D("100000"), D("0.00042")),
                      Tier(None, D("0.00018"))],
        "storage_gb": [Tier(D("50"), D("0")), Tier(None, D("0.023"))],
        "seats": [Tier(D("5"), D("0")), Tier(D("50"), D("8.00")), Tier(None, D("6.50"))],
        "egress_gb": [Tier(D("100"), D("0")), Tier(D("10000"), D("0.085")),
                      Tier(None, D("0.05"))],
        "builds": [Tier(D("300"), D("0")), Tier(None, D("0.008"))],
    }
    tax_rate = {Region.EU: D("0.21"), Region.US: D("0.0725"), Region.UK: D("0.20"),
                Region.CH: D("0.081"), Region.APAC: D("0.10")}
    fx = {"EUR": D("1"), "USD": D("1.0841"), "GBP": D("0.8512"), "CHF": D("0.9433")}
    days_in_month = 30

    def rate(meter, qty):
        remaining = qty
        prev = D("0")
        amount = D("0")
        for t in tiers[meter]:
            span = remaining if t.up_to is None else min(remaining, t.up_to - prev)
            if span <= 0:
                break
            amount += span * t.unit
            remaining -= span
            if t.up_to is not None:
                prev = t.up_to
        return amount

    invoices = []
    with localcontext() as ctx:
        ctx.prec = 28
        for c in state["customers"]:
            inv = Invoice(c["id"], c["currency"])
            region = Region(c["region"])
            flags = Flag(c["flags"])
            fee = base_fee[c["plan"]]
            if c["change_day"]:
                used = Fraction(c["change_day"] - 1, days_in_month)
                prorated = (fee * D(used.numerator) / D(used.denominator)
                            + base_fee[c["new_plan"]] * D((1 - used).numerator)
                            / D((1 - used).denominator))
                fee = prorated
            inv.lines.append(Line("base", D("1"), fee.quantize(CENT, ROUND_HALF_UP)))
            for meter, qty_txt in c["usage"]:
                qty = D(qty_txt)
                amt = rate(meter, qty).quantize(CENT, ROUND_HALF_UP)
                inv.lines.append(Line(meter, qty, amt))
            inv.subtotal = sum((ln.amount for ln in inv.lines), D("0"))
            coupon = c["coupon"]
            if coupon == "WELCOME10":
                inv.discount = (inv.subtotal * D("0.10")).quantize(CENT, ROUND_HALF_UP)
            elif coupon == "PARTNER25":
                inv.discount = (inv.subtotal * D("0.25")).quantize(CENT, ROUND_HALF_UP)
            elif coupon == "FIXED50":
                inv.discount = min(D("50.00"), inv.subtotal)
            if Flag.NONPROFIT in flags:
                inv.discount += ((inv.subtotal - inv.discount) * D("0.15")).quantize(
                    CENT, ROUND_HALF_UP)
            taxable = inv.subtotal - inv.discount
            if Flag.TAX_EXEMPT not in flags:
                inv.tax = (taxable * tax_rate[region]).quantize(CENT, ROUND_HALF_UP)
            total_eur = taxable + inv.tax
            if Flag.INVOICE_BY_POST in flags:
                total_eur += D("1.50")
            inv.total = (total_eur * fx[inv.currency]).quantize(CENT, ROUND_HALF_UP)
            invoices.append(inv)

    totals = [inv.total for inv in invoices if inv.total > 0]
    totals_f = [float(t) for t in totals]
    by_cur = {}
    for inv in invoices:
        by_cur[inv.currency] = by_cur.get(inv.currency, D("0")) + inv.total
    lines = [f"{inv.customer} {inv.currency} {inv.total:>12,.2f} "
             f"(sub {inv.subtotal:,.2f}, disc {inv.discount:,.2f}, tax {inv.tax:,.2f})"
             for inv in invoices[:400]]
    return {
        "count": len(invoices),
        "by_currency": {k: str(v) for k, v in sorted(by_cur.items())},
        "mean": str(statistics.mean(totals)),
        "median": str(statistics.median(totals)),
        "stdev": round(statistics.stdev(totals_f), 6),
        "quartiles": [round(q, 6) for q in statistics.quantiles(totals_f, n=4)],
        "fmean": round(statistics.fmean(totals_f), 6),
        "lines": lines,
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
