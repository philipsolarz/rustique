"""A dependency-injection container and a pluggy-style plugin system serving requests.

Stands for the application "kernel" of service backends, CLIs and internal platforms (and
the core of ``injector``/``punq``/``lagom``/``dependency-injector``, FastAPI's
``Depends``, pytest's fixture and ``pluggy`` hook machinery): services declared against
``abc.ABC`` interfaces, constructor autowiring through ``inspect.signature`` and
``typing.get_type_hints`` (``Optional``/``X | None``, ``list[Plugin]`` multi-bindings,
defaults), singleton/scoped/transient lifetimes, request scopes as context managers that
dispose in reverse order, call-site injection (``container.call(fn)`` inspects ``fn``),
plugins auto-registered by ``__init_subclass__``, hook specs whose implementations
receive only the arguments they declare (``tryfirst``/``trylast``/``firstresult``),
decorator-set markers read with ``getattr``, an event bus holding subscribers through
``weakref.WeakMethod`` and per-request caches in a ``weakref.WeakKeyDictionary``.

Run: 50 application boots (each registers 12 services and 6-9 plugins, then serves 480
requests, as a test suite or a multi-tenant host does), 24,000 requests in all, each
resolving a scoped handler graph, injecting a call and firing up to 5 hooks.
"""

import abc
import enum
import functools
import inspect
import types
import typing
import weakref
from collections import Counter, defaultdict
from dataclasses import dataclass, field

REQUIRES: list[str] = []

N_BOOTS = 50
REQUESTS_PER_BOOT = 480


# --------------------------------------------------------------------------------------
# container
# --------------------------------------------------------------------------------------


class Lifetime(enum.Enum):
    SINGLETON = "singleton"
    SCOPED = "scoped"
    TRANSIENT = "transient"


class ResolutionError(LookupError):
    pass


_EMPTY = inspect.Parameter.empty


@dataclass(frozen=True, slots=True)
class Dependency:
    name: str
    annotation: object
    optional: bool
    has_default: bool
    many: bool


@dataclass(slots=True)
class Registration:
    interface: type
    factory: typing.Callable
    lifetime: Lifetime
    deps: tuple[Dependency, ...] = ()
    disposer: typing.Callable | None = None


def analyze(factory) -> tuple[Dependency, ...]:
    """Read the constructor's (or function's) parameters and resolved annotations."""
    target = factory.__init__ if isinstance(factory, type) else factory
    sig = inspect.signature(target)
    hints = typing.get_type_hints(target)
    deps = []
    for name, param in sig.parameters.items():
        if name == "self" or param.kind in (param.VAR_POSITIONAL, param.VAR_KEYWORD):
            continue
        ann = hints.get(name, param.annotation)
        optional = False
        many = False
        origin = typing.get_origin(ann)
        if origin in (typing.Union, types.UnionType):
            args = [a for a in typing.get_args(ann) if a is not type(None)]
            if len(args) == 1:
                ann, optional = args[0], True
        elif origin is list:
            (ann,) = typing.get_args(ann)
            many = True
        deps.append(Dependency(name, ann, optional, param.default is not _EMPTY, many))
    return tuple(deps)


class Scope:
    def __init__(self, container: "Container", name: str) -> None:
        self.container = container
        self.name = name
        self.instances: dict[type, object] = {}
        self._disposers: list = []
        self.closed = False

    def remember(self, reg: Registration, instance) -> None:
        self.instances[reg.interface] = instance
        if reg.disposer is not None:
            self._disposers.append(functools.partial(reg.disposer, instance))

    def __enter__(self) -> "Scope":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        while self._disposers:
            self._disposers.pop()()
        self.closed = True
        self.instances.clear()

    def resolve(self, interface):
        return self.container.resolve(interface, self)


