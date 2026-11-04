"""A Flask REST API on Flask-SQLAlchemy and marshmallow: the classic Flask service stack.

Stands for the second-most common way to build a JSON API in Python: Flask with the
Flask-SQLAlchemy extension for the ORM and marshmallow schemas for (de)serialization and
validation (the stack of most Flask tutorials, cookiecutters and of webargs/apispec/
flask-smorest). A lending-library API in a ``Blueprint``: bearer-token auth in a
``before_request`` hook (a DB lookup per request), list endpoints with filters, search,
sorting and ``db.paginate``, detail endpoints with nested schemas and ``Method`` fields,
creates and partial updates validated by marshmallow (``Length``/``Range``/``OneOf``/
``Regexp`` validators, ``@validates`` with an ISBN checksum, ``@validates_schema``),
loans and returns with a 409 conflict path, an aggregate stats endpoint (``GROUP BY``),
``errorhandler``s that turn 404/401/``ValidationError`` into JSON. SQLAlchemy 2.x
``select()`` style over sqlite in memory. Driven by Flask's test client (in-process WSGI,
Werkzeug), 630 requests per run; the database is restored from a snapshot (sqlite backup
API) at the start of each run so the run repeats exactly. Data: 60 authors, 1,200 books,
200 members, 1,500 loans.
"""

REQUIRES = ["Flask==3.1.3", "Werkzeug==3.1.9", "Jinja2==3.1.6", "MarkupSafe==3.0.4",
            "itsdangerous==2.2.0", "click==8.5.0", "blinker==1.9.0", "Flask-SQLAlchemy==3.1.1",
            "SQLAlchemy==2.1.6", "greenlet==3.5.6", "typing-extensions==4.16.0", "marshmallow==4.3.1"]

GENRES = ["fiction", "history", "science", "poetry", "travel", "cooking", "children", "biography"]
WORDS = ("night river glass empire garden winter letters stone light machine voyage harvest "
         "silent atlas kitchen orchard mountain secret paper ocean signal forest crown").split()


def _isbn13(stem12):
    s = sum((1 if i % 2 == 0 else 3) * int(c) for i, c in enumerate(stem12))
    return stem12 + str((10 - s % 10) % 10)


