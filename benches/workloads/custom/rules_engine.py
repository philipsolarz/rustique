"""Loan underwriting: a rules engine and a workflow over typed protocols, ABCs and decorators.

Stands for the decision services banks, insurers, marketplaces and compliance teams run
(underwriting, fraud screening, eligibility, pricing, KYC policy): business rules
registered with decorators into a registry, ``typing.Protocol`` interfaces checked with
``@runtime_checkable`` ``isinstance`` when plugins hand objects back, a condition DSL
built on an ``abc.ABC`` hierarchy with operator overloading (``&``, ``|``, ``~``) and
``operator.attrgetter`` field paths, facts derived lazily with ``functools.cached_property``,
reference lookups behind ``functools.lru_cache``, rule factories via
``functools.partial``, an audit trail written through ``contextlib.contextmanager``
sections, ``contextlib.suppress`` and ``ExitStack``, ``Enum``/``IntFlag`` decisions and
risk flags, frozen dataclass outcomes, and a multi-step workflow whose steps satisfy a
protocol and move the case through a state machine.

Data shape: 12,000 loan applications (applicant, 30% with co-applicant, employment,
debts, collateral) through 5 workflow steps and 24 rules; ~264k rule evaluations.
"""

import abc
import bisect
import contextlib
import enum
import functools
import operator
from collections import Counter, defaultdict
from collections.abc import Mapping
from dataclasses import dataclass, field
from datetime import date
from typing import Protocol, runtime_checkable

REQUIRES: list[str] = []

N_APPLICATIONS = 12_000
TODAY = date(2026, 11, 2)


# --------------------------------------------------------------------------------------
# domain
# --------------------------------------------------------------------------------------


class Decision(enum.Enum):
    APPROVE = "approve"
    REFER = "refer"
    DECLINE = "decline"


class Risk(enum.IntFlag):
    NONE = 0
    THIN_FILE = enum.auto()
    HIGH_DTI = enum.auto()
    RECENT_DEFAULT = enum.auto()
    SELF_EMPLOYED = enum.auto()
    HIGH_LTV = enum.auto()
    PEP = enum.auto()
    VELOCITY = enum.auto()
    YOUNG = enum.auto()


class Stage(enum.Enum):
    RECEIVED = "received"
    VALIDATED = "validated"
    KYC_DONE = "kyc_done"
    SCORED = "scored"
    UNDERWRITTEN = "underwritten"
    PRICED = "priced"
    DECIDED = "decided"
    REJECTED = "rejected"


TRANSITIONS = {
    Stage.RECEIVED: {Stage.VALIDATED, Stage.REJECTED},
    Stage.VALIDATED: {Stage.KYC_DONE, Stage.REJECTED},
    Stage.KYC_DONE: {Stage.SCORED, Stage.REJECTED},
    Stage.SCORED: {Stage.UNDERWRITTEN, Stage.REJECTED},
    Stage.UNDERWRITTEN: {Stage.PRICED, Stage.DECIDED},
    Stage.PRICED: {Stage.DECIDED},
}


@dataclass(slots=True)
class Debt:
    kind: str
    balance: int
    monthly: int
    days_past_due: int = 0


@dataclass
class Applicant:
    name: str
    birth: date
    income_yearly: int
    employment: str
    employed_since: date
    country: str
    postcode: str
    debts: list[Debt] = field(default_factory=list)
    credit_lines: int = 0
    defaults_5y: int = 0
    pep: bool = False

    @functools.cached_property
    def age(self) -> int:
        years = TODAY.year - self.birth.year
        return years - ((TODAY.month, TODAY.day) < (self.birth.month, self.birth.day))

    @functools.cached_property
    def monthly_income(self) -> float:
        return self.income_yearly / 12

    @functools.cached_property
    def monthly_debt(self) -> int:
        return sum(d.monthly for d in self.debts)

    @functools.cached_property
    def years_employed(self) -> float:
        return (TODAY - self.employed_since).days / 365.25

    @functools.cached_property
    def worst_delinquency(self) -> int:
        return max((d.days_past_due for d in self.debts), default=0)


