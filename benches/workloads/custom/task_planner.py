"""Build/CI planner for a monorepo: dependency DAG analysis, impact sets and scheduling.

Stands for the planning code in build systems, CI orchestrators, data-pipeline schedulers
(Airflow/Dagster/Luigi-style DAGs), project-management and release tooling: tasks as
``collections.namedtuple``/``typing.NamedTuple`` records, adjacency in ``defaultdict(set)``,
Kahn topological sort on ``deque`` with ``Counter`` in-degrees, cycle reporting by
iterative DFS, ancestor/descendant closures and a transitive reduction with set algebra,
critical-path analysis, change-impact queries by BFS over reverse edges, resource- and
team-constrained list scheduling on ``heapq`` with tuple priorities, test sharding by
greedy bin packing, ``sorted(key=...)``/``max(key=...)`` everywhere, and ``itertools``
(``chain``, ``groupby``, ``pairwise``, ``accumulate``, ``islice``, ``combinations``) in
the reports.

Data shape: a generated monorepo graph of 3,200 targets (libraries, binaries, tests,
codegen) in 14 layers with ~12k edges, 5 teams of workers with resource limits; one run
validates and reduces the graph, answers 1,500 change-impact queries, plans the full
build under 4 what-if scenarios and shards the affected tests of 300 change sets.
"""

import heapq
import itertools
import operator
from collections import Counter, defaultdict, deque, namedtuple
from typing import NamedTuple

REQUIRES: list[str] = []

N_TARGETS = 3_200
LAYERS = 14

Target = namedtuple("Target", "name kind team duration cpu mem tags")
Edge = namedtuple("Edge", "src dst kind")


class Slot(NamedTuple):
    target: str
    worker: str
    start: int
    end: int


class CycleError(ValueError):
    def __init__(self, cycle: list[str]) -> None:
        super().__init__(" -> ".join(cycle))
        self.cycle = cycle


# --------------------------------------------------------------------------------------
# the graph
# --------------------------------------------------------------------------------------


class BuildGraph:
    def __init__(self, targets: list[Target], edges: list[Edge]) -> None:
        self.targets: dict[str, Target] = {t.name: t for t in targets}
        self.deps: defaultdict[str, set[str]] = defaultdict(set)      # target -> what it needs
        self.rdeps: defaultdict[str, set[str]] = defaultdict(set)     # target -> who needs it
        self.edge_kinds: dict[tuple[str, str], str] = {}
        for e in edges:
            if e.src not in self.targets or e.dst not in self.targets:
                raise KeyError(f"dangling edge {e}")
            self.deps[e.src].add(e.dst)
            self.rdeps[e.dst].add(e.src)
            self.edge_kinds[e.src, e.dst] = e.kind

    def __len__(self) -> int:
        return len(self.targets)

    def edges(self):
        for src in sorted(self.deps):
            for dst in sorted(self.deps[src]):
                yield Edge(src, dst, self.edge_kinds[src, dst])

    # -- ordering ------------------------------------------------------------------------
    def topological_order(self) -> list[str]:
        indegree = Counter({name: len(self.deps.get(name, ())) for name in self.targets})
        ready = deque(sorted(name for name, n in indegree.items() if n == 0))
        order = []
        while ready:
            name = ready.popleft()
            order.append(name)
            for parent in sorted(self.rdeps.get(name, ())):
                indegree[parent] -= 1
                if indegree[parent] == 0:
                    ready.append(parent)
        if len(order) != len(self.targets):
            raise CycleError(self.find_cycle())
        return order

    def find_cycle(self) -> list[str]:
        WHITE, GREY, BLACK = 0, 1, 2
        colour = dict.fromkeys(self.targets, WHITE)
        for root in sorted(self.targets):
            if colour[root] != WHITE:
                continue
            stack = [(root, iter(sorted(self.deps.get(root, ()))))]
            path = [root]
            colour[root] = GREY
            while stack:
                node, children = stack[-1]
                for child in children:
                    if colour[child] == GREY:
                        return path[path.index(child):] + [child]
                    if colour[child] == WHITE:
                        colour[child] = GREY
                        stack.append((child, iter(sorted(self.deps.get(child, ())))))
                        path.append(child)
                        break
                else:
                    colour[node] = BLACK
                    stack.pop()
                    path.pop()
        return []

    # -- closures ------------------------------------------------------------------------
    def all_dependencies(self, order: list[str]) -> dict[str, frozenset[str]]:
        closure: dict[str, frozenset[str]] = {}
        for name in order:
            acc: set[str] = set()
            for dep in self.deps.get(name, ()):
                acc.add(dep)
                acc |= closure[dep]
            closure[name] = frozenset(acc)
        return closure

    def transitive_reduction(self, closure: dict[str, frozenset[str]]) -> list[Edge]:
        """Edges implied by another path are redundant: drop them."""
        redundant = []
        for src in sorted(self.deps):
            direct = self.deps[src]
            implied: set[str] = set()
            for dep in direct:
                implied |= closure[dep]
            for dst in sorted(direct & implied):
                redundant.append(Edge(src, dst, self.edge_kinds[src, dst]))
        return redundant

    def affected_by(self, changed: list[str]) -> set[str]:
        seen = set(changed)
        queue = deque(changed)
        while queue:
            node = queue.popleft()
            for parent in self.rdeps.get(node, ()):
                if parent not in seen:
                    seen.add(parent)
                    queue.append(parent)
        return seen

    def critical_path(self, order: list[str], scale: dict[str, float] | None = None):
        finish: dict[str, int] = {}
        via: dict[str, str | None] = {}
        for name in order:
            t = self.targets[name]
            duration = int(t.duration * (scale or {}).get(t.kind, 1.0))
            best_dep = max(sorted(self.deps.get(name, ())), key=finish.__getitem__, default=None)
            finish[name] = duration + (finish[best_dep] if best_dep else 0)
            via[name] = best_dep
        end = max(sorted(finish), key=lambda n: (finish[n], n))
        path = []
        node: str | None = end
        while node is not None:
            path.append(node)
            node = via[node]
        return finish[end], path[::-1]


