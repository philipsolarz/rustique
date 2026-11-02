"""A Django REST framework JSON API: viewsets, nested serializers, filters, token auth.

Stands for the most common way Python teams build a JSON API on Django: Django REST
framework (DRF, ~3% of all Django projects' dependencies and the default Django API
stack). An editorial API for articles, authors, tags and comments: a ``DefaultRouter``
with ``ModelViewSet``/``ReadOnlyModelViewSet`` and ``@action`` routes, ``ModelSerializer``
classes with nested serializers, ``SlugRelatedField``/``PrimaryKeyRelatedField``,
``HyperlinkedIdentityField`` (URL reversing), ``SerializerMethodField``, field and
object validation; ``TokenAuthentication`` (a DB lookup per request), object
permissions, ``PageNumberPagination``, django-filter's ``DjangoFilterBackend`` plus
``SearchFilter`` and ``OrderingFilter``; JSON request parsing and rendering; the
production middleware stack. Every request is real: it goes through the WSGI handler,
middleware, URL resolving, content negotiation, auth, throttling-free permission checks,
ORM queries over sqlite (``select_related``, ``prefetch_related``, ``annotate(Count)``)
and serializer output. Driven by DRF's ``APIClient`` (in-process WSGI), 360 requests per
run (lists with filters/search/ordering, detail pages, creates, partial updates,
comments, deletes, 400/401/403/404 paths); the run is rolled back so it repeats exactly.
Data: 10 authors, 16 tags, 300 articles, ~1,500 comments in an in-memory sqlite DB.
"""

REQUIRES = ["Django==6.1.2", "djangorestframework==3.18.3", "django-filter==26.2",
            "asgiref==3.13.0", "sqlparse==0.6.0"]

_APP = "fwbench_drf"

