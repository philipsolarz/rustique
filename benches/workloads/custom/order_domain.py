"""Order management back office: an e-commerce domain model processing a day of commands.

Stands for the in-house "domain layer" of a shop, ERP or booking system written in the
DDD style that most modern Python business code follows: frozen/slotted value objects
(``@dataclass(frozen=True, slots=True, order=True)`` ``Money`` over ``Decimal``), entities
with ``__post_init__`` validation and computed ``@property`` totals, ``StrEnum`` /
``IntEnum`` / ``Enum`` / ``Flag`` for statuses, priorities, channels and customer flags, a
status state machine, promotion strategies behind an ABC, repositories with dict and
``defaultdict`` indexes, domain events (an inheritance tree of frozen keyword-only
dataclasses) published on an in-process bus whose handlers maintain read models, and an
application service that turns command tuples into calls and domain errors into rejections.

Data shape: 1,500 products in 12 categories, 4,000 customers, a stream of ~98k commands
(10k orders: create, add/remove lines, coupons, place, pay, pack, ship, deliver, cancel,
refund; ~7% of commands are rejected by domain rules).
"""

import abc
import enum
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from datetime import date, timedelta
from decimal import ROUND_HALF_EVEN, Decimal

REQUIRES: list[str] = []

N_PRODUCTS = 1_500
N_CUSTOMERS = 4_000
N_ORDERS = 10_000

CENT = Decimal("0.01")
ZERO = Decimal("0")
CURRENCIES = frozenset({"EUR", "USD", "GBP", "SEK"})
FX_TO_EUR = {"EUR": Decimal("1"), "USD": Decimal("0.9224"), "GBP": Decimal("1.1748"),
             "SEK": Decimal("0.0871")}


# --------------------------------------------------------------------------------------
# enums
# --------------------------------------------------------------------------------------


class OrderStatus(enum.StrEnum):
    DRAFT = enum.auto()
    PLACED = enum.auto()
    PAID = enum.auto()
    PACKED = enum.auto()
    SHIPPED = enum.auto()
    DELIVERED = enum.auto()
    CANCELLED = enum.auto()
    REFUNDED = enum.auto()


class Priority(enum.IntEnum):
    LOW = 0
    NORMAL = 1
    HIGH = 2
    EXPRESS = 3


class Channel(enum.Enum):
    WEB = "web"
    MOBILE = "mobile"
    PHONE = "phone"
    MARKETPLACE = "marketplace"


class CustomerFlag(enum.Flag):
    NONE = 0
    VIP = enum.auto()
    WHOLESALE = enum.auto()
    TAX_EXEMPT = enum.auto()
    NEWSLETTER = enum.auto()
    FRAUD_REVIEW = enum.auto()


class TaxClass(enum.Enum):
    STANDARD = "standard"
    REDUCED = "reduced"
    ZERO = "zero"


ALLOWED_TRANSITIONS: dict[OrderStatus, frozenset[OrderStatus]] = {
    OrderStatus.DRAFT: frozenset({OrderStatus.PLACED, OrderStatus.CANCELLED}),
    OrderStatus.PLACED: frozenset({OrderStatus.PAID, OrderStatus.CANCELLED}),
    OrderStatus.PAID: frozenset({OrderStatus.PACKED, OrderStatus.REFUNDED}),
    OrderStatus.PACKED: frozenset({OrderStatus.SHIPPED, OrderStatus.REFUNDED}),
    OrderStatus.SHIPPED: frozenset({OrderStatus.DELIVERED}),
    OrderStatus.DELIVERED: frozenset({OrderStatus.REFUNDED}),
    OrderStatus.CANCELLED: frozenset(),
    OrderStatus.REFUNDED: frozenset(),
}

TAX_RATES: dict[str, dict[TaxClass, Decimal]] = {
    "DE": {TaxClass.STANDARD: Decimal("0.19"), TaxClass.REDUCED: Decimal("0.07")},
    "FR": {TaxClass.STANDARD: Decimal("0.20"), TaxClass.REDUCED: Decimal("0.055")},
    "SE": {TaxClass.STANDARD: Decimal("0.25"), TaxClass.REDUCED: Decimal("0.12")},
    "GB": {TaxClass.STANDARD: Decimal("0.20"), TaxClass.REDUCED: Decimal("0.05")},
    "US": {TaxClass.STANDARD: Decimal("0.0725"), TaxClass.REDUCED: Decimal("0.0725")},
    "NL": {TaxClass.STANDARD: Decimal("0.21"), TaxClass.REDUCED: Decimal("0.09")},
}