# --------------------------------------------------------------------------------------
# scheduling
# --------------------------------------------------------------------------------------


class Worker:
    __slots__ = ("name", "team", "cpu", "mem", "busy_until", "log")

    def __init__(self, name: str, team: str, cpu: int, mem: int) -> None:
        self.name = name
        self.team = team
        self.cpu = cpu
        self.mem = mem
        self.busy_until = 0
        self.log: list[Slot] = []

    def __repr__(self) -> str:
        return f"Worker({self.name!r}, {self.team!r})"


def schedule(graph: BuildGraph, order: list[str], workers: list[Worker],
             scale: dict[str, float], limits: dict[str, int], lookahead: int = 24) -> list[Slot]:
    """List scheduling: per team, start the ready target with the longest remaining tail
    first; targets that do not fit (machine size, tag limits) wait, within a lookahead."""
    tail: dict[str, int] = {}
    for name in reversed(order):
        t = graph.targets[name]
        own = int(t.duration * scale.get(t.kind, 1.0))
        tail[name] = own + max((tail[p] for p in graph.rdeps.get(name, ())), default=0)
    waiting = Counter({name: len(graph.deps.get(name, ())) for name in order})
    ready: defaultdict[str, list[tuple[int, str]]] = defaultdict(list)
    for n in order:
        if waiting[n] == 0:
            ready[graph.targets[n].team].append((-tail[n], n))
    for heap in ready.values():
        heapq.heapify(heap)
    by_team: defaultdict[str, list[Worker]] = defaultdict(list)
    for w in workers:
        by_team[w.team].append(w)
    running: list[tuple[int, str, str]] = []   # (end, target, worker)
    in_use: Counter[str] = Counter()
    slots: list[Slot] = []
    now = 0
    while running or any(ready.values()):
        for team in sorted(ready):
            heap = ready[team]
            free = [w for w in by_team[team] if w.busy_until <= now]
            skipped = []
            while heap and free and len(skipped) < lookahead:
                prio, name = heapq.heappop(heap)
                t = graph.targets[name]
                fits = [w for w in free if w.cpu >= t.cpu and w.mem >= t.mem]
                if not fits or any(in_use[tag] >= limits.get(tag, 1 << 30) for tag in t.tags):
                    skipped.append((prio, name))
                    continue
                worker = min(fits, key=operator.attrgetter("busy_until", "name"))
                free.remove(worker)
                duration = int(t.duration * scale.get(t.kind, 1.0))
                slot = Slot(name, worker.name, now, now + duration)
                worker.busy_until = slot.end
                worker.log.append(slot)
                slots.append(slot)
                in_use.update(t.tags)
                heapq.heappush(running, (slot.end, name, worker.name))
            for item in skipped:
                heapq.heappush(heap, item)
        if not running:
            raise RuntimeError("deadlock: ready targets cannot be placed")
        now, name, _worker = heapq.heappop(running)
        finished = [name]
        while running and running[0][0] == now:
            finished.append(heapq.heappop(running)[1])
        for done in finished:
            in_use.subtract(graph.targets[done].tags)
            for parent in graph.rdeps.get(done, ()):
                waiting[parent] -= 1
                if waiting[parent] == 0:
                    heapq.heappush(ready[graph.targets[parent].team], (-tail[parent], parent))
    return slots


