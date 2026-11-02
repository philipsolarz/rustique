"""Event-sourced banking ledger: commands in, events stored, aggregates rehydrated, projections.

Stands for the CQRS / event-sourcing services that payment, ledger, inventory and workflow
teams write in-house (and that libraries like ``eventsourcing`` package): commands and
events as frozen, slotted, keyword-only dataclasses in a class hierarchy; a command handler
built on structural pattern matching (``match`` with class patterns and guards); an
aggregate whose ``apply`` is a ``functools.singledispatchmethod`` and that is rehydrated
from a snapshot (``dataclasses.replace``-style value object) plus the tail of its stream
on every command; an append-only event store with per-stream indexes and optimistic
concurrency (``ConcurrencyError`` + retry); read-model projections registered with
``functools.singledispatch``; velocity fraud checks on ``deque(maxlen=)``; domain errors as
exceptions; and a full replay that rebuilds the projections and checks them against the
live ones.

Data shape: 3,000 accounts, 83k commands (deposits, withdrawals, transfers, fees,
interest, freezes, closes; ~10% rejected), ~96k events, snapshots every 16 versions,
~107k aggregate loads replaying ~710k events.
"""

import enum
import functools
from collections import Counter, defaultdict, deque
from dataclasses import asdict, dataclass, field

REQUIRES: list[str] = []

N_ACCOUNTS = 3_000
N_COMMANDS = 80_000
SNAPSHOT_EVERY = 16


class AccountKind(enum.Enum):
    CHECKING = "checking"
    SAVINGS = "savings"
    CREDIT = "credit"
    MERCHANT = "merchant"


class AccountState(enum.Enum):
    NEW = 0
    ACTIVE = 1
    FROZEN = 2
    CLOSED = 3


# --------------------------------------------------------------------------------------
# commands
# --------------------------------------------------------------------------------------


@dataclass(frozen=True, slots=True, kw_only=True)
class Command:
    command_id: int
    at: int


@dataclass(frozen=True, slots=True, kw_only=True)
class OpenAccount(Command):
    account_id: str
    owner: str
    kind: AccountKind
    currency: str
    overdraft_limit: int = 0


@dataclass(frozen=True, slots=True, kw_only=True)
class Deposit(Command):
    account_id: str
    amount: int
    reference: str


@dataclass(frozen=True, slots=True, kw_only=True)
class Withdraw(Command):
    account_id: str
    amount: int
    channel: str = "atm"


@dataclass(frozen=True, slots=True, kw_only=True)
class Transfer(Command):
    source: str
    target: str
    amount: int
    memo: str = ""


@dataclass(frozen=True, slots=True, kw_only=True)
class ChargeFee(Command):
    account_id: str
    amount: int
    fee_type: str


@dataclass(frozen=True, slots=True, kw_only=True)
class ApplyInterest(Command):
    account_id: str
    rate_bp: int


@dataclass(frozen=True, slots=True, kw_only=True)
class Freeze(Command):
    account_id: str
    reason: str


@dataclass(frozen=True, slots=True, kw_only=True)
class Unfreeze(Command):
    account_id: str


@dataclass(frozen=True, slots=True, kw_only=True)
class Close(Command):
    account_id: str


# --------------------------------------------------------------------------------------
# events
# --------------------------------------------------------------------------------------


@dataclass(frozen=True, slots=True, kw_only=True)
class Event:
    stream: str
    version: int
    at: int


@dataclass(frozen=True, slots=True, kw_only=True)
class AccountOpened(Event):
    owner: str
    kind: AccountKind
    currency: str
    overdraft_limit: int


@dataclass(frozen=True, slots=True, kw_only=True)
class Deposited(Event):
    amount: int
    reference: str


@dataclass(frozen=True, slots=True, kw_only=True)
class Withdrawn(Event):
    amount: int
    channel: str


@dataclass(frozen=True, slots=True, kw_only=True)
class TransferSent(Event):
    transfer_id: int
    target: str
    amount: int


@dataclass(frozen=True, slots=True, kw_only=True)
class TransferReceived(Event):
    transfer_id: int
    source: str
    amount: int


@dataclass(frozen=True, slots=True, kw_only=True)
class FeeCharged(Event):
    amount: int
    fee_type: str


@dataclass(frozen=True, slots=True, kw_only=True)
class InterestApplied(Event):
    amount: int
    rate_bp: int


@dataclass(frozen=True, slots=True, kw_only=True)
class AccountFrozen(Event):
    reason: str


