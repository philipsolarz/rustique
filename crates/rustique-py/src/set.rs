//! The `Set` class (ARCHITECTURE §4.3): an insertion-ordered set with native element
//! strategies (`i64`, `str`, or Python objects), and the set algebra it shares with
//! [`FrozenSet`](crate::frozenset::FrozenSet).
//!
//! OWNER: set agent. Items marked `INTERFACE` are used by other modules.
//!
//! Set algebra follows CPython's `setobject.c`: an intersection iterates the smaller operand
//! (the right one on ties) and keeps its elements, a union and a difference keep the left
//! operand's elements, a symmetric difference each side's own. When both operands are
//! rustique sets whose elements compare without Python code (native `int`/`str` keys, or
//! builtin scalars and tuples of them), the result is computed directly on the borrowed
//! tables — under read locks for a `Set`, without any lock for a `FrozenSet` — reusing the
//! stored hashes; otherwise elements are compared with Python equality outside every lock.

use std::ffi::c_int;
use std::hash::Hash;
use std::sync::Arc;

use pyo3::exceptions::{PyKeyError, PyRuntimeError, PyTypeError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::pyclass::CompareOp;
use pyo3::sync::critical_section::with_critical_section;
use pyo3::types::{PyDict, PyFrozenSet, PyList, PyRange, PyRangeMethods, PySet, PyTuple, PyType};
use pyo3::{Borrowed, PyTraverseError, ffi};
use rustique_core::Column;
use rustique_core::index::KeyIndex;

use crate::dict::keys::{
    self, Class, Key, KeyKind, KeyStore, KeyTable, Keyed, Locked, NativeKeys, Prepared, Role, Simple, Slot,
};
use crate::dict::{Raw, ReprGuard, collect_iterable, read_elements};
use crate::frozenset::FrozenSet;
use crate::list::List;
use crate::schema::{self, Schema};

pub(crate) struct SetInner {
    table: KeyTable,
}

impl Keyed for SetInner {
    fn table(&self) -> &KeyTable {
        &self.table
    }

    fn table_mut(&mut self) -> &mut KeyTable {
        &mut self.table
    }
}

impl SetInner {
    /// Remove the live element at `pos`; displaced objects go to `out`.
    fn remove_at(&mut self, py: Python<'_>, pos: usize, out: &mut Vec<Py<PyAny>>) {
        out.extend(self.table.remove_at(py, pos));
        // Sets have no parallel storage, so the remap is not needed.
        let _ = self.table.maybe_compact();
    }
}

// --- CPython-compatible hashing ----------------------------------------------------------

/// Python's `hash()` of a `str`, computed from its UTF-8 text without creating a `str`:
/// CPython hashes the code units of the canonical PEP 393 form (`unicode_hash`), which this
/// rebuilds (1, 2 or 4 bytes per code point, by the widest one). Runs no Python code.
pub(crate) fn py_str_hash(s: &str) -> isize {
    fn hash_units<T>(units: &[T]) -> isize {
        let len = std::mem::size_of_val(units) as ffi::Py_ssize_t;
        // SAFETY: `units` is a valid slice of `len` bytes, which is all `Py_HashBuffer` reads.
        unsafe { ffi::Py_HashBuffer(units.as_ptr().cast(), len) }
    }
    if s.is_ascii() {
        return hash_units(s.as_bytes());
    }
    let widest = s.chars().map(u32::from).max().unwrap_or(0);
    if widest < 0x100 {
        hash_units(&s.chars().map(|c| c as u8).collect::<Vec<u8>>())
    } else if widest < 0x1_0000 {
        hash_units(&s.chars().map(|c| c as u16).collect::<Vec<u16>>())
    } else {
        hash_units(&s.chars().map(u32::from).collect::<Vec<u32>>())
    }
}

/// CPython's `_shuffle_bits` (setobject.c).
#[inline]
fn shuffle_bits(h: u64) -> u64 {
    ((h ^ 89_869_747) ^ (h << 16)).wrapping_mul(3_644_798_167)
}

/// CPython's `frozenset_hash`, given the Python hashes of the elements (in any order): the
/// result is bit-identical to `hash(frozenset(elements))`.
pub(crate) fn frozenset_hash_of(hashes: impl Iterator<Item = isize>) -> isize {
    let (mut hash, mut n) = (0u64, 0u64);
    for h in hashes {
        hash ^= shuffle_bits(h as u64);
        n += 1;
    }
    hash ^= (n + 1).wrapping_mul(1_927_868_237);
    hash ^= (hash >> 11) ^ (hash >> 25);
    hash = hash.wrapping_mul(69_069).wrapping_add(907_133_923);
    if hash == u64::MAX {
        hash = 590_923_713;
    }
    hash as isize
}

/// `hash(frozenset(elements))` of a table, from the stored hashes (no Python code).
pub(crate) fn table_frozenset_hash(table: &KeyTable) -> isize {
    match table.store() {
        KeyStore::Empty => frozenset_hash_of(std::iter::empty()),
        KeyStore::Int(ki) => frozenset_hash_of(ki.iter().map(|(_, &v)| keys::py_hash_i64(v))),
        KeyStore::Str(ki) => frozenset_hash_of(ki.iter().map(|(_, s)| py_str_hash(s))),
        KeyStore::Object(ki) => {
            let index = ki.index();
            frozenset_hash_of(index.iter().map(|pos| keys::unmix(index.hash_at(pos).unwrap_or(0))))
        }
    }
}

/// Classify a lookup probe. An unhashable `Set` or builtin `set` is looked up as the
/// equivalent frozenset, like CPython (`self.thetype(letters) in set_of_frozensets`).
pub(crate) fn probe<'py>(elem: &Bound<'py, PyAny>) -> PyResult<Key<'py>> {
    match Key::classify(elem, Role::SetElement) {
        Err(e) if e.is_instance_of::<PyTypeError>(elem.py()) => {
            let py = elem.py();
            if let Ok(set) = elem.cast::<Set>() {
                // Compared by Python `==` (which `Set` defines against every set type), hashed
                // as the equivalent frozenset.
                let hash = table_frozenset_hash(&set.get().inner.read(py).table);
                return Ok(Key { obj: elem.clone(), class: Class::Other { hash, exotic: false, simple: Simple::No } });
            }
            if elem.is_instance_of::<PySet>() {
                // SAFETY: `elem` is a valid set; `PyFrozenSet_New` copies it (stored hashes).
                let frozen = unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyFrozenSet_New(elem.as_ptr()))? };
                return Key::classify(&frozen, Role::SetElement);
            }
            Err(e)
        }
        other => other,
    }
}

// --- reading builtin sets ----------------------------------------------------------------

unsafe extern "C" {
    /// CPython's `_PySet_NextEntry`: the next entry of a `set`/`frozenset` as a *borrowed*
    /// key and its stored hash. Exported by CPython 3.14 and 3.15 (declared in
    /// `pycore_setobject.h`); valid only while the set cannot change.
    fn _PySet_NextEntry(
        set: *mut ffi::PyObject,
        pos: *mut ffi::Py_ssize_t,
        key: *mut *mut ffi::PyObject,
        hash: *mut ffi::Py_hash_t,
    ) -> c_int;
}

/// Whether `obj` is a builtin `set` or `frozenset` (subclasses included).
#[inline]
fn is_builtin_set(obj: &Bound<'_, PyAny>) -> bool {
    // SAFETY: a type check on a valid object.
    unsafe { ffi::PyAnySet_Check(obj.as_ptr()) != 0 }
}

/// Visit the elements of a builtin `set`/`frozenset` with their stored hashes, inside one
/// critical section on the set. Like CPython's `set_merge`, the table is read directly,
/// whatever a subclass's `__iter__` says. `visit` gets borrowed items: it must not run Python
/// code (nor block); it returns `false` to stop.
fn for_each_set_entry<'py>(set: &Bound<'py, PyAny>, mut visit: impl FnMut(Borrowed<'_, 'py, PyAny>, isize) -> bool) {
    let py = set.py();
    with_critical_section(set, || {
        let (mut pos, mut key, mut hash) = (0, std::ptr::null_mut(), 0);
        // SAFETY: `set` is a set or frozenset (checked by every caller); we hold its critical
        // section and run no Python code, so its table and the borrowed keys stay valid.
        while unsafe { _PySet_NextEntry(set.as_ptr(), &mut pos, &mut key, &mut hash) } == 1 {
            // SAFETY: `key` is a live borrowed reference (see above).
            let item = unsafe { Borrowed::from_ptr(py, key) };
            if !visit(item, hash) {
                break;
            }
        }
    });
}

// --- building tables ---------------------------------------------------------------------

/// An element still to insert after its source was read, in order.
enum Pending<'py> {
    Native(Prepared<'static>),
    /// An object, with its hash when the source stored one (a builtin set's element).
    Object(Bound<'py, PyAny>, Option<isize>),
}

/// A private table holding the elements of `iterable` validated against `schema` (the first
/// of equal elements wins, as in CPython). Nobody else can reach the table, so Python code
/// (iteration, `__hash__`, `__eq__`, validation) may run between insertions. Builtin
/// `list`/`tuple`/`dict`/`set` sources are read inside their critical section, inserting
/// native keys directly until the first element that needs Python code; a rustique set is
/// copied; `range` and native `List`s are read without creating objects.
pub(crate) fn build_table<'py>(
    py: Python<'py>,
    schema: &Arc<Schema>,
    iterable: &Bound<'py, PyAny>,
    name: &dyn Fn() -> String,
) -> PyResult<KeyTable> {
    let kind = KeyKind::for_schema(schema);
    let mut table = KeyTable::new(kind);
    match ingest_native(py, &mut table, schema, iterable)? {
        Some(pending) => {
            for item in pending {
                insert_pending(py, &mut table, schema, item, name)?;
            }
        }
        None => {
            for item in iterable.try_iter()? {
                let key = keys::prepare_owned(item?, schema, Role::SetElement, name)?;
                table.insert_prepared(py, key)?;
            }
        }
    }
    Ok(table)
}

