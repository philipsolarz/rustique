"""Entity-component-system arena simulation: vectors, components, systems, a spatial hash.

Stands for game logic, agent-based models, robotics/sensor fusion loops and interactive
simulations written in plain Python (and for the architecture of ``esper``-style ECS
libraries): a ``Vec2`` value class with ``__slots__`` and arithmetic dunders (``__add__``,
``__sub__``, ``__mul__``/``__rmul__``, ``__truediv__``, ``__neg__``, ``__abs__``,
``__eq__``/``__hash__``, ``__iter__``), components as slotted dataclasses, frozen dataclass
grid cells as dict keys (generated ``__hash__``/``__eq__``), a world holding
``dict[type, dict[int, component]]`` plus cached ``dict``-of-``set`` queries, systems
iterated every tick, an AI state machine over an ``Enum`` with ``match``, a spatial hash
for neighbour and collision queries, an event queue on ``deque``, ``Counter``
inventories, and per-tick statistics.

Run: 4 teams of 60 units, ~250-300 live entities (units, projectiles, loot), 240 ticks:
AI decisions, movement, collision separation, ranged combat with projectiles, deaths, loot
drops and respawn waves.
"""

import enum
import math
from collections import Counter, defaultdict, deque
from dataclasses import dataclass, field

REQUIRES: list[str] = []

TICKS = 240
UNITS_PER_TEAM = 60
ARENA = 600.0
CELL = 50.0
DT = 0.1


# --------------------------------------------------------------------------------------
# math
# --------------------------------------------------------------------------------------


class Vec2:
    __slots__ = ("x", "y")

    def __init__(self, x: float = 0.0, y: float = 0.0) -> None:
        self.x = x
        self.y = y

    def __add__(self, other: "Vec2") -> "Vec2":
        return Vec2(self.x + other.x, self.y + other.y)

    def __sub__(self, other: "Vec2") -> "Vec2":
        return Vec2(self.x - other.x, self.y - other.y)

    def __mul__(self, k: float) -> "Vec2":
        return Vec2(self.x * k, self.y * k)

    __rmul__ = __mul__

    def __truediv__(self, k: float) -> "Vec2":
        return Vec2(self.x / k, self.y / k)

    def __neg__(self) -> "Vec2":
        return Vec2(-self.x, -self.y)

    def __abs__(self) -> float:
        return math.hypot(self.x, self.y)

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Vec2):
            return NotImplemented
        return self.x == other.x and self.y == other.y

    def __hash__(self) -> int:
        return hash((self.x, self.y))

    def __iter__(self):
        yield self.x
        yield self.y

    def __repr__(self) -> str:
        return f"Vec2({self.x:.3f}, {self.y:.3f})"

    def dot(self, other: "Vec2") -> float:
        return self.x * other.x + self.y * other.y

    def length_sq(self) -> float:
        return self.x * self.x + self.y * self.y

    def normalized(self) -> "Vec2":
        n = abs(self)
        return self / n if n > 1e-9 else Vec2()

    def clamped(self, max_len: float) -> "Vec2":
        n = abs(self)
        return self * (max_len / n) if n > max_len else self

    @classmethod
    def polar(cls, angle: float, length: float) -> "Vec2":
        return cls(math.cos(angle) * length, math.sin(angle) * length)


