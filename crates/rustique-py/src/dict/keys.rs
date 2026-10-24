//! Key tables shared by `Dict` and `Set` (ARCHITECTURE §4.2, §4.3, §6).
//!
//! Keys are stored by strategy: native `i64` (Python `int`), native `str`, or Python objects
//! hashed with `__hash__` and compared with `__eq__`. Probes follow Python's equality, so
//! `True`, `1` and `1.0` find the same native int key. An untyped table takes the strategy of
//! its first key and demotes to object keys when a new key would not round-trip exactly
//! (a `bool`, a `float`, a `str` subclass, a big int, …).
//!
//! Locking: hashes are computed before taking a container lock. Lookups that need Python
//! `__eq__` snapshot the candidate keys under the lock, compare outside it and re-validate
//! with the table's mutation counter, retrying like CPython does when a comparison mutates
//! the container.

use std::ffi::c_void;

use compact_str::CompactString;
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use pyo3::exceptions::PyTypeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedStr;
use pyo3::types::{PyBool, PyBytes, PyComplex, PyFloat, PyFrozenSet, PyInt, PyString, PyTuple};
use rustique_core::index::{HashIndex, KeyIndex, Remap};

use crate::schema::{Coerced, Schema};

/// A container lock: readers share, writers exclude, and a contended acquisition waits
/// detached from the interpreter (so it never deadlocks with the GIL or a stop-the-world).
pub(crate) struct Locked<T>(RwLock<T>);

impl<T: Send + Sync> Locked<T> {
    pub(crate) fn new(value: T) -> Self {
        Locked(RwLock::new(value))
    }

    pub(crate) fn read(&self, py: Python<'_>) -> RwLockReadGuard<'_, T> {
        loop {
            if let Some(guard) = self.0.try_read() {
                return guard;
            }
            py.detach(|| drop(self.0.read()));
        }
    }

    pub(crate) fn write(&self, py: Python<'_>) -> RwLockWriteGuard<'_, T> {
        loop {
            if let Some(guard) = self.0.try_write() {
                return guard;
            }
            py.detach(|| drop(self.0.write()));
        }
    }

    /// Non-blocking shared access (for `__traverse__`, which must never block).
    pub(crate) fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        self.0.try_read()
    }
}

/// What a key is used as, for error messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    DictKey,
    SetElement,
}

impl Role {
    fn noun(self) -> &'static str {
        match self {
            Role::DictKey => "a dict key",
            Role::SetElement => "a set element",
        }
    }
}

/// How a key object relates to the native key strategies.
pub(crate) enum Class {
    /// Equal to the Python int `value`. `exact` when the object *is* an `int` (so it can be
    /// stored natively without changing what reads return).
    Int { value: i64, exact: bool },
    /// Equal to the Python str `text` (hash and equality are `str`'s).
    Str { text: PyBackedStr, exact: bool },
    /// Anything else, with its Python hash. `exotic` objects may compare equal to native
    /// keys through a custom `__eq__`; `simple` ones compare without running Python code.
    Other { hash: isize, exotic: bool, simple: bool },
}

/// A key object, classified and hashed (all outside any lock).
pub(crate) struct Key<'py> {
    pub(crate) obj: Bound<'py, PyAny>,
    pub(crate) class: Class,
}

