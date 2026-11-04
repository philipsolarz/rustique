"""A GraphQL API on Strawberry: nested queries resolved through DataLoaders, plus mutations.

Stands for the GraphQL server side of Python web stacks: Strawberry (the type-hint based
GraphQL library recommended by FastAPI and Django docs) on graphql-core, the reference
GraphQL implementation that Graphene, Ariadne and Strawberry all execute on. A social
blog schema with an interface (``Node``), a union (``SearchResult``), enums, input types,
custom scalars (``datetime``), field arguments and async resolvers that batch through
per-request ``DataLoader``s (the standard answer to N+1 queries). Each operation is a
full GraphQL request: parse, validate against the schema, execute (nested resolvers,
fragments, variables, aliases, ``__typename``), and JSON-encode the result -- the work a
GraphQL endpoint does per request (no query-document cache, Strawberry's default).
330 operations per run on one asyncio loop: deep feed queries (posts -> author ->
comments -> author), profiles with paginated posts, union searches with inline
fragments, and mutations with validation errors. In-memory data: 200 users, 2,000 posts,
8,000 comments; mutations go to a per-run overlay so the run repeats exactly.
"""

REQUIRES = ["strawberry-graphql==0.339.1", "graphql-core==3.3.1", "cross-web==0.7.0",
            "python-dateutil==2.9.0.post0", "six==1.17.0", "packaging==26.3",
            "typing-extensions==4.16.0"]

WORDS = ("graph query schema resolver type field union enum mutation scalar loader batch "
         "river cloud stone ember meadow harbor signal lantern orbit prism quiet").split()

FEED = """
query Feed($tag: String, $first: Int!, $after: Int) {
  feed(tag: $tag, first: $first, after: $after) {
    ...PostCard
    comments(limit: 3) { id body created author { id name } }
  }
}
fragment PostCard on Post {
  id title publishedAt genre score
  author { id name handle followersCount }
  tags { name }
}
"""

PROFILE = """
query Profile($id: ID!) {
  user(id: $id) {
    __typename id name handle bio followersCount
    posts(first: 10) { id title publishedAt commentCount tags { name } }
    recent: comments(limit: 5) { body post { id title author { name } } }
  }
}
"""

POST = """
query Post($id: ID!) {
  post(id: $id) {
    id title body publishedAt genre score
    author { ...UserBits posts(first: 3) { id title } }
    comments(limit: 50) { id body created author { ...UserBits } }
  }
}
fragment UserBits on User { id name handle }
"""

SEARCH = """
query Search($text: String!) {
  search(text: $text, limit: 25) {
    __typename
    ... on User { id name handle }
    ... on Post { id title author { name } }
    ... on Tag { name postCount }
  }
}
"""

CREATE = """
mutation Create($input: PostInput!) {
  createPost(input: $input) {
    __typename
    ... on Post { id title tags { name } author { name } }
    ... on InputError { field message }
  }
}
"""

COMMENT = """
mutation Comment($postId: ID!, $userId: ID!, $body: String!) {
  addComment(postId: $postId, userId: $userId, body: $body) { id body author { name } post { id commentCount } }
}
"""


