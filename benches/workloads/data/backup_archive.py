"""Backup/packaging job: checksum a set of files, pack them into a zip and a tar.gz,
read both archives back and verify, and gzip a log bundle.

Stands for release-packaging, backup-rotation and artifact-upload scripts: ``hashlib``
(sha256 + md5 per file, the way manifests and ETags are built), ``zipfile`` with
``ZIP_DEFLATED``, ``tarfile`` in ``w:gz``/``r:gz`` mode, ``gzip.compress``/``decompress``,
``zlib.crc32`` and ``base64`` for the manifest. Mostly C time (zlib, hash cores) with
pure-Python archive bookkeeping on top. Data shape: 260 in-memory files of 1–64 KB
(source/text-like, JSON-like, and a few incompressible binaries), ~6 MB in total.
"""

REQUIRES: list[str] = []


def setup():
    import random

    rng = random.Random(2024)
    vocab = ("def class return import self value result config error data user path "
             "item index list dict for while if else elif try except with as None True "
             "False print len range").split()
    files = []
    for i in range(260):
        kind = rng.random()
        if kind < 0.6:
            n = rng.randint(1_000, 48_000)
            words = rng.choices(vocab, k=n // 6)
            body = " ".join(words).encode()[:n]
            name = f"src/module_{i:03d}.py"
        elif kind < 0.9:
            rows = [f'{{"id": {j}, "name": "item-{rng.randint(1, 999)}", '
                    f'"price": {rng.uniform(1, 99):.2f}, "tags": ["a", "b"]}}'
                    for j in range(rng.randint(20, 600))]
            body = ("[" + ",\n".join(rows) + "]").encode()
            name = f"data/records_{i:03d}.json"
        else:
            body = rng.randbytes(rng.randint(4_000, 64_000))
            name = f"assets/blob_{i:03d}.bin"
        files.append((name, body))
    log = "\n".join(f"2026-10-{1 + i % 28:02d} 12:00:{i % 60:02d} INFO worker-{i % 8} "
                    f"processed batch {i} in {rng.randint(5, 900)}ms" for i in range(20_000))
    return {"files": files, "log": log.encode()}


def run(state):
    import base64
    import gzip
    import hashlib
    import io
    import json
    import tarfile
    import zipfile
    import zlib

    files = state["files"]
    manifest = []
    for name, body in files:
        manifest.append({
            "path": name,
            "size": len(body),
            "sha256": hashlib.sha256(body).hexdigest(),
            "md5": base64.b64encode(hashlib.md5(body).digest()).decode(),
            "crc32": zlib.crc32(body),
        })

    zbuf = io.BytesIO()
    with zipfile.ZipFile(zbuf, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as zf:
        for name, body in files:
            info = zipfile.ZipInfo(name, date_time=(2026, 10, 1, 12, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            zf.writestr(info, body)
        zf.writestr(zipfile.ZipInfo("MANIFEST.json", date_time=(2026, 10, 1, 12, 0, 0)),
                    json.dumps(manifest, indent=2))
    zip_bytes = zbuf.getvalue()

    tbuf = io.BytesIO()
    with tarfile.open(fileobj=tbuf, mode="w:gz", compresslevel=6) as tf:
        for name, body in files:
            ti = tarfile.TarInfo(name)
            ti.size = len(body)
            ti.mtime = 1_760_000_000
            ti.mode = 0o644
            ti.uname = ti.gname = "build"
            tf.addfile(ti, io.BytesIO(body))
    tar_bytes = tbuf.getvalue()

    expected = {m["path"]: m["sha256"] for m in manifest}
    bad = 0
    with zipfile.ZipFile(io.BytesIO(zip_bytes)) as zf:
        for info in zf.infolist():
            if info.filename == "MANIFEST.json":
                continue
            data = zf.read(info)
            if hashlib.sha256(data).hexdigest() != expected[info.filename]:
                bad += 1
    with tarfile.open(fileobj=io.BytesIO(tar_bytes), mode="r:gz") as tf:
        for member in tf:
            data = tf.extractfile(member).read()
            if hashlib.sha256(data).hexdigest() != expected[member.name]:
                bad += 1

    gz = gzip.compress(state["log"], compresslevel=9, mtime=0)
    assert gzip.decompress(gz) == state["log"]
    return {
        "bad": bad,
        "zip_sha256": hashlib.sha256(zip_bytes).hexdigest(),
        "zip_size": len(zip_bytes),
        "tar_gz_size": len(tar_bytes),
        "log_gz_size": len(gz),
        "manifest_sha256": hashlib.sha256(json.dumps(manifest).encode()).hexdigest(),
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
