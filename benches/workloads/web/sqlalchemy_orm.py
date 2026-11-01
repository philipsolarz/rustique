"""SQLAlchemy 2.x ORM data-access layer of a web backend, over sqlite.

Stands for the persistence layer behind most Flask/FastAPI services and many scripts:
declarative models with relationships, a ``Session`` per unit of work, ``select()``
queries with joins, filters, ordering and pagination, eager loading
(``selectinload``/``joinedload``), aggregates with ``group_by``, identity-map lookups
(``session.get``), object creation and attribute updates flushed by the unit of work,
and a bulk ``update()``. Each run executes 180 "requests" worth of ORM work in one
transaction and rolls it back, so runs are repeatable. Data: 400 customers, 1,200
addresses, 200 products, 3,000 orders with ~9,000 order lines in an in-memory sqlite
database (SQLAlchemy's Cython extensions enabled, as installed by default).
"""

REQUIRES = ["SQLAlchemy==2.1.6", "typing-extensions==4.16.0"]


def setup():
    import datetime as dt
    import random
    from decimal import Decimal

    from sqlalchemy import ForeignKey, Numeric, String, create_engine
    from sqlalchemy.orm import DeclarativeBase, Mapped, Session, mapped_column, relationship

    class Base(DeclarativeBase):
        pass

    class Customer(Base):
        __tablename__ = "customer"
        id: Mapped[int] = mapped_column(primary_key=True)
        email: Mapped[str] = mapped_column(String(120), unique=True)
        name: Mapped[str] = mapped_column(String(80))
        tier: Mapped[str] = mapped_column(String(10), index=True)
        created: Mapped[dt.datetime]
        addresses: Mapped[list["Address"]] = relationship(back_populates="customer")
        orders: Mapped[list["Order"]] = relationship(back_populates="customer")

    class Address(Base):
        __tablename__ = "address"
        id: Mapped[int] = mapped_column(primary_key=True)
        customer_id: Mapped[int] = mapped_column(ForeignKey("customer.id"), index=True)
        city: Mapped[str] = mapped_column(String(60))
        country: Mapped[str] = mapped_column(String(2))
        customer: Mapped[Customer] = relationship(back_populates="addresses")

    class Product(Base):
        __tablename__ = "product"
        id: Mapped[int] = mapped_column(primary_key=True)
        sku: Mapped[str] = mapped_column(String(20), unique=True)
        title: Mapped[str] = mapped_column(String(120))
        price: Mapped[Decimal] = mapped_column(Numeric(10, 2))
        stock: Mapped[int]

    class Order(Base):
        __tablename__ = "orders"
        id: Mapped[int] = mapped_column(primary_key=True)
        customer_id: Mapped[int] = mapped_column(ForeignKey("customer.id"), index=True)
        status: Mapped[str] = mapped_column(String(12), index=True)
        placed: Mapped[dt.datetime] = mapped_column(index=True)
        customer: Mapped[Customer] = relationship(back_populates="orders")
        lines: Mapped[list["OrderLine"]] = relationship(back_populates="order", cascade="all, delete-orphan")

    class OrderLine(Base):
        __tablename__ = "order_line"
        id: Mapped[int] = mapped_column(primary_key=True)
        order_id: Mapped[int] = mapped_column(ForeignKey("orders.id"), index=True)
        product_id: Mapped[int] = mapped_column(ForeignKey("product.id"))
        quantity: Mapped[int]
        unit_price: Mapped[Decimal] = mapped_column(Numeric(10, 2))
        order: Mapped[Order] = relationship(back_populates="lines")
        product: Mapped[Product] = relationship()

    engine = create_engine("sqlite://")
    Base.metadata.create_all(engine)
    rng = random.Random(3)
    base = dt.datetime(2024, 1, 1)
    with Session(engine) as s:
        products = [Product(sku=f"P-{i:05d}", title=f"Product {i} {rng.choice('ABCDEFG')}",
                            price=Decimal(rng.randint(199, 99999)) / 100, stock=rng.randint(0, 500))
                    for i in range(200)]
        s.add_all(products)
        customers = []
        for i in range(400):
            c = Customer(email=f"user{i}@example.com", name=f"User {i}", tier=rng.choice(["free", "pro", "team"]),
                         created=base + dt.timedelta(hours=7 * i))
            c.addresses = [Address(city=rng.choice(["Berlin", "Lyon", "Porto", "Krakow", "Austin"]),
                                   country=rng.choice(["DE", "FR", "PT", "PL", "US"])) for _ in range(3)]
            customers.append(c)
        s.add_all(customers)
        s.flush()
        for i in range(3000):
            o = Order(customer=customers[rng.randrange(400)], status=rng.choice(["new", "paid", "shipped", "returned"]),
                      placed=base + dt.timedelta(minutes=173 * i))
            for _ in range(rng.randint(1, 5)):
                p = products[rng.randrange(200)]
                o.lines.append(OrderLine(product=p, quantity=rng.randint(1, 4), unit_price=p.price))
            s.add(o)
        s.commit()
    models = dict(Customer=Customer, Address=Address, Product=Product, Order=Order, OrderLine=OrderLine)
    return {"engine": engine, "models": models}