class Container:
    def __init__(self) -> None:
        self._registrations: dict[type, Registration] = {}
        self._multi: defaultdict[type, list[Registration]] = defaultdict(list)
        self._singletons: dict[type, object] = {}
        self._call_cache: dict = {}
        self.stats: Counter[str] = Counter()

    # -- registration --------------------------------------------------------------------
    def register(self, interface: type, implementation=None, *,
                 lifetime: Lifetime = Lifetime.TRANSIENT, factory=None, disposer=None,
                 multi: bool = False) -> None:
        impl = factory or implementation or interface
        if implementation is not None and isinstance(interface, type) and \
                isinstance(implementation, type) and not issubclass(implementation, interface):
            raise TypeError(f"{implementation.__name__} does not implement {interface.__name__}")
        if isinstance(impl, type) and inspect.isabstract(impl):
            raise TypeError(f"{impl.__name__} is abstract")
        reg = Registration(interface, impl, lifetime, analyze(impl), disposer)
        self.stats["analyzed"] += 1
        if multi:
            self._multi[interface].append(reg)
        else:
            self._registrations[interface] = reg

    def instance(self, interface: type, value, *, multi: bool = False) -> None:
        reg = Registration(interface, lambda: value, Lifetime.TRANSIENT)
        if multi:
            self._multi[interface].append(reg)
        else:
            self._registrations[interface] = reg

    def scope(self, name: str = "request") -> Scope:
        return Scope(self, name)

    # -- resolution ----------------------------------------------------------------------
    def resolve(self, interface, scope: Scope | None = None, _chain: tuple = ()):
        if interface in _chain:
            raise ResolutionError(" -> ".join(t.__name__ for t in _chain + (interface,)))
        reg = self._registrations.get(interface)
        if reg is None:
            if isinstance(interface, type) and not inspect.isabstract(interface) and \
                    interface.__module__ == __name__:
                # autowire concrete classes of our own application
                self.register(interface)
                reg = self._registrations[interface]
            else:
                raise ResolutionError(f"nothing registered for {interface!r}")
        return self._build(reg, scope, _chain + (interface,))

    def resolve_all(self, interface, scope: Scope | None, chain: tuple) -> list:
        return [self._build(reg, scope, chain) for reg in self._multi.get(interface, ())]

    def _build(self, reg: Registration, scope: Scope | None, chain: tuple):
        if reg.lifetime is Lifetime.SINGLETON:
            inst = self._singletons.get(reg.interface)
            if inst is not None:
                self.stats["singleton_hit"] += 1
                return inst
        elif reg.lifetime is Lifetime.SCOPED:
            if scope is None:
                raise ResolutionError(f"{reg.interface.__name__} needs a scope")
            inst = scope.instances.get(reg.interface)
            if inst is not None:
                self.stats["scoped_hit"] += 1
                return inst
        kwargs = self._arguments(reg.deps, scope, chain)
        inst = reg.factory(**kwargs)
        self.stats[f"built:{reg.lifetime.value}"] += 1
        if reg.lifetime is Lifetime.SINGLETON:
            self._singletons[reg.interface] = inst
        elif reg.lifetime is Lifetime.SCOPED:
            scope.remember(reg, inst)
        return inst

    def _arguments(self, deps, scope, chain, overrides=None) -> dict:
        kwargs = {}
        for dep in deps:
            if overrides and dep.name in overrides:
                kwargs[dep.name] = overrides[dep.name]
                continue
            if dep.many:
                kwargs[dep.name] = self.resolve_all(dep.annotation, scope, chain)
                continue
            try:
                kwargs[dep.name] = self.resolve(dep.annotation, scope, chain)
            except ResolutionError:
                if dep.has_default:
                    continue
                if dep.optional:
                    kwargs[dep.name] = None
                    continue
                raise
        return kwargs

    def call(self, fn, scope: Scope | None = None, **overrides):
        """Call ``fn`` injecting whatever its signature asks for."""
        deps = analyze(fn)
        self.stats["call_analyzed"] += 1
        return fn(**self._arguments(deps, scope, (), overrides))


# --------------------------------------------------------------------------------------
# plugins and hooks
# --------------------------------------------------------------------------------------


def hookimpl(fn=None, *, tryfirst: bool = False, trylast: bool = False):
    def mark(f):
        f.hook_opts = {"tryfirst": tryfirst, "trylast": trylast}
        return f
    return mark(fn) if fn is not None else mark