@dataclass
class Application:
    id: str
    applicant: Applicant
    co_applicant: Applicant | None
    amount: int
    term_months: int
    purpose: str
    collateral_value: int
    channel: str
    attributes: dict = field(default_factory=dict)

    @functools.cached_property
    def household_income(self) -> float:
        co = self.co_applicant.monthly_income if self.co_applicant else 0.0
        return self.applicant.monthly_income + co

    @functools.cached_property
    def household_debt(self) -> int:
        co = self.co_applicant.monthly_debt if self.co_applicant else 0
        return self.applicant.monthly_debt + co

    @functools.cached_property
    def instalment(self) -> float:
        r = 0.065 / 12
        n = self.term_months
        return self.amount * r / (1 - (1 + r) ** -n)

    @functools.cached_property
    def dti(self) -> float:
        income = self.household_income
        return (self.household_debt + self.instalment) / income if income else 99.0

    @functools.cached_property
    def ltv(self) -> float:
        return self.amount / self.collateral_value if self.collateral_value else 99.0


@dataclass(frozen=True, slots=True)
class Outcome:
    rule: str
    passed: bool
    score: int = 0
    flags: Risk = Risk.NONE
    reason: str = ""
    hard: bool = False


# --------------------------------------------------------------------------------------
# protocols
# --------------------------------------------------------------------------------------


@runtime_checkable
class Rule(Protocol):
    name: str
    priority: int

    def evaluate(self, app: Application) -> Outcome: ...


@runtime_checkable
class Step(Protocol):
    name: str

    def run(self, case: "Case") -> Stage: ...


@runtime_checkable
class HasScore(Protocol):
    def score(self) -> int: ...


@runtime_checkable
class Explains(Protocol):
    def explain(self) -> str: ...


# --------------------------------------------------------------------------------------
# condition DSL
# --------------------------------------------------------------------------------------


class Condition(abc.ABC):
    @abc.abstractmethod
    def test(self, obj) -> bool: ...

    @abc.abstractmethod
    def describe(self) -> str: ...

    def __and__(self, other: "Condition") -> "Condition":
        return AllOf(self, other)

    def __or__(self, other: "Condition") -> "Condition":
        return AnyOf(self, other)

    def __invert__(self) -> "Condition":
        return Not(self)

    def __call__(self, obj) -> bool:
        return self.test(obj)


class Field:
    """``Field("applicant.age") > 25`` builds a comparison condition."""

    __slots__ = ("path", "get")

    def __init__(self, path: str) -> None:
        self.path = path
        self.get = operator.attrgetter(path)

    def _cmp(self, op, sym: str, value) -> "Compare":
        return Compare(self, op, sym, value)

    def __gt__(self, value):
        return self._cmp(operator.gt, ">", value)

    def __ge__(self, value):
        return self._cmp(operator.ge, ">=", value)

    def __lt__(self, value):
        return self._cmp(operator.lt, "<", value)

    def __le__(self, value):
        return self._cmp(operator.le, "<=", value)

    def __eq__(self, value):  # type: ignore[override]
        return self._cmp(operator.eq, "==", value)

    def __ne__(self, value):  # type: ignore[override]
        return self._cmp(operator.ne, "!=", value)

    __hash__ = None

    def isin(self, values) -> "Condition":
        return InSet(self, frozenset(values))

    def between(self, lo, hi) -> "Condition":
        return (self >= lo) & (self <= hi)


class Compare(Condition):
    def __init__(self, fld: Field, op, sym: str, value) -> None:
        self.field, self.op, self.sym, self.value = fld, op, sym, value

    def test(self, obj) -> bool:
        return self.op(self.field.get(obj), self.value)

    def describe(self) -> str:
        return f"{self.field.path} {self.sym} {self.value!r}"


