//! The `Dict` class (ARCHITECTURE §4.2): an insertion-ordered mapping with native key
//! strategies (`i64`, `str`, or Python objects) and values in a shared [`Storage`].
//!
//! OWNER: dict/set agent. Items marked `INTERFACE` are used by other modules.

pub(crate) mod keys;

use std::cell::RefCell;
use std::sync::Arc;

use pyo3::Borrowed;
use pyo3::exceptions::{PyKeyError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::pyclass::CompareOp;
use pyo3::sync::PyOnceLock;
use pyo3::sync::critical_section::with_critical_section;
use pyo3::types::{
    PyBool, PyDict, PyFloat, PyFrozenSet, PyInt, PyList, PySet, PyString, PyTuple, PyType,
};
use pyo3::{PyTraverseError, ffi};
use rustique_core::Column;
use rustique_core::index::Remap;

use crate::convert;
use crate::schema::{self, Coerced, Schema};
use crate::storage::{Displaced, Storage};

use keys::{
    Key, KeyKind, KeyStore, KeyTable, Keyed, Locked, NativeKeys, Prepared, Quick, Role, Slot,
};

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
}

impl DictInner {
    fn new(key_schema: &Schema, value_schema: &Schema) -> DictInner {
        DictInner {
            table: KeyTable::new(KeyKind::for_schema(key_schema)),
            values: Storage::for_schema(value_schema),
        }
    }

    fn value(&self, py: Python<'_>, pos: usize) -> Py<PyAny> {
        self.values.get(py, pos).unwrap_or_else(|| py.None())
    }

    /// Append an entry for an absent key. If the key cannot be stored, the value goes to
    /// `out` (it may hold the last reference to an object: never drop it under the lock).
    fn push_entry(
        &mut self,
        py: Python<'_>,
        key: &Key<'_>,
        value: Coerced,
        out: &mut Displaced,
    ) -> PyResult<()> {
        if let Err(e) = self.table.push(py, key) {
            out.extend(coerced_object(value));
            return Err(e);
        }
        self.values.push(py, value);
        Ok(())
    }

    /// Remove the live entry at `pos`, returning its value; displaced objects go to `out`.
    fn remove_entry(&mut self, py: Python<'_>, pos: usize, out: &mut Displaced) -> Py<PyAny> {
        let value = take_value(py, &mut self.values, pos);
        out.extend(self.table.remove_at(py, pos));
        if let Some(remap) = self.table.maybe_compact() {
            out.extend(remap_storage(&mut self.values, &remap));
        }
        value
    }

    /// Remove the most recently inserted entry.
    fn pop_last(&mut self, py: Python<'_>, out: &mut Displaced) -> Option<(Py<PyAny>, Py<PyAny>)> {
        let pos = self.table.last_live()?;
        let key = self.table.key_object(py, pos);
        let value = take_value(py, &mut self.values, pos);
        out.extend(self.table.truncate(pos));
        out.extend(truncate_storage(&mut self.values, pos));
        Some((key, value))
    }

    fn clear(&mut self, value_schema: &Schema, out: &mut Displaced) {
        out.extend(self.table.clear());
        let old = std::mem::replace(&mut self.values, Storage::for_schema(value_schema));
        if let Storage::Object(objs) = old {
            out.extend(objs);
        }
    }

    fn clone_ref(&self, py: Python<'_>) -> DictInner {
        DictInner {
            table: self.table.clone_ref(py),
            values: self.values.clone_ref(py),
        }
    }

    fn items(&self, py: Python<'_>) -> Vec<(Py<PyAny>, Py<PyAny>)> {
        self.table
            .positions()
            .into_iter()
            .map(|p| (self.table.key_object(py, p), self.value(py, p)))
            .collect()
    }

    fn values_list(&self, py: Python<'_>) -> Vec<Py<PyAny>> {
        self.table
            .positions()
            .into_iter()
            .map(|p| self.value(py, p))
            .collect()
    }
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
        Storage::Object(objs) => objs
            .get_mut(pos)
            .map_or_else(|| py.None(), |slot| std::mem::replace(slot, py.None())),
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
    obj.is_instance_of::<PyDict>()
        || frozendict_type(obj.py()).is_some_and(|t| obj.is_instance(t).unwrap_or(false))
}