@dataclass(frozen=True, slots=True)
class Cell:
    x: int
    y: int

    @classmethod
    def of(cls, p: Vec2) -> "Cell":
        return cls(int(p.x // CELL), int(p.y // CELL))

    def around(self, r: int = 1):
        for dx in range(-r, r + 1):
            for dy in range(-r, r + 1):
                yield Cell(self.x + dx, self.y + dy)


# --------------------------------------------------------------------------------------
# components
# --------------------------------------------------------------------------------------


class Team(enum.Enum):
    RED = 0
    BLUE = 1
    GREEN = 2
    GOLD = 3


class AI(enum.Enum):
    IDLE = enum.auto()
    SEEK = enum.auto()
    ATTACK = enum.auto()
    FLEE = enum.auto()
    REGROUP = enum.auto()


class Kind(enum.Enum):
    UNIT = "unit"
    PROJECTILE = "projectile"
    LOOT = "loot"


@dataclass(slots=True)
class Position:
    p: Vec2


@dataclass(slots=True)
class Velocity:
    v: Vec2
    max_speed: float
    drag: float = 0.9


@dataclass(slots=True)
class Health:
    hp: int
    max_hp: int
    regen: int = 0

    @property
    def ratio(self) -> float:
        return self.hp / self.max_hp


@dataclass(slots=True)
class Faction:
    team: Team


@dataclass(slots=True)
class Brain:
    state: AI
    home: Vec2
    target: int | None = None
    timer: int = 0
    aggression: float = 0.5


@dataclass(slots=True)
class Weapon:
    damage: int
    reach: float
    cooldown: int
    speed: float
    ready_at: int = 0


@dataclass(slots=True)
class Projectile:
    owner: int
    team: Team
    damage: int
    ttl: int


@dataclass(slots=True)
class Loot:
    item: str
    amount: int


@dataclass(slots=True)
class Inventory:
    items: Counter = field(default_factory=Counter)


@dataclass(slots=True)
class Collider:
    radius: float
    kind: Kind


@dataclass(frozen=True, slots=True)
class Damage:
    target: int
    source: int
    amount: int


@dataclass(frozen=True, slots=True)
class Died:
    entity: int
    killer: int
    where: Vec2


# --------------------------------------------------------------------------------------
# the world
# --------------------------------------------------------------------------------------


class World:
    def __init__(self) -> None:
        self._next = 1
        self.entities: set[int] = set()
        self.store: defaultdict[type, dict[int, object]] = defaultdict(dict)
        self._queries: dict[tuple[type, ...], list[int]] = {}
        self.events: deque = deque()
        self.dead: list[int] = []
        self.tick = 0

    def spawn(self, *components) -> int:
        eid = self._next
        self._next += 1
        self.entities.add(eid)
        for c in components:
            self.store[type(c)][eid] = c
        self._queries.clear()
        return eid

    def get(self, eid: int, ctype: type):
        return self.store[ctype].get(eid)

    def has(self, eid: int, ctype: type) -> bool:
        return eid in self.store[ctype]

    def destroy(self, eid: int) -> None:
        self.dead.append(eid)

    def flush(self) -> None:
        if not self.dead:
            return
        for eid in self.dead:
            if eid in self.entities:
                self.entities.discard(eid)
                for comps in self.store.values():
                    comps.pop(eid, None)
        self.dead.clear()
        self._queries.clear()

    def query(self, *ctypes: type):
        ids = self._queries.get(ctypes)
        if ids is None:
            stores = sorted((self.store[c] for c in ctypes), key=len)
            ids = [eid for eid in stores[0] if all(eid in s for s in stores[1:])]
            ids.sort()
            self._queries[ctypes] = ids
        stores = [self.store[c] for c in ctypes]
        for eid in ids:
            yield (eid, *(s[eid] for s in stores))


class SpatialHash:
    def __init__(self) -> None:
        self.cells: defaultdict[Cell, set[int]] = defaultdict(set)
        self.where: dict[int, Cell] = {}

    def rebuild(self, world: World) -> None:
        self.cells.clear()
        self.where.clear()
        for eid, pos in world.query(Position):
            cell = Cell.of(pos.p)
            self.cells[cell].add(eid)
            self.where[eid] = cell

    def near(self, p: Vec2, radius: float):
        r = max(1, int(radius // CELL) + 1)
        center = Cell.of(p)
        for cell in center.around(r):
            bucket = self.cells.get(cell)
            if bucket:
                yield from sorted(bucket)


# --------------------------------------------------------------------------------------
# systems
# --------------------------------------------------------------------------------------


class AISystem:
    def __init__(self, rng, grid: SpatialHash) -> None:
        self.rng = rng
        self.grid = grid
        self.transitions: Counter[tuple[str, str]] = Counter()

    def nearest_enemy(self, world: World, eid: int, p: Vec2, team: Team, radius: float):
        best, best_d = None, radius * radius
        for other in self.grid.near(p, radius):
            if other == eid:
                continue
            faction = world.get(other, Faction)
            if faction is None or faction.team is team or not world.has(other, Health):
                continue
            d = (world.get(other, Position).p - p).length_sq()
            if d < best_d:
                best, best_d = other, d
        return best

    def update(self, world: World) -> None:
        for eid, brain, pos, vel, hp, fac, weapon in world.query(Brain, Position, Velocity, Health,
                                                                 Faction, Weapon):
            old = brain.state
            match brain.state:
                case AI.IDLE:
                    brain.timer -= 1
                    if brain.timer <= 0:
                        brain.timer = self.rng.randint(5, 25)
                        vel.v = vel.v + Vec2.polar(self.rng.uniform(0, math.tau), vel.max_speed * 0.4)
                    if (world.tick + eid) % 4 == 0:  # think every 4th tick
                        target = self.nearest_enemy(world, eid, pos.p, fac.team,
                                                    90.0 + 90 * brain.aggression)
                        if target is not None:
                            brain.target, brain.state = target, AI.SEEK
                case AI.SEEK:
                    tpos = world.get(brain.target, Position)
                    if tpos is None:
                        brain.state, brain.target = AI.IDLE, None
                    else:
                        delta = tpos.p - pos.p
                        if abs(delta) <= weapon.reach:
                            brain.state = AI.ATTACK
                        else:
                            vel.v = vel.v + delta.normalized() * (vel.max_speed * 0.5)
                case AI.ATTACK:
                    tpos = world.get(brain.target, Position)
                    if tpos is None:
                        brain.state, brain.target = AI.IDLE, None
                    elif abs(tpos.p - pos.p) > weapon.reach * 1.2:
                        brain.state = AI.SEEK
                    if hp.ratio < 0.25 + 0.2 * (1 - brain.aggression):
                        brain.state, brain.timer = AI.FLEE, 30
                case AI.FLEE:
                    away = (pos.p - brain.home) * -1.0
                    vel.v = vel.v + away.normalized() * vel.max_speed
                    brain.timer -= 1
                    if brain.timer <= 0 or abs(pos.p - brain.home) < 60:
                        brain.state = AI.REGROUP
                case AI.REGROUP:
                    if hp.ratio > 0.7:
                        brain.state, brain.target = AI.IDLE, None
            if brain.state is not old:
                self.transitions[old.name, brain.state.name] += 1


class MovementSystem:
    def update(self, world: World) -> None:
        for eid, pos, vel in world.query(Position, Velocity):
            vel.v = (vel.v * vel.drag).clamped(vel.max_speed)
            p = pos.p + vel.v * DT
            x, y = p
            if not (0.0 <= x <= ARENA and 0.0 <= y <= ARENA):
                if world.has(eid, Projectile):
                    world.destroy(eid)
                    continue
                x = min(max(x, 0.0), ARENA)
                y = min(max(y, 0.0), ARENA)
                vel.v = -vel.v
                p = Vec2(x, y)
            pos.p = p


class CollisionSystem:
    def __init__(self, grid: SpatialHash) -> None:
        self.grid = grid
        self.contacts = 0

    def update(self, world: World) -> None:
        seen: set[tuple[int, int]] = set()
        for cell, bucket in sorted(self.grid.cells.items(), key=lambda kv: (kv[0].x, kv[0].y)):
            members = sorted(bucket)
            neighbours = sorted({e for c in cell.around(1) for e in self.grid.cells.get(c, ())})
            for a in members:
                ca = world.get(a, Collider)
                if ca is None or ca.kind is not Kind.UNIT:
                    continue
                pa = world.get(a, Position)
                for b in neighbours:
                    if b <= a or (a, b) in seen:
                        continue
                    cb = world.get(b, Collider)
                    if cb is None:
                        continue
                    pb = world.get(b, Position)
                    delta = pb.p - pa.p
                    min_d = ca.radius + cb.radius
                    if delta.length_sq() >= min_d * min_d:
                        continue
                    seen.add((a, b))
                    self.contacts += 1
                    if cb.kind is Kind.UNIT:
                        push = delta.normalized() * ((min_d - abs(delta)) * 0.5)
                        pa.p = pa.p - push
                        pb.p = pb.p + push
                    elif cb.kind is Kind.LOOT:
                        inv = world.get(a, Inventory)
                        loot = world.get(b, Loot)
                        if inv is not None and loot is not None:
                            inv.items[loot.item] += loot.amount
                            world.destroy(b)


class CombatSystem:
    def __init__(self, grid: SpatialHash) -> None:
        self.grid = grid
        self.shots = 0
        self.hits = 0

    def update(self, world: World) -> None:
        tick = world.tick
        for eid, brain, pos, fac, weapon in world.query(Brain, Position, Faction, Weapon):
            if brain.state is not AI.ATTACK or weapon.ready_at > tick or brain.target is None:
                continue
            tpos = world.get(brain.target, Position)
            if tpos is None:
                continue
            aim = (tpos.p - pos.p).normalized()
            world.spawn(Position(pos.p + aim * 6.0), Velocity(aim * weapon.speed, weapon.speed, 1.0),
                        Projectile(eid, fac.team, weapon.damage, 25), Collider(2.0, Kind.PROJECTILE))
            weapon.ready_at = tick + weapon.cooldown
            self.shots += 1
        for eid, proj, pos in world.query(Projectile, Position):
            proj.ttl -= 1
            if proj.ttl <= 0:
                world.destroy(eid)
                continue
            for other in self.grid.near(pos.p, 8.0):
                fac = world.get(other, Faction)
                if fac is None or fac.team is proj.team or not world.has(other, Health):
                    continue
                if (world.get(other, Position).p - pos.p).length_sq() <= 64.0:
                    world.events.append(Damage(other, proj.owner, proj.damage))
                    world.destroy(eid)
                    self.hits += 1
                    break


class HealthSystem:
    def __init__(self) -> None:
        self.kills: Counter[str] = Counter()
        self.deaths_by_team: Counter[str] = Counter()

    def update(self, world: World) -> None:
        dead: set[int] = set()
        while world.events:
            event = world.events.popleft()
            match event:
                case Damage(target=t, source=s, amount=amount) if t not in dead:
                    hp = world.get(t, Health)
                    if hp is None:
                        continue
                    hp.hp -= amount
                    if hp.hp <= 0:
                        dead.add(t)
                        world.events.append(Died(t, s, world.get(t, Position).p))
                case Died(entity=e, killer=k, where=where):
                    team = world.get(e, Faction).team
                    self.deaths_by_team[team.name] += 1
                    killer_team = world.get(k, Faction)
                    if killer_team is not None:
                        self.kills[killer_team.team.name] += 1
                    inv = world.get(e, Inventory)
                    drops = (inv.items.most_common(2) if inv else []) + [("gold", 5)]
                    for item, amount in drops:
                        world.spawn(Position(where + Vec2(len(item), -len(item))), Loot(item, amount),
                                    Collider(5.0, Kind.LOOT))
                    world.destroy(e)
        if world.tick % 10 == 0:
            for eid, hp in world.query(Health):
                if hp.hp < hp.max_hp:
                    hp.hp = min(hp.max_hp, hp.hp + hp.regen)


# --------------------------------------------------------------------------------------
# workload
# --------------------------------------------------------------------------------------

HOMES = {Team.RED: Vec2(180, 180), Team.BLUE: Vec2(420, 180), Team.GREEN: Vec2(180, 420),
         Team.GOLD: Vec2(420, 420)}


def spawn_unit(world: World, rng, team: Team) -> int:
    home = HOMES[team]
    archetype = rng.choice(["grunt", "grunt", "archer", "tank"])
    stats = {"grunt": (100, 12, 30.0, 8, 26.0), "archer": (70, 9, 120.0, 12, 22.0),
             "tank": (220, 18, 25.0, 15, 14.0)}[archetype]
    hp, dmg, reach, cooldown, speed = stats
    return world.spawn(
        Position(home + Vec2(rng.uniform(-120, 120), rng.uniform(-120, 120))),
        Velocity(Vec2(), speed),
        Health(hp, hp, regen=8),
        Faction(team),
        Brain(AI.IDLE, home, timer=rng.randint(0, 20), aggression=rng.random()),
        Weapon(dmg, reach, cooldown, speed=60.0),
        Inventory(Counter({rng.choice(["potion", "arrow", "gem"]): rng.randint(1, 3)})),
        Collider(6.0 if archetype != "tank" else 9.0, Kind.UNIT),
    )


def setup():
    return {"seed": 2_718}


def run(state):
    import random

    rng = random.Random(state["seed"])
    world = World()
    grid = SpatialHash()
    for team in Team:
        for _ in range(UNITS_PER_TEAM):
            spawn_unit(world, rng, team)
    ai = AISystem(rng, grid)
    systems = [ai, MovementSystem(), CollisionSystem(grid), CombatSystem(grid), HealthSystem()]
    population = []
    for tick in range(TICKS):
        world.tick = tick
        grid.rebuild(world)
        for system in systems:
            system.update(world)
            world.flush()
        if tick % 40 == 39:
            alive = Counter(f.team for _, f in world.query(Faction))
            for team in Team:
                for _ in range(max(0, UNITS_PER_TEAM // 2 - alive[team])):
                    spawn_unit(world, rng, team)
        population.append(len(world.entities))
    combat, health = systems[3], systems[4]
    collisions = systems[2]
    centroid = {}
    for team in Team:
        pts = [pos.p for _, pos, fac in world.query(Position, Faction) if fac.team is team]
        total = sum(pts, Vec2())
        centroid[team.name] = repr(total / len(pts)) if pts else None
    loot = Counter()
    for _, inv in world.query(Inventory):
        loot.update(inv.items)
    return {
        "population": population[::20], "final": len(world.entities),
        "shots": combat.shots, "hits": combat.hits, "contacts": collisions.contacts,
        "kills": dict(sorted(health.kills.items())),
        "deaths": dict(sorted(health.deaths_by_team.items())),
        "transitions": sorted(f"{a}>{b}:{n}" for (a, b), n in ai.transitions.items()),
        "centroid": centroid, "loot": dict(sorted(loot.items())),
        "unique_positions": len({pos.p for _, pos in world.query(Position)}),
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