class HookCaller:
    def __init__(self, name: str, argnames: tuple[str, ...], firstresult: bool) -> None:
        self.name = name
        self.argnames = argnames
        self.firstresult = firstresult
        self._impls: list[tuple[int, object, tuple[str, ...]]] = []

    def add(self, method, opts: dict) -> None:
        params = tuple(p for p in inspect.signature(method).parameters)
        unknown = set(params) - set(self.argnames)
        if unknown:
            raise TypeError(f"{self.name}: unknown arguments {sorted(unknown)}")
        rank = 0 if opts.get("tryfirst") else 2 if opts.get("trylast") else 1
        self._impls.append((rank, method, params))
        self._impls.sort(key=lambda item: item[0])

    def __call__(self, **kwargs):
        results = []
        for _rank, method, params in self._impls:
            res = method(*[kwargs[p] for p in params])
            if res is not None:
                if self.firstresult:
                    return res
                results.append(res)
        return None if self.firstresult else results


class HookRelay:
    pass


class PluginManager:
    def __init__(self, specs: type) -> None:
        self.hook = HookRelay()
        self._specs: dict[str, HookCaller] = {}
        for name, fn in inspect.getmembers(specs, inspect.isfunction):
            argnames = tuple(p for p in inspect.signature(fn).parameters if p != "self")
            caller = HookCaller(name, argnames, getattr(fn, "firstresult", False))
            self._specs[name] = caller
            setattr(self.hook, name, caller)
        self.plugins: list[object] = []

    def register(self, plugin: object) -> None:
        for name in dir(plugin):
            if name.startswith("_"):
                continue
            method = getattr(plugin, name)
            opts = getattr(method, "hook_opts", None)
            if opts is None:
                continue
            if name not in self._specs:
                raise TypeError(f"{type(plugin).__name__}.{name} implements no known hook")
            self._specs[name].add(method, opts)
        self.plugins.append(plugin)


def firstresult(fn):
    fn.firstresult = True
    return fn


class HookSpecs:
    def configure(self, settings): ...

    def before_request(self, request, user): ...

    def validate(self, request, errors): ...

    @firstresult
    def route(self, request): ...

    def transform(self, request, response): ...

    def after_request(self, request, response, elapsed): ...


PLUGIN_TYPES: dict[str, type] = {}


class Plugin(abc.ABC):
    name: typing.ClassVar[str] = ""
    order: typing.ClassVar[int] = 100

    def __init_subclass__(cls, **kwargs) -> None:
        super().__init_subclass__(**kwargs)
        if not inspect.isabstract(cls):
            PLUGIN_TYPES[cls.name or cls.__name__.lower()] = cls

    @abc.abstractmethod
    def describe(self) -> str: ...


# --------------------------------------------------------------------------------------
# application services (interfaces and implementations)
# --------------------------------------------------------------------------------------


class Clock(abc.ABC):
    @abc.abstractmethod
    def now(self) -> int: ...


class Settings:
    def __init__(self, values: dict) -> None:
        self.values = dict(values)

    def get(self, key: str, default=None):
        return self.values.get(key, default)


class Cache(abc.ABC):
    @abc.abstractmethod
    def get(self, key: str): ...

    @abc.abstractmethod
    def set(self, key: str, value) -> None: ...


class Repository(abc.ABC):
    @abc.abstractmethod
    def find(self, key: int) -> dict | None: ...

    @abc.abstractmethod
    def save(self, key: int, record: dict) -> None: ...


class Notifier(abc.ABC):
    @abc.abstractmethod
    def send(self, to: str, message: str) -> None: ...


class EventBus:
    def __init__(self) -> None:
        self._subs: defaultdict[str, list[weakref.WeakMethod]] = defaultdict(list)
        self.delivered = 0
        self.dropped = 0

    def subscribe(self, topic: str, bound_method) -> None:
        self._subs[topic].append(weakref.WeakMethod(bound_method))

    def publish(self, topic: str, payload) -> None:
        live = []
        for ref in self._subs.get(topic, ()):
            fn = ref()
            if fn is None:
                self.dropped += 1
                continue
            fn(payload)
            self.delivered += 1
            live.append(ref)
        self._subs[topic] = live


class TickClock(Clock):
    def __init__(self) -> None:
        self.t = 1_700_000_000

    def now(self) -> int:
        self.t += 7
        return self.t


class DictCache(Cache):
    def __init__(self, settings: Settings) -> None:
        self.data: dict[str, object] = {}
        self.limit = settings.get("cache_size", 256)
        self.hits = self.misses = 0

    def get(self, key: str):
        value = self.data.get(key)
        if value is None:
            self.misses += 1
        else:
            self.hits += 1
        return value

    def set(self, key: str, value) -> None:
        if len(self.data) >= self.limit:
            self.data.pop(next(iter(self.data)))
        self.data[key] = value


