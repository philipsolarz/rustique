"""Rustique: Python's data model, Rust's memory.

Typed, natively stored ``List``/``Dict``/``Set`` containers with builtin-compatible
semantics, vectorised expressions over the element placeholder ``X``, and fixed-width
scalar types. See ``docs/ARCHITECTURE.md``.
"""

from collections.abc import MutableMapping, MutableSequence, MutableSet

from rustique._rustique import (
    Dict,
    DictType,
    Expr,
    List,
    ListType,
    Set,
    SetType,
    X,
    __version__,
    arange,
    f32,
    f64,
    i8,
    i16,
    i32,
    i64,
    u8,
    u16,
    u32,
    u64,
)

# The takeover (docs/TAKEOVER.md). Not in `__all__` yet: tests/test_interop.py pins it and
# other workstreams extend it in parallel; the lead adds these names at integration.
from rustique._takeover import disable, enable, native

__all__ = [
    "Dict",
    "DictType",
    "Expr",
    "List",
    "ListType",
    "Set",
    "SetType",
    "X",
    "__version__",
    "arange",
    "f32",
    "f64",
    "i8",
    "i16",
    "i32",
    "i64",
    "u8",
    "u16",
    "u32",
    "u64",
]

# The containers implement the full mutable ABC protocols; register them so that
# `isinstance(x, collections.abc.MutableSequence)` etc. hold, as for the builtins.
MutableSequence.register(List)
MutableMapping.register(Dict)
MutableSet.register(Set)
