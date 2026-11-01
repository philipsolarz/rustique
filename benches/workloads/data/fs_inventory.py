"""File-tree inventory: the scanning half of backup, cleanup, build and "find large/old/
duplicate files" scripts, run over a generated project tree.

Stands for scripts that walk directories: ``os.walk`` with in-place pruning and
``fnmatch`` ignore patterns (.gitignore-style), ``os.path.join``/``splitext``/``getsize``,
``os.scandir`` recursion with ``DirEntry.is_dir``/``stat``, ``pathlib.Path.rglob``/
``relative_to``/``suffix``/``parts``, and ``glob.glob(recursive=True)``; then grouping
by extension and directory, duplicate-name and stale-file detection. Data shape: a tree
of ~13,000 small files in ~1,100 directories, 5 levels deep, with hidden and cache
directories, created once in a temp dir with fixed mtimes. Much of the wall time is
kernel time (stat/getdents) that user-space acceleration cannot touch.
"""

REQUIRES: list[str] = []


def setup():
    import os
    import random
    import tempfile

    rng = random.Random(5)
    tmp = tempfile.TemporaryDirectory(prefix="rq-fs-")
    root = os.path.join(tmp.name, "project")
    exts = [".py"] * 6 + [".txt", ".md", ".json", ".yaml", ".log", ".csv", ".png", ".tmp",
                          ".pyc", ".html", ".js", ".css", ".bak", ""]
    names = ["main", "utils", "config", "models", "views", "test_api", "README", "index",
             "data", "schema", "helpers", "settings", "app", "core", "client", "server"]
    dirs = ["src", "tests", "docs", "assets", "scripts", "build", "lib", "pkg", "vendor"]
    t0 = 1_760_000_000
    count = 0

    def make(path, depth):
        nonlocal count
        os.makedirs(path, exist_ok=True)
        for _ in range(rng.randint(6, 18)):
            name = rng.choice(names) + (f"_{rng.randint(1, 40)}" if rng.random() < 0.5 else "")
            fp = os.path.join(path, name + rng.choice(exts))
            with open(fp, "wb") as f:
                f.write(b"x" * rng.randint(0, 3000))
            mt = t0 - rng.randint(0, 400) * 86400
            os.utime(fp, (mt, mt))
            count += 1
        if depth < 5:
            subs = rng.sample(dirs, [9, 5, 3, 2, 2][depth])
            for d in subs:
                make(os.path.join(path, d), depth + 1)
            if rng.random() < 0.25:
                make(os.path.join(path, rng.choice([".git", "__pycache__", "node_modules",
                                                    ".venv"])), 5)
    make(root, 0)
    return {"tmp": tmp, "root": root, "now": t0}


def run(state):
    import fnmatch
    import glob
    import os
    from collections import Counter, defaultdict
    from pathlib import Path

    root = state["root"]
    now = state["now"]
    ignore_dirs = {".git", "__pycache__", "node_modules", ".venv", "build"}
    ignore_patterns = ["*.pyc", "*.tmp", "*.bak", "*~", ".DS_Store", "*.log", "Thumbs.db",
                       "*.swp", "*.egg-info", "*.o", "*.so", "coverage.*"]

    # 1. os.walk + fnmatch: what a backup script would copy
    manifest = []
    by_ext = defaultdict(lambda: [0, 0])
    skipped = 0
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in ignore_dirs)
        for fn in sorted(filenames):
            if any(fnmatch.fnmatch(fn, pat) for pat in ignore_patterns):
                skipped += 1
                continue
            full = os.path.join(dirpath, fn)
            size = os.path.getsize(full)
            ext = os.path.splitext(fn)[1].lower() or "<none>"
            by_ext[ext][0] += 1
            by_ext[ext][1] += size
            manifest.append((os.path.relpath(full, root), size))

    # 2. os.scandir recursion: stale and large files
    stale = []
    largest = []

    def scan(path):
        with os.scandir(path) as it:
            for entry in it:
                if entry.is_dir(follow_symlinks=False):
                    scan(entry.path)
                elif entry.is_file(follow_symlinks=False):
                    st = entry.stat(follow_symlinks=False)
                    age_days = (now - st.st_mtime) // 86400
                    if age_days > 365:
                        stale.append(entry.path[len(root) + 1:])
                    largest.append((st.st_size, entry.name))
    scan(root)
    stale.sort()
    largest.sort(reverse=True)

    # 3. pathlib: Python sources per top-level package, duplicate module names
    base = Path(root)
    per_pkg = Counter()
    dup_names = defaultdict(list)
    for p in base.rglob("*.py"):
        rel = p.relative_to(base)
        if any(part in ignore_dirs for part in rel.parts):
            continue
        per_pkg[rel.parts[0] if len(rel.parts) > 1 else "."] += 1
        dup_names[p.stem].append(rel.as_posix())
    dups = sorted((k, sorted(v)) for k, v in dup_names.items() if len(v) > 3)

    # 4. glob: config files anywhere
    configs = sorted(os.path.relpath(p, root)
                     for pat in ("**/*.json", "**/*.yaml", "**/settings*")
                     for p in glob.glob(os.path.join(root, pat), recursive=True))

    manifest.sort()
    return {
        "files": len(manifest),
        "skipped": skipped,
        "bytes": sum(s for _, s in manifest),
        "by_ext": sorted((k, v[0], v[1]) for k, v in by_ext.items()),
        "manifest_head": manifest[:50],
        "stale": len(stale),
        "stale_head": stale[:20],
        "largest": largest[:10],
        "per_pkg": sorted(per_pkg.items()),
        "dups": [(k, len(v)) for k, v in dups],
        "configs": len(configs),
        "configs_head": configs[:20],
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
