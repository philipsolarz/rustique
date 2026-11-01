//! The `Dict` class (ARCHITECTURE §4.2): an insertion-ordered mapping with native key
//! strategies (`i64`, `str`, or Python objects) and values in a shared [`Storage`].
//!
//! OWNER: dict/set agent. Items marked `INTERFACE` are used by other modules.

mod fast;
pub(crate) mod keys;

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use pyo3::Borrowed;
use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::pyclass::CompareOp;
use pyo3::sync::PyOnceLock;
use pyo3::sync::critical_section::with_critical_section;
use pyo3::types::{PyBool, PyDict, PyFloat, PyFrozenSet, PyInt, PyList, PySet, PyString, PyTuple, PyType};
use pyo3::{PyTraverseError, ffi};
use rustique_core::Column;
use rustique_core::index::Remap;

use crate::convert;
use crate::schema::{self, Coerced, Schema};
use crate::storage::{Displaced, Storage};

use keys::{FastKey, Key, KeyKind, KeyStore, KeyTable, Keyed, Locked, Prepared, Role, Slot};

/// Keys and the parallel value storage (same positions, tombstones included).
pub(crate) struct DictInner {
    table: KeyTable,
    values: Storage,
}

impl Keyed for DictInner {
    fn table(&self) -> &KeyTable {
        &self.table
    }

    fn table_mut(&mut self) -> &mut KeyTable {
        &mut self.table
    }

    #[inline]
    fn inserted(&mut self, out: &mut Vec<Py<PyAny>>) {
        if let Some(remap) = self.table.grow() {
            out.extend(remap_storage(&mut self.values, &remap));
        }
    }
}

impl DictInner {
    fn new(key_schema: &Schema, value_schema: &Schema) -> DictInner {
        DictInner::with(KeyTable::for_dict(KeyKind::for_schema(key_schema)), value_storage(value_schema))
    }

    fn with(table: KeyTable, values: Storage) -> DictInner {
        DictInner { table, values }
    }

    fn value(&self, py: Python<'_>, pos: usize) -> Py<PyAny> {
        self.values.get(py, pos).unwrap_or_else(|| py.None())
    }

    /// The value at the live position `pos` as a new reference (for the raw slots).
    #[inline(always)]
    fn value_ptr(&self, py: Python<'_>, pos: usize) -> Option<*mut ffi::PyObject> {
        Some(match &self.values {
            Storage::Object(objs) => objs.get(pos)?.clone_ref(py).into_ptr(),
            Storage::Native(col) => convert::column_item(py, col, pos).into_ptr(),
            Storage::Empty => return None,
        })
    }

    /// Append an entry for an absent key. If the key cannot be stored, the value goes to
    /// `out` (it may hold the last reference to an object: never drop it under the lock).
    fn push_entry(&mut self, py: Python<'_>, key: &Key<'_>, value: Coerced, out: &mut Displaced) -> PyResult<()> {
        if let Err(e) = self.table.push(py, key) {
            out.extend(coerced_object(value));
            return Err(e);
        }
        self.values.push(py, value);
        Ok(())
    }

    /// Remove the live entry at `pos`, returning its value; displaced objects go to `out`.
    /// Like `dict`, the tombstone stays until the table next grows.
    fn remove_entry(&mut self, py: Python<'_>, pos: usize, out: &mut Displaced) -> Py<PyAny> {
        let value = take_value(py, &mut self.values, pos);
        out.extend(self.table.remove_at(py, pos));
        value
    }

    /// Remove the most recently inserted entry.
    fn pop_last(&mut self, py: Python<'_>, out: &mut Displaced) -> Option<(Py<PyAny>, Py<PyAny>)> {
        let pos = self.table.last_live()?;
        let key = self.table.key_object(py, pos);
        let value = take_value(py, &mut self.values, pos);
        // Dropping the removed slots in place runs no Python code: we hold a reference to
        // the popped key, its value slot now holds `None`, and so do the tombstones after it.
        self.table.truncate_in_place(pos);
        match &mut self.values {
            Storage::Object(objs) => objs.truncate(pos),
            values => out.extend(truncate_storage(values, pos)),
        }
        Some((key, value))
    }

    /// Empty the dict, handing back the old contents (drop them after unlocking: dropping
    /// releases keys and values).
    fn clear(&mut self, value_schema: &Schema) -> DictInner {
        let empty = self.table.empty_like();
        let table = std::mem::replace(&mut self.table, empty);
        let values = std::mem::replace(&mut self.values, value_storage(value_schema));
        DictInner { table, values }
    }

    /// The contents of an untyped dict built from an exact builtin `dict`, read in one pass
    /// inside the source's critical section. The source's keys are pairwise distinct, so
    /// they are stored without lookups (like `dict(d)`). `None` when a key's hash might run
    /// Python code (the general path handles it).
    fn from_builtin(py: Python<'_>, src: &Bound<'_, PyDict>) -> Option<DictInner> {
        if src.is_empty() {
            return Some(DictInner::new(&Schema::Any, &Schema::Any));
        }
        with_critical_section(src.as_any(), || {
            // SAFETY: inside the critical section no Python code runs (only pure hashes, see
            // `pure_hash`), so the iteration state and the borrowed items stay valid.
            unsafe {
                let n = src.len();
                let mut keys = Vec::with_capacity(n);
                let mut hashes = Vec::with_capacity(n);
                let mut values = Vec::with_capacity(n);
                let mut ints = Vec::with_capacity(n);
                let (mut all_int, mut str_only) = (true, true);
                let (mut pos, mut k, mut v) = (0, std::ptr::null_mut(), std::ptr::null_mut());
                while ffi::PyDict_Next(src.as_ptr(), &mut pos, &mut k, &mut v) != 0 {
                    let ty = ffi::Py_TYPE(k);
                    let int = if ty == &raw mut ffi::PyLong_Type { keys::exact_int_value(k) } else { None };
                    let hash = if let Some(i) = int {
                        if all_int {
                            ints.push(i);
                        }
                        str_only = false;
                        rustique_core::index::py_hash_i64(i) as isize
                    } else if ty == &raw mut ffi::PyUnicode_Type {
                        all_int = false;
                        keys::exact_str_hash(k)
                    } else {
                        (all_int, str_only) = (false, false);
                        if !keys::pure_hash(k, 0) {
                            return None;
                        }
                        let h = ffi::PyObject_Hash(k);
                        if h == -1 {
                            ffi::PyErr_Clear();
                            return None;
                        }
                        h
                    };
                    keys.push(k);
                    hashes.push(hash);
                    values.push(Bound::from_borrowed_ptr(py, v).unbind());
                }
                let table = if n == 0 {
                    KeyTable::for_dict(KeyKind::Adaptive)
                } else if all_int {
                    KeyTable::dict_from_ints(KeyKind::Adaptive, ints, hashes)
                } else {
                    let objs = keys.into_iter().map(|k| Bound::from_borrowed_ptr(py, k).unbind()).collect();
                    KeyTable::dict_from_objects(KeyKind::Adaptive, objs, hashes, str_only && n > 0)
                };
                Some(DictInner::with(table, Storage::Object(values)))
            }
        })
    }

    /// `self[key] = value` for an untyped dict that nobody else can reach yet (being built):
    /// no lock, and Python code (`__hash__`, `__eq__`, finalizers) may run.
    fn set_private(&mut self, py: Python<'_>, key: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        // SAFETY: `key` is live for the duration of the call.
        if let Some(probe) = unsafe { FastKey::of(key.as_ptr()) }
            && let Some((pos, inserted)) = self.table.upsert_fast_key(py, &probe)
        {
            self.store_private(py, pos, inserted, value);
            return Ok(());
        }
        let key = Key::classify(key, Role::DictKey)?;
        match self.table.find_local(py, &key)? {
            Some(pos) => self.store_private(py, pos, false, value),
            None => {
                let pos = self.table.push(py, &key)?;
                self.store_private(py, pos, true, value);
            }
        }
        Ok(())
    }

    /// Store the value of an entry just located (see [`DictInner::set_private`]).
    #[inline]
    fn store_private(&mut self, py: Python<'_>, pos: usize, inserted: bool, value: &Bound<'_, PyAny>) {
        if inserted {
            match &mut self.values {
                Storage::Object(objs) => objs.push(value.clone().unbind()),
                values => values.push_any(py, value),
            }
            let mut moved = Vec::new();
            self.inserted(&mut moved);
        } else {
            // Private: dropping the old value (which may run `__del__`) cannot reach this dict.
            match &mut self.values {
                Storage::Object(objs) => objs[pos] = value.clone().unbind(),
                values => drop(values.set_any(py, pos, value)),
            }
        }
    }

    /// `self.update(arg)` for a private untyped dict (see [`DictInner::set_private`]),
    /// following `dict.update`: mappings (`keys()` then `m[k]` per key) or iterables of pairs.
    fn update_private(&mut self, py: Python<'_>, arg: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Ok(dict) = arg.cast_exact::<PyDict>() {
            let (pairs, err) = collect_dict(dict);
            for (k, v) in &pairs {
                self.set_private(py, k, v)?;
            }
            return err.map_or(Ok(()), Err);
        }
        if let Ok(d) = arg.cast::<Dict>() {
            let items = d.get().items_snapshot(py);
            for (k, v) in items {
                self.set_private(py, k.bind(py), v.bind(py))?;
            }
            return Ok(());
        }
        if arg.hasattr(pyo3::intern!(py, "keys"))? {
            let keys = arg.call_method0(pyo3::intern!(py, "keys"))?;
            for key in keys.try_iter()? {
                let key = key?;
                let value = arg.get_item(&key)?;
                self.set_private(py, &key, &value)?;
            }
            return Ok(());
        }
        let (items, err) = match arg.cast_exact::<PyList>() {
            Ok(_) => collect_iterable(arg),
            Err(_) => {
                for (i, item) in arg.try_iter()?.enumerate() {
                    let (k, v) = pair_of(item?, i)?;
                    self.set_private(py, &k, &v)?;
                }
                return Ok(());
            }
        };
        for (i, item) in items.into_iter().enumerate() {
            let (k, v) = pair_of(item, i)?;
            self.set_private(py, &k, &v)?;
        }
        err.map_or(Ok(()), Err)
    }

    fn values_native(&self) -> bool {
        matches!(self.values, Storage::Native(_))
    }

    fn clone_ref(&self, py: Python<'_>) -> DictInner {
        DictInner::with(self.table.clone_ref(py), self.values.clone_ref(py))
    }

    fn items(&self, py: Python<'_>) -> Vec<(Py<PyAny>, Py<PyAny>)> {
        self.table.positions().into_iter().map(|p| (self.table.key_object(py, p), self.value(py, p))).collect()
    }

    /// Live entries for merging into another dict: native int keys stay native.
    fn merge_items(&self, py: Python<'_>) -> Vec<(MergeKey, Py<PyAny>)> {
        let Some(index) = self.table.index() else { return Vec::new() };
        match self.table.store() {
            KeyStore::Int(ki) => index.iter().map(|p| (MergeKey::Int(ki.raw_keys()[p]), self.value(py, p))).collect(),
            _ => index.iter().map(|p| (MergeKey::Obj(self.table.key_object(py, p)), self.value(py, p))).collect(),
        }
    }

    fn values_list(&self, py: Python<'_>) -> Vec<Py<PyAny>> {
        self.table.positions().into_iter().map(|p| self.value(py, p)).collect()
    }
}