class InSet(Condition):
    def __init__(self, fld: Field, values: frozenset) -> None:
        self.field, self.values = fld, values

    def test(self, obj) -> bool:
        return self.field.get(obj) in self.values

    def describe(self) -> str:
        return f"{self.field.path} in {sorted(self.values)!r}"


class AllOf(Condition):
    def __init__(self, *parts: Condition) -> None:
        self.parts = parts

    def test(self, obj) -> bool:
        return all(p.test(obj) for p in self.parts)

    def describe(self) -> str:
        return "(" + " and ".join(p.describe() for p in self.parts) + ")"


class AnyOf(Condition):
    def __init__(self, *parts: Condition) -> None:
        self.parts = parts

    def test(self, obj) -> bool:
        return any(p.test(obj) for p in self.parts)

    def describe(self) -> str:
        return "(" + " or ".join(p.describe() for p in self.parts) + ")"


class Not(Condition):
    def __init__(self, inner: Condition) -> None:
        self.inner = inner

    def test(self, obj) -> bool:
        return not self.inner.test(obj)

    def describe(self) -> str:
        return f"not {self.inner.describe()}"


class Predicate(Condition):
    def __init__(self, fn, label: str) -> None:
        self.fn, self.label = fn, label

    def test(self, obj) -> bool:
        return bool(self.fn(obj))

    def describe(self) -> str:
        return self.label


# --------------------------------------------------------------------------------------
# rules and registry
# --------------------------------------------------------------------------------------


class RuleRegistry:
    def __init__(self) -> None:
        self._rules: dict[str, Rule] = {}
        self._by_tag: defaultdict[str, list[str]] = defaultdict(list)

    def add(self, rule, tags: tuple[str, ...]) -> None:
        if not isinstance(rule, Rule):
            raise TypeError(f"{rule!r} does not implement the Rule protocol")
        if rule.name in self._rules:
            raise ValueError(f"duplicate rule {rule.name}")
        self._rules[rule.name] = rule
        for tag in tags:
            self._by_tag[tag].append(rule.name)

    def rule(self, name: str, *, priority: int = 50, tags: tuple[str, ...] = ()):
        """Decorator: register a plain function ``fn(app) -> Outcome | bool`` as a rule."""
        def decorate(fn):
            self.add(FunctionRule(name, priority, fn), tags)
            return fn
        return decorate

    def for_tag(self, tag: str) -> list[Rule]:
        rules = [self._rules[n] for n in self._by_tag.get(tag, ())]
        return sorted(rules, key=lambda r: (r.priority, r.name))

    def __len__(self) -> int:
        return len(self._rules)


class FunctionRule:
    def __init__(self, name: str, priority: int, fn) -> None:
        self.name = name
        self.priority = priority
        self.fn = fn
        functools.update_wrapper(self, fn)

    def evaluate(self, app: Application) -> Outcome:
        result = self.fn(app)
        if isinstance(result, Outcome):
            return result
        return Outcome(self.name, bool(result))


class ConditionRule:
    """A declarative rule: if ``condition`` fails, apply flags / score / hard decline."""

    def __init__(self, name: str, condition: Condition, *, priority: int = 50, score: int = 0,
                 flags: Risk = Risk.NONE, hard: bool = False) -> None:
        self.name = name
        self.priority = priority
        self.condition = condition
        self._score = score
        self.flags = flags
        self.hard = hard

    def evaluate(self, app: Application) -> Outcome:
        ok = self.condition.test(app)
        if ok:
            return Outcome(self.name, True)
        return Outcome(self.name, False, self._score, self.flags, self.condition.describe(),
                       self.hard)

    def explain(self) -> str:
        return f"{self.name}: {self.condition.describe()}"


@functools.lru_cache(maxsize=512)
def postcode_risk(country: str, postcode: str) -> int:
    digits = sum(ord(c) for c in postcode) % 97
    base = {"SE": 2, "DE": 3, "FR": 4, "NL": 2, "ES": 6, "IT": 7}.get(country, 9)
    return base + digits % 5


