"""Text normalisation and word statistics: the preprocessing pass of a search indexer or
NLP pipeline over a multilingual document collection.

Stands for the countless "clean the text, tokenise, count words" scripts: Unicode
normalisation (``unicodedata.normalize`` NFKC/NFD + ``combining`` to strip accents),
``casefold``, ``str.translate`` punctuation removal, ``re.findall``/``re.split``
tokenising and sentence splitting, ``str.split``/``join``/``startswith``, stop-word
filtering, ``Counter`` term frequencies, bigrams, TF-IDF and slugified titles. Data shape:
3,000 documents of ~250 words (~4.8 MB) mixing English, German, French, Polish and Greek
words, some pre-decomposed (NFD) and some full-width characters.
"""

REQUIRES: list[str] = []

N_DOCS = 3_000


def setup():
    import random
    import unicodedata

    rng = random.Random(99)
    words = (
        "the of and to in is that for it as with was on be by this are from or at have an "
        "data system python process file report value result user network server request "
        "time number error analysis model table record index search query text word count "
        "über straße größe müller schön fräulein grüße änderung prüfung "
        "café naïve résumé façade crème élève être déjà où château "
        "zażółć gęślą jaźń łódź kraków źdźbło "
        "αλφα βήτα γάμμα δέλτα ελληνικά λόγος "
        "ＡＢＣ ｆｕｌｌｗｉｄｔｈ №１ ﬁnancial ﬂow"
    ).split()
    weights = [30 if w.isascii() and len(w) <= 3 else 6 if w.isascii() else 2 for w in words]
    punct = [",", ",", ".", ";", ":", "!", "?", " -", "—", "…", "'s", ")"]
    docs = []
    for d in range(N_DOCS):
        sentences = []
        for _ in range(rng.randint(8, 20)):
            n = rng.randint(6, 22)
            ws = rng.choices(words, weights, k=n)
            ws[0] = ws[0].capitalize()
            for j in range(1, n - 1):
                if rng.random() < 0.08:
                    ws[j] += rng.choice(punct)
                if rng.random() < 0.03:
                    ws[j] = ws[j].upper()
            sentences.append(" ".join(ws) + rng.choice([".", ".", ".", "?", "!"]))
        body = "  ".join(sentences) if d % 3 else "\n".join(sentences)
        if d % 5 == 0:
            body = unicodedata.normalize("NFD", body)
        title = " ".join(rng.choices(words, weights, k=rng.randint(3, 8))).title()
        docs.append({"id": d, "title": title, "body": body})
    stop = frozenset("the of and to in is that for it as with was on be by this are from "
                     "or at have an".split())
    return {"docs": docs, "stop": stop}


def run(state):
    import math
    import re
    import string
    import unicodedata
    from collections import Counter

    stop = state["stop"]
    token_re = re.compile(r"\w+(?:['’]\w+)?")
    sentence_re = re.compile(r"(?<=[.!?])\s+")
    punct_table = str.maketrans({c: " " for c in string.punctuation + "—…"})

    def strip_accents(s):
        decomposed = unicodedata.normalize("NFD", s)
        return "".join(c for c in decomposed if not unicodedata.combining(c))

    def slugify(title):
        s = strip_accents(unicodedata.normalize("NFKC", title)).casefold()
        s = re.sub(r"[^\w\s-]", "", s)
        return re.sub(r"[-\s]+", "-", s).strip("-")

    df = Counter()
    doc_tf = []
    total = Counter()
    bigrams = Counter()
    sentence_lengths = Counter()
    slugs = []
    for doc in state["docs"]:
        text = unicodedata.normalize("NFKC", doc["body"])
        for sent in sentence_re.split(text):
            sentence_lengths[min(len(sent.split()), 40)] += 1
        folded = text.casefold()
        tokens = [t for t in token_re.findall(folded) if t not in stop and not t.isdigit()]
        plain = strip_accents(folded.translate(punct_table))
        ascii_words = [w for w in plain.split() if w.isascii() and len(w) > 2]
        tf = Counter(tokens)
        doc_tf.append((doc["id"], tf, len(tokens)))
        df.update(tf.keys())
        total.update(ascii_words)
        bigrams.update(zip(tokens, tokens[1:]))
        slugs.append(slugify(doc["title"]))
    n_docs = len(doc_tf)
    top_terms = []
    for doc_id, tf, n in doc_tf:
        if not n:
            continue
        scored = sorted(((cnt / n) * math.log(n_docs / df[t]), t) for t, cnt in tf.items())
        top_terms.append((doc_id, [t for _, t in scored[-5:]]))
    return {
        "vocab": len(df),
        "top_words": total.most_common(30),
        "top_bigrams": [" ".join(b) for b, _ in bigrams.most_common(20)],
        "sentence_lengths": sorted(sentence_lengths.items()),
        "tfidf_head": top_terms[:40],
        "slugs_head": slugs[:40],
        "slug_chars": sum(map(len, slugs)),
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