class MemoryRepository(Repository):
    def __init__(self, cache: Cache, clock: Clock) -> None:
        self.cache = cache
        self.clock = clock
        self.rows: dict[int, dict] = {}

    def find(self, key: int) -> dict | None:
        cached = self.cache.get(f"row:{key}")
        if cached is not None:
            return cached
        row = self.rows.get(key)
        if row is not None:
            self.cache.set(f"row:{key}", row)
        return row

    def save(self, key: int, record: dict) -> None:
        record = dict(record, updated=self.clock.now())
        self.rows[key] = record
        self.cache.set(f"row:{key}", record)


class OutboxNotifier(Notifier):
    def __init__(self, bus: EventBus, settings: Settings) -> None:
        self.bus = bus
        self.sender = settings.get("sender", "noreply")
        self.outbox: list[tuple[str, str]] = []

    def send(self, to: str, message: str) -> None:
        self.outbox.append((to, f"[{self.sender}] {message}"))
        self.bus.publish("notified", to)


class Metrics:
    def __init__(self) -> None:
        self.counters: Counter[str] = Counter()
        self.timings: defaultdict[str, list[int]] = defaultdict(list)

    def incr(self, key: str, n: int = 1) -> None:
        self.counters[key] += n

    def observe(self, key: str, value: int) -> None:
        self.timings[key].append(value)


@dataclass
class Request:
    id: int
    path: str
    user_id: int
    body: dict
    headers: dict = field(default_factory=dict)

    __hash__ = object.__hash__  # usable as a WeakKeyDictionary key


class RequestContext:
    def __init__(self, clock: Clock, metrics: Metrics) -> None:
        self.started = clock.now()
        self.metrics = metrics
        self.notes: list[str] = []


class UnitOfWork:
    def __init__(self, repo: Repository, bus: EventBus, ctx: RequestContext) -> None:
        self.repo = repo
        self.bus = bus
        self.ctx = ctx
        self.pending: list[tuple[int, dict]] = []
        self.committed = False

    def stage(self, key: int, record: dict) -> None:
        self.pending.append((key, record))

    def commit(self) -> None:
        for key, record in self.pending:
            self.repo.save(key, record)
            self.bus.publish("saved", key)
        self.ctx.metrics.incr("rows_saved", len(self.pending))
        self.pending.clear()
        self.committed = True

    def close(self) -> None:
        if self.pending:
            self.ctx.metrics.incr("rolled_back", len(self.pending))
            self.pending.clear()


class UserService:
    def __init__(self, repo: Repository, cache: Cache, audit: "AuditTrail | None" = None) -> None:
        self.repo = repo
        self.cache = cache
        self.audit = audit

    def load(self, user_id: int) -> dict:
        user = self.repo.find(10_000_000 + user_id)
        if user is None:
            user = {"id": user_id, "name": f"user{user_id}", "plan": "free" if user_id % 3
                    else "pro", "visits": 0}
        return user


class AuditTrail:
    def __init__(self, clock: Clock, bus: EventBus) -> None:
        self.clock = clock
        self.entries: list[tuple[int, str]] = []
        bus.subscribe("saved", self.on_saved)

    def on_saved(self, key) -> None:
        self.entries.append((self.clock.now(), f"saved {key}"))


class OrderHandler:
    def __init__(self, uow: UnitOfWork, users: UserService, notifier: Notifier,
                 ctx: RequestContext, plugins: list[Plugin], limit: int = 50) -> None:
        self.uow = uow
        self.users = users
        self.notifier = notifier
        self.ctx = ctx
        self.plugins = plugins
        self.limit = limit

    def handle(self, request: Request, user: dict) -> dict:
        items = request.body.get("items", [])
        total = sum(q * p for q, p in items)
        if len(items) > self.limit:
            return {"status": 413}
        self.uow.stage(request.id, {"user": user["id"], "total": total, "n": len(items)})
        user = dict(user, visits=user["visits"] + 1)
        self.uow.stage(10_000_000 + user["id"], user)
        if total > 5_000:
            self.notifier.send(user["name"], f"large order {request.id}: {total}")
        return {"status": 201, "total": total, "plugins": len(self.plugins)}


