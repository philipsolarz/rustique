"""Beautiful Soup scraping product pages with the stdlib html.parser.

Stands for the standard Python scraping/HTML-processing stack: beautifulsoup4 (top-100
PyPI, the default answer to "parse HTML in Python") on its default, dependency-free
parser, the stdlib ``html.parser.HTMLParser`` (regex-driven tokenizer + ``html.unescape``),
with soupsieve providing CSS selectors. Per run: 24 generated e-commerce category pages
(~77 kB each: head metadata, inline scripts and JSON-LD, navigation, 48 product cards
with nested markup, prices, ratings, badges, a specification table, pagination, entities
and comments) are parsed into a tree, then queried the way scrapers do: ``find_all`` with
tag/class/attribute filters, ``select``/``select_one`` CSS selectors (descendant, child,
attribute, ``:nth-of-type``, ``:not``), ``.get_text(strip=True)``, attribute access,
sibling and parent navigation, tree edits (``decompose`` scripts, ``wrap``, new tags) and
re-serialization with ``str(soup)`` / ``prettify`` for a few pages. ~1.8 MB of HTML.
"""

REQUIRES = ["beautifulsoup4==4.15.0", "soupsieve==3.0.4", "typing-extensions==4.16.0"]


def setup():
    import json
    import random

    rng = random.Random(55)
    words = ("lamp chair desk oak walnut linen modern classic nordic compact deluxe premium "
             "cotton steel glass ceramic bamboo velvet marble copper").split()

    def card(i, p):
        name = " ".join(rng.choice(words).title() for _ in range(rng.randint(2, 4)))
        price = rng.randint(999, 99999) / 100
        old = price * 1.25 if rng.random() < 0.3 else None
        rating = rng.randint(10, 50) / 10
        badges = "".join(f'<span class="badge badge-{b}">{b.title()}</span>'
                         for b in rng.sample(["new", "sale", "eco", "bestseller"], rng.randint(0, 2)))
        return f"""
<li class="product-card{' on-sale' if old else ''}" data-sku="SKU-{p:02d}{i:04d}" data-position="{i}">
  <div class="card-media"><a href="/p/{p}/{i}?ref=cat&amp;pos={i}" class="card-link">
    <img src="/img/{p}/{i}.webp" srcset="/img/{p}/{i}@2x.webp 2x" alt="{name}" loading="lazy" width="320" height="320"></a>
    {badges}</div>
  <div class="card-body">
    <h3 class="card-title"><a href="/p/{p}/{i}">{name} &ndash; {rng.choice(words)}</a></h3>
    <p class="card-desc">{' '.join(rng.choice(words) for _ in range(rng.randint(8, 25)))} &amp; more&hellip;</p>
    <div class="price-box"><span class="price" itemprop="price" content="{price:.2f}">&euro;{price:,.2f}</span>
      {f'<del class="price-old">&euro;{old:,.2f}</del>' if old else ''}</div>
    <div class="rating" title="{rating} out of 5"><span class="stars" style="width:{rating * 20:.0f}%"></span>
      <a href="/p/{p}/{i}#reviews" class="reviews">{rng.randint(0, 900)} reviews</a></div>
    <ul class="variants">{''.join(f'<li><button type="button" data-variant="{v}" {"disabled" if rng.random() < 0.2 else ""}>{v}</button></li>' for v in rng.sample(["S", "M", "L", "XL", "Oak", "Black", "White"], 3))}</ul>
    <!-- card {i} rendered by tpl/card.html -->
  </div>
  <form action="/cart/add" method="post" class="add-to-cart"><input type="hidden" name="sku" value="SKU-{p:02d}{i:04d}">
    <input type="number" name="qty" value="1" min="1"><button class="btn btn-primary" type="submit">Add to cart</button></form>
</li>"""

    pages = []
    for p in range(24):
        ld = json.dumps({"@context": "https://schema.org", "@type": "ItemList", "numberOfItems": 48,
                         "itemListElement": [{"@type": "ListItem", "position": i, "url": f"/p/{p}/{i}"} for i in range(48)]})
        specs = "".join(f"<tr><th scope=\"row\">{rng.choice(words).title()}</th><td>{rng.randint(1, 500)} cm</td>"
                        f"<td>{rng.choice(words)}</td></tr>" for _ in range(30))
        html = f"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Category {p} &ndash; Shop</title>