/// Value storage for a new dict. Untyped dicts store the value objects themselves (like
/// `dict`): reading a value is then an incref instead of boxing a native number, and values
/// keep their identity. Typed dicts store natively per the value schema.
fn value_storage(value_schema: &Schema) -> Storage {
    if value_schema.is_any() { Storage::Object(Vec::new()) } else { Storage::for_schema(value_schema) }
}

/// The Python object a coerced value holds, if any.
fn coerced_object(value: Coerced) -> Option<Py<PyAny>> {
    match value {
        Coerced::Object(obj) => Some(obj),
        Coerced::Native(_) => None,
    }
}

/// Take the value at `pos` out of `values` (object slots are replaced by `None`).
fn take_value(py: Python<'_>, values: &mut Storage, pos: usize) -> Py<PyAny> {
    match values {
        Storage::Object(objs) => objs.get_mut(pos).map_or_else(|| py.None(), |slot| std::mem::replace(slot, py.None())),
        other => other.get(py, pos).unwrap_or_else(|| py.None()),
    }
}

fn reserve_storage(values: &mut Storage, additional: usize) {
    match values {
        Storage::Empty => {}
        Storage::Native(col) => col.reserve(additional),
        Storage::Object(objs) => objs.reserve(additional),
    }
}

/// Apply a key-table compaction to parallel value storage.
pub(crate) fn remap_storage(values: &mut Storage, remap: &Remap) -> Displaced {
    match values {
        Storage::Empty => Vec::new(),
        Storage::Native(col) => {
            rustique_core::with_column!(col, v => remap.apply(v));
            Vec::new()
        }
        Storage::Object(objs) => remap.apply_collect(objs),
    }
}

fn truncate_storage(values: &mut Storage, len: usize) -> Displaced {
    match values {
        Storage::Empty => Vec::new(),
        Storage::Native(col) => {
            col.truncate(len);
            Vec::new()
        }
        Storage::Object(objs) if len < objs.len() => objs.split_off(len),
        Storage::Object(_) => Vec::new(),
    }
}

/// `builtins.frozendict` (3.15+), if present.
pub(crate) fn frozendict_type(py: Python<'_>) -> Option<&Bound<'_, PyType>> {
    static FROZENDICT: PyOnceLock<Option<Py<PyType>>> = PyOnceLock::new();
    FROZENDICT
        .get_or_init(py, || {
            py.import("builtins")
                .and_then(|b| b.getattr("frozendict"))
                .ok()
                .and_then(|t| t.cast_into::<PyType>().ok())
                .map(Bound::unbind)
        })
        .as_ref()
        .map(|t| t.bind(py))
}

/// Whether `obj` is a builtin `dict` (or subclass) or `frozendict`.
fn is_builtin_mapping(obj: &Bound<'_, PyAny>) -> bool {
    obj.is_instance_of::<PyDict>() || frozendict_type(obj.py()).is_some_and(|t| obj.is_instance(t).unwrap_or(false))
}

/// Strong references to everything an iterable yields, plus the error that stopped it.
/// Builtin lists, sets and dicts are read inside one critical section on the source.
pub(crate) fn collect_iterable<'py>(obj: &Bound<'py, PyAny>) -> (Vec<Bound<'py, PyAny>>, Option<PyErr>) {
    if let Ok(list) = obj.cast_exact::<PyList>() {
        let out = RefCell::new(Vec::with_capacity(list.len()));
        let res = list.locked_for_each(|item| {
            out.borrow_mut().push(item);
            Ok(())
        });
        return (out.into_inner(), res.err());
    }
    if let Ok(tuple) = obj.cast_exact::<PyTuple>() {
        return (tuple.iter().collect(), None);
    }
    if let Ok(set) = obj.cast_exact::<PySet>() {
        return (with_critical_section(set.as_any(), || set.iter().collect()), None);
    }
    if let Ok(set) = obj.cast_exact::<PyFrozenSet>() {
        return (set.iter().collect(), None);
    }
    if let Ok(dict) = obj.cast_exact::<PyDict>() {
        let out = RefCell::new(Vec::with_capacity(dict.len()));
        let res = dict.locked_for_each(|k, _| {
            out.borrow_mut().push(k);
            Ok(())
        });
        return (out.into_inner(), res.err());
    }
    let py = obj.py();
    if let Ok(d) = obj.cast::<Dict>() {
        let keys = d.get().inner.read(py).table.key_objects(py);
        return (keys.into_iter().map(|k| k.into_bound(py)).collect(), None);
    }
    if let Ok(s) = obj.cast::<crate::set::Set>() {
        let keys = s.get().element_objects(py);
        return (keys.into_iter().map(|k| k.into_bound(py)).collect(), None);
    }
    let mut out = Vec::new();
    match obj.try_iter() {
        Ok(iter) => {
            for item in iter {
                match item {
                    Ok(item) => out.push(item),
                    Err(e) => return (out, Some(e)),
                }
            }
            (out, None)
        }
        Err(e) => (out, Some(e)),
    }
}

type Pairs<'py> = Vec<(Bound<'py, PyAny>, Bound<'py, PyAny>)>;

/// An element read from a source: prepared natively while the source was held, or an owned
/// object to prepare afterwards (preparation may run Python code).
pub(crate) enum Raw<'py> {
    Ready(Prepared<'py>),
    Later(Bound<'py, PyAny>),
}

enum RawValue<'py> {
    Ready(Coerced),
    Later(Bound<'py, PyAny>),
}

/// A `(key, value)` pair read from a source, or a sequence element still to be unpacked.
enum RawPair<'py> {
    Pair(Raw<'py>, RawValue<'py>),
    Item(Bound<'py, PyAny>, usize),
}

#[inline]
fn raw_key<'py>(obj: &Bound<'py, PyAny>, schema: &Schema) -> Raw<'py> {
    match keys::native_key(obj, schema) {
        Some(native) => Raw::Ready(native),
        None => Raw::Later(obj.clone()),
    }
}

/// [`raw_key`] for dict tables (`str` keys stay objects).
#[inline]
fn raw_dict_key<'py>(obj: &Bound<'py, PyAny>, schema: &Schema) -> Raw<'py> {
    match keys::native_key_dict(obj, schema) {
        Some(native) => Raw::Ready(native),
        None => Raw::Later(obj.clone()),
    }
}

/// A value read inside a source's critical section, possibly through a borrowed reference.
/// Only exact `int`/`float`/`bool`/`str` values are converted in place, and only when the
/// schema is pure: converting anything else — even unsuccessfully — may run Python code (a
/// subclass's `__str__`/`__repr__` in an error message, …) that could free the borrowed
/// item, so it gets a strong reference and is converted after the critical section.
#[inline]
fn raw_value<'py>(obj: &Bound<'py, PyAny>, schema: &Schema, pure: bool) -> RawValue<'py> {
    if schema.is_any() {
        // Untyped dicts store value objects as they are (see `value_storage`).
        return RawValue::Ready(Coerced::Object(obj.clone().unbind()));
    }
    if pure
        && is_plain_scalar(obj)
        && let Ok(coerced) = schema.coerce(obj)
    {
        return RawValue::Ready(coerced);
    }
    RawValue::Later(obj.clone())
}

/// An exact `int`, `float`, `bool` or `str`: values whose conversion by a pure schema runs
/// no Python code.
#[inline]
fn is_plain_scalar(obj: &Bound<'_, PyAny>) -> bool {
    obj.is_exact_instance_of::<PyInt>()
        || obj.is_exact_instance_of::<PyFloat>()
        || obj.is_exact_instance_of::<PyBool>()
        || obj.is_exact_instance_of::<PyString>()
}

/// Read what `iter(obj)` yields, classifying exact `int`/`str` items natively while the
/// source is held: a builtin `list` or `dict` inside one critical section, a `tuple`
/// directly. Returns the error that stopped iteration, if any.
pub(crate) fn read_elements<'py>(obj: &Bound<'py, PyAny>, schema: &Schema) -> (Vec<Raw<'py>>, Option<PyErr>) {
    let py = obj.py();
    if let Ok(list) = obj.cast_exact::<PyList>() {
        let items = with_critical_section(list.as_any(), || {
            let p = list.as_ptr();
            // SAFETY: we hold the list's critical section and run no Python code, so its size
            // and the borrowed item pointers stay valid while we read them.
            let n = unsafe { ffi::PyList_GET_SIZE(p) };
            (0..n)
                .map(|i| {
                    // SAFETY: `i` is in bounds (see above).
                    let item = unsafe { Borrowed::from_ptr(py, ffi::PyList_GET_ITEM(p, i)) };
                    raw_key(&item, schema)
                })
                .collect()
        });
        return (items, None);
    }
    if let Ok(tuple) = obj.cast_exact::<PyTuple>() {
        return (tuple.iter_borrowed().map(|item| raw_key(&item, schema)).collect(), None);
    }
    if let Ok(dict) = obj.cast_exact::<PyDict>() {
        let items = with_critical_section(dict.as_any(), || {
            let mut out = Vec::with_capacity(dict.len());
            let (mut pos, mut k, mut v) = (0, std::ptr::null_mut(), std::ptr::null_mut());
            // SAFETY: we hold the dict's critical section and run no Python code, so the
            // iteration state and the borrowed keys stay valid.
            while unsafe { ffi::PyDict_Next(dict.as_ptr(), &mut pos, &mut k, &mut v) } != 0 {
                // SAFETY: `k` is a live borrowed key (see above).
                let key = unsafe { Borrowed::from_ptr(py, k) };
                out.push(raw_key(&key, schema));
            }
            out
        });
        return (items, None);
    }
    let (items, err) = collect_iterable(obj);
    (items.into_iter().map(|item| raw_key(&item, schema)).collect(), err)
}

