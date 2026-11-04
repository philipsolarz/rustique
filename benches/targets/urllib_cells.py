"""Callgrind cells for the `urllib` group (benches/targets/utils_cg.py urllib_cells)."""

COMMON = """
import urllib.parse as up
from urllib.parse import quote, unquote, urlsplit, urlparse, parse_qs, parse_qsl, urljoin
P = '/api/v1/items/caf\\u00e9 list'
Q = 'page=2&sort=name&q=hello+world&tag=a&tag=b'
URL = 'https://user@example.com:8443/api/v1/items?page=2&sort=name#top'
URLS = ['https://h%d.example.com/p/%d?q=%d#f' % (i, i, i) for i in range(4096)]
it = iter(URLS * 1000)
class S(str):
    pass
SQ = S('a b')
P2 = '/api/v1/items/\u0161\u010d list'
U2 = 'a%20b%C3%A9/\u0161'
"""

INSTALL = """
import rustique
rustique.enable(only=["urllib"])
"""

CELLS = {
    "quote(path)": "quote(P)",
    "quote('abc') [all safe]": "quote('abc')",
    "quote_plus('a b&c')": "up.quote_plus('a b&c')",
    "unquote('a%20b%C3%A9')": "unquote('a%20b%C3%A9')",
    "unquote('plain') [no %]": "unquote('plain')",
    "unquote_plus('a+b')": "up.unquote_plus('a+b')",
    "parse_qsl(5 fields)": "parse_qsl(Q)",
    "parse_qs(5 fields)": "parse_qs(Q)",
    "urlparse(URL) [uncached]": "urlparse(URL)",
    "urlsplit(new url) [lru miss]": "urlsplit(next(it))",
    "urlsplit(URL) [lru hit]": "urlsplit(URL)",
    "_urlsplit(URL)": "up._urlsplit(URL)",
    "urljoin(base, rel)": "urljoin(URL, '../x?y=1')",
    "quote(path, UCS-2)": "quote(P2)",
    "unquote(mixed, UCS-2)": "unquote(U2)",
    "quote(bytes)": "quote(b'/a b/c')",
    # fallbacks
    "quote(' ') [fallback: _Quoter cache object]": "quote(' ')",
    "quote(str subclass) [fallback]": "quote(SQ)",
    "quote(s, encoding='latin-1') [fallback]": "quote(P, encoding='latin-1')",
    "unquote(s, encoding='latin-1') [fallback]": "unquote('a%20b', encoding='latin-1')",
    "urlparse('http://[::1]/') [fallback]": "urlparse('http://[::1]:80/')",
    "parse_qsl(bytes) [fallback]": "parse_qsl(b'a=1&b=2')",
}