@dataclass(frozen=True, slots=True, kw_only=True)
class AccountUnfrozen(Event):
    pass


@dataclass(frozen=True, slots=True, kw_only=True)
class AccountClosed(Event):
    final_balance: int


# --------------------------------------------------------------------------------------
# errors
# --------------------------------------------------------------------------------------


class LedgerError(Exception):
    reason = "error"


class InsufficientFunds(LedgerError):
    reason = "insufficient_funds"


class AccountUnavailable(LedgerError):
    reason = "unavailable"


class InvalidCommand(LedgerError):
    reason = "invalid"


class SuspectedFraud(LedgerError):
    reason = "fraud"


class ConcurrencyError(Exception):
    pass


# --------------------------------------------------------------------------------------
# the aggregate
# --------------------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class Snapshot:
    account_id: str
    version: int
    owner: str
    kind: AccountKind
    currency: str
    state: AccountState
    balance: int
    overdraft_limit: int
    accrued_fees: int
    last_activity: int


class Account:
    """Aggregate root: state is only ever changed by applying events."""

    def __init__(self, account_id: str) -> None:
        self.id = account_id
        self.version = 0
        self.owner = ""
        self.kind = AccountKind.CHECKING
        self.currency = "EUR"
        self.state = AccountState.NEW
        self.balance = 0
        self.overdraft_limit = 0
        self.accrued_fees = 0
        self.last_activity = 0
        self._pending: list[Event] = []

    @classmethod
    def from_snapshot(cls, snap: Snapshot) -> "Account":
        acct = cls(snap.account_id)
        acct.version = snap.version
        acct.owner = snap.owner
        acct.kind = snap.kind
        acct.currency = snap.currency
        acct.state = snap.state
        acct.balance = snap.balance
        acct.overdraft_limit = snap.overdraft_limit
        acct.accrued_fees = snap.accrued_fees
        acct.last_activity = snap.last_activity
        return acct

    def snapshot(self) -> Snapshot:
        return Snapshot(self.id, self.version, self.owner, self.kind, self.currency, self.state,
                        self.balance, self.overdraft_limit, self.accrued_fees, self.last_activity)

    # -- event application ---------------------------------------------------------------
    @functools.singledispatchmethod
    def apply(self, event: Event) -> None:
        raise TypeError(f"cannot apply {type(event).__name__}")

    @apply.register
    def _(self, event: AccountOpened) -> None:
        self.owner = event.owner
        self.kind = event.kind
        self.currency = event.currency
        self.overdraft_limit = event.overdraft_limit
        self.state = AccountState.ACTIVE

    @apply.register
    def _(self, event: Deposited) -> None:
        self.balance += event.amount

    @apply.register
    def _(self, event: Withdrawn) -> None:
        self.balance -= event.amount

    @apply.register
    def _(self, event: TransferSent) -> None:
        self.balance -= event.amount

    @apply.register
    def _(self, event: TransferReceived) -> None:
        self.balance += event.amount

    @apply.register
    def _(self, event: FeeCharged) -> None:
        self.balance -= event.amount
        self.accrued_fees += event.amount

    @apply.register
    def _(self, event: InterestApplied) -> None:
        self.balance += event.amount

    @apply.register
    def _(self, event: AccountFrozen) -> None:
        self.state = AccountState.FROZEN

    @apply.register
    def _(self, event: AccountUnfrozen) -> None:
        self.state = AccountState.ACTIVE

    @apply.register
    def _(self, event: AccountClosed) -> None:
        self.state = AccountState.CLOSED
        self.balance = 0

    def _record(self, event_type: type, at: int, **data) -> Event:
        event = event_type(stream=self.id, version=self.version + 1, at=at, **data)
        self.apply(event)
        self.version = event.version
        self.last_activity = at
        self._pending.append(event)
        return event

    def take_pending(self) -> list[Event]:
        out, self._pending = self._pending, []
        return out

    # -- decisions -------------------------------------------------------------------------
    @property
    def available(self) -> int:
        return self.balance + self.overdraft_limit

    def _ensure_active(self) -> None:
        if self.state is not AccountState.ACTIVE:
            raise AccountUnavailable(f"{self.id} is {self.state.name.lower()}")

    def open(self, cmd: OpenAccount) -> None:
        if self.state is not AccountState.NEW:
            raise InvalidCommand(f"{self.id} already exists")
        self._record(AccountOpened, cmd.at, owner=cmd.owner, kind=cmd.kind,
                     currency=cmd.currency, overdraft_limit=cmd.overdraft_limit)

    def deposit(self, amount: int, reference: str, at: int) -> None:
        self._ensure_active()
        self._record(Deposited, at, amount=amount, reference=reference)

    def withdraw(self, amount: int, channel: str, at: int) -> None:
        self._ensure_active()
        if amount > self.available:
            raise InsufficientFunds(f"{self.id}: {amount} > {self.available}")
        self._record(Withdrawn, at, amount=amount, channel=channel)

    def send(self, transfer_id: int, target: str, amount: int, at: int) -> None:
        self._ensure_active()
        if self.kind is AccountKind.SAVINGS and amount > 500_000:
            raise InvalidCommand("savings transfers above 5,000 need a notice period")
        if amount > self.available:
            raise InsufficientFunds(f"{self.id}: {amount} > {self.available}")
        self._record(TransferSent, at, transfer_id=transfer_id, target=target, amount=amount)

    def receive(self, transfer_id: int, source: str, amount: int, at: int) -> None:
        if self.state is AccountState.CLOSED:
            raise AccountUnavailable(f"{self.id} is closed")
        self._record(TransferReceived, at, transfer_id=transfer_id, source=source, amount=amount)

    def charge(self, amount: int, fee_type: str, at: int) -> None:
        if self.state is AccountState.CLOSED:
            raise AccountUnavailable(f"{self.id} is closed")
        self._record(FeeCharged, at, amount=amount, fee_type=fee_type)

    def accrue_interest(self, rate_bp: int, at: int) -> None:
        self._ensure_active()
        if self.balance <= 0:
            return
        amount = self.balance * rate_bp // 10_000 // 12
        if amount:
            self._record(InterestApplied, at, amount=amount, rate_bp=rate_bp)

    def freeze(self, reason: str, at: int) -> None:
        self._ensure_active()
        self._record(AccountFrozen, at, reason=reason)

    def unfreeze(self, at: int) -> None:
        if self.state is not AccountState.FROZEN:
            raise InvalidCommand(f"{self.id} is not frozen")
        self._record(AccountUnfrozen, at)

    def close(self, at: int) -> None:
        self._ensure_active()
        if self.balance < 0:
            raise InsufficientFunds(f"{self.id}: settle {-self.balance} before closing")
        self._record(AccountClosed, at, final_balance=self.balance)


