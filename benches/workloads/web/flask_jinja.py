"""A Flask storefront: Werkzeug request/response handling with Jinja2 server-side rendering.

Stands for the second most common Python web stack: a Flask app rendering Jinja2
templates. Each request goes through Werkzeug's WSGI environ building, routing with
converters, Flask's request/app contexts, signed-cookie ``session`` (itsdangerous),
``url_for``, form and query-string parsing, and Jinja2 rendering of a page with template
inheritance, macros, loops, filters (``format``, ``truncate``, ``title``, custom money
filter) and MarkupSafe autoescaping; plus ``jsonify`` API endpoints and a form POST that
validates input and redirects. Driven by Flask's test client (in-process WSGI), 600 requests
per run over an in-memory catalogue of 600 products in 12 categories.
"""

REQUIRES = ["Flask==3.1.3", "Werkzeug==3.1.9", "Jinja2==3.1.6", "MarkupSafe==3.0.4",
            "itsdangerous==2.2.0", "blinker==1.9.0", "click==8.5.0"]

TEMPLATES = {
    "layout.html": """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>{% block title %}Shop{% endblock %} | Northwind</title>
<link rel="stylesheet" href="{{ url_for('static', filename='shop.css') }}"></head>
<body>
<header><a href="{{ url_for('index') }}">Northwind</a>
<form action="{{ url_for('search') }}"><input name="q" value="{{ request.args.get('q', '') }}"></form>
<span class="cart">{{ session.get('cart', [])|length }} item(s)</span></header>
<nav>{% for c in categories %}<a href="{{ url_for('category', slug=c.slug) }}"{% if c.slug == active %} class="on"{% endif %}>{{ c.name }}</a>{% endfor %}</nav>
{% with msgs = get_flashed_messages(with_categories=true) %}{% if msgs %}<ul class="flash">{% for cat, m in msgs %}<li class="{{ cat }}">{{ m }}</li>{% endfor %}</ul>{% endif %}{% endwith %}
<main>{% block body %}{% endblock %}</main>
<footer>{{ year }} Northwind Trading &middot; {{ product_count }} products</footer>
</body></html>""",
    "macros.html": """{% macro product_card(p) -%}
<div class="product{% if p.stock == 0 %} sold-out{% endif %}">
  <a href="{{ url_for('product', pid=p.id) }}"><h3>{{ p.name|title }}</h3></a>
  <p class="price">{{ p.price|money }}{% if p.discount %} <s>{{ (p.price / (1 - p.discount))|money }}</s> <em>-{{ "%d"|format(p.discount * 100) }}%</em>{% endif %}</p>
  <p>{{ p.description|truncate(90) }}</p>
  <ul class="tags">{% for t in p.tags %}<li>{{ t }}</li>{% endfor %}</ul>
  {% if p.rating %}<span class="stars" title="{{ '%.1f'|format(p.rating) }}">{{ '*' * (p.rating|round|int) }}</span>{% endif %}
</div>
{%- endmacro %}""",
    "index.html": """{% extends "layout.html" %}{% from "macros.html" import product_card %}
{% block body %}<h1>Featured</h1><div class="grid">{% for p in featured %}{{ product_card(p) }}{% endfor %}</div>{% endblock %}""",
    "category.html": """{% extends "layout.html" %}{% from "macros.html" import product_card %}
{% block title %}{{ category.name }}{% endblock %}
{% block body %}<h1>{{ category.name }} <small>({{ total }})</small></h1>
<div class="sort">Sort: {% for key in ['name', 'price', 'rating'] %}<a href="{{ url_for('category', slug=category.slug, sort=key, page=1) }}"{% if key == sort %} class="on"{% endif %}>{{ key }}</a> {% endfor %}</div>
<div class="grid">{% for p in products %}{{ product_card(p) }}{% else %}<p>Nothing here.</p>{% endfor %}</div>
<nav class="pages">{% for n in range(1, pages + 1) %}<a href="{{ url_for('category', slug=category.slug, sort=sort, page=n) }}"{% if n == page %} class="on"{% endif %}>{{ n }}</a>{% endfor %}</nav>
{% endblock %}""",
    "product.html": """{% extends "layout.html" %}{% from "macros.html" import product_card %}
{% block title %}{{ p.name|title }}{% endblock %}
{% block body %}<article><h1>{{ p.name|title }}</h1><p class="price">{{ p.price|money }}</p>
<p>{{ p.description }}</p><dl>{% for k, v in p.specs.items()|sort %}<dt>{{ k|capitalize }}</dt><dd>{{ v }}</dd>{% endfor %}</dl>
<form method="post" action="{{ url_for('add_to_cart') }}"><input type="hidden" name="pid" value="{{ p.id }}">
<input name="qty" value="1"><button>Add to cart</button></form></article>
<h2>Reviews ({{ p.reviews|length }})</h2>{% for r in p.reviews %}<blockquote><p>{{ r.text }}</p><cite>{{ r.author }} &mdash; {{ r.stars }}/5</cite></blockquote>{% endfor %}
<h2>Related</h2><div class="grid">{% for q in related %}{{ product_card(q) }}{% endfor %}</div>{% endblock %}""",
}

