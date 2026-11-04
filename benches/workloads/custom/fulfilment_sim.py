"""Discrete-event simulation of a fulfilment centre, on an in-house process-based engine.

Stands for the capacity-planning, logistics, queueing and network simulations that
operations-research and engineering teams write (in-house or with SimPy, whose design this
engine follows): a ``heapq`` scheduler over event objects ordered by user ``__lt__`` /
``__eq__`` (``functools.total_ordering``, ``__slots__``), processes written as generators
that ``yield`` events and are resumed with ``send``/``throw``, resources/stores with
``deque`` wait queues and ``with request:`` context managers, priority requests and
preemption via exceptions thrown into generators, closures for time-varying arrival
rates and service-time distributions, ``functools.partial`` callbacks, and monitors
(Welford running statistics, ``bisect`` histograms, time-weighted queue lengths).

Model: orders arrive (non-homogeneous Poisson, hourly profile, thinning) over 3 simulated
days; each is picked (30 pickers), packed (10 stations; express orders jump the queue and
can preempt), and loaded onto trucks that leave every 45 minutes; stock is drawn from a bin
container replenished by a supplier process; packing machines break down and get repaired.
Three what-if scenarios per run: ~40k orders, ~400k processed events in all.
"""

import bisect
import functools
import heapq
import itertools
import math
from collections import deque

REQUIRES: list[str] = []

SIM_DAYS = 3
SEED = 9_101


# --------------------------------------------------------------------------------------
# the engine
# --------------------------------------------------------------------------------------

URGENT, NORMAL = 0, 1


@functools.total_ordering
class Scheduled:
    """An entry of the event queue; ordered by (time, priority, insertion order)."""

    __slots__ = ("time", "priority", "seq", "event")

    def __init__(self, time: float, priority: int, seq: int, event: "Event") -> None:
        self.time = time
        self.priority = priority
        self.seq = seq
        self.event = event

    def _key(self):
        return (self.time, self.priority, self.seq)

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Scheduled):
            return NotImplemented
        return self._key() == other._key()

    def __lt__(self, other: "Scheduled") -> bool:
        if not isinstance(other, Scheduled):
            return NotImplemented
        return self._key() < other._key()

    __hash__ = None  # mutable ordering key


class Interrupt(Exception):
    def __init__(self, cause: object) -> None:
        super().__init__(cause)
        self.cause = cause


class Event:
    __slots__ = ("env", "callbacks", "value", "ok", "triggered", "processed")

    def __init__(self, env: "Environment") -> None:
        self.env = env
        self.callbacks: list = []
        self.value = None
        self.ok = True
        self.triggered = False
        self.processed = False

    def succeed(self, value=None, priority: int = NORMAL) -> "Event":
        if self.triggered:
            raise RuntimeError(f"{self!r} already triggered")
        self.triggered = True
        self.value = value
        self.env._push(0.0, priority, self)
        return self

    def fail(self, exc: BaseException) -> "Event":
        self.triggered = True
        self.ok = False
        self.value = exc
        self.env._push(0.0, NORMAL, self)
        return self


class Timeout(Event):
    __slots__ = ("delay",)

    def __init__(self, env: "Environment", delay: float, value=None) -> None:
        super().__init__(env)
        if delay < 0:
            raise ValueError(f"negative delay {delay}")
        self.delay = delay
        self.triggered = True
        self.value = value
        env._push(delay, NORMAL, self)


class Process(Event):
    __slots__ = ("gen", "target", "name")

    def __init__(self, env: "Environment", gen, name: str = "") -> None:
        super().__init__(env)
        self.gen = gen
        self.name = name
        self.target: Event | None = None
        start = Event(env)
        start.callbacks.append(self._resume)
        start.succeed(priority=URGENT)

    @property
    def is_alive(self) -> bool:
        return not self.triggered

    def interrupt(self, cause: object = None) -> None:
        if not self.is_alive:
            return
        kick = Event(self.env)
        kick.ok = False
        kick.value = Interrupt(cause)
        kick.callbacks.append(self._resume)
        kick.triggered = True
        self.env._push(0.0, URGENT, kick)
        # detach from whatever we were waiting for
        if self.target is not None and self._resume in self.target.callbacks:
            self.target.callbacks.remove(self._resume)

    def _resume(self, event: Event) -> None:
        if self.triggered:
            return
        while True:
            try:
                if event.ok:
                    nxt = self.gen.send(event.value)
                else:
                    nxt = self.gen.throw(event.value)
            except StopIteration as stop:
                self.target = None
                self.succeed(stop.value)
                return
            if not isinstance(nxt, Event):
                raise TypeError(f"{self.name}: yielded {nxt!r}, not an Event")
            if nxt.processed:
                event = nxt  # already happened: continue immediately
                continue
            nxt.callbacks.append(self._resume)
            self.target = nxt
            return


