"""A long-running cache tier: a RESP (Redis-protocol) cache server with LRU/TTL eviction,
and application clients using redis-py's asyncio client with pickled values.

Stands for the caching layer of Python web stacks: Django's and Flask-Caching's Redis
backends, ``aiocache``, rate limiters and session stores all talk RESP through redis-py
(a top-100 PyPI package; without the optional hiredis extension its parser is pure
Python) and pickle the values they store. The server side is an in-process asyncio
``Protocol`` speaking RESP2 (one ``cachetools.LRUCache`` per logical database plus
logical-clock TTLs, ``GET/SET EX NX/MGET/DEL/INCRBY/EXPIRE/TTL/EXISTS/FLUSHDB``), so the
whole request path -- client command packing and response parsing, connection pools,
pipelines, socket I/O over 127.0.0.1, server-side parsing, eviction and expiry -- runs in
one process. The server and the client connection pools start once in setup and persist
across runs (the heap, the eviction structures and the GC generations age as in a
long-running service); each of 16 concurrent clients works in its own database, flushes
it, then runs a fixed mix of get-or-set of pickled session/profile objects (skewed key
popularity), ``MGET`` fan-outs, counter ``INCRBY``s, ``EXPIRE``/``TTL``, ``SET NX`` locks
and non-transactional pipelines: 9,600 operations (~24,800 commands) per run, from a cold
cache each run (about 1 in 4 get-or-sets hits). Peak RSS and heap growth are recorded by
``services/_steady.py``.
"""

REQUIRES = ["redis==8.1.0", "cachetools==7.2.8"]

N_CLIENTS = 16
OPS_PER_CLIENT = 600


class _DB:
    def __init__(self, maxsize):
        import cachetools

        self.data = cachetools.LRUCache(maxsize=maxsize)
        self.expires = {}
        self.clock = 0

    def alive(self, key):
        exp = self.expires.get(key)
        if exp is not None and exp <= self.clock:
            self.expires.pop(key, None)
            self.data.pop(key, None)
            return False
        return key in self.data


def _make_server(dbs):
    import asyncio

    def bulk(v):
        return b"$-1\r\n" if v is None else b"$%d\r\n%s\r\n" % (len(v), v)

    def integer(n):
        return b":%d\r\n" % n

    OK = b"+OK\r\n"

    def execute(proto, args):
        cmd = args[0].upper()
        db = dbs[proto.db]
        db.clock += 1
        if cmd == b"GET":
            return bulk(db.data[args[1]] if db.alive(args[1]) else None)
        if cmd == b"SET":
            key, val = args[1], args[2]
            opts = [a.upper() for a in args[3:]]
            if b"NX" in opts and db.alive(key):
                return b"$-1\r\n"
            db.data[key] = val
            db.expires.pop(key, None)
            if b"EX" in opts:
                db.expires[key] = db.clock + int(args[3 + opts.index(b"EX") + 1])
            return OK
        if cmd == b"MGET":
            out = [b"*%d\r\n" % (len(args) - 1)]
            out += [bulk(db.data[k] if db.alive(k) else None) for k in args[1:]]
            return b"".join(out)
        if cmd in (b"INCRBY", b"INCR"):
            key = args[1]
            n = int(db.data[key]) if db.alive(key) else 0
            n += int(args[2]) if cmd == b"INCRBY" else 1
            db.data[key] = str(n).encode()
            return integer(n)
        if cmd == b"DEL":
            n = 0
            for k in args[1:]:
                if db.alive(k):
                    del db.data[k]
                    db.expires.pop(k, None)
                    n += 1
            return integer(n)
        if cmd == b"EXISTS":
            return integer(sum(db.alive(k) for k in args[1:]))
        if cmd == b"EXPIRE":
            if not db.alive(args[1]):
                return integer(0)
            db.expires[args[1]] = db.clock + int(args[2])
            return integer(1)
        if cmd == b"TTL":
            if not db.alive(args[1]):
                return integer(-2)
            exp = db.expires.get(args[1])
            return integer(-1 if exp is None else exp - db.clock)
        if cmd == b"FLUSHDB":
            dbs[proto.db] = _DB(db.data.maxsize)
            return OK
        if cmd == b"SELECT":
            proto.db = int(args[1])
            return OK
        if cmd in (b"PING",):
            return b"+PONG\r\n"
        if cmd == b"CLIENT":
            return OK
        return b"-ERR unknown command '%s'\r\n" % cmd

    class RespProtocol(asyncio.Protocol):
        def connection_made(self, transport):
            self.transport = transport
            self.buf = bytearray()
            self.db = 0

        def data_received(self, data):
            buf = self.buf
            buf += data
            out = []
            pos = 0
            n = len(buf)
            while pos < n:
                if buf[pos] != 42:  # '*'
                    raise ValueError("inline commands are not supported")
                eol = buf.find(b"\r\n", pos)
                if eol < 0:
                    break
                count = int(buf[pos + 1:eol])
                p = eol + 2
                args = []
                for _ in range(count):
                    eol = buf.find(b"\r\n", p)
                    if eol < 0:
                        break
                    size = int(buf[p + 1:eol])
                    start = eol + 2
                    if start + size + 2 > n:
                        break
                    args.append(bytes(buf[start:start + size]))
                    p = start + size + 2
                if len(args) < count:
                    break
                out.append(execute(self, args))
                pos = p
            del buf[:pos]
            if out:
                self.transport.write(b"".join(out))

    return RespProtocol


