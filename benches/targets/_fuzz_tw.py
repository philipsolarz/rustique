import faulthandler, os, random, sys, textwrap
import rustique
rustique.enable(only=["textwrap"])
alpha = list("ab -\t\n.!?\"'_2²٣éZ  \x0b\x85") + ["--", "  ", "x-y", "Mr. "]
rnd = random.Random(int(sys.argv[1]) if len(sys.argv) > 1 else 0)
log = open("/work/utils/fuzz_last.txt", "w")
faulthandler.dump_traceback_later(20, exit=True)
for i in range(200000):
    t = "".join(rnd.choice(alpha) for _ in range(rnd.randint(0, 60)))
    width = rnd.randint(1, 30)
    ind = ["", " ", "> ", "\t", "xxxxxxxxxx"]
    kw = dict(initial_indent=rnd.choice(ind), subsequent_indent=rnd.choice(ind), expand_tabs=rnd.random() < .5,
              replace_whitespace=rnd.random() < .5, fix_sentence_endings=rnd.random() < .5, break_long_words=rnd.random() < .5,
              drop_whitespace=rnd.random() < .5, break_on_hyphens=rnd.random() < .5, tabsize=rnd.randint(0, 9),
              max_lines=rnd.choice([None, 1, 2, 3]), placeholder=rnd.choice([" [...]", "...", "", "  ~ "]))
    ws = " \t\n\x0b\x0c\r"
    if max(len(kw["initial_indent"]), len(kw["subsequent_indent"])) >= width:
        continue
    log.seek(0); log.write(repr((t, width, kw)) + "\n" + " " * 200); log.flush()
    faulthandler.dump_traceback_later(20, exit=True)
    try:
        textwrap.wrap(t, width, **kw)
    except ValueError:
        pass
print("no hang")