class Environment:
    def __init__(self) -> None:
        self.now = 0.0
        self._queue: list[Scheduled] = []
        self._seq = itertools.count()
        self.steps = 0

    def _push(self, delay: float, priority: int, event: Event) -> None:
        heapq.heappush(self._queue, Scheduled(self.now + delay, priority, next(self._seq), event))

    def timeout(self, delay: float, value=None) -> Timeout:
        return Timeout(self, delay, value)

    def event(self) -> Event:
        return Event(self)

    def process(self, gen, name: str = "") -> Process:
        return Process(self, gen, name)

    def all_of(self, events: list[Event]) -> Event:
        done = Event(self)
        pending = [e for e in events if not e.processed]
        if not pending:
            return done.succeed([e.value for e in events])
        remaining = {"n": len(pending)}

        def one_done(_ev: Event) -> None:
            remaining["n"] -= 1
            if remaining["n"] == 0 and not done.triggered:
                done.succeed([e.value for e in events])

        for e in pending:
            e.callbacks.append(one_done)
        return done

    def run(self, until: float) -> None:
        horizon = Scheduled(until, NORMAL, -1, None)
        queue = self._queue
        while queue:
            if queue[0] >= horizon:
                break
            item = heapq.heappop(queue)
            self.now = item.time
            event = item.event
            callbacks, event.callbacks = event.callbacks, []
            event.processed = True
            self.steps += 1
            for cb in callbacks:
                cb(event)
        self.now = until


class Request(Event):
    __slots__ = ("resource", "priority", "key", "preempt", "usage_since", "owner")

    def __init__(self, resource: "Resource", priority: int, preempt: bool, owner) -> None:
        super().__init__(resource.env)
        self.resource = resource
        self.priority = priority
        self.key = (priority, resource.env.now, next(resource._ticket))
        self.preempt = preempt
        self.usage_since = 0.0
        self.owner = owner
        resource._request(self)

    def __enter__(self) -> "Request":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.resource.release(self)

    def __lt__(self, other: "Request") -> bool:
        return self.key < other.key


class Resource:
    """Capacity-limited resource with a priority queue; optional preemption."""

    def __init__(self, env: Environment, capacity: int, name: str) -> None:
        self.env = env
        self.capacity = capacity
        self.name = name
        self.users: list[Request] = []
        self.waiting: list[Request] = []
        self._ticket = itertools.count()
        self.monitor = QueueMonitor(env, name)

    def request(self, priority: int = NORMAL, preempt: bool = False, owner=None) -> Request:
        return Request(self, priority, preempt, owner)

    def _request(self, req: Request) -> None:
        if len(self.users) < self.capacity:
            self._grant(req)
            return
        if req.preempt:
            victim = max(self.users)
            if req < victim:
                self.users.remove(victim)
                if victim.owner is not None:
                    victim.owner.interrupt(("preempted", self.name))
                self._grant(req)
                return
        heapq.heappush(self.waiting, req)
        self.monitor.record(len(self.waiting))

    def _grant(self, req: Request) -> None:
        req.usage_since = self.env.now
        self.users.append(req)
        req.succeed()

    def release(self, req: Request) -> None:
        if req in self.users:
            self.users.remove(req)
        elif req in self.waiting:
            self.waiting.remove(req)
            heapq.heapify(self.waiting)
        self.wake()

    def resize(self, capacity: int) -> None:
        self.capacity = capacity
        self.wake()

    def wake(self) -> None:
        while self.waiting and len(self.users) < self.capacity:
            self._grant(heapq.heappop(self.waiting))
        self.monitor.record(len(self.waiting))


class Store:
    """FIFO store of items with blocking get."""

    def __init__(self, env: Environment) -> None:
        self.env = env
        self.items: deque = deque()
        self.getters: deque[Event] = deque()

    def put(self, item) -> None:
        if self.getters:
            self.getters.popleft().succeed(item)
        else:
            self.items.append(item)

    def get(self) -> Event:
        ev = self.env.event()
        if self.items:
            ev.succeed(self.items.popleft())
        else:
            self.getters.append(ev)
        return ev


class Container:
    """Continuous level (stock units) with blocking ``take``."""

    def __init__(self, env: Environment, level: int, capacity: int) -> None:
        self.env = env
        self.level = level
        self.capacity = capacity
        self.takers: deque[tuple[int, Event]] = deque()
        self.stockouts = 0

    def take(self, amount: int) -> Event:
        ev = self.env.event()
        if not self.takers and self.level >= amount:
            self.level -= amount
            ev.succeed(amount)
        else:
            self.stockouts += 1
            self.takers.append((amount, ev))
        return ev

    def refill(self, amount: int) -> int:
        added = min(amount, self.capacity - self.level)
        self.level += added
        while self.takers and self.takers[0][0] <= self.level:
            want, ev = self.takers.popleft()
            self.level -= want
            ev.succeed(want)
        return added