WORDS = (
    "api rest django model view serializer token router field query filter search order "
    "page cache index schema request response header client server python rust fast slow "
    "green blue river mountain coffee paper winter summer forest harbor market station"
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
            DATABASES={"default": {"ENGINE": "django.db.backends.sqlite3", "NAME": ":memory:"}},
            INSTALLED_APPS=[
                "django.contrib.contenttypes",
                "django.contrib.auth",
                "django.contrib.sessions",
                "rest_framework",
                "rest_framework.authtoken",
                "django_filters",
                _APP,
            ],
            MIDDLEWARE=[
                "django.middleware.security.SecurityMiddleware",
                "django.contrib.sessions.middleware.SessionMiddleware",
                "django.middleware.common.CommonMiddleware",
                "django.middleware.csrf.CsrfViewMiddleware",
                "django.contrib.auth.middleware.AuthenticationMiddleware",
                "django.middleware.clickjacking.XFrameOptionsMiddleware",
            ],
            SESSION_ENGINE="django.contrib.sessions.backends.signed_cookies",
            PASSWORD_HASHERS=["django.contrib.auth.hashers.MD5PasswordHasher"],
            TEMPLATES=[{"BACKEND": "django.template.backends.django.DjangoTemplates",
                        "APP_DIRS": True, "OPTIONS": {}}],
            REST_FRAMEWORK={
                "DEFAULT_AUTHENTICATION_CLASSES": [
                    "rest_framework.authentication.TokenAuthentication",
                    "rest_framework.authentication.SessionAuthentication",
                ],
                "DEFAULT_PERMISSION_CLASSES": [
                    "rest_framework.permissions.IsAuthenticatedOrReadOnly"],
                "DEFAULT_PAGINATION_CLASS": "rest_framework.pagination.PageNumberPagination",
                "PAGE_SIZE": 20,
                "DEFAULT_FILTER_BACKENDS": [
                    "django_filters.rest_framework.DjangoFilterBackend",
                    "rest_framework.filters.SearchFilter",
                    "rest_framework.filters.OrderingFilter",
                ],
                "TEST_REQUEST_DEFAULT_FORMAT": "json",
            },
        )
        django.setup()

    from django.contrib.auth.models import User
    from django.db import connection, models
    from django.db.models import Count
    from rest_framework import permissions, serializers, status, viewsets
    from rest_framework.authtoken.models import Token
    from rest_framework.decorators import action
    from rest_framework.response import Response
    from rest_framework.routers import DefaultRouter

    class Author(models.Model):
        user = models.OneToOneField(User, on_delete=models.CASCADE, related_name="author")
        name = models.CharField(max_length=80)
        bio = models.TextField(blank=True)
        joined = models.DateField()

        class Meta:
            app_label = _APP
            ordering = ["id"]

    class Tag(models.Model):
        name = models.CharField(max_length=40, unique=True)
        slug = models.SlugField(unique=True)

        class Meta:
            app_label = _APP
            ordering = ["slug"]

    class Article(models.Model):
        DRAFT, PUBLISHED, ARCHIVED = "draft", "published", "archived"
        STATUS = [(DRAFT, "Draft"), (PUBLISHED, "Published"), (ARCHIVED, "Archived")]
        title = models.CharField(max_length=200)
        slug = models.SlugField(unique=True, max_length=220)
        author = models.ForeignKey(Author, on_delete=models.CASCADE, related_name="articles")
        tags = models.ManyToManyField(Tag, related_name="articles", blank=True)
        body = models.TextField()
        status = models.CharField(max_length=10, choices=STATUS, default=DRAFT)
        published_at = models.DateTimeField(null=True, blank=True, db_index=True)
        rating = models.DecimalField(max_digits=4, decimal_places=2, default=Decimal("0"))
        word_count = models.PositiveIntegerField(default=0)

        class Meta:
            app_label = _APP
            ordering = ["-published_at", "id"]

    class Comment(models.Model):
        article = models.ForeignKey(Article, on_delete=models.CASCADE, related_name="comments")
        author_name = models.CharField(max_length=80)
        body = models.TextField()
        created = models.DateTimeField()

        class Meta:
            app_label = _APP
            ordering = ["created", "id"]

    with connection.schema_editor() as editor:
        from django.contrib.auth.models import Group, Permission
        from django.contrib.contenttypes.models import ContentType

        for model in (ContentType, Permission, Group, User, Token, Author, Tag, Article, Comment):
            editor.create_model(model)

    fixed_now = dt.datetime(2025, 9, 1, 12, 0, tzinfo=dt.timezone.utc)

    class AuthorSerializer(serializers.ModelSerializer):
        username = serializers.CharField(source="user.username", read_only=True)

        class Meta:
            model = Author
            fields = ["id", "name", "username", "bio", "joined"]

    class TagSerializer(serializers.ModelSerializer):
        n_articles = serializers.IntegerField(read_only=True)

        class Meta:
            model = Tag
            fields = ["name", "slug", "n_articles"]

    class CommentSerializer(serializers.ModelSerializer):
        class Meta:
            model = Comment
            fields = ["id", "author_name", "body", "created"]
            read_only_fields = ["created"]

        def validate_body(self, value):
            value = value.strip()
            if len(value) < 3:
                raise serializers.ValidationError("Comment is too short.")
            return value

    class ArticleListSerializer(serializers.ModelSerializer):
        url = serializers.HyperlinkedIdentityField(view_name="article-detail", lookup_field="slug")
        author = AuthorSerializer(read_only=True)
        tags = serializers.SlugRelatedField(many=True, slug_field="slug", read_only=True)
        n_comments = serializers.IntegerField(read_only=True)

        class Meta:
            model = Article
            fields = ["url", "id", "title", "slug", "author", "tags", "status", "published_at",
                      "rating", "word_count", "n_comments"]

    class ArticleDetailSerializer(ArticleListSerializer):
        comments = CommentSerializer(many=True, read_only=True)
        reading_minutes = serializers.SerializerMethodField()

        class Meta(ArticleListSerializer.Meta):
            fields = ArticleListSerializer.Meta.fields + ["body", "comments", "reading_minutes"]

        def get_reading_minutes(self, obj):
            return max(1, round(obj.word_count / 220))

    class ArticleWriteSerializer(serializers.ModelSerializer):
        tags = serializers.SlugRelatedField(many=True, slug_field="slug", queryset=Tag.objects.all(),
                                            required=False)

        class Meta:
            model = Article
            fields = ["title", "slug", "tags", "body", "status", "published_at", "rating"]

        def validate_title(self, value):
            value = " ".join(value.split())
            if len(value) < 5:
                raise serializers.ValidationError("Title must have at least 5 characters.")
            if value.isupper():
                raise serializers.ValidationError("Title must not be all capitals.")
            return value

        def validate(self, attrs):
            st = attrs.get("status", getattr(self.instance, "status", Article.DRAFT))
            pub = attrs.get("published_at", getattr(self.instance, "published_at", None))
            if st == Article.PUBLISHED and pub is None:
                raise serializers.ValidationError({"published_at": "Required when publishing."})
            if "body" in attrs:
                attrs["word_count"] = len(attrs["body"].split())
            return attrs

        def to_representation(self, instance):
            return ArticleDetailSerializer(instance, context=self.context).data

    class IsAuthorOrReadOnly(permissions.BasePermission):
        def has_object_permission(self, request, view, obj):
            if request.method in permissions.SAFE_METHODS:
                return True
            return obj.author.user_id == request.user.id

    class ArticleViewSet(viewsets.ModelViewSet):
        lookup_field = "slug"
        permission_classes = [permissions.IsAuthenticatedOrReadOnly, IsAuthorOrReadOnly]
        filterset_fields = ["status", "author", "tags__slug"]
        search_fields = ["title", "body"]
        ordering_fields = ["published_at", "rating", "title", "word_count"]

        def get_queryset(self):
            qs = (Article.objects.select_related("author__user").prefetch_related("tags")
                  .annotate(n_comments=Count("comments", distinct=True))
                  .order_by("-published_at", "id"))
            if self.action == "retrieve":
                qs = qs.prefetch_related("comments")
            return qs

        def get_serializer_class(self):
            if self.action in ("create", "update", "partial_update"):
                return ArticleWriteSerializer
            if self.action == "retrieve":
                return ArticleDetailSerializer
            return ArticleListSerializer

        def perform_create(self, serializer):
            serializer.save(author=self.request.user.author)

        @action(detail=True, methods=["post"], permission_classes=[permissions.IsAuthenticated])
        def comments(self, request, slug=None):
            article = self.get_object()
            ser = CommentSerializer(data=request.data)
            ser.is_valid(raise_exception=True)
            ser.save(article=article, created=fixed_now)
            return Response(ser.data, status=status.HTTP_201_CREATED)

    class TagViewSet(viewsets.ReadOnlyModelViewSet):
        serializer_class = TagSerializer
        lookup_field = "slug"
        pagination_class = None
        filter_backends = []

        def get_queryset(self):
            return Tag.objects.annotate(n_articles=Count("articles")).order_by("slug")

    class AuthorViewSet(viewsets.ReadOnlyModelViewSet):
        serializer_class = AuthorSerializer
        queryset = Author.objects.select_related("user")
        filter_backends = []

        @action(detail=True)
        def articles(self, request, pk=None):
            author = self.get_object()
            qs = (Article.objects.filter(author=author, status=Article.PUBLISHED)
                  .select_related("author__user").prefetch_related("tags")
                  .annotate(n_comments=Count("comments", distinct=True))
                  .order_by("-published_at", "id"))
            page = self.paginate_queryset(qs)
            ser = ArticleListSerializer(page, many=True, context=self.get_serializer_context())
            return self.get_paginated_response(ser.data)

    router = DefaultRouter()
    router.register("articles", ArticleViewSet, basename="article")
    router.register("tags", TagViewSet, basename="tag")
    router.register("authors", AuthorViewSet, basename="author")
    from django.urls import include, path

    sys.modules[_APP].urlpatterns = [path("api/", include(router.urls))]

    # ---- data -------------------------------------------------------------------------
    rng = random.Random(7)

    def sentence(n):
        return " ".join(rng.choice(WORDS) for _ in range(n)).capitalize() + "."

    users = []
    for i in range(10):
        u = User(username=f"writer{i}", email=f"writer{i}@example.com", first_name=f"W{i}")
        u.set_unusable_password()
        users.append(u)
    User.objects.bulk_create(users)
    users = list(User.objects.order_by("id"))
    tokens = {}
    for i, u in enumerate(users):
        key = f"{i:02d}" + "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8"[: 38]
        Token.objects.create(user=u, key=key)
        tokens[i] = key
    authors = Author.objects.bulk_create(
        [Author(user=u, name=f"{rng.choice(WORDS).title()} {rng.choice(WORDS).title()}",
                bio=sentence(rng.randint(5, 15)), joined=dt.date(2020, 1 + i, 3 + i))
         for i, u in enumerate(users)])
    tag_words = sorted(set(WORDS))[:16]
    tags = Tag.objects.bulk_create([Tag(name=w.title(), slug=w) for w in tag_words])
    base = dt.datetime(2024, 1, 1, 8, 0, tzinfo=dt.timezone.utc)
    arts = []
    for i in range(300):
        body = " ".join(sentence(rng.randint(8, 20)) for _ in range(rng.randint(4, 12)))
        st = rng.choice([Article.PUBLISHED] * 4 + [Article.DRAFT, Article.ARCHIVED])
        arts.append(Article(
            title=sentence(rng.randint(3, 8))[:-1], slug=f"article-{i:04d}",
            author=authors[i % len(authors)], body=body, status=st,
            published_at=None if st == Article.DRAFT else base + dt.timedelta(hours=29 * i),
            rating=Decimal(rng.randint(0, 500)) / 100, word_count=len(body.split())))
    arts = Article.objects.bulk_create(arts)
    Through = Article.tags.through
    Through.objects.bulk_create(
        [Through(article_id=a.id, tag_id=t.id) for a in arts for t in rng.sample(tags, rng.randint(0, 4))])
    comments = []
    for a in arts:
        for j in range(rng.randint(0, 10)):
            comments.append(Comment(article=a, author_name=rng.choice(WORDS).title(),
                                    body=sentence(rng.randint(5, 25)),
                                    created=base + dt.timedelta(hours=29 * a.id + j)))
    Comment.objects.bulk_create(comments)

    # ---- request plan -----------------------------------------------------------------
    plan = []
    for i in range(240):
        k = i % 12
        tok = tokens[i % 10]
        if k in (0, 6):
            q = [f"?page={1 + i % 8}", "?status=published&ordering=-rating",
                 f"?search={WORDS[i % len(WORDS)]}", f"?tags__slug={tag_words[i % 16]}",
                 f"?author={1 + i % 10}&ordering=title", "?ordering=-word_count&page=2"][i % 6]
            plan.append(("get", f"/api/articles/{q}", None, None))
        elif k in (1, 4, 9):
            plan.append(("get", f"/api/articles/article-{(i * 37) % 300:04d}/", None, None))
        elif k == 2:
            plan.append(("get", "/api/tags/", None, None))
            plan.append(("get", f"/api/authors/{1 + i % 10}/articles/", None, None))
        elif k == 3:
            slug = f"new-article-{i:04d}"
            plan.append(("post", "/api/articles/", tok, {
                "title": f"Notes on {WORDS[i % 30]} and {WORDS[(i * 7) % 30]} number {i}",
                "slug": slug, "tags": [tag_words[i % 16], tag_words[(i + 5) % 16]],
                "body": " ".join(WORDS[(i + j) % len(WORDS)] for j in range(120)),
                "status": "published", "published_at": "2025-08-01T10:00:00Z", "rating": "3.50"}))
            plan.append(("patch", f"/api/articles/{slug}/", tok, {"rating": "4.25", "tags": [tag_words[i % 16]]}))
            plan.append(("post", f"/api/articles/{slug}/comments/", tokens[(i + 1) % 10],
                         {"author_name": "Reader", "body": f"Comment {i} on the new article."}))
            plan.append(("get", f"/api/articles/{slug}/", None, None))
            plan.append(("delete", f"/api/articles/{slug}/", tok, None))
        elif k == 5:
            plan.append(("post", "/api/articles/", tok, {
                "title": "NO", "slug": f"bad slug {i}", "status": "published", "body": "x",
                "tags": ["no-such-tag"], "rating": "123.456"}))
        elif k == 7:
            a = (i * 13) % 300
            owner = tokens[a % 10]
            other = tokens[(a + 1) % 10]
            plan.append(("patch", f"/api/articles/article-{a:04d}/", other, {"rating": "1.00"}))
            plan.append(("patch", f"/api/articles/article-{a:04d}/", owner, {"title": f"Revised title {i}"}))
        elif k == 8:
            plan.append(("post", f"/api/articles/article-{(i * 3) % 300:04d}/comments/", None,
                         {"author_name": "Anon", "body": "No token."}))
            plan.append(("get", "/api/articles/no-such-article/", None, None))
        elif k == 10:
            plan.append(("post", f"/api/articles/article-{(i * 11) % 300:04d}/comments/", tok,
                         {"author_name": f"Reader {i}", "body": f"Thoughtful remark number {i}."}))
        else:
            plan.append(("get", f"/api/authors/{1 + i % 10}/", None, None))
            plan.append(("get", "/api/", None, None))

    from rest_framework.test import APIClient

    return {"client": APIClient(), "plan": plan}


def run(state):
    import hashlib

    from django.db import transaction

    client = state["client"]
    out = []
    with transaction.atomic():
        for method, url, token, data in state["plan"]:
            if token is None:
                client.credentials()
            else:
                client.credentials(HTTP_AUTHORIZATION="Token " + token)
            resp = getattr(client, method)(url, data, format="json") if data is not None \
                else getattr(client, method)(url)
            body = resp.content
            out.append((method, url, resp.status_code, len(body), hashlib.sha256(body).hexdigest()))
        transaction.set_rollback(True)
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