# --------------------------------------------------------------------------------------
# storage
# --------------------------------------------------------------------------------------


class EventStore:
    def __init__(self) -> None:
        self._log: list[Event] = []
        self._streams: defaultdict[str, list[int]] = defaultdict(list)
        self._subscribers: list = []

    def subscribe(self, handler) -> None:
        self._subscribers.append(handler)

    def version(self, stream: str) -> int:
        return len(self._streams.get(stream, ()))

    def append(self, stream: str, expected_version: int, events: list[Event]) -> None:
        if self.version(stream) != expected_version:
            raise ConcurrencyError(f"{stream}: expected v{expected_version}, "
                                   f"store has v{self.version(stream)}")
        positions = self._streams[stream]
        for event in events:
            positions.append(len(self._log))
            self._log.append(event)
        for handler in self._subscribers:
            for event in events:
                handler(event)

    def load(self, stream: str, after_version: int = 0) -> list[Event]:
        log = self._log
        return [log[p] for p in self._streams.get(stream, ())[after_version:]]

    def __iter__(self):
        return iter(self._log)

    def __len__(self) -> int:
        return len(self._log)


class AccountRepository:
    def __init__(self, store: EventStore) -> None:
        self.store = store
        self.snapshots: dict[str, Snapshot] = {}
        self.loads = 0
        self.replayed = 0

    def load(self, account_id: str) -> Account:
        self.loads += 1
        snap = self.snapshots.get(account_id)
        if snap is None:
            acct = Account(account_id)
        else:
            acct = Account.from_snapshot(snap)
        tail = self.store.load(account_id, acct.version)
        for event in tail:
            acct.apply(event)
            acct.version = event.version
            acct.last_activity = event.at
        self.replayed += len(tail)
        return acct

    def save(self, account: Account, expected_version: int) -> None:
        events = account.take_pending()
        if not events:
            return
        self.store.append(account.id, expected_version, events)
        snap = self.snapshots.get(account.id)
        if account.version - (snap.version if snap else 0) >= SNAPSHOT_EVERY:
            self.snapshots[account.id] = account.snapshot()


# --------------------------------------------------------------------------------------
# projections
# --------------------------------------------------------------------------------------