# --------------------------------------------------------------------------------------
# monitoring
# --------------------------------------------------------------------------------------


class RunningStats:
    """Welford's online mean/variance plus a fixed-bin histogram."""

    BINS = [1, 2, 5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 240, 360, 480, 720]

    def __init__(self, name: str) -> None:
        self.name = name
        self.n = 0
        self.mean = 0.0
        self._m2 = 0.0
        self.min = math.inf
        self.max = -math.inf
        self.hist = [0] * (len(self.BINS) + 1)

    def add(self, x: float) -> None:
        self.n += 1
        delta = x - self.mean
        self.mean += delta / self.n
        self._m2 += delta * (x - self.mean)
        if x < self.min:
            self.min = x
        if x > self.max:
            self.max = x
        self.hist[bisect.bisect_right(self.BINS, x)] += 1

    @property
    def stdev(self) -> float:
        return math.sqrt(self._m2 / (self.n - 1)) if self.n > 1 else 0.0

    def percentile(self, q: float) -> float:
        target = q * self.n
        seen = 0
        for i, count in enumerate(self.hist):
            seen += count
            if seen >= target:
                return float(self.BINS[i]) if i < len(self.BINS) else self.max
        return self.max

    def summary(self) -> dict:
        return {"n": self.n, "mean": round(self.mean, 6), "stdev": round(self.stdev, 6),
                "min": round(self.min, 6), "max": round(self.max, 6),
                "p50": self.percentile(0.5), "p95": self.percentile(0.95), "hist": self.hist}


class QueueMonitor:
    """Time-weighted average of a queue length."""

    def __init__(self, env: Environment, name: str) -> None:
        self.env = env
        self.name = name
        self.last_t = 0.0
        self.last_v = 0
        self.area = 0.0
        self.peak = 0

    def record(self, value: int) -> None:
        now = self.env.now
        self.area += self.last_v * (now - self.last_t)
        self.last_t, self.last_v = now, value
        if value > self.peak:
            self.peak = value

    def average(self, until: float) -> float:
        return (self.area + self.last_v * (until - self.last_t)) / until if until else 0.0


# --------------------------------------------------------------------------------------
# the model
# --------------------------------------------------------------------------------------

HOURLY_PROFILE = [0.2, 0.15, 0.1, 0.1, 0.15, 0.3, 0.6, 1.0, 1.3, 1.5, 1.6, 1.7,
                  1.8, 1.7, 1.6, 1.5, 1.5, 1.6, 1.9, 2.1, 2.0, 1.5, 0.9, 0.4]