/// A copy of a rustique set's table when its elements need no validation for `schema`.
fn copy_rustique_set(py: Python<'_>, schema: &Arc<Schema>, obj: &Bound<'_, PyAny>) -> Option<KeyTable> {
    let fits = |theirs: &Arc<Schema>| schema.is_any() || same_schema(theirs, schema);
    if let Ok(set) = obj.cast::<Set>() {
        let set = set.get();
        return fits(&set.schema).then(|| set.snapshot_table(py));
    }
    if let Ok(frozen) = obj.cast::<FrozenSet>() {
        let frozen = frozen.get();
        return fits(frozen.schema()).then(|| frozen.table().clone_ref(py));
    }
    None
}

pub(crate) fn same_schema(a: &Arc<Schema>, b: &Arc<Schema>) -> bool {
    Arc::ptr_eq(a, b) || **a == **b
}

fn insert_pending<'py>(
    py: Python<'py>,
    table: &mut KeyTable,
    schema: &Schema,
    item: Pending<'py>,
    name: &dyn Fn() -> String,
) -> PyResult<()> {
    let key = match item {
        Pending::Native(key) => key,
        Pending::Object(obj, Some(hash)) => Prepared::Key(Key::with_hash(obj, hash)?),
        Pending::Object(obj, None) => keys::prepare_owned(obj, schema, Role::SetElement, name)?,
    };
    table.insert_prepared(py, key)
}

/// Collects the elements a source critical section could not insert natively.
struct Ingest<'py> {
    pending: Vec<Pending<'py>>,
    err: Option<PyErr>,
}

impl<'py> Ingest<'py> {
    fn new() -> Self {
        Ingest { pending: Vec::new(), err: None }
    }

    /// Take one borrowed source element: inserted natively while nothing is pending (no
    /// Python code runs: a private table of exact `int`/`str` keys compares inline), queued
    /// otherwise (the order of insertion is the source's).
    #[inline]
    fn push(
        &mut self,
        py: Python<'py>,
        table: &mut KeyTable,
        schema: &Schema,
        item: Borrowed<'_, 'py, PyAny>,
        hash: Option<isize>,
    ) {
        match keys::native_key(&item, schema) {
            Some(key) if self.pending.is_empty() && self.err.is_none() => {
                if let Err(e) = table.insert_prepared(py, key) {
                    self.err = Some(e);
                }
            }
            Some(key) => self.pending.push(Pending::Native(key)),
            None => self.pending.push(Pending::Object(item.to_owned(), if schema.is_any() { hash } else { None })),
        }
    }

    fn finish(self) -> PyResult<Option<Vec<Pending<'py>>>> {
        match self.err {
            Some(e) => Err(e),
            None => Ok(Some(self.pending)),
        }
    }
}

/// Read a source with a native fast path into `table` (empty); `None` for other sources
/// (iterate them generically).
fn ingest_native<'py>(
    py: Python<'py>,
    table: &mut KeyTable,
    schema: &Arc<Schema>,
    obj: &Bound<'py, PyAny>,
) -> PyResult<Option<Vec<Pending<'py>>>> {
    let mut ingest = Ingest::new();
    if let Ok(list) = obj.cast_exact::<PyList>() {
        with_critical_section(list.as_any(), || {
            let p = list.as_ptr();
            // SAFETY: we hold the list's critical section and run no Python code, so its size
            // and the borrowed item pointers stay valid while we read them.
            let n = unsafe { ffi::PyList_GET_SIZE(p) };
            table.reserve(n as usize);
            for i in 0..n {
                // SAFETY: `i` is in bounds (see above).
                let item = unsafe { Borrowed::from_ptr(py, ffi::PyList_GET_ITEM(p, i)) };
                ingest.push(py, table, schema, item, None);
            }
        });
        return ingest.finish();
    }
    if let Ok(tuple) = obj.cast_exact::<PyTuple>() {
        table.reserve(tuple.len());
        for item in tuple.iter_borrowed() {
            ingest.push(py, table, schema, item, None);
        }
        return ingest.finish();
    }
    if is_builtin_set(obj) {
        // SAFETY: `obj` is a set or frozenset.
        table.reserve(unsafe { ffi::PySet_Size(obj.as_ptr()) }.max(0) as usize);
        for_each_set_entry(obj, |item, hash| {
            ingest.push(py, table, schema, item, Some(hash));
            true
        });
        return ingest.finish();
    }
    if let Ok(dict) = obj.cast_exact::<PyDict>() {
        with_critical_section(dict.as_any(), || {
            table.reserve(dict.len());
            let (mut pos, mut k, mut v) = (0, std::ptr::null_mut(), std::ptr::null_mut());
            // SAFETY: we hold the dict's critical section and run no Python code, so the
            // iteration state and the borrowed keys stay valid.
            while unsafe { ffi::PyDict_Next(dict.as_ptr(), &mut pos, &mut k, &mut v) } != 0 {
                // SAFETY: `k` is a live borrowed key (see above).
                let key = unsafe { Borrowed::from_ptr(py, k) };
                ingest.push(py, table, schema, key, None);
            }
        });
        return ingest.finish();
    }
    if let Some(copy) = copy_rustique_set(py, schema, obj) {
        *table = copy.with_kind(KeyKind::for_schema(schema));
        return Ok(Some(Vec::new()));
    }
    let ints = matches!(**schema, Schema::Any | Schema::Int);
    if ints && let Ok(range) = obj.cast_exact::<PyRange>() {
        let (start, stop, step) = (range.start(), range.stop(), range.step());
        if let (Ok(start), Ok(stop), Ok(step)) = (start, stop, step) {
            let (start, stop, step) = (start as i64, stop as i64, step as i64);
            let n = crate::convert::range_len(start, stop, step);
            table.reserve(n);
            let mut v = start;
            for _ in 0..n {
                table.insert_prepared(py, Prepared::Int(v))?;
                v = v.wrapping_add(step);
            }
            return Ok(Some(Vec::new()));
        }
        return Ok(None);
    }
    if let Ok(list) = obj.cast::<List>() {
        let strs = matches!(**schema, Schema::Any | Schema::Str);
        let done = list.get().with_column(py, |column| match column {
            Column::I64(values) if ints => {
                table.reserve(values.len());
                values.iter().try_for_each(|&v| table.insert_prepared(py, Prepared::Int(v))).map(|()| true)
            }
            Column::Str(values) if strs => {
                table.reserve(values.len());
                values.iter().try_for_each(|s| table.insert_prepared(py, Prepared::Str(s.clone()))).map(|()| true)
            }
            _ => Ok(false),
        });
        if let Some(done) = done
            && done?
        {
            return Ok(Some(Vec::new()));
        }
    }
    Ok(None)
}

/// Collect `obj`'s elements into a private, untyped table (an operand of set algebra).
fn local_table(obj: &Bound<'_, PyAny>) -> PyResult<KeyTable> {
    let any = schema::any();
    build_table(obj.py(), &any, obj, &|| "Set".to_owned())
}

// --- operands ----------------------------------------------------------------------------

