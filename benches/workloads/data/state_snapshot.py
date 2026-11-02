"""Object-graph persistence and copying: checkpoint, cache, diff and export a project-
tracker domain model built from dataclasses and enums.

Stands for applications and batch jobs that snapshot in-memory state: ``copy.deepcopy``
for undo/what-if copies, ``pickle.dumps``/``loads`` for caches, checkpoints and
multiprocessing hand-off, ``dataclasses.asdict`` + ``json.dumps(default=...)`` for API
or file export, ``dataclasses.replace`` for immutable-style updates, and dataclass
``__eq__`` to diff two snapshots. Data shape: one workspace of 160 projects, 9,600 tasks,
~28,000 comments, 150 users, with datetimes, enums, sets, nested lists and dicts
(~100k objects).
"""

import enum
import sys
import types
from dataclasses import dataclass, field
from datetime import date, datetime

REQUIRES: list[str] = []


class Status(enum.Enum):
    TODO = "todo"
    DOING = "doing"
    REVIEW = "review"
    DONE = "done"


class Priority(enum.IntEnum):
    LOW = 1
    MEDIUM = 2
    HIGH = 3
    URGENT = 4


@dataclass
class User:
    id: int
    name: str
    email: str
    roles: list[str] = field(default_factory=list)


@dataclass
class Comment:
    author_id: int
    created: datetime
    body: str
    reactions: dict[str, int] = field(default_factory=dict)


@dataclass
class Task:
    id: int
    title: str
    status: Status
    priority: Priority
    assignee_id: int | None
    due: date | None
    estimate_h: float
    labels: set[str] = field(default_factory=set)
    comments: list[Comment] = field(default_factory=list)
    subtasks: list[str] = field(default_factory=list)


@dataclass
class Project:
    key: str
    name: str
    owner_id: int
    tasks: list[Task] = field(default_factory=list)
    settings: dict[str, object] = field(default_factory=dict)


@dataclass
class Workspace:
    name: str
    users: list[User]
    projects: list[Project]
    version: int = 1


# pickle finds classes through sys.modules[cls.__module__]; register them under a fixed
# module name so this works however the runner loads this file.
_MODEL = types.ModuleType("rq_workloads_data_state_snapshot_model")
for _cls in (Status, Priority, User, Comment, Task, Project, Workspace):
    _cls.__module__ = _MODEL.__name__
    setattr(_MODEL, _cls.__name__, _cls)
sys.modules[_MODEL.__name__] = _MODEL


def setup():
    import random
    from datetime import timedelta

    rng = random.Random(8)
    users = [User(i, f"User {i}", f"user{i}@example.com",
                  rng.sample(["admin", "dev", "pm", "qa", "viewer"], rng.randint(1, 3)))
             for i in range(150)]
    labels = ["bug", "feature", "ux", "backend", "frontend", "infra", "docs", "security",
              "perf", "tech-debt"]
    base = datetime(2026, 1, 1, 9, 0)
    projects = []
    tid = 0
    for p in range(160):
        tasks = []
        for _ in range(60):
            comments = [Comment(rng.randrange(150), base + timedelta(minutes=rng.randint(0, 400_000)),
                                " ".join(rng.choices(["looks", "good", "needs", "work", "fixed",
                                                      "in", "next", "build", "please",
                                                      "review", "LGTM"], k=rng.randint(3, 25))),
                                {e: rng.randint(1, 5) for e in rng.sample(["+1", "heart", "eyes",
                                                                         "rocket"],
                                                                        rng.randint(0, 2))})
                        for _ in range(rng.choice([0, 1, 2, 3, 3, 4, 5, 8]))]
            tasks.append(Task(
                tid, f"Task {tid}: {rng.choice(['fix', 'add', 'refactor', 'remove'])} "
                     f"{rng.choice(['login', 'search', 'cache', 'export', 'billing'])}",
                rng.choice(list(Status)), rng.choice(list(Priority)),
                rng.randrange(150) if rng.random() < 0.85 else None,
                date(2026, rng.randint(1, 12), rng.randint(1, 28)) if rng.random() < 0.6 else None,
                round(rng.uniform(0.5, 40), 1), set(rng.sample(labels, rng.randint(0, 3))),
                comments, [f"step {k}" for k in range(rng.randint(0, 6))]))
            tid += 1
        projects.append(Project(f"P{p:02d}", f"Project {p}", rng.randrange(150), tasks,
                                {"public": rng.random() < 0.3, "wip_limit": rng.randint(3, 9),
                                 "columns": [s.value for s in Status]}))
    return {"ws": Workspace("acme", users, projects)}


def run(state):
    import copy
    import dataclasses
    import json
    import pickle

    ws = state["ws"]

    # 1. undo snapshot + what-if edit on the copy
    draft = copy.deepcopy(ws)
    moved = 0
    for proj in draft.projects:
        for i, task in enumerate(proj.tasks):
            if task.status is Status.REVIEW and task.priority >= Priority.HIGH:
                proj.tasks[i] = dataclasses.replace(task, status=Status.DONE)
                moved += 1
    changed = sum(a != b for pa, pb in zip(ws.projects, draft.projects)
                  for a, b in zip(pa.tasks, pb.tasks))

    # 2. cache / checkpoint round trip
    blob = pickle.dumps(ws, protocol=pickle.HIGHEST_PROTOCOL)
    restored = pickle.loads(blob)
    same = restored == ws
    blob2 = pickle.dumps(draft, protocol=pickle.HIGHEST_PROTOCOL)

    # 3. API export of a few projects
    def default(o):
        if isinstance(o, (datetime, date)):
            return o.isoformat()
        if isinstance(o, set):
            return sorted(o)
        if isinstance(o, Status):
            return o.value
        raise TypeError(type(o))

    exported = [json.dumps(dataclasses.asdict(p), default=default, sort_keys=True)
                for p in draft.projects[:20]]

    # 4. shallow per-project copies for a board view
    board = {p.key: {s: [t.id for t in copy.copy(p.tasks) if t.status is s] for s in Status}
             for p in ws.projects}
    return {
        "moved": moved,
        "changed": changed,
        "roundtrip_equal": same,
        "pickle_len": len(blob),
        "pickle2_len": len(blob2),
        "export_len": sum(map(len, exported)),
        "export_head": exported[0][:3000],
        "board": {k: {s.value: len(v) for s, v in cols.items()} for k, cols in board.items()},
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