/// The pairs `dict(arg)` would insert, read like [`read_elements`].
fn read_pairs<'py>(
    arg: &Bound<'py, PyAny>,
    key_schema: &Schema,
    value_schema: &Schema,
) -> (Vec<RawPair<'py>>, Option<PyErr>) {
    let py = arg.py();
    let pure = value_schema.is_pure();
    let pair = |k: &Bound<'py, PyAny>, v: &Bound<'py, PyAny>| {
        RawPair::Pair(raw_dict_key(k, key_schema), raw_value(v, value_schema, pure))
    };
    // Unpack exact 2-tuples in place (their items are kept alive by the tuple).
    let element = |item: &Bound<'py, PyAny>, index: usize| match item.cast_exact::<PyTuple>() {
        Ok(t) if t.len() == 2 => {
            // SAFETY: indices 0 and 1 are in bounds of this tuple.
            let (k, v) = unsafe { (t.get_borrowed_item_unchecked(0), t.get_borrowed_item_unchecked(1)) };
            pair(&k, &v)
        }
        _ => RawPair::Item(item.clone(), index),
    };
    if let Ok(dict) = arg.cast_exact::<PyDict>() {
        let items = with_critical_section(dict.as_any(), || {
            let mut out = Vec::with_capacity(dict.len());
            let (mut pos, mut k, mut v) = (0, std::ptr::null_mut(), std::ptr::null_mut());
            // SAFETY: we hold the dict's critical section and run no Python code (only exact-type
            // reads, see `raw_value`), so the iteration state and borrowed items stay valid.
            while unsafe { ffi::PyDict_Next(dict.as_ptr(), &mut pos, &mut k, &mut v) } != 0 {
                // SAFETY: live borrowed key and value (see above).
                let (k, v) = unsafe { (Borrowed::from_ptr(py, k), Borrowed::from_ptr(py, v)) };
                out.push(pair(&k, &v));
            }
            out
        });
        return (items, None);
    }
    if let Ok(list) = arg.cast_exact::<PyList>() {
        let items = with_critical_section(list.as_any(), || {
            let p = list.as_ptr();
            // SAFETY: we hold the list's critical section and run no Python code, so its size
            // and the borrowed item pointers stay valid while we read them.
            let n = unsafe { ffi::PyList_GET_SIZE(p) };
            (0..n)
                .map(|i| {
                    // SAFETY: `i` is in bounds (see above).
                    let item = unsafe { Borrowed::from_ptr(py, ffi::PyList_GET_ITEM(p, i)) };
                    element(&item, i as usize)
                })
                .collect()
        });
        return (items, None);
    }
    if let Ok(tuple) = arg.cast_exact::<PyTuple>() {
        return (tuple.iter_borrowed().enumerate().map(|(i, item)| element(&item, i)).collect(), None);
    }
    let later = |(k, v): (Bound<'py, PyAny>, Bound<'py, PyAny>)| RawPair::Pair(Raw::Later(k), RawValue::Later(v));
    if let Ok(d) = arg.cast::<Dict>() {
        let items = d.get().inner.read(py).items(py);
        return (items.into_iter().map(|(k, v)| later((k.into_bound(py), v.into_bound(py)))).collect(), None);
    }
    match arg.hasattr(pyo3::intern!(py, "keys")) {
        Ok(true) => {
            let (pairs, err) = collect_mapping(arg);
            return (pairs.into_iter().map(later).collect(), err);
        }
        Ok(false) => {}
        Err(e) => return (Vec::new(), Some(e)),
    }
    let (items, err) = collect_iterable(arg);
    (items.into_iter().enumerate().map(|(i, item)| RawPair::Item(item, i)).collect(), err)
}

/// Strong references to the items of a builtin `dict`, read inside its critical section.
fn collect_dict<'py>(dict: &Bound<'py, PyDict>) -> (Pairs<'py>, Option<PyErr>) {
    let out = RefCell::new(Vec::with_capacity(dict.len()));
    let res = dict.locked_for_each(|k, v| {
        out.borrow_mut().push((k, v));
        Ok(())
    });
    (out.into_inner(), res.err())
}

