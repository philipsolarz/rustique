"""Regenerate the `re` port's cargo test data from the running CPython (3.15 or 3.14):

    python crates/rustique-core/src/re/testdata/gen.py [corpus.jsonl] [CPYTHON_SRC]

Writes, next to this file:
- `casemap.txt`: `c lower iscased` (hex) for every code point whose lowercase differs or
  that is cased (`_sre.unicode_tolower`, `_sre.unicode_iscased`);
- `extra.txt`: `re._casefix._EXTRA_CASES` (`key: v v ...`, hex);
- `cases.txt`: one compile per line, tab-separated: `s|b`, flags (decimal), the pattern's
  code units (hex, comma-separated), and CPython's `_sre.compile` arguments
  (`flags|code,...|groups|name:gid,...`) or `ERR` when CPython raises or warns.
"""

import json
import os
import random
import sys
import warnings

import _sre
from re import _casefix, _compiler, _parser
from re import _constants as C

HERE = os.path.dirname(os.path.abspath(__file__))
MAXUNICODE = 0x110000


def ref(p, flags):
    """CPython's _sre.compile arguments, or None if it raises or warns."""
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        try:
            tree = _parser.parse(p, flags)
            code = _compiler._code(tree, flags)
        except Exception:  # noqa: BLE001 - any error: the port must decline
            return None
    st = tree.state
    return (flags | st.flags, [int(x) for x in code], st.groups - 1, list(st.groupdict.items()))


