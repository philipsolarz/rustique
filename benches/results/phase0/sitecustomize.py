# pyperformance injection (docs/BENCHMARKS.md): on PYTHONPATH of every benchmark process;
# calls rustique.enable() when RUSTIQUE_ENABLE=1, does nothing otherwise. Fails loudly if
# rustique cannot be imported (a silently plain "enabled" run would be worthless).
import os

if os.environ.get("RUSTIQUE_ENABLE") == "1":
    import rustique

    rustique.enable()
    if os.environ.get("RUSTIQUE_SITE_LOG"):
        import sys

        with open(os.environ["RUSTIQUE_SITE_LOG"], "a") as _f:
            _f.write(f"{os.getpid()} {sys.executable} {' '.join(sys.argv[:2])}\n")