/// Mapping protocol: `for k in m.keys(): m[k]`.
fn collect_mapping<'py>(map: &Bound<'py, PyAny>) -> (Pairs<'py>, Option<PyErr>) {
    let keys = match map.call_method0(pyo3::intern!(map.py(), "keys")) {
        Ok(keys) => keys,
        Err(e) => return (Vec::new(), Some(e)),
    };
    let (keys, mut err) = collect_iterable(&keys);
    let mut out = Vec::with_capacity(keys.len());
    for k in keys {
        match map.get_item(&k) {
            Ok(v) => out.push((k, v)),
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    (out, err)
}

fn pair_of<'py>(item: Bound<'py, PyAny>, index: usize) -> PyResult<(Bound<'py, PyAny>, Bound<'py, PyAny>)> {
    if let Ok(t) = item.cast_exact::<PyTuple>()
        && t.len() == 2
    {
        return Ok((t.get_item(0)?, t.get_item(1)?));
    }
    let py = item.py();
    let iter = item.try_iter().map_err(|e| {
        if e.is_instance_of::<PyTypeError>(py) {
            PyTypeError::new_err(format!("cannot convert dictionary update sequence element #{index} to a sequence"))
        } else {
            e
        }
    })?;
    let seq = iter.collect::<PyResult<Vec<_>>>()?;
    match <[_; 2]>::try_from(seq) {
        Ok([k, v]) => Ok((k, v)),
        Err(seq) => Err(PyValueError::new_err(format!(
            "dictionary update sequence element #{index} has length {}; 2 is required",
            seq.len()
        ))),
    }
}

/// Guard for recursive reprs (`Py_ReprEnter`/`Py_ReprLeave`).
pub(crate) struct ReprGuard<'a, 'py>(&'a Bound<'py, PyAny>);

impl<'a, 'py> ReprGuard<'a, 'py> {
    /// `Ok(None)` when `obj` is already being repr'd further up the stack.
    pub(crate) fn enter(obj: &'a Bound<'py, PyAny>) -> PyResult<Option<Self>> {
        // SAFETY: `obj` is a valid object and we are attached to the interpreter.
        match unsafe { ffi::Py_ReprEnter(obj.as_ptr()) } {
            0 => Ok(Some(ReprGuard(obj))),
            n if n > 0 => Ok(None),
            _ => Err(PyErr::fetch(obj.py())),
        }
    }
}

impl Drop for ReprGuard<'_, '_> {
    fn drop(&mut self) {
        // SAFETY: paired with the successful `Py_ReprEnter` in `enter`, on the same thread.
        unsafe { ffi::Py_ReprLeave(self.0.as_ptr()) }
    }
}

/// Equality of two values as `dict` compares them: identity first, then `==`. Values read
/// from native storage have no identity, so there two NaN floats count as equal
/// (`nan_equal`; documented deviation of native float storage).
pub(crate) fn values_equal(a: &Bound<'_, PyAny>, b: &Bound<'_, PyAny>, nan_equal: bool) -> PyResult<bool> {
    if nan_equal { objects_equal(a, b) } else { Ok(a.is(b) || a.eq(b)?) }
}

/// Equality of two objects as containers compare them: identity first, and floats that are
/// both NaN count as equal (native float storage does not preserve NaN identity).
pub(crate) fn objects_equal(a: &Bound<'_, PyAny>, b: &Bound<'_, PyAny>) -> PyResult<bool> {
    if a.is(b) {
        return Ok(true);
    }
    if let (Ok(x), Ok(y)) = (a.cast_exact::<PyFloat>(), b.cast_exact::<PyFloat>()) {
        let (x, y) = (x.value(), y.value());
        return Ok(x == y || (x.is_nan() && y.is_nan()));
    }
    a.eq(b)
}

/// Values at two positions of native columns, compared like Python.
fn native_values_equal(a: &Column, i: usize, b: &Column, j: usize) -> Option<bool> {
    Some(keys::values_equal(&a.get(i)?, &b.get(j)?))
}

/// INTERFACE: a `Dict`'s keys borrowed under its read lock (see [`Dict::with_keys`]). Each
/// variant is a [`KeyIndex`](rustique_core::index::KeyIndex): `iter()` yields the live
/// `(position, key)` pairs in insertion order (tombstoned slots are skipped).
pub enum DictKeysRef<'a> {
    /// No keys yet (an untyped dict before its first key).
    Empty,
    /// Native `int` keys (each read back as a new `int`).
    Int(&'a rustique_core::index::KeyIndex<i64>),
    /// Native `str` keys (set-style tables only; dicts keep `str` keys as objects).
    NativeStr(&'a rustique_core::index::KeyIndex<rustique_core::CompactString>),
    /// Key objects, including exact `str` keys (the stored objects themselves).
    Object(&'a rustique_core::index::KeyIndex<Py<PyAny>, ()>),
}

/// An insertion-ordered mapping with native key and value storage.
#[pyclass(module = "rustique", frozen, mapping, subclass, immutable_type)]
pub struct Dict {
    key_schema: Arc<Schema>,
    value_schema: Arc<Schema>,
    inner: Locked<DictInner>,
    /// The argument tuple `__new__` already applied (exact `Dict` only), so that the
    /// `__init__` call `type.__call__` makes right after it does not apply it again.
    init_args: AtomicUsize,
    /// The number of live entries, mirrored for lock-free `len()`: every change publishes
    /// it while still holding the write lock ([`Dict::publish_len`]); readers use relaxed
    /// loads, like CPython's `ma_used`.
    len: AtomicUsize,
}

impl Dict {
    /// INTERFACE. Construct like `dict(arg, **kwargs)` with the given key/value schemas.
    pub fn from_args<'py>(
        py: Python<'py>,
        key_schema: Arc<Schema>,
        value_schema: Arc<Schema>,
        arg: Option<&Bound<'py, PyAny>>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Bound<'py, Dict>> {
        Bound::new(py, Dict::build(py, key_schema, value_schema, arg, kwargs)?)
    }

    /// INTERFACE (builtins kernels): run `f` on the keys under the read lock, without boxing.
    /// `f` must not run Python code (no `__eq__`/`__hash__`/`__lt__`, no decref of objects
    /// that may have finalizers, no allocation of GC-tracked objects).
    pub fn with_keys<R>(&self, py: Python<'_>, f: impl FnOnce(DictKeysRef<'_>) -> R) -> R {
        let inner = self.inner.read(py);
        f(match inner.table.store() {
            KeyStore::Empty => DictKeysRef::Empty,
            KeyStore::Int(ki) => DictKeysRef::Int(ki),
            KeyStore::Str(ki) => DictKeysRef::NativeStr(ki),
            KeyStore::Object(ki) => DictKeysRef::Object(ki),
        })
    }

    /// INTERFACE.
    pub fn key_schema(&self) -> &Arc<Schema> {
        &self.key_schema
    }

    /// INTERFACE.
    pub fn value_schema(&self) -> &Arc<Schema> {
        &self.value_schema
    }

    fn assemble(key_schema: Arc<Schema>, value_schema: Arc<Schema>, inner: DictInner) -> Dict {
        let len = AtomicUsize::new(inner.table.len());
        Dict { key_schema, value_schema, inner: Locked::new(inner), init_args: AtomicUsize::new(0), len }
    }

    /// Publish the entry count of `inner` (call while holding its write lock).
    #[inline(always)]
    fn publish_len(&self, inner: &DictInner) {
        self.len.store(inner.table.len(), Ordering::Relaxed);
    }

    fn empty(key_schema: Arc<Schema>, value_schema: Arc<Schema>) -> Dict {
        let inner = DictInner::new(&key_schema, &value_schema);
        Dict::assemble(key_schema, value_schema, inner)
    }

    pub(crate) fn build<'py>(
        py: Python<'py>,
        key_schema: Arc<Schema>,
        value_schema: Arc<Schema>,
        arg: Option<&Bound<'py, PyAny>>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Dict> {
        if let Some(arg) = arg
            && key_schema.is_any()
            && value_schema.is_any()
            && let Ok(src) = arg.cast_exact::<PyDict>()
            && let Some(inner) = DictInner::from_builtin(py, src)
        {
            let dict = Dict::assemble(key_schema, value_schema, inner);
            if let Some(kwargs) = kwargs {
                dict.update_from(py, kwargs.as_any())?;
            }
            return Ok(dict);
        }
        if let Some(arg) = arg
            && let Ok(other) = arg.cast::<Dict>()
            && other.get().same_schema(&key_schema, &value_schema)
        {
            let inner = other.get().inner.read(py).clone_ref(py);
            let dict = Dict::assemble(key_schema, value_schema, inner);
            if let Some(kwargs) = kwargs {
                dict.update_from(py, kwargs.as_any())?;
            }
            return Ok(dict);
        }
        if key_schema.is_any() && value_schema.is_any() {
            let mut inner = DictInner::new(&key_schema, &value_schema);
            if let Some(arg) = arg {
                inner.update_private(py, arg)?;
            }
            if let Some(kwargs) = kwargs {
                inner.update_private(py, kwargs.as_any())?;
            }
            return Ok(Dict::assemble(key_schema, value_schema, inner));
        }
        let dict = Dict::empty(key_schema, value_schema);
        if let Some(arg) = arg {
            dict.update_from(py, arg)?;
        }
        if let Some(kwargs) = kwargs {
            dict.update_from(py, kwargs.as_any())?;
        }
        Ok(dict)
    }

    fn same_schema(&self, key_schema: &Arc<Schema>, value_schema: &Arc<Schema>) -> bool {
        (Arc::ptr_eq(&self.key_schema, key_schema) || *self.key_schema == **key_schema)
            && (Arc::ptr_eq(&self.value_schema, value_schema) || *self.value_schema == **value_schema)
    }

    fn is_typed(&self) -> bool {
        !(self.key_schema.is_any() && self.value_schema.is_any())
    }

    /// `Dict` or `Dict[K, V]`, for reprs and error messages.
    fn type_name(&self) -> String {
        if self.is_typed() { format!("Dict[{}, {}]", self.key_schema, self.value_schema) } else { "Dict".to_owned() }
    }

    fn prepare_key<'py>(&self, key: Bound<'py, PyAny>) -> PyResult<Prepared<'py>> {
        keys::prepare_owned_dict(key, &self.key_schema, || self.type_name())
    }

    fn prepare_value(&self, value: &Bound<'_, PyAny>) -> PyResult<Coerced> {
        if self.value_schema.is_any() {
            // Untyped dicts store value objects as they are (see `value_storage`).
            return Ok(Coerced::Object(value.clone().unbind()));
        }
        self.value_schema.coerce(value).map_err(|e| e.into_pyerr(&self.type_name(), None))
    }

    /// Finish reading: unpack, validate and convert in order. Stops at the first invalid
    /// item; the valid prefix is still returned (like `dict.update`).
    fn finish_pairs<'py>(
        &self,
        raw: Vec<RawPair<'py>>,
        mut err: Option<PyErr>,
    ) -> (Vec<(Prepared<'py>, Coerced)>, Option<PyErr>) {
        let mut out = Vec::with_capacity(raw.len());
        for item in raw {
            let prepared = (|| -> PyResult<(Prepared<'py>, Coerced)> {
                let (k, v) = match item {
                    RawPair::Pair(k, v) => (k, v),
                    RawPair::Item(obj, index) => {
                        let (k, v) = pair_of(obj, index)?;
                        (Raw::Later(k), RawValue::Later(v))
                    }
                };
                let key = match k {
                    Raw::Ready(native) => native,
                    Raw::Later(obj) => self.prepare_key(obj)?,
                };
                let value = match v {
                    RawValue::Ready(coerced) => coerced,
                    RawValue::Later(obj) => self.prepare_value(&obj)?,
                };
                Ok((key, value))
            })();
            match prepared {
                Ok(item) => out.push(item),
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        (out, err)
    }

    /// Insert or overwrite prepared items in order, then report `err`.
    fn insert_items<'py>(
        &self,
        py: Python<'py>,
        items: Vec<(Prepared<'py>, Coerced)>,
        err: Option<PyErr>,
    ) -> PyResult<()> {
        if items.len() > 1 {
            let mut inner = self.inner.write(py);
            inner.table.reserve(items.len());
            reserve_storage(&mut inner.values, items.len());
        }
        let mut displaced = Vec::new();
        let res = keys::batch_upsert(py, &self.inner, items, |inner, value, slot| {
            match slot {
                Slot::Existing(pos) => displaced.extend(inner.values.set(py, pos, value)),
                Slot::Inserted(pos) => {
                    debug_assert_eq!(pos, inner.values.len());
                    inner.values.push(py, value);
                }
                Slot::Absent(key) => inner.push_entry(py, key, value, &mut displaced)?,
            }
            self.publish_len(inner);
            Ok(())
        });
        drop(displaced);
        res?;
        err.map_or(Ok(()), Err)
    }

    fn update_from(&self, py: Python<'_>, arg: &Bound<'_, PyAny>) -> PyResult<()> {
        if !self.is_typed() {
            // `dict.update(mapping)` with a builtin dict or a Dict: merge under one write lock
            // as far as no Python code is needed, then the general path for the rest.
            let items: Option<Vec<(MergeKey, Py<PyAny>)>> = if let Ok(dict) = arg.cast_exact::<PyDict>() {
                let (pairs, err) = collect_dict(dict);
                if let Some(err) = err {
                    return Err(err);
                }
                Some(pairs.into_iter().map(|(k, v)| (MergeKey::Obj(k.unbind()), v.unbind())).collect())
            } else if let Ok(other) = arg.cast_exact::<Dict>() {
                Some(other.get().inner.read(py).merge_items(py))
            } else {
                None
            };
            if let Some(items) = items {
                let done = self.merge_fast(py, &items);
                if done < items.len() {
                    let raw = items[done..]
                        .iter()
                        .map(|(k, v)| {
                            let k = match k {
                                MergeKey::Int(n) => Key::from_int(py, *n).obj,
                                MergeKey::Obj(o) => o.bind(py).clone(),
                            };
                            RawPair::Pair(Raw::Later(k), RawValue::Later(v.bind(py).clone()))
                        })
                        .collect();
                    let (items, err) = self.finish_pairs(raw, None);
                    self.insert_items(py, items, err)?;
                }
                return Ok(());
            }
        }
        let (raw, err) = read_pairs(arg, &self.key_schema, &self.value_schema);
        let (items, err) = self.finish_pairs(raw, err);
        self.insert_items(py, items, err)
    }

    /// Merge `items` in order into an untyped dict under one write lock while every key can
    /// be placed without running Python code; returns how many were merged.
    fn merge_fast(&self, py: Python<'_>, items: &[(MergeKey, Py<PyAny>)]) -> usize {
        let mut replaced = Vec::new();
        let mut moved = Vec::new();
        let mut boxed = Vec::new();
        let mut done = 0;
        {
            let mut inner = self.inner.write(py);
            inner.table.reserve(items.len());
            for (key, value) in items {
                let slot = match key {
                    MergeKey::Int(n) => match inner.table.upsert_int(*n) {
                        Some(slot) => Some(slot),
                        None => {
                            // An object table: the key needs an `int` object (creating one is
                            // fine under the lock).
                            let obj = Key::from_int(py, *n).obj.unbind();
                            let slot = inner.table.upsert_fast_key(py, &FastKey::Int(*n, obj.as_ptr()));
                            boxed.push(obj);
                            slot
                        }
                    },
                    // SAFETY: the key object is live (held by `items`).
                    MergeKey::Obj(obj) => match unsafe { FastKey::of(obj.as_ptr()) } {
                        Some(probe) => inner.table.upsert_fast_key(py, &probe),
                        None => None,
                    },
                };
                let Some((pos, inserted)) = slot else { break };
                let Storage::Object(objs) = &mut inner.values else { break };
                if inserted {
                    objs.push(value.clone_ref(py));
                    inner.inserted(&mut moved);
                } else {
                    replaced.push(std::mem::replace(&mut objs[pos], value.clone_ref(py)));
                }
                done += 1;
            }
            self.publish_len(&inner);
        }
        drop(replaced);
        drop(moved);
        drop(boxed);
        done
    }

    fn set_item(&self, py: Python<'_>, key: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let key = self.prepare_key(key.clone())?;
        let mut displaced = Vec::new();
        if self.value_schema.is_any() {
            // Untyped values keep their identity in object storage.
            keys::upsert_one(py, &self.inner, key, value, |inner, value, slot| {
                match slot {
                    Slot::Existing(pos) => displaced.extend(inner.values.set_any(py, pos, value)),
                    Slot::Inserted(_) => inner.values.push_any(py, value),
                    Slot::Absent(key) => {
                        inner.table.push(py, key)?;
                        inner.values.push_any(py, value);
                    }
                }
                self.publish_len(inner);
                Ok(())
            })?;
        } else {
            let value = self.prepare_value(value)?;
            keys::upsert_one(py, &self.inner, key, value, |inner, value, slot| {
                match slot {
                    Slot::Existing(pos) => displaced.extend(inner.values.set(py, pos, value)),
                    Slot::Inserted(_) => inner.values.push(py, value),
                    Slot::Absent(key) => inner.push_entry(py, key, value, &mut displaced)?,
                }
                self.publish_len(inner);
                Ok(())
            })?;
        }
        drop(displaced);
        Ok(())
    }

    fn get_value(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<Option<Py<PyAny>>> {
        keys::lookup(py, &self.inner, key, classify_key, |inner, pos| pos.map(|p| inner.value(py, p)))
    }

    /// Remove `key`, returning its value (`None` if absent).
    fn remove_key(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<Option<Py<PyAny>>> {
        let mut displaced = Vec::new();
        let value = keys::lookup_mut(py, &self.inner, key, classify_key, |inner, pos| {
            let value = pos.map(|p| inner.remove_entry(py, p, &mut displaced));
            self.publish_len(inner);
            value
        })?;
        drop(displaced);
        Ok(value)
    }

    pub(crate) fn len(&self, _py: Python<'_>) -> usize {
        self.len.load(Ordering::Relaxed)
    }

    fn items_snapshot(&self, py: Python<'_>) -> Vec<(Py<PyAny>, Py<PyAny>)> {
        self.inner.read(py).items(py)
    }

    fn key_snapshot(&self, py: Python<'_>) -> Vec<Py<PyAny>> {
        self.inner.read(py).table.key_objects(py)
    }

    /// Native comparison when both sides store native keys and values (typed dicts), in
    /// place under both read locks. `None`: not applicable.
    fn native_eq(&self, py: Python<'_>, other: &Dict) -> Option<bool> {
        let (a, b) = crate::sync::read_both(py, &self.inner, &other.inner);
        let (Storage::Native(mine), Storage::Native(theirs)) = (&a.values, &b.values) else {
            return None;
        };
        if a.table.len() != b.table.len() {
            return Some(false);
        }
        for pos in a.table.index()?.iter() {
            let found = match (a.table.store(), b.table.store()) {
                (KeyStore::Int(x), KeyStore::Int(y)) => y.get(&x.raw_keys()[pos]),
                (KeyStore::Str(x), KeyStore::Str(y)) => y.get(x.raw_keys()[pos].as_str()),
                _ => return None,
            };
            match found {
                None => return Some(false),
                Some(j) if !native_values_equal(mine, pos, theirs, j)? => return Some(false),
                Some(_) => {}
            }
        }
        Some(true)
    }

    /// Values of `self` and `other` paired by key (in `self`'s order), matched under both
    /// read locks without running Python code. `Err(())`: the key sets differ; `None`: a key
    /// could not be matched without Python code (use the general path).
    fn pair_with_dict(&self, py: Python<'_>, other: &Dict) -> Option<Result<Paired, ()>> {
        let (a, b) = crate::sync::read_both(py, &self.inner, &other.inner);
        if a.table.len() != b.table.len() {
            return Some(Err(()));
        }
        let mut out = Vec::with_capacity(a.table.len());
        let native = a.values_native() || b.values_native();
        let Some(index) = a.table.index() else { return Some(Ok((out, native))) };
        for pos in index.iter() {
            let probe = match a.table.store() {
                KeyStore::Int(ki) => FastKey::Int(ki.raw_keys()[pos], std::ptr::null_mut()),
                // SAFETY: stored keys are live while the read lock is held.
                KeyStore::Object(ki) => unsafe { FastKey::of(ki.raw_keys()[pos].as_ptr())? },
                _ => return None,
            };
            match b.table.find_fast(&probe)? {
                None => return Some(Err(())),
                Some(j) => {
                    if !native && let (Storage::Object(x), Storage::Object(y)) = (&a.values, &b.values) {
                        // SAFETY: both values are live while the read locks are held.
                        match unsafe { settle(x[pos].as_ptr(), y[j].as_ptr()) } {
                            Some(true) => continue,
                            Some(false) => return Some(Err(())),
                            None => {}
                        }
                    }
                    out.push((a.value(py, pos), b.value(py, j)));
                }
            }
        }
        Some(Ok((out, native)))
    }

    /// [`Dict::pair_with_dict`] against an exact builtin `dict` (read inside its critical
    /// section; our read lock is held meanwhile and only pure reads run inside).
    fn pair_with_builtin(&self, py: Python<'_>, other: &Bound<'_, PyDict>) -> Option<Result<Paired, ()>> {
        let inner = self.inner.read(py);
        if inner.table.len() != other.len() {
            return Some(Err(()));
        }
        let native = inner.values_native();
        with_critical_section(other.as_any(), || {
            let mut out = Vec::with_capacity(other.len());
            let (mut pos, mut k, mut v) = (0, std::ptr::null_mut(), std::ptr::null_mut());
            // SAFETY: we hold the dict's critical section and run no Python code (fast probes
            // only), so the iteration state and the borrowed items stay valid.
            unsafe {
                while ffi::PyDict_Next(other.as_ptr(), &mut pos, &mut k, &mut v) != 0 {
                    let probe = FastKey::of(k)?;
                    match inner.table.find_fast(&probe)? {
                        None => return Some(Err(())),
                        Some(at) => {
                            if let Storage::Object(ours) = &inner.values {
                                match settle(ours[at].as_ptr(), v) {
                                    Some(true) => continue,
                                    Some(false) => return Some(Err(())),
                                    None => {}
                                }
                            }
                            out.push((inner.value(py, at), Bound::from_borrowed_ptr(py, v).unbind()));
                        }
                    }
                }
            }
            Some(Ok((out, native)))
        })
    }

    /// Compare paired values (outside every lock: `__eq__` may run).
    fn values_all_equal(py: Python<'_>, (pairs, native): Paired) -> PyResult<bool> {
        for (a, b) in &pairs {
            if !values_equal(a.bind(py), b.bind(py), native)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn eq_dict(&self, py: Python<'_>, other: &Dict) -> PyResult<bool> {
        if let Some(eq) = self.native_eq(py, other) {
            return Ok(eq);
        }
        match self.pair_with_dict(py, other) {
            Some(Ok(pairs)) => return Dict::values_all_equal(py, pairs),
            Some(Err(())) => return Ok(false),
            None => {}
        }
        let (keys, values, native) = {
            let inner = self.inner.read(py);
            (inner.table.element_keys(py)?, inner.values_list(py), inner.values_native())
        };
        if keys.len() != other.len(py) {
            return Ok(false);
        }
        for (key, v) in keys.iter().zip(values) {
            let theirs = keys::with_entry(py, &other.inner, key, |inner, pos| {
                pos.map(|p| (inner.value(py, p), inner.values_native()))
            })?;
            match theirs {
                Some((theirs, n)) if values_equal(v.bind(py), theirs.bind(py), native || n)? => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    /// Native comparison with an exact builtin `dict` whose keys are exact `int`/`str` and
    /// whose values are exact `int`/`float`/`bool`/`str`. `None`: not applicable.
    fn native_eq_builtin(&self, py: Python<'_>, other: &Bound<'_, PyDict>) -> Option<bool> {
        let inner = self.inner.read(py);
        if inner.table.len() != other.len() {
            return Some(false);
        }
        let col = match &inner.values {
            Storage::Native(col) => col,
            Storage::Empty => return Some(true),
            Storage::Object(_) => return None,
        };
        // Our read lock is held while we enter the dict's critical section; only pure Rust and
        // exact-type reads run inside it.
        with_critical_section(other.as_any(), || {
            let (mut pos, mut k, mut v) = (0, std::ptr::null_mut(), std::ptr::null_mut());
            // SAFETY: we hold the dict's critical section and run no Python code, so the
            // iteration state and the borrowed items stay valid.
            while unsafe { ffi::PyDict_Next(other.as_ptr(), &mut pos, &mut k, &mut v) } != 0 {
                // SAFETY: live borrowed key and value (see above).
                let (k, v) = unsafe { (Borrowed::from_ptr(py, k), Borrowed::from_ptr(py, v)) };
                // SAFETY: `k` is a live key of the dict (see above).
                let probe = unsafe { FastKey::of(k.as_ptr()) }?;
                let found = inner.table.find_fast(&probe)?;
                let Some(at) = found else { return Some(false) };
                if !keys::values_equal(&col.get(at)?, &keys::native_value(&v)?) {
                    return Some(false);
                }
            }
            Some(true)
        })
    }

    fn eq_mapping(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(dict) = other.cast_exact::<PyDict>() {
            if let Some(eq) = self.native_eq_builtin(py, dict) {
                return Ok(eq);
            }
            match self.pair_with_builtin(py, dict) {
                Some(Ok(pairs)) => return Dict::values_all_equal(py, pairs),
                Some(Err(())) => return Ok(false),
                None => {}
            }
        }
        let (items, native) = {
            let inner = self.inner.read(py);
            (inner.items(py), inner.values_native())
        };
        if items.len() != other.len()? {
            return Ok(false);
        }
        let dict = other.cast::<PyDict>().ok();
        for (k, v) in items {
            let theirs = match dict {
                Some(d) => d.get_item(&k)?,
                None => match other.get_item(&k) {
                    Ok(v) => Some(v),
                    Err(e) if e.is_instance_of::<PyKeyError>(py) => None,
                    Err(e) => return Err(e),
                },
            };
            match theirs {
                Some(theirs) if values_equal(v.bind(py), &theirs, native)? => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    /// `self == other` for supported operands; `None` means `NotImplemented`.
    fn equals(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Option<bool>> {
        let py = slf.py();
        if let Ok(other) = other.cast::<Dict>() {
            if other.is(slf) {
                return Ok(Some(true));
            }
            return slf.get().eq_dict(py, other.get()).map(Some);
        }
        if is_builtin_mapping(other) {
            return slf.get().eq_mapping(py, other).map(Some);
        }
        Ok(None)
    }

    /// A copy with this dict's schemas when `keep_schema`, untyped otherwise.
    fn derived(&self, py: Python<'_>, keep_schema: bool) -> PyResult<Dict> {
        if keep_schema {
            let inner = self.inner.read(py).clone_ref(py);
            return Ok(Dict::assemble(self.key_schema.clone(), self.value_schema.clone(), inner));
        }
        let mut inner = DictInner::new(&Schema::Any, &Schema::Any);
        for (k, v) in self.items_snapshot(py) {
            inner.set_private(py, k.bind(py), v.bind(py))?;
        }
        Ok(Dict::assemble(schema::any(), schema::any(), inner))
    }

    fn schema_type_of<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if self.is_typed() {
            crate::typing::dict_alias_for(py, &self.key_schema, &self.value_schema)
        } else {
            Ok(py.get_type::<Dict>().into_any())
        }
    }

    fn iter_of(slf: &Bound<'_, Self>, kind: ItemKind, reverse: bool) -> DictIterator {
        let py = slf.py();
        let (len, entries) = {
            let inner = slf.get().inner.read(py);
            (inner.table.len(), inner.table.entries())
        };
        let state = IterState {
            pos: AtomicUsize::new(if reverse { entries } else { 0 }),
            expected: AtomicUsize::new(len),
            remaining: AtomicUsize::new(len),
            exhausted: AtomicBool::new(false),
        };
        // Items iteration recycles its result tuple (see `DictIterator::item`).
        let result = (kind == ItemKind::Items)
            .then(|| PyTuple::new(py, [py.None(), py.None()]).map_or_else(|_| py.None(), |t| t.into_any().unbind()));
        DictIterator { dict: slf.clone().unbind(), kind, reverse, state, result }
    }
}

/// Decref objects directly (the raw slots run without PyO3's attach count, where dropping a
/// `Py` would defer the decref). Call only after releasing every lock.
#[inline(always)]
fn release(obj: Option<Py<PyAny>>) {
    if let Some(obj) = obj {
        // SAFETY: attached to the interpreter; `into_ptr` hands over our reference.
        unsafe { ffi::Py_DECREF(obj.into_ptr()) };
    }
}

/// [`release`] for a batch.
#[inline(always)]
fn release_all(objs: Vec<Py<PyAny>>) {
    if !objs.is_empty() {
        objs.into_iter().for_each(|obj| release(Some(obj)));
    }
}

impl Dict {
    /// `self[key] = value` for an untyped dict and a fast key (raw slot); `None` means
    /// "use the general path" (nothing was changed).
    ///
    /// # Safety
    /// `key` must be a live object.
    #[inline(always)]
    unsafe fn set_fast(&self, py: Python<'_>, key: *mut ffi::PyObject, value: Borrowed<'_, '_, PyAny>) -> Option<()> {
        if !(self.key_schema.is_any() && self.value_schema.is_any()) {
            return None;
        }
        let replaced = {
            let mut inner = self.inner.write(py);
            // SAFETY: `key` is live (guaranteed by the caller).
            match unsafe { inner.table.find_object(key) }? {
                Some(pos) => match &mut inner.values {
                    Storage::Object(objs) => std::mem::replace(&mut objs[pos], value.to_owned().unbind()),
                    values => values.set_any(py, pos, &value)?,
                },
                None => {
                    drop(inner);
                    // SAFETY: as above.
                    return unsafe { self.insert_fast(py, key, value) };
                }
            }
        };
        release(Some(replaced));
        Some(())
    }

    /// The insertion half of [`Dict::set_fast`] (out of line: growth may move entries).
    ///
    /// # Safety
    /// `key` must be a live object.
    #[inline(never)]
    unsafe fn insert_fast(
        &self,
        py: Python<'_>,
        key: *mut ffi::PyObject,
        value: Borrowed<'_, '_, PyAny>,
    ) -> Option<()> {
        let mut replaced = None;
        let mut moved = Vec::new();
        {
            // SAFETY: `key` is live (guaranteed by the caller).
            let probe = unsafe { FastKey::of(key) }?;
            let mut inner = self.inner.write(py);
            let (pos, inserted) = inner.table.upsert_fast_key(py, &probe)?;
            if inserted {
                match &mut inner.values {
                    Storage::Object(objs) => objs.push(value.to_owned().unbind()),
                    values => values.push_any(py, &value),
                }
                self.publish_len(&inner);
                inner.inserted(&mut moved);
            } else {
                // Inserted by another thread meanwhile.
                replaced = match &mut inner.values {
                    Storage::Object(objs) => Some(std::mem::replace(&mut objs[pos], value.to_owned().unbind())),
                    values => values.set_any(py, pos, &value),
                };
            }
        }
        release(replaced);
        release_all(moved);
        Some(())
    }

    /// `del self[key]` for a fast key (raw slot); `None`: use the general path (a miss,
    /// which raises, or an undecidable comparison).
    #[inline(always)]
    fn del_fast(&self, py: Python<'_>, probe: &FastKey) -> Option<()> {
        let mut displaced = Vec::new();
        let value = {
            let mut inner = self.inner.write(py);
            let pos = inner.table.find_fast(probe)??;
            let value = inner.remove_entry(py, pos, &mut displaced);
            self.publish_len(&inner);
            value
        };
        release(Some(value));
        release_all(displaced);
        Some(())
    }

    /// `self.pop(key[, default])` for a fast key (raw method): the value or the default as a
    /// new reference; `None`: use the general path (a miss without default, which raises).
    #[inline(always)]
    fn pop_fast(
        &self,
        py: Python<'_>,
        key: *mut ffi::PyObject,
        default: Option<*mut ffi::PyObject>,
    ) -> Option<*mut ffi::PyObject> {
        let mut displaced = Vec::new();
        let value = {
            let mut inner = self.inner.write(py);
            // SAFETY: `key` is live for the duration of the call.
            match unsafe { inner.table.find_object(key) }? {
                Some(pos) => {
                    let value = inner.remove_entry(py, pos, &mut displaced);
                    self.publish_len(&inner);
                    value.into_ptr()
                }
                // SAFETY: attached; a new reference to the default.
                None => unsafe { ffi::Py_NewRef(default?) },
            }
        };
        release_all(displaced);
        Some(value)
    }

    /// `self.popitem()` (raw method): the `(key, value)` tuple; `None` when empty (the
    /// general path raises).
    #[inline(always)]
    fn popitem_fast(&self, py: Python<'_>) -> Option<*mut ffi::PyObject> {
        let mut displaced = Vec::new();
        let (key, value) = {
            let mut inner = self.inner.write(py);
            let item = inner.pop_last(py, &mut displaced)?;
            self.publish_len(&inner);
            item
        };
        release_all(displaced);
        // SAFETY: attached; the tuple steals both references (released on failure).
        unsafe {
            let (key, value) = (key.into_ptr(), value.into_ptr());
            let t = ffi::PyTuple_New(2);
            if t.is_null() {
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(value);
            } else {
                ffi::PyTuple_SET_ITEM(t, 0, key);
                ffi::PyTuple_SET_ITEM(t, 1, value);
            }
            Some(t)
        }
    }

    /// `self.setdefault(key, default)` for an untyped dict and a fast key (raw method): the
    /// value as a new reference; `None`: use the general path.
    #[inline(always)]
    fn setdefault_fast(
        &self,
        py: Python<'_>,
        probe: &FastKey,
        default: Borrowed<'_, '_, PyAny>,
    ) -> Option<*mut ffi::PyObject> {
        if !(self.key_schema.is_any() && self.value_schema.is_any()) {
            return None;
        }
        let mut moved = Vec::new();
        let value = {
            let mut inner = self.inner.write(py);
            let (pos, inserted) = inner.table.upsert_fast_key(py, probe)?;
            if inserted {
                match &mut inner.values {
                    Storage::Object(objs) => objs.push(default.to_owned().unbind()),
                    values => values.push_any(py, &default),
                }
                let value = inner.value_ptr(py, pos);
                self.publish_len(&inner);
                inner.inserted(&mut moved);
                value
            } else {
                inner.value_ptr(py, pos)
            }
        };
        release_all(moved);
        value
    }
}

impl DictIterator {
    /// `next(self)`: a new reference; NULL with an exception set on error, NULL without
    /// one when exhausted (the `tp_iternext` convention).
    #[inline(always)]
    fn next_ptr(&self, py: Python<'_>) -> *mut ffi::PyObject {
        let (key, value) = {
            let inner = self.dict.get().inner.read(py);
            let pos = match self.state.step(&inner.table, self.reverse) {
                Step::Item(pos) => pos,
                Step::Done => return std::ptr::null_mut(),
                Step::Changed(message) => {
                    drop(inner);
                    // SAFETY: attached; the message is a static NUL-terminated string.
                    unsafe { ffi::PyErr_SetString(ffi::PyExc_RuntimeError, message.as_ptr()) };
                    return std::ptr::null_mut();
                }
            };
            let key = match self.kind {
                ItemKind::Values => std::ptr::null_mut(),
                _ => inner.table.key_object(py, pos).into_ptr(),
            };
            let value = match self.kind {
                ItemKind::Keys => std::ptr::null_mut(),
                _ => inner.value_ptr(py, pos).unwrap_or_else(|| py.None().into_ptr()),
            };
            (key, value)
        };
        match self.kind {
            ItemKind::Keys => key,
            ItemKind::Values => value,
            // SAFETY: attached; `key` and `value` are new references handed over.
            ItemKind::Items => unsafe { self.item(key, value) },
        }
    }

    /// The `(key, value)` tuple for items iteration, stealing both references. Like
    /// CPython's `dictiter_iternextitem`, the previous result tuple is reused when nobody
    /// else references it any more.
    ///
    /// # Safety
    /// Attached to the interpreter; `key` and `value` are owned references.
    #[inline(always)]
    unsafe fn item(&self, key: *mut ffi::PyObject, value: *mut ffi::PyObject) -> *mut ffi::PyObject {
        // SAFETY: guaranteed by the caller; the cached tuple is a live 2-tuple we own.
        unsafe {
            if let Some(result) = &self.result {
                let t = result.as_ptr();
                if uniquely_referenced(t) {
                    ffi::Py_INCREF(t);
                    let (old_key, old_value) = (ffi::PyTuple_GET_ITEM(t, 0), ffi::PyTuple_GET_ITEM(t, 1));
                    ffi::PyTuple_SET_ITEM(t, 0, key);
                    ffi::PyTuple_SET_ITEM(t, 1, value);
                    ffi::Py_DECREF(old_key);
                    ffi::Py_DECREF(old_value);
                    // The GC may have untracked the tuple while it held only untracked items.
                    if ffi::PyObject_GC_IsTracked(t) == 0 {
                        ffi::PyObject_GC_Track(t.cast());
                    }
                    return t;
                }
            }
            let t = ffi::PyTuple_New(2);
            if t.is_null() {
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(value);
            } else {
                ffi::PyTuple_SET_ITEM(t, 0, key);
                ffi::PyTuple_SET_ITEM(t, 1, value);
            }
            t
        }
    }
}

/// Whether we hold the only reference to `obj` (and, on free-threaded builds, this thread
/// owns it), so it can be reused in place.
///
/// # Safety
/// `obj` must be a live object.
#[inline(always)]
unsafe fn uniquely_referenced(obj: *mut ffi::PyObject) -> bool {
    // SAFETY: `obj` is live. (CPython 3.14+: a reference count of one on GIL builds; on
    // free-threaded builds it also requires that this thread owns the object.)
    unsafe { ffi::PyUnstable_Object_IsUniquelyReferenced(obj) != 0 }
}

/// `a == b` for two values when it is decided without running Python code: identical
/// objects, or builtin scalars (`int`, `float`, `str`, `bool`, `complex`, `None`), whose
/// comparisons are pure C. `None`: compare outside the locks.
///
/// # Safety
/// Both objects must be live.
#[inline]
unsafe fn settle(a: *mut ffi::PyObject, b: *mut ffi::PyObject) -> Option<bool> {
    if a == b {
        return Some(true);
    }
    // SAFETY: both objects are live; comparing builtin scalars runs no Python code.
    unsafe {
        let scalar = |o: *mut ffi::PyObject| {
            let t = ffi::Py_TYPE(o);
            t == &raw mut ffi::PyLong_Type
                || t == &raw mut ffi::PyUnicode_Type
                || t == &raw mut ffi::PyFloat_Type
                || t == &raw mut ffi::PyBool_Type
                || t == &raw mut ffi::PyComplex_Type
                || o == ffi::Py_None()
        };
        if !(scalar(a) && scalar(b)) {
            return None;
        }
        match ffi::PyObject_RichCompareBool(a, b, ffi::Py_EQ) {
            1 => Some(true),
            0 => Some(false),
            _ => {
                ffi::PyErr_Clear();
                None
            }
        }
    }
}

/// A key read for merging into another dict.
enum MergeKey {
    Int(i64),
    Obj(Py<PyAny>),
}

/// Values paired by key, and whether either side stores values natively.
type Paired = (Vec<(Py<PyAny>, Py<PyAny>)>, bool);

/// The optional single positional argument of `Dict(...)` / `Dict.__init__`.
fn single_arg<'py>(args: &Bound<'py, PyTuple>) -> PyResult<Option<Bound<'py, PyAny>>> {
    match args.len() {
        0 => Ok(None),
        1 => Ok(Some(args.get_item(0)?)),
        n => Err(PyTypeError::new_err(format!("Dict expected at most 1 argument, got {n}"))),
    }
}

fn classify_key<'py>(key: &Bound<'py, PyAny>) -> PyResult<Key<'py>> {
    Key::classify(key, Role::DictKey)
}

fn key_error(key: &Bound<'_, PyAny>) -> PyErr {
    PyKeyError::new_err((key.clone().unbind(),))
}

#[pymethods]
impl Dict {
    /// `Dict(mapping_or_iterable=(), /, **kwargs)`. For subclasses, like `dict.__new__`,
    /// this creates an empty dict and `__init__` fills it.
    #[new]
    #[classmethod]
    #[pyo3(signature = (*args, **kwargs))]
    fn __new__(
        cls: &Bound<'_, PyType>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let py = cls.py();
        if !cls.is(py.get_type::<Dict>()) {
            return Ok(Dict::empty(schema::any(), schema::any()));
        }
        let arg = single_arg(args)?;
        let dict = Dict::build(py, schema::any(), schema::any(), arg.as_ref(), kwargs)?;
        dict.init_args.store(args.as_ptr() as usize, Ordering::Relaxed);
        Ok(dict)
    }

    /// `dict.__init__`: update with the arguments (skipped right after `Dict(...)`, whose
    /// `__new__` applied them already).
    #[pyo3(signature = (*args, **kwargs))]
    fn __init__(&self, py: Python<'_>, args: &Bound<'_, PyTuple>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<()> {
        if self.init_args.swap(0, Ordering::Relaxed) == args.as_ptr() as usize {
            return Ok(());
        }
        let arg = single_arg(args)?;
        self.update(py, arg.as_ref(), kwargs)
    }

    #[classmethod]
    fn __class_getitem__<'py>(cls: &Bound<'py, PyType>, params: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        crate::typing::dict_alias(cls.py(), params)
    }

    #[classattr]
    const __hash__: Option<Py<PyAny>> = None;

    /// Create a new (untyped) `Dict` with keys from `iterable` and values set to `value`.
    #[classmethod]
    #[pyo3(signature = (iterable, value=None))]
    fn fromkeys<'py>(
        cls: &Bound<'py, PyType>,
        iterable: &Bound<'py, PyAny>,
        value: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let py = cls.py();
        let value = value.cloned().unwrap_or_else(|| py.None().into_bound(py));
        if !cls.is(py.get_type::<Dict>()) {
            // Like `dict.fromkeys`: `cls()`, then `d[key] = value` (subclasses may override
            // `__new__`, `__init__` or `__setitem__`).
            let d = cls.call0()?;
            for key in iterable.try_iter()? {
                d.set_item(key?, &value)?;
            }
            return Ok(d);
        }
        let mut inner = DictInner::new(&Schema::Any, &Schema::Any);
        let (keys, err) = collect_iterable(iterable);
        for key in &keys {
            inner.set_private(py, key, &value)?;
        }
        if let Some(err) = err {
            return Err(err);
        }
        Ok(Bound::new(py, Dict::assemble(schema::any(), schema::any(), inner))?.into_any())
    }

    fn __len__(&self, py: Python<'_>) -> usize {
        self.len(py)
    }

    fn __contains__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<bool> {
        keys::lookup(py, &self.inner, key, classify_key, |_, pos| pos.is_some())
    }

    fn __getitem__(slf: &Bound<'_, Self>, key: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        if let Some(value) = slf.get().get_value(py, key)? {
            return Ok(value);
        }
        // Subclasses may define `__missing__` (looked up on the type, like `dict`).
        if !slf.is_exact_instance_of::<Dict>()
            && let Ok(missing) = slf.get_type().getattr(pyo3::intern!(py, "__missing__"))
        {
            return Ok(missing.call1((slf, key))?.unbind());
        }
        Err(key_error(key))
    }

    fn __setitem__(&self, py: Python<'_>, key: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.set_item(py, key, value)
    }

    fn __delitem__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<()> {
        self.remove_key(py, key)?.map(drop).ok_or_else(|| key_error(key))
    }

    fn __iter__(slf: &Bound<'_, Self>) -> DictIterator {
        Dict::iter_of(slf, ItemKind::Keys, false)
    }

    fn __reversed__(slf: &Bound<'_, Self>) -> DictIterator {
        Dict::iter_of(slf, ItemKind::Keys, true)
    }

    fn __richcmp__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let eq = match op {
            CompareOp::Eq | CompareOp::Ne => Dict::equals(slf, other)?,
            _ => None,
        };
        Ok(match eq {
            Some(eq) => (eq == matches!(op, CompareOp::Eq)).into_pyobject(py)?.to_owned().into_any().unbind(),
            None => py.NotImplemented(),
        })
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let this = slf.get();
        let name =
            if slf.is_exact_instance_of::<Dict>() { this.type_name() } else { slf.get_type().name()?.to_string() };
        let Some(_guard) = ReprGuard::enter(slf.as_any())? else {
            return Ok(format!("{name}({{...}})"));
        };
        let mut out = format!("{name}({{");
        for (i, (k, v)) in this.items_snapshot(py).into_iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&k.bind(py).repr()?.to_cow()?);
            out.push_str(": ");
            out.push_str(&v.bind(py).repr()?.to_cow()?);
        }
        out.push_str("})");
        Ok(out)
    }

    fn __or__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let same_schema = match other.cast::<Dict>() {
            Ok(o) => o.get().same_schema(&self.key_schema, &self.value_schema),
            Err(_) if is_builtin_mapping(other) => false,
            Err(_) => return Ok(py.NotImplemented()),
        };
        let mut result = self.derived(py, same_schema || !self.is_typed())?;
        if result.is_typed() {
            result.update_from(py, other)?;
        } else {
            // The result is private until returned.
            let inner = result.inner.get_mut();
            inner.update_private(py, other)?;
            result.len.store(inner.table.len(), Ordering::Relaxed);
        }
        Ok(Bound::new(py, result)?.into_any().unbind())
    }

    fn __ror__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if !is_builtin_mapping(other) {
            return Ok(py.NotImplemented());
        }
        let mut inner = DictInner::new(&Schema::Any, &Schema::Any);
        inner.update_private(py, other)?;
        for (k, v) in self.items_snapshot(py) {
            inner.set_private(py, k.bind(py), v.bind(py))?;
        }
        let result = Dict::assemble(schema::any(), schema::any(), inner);
        Ok(Bound::new(py, result)?.into_any().unbind())
    }

    fn __ior__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<()> {
        self.update_from(py, other)
    }

    fn keys(slf: &Bound<'_, Self>) -> DictKeys {
        DictKeys { dict: slf.clone().unbind() }
    }

    fn values(slf: &Bound<'_, Self>) -> DictValues {
        DictValues { dict: slf.clone().unbind() }
    }

    fn items(slf: &Bound<'_, Self>) -> DictItems {
        DictItems { dict: slf.clone().unbind() }
    }

    #[pyo3(signature = (key, default=None))]
    fn get(&self, py: Python<'_>, key: &Bound<'_, PyAny>, default: Option<Py<PyAny>>) -> PyResult<Py<PyAny>> {
        Ok(self.get_value(py, key)?.or(default).unwrap_or_else(|| py.None()))
    }

    /// `pop(key[, default])`.
    #[pyo3(signature = (key, *default))]
    fn pop(&self, py: Python<'_>, key: &Bound<'_, PyAny>, default: &Bound<'_, PyTuple>) -> PyResult<Py<PyAny>> {
        if default.len() > 1 {
            return Err(PyTypeError::new_err(format!("pop expected at most 2 arguments, got {}", default.len() + 1)));
        }
        match self.remove_key(py, key)? {
            Some(value) => Ok(value),
            None if default.len() == 1 => Ok(default.get_item(0)?.unbind()),
            None => Err(key_error(key)),
        }
    }

    /// Remove and return the most recently inserted `(key, value)` pair.
    fn popitem<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let mut displaced = Vec::new();
        let item = {
            let mut inner = self.inner.write(py);
            let item = inner.pop_last(py, &mut displaced);
            self.publish_len(&inner);
            item
        };
        drop(displaced);
        match item {
            Some((k, v)) => PyTuple::new(py, [k, v]),
            None => Err(PyKeyError::new_err("popitem(): dictionary is empty")),
        }
    }

    #[pyo3(signature = (key, default=None))]
    fn setdefault(
        &self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        default: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let prepared = self.prepare_key(key.clone())?;
        // Present: no value conversion (typed dicts only validate what they store).
        if let Some(v) = keys::lookup(py, &self.inner, key, classify_key, |inner, pos| pos.map(|p| inner.value(py, p)))?
        {
            return Ok(v);
        }
        let none = py.None().into_bound(py);
        let value = self.prepare_value(default.unwrap_or(&none))?;
        let mut result = None;
        // The converted default may be a new container holding the last references to its
        // elements: when the key turned up meanwhile it is released after the lock.
        let mut unused = Vec::new();
        let res = keys::batch_upsert(py, &self.inner, [(prepared, value)], |inner, value, slot| {
            let pos = match slot {
                Slot::Existing(pos) => {
                    unused.extend(coerced_object(value));
                    pos
                }
                Slot::Inserted(pos) => {
                    inner.values.push(py, value);
                    pos
                }
                Slot::Absent(key) => {
                    inner.push_entry(py, key, value, &mut unused)?;
                    inner.table.entries() - 1
                }
            };
            result = Some(inner.value(py, pos));
            self.publish_len(inner);
            Ok(())
        });
        drop(unused);
        res?;
        Ok(result.unwrap_or_else(|| py.None()))
    }

    #[pyo3(signature = (other=None, /, **kwargs))]
    fn update(
        &self,
        py: Python<'_>,
        other: Option<&Bound<'_, PyAny>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        if let Some(other) = other {
            self.update_from(py, other)?;
        }
        if let Some(kwargs) = kwargs {
            self.update_from(py, kwargs.as_any())?;
        }
        Ok(())
    }

    fn clear(&self, py: Python<'_>) {
        let old = {
            let mut inner = self.inner.write(py);
            let old = inner.clear(&self.value_schema);
            self.publish_len(&inner);
            old
        };
        drop(old);
    }

    fn copy(&self, py: Python<'_>) -> PyResult<Dict> {
        self.derived(py, true)
    }

    fn __copy__(slf: &Bound<'_, Self>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        if slf.is_exact_instance_of::<Dict>() {
            return Ok(Bound::new(py, slf.get().derived(py, true)?)?.into_any().unbind());
        }
        // A subclass instance: like `copy.copy` of a `dict` subclass (`__reduce_ex__`).
        let cls = slf.get_type();
        let new = cls.call_method1(pyo3::intern!(py, "__new__"), (&cls,))?;
        if let Ok(state) = slf.getattr(pyo3::intern!(py, "__dict__")) {
            new.getattr(pyo3::intern!(py, "__dict__"))?.call_method1(pyo3::intern!(py, "update"), (state,))?;
        }
        for (k, v) in slf.get().items_snapshot(py) {
            new.set_item(k, v)?;
        }
        Ok(new.unbind())
    }

    /// A builtin `dict` with the same items.
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        for (k, v) in self.items_snapshot(py) {
            out.set_item(k, v)?;
        }
        Ok(out)
    }

    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyTuple>> {
        let py = slf.py();
        if slf.is_exact_instance_of::<Dict>() {
            let this = slf.get();
            return PyTuple::new(py, [this.schema_type_of(py)?, PyTuple::new(py, [this.to_dict(py)?])?.into_any()]);
        }
        // Subclasses pickle like `dict` subclasses: `cls.__new__(cls)`, the instance
        // `__dict__`, then the items through `obj[key] = value`.
        let newobj = py.import("copyreg")?.getattr("__newobj__")?;
        let state = match slf.getattr(pyo3::intern!(py, "__dict__")) {
            Ok(d) if d.len().unwrap_or(0) > 0 => d,
            _ => py.None().into_bound(py),
        };
        let items = Dict::items(slf).into_pyobject(py)?.try_iter()?;
        PyTuple::new(
            py,
            [newobj, PyTuple::new(py, [slf.get_type()])?.into_any(), state, py.None().into_bound(py), items.into_any()],
        )
    }

    /// The alias describing this dict's type (`Dict[str, int]`, or `Dict` when untyped).
    #[getter]
    fn schema_type<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.schema_type_of(py)
    }

    /// Key strategy (`"empty"`, `"i64"`, `"str"` or `"object"`), for introspection.
    #[getter]
    fn _key_strategy(&self, py: Python<'_>) -> &'static str {
        self.inner.read(py).table.strategy()
    }

    /// Value storage strategy, for introspection.
    #[getter]
    fn _value_strategy(&self, py: Python<'_>) -> &'static str {
        self.inner.read(py).values.strategy()
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        if let Some(inner) = self.inner.try_read() {
            for k in inner.table.object_keys() {
                visit.call(k)?;
            }
            if let Storage::Object(values) = &inner.values {
                for v in values {
                    visit.call(v)?;
                }
            }
        }
        Ok(())
    }

    fn __clear__(&self) {
        // Runs inside the GC: never block (a contended dict is simply not cleared now).
        let old = self.inner.try_write().map(|mut inner| {
            let old = inner.clear(&self.value_schema);
            self.publish_len(&inner);
            old
        });
        drop(old);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemKind {
    Keys,
    Values,
    Items,
}

/// CPython's `dictiterobject` state. Positions are entry positions, which move only when
/// the table grows (as in CPython), so iteration over a dict mutated at constant size
/// behaves like `dict`'s. The fields are relaxed atomics read and written under the
/// dict's read lock, like CPython's free-threaded build: threads sharing one iterator may
/// see an item twice or not at all, but never anything inconsistent.
struct IterState {
    /// Next position to examine (forward), or one past it (reverse).
    pos: AtomicUsize,
    /// The size at creation; `usize::MAX` once a size change was reported (sticky).
    expected: AtomicUsize,
    /// Keys left to yield; finding more (forward) means the keys changed.
    remaining: AtomicUsize,
    exhausted: AtomicBool,
}

/// Outcome of one iteration step.
enum Step {
    Item(usize),
    Done,
    Changed(&'static std::ffi::CStr),
}

impl IterState {
    /// One step of `dictiter_iternextkey` over `table`.
    #[inline(always)]
    fn step(&self, table: &KeyTable, reverse: bool) -> Step {
        const R: Ordering = Ordering::Relaxed;
        if self.exhausted.load(R) {
            return Step::Done;
        }
        if table.len() != self.expected.load(R) {
            self.expected.store(usize::MAX, R);
            return Step::Changed(c"dictionary changed size during iteration");
        }
        let at = self.pos.load(R);
        let next = if reverse { table.prev_live(at) } else { table.next_live(at) };
        let Some(pos) = next else {
            self.exhausted.store(true, R);
            return Step::Done;
        };
        let remaining = self.remaining.load(R);
        // CPython checks this for forward iteration only.
        if remaining == 0 && !reverse {
            self.exhausted.store(true, R);
            return Step::Changed(c"dictionary keys changed during iteration");
        }
        self.remaining.store(remaining.saturating_sub(1), R);
        self.pos.store(if reverse { pos } else { pos + 1 }, R);
        Step::Item(pos)
    }
}

/// Live iterator over a `Dict`'s keys, values or items.
#[pyclass(module = "rustique", frozen)]
pub struct DictIterator {
    dict: Py<Dict>,
    kind: ItemKind,
    reverse: bool,
    state: IterState,
    /// Items iteration: the result tuple, recycled while nobody else references it.
    result: Option<Py<PyAny>>,
}

#[pymethods]
impl DictIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let item = self.next_ptr(py);
        if item.is_null() {
            return match PyErr::take(py) {
                Some(err) => Err(err),
                None => Ok(None),
            };
        }
        // SAFETY: `next_ptr` returned a new reference.
        Ok(Some(unsafe { Bound::from_owned_ptr(py, item) }.unbind()))
    }

    fn __length_hint__(&self, py: Python<'_>) -> usize {
        let st = &self.state;
        let expected = st.expected.load(Ordering::Relaxed);
        let unchanged = !st.exhausted.load(Ordering::Relaxed) && self.dict.get().len(py) == expected;
        if unchanged { st.remaining.load(Ordering::Relaxed) } else { 0 }
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.dict)?;
        if let Some(result) = &self.result {
            visit.call(result)?;
        }
        Ok(())
    }
}

/// Builtin `set` of a view's elements.
fn view_set<'py>(view: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PySet>> {
    PySet::new(view.py(), view.try_iter()?.collect::<PyResult<Vec<_>>>()?)
}