/// What a set operation's operand is.
pub(crate) enum Operand<'py> {
    Set(Bound<'py, Set>),
    Frozen(Bound<'py, FrozenSet>),
    /// Elements collected into a private table; `is_set` when they came from a builtin set.
    Local {
        table: KeyTable,
        is_set: bool,
    },
}

impl<'py> Operand<'py> {
    pub(crate) fn new(obj: &Bound<'py, PyAny>) -> PyResult<Operand<'py>> {
        if let Ok(s) = obj.cast::<Set>() {
            return Ok(Operand::Set(s.clone()));
        }
        if let Ok(f) = obj.cast::<FrozenSet>() {
            return Ok(Operand::Frozen(f.clone()));
        }
        Ok(Operand::Local { table: local_table(obj)?, is_set: is_builtin_set(obj) })
    }

    fn is_set(&self) -> bool {
        match self {
            Operand::Local { is_set, .. } => *is_set,
            _ => true,
        }
    }

    pub(crate) fn len(&self, py: Python<'_>) -> usize {
        match self {
            Operand::Set(s) => s.get().len(py),
            Operand::Frozen(f) => f.get().table().len(),
            Operand::Local { table, .. } => table.len(),
        }
    }

    fn contains(&self, key: &Key<'py>) -> PyResult<bool> {
        let py = key.obj.py();
        match self {
            Operand::Set(s) => s.get().contains_key(key),
            // An immutable table: comparisons may run Python code but cannot change it.
            Operand::Frozen(f) => Ok(f.get().table().find_local(py, key)?.is_some()),
            Operand::Local { table, .. } => Ok(table.find_local(py, key)?.is_some()),
        }
    }

    /// The elements, classified with their stored hashes.
    fn keys(&self, py: Python<'py>) -> PyResult<Vec<Key<'py>>> {
        self.with_table(py, |t| t.element_keys(py))
    }

    /// Native keys, when the operand stores native keys.
    fn native(&self, py: Python<'_>) -> Option<NativeKeys> {
        self.with_table(py, KeyTable::native_clone)
    }

    /// Schema of a rustique operand.
    fn schema(&self) -> Option<&Arc<Schema>> {
        match self {
            Operand::Set(s) => Some(&s.get().schema),
            Operand::Frozen(f) => Some(f.get().schema()),
            Operand::Local { .. } => None,
        }
    }

    /// A private copy of the table.
    fn snapshot(&self, py: Python<'_>) -> KeyTable {
        self.with_table(py, |t| t.clone_ref(py))
    }

    /// `f` on the table, read-locked for a `Set`. `f` must not run Python code.
    fn with_table<R>(&self, py: Python<'_>, f: impl FnOnce(&KeyTable) -> R) -> R {
        match self {
            Operand::Set(s) => f(&s.get().inner.read(py).table),
            Operand::Frozen(fs) => f(fs.get().table()),
            Operand::Local { table, .. } => f(table),
        }
    }

    fn same_object(&self, other: &Operand<'_>) -> bool {
        match (self, other) {
            (Operand::Set(a), Operand::Set(b)) => a.is(b),
            (Operand::Frozen(a), Operand::Frozen(b)) => a.is(b),
            _ => false,
        }
    }
}

/// `f` on both operands' tables at once (read-locked as needed, without lock-order
/// deadlocks). `f` must not run Python code.
fn with_tables<R>(py: Python<'_>, a: &Operand<'_>, b: &Operand<'_>, f: impl FnOnce(&KeyTable, &KeyTable) -> R) -> R {
    match (a, b) {
        (Operand::Set(x), Operand::Set(y)) if x.is(y) => {
            let guard = x.get().inner.read(py);
            f(&guard.table, &guard.table)
        }
        (Operand::Set(x), Operand::Set(y)) => {
            let (gx, gy) = crate::sync::read_both(py, &x.get().inner, &y.get().inner);
            f(&gx.table, &gy.table)
        }
        (Operand::Set(x), other) => {
            let guard = x.get().inner.read(py);
            other.with_table(py, |t| f(&guard.table, t))
        }
        (other, Operand::Set(y)) => {
            let guard = y.get().inner.read(py);
            other.with_table(py, |t| f(t, &guard.table))
        }
        (a, b) => a.with_table(py, |ta| b.with_table(py, |tb| f(ta, tb))),
    }
}

/// Result schema of a binary operation: `schema` (the left operand's) when every operand is
/// a rustique set of that schema, otherwise untyped.
pub(crate) fn result_schema(schema: &Arc<Schema>, others: &[Operand<'_>]) -> Arc<Schema> {
    if !schema.is_any() && others.iter().all(|o| o.schema().is_some_and(|s| same_schema(schema, s))) {
        schema.clone()
    } else {
        schema::any()
    }
}

/// Operator forms accept only `Set`, `FrozenSet`, `set` and `frozenset`.
pub(crate) fn is_set_operand(obj: &Bound<'_, PyAny>) -> bool {
    obj.is_instance_of::<Set>() || obj.is_instance_of::<FrozenSet>() || is_builtin_set(obj)
}

// --- algebra kernels on borrowed tables -------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Algebra {
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
}

/// The stored hash of the live key at `pos`.
#[inline]
fn stored_hash<K: Hash + Eq>(ki: &KeyIndex<K>, pos: usize, key: &K) -> u64 {
    ki.index().hash_at(pos).unwrap_or_else(|| ki.hash_key(key))
}

/// `a <op> b` for native keys of one kind (`b_set`: `b` came from a set, so an intersection
/// may iterate `a` when it is smaller; elements of an iterable are always iterated). Results
/// reuse the stored hashes of the table whose hasher they inherit.
fn native_op<K: Hash + Eq + Clone + Default>(op: Algebra, a: &KeyIndex<K>, b: &KeyIndex<K>, b_set: bool) -> KeyIndex<K> {
    match op {
        Algebra::Union => {
            let mut out = a.clone();
            out.reserve(b.len());
            for (_, k) in b.iter() {
                let h = a.hash_key(k);
                if a.find_hashed(h, |x| x == k).is_none() {
                    out.push_hashed(h, k.clone());
                }
            }
            out
        }
        Algebra::Intersection => {
            // CPython iterates the smaller set (the right one on ties, or an iterable) and keeps
            // its elements.
            let (small, large) = if b_set && a.len() < b.len() { (a, b) } else { (b, a) };
            let mut out = KeyIndex::with_capacity_and_hasher(small.len(), small.hasher().clone());
            for (pos, k) in small.iter() {
                if large.contains(k) {
                    out.push_hashed(stored_hash(small, pos, k), k.clone());
                }
            }
            out
        }
        Algebra::Difference if a.len() / 4 > b.len() => {
            // Most of `a` survives: copy it and remove `b`'s elements (CPython's
            // `set_copy_and_difference`).
            let mut out = a.clone();
            for (_, k) in b.iter() {
                out.remove(k);
            }
            if out.wants_compaction() {
                out.compact();
            }
            out
        }
        Algebra::Difference => {
            let mut out = KeyIndex::with_capacity_and_hasher(a.len(), a.hasher().clone());
            for (pos, k) in a.iter() {
                if !b.contains(k) {
                    out.push_hashed(stored_hash(a, pos, k), k.clone());
                }
            }
            out
        }
        Algebra::SymmetricDifference => {
            let mut out = KeyIndex::with_capacity_and_hasher(a.len(), a.hasher().clone());
            for (pos, k) in a.iter() {
                if !b.contains(k) {
                    out.push_hashed(stored_hash(a, pos, k), k.clone());
                }
            }
            for (_, k) in b.iter() {
                let h = a.hash_key(k);
                if a.find_hashed(h, |x| x == k).is_none() {
                    out.push_hashed(h, k.clone());
                }
            }
            out
        }
    }
}

/// Whether two stored objects are equal, decided without running Python code: identity, or
/// `==` between builtin scalars (and tuples of them). `None` when `==` could run Python code.
#[inline]
fn inline_equal(a: &Bound<'_, PyAny>, b: &Bound<'_, PyAny>) -> Option<bool> {
    if a.is(b) {
        return Some(true);
    }
    match (keys::simplicity(a, 0), keys::simplicity(b, 0)) {
        (Simple::Yes, Simple::Yes) | (Simple::Bytes, Simple::Bytes) => a.eq(b).ok(),
        _ => None,
    }
}

type ObjectIndex = KeyIndex<Py<PyAny>, ()>;

/// Whether the object table `ki` holds an element equal to `obj` (whose mixed hash is `hash`),
/// without running Python code; `None` when that would take a Python comparison.
#[inline]
fn object_contains(py: Python<'_>, ki: &ObjectIndex, obj: &Bound<'_, PyAny>, hash: u64) -> Option<bool> {
    for pos in ki.index().candidates(hash) {
        // CPython's operand order: stored == probe.
        if inline_equal(ki.raw_keys()[pos].bind(py), obj)? {
            return Some(true);
        }
    }
    Some(false)
}

/// The live `(position, object, mixed hash)` entries of an object table.
fn object_entries<'a, 'py: 'a>(
    py: Python<'py>,
    ki: &'a ObjectIndex,
) -> impl Iterator<Item = (usize, &'a Bound<'py, PyAny>, u64)> + 'a {
    let index = ki.index();
    index.iter().map(move |pos| (pos, ki.raw_keys()[pos].bind(py), index.hash_at(pos).unwrap_or(0)))
}

/// A key to append to an object table: `push` only reads its (already known) hash.
#[inline]
fn hashed_key<'py>(obj: &Bound<'py, PyAny>, mixed: u64) -> Key<'py> {
    Key { obj: obj.clone(), class: Class::Other { hash: keys::unmix(mixed), exotic: true, simple: Simple::No } }
}

/// `a <op> b` for two object tables whose elements compare without Python code; `None` as
/// soon as some comparison would need it. Object tables share one hash mixing, so stored
/// hashes are reused across them.
fn object_op(
    py: Python<'_>,
    op: Algebra,
    (ta, a): (&KeyTable, &ObjectIndex),
    (b, b_set): (&ObjectIndex, bool),
    kind: KeyKind,
) -> Option<KeyTable> {
    let push = |out: &mut KeyTable, obj: &Bound<'_, PyAny>, h: u64| out.push(py, &hashed_key(obj, h)).ok().map(drop);
    let mut out = match op {
        Algebra::Union => ta.clone_ref(py).with_kind(kind),
        _ => KeyTable::new(kind),
    };
    match op {
        Algebra::Union | Algebra::SymmetricDifference => {
            out.reserve(b.len());
            if op == Algebra::SymmetricDifference {
                for (_, obj, h) in object_entries(py, a) {
                    if !object_contains(py, b, obj, h)? {
                        push(&mut out, obj, h)?;
                    }
                }
            }
            for (_, obj, h) in object_entries(py, b) {
                if !object_contains(py, a, obj, h)? {
                    push(&mut out, obj, h)?;
                }
            }
        }
        Algebra::Intersection => {
            let (small, large) = if b_set && a.len() < b.len() { (a, b) } else { (b, a) };
            for (_, obj, h) in object_entries(py, small) {
                if object_contains(py, large, obj, h)? {
                    push(&mut out, obj, h)?;
                }
            }
        }
        Algebra::Difference => {
            for (_, obj, h) in object_entries(py, a) {
                if !object_contains(py, b, obj, h)? {
                    push(&mut out, obj, h)?;
                }
            }
        }
    }
    Some(out)
}