def shard_tests(tests: list[Target], n_shards: int) -> list[list[str]]:
    """Longest-processing-time-first bin packing."""
    shards = [(0, i, []) for i in range(n_shards)]
    heapq.heapify(shards)
    for t in sorted(tests, key=lambda t: (-t.duration, t.name)):
        load, i, names = heapq.heappop(shards)
        names.append(t.name)
        heapq.heappush(shards, (load + t.duration, i, names))
    return [names for _, _, names in sorted(shards, key=operator.itemgetter(1))]


# --------------------------------------------------------------------------------------
# workload
# --------------------------------------------------------------------------------------

TEAMS = ["infra", "payments", "search", "mobile", "data"]
KINDS = ["lib", "lib", "lib", "codegen", "binary", "test", "test", "test"]


def setup():
    import random

    rng = random.Random(1_000_003)
    targets = []
    layers: list[list[str]] = [[] for _ in range(LAYERS)]
    for i in range(N_TARGETS):
        layer = min(int(rng.expovariate(0.25)), LAYERS - 1)
        kind = "codegen" if layer == 0 and rng.random() < 0.3 else rng.choice(KINDS)
        team = TEAMS[(i * 7 + layer) % len(TEAMS)] if rng.random() < 0.8 else rng.choice(TEAMS)
        pkg = f"//{team}/{rng.choice(['core', 'api', 'util', 'model', 'svc', 'ui', 'etl'])}"
        name = f"{pkg}:{kind}_{i}"
        dur = {"lib": 30, "codegen": 15, "binary": 90, "test": 45}[kind]
        tags = tuple(sorted(rng.sample(["network", "gpu", "db", "large_disk"],
                                       rng.choice([0, 0, 0, 1, 1, 2]))))
        targets.append((name, kind, team, int(dur * rng.lognormvariate(0, 0.6)) + 1,
                        rng.choice([1, 1, 2, 4]), rng.choice([1, 2, 4, 8]), tags))
        layers[layer].append(name)
    edges = []
    for layer in range(1, LAYERS):
        below = [n for lower in layers[:layer] for n in lower]
        for name in layers[layer]:
            k = min(len(below), rng.randint(1, 9))
            for dep in rng.sample(below, k):
                edges.append((name, dep, rng.choice(["compile", "compile", "runtime", "data"])))
    names = [t[0] for t in targets]
    changes = [sorted(rng.sample(names, rng.randint(1, 6))) for _ in range(1_500)]
    return {"targets": targets, "edges": edges, "changes": changes}