/// Whether `obj` behaves like a set for view comparisons.
fn is_setlike(obj: &Bound<'_, PyAny>) -> bool {
    obj.is_instance_of::<PySet>()
        || obj.is_instance_of::<PyFrozenSet>()
        || obj.is_instance_of::<DictKeys>()
        || obj.is_instance_of::<DictItems>()
        || obj.is_instance_of::<crate::set::Set>()
        || is_builtin_view(obj)
}

fn is_builtin_view(obj: &Bound<'_, PyAny>) -> bool {
    let name = obj.get_type().name().map(|n| n.to_string()).unwrap_or_default();
    (name == "dict_keys" || name == "dict_items")
        && obj.get_type().module().is_ok_and(|m| m.to_cow().is_ok_and(|m| m == "builtins"))
}

/// Shared set-like behaviour of key and item views.
macro_rules! setlike_view {
    ($view:ty { $($methods:tt)* }) => {
        #[pymethods]
        impl $view {
            $($methods)*

            #[classattr]
            const __hash__: Option<Py<PyAny>> = None;

            fn __richcmp__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<Py<PyAny>> {
                let py = slf.py();
                if !is_setlike(other) {
                    return Ok(py.NotImplemented());
                }
                let mine = view_set(slf.as_any())?;
                let theirs = if other.is_instance_of::<PySet>() || other.is_instance_of::<PyFrozenSet>() {
                    other.clone()
                } else {
                    view_set(other)?.into_any()
                };
                Ok(mine.rich_compare(theirs, op)?.unbind())
            }

            /// `_PyDictView_Intersect`: iterate the smaller operand, keep what the other
            /// contains (membership in a `Dict` view is a direct lookup).
            fn __and__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                let py = slf.py();
                let mine = slf.get().view_len(py);
                if other.is_exact_instance_of::<PySet>() && mine <= other.len()? {
                    return other.call_method1(pyo3::intern!(py, "intersection"), (slf,));
                }
                let result = PySet::empty(py)?;
                if is_setlike(other) && !other.is_instance_of::<PySet>() && other.len()? > mine {
                    for item in slf.try_iter()? {
                        let item = item?;
                        if other.contains(&item)? {
                            result.add(item)?;
                        }
                    }
                } else {
                    for item in other.try_iter()? {
                        let item = item?;
                        if slf.get().view_contains(py, &item)? {
                            result.add(item)?;
                        }
                    }
                }
                Ok(result.into_any())
            }

            fn __rand__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                view_set(other)?.call_method1("intersection", (slf,))
            }

            fn __or__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                let s = view_set(slf.as_any())?;
                s.call_method1("update", (other,))?;
                Ok(s.into_any())
            }

            fn __ror__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                let s = view_set(other)?;
                s.call_method1("update", (slf,))?;
                Ok(s.into_any())
            }

            fn __sub__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                let s = view_set(slf.as_any())?;
                s.call_method1("difference_update", (other,))?;
                Ok(s.into_any())
            }

            fn __rsub__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                let s = view_set(other)?;
                s.call_method1("difference_update", (slf,))?;
                Ok(s.into_any())
            }

            fn __xor__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                let s = view_set(slf.as_any())?;
                s.call_method1("symmetric_difference_update", (other,))?;
                Ok(s.into_any())
            }

            fn __rxor__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                let s = view_set(other)?;
                s.call_method1("symmetric_difference_update", (slf,))?;
                Ok(s.into_any())
            }

            fn isdisjoint(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
                view_set(slf.as_any())?.call_method1("isdisjoint", (other,))?.extract()
            }
        }
    };
}