impl<'py> Key<'py> {
    /// Classify `obj` for lookup or insertion. Unhashable objects raise `TypeError`.
    pub(crate) fn classify(obj: &Bound<'py, PyAny>, role: Role) -> PyResult<Key<'py>> {
        let class = if obj.is_exact_instance_of::<PyInt>() {
            match int_value(obj) {
                Some(value) => Class::Int { value, exact: true },
                None => Class::Other { hash: obj.hash()?, exotic: false, simple: true },
            }
        } else if obj.is_exact_instance_of::<PyString>() {
            str_class(obj, true)?
        } else if obj.is_exact_instance_of::<PyBool>() {
            Class::Int { value: i64::from(obj.is(PyBool::new(obj.py(), true))), exact: false }
        } else if let Ok(f) = obj.cast_exact::<PyFloat>() {
            match integral_float(f.value()) {
                Some(value) => Class::Int { value, exact: false },
                None => Class::Other { hash: obj.hash()?, exotic: false, simple: true },
            }
        } else if obj.is_instance_of::<PyInt>() && has_builtin_slots(obj, BuiltinBase::Int) {
            match int_value(obj) {
                Some(value) => Class::Int { value, exact: false },
                None => Class::Other { hash: obj.hash()?, exotic: false, simple: true },
            }
        } else if obj.is_instance_of::<PyString>() && has_builtin_slots(obj, BuiltinBase::Str) {
            str_class(obj, false)?
        } else {
            let hash = hash_or_unhashable(obj, role)?;
            Class::Other { hash, exotic: !never_equals_native(obj), simple: is_simple(obj, 0) }
        };
        Ok(Key { obj: obj.clone(), class })
    }

    /// Python's `hash()` of the key (no Python code runs for native classes).
    pub(crate) fn py_hash(&self) -> PyResult<isize> {
        match &self.class {
            Class::Int { value, .. } => Ok(py_hash_i64(*value)),
            Class::Str { .. } => self.obj.hash(),
            Class::Other { hash, .. } => Ok(*hash),
        }
    }

    fn is_simple(&self) -> bool {
        match self.class {
            Class::Int { .. } | Class::Str { .. } => true,
            Class::Other { simple, .. } => simple,
        }
    }
}

fn str_class(obj: &Bound<'_, PyAny>, exact: bool) -> PyResult<Class> {
    // SAFETY-free cast: callers checked that `obj` is a `str` (or subclass).
    let s = obj.cast::<PyString>().map_err(PyErr::from)?;
    Ok(match PyBackedStr::try_from(s.clone()) {
        Ok(text) => Class::Str { text, exact },
        // Lone surrogates: not representable natively, never equal to a native key.
        Err(_) => Class::Other { hash: obj.hash()?, exotic: false, simple: true },
    })
}

/// The value of an `int` (or `int` subclass) if it fits in i64. Runs no Python code.
fn int_value(obj: &Bound<'_, PyAny>) -> Option<i64> {
    let mut overflow = 0;
    // SAFETY: `obj` is a live `int` instance (checked by the caller), so this reads its value
    // without calling `__index__`; on overflow no exception is set and `overflow` is nonzero.
    let v = unsafe { ffi::PyLong_AsLongLongAndOverflow(obj.as_ptr(), &mut overflow) };
    if overflow != 0 {
        return None;
    }
    if v == -1 && PyErr::take(obj.py()).is_some() {
        return None;
    }
    Some(v)
}

/// The i64 equal to `f`, if any.
pub(crate) fn integral_float(f: f64) -> Option<i64> {
    // 2**63 is exactly representable; every float in [-2**63, 2**63) that is integral fits.
    if f.fract() == 0.0 && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&f) {
        Some(f as i64)
    } else {
        None
    }
}

#[derive(Clone, Copy)]
enum BuiltinBase {
    Int,
    Str,
}

/// Whether `obj`'s type hashes and compares exactly like the builtin base (true for e.g.
/// `IntEnum`/`StrEnum` members and plain subclasses, false when `__eq__`/`__hash__` are
/// overridden).
fn has_builtin_slots(obj: &Bound<'_, PyAny>, base: BuiltinBase) -> bool {
    let base_type = match base {
        BuiltinBase::Int => obj.py().get_type::<PyInt>(),
        BuiltinBase::Str => obj.py().get_type::<PyString>(),
    };
    let slot = |tp: *mut ffi::PyTypeObject, slot| -> *mut c_void {
        // SAFETY: `tp` is a valid, ready type object kept alive by `obj`/`base_type`, and
        // `PyType_GetSlot` accepts static and heap types for these slot ids (3.10+).
        unsafe { ffi::PyType_GetSlot(tp, slot) }
    };
    let tp = obj.get_type_ptr();
    let bp = base_type.as_type_ptr();
    slot(tp, ffi::Py_tp_hash) == slot(bp, ffi::Py_tp_hash)
        && slot(tp, ffi::Py_tp_richcompare) == slot(bp, ffi::Py_tp_richcompare)
}

/// Python `hash(obj)`, with CPython's container-specific message for unhashable types.
fn hash_or_unhashable(obj: &Bound<'_, PyAny>, role: Role) -> PyResult<isize> {
    obj.hash().map_err(|err| {
        let py = obj.py();
        let unhashable = err.is_instance_of::<PyTypeError>(py)
            && obj.get_type().getattr("__hash__").is_ok_and(|h| h.is_none());
        if unhashable {
            let name = obj.get_type().name().map(|n| n.to_string()).unwrap_or_else(|_| "?".into());
            PyTypeError::new_err(format!("cannot use '{name}' as {} (unhashable type: '{name}')", role.noun()))
        } else {
            err
        }
    })
}