WORDS = ("oak walnut steel linen wool copper ceramic glass cotton leather bamboo marble "
         "lamp chair table mug kettle blanket shelf vase rug clock bowl desk stool "
         "classic modern rustic compact deluxe handmade nordic vintage minimal").split()


def setup():
    import random

    from flask import Flask, flash, jsonify, redirect, render_template, request, session, url_for
    from jinja2 import DictLoader

    rng = random.Random(7)
    cats = [{"slug": f"cat{i}", "name": f"{rng.choice(WORDS).title()} {rng.choice(WORDS).title()}"}
            for i in range(12)]
    products = []
    for i in range(600):
        name = " ".join(rng.choice(WORDS) for _ in range(rng.randint(2, 4)))
        products.append({
            "id": i, "name": name, "category": cats[i % 12]["slug"],
            "price": round(rng.uniform(3, 900), 2), "discount": rng.choice([0, 0, 0, 0.1, 0.15, 0.3]),
            "stock": rng.choice([0, 1, 5, 20, 100]), "rating": round(rng.uniform(1, 5), 1) if rng.random() < .8 else None,
            "description": " ".join(rng.choice(WORDS) for _ in range(rng.randint(12, 40))).capitalize() + ".",
            "tags": sorted({rng.choice(WORDS) for _ in range(rng.randint(0, 4))}),
            "specs": {"material": rng.choice(WORDS), "weight": f"{rng.randint(1, 50) / 10} kg",
                      "origin": rng.choice(["PL", "DE", "SE", "PT", "VN", "MX"]), "sku": f"NW-{i:05d}"},
            "reviews": [{"author": rng.choice(WORDS).title(), "stars": rng.randint(1, 5),
                         "text": " ".join(rng.choice(WORDS) for _ in range(rng.randint(5, 25)))}
                        for _ in range(rng.randint(0, 6))],
        })
    by_cat = {c["slug"]: [p for p in products if p["category"] == c["slug"]] for c in cats}
    by_id = {p["id"]: p for p in products}

    app = Flask(__name__)
    app.config.update(SECRET_KEY="webbench-flask-secret", TESTING=False)
    app.jinja_loader = DictLoader(TEMPLATES)

    @app.template_filter("money")
    def money(v):
        return f"${v:,.2f}"

    @app.context_processor
    def common():
        return {"categories": cats, "year": 2025, "product_count": len(products), "active": None}

    @app.route("/")
    def index():
        featured = sorted(products, key=lambda p: (-(p["rating"] or 0), p["id"]))[:24]
        return render_template("index.html", featured=featured)

    @app.route("/c/<slug>")
    def category(slug):
        items = by_cat.get(slug)
        if items is None:
            return render_template("category.html", category={"slug": slug, "name": "Unknown"},
                                   products=[], total=0, pages=0, page=1, sort="name"), 404
        sort = request.args.get("sort", "name")
        page = request.args.get("page", 1, type=int)
        key = {"name": lambda p: p["name"], "price": lambda p: p["price"],
               "rating": lambda p: -(p["rating"] or 0)}.get(sort, lambda p: p["name"])
        items = sorted(items, key=key)
        pages = (len(items) + 19) // 20
        cat = next(c for c in cats if c["slug"] == slug)
        return render_template("category.html", category=cat, products=items[(page - 1) * 20:page * 20],
                               total=len(items), pages=pages, page=page, sort=sort, active=slug)

    @app.route("/p/<int:pid>")
    def product(pid):
        p = by_id.get(pid)
        if p is None:
            return "not found", 404
        related = [q for q in by_cat[p["category"]] if q["id"] != pid][:6]
        return render_template("product.html", p=p, related=related)

    @app.route("/search")
    def search():
        q = request.args.get("q", "").strip().lower()
        words = q.split()
        hits = [p for p in products if all(w in p["name"] or w in p["description"].lower() for w in words)]
        return render_template("category.html", category={"slug": "search", "name": f"Search: {q}"},
                               products=hits[:20], total=len(hits), pages=1, page=1, sort="name")

    @app.post("/cart")
    def add_to_cart():
        try:
            pid = int(request.form["pid"])
            qty = int(request.form.get("qty", "1"))
        except (KeyError, ValueError):
            flash("Invalid request", "error")
            return redirect(url_for("index"))
        if pid not in by_id or not 1 <= qty <= 10:
            flash("Cannot add that item", "error")
            return redirect(url_for("index"))
        cart = session.get("cart", [])
        cart.append([pid, qty])
        session["cart"] = cart[-20:]
        flash(f"Added {qty} x {by_id[pid]['name']}", "success")
        return redirect(url_for("product", pid=pid))

    @app.get("/api/products")
    def api_products():
        cat = request.args.get("category")
        limit = request.args.get("limit", 50, type=int)
        items = by_cat.get(cat, products)[:limit]
        return jsonify({"count": len(items), "items": [
            {"id": p["id"], "name": p["name"], "price": p["price"], "stock": p["stock"],
             "url": url_for("product", pid=p["id"], _external=True)} for p in items]})

    @app.get("/api/products/<int:pid>")
    def api_product(pid):
        p = by_id.get(pid)
        if p is None:
            return jsonify(error="not found"), 404
        return jsonify(p)

    plan = []
    for i in range(600):
        k = i % 10
        if k == 0:
            plan.append(("get", "/", None))
        elif k in (1, 6):
            plan.append(("get", f"/c/cat{i % 12}?sort={('name', 'price', 'rating')[i % 3]}&page={1 + i % 3}", None))
        elif k in (2, 5, 8):
            plan.append(("get", f"/p/{(i * 47) % 600}", None))
        elif k == 3:
            plan.append(("get", f"/search?q={WORDS[i % len(WORDS)]}+{WORDS[(i * 7) % len(WORDS)]}", None))
        elif k == 4:
            plan.append(("post", "/cart", {"pid": str((i * 13) % 600), "qty": str(1 + i % 3)}))
        elif k == 7:
            plan.append(("get", f"/api/products?category=cat{i % 12}&limit=40", None))
        else:
            plan.append(("get", f"/api/products/{(i * 31) % 650}", None))
    return {"app": app, "plan": plan}


def run(state):
    import hashlib

    out = []
    with state["app"].test_client() as client:
        for method, url, data in state["plan"]:
            resp = client.get(url) if method == "get" else client.post(url, data=data)
            body = resp.get_data()
            out.append((url, resp.status_code, len(body), hashlib.sha256(body).hexdigest()))
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
