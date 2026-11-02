"""XML feed processing: parse a namespaced product feed, filter and transform items, build
and serialise a new feed, stream a large sitemap with ``iterparse``, and pretty-print a
small document with ``minidom``.

Stands for the XML that data scripts still meet every day (merchant/RSS feeds, sitemaps,
SOAP/XML exports, Maven/Android manifests): ``xml.etree.ElementTree.fromstring``,
``find``/``findall``/``iter`` with namespaces, ``.text``/``.get``, ``SubElement`` tree
building, ``ET.tostring``, ``ET.iterparse`` with ``elem.clear()``, and
``xml.dom.minidom.parseString(...).toprettyxml()``. Data shape: a 10,000-item feed
(~6 MB), a 60,000-URL sitemap (~8.7 MB), a 150-item document for minidom.
"""

REQUIRES: list[str] = []

N_ITEMS = 10_000
N_URLS = 60_000


def setup():
    import random
    from xml.sax.saxutils import escape

    rng = random.Random(3)
    cats = ["Electronics > Audio", "Home & Garden > Kitchen", "Apparel > Shoes",
            "Toys & Games", "Sporting Goods > Outdoor", "Books"]
    items = []
    for i in range(N_ITEMS):
        price = rng.uniform(2, 900)
        sale = f"<g:sale_price>{price * 0.8:.2f} EUR</g:sale_price>" if rng.random() < 0.3 else ""
        items.append(
            f"<item><g:id>P{i:06d}</g:id>"
            f"<title>{escape(f'Product {i} - {rng.choice(cats)} & more')}</title>"
            f"<link>https://shop.example.com/p/{i}?ref=feed&amp;lang=en</link>"
            f"<description>{escape('Great item. ' * rng.randint(2, 12))}</description>"
            f"<g:price>{price:.2f} EUR</g:price>{sale}"
            f"<g:availability>{rng.choice(['in stock', 'out of stock', 'preorder'])}"
            f"</g:availability>"
            f"<g:brand>Brand{rng.randint(1, 80)}</g:brand>"
            f"<g:google_product_category>{escape(rng.choice(cats))}</g:google_product_category>"
            f"<g:shipping><g:country>{rng.choice(['DE', 'FR', 'PL', 'US'])}</g:country>"
            f"<g:price>{rng.choice(['0.00', '4.99', '9.99'])} EUR</g:price></g:shipping>"
            f'<g:product_detail attr="weight" unit="kg">{rng.uniform(0.1, 30):.1f}'
            f"</g:product_detail></item>")
    feed = ('<?xml version="1.0" encoding="UTF-8"?>\n'
            '<rss version="2.0" xmlns:g="http://base.google.com/ns/1.0"><channel>'
            "<title>Example shop</title><link>https://shop.example.com</link>"
            + "".join(items) + "</channel></rss>")
    urls = "".join(
        f"<url><loc>https://www.example.com/{rng.choice(['blog', 'p', 'c', 'docs'])}/"
        f"{i}-{rng.getrandbits(24):x}</loc><lastmod>2026-{rng.randint(1, 10):02d}-"
        f"{rng.randint(1, 28):02d}</lastmod><changefreq>"
        f"{rng.choice(['daily', 'weekly', 'monthly'])}</changefreq>"
        f"<priority>{rng.choice(['0.3', '0.5', '0.8', '1.0'])}</priority></url>"
        for i in range(N_URLS))
    sitemap = ('<?xml version="1.0" encoding="UTF-8"?>\n'
               '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">'
               + urls + "</urlset>").encode()
    return {"feed": feed.encode(), "sitemap": sitemap}


def run(state):
    import io
    import xml.etree.ElementTree as ET
    from collections import Counter
    from xml.dom import minidom

    ns = {"g": "http://base.google.com/ns/1.0"}
    G = "{http://base.google.com/ns/1.0}"
    root = ET.fromstring(state["feed"])
    out_root = ET.Element("products", {"source": "feed", "version": "2"})
    brands = Counter()
    categories = Counter()
    total = 0.0
    kept = 0
    for item in root.iter("item"):
        avail = item.findtext("g:availability", namespaces=ns)
        price_txt = (item.findtext("g:sale_price", namespaces=ns)
                     or item.findtext("g:price", namespaces=ns))
        amount, _, currency = price_txt.partition(" ")
        price = float(amount)
        brand = item.find("g:brand", ns).text
        brands[brand] += 1
        top_cat = item.findtext("g:google_product_category", namespaces=ns).split(" > ")[0]
        categories[top_cat] += 1
        if avail == "out of stock":
            continue
        kept += 1
        total += price
        p = ET.SubElement(out_root, "product", id=item.findtext("g:id", namespaces=ns))
        ET.SubElement(p, "name").text = item.findtext("title").strip()
        ET.SubElement(p, "price", currency=currency).text = f"{price:.2f}"
        ET.SubElement(p, "brand").text = brand
        ET.SubElement(p, "category").text = top_cat
        ship = item.find("g:shipping", ns)
        ET.SubElement(p, "ship", country=ship.findtext("g:country", namespaces=ns)).text = (
            ship.findtext("g:price", namespaces=ns))
        for detail in item.iter(G + "product_detail"):
            ET.SubElement(p, "detail", name=detail.get("attr"),
                          unit=detail.get("unit")).text = detail.text
        desc = item.findtext("description") or ""
        ET.SubElement(p, "summary").text = desc[:60]
    out_xml = ET.tostring(out_root, encoding="unicode")

    sm = "{http://www.sitemaps.org/schemas/sitemap/0.9}"
    per_section = Counter()
    per_month = Counter()
    prio_sum = 0.0
    n_urls = 0
    for _event, elem in ET.iterparse(io.BytesIO(state["sitemap"]), events=("end",)):
        if elem.tag == sm + "url":
            loc = elem.findtext(sm + "loc")
            per_section[loc.split("/", 4)[3]] += 1
            per_month[elem.findtext(sm + "lastmod")[:7]] += 1
            prio_sum += float(elem.findtext(sm + "priority"))
            n_urls += 1
            elem.clear()

    small = ET.Element("report")
    for brand, n in brands.most_common(150):
        ET.SubElement(small, "brand", name=brand, count=str(n))
    pretty = minidom.parseString(ET.tostring(small)).toprettyxml(indent="  ")
    return {
        "kept": kept,
        "avg_price": round(total / kept, 4),
        "categories": sorted(categories.items()),
        "out_len": len(out_xml),
        "out_head": out_xml[:2000],
        "urls": n_urls,
        "sections": sorted(per_section.items()),
        "months": sorted(per_month.items()),
        "prio": round(prio_sum, 3),
        "pretty_len": len(pretty),
        "pretty_head": pretty[:1000],
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