/// Builtin objects that can never equal an `int` or a `str` (whatever their contents).
fn never_equals_native(obj: &Bound<'_, PyAny>) -> bool {
    obj.is_none()
        || obj.is_exact_instance_of::<PyTuple>()
        || obj.is_exact_instance_of::<PyBytes>()
        || obj.is_exact_instance_of::<PyFrozenSet>()
}

/// Objects whose equality with other simple objects runs no Python code (so it may be
/// evaluated under a container lock).
pub(crate) fn is_simple(obj: &Bound<'_, PyAny>, depth: u32) -> bool {
    if obj.is_exact_instance_of::<PyInt>()
        || obj.is_exact_instance_of::<PyString>()
        || obj.is_exact_instance_of::<PyFloat>()
        || obj.is_exact_instance_of::<PyBool>()
        || obj.is_exact_instance_of::<PyBytes>()
        || obj.is_exact_instance_of::<PyComplex>()
        || obj.is_none()
    {
        return true;
    }
    if let Ok(t) = obj.cast_exact::<PyTuple>() {
        return depth < 8 && t.iter_borrowed().all(|item| is_simple(&item, depth + 1));
    }
    (obj.is_instance_of::<PyInt>() && has_builtin_slots(obj, BuiltinBase::Int))
        || (obj.is_instance_of::<PyString>() && has_builtin_slots(obj, BuiltinBase::Str))
}

const HASH_MODULUS: u64 = (1 << 61) - 1;

/// CPython's `hash()` of an int.
pub(crate) fn py_hash_i64(v: i64) -> isize {
    let mut h = (v.unsigned_abs() % HASH_MODULUS) as i64;
    if v < 0 {
        h = -h;
    }
    if h == -1 {
        h = -2;
    }
    h as isize
}

/// Every i64 whose Python hash is `hash` (at most a handful).
pub(crate) fn int_hash_preimages(hash: isize) -> Vec<i64> {
    let p = i128::from(HASH_MODULUS as i64);
    let (lo, hi) = (i128::from(i64::MIN), i128::from(i64::MAX));
    let mut out = Vec::new();
    let mut push_progression = |start: i128, step: i128| {
        let mut v = start;
        while (lo..=hi).contains(&v) {
            out.push(v as i64);
            v += step;
        }
    };
    let h = hash as i128;
    if h >= 0 && h < p {
        push_progression(h, p);
        if h == 0 {
            push_progression(-p, -p);
        }
    } else if h < 0 && -h < p {
        push_progression(h, -p);
        if h == -2 {
            push_progression(-1, -p);
        }
    }
    out
}

/// Spread a Python hash over all 64 bits (Python's int hashes are the identity on small
/// ints; the index needs entropy in both the low and high bits). Shared by all object
/// tables, so mixed hashes are comparable across them.
#[inline]
pub(crate) fn mix(raw: isize) -> u64 {
    let mut z = raw as u64;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The physical key strategy.
pub(crate) enum KeyStore {
    /// Untyped and empty: the first key decides.
    Empty,
    Int(KeyIndex<i64>),
    Str(KeyIndex<CompactString>),
    /// Python objects; index hashes are `mix(hash(key))`.
    Object(KeyIndex<Py<PyAny>, ()>),
}

/// Native keys, cloned out of a table for lock-free set algebra.
pub(crate) enum NativeKeys {
    Int(KeyIndex<i64>),
    Str(KeyIndex<CompactString>),
}

/// The strategy a schema pins (`None`: adaptive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyKind {
    Adaptive,
    Int,
    Str,
    Object,
}

impl KeyKind {
    pub(crate) fn for_schema(schema: &Schema) -> KeyKind {
        match schema {
            Schema::Any => KeyKind::Adaptive,
            Schema::Int => KeyKind::Int,
            Schema::Str => KeyKind::Str,
            Schema::Fixed(d) if d.is_integer() => KeyKind::Int,
            _ => KeyKind::Object,
        }
    }
}

/// Outcome of resolving a key under the lock.
pub(crate) enum Resolve {
    Found(usize),
    Absent,
    /// Python comparisons are needed; run them outside the lock.
    Compare(Pending),
}

