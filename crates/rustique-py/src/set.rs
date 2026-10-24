//! The `Set` class (ARCHITECTURE §4.3): an insertion-ordered set with native element
//! strategies (`i64`, `str`, or Python objects).
//!
//! OWNER: dict/set agent. Items marked `INTERFACE` are used by other modules.

use std::hash::Hash;
use std::sync::Arc;

use pyo3::exceptions::{PyKeyError, PyRuntimeError, PyTypeError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::pyclass::CompareOp;
use pyo3::types::{PyFrozenSet, PyList, PySet, PyType};
use pyo3::PyTraverseError;
use rustique_core::index::KeyIndex;

use crate::dict::keys::{self, Key, KeyKind, KeyTable, Keyed, Locked, NativeKeys, Role};
use crate::dict::{ReprGuard, collect_iterable};
use crate::schema::{self, Schema};

pub(crate) struct SetInner {
    table: KeyTable,
}

impl Keyed for SetInner {
    fn table(&self) -> &KeyTable {
        &self.table
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

/// What a set operation's other operand is.
enum Operand<'py> {
    Rustique(Bound<'py, Set>),
    /// Builtin `set`/`frozenset`: membership via Python `in`.
    Builtin(Bound<'py, PyAny>),
    /// Elements collected into a private table; `is_set` when they came from a set.
    Local { table: KeyTable, is_set: bool },
}

impl<'py> Operand<'py> {
    fn new(obj: &Bound<'py, PyAny>) -> PyResult<Operand<'py>> {
        if let Ok(s) = obj.cast::<Set>() {
            return Ok(Operand::Rustique(s.clone()));
        }
        if obj.is_instance_of::<PySet>() || obj.is_instance_of::<PyFrozenSet>() {
            return Ok(Operand::Builtin(obj.clone()));
        }
        Ok(Operand::Local { table: local_table(obj, KeyKind::Adaptive)?, is_set: false })
    }

    fn is_set(&self) -> bool {
        match self {
            Operand::Local { is_set, .. } => *is_set,
            _ => true,
        }
    }

    fn len(&self) -> PyResult<usize> {
        match self {
            Operand::Rustique(s) => Ok(s.get().len(s.py())),
            Operand::Builtin(obj) => obj.len(),
            Operand::Local { table, .. } => Ok(table.len()),
        }
    }

    fn contains(&self, elem: &Bound<'py, PyAny>) -> PyResult<bool> {
        match self {
            Operand::Rustique(s) => s.get().contains(elem),
            Operand::Builtin(obj) => obj.contains(elem),
            Operand::Local { table, .. } => {
                Ok(table.find_local(elem.py(), &Key::classify(elem, Role::SetElement)?)?.is_some())
            }
        }
    }

    fn elements(&self, py: Python<'py>) -> Vec<Bound<'py, PyAny>> {
        match self {
            Operand::Rustique(s) => s.get().element_objects(py).into_iter().map(|o| o.into_bound(py)).collect(),
            Operand::Builtin(obj) => collect_iterable(obj).0,
            Operand::Local { table, .. } => table.key_objects(py).into_iter().map(|o| o.into_bound(py)).collect(),
        }
    }

    /// Native keys, when the operand stores native keys.
    fn native(&self) -> Option<NativeKeys> {
        match self {
            Operand::Rustique(s) => s.get().inner.read(s.py()).table.native_clone(),
            Operand::Local { table, .. } => table.native_clone(),
            Operand::Builtin(_) => None,
        }
    }

    /// Schema of a rustique operand.
    fn schema(&self) -> Option<&Arc<Schema>> {
        match self {
            Operand::Rustique(s) => Some(&s.get().schema),
            _ => None,
        }
    }
}

/// Collect `obj`'s elements into a private (unshared) table.
fn local_table(obj: &Bound<'_, PyAny>, kind: KeyKind) -> PyResult<KeyTable> {
    let py = obj.py();
    let (items, err) = collect_iterable(obj);
    if let Some(err) = err {
        return Err(err);
    }
    let mut table = KeyTable::new(kind);
    for item in &items {
        table.insert_local(py, &Key::classify(item, Role::SetElement)?)?;
    }
    Ok(table)
}

fn union_native<K: Hash + Eq + Clone>(mut a: KeyIndex<K>, b: &KeyIndex<K>) -> KeyIndex<K> {
    a.reserve(b.len());
    for (_, k) in b.iter() {
        a.insert(k.clone());
    }
    a
}

fn intersection_native<K: Hash + Eq + Clone + Default>(a: &KeyIndex<K>, b: &KeyIndex<K>) -> KeyIndex<K> {
    let (small, large) = if b.len() < a.len() { (b, a) } else { (a, b) };
    let mut out = KeyIndex::default();
    for (_, k) in small.iter() {
        if large.contains(k) {
            out.insert(k.clone());
        }
    }
    out
}

fn difference_native<K: Hash + Eq + Clone + Default>(a: &KeyIndex<K>, b: &KeyIndex<K>) -> KeyIndex<K> {
    let mut out = KeyIndex::default();
    for (_, k) in a.iter() {
        if !b.contains(k) {
            out.insert(k.clone());
        }
    }
    out
}

fn symmetric_difference_native<K: Hash + Eq + Clone + Default>(a: &KeyIndex<K>, b: &KeyIndex<K>) -> KeyIndex<K> {
    let mut out = KeyIndex::default();
    for (_, k) in a.iter() {
        if !b.contains(k) {
            out.insert(k.clone());
        }
    }
    for (_, k) in b.iter() {
        if !a.contains(k) {
            out.insert(k.clone());
        }
    }
    out
}

fn subset_native<K: Hash + Eq>(a: &KeyIndex<K>, b: &KeyIndex<K>) -> bool {
    a.len() <= b.len() && a.iter().all(|(_, k)| b.contains(k))
}

fn disjoint_native<K: Hash + Eq>(a: &KeyIndex<K>, b: &KeyIndex<K>) -> bool {
    let (small, large) = if b.len() < a.len() { (b, a) } else { (a, b) };
    small.iter().all(|(_, k)| !large.contains(k))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Algebra {
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
}

fn native_algebra(op: Algebra, a: NativeKeys, b: &NativeKeys) -> Result<NativeKeys, NativeKeys> {
    Ok(match (a, b) {
        (NativeKeys::Int(a), NativeKeys::Int(b)) => NativeKeys::Int(match op {
            Algebra::Union => union_native(a, b),
            Algebra::Intersection => intersection_native(&a, b),
            Algebra::Difference => difference_native(&a, b),
            Algebra::SymmetricDifference => symmetric_difference_native(&a, b),
        }),
        (NativeKeys::Str(a), NativeKeys::Str(b)) => NativeKeys::Str(match op {
            Algebra::Union => union_native(a, b),
            Algebra::Intersection => intersection_native(&a, b),
            Algebra::Difference => difference_native(&a, b),
            Algebra::SymmetricDifference => symmetric_difference_native(&a, b),
        }),
        (a, _) => return Err(a),
    })
}

/// An insertion-ordered set with native element storage.
#[pyclass(module = "rustique", frozen)]
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

    fn with_table(schema: Arc<Schema>, table: KeyTable) -> Set {
        Set { schema, inner: Locked::new(SetInner { table }) }
    }

    fn empty(schema: Arc<Schema>) -> Set {
        let table = KeyTable::new(KeyKind::for_schema(&schema));
        Set::with_table(schema, table)
    }

    fn build<'py>(py: Python<'py>, schema: Arc<Schema>, iterable: Option<&Bound<'py, PyAny>>) -> PyResult<Set> {
        if let Some(it) = iterable
            && let Ok(other) = it.cast::<Set>()
            && other.get().same_schema(&schema)
        {
            let table = other.get().inner.read(py).table.clone_ref(py);
            return Ok(Set::with_table(schema, table));
        }
        let set = Set::empty(schema);
        if let Some(it) = iterable {
            set.add_all(py, it)?;
        }
        Ok(set)
    }

    fn same_schema(&self, schema: &Arc<Schema>) -> bool {
        Arc::ptr_eq(&self.schema, schema) || *self.schema == **schema
    }

    fn type_name(&self) -> String {
        if self.schema.is_any() { "Set".to_owned() } else { format!("Set[{}]", self.schema) }
    }

    fn prepare<'py>(&self, elem: &Bound<'py, PyAny>) -> PyResult<Key<'py>> {
        keys::prepare(elem, &self.schema, Role::SetElement, || self.type_name())
    }

    /// Classify a lookup probe; an unhashable builtin `set` is looked up as a `frozenset`
    /// (like CPython).
    fn probe<'py>(elem: &Bound<'py, PyAny>) -> PyResult<Key<'py>> {
        match Key::classify(elem, Role::SetElement) {
            Err(e) if elem.is_instance_of::<PySet>() && e.is_instance_of::<PyTypeError>(elem.py()) => {
                let frozen = PyFrozenSet::new(elem.py(), elem.try_iter()?.collect::<PyResult<Vec<_>>>()?)?;
                Key::classify(frozen.as_any(), Role::SetElement)
            }
            other => other,
        }
    }

    pub(crate) fn len(&self, py: Python<'_>) -> usize {
        self.inner.read(py).table.len()
    }

    pub(crate) fn element_objects(&self, py: Python<'_>) -> Vec<Py<PyAny>> {
        self.inner.read(py).table.key_objects(py)
    }

    fn contains(&self, elem: &Bound<'_, PyAny>) -> PyResult<bool> {
        let key = Set::probe(elem)?;
        keys::with_entry(elem.py(), &self.inner, &key, |_, pos| pos.is_some())
    }

    /// Insert every element of `iterable` (validated) in order; the valid prefix is kept
    /// when an element is rejected.
    fn add_all(&self, py: Python<'_>, iterable: &Bound<'_, PyAny>) -> PyResult<()> {
        let (items, mut err) = collect_iterable(iterable);
        let mut prepared = Vec::with_capacity(items.len());
        for item in &items {
            match self.prepare(item) {
                Ok(key) => prepared.push(key),
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        keys::batch_mut(py, &self.inner, prepared, |k| k, |inner, key, pos| {
            if pos.is_none() {
                inner.table.push(py, &key)?;
            }
            Ok(())
        })?;
        err.map_or(Ok(()), Err)
    }

    /// Remove every element of `iterable` that is present.
    fn remove_all(&self, py: Python<'_>, iterable: &Bound<'_, PyAny>) -> PyResult<()> {
        let (items, err) = collect_iterable(iterable);
        if let Some(err) = err {
            return Err(err);
        }
        let probes = items.iter().map(Set::probe).collect::<PyResult<Vec<_>>>()?;
        let mut displaced = Vec::new();
        keys::batch_mut(py, &self.inner, probes, |k| k, |inner, _, pos| {
            if let Some(pos) = pos {
                inner.remove_at(py, pos, &mut displaced);
            }
            Ok(())
        })?;
        drop(displaced);
        Ok(())
    }

    /// Remove `elem`; returns whether it was present.
    fn discard_elem(&self, py: Python<'_>, elem: &Bound<'_, PyAny>) -> PyResult<bool> {
        let key = Set::probe(elem)?;
        let mut displaced = Vec::new();
        let found = keys::with_entry_mut(py, &self.inner, &key, |inner, pos| {
            pos.map(|p| inner.remove_at(py, p, &mut displaced)).is_some()
        })?;
        drop(displaced);
        Ok(found)
    }

    fn snapshot_table(&self, py: Python<'_>) -> (KeyTable, u64) {
        let inner = self.inner.read(py);
        (inner.table.clone_ref(py), inner.table.version())
    }

    /// Result schema of a binary operation: ours when every operand is a `Set` of the
    /// same schema, otherwise untyped.
    fn result_schema(&self, others: &[Operand<'_>]) -> Arc<Schema> {
        if !self.schema.is_any() && others.iter().all(|o| o.schema().is_some_and(|s| self.same_schema(s))) {
            self.schema.clone()
        } else {
            schema::any()
        }
    }

    fn compute(&self, py: Python<'_>, op: Algebra, others: &[Bound<'_, PyAny>]) -> PyResult<Set> {
        let operands = others.iter().map(Operand::new).collect::<PyResult<Vec<_>>>()?;
        let schema = self.result_schema(&operands);
        let kind = KeyKind::for_schema(&schema);
        let mut table = self.snapshot_table(py).0.with_kind(kind);
        for operand in &operands {
            table = table_algebra(py, op, table, operand, kind)?;
        }
        Ok(Set::with_table(schema, table))
    }

    /// Replace the contents with `f(snapshot)`, retrying if the set changed meanwhile.
    fn replace_with(&self, py: Python<'_>, f: impl Fn(KeyTable) -> PyResult<KeyTable>) -> PyResult<()> {
        loop {
            let (snapshot, version) = self.snapshot_table(py);
            let table = f(snapshot)?;
            let displaced = {
                let mut inner = self.inner.write(py);
                if inner.table.version() != version {
                    continue;
                }
                inner.table.replace(table)
            };
            drop(displaced);
            return Ok(());
        }
    }

    fn is_subset_of(&self, py: Python<'_>, other: &Operand<'_>) -> PyResult<bool> {
        if let Some(theirs) = other.native()
            && let Some(mine) = self.inner.read(py).table.native_clone()
        {
            match (&mine, &theirs) {
                (NativeKeys::Int(a), NativeKeys::Int(b)) => return Ok(subset_native(a, b)),
                (NativeKeys::Str(a), NativeKeys::Str(b)) => return Ok(subset_native(a, b)),
                _ => {}
            }
        }
        let elems = self.element_objects(py);
        if elems.len() > other.len()? {
            return Ok(false);
        }
        for e in elems {
            if !other.contains(e.bind(py))? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn is_superset_of(&self, py: Python<'_>, other: &Operand<'_>) -> PyResult<bool> {
        if let Some(theirs) = other.native()
            && let Some(mine) = self.inner.read(py).table.native_clone()
        {
            match (&mine, &theirs) {
                (NativeKeys::Int(a), NativeKeys::Int(b)) => return Ok(subset_native(b, a)),
                (NativeKeys::Str(a), NativeKeys::Str(b)) => return Ok(subset_native(b, a)),
                _ => {}
            }
        }
        for e in other.elements(py) {
            if !self.contains(&e)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn is_disjoint_with(&self, py: Python<'_>, other: &Operand<'_>) -> PyResult<bool> {
        if let Some(theirs) = other.native()
            && let Some(mine) = self.inner.read(py).table.native_clone()
        {
            match (&mine, &theirs) {
                (NativeKeys::Int(a), NativeKeys::Int(b)) => return Ok(disjoint_native(a, b)),
                (NativeKeys::Str(a), NativeKeys::Str(b)) => return Ok(disjoint_native(a, b)),
                _ => {}
            }
        }
        if other.is_set() && other.len()? > self.len(py) {
            for e in self.element_objects(py) {
                if other.contains(e.bind(py))? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        for e in other.elements(py) {
            if self.contains(&e)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn schema_type_of<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if self.schema.is_any() {
            Ok(py.get_type::<Set>().into_any())
        } else {
            crate::typing::set_alias_for(py, &self.schema)
        }
    }

    fn binary(&self, py: Python<'_>, op: Algebra, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if !is_set_operand(other) {
            return Ok(py.NotImplemented());
        }
        Ok(Bound::new(py, self.compute(py, op, std::slice::from_ref(other))?)?.into_any().unbind())
    }

    /// `other <op> self` with a builtin set on the left: an untyped result.
    fn reflected(&self, py: Python<'_>, op: Algebra, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if !is_set_operand(other) {
            return Ok(py.NotImplemented());
        }
        let left = local_table(other, KeyKind::Adaptive)?;
        let me = Operand::Local { table: self.snapshot_table(py).0, is_set: true };
        let table = table_algebra(py, op, left, &me, KeyKind::Adaptive)?;
        Ok(Bound::new(py, Set::with_table(schema::any(), table))?.into_any().unbind())
    }

    fn in_place(&self, py: Python<'_>, op: Algebra, others: &[Bound<'_, PyAny>]) -> PyResult<()> {
        match op {
            Algebra::Union => others.iter().try_for_each(|other| self.add_all(py, other)),
            Algebra::Difference => others.iter().try_for_each(|other| self.remove_all(py, other)),
            Algebra::Intersection => {
                let operands = others.iter().map(Operand::new).collect::<PyResult<Vec<_>>>()?;
                let kind = KeyKind::for_schema(&self.schema);
                self.replace_with(py, |mut table| {
                    for operand in &operands {
                        table = retain_contained(py, table, operand, kind)?;
                    }
                    Ok(table)
                })
            }
            Algebra::SymmetricDifference => {
                let mut incoming = KeyTable::new(KeyKind::Adaptive);
                let (items, err) = collect_iterable(&others[0]);
                if let Some(err) = err {
                    return Err(err);
                }
                let mut prepared = Vec::with_capacity(items.len());
                for item in &items {
                    let key = self.prepare(item)?;
                    if incoming.insert_local(py, &key)? {
                        prepared.push(key);
                    }
                }
                let mut displaced = Vec::new();
                keys::batch_mut(py, &self.inner, prepared, |k| k, |inner, key, pos| {
                    match pos {
                        Some(pos) => inner.remove_at(py, pos, &mut displaced),
                        None => {
                            inner.table.push(py, &key)?;
                        }
                    }
                    Ok(())
                })?;
                drop(displaced);
                Ok(())
            }
        }
    }
}

/// Operator forms accept only `Set`, `set` and `frozenset`.
fn is_set_operand(obj: &Bound<'_, PyAny>) -> bool {
    obj.is_instance_of::<Set>() || obj.is_instance_of::<PySet>() || obj.is_instance_of::<PyFrozenSet>()
}

/// `mine <op> other` on private tables: natively when both store the same native keys,
/// otherwise with Python equality.
fn table_algebra(py: Python<'_>, op: Algebra, mine: KeyTable, other: &Operand<'_>, kind: KeyKind) -> PyResult<KeyTable> {
    let mine = match other.native() {
        Some(theirs) => match mine.into_native() {
            Ok(native) => match native_algebra(op, native, &theirs) {
                Ok(result) => return Ok(KeyTable::from_native(result, kind)),
                Err(native) => KeyTable::from_native(native, kind),
            },
            Err(table) => table,
        },
        None => mine,
    };
    match op {
        Algebra::Union => {
            let mut mine = mine;
            for e in other.elements(py) {
                mine.insert_local(py, &Key::classify(&e, Role::SetElement)?)?;
            }
            Ok(mine)
        }
        Algebra::Intersection => {
            // CPython iterates the smaller set (or the iterable) and keeps its elements.
            if other.is_set() && other.len()? > mine.len() {
                return retain_contained(py, mine, other, kind);
            }
            let mut out = KeyTable::new(kind);
            for e in other.elements(py) {
                let key = Key::classify(&e, Role::SetElement)?;
                if mine.find_local(py, &key)?.is_some() {
                    out.insert_local(py, &key)?;
                }
            }
            Ok(out)
        }
        Algebra::Difference => {
            let mut out = KeyTable::new(kind);
            for e in mine.key_objects(py) {
                let e = e.into_bound(py);
                if !other.contains(&e)? {
                    out.insert_local(py, &Key::classify(&e, Role::SetElement)?)?;
                }
            }
            Ok(out)
        }
        Algebra::SymmetricDifference => {
            let mut mine = mine;
            for e in other.elements(py) {
                let key = Key::classify(&e, Role::SetElement)?;
                match mine.find_local(py, &key)? {
                    Some(pos) => {
                        drop(mine.remove_at(py, pos));
                        let _ = mine.maybe_compact();
                    }
                    None => {
                        mine.push(py, &key)?;
                    }
                }
            }
            Ok(mine)
        }
    }
}

/// The elements of `mine` (keeping their objects) that `other` contains.
fn retain_contained(py: Python<'_>, mine: KeyTable, other: &Operand<'_>, kind: KeyKind) -> PyResult<KeyTable> {
    if let Some(theirs) = other.native() {
        match (mine.native_clone(), &theirs) {
            (Some(NativeKeys::Int(a)), NativeKeys::Int(b)) => {
                return Ok(KeyTable::from_native(NativeKeys::Int(retain_native(&a, b)), kind));
            }
            (Some(NativeKeys::Str(a)), NativeKeys::Str(b)) => {
                return Ok(KeyTable::from_native(NativeKeys::Str(retain_native(&a, b)), kind));
            }
            _ => {}
        }
    }
    let mut out = KeyTable::new(kind);
    for e in mine.key_objects(py) {
        let e = e.into_bound(py);
        if other.contains(&e)? {
            out.insert_local(py, &Key::classify(&e, Role::SetElement)?)?;
        }
    }
    Ok(out)
}

fn retain_native<K: Hash + Eq + Clone + Default>(a: &KeyIndex<K>, b: &KeyIndex<K>) -> KeyIndex<K> {
    let mut out = KeyIndex::default();
    for (_, k) in a.iter() {
        if b.contains(k) {
            out.insert(k.clone());
        }
    }
    out
}

#[pymethods]
impl Set {
    #[new]
    #[pyo3(signature = (iterable=None, /))]
    fn __new__(py: Python<'_>, iterable: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        Set::build(py, schema::any(), iterable)
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
        let py = slf.py();
        let inner = slf.get().inner.read(py);
        let len = inner.table.len();
        let state = SetIterState { pos: 0, expected: len, layout: inner.table.layout(), exhausted: false };
        drop(inner);
        SetIterator { set: slf.clone().unbind(), state: Locked::new(state) }
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let this = slf.get();
        let name = this.type_name();
        let Some(_guard) = ReprGuard::enter(slf.as_any())? else {
            return Ok(format!("{name}(...)"));
        };
        let elems = this.element_objects(py);
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

    fn __richcmp__(slf: &Bound<'_, Self>, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        if !is_set_operand(other) {
            return Ok(py.NotImplemented());
        }
        let operand = Operand::new(other)?;
        let this = slf.get();
        let (mine, theirs) = (this.len(py), operand.len()?);
        let result = match op {
            CompareOp::Eq | CompareOp::Ne => {
                let eq = other.is(slf) || (mine == theirs && this.is_subset_of(py, &operand)?);
                eq == matches!(op, CompareOp::Eq)
            }
            CompareOp::Le => this.is_subset_of(py, &operand)?,
            CompareOp::Lt => mine < theirs && this.is_subset_of(py, &operand)?,
            CompareOp::Ge => mine >= theirs && this.is_superset_of(py, &operand)?,
            CompareOp::Gt => mine > theirs && this.is_superset_of(py, &operand)?,
        };
        Ok(result.into_pyobject(py)?.to_owned().into_any().unbind())
    }

    fn add(&self, py: Python<'_>, elem: &Bound<'_, PyAny>) -> PyResult<()> {
        let key = self.prepare(elem)?;
        keys::with_entry_mut(py, &self.inner, &key, |inner, pos| match pos {
            Some(_) => Ok(()),
            None => inner.table.push(py, &key).map(drop),
        })?
    }

    fn discard(&self, py: Python<'_>, elem: &Bound<'_, PyAny>) -> PyResult<()> {
        self.discard_elem(py, elem).map(drop)
    }

    fn remove(&self, py: Python<'_>, elem: &Bound<'_, PyAny>) -> PyResult<()> {
        if self.discard_elem(py, elem)? { Ok(()) } else { Err(PyKeyError::new_err(elem.clone().unbind())) }
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
        Set::with_table(self.schema.clone(), self.snapshot_table(py).0)
    }

    fn __copy__(&self, py: Python<'_>) -> Set {
        self.copy(py)
    }

    #[pyo3(signature = (*others))]
    fn union(&self, py: Python<'_>, others: Vec<Bound<'_, PyAny>>) -> PyResult<Set> {
        self.compute(py, Algebra::Union, &others)
    }

    #[pyo3(signature = (*others))]
    fn intersection(&self, py: Python<'_>, others: Vec<Bound<'_, PyAny>>) -> PyResult<Set> {
        self.compute(py, Algebra::Intersection, &others)
    }

    #[pyo3(signature = (*others))]
    fn difference(&self, py: Python<'_>, others: Vec<Bound<'_, PyAny>>) -> PyResult<Set> {
        self.compute(py, Algebra::Difference, &others)
    }

    fn symmetric_difference(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Set> {
        self.compute(py, Algebra::SymmetricDifference, std::slice::from_ref(other))
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

    fn issubset(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        self.is_subset_of(py, &Operand::new(other)?)
    }

    fn issuperset(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        self.is_superset_of(py, &Operand::new(other)?)
    }

    fn isdisjoint(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        self.is_disjoint_with(py, &Operand::new(other)?)
    }

    fn __or__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, Algebra::Union, other)
    }

    fn __ror__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.reflected(py, Algebra::Union, other)
    }

    fn __and__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, Algebra::Intersection, other)
    }

    fn __rand__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.reflected(py, Algebra::Intersection, other)
    }

    fn __sub__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, Algebra::Difference, other)
    }

    fn __rsub__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.reflected(py, Algebra::Difference, other)
    }

    fn __xor__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.binary(py, Algebra::SymmetricDifference, other)
    }

    fn __rxor__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.reflected(py, Algebra::SymmetricDifference, other)
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

    fn __reduce__<'py>(&self, py: Python<'py>) -> PyResult<(Bound<'py, PyAny>, (Bound<'py, PyList>,))> {
        Ok((self.schema_type_of(py)?, (PyList::new(py, self.element_objects(py))?,)))
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

    fn __clear__(&self, py: Python<'_>) {
        self.clear(py);
    }
}

