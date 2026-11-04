"""The engine's own surface (docs/MIGRATION.md, docs/DECISIONS.md §10.1 and §10.7): the
public API after the native types' retirement, the pointers left for removed names, the
extension's two names and layouts (top-level `_rustique` in release wheels, the shim of the
in-package layout), and the wheel repack script (tools/repack_wheel.py).
"""

from __future__ import annotations

import base64
import hashlib
import importlib.util
import os
import shutil
import subprocess
import sys
import textwrap
import zipfile

import pytest

import rustique

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PYSRC = os.path.join(ROOT, "python")
EXT = rustique._rustique.__file__
SUFFIX = os.path.basename(EXT)[len("_rustique"):]

REMOVED = ["List", "Dict", "Set", "Tuple", "FrozenSet", "ListType", "DictType", "SetType", "FrozenSetType",
           "Expr", "X", "arange", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "native"]


def py(code: str, *args: str, env: dict[str, str] | None = None, isolated_site: bool = False,
       path_only: str | None = None):
    """Run `code` in a fresh interpreter. `isolated_site`: without `site` (`-S`), with only the
    `rustique` package's directory on sys.path, so that no installed `.pth` runs (a test
    site directory is then the only place the startup line comes from). `path_only`: without
    `site`, with only that directory on sys.path."""
    e = {k: v for k, v in os.environ.items() if not k.startswith(("RUSTIQUE", "PYTHONPATH"))}
    flags = []
    if isolated_site or path_only:
        flags = ["-S"]
        e["PYTHONPATH"] = path_only or os.path.dirname(os.path.dirname(rustique.__file__))
    e.update(env or {})
    return subprocess.run([sys.executable, *flags, "-c", textwrap.dedent(code), *args], capture_output=True,
                          text=True, env=e, timeout=120)


# --- the API after 0.3 ------------------------------------------------------------------------


def test_public_api():
    assert rustique.__all__ == ["RustiqueCheckError", "__version__", "check", "disable", "enable", "stats", "status"]
    assert rustique.__version__ == rustique._rustique.__version__ == "0.3.0"
    assert issubclass(rustique.RustiqueCheckError, AssertionError)
    assert not set(REMOVED) & set(dir(rustique))


@pytest.mark.parametrize("name", REMOVED)
def test_removed_names_point_to_the_migration_note(name):
    with pytest.raises(AttributeError) as e:
        getattr(rustique, name)
    msg = str(e.value)
    assert "\n" not in msg and msg.startswith(f"module 'rustique' has no attribute {name!r}: ")
    assert "rustique<0.3" in msg and msg.endswith("docs/MIGRATION.md")
    assert e.value.name == name
    assert not hasattr(rustique, name)


def test_other_missing_names_are_plain():
    with pytest.raises(AttributeError, match=r"^module 'rustique' has no attribute 'nope'$"):
        rustique.nope  # noqa: B018


def test_from_import_of_a_removed_name():
    r = py("from rustique import List")
    assert r.returncode == 1 and "ImportError: cannot import name 'List' from 'rustique'" in r.stderr


def test_enable_native_is_refused():
    with pytest.raises(TypeError, match=r"native= was removed in Rustique 0\.3; see .*MIGRATION\.md$"):
        rustique.enable(native="mypkg.*")
    with pytest.raises(TypeError, match="native= was removed"):
        rustique._enable(None, None, ["mypkg"])


def test_extension_surface():
    ext = rustique._rustique
    assert sorted(n for n in dir(ext) if not n.startswith("__")) == ["_accel", "_builtins", "_takeover", "startup"]
    for name in ("List", "X", "i64", "_capi", "_schema_repr"):
        with pytest.raises(AttributeError, match=f"^module '_rustique' has no attribute '{name}'$"):
            getattr(ext, name)


def test_takeover_package_has_no_native_tier():
    from rustique import _takeover

    assert _takeover.__all__ == ["RustiqueCheckError", "check", "disable", "enable", "stats", "status"]
    with pytest.raises(TypeError):
        _takeover.enable(native="x")  # type: ignore[call-arg]
    for mod in ("rustique._takeover._hook", "rustique._takeover._rewrite"):
        assert importlib.util.find_spec(mod) is None


# --- one module, two names, every layout --------------------------------------------------


def test_both_names_are_one_module():
    assert sys.modules["_rustique"] is sys.modules["rustique._rustique"] is rustique._rustique


