"""Rustique: Python's data model, Rust's memory.

Typed, natively stored ``List``/``Dict``/``Set``/``FrozenSet`` containers with builtin-compatible
semantics, vectorised expressions over the element placeholder ``X``, and fixed-width
scalar types. See ``docs/ARCHITECTURE.md``.
"""

from collections.abc import MutableMapping, MutableSequence, MutableSet, Sequence
from collections.abc import Set as AbstractSet

from rustique._rustique import (
    Dict,
    DictType,
    Expr,
    FrozenSet,
    FrozenSetType,
    List,
    ListType,
    Set,
    SetType,
    Tuple,
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

# The takeover (docs/TAKEOVER.md): `enable()` accelerates the builtin containers and
# functions in place; `native` opts modules into Rustique containers for their literals.
from rustique._takeover import disable, enable, native

__all__ = [
    "Dict",
    "DictType",
    "Expr",
    "FrozenSet",
    "FrozenSetType",
    "List",
    "ListType",
    "Set",
    "SetType",
    "Tuple",
    "X",
    "__version__",
    "arange",
    "disable",
    "enable",
    "f32",
    "f64",
    "i8",
    "i16",
    "i32",
    "i64",
    "native",
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
Sequence.register(Tuple)
AbstractSet.register(FrozenSet)
