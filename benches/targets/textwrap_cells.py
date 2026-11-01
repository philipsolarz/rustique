"""Callgrind cells for the `textwrap` group (benches/targets/utils_cg.py textwrap_cells)."""

COMMON = """
import textwrap
DESC = ' '.join(['alpha beta gamma delta epsilon release deploy rollback latency throughput'.split()[i % 10] for i in range(40)]) + '.'
TITLE = 'Incident report for the payments search platform rollout'
T540 = ('The quick brown fox jumps over the lazy dog; ' * 12)[:540]
SRC = '\\n'.join('    line %d of some indented source' % i for i in range(20))
LINES = '\\n'.join('line %d' % i for i in range(30))
TW = textwrap.TextWrapper(width=60)
class MyWrapper(textwrap.TextWrapper):
    def _split(self, text):
        return text.split()
MW = MyWrapper(width=60)
class S(str):
    pass
SS = S('subclass text that must fall back to CPython')
"""

INSTALL = """
import rustique
rustique.enable(only=["textwrap"])
"""

CELLS = {
    "fill(desc, 88, indents) [report_render]": "textwrap.fill(DESC, width=88, initial_indent='> ', subsequent_indent='> ')",
    "shorten(title, 30, placeholder) [report_render]": "textwrap.shorten(TITLE, 30, placeholder='...')",
    "indent(30 lines, '  ') [report_render]": "textwrap.indent(LINES, '  ')",
    "wrap(540 chars)": "textwrap.wrap(T540)",
    "fill(short)": "textwrap.fill('hello world')",
    "wrap(word)": "textwrap.wrap('word')",
    "dedent(20 lines)": "textwrap.dedent(SRC)",
    "dedent('x')": "textwrap.dedent('x')",
    "indent('x', '> ')": "textwrap.indent('x', '> ')",
    "TextWrapper.wrap(540)": "TW.wrap(T540)",
    "TextWrapper.fill(desc)": "TW.fill(DESC)",
    # fallbacks
    "wrap(str subclass) [fallback]": "textwrap.wrap(SS, 20)",
    "subclass with _split [fallback]": "MW.wrap(T540)",
    "indent(predicate) [fallback]": "textwrap.indent(LINES, '> ', str.strip)",
    "dedent(str subclass) [fallback]": "textwrap.dedent(SS)",
}