def setup():
    import datetime as dt
    import random
    import sqlite3
    from decimal import Decimal

    from flask import Blueprint, Flask, abort, g, jsonify, request
    from flask_sqlalchemy import SQLAlchemy
    from marshmallow import (EXCLUDE, Schema, ValidationError, fields, validate, validates,
                             validates_schema)
    from sqlalchemy import ForeignKey, String, func, or_, select
    from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column, relationship

    class Base(DeclarativeBase):
        pass

    db = SQLAlchemy(model_class=Base)

    class Author(db.Model):
        id: Mapped[int] = mapped_column(primary_key=True)
        name: Mapped[str] = mapped_column(String(80))
        country: Mapped[str] = mapped_column(String(2))
        born: Mapped[dt.date]
        books: Mapped[list["Book"]] = relationship(back_populates="author")

    class Book(db.Model):
        id: Mapped[int] = mapped_column(primary_key=True)
        title: Mapped[str] = mapped_column(String(200), index=True)
        isbn: Mapped[str] = mapped_column(String(13), unique=True)
        author_id: Mapped[int] = mapped_column(ForeignKey("author.id"))
        published: Mapped[dt.date]
        pages: Mapped[int]
        price: Mapped[Decimal]
        genre: Mapped[str] = mapped_column(String(20), index=True)
        author: Mapped[Author] = relationship(back_populates="books", lazy="joined")
        loans: Mapped[list["Loan"]] = relationship(back_populates="book", cascade="all, delete-orphan")

    class Member(db.Model):
        id: Mapped[int] = mapped_column(primary_key=True)
        name: Mapped[str] = mapped_column(String(80))
        email: Mapped[str] = mapped_column(String(120), unique=True)
        api_token: Mapped[str] = mapped_column(String(40), unique=True)
        joined: Mapped[dt.date]

    class Loan(db.Model):
        id: Mapped[int] = mapped_column(primary_key=True)
        book_id: Mapped[int] = mapped_column(ForeignKey("book.id"), index=True)
        member_id: Mapped[int] = mapped_column(ForeignKey("member.id"), index=True)
        loaned_at: Mapped[dt.datetime]
        due: Mapped[dt.date]
        returned_at: Mapped[dt.datetime | None]
        book: Mapped[Book] = relationship(back_populates="loans")
        member: Mapped[Member] = relationship(lazy="joined")

    today = dt.date(2025, 6, 1)

    class AuthorSchema(Schema):
        id = fields.Int(dump_only=True)
        name = fields.Str(required=True, validate=validate.Length(min=2, max=80))
        country = fields.Str(required=True, validate=validate.Length(equal=2))
        born = fields.Date(required=True)

    class BookSchema(Schema):
        class Meta:
            unknown = EXCLUDE
            ordered = True

        id = fields.Int(dump_only=True)
        title = fields.Str(required=True, validate=validate.Length(min=3, max=200))
        isbn = fields.Str(required=True, validate=validate.Regexp(r"^97[89]\d{10}$"))
        author = fields.Nested(AuthorSchema, only=("id", "name"), dump_only=True)
        author_id = fields.Int(required=True, load_only=True, validate=validate.Range(min=1))
        published = fields.Date(required=True)
        pages = fields.Int(required=True, validate=validate.Range(min=1, max=5000))
        price = fields.Decimal(required=True, places=2, as_string=True, validate=validate.Range(min=0))
        genre = fields.Str(required=True, validate=validate.OneOf(GENRES))
        age_years = fields.Method("get_age", dump_only=True)
        links = fields.Method("get_links", dump_only=True)

        def get_age(self, obj):
            return (today - obj.published).days // 365

        def get_links(self, obj):
            return {"self": f"/api/books/{obj.id}", "author": f"/api/authors/{obj.author_id}/books"}

        @validates("isbn")
        def check_isbn(self, value, **kwargs):
            if _isbn13(value[:12]) != value:
                raise ValidationError("Invalid ISBN-13 checksum.")

        @validates_schema
        def check_dates(self, data, **kwargs):
            if "published" in data and data["published"] > today:
                raise ValidationError({"published": ["Publication date is in the future."]})

    class LoanSchema(Schema):
        id = fields.Int(dump_only=True)
        book = fields.Nested(BookSchema, only=("id", "title", "isbn"), dump_only=True)
        book_id = fields.Int(required=True, load_only=True)
        member = fields.Function(lambda o: o.member.name, dump_only=True)
        loaned_at = fields.DateTime(required=True)
        due = fields.Date(dump_only=True)
        returned_at = fields.DateTime(dump_only=True, allow_none=True)
        overdue = fields.Method("get_overdue", dump_only=True)

        def get_overdue(self, obj):
            return obj.returned_at is None and obj.due < today

    book_schema, books_schema = BookSchema(), BookSchema(many=True)
    book_patch_schema = BookSchema(partial=True)
    loan_schema, loans_schema = LoanSchema(), LoanSchema(many=True)

    app = Flask("fwbench_library")
    app.config.update(SQLALCHEMY_DATABASE_URI="sqlite://")
    db.init_app(app)
    api = Blueprint("api", "fwbench_library", url_prefix="/api")

    @api.before_request
    def authenticate():
        g.member = None
        auth = request.headers.get("Authorization", "")
        if auth.startswith("Bearer "):
            g.member = db.session.scalar(select(Member).filter_by(api_token=auth[7:]))
            if g.member is None:
                abort(401)
        elif request.method != "GET":
            abort(401)

    @api.errorhandler(ValidationError)
    def on_validation(err):
        return jsonify(errors=err.messages), 422

    @api.errorhandler(404)
    def on_404(err):
        return jsonify(error="not found"), 404

    @api.errorhandler(401)
    def on_401(err):
        return jsonify(error="authentication required"), 401

    @api.get("/books")
    def list_books():
        q = select(Book)
        if genre := request.args.get("genre"):
            q = q.where(Book.genre == genre)
        if author := request.args.get("author", type=int):
            q = q.where(Book.author_id == author)
        if text := request.args.get("q"):
            q = q.where(or_(Book.title.ilike(f"%{text}%"), Book.isbn.startswith(text)))
        sort = request.args.get("sort", "title")
        col = {"title": Book.title, "published": Book.published, "price": Book.price,
               "pages": Book.pages}[sort.lstrip("-")]
        q = q.order_by(col.desc() if sort.startswith("-") else col, Book.id)
        page = db.paginate(q, per_page=request.args.get("per_page", 20, type=int), error_out=False)
        return jsonify(items=books_schema.dump(page.items), page=page.page, pages=page.pages,
                       total=page.total)

    @api.get("/books/<int:book_id>")
    def get_book(book_id):
        book = db.get_or_404(Book, book_id)
        data = book_schema.dump(book)
        data["recent_loans"] = loans_schema.dump(
            db.session.scalars(select(Loan).where(Loan.book_id == book_id)
                               .order_by(Loan.loaned_at.desc()).limit(5)).all())
        return jsonify(data)

    @api.post("/books")
    def create_book():
        data = book_schema.load(request.get_json())
        if db.session.get(Author, data["author_id"]) is None:
            raise ValidationError({"author_id": ["Unknown author."]})
        book = Book(**data)
        db.session.add(book)
        db.session.commit()
        return jsonify(book_schema.dump(book)), 201

    @api.patch("/books/<int:book_id>")
    def update_book(book_id):
        book = db.get_or_404(Book, book_id)
        for k, v in book_patch_schema.load(request.get_json()).items():
            setattr(book, k, v)
        db.session.commit()
        return jsonify(book_schema.dump(book))

    @api.delete("/books/<int:book_id>")
    def delete_book(book_id):
        db.session.delete(db.get_or_404(Book, book_id))
        db.session.commit()
        return "", 204

    @api.get("/authors/<int:author_id>/books")
    def author_books(author_id):
        author = db.get_or_404(Author, author_id)
        books = db.session.scalars(select(Book).where(Book.author_id == author_id)
                                   .order_by(Book.published)).unique().all()
        return jsonify(author=AuthorSchema().dump(author), books=books_schema.dump(books))

    @api.post("/loans")
    def create_loan():
        data = loan_schema.load(request.get_json())
        book = db.get_or_404(Book, data["book_id"])
        open_loan = db.session.scalar(select(func.count(Loan.id)).where(
            Loan.book_id == book.id, Loan.returned_at.is_(None)))
        if open_loan:
            return jsonify(error="book is on loan"), 409
        loan = Loan(book=book, member=g.member, loaned_at=data["loaned_at"],
                    due=data["loaned_at"].date() + dt.timedelta(days=21))
        db.session.add(loan)
        db.session.commit()
        return jsonify(loan_schema.dump(loan)), 201

    @api.post("/loans/<int:loan_id>/return")
    def return_loan(loan_id):
        loan = db.get_or_404(Loan, loan_id)
        loan.returned_at = loan.loaned_at + dt.timedelta(days=9)
        db.session.commit()
        return jsonify(loan_schema.dump(loan))

    @api.get("/members/me/loans")
    def my_loans():
        if g.member is None:
            abort(401)
        loans = db.session.scalars(select(Loan).where(Loan.member_id == g.member.id)
                                   .order_by(Loan.loaned_at.desc(), Loan.id).limit(30)).all()
        return jsonify(member=g.member.name, loans=loans_schema.dump(loans))

    @api.get("/stats")
    def stats():
        rows = db.session.execute(
            select(Book.genre, func.count(Book.id), func.avg(Book.pages), func.min(Book.published))
            .group_by(Book.genre).order_by(Book.genre)).all()
        busiest = db.session.execute(
            select(Member.name, func.count(Loan.id).label("n")).join(Loan, Loan.member_id == Member.id)
            .group_by(Member.id).order_by(func.count(Loan.id).desc(), Member.id).limit(5)).all()
        return jsonify(genres=[{"genre": g_, "books": n, "avg_pages": round(p, 1), "oldest": o.isoformat()}
                               for g_, n, p, o in rows],
                       busiest=[{"member": m, "loans": n} for m, n in busiest])

    app.register_blueprint(api)

    # ---- data ---------------------------------------------------------------------------
    rng = random.Random(5)
    with app.app_context():
        db.create_all()
        authors = [Author(name=f"{rng.choice(WORDS).title()} {rng.choice(WORDS).title()}son",
                          country=rng.choice(["GB", "US", "FR", "PL", "JP", "BR", "NG"]),
                          born=dt.date(1900 + rng.randint(0, 90), rng.randint(1, 12), rng.randint(1, 28)))
                   for _ in range(60)]
        db.session.add_all(authors)
        books = []
        for i in range(1200):
            books.append(Book(title=" ".join(rng.choice(WORDS).title() for _ in range(rng.randint(2, 5))),
                              isbn=_isbn13(f"978{i:09d}"), author=authors[rng.randrange(60)],
                              published=dt.date(1950, 1, 1) + dt.timedelta(days=rng.randint(0, 27000)),
                              pages=rng.randint(80, 900), price=Decimal(rng.randint(499, 4999)) / 100,
                              genre=rng.choice(GENRES)))
        db.session.add_all(books)
        members = [Member(name=f"Member {i}", email=f"m{i}@example.org", api_token=f"tok-{i:04d}-{i * 7919 % 10007:05d}",
                          joined=dt.date(2020, 1, 1) + dt.timedelta(days=i * 5)) for i in range(200)]
        db.session.add_all(members)
        base = dt.datetime(2024, 1, 1, 10, 0)
        for j in range(1500):
            at = base + dt.timedelta(hours=7 * j)
            db.session.add(Loan(book=books[(j * 31) % 1200], member=members[j % 200], loaned_at=at,
                                due=at.date() + dt.timedelta(days=21),
                                returned_at=None if j % 9 == 0 else at + dt.timedelta(days=12)))
        db.session.commit()
        tokens = [m.api_token for m in members]
        raw = db.engine.raw_connection()
        snapshot = sqlite3.connect(":memory:", check_same_thread=False)
        raw.driver_connection.backup(snapshot)
        raw.close()
        db.session.remove()

    # ---- request plan -------------------------------------------------------------------
    plan = []
    for i in range(480):
        k = i % 16
        hdr = {"Authorization": f"Bearer {tokens[i % 200]}"}
        if k in (0, 8):
            q = [f"?page={1 + i % 30}", f"?genre={GENRES[i % 8]}&sort=-published",
                 f"?q={WORDS[i % len(WORDS)]}&per_page=10", f"?author={1 + i % 60}&sort=price",
                 "?sort=-pages&per_page=50", f"?q=978000000{i % 10}"][i % 6]
            plan.append(("GET", f"/api/books{q}", None, None))
        elif k in (1, 5, 11):
            plan.append(("GET", f"/api/books/{1 + (i * 37) % 1200}", None, None))
        elif k == 2:
            plan.append(("GET", f"/api/authors/{1 + i % 60}/books", None, None))
        elif k == 3:
            body = {"title": f"New Book {i} {WORDS[i % 23]}", "isbn": _isbn13(f"979{i:09d}"),
                    "author_id": 1 + i % 60, "published": f"20{10 + i % 14}-0{1 + i % 9}-15",
                    "pages": 100 + i, "price": f"{10 + i % 30}.99", "genre": GENRES[i % 8]}
            plan.append(("POST", "/api/books", hdr, body))
            plan.append(("PATCH", "/api/books/1201", hdr, {"price": "12.50", "pages": 222}))
            plan.append(("GET", "/api/books/1201", None, None))
        elif k == 4:
            plan.append(("POST", "/api/books", hdr, {"title": "X", "isbn": "9780000000000",
                                                     "author_id": 0, "published": "2999-01-01",
                                                     "pages": -1, "price": "abc", "genre": "horror"}))
        elif k in (6, 13):
            plan.append(("POST", "/api/loans", hdr, {"book_id": 1 + (i * 53) % 1200,
                                                     "loaned_at": f"2025-05-{1 + i % 28:02d}T09:30:00"}))
            plan.append(("GET", "/api/members/me/loans", hdr, None))
        elif k == 7:
            plan.append(("POST", f"/api/loans/{1 + (i * 9) % 1500}/return", hdr, None))
        elif k == 9:
            plan.append(("GET", "/api/stats", None, None))
        elif k == 10:
            plan.append(("POST", "/api/books", None, {"title": "No auth"}))
            plan.append(("GET", "/api/books/99999", None, None))
        elif k == 12:
            plan.append(("PATCH", f"/api/books/{1 + i % 1200}", hdr, {"genre": "horror", "pages": 0}))
        elif k == 14:
            plan.append(("DELETE", "/api/books/1201", hdr, None))  # sqlite reuses the rowid
        else:
            plan.append(("GET", f"/api/books?genre={GENRES[(i // 16) % 8]}&page=2", None, None))
    return {"app": app, "db": db, "snapshot": snapshot, "plan": plan}


def run(state):
    import hashlib

    app, db = state["app"], state["db"]
    with app.app_context():
        db.session.remove()
        raw = db.engine.raw_connection()
        state["snapshot"].backup(raw.driver_connection)
        raw.close()
    client = app.test_client()
    out = []
    for method, url, headers, body in state["plan"]:
        resp = client.open(url, method=method, headers=headers, json=body)
        data = resp.get_data()
        out.append((method, url, resp.status_code, len(data), hashlib.sha256(data).hexdigest()))
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