def run(state):
    targets = [Target(*t) for t in state["targets"]]
    edges = [Edge(*e) for e in state["edges"]]
    graph = BuildGraph(targets, edges)
    order = graph.topological_order()

    # inject a cycle into a copy to exercise the error path
    bad = BuildGraph(targets, edges + [Edge(order[0], order[-1], "compile")])
    try:
        bad.topological_order()
        cycle = []
    except CycleError as exc:
        cycle = exc.cycle

    closure = graph.all_dependencies(order)
    redundant = graph.transitive_reduction(closure)
    depth_hist = Counter(min(len(v) // 50, 20) for v in closure.values())

    # change-impact queries (CI: which targets to rebuild/test for a commit)
    impact_sizes = []
    tests_to_run = Counter()
    for changed in state["changes"]:
        affected = graph.affected_by(changed)
        impact_sizes.append(len(affected))
        for name in affected:
            if graph.targets[name].kind == "test":
                tests_to_run[graph.targets[name].team] += 1

    # what-if build plans
    plans = {}
    scenarios = {
        "baseline": ({}, 6, {"gpu": 2, "db": 4}),
        "slow_tests": ({"test": 1.8}, 6, {"gpu": 2, "db": 4}),
        "small_pool": ({}, 3, {"gpu": 1, "db": 2}),
        "fast_codegen": ({"codegen": 0.3, "binary": 1.2}, 5, {"gpu": 2, "db": 4}),
    }
    for label, (scale, per_team, limits) in scenarios.items():
        workers = [Worker(f"{team}-{i}", team, cpu=4 if i % 2 else 8, mem=16)
                   for team in TEAMS for i in range(per_team)]
        slots = schedule(graph, order, workers, scale, limits)
        makespan = max(s.end for s in slots)
        cp_len, cp = graph.critical_path(order, scale)
        utilisation = {}
        for w in workers:
            busy = sum(s.end - s.start for s in w.log)
            utilisation[w.name] = round(busy / makespan, 4)
        by_team = {team: round(sum(u for _, u in group) / per_team, 4) for team, group in
                   itertools.groupby(sorted(utilisation.items(), key=lambda kv: kv[0].split("-")[0]),
                                     key=lambda kv: kv[0].split("-")[0])}
        gaps = []
        for w in workers:
            gaps.extend(b.start - a.end for a, b in itertools.pairwise(w.log) if b.start > a.end)
        load_curve = list(itertools.accumulate(
            sorted(Counter(s.start // 600 for s in slots).items()), lambda acc, kv: (kv[0], acc[1] + kv[1])))
        plans[label] = {"makespan": makespan, "critical": cp_len, "cp_len": len(cp),
                        "cp_head": cp[:3], "by_team": by_team, "idle_gaps": [len(gaps), sum(gaps)],
                        "load_curve": load_curve[-3:],
                        "first": [tuple(s) for s in itertools.islice(sorted(slots, key=lambda s: (s.start, s.target)), 5)]}

    # shard the tests of the larger change sets across CI machines
    shard_report = []
    for changed in state["changes"][:300]:
        tests = [graph.targets[n] for n in sorted(graph.affected_by(changed))
                 if graph.targets[n].kind == "test"]
        shards = shard_tests(tests, 8)
        loads = [sum(graph.targets[n].duration for n in s) for s in shards]
        shard_report.append([len(tests), max(loads) - min(loads) if loads else 0])

    # coupling between teams: pairs of teams sharing dependencies
    team_deps = defaultdict(set)
    for e in edges:
        team_deps[graph.targets[e.src].team].add(graph.targets[e.dst].team)
    coupling = sorted((a, b) for a, b in itertools.combinations(sorted(team_deps), 2)
                      if b in team_deps[a] and a in team_deps[b])

    hubs = sorted(graph.rdeps, key=lambda n: (-len(graph.rdeps[n]), n))[:10]
    return {
        "targets": len(graph), "edges": len(edges), "order_head": order[:5],
        "cycle_len": len(cycle), "redundant": len(redundant),
        "redundant_kinds": dict(sorted(Counter(e.kind for e in redundant).items())),
        "depth_hist": sorted(depth_hist.items()),
        "impact": [sum(impact_sizes), max(impact_sizes), sorted(tests_to_run.items())],
        "plans": plans, "shards": shard_report, "coupling": coupling,
        "hubs": [(h, len(graph.rdeps[h])) for h in hubs],
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