/// Candidate keys to compare with Python `==` outside the lock.
pub(crate) struct Pending {
    candidates: Vec<(usize, Py<PyAny>)>,
    version: u64,
}

impl Pending {
    /// The position of the first candidate equal to `key` (`stored == key`, like CPython).
    pub(crate) fn compare(&self, key: &Bound<'_, PyAny>) -> PyResult<Option<usize>> {
        for (pos, cand) in &self.candidates {
            if cand.bind(key.py()).eq(key)? {
                return Ok(Some(*pos));
            }
        }
        Ok(None)
    }
}

/// An insertion-ordered key table.
pub(crate) struct KeyTable {
    store: KeyStore,
    kind: KeyKind,
    /// Bumped by every change to the key set (insert, remove, clear, demotion, compaction).
    version: u64,
    /// Bumped whenever positions move (compaction, clear).
    layout: u64,
}

impl KeyTable {
    pub(crate) fn new(kind: KeyKind) -> KeyTable {
        KeyTable { store: Self::empty_store(kind), kind, version: 0, layout: 0 }
    }

    fn empty_store(kind: KeyKind) -> KeyStore {
        match kind {
            KeyKind::Adaptive => KeyStore::Empty,
            KeyKind::Int => KeyStore::Int(KeyIndex::default()),
            KeyKind::Str => KeyStore::Str(KeyIndex::default()),
            KeyKind::Object => KeyStore::Object(KeyIndex::with_hasher(())),
        }
    }

    pub(crate) fn from_native(native: NativeKeys, kind: KeyKind) -> KeyTable {
        let store = match native {
            NativeKeys::Int(ki) => KeyStore::Int(ki),
            NativeKeys::Str(ki) => KeyStore::Str(ki),
        };
        KeyTable { store, kind, version: 0, layout: 0 }
    }

    /// The same keys under a different pinned strategy (e.g. an untyped copy of a typed set).
    pub(crate) fn with_kind(mut self, kind: KeyKind) -> KeyTable {
        self.kind = kind;
        self
    }

    /// The native keys, or the table back if it does not store native keys.
    pub(crate) fn into_native(self) -> Result<NativeKeys, KeyTable> {
        match self.store {
            KeyStore::Int(ki) => Ok(NativeKeys::Int(ki)),
            KeyStore::Str(ki) => Ok(NativeKeys::Str(ki)),
            store => Err(KeyTable { store, ..self }),
        }
    }

    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    pub(crate) fn layout(&self) -> u64 {
        self.layout
    }

    pub(crate) fn store(&self) -> &KeyStore {
        &self.store
    }

    pub(crate) fn index(&self) -> Option<&HashIndex> {
        match &self.store {
            KeyStore::Empty => None,
            KeyStore::Int(ki) => Some(ki.index()),
            KeyStore::Str(ki) => Some(ki.index()),
            KeyStore::Object(ki) => Some(ki.index()),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.index().map_or(0, HashIndex::len)
    }

    /// Positions in use, including tombstones (parallel storage has this length).
    pub(crate) fn entries(&self) -> usize {
        self.index().map_or(0, HashIndex::entries)
    }

    pub(crate) fn next_live(&self, from: usize) -> Option<usize> {
        self.index()?.next_live(from)
    }

    /// Last live position `< before`.
    pub(crate) fn prev_live(&self, before: usize) -> Option<usize> {
        let index = self.index()?;
        (0..before.min(index.entries())).rev().find(|&p| index.is_live(p))
    }

    pub(crate) fn last_live(&self) -> Option<usize> {
        self.index()?.last_live()
    }

    pub(crate) fn positions(&self) -> Vec<usize> {
        self.index().map_or_else(Vec::new, |i| i.iter().collect())
    }

    /// Strategy name, for introspection.
    pub(crate) fn strategy(&self) -> &'static str {
        match self.store {
            KeyStore::Empty => "empty",
            KeyStore::Int(_) => "i64",
            KeyStore::Str(_) => "str",
            KeyStore::Object(_) => "object",
        }
    }

    /// The live key at `pos` as a Python object (creating `int`/`str` objects is allowed
    /// under a lock).
    pub(crate) fn key_object(&self, py: Python<'_>, pos: usize) -> Py<PyAny> {
        match &self.store {
            KeyStore::Empty => py.None(),
            KeyStore::Int(ki) => ki.raw_keys()[pos].into_pyobject(py).map(|o| o.into_any().unbind()).unwrap_or_else(|e| match e {}),
            KeyStore::Str(ki) => PyString::new(py, &ki.raw_keys()[pos]).into_any().unbind(),
            KeyStore::Object(ki) => ki.raw_keys()[pos].clone_ref(py),
        }
    }