HANDCRAFTED = [
    "", "a", "abc", "a|b", "a|b|c", "ab|ac", "ab|ab", "a|", "|a", "(a)", "(?:a)", "(?:ab)*", "(a)*", "a*", "a+", "a?",
    "a*?", "a+?", "a??", "a*+", "a++", "a?+", "a{3}", "a{3,}", "a{,3}", "a{2,5}", "a{2,5}?", "a{2,5}+", "a{}", "a{",
    "a{x}", "a{1,x}", "{", "}", "a{,}", "x{4294967294}", "x{4294967295}", "x{99999999999}", "x{3,2}", ".", ".*", "^a$",
    "\\Aa\\Z", "\\Aa\\z", "\\ba\\B", "\\d+\\D\\s\\S\\w\\W", "[abc]", "[^abc]", "[a-z]", "[^a-z]", "[a-zA-Z0-9_]",
    "[\\d\\s]", "[\\s\\S]", "[^\\s\\S]", "[\\w\\W]", "[\\d\\D]", "[\\D\\d]", "[a\\Sb\\s]", "[]a]", "[^]a]", "[a-]",
    "[-a]", "[a\\-z]", "[\\]]", "[\\\\]", "[\\b]", "[\\x41-\\x5a]", "[\\u0100-\\u0200]", "[\\U00010000-\\U00010010]",
    "[\\0]", "[\\01]", "[\\012]", "[\\177]", "[\\400]", "[\\8]", "[\\A]", "[\\z]", "[a--b]", "[a&&b]", "[a~~b]",
    "[a||b]", "[[a]", "[a-\\d]", "[\\d-a]", "[z-a]", "[\u00e9\u00e8]", "[\u0100\u0101]", "[\U0001f600]",
    "[\U0001f600-\U0001f64f]", "[a-\U0001f600]", "[\x00-\uffff]", "[\x00-\U0010ffff]", "[^\x00-\uffff]",
    "[aeiouAEIOU]", "[abcdefghijklmnopqrstuvwxyz0123456789]", "[a-ce-gi-k]", "[\u0400-\u04ff\u0500-\u0520]",
    "[\u0100\u0300\u0500\u0700]", "[\u0100-\u0102\u0300-\u0302\u0500-\u0502]",
    "\\x41", "\\x4", "\\u0041", "\\u004", "\\U00000041", "\\U00110000", "\\N{EM DASH}", "\\0", "\\01", "\\012",
    "\\0123", "\\08", "\\1", "(a)\\1", "(a)\\2", "(a)\\10", "(a)\\100", "(a)\\141", "(a)\\400", "(a\\1)", "\\q",
    "\\%", "\\-", "\\ ", "\\", "a\\", "(?P<name>a)(?P=name)", "(?P<name>a)(?P=nam)", "(?P<1>a)", "(?P<a b>x)",
    "(?P<n\u00e9>a)", "(?P<name>a)(?P<name>b)", "(?P<a>x)(?P<b>y)(?P<c>z)", "(?P<x>a)|(?P<y>b)", "(?P=x)",
    "(?P<x>a(?P=x))", "(?P", "(?Px)", "(?#comment)a", "(?#unterminated", "(?=a)b", "(?!a)b", "(?<=a)b", "(?<!a)b",
    "(?<=ab|cd)x", "(?<=a|bc)x", "(?<=a*)x", "(?!)", "(?=)", "(?<=)", "(?<!)a", "(?<x)", "(a)(?(1)b|c)",
    "(a)(?(1)b)", "(a)(?(1)b|)", "(?(1)a|b)(c)", "(?(1)a|b)", "(?P<g>a)(?(g)b|c)", "(?(g)a)", "(a)(?(1)b|c|d)",
    "(a)(?(0)b)", "(a)(?(1a)b)", "(?>abc)", "(?>a|b)*", "(?i)abc", "(?i)ABC", "(?i)[a-z]", "(?i)[A-Z0-9]",
    "(?i)k", "(?i)s", "(?i)\u212a", "(?i)\u017f", "(?i)i", "(?i)\u0130", "(?i)\u0131", "(?i)[ks]", "(?i)[^ks]",
    "(?i)[\u0100-\u01ff]", "(?i)[\U00010400-\U0001044f]", "(?i)\U00010400", "(?i)[\\w]", "(?i)\\w+",
    "(?i)(a)\\1", "(?a)abc", "(?a)\\w", "(?a)\\b", "(?u)\\w", "(?a)(?u)x", "(?au)x", "(?L)x", "(?m)^a$",
    "(?s).", "(?x) a b # comment\n c", "(?x)[ a]", "(?x)a\\ b", "(?x)(?-x: a b) c", "(?x: a b)", "(?-x: a b)",
    "(?i:a)b", "(?-i:a)b", "(?i-i:a)", "(?a:\\w)\\w", "(?u:\\w)", "(?L:x)", "(?i)(?m)a", "a(?i)b", "(?i)a|(?m)b",
    "(?ix)a b", "(?i", "(?i-", "(?-)", "(?i-:a)", "(?--i:a)", "(?-a:x)", "(?x)", "(?x)|a b", "a|(?x)b",
    "(", ")", "a)", "(a", "((a)", "(?:", "*", "+a", "a**", "a*+*", "^*", "$*", "\\b*", "(?=a)*", "\\1*",
    "(a)\\1*", "(?:)*", "()", "()*", "(a|b)*c", "(a|bc)+?d", "(?:a|b|c)", "(?:ab|cd)", "a|b|[cd]|[^e]",
    "x(?:a|b)", "(?:a|b)x", "ab*c|abd", "hello world", "foo.*bar", "foo\\.bar", "^\\s*$", "^(\\d+)-(\\d+)$",
    r"(?P<major>[0-9]+)\.(?P<minor>[0-9]+)", r"[^.\[\]]+(?![^\[]*\])", r"\[(.*)\]$", r"\$\{([^}]+)\}",
    r"^[a-zA-Z_][a-zA-Z0-9_]*$", r"(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})", r"\s+", r"[\s,;]+",
    r"(?:(?:25[0-5]|2[0-4]\d|[01]?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d?\d)", r"https?://[^\s/$.?#].[^\s]*",
    r"[\w.+-]+@[\w-]+\.[\w.-]+", r"(?<!\\)\$", r"(?<=\d)(?=(\d{3})+$)", r"<([a-z]+)([^<]+)*(?:>(.*)<\/\1>|\s+\/>)",
    "abcabcab", "aaaa", "abab", "aabaaab", "x" * 300, "(" * 40 + "a" + ")" * 40, "(" * 70 + "a" + ")" * 70,
    "(?:" * 70 + "a" + ")" * 70, "a" + "|a" * 20, "|".join(chr(0x61 + i) for i in range(26)),
    "|".join(chr(0x100 + 7 * i) for i in range(60)), "[" + "".join(chr(0x100 + 3 * i) for i in range(400)) + "]",
    "[" + "".join(chr(0x61 + 2 * i) for i in range(12)) + "]", "\u00e9t\u00e9", "\u4f60\u597d", "\U0001f600+",
    "(?i)\u00e9t\u00e9", "(?i)\u03a3", "(?i)[\u03a3\u03c3]", "(?i)\u1e9e", "(?i)[\u1e9e]", "(?i)\u0345",
]