class ReportHandler:
    def __init__(self, repo: Repository, cache: Cache, ctx: RequestContext) -> None:
        self.repo = repo
        self.cache = cache
        self.ctx = ctx

    def handle(self, request: Request, user: dict) -> dict:
        keys = request.body.get("keys", [])
        found = [self.repo.find(k) for k in keys]
        return {"status": 200, "found": sum(1 for f in found if f), "visits": user["visits"]}


# --------------------------------------------------------------------------------------
# plugins
# --------------------------------------------------------------------------------------


class AuthPlugin(Plugin):
    name = "auth"

    def describe(self) -> str:
        return "rejects anonymous writes"

    @hookimpl(tryfirst=True)
    def validate(self, request, errors):
        if request.user_id == 0 and request.path.startswith("/orders"):
            errors.append("auth: anonymous")


class SchemaPlugin(Plugin):
    name = "schema"

    def describe(self) -> str:
        return "validates bodies"

    @hookimpl
    def validate(self, request, errors):
        if request.path.startswith("/orders"):
            for item in request.body.get("items", ()):
                if not (isinstance(item, (list, tuple)) and len(item) == 2):
                    errors.append("schema: bad item")
                    break
                if item[0] <= 0:
                    errors.append("schema: quantity")
                    break


class RouterPlugin(Plugin):
    name = "router"

    def describe(self) -> str:
        return "maps paths to handlers"

    @hookimpl
    def route(self, request):
        if request.path.startswith("/orders"):
            return OrderHandler
        if request.path.startswith("/reports"):
            return ReportHandler
        return None


class LegacyRouterPlugin(Plugin):
    name = "legacy_router"

    def describe(self) -> str:
        return "old /v1 paths"

    @hookimpl(trylast=True)
    def route(self, request):
        return ReportHandler if request.path.startswith("/v1") else None


class TimingPlugin(Plugin):
    name = "timing"

    def __init__(self) -> None:
        self.slow = 0

    def describe(self) -> str:
        return "records latency"

    @hookimpl
    def after_request(self, response, elapsed):
        if elapsed > 30:
            self.slow += 1


class HeadersPlugin(Plugin):
    name = "headers"

    def describe(self) -> str:
        return "adds response headers"

    @hookimpl
    def transform(self, request, response):
        response.setdefault("headers", {})["x-request-id"] = str(request.id)
        return None


class RedactPlugin(Plugin):
    name = "redact"

    def describe(self) -> str:
        return "removes internals"

    @hookimpl(trylast=True)
    def transform(self, response):
        response.pop("plugins", None)


class QuotaPlugin(Plugin):
    name = "quota"

    def __init__(self) -> None:
        self.used: Counter[int] = Counter()

    def describe(self) -> str:
        return "per-user request quota"

    @hookimpl
    def before_request(self, request, user):
        self.used[request.user_id] += 1

    @hookimpl
    def validate(self, request, errors):
        if self.used[request.user_id] > 40 and user_plan(request.user_id) == "free":
            errors.append("quota: exceeded")


class ConfigPlugin(Plugin):
    name = "config"

    def describe(self) -> str:
        return "reads settings"

    @hookimpl
    def configure(self, settings):
        settings.values.setdefault("cache_size", 128)


def user_plan(user_id: int) -> str:
    return "free" if user_id % 3 else "pro"


# --------------------------------------------------------------------------------------
# the application kernel
# --------------------------------------------------------------------------------------