/// Membership and size shared by the set-like views.
trait SetLikeView {
    fn view_len(&self, py: Python<'_>) -> usize;
    fn view_contains(&self, py: Python<'_>, item: &Bound<'_, PyAny>) -> PyResult<bool>;
}

impl SetLikeView for DictKeys {
    fn view_len(&self, py: Python<'_>) -> usize {
        self.dict.get().len(py)
    }

    fn view_contains(&self, py: Python<'_>, item: &Bound<'_, PyAny>) -> PyResult<bool> {
        keys::lookup(py, &self.dict.get().inner, item, classify_key, |_, pos| pos.is_some())
    }
}

impl SetLikeView for DictItems {
    fn view_len(&self, py: Python<'_>) -> usize {
        self.dict.get().len(py)
    }

    fn view_contains(&self, py: Python<'_>, item: &Bound<'_, PyAny>) -> PyResult<bool> {
        self.__contains__(py, item)
    }
}

/// `Dict.keys()`: a live, set-like view of the keys.
#[pyclass(module = "rustique", frozen)]
pub struct DictKeys {
    dict: Py<Dict>,
}

setlike_view!(DictKeys {
    fn __len__(&self, py: Python<'_>) -> usize {
        self.dict.get().len(py)
    }

    fn __iter__(&self, py: Python<'_>) -> DictIterator {
        Dict::iter_of(self.dict.bind(py), ItemKind::Keys, false)
    }

    fn __reversed__(&self, py: Python<'_>) -> DictIterator {
        Dict::iter_of(self.dict.bind(py), ItemKind::Keys, true)
    }

    fn __contains__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<bool> {
        self.dict.get().__contains__(py, key)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let keys = PyList::new(py, self.dict.get().key_snapshot(py))?;
        Ok(format!("DictKeys({})", keys.repr()?))
    }

    #[getter]
    fn mapping(&self, py: Python<'_>) -> Py<Dict> {
        self.dict.clone_ref(py)
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.dict)
    }
});

