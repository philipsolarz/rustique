"""After enable(), call every `re` entry point once, then list targets that status() reports as degraded."""
import json
import re
import sys

import rustique

rustique.enable()
re.compile(r"(a+)b"); re.match(r"x(\d+)", "x12"); re.sub("a", "b", "aaa"); re.subn("a", "b", "a")
re.findall(r"\w+", "a b"); re.search("b", "ab"); re.split(",", "a,b"); re.fullmatch("a", "a"); list(re.finditer("a", "aa"))
st = rustique.status()
print(sys.version.split()[0], sys.prefix, json.dumps({t["name"]: t["degraded"] for t in st["targets"] if t.get("degraded")}))
