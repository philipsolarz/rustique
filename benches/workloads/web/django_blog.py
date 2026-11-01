"""A Django blog/content site serving page views through the full request cycle.

Stands for the most common Python web deployment: a server-rendered Django site. Each
request goes through the WSGI handler and a production middleware stack (security,
signed-cookie sessions, common, CSRF, auth, messages), URL resolving with path
converters, ORM queries over sqlite (``select_related``, ``prefetch_related``,
``annotate(Count)``, pagination, date filtering) that instantiate model objects, cached
Django templates with inheritance, ``include``, ``{% url %}``, filters (``date``,
``truncatewords``, ``linebreaksbr``, ``pluralize``) and autoescaping, ``ModelForm``
rendering and validation on a comment POST (redirect + messages), and a ``JsonResponse``
API endpoint. Driven by Django's test ``Client`` (in-process WSGI), 196 requests per run.
Data: 12 authors, 6 categories, 240 posts, ~1,400 comments in an in-memory sqlite DB.
"""

REQUIRES = ["Django==6.1.2", "asgiref==3.13.0", "sqlparse==0.6.0"]

_APP = "webbench_djblog"

TEMPLATES = {
    "base.html": """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>{% block title %}Field Notes{% endblock %}</title>
<link rel="stylesheet" href="/static/site.css"></head>
<body>
<header><a href="{% url 'index' %}">Field Notes</a>
<nav>{% for c in nav_categories %}<a href="{% url 'category' c.slug %}"{% if c.slug == active %} class="active"{% endif %}>{{ c.name }}</a>{% endfor %}</nav></header>
{% if messages %}<ul class="messages">{% for m in messages %}<li class="{{ m.tags }}">{{ m }}</li>{% endfor %}</ul>{% endif %}
<main>{% block content %}{% endblock %}</main>
<footer>&copy; {{ year }} Field Notes &middot; {{ post_total }} post{{ post_total|pluralize }}</footer>
</body></html>""",
    "post_card.html": """<article class="card">
<h2><a href="{% url 'post' post.slug %}">{{ post.title }}</a></h2>
<p class="meta">by {{ post.author.name }} in <a href="{% url 'category' post.category.slug %}">{{ post.category.name }}</a>
on <time datetime="{{ post.published|date:'c' }}">{{ post.published|date:"N j, Y" }}</time>
{% if post.n_comments is not None %}&middot; {{ post.n_comments }} comment{{ post.n_comments|pluralize }}{% endif %}</p>
<p>{{ post.body|truncatewords:40 }}</p>
{% if post.tags %}<ul class="tags">{% for t in post.tag_list %}<li>{{ t|lower }}</li>{% endfor %}</ul>{% endif %}
</article>""",
    "index.html": """{% extends "base.html" %}
{% block content %}<h1>Latest posts</h1>
{% for post in posts %}{% include "post_card.html" %}{% empty %}<p>No posts.</p>{% endfor %}
{% endblock %}""",
    "category.html": """{% extends "base.html" %}
{% block title %}{{ category.name }} - Field Notes{% endblock %}
{% block content %}<h1>{{ category.name }}</h1><p>{{ category.description }}</p>
{% for post in page_obj %}{% include "post_card.html" %}{% endfor %}
<nav class="pager">{% if page_obj.has_previous %}<a href="?page={{ page_obj.previous_page_number }}">newer</a>{% endif %}
Page {{ page_obj.number }} of {{ page_obj.paginator.num_pages }}
{% if page_obj.has_next %}<a href="?page={{ page_obj.next_page_number }}">older</a>{% endif %}</nav>
{% endblock %}""",
    "post.html": """{% extends "base.html" %}
{% block title %}{{ post.title }} - Field Notes{% endblock %}
{% block content %}<article><h1>{{ post.title }}</h1>
<p class="meta">by <a href="mailto:{{ post.author.email }}">{{ post.author.name }}</a>,
{{ post.published|date:"l, F j, Y H:i" }}</p>
<div class="body">{{ post.body|linebreaksbr }}</div></article>
<section id="comments"><h2>{{ comments|length }} comment{{ comments|length|pluralize }}</h2>
{% for c in comments %}<div class="comment"><p><strong>{{ c.name }}</strong> wrote on {{ c.created|date:"M j, Y" }}:</p>
<p>{{ c.body|linebreaksbr }}</p></div>{% endfor %}</section>
<form method="post" action="{% url 'comment' post.slug %}">{% csrf_token %}{{ form.as_p }}
<button type="submit">Post comment</button></form>
{% endblock %}""",
    "archive.html": """{% extends "base.html" %}
{% block content %}<h1>Archive {{ month|date:"F Y" }}</h1><ul>
{% for post in posts %}<li><a href="{% url 'post' post.slug %}">{{ post.title }}</a> ({{ post.published|date:"j M" }}, {{ post.author.name }})</li>{% endfor %}
</ul>{% endblock %}""",
}

