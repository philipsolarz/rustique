"""Install cost of the `re` group (ACCELERATION §3.12, T9): callgrind instructions of the lazy
install at the end of `import re`, against `import re` itself (fresh process each):

    import re                       (rustique imported, nothing enabled)
    import re, group enabled        (enable(only=["re"]) first: re installs lazily at import)
    enable(only=["re"]) with re already imported (eager install)

    python benches/targets/re_install.py
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

PRE = "import sys, rustique\nassert 're' not in sys.modules, 'rustique imports re'\n"


def main() -> None:
    plain = region("import re", PRE)
    lazy = region("import re", PRE + "rustique.enable(only=['re'])\n")
    eager_base = region("pass", PRE + "import re\n")
    eager = region("rustique.enable(only=['re'])", PRE + "import re\n")
    enable_empty = region("rustique.enable(only=['re'])", PRE)
    print(f"import re (plain)                  {plain:12,d}")
    print(f"import re (re group enabled)       {lazy:12,d}  install {lazy - plain:,d} "
          f"= {100 * (lazy - plain) / plain:.1f}% of import re")
    print(f"enable(only=re), re imported       {eager - eager_base:12,d}")
    print(f"enable(only=re), re not imported   {enable_empty:12,d}")


if __name__ == "__main__":
    main()
