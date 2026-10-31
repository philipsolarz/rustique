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

use std::ffi::{CStr, c_int};
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

/// `elem in table` for the common cases, without classifying `elem` or creating Python
/// references: native probes of native tables, and identity hits and clean misses (no
/// stored key with an equal hash) in object tables. Otherwise `Err` with `hash(elem)` when
/// computed (`None`: unhashable, the error was cleared; the full lookup raises it again).
/// `read` runs its argument on the table (under the set's read lock for a `Set`).
#[inline]
pub(crate) fn quick_contains(
    elem: &Bound<'_, PyAny>,
    read: impl Fn(&dyn Fn(&KeyTable) -> Option<bool>) -> Option<bool>,
) -> Result<bool, Option<isize>> {
    if let Some(probe) = keys::quick(elem)
        && let Some(found) = read(&|t: &KeyTable| t.find_quick(&probe).map(|pos| pos.is_some()))
    {
        return Ok(found);
    }
    // SAFETY: `elem` is a valid object; on failure the error is cleared and the full lookup
    // computes the hash again (and raises).
    let hash = unsafe { ffi::PyObject_Hash(elem.as_ptr()) };
    if hash == -1 {
        // SAFETY: attached; clears the error `PyObject_Hash` set.
        unsafe { ffi::PyErr_Clear() };
        return Err(None);
    }
    let found = read(&|t: &KeyTable| match t.store() {
        KeyStore::Object(ki) => find_object_inline(elem.py(), ki, elem, hash).map(|pos| pos.is_some()),
        KeyStore::Empty => Some(false),
        _ => None,
    });
    found.ok_or(Some(hash))
}

