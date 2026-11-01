import textwrap, random
from rustique import _takeover as rq
st = rq.enable(only=["textwrap"])
print([(t["name"], t["state"], t.get("reason")) for t in st["targets"] if t["group"] == "textwrap"])
rq.stats(True)
texts = ["Hello there -- you goof-ball, use the -b option!", "The quick brown fox jumps over the lazy dog. " * 7,
         "a\tb\tc\n  indented line\n\n", "supercalifragilisticexpialidocious " * 3, "x--y well-known self-evident co-op", "", "   ", "single"]
def run():
    out = []
    for t in texts:
        for w in (5, 10, 30, 70):
            out.append(textwrap.wrap(t, w)); out.append(textwrap.fill(t, w, initial_indent="> ", subsequent_indent="  "))
            out.append(textwrap.shorten(t, max(w, 12), placeholder="..."))
            out.append(textwrap.wrap(t, w, break_on_hyphens=False, fix_sentence_endings=True, max_lines=2))
            tw = textwrap.TextWrapper(width=w, drop_whitespace=False, break_long_words=False)
            out.append(tw.wrap(t)); out.append(tw.fill(t))
        out.append(textwrap.dedent(t)); out.append(textwrap.indent(t, "  "))
    return out
on = run()
print({k: v for k, v in rq.stats().items() if k.startswith("textwrap")})
rq.disable()
off = run()
print("identical:", on == off)
for i, (a, b) in enumerate(zip(on, off)):
    if a != b: print(i, repr(a), repr(b))
