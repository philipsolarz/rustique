"""Move the Rustique extension to the top level of a wheel (docs/DECISIONS.md §10.7).

    python tools/repack_wheel.py WHEEL [WHEEL ...] [-o OUTDIR]

maturin builds the extension inside the package (``rustique/_rustique.<abi>.so|pyd``) and,
through an ``include``, ships the top-level shim ``_rustique.py`` that loads it from there.
The ``.pth`` startup line imports ``_rustique``: as a top-level extension that is one plain
import, ~0.46M instructions cheaper per ``RUSTIQUE=1`` start on 3.15 and ~1.16M on 3.14
than loading through a package (docs/atlas/w2-startup.md §1.2). This script does what
``wheel unpack`` / move / ``wheel pack`` would, with the standard library only:

- ``rustique/_rustique.<abi>.<ext>`` becomes ``_rustique.<abi>.<ext>`` (same bytes, same
  file attributes);
- the shim ``_rustique.py`` is dropped;
- ``*.dist-info/RECORD`` is regenerated (sha256 and size of every file) and written last.

Every other entry is copied unchanged, in order; the file name, tags and ``WHEEL`` metadata
do not change. Without ``-o`` each wheel is replaced in place (atomically); with ``-o`` the
repacked wheels go to OUTDIR. A wheel already in top-level form is left alone. So is a wheel
with libraries grafted next to the extension by a repair tool (``*.libs/``, ``.dylibs/``; on
musl, ``libgcc_s`` for Rust's unwinder): the extension's run-time search path is relative to
its directory, so it keeps maturin's layout and the ``_rustique.py`` shim, which every layout
supports. A wheel with neither layout is an error: exit status 1.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import os
import re
import shutil
import sys
import tempfile
import zipfile

IN_PACKAGE = re.compile(r"^rustique/(_rustique\.[^/]+\.(?:so|pyd))$")
TOP_LEVEL = re.compile(r"^_rustique\.[^/]+\.(?:so|pyd)$")
SHIM = "_rustique.py"
GRAFTED = re.compile(r"(^|/)[^/]*\.libs/|(^|/)\.dylibs/")


def record_hash(data: bytes) -> str:
    return "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()


def repack(src: str, dest: str) -> str:
    """Repack `src` into `dest` (may be the same path). Returns a one-line summary."""
    with zipfile.ZipFile(src) as zin:
        infos = zin.infolist()
        names = [i.filename for i in infos]
        records = [n for n in names if re.fullmatch(r"[^/]+\.dist-info/RECORD", n)]
        if len(records) != 1:
            raise SystemExit(f"{src}: expected one *.dist-info/RECORD, found {records}")
        record = records[0]
        grafted = sorted({n for n in names if GRAFTED.search(n)})
        moved = [n for n in names if IN_PACKAGE.match(n)]
        top = [n for n in names if TOP_LEVEL.match(n)]
        if grafted:
            if moved and SHIM in names and os.path.abspath(src) != os.path.abspath(dest):
                shutil.copyfile(src, dest)
            if moved and SHIM in names:
                return (f"{os.path.basename(src)}: kept maturin's layout and the shim "
                        f"(libraries grafted by a repair tool: {', '.join(grafted[:2])})")
            raise SystemExit(f"{src}: libraries grafted by a repair tool ({grafted[0]}, ...) without the shim layout")
        if not moved and top and SHIM not in names:
            if os.path.abspath(src) != os.path.abspath(dest):
                shutil.copyfile(src, dest)
            return f"{os.path.basename(src)}: already top-level ({top[0]})"
        if len(moved) + len(top) != 1:
            raise SystemExit(f"{src}: expected exactly one _rustique extension, found {moved + top}")
        rename = {n: IN_PACKAGE.match(n).group(1) for n in moved}  # type: ignore[union-attr]
        rows: list[str] = []
        tmp = tempfile.NamedTemporaryFile(dir=os.path.dirname(os.path.abspath(dest)), suffix=".whl.tmp", delete=False)
        try:
            with tmp, zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as zout:
                for info in infos:
                    name = info.filename
                    if name == SHIM or name == record or name.startswith(record.rsplit("/", 1)[0] + "/RECORD."):
                        continue  # the shim; RECORD (rewritten below) and its signatures
                    data = zin.read(info)
                    new = zipfile.ZipInfo(rename.get(name, name), date_time=info.date_time)
                    new.external_attr = info.external_attr
                    new.create_system = info.create_system
                    new.compress_type = zipfile.ZIP_DEFLATED
                    zout.writestr(new, data)
                    if not new.filename.endswith("/"):
                        rows.append(f"{new.filename},{record_hash(data)},{len(data)}")
                rows.append(f"{record},,")
                info = zin.getinfo(record)
                new = zipfile.ZipInfo(record, date_time=info.date_time)
                new.external_attr = info.external_attr
                new.create_system = info.create_system
                new.compress_type = zipfile.ZIP_DEFLATED
                zout.writestr(new, "\n".join(rows) + "\n")
        except BaseException:
            os.unlink(tmp.name)
            raise
    # Replace only once the source is closed: Windows refuses to replace a file that is open.
    try:
        verify(tmp.name)
        os.replace(tmp.name, dest)
    except BaseException:
        os.unlink(tmp.name)
        raise
    what = f"{moved[0]} -> {rename[moved[0]]}" if moved else f"{top[0]} kept"
    return f"{os.path.basename(src)}: {what}" + (f", {SHIM} dropped" if SHIM in names else "")


def verify(path: str) -> None:
    """Every file is in RECORD with its hash and size; the extension is top-level only."""
    with zipfile.ZipFile(path) as z:
        names = [n for n in z.namelist() if not n.endswith("/")]
        record = next(n for n in names if n.endswith(".dist-info/RECORD"))
        rows = {}
        for line in z.read(record).decode().splitlines():
            p, h, size = line.rsplit(",", 2)
            rows[p] = (h, size)
        assert set(rows) == set(names), sorted(set(rows) ^ set(names))
        for n in names:
            if n != record:
                data = z.read(n)
                assert rows[n] == (record_hash(data), str(len(data))), n
        assert sum(bool(TOP_LEVEL.match(n)) for n in names) == 1, names
        assert not any(IN_PACKAGE.match(n) for n in names) and SHIM not in names, names


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("wheels", nargs="+", metavar="WHEEL")
    ap.add_argument("-o", "--outdir", help="write the repacked wheels here (default: replace in place)")
    args = ap.parse_args(argv)
    if args.outdir:
        os.makedirs(args.outdir, exist_ok=True)
    status = 0
    for wheel in args.wheels:
        dest = os.path.join(args.outdir, os.path.basename(wheel)) if args.outdir else wheel
        try:
            print(repack(wheel, dest))
        except SystemExit as e:
            print(f"error: {e}", file=sys.stderr)
            status = 1
    return status


if __name__ == "__main__":
    sys.exit(main())
