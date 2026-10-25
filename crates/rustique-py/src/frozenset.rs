//! The `FrozenSet` class: an immutable, hashable, insertion-ordered set with native element
//! strategies (`i64`, `str`, or Python objects).
//!
//! OWNER: set agent. Items marked `INTERFACE` are used by other modules.
//!
//! A `FrozenSet` never changes after construction, so it has no lock: lookups compare against
//! the table directly (a comparison that runs Python code cannot change it) and iterators read
//! it without synchronisation. `hash()` is CPython's `frozenset_hash` over the elements'
//! Python hashes, bit-identical to `hash(frozenset(...))` of equal contents; it is computed
//! on first use and cached. Algebra follows the left operand's type, like CPython:
//! `FrozenSet | Set` is a `FrozenSet`, `Set | FrozenSet` a `Set`.

use std::sync::atomic::{AtomicIsize, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use pyo3::exceptions::PyTypeError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::pyclass::CompareOp;
use pyo3::types::{PyDict, PyTuple, PyType};
use pyo3::{Borrowed, PyTraverseError, ffi};

use crate::dict::keys::{self, Key, KeyKind, KeyStore, KeyTable};
use crate::schema::{self, Schema};
use crate::set::{self, Algebra, Operand};

/// No hash computed yet (CPython's frozenset hash is never -1).
const NO_HASH: isize = -1;

/// An immutable, hashable set.
#[pyclass(module = "rustique", frozen, weakref, subclass, immutable_type)]
pub struct FrozenSet {
    schema: Arc<Schema>,
    table: KeyTable,
    /// `hash(self)` once computed, else [`NO_HASH`].
    hash: AtomicIsize,
}

impl FrozenSet {
    /// INTERFACE. Construct from an optional iterable; an untyped `FrozenSet` argument of the
    /// same schema is returned as it is (CPython's `frozenset(fs) is fs`).
    pub fn from_iterable<'py>(
        py: Python<'py>,
        schema: Arc<Schema>,
        iterable: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, FrozenSet>> {
        if let Some(it) = iterable
            && let Ok(frozen) = it.cast_exact::<FrozenSet>()
            && set::same_schema(&frozen.get().schema, &schema)
        {
            return Ok(frozen.clone());
        }
        Bound::new(py, FrozenSet::build(py, schema, iterable)?)
    }

    /// INTERFACE (set.rs). Wrap a finished table.
    pub(crate) fn from_table(schema: Arc<Schema>, table: KeyTable) -> FrozenSet {
        FrozenSet { schema, table, hash: AtomicIsize::new(NO_HASH) }
    }

    fn build<'py>(py: Python<'py>, schema: Arc<Schema>, iterable: Option<&Bound<'py, PyAny>>) -> PyResult<FrozenSet> {
        let table = match iterable {
            Some(it) => set::build_table(py, &schema, it, &|| FrozenSet::name_for(&schema))?,
            None => KeyTable::new(KeyKind::for_schema(&schema)),
        };
        Ok(FrozenSet::from_table(schema, table))
    }

    /// INTERFACE.
    pub fn schema(&self) -> &Arc<Schema> {
        &self.schema
    }

    pub(crate) fn table(&self) -> &KeyTable {
        &self.table
    }

    /// The hash, if it was computed already.
    pub(crate) fn cached_hash(&self) -> Option<isize> {
        let h = self.hash.load(Ordering::Relaxed);
        (h != NO_HASH).then_some(h)
    }

    fn hash_value(&self) -> isize {
        if let Some(h) = self.cached_hash() {
            return h;
        }
        let h = set::table_frozenset_hash(&self.table);
        self.hash.store(h, Ordering::Relaxed);
        h
    }

    fn name_for(schema: &Schema) -> String {
        if schema.is_any() { "FrozenSet".to_owned() } else { format!("FrozenSet[{schema}]") }
    }

    fn element_objects(&self, py: Python<'_>) -> Vec<Py<PyAny>> {
        self.table.key_objects(py)
    }

    /// `elem in self` (identity hits and clean misses in object tables take one hash).
    fn contains(&self, elem: &Bound<'_, PyAny>) -> PyResult<bool> {
        let py = elem.py();
        if let Some(probe) = keys::quick(elem)
            && let Some(pos) = self.table.find_quick(&probe)
        {
            return Ok(pos.is_some());
        }
        let Ok(hash) = elem.hash() else {
            // Unhashable: a set probe or an error (raised again by `probe`).
            let key = set::probe(elem)?;
            return Ok(self.table.find_local(py, &key)?.is_some());
        };
        if let KeyStore::Object(ki) = self.table.store() {
            let mut candidates = ki.index().candidates(keys::mix(hash)).peekable();
            if candidates.peek().is_none() {
                return Ok(false);
            }
            if candidates.any(|pos| ki.raw_keys()[pos].is(elem)) {
                return Ok(true);
            }
        }
        let key = Key::with_hash(elem.clone(), hash)?;
        Ok(self.table.find_local(py, &key)?.is_some())
    }

    fn schema_type_of<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if self.schema.is_any() {
            Ok(py.get_type::<FrozenSet>().into_any())
        } else {
            crate::typing::frozenset_alias_for(py, &self.schema)
        }
    }

    /// `self <op> others…` as a new `FrozenSet`.
    fn compute(slf: &Bound<'_, Self>, op: Algebra, others: &[Bound<'_, PyAny>]) -> PyResult<FrozenSet> {
        let py = slf.py();
        let operands = others.iter().map(Operand::new).collect::<PyResult<Vec<_>>>()?;
        let schema = set::result_schema(&slf.get().schema, &operands);
        let kind = KeyKind::for_schema(&schema);
        let table = set::fold(py, op, &Operand::Frozen(slf.clone()), &operands, kind)?;
        Ok(FrozenSet::from_table(schema, table))
    }

    fn binary(slf: &Bound<'_, Self>, op: Algebra, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        if !set::is_set_operand(other) {
            return Ok(py.NotImplemented());
        }
        Ok(Bound::new(py, FrozenSet::compute(slf, op, std::slice::from_ref(other))?)?.into_any().unbind())
    }
}