class Application:
    def __init__(self, settings: dict, plugin_names: list[str]) -> None:
        self.container = c = Container()
        self.pm = PluginManager(HookSpecs)
        cfg = Settings(settings)
        for name in plugin_names:
            plugin = PLUGIN_TYPES[name]()
            self.pm.register(plugin)
            c.instance(Plugin, plugin, multi=True)
        self.pm.hook.configure(settings=cfg)
        c.instance(Settings, cfg)
        c.register(Clock, TickClock, lifetime=Lifetime.SINGLETON)
        c.register(EventBus, lifetime=Lifetime.SINGLETON)
        c.register(Metrics, lifetime=Lifetime.SINGLETON)
        c.register(Cache, DictCache, lifetime=Lifetime.SINGLETON)
        c.register(Repository, MemoryRepository, lifetime=Lifetime.SINGLETON)
        c.register(Notifier, OutboxNotifier, lifetime=Lifetime.SINGLETON)
        c.register(AuditTrail, lifetime=Lifetime.SINGLETON)
        c.register(RequestContext, lifetime=Lifetime.SCOPED)
        c.register(UnitOfWork, lifetime=Lifetime.SCOPED, disposer=UnitOfWork.close)
        c.register(UserService, lifetime=Lifetime.SCOPED)
        c.register(OrderHandler, lifetime=Lifetime.TRANSIENT)
        c.register(ReportHandler, lifetime=Lifetime.TRANSIENT)
        self.audit = c.resolve(AuditTrail)
        self.request_cache: weakref.WeakKeyDictionary = weakref.WeakKeyDictionary()

    def handle(self, request: Request) -> dict:
        c = self.container
        hook = self.pm.hook
        with c.scope() as scope:
            ctx = scope.resolve(RequestContext)
            users = scope.resolve(UserService)
            user = users.load(request.user_id)
            self.request_cache[request] = user
            hook.before_request(request=request, user=user)
            errors: list[str] = []
            hook.validate(request=request, errors=errors)
            if errors:
                ctx.metrics.incr("rejected")
                return {"status": 400, "errors": errors}
            handler_type = hook.route(request=request)
            if handler_type is None:
                return {"status": 404}
            handler = c.resolve(handler_type, scope)
            response = c.call(handler.handle, scope, request=request, user=user)
            uow = scope.resolve(UnitOfWork)
            if response["status"] < 400:
                uow.commit()
            hook.transform(request=request, response=response)
            elapsed = c.resolve(Clock).now() - ctx.started
            hook.after_request(request=request, response=response, elapsed=elapsed)
            ctx.metrics.observe(request.path.split("/")[1], elapsed)
            return response


def setup():
    import random

    rng = random.Random(4242)
    boots = []
    all_plugins = sorted(PLUGIN_TYPES)
    for b in range(N_BOOTS):
        plugins = [p for p in all_plugins if p in ("auth", "router", "schema", "config")
                   or rng.random() < 0.8]
        reqs = []
        for i in range(REQUESTS_PER_BOOT):
            r = rng.random()
            path = "/orders/new" if r < 0.55 else "/reports/daily" if r < 0.85 else \
                "/v1/report" if r < 0.95 else "/unknown"
            items = [[rng.choice([1, 1, 2, 3, -1] if rng.random() < 0.05 else [1, 2, 3]),
                      rng.randint(5, 900)] for _ in range(rng.randint(1, 12))]
            reqs.append((b * 10_000 + i, path, rng.choice([0] + list(range(1, 60))),
                         {"items": items} if path.startswith("/orders") else
                         {"keys": [b * 10_000 + rng.randrange(max(i, 1)) for _ in range(5)]}))
        boots.append({"settings": {"sender": f"tenant{b}", "cache_size": rng.choice([64, 256])},
                      "plugins": plugins, "requests": reqs})
    return {"boots": boots}


def run(state):
    statuses: Counter[int] = Counter()
    stats: Counter[str] = Counter()
    errors: Counter[str] = Counter()
    summary = []
    for boot in state["boots"]:
        app = Application(boot["settings"], boot["plugins"])
        for rid, path, user_id, body in boot["requests"]:
            response = app.handle(Request(rid, path, user_id, body))
            statuses[response["status"]] += 1
            for e in response.get("errors", ()):
                errors[e] += 1
        c = app.container
        stats.update(c.stats)
        metrics = c.resolve(Metrics)
        notifier = c.resolve(Notifier)
        cache = c.resolve(Cache)
        bus = c.resolve(EventBus)
        summary.append([len(boot["plugins"]), metrics.counters["rows_saved"],
                        metrics.counters.get("rejected", 0), len(notifier.outbox),
                        cache.hits, cache.misses, bus.delivered, len(app.audit.entries),
                        len(app.request_cache),
                        {k: sum(v) for k, v in sorted(metrics.timings.items())}])
    described = sorted(f"{name}: {cls().describe()}" for name, cls in PLUGIN_TYPES.items())
    return {"statuses": dict(sorted(statuses.items())), "stats": dict(sorted(stats.items())),
            "errors": dict(sorted(errors.items())), "boots": summary, "plugins": described}


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