# --------------------------------------------------------------------------------------
# errors
# --------------------------------------------------------------------------------------


class DomainError(Exception):
    code = "domain"


class InvalidTransition(DomainError):
    code = "transition"


class OutOfStock(DomainError):
    code = "stock"


class CouponRejected(DomainError):
    code = "coupon"


class PaymentMismatch(DomainError):
    code = "payment"


class NotFound(DomainError):
    code = "not_found"


# --------------------------------------------------------------------------------------
# value objects
# --------------------------------------------------------------------------------------


@dataclass(frozen=True, slots=True, order=True)
class Money:
    amount: Decimal
    currency: str = "EUR"

    def __post_init__(self) -> None:
        if not isinstance(self.amount, Decimal):
            raise TypeError(f"amount must be Decimal, not {type(self.amount).__name__}")
        if self.currency not in CURRENCIES:
            raise ValueError(f"unknown currency {self.currency!r}")
        quantized = self.amount.quantize(CENT, ROUND_HALF_EVEN)
        if quantized != self.amount:
            object.__setattr__(self, "amount", quantized)

    @classmethod
    def zero(cls, currency: str = "EUR") -> "Money":
        return cls(ZERO, currency)

    def _same(self, other: "Money") -> None:
        if other.currency != self.currency:
            raise ValueError(f"currency mismatch: {self.currency} vs {other.currency}")

    def __add__(self, other: object) -> "Money":
        if not isinstance(other, Money):
            return NotImplemented
        self._same(other)
        return Money(self.amount + other.amount, self.currency)

    def __sub__(self, other: object) -> "Money":
        if not isinstance(other, Money):
            return NotImplemented
        self._same(other)
        return Money(self.amount - other.amount, self.currency)

    def __mul__(self, factor: object) -> "Money":
        if isinstance(factor, (int, Decimal)):
            return Money(self.amount * factor, self.currency)
        return NotImplemented

    __rmul__ = __mul__

    def __neg__(self) -> "Money":
        return Money(-self.amount, self.currency)

    def __bool__(self) -> bool:
        return bool(self.amount)

    def to_eur(self) -> "Money":
        return Money(self.amount * FX_TO_EUR[self.currency], "EUR")

    def allocate(self, weights: list[int]) -> list["Money"]:
        """Split into parts proportional to ``weights``; the cents left over go first-come."""
        total = sum(weights)
        cents = int(self.amount / CENT)
        parts = [cents * w // total for w in weights]
        for i in range(cents - sum(parts)):
            parts[i % len(parts)] += 1
        return [Money(Decimal(p) * CENT, self.currency) for p in parts]

    def __str__(self) -> str:
        return f"{self.amount:,.2f} {self.currency}"


def money_sum(items, currency: str) -> Money:
    total = Money.zero(currency)
    for m in items:
        total = total + m
    return total


@dataclass(frozen=True, slots=True)
class Address:
    street: str
    city: str
    postcode: str
    country: str

    def __post_init__(self) -> None:
        if self.country not in TAX_RATES:
            raise ValueError(f"we do not ship to {self.country}")


# --------------------------------------------------------------------------------------
# entities
# --------------------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class Product:
    sku: str
    name: str
    category: str
    price: Money
    weight_g: int
    tax_class: TaxClass = TaxClass.STANDARD

    def __post_init__(self) -> None:
        if self.weight_g <= 0:
            raise ValueError(f"{self.sku}: weight must be positive")


@dataclass(slots=True)
class Customer:
    id: str
    name: str
    address: Address
    currency: str
    flags: CustomerFlag = CustomerFlag.NONE
    since: date = date(2020, 1, 1)

    @property
    def is_vip(self) -> bool:
        return CustomerFlag.VIP in self.flags

    @property
    def tax_exempt(self) -> bool:
        return CustomerFlag.TAX_EXEMPT in self.flags or CustomerFlag.WHOLESALE in self.flags

    @property
    def needs_review(self) -> bool:
        return bool(self.flags & CustomerFlag.FRAUD_REVIEW)


@dataclass(slots=True)
class OrderLine:
    product: Product
    quantity: int
    unit_price: Money

    def __post_init__(self) -> None:
        if self.quantity <= 0:
            raise ValueError("quantity must be positive")

    @property
    def total(self) -> Money:
        return self.unit_price * self.quantity

    @property
    def weight_g(self) -> int:
        return self.product.weight_g * self.quantity


# --------------------------------------------------------------------------------------
# domain events
# --------------------------------------------------------------------------------------


@dataclass(frozen=True, slots=True, kw_only=True)
class DomainEvent:
    order_id: str
    day: date


@dataclass(frozen=True, slots=True, kw_only=True)
class LineAdded(DomainEvent):
    sku: str
    quantity: int


@dataclass(frozen=True, slots=True, kw_only=True)
class LineRemoved(DomainEvent):
    sku: str
    quantity: int


@dataclass(frozen=True, slots=True, kw_only=True)
class CouponApplied(DomainEvent):
    code: str


@dataclass(frozen=True, slots=True, kw_only=True)
class StatusChanged(DomainEvent):
    old: OrderStatus
    new: OrderStatus


@dataclass(frozen=True, slots=True, kw_only=True)
class OrderPlaced(StatusChanged):
    total: Money


@dataclass(frozen=True, slots=True, kw_only=True)
class OrderPaid(StatusChanged):
    amount: Money


@dataclass(frozen=True, slots=True, kw_only=True)
class OrderShipped(StatusChanged):
    carrier: str
    weight_g: int


@dataclass(frozen=True, slots=True, kw_only=True)
class OrderCancelled(StatusChanged):
    reason: str


@dataclass(frozen=True, slots=True, kw_only=True)
class OrderRefunded(StatusChanged):
    amount: Money


# --------------------------------------------------------------------------------------
# promotions
# --------------------------------------------------------------------------------------


class Promotion(abc.ABC):
    def __init__(self, code: str) -> None:
        self.code = code

    @abc.abstractmethod
    def discount_for(self, order: "Order") -> Money: ...

    def check(self, order: "Order") -> None:
        if not order.lines:
            raise CouponRejected(f"{self.code}: empty order")


class PercentOff(Promotion):
    def __init__(self, code: str, percent: int, min_subtotal: Decimal) -> None:
        super().__init__(code)
        self.rate = Decimal(percent) / 100
        self.min_subtotal = min_subtotal

    def check(self, order: "Order") -> None:
        super().check(order)
        if order.subtotal.to_eur().amount < self.min_subtotal:
            raise CouponRejected(f"{self.code}: subtotal below {self.min_subtotal}")

    def discount_for(self, order: "Order") -> Money:
        return order.subtotal * self.rate


class FixedOff(Promotion):
    def __init__(self, code: str, amount_eur: Decimal) -> None:
        super().__init__(code)
        self.amount_eur = amount_eur

    def discount_for(self, order: "Order") -> Money:
        cap = order.subtotal
        off = Money(self.amount_eur / FX_TO_EUR[order.currency], order.currency)
        return min(off, cap)


class CategoryDeal(Promotion):
    """Buy ``n`` of a category, the cheapest one is free."""

    def __init__(self, code: str, category: str, n: int) -> None:
        super().__init__(code)
        self.category = category
        self.n = n

    def check(self, order: "Order") -> None:
        super().check(order)
        if sum(ln.quantity for ln in order.lines if ln.product.category == self.category) < self.n:
            raise CouponRejected(f"{self.code}: needs {self.n} x {self.category}")

    def discount_for(self, order: "Order") -> Money:
        prices = sorted(ln.unit_price for ln in order.lines if ln.product.category == self.category)
        return prices[0] if prices else Money.zero(order.currency)


class FreeShipping(Promotion):
    def discount_for(self, order: "Order") -> Money:
        return order.shipping


# --------------------------------------------------------------------------------------
# the aggregate
# --------------------------------------------------------------------------------------


SHIPPING_TIERS = [(500, Decimal("3.90")), (2_000, Decimal("5.90")), (10_000, Decimal("9.90")),
                  (30_000, Decimal("19.90"))]
PRIORITY_SURCHARGE = {Priority.LOW: Decimal("0"), Priority.NORMAL: Decimal("0"),
                      Priority.HIGH: Decimal("4.00"), Priority.EXPRESS: Decimal("12.00")}


@dataclass(eq=False)
class Order:
    id: str
    customer: Customer
    channel: Channel
    priority: Priority
    opened: date
    lines: list[OrderLine] = field(default_factory=list)
    promotions: list[Promotion] = field(default_factory=list)
    status: OrderStatus = OrderStatus.DRAFT
    paid: Money | None = None
    pending_events: list[DomainEvent] = field(default_factory=list, repr=False)

    def __post_init__(self) -> None:
        if not self.id.startswith("O-"):
            raise ValueError(f"bad order id {self.id!r}")

    # -- computed values ---------------------------------------------------------------
    @property
    def currency(self) -> str:
        return self.customer.currency

    @property
    def subtotal(self) -> Money:
        return money_sum((ln.total for ln in self.lines), self.currency)

    @property
    def weight_g(self) -> int:
        return sum(ln.weight_g for ln in self.lines)

    @property
    def shipping(self) -> Money:
        w = self.weight_g
        for limit, price in SHIPPING_TIERS:
            if w <= limit:
                base = price
                break
        else:
            base = Decimal("49.00")
        base += PRIORITY_SURCHARGE[self.priority]
        return Money(base / FX_TO_EUR[self.currency], self.currency)

    @property
    def discount(self) -> Money:
        total = Money.zero(self.currency)
        for promo in self.promotions:
            total = total + promo.discount_for(self)
        return min(total, self.subtotal + self.shipping)

    @property
    def tax(self) -> Money:
        if self.customer.tax_exempt:
            return Money.zero(self.currency)
        rates = TAX_RATES[self.customer.address.country]
        taxable = [ln.total for ln in self.lines]
        if not taxable:
            return Money.zero(self.currency)
        # the discount is spread over the lines in proportion to their value
        shares = self.discount.allocate([max(int(t.amount * 100), 1) for t in taxable])
        tax = Money.zero(self.currency)
        for ln, value, share in zip(self.lines, taxable, shares):
            tax = tax + (value - share) * rates[ln.product.tax_class]
        return tax

    @property
    def total(self) -> Money:
        return self.subtotal + self.shipping - self.discount + self.tax

    # -- behaviour ---------------------------------------------------------------------
    def _require(self, *states: OrderStatus) -> None:
        if self.status not in states:
            raise InvalidTransition(f"{self.id}: {self.status} not in {[str(s) for s in states]}")

    def _transition(self, new: OrderStatus, day: date, event_type=StatusChanged, **extra) -> None:
        if new not in ALLOWED_TRANSITIONS[self.status]:
            raise InvalidTransition(f"{self.id}: {self.status} -> {new}")
        old, self.status = self.status, new
        self.pending_events.append(event_type(order_id=self.id, day=day, old=old, new=new, **extra))

    def add_line(self, product: Product, quantity: int, day: date) -> None:
        self._require(OrderStatus.DRAFT)
        for i, ln in enumerate(self.lines):
            if ln.product.sku == product.sku:
                self.lines[i] = OrderLine(product, ln.quantity + quantity, ln.unit_price)
                break
        else:
            price = product.price
            if price.currency != self.currency:
                price = Money(price.amount / FX_TO_EUR[self.currency], self.currency)
            if self.customer.flags & CustomerFlag.WHOLESALE:
                price = price * Decimal("0.85")
            self.lines.append(OrderLine(product, quantity, price))
        self.pending_events.append(LineAdded(order_id=self.id, day=day, sku=product.sku,
                                             quantity=quantity))

    def remove_line(self, sku: str, day: date) -> None:
        self._require(OrderStatus.DRAFT)
        for i, ln in enumerate(self.lines):
            if ln.product.sku == sku:
                del self.lines[i]
                self.pending_events.append(LineRemoved(order_id=self.id, day=day, sku=sku,
                                                       quantity=ln.quantity))
                return
        raise NotFound(f"{self.id}: no line {sku}")

    def apply(self, promo: Promotion, day: date) -> None:
        self._require(OrderStatus.DRAFT)
        if any(p.code == promo.code for p in self.promotions):
            raise CouponRejected(f"{promo.code} already applied")
        if len(self.promotions) >= 2:
            raise CouponRejected("at most two coupons per order")
        promo.check(self)
        self.promotions.append(promo)
        self.pending_events.append(CouponApplied(order_id=self.id, day=day, code=promo.code))

    def place(self, day: date) -> None:
        if not self.lines:
            raise DomainError(f"{self.id}: cannot place an empty order")
        if self.customer.needs_review and self.subtotal.to_eur().amount > 800:
            raise DomainError(f"{self.id}: held for fraud review")
        self._transition(OrderStatus.PLACED, day, OrderPlaced, total=self.total)

    def pay(self, amount: Money, day: date) -> None:
        if amount != self.total:
            raise PaymentMismatch(f"{self.id}: paid {amount}, due {self.total}")
        self.paid = amount
        self._transition(OrderStatus.PAID, day, OrderPaid, amount=amount)

    def pack(self, day: date) -> None:
        self._transition(OrderStatus.PACKED, day)

    def ship(self, carrier: str, day: date) -> None:
        self._transition(OrderStatus.SHIPPED, day, OrderShipped, carrier=carrier,
                         weight_g=self.weight_g)

    def deliver(self, day: date) -> None:
        self._transition(OrderStatus.DELIVERED, day)

    def cancel(self, reason: str, day: date) -> None:
        self._transition(OrderStatus.CANCELLED, day, OrderCancelled, reason=reason)

    def refund(self, day: date) -> None:
        if self.paid is None:
            raise PaymentMismatch(f"{self.id}: nothing to refund")
        self._transition(OrderStatus.REFUNDED, day, OrderRefunded, amount=self.paid)

    def collect_events(self) -> list[DomainEvent]:
        events, self.pending_events = self.pending_events, []
        return events


# --------------------------------------------------------------------------------------
# repositories
# --------------------------------------------------------------------------------------


class ProductCatalog:
    def __init__(self) -> None:
        self._by_sku: dict[str, Product] = {}
        self._by_category: defaultdict[str, list[Product]] = defaultdict(list)

    def add(self, product: Product) -> None:
        if product.sku in self._by_sku:
            raise ValueError(f"duplicate sku {product.sku}")
        self._by_sku[product.sku] = product
        self._by_category[product.category].append(product)

    def get(self, sku: str) -> Product:
        try:
            return self._by_sku[sku]
        except KeyError:
            raise NotFound(f"no product {sku}") from None

    def categories(self) -> list[str]:
        return sorted(self._by_category)

    def cheapest(self, category: str, n: int) -> list[Product]:
        return sorted(self._by_category[category], key=lambda p: (p.price.to_eur(), p.sku))[:n]


class OrderRepository:
    def __init__(self) -> None:
        self._orders: dict[str, Order] = {}
        self._by_customer: defaultdict[str, list[str]] = defaultdict(list)
        self._by_status: defaultdict[OrderStatus, set[str]] = defaultdict(set)

    def add(self, order: Order) -> None:
        self._orders[order.id] = order
        self._by_customer[order.customer.id].append(order.id)
        self._by_status[order.status].add(order.id)

    def get(self, order_id: str) -> Order:
        order = self._orders.get(order_id)
        if order is None:
            raise NotFound(f"no order {order_id}")
        return order

    def reindex(self, order: Order, old: OrderStatus) -> None:
        if old is not order.status:
            self._by_status[old].discard(order.id)
            self._by_status[order.status].add(order.id)

    def with_status(self, status: OrderStatus) -> list[Order]:
        return [self._orders[i] for i in sorted(self._by_status[status])]

    def for_customer(self, customer_id: str) -> list[Order]:
        return [self._orders[i] for i in self._by_customer[customer_id]]

    def __len__(self) -> int:
        return len(self._orders)


class Inventory:
    def __init__(self, stock: dict[str, int]) -> None:
        self.on_hand = dict(stock)
        self.reserved: Counter[str] = Counter()

    def available(self, sku: str) -> int:
        return self.on_hand.get(sku, 0) - self.reserved[sku]

    def reserve(self, order: Order) -> None:
        short = [ln.product.sku for ln in order.lines if self.available(ln.product.sku) < ln.quantity]
        if short:
            raise OutOfStock(f"{order.id}: short on {', '.join(short)}")
        for ln in order.lines:
            self.reserved[ln.product.sku] += ln.quantity

    def release(self, order: Order) -> None:
        for ln in order.lines:
            self.reserved[ln.product.sku] -= ln.quantity

    def commit(self, order: Order) -> None:
        for ln in order.lines:
            self.reserved[ln.product.sku] -= ln.quantity
            self.on_hand[ln.product.sku] -= ln.quantity


# --------------------------------------------------------------------------------------
# event bus and read models
# --------------------------------------------------------------------------------------


class EventBus:
    def __init__(self) -> None:
        self._handlers: defaultdict[type, list] = defaultdict(list)
        self.published: Counter[str] = Counter()

    def subscribe(self, event_type: type, handler) -> None:
        self._handlers[event_type].append(handler)

    def publish(self, event: DomainEvent) -> None:
        self.published[type(event).__name__] += 1
        for klass in type(event).__mro__:
            for handler in self._handlers.get(klass, ()):
                handler(event)


class SalesReport:
    def __init__(self, orders: OrderRepository) -> None:
        self.orders = orders
        self.revenue_by_category: defaultdict[str, Money] = defaultdict(Money.zero)
        self.units_by_sku: Counter[str] = Counter()
        self.revenue_by_day: defaultdict[date, Money] = defaultdict(Money.zero)
        self.refunds = Money.zero()

    def on_paid(self, event: OrderPaid) -> None:
        order = self.orders.get(event.order_id)
        for ln in order.lines:
            self.revenue_by_category[ln.product.category] += ln.total.to_eur()
            self.units_by_sku[ln.product.sku] += ln.quantity
        self.revenue_by_day[event.day] += event.amount.to_eur()

    def on_refunded(self, event: OrderRefunded) -> None:
        self.refunds += event.amount.to_eur()


class CustomerValue:
    def __init__(self) -> None:
        self.lifetime: defaultdict[str, Money] = defaultdict(Money.zero)
        self.orders: Counter[str] = Counter()

    def __call__(self, event: DomainEvent) -> None:
        match event:
            case OrderPaid(amount=amount):
                cid = event.order_id.split("-")[2]
                self.lifetime[cid] += amount.to_eur()
                self.orders[cid] += 1
            case OrderRefunded(amount=amount):
                cid = event.order_id.split("-")[2]
                self.lifetime[cid] -= amount.to_eur()


class StatusTimeline:
    def __init__(self) -> None:
        self.transitions: Counter[tuple[str, str]] = Counter()
        self.cancel_reasons: Counter[str] = Counter()
        self.shipped_weight: defaultdict[str, int] = defaultdict(int)

    def on_status(self, event: StatusChanged) -> None:
        self.transitions[event.old.value, event.new.value] += 1
        if isinstance(event, OrderCancelled):
            self.cancel_reasons[event.reason] += 1
        elif isinstance(event, OrderShipped):
            self.shipped_weight[event.carrier] += event.weight_g


# --------------------------------------------------------------------------------------
# application service
# --------------------------------------------------------------------------------------


class OrderService:
    def __init__(self, catalog: ProductCatalog, customers: dict[str, Customer],
                 promotions: dict[str, Promotion], inventory: Inventory) -> None:
        self.catalog = catalog
        self.customers = customers
        self.promotions = promotions
        self.inventory = inventory
        self.orders = OrderRepository()
        self.bus = EventBus()
        self.rejected: Counter[str] = Counter()
        self.handlers = {
            "create": self.create, "add": self.add, "remove": self.remove,
            "coupon": self.coupon, "place": self.place, "pay": self.pay, "pack": self.pack,
            "ship": self.ship, "deliver": self.deliver, "cancel": self.cancel,
            "refund": self.refund,
        }

    def handle(self, command: tuple) -> None:
        name, day, order_id, *args = command
        try:
            self.handlers[name](order_id, day, *args)
        except DomainError as exc:
            self.rejected[f"{name}:{exc.code}"] += 1

    def _save(self, order: Order, old: OrderStatus) -> None:
        self.orders.reindex(order, old)
        for event in order.collect_events():
            self.bus.publish(event)

    def create(self, order_id: str, day: date, customer_id: str, channel: str, prio: int) -> None:
        customer = self.customers.get(customer_id)
        if customer is None:
            raise NotFound(customer_id)
        order = Order(order_id, customer, Channel(channel), Priority(prio), day)
        self.orders.add(order)

    def add(self, order_id: str, day: date, sku: str, qty: int) -> None:
        order = self.orders.get(order_id)
        order.add_line(self.catalog.get(sku), qty, day)
        self._save(order, order.status)

    def remove(self, order_id: str, day: date, sku: str) -> None:
        order = self.orders.get(order_id)
        order.remove_line(sku, day)
        self._save(order, order.status)

    def coupon(self, order_id: str, day: date, code: str) -> None:
        order = self.orders.get(order_id)
        promo = self.promotions.get(code)
        if promo is None:
            raise CouponRejected(f"unknown coupon {code}")
        order.apply(promo, day)
        self._save(order, order.status)

    def place(self, order_id: str, day: date) -> None:
        order = self.orders.get(order_id)
        old = order.status
        self.inventory.reserve(order)
        try:
            order.place(day)
        except DomainError:
            self.inventory.release(order)
            raise
        self._save(order, old)

    def pay(self, order_id: str, day: date, short: bool) -> None:
        order = self.orders.get(order_id)
        old = order.status
        amount = order.total
        if short:
            amount = amount - Money(Decimal("0.01"), amount.currency)
        order.pay(amount, day)
        self._save(order, old)

    def pack(self, order_id: str, day: date) -> None:
        order = self.orders.get(order_id)
        old = order.status
        order.pack(day)
        self.inventory.commit(order)
        self._save(order, old)

    def ship(self, order_id: str, day: date, carrier: str) -> None:
        order = self.orders.get(order_id)
        old = order.status
        order.ship(carrier, day)
        self._save(order, old)

    def deliver(self, order_id: str, day: date) -> None:
        order = self.orders.get(order_id)
        old = order.status
        order.deliver(day)
        self._save(order, old)

    def cancel(self, order_id: str, day: date, reason: str) -> None:
        order = self.orders.get(order_id)
        old = order.status
        order.cancel(reason, day)
        if old is OrderStatus.PLACED:
            self.inventory.release(order)
        self._save(order, old)

    def refund(self, order_id: str, day: date) -> None:
        order = self.orders.get(order_id)
        old = order.status
        order.refund(day)
        self._save(order, old)


# --------------------------------------------------------------------------------------
# workload
# --------------------------------------------------------------------------------------

CATEGORIES = ["books", "games", "kitchen", "garden", "toys", "audio", "computers", "office",
              "sports", "beauty", "grocery", "pets"]
CARRIERS = ["dhl", "ups", "postnord", "royalmail", "colissimo"]
CANCEL_REASONS = ["customer_request", "payment_timeout", "duplicate", "address_invalid"]


def setup():
    import random

    rng = random.Random(4711)
    products = []
    for i in range(N_PRODUCTS):
        cat = CATEGORIES[i % len(CATEGORIES)]
        products.append({
            "sku": f"{cat[:3].upper()}-{i:05d}", "name": f"{cat.title()} item {i}",
            "category": cat, "price": f"{rng.lognormvariate(3.0, 0.9):.2f}",
            "currency": rng.choice(["EUR", "EUR", "EUR", "USD", "GBP"]),
            "weight": rng.randint(40, 6000),
            "tax": "reduced" if cat in ("books", "grocery") else "standard",
            "stock": rng.randint(20, 400),
        })
    customers = []
    countries = list(TAX_RATES)
    cur_for = {"DE": "EUR", "FR": "EUR", "NL": "EUR", "SE": "SEK", "GB": "GBP", "US": "USD"}
    for i in range(N_CUSTOMERS):
        country = rng.choice(countries)
        customers.append({
            "id": f"{i:05d}", "name": f"Customer {i}", "street": f"{rng.randint(1, 200)} Main St",
            "city": f"City{rng.randint(1, 90)}", "postcode": f"{rng.randint(10000, 99999)}",
            "country": country, "currency": cur_for[country], "flags": rng.getrandbits(5)
            & rng.getrandbits(5), "since": 18_000 + rng.randint(0, 1500),
        })
    coupons = ["WELCOME10", "SPRING15", "VIP20", "FIVER", "TENNER", "BOOKS3", "GAMES2",
               "SHIPFREE", "BOGUS"]
    start = date(2026, 3, 1)
    commands = []
    open_orders = []
    for n in range(N_ORDERS):
        day = start + timedelta(days=n * 30 // N_ORDERS)
        cust = rng.randrange(N_CUSTOMERS)
        oid = f"O-{n:06d}-{cust:05d}"
        commands.append(("create", day, oid, f"{cust:05d}", rng.choice(
            ["web", "web", "mobile", "phone", "marketplace"]), rng.choice([0, 1, 1, 1, 2, 3])))
        cat = rng.choice(CATEGORIES)
        skus = []
        for _ in range(rng.choice([1, 1, 2, 3, 4, 5, 7])):
            p = products[rng.randrange(N_PRODUCTS)] if rng.random() < 0.6 else \
                products[CATEGORIES.index(cat) + len(CATEGORIES) * rng.randrange(N_PRODUCTS
                                                                               // len(CATEGORIES))]
            skus.append(p["sku"])
            commands.append(("add", day, oid, p["sku"], rng.choice([1, 1, 1, 2, 3])))
        if rng.random() < 0.12:
            commands.append(("remove", day, oid, rng.choice(skus + ["NOPE-1"])))
        for _ in range(rng.choice([0, 0, 0, 1, 1, 2])):
            commands.append(("coupon", day, oid, rng.choice(coupons)))
        commands.append(("place", day, oid))
        open_orders.append(oid)
        # progress older orders through the pipeline
        while len(open_orders) > 40:
            o = open_orders.pop(rng.randrange(len(open_orders)))
            r = rng.random()
            if r < 0.07:
                commands.append(("cancel", day, o, rng.choice(CANCEL_REASONS)))
                continue
            commands.append(("pay", day, o, rng.random() < 0.02))
            if r < 0.10:
                commands.append(("cancel", day, o, "late_cancel"))
            commands.append(("pack", day, o))
            commands.append(("ship", day, o, rng.choice(CARRIERS)))
            if rng.random() < 0.9:
                commands.append(("deliver", day + timedelta(days=2), o))
            if rng.random() < 0.04:
                commands.append(("refund", day + timedelta(days=5), o))
    return {"products": products, "customers": customers, "commands": commands}


def build_service(state) -> OrderService:
    catalog = ProductCatalog()
    stock = {}
    for p in state["products"]:
        catalog.add(Product(p["sku"], p["name"], p["category"],
                            Money(Decimal(p["price"]), p["currency"]), p["weight"],
                            TaxClass(p["tax"])))
        stock[p["sku"]] = p["stock"]
    customers = {}
    for c in state["customers"]:
        customers[c["id"]] = Customer(
            c["id"], c["name"], Address(c["street"], c["city"], c["postcode"], c["country"]),
            c["currency"], CustomerFlag(c["flags"]), date.fromordinal(730_000 + c["since"]))
    promotions: dict[str, Promotion] = {
        "WELCOME10": PercentOff("WELCOME10", 10, Decimal("20")),
        "SPRING15": PercentOff("SPRING15", 15, Decimal("80")),
        "VIP20": PercentOff("VIP20", 20, Decimal("150")),
        "FIVER": FixedOff("FIVER", Decimal("5")),
        "TENNER": FixedOff("TENNER", Decimal("10")),
        "BOOKS3": CategoryDeal("BOOKS3", "books", 3),
        "GAMES2": CategoryDeal("GAMES2", "games", 2),
        "SHIPFREE": FreeShipping("SHIPFREE"),
    }
    service = OrderService(catalog, customers, promotions, Inventory(stock))
    return service


def run(state):
    service = build_service(state)
    sales = SalesReport(service.orders)
    value = CustomerValue()
    timeline = StatusTimeline()
    service.bus.subscribe(OrderPaid, sales.on_paid)
    service.bus.subscribe(OrderRefunded, sales.on_refunded)
    service.bus.subscribe(DomainEvent, value)
    service.bus.subscribe(StatusChanged, timeline.on_status)
    for command in state["commands"]:
        service.handle(command)

    by_status = {str(s): len(service.orders.with_status(s)) for s in OrderStatus}
    top = sorted(value.lifetime.items(), key=lambda kv: (-kv[1].amount, kv[0]))[:40]
    backlog = service.orders.with_status(OrderStatus.PLACED)
    backlog_value = money_sum((o.total.to_eur() for o in backlog), "EUR")
    by_priority = Counter(o.priority.name for o in backlog)
    return {
        "orders": len(service.orders),
        "by_status": by_status,
        "rejected": dict(sorted(service.rejected.items())),
        "published": dict(sorted(service.bus.published.items())),
        "revenue": {c: str(m) for c, m in sorted(sales.revenue_by_category.items())},
        "refunds": str(sales.refunds),
        "days": [str(m) for _, m in sorted(sales.revenue_by_day.items())],
        "top_units": sorted(sales.units_by_sku.items(), key=lambda kv: (-kv[1], kv[0]))[:20],
        "top_customers": [(cid, str(m), value.orders[cid]) for cid, m in top],
        "transitions": sorted(f"{a}->{b}:{n}" for (a, b), n in timeline.transitions.items()),
        "cancel_reasons": dict(sorted(timeline.cancel_reasons.items())),
        "shipped_weight": dict(sorted(timeline.shipped_weight.items())),
        "backlog": [str(backlog_value), sorted(by_priority.items())],
        "low_stock": sorted(sku for sku in service.inventory.on_hand
                            if service.inventory.available(sku) < 5)[:50],
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
