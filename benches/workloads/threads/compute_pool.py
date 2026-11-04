"""Parallel pure-Python compute on a thread pool: the free-threading promise itself.

Stands for CPU-bound Python split across threads now that free-threaded CPython can run
them in parallel: batch scoring / matching jobs, simulations and data cleaning that used
to need `multiprocessing`. One run scores 300 documents on a `ThreadPoolExecutor`
(`WL_THREADS`, default 8): tokenizing with `str` methods and a shared compiled regex,
term counting in dicts, TF-IDF cosine similarity against 24 reference profiles held in a
read-only dict shared by every thread, fuzzy matching of title words against a shared
vocabulary with a pure-Python edit distance, a small Dijkstra over a shared graph
(`heapq`), and `sorted(..., key=)` / `statistics` summaries. Read-only sharing is the
realistic pattern (one model, many workers) and the one free-threaded CPython handles
with refcount and dict-lookup contention. Results come back in submission order.
"""

import os

REQUIRES: list[str] = []

N_DOCS = 300


def setup():
    import math
    import random

    rng = random.Random(47)
    syll = ["ka", "ro", "mi", "tan", "sel", "or", "vi", "lu", "pe", "dra", "no", "quin", "est", "ur", "ba"]
    vocab = sorted({"".join(rng.choice(syll) for _ in range(rng.randint(2, 4))) for _ in range(1200)})
    # Reference profiles: 24 topics, each a sparse weighted term vector.
    profiles = {}
    for t in range(24):
        terms = rng.sample(vocab, 60)
        profiles[f"topic-{t:02d}"] = {w: round(rng.uniform(0.2, 3.0), 3) for w in terms}
    df = {}
    docs = []
    for d in range(N_DOCS):
        topic = profiles[f"topic-{rng.randrange(24):02d}"]
        tw = list(topic)
        words = [rng.choice(tw) if rng.random() < 0.55 else rng.choice(vocab) for _ in range(rng.randint(120, 260))]
        # typos in the title so fuzzy matching has work to do
        title = []
        for w in rng.sample(words, 5):
            if rng.random() < 0.6 and len(w) > 3:
                i = rng.randrange(len(w))
                w = w[:i] + rng.choice("aeiouxz") + w[i + 1:]
            title.append(w)
        text = " ".join(words)
        text = text.replace(" ka", ", Ka").replace(" ro", ". Ro")
        docs.append((f"doc-{d:04d}", " ".join(title).title(), text))
        for w in set(words):
            df[w] = df.get(w, 0) + 1
    idf = {w: math.log((N_DOCS + 1) / (c + 1)) + 1.0 for w, c in df.items()}
    # A shared graph: 400 nodes, ~6 weighted edges each.
    graph = {n: [] for n in range(400)}
    for n in range(400):
        for _ in range(6):
            m = rng.randrange(400)
            if m != n:
                graph[n].append((m, rng.randint(1, 30)))
    threads = int(os.environ.get("WL_THREADS", "8"))
    return {"docs": docs, "profiles": profiles, "idf": idf, "vocab": vocab, "graph": graph, "threads": threads}


def _scorer(state):
    import heapq
    import math
    import re
    import statistics

    profiles, idf, vocab, graph = state["profiles"], state["idf"], state["vocab"], state["graph"]
    word_re = re.compile(r"[a-z]+")
    by_len = {}
    for w in vocab:
        by_len.setdefault(len(w), []).append(w)
    norms = {k: math.sqrt(sum(v * v for v in p.values())) for k, p in profiles.items()}

    def edit_distance(a, b, cap):
        if abs(len(a) - len(b)) > cap:
            return cap + 1
        prev = list(range(len(b) + 1))
        for i, ca in enumerate(a, 1):
            cur = [i]
            best = i
            for j, cb in enumerate(b, 1):
                v = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb))
                cur.append(v)
                if v < best:
                    best = v
            if best > cap:
                return cap + 1
            prev = cur
        return prev[-1]

    def fuzzy(word):
        best = (3, word)
        for n in (len(word) - 1, len(word), len(word) + 1):
            for cand in by_len.get(n, ()):
                d = edit_distance(word, cand, best[0] - 1 if best[0] > 0 else 0)
                if d < best[0] or (d == best[0] and cand < best[1]):
                    best = (d, cand)
                    if d == 0:
                        return best
        return best

    def shortest(src, dst):
        dist = {src: 0}
        heap = [(0, src)]
        while heap:
            d, n = heapq.heappop(heap)
            if n == dst:
                return d
            if d > dist.get(n, 1 << 30):
                continue
            for m, w in graph[n]:
                nd = d + w
                if nd < dist.get(m, 1 << 30):
                    dist[m] = nd
                    heapq.heappush(heap, (nd, m))
        return -1

    def score(doc):
        doc_id, title, text = doc
        tf = {}
        for w in word_re.findall(text.lower()):
            tf[w] = tf.get(w, 0) + 1
        vec = {w: c * idf.get(w, 1.0) for w, c in tf.items()}
        vnorm = math.sqrt(sum(v * v for v in vec.values()))
        sims = []
        for k, prof in profiles.items():
            dot = 0.0
            for w, pv in prof.items():
                x = vec.get(w)
                if x is not None:
                    dot += x * pv
            sims.append((round(dot / (vnorm * norms[k]), 9), k))
        sims.sort(key=lambda s: (-s[0], s[1]))
        fixed = [fuzzy(w.lower()) for w in title.split()]
        top_terms = sorted(vec.items(), key=lambda kv: (-kv[1], kv[0]))[:8]
        a = int(doc_id[4:]) % 400
        hops = shortest(a, (a * 7 + 13) % 400)
        lens = [len(w) for w in tf]
        return (doc_id, sims[:3], fixed, [t for t, _ in top_terms], hops,
                round(statistics.fmean(lens), 6), statistics.median(lens))

    return score


def run(state):
    from concurrent.futures import ThreadPoolExecutor

    score = _scorer(state)
    with ThreadPoolExecutor(max_workers=state["threads"]) as pool:
        return list(pool.map(score, state["docs"]))


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
