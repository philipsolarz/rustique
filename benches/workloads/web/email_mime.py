"""Transactional e-mail: build templated MIME messages and parse inbound ones (stdlib ``email``).

Stands for what web apps do around e-mail (order confirmations, password resets,
notifications, support-inbox processing; Django's and Flask-Mail's ``send_mail`` build on
the same package): ``string.Template`` text bodies, ``EmailMessage`` with the modern
policy -- structured headers (display-name addresses incl. non-ASCII names, RFC 2047
encoded subjects, ``formatdate``), ``set_content`` + ``add_alternative`` (HTML) +
``add_attachment`` (PDF invoice, CSV), serialization with ``as_bytes`` (base64 and
quoted-printable transfer encodings) -- and the inbound side: ``BytesParser`` with
``policy.default``, address/subject header parsing, ``get_body``/``iter_attachments``/
``get_content``. 240 messages (120 outbound, 120 inbound) built and parsed per run.
"""

REQUIRES: list[str] = []

N_MESSAGES = 120


def setup():
    import random
    import string

    rng = random.Random(9)
    names = ["Ana Müller", "Bjørn Ødegård", "Chloé Dubois", "Dmitri Ivanov", "Eve Smith", "Farah Khan",
             "Grzegorz Brzęczyszczykiewicz", "Hiro Tanaka", "Ines García", "Jon O'Neil"]
    products = ["Walnut desk", "Linen throw", "Copper kettle", "Wool rug – large", "Glass vase", "Oak stool"]
    text_t = string.Template(
        "Hi $first,\n\nthanks for your order #$order placed on $date.\n\n$lines\n\n"
        "Subtotal: $subtotal\nShipping: $shipping\nTotal: $total\n\n"
        "We'll let you know when it ships to:\n$address\n\n"
        "Questions? Just reply to this e-mail – we're happy to help.\n\n-- \nNorthwind Trading\n")
    html_t = string.Template(
        "<!doctype html><html><body style=\"font-family:sans-serif\"><h1>Thanks, $first!</h1>"
        "<p>Your order <b>#$order</b> placed on $date is confirmed.</p><table>$rows</table>"
        "<p>Total: <b>$total</b></p><p>Shipping to:<br>$address_html</p>"
        "<p style=\"color:#888\">Northwind Trading · Unsubscribe: https://shop.example/u/$token</p></body></html>")
    orders = []
    for i in range(N_MESSAGES):
        name = rng.choice(names)
        lines = [(rng.choice(products), rng.randint(1, 3), rng.randint(500, 40000) / 100) for _ in range(rng.randint(1, 6))]
        orders.append({"i": i, "name": name, "email": f"user{i}@example.{rng.choice(['com', 'de', 'pl', 'jp'])}",
                       "lines": lines, "address": f"{rng.randint(1, 200)} {rng.choice(['High St', 'Rue de Rivoli', 'ul. Długa'])}\n{rng.randint(10000, 99999)} {rng.choice(['Berlin', 'Paris', 'Gdańsk', 'Kyoto'])}",
                       "pdf": bytes(rng.getrandbits(8) for _ in range(rng.randint(2000, 12000)))})
    inbound = []
    for i in range(N_MESSAGES):
        name = rng.choice(names)
        body = ("> On Monday you wrote:\n> " + "\n> ".join("quoted line %d of the previous message" % k for k in range(rng.randint(2, 12)))
                + "\n\n" + "Hello, I have a question about my order #%d. Ünïcödé text included. " % (1000 + i) * rng.randint(1, 5))
        inbound.append({"name": name, "addr": f"customer{i}@mail.example.org", "subject": f"Re: Order #{1000 + i} – question",
                        "body": body, "csv": "sku,qty\n" + "".join(f"SKU-{k:04d},{k % 7}\n" for k in range(rng.randint(5, 80)))})
    return {"orders": orders, "inbound": inbound, "text_t": text_t, "html_t": html_t}


def run(state):
    import hashlib
    import html
    import random
    from email import policy
    from email.headerregistry import Address
    from email.message import EmailMessage
    from email.parser import BytesParser
    from email.utils import formatdate, parseaddr

    random.seed(20251101)  # MIME boundaries come from the random module
    text_t, html_t = state["text_t"], state["html_t"]
    smtp = policy.SMTP
    built = []
    for o in state["orders"]:
        i = o["i"]
        first = o["name"].split()[0]
        subtotal = sum(q * p for _, q, p in o["lines"])
        shipping = 0.0 if subtotal > 100 else 7.5
        date = formatdate(1735689600 + i * 3600, usegmt=True)
        msg = EmailMessage(policy=smtp)
        msg["Subject"] = f"Your order #{1000 + i} is confirmed ✓"
        msg["From"] = Address("Northwind Trading", "orders", "shop.example")
        local, domain = o["email"].split("@")
        msg["To"] = Address(o["name"], local, domain)
        msg["Date"] = date
        msg["Message-ID"] = f"<order-{1000 + i}.{i * 7919}@shop.example>"
        msg["List-Unsubscribe"] = f"<https://shop.example/u/{i:08x}>"
        lines = "\n".join(f"  {q} x {name:<20} {p * q:>9.2f} EUR" for name, q, p in o["lines"])
        msg.set_content(text_t.substitute(first=first, order=1000 + i, date=date, lines=lines,
                                          subtotal=f"{subtotal:.2f} EUR", shipping=f"{shipping:.2f} EUR",
                                          total=f"{subtotal + shipping:.2f} EUR", address=o["address"]))
        rows = "".join(f"<tr><td>{html.escape(name)}</td><td>{q}</td><td>{p * q:.2f}</td></tr>" for name, q, p in o["lines"])
        msg.add_alternative(html_t.substitute(first=html.escape(first), order=1000 + i, date=date, rows=rows,
                                              total=f"{subtotal + shipping:.2f} EUR",
                                              address_html=html.escape(o["address"]).replace("\n", "<br>"),
                                              token=f"{i:08x}"), subtype="html")
        msg.add_attachment(o["pdf"], maintype="application", subtype="pdf", filename=f"invoice-{1000 + i}.pdf")
        built.append(msg.as_bytes())
    for k, m in enumerate(state["inbound"]):
        msg = EmailMessage(policy=smtp)
        msg["From"] = f"{m['name']} <{m['addr']}>"
        msg["To"] = "Support <support@shop.example>"
        msg["Subject"] = m["subject"]
        msg["Date"] = formatdate(1735689600 + k * 600, usegmt=True)
        msg["In-Reply-To"] = f"<order-{1000 + k}.{k * 7919}@shop.example>"
        msg.set_content(m["body"], cte="quoted-printable")
        msg.add_attachment(m["csv"], subtype="csv", filename="items.csv")
        built.append(msg.as_bytes())

    parser = BytesParser(policy=policy.default)
    out = []
    for raw in built:
        msg = parser.parsebytes(raw)
        frm = msg["from"].addresses[0]
        to = msg["to"].addresses[0]
        body = msg.get_body(preferencelist=("plain", "html"))
        text = body.get_content()
        atts = [(a.get_filename(), a.get_content_type(), len(a.get_content())) for a in msg.iter_attachments()]
        out.append((str(msg["subject"]), frm.display_name, frm.addr_spec, to.domain,
                    parseaddr(str(msg["to"]))[1], len(text), text.count("\n"), atts,
                    hashlib.sha256(raw).hexdigest()[:16]))
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
