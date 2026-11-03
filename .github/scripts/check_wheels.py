"""Check the file layout of built wheels (wheels.yml; docs/targets/dx.md §5). Standard library only.

    python .github/scripts/check_wheels.py --layout top|shim WHEEL [WHEEL ...]

``top`` (release wheels, after ``tools/repack_wheel.py``): the extension is a top-level
``_rustique.<abi>.{so,pyd}``, nothing at ``rustique/_rustique.*``, no ``_rustique.py`` shim.
``shim`` (maturin's own layout, e.g. a wheel built from the sdist): the extension inside the
package and the top-level ``_rustique.py`` shim that loads it. Under ``top``, a wheel with
libraries grafted by a repair tool (``*.libs/``, ``.dylibs/``: musl wheels carry ``libgcc_s``)
must have the ``shim`` layout instead: repack_wheel.py leaves those alone. Both: the startup ``.pth`` line
at the root and RECORD listing every file with its hash.
"""

import base64
import hashlib
import re
import sys
import zipfile

PTH = "_rustique_startup.pth"
EXT = re.compile(r"^_rustique\.[^/]+\.(so|pyd)$")
PKG_EXT = re.compile(r"^rustique/_rustique\.[^/]+\.(so|pyd)$")


def check(path: str, layout: str) -> list[str]:
    errors = []
    with zipfile.ZipFile(path) as z:
        names = z.namelist()
        top = [n for n in names if EXT.match(n)]
        inpkg = [n for n in names if PKG_EXT.match(n)]
        shim = "_rustique.py" in names
        if layout == "top" and any(re.search(r"(^|/)[^/]*\.libs/|(^|/)\.dylibs/", n) for n in names):
            layout = "shim"  # grafted libraries: repack_wheel.py keeps maturin's layout
        if layout == "top":
            if len(top) != 1:
                errors.append(f"expected one top-level _rustique extension, found {top or 'none'}")
            if inpkg:
                errors.append(f"extension still inside the package: {inpkg}")
            if shim:
                errors.append("top-level _rustique.py shim still present")
        else:
            if len(inpkg) != 1:
                errors.append(f"expected the extension inside the package, found {inpkg or 'none'}")
            if not shim:
                errors.append("no top-level _rustique.py shim")
        if PTH not in names:
            errors.append(f"no {PTH} at the root")
        else:
            line = z.read(PTH).decode().strip()
            if "_rustique" not in line or "RUSTIQUE" not in line:
                errors.append(f"unexpected {PTH}: {line!r}")
        record = next((n for n in names if n.endswith(".dist-info/RECORD")), None)
        if record is None:
            errors.append("no RECORD")
        else:
            listed = {}
            for row in z.read(record).decode().splitlines():
                parts = row.rsplit(",", 2)
                if len(parts) == 3:
                    listed[parts[0]] = parts[1]
            for n in names:
                if n == record or n.endswith("/"):
                    continue
                h = listed.get(n)
                if not h:
                    errors.append(f"{n} not in RECORD")
                    continue
                algo, _, digest = h.partition("=")
                got = base64.urlsafe_b64encode(hashlib.new(algo, z.read(n)).digest()).rstrip(b"=").decode()
                if got != digest:
                    errors.append(f"{n}: RECORD hash mismatch")
            for n in listed:
                if n not in names:
                    errors.append(f"RECORD lists {n}, which is not in the wheel")
    return errors


def main() -> int:
    args = sys.argv[1:]
    if len(args) < 3 or args[0] != "--layout" or args[1] not in ("top", "shim"):
        print(__doc__)
        return 2
    layout, wheels = args[1], args[2:]
    bad = 0
    for w in wheels:
        errs = check(w, layout)
        print(f"{'ok  ' if not errs else 'FAIL'} {w} ({layout})")
        for e in errs:
            print(f"     {e}")
        bad += bool(errs)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
