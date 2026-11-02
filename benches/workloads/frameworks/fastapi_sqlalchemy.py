"""A FastAPI service on SQLAlchemy's async ORM: the full modern async stack with a database.

Stands for the typical production FastAPI service (the shape of FastAPI's "bigger
applications" layout and of most FastAPI templates): ``APIRouter``s, a per-request
``AsyncSession`` dependency (``async def`` generator), an auth dependency that looks the
user up by API key, a class-based pagination dependency, pydantic v2 request and response
models built ``from_attributes`` (nested relations, ``Literal``/enum fields, constrained
fields, a ``model_validator``), SQLAlchemy 2.1 async ORM queries (``select()`` with
``joinedload``/``selectinload``, filters, ``ilike`` search, ``GROUP BY`` aggregates, unit
of work inserts/updates/deletes) through ``greenlet`` and ``aiosqlite`` (sqlite runs in
aiosqlite's worker thread: a thread hand-off per statement, as with any async driver),
``CORSMiddleware`` plus a function middleware (``BaseHTTPMiddleware``), a custom
exception handler and 404/409/422 paths. A task tracker: users, projects, tasks, labels.
Driven by ``httpx.AsyncClient(transport=ASGITransport(app))`` on one persistent asyncio
loop, 450 requests per run; the database is restored from a snapshot (sqlite backup API)
at the start of each run so the run repeats exactly. Data: 30 users, 12 projects,
1,500 tasks, 10 labels.
"""

REQUIRES = ["fastapi==0.162.0", "starlette==1.10.0", "pydantic==2.14.2", "pydantic-core==2.50.2",
            "SQLAlchemy==2.1.6", "greenlet==3.5.6", "aiosqlite==0.22.1", "httpx==0.28.1",
            "httpcore==1.0.9", "anyio==4.15.1", "h11==0.16.0", "idna==3.20", "certifi==2026.10.23",
            "sniffio==1.3.1", "typing-extensions==4.16.0", "annotated-types==0.8.0",
            "typing-inspection==0.4.4", "annotated-doc==0.0.5", "opentelemetry-api==1.45.2"]

WORDS = ("deploy fix refactor review design migrate test document benchmark audit cache "
         "index login billing search export import upload report dashboard api queue").split()


