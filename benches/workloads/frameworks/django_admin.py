"""The Django admin of a small shop: changelists, change forms with inlines, actions.

Stands for the most-used server-rendered page set in the Python world: nearly every
Django project ships ``django.contrib.admin`` as its back office, and it is Django's most
template-heavy code (nested ``{% include %}``/inclusion tags, form and widget templates,
``admin_list`` result rows, filters, i18n strings, ``format_html``). A logged-in staff user
browses and edits a shop: admin index and app index, product and order changelists with
``list_display`` callables, ``list_filter`` (related, choices, boolean, date), search,
``date_hierarchy``, sorting and pagination; change forms (fieldsets, ``readonly_fields``,
``raw_id_fields``, a ``TabularInline`` of order lines), the add form, a delete
confirmation (related-object collection), POSTs that validate and save a product and an
order with its inline formset (``LogEntry`` + messages + redirect) and a bulk changelist
action. Driven by Django's test ``Client`` (in-process WSGI) with ``force_login``,
108 requests per run, rolled back so the run repeats exactly. Data: 40 customers,
8 categories, 500 products, 600 orders with ~2,100 lines in an in-memory sqlite DB.
"""

REQUIRES = ["Django==6.1.2", "asgiref==3.13.0", "sqlparse==0.6.0"]

_APP = "fwbench_shop"

WORDS = (
    "oak linen copper glass walnut steel cotton wool ceramic bamboo marble leather "
    "lamp chair table mug bowl vase shelf rug pillow clock mirror basket kettle "
    "small large classic modern rustic nordic deluxe compact"
).split()


def _make_app_module():
    import os
    import sys
    import types

    if _APP in sys.modules:
        return
    mod = types.ModuleType(_APP)
    mod.__file__ = os.path.abspath(__file__)
    sys.modules[_APP] = mod