/// Strong references to everything an iterable yields, plus the error that stopped it.
/// Builtin lists, sets and dicts are read inside one critical section on the source.
pub(crate) fn collect_iterable<'py>(
    obj: &Bound<'py, PyAny>,
) -> (Vec<Bound<'py, PyAny>>, Option<PyErr>) {
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
        return (
            with_critical_section(set.as_any(), || set.iter().collect()),
            None,
        );
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

/// A value read inside a source's critical section, possibly through a borrowed reference.
/// Only exact `int`/`float`/`bool`/`str` values are converted in place, and only when the
/// schema is pure: converting anything else — even unsuccessfully — may run Python code (a
/// subclass's `__str__`/`__repr__` in an error message, …) that could free the borrowed
/// item, so it gets a strong reference and is converted after the critical section.
#[inline]
fn raw_value<'py>(obj: &Bound<'py, PyAny>, schema: &Schema, pure: bool) -> RawValue<'py> {
    if schema.is_any() {
        // Exact-type classification only reads the object (anything else is kept as is).
        return RawValue::Ready(convert::classify(obj));
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
pub(crate) fn read_elements<'py>(
    obj: &Bound<'py, PyAny>,
    schema: &Schema,
) -> (Vec<Raw<'py>>, Option<PyErr>) {
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
        return (
            tuple
                .iter_borrowed()
                .map(|item| raw_key(&item, schema))
                .collect(),
            None,
        );
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
    (
        items
            .into_iter()
            .map(|item| raw_key(&item, schema))
            .collect(),
        err,
    )
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
        RawPair::Pair(raw_key(k, key_schema), raw_value(v, value_schema, pure))
    };
    // Unpack exact 2-tuples in place (their items are kept alive by the tuple).
    let element = |item: &Bound<'py, PyAny>, index: usize| match item.cast_exact::<PyTuple>() {
        Ok(t) if t.len() == 2 => {
            // SAFETY: indices 0 and 1 are in bounds of this tuple.
            let (k, v) = unsafe {
                (
                    t.get_borrowed_item_unchecked(0),
                    t.get_borrowed_item_unchecked(1),
                )
            };
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
        return (
            tuple
                .iter_borrowed()
                .enumerate()
                .map(|(i, item)| element(&item, i))
                .collect(),
            None,
        );
    }
    let later = |(k, v): (Bound<'py, PyAny>, Bound<'py, PyAny>)| {
        RawPair::Pair(Raw::Later(k), RawValue::Later(v))
    };
    if let Ok(d) = arg.cast::<Dict>() {
        let items = d.get().inner.read(py).items(py);
        return (
            items
                .into_iter()
                .map(|(k, v)| later((k.into_bound(py), v.into_bound(py))))
                .collect(),
            None,
        );
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
    (
        items
            .into_iter()
            .enumerate()
            .map(|(i, item)| RawPair::Item(item, i))
            .collect(),
        err,
    )
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

fn pair_of<'py>(
    item: Bound<'py, PyAny>,
    index: usize,
) -> PyResult<(Bound<'py, PyAny>, Bound<'py, PyAny>)> {
    if let Ok(t) = item.cast_exact::<PyTuple>()
        && t.len() == 2
    {
        return Ok((t.get_item(0)?, t.get_item(1)?));
    }
    let py = item.py();
    let iter = item.try_iter().map_err(|e| {
        if e.is_instance_of::<PyTypeError>(py) {
            PyTypeError::new_err(format!(
                "cannot convert dictionary update sequence element #{index} to a sequence"
            ))
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

/// An insertion-ordered mapping with native key and value storage.
#[pyclass(module = "rustique", frozen, mapping)]
pub struct Dict {
    key_schema: Arc<Schema>,
    value_schema: Arc<Schema>,
    inner: Locked<DictInner>,
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

    /// INTERFACE.
    pub fn key_schema(&self) -> &Arc<Schema> {
        &self.key_schema
    }

    /// INTERFACE.
    pub fn value_schema(&self) -> &Arc<Schema> {
        &self.value_schema
    }

    fn empty(key_schema: Arc<Schema>, value_schema: Arc<Schema>) -> Dict {
        let inner = Locked::new(DictInner::new(&key_schema, &value_schema));
        Dict {
            key_schema,
            value_schema,
            inner,
        }
    }

    fn build<'py>(
        py: Python<'py>,
        key_schema: Arc<Schema>,
        value_schema: Arc<Schema>,
        arg: Option<&Bound<'py, PyAny>>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Dict> {
        if let Some(arg) = arg
            && let Ok(other) = arg.cast::<Dict>()
            && other.get().same_schema(&key_schema, &value_schema)
        {
            let inner = other.get().inner.read(py).clone_ref(py);
            let dict = Dict {
                key_schema,
                value_schema,
                inner: Locked::new(inner),
            };
            if let Some(kwargs) = kwargs {
                dict.update_from(py, kwargs.as_any())?;
            }
            return Ok(dict);
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
            && (Arc::ptr_eq(&self.value_schema, value_schema)
                || *self.value_schema == **value_schema)
    }

    fn is_typed(&self) -> bool {
        !(self.key_schema.is_any() && self.value_schema.is_any())
    }

    /// `Dict` or `Dict[K, V]`, for reprs and error messages.
    fn type_name(&self) -> String {
        if self.is_typed() {
            format!("Dict[{}, {}]", self.key_schema, self.value_schema)
        } else {
            "Dict".to_owned()
        }
    }

    fn prepare_key<'py>(&self, key: Bound<'py, PyAny>) -> PyResult<Prepared<'py>> {
        keys::prepare_owned(key, &self.key_schema, Role::DictKey, || self.type_name())
    }

    fn prepare_value(&self, value: &Bound<'_, PyAny>) -> PyResult<Coerced> {
        self.value_schema
            .coerce(value)
            .map_err(|e| e.into_pyerr(&self.type_name(), None))
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
            Ok(())
        });
        drop(displaced);
        res?;
        err.map_or(Ok(()), Err)
    }

    fn insert_pairs<'py>(
        &self,
        py: Python<'py>,
        pairs: Pairs<'py>,
        err: Option<PyErr>,
    ) -> PyResult<()> {
        let raw = pairs
            .into_iter()
            .map(|(k, v)| RawPair::Pair(Raw::Later(k), RawValue::Later(v)))
            .collect();
        let (items, err) = self.finish_pairs(raw, err);
        self.insert_items(py, items, err)
    }

    fn update_from(&self, py: Python<'_>, arg: &Bound<'_, PyAny>) -> PyResult<()> {
        let (raw, err) = read_pairs(arg, &self.key_schema, &self.value_schema);
        let (items, err) = self.finish_pairs(raw, err);
        self.insert_items(py, items, err)
    }

    fn set_item(
        &self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let key = self.prepare_key(key.clone())?;
        let mut displaced = Vec::new();
        if self.value_schema.is_any() {
            // Untyped values keep their identity in object storage.
            keys::batch_upsert(py, &self.inner, [(key, value)], |inner, value, slot| {
                match slot {
                    Slot::Existing(pos) => displaced.extend(inner.values.set_any(py, pos, value)),
                    Slot::Inserted(_) => inner.values.push_any(py, value),
                    Slot::Absent(key) => {
                        inner.table.push(py, key)?;
                        inner.values.push_any(py, value);
                    }
                }
                Ok(())
            })?;
        } else {
            let value = self.prepare_value(value)?;
            keys::batch_upsert(py, &self.inner, [(key, value)], |inner, value, slot| {
                match slot {
                    Slot::Existing(pos) => displaced.extend(inner.values.set(py, pos, value)),
                    Slot::Inserted(_) => inner.values.push(py, value),
                    Slot::Absent(key) => inner.push_entry(py, key, value, &mut displaced)?,
                }
                Ok(())
            })?;
        }
        drop(displaced);
        Ok(())
    }

    fn get_value(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<Option<Py<PyAny>>> {
        keys::lookup(py, &self.inner, key, classify_key, |inner, pos| {
            pos.map(|p| inner.value(py, p))
        })
    }

    /// Remove `key`, returning its value (`None` if absent).
    fn remove_key(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<Option<Py<PyAny>>> {
        let mut displaced = Vec::new();
        let value = keys::lookup_mut(py, &self.inner, key, classify_key, |inner, pos| {
            pos.map(|p| inner.remove_entry(py, p, &mut displaced))
        })?;
        drop(displaced);
        Ok(value)
    }

    pub(crate) fn len(&self, py: Python<'_>) -> usize {
        self.inner.read(py).table.len()
    }

    fn items_snapshot(&self, py: Python<'_>) -> Vec<(Py<PyAny>, Py<PyAny>)> {
        self.inner.read(py).items(py)
    }

    fn key_snapshot(&self, py: Python<'_>) -> Vec<Py<PyAny>> {
        self.inner.read(py).table.key_objects(py)
    }

    fn native_view(&self, py: Python<'_>) -> Option<(NativeKeys, Column)> {
        let inner = self.inner.read(py);
        match (&inner.values, inner.table.native_clone()) {
            (Storage::Native(col), Some(keys)) => Some((keys, col.clone())),
            _ => None,
        }
    }

    /// Native comparison when both sides store native keys and values. `None`: not applicable.
    fn native_eq(&self, py: Python<'_>, other: &Dict) -> Option<bool> {
        let (keys, col) = other.native_view(py)?;
        let inner = self.inner.read(py);
        let Storage::Native(mine) = &inner.values else {
            return None;
        };
        if inner.table.len() != keys_len(&keys) {
            return Some(false);
        }
        for pos in inner.table.positions() {
            let theirs = match (inner.table.store(), &keys) {
                (KeyStore::Int(a), NativeKeys::Int(b)) => b.get(&a.raw_keys()[pos]),
                (KeyStore::Str(a), NativeKeys::Str(b)) => b.get(a.raw_keys()[pos].as_str()),
                _ => return None,
            };
            match theirs {
                None => return Some(false),
                Some(j) => {
                    if !native_values_equal(mine, pos, &col, j)? {
                        return Some(false);
                    }
                }
            }
        }
        Some(true)
    }

    fn eq_dict(&self, py: Python<'_>, other: &Dict) -> PyResult<bool> {
        if let Some(eq) = self.native_eq(py, other) {
            return Ok(eq);
        }
        let (keys, values) = {
            let inner = self.inner.read(py);
            (inner.table.element_keys(py)?, inner.values_list(py))
        };
        if keys.len() != other.len(py) {
            return Ok(false);
        }
        for (key, v) in keys.iter().zip(values) {
            let theirs = keys::with_entry(py, &other.inner, key, |inner, pos| {
                pos.map(|p| inner.value(py, p))
            })?;
            match theirs {
                Some(theirs) if objects_equal(v.bind(py), theirs.bind(py))? => {}
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
                let found = match keys::native_key(&k, &Schema::Any)? {
                    Prepared::Int(i) => inner.table.find_quick(&Quick::Int(i))?,
                    Prepared::Str(s) => inner.table.find_quick(&Quick::Str(&s))?,
                    Prepared::Key(_) => return None,
                };
                let Some(at) = found else { return Some(false) };
                if !keys::values_equal(&col.get(at)?, &keys::native_value(&v)?) {
                    return Some(false);
                }
            }
            Some(true)
        })
    }

    fn eq_mapping(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(dict) = other.cast_exact::<PyDict>()
            && let Some(eq) = self.native_eq_builtin(py, dict)
        {
            return Ok(eq);
        }
        let items = self.items_snapshot(py);
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
                Some(theirs) if objects_equal(v.bind(py), &theirs)? => {}
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
            return Ok(Dict {
                key_schema: self.key_schema.clone(),
                value_schema: self.value_schema.clone(),
                inner: Locked::new(inner),
            });
        }
        let dict = Dict::empty(schema::any(), schema::any());
        dict.insert_pairs(
            py,
            self.items_snapshot(py)
                .into_iter()
                .map(|(k, v)| (k.into_bound(py), v.into_bound(py)))
                .collect(),
            None,
        )?;
        Ok(dict)
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
        let inner = slf.get().inner.read(py);
        let len = inner.table.len();
        let pos = if reverse { inner.table.entries() } else { 0 };
        let state = IterState {
            pos,
            expected: len,
            remaining: len,
            layout: inner.table.layout(),
            exhausted: false,
        };
        drop(inner);
        DictIterator {
            dict: slf.clone().unbind(),
            kind,
            reverse,
            state: Locked::new(state),
        }
    }
}