@dataclass
class LedgerView:
    balances: dict[str, int] = field(default_factory=dict)
    owners: defaultdict[str, set[str]] = field(default_factory=lambda: defaultdict(set))
    states: dict[str, str] = field(default_factory=dict)
    daily_volume: Counter[tuple[int, str]] = field(default_factory=Counter)
    fees_by_type: Counter[str] = field(default_factory=Counter)
    transfers_in_flight: dict[int, int] = field(default_factory=dict)
    currency: dict[str, str] = field(default_factory=dict)


@functools.singledispatch
def project(event: Event, view: LedgerView) -> None:
    raise TypeError(type(event).__name__)


@project.register
def _(event: AccountOpened, view: LedgerView) -> None:
    view.balances[event.stream] = 0
    view.owners[event.owner].add(event.stream)
    view.states[event.stream] = "active"
    view.currency[event.stream] = event.currency


@project.register(Deposited)
@project.register(InterestApplied)
def _(event, view: LedgerView) -> None:
    view.balances[event.stream] += event.amount
    view.daily_volume[event.at // 86_400, view.currency[event.stream]] += event.amount


@project.register
def _(event: Withdrawn, view: LedgerView) -> None:
    view.balances[event.stream] -= event.amount
    view.daily_volume[event.at // 86_400, view.currency[event.stream]] += event.amount


@project.register
def _(event: TransferSent, view: LedgerView) -> None:
    view.balances[event.stream] -= event.amount
    view.transfers_in_flight[event.transfer_id] = event.amount


@project.register
def _(event: TransferReceived, view: LedgerView) -> None:
    view.balances[event.stream] += event.amount
    sent = view.transfers_in_flight.pop(event.transfer_id)
    if sent != event.amount:
        raise AssertionError(f"transfer {event.transfer_id} unbalanced")


@project.register
def _(event: FeeCharged, view: LedgerView) -> None:
    view.balances[event.stream] -= event.amount
    view.fees_by_type[event.fee_type] += event.amount


@project.register(AccountFrozen)
@project.register(AccountUnfrozen)
@project.register(AccountClosed)
def _(event, view: LedgerView) -> None:
    match event:
        case AccountFrozen():
            view.states[event.stream] = "frozen"
        case AccountUnfrozen():
            view.states[event.stream] = "active"
        case AccountClosed(final_balance=final):
            view.states[event.stream] = "closed"
            view.balances[event.stream] -= final


class FraudMonitor:
    """Velocity check: too many debits in a sliding window freeze the account."""

    WINDOW = 7_200
    MAX_DEBITS = 6

    def __init__(self) -> None:
        self.recent: defaultdict[str, deque[int]] = defaultdict(lambda: deque(maxlen=self.MAX_DEBITS))
        self.alerts: list[tuple[str, int]] = []

    def check(self, account_id: str, at: int) -> None:
        window = self.recent[account_id]
        if len(window) == window.maxlen and at - window[0] < self.WINDOW:
            self.alerts.append((account_id, at))
            raise SuspectedFraud(f"{account_id}: {len(window)} debits within {at - window[0]}s")
        window.append(at)


# --------------------------------------------------------------------------------------
# command handling
# --------------------------------------------------------------------------------------


class CommandHandler:
    def __init__(self, repo: AccountRepository) -> None:
        self.repo = repo
        self.fraud = FraudMonitor()
        self.rejected: Counter[str] = Counter()
        self.conflicts = 0

    def handle(self, cmd: Command) -> None:
        try:
            self._dispatch(cmd)
        except LedgerError as exc:
            self.rejected[f"{type(cmd).__name__}:{exc.reason}"] += 1

    def _change(self, account_id: str, action) -> None:
        for _attempt in range(3):
            acct = self.repo.load(account_id)
            expected = acct.version
            action(acct)
            try:
                self.repo.save(acct, expected)
                return
            except ConcurrencyError:
                self.conflicts += 1
        raise LedgerError(f"{account_id}: gave up after 3 conflicts")

    def _dispatch(self, cmd: Command) -> None:
        match cmd:
            case OpenAccount(account_id=aid):
                self._change(aid, lambda a: a.open(cmd))
            case Deposit(amount=amount) | Withdraw(amount=amount) | Transfer(amount=amount) \
                    if amount <= 0:
                raise InvalidCommand(f"non-positive amount {amount}")
            case Deposit(account_id=aid, amount=amount, reference=ref):
                self._change(aid, lambda a: a.deposit(amount, ref, cmd.at))
            case Withdraw(account_id=aid, amount=amount, channel=channel):
                self.fraud.check(aid, cmd.at)
                self._change(aid, lambda a: a.withdraw(amount, channel, cmd.at))
            case Transfer(source=src, target=dst) if src == dst:
                raise InvalidCommand("transfer to self")
            case Transfer(source=src, target=dst, amount=amount):
                self.fraud.check(src, cmd.at)
                self._transfer(cmd.command_id, src, dst, amount, cmd.at)
            case ChargeFee(account_id=aid, amount=amount, fee_type=kind):
                self._change(aid, lambda a: a.charge(amount, kind, cmd.at))
            case ApplyInterest(account_id=aid, rate_bp=rate):
                self._change(aid, lambda a: a.accrue_interest(rate, cmd.at))
            case Freeze(account_id=aid, reason=reason):
                self._change(aid, lambda a: a.freeze(reason, cmd.at))
            case Unfreeze(account_id=aid):
                self._change(aid, lambda a: a.unfreeze(cmd.at))
            case Close(account_id=aid):
                self._change(aid, lambda a: a.close(cmd.at))
            case _:
                raise InvalidCommand(f"unknown command {cmd!r}")

    def _transfer(self, transfer_id: int, src: str, dst: str, amount: int, at: int) -> None:
        source = self.repo.load(src)
        target = self.repo.load(dst)
        if target.state is AccountState.NEW:
            raise AccountUnavailable(f"{dst} does not exist")
        if source.currency != target.currency:
            raise InvalidCommand(f"currency mismatch {source.currency}/{target.currency}")
        sv, tv = source.version, target.version
        source.send(transfer_id, dst, amount, at)
        target.receive(transfer_id, src, amount, at)
        self.repo.save(source, sv)
        self.repo.save(target, tv)


def statement(store: EventStore, account_id: str, upto_version: int) -> list[str]:
    """Rebuild an account at a past version and render its statement lines."""
    acct = Account(account_id)
    lines = []
    for event in store.load(account_id)[:upto_version]:
        acct.apply(event)
        acct.version = event.version
        payload = {k: v for k, v in asdict(event).items() if k not in ("stream", "version", "at")}
        detail = ", ".join(f"{k}={v.value if isinstance(v, enum.Enum) else v}"
                           for k, v in payload.items())
        lines.append(f"v{event.version:>3} {type(event).__name__:<16} {detail:<60} "
                     f"balance={acct.balance / 100:>12,.2f}")
    return lines


# --------------------------------------------------------------------------------------
# workload
# --------------------------------------------------------------------------------------


def setup():
    import random

    rng = random.Random(20260315)
    cmds: list[tuple] = []
    accounts = []
    t = 1_773_000_000
    for i in range(N_ACCOUNTS):
        kind = rng.choice(["checking", "checking", "savings", "credit", "merchant"])
        cur = rng.choice(["EUR", "EUR", "EUR", "USD", "SEK"])
        aid = f"ACC{i:06d}"
        accounts.append((aid, cur))
        cmds.append(("open", t, aid, f"owner{rng.randrange(N_ACCOUNTS * 2 // 3):05d}", kind, cur,
                     rng.choice([0, 0, 50_000, 200_000]) if kind != "savings" else 0))
        t += rng.randint(1, 30)
    by_cur = defaultdict(list)
    for aid, cur in accounts:
        by_cur[cur].append(aid)
    frozen: list[str] = []
    for n in range(N_COMMANDS):
        t += rng.randint(1, 90)
        r = rng.random()
        aid, cur = accounts[min(int(rng.paretovariate(1.2)) - 1, N_ACCOUNTS - 1)
                            if rng.random() < 0.15 else rng.randrange(N_ACCOUNTS)]
        if r < 0.30:
            cmds.append(("deposit", t, aid, rng.choice([-500, 0]) if rng.random() < 0.01
                         else rng.randint(500, 400_000), f"INV-{n}"))
        elif r < 0.52:
            cmds.append(("withdraw", t, aid, rng.randint(1_000, 80_000),
                         rng.choice(["atm", "pos", "online"])))
        elif r < 0.85:
            dst = rng.choice(by_cur[cur]) if rng.random() < 0.97 else rng.choice(accounts)[0]
            cmds.append(("transfer", t, aid, dst, rng.randint(100, 120_000),
                         rng.choice(["rent", "invoice", "gift", ""])))
        elif r < 0.92:
            cmds.append(("fee", t, aid, rng.choice([150, 250, 500, 1_500]),
                         rng.choice(["monthly", "overdraft", "fx", "card"])))
        elif r < 0.97:
            cmds.append(("interest", t, aid, rng.choice([50, 125, 300])))
        elif r < 0.977:
            aid = accounts[rng.randrange(100, N_ACCOUNTS)][0]
            frozen.append(aid)
            cmds.append(("freeze", t, aid, rng.choice(["kyc", "chargeback", "court_order"])))
        elif r < 0.999:
            if frozen and rng.random() < 0.9:
                aid = frozen.pop(rng.randrange(len(frozen)))
            cmds.append(("unfreeze", t, aid))
        else:
            cmds.append(("close", t, accounts[rng.randrange(100, N_ACCOUNTS)][0]))
    return {"commands": cmds}


def build_commands(raw: list[tuple]) -> list[Command]:
    out: list[Command] = []
    for i, c in enumerate(raw):
        match c:
            case ("open", at, aid, owner, kind, cur, od):
                out.append(OpenAccount(command_id=i, at=at, account_id=aid, owner=owner,
                                       kind=AccountKind(kind), currency=cur, overdraft_limit=od))
            case ("deposit", at, aid, amount, ref):
                out.append(Deposit(command_id=i, at=at, account_id=aid, amount=amount,
                                   reference=ref))
            case ("withdraw", at, aid, amount, channel):
                out.append(Withdraw(command_id=i, at=at, account_id=aid, amount=amount,
                                    channel=channel))
            case ("transfer", at, src, dst, amount, memo):
                out.append(Transfer(command_id=i, at=at, source=src, target=dst, amount=amount,
                                    memo=memo))
            case ("fee", at, aid, amount, kind):
                out.append(ChargeFee(command_id=i, at=at, account_id=aid, amount=amount,
                                     fee_type=kind))
            case ("interest", at, aid, rate):
                out.append(ApplyInterest(command_id=i, at=at, account_id=aid, rate_bp=rate))
            case ("freeze", at, aid, reason):
                out.append(Freeze(command_id=i, at=at, account_id=aid, reason=reason))
            case ("unfreeze", at, aid):
                out.append(Unfreeze(command_id=i, at=at, account_id=aid))
            case ("close", at, aid):
                out.append(Close(command_id=i, at=at, account_id=aid))
            case _:
                raise ValueError(c)
    return out


def run(state):
    store = EventStore()
    live = LedgerView()
    store.subscribe(functools.partial(lambda view, e: project(e, view), live))
    repo = AccountRepository(store)
    handler = CommandHandler(repo)
    for cmd in build_commands(state["commands"]):
        handler.handle(cmd)

    # rebuild the read model from the log, as a deployment of a new projection would
    rebuilt = LedgerView()
    for event in store:
        project(event, rebuilt)
    if rebuilt.balances != live.balances or rebuilt.states != live.states:
        raise AssertionError("replayed projection differs from the live one")

    # cross-check: every aggregate's balance equals the projection's
    mismatches = 0
    for aid in sorted(live.balances)[::7]:
        acct = repo.load(aid)
        expected = 0 if acct.state is AccountState.CLOSED else acct.balance
        if live.balances[aid] != expected:
            mismatches += 1

    busiest = sorted(live.balances, key=lambda a: (-store.version(a), a))[:5]
    statements = [line for aid in busiest for line in statement(store, aid, 40)]
    portfolios = sorted(((owner, len(accts), sum(live.balances[a] for a in accts))
                         for owner, accts in live.owners.items()),
                        key=lambda row: (-row[2], row[0]))[:25]
    events_by_type = Counter(type(e).__name__ for e in store)
    return {
        "events": len(store),
        "events_by_type": dict(sorted(events_by_type.items())),
        "rejected": dict(sorted(handler.rejected.items())),
        "conflicts": handler.conflicts,
        "fraud_alerts": len(handler.fraud.alerts),
        "loads": repo.loads,
        "replayed": repo.replayed,
        "snapshots": len(repo.snapshots),
        "total_balance": sum(live.balances.values()),
        "states": dict(sorted(Counter(live.states.values()).items())),
        "fees": dict(sorted(live.fees_by_type.items())),
        "volume_days": len(live.daily_volume),
        "peak_day": max(live.daily_volume.items(), key=lambda kv: (kv[1], kv[0])),
        "in_flight": len(live.transfers_in_flight),
        "mismatches": mismatches,
        "portfolios": portfolios,
        "statements": statements,
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