/// `a <op> b` without Python code, or `None` when Python equality is needed (`b_set`: `b`
/// came from a set). Runs under the operands' read locks: creates no objects other than
/// increfs of stored ones (and drops only such copies, whose objects the operands still hold).
fn table_op(py: Python<'_>, op: Algebra, a: &KeyTable, (b, b_set): (&KeyTable, bool), kind: KeyKind) -> Option<KeyTable> {
    let copy = |t: &KeyTable| t.clone_ref(py).with_kind(kind);
    match (a.store(), b.store()) {
        (KeyStore::Int(x), KeyStore::Int(y)) => {
            return Some(KeyTable::from_native(NativeKeys::Int(native_op(op, x, y, b_set)), kind));
        }
        (KeyStore::Str(x), KeyStore::Str(y)) => {
            return Some(KeyTable::from_native(NativeKeys::Str(native_op(op, x, y, b_set)), kind));
        }
        _ => {}
    }
    if a.len() == 0 {
        return Some(match op {
            Algebra::Intersection | Algebra::Difference => KeyTable::new(kind),
            Algebra::Union | Algebra::SymmetricDifference => copy(b),
        });
    }
    if b.len() == 0 {
        return Some(match op {
            Algebra::Intersection => KeyTable::new(kind),
            _ => copy(a),
        });
    }
    match (a.store(), b.store()) {
        // A `str` never equals an `int`.
        (KeyStore::Int(_), KeyStore::Str(_)) | (KeyStore::Str(_), KeyStore::Int(_)) => match op {
            Algebra::Intersection => Some(KeyTable::new(kind)),
            Algebra::Difference => Some(copy(a)),
            _ => None,
        },
        (KeyStore::Object(x), KeyStore::Object(y)) => object_op(py, op, (a, x), (y, b_set), kind),
        _ => None,
    }
}

/// `a <= b` without Python code (`None`: Python equality needed).
fn table_subset(py: Python<'_>, a: &KeyTable, b: &KeyTable) -> Option<bool> {
    if a.len() > b.len() {
        return Some(false);
    }
    if a.len() == 0 {
        return Some(true);
    }
    match (a.store(), b.store()) {
        (KeyStore::Int(x), KeyStore::Int(y)) => Some(x.iter().all(|(_, k)| y.contains(k))),
        (KeyStore::Str(x), KeyStore::Str(y)) => Some(x.iter().all(|(_, k)| y.contains(k))),
        (KeyStore::Int(_), KeyStore::Str(_)) | (KeyStore::Str(_), KeyStore::Int(_)) => Some(false),
        (KeyStore::Object(x), KeyStore::Object(y)) => {
            for (_, obj, h) in object_entries(py, x) {
                if !object_contains(py, y, obj, h)? {
                    return Some(false);
                }
            }
            Some(true)
        }
        _ => None,
    }
}

/// Whether `a` and `b` share no element, without Python code (`None`: Python equality needed).
fn table_disjoint(py: Python<'_>, a: &KeyTable, b: &KeyTable) -> Option<bool> {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if small.len() == 0 {
        return Some(true);
    }
    match (small.store(), large.store()) {
        (KeyStore::Int(x), KeyStore::Int(y)) => Some(!x.iter().any(|(_, k)| y.contains(k))),
        (KeyStore::Str(x), KeyStore::Str(y)) => Some(!x.iter().any(|(_, k)| y.contains(k))),
        (KeyStore::Int(_), KeyStore::Str(_)) | (KeyStore::Str(_), KeyStore::Int(_)) => Some(true),
        (KeyStore::Object(x), KeyStore::Object(y)) => {
            for (_, obj, h) in object_entries(py, x) {
                if object_contains(py, y, obj, h)? {
                    return Some(false);
                }
            }
            Some(true)
        }
        _ => None,
    }
}

// --- algebra with Python equality --------------------------------------------------------

/// `left <op> right`: natively when possible, otherwise with Python equality on private
/// copies (no lock held while comparing).
fn combine(py: Python<'_>, op: Algebra, left: &Operand<'_>, right: &Operand<'_>, kind: KeyKind) -> PyResult<KeyTable> {
    if left.same_object(right) {
        // `s | s`, `s & s` copy, `s - s`, `s ^ s` are empty.
        return Ok(match op {
            Algebra::Union | Algebra::Intersection => left.snapshot(py).with_kind(kind),
            _ => KeyTable::new(kind),
        });
    }
    if let Some(table) = with_tables(py, left, right, |a, b| table_op(py, op, a, (b, right.is_set()), kind)) {
        return Ok(table);
    }
    table_algebra(py, op, left.snapshot(py).with_kind(kind), right, kind)
}