    /// All live keys as Python objects, in order.
    pub(crate) fn key_objects(&self, py: Python<'_>) -> Vec<Py<PyAny>> {
        self.positions().into_iter().map(|p| self.key_object(py, p)).collect()
    }

    /// A copy of the native keys (`None` for object or empty tables).
    pub(crate) fn native_clone(&self) -> Option<NativeKeys> {
        match &self.store {
            KeyStore::Int(ki) => Some(NativeKeys::Int(ki.clone())),
            KeyStore::Str(ki) => Some(NativeKeys::Str(ki.clone())),
            _ => None,
        }
    }

    pub(crate) fn clone_ref(&self, py: Python<'_>) -> KeyTable {
        let store = match &self.store {
            KeyStore::Empty => KeyStore::Empty,
            KeyStore::Int(ki) => KeyStore::Int(ki.clone()),
            KeyStore::Str(ki) => KeyStore::Str(ki.clone()),
            KeyStore::Object(ki) => {
                let keys = ki.raw_keys().iter().map(|k| k.clone_ref(py)).collect();
                KeyStore::Object(KeyIndex::from_parts(keys, ki.index().clone(), ()))
            }
        };
        KeyTable { store, kind: self.kind, version: 0, layout: 0 }
    }

    /// Find `key` without running Python code; Python comparisons become `Compare`.
    pub(crate) fn resolve(&self, py: Python<'_>, key: &Key<'_>) -> PyResult<Resolve> {
        let mut candidates = Vec::new();
        match (&self.store, &key.class) {
            (KeyStore::Empty, _) => return Ok(Resolve::Absent),
            (KeyStore::Int(ki), Class::Int { value, .. }) => return Ok(found(ki.get(value))),
            (KeyStore::Str(ki), Class::Str { text, .. }) => return Ok(found(ki.get(&**text))),
            (KeyStore::Int(ki), Class::Other { hash, exotic: true, .. }) => {
                for v in int_hash_preimages(*hash) {
                    if let Some(pos) = ki.get(&v) {
                        candidates.push((pos, v.into_pyobject(py)?.into_any().unbind()));
                    }
                }
            }
            (KeyStore::Str(ki), Class::Other { hash, exotic: true, .. }) => {
                for (pos, s) in ki.iter() {
                    let obj = PyString::new(py, s);
                    if obj.hash()? == *hash {
                        candidates.push((pos, obj.into_any().unbind()));
                    }
                }
            }
            (KeyStore::Int(_) | KeyStore::Str(_), _) => return Ok(Resolve::Absent),
            (KeyStore::Object(ki), _) => {
                let hash = mix(key.py_hash()?);
                let simple = key.is_simple();
                for pos in ki.index().candidates(hash) {
                    let stored = ki.raw_keys()[pos].bind(py);
                    if stored.is(&key.obj) {
                        return Ok(Resolve::Found(pos));
                    }
                    if simple && is_simple(stored, 0) {
                        if stored.eq(&key.obj)? {
                            return Ok(Resolve::Found(pos));
                        }
                    } else {
                        candidates.push((pos, stored.clone().unbind()));
                    }
                }
            }
        }
        Ok(if candidates.is_empty() {
            Resolve::Absent
        } else {
            Resolve::Compare(Pending { candidates, version: self.version })
        })
    }

    /// Find `key` in a table nobody else can reach, comparing inline.
    pub(crate) fn find_local(&self, py: Python<'_>, key: &Key<'_>) -> PyResult<Option<usize>> {
        match self.resolve(py, key)? {
            Resolve::Found(pos) => Ok(Some(pos)),
            Resolve::Absent => Ok(None),
            Resolve::Compare(pending) => pending.compare(&key.obj),
        }
    }

    /// Insert `key` into a table nobody else can reach; returns `false` if it was present.
    pub(crate) fn insert_local(&mut self, py: Python<'_>, key: &Key<'_>) -> PyResult<bool> {
        if self.find_local(py, key)?.is_some() {
            return Ok(false);
        }
        self.push(py, key)?;
        Ok(true)
    }