/// An in-place operator operand: `Set`, `set` or `frozenset` (anything else → `NotImplemented`).
pub struct SetLike<'py>(Bound<'py, PyAny>);

impl<'a, 'py> FromPyObject<'a, 'py> for SetLike<'py> {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
        if obj.is_instance_of::<Set>() || obj.is_instance_of::<PySet>() || obj.is_instance_of::<PyFrozenSet>() {
            Ok(SetLike(obj.to_owned()))
        } else {
            Err(PyTypeError::new_err("expected a set"))
        }
    }
}

struct SetIterState {
    pos: usize,
    expected: usize,
    layout: u64,
    exhausted: bool,
}

/// Live iterator over a `Set`.
#[pyclass(module = "rustique", frozen)]
pub struct SetIterator {
    set: Py<Set>,
    state: Locked<SetIterState>,
}

#[pymethods]
impl SetIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let mut st = self.state.write(py);
        if st.exhausted {
            return Ok(None);
        }
        let inner = self.set.get().inner.read(py);
        if inner.table.len() != st.expected {
            st.expected = usize::MAX;
            return Err(PyRuntimeError::new_err("Set changed size during iteration"));
        }
        if inner.table.layout() != st.layout {
            return Err(PyRuntimeError::new_err("Set changed during iteration"));
        }
        match inner.table.next_live(st.pos) {
            Some(pos) => {
                st.pos = pos + 1;
                Ok(Some(inner.table.key_object(py, pos)))
            }
            None => {
                st.exhausted = true;
                Ok(None)
            }
        }
    }

    fn __length_hint__(&self, py: Python<'_>) -> usize {
        let st = self.state.read(py);
        if st.exhausted {
            return 0;
        }
        let inner = self.set.get().inner.read(py);
        inner.table.index().map_or(0, |i| (st.pos..i.entries()).filter(|&p| i.is_live(p)).count())
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Set>()?;
    m.add_class::<SetIterator>()
}