/// `left <op> others[0] <op> others[1] …`.
pub(crate) fn fold(
    py: Python<'_>,
    op: Algebra,
    left: &Operand<'_>,
    others: &[Operand<'_>],
    kind: KeyKind,
) -> PyResult<KeyTable> {
    let Some((first, rest)) = others.split_first() else {
        return Ok(left.snapshot(py).with_kind(kind));
    };
    let mut acc = combine(py, op, left, first, kind)?;
    for other in rest {
        acc = match other.with_table(py, |b| native_into(op, acc, b, kind)) {
            Ok(table) => table,
            Err(table) => match other.with_table(py, |b| table_op(py, op, &table, (b, other.is_set()), kind)) {
                Some(result) => result,
                None => table_algebra(py, op, table, other, kind)?,
            },
        };
    }
    Ok(acc)
}

/// `acc <op> other` in `acc`'s own storage, for a union or difference of native keys of one
/// kind; `acc` back otherwise.
fn native_into(op: Algebra, acc: KeyTable, other: &KeyTable, kind: KeyKind) -> Result<KeyTable, KeyTable> {
    if !matches!(op, Algebra::Union | Algebra::Difference) {
        return Err(acc);
    }
    match (acc.into_native(), other.store()) {
        (Ok(NativeKeys::Int(mut ki)), KeyStore::Int(theirs)) => {
            native_update(op, &mut ki, theirs);
            Ok(KeyTable::from_native(NativeKeys::Int(ki), kind))
        }
        (Ok(NativeKeys::Str(mut ki)), KeyStore::Str(theirs)) => {
            native_update(op, &mut ki, theirs);
            Ok(KeyTable::from_native(NativeKeys::Str(ki), kind))
        }
        (Ok(native), _) => Err(KeyTable::from_native(native, kind)),
        (Err(table), _) => Err(*table),
    }
}

/// `ki |= theirs` or `ki -= theirs` (other operations are not done in place).
fn native_update<K: Hash + Eq + Clone + Default>(op: Algebra, ki: &mut KeyIndex<K>, theirs: &KeyIndex<K>) {
    if op == Algebra::Union {
        ki.reserve(theirs.len());
        for (_, k) in theirs.iter() {
            let h = ki.hash_key(k);
            if ki.find_hashed(h, |x| x == k).is_none() {
                ki.push_hashed(h, k.clone());
            }
        }
    } else {
        for (_, k) in theirs.iter() {
            ki.remove(k);
        }
        if ki.wants_compaction() {
            ki.compact();
        }
    }
}

/// `mine <op> other` with Python equality (`mine` is private).
fn table_algebra(py: Python<'_>, op: Algebra, mine: KeyTable, other: &Operand<'_>, kind: KeyKind) -> PyResult<KeyTable> {
    match op {
        Algebra::Union => {
            let mut mine = mine;
            for key in &other.keys(py)? {
                if mine.upsert_fast(key).is_none() {
                    mine.insert_local(py, key)?;
                }
            }
            Ok(mine)
        }
        Algebra::Intersection => {
            // CPython iterates the smaller set (or the iterable) and keeps its elements.
            if other.is_set() && other.len(py) > mine.len() {
                return retain_contained(py, mine, other, kind);
            }
            let mut out = KeyTable::new(kind);
            for key in &other.keys(py)? {
                if mine.find_local(py, key)?.is_some() {
                    out.push(py, key)?;
                }
            }
            Ok(out)
        }
        Algebra::Difference => {
            let mut out = KeyTable::new(kind);
            for key in &mine.element_keys(py)? {
                if !other.contains(key)? {
                    out.push(py, key)?;
                }
            }
            Ok(out)
        }
        Algebra::SymmetricDifference => {
            let mut mine = mine;
            for key in &other.keys(py)? {
                match mine.find_local(py, key)? {
                    Some(pos) => {
                        drop(mine.remove_at(py, pos));
                        let _ = mine.maybe_compact();
                    }
                    None => {
                        mine.push(py, key)?;
                    }
                }
            }
            Ok(mine)
        }
    }
}

/// The elements of `mine` (keeping their objects) that `other` contains.
fn retain_contained(py: Python<'_>, mine: KeyTable, other: &Operand<'_>, kind: KeyKind) -> PyResult<KeyTable> {
    let native = other.with_table(py, |theirs| match (mine.store(), theirs.store()) {
        (KeyStore::Int(a), KeyStore::Int(b)) => Some(NativeKeys::Int(retain_native(a, b))),
        (KeyStore::Str(a), KeyStore::Str(b)) => Some(NativeKeys::Str(retain_native(a, b))),
        _ => None,
    });
    if let Some(native) = native {
        return Ok(KeyTable::from_native(native, kind));
    }
    let mut out = KeyTable::new(kind);
    for key in &mine.element_keys(py)? {
        if other.contains(key)? {
            out.push(py, key)?;
        }
    }
    Ok(out)
}

/// `table & natives[0] & …` when all of them store native keys of one kind (`None`
/// otherwise); typed tables keep their own order, like [`retain_contained`].
fn native_intersection(table: &KeyTable, natives: &[NativeKeys], kind: KeyKind, typed: bool) -> Option<KeyTable> {
    let (first, rest) = natives.split_first()?;
    let mut result = match (table.store(), first) {
        (KeyStore::Int(a), NativeKeys::Int(b)) if typed => NativeKeys::Int(retain_native(a, b)),
        (KeyStore::Int(a), NativeKeys::Int(b)) => NativeKeys::Int(native_op(Algebra::Intersection, a, b, true)),
        (KeyStore::Str(a), NativeKeys::Str(b)) if typed => NativeKeys::Str(retain_native(a, b)),
        (KeyStore::Str(a), NativeKeys::Str(b)) => NativeKeys::Str(native_op(Algebra::Intersection, a, b, true)),
        _ => return None,
    };
    for theirs in rest {
        result = match (result, theirs) {
            (NativeKeys::Int(a), NativeKeys::Int(b)) if typed => NativeKeys::Int(retain_native(&a, b)),
            (NativeKeys::Str(a), NativeKeys::Str(b)) if typed => NativeKeys::Str(retain_native(&a, b)),
            (NativeKeys::Int(a), NativeKeys::Int(b)) => NativeKeys::Int(native_op(Algebra::Intersection, &a, b, true)),
            (NativeKeys::Str(a), NativeKeys::Str(b)) => NativeKeys::Str(native_op(Algebra::Intersection, &a, b, true)),
            _ => return None,
        };
    }
    Some(KeyTable::from_native(result, kind))
}

/// Whether two keys are the same stored key: one object, or equal native `int`s/`str`s.
fn same_key(a: &Key<'_>, b: &Key<'_>) -> bool {
    a.obj.is(&b.obj)
        || match (&a.class, &b.class) {
            (Class::Int { value: x, exact: true }, Class::Int { value: y, exact: true }) => x == y,
            (Class::Str { text: x, exact: true }, Class::Str { text: y, exact: true }) => **x == **y,
            _ => false,
        }
}

fn retain_native<K: Hash + Eq + Clone + Default>(a: &KeyIndex<K>, b: &KeyIndex<K>) -> KeyIndex<K> {
    let mut out = KeyIndex::with_capacity_and_hasher(a.len().min(b.len()), a.hasher().clone());
    for (pos, k) in a.iter() {
        if b.contains(k) {
            out.push_hashed(stored_hash(a, pos, k), k.clone());
        }
    }
    out
}

// --- comparisons -------------------------------------------------------------------------

/// `a <= b` (CPython's `set_issubset`: stops at the first missing element).
pub(crate) fn is_subset(py: Python<'_>, a: &Operand<'_>, b: &Operand<'_>) -> PyResult<bool> {
    if a.same_object(b) {
        return Ok(true);
    }
    if let Some(result) = with_tables(py, a, b, |x, y| table_subset(py, x, y)) {
        return Ok(result);
    }
    if a.len(py) > b.len(py) {
        return Ok(false);
    }
    for key in &a.keys(py)? {
        if !b.contains(key)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `a.isdisjoint(b)`: iterates the smaller set (or `b`, when it is not a set).
pub(crate) fn is_disjoint(py: Python<'_>, a: &Operand<'_>, b: &Operand<'_>) -> PyResult<bool> {
    if a.same_object(b) {
        return Ok(a.len(py) == 0);
    }
    if let Some(result) = with_tables(py, a, b, |x, y| table_disjoint(py, x, y)) {
        return Ok(result);
    }
    let (iterated, probed) = if b.is_set() && b.len(py) > a.len(py) { (a, b) } else { (b, a) };
    for key in &iterated.keys(py)? {
        if probed.contains(key)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Rich comparison of a rustique set (`me`, of length `mine`, cached hash `my_hash`) with
/// `other`: equality and subset ordering against every set type, `NotImplemented` otherwise.
pub(crate) fn richcompare(
    py: Python<'_>,
    me: &Operand<'_>,
    other: &Bound<'_, PyAny>,
    op: CompareOp,
    my_hash: Option<isize>,
) -> PyResult<Py<PyAny>> {
    if !is_set_operand(other) {
        return Ok(py.NotImplemented());
    }
    let operand = Operand::new(other)?;
    let (mine, theirs) = (me.len(py), operand.len(py));
    let result = match op {
        CompareOp::Eq | CompareOp::Ne => {
            let differ_by_hash = match (&operand, my_hash) {
                (Operand::Frozen(f), Some(h)) => f.get().cached_hash().is_some_and(|theirs| theirs != h),
                _ => false,
            };
            let eq = me.same_object(&operand) || (mine == theirs && !differ_by_hash && is_subset(py, me, &operand)?);
            eq == matches!(op, CompareOp::Eq)
        }
        CompareOp::Le => is_subset(py, me, &operand)?,
        CompareOp::Lt => mine < theirs && is_subset(py, me, &operand)?,
        CompareOp::Ge => is_subset(py, &operand, me)?,
        CompareOp::Gt => mine > theirs && is_subset(py, &operand, me)?,
    };
    Ok(pyo3::types::PyBool::new(py, result).to_owned().into_any().unbind())
}

/// `issuperset(other)`: every element of `other` is in `me`.
pub(crate) fn is_superset(py: Python<'_>, me: &Operand<'_>, other: &Operand<'_>) -> PyResult<bool> {
    is_subset(py, other, me)
}

/// Repr of a set type: `Name({e1, e2})`, `Name()` when empty, `Name(...)` when recursive.
pub(crate) fn set_repr(slf: &Bound<'_, PyAny>, name: &str, elems: impl FnOnce() -> Vec<Py<PyAny>>) -> PyResult<String> {
    let py = slf.py();
    let Some(_guard) = ReprGuard::enter(slf)? else {
        return Ok(format!("{name}(...)"));
    };
    let elems = elems();
    if elems.is_empty() {
        return Ok(format!("{name}()"));
    }
    let mut out = format!("{name}({{");
    for (i, e) in elems.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&e.bind(py).repr()?.to_cow()?);
    }
    out.push_str("})");
    Ok(out)
}

/// The display name of a set type: the class name for subclasses, else `Base` or `Base[T]`.
pub(crate) fn display_name(slf: &Bound<'_, PyAny>, base: &Bound<'_, PyType>, base_name: &str, schema: &Schema) -> PyResult<String> {
    let ty = slf.get_type();
    if !ty.is(base) {
        return Ok(ty.name()?.to_string());
    }
    Ok(if schema.is_any() { base_name.to_owned() } else { format!("{base_name}[{schema}]") })
}

/// `__reduce__` of a set type: `(cls, (elements,))`, plus the instance `__dict__` of a
/// subclass instance (CPython's `set___reduce__`).
pub(crate) fn reduce<'py>(
    slf: &Bound<'py, PyAny>,
    base: &Bound<'py, PyType>,
    alias: Bound<'py, PyAny>,
    elems: Vec<Py<PyAny>>,
) -> PyResult<Bound<'py, PyTuple>> {
    let py = slf.py();
    let list = PyList::new(py, elems)?;
    let ty = slf.get_type();
    if ty.is(base) {
        return PyTuple::new(py, [alias, PyTuple::new(py, [list])?.into_any()]);
    }
    let state = match slf.getattr(pyo3::intern!(py, "__dict__")) {
        Ok(d) => d,
        Err(_) => py.None().into_bound(py),
    };
    PyTuple::new(py, [ty.into_any(), PyTuple::new(py, [list])?.into_any(), state])
}

// --- Set ---------------------------------------------------------------------------------

/// An insertion-ordered set with native element storage.
#[pyclass(module = "rustique", frozen, weakref, subclass, immutable_type)]
pub struct Set {
    schema: Arc<Schema>,
    inner: Locked<SetInner>,
}

impl Set {
    /// INTERFACE. Construct from an optional iterable.
    pub fn from_iterable<'py>(
        py: Python<'py>,
        schema: Arc<Schema>,
        iterable: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, Set>> {
        Bound::new(py, Set::build(py, schema, iterable)?)
    }

    /// INTERFACE.
    pub fn schema(&self) -> &Arc<Schema> {
        &self.schema
    }

    pub(crate) fn with_table(schema: Arc<Schema>, table: KeyTable) -> Set {
        Set { schema, inner: Locked::new(SetInner { table }) }
    }

    fn empty(schema: Arc<Schema>) -> Set {
        let table = KeyTable::new(KeyKind::for_schema(&schema));
        Set::with_table(schema, table)
    }

    fn build<'py>(py: Python<'py>, schema: Arc<Schema>, iterable: Option<&Bound<'py, PyAny>>) -> PyResult<Set> {
        let Some(iterable) = iterable else {
            return Ok(Set::empty(schema));
        };
        let table = build_table(py, &schema, iterable, &|| Set::name_for(&schema))?;
        Ok(Set::with_table(schema, table))
    }

    fn name_for(schema: &Schema) -> String {
        if schema.is_any() { "Set".to_owned() } else { format!("Set[{schema}]") }
    }

    fn type_name(&self) -> String {
        Set::name_for(&self.schema)
    }

    fn prepare<'py>(&self, elem: Bound<'py, PyAny>) -> PyResult<Prepared<'py>> {
        keys::prepare_owned(elem, &self.schema, Role::SetElement, || self.type_name())
    }

    /// Read and validate the elements of `iterable`; stops at the first invalid one.
    fn prepare_all<'py>(&self, iterable: &Bound<'py, PyAny>) -> (Vec<Prepared<'py>>, Option<PyErr>) {
        let (raw, mut err) = read_elements(iterable, &self.schema);
        let mut out = Vec::with_capacity(raw.len());
        for item in raw {
            let prepared = match item {
                Raw::Ready(native) => Ok(native),
                Raw::Later(obj) => self.prepare(obj),
            };
            match prepared {
                Ok(p) => out.push(p),
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        (out, err)
    }

    pub(crate) fn len(&self, py: Python<'_>) -> usize {
        self.inner.read(py).table.len()
    }

    pub(crate) fn element_objects(&self, py: Python<'_>) -> Vec<Py<PyAny>> {
        self.inner.read(py).table.key_objects(py)
    }

    /// `elem in self`. Identity hits and clean misses in object tables are decided with one
    /// hash and no classification.
    fn contains(&self, elem: &Bound<'_, PyAny>) -> PyResult<bool> {
        let py = elem.py();
        if let Some(probe) = keys::quick(elem) {
            let guard = self.inner.read(py);
            if let Some(pos) = guard.table.find_quick(&probe) {
                return Ok(pos.is_some());
            }
        }
        let Ok(hash) = elem.hash() else {
            // Unhashable: a set probe or an error (raised again by the full path).
            return keys::lookup(py, &self.inner, elem, probe, |_, pos| pos.is_some());
        };
        {
            let guard = self.inner.read(py);
            if let KeyStore::Object(ki) = guard.table.store() {
                let mut candidates = ki.index().candidates(keys::mix(hash)).peekable();
                if candidates.peek().is_none() {
                    return Ok(false);
                }
                if candidates.any(|pos| ki.raw_keys()[pos].is(elem)) {
                    return Ok(true);
                }
            }
        }
        let key = Key::with_hash(elem.clone(), hash)?;
        keys::with_entry(py, &self.inner, &key, |_, pos| pos.is_some())
    }

    fn contains_key(&self, key: &Key<'_>) -> PyResult<bool> {
        keys::with_entry(key.obj.py(), &self.inner, key, |_, pos| pos.is_some())
    }

    /// The native keys of a rustique set source as insertable keys, when this set stores them
    /// as they are (untyped, or typed with the same native kind).
    fn native_source(&self, py: Python<'_>, source: &Bound<'_, PyAny>) -> Option<Vec<Prepared<'static>>> {
        let (ints, strs) = match *self.schema {
            Schema::Any => (true, true),
            Schema::Int => (true, false),
            Schema::Str => (false, true),
            _ => return None,
        };
        let read = |table: &KeyTable| match table.store() {
            KeyStore::Int(ki) if ints => Some(ki.iter().map(|(_, &v)| Prepared::Int(v)).collect()),
            KeyStore::Str(ki) if strs => Some(ki.iter().map(|(_, s)| Prepared::Str(s.clone())).collect()),
            KeyStore::Empty => Some(Vec::new()),
            _ => None,
        };
        if let Ok(set) = source.cast::<Set>() {
            return read(&set.get().inner.read(py).table);
        }
        if let Ok(frozen) = source.cast::<FrozenSet>() {
            return read(frozen.get().table());
        }
        None
    }

    /// Insert every element of `iterable` (validated) in order; the valid prefix is kept
    /// when an element is rejected.
    fn add_all(&self, py: Python<'_>, iterable: &Bound<'_, PyAny>) -> PyResult<()> {
        let (prepared, err) = match self.native_source(py, iterable) {
            Some(natives) => (natives, None),
            None => self.prepare_all(iterable),
        };
        if prepared.len() > 1 {
            self.inner.write(py).table.reserve(prepared.len());
        }
        keys::batch_upsert(py, &self.inner, prepared.into_iter().map(|p| (p, ())), |inner, (), slot| {
            if let Slot::Absent(key) = slot {
                inner.table.push(py, key)?;
            }
            Ok(())
        })?;
        err.map_or(Ok(()), Err)
    }

    /// Remove every element of `iterable` that is present.
    fn remove_all(&self, py: Python<'_>, iterable: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Some(natives) = self.native_source(py, iterable) {
            let mut displaced = Vec::new();
            let mut inner = self.inner.write(py);
            if !matches!(inner.table.store(), KeyStore::Object(_)) {
                for key in &natives {
                    let probe = match key {
                        Prepared::Int(v) => keys::Quick::Int(*v),
                        Prepared::Str(s) => keys::Quick::Str(s),
                        Prepared::Key(_) => continue,
                    };
                    if let Some(Some(pos)) = inner.table.find_quick(&probe) {
                        inner.remove_at(py, pos, &mut displaced);
                    }
                }
                drop(inner);
                drop(displaced);
                return Ok(());
            }
        }
        let (items, err) = collect_iterable(iterable);
        if let Some(err) = err {
            return Err(err);
        }
        let probes = items.iter().map(probe).collect::<PyResult<Vec<_>>>()?;
        let mut displaced = Vec::new();
        keys::batch_mut(
            py,
            &self.inner,
            probes,
            |k| k,
            |inner, _, pos| {
                if let Some(pos) = pos {
                    inner.remove_at(py, pos, &mut displaced);
                }
                Ok(())
            },
        )?;
        drop(displaced);
        Ok(())
    }

    /// Remove `elem`; returns whether it was present.
    fn discard_elem(&self, py: Python<'_>, elem: &Bound<'_, PyAny>) -> PyResult<bool> {
        let mut displaced = Vec::new();
        let found = keys::lookup_mut(py, &self.inner, elem, probe, |inner, pos| {
            pos.map(|p| inner.remove_at(py, p, &mut displaced)).is_some()
        })?;
        drop(displaced);
        Ok(found)
    }

    fn snapshot_table(&self, py: Python<'_>) -> KeyTable {
        self.inner.read(py).table.clone_ref(py)
    }

    /// `self &= operand & …` in place. Untyped sets keep the elements CPython keeps (which
    /// side's object survives depends on the operand sizes); typed sets keep their own,
    /// already validated, elements.
    fn intersection_update_with(&self, py: Python<'_>, operands: &[Operand<'_>]) -> PyResult<()> {
        let kind = KeyKind::for_schema(&self.schema);
        let typed = !self.schema.is_any();
        // Native keys on every side need no Python code: one step under the write lock, so
        // concurrent writers cannot invalidate it.
        if let Some(natives) = operands.iter().map(|o| o.native(py)).collect::<Option<Vec<_>>>() {
            let mut inner = self.inner.write(py);
            if let Some(table) = native_intersection(&inner.table, &natives, kind, typed) {
                let displaced = inner.table.replace(table);
                drop(inner);
                drop(displaced);
                return Ok(());
            }
        }
        // Decide each element's fate on a snapshot (Python equality, unlocked), then apply
        // the decisions to the elements still present; elements added meanwhile stay.
        let snapshot = self.snapshot_table(py);
        let elements = snapshot.element_keys(py)?;
        let mut result = snapshot;
        for operand in operands {
            result = if typed {
                retain_contained(py, result, operand, kind)?
            } else {
                table_algebra(py, Algebra::Intersection, result, operand, kind)?
            };
        }
        let mut fates = Vec::new();
        for key in elements {
            match result.find_local(py, &key)? {
                None => fates.push((key, None)),
                Some(pos) => {
                    let kept = result.element_key(py, pos)?;
                    if !same_key(&key, &kept) {
                        fates.push((key, Some(kept)));
                    }
                }
            }
        }
        let mut displaced = Vec::new();
        let res = self.inner.write(py).table.apply_fates(py, &fates, &mut displaced);
        drop(displaced);
        drop(fates);
        drop(result);
        res
    }

    fn schema_type_of<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if self.schema.is_any() {
            Ok(py.get_type::<Set>().into_any())
        } else {
            crate::typing::set_alias_for(py, &self.schema)
        }
    }

    /// `self <op> others…` as a new `Set` (typed when every operand has this set's schema).
    fn compute(slf: &Bound<'_, Self>, op: Algebra, others: &[Bound<'_, PyAny>]) -> PyResult<Set> {
        let py = slf.py();
        let operands = others.iter().map(Operand::new).collect::<PyResult<Vec<_>>>()?;
        let schema = result_schema(&slf.get().schema, &operands);
        let kind = KeyKind::for_schema(&schema);
        let table = fold(py, op, &Operand::Set(slf.clone()), &operands, kind)?;
        Ok(Set::with_table(schema, table))
    }

    fn binary(slf: &Bound<'_, Self>, op: Algebra, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        if !is_set_operand(other) {
            return Ok(py.NotImplemented());
        }
        Ok(Bound::new(py, Set::compute(slf, op, std::slice::from_ref(other))?)?.into_any().unbind())
    }

    fn in_place(&self, py: Python<'_>, op: Algebra, others: &[Bound<'_, PyAny>]) -> PyResult<()> {
        match op {
            Algebra::Union => others.iter().try_for_each(|other| self.add_all(py, other)),
            Algebra::Difference => others.iter().try_for_each(|other| self.remove_all(py, other)),
            Algebra::Intersection => {
                let operands = others.iter().map(Operand::new).collect::<PyResult<Vec<_>>>()?;
                self.intersection_update_with(py, &operands)
            }
            Algebra::SymmetricDifference => {
                // Validate, then de-duplicate the incoming elements (Python equality) first.
                let (prepared, err) = self.prepare_all(&others[0]);
                if let Some(err) = err {
                    return Err(err);
                }
                let mut incoming = KeyTable::new(KeyKind::Adaptive);
                for p in prepared {
                    incoming.insert_prepared(py, p)?;
                }
                let incoming = incoming.element_keys(py)?;
                let mut displaced = Vec::new();
                keys::batch_mut(
                    py,
                    &self.inner,
                    incoming,
                    |k| k,
                    |inner, key, pos| {
                        match pos {
                            Some(pos) => inner.remove_at(py, pos, &mut displaced),
                            None => {
                                inner.table.push(py, key)?;
                            }
                        }
                        Ok(())
                    },
                )?;
                drop(displaced);
                Ok(())
            }
        }
    }

}