<meta name="description" content="Browse {rng.choice(words)} products in category {p}">
<meta property="og:title" content="Category {p}"><link rel="canonical" href="https://shop.example.com/c/{p}">
<link rel="stylesheet" href="/static/app.{p:04x}.css">
<script>window.dataLayer=window.dataLayer||[];function gtag(){{dataLayer.push(arguments)}};gtag('js',new Date());
if (a < b && c > d) {{ console.log("</div> not a tag"); }}</script>
<script type="application/ld+json">{ld}</script>
<style>.product-card{{display:grid}} .price{{font-weight:700}}</style></head>
<body class="category-page theme-light"><a class="skip" href="#main">Skip to content</a>
<header id="top"><nav class="main-nav" aria-label="Main"><ul>{''.join(f'<li><a href="/c/{k}" class="{"active" if k == p else ""}">Cat {k}</a></li>' for k in range(24))}</ul></nav>
<form role="search" action="/search"><input type="search" name="q" placeholder="Search&hellip;"></form></header>
<main id="main"><nav class="breadcrumbs"><ol><li><a href="/">Home</a></li><li><a href="/c">Shop</a></li><li aria-current="page">Category {p}</li></ol></nav>
<h1>Category {p} <small>(48 of {rng.randint(100, 900)})</small></h1>
<aside class="filters"><fieldset><legend>Price</legend>{''.join(f'<label><input type="checkbox" name="price" value="{a}-{a + 50}"> &euro;{a}&ndash;{a + 50}</label>' for a in range(0, 500, 50))}</fieldset></aside>
<ul class="product-grid" id="grid">{''.join(card(i, p) for i in range(48))}</ul>
<section class="specs"><h2>Size guide</h2><table class="spec-table"><thead><tr><th>Part</th><th>Size</th><th>Material</th></tr></thead><tbody>{specs}</tbody></table></section>
<nav class="pagination">{''.join(f'<a href="?page={k}" class="page{" current" if k == 1 else ""}">{k}</a>' for k in range(1, 9))}<a rel="next" href="?page=2">Next &rarr;</a></nav>
</main><footer><p>&copy; 2025 Shop &middot; <a href="/privacy">Privacy</a> &middot; <a href="/terms">Terms</a></p></footer>
<script src="/static/app.{p:04x}.js" defer></script></body></html>"""
        pages.append(html)
    return {"pages": pages}


def run(state):
    import hashlib
    import re

    from bs4 import BeautifulSoup

    out = []
    price_re = re.compile(r"[\d,.]+")
    for n, html in enumerate(state["pages"]):
        soup = BeautifulSoup(html, "html.parser")
        title = soup.title.get_text(strip=True)
        canonical = soup.find("link", rel="canonical")["href"]
        products = []
        for li in soup.find_all("li", class_="product-card"):
            a = li.find("h3", class_="card-title").a
            price = li.select_one(".price-box .price")
            old = li.find("del", class_="price-old")
            rating = li.find("div", class_="rating")["title"].split()[0]
            variants = [b.get_text() for b in li.select("ul.variants button:not([disabled])")]
            badges = [s.get_text(strip=True) for s in li.find_all("span", class_=re.compile(r"^badge-"))]
            products.append((li["data-sku"], a.get_text(" ", strip=True), a["href"], price["content"],
                             float(price_re.search(old.get_text()).group().replace(",", "")) if old else None,
                             rating, tuple(variants), tuple(badges), len(li.find("p", class_="card-desc").get_text())))
        on_sale = len(soup.select("#grid > li.on-sale"))
        third = soup.select_one("#grid > li:nth-of-type(3) .card-title a").get_text(strip=True)
        specs = [tuple(td.get_text(strip=True) for td in tr.find_all(["th", "td"]))
                 for tr in soup.select("table.spec-table tbody tr")]
        nav = [(a.get_text(), a.get("class")) for a in soup.select("nav.main-nav a")]
        crumbs = [li.get_text(strip=True) for li in soup.select("nav.breadcrumbs li")]
        nxt = soup.find("a", rel="next")
        for s in soup.find_all(["script", "style"]):
            s.decompose()
        for img in soup.find_all("img"):
            fig = soup.new_tag("figure", attrs={"class": "media"})
            img.wrap(fig)
        text = soup.body.get_text(" ", strip=True)
        rec = (title, canonical, on_sale, third, len(products), products[:: 7], specs[:5], nav[n], crumbs,
               nxt.find_previous_sibling("a").get_text(), len(text), hashlib.sha256(text.encode()).hexdigest())
        if n % 6 == 0:
            rec += (hashlib.sha256(str(soup).encode()).hexdigest(),)
        if n % 12 == 0:
            rec += (hashlib.sha256(soup.find("ul", id="grid").li.prettify().encode()).hexdigest(),)
        out.append(rec)
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
