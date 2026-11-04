"""One representative small-input cell per target (84) plus entry-floor references, for
benches/floors/cg_breakdown.py (docs/atlas/w2-floors.md §1).

    python benches/floors/floor_cells.py > /tmp/cells.json          # plain + enabled
    python benches/floors/cg_breakdown.py --cells /tmp/cells.json -o floors-3.15.1.json -j 6

Setups are the targets' own benches (benches/targets/*_cg.py, *_cells.py), so the cells
match the target docs' headline cells.
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "targets"))

import copy_cells, containers, datetime_cg, json_cg, logging_cg, paths_cg  # noqa: E401,E402
import random_cells, statistics_cells, textwrap_cells, urllib_cells  # noqa: E401,E402

SELFTEST_SRC = '''DEP = object()
SEP = "/"
class K:
    attr = 1000
def ident(x):
    return ("orig", x)
def same(x):
    return x
'''
SELFTEST = (
    "import sys, os, tempfile\n_d = tempfile.mkdtemp()\n"
    f"open(os.path.join(_d, '_rustique_selftest.py'), 'w').write({SELFTEST_SRC!r})\n"
    "sys.path.insert(0, _d)\nimport _rustique_selftest as _st\nsame = _st.same\nident = _st.ident\nx = 'abc'\n"
    "import operator\nidx = operator.index\nn5 = 5\n"
)

S_JSON = json_cg.COMMON + "\nenc = json.JSONEncoder()\n"
S_COPY = copy_cells.SETUP
S_CONT = containers.SETUP + "\nfrom operator import itemgetter\npairs = [('b', 2), ('a', 1), ('c', 3), ('d', 0)]\n"
S_DT = datetime_cg.COMMON
S_PATHS = paths_cg.COMMON + "\npq = PurePosixPath(p)\n"
S_LOG = logging_cg.COMMON
S_RAND = random_cells.COMMON
S_STAT = statistics_cells.COMMON
S_TW = textwrap_cells.COMMON
S_URL = urllib_cells.COMMON
S_DC = S_COPY

# target -> (group(s) to enable, setup, statement, what the cell is)
CELLS = {
    # entry floors (not targets)
    "_floor.vectorcall same(x)": ("_selftest.same", SELFTEST, "same(x)", "exact identity kernel on a vectorcall site"),
    "_floor.meth_o operator.index(5)": (None, SELFTEST, "idx(n5)", "plain METH_O C call returning its argument"),
    "_floor.vectorcall ident(x)": ("_selftest.vectorcall", SELFTEST, "ident(x)", "vectorcall kernel returning a fresh 2-tuple"),
    # containers
    "list.sort": ("containers", S_CONT, "ints[:].sort()", "8 ints (copy included)"),
    "list.index": ("containers", S_CONT, "ints.index(7)", "8 ints"),
    "list.count": ("containers", S_CONT, "ints.count(3)", "8 ints"),
    "tuple.index": ("containers", S_CONT, "t.index(1)", "4 ints"),
    "tuple.count": ("containers", S_CONT, "t.count(9)", "4 ints"),
    "list.__contains__": ("containers", S_CONT, "4 in ints", "miss, 8 ints"),
    "tuple.__contains__": ("containers", S_CONT, "9 in t", "4 ints"),
    "builtins.sum": ("builtins", S_CONT, "sum(small)", "3 ints"),
    "builtins.min": ("builtins", S_CONT, "min(ints)", "8 ints"),
    "builtins.max": ("builtins", S_CONT, "max(ints)", "8 ints"),
    "builtins.sorted": ("builtins,containers", S_CONT, "sorted(ints)", "8 ints (sorted -> list.sort kernel)"),
    "builtins.any": ("builtins", S_CONT, "any(small)", "3 ints"),
    "builtins.all": ("builtins", S_CONT, "all(strs)", "4 strs"),
    # float, json
    "float.repr": ("float", S_JSON, "repr(x)", "repr(1.23)"),
    "json.dumps": ("float,json", S_JSON, "dumps(record)", "10-field record"),
    "json.loads": ("float,json", S_JSON, "loads(rec_text)", "10-field record"),
    # copy, dataclasses
    "copy.deepcopy": ("copy,dataclasses", S_COPY, "deepcopy(rec)", "10-field dict"),
    "copy.copy": ("copy,dataclasses", S_COPY, "copy.copy(p)", "small dataclass"),
    "dataclasses.asdict": ("copy,dataclasses", S_DC, "dataclasses.asdict(p)", "3-field dataclass"),
    "dataclasses.astuple": ("copy,dataclasses", S_DC, "dataclasses.astuple(p)", "3-field dataclass"),
    # datetime
    "datetime.datetime.strptime": ("datetime", S_DT, "datetime.strptime(LOG, LOGF)", "access-log format"),
    "datetime.date.strptime": ("datetime", S_DT, "date.strptime('2024-03-15', '%Y-%m-%d')", "ISO date"),
    "datetime.time.strptime": ("datetime", S_DT, "D.time.strptime('13:45:30', '%H:%M:%S')", "H:M:S"),
    "time.strptime": ("datetime", S_DT, "time.strptime(ISO, ISOF)", "ISO datetime"),
    "datetime.date.strftime": ("datetime", S_DT, "dt.strftime('%Y-%m-%d %H:%M:%S')", "datetime, numeric"),
    "datetime.time.strftime": ("datetime", S_DT, "t0.strftime('%H:%M:%S.%f')", "time"),
    "time.strftime": ("datetime", S_DT, "time.strftime('%Y-%m-%d %H:%M:%S', st)", "struct_time"),
    "datetime.datetime.isoformat": ("datetime", S_DT, "dt.isoformat()", "naive, microseconds"),
    "datetime.date.isoformat": ("datetime", S_DT, "d0.isoformat()", "date"),
    "datetime.time.isoformat": ("datetime", S_DT, "t0.isoformat()", "time"),
    # posixpath, fnmatch, pathlib
    "posixpath.join": ("posixpath", S_PATHS, "join(d, f)", "two components"),
    "posixpath.split": ("posixpath", S_PATHS, "split(p)", ""),
    "posixpath.splitext": ("posixpath", S_PATHS, "splitext(f)", ""),
    "posixpath.basename": ("posixpath", S_PATHS, "basename(p)", ""),
    "posixpath.dirname": ("posixpath", S_PATHS, "dirname(p)", ""),
    "posixpath.isabs": ("posixpath", S_PATHS, "isabs(p)", ""),
    "posixpath.relpath": ("posixpath", S_PATHS, "relpath(p, root)", ""),
    "posixpath.abspath": ("posixpath", S_PATHS, "abspath(p)", "absolute input"),
    "fnmatch.fnmatch": ("fnmatch", S_PATHS, "fnmatch_(f, '*.py')", ""),
    "fnmatch.fnmatchcase": ("fnmatch", S_PATHS, "fnmatchcase(f, '*.py')", ""),
    "fnmatch.filter": ("fnmatch", S_PATHS, "ffilter(names, '*.py')", "32 names"),
    "pathlib.PurePath.__new__": ("pathlib", S_PATHS, "PurePosixPath(d, f)", "with __init__"),
    "pathlib.Path.__new__": ("pathlib", S_PATHS, "Path(p)", "with __init__"),
    "pathlib.PurePath.__init__": ("pathlib", S_PATHS, "Path(p)", "same cell as Path.__new__"),
    "pathlib.PurePath.__str__": ("pathlib", S_PATHS, "str(pp)", "cached str"),
    "pathlib.PurePath.__truediv__": ("pathlib", S_PATHS, "pp / f", ""),
    "pathlib.PurePath.parent": ("pathlib", S_PATHS, "pp.parent", ""),
    "pathlib.PurePath.name": ("pathlib", S_PATHS, "pp.name", ""),
    "pathlib.PurePath.suffix": ("pathlib", S_PATHS, "pp.suffix", ""),
    "pathlib.PurePath.stem": ("pathlib", S_PATHS, "pp.stem", ""),
    "pathlib.PurePath.parts": ("pathlib", S_PATHS, "pp.parts", ""),
    "pathlib.PurePath.relative_to": ("pathlib", S_PATHS, "pp.relative_to(root)", ""),
    "pathlib.PurePath.__eq__": ("pathlib", S_PATHS, "pp == pq", ""),
    "pathlib.PurePath.__hash__": ("pathlib", S_PATHS, "hash(pp)", "cached"),
    # logging
    "logging.Logger.level": ("logging", S_LOG, "L_OFF.debug('x=%s', v)", "disabled call"),
    "logging.Logger.findCaller": ("logging", S_LOG, "L_TIME.findCaller()", ""),
    "logging.LogRecord.__init__": ("logging", S_LOG, logging_cg.CELLS["LogRecord()"], ""),
    "logging.Formatter.formatTime": ("logging", S_LOG, "F_TIME.formatTime(rec)", ""),
    "logging.Formatter.format": ("logging", S_LOG, "F_TIME.format(rec)", "asctime format"),
    # random
    "random.randint": ("random", S_RAND, "rng.randint(3, 7)", ""),
    "random.randrange": ("random", S_RAND, "rng.randrange(10)", ""),
    "random.choice": ("random", S_RAND, "rng.choice(L6)", ""),
    "random.shuffle": ("random", S_RAND, "rng.shuffle(L100)", "100 items"),
    "random.sample": ("random", S_RAND, "rng.sample(W, 5)", ""),
    "random.choices": ("random", S_RAND, "rng.choices(T4, weights=[50, 30, 15, 5])", ""),
    # statistics
    "statistics.mean": ("statistics", S_STAT, "S.mean(F10)", "10 floats"),
    "statistics.variance": ("statistics", S_STAT, "S.variance(F10)", "10 floats"),
    "statistics.pvariance": ("statistics", S_STAT, "S.pvariance(F10)", "10 floats"),
    "statistics.stdev": ("statistics", S_STAT, "S.stdev(F10)", "10 floats"),
    "statistics.pstdev": ("statistics", S_STAT, "S.pstdev(I10)", "10 ints"),
    # textwrap
    "textwrap.wrap": ("textwrap", S_TW, "textwrap.wrap('hello world')", "short"),
    "textwrap.fill": ("textwrap", S_TW, textwrap_cells.CELLS["fill(desc, 88, indents) [report_render]"], "~330 chars"),
    "textwrap.shorten": ("textwrap", S_TW, textwrap_cells.CELLS["shorten(title, 30, placeholder) [report_render]"], ""),
    "textwrap.dedent": ("textwrap", S_TW, "textwrap.dedent(SRC)", "20 lines"),
    "textwrap.indent": ("textwrap", S_TW, "textwrap.indent(LINES, '  ')", "30 lines"),
    "textwrap.TextWrapper.wrap": ("textwrap", S_TW, "TW.wrap(T540)", "540 chars"),
    "textwrap.TextWrapper.fill": ("textwrap", S_TW, "TW.fill(DESC)", ""),
    # urllib
    "urllib.parse.quote": ("urllib", S_URL, "quote(P)", ""),
    "urllib.parse.quote_plus": ("urllib", S_URL, "up.quote_plus('a b&c')", ""),
    "urllib.parse.unquote": ("urllib", S_URL, "unquote('a%20b%C3%A9')", ""),
    "urllib.parse.unquote_plus": ("urllib", S_URL, "up.unquote_plus('a+b')", ""),
    "urllib.parse.parse_qsl": ("urllib", S_URL, "parse_qsl(Q)", "5 fields"),
    "urllib.parse.parse_qs": ("urllib", S_URL, "parse_qs(Q)", "5 fields"),
    "urllib.parse._urlsplit": ("urllib", S_URL, "up._urlsplit(URL)", ""),
}


def cells(variants=("plain", "on")):
    out = []
    for name, (groups, setup, stmt, _what) in CELLS.items():
        for v in variants:
            if v == "on" and groups is None:
                continue
            out.append({"name": f"{name} [{v}]", "stmt": stmt, "setup": setup,
                        "enable": groups.split(",") if v == "on" else None})
    return out


if __name__ == "__main__":
    json.dump(cells(), sys.stdout, indent=1)