/// `other <op> right` with a builtin set on the left: a rustique set of the left operand's
/// kind (`frozenset` → `FrozenSet`, `set` → `Set`), untyped.
pub(crate) fn reflected(
    py: Python<'_>,
    op: Algebra,
    other: &Bound<'_, PyAny>,
    right: &Operand<'_>,
) -> PyResult<Py<PyAny>> {
    if !is_set_operand(other) {
        return Ok(py.NotImplemented());
    }
    let left = Operand::Local { table: local_table(other)?, is_set: true };
    let table = combine(py, op, &left, right, KeyKind::Adaptive)?;
    if other.is_instance_of::<PyFrozenSet>() {
        Ok(Bound::new(py, FrozenSet::from_table(schema::any(), table))?.into_any().unbind())
    } else {
        Ok(Bound::new(py, Set::with_table(schema::any(), table))?.into_any().unbind())
    }
}

/// No keyword arguments (`set()` takes none).
pub(crate) fn no_keywords(name: &str, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<()> {
    match kwargs {
        Some(kw) if !kw.is_empty() => Err(PyTypeError::new_err(format!("{name}() takes no keyword arguments"))),
        _ => Ok(()),
    }
}

/// The optional single positional argument of a constructor.
pub(crate) fn at_most_one<'py>(name: &str, args: &Bound<'py, PyTuple>) -> PyResult<Option<Bound<'py, PyAny>>> {
    match args.len() {
        0 => Ok(None),
        1 => Ok(Some(args.get_item(0)?)),
        n => Err(PyTypeError::new_err(format!("{name} expected at most 1 argument, got {n}"))),
    }
}

