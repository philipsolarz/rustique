"""Rustique: Python's data model, Rust's memory.

Typed, natively stored ``List``/``Dict``/``Set``/``FrozenSet`` containers with builtin-compatible
semantics, vectorised expressions over the element placeholder ``X``, and fixed-width
scalar types. See ``docs/ARCHITECTURE.md``.
"""

# `_collections_abc` holds the classes `collections.abc` re-exports. The interpreter has
# imported it by the time `site` runs, while `collections.abc` would load the whole
# `collections` package (~1 ms of startup).
from _collections_abc import MutableMapping, MutableSequence, MutableSet, Sequence
from _collections_abc import Set as AbstractSet

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

# The switch (docs/ACCELERATION.md, docs/USAGE.md): `enable()` puts Rust kernels behind
# CPython's own entry points, process-wide, under the RUSTIQUE* environment variables
# (RUSTIQUE=0 is the kill switch); `status()`/`stats()` show what runs; `native` opts modules
# into Rustique containers for their literals (docs/TAKEOVER.md).
from rustique._switch import disable, enable, native, stats, status

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
    "stats",
    "status",
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