def run(state):
    import datetime as dt
    from decimal import Decimal

    from sqlalchemy import func, select, update
    from sqlalchemy.orm import Session, joinedload, selectinload

    m = state["models"]
    Customer, Address, Product, Order, OrderLine = (m[k] for k in ("Customer", "Address", "Product", "Order", "OrderLine"))
    out = []
    with Session(state["engine"]) as s:
        s.begin()
        for i in range(180):
            k = i % 6
            if k == 0:  # customer dashboard: customer + recent orders with lines and products
                cid = 1 + (i * 37) % 400
                c = s.get(Customer, cid)
                orders = s.scalars(select(Order).where(Order.customer_id == cid)
                                   .options(selectinload(Order.lines).joinedload(OrderLine.product))
                                   .order_by(Order.placed.desc()).limit(10)).all()
                out.append((c.email, len(c.addresses), [(o.id, o.status, sum(l.quantity * l.unit_price for l in o.lines),
                                                         [l.product.sku for l in o.lines]) for o in orders]))
            elif k == 1:  # admin order list page with customer eager-loaded
                rows = s.scalars(select(Order).options(joinedload(Order.customer))
                                 .where(Order.status == ("paid", "new", "shipped")[i % 3])
                                 .order_by(Order.placed.desc()).offset((i * 13) % 300).limit(50)).all()
                out.append([(o.id, o.customer.name, o.placed.isoformat()) for o in rows])
            elif k == 2:  # report: revenue per tier and status
                q = (select(Customer.tier, Order.status, func.count(OrderLine.id),
                            func.sum(OrderLine.quantity * OrderLine.unit_price))
                     .join(Order, Order.customer_id == Customer.id).join(OrderLine, OrderLine.order_id == Order.id)
                     .group_by(Customer.tier, Order.status).order_by(Customer.tier, Order.status))
                out.append([(t, st, n, str(round(tot, 2))) for t, st, n, tot in s.execute(q)])
            elif k == 3:  # checkout: create an order with lines, adjust stock
                cid = 1 + (i * 53) % 400
                o = Order(customer_id=cid, status="new", placed=dt.datetime(2025, 1, 1) + dt.timedelta(minutes=i))
                for j in range(4):
                    p = s.get(Product, 1 + (i * 7 + j * 31) % 200)
                    o.lines.append(OrderLine(product=p, quantity=1 + j % 3, unit_price=p.price))
                    p.stock -= 1 + j % 3
                s.add(o)
                s.flush()
                out.append((o.id, len(o.lines)))
            elif k == 4:  # customers by city with their addresses (selectinload)
                rows = s.scalars(select(Customer).join(Customer.addresses)
                                 .where(Address.city == ("Berlin", "Lyon", "Porto", "Krakow", "Austin")[i % 5])
                                 .options(selectinload(Customer.addresses)).order_by(Customer.id)
                                 .distinct().limit(60)).all()
                out.append([(c.id, c.tier, sorted(a.country for a in c.addresses)) for c in rows])
            else:  # bulk status change + product search
                s.execute(update(Order).where(Order.status == "paid", Order.id % 17 == i % 17)
                          .values(status="shipped"))
                prods = s.scalars(select(Product).where(Product.title.like(f"%{'ABCDEFG'[i % 7]}"),
                                                        Product.price > Decimal("50")).order_by(Product.price)).all()
                out.append([(p.sku, str(p.price), p.stock) for p in prods])
        s.rollback()
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