def setup():
    import datetime as dt
    import random
    import sys
    from decimal import Decimal

    _make_app_module()
    import django
    from django.conf import settings

    if not settings.configured:
        settings.configure(
            DEBUG=False,
            SECRET_KEY="fwbench-not-secret-0123456789abcdefghijklmnopqrstuvwxyz",
            ALLOWED_HOSTS=["testserver"],
            ROOT_URLCONF=_APP,
            USE_TZ=True,
            TIME_ZONE="UTC",
            USE_I18N=True,
            LANGUAGE_CODE="en-us",
            STATIC_URL="/static/",
            DATABASES={"default": {"ENGINE": "django.db.backends.sqlite3", "NAME": ":memory:"}},
            INSTALLED_APPS=[
                "django.contrib.admin",
                "django.contrib.auth",
                "django.contrib.contenttypes",
                "django.contrib.sessions",
                "django.contrib.messages",
                "django.contrib.staticfiles",
                _APP,
            ],
            MIDDLEWARE=[
                "django.middleware.security.SecurityMiddleware",
                "django.contrib.sessions.middleware.SessionMiddleware",
                "django.middleware.common.CommonMiddleware",
                "django.middleware.csrf.CsrfViewMiddleware",
                "django.contrib.auth.middleware.AuthenticationMiddleware",
                "django.contrib.messages.middleware.MessageMiddleware",
                "django.middleware.clickjacking.XFrameOptionsMiddleware",
            ],
            SESSION_ENGINE="django.contrib.sessions.backends.signed_cookies",
            MESSAGE_STORAGE="django.contrib.messages.storage.cookie.CookieStorage",
            PASSWORD_HASHERS=["django.contrib.auth.hashers.MD5PasswordHasher"],
            TEMPLATES=[{
                "BACKEND": "django.template.backends.django.DjangoTemplates",
                "APP_DIRS": True,
                "OPTIONS": {"context_processors": [
                    "django.template.context_processors.request",
                    "django.contrib.auth.context_processors.auth",
                    "django.contrib.messages.context_processors.messages",
                ]},
            }],
        )
        django.setup()

    from django.contrib import admin, messages
    from django.contrib.auth.models import User
    from django.core.management import call_command
    from django.db import models
    from django.db.models import Count, F, Sum
    from django.urls import path
    from django.utils.html import format_html

    class Category(models.Model):
        name = models.CharField(max_length=60)
        slug = models.SlugField(unique=True)

        class Meta:
            app_label = _APP
            verbose_name_plural = "categories"
            ordering = ["name"]

        def __str__(self):
            return self.name

    class Customer(models.Model):
        name = models.CharField(max_length=80)
        email = models.EmailField(unique=True)
        city = models.CharField(max_length=60)
        vip = models.BooleanField(default=False)

        class Meta:
            app_label = _APP
            ordering = ["name"]

        def __str__(self):
            return self.name

    class Product(models.Model):
        name = models.CharField(max_length=120)
        sku = models.CharField(max_length=20, unique=True)
        category = models.ForeignKey(Category, on_delete=models.PROTECT, related_name="products")
        price = models.DecimalField(max_digits=8, decimal_places=2)
        stock = models.PositiveIntegerField(default=0)
        active = models.BooleanField(default=True)
        description = models.TextField(blank=True)
        added = models.DateField()

        class Meta:
            app_label = _APP
            ordering = ["name", "id"]

        def __str__(self):
            return f"{self.name} ({self.sku})"

    class Order(models.Model):
        STATUS = [("new", "New"), ("paid", "Paid"), ("shipped", "Shipped"), ("cancelled", "Cancelled")]
        customer = models.ForeignKey(Customer, on_delete=models.CASCADE, related_name="orders")
        status = models.CharField(max_length=10, choices=STATUS, default="new")
        created = models.DateTimeField()
        note = models.TextField(blank=True)

        class Meta:
            app_label = _APP
            ordering = ["-created", "id"]

        def __str__(self):
            return f"Order #{self.pk}"

    class OrderLine(models.Model):
        order = models.ForeignKey(Order, on_delete=models.CASCADE, related_name="lines")
        product = models.ForeignKey(Product, on_delete=models.PROTECT, related_name="lines")
        quantity = models.PositiveIntegerField(default=1)
        unit_price = models.DecimalField(max_digits=8, decimal_places=2)

        class Meta:
            app_label = _APP
            ordering = ["id"]

        def __str__(self):
            return f"{self.quantity} x {self.product_id}"

    @admin.register(Category)
    class CategoryAdmin(admin.ModelAdmin):
        list_display = ["name", "slug", "n_products"]
        prepopulated_fields = {"slug": ["name"]}

        def get_queryset(self, request):
            return super().get_queryset(request).annotate(_n=Count("products"))

        @admin.display(description="Products", ordering="_n")
        def n_products(self, obj):
            return obj._n

    @admin.register(Customer)
    class CustomerAdmin(admin.ModelAdmin):
        list_display = ["name", "email", "city", "vip"]
        list_filter = ["vip", "city"]
        search_fields = ["name", "email"]

    @admin.register(Product)
    class ProductAdmin(admin.ModelAdmin):
        list_display = ["name", "sku", "category", "price", "stock", "active", "stock_badge"]
        list_filter = ["active", "category", "added"]
        search_fields = ["name", "sku", "description"]
        list_select_related = ["category"]
        list_per_page = 50
        date_hierarchy = "added"
        ordering = ["name", "id"]
        fieldsets = [
            (None, {"fields": ["name", "sku", "category"]}),
            ("Pricing and stock", {"fields": ["price", "stock", "active"]}),
            ("Details", {"classes": ["collapse"], "fields": ["description", "added"]}),
        ]
        actions = ["deactivate"]

        @admin.display(description="Stock")
        def stock_badge(self, obj):
            color = "red" if obj.stock < 5 else ("orange" if obj.stock < 20 else "green")
            return format_html('<span style="color:{}">{} in stock</span>', color, obj.stock)

        @admin.action(description="Deactivate selected products")
        def deactivate(self, request, queryset):
            n = queryset.update(active=False)
            self.message_user(request, f"{n} products deactivated.", messages.SUCCESS)

    class OrderLineInline(admin.TabularInline):
        model = OrderLine
        raw_id_fields = ["product"]
        extra = 1
        fields = ["product", "quantity", "unit_price", "line_total"]
        readonly_fields = ["line_total"]

        @admin.display(description="Total")
        def line_total(self, obj):
            if obj.pk is None:
                return "-"
            return f"{obj.quantity * obj.unit_price:.2f}"

    @admin.register(Order)
    class OrderAdmin(admin.ModelAdmin):
        list_display = ["__str__", "customer", "status", "created", "n_lines", "order_total", "colored_status"]
        list_filter = ["status", "created", "customer__vip"]
        search_fields = ["customer__name", "customer__email", "note"]
        list_select_related = ["customer"]
        raw_id_fields = ["customer"]
        date_hierarchy = "created"
        inlines = [OrderLineInline]
        list_per_page = 40
        actions = ["mark_shipped"]
        readonly_fields = ["order_total"]

        def get_queryset(self, request):
            return super().get_queryset(request).annotate(
                _n=Count("lines"), _total=Sum(F("lines__quantity") * F("lines__unit_price")))

        @admin.display(description="Lines", ordering="_n")
        def n_lines(self, obj):
            return obj._n

        @admin.display(description="Total", ordering="_total")
        def order_total(self, obj):
            return f"{obj._total or 0:.2f}"

        @admin.display(description="Status")
        def colored_status(self, obj):
            colors = {"new": "#888", "paid": "#06c", "shipped": "#090", "cancelled": "#c00"}
            return format_html('<b style="color:{}">{}</b>', colors[obj.status], obj.get_status_display())

        @admin.action(description="Mark selected orders as shipped")
        def mark_shipped(self, request, queryset):
            n = queryset.filter(status="paid").update(status="shipped")
            self.message_user(request, f"{n} orders marked as shipped.", messages.SUCCESS)

    sys.modules[_APP].urlpatterns = [path("admin/", admin.site.urls)]
    call_command("migrate", verbosity=0)
    from django.db import connection

    with connection.schema_editor() as editor:
        for model in (Category, Customer, Product, Order, OrderLine):
            editor.create_model(model)
    from django.contrib.contenttypes.models import ContentType

    for model in (Category, Customer, Product, Order, OrderLine):  # outside the per-run transaction
        ContentType.objects.get_for_model(model)

    # ---- data -------------------------------------------------------------------------
    rng = random.Random(3)
    staff = User.objects.create_superuser("admin", "admin@example.com", "pw")
    cats = Category.objects.bulk_create(
        [Category(name=w.title() + " goods", slug=w) for w in WORDS[:8]])
    custs = Customer.objects.bulk_create(
        [Customer(name=f"{rng.choice(WORDS).title()} {rng.choice(WORDS).title()} {i}",
                  email=f"c{i}@example.com", city=rng.choice(["Berlin", "Lyon", "Oslo", "Porto", "Turin"]),
                  vip=i % 7 == 0) for i in range(40)])
    prods = Product.objects.bulk_create(
        [Product(name=" ".join(rng.choice(WORDS).title() for _ in range(3)), sku=f"SKU-{i:05d}",
                 category=cats[i % 8], price=Decimal(rng.randint(299, 49999)) / 100,
                 stock=rng.randint(0, 120), active=rng.random() < 0.9,
                 description=" ".join(rng.choice(WORDS) for _ in range(rng.randint(5, 30))),
                 added=dt.date(2023, 1, 1) + dt.timedelta(days=rng.randint(0, 900)))
         for i in range(500)])
    base = dt.datetime(2024, 1, 1, 9, 0, tzinfo=dt.timezone.utc)
    orders = Order.objects.bulk_create(
        [Order(customer=custs[rng.randrange(40)], status=rng.choice(["new", "paid", "paid", "shipped", "cancelled"]),
               created=base + dt.timedelta(hours=13 * i + rng.randint(0, 12)),
               note=rng.choice(["", "", "gift", "call before delivery", "fragile"])) for i in range(600)])
    lines = []
    for o in orders:
        for _ in range(rng.randint(1, 6)):
            p = prods[rng.randrange(500)]
            lines.append(OrderLine(order=o, product=p, quantity=rng.randint(1, 4), unit_price=p.price))
    OrderLine.objects.bulk_create(lines)
    lines_by_order = {}
    for ln in OrderLine.objects.order_by("id").values("id", "order_id", "product_id", "quantity", "unit_price"):
        lines_by_order.setdefault(ln["order_id"], []).append(ln)

    def order_post(o, i):
        ls = lines_by_order[o.id]
        data = {"customer": str(o.customer_id), "status": "paid", "note": f"edited {i}",
                "created_0": o.created.strftime("%Y-%m-%d"), "created_1": o.created.strftime("%H:%M:%S"),
                "lines-TOTAL_FORMS": str(len(ls) + 1), "lines-INITIAL_FORMS": str(len(ls)),
                "lines-MIN_NUM_FORMS": "0", "lines-MAX_NUM_FORMS": "1000", "_save": "Save"}
        for j, ln in enumerate(ls):
            data.update({f"lines-{j}-id": str(ln["id"]), f"lines-{j}-order": str(o.id),
                         f"lines-{j}-product": str(ln["product_id"]),
                         f"lines-{j}-quantity": str(ln["quantity"] + (j == 0)),
                         f"lines-{j}-unit_price": str(ln["unit_price"])})
        n = len(ls)
        data.update({f"lines-{n}-id": "", f"lines-{n}-order": str(o.id), f"lines-{n}-product": str(prods[i].id),
                     f"lines-{n}-quantity": "2", f"lines-{n}-unit_price": str(prods[i].price)})
        return data

    app = _APP
    plan = []
    for i in range(48):
        k = i % 8
        if k == 0:
            plan.append(("get", "/admin/", None))
            plan.append(("get", f"/admin/{app}/", None))
        elif k == 1:
            q = ["", "?active__exact=1&o=4", f"?q={WORDS[i % 30]}", "?added__year=2024&p=1",
                 f"?category__id__exact={cats[i % 8].id}", "?o=-5.1&p=3"][i % 6]
            plan.append(("get", f"/admin/{app}/product/{q}", None))
        elif k == 2:
            q = ["", "?status__exact=paid", "?created__year=2024&created__month=3", "?q=Oak",
                 "?customer__vip__exact=1&o=-6", "?p=4"][i % 6]
            plan.append(("get", f"/admin/{app}/order/{q}", None))
        elif k == 3:
            p = prods[(i * 37) % 500]
            plan.append(("get", f"/admin/{app}/product/{p.id}/change/", None))
            plan.append(("post", f"/admin/{app}/product/{p.id}/change/", {
                "name": p.name + " v2", "sku": p.sku, "category": str(p.category_id),
                "price": str(p.price), "stock": str(p.stock + 1), "active": "on",
                "description": p.description, "added": p.added.isoformat(), "_save": "Save"}))
            plan.append(("get", f"/admin/{app}/product/?q={p.sku}", None))
        elif k == 4:
            o = orders[(i * 53) % 600]
            plan.append(("get", f"/admin/{app}/order/{o.id}/change/", None))
            plan.append(("post", f"/admin/{app}/order/{o.id}/change/", order_post(o, i)))
            plan.append(("get", f"/admin/{app}/order/{o.id}/change/", None))
        elif k == 5:
            plan.append(("get", f"/admin/{app}/product/add/", None))
            plan.append(("get", f"/admin/{app}/customer/?vip__exact=1", None))
            plan.append(("get", f"/admin/{app}/category/", None))
        elif k == 6:
            o = orders[(i * 7) % 600]
            plan.append(("get", f"/admin/{app}/order/{o.id}/delete/", None))
            ids = [str(x.id) for x in orders[i * 10:i * 10 + 30]]
            plan.append(("post", f"/admin/{app}/order/", {"action": "mark_shipped", "_selected_action": ids,
                                                           "index": "0"}))
            plan.append(("get", f"/admin/{app}/order/?status__exact=shipped", None))
        else:
            plan.append(("get", f"/admin/{app}/customer/{custs[i % 40].id}/change/", None))
            plan.append(("get", f"/admin/{app}/product/?p={1 + i % 10}", None))

    from django.test import Client

    client = Client()
    client.force_login(staff)
    return {"client": client, "plan": plan, "session": dict(client.cookies.items())}


def run(state):
    import hashlib
    import re

    from django.db import transaction

    client = state["client"]
    # each run starts with the same logged-in session and no pending messages
    for k in list(client.cookies.keys()):
        if k != "sessionid":
            del client.cookies[k]
    token_re = re.compile(rb'name="csrfmiddlewaretoken" value="[^"]+"')
    out = []
    with transaction.atomic():
        for method, url, data in state["plan"]:
            resp = client.get(url) if method == "get" else client.post(url, data)
            body = token_re.sub(b"TOKEN", resp.content)
            out.append((method, url, resp.status_code, resp.get("Location", ""), len(body),
                        hashlib.sha256(body).hexdigest()))
        transaction.set_rollback(True)
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