RATE_BANDS = [480, 560, 620, 680, 740, 800]
RATE_TABLE = {12: [0.149, 0.119, 0.094, 0.071, 0.058, 0.049, 0.043],
              36: [0.159, 0.126, 0.099, 0.076, 0.061, 0.052, 0.046],
              60: [0.169, 0.134, 0.104, 0.081, 0.065, 0.055, 0.049],
              120: [0.179, 0.141, 0.111, 0.086, 0.069, 0.059, 0.052]}


@functools.lru_cache(maxsize=None)
def base_rate(score: int, term: int) -> float:
    terms = sorted(RATE_TABLE)
    t = terms[min(bisect.bisect_left(terms, term), len(terms) - 1)]
    return RATE_TABLE[t][bisect.bisect_right(RATE_BANDS, score)]


def build_registry() -> RuleRegistry:
    reg = RuleRegistry()
    F = Field

    # -- validation ----------------------------------------------------------------------
    for name, cond in [
        ("amount_range", F("amount").between(1_000, 750_000)),
        ("term_allowed", F("term_months").isin({12, 24, 36, 48, 60, 84, 120})),
        ("adult", F("applicant.age") >= 18),
        ("income_present", F("applicant.income_yearly") > 0),
        ("known_purpose", F("purpose").isin({"car", "home", "renovation", "education",
                                             "consolidation", "business", "travel"})),
    ]:
        reg.add(ConditionRule(name, cond, priority=10, hard=True), ("validate",))

    # -- KYC -----------------------------------------------------------------------------
    reg.add(ConditionRule("sanctioned_country", ~F("applicant.country").isin({"XX", "YY"}),
                          priority=5, hard=True), ("kyc",))
    reg.add(ConditionRule("pep", F("applicant.pep") == False, priority=20,  # noqa: E712
                          flags=Risk.PEP, score=-20), ("kyc",))

    @reg.rule("velocity", priority=30, tags=("kyc",))
    def velocity(app: Application) -> Outcome:
        n = app.attributes.get("applications_90d", 0)
        if n > 3:
            return Outcome("velocity", False, -15 * (n - 3), Risk.VELOCITY, f"{n} in 90 days")
        return Outcome("velocity", True)

    @reg.rule("address_risk", priority=40, tags=("kyc",))
    def address_risk(app: Application) -> Outcome:
        risk = postcode_risk(app.applicant.country, app.applicant.postcode)
        return Outcome("address_risk", risk < 9, -risk, reason=f"postcode risk {risk}")

    # -- scoring (each returns a score contribution) --------------------------------------
    scoring = [
        ("age_band", F("applicant.age") >= 25, -25, Risk.YOUNG),
        ("thin_file", F("applicant.credit_lines") >= 2, -30, Risk.THIN_FILE),
        ("no_defaults", F("applicant.defaults_5y") == 0, -90, Risk.RECENT_DEFAULT),
        ("tenure", (F("applicant.years_employed") >= 2) | F("applicant.employment").isin(
            {"public", "retired"}), -20, Risk.NONE),
        ("self_employed", F("applicant.employment") != "self", -15, Risk.SELF_EMPLOYED),
        ("delinquency", F("applicant.worst_delinquency") < 30, -60, Risk.RECENT_DEFAULT),
        ("dti_soft", F("dti") < 0.35, -25, Risk.HIGH_DTI),
        ("dti_hard", F("dti") < 0.5, -60, Risk.HIGH_DTI),
        ("income_level", F("household_income") >= 2_500, -20, Risk.NONE),
    ]
    for name, cond, score, flags in scoring:
        reg.add(ConditionRule(name, cond, priority=50, score=score, flags=flags), ("score",))

    def debt_mix(app: Application, max_cards: int) -> bool:
        return sum(1 for d in app.applicant.debts if d.kind == "card") <= max_cards

    reg.add(ConditionRule("card_mix", Predicate(functools.partial(debt_mix, max_cards=3),
                                                "at most 3 card debts"),
                          priority=55, score=-15), ("score",))

    @reg.rule("co_applicant_bonus", priority=60, tags=("score",))
    def co_applicant_bonus(app: Application) -> Outcome:
        co = app.co_applicant
        if co is not None and co.defaults_5y == 0 and co.income_yearly > 30_000:
            return Outcome("co_applicant_bonus", True, 25)
        return Outcome("co_applicant_bonus", True)

    # -- underwriting --------------------------------------------------------------------
    collateralised = F("purpose").isin({"home", "car", "renovation"})
    reg.add(ConditionRule("ltv", ~collateralised | (F("ltv") <= 0.9), priority=10,
                          flags=Risk.HIGH_LTV, score=-40), ("underwrite",))
    reg.add(ConditionRule("unsecured_cap", collateralised | (F("amount") <= 60_000), priority=20,
                          hard=True), ("underwrite",))
    reg.add(ConditionRule("affordability",
                          Predicate(lambda a: a.household_income - a.household_debt
                                    - a.instalment > 600, "residual income > 600"),
                          priority=30, hard=True), ("underwrite",))

    @reg.rule("term_vs_age", priority=40, tags=("underwrite",))
    def term_vs_age(app: Application) -> bool:
        return app.applicant.age + app.term_months / 12 <= 80

    return reg