def test_a_second_load_of_the_file_returns_the_first_module():
    r = py(
        """
        import importlib.util, sys
        import rustique
        first = rustique._rustique
        for name in ('_rustique', 'rustique._rustique'):
            spec = importlib.util.spec_from_file_location(name, first.__file__)
            again = sys.modules['_frozen_importlib']._load(spec)
            assert again is first and sys.modules[name] is first, name
        del sys.modules['_rustique'], sys.modules['rustique._rustique']
        import _rustique
        assert _rustique is first and sys.modules['rustique._rustique'] is first
        print('ok')
        """
    )
    assert r.returncode == 0 and r.stdout == "ok\n", r.stderr


PROBE = """
import site, sys
sys.path.insert(0, sys.argv[1])
site.addsitedir(sys.argv[1])
import json, os
ext = sys.modules.get('_rustique')
out = {'loaded': sorted(m for m in sys.modules if 'rustique' in m)}
if ext is not None:
    out.update(name=ext.__name__, file=os.path.relpath(ext.__file__, sys.argv[1]),
               same=sys.modules.get('rustique._rustique') is ext,
               source=ext._accel.status()['process']['switch']['source'])
import rustique
out['package_reuses'] = rustique._rustique is ext
print(json.dumps(out))
"""


def _site(tmp_path, layout: str) -> str:
    d = tmp_path / "site"
    d.mkdir()
    shutil.copy(os.path.join(PYSRC, "_rustique_startup.pth"), d)
    if layout == "top-level":  # a repacked release wheel
        os.symlink(EXT, d / f"_rustique{SUFFIX}")
    else:  # maturin's in-package layout with the shim (a wheel built from the sdist)
        shutil.copy(os.path.join(PYSRC, "_rustique.py"), d)
        (d / "rustique").mkdir()
        os.symlink(EXT, d / "rustique" / f"_rustique{SUFFIX}")
    return str(d)


@pytest.mark.parametrize(("layout", "name", "file"), [
    ("top-level", "_rustique", f"_rustique{SUFFIX}"),
    ("in-package", "rustique._rustique", os.path.join("rustique", f"_rustique{SUFFIX}")),
])
def test_startup_in_each_wheel_layout(tmp_path, layout, name, file):
    import json

    r = py(PROBE, _site(tmp_path, layout), env={"RUSTIQUE": "1"}, isolated_site=True)
    assert r.returncode == 0 and r.stderr == "", r.stderr
    out = json.loads(r.stdout)
    assert out["loaded"] == ["_rustique", "rustique._rustique"]  # not the package
    assert (out["name"], out["file"], out["same"]) == (name, file, True)
    assert out["source"] == "RUSTIQUE=1" and out["package_reuses"]


@pytest.mark.parametrize("layout", ["top-level", "in-package"])
@pytest.mark.parametrize("value", ["off", "", "No"])
def test_off_values_leave_nothing_loaded_in_each_layout(tmp_path, layout, value):
    import json

    r = py(PROBE, _site(tmp_path, layout), env={"RUSTIQUE": value}, isolated_site=True)
    assert r.returncode == 0 and r.stderr == "", r.stderr
    out = json.loads(r.stdout)
    assert out["loaded"] == [] and out["package_reuses"] is False  # `import rustique` loads it afresh


def test_shim_without_an_extension_raises_importerror(tmp_path):
    d = tmp_path / "site"
    (d / "rustique").mkdir(parents=True)
    shutil.copy(os.path.join(PYSRC, "_rustique.py"), d)
    r = py("import sys; sys.path.insert(0, sys.argv[1]); import _rustique", str(d), isolated_site=True)
    assert r.returncode == 1 and "ImportError: rustique: no extension module in" in r.stderr


def test_import_errors_of_the_extension_are_not_masked(tmp_path):
    # A refusal (subinterpreters) or a broken build raises ImportError from `import
    # _rustique`: `import rustique` reports that error, not a fallback's.
    (tmp_path / "_rustique.py").write_text("raise ImportError('boom: the extension refused')\n")
    r = py("import sys; sys.path.insert(0, sys.argv[1]); import rustique", str(tmp_path), isolated_site=True)
    assert r.returncode == 1 and r.stderr.rstrip().endswith("ImportError: boom: the extension refused"), r.stderr


def test_package_relative_fallback_when_no_top_level_module(tmp_path):
    # A tree with the package but neither the shim nor a top-level extension.
    pkg = tmp_path / "rustique"
    shutil.copytree(os.path.dirname(rustique.__file__), pkg, ignore=shutil.ignore_patterns("__pycache__", "_rustique*"))
    os.symlink(EXT, pkg / f"_rustique{SUFFIX}")
    r = py("import sys, rustique; print(rustique._rustique.__name__, rustique._rustique is sys.modules['_rustique'])",
           path_only=str(tmp_path))
    assert r.returncode == 0 and r.stdout.split() == ["rustique._rustique", "True"], r.stderr