/// `Dict.values()`: a live view of the values.
#[pyclass(module = "rustique", frozen)]
pub struct DictValues {
    dict: Py<Dict>,
}

#[pymethods]
impl DictValues {
    fn __len__(&self, py: Python<'_>) -> usize {
        self.dict.get().len(py)
    }

    fn __iter__(&self, py: Python<'_>) -> DictIterator {
        Dict::iter_of(self.dict.bind(py), ItemKind::Values, false)
    }

    fn __reversed__(&self, py: Python<'_>) -> DictIterator {
        Dict::iter_of(self.dict.bind(py), ItemKind::Values, true)
    }

    fn __contains__(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<bool> {
        let (values, native) = {
            let inner = self.dict.get().inner.read(py);
            (inner.values_list(py), inner.values_native())
        };
        for v in values {
            if values_equal(v.bind(py), value, native)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let values = PyList::new(py, self.dict.get().inner.read(py).values_list(py))?;
        Ok(format!("DictValues({})", values.repr()?))
    }

    #[getter]
    fn mapping(&self, py: Python<'_>) -> Py<Dict> {
        self.dict.clone_ref(py)
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.dict)
    }
}

/// `Dict.items()`: a live, set-like view of `(key, value)` pairs.
#[pyclass(module = "rustique", frozen)]
pub struct DictItems {
    dict: Py<Dict>,
}

setlike_view!(DictItems {
    fn __len__(&self, py: Python<'_>) -> usize {
        self.dict.get().len(py)
    }

    fn __iter__(&self, py: Python<'_>) -> DictIterator {
        Dict::iter_of(self.dict.bind(py), ItemKind::Items, false)
    }

    fn __reversed__(&self, py: Python<'_>) -> DictIterator {
        Dict::iter_of(self.dict.bind(py), ItemKind::Items, true)
    }

    fn __contains__(&self, py: Python<'_>, item: &Bound<'_, PyAny>) -> PyResult<bool> {
        let Ok(pair) = item.cast::<PyTuple>() else { return Ok(false) };
        if pair.len() != 2 {
            return Ok(false);
        }
        let dict = self.dict.get();
        let native = dict.inner.read(py).values_native();
        match dict.get_value(py, &pair.get_item(0)?)? {
            Some(v) => values_equal(v.bind(py), &pair.get_item(1)?, native),
            None => Ok(false),
        }
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let items = self.dict.get().items_snapshot(py);
        let tuples = items.into_iter().map(|(k, v)| PyTuple::new(py, [k, v])).collect::<PyResult<Vec<_>>>()?;
        Ok(format!("DictItems({})", PyList::new(py, tuples)?.repr()?))
    }

    #[getter]
    fn mapping(&self, py: Python<'_>) -> Py<Dict> {
        self.dict.clone_ref(py)
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.dict)
    }
});

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Dict>()?;
    m.add_class::<DictKeys>()?;
    m.add_class::<DictValues>()?;
    m.add_class::<DictItems>()?;
    m.add_class::<DictIterator>()?;
    fast::install(m.py())
}