# --------------------------------------------------------------------------------------
# audit and workflow
# --------------------------------------------------------------------------------------


@dataclass(slots=True)
class AuditEntry:
    section: str
    depth: int
    events: int


class AuditLog:
    def __init__(self) -> None:
        self.entries: list[AuditEntry] = []
        self._stack: list[str] = []
        self.counts: Counter[str] = Counter()

    @contextlib.contextmanager
    def section(self, name: str):
        self._stack.append(name)
        start = len(self.entries)
        try:
            yield self
        except Exception:
            self.counts[f"error:{name}"] += 1
            raise
        finally:
            self._stack.pop()
            self.entries.append(AuditEntry(name, len(self._stack), len(self.entries) - start))

    def note(self, key: str) -> None:
        self.counts[key] += 1


class HardDecline(Exception):
    def __init__(self, outcome: Outcome) -> None:
        super().__init__(outcome.rule)
        self.outcome = outcome


@dataclass
class Case:
    app: Application
    stage: Stage = Stage.RECEIVED
    score: int = 700
    flags: Risk = Risk.NONE
    outcomes: list[Outcome] = field(default_factory=list)
    rate: float | None = None
    decision: Decision | None = None
    reasons: list[str] = field(default_factory=list)

    def move(self, new: Stage) -> None:
        if new not in TRANSITIONS.get(self.stage, ()):
            raise RuntimeError(f"{self.app.id}: illegal {self.stage} -> {new}")
        self.stage = new


class RuleStep:
    """A workflow step that evaluates every rule of one tag."""

    def __init__(self, name: str, registry: RuleRegistry, tag: str, success: Stage,
                 audit: AuditLog) -> None:
        self.name = name
        self.rules = registry.for_tag(tag)
        self.success = success
        self.audit = audit

    def run(self, case: Case) -> Stage:
        for rule in self.rules:
            with self.audit.section(rule.name):
                outcome = rule.evaluate(case.app)
            case.outcomes.append(outcome)
            if outcome.passed:
                if outcome.score:
                    case.score += outcome.score
                continue
            case.score += outcome.score
            case.flags |= outcome.flags
            if outcome.reason:
                case.reasons.append(outcome.reason)
            if outcome.hard:
                raise HardDecline(outcome)
        return self.success


class ScoreSummary:
    def __init__(self, case: Case) -> None:
        self.case = case

    def score(self) -> int:
        return max(300, min(850, self.case.score))

    def explain(self) -> str:
        return f"{self.case.app.id}: {self.score()} {self.case.flags!r}"