/// Find `elem` (Python hash `hash`) in an object table without running Python code: by
/// identity, or by `==` between builtin scalars (and tuples of them) like a lookup does
/// under its lock. `None` when some candidate needs a Python comparison (full lookup).
fn find_object_inline(py: Python<'_>, ki: &ObjectIndex, elem: &Bound<'_, PyAny>, hash: isize) -> Option<Option<usize>> {
    let mut mine = None;
    for pos in ki.index().candidates(keys::mix(hash)) {
        let stored = ki.raw_keys()[pos].bind(py);
        if stored.is(elem) {
            return Some(Some(pos));
        }
        let mine = *mine.get_or_insert_with(|| keys::simplicity(elem, 0));
        match (keys::simplicity(stored, 0), mine) {
            (Simple::Yes, Simple::Yes) | (Simple::Bytes, Simple::Bytes) => {
                // CPython's operand order: stored == probe.
                if stored.eq(elem).ok()? {
                    return Some(Some(pos));
                }
            }
            _ => return None,
        }
    }
    Some(None)
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
    // An exact rustique set first (the copy constructor is the common case; subclass
    // instances are found below, after the builtin sources).
    if (iterable.is_exact_instance_of::<Set>() || iterable.is_exact_instance_of::<FrozenSet>())
        && let Some(table) = copy_rustique_set(py, schema, iterable)
    {
        return Ok(table.with_kind(kind));
    }
    let mut table = KeyTable::new(kind);
    match ingest_native(py, &mut table, schema, iterable)? {
        Some(pending) => {
            for item in pending {
                insert_pending(py, &mut table, schema, item, name)?;
            }
        }
        None => {
            for item in iterable.try_iter()? {
                let item = item?;
                match keys::native_key(&item, schema) {
                    Some(key) => table.insert_prepared(py, key)?,
                    None => insert_pending(py, &mut table, schema, Pending::Object(item, None), name)?,
                }
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
        Pending::Object(obj, known) if schema.is_any() => match insert_object_fast(py, table, &obj, known)? {
            Ok(()) => return Ok(()),
            Err(Some(hash)) => Prepared::Key(Key::with_hash(obj, hash)?),
            Err(None) => keys::prepare_owned(obj, schema, Role::SetElement, name)?,
        },
        Pending::Object(obj, _) => keys::prepare_owned(obj, schema, Role::SetElement, name)?,
    };
    table.insert_prepared(py, key)
}

/// Insert `obj` into a private object table without classifying it, when it compares with
/// the stored keys it meets without Python code (builtin scalars, tuples of them, objects
/// compared by identity). Otherwise `Err` with its hash when computed (the general path
/// then reuses it, so `__hash__` runs once).
fn insert_object_fast(
    py: Python<'_>,
    table: &mut KeyTable,
    obj: &Bound<'_, PyAny>,
    known: Option<isize>,
) -> PyResult<Result<(), Option<isize>>> {
    if !matches!(table.store(), KeyStore::Object(_)) || keys::simplicity(obj, 0) != Simple::Yes {
        return Ok(Err(known));
    }
    let hash = match known {
        Some(h) => h,
        None => obj.hash()?,
    };
    let mixed = keys::mix(hash);
    let KeyStore::Object(ki) = table.store() else {
        return Ok(Err(Some(hash)));
    };
    match object_contains(py, ki, obj, mixed) {
        Some(true) => Ok(Ok(())),
        Some(false) => {
            table
                .push(py, &Key { obj: obj.clone(), class: Class::Other { hash, exotic: true, simple: Simple::No } })?;
            Ok(Ok(()))
        }
        None => Ok(Err(Some(hash))),
    }
}

/// Collects the elements a source critical section could not insert natively.
struct Ingest<'py> {
    pending: Vec<Pending<'py>>,
    err: Option<PyErr>,
    /// Elements the source holds (capacity for `pending` once something is queued).
    size: usize,
    seen: usize,
}

impl<'py> Ingest<'py> {
    fn new() -> Self {
        Ingest { pending: Vec::new(), err: None, size: 0, seen: 0 }
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
        self.seen += 1;
        let key = keys::native_key(&item, schema);
        if self.pending.is_empty() {
            match key {
                Some(key) => {
                    if self.err.is_none()
                        && let Err(e) = table.insert_prepared(py, key)
                    {
                        self.err = Some(e);
                    }
                    return;
                }
                None => self.pending.reserve(self.size.saturating_sub(self.seen) + 1),
            }
        }
        match key {
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
            ingest.size = n as usize;
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
        ingest.size = tuple.len();
        for item in tuple.iter_borrowed() {
            ingest.push(py, table, schema, item, None);
        }
        return ingest.finish();
    }
    if is_builtin_set(obj) {
        // SAFETY: `obj` is a set or frozenset.
        ingest.size = unsafe { ffi::PySet_Size(obj.as_ptr()) }.max(0) as usize;
        table.reserve(ingest.size);
        for_each_set_entry(obj, |item, hash| {
            ingest.push(py, table, schema, item, Some(hash));
            true
        });
        return ingest.finish();
    }
    if let Ok(dict) = obj.cast_exact::<PyDict>() {
        with_critical_section(dict.as_any(), || {
            table.reserve(dict.len());
            ingest.size = dict.len();
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
            // Presize moderately (a huge range grows the table step by step, like any
            // other source, instead of reserving everything up front).
            table.reserve(n.min(1 << 20));
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
fn native_op<K: Hash + Eq + Clone + Default>(
    op: Algebra,
    a: &KeyIndex<K>,
    b: &KeyIndex<K>,
    b_set: bool,
) -> KeyIndex<K> {
    match op {
        Algebra::Union => {
            // Small results: one allocation sized for both operands (cloning `a` and growing
            // it reallocates every part); large ones: a bulk copy of `a`, then growth.
            let mut out = if a.len() + b.len() <= 64 {
                let mut out = KeyIndex::with_capacity_and_hasher(a.len() + b.len(), a.hasher().clone());
                for (pos, k) in a.iter() {
                    out.push_hashed(stored_hash(a, pos, k), k.clone());
                }
                out
            } else {
                let mut out = a.clone();
                out.reserve(b.len());
                out
            };
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
            let mut out = KeyIndex::with_capacity_and_hasher(a.len() + b.len(), a.hasher().clone());
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
            out.reserve(if op == Algebra::Union { b.len() } else { a.len() + b.len() });
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
fn table_op(
    py: Python<'_>,
    op: Algebra,
    a: &KeyTable,
    (b, b_set): (&KeyTable, bool),
    kind: KeyKind,
) -> Option<KeyTable> {
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

/// Raw positions `0..n` in a scrambled order: blocks of 8 consecutive positions (one cache
/// line of `i64` keys) visited with an odd stride over a power-of-two block count, each
/// position exactly once. Early-exit predicates (`isdisjoint`, `<=`) then sample the table the way
/// CPython's hash-ordered iteration does, instead of scanning it in insertion order (which
/// meets an overlapping tail only at the end: O(n) where `set` stops after a few probes).
fn scrambled(n: usize) -> impl Iterator<Item = usize> {
    const BLOCK: usize = 8;
    let blocks = n.div_ceil(BLOCK);
    // An odd stride over a power-of-two count of (virtual) blocks visits each exactly once,
    // with a mask instead of divisions; virtual blocks past the end are skipped.
    let slots = blocks.next_power_of_two();
    let stride = (slots / 8 * 5) | 1;
    (0..slots)
        .filter_map(move |i| {
            let block = i.wrapping_mul(stride) & (slots - 1);
            (block < blocks).then(|| block * BLOCK..((block + 1) * BLOCK).min(n))
        })
        .flatten()
}

/// Tables up to this many positions are scanned in order (too small for the order to
/// matter more than the iteration overhead).
const SCRAMBLE_ABOVE: usize = 64;

/// Whether some live key of `x` (visited in [`scrambled`] order) satisfies `f`.
fn any_scrambled<K: Hash + Eq>(x: &KeyIndex<K>, mut f: impl FnMut(&K) -> bool) -> bool {
    if x.entries() <= SCRAMBLE_ABOVE {
        return x.iter().any(|(_, k)| f(k));
    }
    scrambled(x.entries()).any(|pos| x.get_at(pos).is_some_and(&mut f))
}

/// The first `Some` of `f` over the live entries of an object table, in [`scrambled`] order.
fn find_scrambled_object<R>(
    py: Python<'_>,
    x: &ObjectIndex,
    mut f: impl FnMut(&Bound<'_, PyAny>, u64) -> Option<R>,
) -> Option<R> {
    if x.entries() <= SCRAMBLE_ABOVE {
        return object_entries(py, x).find_map(|(_, obj, h)| f(obj, h));
    }
    let index = x.index();
    scrambled(x.entries()).find_map(|pos| index.hash_at(pos).and_then(|h| f(x.raw_keys()[pos].bind(py), h)))
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
        (KeyStore::Int(x), KeyStore::Int(y)) => Some(!any_scrambled(x, |k| !y.contains(k))),
        (KeyStore::Str(x), KeyStore::Str(y)) => Some(!any_scrambled(x, |k| !y.contains(k))),
        (KeyStore::Int(_), KeyStore::Str(_)) | (KeyStore::Str(_), KeyStore::Int(_)) => Some(false),
        // `Some(Some(()))`: a missing element; `Some(None)`: needs Python equality.
        (KeyStore::Object(x), KeyStore::Object(y)) => {
            match find_scrambled_object(py, x, |obj, h| match object_contains(py, y, obj, h) {
                Some(true) => None,
                Some(false) => Some(Some(())),
                None => Some(None),
            }) {
                None => Some(true),
                Some(Some(())) => Some(false),
                Some(None) => None,
            }
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
        (KeyStore::Int(x), KeyStore::Int(y)) => Some(!any_scrambled(x, |k| y.contains(k))),
        (KeyStore::Str(x), KeyStore::Str(y)) => Some(!any_scrambled(x, |k| y.contains(k))),
        (KeyStore::Int(_), KeyStore::Str(_)) | (KeyStore::Str(_), KeyStore::Int(_)) => Some(true),
        // `Some(Some(()))`: a shared element; `Some(None)`: needs Python equality.
        (KeyStore::Object(x), KeyStore::Object(y)) => {
            match find_scrambled_object(py, x, |obj, h| match object_contains(py, y, obj, h) {
                Some(false) => None,
                Some(true) => Some(Some(())),
                None => Some(None),
            }) {
                None => Some(true),
                Some(Some(())) => Some(false),
                Some(None) => None,
            }
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
                None => table_algebra(py, op, *table, other, kind)?,
            },
        };
    }
    Ok(acc)
}

/// `acc <op> other` in `acc`'s own storage, for a union or difference of native keys of one
/// kind; `acc` back otherwise.
fn native_into(op: Algebra, acc: KeyTable, other: &KeyTable, kind: KeyKind) -> Result<KeyTable, Box<KeyTable>> {
    if !matches!(op, Algebra::Union | Algebra::Difference) {
        return Err(Box::new(acc));
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
        (Ok(native), _) => Err(Box::new(KeyTable::from_native(native, kind))),
        (Err(table), _) => Err(table),
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
fn table_algebra(
    py: Python<'_>,
    op: Algebra,
    mine: KeyTable,
    other: &Operand<'_>,
    kind: KeyKind,
) -> PyResult<KeyTable> {
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
    let boolean = |b: bool| pyo3::types::PyBool::new(py, b).to_owned().into_any().unbind();
    // Two rustique sets: sizes and native contents decided under one pair of read locks.
    let rustique = match other.cast::<Set>() {
        Ok(s) => Some(Operand::Set(s.clone())),
        Err(_) => other.cast::<FrozenSet>().ok().map(|f| Operand::Frozen(f.clone())),
    };
    if let Some(theirs) = &rustique {
        let differ_by_hash = match (theirs, my_hash) {
            (Operand::Frozen(f), Some(h)) => f.get().cached_hash().is_some_and(|t| t != h),
            _ => false,
        };
        if me.same_object(theirs) {
            return Ok(boolean(matches!(op, CompareOp::Eq | CompareOp::Le | CompareOp::Ge)));
        }
        let decided = with_tables(py, me, theirs, |a, b| {
            let (mine, n) = (a.len(), b.len());
            match op {
                CompareOp::Eq | CompareOp::Ne if mine != n || differ_by_hash => Some(matches!(op, CompareOp::Ne)),
                CompareOp::Lt if mine >= n => Some(false),
                CompareOp::Gt if mine <= n => Some(false),
                CompareOp::Eq => table_subset(py, a, b),
                CompareOp::Ne => table_subset(py, a, b).map(|sub| !sub),
                CompareOp::Le | CompareOp::Lt => table_subset(py, a, b),
                CompareOp::Ge | CompareOp::Gt => table_subset(py, b, a),
            }
        });
        if let Some(result) = decided {
            return Ok(boolean(result));
        }
    }
    let mine = me.len(py);
    // Decided by the sizes alone (a builtin set is then never read).
    if is_builtin_set(other) {
        // SAFETY: `other` is a set or frozenset.
        let theirs = unsafe { ffi::PySet_Size(other.as_ptr()) }.max(0) as usize;
        let by_size = match op {
            CompareOp::Eq | CompareOp::Ne if mine != theirs => Some(matches!(op, CompareOp::Ne)),
            CompareOp::Le if mine > theirs => Some(false),
            CompareOp::Lt if mine >= theirs => Some(false),
            CompareOp::Ge if mine < theirs => Some(false),
            CompareOp::Gt if mine <= theirs => Some(false),
            CompareOp::Le | CompareOp::Lt if is_large_builtin(mine, other) => {
                Some(probe_builtin(py, me, other, |_, found| (!found).then_some(()))?.is_none())
            }
            _ => None,
        };
        if let Some(result) = by_size {
            return Ok(pyo3::types::PyBool::new(py, result).to_owned().into_any().unbind());
        }
    }
    let operand = match rustique {
        Some(operand) => operand,
        None => Operand::new(other)?,
    };
    let theirs = operand.len(py);
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

/// A builtin set operand this many times larger than the rustique set is probed element by
/// element (CPython iterates the smaller set) instead of being read whole.
const PROBE_RATIO: usize = 4;

/// Whether `other` is a builtin set large enough, next to a rustique set of `mine`
/// elements, to be probed rather than read.
fn is_large_builtin(mine: usize, other: &Bound<'_, PyAny>) -> bool {
    // SAFETY: a type check, then the size of a set or frozenset.
    is_builtin_set(other)
        && unsafe { ffi::PySet_Size(other.as_ptr()) }.max(0) as usize > PROBE_RATIO.saturating_mul(mine)
}

/// Look up each element of `me` in the builtin set `big` with `PySet_Contains` (so `big`
/// compares exactly as CPython would; no lock is held), until `visit` returns `Some`.
fn probe_builtin<'py, R>(
    py: Python<'py>,
    me: &Operand<'py>,
    big: &Bound<'py, PyAny>,
    mut visit: impl FnMut(&Key<'py>, bool) -> Option<R>,
) -> PyResult<Option<R>> {
    for key in &me.keys(py)? {
        // SAFETY: `big` is a set or frozenset and `key.obj` a live object.
        let found = match unsafe { ffi::PySet_Contains(big.as_ptr(), key.obj.as_ptr()) } {
            -1 => return Err(PyErr::fetch(py)),
            n => n == 1,
        };
        if let Some(r) = visit(key, found) {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

/// `me & big` or `me - big` for a builtin set `big` much larger than `me`: `me`'s elements
/// that `big` does (or does not) contain; `None` when `other` is not such a set.
pub(crate) fn small_op_large_builtin(
    py: Python<'_>,
    op: Algebra,
    me: &Operand<'_>,
    other: &Bound<'_, PyAny>,
    kind: KeyKind,
) -> PyResult<Option<KeyTable>> {
    if !matches!(op, Algebra::Intersection | Algebra::Difference) || !is_large_builtin(me.len(py), other) {
        return Ok(None);
    }
    let keep = op == Algebra::Intersection;
    let mut out = KeyTable::new(kind);
    let failed = probe_builtin(py, me, other, |key, found| (found == keep).then(|| out.push(py, key).err()).flatten())?;
    match failed {
        Some(e) => Err(e),
        None => Ok(Some(out)),
    }
}

/// `me.isdisjoint(other)` / `me.issubset(other)` by probing a large builtin set (`None`: not
/// such a set).
pub(crate) fn predicate_large_builtin(
    py: Python<'_>,
    me: &Operand<'_>,
    other: &Bound<'_, PyAny>,
    subset: bool,
) -> PyResult<Option<bool>> {
    if !is_large_builtin(me.len(py), other) {
        return Ok(None);
    }
    // Subset: stop at a missing element; disjoint: stop at a shared one.
    let stopped = probe_builtin(py, me, other, |_, found| (found != subset).then_some(()))?;
    Ok(Some(stopped.is_none()))
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
pub(crate) fn display_name(
    slf: &Bound<'_, PyAny>,
    base: &Bound<'_, PyType>,
    base_name: &str,
    schema: &Schema,
) -> PyResult<String> {
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

/// INTERFACE (builtins.rs). A borrowed view of a set's live elements, in insertion order:
/// native `int`/`str` keys without boxing, or the element objects.
#[derive(Clone, Copy)]
pub enum SetElements<'a> {
    Empty,
    Int(&'a KeyIndex<i64>),
    Str(&'a KeyIndex<compact_str::CompactString>),
    /// Element objects (iterate the live ones with `.iter()`).
    Object(&'a KeyIndex<Py<PyAny>, ()>),
}

impl<'a> SetElements<'a> {
    pub(crate) fn of(table: &'a KeyTable) -> SetElements<'a> {
        match table.store() {
            KeyStore::Empty => SetElements::Empty,
            KeyStore::Int(ki) => SetElements::Int(ki),
            KeyStore::Str(ki) => SetElements::Str(ki),
            KeyStore::Object(ki) => SetElements::Object(ki),
        }
    }

    /// Number of live elements.
    pub fn len(&self) -> usize {
        match self {
            SetElements::Empty => 0,
            SetElements::Int(ki) => ki.len(),
            SetElements::Str(ki) => ki.len(),
            SetElements::Object(ki) => ki.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
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

    /// INTERFACE. Run `f` on a borrowed view of the elements, under the set's read lock: `f`
    /// must not run Python code (copy what it needs, e.g. `clone_ref` the objects, and work
    /// on the copy after returning).
    pub fn with_elements<R>(&self, py: Python<'_>, f: impl FnOnce(SetElements<'_>) -> R) -> R {
        f(SetElements::of(&self.inner.read(py).table))
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
        match self.contains_fast(elem) {
            Ok(found) => Ok(found),
            Err(hash) => self.contains_slow(elem, hash),
        }
    }

    /// The common cases of `elem in self` without classifying `elem` or creating Python
    /// references (see [`quick_contains`]); `Err` carries `hash(elem)` when it was computed.
    fn contains_fast(&self, elem: &Bound<'_, PyAny>) -> Result<bool, Option<isize>> {
        quick_contains(elem, |f| f(&self.inner.read(elem.py()).table))
    }

    fn contains_slow(&self, elem: &Bound<'_, PyAny>, hash: Option<isize>) -> PyResult<bool> {
        let py = elem.py();
        match hash {
            Some(hash) => {
                let key = Key::with_hash(elem.clone(), hash)?;
                keys::with_entry(py, &self.inner, &key, |_, pos| pos.is_some())
            }
            // Unhashable: a set probe or an error (raised again here).
            None => keys::lookup(py, &self.inner, elem, probe, |_, pos| pos.is_some()),
        }
    }

    fn contains_key(&self, key: &Key<'_>) -> PyResult<bool> {
        keys::with_entry(key.obj.py(), &self.inner, key, |_, pos| pos.is_some())
    }

    /// The native keys of a rustique set source, when this set stores them as they are
    /// (untyped, or typed with the same native kind).
    fn native_source(&self, py: Python<'_>, source: &Bound<'_, PyAny>) -> Option<Natives> {
        let (ints, strs) = match *self.schema {
            Schema::Any => (true, true),
            Schema::Int => (true, false),
            Schema::Str => (false, true),
            _ => return None,
        };
        let read = |table: &KeyTable| match table.store() {
            KeyStore::Int(ki) if ints => Some(Natives::Int(ki.iter().map(|(_, &v)| v).collect())),
            KeyStore::Str(ki) if strs => Some(Natives::Str(ki.iter().map(|(_, s)| s.clone()).collect())),
            KeyStore::Empty => Some(Natives::Int(Vec::new())),
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
            Some(natives) => {
                // Native keys into a table without arbitrary objects: one write lock, no
                // Python code (comparisons, if any, are between exact ints/strs).
                let mut inner = self.inner.write(py);
                if !matches!(inner.table.store(), KeyStore::Object(_)) {
                    inner.table.reserve(natives.len());
                    return natives.insert_into(py, &mut inner.table);
                }
                drop(inner);
                (natives.into_prepared(), None)
            }
            None => match self.add_objects(py, iterable)? {
                Some(rest) => (rest, None),
                None => self.prepare_all(iterable),
            },
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

    /// `self |= other` for an untyped set and a rustique set `other` storing objects: the
    /// elements are added with their stored hashes, comparing inline under the write lock
    /// while that needs no Python code. Returns the elements left for the general path
    /// (`None`: `other` is not such a source).
    fn add_objects<'py>(&self, py: Python<'py>, other: &Bound<'py, PyAny>) -> PyResult<Option<Vec<Prepared<'py>>>> {
        if !self.schema.is_any() {
            return Ok(None);
        }
        let read = |table: &KeyTable| match table.store() {
            KeyStore::Object(ki) => {
                Some(object_entries(py, ki).map(|(_, obj, h)| (obj.clone(), h)).collect::<Vec<_>>())
            }
            _ => None,
        };
        let objects = if let Ok(set) = other.cast::<Set>() {
            read(&set.get().inner.read(py).table)
        } else if let Ok(frozen) = other.cast::<FrozenSet>() {
            read(frozen.get().table())
        } else {
            None
        };
        let Some(objects) = objects else {
            return Ok(None);
        };
        let mut done = 0;
        {
            let mut inner = self.inner.write(py);
            inner.table.reserve(objects.len());
            for (obj, h) in &objects {
                let present = match inner.table.store() {
                    KeyStore::Object(ki) => object_contains(py, ki, obj, *h),
                    KeyStore::Empty => Some(false),
                    _ => None,
                };
                match present {
                    Some(true) => {}
                    Some(false) => {
                        inner.table.push(py, &hashed_key(obj, *h))?;
                    }
                    None => break,
                }
                done += 1;
            }
        }
        let rest = objects
            .into_iter()
            .skip(done)
            .map(|(obj, h)| Key::with_hash(obj, keys::unmix(h)).map(Prepared::Key))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(Some(rest))
    }

    /// Remove every element of `iterable` that is present.
    fn remove_all(&self, py: Python<'_>, iterable: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Some(natives) = self.native_source(py, iterable) {
            let mut displaced = Vec::new();
            let mut inner = self.inner.write(py);
            if !matches!(inner.table.store(), KeyStore::Object(_)) {
                natives.for_each_quick(|probe| {
                    if let Some(Some(pos)) = inner.table.find_quick(&probe) {
                        inner.remove_at(py, pos, &mut displaced);
                    }
                });
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
        let quick = keys::quick(elem);
        let hash = {
            let mut inner = self.inner.write(py);
            // Native probes of native tables: one probe.
            let found = quick.as_ref().and_then(|probe| inner.table.find_quick(probe));
            let hash = match (&found, &quick) {
                (Some(_), _) => None,
                (None, Some(keys::Quick::Int(v))) => Some(keys::py_hash_i64(*v)),
                // Hashing a `str` runs no Python code (and caches the hash in the object).
                (None, _) => {
                    // SAFETY: `elem` is a live object; a failing hash is cleared and raised
                    // again by the full lookup below.
                    let h = unsafe { ffi::PyObject_Hash(elem.as_ptr()) };
                    if h == -1 {
                        // SAFETY: attached; clears the error `PyObject_Hash` set.
                        unsafe { ffi::PyErr_Clear() };
                    }
                    (h != -1).then_some(h)
                }
            };
            // Object tables: decided inline when no Python comparison is needed.
            let decided = match (found, hash, inner.table.store()) {
                (Some(pos), _, _) => Some(pos),
                (None, Some(h), KeyStore::Object(ki)) => find_object_inline(py, ki, elem, h),
                (None, Some(_), KeyStore::Empty) => Some(None),
                _ => None,
            };
            if let Some(pos) = decided {
                if let Some(pos) = pos {
                    inner.remove_at(py, pos, &mut displaced);
                }
                drop(inner);
                drop(displaced);
                return Ok(pos.is_some());
            }
            hash
        };
        let found = match hash {
            Some(hash) => {
                let key = Key::with_hash(elem.clone(), hash)?;
                keys::with_entry_mut(py, &self.inner, &key, |inner, pos| {
                    pos.map(|p| inner.remove_at(py, p, &mut displaced)).is_some()
                })?
            }
            // Unhashable: a set probe or an error (raised again here).
            None => keys::lookup_mut(py, &self.inner, elem, probe, |inner, pos| {
                pos.map(|p| inner.remove_at(py, p, &mut displaced)).is_some()
            })?,
        };
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
        if let [other] = others
            && let Some(table) =
                small_op_large_builtin(py, op, &Operand::Set(slf.clone()), other, KeyKind::for_schema(&schema::any()))?
        {
            return Ok(Set::with_table(schema::any(), table));
        }
        // One operand (the operators) needs no vector.
        let one;
        let many;
        let operands: &[Operand<'_>] = match others {
            [other] => {
                one = [Operand::new(other)?];
                &one
            }
            _ => {
                many = others.iter().map(Operand::new).collect::<PyResult<Vec<_>>>()?;
                &many
            }
        };
        let schema = result_schema(&slf.get().schema, operands);
        let kind = KeyKind::for_schema(&schema);
        let table = fold(py, op, &Operand::Set(slf.clone()), operands, kind)?;
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

/// Native keys copied out of a rustique set.
enum Natives {
    Int(Vec<i64>),
    Str(Vec<compact_str::CompactString>),
}

impl Natives {
    fn len(&self) -> usize {
        match self {
            Natives::Int(v) => v.len(),
            Natives::Str(v) => v.len(),
        }
    }

    /// Insert into a table that holds no arbitrary objects (runs no Python code).
    fn insert_into(self, py: Python<'_>, table: &mut KeyTable) -> PyResult<()> {
        match self {
            Natives::Int(v) => v.into_iter().try_for_each(|k| table.insert_prepared(py, Prepared::Int(k))),
            Natives::Str(v) => v.into_iter().try_for_each(|k| table.insert_prepared(py, Prepared::Str(k))),
        }
    }

    fn into_prepared(self) -> Vec<Prepared<'static>> {
        match self {
            Natives::Int(v) => v.into_iter().map(Prepared::Int).collect(),
            Natives::Str(v) => v.into_iter().map(Prepared::Str).collect(),
        }
    }

    fn for_each_quick(&self, mut f: impl FnMut(keys::Quick<'_>)) {
        match self {
            Natives::Int(v) => v.iter().for_each(|&k| f(keys::Quick::Int(k))),
            Natives::Str(v) => v.iter().for_each(|k| f(keys::Quick::Str(k))),
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
    fn __new__(
        cls: &Bound<'_, PyType>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
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
    fn _rustique_init(
        slf: &Bound<'_, Self>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
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
    fn union(slf: &Bound<'_, Self>, others: &Bound<'_, PyTuple>) -> PyResult<Set> {
        Set::compute(slf, Algebra::Union, &others.iter().collect::<Vec<_>>())
    }

    #[pyo3(signature = (*others))]
    fn intersection(slf: &Bound<'_, Self>, others: &Bound<'_, PyTuple>) -> PyResult<Set> {
        Set::compute(slf, Algebra::Intersection, &others.iter().collect::<Vec<_>>())
    }

    #[pyo3(signature = (*others))]
    fn difference(slf: &Bound<'_, Self>, others: &Bound<'_, PyTuple>) -> PyResult<Set> {
        Set::compute(slf, Algebra::Difference, &others.iter().collect::<Vec<_>>())
    }

    fn symmetric_difference(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Set> {
        Set::compute(slf, Algebra::SymmetricDifference, std::slice::from_ref(other))
    }

    #[pyo3(signature = (*others))]
    fn update(&self, py: Python<'_>, others: &Bound<'_, PyTuple>) -> PyResult<()> {
        match others.len() {
            1 => self.in_place(py, Algebra::Union, std::slice::from_ref(&others.get_item(0)?)),
            _ => self.in_place(py, Algebra::Union, &others.iter().collect::<Vec<_>>()),
        }
    }

    #[pyo3(signature = (*others))]
    fn intersection_update(&self, py: Python<'_>, others: &Bound<'_, PyTuple>) -> PyResult<()> {
        match others.len() {
            1 => self.in_place(py, Algebra::Intersection, std::slice::from_ref(&others.get_item(0)?)),
            _ => self.in_place(py, Algebra::Intersection, &others.iter().collect::<Vec<_>>()),
        }
    }

    #[pyo3(signature = (*others))]
    fn difference_update(&self, py: Python<'_>, others: &Bound<'_, PyTuple>) -> PyResult<()> {
        match others.len() {
            1 => self.in_place(py, Algebra::Difference, std::slice::from_ref(&others.get_item(0)?)),
            _ => self.in_place(py, Algebra::Difference, &others.iter().collect::<Vec<_>>()),
        }
    }

    fn symmetric_difference_update(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<()> {
        self.in_place(py, Algebra::SymmetricDifference, std::slice::from_ref(other))
    }

    fn issubset(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let me = Operand::Set(slf.clone());
        if let Some(result) = predicate_large_builtin(slf.py(), &me, other, true)? {
            return Ok(result);
        }
        is_subset(slf.py(), &me, &Operand::new(other)?)
    }

    fn issuperset(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        is_superset(slf.py(), &Operand::Set(slf.clone()), &Operand::new(other)?)
    }

    fn isdisjoint(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let me = Operand::Set(slf.clone());
        if let Some(result) = predicate_large_builtin(slf.py(), &me, other, false)? {
            return Ok(result);
        }
        is_disjoint(slf.py(), &me, &Operand::new(other)?)
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
        self.next(py)
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

/// Raw `tp_iternext` of `SetIterator` (`for x in s` without pyo3's slot trampoline).
unsafe extern "C" fn set_iter_next(slf: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: CPython calls the slot attached, with a live object of the slot's type
    // (`SetIterator` is final).
    unsafe {
        raw_slot(std::ptr::null_mut(), |py| {
            let it = Borrowed::from_ptr(py, slf).cast_unchecked::<SetIterator>();
            Ok(it.get().next(py)?.map_or(std::ptr::null_mut(), Py::into_ptr))
        })
    }
}

/// Run the body of a raw type slot: errors are raised, panics become `PanicException`s.
/// Bodies must release Python references through `Bound` (pyo3's GIL count is not
/// maintained here, so dropping a `Py` would defer its decref to pyo3's pool).
///
/// # Safety
/// The caller must be attached to the interpreter (CPython calls slots attached).
pub(crate) unsafe fn raw_slot<R>(failed: R, body: impl FnOnce(Python<'_>) -> PyResult<R>) -> R {
    // SAFETY: guaranteed by the caller.
    let py = unsafe { Python::assume_attached() };
    let err = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(py))) {
        Ok(Ok(r)) => return r,
        Ok(Err(e)) => e,
        Err(_) => pyo3::panic::PanicException::new_err("rustique: panic in a type slot"),
    };
    err.restore(py);
    failed
}

/// Raw `sq_contains` of `Set` (`x in s` without pyo3's slot trampoline for the common
/// cases; the full lookup runs attached through pyo3).
unsafe extern "C" fn set_contains_slot(slf: *mut ffi::PyObject, elem: *mut ffi::PyObject) -> c_int {
    // SAFETY: CPython calls the slot attached, with a live `Set` (or subclass instance, which
    // shares the layout) and a live element.
    unsafe {
        raw_slot(-1, |py| {
            let this = Borrowed::from_ptr(py, slf).cast_unchecked::<Set>();
            let elem = Borrowed::from_ptr(py, elem);
            let found = match this.get().contains_fast(&elem) {
                Ok(found) => found,
                Err(hash) => Python::attach(|_| this.get().contains_slow(&elem, hash))?,
            };
            Ok(c_int::from(found))
        })
    }
}

/// Raw `mp_length`/`sq_length` of `Set`.
unsafe extern "C" fn set_len_slot(slf: *mut ffi::PyObject) -> ffi::Py_ssize_t {
    // SAFETY: as for `set_contains_slot`.
    unsafe {
        raw_slot(-1, |py| {
            let this = Borrowed::from_ptr(py, slf).cast_unchecked::<Set>();
            Ok(this.get().len(py) as ffi::Py_ssize_t)
        })
    }
}

/// Install a `METH_O` method descriptor named `name` on `ty`, replacing pyo3's
/// (`METH_FASTCALL | METH_KEYWORDS`) one: the interpreter then specialises calls to
/// `CALL_METHOD_DESCRIPTOR_O` (`s.add(x)` like `set.add`), and no argument parsing runs.
/// `f` is a pyo3 slot trampoline (panics and errors handled, attach count maintained).
pub(crate) fn install_meth_o(
    ty: &Bound<'_, PyType>,
    name: &'static CStr,
    doc: &'static CStr,
    f: ffi::PyCFunction,
) -> PyResult<()> {
    install_method(ty, name, doc, f, ffi::METH_O)
}

/// Install a `METH_NOARGS` method descriptor (see [`install_meth_o`]).
pub(crate) fn install_noargs(
    ty: &Bound<'_, PyType>,
    name: &'static CStr,
    doc: &'static CStr,
    f: ffi::PyCFunction,
) -> PyResult<()> {
    install_method(ty, name, doc, f, ffi::METH_NOARGS)
}

fn install_method(
    ty: &Bound<'_, PyType>,
    name: &'static CStr,
    doc: &'static CStr,
    f: ffi::PyCFunction,
    flags: c_int,
) -> PyResult<()> {
    let py = ty.py();
    // A method table entry must outlive the type: leaked once per process.
    let def = Box::leak(Box::new(ffi::PyMethodDef {
        ml_name: name.as_ptr(),
        ml_meth: ffi::PyMethodDefPointer { PyCFunction: f },
        ml_flags: flags,
        ml_doc: doc.as_ptr(),
    }));
    // SAFETY: `ty` is a valid type and `def` a valid, never freed method table entry;
    // `PyDescr_NewMethod` returns a new reference (or NULL with an exception).
    let descr = unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyDescr_NewMethod(ty.as_type_ptr(), def))? };
    // SAFETY: `PyType_GetDict` returns a new reference to the type's dict.
    let dict =
        unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyType_GetDict(ty.as_type_ptr()))? }.cast_into::<PyDict>()?;
    dict.set_item(name.to_str().map_err(|e| PyRuntimeError::new_err(e.to_string()))?, descr)?;
    // SAFETY: the dict was changed directly; invalidate the type's attribute caches.
    unsafe { ffi::PyType_Modified(ty.as_type_ptr()) };
    Ok(())
}

/// Install a `METH_FASTCALL | METH_KEYWORDS` method descriptor whose positional arguments are
/// read in place (no argument tuple, unlike pyo3's `*args`); see [`install_meth_o`].
pub(crate) fn install_fastcall(
    ty: &Bound<'_, PyType>,
    name: &'static CStr,
    doc: &'static CStr,
    f: ffi::PyCFunctionFastWithKeywords,
) -> PyResult<()> {
    // SAFETY: a `METH_FASTCALL | METH_KEYWORDS` entry is called with this signature; the
    // union stores the pointer under the `PyCFunction` variant like CPython's `_PyCFunction_CAST`.
    let f = unsafe { std::mem::transmute::<ffi::PyCFunctionFastWithKeywords, ffi::PyCFunction>(f) };
    install_method(ty, name, doc, f, ffi::METH_FASTCALL | ffi::METH_KEYWORDS)
}

/// The positional arguments of a fastcall method (`*others`), rejecting keywords like
/// `set.update`.
///
/// # Safety
/// `args` holds `nargs` live references for the duration of the call; `kwnames` is NULL or a
/// tuple.
pub(crate) unsafe fn fastcall_args<'py>(
    py: Python<'py>,
    name: &str,
    args: *const *mut ffi::PyObject,
    nargs: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    // SAFETY: guaranteed by the caller.
    unsafe {
        if !kwnames.is_null() && ffi::PyTuple_GET_SIZE(kwnames) > 0 {
            return Err(PyTypeError::new_err(format!("{name}() takes no keyword arguments")));
        }
        let n = ffi::PyVectorcall_NARGS(nargs as usize).max(0) as usize;
        Ok((0..n).map(|i| Borrowed::from_ptr(py, *args.add(i)).to_owned()).collect())
    }
}

/// `Set`'s varargs methods as fastcall functions (bodies for pyo3's fastcall trampoline).
mod fastcall {
    use super::*;

    type Args = (*mut ffi::PyObject, *const *mut ffi::PyObject, ffi::Py_ssize_t, *mut ffi::PyObject);

    /// # Safety
    /// Called by the trampoline with a live `Set` (checked by the method descriptor) and
    /// valid fastcall arguments.
    unsafe fn call<'py, R: IntoPyObject<'py>>(
        py: Python<'py>,
        name: &str,
        (slf, args, nargs, kwnames): Args,
        f: impl FnOnce(&Bound<'py, Set>, &[Bound<'py, PyAny>]) -> PyResult<R>,
    ) -> PyResult<*mut ffi::PyObject> {
        use pyo3::IntoPyObjectExt;
        // SAFETY: guaranteed by the caller.
        let (this, others) = unsafe {
            (
                Borrowed::from_ptr(py, slf).cast_unchecked::<Set>().to_owned(),
                fastcall_args(py, name, args, nargs, kwnames)?,
            )
        };
        Ok(f(&this, &others)?.into_bound_py_any(py)?.into_ptr())
    }

    macro_rules! method {
        ($name:ident, |$this:ident, $others:ident| $body:expr) => {
            pub(super) unsafe fn $name(
                py: Python<'_>,
                slf: *mut ffi::PyObject,
                args: *const *mut ffi::PyObject,
                nargs: ffi::Py_ssize_t,
                kwnames: *mut ffi::PyObject,
            ) -> PyResult<*mut ffi::PyObject> {
                // SAFETY: forwarded from the trampoline.
                unsafe { call(py, stringify!($name), (slf, args, nargs, kwnames), |$this, $others| $body) }
            }
        };
    }

    method!(union, |this, others| Set::compute(this, Algebra::Union, others));
    method!(intersection, |this, others| Set::compute(this, Algebra::Intersection, others));
    method!(difference, |this, others| Set::compute(this, Algebra::Difference, others));
    method!(update, |this, others| this.get().in_place(this.py(), Algebra::Union, others).map(|()| this.py().None()));
    method!(intersection_update, |this, others| this
        .get()
        .in_place(this.py(), Algebra::Intersection, others)
        .map(|()| this.py().None()));
    method!(difference_update, |this, others| this
        .get()
        .in_place(this.py(), Algebra::Difference, others)
        .map(|()| this.py().None()));
}

/// The bodies of `Set`'s `METH_O` methods: `self` and the argument as borrowed references.
mod meth_o {
    use super::*;

    /// # Safety
    /// `slf` is a live `Set` (the method descriptor checks the type) and `arg` a live object.
    unsafe fn args<'a, 'py>(
        py: Python<'py>,
        slf: *mut ffi::PyObject,
        arg: *mut ffi::PyObject,
    ) -> (Borrowed<'a, 'py, Set>, Borrowed<'a, 'py, PyAny>) {
        // SAFETY: guaranteed by the caller.
        unsafe { (Borrowed::from_ptr(py, slf).cast_unchecked::<Set>(), Borrowed::from_ptr(py, arg)) }
    }

    fn none(py: Python<'_>) -> *mut ffi::PyObject {
        py.None().into_ptr()
    }

    fn boolean(py: Python<'_>, b: bool) -> *mut ffi::PyObject {
        pyo3::types::PyBool::new(py, b).to_owned().into_any().into_ptr()
    }

    pub(super) unsafe fn add(
        py: Python<'_>,
        slf: *mut ffi::PyObject,
        arg: *mut ffi::PyObject,
    ) -> PyResult<*mut ffi::PyObject> {
        // SAFETY: called by the `METH_O` trampoline with the descriptor's arguments.
        let (this, elem) = unsafe { args(py, slf, arg) };
        this.get().add(py, &elem)?;
        Ok(none(py))
    }

    pub(super) unsafe fn discard(
        py: Python<'_>,
        slf: *mut ffi::PyObject,
        arg: *mut ffi::PyObject,
    ) -> PyResult<*mut ffi::PyObject> {
        // SAFETY: as for `add`.
        let (this, elem) = unsafe { args(py, slf, arg) };
        this.get().discard(py, &elem)?;
        Ok(none(py))
    }

    pub(super) unsafe fn remove(
        py: Python<'_>,
        slf: *mut ffi::PyObject,
        arg: *mut ffi::PyObject,
    ) -> PyResult<*mut ffi::PyObject> {
        // SAFETY: as for `add`.
        let (this, elem) = unsafe { args(py, slf, arg) };
        this.get().remove(py, &elem)?;
        Ok(none(py))
    }

    pub(super) unsafe fn isdisjoint(
        py: Python<'_>,
        slf: *mut ffi::PyObject,
        arg: *mut ffi::PyObject,
    ) -> PyResult<*mut ffi::PyObject> {
        // SAFETY: as for `add`.
        let (this, other) = unsafe { args(py, slf, arg) };
        Ok(boolean(py, Set::isdisjoint(&this, &other)?))
    }

    pub(super) unsafe fn issubset(
        py: Python<'_>,
        slf: *mut ffi::PyObject,
        arg: *mut ffi::PyObject,
    ) -> PyResult<*mut ffi::PyObject> {
        // SAFETY: as for `add`.
        let (this, other) = unsafe { args(py, slf, arg) };
        Ok(boolean(py, Set::issubset(&this, &other)?))
    }

    pub(super) unsafe fn issuperset(
        py: Python<'_>,
        slf: *mut ffi::PyObject,
        arg: *mut ffi::PyObject,
    ) -> PyResult<*mut ffi::PyObject> {
        // SAFETY: as for `add`.
        let (this, other) = unsafe { args(py, slf, arg) };
        Ok(boolean(py, Set::issuperset(&this, &other)?))
    }
}

/// Point a type's `len()` and `in` slots at raw implementations (the `__len__` and
/// `__contains__` methods keep pyo3's; both behave the same).
pub(crate) fn install_len_contains(ty: &Bound<'_, PyType>, len: ffi::lenfunc, contains: ffi::objobjproc) {
    // SAFETY: `ty` is a fully initialised heap type with embedded slot tables, patched at
    // module initialisation before any instance exists.
    unsafe {
        let tp = ty.as_type_ptr();
        if let Some(sq) = (*tp).tp_as_sequence.as_mut() {
            sq.sq_contains = Some(contains);
            if sq.sq_length.is_some() {
                sq.sq_length = Some(len);
            }
        }
        if let Some(mp) = (*tp).tp_as_mapping.as_mut()
            && mp.mp_length.is_some()
        {
            mp.mp_length = Some(len);
        }
        ffi::PyType_Modified(tp);
    }
}

/// Point `tp_iternext` of an iterator type at a raw slot (the `__next__` method keeps
/// pyo3's implementation; both behave the same).
pub(crate) fn install_iternext(ty: &Bound<'_, PyType>, f: ffi::iternextfunc) {
    // SAFETY: `ty` is a fully initialised heap type, patched at module initialisation
    // before any instance exists; `PyType_Modified` drops caches derived from slots.
    unsafe {
        (*ty.as_type_ptr()).tp_iternext = Some(f);
        ffi::PyType_Modified(ty.as_type_ptr());
    }
}

impl SetIterator {
    fn next(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
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
        // Released through `Bound` so the decref happens now even without pyo3's GIL count
        // (the raw `tp_iternext` slot does not maintain it).
        drop(released.map(|s| s.into_bound(py)));
        Ok(None)
    }
}

/// The optional iterable argument of a vectorcall constructor (`T(iterable=(), /)`),
/// rejecting keywords and extra arguments like the builtins.
///
/// # Safety
/// `args` holds `PyVectorcall_NARGS(nargsf)` valid references for the call and `kwnames` is
/// NULL or a tuple of keyword names.
pub(crate) unsafe fn ctor_arg<'py>(
    py: Python<'py>,
    name: &str,
    args: *const *mut ffi::PyObject,
    nargsf: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    // SAFETY: guaranteed by the caller (the offset flag is masked off by `PyVectorcall_NARGS`).
    unsafe {
        if !kwnames.is_null() && ffi::PyTuple_GET_SIZE(kwnames) > 0 {
            return Err(PyTypeError::new_err(format!("{name}() takes no keyword arguments")));
        }
        match ffi::PyVectorcall_NARGS(nargsf as usize) {
            0 => Ok(None),
            1 => Ok(Some(Borrowed::from_ptr(py, *args).to_owned())),
            n => Err(PyTypeError::new_err(format!("{name} expected at most 1 argument, got {n}"))),
        }
    }
}

/// Body of `Set`'s vectorcall constructor, run in pyo3's fastcall trampoline (which has the
/// vectorcall shape: errors raised, panics caught, attach count maintained).
unsafe fn set_ctor(
    py: Python<'_>,
    _callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: ffi::Py_ssize_t,
    kwnames: *mut ffi::PyObject,
) -> PyResult<*mut ffi::PyObject> {
    // SAFETY: forwarded from CPython's vectorcall.
    let arg = unsafe { ctor_arg(py, "Set", args, nargsf, kwnames)? };
    Ok(Bound::new(py, Set::build(py, schema::any(), arg.as_ref())?)?.into_any().into_ptr())
}

/// Vectorcall of the `Set` type object (CPython's `set_vectorcall`): `Set(it)` without
/// `type_call`, an argument tuple or a `tp_init` call. CPython never inherits
/// `tp_vectorcall`, so subclasses keep `tp_new` + `__init__`.
unsafe extern "C" fn set_vectorcall(
    callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let body = pyo3::get_trampoline_function!(fastcall_cfunction_with_keywords, set_ctor);
    // SAFETY: forwarded from CPython's vectorcall (same argument layout as fastcall).
    unsafe { body(callable, args, nargsf as ffi::Py_ssize_t, kwnames) }
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
    install_iternext(&m.py().get_type::<SetIterator>(), set_iter_next);
    let ty = m.py().get_type::<Set>();
    install_len_contains(&ty, set_len_slot, set_contains_slot);
    for (name, doc, f) in [
        (c"union", c"union($self, /, *others)\n--\n\nReturn a new set with elements from the set and all others.", pyo3::get_trampoline_function!(fastcall_cfunction_with_keywords, fastcall::union) as ffi::PyCFunctionFastWithKeywords),
        (c"intersection", c"intersection($self, /, *others)\n--\n\nReturn a new set with elements common to the set and all others.", pyo3::get_trampoline_function!(fastcall_cfunction_with_keywords, fastcall::intersection)),
        (c"difference", c"difference($self, /, *others)\n--\n\nReturn a new set with elements in the set that are not in the others.", pyo3::get_trampoline_function!(fastcall_cfunction_with_keywords, fastcall::difference)),
        (c"update", c"update($self, /, *others)\n--\n\nUpdate the set, adding elements from all others.", pyo3::get_trampoline_function!(fastcall_cfunction_with_keywords, fastcall::update)),
        (c"intersection_update", c"intersection_update($self, /, *others)\n--\n\nUpdate the set, keeping only elements found in it and all others.", pyo3::get_trampoline_function!(fastcall_cfunction_with_keywords, fastcall::intersection_update)),
        (c"difference_update", c"difference_update($self, /, *others)\n--\n\nUpdate the set, removing elements found in others.", pyo3::get_trampoline_function!(fastcall_cfunction_with_keywords, fastcall::difference_update)),
    ] {
        install_fastcall(&ty, name, doc, f)?;
    }
    install_meth_o(
        &ty,
        c"add",
        c"add($self, object, /)\n--\n\nAdd an element to a set.",
        pyo3::get_trampoline_function!(binaryfunc, meth_o::add),
    )?;
    install_meth_o(
        &ty,
        c"discard",
        c"discard($self, object, /)\n--\n\nRemove an element from a set if it is a member.",
        pyo3::get_trampoline_function!(binaryfunc, meth_o::discard),
    )?;
    install_meth_o(
        &ty,
        c"remove",
        c"remove($self, object, /)\n--\n\nRemove an element from a set; it must be a member.",
        pyo3::get_trampoline_function!(binaryfunc, meth_o::remove),
    )?;
    install_meth_o(
        &ty,
        c"isdisjoint",
        c"isdisjoint($self, other, /)\n--\n\nReturn True if two sets have a null intersection.",
        pyo3::get_trampoline_function!(binaryfunc, meth_o::isdisjoint),
    )?;
    install_meth_o(
        &ty,
        c"issubset",
        c"issubset($self, other, /)\n--\n\nReport whether another set contains this set.",
        pyo3::get_trampoline_function!(binaryfunc, meth_o::issubset),
    )?;
    install_meth_o(
        &ty,
        c"issuperset",
        c"issuperset($self, other, /)\n--\n\nReport whether this set contains another set.",
        pyo3::get_trampoline_function!(binaryfunc, meth_o::issuperset),
    )?;
    install_vectorcall(&ty, set_vectorcall);
    install_init(&ty, "_rustique_init")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded from CPython 3.15 (`hash(frozenset(...))`; int hashes are process-independent).
    #[test]
    fn frozenset_hash_matches_cpython() {
        let h = |xs: &[i64]| frozenset_hash_of(xs.iter().map(|&v| keys::py_hash_i64(v)));
        assert_eq!(h(&[]), 133_146_708_735_736);
        assert_eq!(h(&[1]), -558_064_481_276_695_278);
        assert_eq!(h(&[1, 2, -1]), 3_528_648_450_907_520_039);
        assert_eq!(h(&[2, -1, 1]), h(&[1, 2, -1]));
        // 2**61 hashes like 1.
        assert_eq!(h(&[1 << 61]), h(&[1]));
    }

    #[test]
    fn scrambled_visits_every_position_once() {
        for n in [0usize, 1, 7, 8, 9, 16, 17, 100, 1000, 4096, 12_345] {
            let mut seen: Vec<usize> = scrambled(n).collect();
            assert_eq!(seen.len(), n);
            seen.sort_unstable();
            assert!(seen.iter().copied().eq(0..n), "n = {n}");
        }
        // An overlapping second half is met early, not after scanning the first half.
        let first_tail = scrambled(1000).position(|p| p >= 500).unwrap();
        assert!(first_tail < 32);
    }

    #[test]
    fn native_kernels_follow_set_semantics() {
        let ki = |xs: &[i64]| {
            let mut k: KeyIndex<i64> = KeyIndex::default();
            for &x in xs {
                k.insert(x);
            }
            k
        };
        let keys = |k: &KeyIndex<i64>| k.iter().map(|(_, &v)| v).collect::<Vec<_>>();
        let (a, b) = (ki(&[1, 2, 3, 4]), ki(&[3, 4, 5]));
        assert_eq!(keys(&native_op(Algebra::Union, &a, &b, true)), [1, 2, 3, 4, 5]);
        // The smaller set is iterated (its order kept); an iterable is always iterated.
        assert_eq!(keys(&native_op(Algebra::Intersection, &a, &b, true)), [3, 4]);
        assert_eq!(keys(&native_op(Algebra::Intersection, &ki(&[4, 3]), &ki(&[3, 4, 9]), true)), [4, 3]);
        assert_eq!(keys(&native_op(Algebra::Intersection, &ki(&[4, 3]), &ki(&[3, 4, 9]), false)), [3, 4]);
        assert_eq!(keys(&native_op(Algebra::Difference, &a, &b, true)), [1, 2]);
        assert_eq!(keys(&native_op(Algebra::SymmetricDifference, &a, &b, true)), [1, 2, 5]);
        // Copy-and-remove difference (most of `a` survives).
        let big = ki(&(0..100).collect::<Vec<_>>());
        let d = native_op(Algebra::Difference, &big, &ki(&[5, 50]), true);
        assert_eq!(d.len(), 98);
        assert!(!d.contains(&5) && !d.contains(&50) && d.contains(&99));
        let mut acc = ki(&[1]);
        native_update(Algebra::Union, &mut acc, &ki(&[2, 1]));
        native_update(Algebra::Difference, &mut acc, &ki(&[1]));
        assert_eq!(keys(&acc), [2]);
    }
}