    /// Append `key`, which the caller has resolved as absent. Picks or demotes the strategy.
    pub(crate) fn push(&mut self, py: Python<'_>, key: &Key<'_>) -> PyResult<usize> {
        self.version += 1;
        loop {
            match (&mut self.store, &key.class) {
                (KeyStore::Empty, class) => {
                    self.store = match class {
                        Class::Int { exact: true, .. } => KeyStore::Int(KeyIndex::default()),
                        Class::Str { exact: true, .. } => KeyStore::Str(KeyIndex::default()),
                        _ => KeyStore::Object(KeyIndex::with_hasher(())),
                    };
                }
                (KeyStore::Int(ki), Class::Int { value, exact: true }) => {
                    let hash = ki.hash_key(value);
                    return Ok(ki.push_hashed(hash, *value));
                }
                (KeyStore::Str(ki), Class::Str { text, exact: true }) => {
                    let hash = ki.hash_key(&**text);
                    return Ok(ki.push_hashed(hash, CompactString::from(&**text)));
                }
                (KeyStore::Object(ki), _) => {
                    let hash = mix(key.py_hash()?);
                    return Ok(ki.push_hashed(hash, key.obj.clone().unbind()));
                }
                (KeyStore::Int(_) | KeyStore::Str(_), _) => self.demote(py)?,
            }
        }
    }

    /// Convert native keys to object keys in place (positions are preserved). Creates only
    /// `int`/`str` objects and hashes them, so it is safe under a lock.
    pub(crate) fn demote(&mut self, py: Python<'_>) -> PyResult<()> {
        let store = std::mem::replace(&mut self.store, KeyStore::Empty);
        let result: PyResult<KeyStore> = match store {
            KeyStore::Int(ki) => {
                let (keys, mut index, _) = ki.into_parts();
                index.rehash(|pos| mix(py_hash_i64(keys[pos])));
                let objs = keys
                    .iter()
                    .map(|k| k.into_pyobject(py).map(|o| o.into_any().unbind()))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(KeyStore::Object(KeyIndex::from_parts(objs, index, ())))
            }
            KeyStore::Str(ki) => {
                let (keys, mut index, _) = ki.into_parts();
                let objs: Vec<Py<PyAny>> = keys.iter().map(|k| PyString::new(py, k).into_any().unbind()).collect();
                let hashes = objs.iter().map(|o| o.bind(py).hash()).collect::<PyResult<Vec<_>>>();
                match hashes {
                    Ok(hashes) => {
                        index.rehash(|pos| mix(hashes[pos]));
                        Ok(KeyStore::Object(KeyIndex::from_parts(objs, index, ())))
                    }
                    Err(e) => {
                        let ki = KeyIndex::from_parts(keys, index, Default::default());
                        self.store = KeyStore::Str(ki);
                        return Err(e);
                    }
                }
            }
            other => Ok(other),
        };
        self.store = result?;
        self.version += 1;
        Ok(())
    }

    /// Tombstone the live entry at `pos`; returns the displaced key object, if any.
    pub(crate) fn remove_at(&mut self, py: Python<'_>, pos: usize) -> Option<Py<PyAny>> {
        self.version += 1;
        match &mut self.store {
            KeyStore::Empty => None,
            KeyStore::Int(ki) => {
                ki.remove_at(pos, 0);
                None
            }
            KeyStore::Str(ki) => {
                ki.remove_at(pos, CompactString::default());
                None
            }
            KeyStore::Object(ki) => ki.remove_at(pos, py.None()),
        }
    }

    /// Compact when tombstones dominate; returns the remap for parallel storage.
    pub(crate) fn maybe_compact(&mut self) -> Option<Remap> {
        let remap = match &mut self.store {
            KeyStore::Empty => return None,
            KeyStore::Int(ki) if ki.wants_compaction() => ki.compact(),
            KeyStore::Str(ki) if ki.wants_compaction() => ki.compact(),
            // Tombstoned object slots hold `None`, so dropping them here runs no Python code.
            KeyStore::Object(ki) if ki.wants_compaction() => ki.compact(),
            _ => return None,
        };
        self.version += 1;
        self.layout += 1;
        Some(remap)
    }