class PricingStep:
    name = "pricing"

    def __init__(self, audit: AuditLog) -> None:
        self.audit = audit

    def run(self, case: Case) -> Stage:
        summary = ScoreSummary(case)
        if not isinstance(summary, HasScore):
            raise TypeError("pricing needs a scored case")
        rate = base_rate(summary.score(), case.app.term_months)
        risk_loading = 0.0
        for flag, loading in ((Risk.HIGH_DTI, 0.01), (Risk.SELF_EMPLOYED, 0.005),
                              (Risk.HIGH_LTV, 0.0075), (Risk.THIN_FILE, 0.004)):
            if flag in case.flags:
                risk_loading += loading
        with contextlib.suppress(KeyError):
            risk_loading -= {"branch": 0.0, "broker": -0.002, "online": 0.003}[case.app.channel]
        case.rate = round(rate + risk_loading, 5)
        return Stage.DECIDED


class Workflow:
    def __init__(self, steps: list, audit: AuditLog) -> None:
        for step in steps:
            if not isinstance(step, Step):
                raise TypeError(f"{step!r} is not a workflow step")
        self.steps = steps
        self.audit = audit

    def process(self, app: Application) -> Case:
        case = Case(app)
        with contextlib.ExitStack() as stack:
            stack.enter_context(self.audit.section(f"case:{app.purpose}"))
            stack.callback(self.audit.note, "closed")
            try:
                for step in self.steps:
                    if case.stage is Stage.UNDERWRITTEN and case.score < 520:
                        break  # not worth pricing: straight to a decision
                    with self.audit.section(step.name):
                        nxt = step.run(case)
                    case.move(nxt)
            except HardDecline as hd:
                case.move(Stage.REJECTED)
                case.decision = Decision.DECLINE
                case.reasons.append(f"hard:{hd.outcome.rule}")
                return case
        summary = ScoreSummary(case)
        score = summary.score() if isinstance(summary, HasScore) else 0
        if score >= 640 and not (case.flags & (Risk.RECENT_DEFAULT | Risk.PEP)):
            case.decision = Decision.APPROVE
        elif score >= 520:
            case.decision = Decision.REFER
        else:
            case.decision = Decision.DECLINE
        if case.stage is not Stage.DECIDED:
            case.move(Stage.DECIDED)
        return case


class ValidateStep:
    name = "validate"

    def __init__(self, inner: RuleStep) -> None:
        self.inner = inner

    def run(self, case: Case) -> Stage:
        attrs = case.app.attributes
        if not isinstance(attrs, Mapping):
            raise TypeError("attributes must be a mapping")
        return self.inner.run(case)


# --------------------------------------------------------------------------------------
# workload
# --------------------------------------------------------------------------------------


def _person(rng, i: int) -> dict:
    debts = [{"kind": rng.choice(["card", "card", "car", "student", "mortgage", "personal"]),
              "balance": rng.randint(200, 250_000), "monthly": rng.randint(20, 700),
              "dpd": rng.choice([0] * 12 + [15, 30, 60, 90])}
             for _ in range(rng.choice([0, 1, 1, 2, 3, 4, 6]))]
    return {
        "name": f"Person {i}", "birth": (rng.randint(1940, 2009), rng.randint(1, 12),
                                         rng.randint(1, 28)),
        "income": int(rng.lognormvariate(10.7, 0.5)) if rng.random() > 0.01 else 0,
        "employment": rng.choice(["private", "private", "private", "public", "self", "retired",
                                  "student"]),
        "since": (rng.randint(1985, 2026), rng.randint(1, 12), 1),
        "country": rng.choice(["SE", "DE", "FR", "NL", "ES", "IT", "PL", "XX"] if
                              rng.random() < 0.01 else ["SE", "DE", "FR", "NL", "ES", "IT"]),
        "postcode": f"{rng.randint(100, 499) * 100 + rng.choice([11, 22, 33]):05d}",
        "debts": debts,
        "lines": rng.choice([0, 1, 2, 3, 4, 6, 9]), "defaults": rng.choice([0] * 14 + [1, 2]),
        "pep": rng.random() < 0.01,
    }