/// Whether `cls` defines its own `__init__` (then, like CPython's `frozenset_new`, keyword
/// arguments are left to it).
fn has_own_init(cls: &Bound<'_, PyType>) -> bool {
    // SAFETY: valid, ready type objects; `PyType_GetSlot` accepts static and heap types.
    unsafe {
        ffi::PyType_GetSlot(cls.as_type_ptr(), ffi::Py_tp_init)
            != ffi::PyType_GetSlot(&raw mut ffi::PyBaseObject_Type, ffi::Py_tp_init)
    }
}

#[pymethods]
impl FrozenSet {
    /// `FrozenSet(iterable=(), /)`.
    #[new]
    #[classmethod]
    #[pyo3(signature = (*args, **kwargs))]
    fn __new__(cls: &Bound<'_, PyType>, args: &Bound<'_, PyTuple>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
        let py = cls.py();
        let name = cls.name()?.to_string();
        if cls.is(py.get_type::<FrozenSet>()) || !has_own_init(cls) {
            set::no_keywords(&name, kwargs)?;
        }
        FrozenSet::build(py, schema::any(), set::at_most_one(&name, args)?.as_ref())
    }

    #[classmethod]
    fn __class_getitem__<'py>(cls: &Bound<'py, PyType>, item: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        crate::typing::frozenset_alias(cls.py(), item)
    }

    fn __hash__(&self) -> isize {
        self.hash_value()
    }

    fn __len__(&self) -> usize {
        self.table.len()
    }

    fn __contains__(&self, elem: &Bound<'_, PyAny>) -> PyResult<bool> {
        self.contains(elem)
    }

    fn __iter__(slf: &Bound<'_, Self>) -> FrozenSetIterator {
        FrozenSetIterator { set: slf.clone().unbind(), pos: AtomicUsize::new(0), yielded: AtomicUsize::new(0) }
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let this = slf.get();
        let name = set::display_name(slf.as_any(), &py.get_type::<FrozenSet>(), "FrozenSet", &this.schema)?;
        set::set_repr(slf.as_any(), &name, || this.element_objects(py))
    }

    fn __richcmp__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<Py<PyAny>> {
        set::richcompare(slf.py(), &Operand::Frozen(slf.clone()), other, op, slf.get().cached_hash())
    }

    /// Frozen sets are immutable: a copy is the set itself.
    fn copy(slf: &Bound<'_, Self>) -> PyResult<Py<PyAny>> {
        copy_of(slf)
    }

    fn __copy__(slf: &Bound<'_, Self>) -> PyResult<Py<PyAny>> {
        copy_of(slf)
    }

    #[pyo3(signature = (*others))]
    fn union(slf: &Bound<'_, Self>, others: Vec<Bound<'_, PyAny>>) -> PyResult<FrozenSet> {
        FrozenSet::compute(slf, Algebra::Union, &others)
    }

    #[pyo3(signature = (*others))]
    fn intersection(slf: &Bound<'_, Self>, others: Vec<Bound<'_, PyAny>>) -> PyResult<FrozenSet> {
        FrozenSet::compute(slf, Algebra::Intersection, &others)
    }

    #[pyo3(signature = (*others))]
    fn difference(slf: &Bound<'_, Self>, others: Vec<Bound<'_, PyAny>>) -> PyResult<FrozenSet> {
        FrozenSet::compute(slf, Algebra::Difference, &others)
    }

    fn symmetric_difference(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<FrozenSet> {
        FrozenSet::compute(slf, Algebra::SymmetricDifference, std::slice::from_ref(other))
    }

    fn issubset(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        set::is_subset(slf.py(), &Operand::Frozen(slf.clone()), &Operand::new(other)?)
    }

    fn issuperset(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        set::is_superset(slf.py(), &Operand::Frozen(slf.clone()), &Operand::new(other)?)
    }

    fn isdisjoint(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        set::is_disjoint(slf.py(), &Operand::Frozen(slf.clone()), &Operand::new(other)?)
    }

    fn __or__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        FrozenSet::binary(slf, Algebra::Union, other)
    }

    fn __ror__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        set::reflected(slf.py(), Algebra::Union, other, &Operand::Frozen(slf.clone()))
    }

    fn __and__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        FrozenSet::binary(slf, Algebra::Intersection, other)
    }

    fn __rand__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        set::reflected(slf.py(), Algebra::Intersection, other, &Operand::Frozen(slf.clone()))
    }

    fn __sub__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        FrozenSet::binary(slf, Algebra::Difference, other)
    }

    fn __rsub__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        set::reflected(slf.py(), Algebra::Difference, other, &Operand::Frozen(slf.clone()))
    }

    fn __xor__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        FrozenSet::binary(slf, Algebra::SymmetricDifference, other)
    }

    fn __rxor__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        set::reflected(slf.py(), Algebra::SymmetricDifference, other, &Operand::Frozen(slf.clone()))
    }

    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyTuple>> {
        let py = slf.py();
        let this = slf.get();
        set::reduce(slf.as_any(), &py.get_type::<FrozenSet>(), this.schema_type_of(py)?, this.element_objects(py))
    }

    /// The alias describing this set's type (`FrozenSet[int]`, or `FrozenSet` when untyped).
    #[getter]
    fn schema_type<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.schema_type_of(py)
    }

    /// Element strategy (`"empty"`, `"i64"`, `"str"` or `"object"`), for introspection.
    #[getter]
    fn _strategy(&self) -> &'static str {
        self.table.strategy()
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        // Immutable: no lock to take. (No `__clear__`: like a tuple, a frozen set cannot be
        // part of a cycle that some mutable member does not break.)
        for k in self.table.object_keys() {
            visit.call(k)?;
        }
        Ok(())
    }
}

