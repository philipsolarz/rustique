"""Prototype loader for handler swaps (benches/interp/rqeval): finds the core interpreter's
tail-call dispatch table and handler addresses in the running python's symbol table (PBS
binaries are non-PIE and not stripped) and swaps chosen opcode handlers for an _rqeval build's.

    import rqpatch; rqpatch.swap("/work/w2-interp/rqeval", "_rqeval_isinst", ["CALL_ISINSTANCE"])
"""
import importlib, os, subprocess, sys

_SYMS = None


def core_symbols():
    """The dispatch table and _TAIL_CALL_* addresses of the running binary. Parsing the
    33 MB symbol table takes ~0.5 s, so the result is cached per binary identity (a product
    would key it by the ELF build-id and ship it per known build)."""
    global _SYMS
    if _SYMS is None:
        import json
        exe = os.path.realpath(sys.executable)
        st = os.stat(exe)
        cache = os.path.join(os.environ.get("RQEVAL_DIR", "/work/w2-interp/rqeval"),
                             f"syms-{st.st_size}-{st.st_mtime_ns}-{st.st_ino}.json")
        try:
            with open(cache) as f:
                _SYMS = json.load(f)
            return _SYMS
        except OSError:
            pass
        out = subprocess.run(["nm", exe], capture_output=True, text=True, check=True).stdout
        syms = {}
        for line in out.splitlines():
            parts = line.split()
            if len(parts) == 3 and (parts[2].startswith("_TAIL_CALL_")
                                    or parts[2] == "instruction_funcptr_handler_table"):
                syms.setdefault(parts[2], int(parts[0], 16))
        _SYMS = syms
        try:
            tmp = cache + f".{os.getpid()}"
            with open(tmp, "w") as f:
                json.dump(syms, f)
            os.replace(tmp, cache)
        except OSError:
            pass
    return _SYMS


def opcode_of(name):
    import _opcode_metadata as m
    return m._specialized_opmap.get(name, m.opmap.get(name))


def swap(path, modname, opnames):
    if path not in sys.path:
        sys.path.insert(0, path)
    mod = importlib.import_module(modname)
    syms = core_symbols()
    table = syms["instruction_funcptr_handler_table"]
    done = []
    for name in opnames:
        op = opcode_of(name)
        expect = syms["_TAIL_CALL_" + name]
        old = mod.patch(table, op, expect)
        done.append((op, old))
    return mod, table, done


def unswap(mod, table, done):
    for op, old in done:
        mod.unpatch(table, op, old)


def all_opnames():
    import _opcode_metadata as m
    names = set(m.opmap) | set(m._specialized_opmap)
    return sorted(n for n in names if (m._specialized_opmap.get(n, m.opmap.get(n)) or 0) < 256)


JIT_OPS = {"ENTER_EXECUTOR", "JUMP_BACKWARD", "JUMP_BACKWARD_JIT", "JUMP_BACKWARD_NO_JIT",
           "RESUME_CHECK_JIT"}


def swap_all(path, modname, exclude=JIT_OPS):
    """Swap every opcode handler that the core table holds under its own name."""
    syms = core_symbols()
    names = [n for n in all_opnames() if n not in exclude and ("_TAIL_CALL_" + n) in syms]
    return swap(path, modname, names)