def setup():
    import asyncio
    import datetime as dt
    import enum
    import random
    import sqlite3
    from typing import Annotated, Literal

    from fastapi import APIRouter, Depends, FastAPI, Header, HTTPException, Query, Request
    from fastapi.middleware.cors import CORSMiddleware
    from fastapi.responses import JSONResponse
    from pydantic import BaseModel, ConfigDict, Field, model_validator
    from sqlalchemy import Column, ForeignKey, String, Table, func, select
    from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
    from sqlalchemy.orm import DeclarativeBase, Mapped, joinedload, mapped_column, relationship, selectinload
    from sqlalchemy.pool import StaticPool

    class Base(DeclarativeBase):
        pass

    task_labels = Table("task_labels", Base.metadata,
                        Column("task_id", ForeignKey("tasks.id", ondelete="CASCADE"), primary_key=True),
                        Column("label_id", ForeignKey("labels.id"), primary_key=True))

    class Status(str, enum.Enum):
        todo = "todo"
        doing = "doing"
        review = "review"
        done = "done"

    class User(Base):
        __tablename__ = "users"
        id: Mapped[int] = mapped_column(primary_key=True)
        name: Mapped[str] = mapped_column(String(80))
        email: Mapped[str] = mapped_column(String(120), unique=True)
        api_key: Mapped[str] = mapped_column(String(40), unique=True)

    class Project(Base):
        __tablename__ = "projects"
        id: Mapped[int] = mapped_column(primary_key=True)
        key: Mapped[str] = mapped_column(String(8), unique=True)
        name: Mapped[str] = mapped_column(String(80))
        owner_id: Mapped[int] = mapped_column(ForeignKey("users.id"))
        owner: Mapped[User] = relationship()

    class Label(Base):
        __tablename__ = "labels"
        id: Mapped[int] = mapped_column(primary_key=True)
        name: Mapped[str] = mapped_column(String(30), unique=True)
        color: Mapped[str] = mapped_column(String(7))

    class Task(Base):
        __tablename__ = "tasks"
        id: Mapped[int] = mapped_column(primary_key=True)
        project_id: Mapped[int] = mapped_column(ForeignKey("projects.id"), index=True)
        title: Mapped[str] = mapped_column(String(200))
        description: Mapped[str] = mapped_column(String(2000), default="")
        status: Mapped[Status] = mapped_column(default=Status.todo, index=True)
        priority: Mapped[int] = mapped_column(default=3)
        estimate: Mapped[float | None]
        due: Mapped[dt.date | None]
        created: Mapped[dt.datetime]
        assignee_id: Mapped[int | None] = mapped_column(ForeignKey("users.id"), index=True)
        assignee: Mapped[User | None] = relationship()
        project: Mapped[Project] = relationship()
        labels: Mapped[list[Label]] = relationship(secondary=task_labels, order_by=Label.name)

    # ---- schemas ------------------------------------------------------------------------
    class UserOut(BaseModel):
        model_config = ConfigDict(from_attributes=True)
        id: int
        name: str
        email: str

    class LabelOut(BaseModel):
        model_config = ConfigDict(from_attributes=True)
        name: str
        color: str

    class ProjectOut(BaseModel):
        model_config = ConfigDict(from_attributes=True)
        id: int
        key: str
        name: str
        owner: UserOut

    class TaskOut(BaseModel):
        model_config = ConfigDict(from_attributes=True)
        id: int
        project_id: int
        title: str
        status: Status
        priority: int
        estimate: float | None
        due: dt.date | None
        created: dt.datetime
        assignee: UserOut | None
        labels: list[LabelOut]

    class TaskDetail(TaskOut):
        description: str
        project: ProjectOut

    class TaskIn(BaseModel):
        title: str = Field(min_length=3, max_length=200)
        description: str = Field(default="", max_length=2000)
        status: Status = Status.todo
        priority: int = Field(default=3, ge=1, le=5)
        estimate: float | None = Field(default=None, ge=0, le=100)
        due: dt.date | None = None
        assignee_id: int | None = None
        labels: list[str] = Field(default_factory=list, max_length=5)

        @model_validator(mode="after")
        def check_done(self):
            if self.status == Status.done and self.estimate is None:
                raise ValueError("a done task needs an estimate")
            return self

    class TaskPatch(BaseModel):
        title: str | None = Field(default=None, min_length=3, max_length=200)
        status: Status | None = None
        priority: int | None = Field(default=None, ge=1, le=5)
        assignee_id: int | None = None

    class PageOut(BaseModel):
        total: int
        offset: int
        limit: int
        items: list[TaskOut]

    class ProjectSummary(BaseModel):
        project: ProjectOut
        by_status: dict[str, int]
        open_points: float
        overdue: int

    class BulkStatus(BaseModel):
        ids: list[int] = Field(min_length=1, max_length=50)
        status: Literal["todo", "doing", "review", "done"]

    # ---- app ----------------------------------------------------------------------------
    engine = create_async_engine("sqlite+aiosqlite://", poolclass=StaticPool,
                                 connect_args={"check_same_thread": False})
    Session = async_sessionmaker(engine, expire_on_commit=False)
    today = dt.date(2025, 6, 1)
    fixed_now = dt.datetime(2025, 6, 1, 12, 0)

    async def get_session():
        async with Session() as session:
            yield session

    DB = Annotated[AsyncSession, Depends(get_session)]

    async def current_user(db: DB, x_api_key: Annotated[str, Header()]):
        user = await db.scalar(select(User).where(User.api_key == x_api_key))
        if user is None:
            raise HTTPException(401, "invalid API key")
        return user

    Me = Annotated[User, Depends(current_user)]

    class Paging:
        def __init__(self, offset: Annotated[int, Query(ge=0)] = 0,
                     limit: Annotated[int, Query(ge=1, le=100)] = 25):
            self.offset, self.limit = offset, limit

    class Conflict(Exception):
        def __init__(self, msg):
            self.msg = msg

    def task_query():
        return select(Task).options(joinedload(Task.assignee), selectinload(Task.labels))

    async def load_labels(db, names):
        if not names:
            return []
        labels = (await db.scalars(select(Label).where(Label.name.in_(names)))).all()
        if len(labels) != len(set(names)):
            raise HTTPException(422, "unknown label")
        return sorted(labels, key=lambda lb: lb.name)

    router = APIRouter(prefix="/api")

    @router.get("/projects/{key}/tasks", response_model=PageOut)
    async def list_tasks(key: str, db: DB, _: Me, paging: Annotated[Paging, Depends()],
                         status: Status | None = None, assignee: int | None = None,
                         label: str | None = None, q: str | None = None,
                         sort: Literal["priority", "-priority", "due", "created", "-created"] = "-created"):
        project = await db.scalar(select(Project).where(Project.key == key))
        if project is None:
            raise HTTPException(404, "no such project")
        cond = [Task.project_id == project.id]
        if status is not None:
            cond.append(Task.status == status)
        if assignee is not None:
            cond.append(Task.assignee_id == assignee)
        if label is not None:
            cond.append(Task.labels.any(Label.name == label))
        if q:
            cond.append(Task.title.ilike(f"%{q}%"))
        total = await db.scalar(select(func.count(Task.id)).where(*cond))
        col = {"priority": Task.priority, "due": Task.due, "created": Task.created}[sort.lstrip("-")]
        rows = (await db.scalars(task_query().where(*cond)
                                 .order_by(col.desc() if sort.startswith("-") else col, Task.id)
                                 .offset(paging.offset).limit(paging.limit))).unique().all()
        return PageOut(total=total, offset=paging.offset, limit=paging.limit,
                       items=[TaskOut.model_validate(t) for t in rows])

    @router.get("/tasks/{task_id}", response_model=TaskDetail)
    async def get_task(task_id: int, db: DB, _: Me):
        task = await db.scalar(task_query().options(joinedload(Task.project).joinedload(Project.owner))
                               .where(Task.id == task_id))
        if task is None:
            raise HTTPException(404, "no such task")
        return task

    @router.post("/projects/{key}/tasks", response_model=TaskDetail, status_code=201)
    async def create_task(key: str, body: TaskIn, db: DB, me: Me):
        project = await db.scalar(select(Project).options(joinedload(Project.owner)).where(Project.key == key))
        if project is None:
            raise HTTPException(404, "no such project")
        exists = await db.scalar(select(func.count(Task.id)).where(Task.project_id == project.id,
                                                                    Task.title == body.title))
        if exists:
            raise Conflict(f"task {body.title!r} already exists")
        task = Task(project=project, title=body.title, description=body.description, status=body.status,
                    priority=body.priority, estimate=body.estimate, due=body.due, created=fixed_now,
                    assignee_id=body.assignee_id or me.id, labels=await load_labels(db, body.labels))
        db.add(task)
        await db.commit()
        return await db.scalar(task_query().options(joinedload(Task.project).joinedload(Project.owner))
                               .where(Task.id == task.id).execution_options(populate_existing=True))

    @router.patch("/tasks/{task_id}", response_model=TaskOut)
    async def patch_task(task_id: int, body: TaskPatch, db: DB, _: Me):
        task = await db.scalar(task_query().where(Task.id == task_id))
        if task is None:
            raise HTTPException(404, "no such task")
        for k, v in body.model_dump(exclude_unset=True).items():
            setattr(task, k, v)
        await db.commit()
        return await db.scalar(task_query().where(Task.id == task_id).execution_options(populate_existing=True))

    @router.delete("/tasks/{task_id}", status_code=204)
    async def delete_task(task_id: int, db: DB, _: Me):
        task = await db.get(Task, task_id)
        if task is None:
            raise HTTPException(404, "no such task")
        await db.delete(task)
        await db.commit()

    @router.post("/tasks/bulk-status")
    async def bulk_status(body: BulkStatus, db: DB, _: Me):
        tasks = (await db.scalars(select(Task).where(Task.id.in_(body.ids)).order_by(Task.id))).all()
        for t in tasks:
            t.status = Status(body.status)
        await db.commit()
        return {"updated": [t.id for t in tasks], "status": body.status}

    @router.get("/projects/{key}/summary", response_model=ProjectSummary)
    async def summary(key: str, db: DB, _: Me):
        project = await db.scalar(select(Project).options(joinedload(Project.owner)).where(Project.key == key))
        if project is None:
            raise HTTPException(404, "no such project")
        rows = (await db.execute(select(Task.status, func.count(Task.id))
                                 .where(Task.project_id == project.id).group_by(Task.status))).all()
        points = await db.scalar(select(func.coalesce(func.sum(Task.estimate), 0.0))
                                 .where(Task.project_id == project.id, Task.status != Status.done))
        overdue = await db.scalar(select(func.count(Task.id)).where(
            Task.project_id == project.id, Task.status != Status.done, Task.due < today))
        return ProjectSummary(project=ProjectOut.model_validate(project),
                              by_status={s.value: n for s, n in sorted(rows, key=lambda r: r[0].value)},
                              open_points=round(points, 2), overdue=overdue)

    app = FastAPI(title="tracker")
    app.add_middleware(CORSMiddleware, allow_origins=["https://app.example.com"], allow_methods=["*"],
                       allow_headers=["*"])

    @app.middleware("http")
    async def server_timing(request: Request, call_next):
        response = await call_next(request)
        response.headers["X-Service"] = "tracker"
        return response

    @app.exception_handler(Conflict)
    async def on_conflict(request: Request, exc: Conflict):
        return JSONResponse({"detail": exc.msg}, status_code=409)

    app.include_router(router)

    # ---- data ---------------------------------------------------------------------------
    rng = random.Random(9)
    loop = asyncio.new_event_loop()

    async def seed():
        async with engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all)
        async with Session() as db:
            users = [User(name=f"{rng.choice(WORDS).title()} {i}", email=f"u{i}@example.com",
                          api_key=f"key-{i:03d}-{(i * 7919) % 99991:05d}") for i in range(30)]
            labels = [Label(name=n, color=f"#{rng.randrange(1 << 24):06x}") for n in
                      ("bug", "feature", "chore", "docs", "perf", "security", "ui", "backend", "infra", "urgent")]
            projects = [Project(key=f"P{i:02d}", name=f"Project {WORDS[i]}", owner=users[i]) for i in range(12)]
            db.add_all(users + labels + projects)
            base = dt.datetime(2025, 1, 1, 9, 0)
            for i in range(1500):
                st = rng.choice(list(Status))
                db.add(Task(project=projects[i % 12], title=f"{rng.choice(WORDS).title()} {rng.choice(WORDS)} #{i}",
                            description=" ".join(rng.choice(WORDS) for _ in range(rng.randint(5, 40))),
                            status=st, priority=rng.randint(1, 5),
                            estimate=None if rng.random() < 0.2 and st != Status.done else rng.randint(1, 13) / 2,
                            due=None if rng.random() < 0.3 else today + dt.timedelta(days=rng.randint(-60, 90)),
                            created=base + dt.timedelta(hours=2 * i),
                            assignee=None if rng.random() < 0.15 else users[rng.randrange(30)],
                            labels=rng.sample(labels, rng.randint(0, 3))))
            await db.commit()
        async with engine.connect() as conn:
            raw = await conn.get_raw_connection()
            snap = sqlite3.connect(":memory:", check_same_thread=False)
            await raw.driver_connection.backup(snap)
            return snap, raw.driver_connection

    snapshot, driver_conn = loop.run_until_complete(seed())
    keys = [f"key-{i:03d}-{(i * 7919) % 99991:05d}" for i in range(30)]

    # ---- plan ---------------------------------------------------------------------------
    plan = []
    for i in range(300):
        k = i % 10
        key = keys[i % 30]
        proj = f"P{i % 12:02d}"
        if k in (0, 5):
            q = ["", "?status=doing", f"?assignee={1 + i % 30}&sort=priority", "?label=bug&sort=due",
                 f"?q={WORDS[i % 21]}&limit=10", "?offset=50&limit=50&sort=-priority"][i % 6]
            plan.append(("GET", f"/api/projects/{proj}/tasks{q}", key, None))
        elif k in (1, 6, 8):
            plan.append(("GET", f"/api/tasks/{1 + (i * 37) % 1500}", key, None))
        elif k == 2:
            title = f"New work item {i} {WORDS[i % 21]}"
            plan.append(("POST", f"/api/projects/{proj}/tasks", key, {
                "title": title, "description": "Created by the benchmark.", "priority": 1 + i % 5,
                "estimate": 2.5, "due": "2025-07-01", "labels": ["feature", "backend"][: 1 + i % 2]}))
            plan.append(("POST", f"/api/projects/{proj}/tasks", key, {"title": title}))
            plan.append(("PATCH", "/api/tasks/1501", key, {"status": "review", "priority": 2}))
            plan.append(("DELETE", "/api/tasks/1501", key, None))  # sqlite reuses the rowid
        elif k == 3:
            plan.append(("POST", f"/api/projects/{proj}/tasks", key, {
                "title": "x", "priority": 9, "status": "done", "labels": ["a"] * 6}))
            plan.append(("GET", f"/api/tasks/{1 + i % 1500}", "bad-key", None))
        elif k == 4:
            plan.append(("GET", f"/api/projects/{proj}/summary", key, None))
        elif k == 7:
            ids = [1 + (i * 11 + j) % 1500 for j in range(10)]
            plan.append(("POST", "/api/tasks/bulk-status", key, {"ids": ids, "status": "doing"}))
        else:
            plan.append(("PATCH", f"/api/tasks/{1 + (i * 13) % 1500}", key, {"assignee_id": 1 + i % 30}))
            plan.append(("GET", "/api/projects/NOPE/tasks", key, None))
    return {"app": app, "loop": loop, "snapshot": snapshot, "driver_conn": driver_conn, "plan": plan}


def run(state):
    import hashlib

    import httpx

    app, loop = state["app"], state["loop"]

    async def main():
        # restore the database in aiosqlite's own worker thread
        await state["driver_conn"]._execute(state["snapshot"].backup, state["driver_conn"]._conn)
        out = []
        transport = httpx.ASGITransport(app=app)
        async with httpx.AsyncClient(transport=transport, base_url="http://api.test") as client:
            for method, url, key, body in state["plan"]:
                r = await client.request(method, url, json=body, headers={"x-api-key": key})
                out.append((method, url, r.status_code, hashlib.sha256(r.content).hexdigest()))
        return out

    return loop.run_until_complete(main())


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
