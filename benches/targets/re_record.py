"""Record what the `re` group validates per CPython minor version (run once per version,
paste into `crates/rustique-py/src/targets/re/fps.rs`):

    python benches/targets/re_record.py

Prints the fingerprint of every Python function the group's kernels stand in for (the
whole call graph of `re._compiler.compile` on its success path, the module functions and
`_compile`'s hit path, `parse_template`) and the content hash of `_casefix._EXTRA_CASES`.
"""

from __future__ import annotations

import re
import sys

from rustique import _rustique

A = _rustique._accel
P = re._parser
CO = re._compiler

NAMES = {
    "re._compiler.compile": CO.compile,
    "re._compiler.isstring": CO.isstring,
    "re._compiler._code": CO._code,
    "re._compiler._compile_info": CO._compile_info,
    "re._compiler._compile": CO._compile,
    "re._compiler._compile_charset": CO._compile_charset,
    "re._compiler._optimize_charset": CO._optimize_charset,
    "re._compiler._mk_bitmap": CO._mk_bitmap,
    "re._compiler._bytes_to_codes": CO._bytes_to_codes,
    "re._compiler._simple": CO._simple,
    "re._compiler._generate_overlap_table": CO._generate_overlap_table,
    "re._compiler._get_iscased": CO._get_iscased,
    "re._compiler._get_literal_prefix": CO._get_literal_prefix,
    "re._compiler._get_charset_prefix": CO._get_charset_prefix,
    "re._compiler._combine_flags": CO._combine_flags,
    "re._parser.parse": P.parse,
    "re._parser.fix_flags": P.fix_flags,
    "re._parser._parse_sub": P._parse_sub,
    "re._parser._parse": P._parse,
    "re._parser._parse_flags": P._parse_flags,
    "re._parser._escape": P._escape,
    "re._parser._class_escape": P._class_escape,
    "re._parser._uniq": P._uniq,
    "re._parser.parse_template": P.parse_template,
    "re._parser.Tokenizer.__init__": P.Tokenizer.__dict__["__init__"],
    "re._parser.Tokenizer.__next": P.Tokenizer.__dict__["_Tokenizer__next"],
    "re._parser.Tokenizer.match": P.Tokenizer.__dict__["match"],
    "re._parser.Tokenizer.get": P.Tokenizer.__dict__["get"],
    "re._parser.Tokenizer.getwhile": P.Tokenizer.__dict__["getwhile"],
    "re._parser.Tokenizer.getuntil": P.Tokenizer.__dict__["getuntil"],
    "re._parser.Tokenizer.pos": P.Tokenizer.__dict__["pos"].fget,
    "re._parser.Tokenizer.tell": P.Tokenizer.__dict__["tell"],
    "re._parser.Tokenizer.seek": P.Tokenizer.__dict__["seek"],
    "re._parser.Tokenizer.checkgroupname": P.Tokenizer.__dict__["checkgroupname"],
    "re._parser.State.__init__": P.State.__dict__["__init__"],
    "re._parser.State.groups": P.State.__dict__["groups"].fget,
    "re._parser.State.opengroup": P.State.__dict__["opengroup"],
    "re._parser.State.closegroup": P.State.__dict__["closegroup"],
    "re._parser.State.checkgroup": P.State.__dict__["checkgroup"],
    "re._parser.State.checklookbehindgroup": P.State.__dict__["checklookbehindgroup"],
    "re._parser.SubPattern.__init__": P.SubPattern.__dict__["__init__"],
    "re._parser.SubPattern.__len__": P.SubPattern.__dict__["__len__"],
    "re._parser.SubPattern.__delitem__": P.SubPattern.__dict__["__delitem__"],
    "re._parser.SubPattern.__getitem__": P.SubPattern.__dict__["__getitem__"],
    "re._parser.SubPattern.__setitem__": P.SubPattern.__dict__["__setitem__"],
    "re._parser.SubPattern.insert": P.SubPattern.__dict__["insert"],
    "re._parser.SubPattern.append": P.SubPattern.__dict__["append"],
    "re._parser.SubPattern.getwidth": P.SubPattern.__dict__["getwidth"],
    "re.match": re.match,
    "re.fullmatch": re.fullmatch,
    "re.search": re.search,
    "re.sub": re.sub,
    "re.subn": re.subn,
    "re.split": re.split,
    "re.findall": re.findall,
    "re.finditer": re.finditer,
    "re.compile": re.compile,
    "re._compile": re._compile,
    "re._compile_template": re._compile_template.__wrapped__,
}


def extra_cases_hash(d):
    """FNV-1a (64-bit) over the little-endian u32 words k, len(v), *v of the sorted items
    (the group computes the same at install)."""
    h = 0xCBF29CE484222325
    for k, v in sorted(d.items()):
        for w in (k, len(v), *v):
            for b in int(w).to_bytes(4, "little"):
                h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return "%016x" % h


def main() -> None:
    major, minor = sys.version_info[:2]
    for name, obj in NAMES.items():
        print(f'    ("{name}", "{A.fingerprint(obj)}"),  // ({major}, {minor})')
    print(f'    EXTRA_CASES ({major}, {minor}): "{extra_cases_hash(CO._EXTRA_CASES)}" ({len(CO._EXTRA_CASES)} entries)')


if __name__ == "__main__":
    main()