def make_arrival_rate(base_per_hour: float, profile: list[float], weekend_factor: float):
    """Closure over the demand profile: orders per minute at simulated minute ``t``."""
    def rate(t: float) -> float:
        hour = int(t // 60) % 24
        day = int(t // 1440)
        factor = weekend_factor if day % 7 in (5, 6) else 1.0
        return base_per_hour * profile[hour] * factor / 60.0
    return rate


def make_service_time(rng, base: float, per_item: float, sigma: float):
    def sample(n_items: int) -> float:
        return base + per_item * n_items * rng.lognormvariate(0.0, sigma)
    return sample


class Centre:
    def __init__(self, env: Environment, rng, express_share: float) -> None:
        self.env = env
        self.rng = rng
        self.express_share = express_share
        self.pickers = Resource(env, 30, "pickers")
        self.packers = Resource(env, 10, "packers")
        self.dock = Store(env)
        self.stock = Container(env, level=4_000, capacity=6_000)
        self.pick_time = make_service_time(rng, 1.5, 0.9, 0.35)
        self.pack_time = make_service_time(rng, 0.8, 0.25, 0.25)
        self.stats = {name: RunningStats(name) for name in
                      ("pick_wait", "pick", "pack_wait", "pack", "dock_wait", "lead_time")}
        self.shipped_by_truck: list[int] = []
        self.counters = {"orders": 0, "express": 0, "preempted": 0, "repacked": 0,
                         "breakdowns": 0, "refills": 0, "late": 0}
        self.packing_now: dict[int, Process] = {}

    def order(self, order_id: int, n_items: int, express: bool):
        env = self.env
        arrived = env.now
        stats = self.stats
        yield self.stock.take(n_items)
        prio = URGENT if express else NORMAL
        with self.pickers.request(prio) as req:
            yield req
            stats["pick_wait"].add(env.now - arrived)
            t = self.pick_time(n_items)
            yield env.timeout(t)
            stats["pick"].add(t)
        packed = False
        attempts = 0
        while not packed:
            attempts += 1
            queued = env.now
            req = self.packers.request(prio, preempt=express, owner=self.packing_now.get(order_id))
            try:
                yield req
                stats["pack_wait"].add(env.now - queued)
                t = self.pack_time(n_items)
                yield env.timeout(t)
                stats["pack"].add(t)
                packed = True
            except Interrupt as intr:
                kind, _ = intr.cause
                self.counters["preempted" if kind == "preempted" else "repacked"] += 1
            finally:
                self.packers.release(req)
        ready = env.now
        truck = yield self.dock.get()
        stats["dock_wait"].add(env.now - ready)
        lead = env.now - arrived
        stats["lead_time"].add(lead)
        if lead > (240 if express else 1_440):
            self.counters["late"] += 1
        truck.append(order_id)
        return attempts

    def start_order(self, order_id: int, n_items: int, express: bool) -> Process:
        """Start an order's process and register it so preemption can reach it."""
        proc = self.env.process(self.order(order_id, n_items, express), f"order-{order_id}")
        self.packing_now[order_id] = proc
        proc.callbacks.append(functools.partial(self._finished, order_id))
        return proc

    def _finished(self, order_id: int, _event: Event) -> None:
        self.packing_now.pop(order_id, None)

    def source(self, rate_fn, horizon: float):
        env, rng = self.env, self.rng
        order_id = 0
        lam_max = max(rate_fn(h * 60.0) for h in range(24 * 7))
        # thinning (Lewis-Shedler) for a non-homogeneous Poisson process
        while env.now < horizon:
            yield env.timeout(rng.expovariate(lam_max))
            if rng.random() * lam_max > rate_fn(env.now):
                continue
            order_id += 1
            express = rng.random() < self.express_share
            n_items = 1 + int(rng.paretovariate(2.0)) + rng.randrange(3)
            self.counters["orders"] += 1
            self.counters["express"] += express
            self.start_order(order_id, n_items, express)

    def trucks(self, every: float, capacity: int):
        env = self.env
        while True:
            yield env.timeout(every)
            truck: list[int] = []
            for _ in range(capacity):
                self.dock.put(truck)
            yield env.timeout(0.5)
            # the truck leaves; getters still holding it were already served
            self.shipped_by_truck.append(len(truck))
            self.dock.items.clear()

    def supplier(self, every: float):
        env, rng = self.env, self.rng
        while True:
            yield env.timeout(every * rng.uniform(0.8, 1.2))
            added = self.stock.refill(rng.randint(1_200, 2_400))
            if added:
                self.counters["refills"] += 1

    def breakdowns(self, mtbf: float, repair: float):
        env, rng = self.env, self.rng
        while True:
            yield env.timeout(rng.expovariate(1.0 / mtbf))
            self.counters["breakdowns"] += 1
            # the machine fails under one of the packers: its order must start over
            busy = [r for r in self.packers.users if r.owner is not None]
            if busy:
                victim = busy[rng.randrange(len(busy))]
                self.packers.users.remove(victim)
                victim.owner.interrupt(("breakdown", self.packers.name))
            self.packers.resize(self.packers.capacity - 1)
            yield env.timeout(rng.expovariate(1.0 / repair))
            self.packers.resize(self.packers.capacity + 1)


def run_once(seed: int, base_per_hour: float, express_share: float) -> dict:
    import random

    rng = random.Random(seed)
    env = Environment()
    centre = Centre(env, rng, express_share)
    horizon = SIM_DAYS * 1_440.0
    rate = make_arrival_rate(base_per_hour, HOURLY_PROFILE, weekend_factor=0.7)
    env.process(centre.source(rate, horizon), "source")
    env.process(centre.trucks(45.0, 160), "trucks")
    env.process(centre.supplier(90.0), "supplier")
    env.process(centre.breakdowns(mtbf=300.0, repair=25.0), "breakdowns")
    env.run(until=horizon + 600.0)
    end = env.now
    return {
        "steps": env.steps,
        "counters": centre.counters,
        "stats": {k: s.summary() for k, s in centre.stats.items()},
        "queues": {r.name: [round(r.monitor.average(end), 6), r.monitor.peak]
                   for r in (centre.pickers, centre.packers)},
        "trucks": [len(centre.shipped_by_truck), sum(centre.shipped_by_truck),
                   max(centre.shipped_by_truck, default=0)],
        "stockouts": centre.stock.stockouts,
        "in_system": len(centre.packing_now),
    }


def setup():
    # three what-if scenarios of the planning study: today, peak season, more express
    return {"scenarios": [(SEED, 150.0, 0.08), (SEED + 1, 190.0, 0.08), (SEED + 2, 150.0, 0.2)]}


def run(state):
    return [run_once(*scenario) for scenario in state["scenarios"]]


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