    /// Drop positions `>= len`; returns displaced key objects.
    pub(crate) fn truncate(&mut self, len: usize) -> Vec<Py<PyAny>> {
        self.version += 1;
        match &mut self.store {
            KeyStore::Empty => Vec::new(),
            KeyStore::Int(ki) => {
                ki.truncate(len);
                Vec::new()
            }
            KeyStore::Str(ki) => {
                ki.truncate(len);
                Vec::new()
            }
            KeyStore::Object(ki) => ki.truncate(len),
        }
    }

    /// Remove everything (an untyped table becomes adaptive again); returns displaced keys.
    pub(crate) fn clear(&mut self) -> Vec<Py<PyAny>> {
        self.version += 1;
        self.layout += 1;
        match std::mem::replace(&mut self.store, Self::empty_store(self.kind)) {
            KeyStore::Object(ki) => ki.into_parts().0,
            _ => Vec::new(),
        }
    }

    /// Replace the contents, returning displaced keys.
    pub(crate) fn replace(&mut self, other: KeyTable) -> Vec<Py<PyAny>> {
        let displaced = self.clear();
        self.store = other.store;
        displaced
    }

    /// Object keys, for `__traverse__`.
    pub(crate) fn object_keys(&self) -> &[Py<PyAny>] {
        match &self.store {
            KeyStore::Object(ki) => ki.raw_keys(),
            _ => &[],
        }
    }
}

fn found(pos: Option<usize>) -> Resolve {
    pos.map_or(Resolve::Absent, Resolve::Found)
}

/// State guarded by a container lock that holds a key table.
pub(crate) trait Keyed: Send + Sync {
    fn table(&self) -> &KeyTable;
}

/// Locate `key` and run `read` on the entry under a shared lock.
pub(crate) fn with_entry<T: Keyed, R>(
    py: Python<'_>,
    lock: &Locked<T>,
    key: &Key<'_>,
    read: impl FnOnce(&T, Option<usize>) -> R,
) -> PyResult<R> {
    loop {
        let pending = {
            let guard = lock.read(py);
            match guard.table().resolve(py, key)? {
                Resolve::Found(pos) => return Ok(read(&guard, Some(pos))),
                Resolve::Absent => return Ok(read(&guard, None)),
                Resolve::Compare(pending) => pending,
            }
        };
        let found = pending.compare(&key.obj)?;
        let version = pending.version;
        drop(pending);
        let guard = lock.read(py);
        if guard.table().version == version {
            return Ok(read(&guard, found));
        }
    }
}

/// Locate `key` and run `write` on the entry under the exclusive lock.
pub(crate) fn with_entry_mut<T: Keyed, R>(
    py: Python<'_>,
    lock: &Locked<T>,
    key: &Key<'_>,
    write: impl FnOnce(&mut T, Option<usize>) -> R,
) -> PyResult<R> {
    loop {
        let pending = {
            let mut guard = lock.write(py);
            match guard.table().resolve(py, key)? {
                Resolve::Found(pos) => return Ok(write(&mut guard, Some(pos))),
                Resolve::Absent => return Ok(write(&mut guard, None)),
                Resolve::Compare(pending) => pending,
            }
        };
        let found = pending.compare(&key.obj)?;
        let version = pending.version;
        drop(pending);
        let mut guard = lock.write(py);
        if guard.table().version == version {
            return Ok(write(&mut guard, found));
        }
    }
}

/// Apply `commit` to each item's located entry in order, holding the exclusive lock except
/// while Python comparisons run.
pub(crate) fn batch_mut<'py, T: Keyed, I>(
    py: Python<'py>,
    lock: &Locked<T>,
    items: impl IntoIterator<Item = I>,
    key_of: impl Fn(&I) -> &Key<'py>,
    mut commit: impl FnMut(&mut T, I, Option<usize>) -> PyResult<()>,
) -> PyResult<()> {
    let mut guard = lock.write(py);
    for item in items {
        loop {
            let pending = match guard.table().resolve(py, key_of(&item))? {
                Resolve::Found(pos) => {
                    commit(&mut guard, item, Some(pos))?;
                    break;
                }
                Resolve::Absent => {
                    commit(&mut guard, item, None)?;
                    break;
                }
                Resolve::Compare(pending) => pending,
            };
            drop(guard);
            let found = pending.compare(&key_of(&item).obj)?;
            let version = pending.version;
            drop(pending);
            guard = lock.write(py);
            if guard.table().version == version {
                commit(&mut guard, item, found)?;
                break;
            }
        }
    }
    Ok(())
}