# --- the wheel repack (tools/repack_wheel.py) ------------------------------------------------

_spec = importlib.util.spec_from_file_location("repack_wheel", os.path.join(ROOT, "tools", "repack_wheel.py"))
repack = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(repack)

DIST = "rustique-0.3.0.dist-info"


def _hash(data: bytes) -> str:
    return "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()


def make_wheel(path, files: dict[str, bytes]) -> str:
    files = {**files, f"{DIST}/WHEEL": b"Wheel-Version: 1.0\nGenerator: maturin\nRoot-Is-Purelib: false\n",
             f"{DIST}/METADATA": b"Metadata-Version: 2.4\nName: rustique\nVersion: 0.3.0\n"}
    rows = [f"{n},{_hash(d)},{len(d)}" for n, d in files.items()] + [f"{DIST}/RECORD,,"]
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for n, d in files.items():
            info = zipfile.ZipInfo(n, date_time=(2026, 11, 2, 0, 0, 0))
            info.external_attr = (0o755 if n.endswith(".so") else 0o644) << 16
            z.writestr(info, d)
        z.writestr(f"{DIST}/RECORD", "\n".join(rows) + "\n")
    return str(path)


SO = "_rustique.cpython-315-x86_64-linux-gnu.so"
MATURIN = {f"rustique/{SO}": b"\x7fELF extension bytes", "rustique/__init__.py": b"import _rustique\n",
           "_rustique.py": b"# shim\n", "_rustique_startup.pth": b"import os\n"}


def test_repack_moves_the_extension_and_rewrites_record(tmp_path):
    whl = make_wheel(tmp_path / "rustique-0.3.0-cp315-cp315-manylinux_2_28_x86_64.whl", MATURIN)
    assert repack.main([whl]) == 0
    with zipfile.ZipFile(whl) as z:
        names = z.namelist()
        assert SO in names and f"rustique/{SO}" not in names and "_rustique.py" not in names
        assert names[-1] == f"{DIST}/RECORD"
        assert z.read(SO) == MATURIN[f"rustique/{SO}"]
        assert z.getinfo(SO).external_attr >> 16 == 0o755
        rows = z.read(f"{DIST}/RECORD").decode().splitlines()
        assert f"{SO},{_hash(z.read(SO))},{len(z.read(SO))}" in rows and f"{DIST}/RECORD,," in rows
        assert {r.split(",")[0] for r in rows} == set(names)
    repack.verify(whl)
    assert repack.main([whl]) == 0  # idempotent: already top-level


def test_repack_to_an_output_directory(tmp_path):
    whl = make_wheel(tmp_path / "rustique-0.3.0-cp314-cp314-win_amd64.whl",
                     {"rustique/_rustique.cp314-win_amd64.pyd": b"MZ", "rustique/__init__.py": b"", "_rustique.py": b""})
    before = open(whl, "rb").read()
    assert repack.main([whl, "-o", str(tmp_path / "out")]) == 0
    assert open(whl, "rb").read() == before
    out = tmp_path / "out" / os.path.basename(whl)
    with zipfile.ZipFile(out) as z:
        assert "_rustique.cp314-win_amd64.pyd" in z.namelist()


@pytest.mark.parametrize("files", [
    {"rustique/__init__.py": b""},  # no extension
    {k: v for k, v in MATURIN.items() if k != "_rustique.py"} | {"rustique.libs/libfoo.so": b""},  # grafted, no shim
])
def test_repack_refuses_what_it_cannot_move(tmp_path, files, capsys):
    whl = make_wheel(tmp_path / "rustique-0.3.0-cp315-cp315-linux_x86_64.whl", files)
    before = open(whl, "rb").read()
    assert repack.main([whl]) == 1
    assert open(whl, "rb").read() == before and "error:" in capsys.readouterr().err


def test_repack_keeps_maturins_layout_when_a_repair_tool_grafted_libraries(tmp_path, capsys):
    """musl wheels carry libgcc_s (Rust's unwinder) in `rustique.libs/`, found through the
    extension's `$ORIGIN`-relative search path: moving the extension would break it, so the
    wheel keeps the in-package extension and the `_rustique.py` shim."""
    whl = make_wheel(tmp_path / "rustique-0.3.0-cp315-cp315-musllinux_1_2_x86_64.whl",
                     {**MATURIN, "rustique.libs/libgcc_s-f685abf1.so.1": b""})
    before = open(whl, "rb").read()
    assert repack.main([whl]) == 0
    assert open(whl, "rb").read() == before and "kept maturin's layout" in capsys.readouterr().out
