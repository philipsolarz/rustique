"""``python -m rustique``: run a program with Rustique enabled, or inspect and verify it.

    python -m rustique [run] [options] script.py [args...]     run a program, Rustique enabled
    python -m rustique [run] [options] -m module [args...]
    python -m rustique [run] [options] -c command [args...]
    python -m rustique status [--json]                         what would be accelerated here, and why not
    python -m rustique report [options] -- <command...>        run a command, print the per-target report
    python -m rustique diff [options] -- <command...>          run a command plain and enabled, compare

See ``rustique._cli`` and docs/USAGE.md.
"""

from rustique._cli import main

if __name__ == "__main__":
    main()