/// `copy()` of a frozen set: itself for `FrozenSet`, a `FrozenSet` for subclass instances
/// (CPython's `frozenset.copy`).
fn copy_of(slf: &Bound<'_, FrozenSet>) -> PyResult<Py<PyAny>> {
    let py = slf.py();
    if slf.get_type().is(py.get_type::<FrozenSet>()) {
        return Ok(slf.clone().into_any().unbind());
    }
    let this = slf.get();
    let copy = FrozenSet::from_table(this.schema.clone(), this.table.clone_ref(py));
    Ok(Bound::new(py, copy)?.into_any().unbind())
}

/// Iterator over a `FrozenSet`: the table never changes, so the position is a lock-free
/// atomic (each element is yielded once even when threads share the iterator).
#[pyclass(module = "rustique", frozen)]
pub struct FrozenSetIterator {
    set: Py<FrozenSet>,
    /// Next position to examine.
    pos: AtomicUsize,
    yielded: AtomicUsize,
}

#[pymethods]
impl FrozenSetIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        let table = &self.set.get().table;
        let mut pos = self.pos.load(Ordering::Relaxed);
        loop {
            let found = table.next_live(pos)?;
            match self.pos.compare_exchange_weak(pos, found + 1, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => {
                    self.yielded.fetch_add(1, Ordering::Relaxed);
                    return Some(table.key_object(py, found));
                }
                Err(now) => pos = now,
            }
        }
    }

    fn __length_hint__(&self) -> usize {
        self.set.get().table.len().saturating_sub(self.yielded.load(Ordering::Relaxed))
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.set)
    }

    /// `(iter, (remaining elements,))`, like CPython's set iterators.
    fn __reduce__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let table = &self.set.get().table;
        let mut remaining = Vec::new();
        let mut pos = self.pos.load(Ordering::Relaxed);
        while let Some(p) = table.next_live(pos) {
            remaining.push(table.key_object(py, p));
            pos = p + 1;
        }
        let iter = py.import("builtins")?.getattr("iter")?;
        PyTuple::new(py, [iter, PyTuple::new(py, [pyo3::types::PyList::new(py, remaining)?])?.into_any()])
    }
}

