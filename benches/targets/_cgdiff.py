import re, subprocess, sys
def prof(path):
    out = subprocess.run(["callgrind_annotate", "--inclusive=no", "--threshold=100", path], capture_output=True, text=True).stdout
    d = {}
    for line in out.splitlines():
        m = re.match(r"^\s*([\d,]+) \([^)]*\)\s+(.*)$", line)
        if m and "PROGRAM TOTALS" not in line:
            name = re.sub(r"'\d+", "", m.group(2).split(" [")[0])
            d[name] = d.get(name, 0) + int(m.group(1).replace(",", ""))
    return d
a, b = prof(sys.argv[1]), prof(sys.argv[2])
diff = sorted(((b.get(k, 0) - a.get(k, 0), k) for k in set(a) | set(b)), reverse=True)
print("total", sum(b.values()) - sum(a.values()))
for v, k in diff[:45]:
    print(f"{v:>9,}  {k[:150]}")