#[pymethods]
impl Set {
    /// `Set(iterable=(), /)`. A subclass instance starts empty and is filled by `__init__`
    /// (CPython's `set_new`/`set_init` split); `Set` itself is filled here, so that
    /// construction does not pay for a second pass.
    #[new]
    #[classmethod]
    #[pyo3(signature = (*args, **kwargs))]
    fn __new__(cls: &Bound<'_, PyType>, args: &Bound<'_, PyTuple>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
        let py = cls.py();
        if !cls.is(py.get_type::<Set>()) {
            return Ok(Set::empty(schema::any()));
        }
        no_keywords("Set", kwargs)?;
        Set::build(py, schema::any(), at_most_one("Set", args)?.as_ref())
    }

    /// `set.__init__`: clear, then add the elements of `iterable` (installed as `__init__`
    /// by [`register`], without a `tp_init` slot: see there).
    #[pyo3(signature = (*args, **kwargs))]
    fn _rustique_init(slf: &Bound<'_, Self>, args: &Bound<'_, PyTuple>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<()> {
        let py = slf.py();
        let name = slf.get_type().name()?.to_string();
        no_keywords(&name, kwargs)?;
        let iterable = at_most_one(&name, args)?;
        let this = slf.get();
        let displaced = this.inner.write(py).table.clear();
        drop(displaced);
        match iterable {
            Some(it) => this.add_all(py, &it),
            None => Ok(()),
        }
    }

    #[classmethod]
    fn __class_getitem__<'py>(cls: &Bound<'py, PyType>, item: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        crate::typing::set_alias(cls.py(), item)
    }

    #[classattr]
    const __hash__: Option<Py<PyAny>> = None;

    fn __len__(&self, py: Python<'_>) -> usize {
        self.len(py)
    }

    fn __contains__(&self, elem: &Bound<'_, PyAny>) -> PyResult<bool> {
        self.contains(elem)
    }

    fn __iter__(slf: &Bound<'_, Self>) -> SetIterator {
        let len = slf.get().len(slf.py());
        let state = SetIterState { set: Some(slf.clone().unbind()), pos: 0, expected: len, remaining: len };
        SetIterator { state: Locked::new(state) }
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let this = slf.get();
        let name = display_name(slf.as_any(), &py.get_type::<Set>(), "Set", &this.schema)?;
        set_repr(slf.as_any(), &name, || this.element_objects(py))
    }

    fn __richcmp__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<Py<PyAny>> {
        richcompare(slf.py(), &Operand::Set(slf.clone()), other, op, None)
    }

    fn add(&self, py: Python<'_>, elem: &Bound<'_, PyAny>) -> PyResult<()> {
        let key = self.prepare(elem.clone())?;
        keys::upsert_one(py, &self.inner, key, (), |inner, (), slot| {
            if let Slot::Absent(key) = slot {
                inner.table.push(py, key)?;
            }
            Ok(())
        })
    }

    fn discard(&self, py: Python<'_>, elem: &Bound<'_, PyAny>) -> PyResult<()> {
        self.discard_elem(py, elem).map(drop)
    }

    fn remove(&self, py: Python<'_>, elem: &Bound<'_, PyAny>) -> PyResult<()> {
        if self.discard_elem(py, elem)? { Ok(()) } else { Err(PyKeyError::new_err((elem.clone().unbind(),))) }
    }

    /// Remove and return an element (the most recently inserted one).
    fn pop(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let (elem, displaced) = {
            let mut inner = self.inner.write(py);
            let Some(pos) = inner.table.last_live() else {
                return Err(PyKeyError::new_err("pop from an empty set"));
            };
            let elem = inner.table.key_object(py, pos);
            (elem, inner.table.truncate(pos))
        };
        drop(displaced);
        Ok(elem)
    }

    fn clear(&self, py: Python<'_>) {
        let displaced = self.inner.write(py).table.clear();
        drop(displaced);
    }

    fn copy(&self, py: Python<'_>) -> Set {
        Set::with_table(self.schema.clone(), self.snapshot_table(py))
    }

    fn __copy__(&self, py: Python<'_>) -> Set {
        self.copy(py)
    }

    #[pyo3(signature = (*others))]
    fn union(slf: &Bound<'_, Self>, others: Vec<Bound<'_, PyAny>>) -> PyResult<Set> {
        Set::compute(slf, Algebra::Union, &others)
    }

    #[pyo3(signature = (*others))]
    fn intersection(slf: &Bound<'_, Self>, others: Vec<Bound<'_, PyAny>>) -> PyResult<Set> {
        Set::compute(slf, Algebra::Intersection, &others)
    }

    #[pyo3(signature = (*others))]
    fn difference(slf: &Bound<'_, Self>, others: Vec<Bound<'_, PyAny>>) -> PyResult<Set> {
        Set::compute(slf, Algebra::Difference, &others)
    }

    fn symmetric_difference(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Set> {
        Set::compute(slf, Algebra::SymmetricDifference, std::slice::from_ref(other))
    }

    #[pyo3(signature = (*others))]
    fn update(&self, py: Python<'_>, others: Vec<Bound<'_, PyAny>>) -> PyResult<()> {
        self.in_place(py, Algebra::Union, &others)
    }

    #[pyo3(signature = (*others))]
    fn intersection_update(&self, py: Python<'_>, others: Vec<Bound<'_, PyAny>>) -> PyResult<()> {
        self.in_place(py, Algebra::Intersection, &others)
    }

    #[pyo3(signature = (*others))]
    fn difference_update(&self, py: Python<'_>, others: Vec<Bound<'_, PyAny>>) -> PyResult<()> {
        self.in_place(py, Algebra::Difference, &others)
    }

    fn symmetric_difference_update(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<()> {
        self.in_place(py, Algebra::SymmetricDifference, std::slice::from_ref(other))
    }

    fn issubset(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        is_subset(slf.py(), &Operand::Set(slf.clone()), &Operand::new(other)?)
    }

    fn issuperset(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        is_superset(slf.py(), &Operand::Set(slf.clone()), &Operand::new(other)?)
    }

    fn isdisjoint(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        is_disjoint(slf.py(), &Operand::Set(slf.clone()), &Operand::new(other)?)
    }

    fn __or__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        Set::binary(slf, Algebra::Union, other)
    }

    fn __ror__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        reflected(slf.py(), Algebra::Union, other, &Operand::Set(slf.clone()))
    }