def setup():
    import asyncio
    import dataclasses
    import datetime as dt
    import enum
    import random
    import typing

    import sys
    import types

    import strawberry
    from strawberry.dataloader import DataLoader

    # Strawberry resolves annotations in the defining module's namespace, which must be in
    # sys.modules (run.py loads workloads without registering them).
    namespace = sys.modules.setdefault(__name__, types.ModuleType(__name__)).__dict__

    @dataclasses.dataclass
    class UserRow:
        id: int
        name: str
        handle: str
        bio: str
        followers: int

    @dataclasses.dataclass
    class PostRow:
        id: int
        author_id: int
        title: str
        body: str
        published: dt.datetime
        genre: str
        score: float
        tags: list

    @dataclasses.dataclass
    class CommentRow:
        id: int
        post_id: int
        author_id: int
        body: str
        created: dt.datetime

    rng = random.Random(21)
    base = dt.datetime(2024, 1, 1, 8, 0, tzinfo=dt.timezone.utc)
    users = {i: UserRow(i, f"{rng.choice(WORDS).title()} {rng.choice(WORDS).title()}", f"user{i}",
                        " ".join(rng.choice(WORDS) for _ in range(rng.randint(3, 12))), rng.randint(0, 5000))
             for i in range(1, 201)}
    tags_all = sorted(set(WORDS))
    posts = {}
    for i in range(1, 2001):
        posts[i] = PostRow(i, rng.randint(1, 200), " ".join(rng.choice(WORDS).title() for _ in range(rng.randint(2, 6))),
                           " ".join(rng.choice(WORDS) for _ in range(rng.randint(20, 80))),
                           base + dt.timedelta(minutes=97 * i), rng.choice(["NEWS", "HOWTO", "OPINION", "REVIEW"]),
                           round(rng.random() * 100, 2), rng.sample(tags_all, rng.randint(0, 4)))
    comments = {}
    for i in range(1, 8001):
        p = rng.randint(1, 2000)
        comments[i] = CommentRow(i, p, rng.randint(1, 200), " ".join(rng.choice(WORDS) for _ in range(rng.randint(3, 25))),
                                 posts[p].published + dt.timedelta(minutes=rng.randint(1, 5000)))
    by_author = {}
    for p in sorted(posts.values(), key=lambda p: p.published, reverse=True):
        by_author.setdefault(p.author_id, []).append(p.id)
    comments_by_post, comments_by_user = {}, {}
    for c in sorted(comments.values(), key=lambda c: (c.created, c.id)):
        comments_by_post.setdefault(c.post_id, []).append(c.id)
        comments_by_user.setdefault(c.author_id, []).append(c.id)
    tag_counts = {}
    for p in posts.values():
        for t in p.tags:
            tag_counts[t] = tag_counts.get(t, 0) + 1
    feed_order = sorted(posts, key=lambda i: (posts[i].published, i), reverse=True)

    class Store:
        """The data layer: base tables plus a per-run overlay for writes."""

        def reset(self):
            self.new_posts, self.new_comments = {}, {}
            self.extra_by_post, self.next_post, self.next_comment = {}, 2001, 8001

        def post(self, i):
            return self.new_posts.get(i) or posts.get(i)

        def comment_ids(self, post_id):
            return comments_by_post.get(post_id, []) + self.extra_by_post.get(post_id, [])

        def comment(self, i):
            return self.new_comments.get(i) or comments.get(i)

    store = Store()
    store.reset()

    @strawberry.enum
    class Genre(enum.Enum):
        NEWS = "NEWS"
        HOWTO = "HOWTO"
        OPINION = "OPINION"
        REVIEW = "REVIEW"

    @strawberry.interface
    class Node:
        id: strawberry.ID

    @strawberry.type
    class Tag:
        name: str

        @strawberry.field
        def post_count(self) -> int:
            return tag_counts.get(self.name, 0)

    @strawberry.type
    class User(Node):
        name: str
        handle: str
        bio: str
        followers_count: int
        _row: strawberry.Private[UserRow]

        @strawberry.field
        def posts(self, first: int = 10) -> list["Post"]:
            return [Post.from_row(posts[i]) for i in by_author.get(self._row.id, [])[:first]]

        @strawberry.field
        def comments(self, limit: int = 10) -> list["Comment"]:
            ids = comments_by_user.get(self._row.id, [])[-limit:]
            return [Comment.from_row(comments[i]) for i in reversed(ids)]

        @staticmethod
        def from_row(r):
            return User(id=strawberry.ID(str(r.id)), name=r.name, handle=r.handle, bio=r.bio,
                        followers_count=r.followers, _row=r)

    @strawberry.type
    class Comment(Node):
        body: str
        created: dt.datetime
        _row: strawberry.Private[CommentRow]

        @strawberry.field
        async def author(self, info: strawberry.Info) -> User:
            return await info.context["users"].load(self._row.author_id)

        @strawberry.field
        def post(self) -> "Post":
            return Post.from_row(store.post(self._row.post_id))

        @staticmethod
        def from_row(r):
            return Comment(id=strawberry.ID(str(r.id)), body=r.body, created=r.created, _row=r)

    @strawberry.type
    class Post(Node):
        title: str
        body: str
        published_at: dt.datetime
        genre: Genre
        score: float
        _row: strawberry.Private[PostRow]

        @strawberry.field
        async def author(self, info: strawberry.Info) -> User:
            return await info.context["users"].load(self._row.author_id)

        @strawberry.field
        def tags(self) -> list[Tag]:
            return [Tag(name=t) for t in self._row.tags]

        @strawberry.field
        async def comments(self, info: strawberry.Info, limit: int = 20) -> list[Comment]:
            rows = await info.context["comments"].load(self._row.id)
            return [Comment.from_row(c) for c in rows[:limit]]

        @strawberry.field
        def comment_count(self) -> int:
            return len(store.comment_ids(self._row.id))

        @staticmethod
        def from_row(r):
            return Post(id=strawberry.ID(str(r.id)), title=r.title, body=r.body, published_at=r.published,
                        genre=Genre(r.genre), score=r.score, _row=r)

    @strawberry.type
    class InputError:
        field: str
        message: str

    SearchResult = typing.Annotated[User | Post | Tag, strawberry.union("SearchResult")]
    CreateResult = typing.Annotated[Post | InputError, strawberry.union("CreateResult")]

    @strawberry.input
    class PostInput:
        author_id: strawberry.ID
        title: str
        body: str
        genre: Genre = Genre.NEWS
        tags: list[str] = strawberry.field(default_factory=list)

    @strawberry.type
    class Query:
        @strawberry.field
        def user(self, id: strawberry.ID) -> User | None:
            r = users.get(int(id))
            return User.from_row(r) if r else None

        @strawberry.field
        def post(self, id: strawberry.ID) -> Post | None:
            r = store.post(int(id))
            return Post.from_row(r) if r else None

        @strawberry.field
        def feed(self, first: int = 20, after: int | None = None, tag: str | None = None) -> list[Post]:
            out = []
            for i in feed_order[after or 0:]:
                p = posts[i]
                if tag is None or tag in p.tags:
                    out.append(Post.from_row(p))
                    if len(out) == first:
                        break
            return out

        @strawberry.field
        def search(self, text: str, limit: int = 20) -> list[SearchResult]:
            t = text.lower()
            res = [User.from_row(u) for u in users.values() if t in u.name.lower()][:8]
            res += [Tag(name=n) for n in tags_all if t in n][:4]
            res += [Post.from_row(p) for p in posts.values() if t in p.title.lower()][: limit - len(res)]
            return res

    @strawberry.type
    class Mutation:
        @strawberry.mutation
        def create_post(self, input: PostInput) -> CreateResult:
            if len(input.title.strip()) < 4:
                return InputError(field="title", message="Title is too short.")
            if int(input.author_id) not in users:
                return InputError(field="authorId", message="Unknown author.")
            if any(t not in tags_all for t in input.tags):
                return InputError(field="tags", message="Unknown tag.")
            pid = store.next_post
            store.next_post += 1
            row = PostRow(pid, int(input.author_id), input.title.strip(), input.body,
                          base + dt.timedelta(days=900), input.genre.value, 0.0, list(input.tags))
            store.new_posts[pid] = row
            return Post.from_row(row)

        @strawberry.mutation
        def add_comment(self, post_id: strawberry.ID, user_id: strawberry.ID, body: str) -> Comment:
            cid = store.next_comment
            store.next_comment += 1
            row = CommentRow(cid, int(post_id), int(user_id), body, base + dt.timedelta(days=901))
            store.new_comments[cid] = row
            store.extra_by_post.setdefault(int(post_id), []).append(cid)
            return Comment.from_row(row)

    namespace.update(User=User, Post=Post, Comment=Comment, Tag=Tag)  # forward references
    schema = strawberry.Schema(query=Query, mutation=Mutation)

    async def load_users(keys):
        return [User.from_row(users[k]) for k in keys]

    async def load_comments(keys):
        return [[store.comment(c) for c in store.comment_ids(k)] for k in keys]

    def context():
        return {"users": DataLoader(load_fn=load_users), "comments": DataLoader(load_fn=load_comments)}

    ops = []
    for i in range(300):
        k = i % 10
        if k in (0, 5):
            ops.append((FEED, {"first": 20, "after": (i * 13) % 300, "tag": None if i % 3 else tags_all[i % len(tags_all)]}))
        elif k in (1, 6):
            ops.append((PROFILE, {"id": str(1 + (i * 7) % 200)}))
        elif k in (2, 7):
            ops.append((POST, {"id": str(1 + (i * 37) % 2000)}))
        elif k == 3:
            ops.append((SEARCH, {"text": WORDS[i % len(WORDS)][:4]}))
        elif k == 4:
            ops.append((CREATE, {"input": {"authorId": str(1 + i % 200), "title": f"Fresh post {i}",
                                           "body": "Body text " * 20, "genre": "HOWTO",
                                           "tags": [tags_all[i % len(tags_all)]]}}))
            ops.append((CREATE, {"input": {"authorId": "999", "title": "no", "body": "", "tags": ["zzz"]}}))
        elif k == 8:
            ops.append((COMMENT, {"postId": str(1 + (i * 3) % 2000), "userId": str(1 + i % 200),
                                  "body": f"Comment number {i}"}))
        else:
            ops.append((FEED, {"first": 5, "after": i}))
    loop = asyncio.new_event_loop()
    return {"schema": schema, "ops": ops, "store": store, "context": context, "loop": loop}


def run(state):
    import hashlib
    import json

    schema, store, context = state["schema"], state["store"], state["context"]
    store.reset()

    async def main():
        out = []
        for query, variables in state["ops"]:
            res = await schema.execute(query, variable_values=variables, context_value=context())
            payload = {"data": res.data}
            if res.errors:
                payload["errors"] = [e.formatted for e in res.errors]
            body = json.dumps(payload, default=str).encode()
            out.append((len(body), hashlib.sha256(body).hexdigest()))
        return out

    return state["loop"].run_until_complete(main())


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