def setup():
    import random

    rng = random.Random(8086)
    apps = []
    for i in range(N_APPLICATIONS):
        applicant = _person(rng, i)
        purpose = rng.choice(["car", "home", "home", "renovation", "education", "consolidation",
                              "business", "travel"] if rng.random() > 0.02 else ["crypto"])
        multiple = {"home": 4.0, "business": 1.5, "car": 0.6}.get(purpose, 0.3)
        amount = int(max(applicant["income"], 15_000) * multiple * rng.uniform(0.2, 1.3))
        if rng.random() < 0.02:
            amount = rng.choice([500, 2_000_000])
        term = 120 if amount > 150_000 else rng.choice([24, 36, 48, 60, 84]) \
            if amount > 20_000 else rng.choice([12, 24, 36])
        apps.append({
            "id": f"L{i:06d}", "applicant": applicant,
            "co": _person(rng, i + 1_000_000) if rng.random() < 0.3 else None,
            "amount": amount, "term": term if rng.random() > 0.02 else 72,
            "purpose": purpose,
            "collateral": int(amount * rng.uniform(0.9, 2.5)) if purpose in
            ("home", "car", "renovation") and rng.random() < 0.9 else 0,
            "channel": rng.choice(["branch", "broker", "online", "online", "partner"]),
            "applications_90d": rng.choice([0, 0, 0, 1, 1, 2, 4, 6]),
        })
    return {"apps": apps}


def make_applicant(p: dict) -> Applicant:
    return Applicant(
        p["name"], date(*p["birth"]), p["income"], p["employment"], date(*p["since"]),
        p["country"], p["postcode"],
        [Debt(d["kind"], d["balance"], d["monthly"], d["dpd"]) for d in p["debts"]],
        p["lines"], p["defaults"], p["pep"])


def run(state):
    postcode_risk.cache_clear()
    base_rate.cache_clear()
    audit = AuditLog()
    registry = build_registry()
    workflow = Workflow([
        ValidateStep(RuleStep("validate", registry, "validate", Stage.VALIDATED, audit)),
        RuleStep("kyc", registry, "kyc", Stage.KYC_DONE, audit),
        RuleStep("score", registry, "score", Stage.SCORED, audit),
        RuleStep("underwrite", registry, "underwrite", Stage.UNDERWRITTEN, audit),
        PricingStep(audit),
    ], audit)

    decisions: Counter[str] = Counter()
    reasons: Counter[str] = Counter()
    flags: Counter[str] = Counter()
    rates: defaultdict[str, list[float]] = defaultdict(list)
    explained = []
    for raw in state["apps"]:
        app = Application(raw["id"], make_applicant(raw["applicant"]),
                          make_applicant(raw["co"]) if raw["co"] else None, raw["amount"],
                          raw["term"], raw["purpose"], raw["collateral"], raw["channel"],
                          {"applications_90d": raw["applications_90d"]})
        case = workflow.process(app)
        decisions[case.decision.value] += 1
        for r in case.reasons:
            reasons[r.split(" ")[0]] += 1
        for f in Risk:
            if f and f in case.flags:
                flags[f.name] += 1
        if case.rate is not None:
            rates[case.decision.value].append(case.rate)
        summary = ScoreSummary(case)
        if len(explained) < 40 and isinstance(summary, Explains) and case.decision is Decision.REFER:
            explained.append(summary.explain())

    explanations = sorted(r.explain() for r in registry.for_tag("underwrite")
                          if isinstance(r, Explains))
    return {
        "rules": len(registry),
        "decisions": dict(sorted(decisions.items())),
        "reasons": sorted(reasons.items(), key=lambda kv: (-kv[1], kv[0]))[:30],
        "flags": dict(sorted(flags.items())),
        "rates": {k: [len(v), round(sum(v) / len(v), 6), min(v), max(v)]
                  for k, v in sorted(rates.items())},
        "audit": [len(audit.entries), dict(sorted(audit.counts.items()))],
        "deepest": max(e.depth for e in audit.entries),
        "explained": explained,
        "explanations": explanations,
        "cache": [postcode_risk.cache_info().hits, base_rate.cache_info().misses],
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