    fn __and__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        Set::binary(slf, Algebra::Intersection, other)
    }

    fn __rand__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        reflected(slf.py(), Algebra::Intersection, other, &Operand::Set(slf.clone()))
    }

    fn __sub__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        Set::binary(slf, Algebra::Difference, other)
    }

    fn __rsub__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        reflected(slf.py(), Algebra::Difference, other, &Operand::Set(slf.clone()))
    }

    fn __xor__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        Set::binary(slf, Algebra::SymmetricDifference, other)
    }

    fn __rxor__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        reflected(slf.py(), Algebra::SymmetricDifference, other, &Operand::Set(slf.clone()))
    }

    fn __ior__(&self, py: Python<'_>, other: SetLike<'_>) -> PyResult<()> {
        self.in_place(py, Algebra::Union, std::slice::from_ref(&other.0))
    }

    fn __iand__(&self, py: Python<'_>, other: SetLike<'_>) -> PyResult<()> {
        self.in_place(py, Algebra::Intersection, std::slice::from_ref(&other.0))
    }

    fn __isub__(&self, py: Python<'_>, other: SetLike<'_>) -> PyResult<()> {
        self.in_place(py, Algebra::Difference, std::slice::from_ref(&other.0))
    }

    fn __ixor__(&self, py: Python<'_>, other: SetLike<'_>) -> PyResult<()> {
        self.in_place(py, Algebra::SymmetricDifference, std::slice::from_ref(&other.0))
    }

    /// A builtin `set` with the same elements.
    fn to_set<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PySet>> {
        PySet::new(py, self.element_objects(py))
    }

    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyTuple>> {
        let py = slf.py();
        let this = slf.get();
        reduce(slf.as_any(), &py.get_type::<Set>(), this.schema_type_of(py)?, this.element_objects(py))
    }

    /// The alias describing this set's type (`Set[int]`, or `Set` when untyped).
    #[getter]
    fn schema_type<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.schema_type_of(py)
    }

    /// Element strategy (`"empty"`, `"i64"`, `"str"` or `"object"`), for introspection.
    #[getter]
    fn _strategy(&self, py: Python<'_>) -> &'static str {
        self.inner.read(py).table.strategy()
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        if let Some(inner) = self.inner.try_read() {
            for k in inner.table.object_keys() {
                visit.call(k)?;
            }
        }
        Ok(())
    }

    fn __clear__(&self) {
        // Runs inside the GC: never block (a contended set is simply not cleared now).
        let displaced = self.inner.try_write().map(|mut inner| inner.table.clear());
        drop(displaced);
    }
}

/// An in-place operator operand: any set type (anything else → `NotImplemented`).
pub struct SetLike<'py>(Bound<'py, PyAny>);

impl<'a, 'py> FromPyObject<'a, 'py> for SetLike<'py> {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
        if is_set_operand(&obj) { Ok(SetLike(obj.to_owned())) } else { Err(PyTypeError::new_err("expected a set")) }
    }
}

/// CPython's `setiterobject` state: only a size change is an error. Positions are insertion
/// positions, so an element discarded and re-added goes to the end; yielding at most the
/// size at creation keeps such loops from seeing it again (CPython's table reuses the slot).
struct SetIterState {
    /// The set, released once the iterator is exhausted (like CPython's).
    set: Option<Py<Set>>,
    pos: usize,
    /// The size at creation; `usize::MAX` once a size change was reported (sticky).
    expected: usize,
    /// Elements left to yield.
    remaining: usize,
}

/// Live iterator over a `Set`.
#[pyclass(module = "rustique", frozen)]
pub struct SetIterator {
    state: Locked<SetIterState>,
}

#[pymethods]
impl SetIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let mut st = self.state.write(py);
        let SetIterState { set, pos, expected, remaining } = &mut *st;
        let Some(owner) = set.as_ref() else {
            return Ok(None);
        };
        let inner = owner.get().inner.read(py);
        if inner.table.len() != *expected {
            *expected = usize::MAX;
            return Err(PyRuntimeError::new_err("Set changed size during iteration"));
        }
        if *remaining > 0
            && let Some(p) = inner.table.next_live(*pos)
        {
            *pos = p + 1;
            *remaining -= 1;
            return Ok(Some(inner.table.key_object(py, p)));
        }
        drop(inner);
        let released = set.take();
        drop(st);
        drop(released);
        Ok(None)
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        if let Some(st) = self.state.try_read()
            && let Some(set) = &st.set
        {
            visit.call(set)?;
        }
        Ok(())
    }

    fn __length_hint__(&self, py: Python<'_>) -> usize {
        let st = self.state.read(py);
        match &st.set {
            Some(set) if set.get().len(py) == st.expected => st.remaining,
            _ => 0,
        }
    }

    /// `(iter, (remaining elements,))`, like CPython's (set iterators unpickle as list
    /// iterators).
    fn __reduce__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let remaining = {
            let st = self.state.read(py);
            match &st.set {
                Some(set) => {
                    let inner = set.get().inner.read(py);
                    if inner.table.len() != st.expected {
                        return Err(PyRuntimeError::new_err("Set changed size during iteration"));
                    }
                    let mut out = Vec::with_capacity(st.remaining);
                    let mut pos = st.pos;
                    while out.len() < st.remaining
                        && let Some(p) = inner.table.next_live(pos)
                    {
                        out.push(inner.table.key_object(py, p));
                        pos = p + 1;
                    }
                    out
                }
                None => Vec::new(),
            }
        };
        let iter = py.import("builtins")?.getattr("iter")?;
        PyTuple::new(py, [iter, PyTuple::new(py, [PyList::new(py, remaining)?])?.into_any()])
    }
}

/// The body of a type's vectorcall constructor (`T(iterable=(), /)`): rejects keywords and
/// extra arguments like the builtins, builds the object with `make`, and turns errors and
/// panics into Python exceptions.
///
/// # Safety
/// Called by CPython as a `vectorcallfunc`: `args` holds `nargs` valid references, `kwnames`
/// is NULL or a tuple of keyword names, and the thread is attached.
pub(crate) unsafe fn vectorcall_ctor(
    name: &str,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
    make: impl for<'py> FnOnce(Python<'py>, Option<&Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyAny>>,
) -> *mut ffi::PyObject {
    // SAFETY: CPython calls vectorcall functions attached to the interpreter.
    let py = unsafe { Python::assume_attached() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: see the function's contract.
        let nargs = unsafe { ffi::PyVectorcall_NARGS(nargsf) };
        // SAFETY: `kwnames` is NULL or a tuple.
        if !kwnames.is_null() && unsafe { ffi::PyTuple_GET_SIZE(kwnames) } > 0 {
            return Err(PyTypeError::new_err(format!("{name}() takes no keyword arguments")));
        }
        match nargs {
            0 => make(py, None),
            1 => {
                // SAFETY: `args[0]` is a valid reference for the duration of the call.
                let arg = unsafe { Borrowed::from_ptr(py, *args) }.to_owned();
                make(py, Some(&arg))
            }
            n => Err(PyTypeError::new_err(format!("{name} expected at most 1 argument, got {n}"))),
        }
    }));
    let err = match result {
        Ok(Ok(obj)) => return obj.into_ptr(),
        Ok(Err(e)) => e,
        Err(_) => pyo3::panic::PanicException::new_err(format!("rustique: panic in {name}()")),
    };
    err.restore(py);
    std::ptr::null_mut()
}

/// Vectorcall of the `Set` type object (CPython's `set_vectorcall`): `Set(it)` without
/// `type_call`, an argument tuple or a `tp_init` call. CPython never inherits
/// `tp_vectorcall`, so subclasses keep `tp_new` + `__init__`.
unsafe extern "C" fn set_vectorcall(
    _callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: forwarded from CPython's vectorcall.
    unsafe {
        vectorcall_ctor("Set", args, nargsf, kwnames, |py, arg| {
            Ok(Bound::new(py, Set::build(py, schema::any(), arg)?)?.into_any())
        })
    }
}

/// Give a type object a vectorcall constructor (the interpreter then specialises calls of
/// the type to `CALL_BUILTIN_CLASS`).
pub(crate) fn install_vectorcall(ty: &Bound<'_, PyType>, f: ffi::vectorcallfunc) {
    // SAFETY: `ty` is a fully initialised heap type created at module initialisation, before
    // any call of it; `tp_vectorcall` of a type object is read by `PyVectorcall_Function` of
    // its metatype (`type`), which supports vectorcall. `PyType_Modified` drops caches.
    unsafe {
        (*ty.as_type_ptr()).tp_vectorcall = Some(f);
        ffi::PyType_Modified(ty.as_type_ptr());
    }
}

/// Install `Set._rustique_init` as `Set.__init__` directly in the type's dict. Assigning it
/// through `setattr` (or naming the method `__init__`) would fill the `tp_init` slot, and
/// `Set(it)` would then fill the set a second time after `__new__`; this way `Set` keeps
/// `object.__init__` as its slot while `s.__init__(it)` re-initialises (CPython's
/// `set_init`), and a subclass — whose `tp_init` `type_new` derives from the `__init__` it
/// finds in this dict — is filled by `__init__` after `__new__` created it empty.
pub(crate) fn install_init(ty: &Bound<'_, PyType>, method: &str) -> PyResult<()> {
    let py = ty.py();
    // SAFETY: `ty` is a valid type object; `PyType_GetDict` returns a new reference to its dict.
    let dict = unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyType_GetDict(ty.as_type_ptr()))? };
    let dict = dict.cast_into::<PyDict>()?;
    let init = dict.get_item(method)?.ok_or_else(|| PyRuntimeError::new_err("rustique: init method missing"))?;
    dict.set_item("__init__", init)?;
    dict.del_item(method)?;
    // SAFETY: the type's dict was modified directly; invalidate its attribute caches.
    unsafe { ffi::PyType_Modified(ty.as_type_ptr()) };
    Ok(())
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Set>()?;
    m.add_class::<SetIterator>()?;
    let ty = m.py().get_type::<Set>();
    install_vectorcall(&ty, set_vectorcall);
    install_init(&ty, "_rustique_init")
}