BYTES = [
    b"", b"a", b"abc", b"[a-z]+", b"\\d+\\s\\w", b"(?i)abc", b"(?i)[a-z]", b"(?i)\xe9", b"[\x80-\xff]", b"\xff+",
    b"(?L)a", b"(?u)a", b"(?a)\\w", b"\\u0041", b"\\N{x}", b"(?P<a>x)(?P=a)", b"(?P<\xe9>x)", b"\\x41\\101",
    b"(?i)[^k]", b"[^\x00-\x7f]", b"(?x) a b", b"\\bx\\B", b"(?m)^x$", b"[\\s\\S]",
]

FLAGS = [0, C.SRE_FLAG_IGNORECASE, C.SRE_FLAG_MULTILINE, C.SRE_FLAG_DOTALL, C.SRE_FLAG_VERBOSE, C.SRE_FLAG_ASCII,
         C.SRE_FLAG_UNICODE, C.SRE_FLAG_IGNORECASE | C.SRE_FLAG_ASCII, C.SRE_FLAG_IGNORECASE | C.SRE_FLAG_MULTILINE,
         C.SRE_FLAG_VERBOSE | C.SRE_FLAG_IGNORECASE]


def re_tests_patterns(src):
    path = os.path.join(src, "Lib", "test", "re_tests.py")
    ns = {}
    try:
        with open(path) as f:
            exec(compile(f.read(), path, "exec"), ns)
    except Exception:  # noqa: BLE001
        return []
    return [t[0] for t in ns.get("tests", []) if isinstance(t[0], str)]


def line(p, flags):
    r = ref(p, flags)
    kind = "s" if isinstance(p, str) else "b"
    units = ",".join("%x" % (ord(c) if isinstance(p, str) else c) for c in p)
    if r is None:
        out = "ERR"
    else:
        fl, code, groups, gi = r
        out = "%d|%s|%d|%s" % (fl, ",".join(map(str, code)), groups, ",".join("%s:%d" % kv for kv in gi))
    return "%s\t%d\t%s\t%s" % (kind, flags, units, out)


def main():
    corpus = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/rustique-research/static/results/re_patterns.jsonl")
    src = sys.argv[2] if len(sys.argv) > 2 else os.path.expanduser("~/src/cpython-%d.%d.1" % sys.version_info[:2])
    with open(os.path.join(HERE, "casemap.txt"), "w") as f:
        for c in range(MAXUNICODE):
            lo = _sre.unicode_tolower(c)
            cased = _sre.unicode_iscased(c)
            if lo != c or cased:
                f.write("%x %x %d\n" % (c, lo, cased))
    with open(os.path.join(HERE, "extra.txt"), "w") as f:
        for k, v in sorted(_casefix._EXTRA_CASES.items()):
            f.write("%x: %s\n" % (k, " ".join("%x" % x for x in v)))
    rng = random.Random(1234)
    lines = []
    for p in HANDCRAFTED:
        for fl in FLAGS:
            lines.append(line(p, fl))
    for p in BYTES:
        for fl in (0, C.SRE_FLAG_IGNORECASE, C.SRE_FLAG_ASCII, C.SRE_FLAG_MULTILINE | C.SRE_FLAG_DOTALL):
            lines.append(line(p, fl))
    for p in re_tests_patterns(src):
        lines.append(line(p, 0))
        lines.append(line(p, C.SRE_FLAG_IGNORECASE))
    if os.path.exists(corpus):
        seen = set()
        pats = []
        with open(corpus) as f:
            for row in f:
                d = json.loads(row)
                if d.get("bytes"):
                    continue
                if d["pattern"] not in seen:
                    seen.add(d["pattern"])
                    pats.append(d["pattern"])
        for p in rng.sample(pats, min(1500, len(pats))):
            lines.append(line(p, rng.choice(FLAGS)))
    with open(os.path.join(HERE, "cases.txt"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print(len(lines), "cases")


if __name__ == "__main__":
    main()