fn keys_len(keys: &NativeKeys) -> usize {
    match keys {
        NativeKeys::Int(k) => k.len(),
        NativeKeys::Str(k) => k.len(),
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
    #[new]
    #[pyo3(signature = (mapping_or_iterable=None, /, **kwargs))]
    fn __new__(
        py: Python<'_>,
        mapping_or_iterable: Option<&Bound<'_, PyAny>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        Dict::build(
            py,
            schema::any(),
            schema::any(),
            mapping_or_iterable,
            kwargs,
        )
    }

    #[classmethod]
    fn __class_getitem__<'py>(
        cls: &Bound<'py, PyType>,
        params: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
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
    ) -> PyResult<Bound<'py, Dict>> {
        let py = cls.py();
        let value = value.cloned().unwrap_or_else(|| py.None().into_bound(py));
        let (raw, err) = read_elements(iterable, &Schema::Any);
        let dict = Dict::empty(schema::any(), schema::any());
        let pairs = raw
            .into_iter()
            .map(|k| RawPair::Pair(k, RawValue::Later(value.clone())))
            .collect();
        let (items, err) = dict.finish_pairs(pairs, err);
        dict.insert_items(py, items, err)?;
        Bound::new(py, dict)
    }

    fn __len__(&self, py: Python<'_>) -> usize {
        self.len(py)
    }

    fn __contains__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<bool> {
        keys::lookup(py, &self.inner, key, classify_key, |_, pos| pos.is_some())
    }

    fn __getitem__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.get_value(py, key)?.ok_or_else(|| key_error(key))
    }

    fn __setitem__(
        &self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.set_item(py, key, value)
    }

    fn __delitem__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<()> {
        self.remove_key(py, key)?
            .map(drop)
            .ok_or_else(|| key_error(key))
    }

    fn __iter__(slf: &Bound<'_, Self>) -> DictIterator {
        Dict::iter_of(slf, ItemKind::Keys, false)
    }

    fn __reversed__(slf: &Bound<'_, Self>) -> DictIterator {
        Dict::iter_of(slf, ItemKind::Keys, true)
    }

    fn __richcmp__(
        slf: &Bound<'_, Self>,
        other: &Bound<'_, PyAny>,
        op: CompareOp,
    ) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let eq = match op {
            CompareOp::Eq | CompareOp::Ne => Dict::equals(slf, other)?,
            _ => None,
        };
        Ok(match eq {
            Some(eq) => (eq == matches!(op, CompareOp::Eq))
                .into_pyobject(py)?
                .to_owned()
                .into_any()
                .unbind(),
            None => py.NotImplemented(),
        })
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let this = slf.get();
        let name = this.type_name();
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
        let result = self.derived(py, same_schema || !self.is_typed())?;
        result.update_from(py, other)?;
        Ok(Bound::new(py, result)?.into_any().unbind())
    }

    fn __ror__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if !is_builtin_mapping(other) {
            return Ok(py.NotImplemented());
        }
        let result = Dict::empty(schema::any(), schema::any());
        result.update_from(py, other)?;
        let pairs = self
            .items_snapshot(py)
            .into_iter()
            .map(|(k, v)| (k.into_bound(py), v.into_bound(py)))
            .collect();
        result.insert_pairs(py, pairs, None)?;
        Ok(Bound::new(py, result)?.into_any().unbind())
    }

    fn __ior__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<()> {
        self.update_from(py, other)
    }

    fn keys(slf: &Bound<'_, Self>) -> DictKeys {
        DictKeys {
            dict: slf.clone().unbind(),
        }
    }

    fn values(slf: &Bound<'_, Self>) -> DictValues {
        DictValues {
            dict: slf.clone().unbind(),
        }
    }

    fn items(slf: &Bound<'_, Self>) -> DictItems {
        DictItems {
            dict: slf.clone().unbind(),
        }
    }

    #[pyo3(signature = (key, default=None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        default: Option<Py<PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        Ok(self
            .get_value(py, key)?
            .or(default)
            .unwrap_or_else(|| py.None()))
    }

    /// `pop(key[, default])`.
    #[pyo3(signature = (key, *default))]
    fn pop(
        &self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        default: &Bound<'_, PyTuple>,
    ) -> PyResult<Py<PyAny>> {
        if default.len() > 1 {
            return Err(PyTypeError::new_err(format!(
                "pop expected at most 2 arguments, got {}",
                default.len() + 1
            )));
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
        let item = self.inner.write(py).pop_last(py, &mut displaced);
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
        if let Some(v) = keys::lookup(py, &self.inner, key, classify_key, |inner, pos| {
            pos.map(|p| inner.value(py, p))
        })? {
            return Ok(v);
        }
        let none = py.None().into_bound(py);
        let value = self.prepare_value(default.unwrap_or(&none))?;
        let mut result = None;
        // The converted default may be a new container holding the last references to its
        // elements: when the key turned up meanwhile it is released after the lock.
        let mut unused = Vec::new();
        let res = keys::batch_upsert(
            py,
            &self.inner,
            [(prepared, value)],
            |inner, value, slot| {
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
                Ok(())
            },
        );
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
        let mut displaced = Vec::new();
        self.inner
            .write(py)
            .clear(&self.value_schema, &mut displaced);
        drop(displaced);
    }

    fn copy(&self, py: Python<'_>) -> PyResult<Dict> {
        self.derived(py, true)
    }

    fn __copy__(&self, py: Python<'_>) -> PyResult<Dict> {
        self.derived(py, true)
    }

    /// A builtin `dict` with the same items.
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        for (k, v) in self.items_snapshot(py) {
            out.set_item(k, v)?;
        }
        Ok(out)
    }

    fn __reduce__<'py>(
        &self,
        py: Python<'py>,
    ) -> PyResult<(Bound<'py, PyAny>, (Bound<'py, PyDict>,))> {
        Ok((self.schema_type_of(py)?, (self.to_dict(py)?,)))
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
        let displaced = self.inner.try_write().map(|mut inner| {
            let mut out = Vec::new();
            inner.clear(&self.value_schema, &mut out);
            out
        });
        drop(displaced);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemKind {
    Keys,
    Values,
    Items,
}

struct IterState {
    /// Next position to examine (forward), or one past it (reverse).
    pos: usize,
    expected: usize,
    remaining: usize,
    layout: u64,
    exhausted: bool,
}

/// Live iterator over a `Dict`'s keys, values or items.
#[pyclass(module = "rustique", frozen)]
pub struct DictIterator {
    dict: Py<Dict>,
    kind: ItemKind,
    reverse: bool,
    state: Locked<IterState>,
}

#[pymethods]
impl DictIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let (key, value) = {
            let mut st = self.state.write(py);
            if st.exhausted {
                return Ok(None);
            }
            let inner = self.dict.get().inner.read(py);
            if inner.table.len() != st.expected {
                st.expected = usize::MAX;
                return Err(PyRuntimeError::new_err(
                    "dictionary changed size during iteration",
                ));
            }
            if inner.table.layout() != st.layout {
                return Err(PyRuntimeError::new_err(
                    "dictionary keys changed during iteration",
                ));
            }
            let next = if self.reverse {
                inner.table.prev_live(st.pos)
            } else {
                inner.table.next_live(st.pos)
            };
            let Some(pos) = next else {
                st.exhausted = true;
                return Ok(None);
            };
            if st.remaining == 0 {
                st.expected = usize::MAX;
                return Err(PyRuntimeError::new_err(
                    "dictionary keys changed during iteration",
                ));
            }
            st.remaining -= 1;
            st.pos = if self.reverse { pos } else { pos + 1 };
            let key = (self.kind != ItemKind::Values).then(|| inner.table.key_object(py, pos));
            let value = (self.kind != ItemKind::Keys).then(|| inner.value(py, pos));
            (key, value)
        };
        Ok(Some(match (key, value) {
            (Some(k), Some(v)) => PyTuple::new(py, [k, v])?.into_any().unbind(),
            (Some(k), None) => k,
            (None, Some(v)) => v,
            (None, None) => py.None(),
        }))
    }

    fn __length_hint__(&self, py: Python<'_>) -> usize {
        let st = self.state.read(py);
        if st.exhausted { 0 } else { st.remaining }
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.dict)
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
    let name = obj
        .get_type()
        .name()
        .map(|n| n.to_string())
        .unwrap_or_default();
    (name == "dict_keys" || name == "dict_items")
        && obj
            .get_type()
            .module()
            .is_ok_and(|m| m.to_cow().is_ok_and(|m| m == "builtins"))
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

            fn __and__<'py>(slf: &Bound<'py, Self>, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
                view_set(slf.as_any())?.call_method1("intersection", (other,))
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
        let values = self.dict.get().inner.read(py).values_list(py);
        for v in values {
            if objects_equal(v.bind(py), value)? {
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
        let Ok(pair) = item.cast_exact::<PyTuple>() else { return Ok(false) };
        if pair.len() != 2 {
            return Ok(false);
        }
        match self.dict.get().get_value(py, &pair.get_item(0)?)? {
            Some(v) => objects_equal(v.bind(py), &pair.get_item(1)?),
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
    m.add_class::<DictIterator>()
}
