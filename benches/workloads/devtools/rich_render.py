"""rich rendering a CLI report to a string console: table, markdown, syntax, tree, pretty.

Stands for the terminal output layer of modern CLIs (pip, pytest plugins, typer apps,
httpie, textual) that render through rich: cell-width measurement and wrapping of styled
text (`Text`, `Segment`), style parsing/combining, a 600-row table, a Markdown document
(markdown-it-py parse + render), a Pygments-highlighted ~950-line source listing, a
directory-style tree and `rich.pretty` of a nested structure. Output goes to an in-memory
truecolor console of fixed width, so it is deterministic and exercises the ANSI encoder.
"""

import hashlib
import io
import random

REQUIRES = ["rich==15.0.0", "markdown-it-py==4.2.0", "pygments==2.21.0"]

MD_SECTION = """\
## Section {i}: configuring `tool-{i}`

The *tool* reads **settings** from `pyproject.toml`, then the environment. See
[the docs](https://example.org/{i}) for details. Options:

1. `--fast` skips the safety check.
2. `--line-length N` wraps at *N* columns (default **88**).
3. `--target-version` accepts `py3{i}` and newer.

> Note: values given on the command line override the file.

| option | type | default |
|--------|------|---------|
| fast | bool | false |
| line-length | int | 88 |
| exclude | regex | `/(\\.git|build)/` |

```python
def configure(path: str = "pyproject.toml") -> dict[str, object]:
    with open(path, "rb") as f:
        return tomllib.load(f)["tool"]["tool-{i}"]
```
"""


def setup():
    import sysconfig
    import os
    import rich.console  # noqa: F401
    import rich.markdown  # noqa: F401
    import rich.syntax  # noqa: F401

    rng = random.Random(7)
    rows = [(f"pkg-{i:03d}", f"{rng.randint(0, 9)}.{rng.randint(0, 30)}.{rng.randint(0, 9)}",
             rng.choice(["ok", "outdated", "[red]vulnerable[/red]", "[yellow]yanked[/yellow]"]),
             f"{rng.random() * 1000:.1f} kB", "A longer description of package number %d that wraps" % i)
            for i in range(600)]
    markdown = "# Tool configuration\n\n" + "\n".join(MD_SECTION.format(i=i) for i in range(24))
    with open(os.path.join(sysconfig.get_path("stdlib"), "textwrap.py"), encoding="utf-8") as f:
        code = f.read()
    code = code + "\n" + code  # ~950 lines
    data = {"name": "report", "items": [{"id": i, "tags": [f"t{j}" for j in range(i % 5)],
                                         "score": i * 1.5, "ok": i % 3 == 0, "meta": None}
                                        for i in range(150)]}
    return rows, markdown, code, data


def run(state):
    from rich.console import Console
    from rich.markdown import Markdown
    from rich.pretty import Pretty
    from rich.syntax import Syntax
    from rich.table import Table
    from rich.tree import Tree

    rows, markdown, code, data = state
    out = io.StringIO()
    console = Console(file=out, width=110, color_system="truecolor", force_terminal=True,
                      legacy_windows=False, record=False, emoji=True, highlight=True)
    table = Table(title="Dependency report", show_lines=False)
    for col, kw in (("package", {"style": "cyan"}), ("version", {"justify": "right"}),
                    ("status", {}), ("size", {"justify": "right"}), ("description", {"ratio": 1})):
        table.add_column(col, **kw)
    for row in rows:
        table.add_row(*row)
    console.print(table)
    console.print(Markdown(markdown))
    console.print(Syntax(code, "python", theme="monokai", line_numbers=True, word_wrap=True))
    tree = Tree("[bold]project")
    for i in range(40):
        branch = tree.add(f"[green]src/pkg{i}")
        for j in range(5):
            branch.add(f"module_{j}.py [dim]{(i * 7 + j) % 300} lines")
    console.print(tree)
    console.print(Pretty(data))
    return out.getvalue()


def digest(result):
    import re

    # rich gives every hyperlink style a random OSC-8 `id=`; drop it
    return hashlib.sha256(re.sub("\x1b\\]8;id=[^;]*;", "\x1b]8;;", result).encode()).hexdigest()