/// pyo3's `tp_new` for `FrozenSet` (wrapped by [`frozenset_new`]) and the type it belongs to.
static PYO3_NEW: OnceLock<ffi::newfunc> = OnceLock::new();
static FROZENSET_TYPE: AtomicPtr<ffi::PyTypeObject> = AtomicPtr::new(std::ptr::null_mut());

/// `tp_new` of `FrozenSet`: `FrozenSet(fs)` returns an untyped `FrozenSet` argument itself
/// (CPython's `frozenset_new` does the same for an exact `frozenset`, which makes copying a
/// frozen set O(1)); every other call goes to pyo3's constructor.
unsafe extern "C" fn frozenset_new(
    subtype: *mut ffi::PyTypeObject,
    args: *mut ffi::PyObject,
    kwds: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: CPython calls `tp_new` with a valid type, an argument tuple and an optional
    // keyword dict, attached to the interpreter. `Py_TYPE(arg) == subtype` with `subtype` the
    // `FrozenSet` type means `arg` is a `FrozenSet` instance (exact type).
    unsafe {
        if subtype == FROZENSET_TYPE.load(Ordering::Relaxed)
            && (kwds.is_null() || ffi::PyDict_Size(kwds) == 0)
            && ffi::PyTuple_GET_SIZE(args) == 1
        {
            let arg = ffi::PyTuple_GET_ITEM(args, 0);
            if ffi::Py_TYPE(arg) == subtype {
                let untyped = Python::attach(|py| {
                    Borrowed::from_ptr(py, arg).cast::<FrozenSet>().is_ok_and(|fs| fs.get().schema.is_any())
                });
                if untyped {
                    ffi::Py_IncRef(arg);
                    return arg;
                }
            }
        }
        match PYO3_NEW.get() {
            Some(new) => new(subtype, args, kwds),
            None => {
                ffi::PyErr_SetString(ffi::PyExc_SystemError, c"rustique: FrozenSet constructor missing".as_ptr());
                std::ptr::null_mut()
            }
        }
    }
}

/// Vectorcall of the `FrozenSet` type object (CPython's `frozenset_vectorcall`), with the
/// identity rule of [`frozenset_new`].
unsafe extern "C" fn frozenset_vectorcall(
    _callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: forwarded from CPython's vectorcall.
    unsafe {
        set::vectorcall_ctor("FrozenSet", args, nargsf, kwnames, |py, arg| {
            Ok(FrozenSet::from_iterable(py, schema::any(), arg)?.into_any())
        })
    }
}

/// Route `FrozenSet`'s `tp_new` through [`frozenset_new`] (once per process).
fn install_new(ty: &Bound<'_, PyType>) -> PyResult<()> {
    let tp = ty.as_type_ptr();
    // SAFETY: `tp` is the fully initialised `FrozenSet` heap type; module initialisation runs
    // before any `FrozenSet` is created, and the slot is only replaced by our wrapper, which
    // forwards to the original. `PyType_Modified` invalidates caches derived from the slot.
    unsafe {
        let current = (*tp).tp_new;
        let ours = frozenset_new as ffi::newfunc as usize;
        if current.map(|f| f as usize) != Some(ours) {
            let Some(original) = current else {
                return Err(PyTypeError::new_err("rustique: FrozenSet has no tp_new"));
            };
            let _ = PYO3_NEW.set(original);
            FROZENSET_TYPE.store(tp, Ordering::Relaxed);
            (*tp).tp_new = Some(frozenset_new);
            ffi::PyType_Modified(tp);
        }
    }
    Ok(())
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<FrozenSet>()?;
    m.add_class::<FrozenSetIterator>()?;
    let ty = m.py().get_type::<FrozenSet>();
    set::install_vectorcall(&ty, frozenset_vectorcall);
    install_new(&ty)
}