/// The Python object for a coerced value, reusing `original` when it already has the
/// stored type.
pub(crate) fn materialize<'py>(original: &Bound<'py, PyAny>, coerced: Coerced) -> PyResult<Bound<'py, PyAny>> {
    use rustique_core::Value;
    let py = original.py();
    let value = match coerced {
        Coerced::Object(obj) => return Ok(obj.into_bound(py)),
        Coerced::Native(value) => value,
    };
    let same_type = match &value {
        Value::I64(_) => original.is_exact_instance_of::<PyInt>(),
        Value::Str(_) => original.is_exact_instance_of::<PyString>(),
        Value::F64(_) => original.is_exact_instance_of::<PyFloat>(),
        Value::Bool(_) => original.is_exact_instance_of::<PyBool>(),
        _ => false,
    };
    if same_type {
        return Ok(original.clone());
    }
    Ok(match value {
        Value::Bool(b) => PyBool::new(py, b).to_owned().into_any(),
        Value::I8(v) => v.into_pyobject(py)?.into_any(),
        Value::I16(v) => v.into_pyobject(py)?.into_any(),
        Value::I32(v) => v.into_pyobject(py)?.into_any(),
        Value::I64(v) => v.into_pyobject(py)?.into_any(),
        Value::U8(v) => v.into_pyobject(py)?.into_any(),
        Value::U16(v) => v.into_pyobject(py)?.into_any(),
        Value::U32(v) => v.into_pyobject(py)?.into_any(),
        Value::U64(v) => v.into_pyobject(py)?.into_any(),
        Value::F32(v) => PyFloat::new(py, f64::from(v)).into_any(),
        Value::F64(v) => PyFloat::new(py, v).into_any(),
        Value::Str(s) => PyString::new(py, &s).into_any(),
    })
}

/// Validate `obj` against a key/element schema and classify the stored form.
pub(crate) fn prepare<'py>(
    obj: &Bound<'py, PyAny>,
    schema: &Schema,
    role: Role,
    container: impl FnOnce() -> String,
) -> PyResult<Key<'py>> {
    let fast = match schema {
        Schema::Any => true,
        Schema::Int => obj.is_exact_instance_of::<PyInt>(),
        Schema::Str => obj.is_exact_instance_of::<PyString>(),
        _ => false,
    };
    if fast {
        let key = Key::classify(obj, role)?;
        // A typed int table demotes huge ints like the schema says; anything else is native.
        return Ok(key);
    }
    match schema.coerce(obj) {
        Ok(coerced) => Key::classify(&materialize(obj, coerced)?, role),
        Err(e) => Err(e.into_pyerr(&container(), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_hash_matches_cpython_rules() {
        assert_eq!(py_hash_i64(0), 0);
        assert_eq!(py_hash_i64(1), 1);
        assert_eq!(py_hash_i64(-1), -2);
        assert_eq!(py_hash_i64(-2), -2);
        assert_eq!(py_hash_i64((1 << 61) - 1), 0);
        assert_eq!(py_hash_i64(1 << 61), 1);
        assert_eq!(py_hash_i64(i64::MIN), -4);
        assert_eq!(py_hash_i64(i64::MAX), (i64::MAX as u64 % HASH_MODULUS) as isize);
    }

    #[test]
    fn preimages_invert_the_hash() {
        for v in [0i64, 1, -1, -2, 2, 12345, -12345, i64::MAX, i64::MIN, (1 << 61) - 1, 1 << 62, -(1 << 62)] {
            let h = py_hash_i64(v);
            let pre = int_hash_preimages(h);
            assert!(pre.contains(&v), "{v} not among preimages of {h}: {pre:?}");
            assert!(pre.iter().all(|&p| py_hash_i64(p) == h));
            assert!(pre.len() <= 10);
        }
        assert!(int_hash_preimages(-1).is_empty());
        assert!(int_hash_preimages(isize::MAX).is_empty());
    }

    #[test]
    fn integral_floats() {
        assert_eq!(integral_float(1.0), Some(1));
        assert_eq!(integral_float(-0.0), Some(0));
        assert_eq!(integral_float(1.5), None);
        assert_eq!(integral_float(f64::NAN), None);
        assert_eq!(integral_float(f64::INFINITY), None);
        assert_eq!(integral_float(9_223_372_036_854_775_808.0), None);
        assert_eq!(integral_float(-9_223_372_036_854_775_808.0), Some(i64::MIN));
    }
}