def setup():
    import asyncio
    import datetime as dt
    import random
    import socket
    from decimal import Decimal

    import redis.asyncio as aioredis

    dbs = {i: _DB(maxsize=2000) for i in range(N_CLIENTS)}
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    server = loop.run_until_complete(loop.create_server(_make_server(dbs), sock=sock))
    clients = [aioredis.Redis(host="127.0.0.1", port=port, db=i, protocol=2, max_connections=4,
                              decode_responses=False, health_check_interval=0) for i in range(N_CLIENTS)]

    rng = random.Random(6)
    words = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda".split()

    def profile(uid):
        return {"id": uid, "name": f"{rng.choice(words).title()} {uid}", "email": f"u{uid}@example.com",
                "roles": rng.sample(["admin", "editor", "viewer", "billing"], 2),
                "prefs": {"theme": rng.choice(["dark", "light"]), "lang": rng.choice(["en", "de", "pl"]),
                          "tz": "Europe/Warsaw", "notifications": {w: rng.random() < 0.5 for w in words[:6]}},
                "balance": Decimal(rng.randint(0, 10 ** 6)) / 100,
                "last_login": dt.datetime(2025, 3, 1, rng.randint(0, 23), rng.randint(0, 59)),
                "recent": [{"sku": f"SKU-{rng.randint(1, 9999)}", "qty": rng.randint(1, 3)} for _ in range(rng.randint(0, 8))]}

    profiles = {uid: profile(uid) for uid in range(1500)}
    plans = []
    for c in range(N_CLIENTS):
        ops = []
        for i in range(OPS_PER_CLIENT):
            k = i % 10
            uid = int(1500 * rng.random() ** 3)  # skewed popularity: hot keys hit, the tail misses
            if k in (0, 3, 6):
                ops.append(("getset", uid))
            elif k in (1, 7):
                ops.append(("mget", [(uid + j * 11) % 1500 for j in range(8)]))
            elif k == 2:
                ops.append(("incr", f"rate:{uid % 50}"))
            elif k == 4:
                ops.append(("pipe", [(uid + j) % 1500 for j in range(6)]))
            elif k == 5:
                ops.append(("ttl", uid))
            elif k == 8 and i % 40 == 8:
                ops.append(("delete", uid))
            elif k == 8:
                ops.append(("getset", uid))
            else:
                ops.append(("lock", f"lock:{uid % 20}"))
        plans.append(ops)
    return {"loop": loop, "server": server, "clients": clients, "plans": plans, "profiles": profiles}


async def _client(r, ops, profiles):
    import pickle

    hits = misses = 0
    log = []
    await r.flushdb()
    for op, arg in ops:
        if op == "getset":
            key = f"user:{arg}"
            raw = await r.get(key)
            if raw is None:
                misses += 1
                await r.set(key, pickle.dumps(profiles[arg], protocol=5), ex=3000)
            else:
                hits += 1
                log.append(pickle.loads(raw)["name"])
        elif op == "mget":
            vals = await r.mget([f"user:{u}" for u in arg])
            found = [pickle.loads(v)["id"] for v in vals if v is not None]
            log.append(sum(found))
        elif op == "incr":
            n = await r.incrby(arg, 3)
            await r.expire(arg, 60)
            log.append(n)
        elif op == "pipe":
            async with r.pipeline(transaction=False) as p:
                for u in arg:
                    p.set(f"session:{u}", pickle.dumps({"uid": u, "csrf": f"tok{u}", "cart": [u, u + 1]}), ex=120)
                    p.exists(f"user:{u}")
                res = await p.execute()
            log.append(sum(1 for x in res if x == 1))
        elif op == "ttl":
            log.append(await r.ttl(f"user:{arg}"))
        elif op == "delete":
            log.append(await r.delete(f"user:{arg}", f"session:{arg}"))
        else:
            got = await r.set(arg, b"1", nx=True, ex=5)
            log.append(bool(got))
            if got:
                await r.delete(arg)
    return hits, misses, log


def run(state):
    import asyncio
    import hashlib

    async def main():
        return await asyncio.gather(*[_client(r, ops, state["profiles"])
                                      for r, ops in zip(state["clients"], state["plans"])])

    res = state["loop"].run_until_complete(main())
    h = hashlib.sha256(repr([x[2] for x in res]).encode()).hexdigest()
    return [(x[0], x[1]) for x in res], h


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
