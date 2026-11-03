"""Print the scoreboard's instr cells (builtin vs enabled) of a result file."""
import json, sys
d = json.load(open(sys.argv[1]))
for c in sorted(d["cells"], key=lambda c: c["key"]):
    b, e = c.get("builtin"), c.get("enabled")
    if b and e:
        print(f"{c['key']:50s} {b:9.1f} {e:9.1f} {e / b:6.3f}")
    else:
        print(f"{c['key']:50s} {c}")