WORDS = (
    "the quick brown fox jumps over lazy dog river mountain coffee rust python "
    "garden autumn library window travel music silent bright market harbor north "
    "station evening paper letter forest winter summer garden bicycle kitchen "
    "notebook camera journey village ocean bridge lantern meadow"
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

    _make_app_module()
    import django
    from django.conf import settings

    if not settings.configured:
        settings.configure(
            DEBUG=False,
            SECRET_KEY="webbench-not-secret-0123456789abcdefghijklmnopqrstuvwxyz",
            ALLOWED_HOSTS=["testserver"],
            ROOT_URLCONF=_APP,
            USE_TZ=True,
            TIME_ZONE="UTC",
            USE_I18N=True,
            DATABASES={"default": {"ENGINE": "django.db.backends.sqlite3", "NAME": ":memory:"}},
            INSTALLED_APPS=[
                "django.contrib.contenttypes",
                "django.contrib.auth",
                "django.contrib.sessions",
                "django.contrib.messages",
                _APP,
            ],
            MIDDLEWARE=[
                "django.middleware.security.SecurityMiddleware",
                "django.contrib.sessions.middleware.SessionMiddleware",
                "django.middleware.common.CommonMiddleware",
                "django.middleware.csrf.CsrfViewMiddleware",
                "django.contrib.auth.middleware.AuthenticationMiddleware",
                "django.contrib.messages.middleware.MessageMiddleware",
            ],
            SESSION_ENGINE="django.contrib.sessions.backends.signed_cookies",
            MESSAGE_STORAGE="django.contrib.messages.storage.cookie.CookieStorage",
            TEMPLATES=[{
                "BACKEND": "django.template.backends.django.DjangoTemplates",
                "OPTIONS": {
                    "context_processors": [
                        "django.template.context_processors.request",
                        "django.contrib.auth.context_processors.auth",
                        "django.contrib.messages.context_processors.messages",
                    ],
                    "loaders": [("django.template.loaders.cached.Loader", [
                        ("django.template.loaders.locmem.Loader", TEMPLATES)])],
                },
            }],
        )
        django.setup()

    from django import forms
    from django.db import connection, models

    class Author(models.Model):
        name = models.CharField(max_length=80)
        email = models.EmailField()

        class Meta:
            app_label = _APP

    class Category(models.Model):
        name = models.CharField(max_length=60)
        slug = models.SlugField(unique=True)
        description = models.TextField()

        class Meta:
            app_label = _APP

    class Post(models.Model):
        title = models.CharField(max_length=200)
        slug = models.SlugField(unique=True)
        author = models.ForeignKey(Author, on_delete=models.CASCADE)
        category = models.ForeignKey(Category, on_delete=models.CASCADE)
        body = models.TextField()
        tags = models.CharField(max_length=200, blank=True)
        published = models.DateTimeField(db_index=True)
        views = models.PositiveIntegerField(default=0)

        class Meta:
            app_label = _APP
            ordering = ["-published"]

        def tag_list(self):
            return [t for t in self.tags.split(",") if t]

    class Comment(models.Model):
        post = models.ForeignKey(Post, on_delete=models.CASCADE, related_name="comments")
        name = models.CharField(max_length=80)
        email = models.EmailField()
        body = models.TextField()
        created = models.DateTimeField()

        class Meta:
            app_label = _APP
            ordering = ["created"]

    class CommentForm(forms.ModelForm):
        class Meta:
            model = Comment
            fields = ["name", "email", "body"]
            widgets = {"body": forms.Textarea(attrs={"rows": 4})}

        def clean_body(self):
            body = self.cleaned_data["body"].strip()
            if len(body) < 5:
                raise forms.ValidationError("Comment is too short.")
            return body

    with connection.schema_editor() as editor:
        for model in (Author, Category, Post, Comment):
            editor.create_model(model)

    rng = random.Random(42)
    tz = dt.timezone.utc

    def sentence(n):
        return " ".join(rng.choice(WORDS) for _ in range(n)).capitalize() + "."

    authors = Author.objects.bulk_create(
        [Author(name=f"Author {i:02d} {rng.choice(WORDS).title()}", email=f"author{i}@example.com")
         for i in range(12)])
    cats = Category.objects.bulk_create(
        [Category(name=w.title(), slug=w, description=sentence(12))
         for w in ("travel", "coffee", "python", "garden", "music", "notes")])
    base = dt.datetime(2024, 1, 1, 8, 0, tzinfo=tz)
    posts = []
    for i in range(240):
        paras = "\n\n".join(" ".join(sentence(rng.randint(8, 18)) for _ in range(4)) for _ in range(3))
        posts.append(Post(
            title=sentence(rng.randint(4, 8))[:-1], slug=f"post-{i:04d}", author=rng.choice(authors),
            category=rng.choice(cats), body=paras,
            tags=",".join(sorted(set(rng.choice(WORDS) for _ in range(rng.randint(0, 4))))),
            published=base + dt.timedelta(hours=37 * i + rng.randint(0, 30)),
            views=rng.randint(0, 5000)))
    posts = Post.objects.bulk_create(posts)
    comments = []
    for p in posts:
        for j in range(rng.randint(0, 11)):
            comments.append(Comment(post=p, name=rng.choice(WORDS).title() + f" {j}",
                                    email=f"reader{rng.randint(1, 500)}@example.org",
                                    body=sentence(rng.randint(6, 30)),
                                    created=p.published + dt.timedelta(hours=j * 5 + 1)))
    Comment.objects.bulk_create(comments)

    from django.contrib import messages
    from django.core.paginator import Paginator
    from django.db.models import Count
    from django.http import Http404, JsonResponse
    from django.shortcuts import redirect, render
    from django.urls import path

    fixed_created = dt.datetime(2025, 6, 1, 12, 0, tzinfo=tz)

    def common_ctx(active=None):
        return {"nav_categories": list(Category.objects.order_by("name")), "active": active,
                "year": 2025, "post_total": Post.objects.count()}

    def index(request):
        qs = (Post.objects.select_related("author", "category")
              .annotate(n_comments=Count("comments")).order_by("-published")[:20])
        return render(request, "index.html", {"posts": qs, **common_ctx()})

    def category(request, slug):
        cat = Category.objects.filter(slug=slug).first()
        if cat is None:
            raise Http404("no category")
        qs = Post.objects.filter(category=cat).select_related("author", "category").order_by("-published")
        page = Paginator(qs, 10).get_page(request.GET.get("page"))
        return render(request, "category.html", {"category": cat, "page_obj": page, **common_ctx(slug)})

    def post_detail(request, slug, form=None):
        post = Post.objects.select_related("author", "category").filter(slug=slug).first()
        if post is None:
            raise Http404("no post")
        comments = list(post.comments.all())
        return render(request, "post.html", {"post": post, "comments": comments,
                                             "form": form or CommentForm(), **common_ctx()})

    def comment(request, slug):
        post = Post.objects.filter(slug=slug).first()
        if post is None:
            raise Http404("no post")
        form = CommentForm(request.POST)
        if form.is_valid():
            c = form.save(commit=False)
            c.post = post
            c.created = fixed_created
            c.save()
            messages.success(request, "Thanks for your comment, %s!" % c.name)
            return redirect("post", slug=slug)
        return post_detail(request, slug, form=form)

    def archive(request, year, month):
        start = dt.datetime(year, month, 1, tzinfo=tz)
        posts = (Post.objects.filter(published__year=year, published__month=month)
                 .select_related("author").order_by("published"))
        return render(request, "archive.html", {"posts": posts, "month": start, **common_ctx()})

    def api_posts(request):
        limit = int(request.GET.get("limit", 25))
        rows = list(Post.objects.filter(category__slug=request.GET.get("category", "python"))
                    .values("id", "slug", "title", "published", "views", "author__name")[:limit])
        return JsonResponse({"count": len(rows), "results": rows})

    import sys

    sys.modules[_APP].urlpatterns = [
        path("", index, name="index"),
        path("category/<slug:slug>/", category, name="category"),
        path("post/<slug:slug>/", post_detail, name="post"),
        path("post/<slug:slug>/comment/", comment, name="comment"),
        path("archive/<int:year>/<int:month>/", archive, name="archive"),
        path("api/posts.json", api_posts, name="api_posts"),
    ]

    from django.test import Client

    plan = []
    for i in range(196):
        k = i % 14
        if k in (0, 7):
            plan.append(("get", "/", None))
        elif k in (1, 8, 12):
            plan.append(("get", f"/post/post-{(i * 37) % 240:04d}/", None))
        elif k in (2, 9):
            c = ("travel", "coffee", "python", "garden", "music", "notes")[i % 6]
            plan.append(("get", f"/category/{c}/?page={1 + (i // 14) % 4}", None))
        elif k == 3:
            p = f"post-{(i * 11) % 240:04d}"
            plan.append(("post", f"/post/{p}/comment/",
                         {"name": f"Visitor {i}", "email": f"v{i}@example.net",
                          "body": f"Great post number {i}, thanks for writing it up!"}))
            plan.append(("get", f"/post/{p}/", None))
        elif k == 4:
            plan.append(("post", f"/post/post-{(i * 13) % 240:04d}/comment/",
                         {"name": "", "email": "not-an-email", "body": "hm"}))
        elif k in (5, 10):
            plan.append(("get", f"/archive/{2024 + (i // 70)}/{1 + (i // 14) % 12}/", None))
        elif k in (6, 11):
            c = ("python", "travel", "music")[i % 3]
            plan.append(("get", f"/api/posts.json?category={c}&limit=25", None))
        else:
            plan.append(("get", "/category/no-such-thing/", None))
    return {"client": Client(), "plan": plan}


def run(state):
    import hashlib
    import re

    from django.db import transaction

    client = state["client"]
    client.cookies.clear()
    token_re = re.compile(rb'name="csrfmiddlewaretoken" value="[^"]+"')
    out = []
    with transaction.atomic():
        for method, url, data in state["plan"]:
            if method == "get":
                resp = client.get(url)
            else:
                resp = client.post(url, data)
            body = token_re.sub(b"TOKEN", resp.content)
            out.append((url, resp.status_code, len(body), hashlib.sha256(body).hexdigest()))
        transaction.set_rollback(True)
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
