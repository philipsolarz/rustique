import random, pprint
from rustique._rustique import _t_random as t
t.install()
pprint.pprint(t.status())
t.stats(True)
def run(mod_on):
    r = random.Random(42)
    out = []
    out.append([r.randint(1, 6) for _ in range(20)])
    out.append([r.randrange(10) for _ in range(5)] + [r.randrange(0, 100, 7) for _ in range(5)] + [r.randrange(50, 0, -3) for _ in range(5)])
    out.append([r.choice('abcdef') for _ in range(5)] + [r.choice([1,2,3]) for _ in range(5)] + [r.choice(range(10**9)) for _ in range(3)])
    l = list(range(50)); r.shuffle(l); out.append(l)
    out.append(r.sample(range(100), 10)); out.append(r.sample(['a','b','c','d'], k=3)); out.append(r.sample(range(10**6), 30))
    out.append(r.choices('abc', k=10)); out.append(r.choices(['x','y','z'], [1, 2.5, 3], k=10)); out.append(r.choices(['x','y'], cum_weights=[1, 3], k=5))
    out.append(r.randint(0, 2**40)); out.append(r.randint(-2**62, 2**62))
    out.append(r.getstate())
    return out
on = run(True)
t.uninstall()
off = run(False)
print("identical:", on == off)
for i,(a,b) in enumerate(zip(on, off)):
    if a != b: print(i, a if i < 13 else "state", b if i < 13 else "")
pprint.pprint(t.counters())
