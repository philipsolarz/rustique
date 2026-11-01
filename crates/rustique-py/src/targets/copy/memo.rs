//! The `deepcopy` memo.
//!
//! `copy.deepcopy(x)` (no memo argument) creates a private `{}` that nothing but `copy.py`
//! can see until a hook receives it: `__deepcopy__(memo)`, a copier registered in
//! `copy._deepcopy_dispatch`, or a call into `copy._reconstruct`. Until then the kernel keeps
//! the same entries natively — `id(x) -> copy` in insertion order, plus the keep-alive list
//! stored under `id(memo)` — and *materialises* the dict, with exactly CPython's contents and
//! insertion order, the moment a hook needs it. From then on (and whenever the caller passed
//! a memo dict) every memo operation is the dict operation `copy.py` performs.

use std::ptr::null_mut;

use pyo3::ffi;

use super::rt::*;

/// Native image of CPython's memo dict.
pub struct Native {
    /// `(id(x), copy)` in insertion order (values strong).
    entries: SVec<(usize, O), 16>,
    index: hashbrown::HashMap<usize, u32, foldhash::fast::FixedState>,
    /// The list at memo[id(memo)] (strong).
    keep: SVec<O, 16>,
    /// Number of entries inserted before the keep-alive list was (position of id(memo)).
    ka_pos: usize,
}

const LINEAR: usize = 12;

impl Native {
    #[inline(always)]
    pub fn new() -> Native {
        Native {
            entries: SVec::new(),
            index: hashbrown::HashMap::with_hasher(foldhash::fast::FixedState::with_seed(0)),
            keep: SVec::new(),
            ka_pos: usize::MAX,
        }
    }

    #[inline(always)]
    fn find(&self, k: usize) -> Option<usize> {
        let e = self.entries.as_slice();
        if e.len() <= LINEAR { e.iter().position(|&(x, _)| x == k) } else { self.index.get(&k).map(|&i| i as usize) }
    }

    /// Borrowed value for `id(x)`.
    #[inline(always)]
    pub fn get(&self, x: O) -> Option<O> {
        self.find(x as usize).map(|i| self.entries.as_slice()[i].1)
    }

    /// `memo[id(x)] = y` (y borrowed; a new key goes last, an existing one keeps its place).
    #[inline(always)]
    pub unsafe fn set(&mut self, x: O, y: O) {
        let k = x as usize;
        // SAFETY: live object.
        unsafe { incref(y) };
        if let Some(i) = self.find(k) {
            let old = std::mem::replace(&mut self.entries.as_mut_slice()[i].1, y);
            // SAFETY: owned.
            unsafe { decref(old) };
            return;
        }
        let i = self.entries.len();
        self.entries.push((k, y));
        if i == LINEAR {
            for (j, &(key, _)) in self.entries.as_slice().iter().enumerate() {
                self.index.insert(key, j as u32);
            }
        } else if i > LINEAR {
            self.index.insert(k, i as u32);
        }
    }

    /// `_keep_alive(x, memo)`.
    #[inline(always)]
    pub unsafe fn keep_alive(&mut self, x: O) {
        if self.ka_pos == usize::MAX {
            self.ka_pos = self.entries.len();
        }
        // SAFETY: live object.
        self.keep.push(unsafe { incref(x) });
    }

    /// Build the dict CPython would hold now: same keys (ints), values and insertion
    /// order, the keep-alive list (grown by appends from `[x]`, as `_keep_alive` grows it)
    /// at its position. Consumes the native entries.
    pub unsafe fn materialize(&mut self) -> R<O> {
        // SAFETY: attached thread.
        unsafe {
            let d = Own::new(ffi::PyDict_New())?;
            let e = self.entries.as_slice();
            let keep = self.keep.as_slice();
            let ka = self.ka_pos;
            let insert_ka = |d: O| -> R<()> {
                let list = Own::new(ffi::PyList_New(1))?;
                ffi::PyList_SET_ITEM(list.get(), 0, incref(keep[0]));
                for &o in &keep[1..] {
                    if ffi::PyList_Append(list.get(), o) < 0 {
                        return Err(());
                    }
                }
                let key = Own::new(ffi::PyLong_FromVoidPtr(d.cast()))?;
                if ffi::PyDict_SetItem(d, key.get(), list.get()) < 0 { Err(()) } else { Ok(()) }
            };
            for (i, &(k, v)) in e.iter().enumerate() {
                if i == ka {
                    insert_ka(d.get())?;
                }
                let key = Own::new(ffi::PyLong_FromVoidPtr(k as *mut _))?;
                if ffi::PyDict_SetItem(d.get(), key.get(), v) < 0 {
                    return Err(());
                }
            }
            if ka == e.len() {
                insert_ka(d.get())?;
            }
            self.release();
            Ok(d.into_raw())
        }
    }

    /// Drop every reference in the order CPython's memo dict releases them when it dies:
    /// entries in insertion order, the keep-alive list (released back to front, as
    /// `list_dealloc`) at its position.
    pub unsafe fn release(&mut self) {
        let ka = self.ka_pos;
        self.ka_pos = usize::MAX;
        if self.entries.is_empty() && self.keep.is_empty() {
            return;
        }
        {
            let keep = self.keep.as_slice();
            let entries = self.entries.as_slice();
            let release_keep = || {
                for &o in keep.iter().rev() {
                    // SAFETY: owned.
                    unsafe { decref(o) };
                }
            };
            for (i, &(_, v)) in entries.iter().enumerate() {
                if i == ka {
                    release_keep();
                }
                // SAFETY: owned.
                unsafe { decref(v) };
            }
            if ka == entries.len() {
                release_keep();
            }
        }
        if self.entries.len() > LINEAR {
            self.index.clear();
        }
        self.keep.clear();
        self.entries.clear();
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: owned references (attached thread: engines live inside kernel calls).
        unsafe { self.release() }
    }
}

/// The memo of one `deepcopy` call tree. The native variant is large on purpose: its
/// first entries live inline (no allocation for small call trees); the enum lives on the
/// kernel's stack.
#[allow(clippy::large_enum_variant)]
pub enum Memo {
    Native(Native),
    /// The caller's memo (any object: `copy.py` only uses `in`, `[]`, `[]=` and, on 3.14,
    /// `.get`), or the materialised dict. Strong reference.
    Dict(O),
}

/// Result of the memo lookup at the top of `deepcopy`.
pub enum Look {
    /// `return memo[d]` (new reference).
    Hit(O),
    /// Not memoised; `d` is the `id(x)` int in dict mode (strong), null in native mode.
    Miss(O),
}

impl Memo {
    /// `if d in memo: return memo[d]` (3.15) / `y = memo.get(d, _nil)` (3.14).
    #[inline(always)]
    pub unsafe fn lookup(&mut self, x: O, minor: u8, st: &St) -> R<Look> {
        match self {
            Memo::Native(n) => Ok(match n.get(x) {
                // SAFETY: live value.
                Some(y) => Look::Hit(unsafe { incref(y) }),
                None => Look::Miss(null_mut()),
            }),
            // SAFETY: exact dict memo; the same dict operations as copy.py.
            Memo::Dict(m) => unsafe {
                let m = *m;
                let d = Own::new(ffi::PyLong_FromVoidPtr(x.cast()))?;
                let exact = ty(m) == &raw mut ffi::PyDict_Type;
                if minor >= 15 {
                    // `if d in memo: return memo[d]`
                    let r = if exact { ffi::PyDict_Contains(m, d.get()) } else { ffi::PySequence_Contains(m, d.get()) };
                    match r {
                        r if r < 0 => Err(()),
                        0 => Ok(Look::Miss(d.into_raw())),
                        _ => Ok(Look::Hit(nn(ffi::PyObject_GetItem(m, d.get()))?)),
                    }
                } else if exact {
                    // `y = memo.get(d, _nil)` on an exact dict.
                    let y = dict_get(m, d.get())?;
                    if y.is_null() { Ok(Look::Miss(d.into_raw())) } else { Ok(Look::Hit(y)) }
                } else {
                    let args = [m, d.get(), st.nil];
                    let y = nn(ffi::PyObject_VectorcallMethod(
                        st.s_get,
                        args.as_ptr(),
                        3 | ffi::PY_VECTORCALL_ARGUMENTS_OFFSET,
                        std::ptr::null_mut(),
                    ))?;
                    if y == st.nil {
                        decref(y);
                        Ok(Look::Miss(d.into_raw()))
                    } else {
                        Ok(Look::Hit(y))
                    }
                }
            },
        }
    }

    /// `memo[d] = y` / `memo[id(x)] = y`.
    #[inline(always)]
    pub unsafe fn set(&mut self, x: O, d: O, y: O) -> R<()> {
        match self {
            Memo::Native(n) => {
                // SAFETY: live objects.
                unsafe { n.set(x, y) };
                Ok(())
            }
            // SAFETY: dict memo, int key.
            Memo::Dict(m) => unsafe {
                if d.is_null() {
                    let k = Own::new(ffi::PyLong_FromVoidPtr(x.cast()))?;
                    if ffi::PyObject_SetItem(*m, k.get(), y) < 0 { Err(()) } else { Ok(()) }
                } else if ffi::PyObject_SetItem(*m, d, y) < 0 {
                    Err(())
                } else {
                    Ok(())
                }
            },
        }
    }

    /// `_keep_alive(x, memo)` (its frame is accounted for by the caller).
    #[inline(always)]
    pub unsafe fn keep_alive(&mut self, st: &St, x: O) -> R<()> {
        match self {
            Memo::Native(n) => {
                // SAFETY: live object.
                unsafe { n.keep_alive(x) };
                Ok(())
            }
            // SAFETY: dict memo; `memo[id(memo)].append(x)` / `memo[id(memo)] = [x]`.
            Memo::Dict(m) => unsafe {
                let m = *m;
                let k = Own::new(ffi::PyLong_FromVoidPtr(m.cast()))?;
                let l = ffi::PyObject_GetItem(m, k.get());
                if l.is_null() {
                    if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) == 0 {
                        return Err(());
                    }
                    ffi::PyErr_Clear();
                    let list = Own::new(ffi::PyList_New(1))?;
                    ffi::PyList_SET_ITEM(list.get(), 0, incref(x));
                    return if ffi::PyObject_SetItem(m, k.get(), list.get()) < 0 { Err(()) } else { Ok(()) };
                }
                let l = Own(l);
                if ty(l.get()) == &raw mut ffi::PyList_Type {
                    if ffi::PyList_Append(l.get(), x) < 0 { Err(()) } else { Ok(()) }
                } else {
                    call_method1(l.get(), st.s_append, x).map(|r| decref(r))
                }
            },
        }
    }

    /// The dict to hand to a hook (materialising a native memo). Borrowed.
    #[inline(always)]
    pub unsafe fn dict(&mut self) -> R<O> {
        match self {
            Memo::Dict(m) => Ok(*m),
            Memo::Native(n) => {
                // SAFETY: attached thread.
                let d = unsafe { n.materialize()? };
                *self = Memo::Dict(d);
                Ok(d)
            }
        }
    }

    #[inline(always)]
    pub fn is_native(&self) -> bool {
        matches!(self, Memo::Native(_))
    }
}

impl Drop for Memo {
    fn drop(&mut self) {
        if let Memo::Dict(m) = self {
            // SAFETY: owned reference.
            unsafe { decref(*m) }
        }
    }
}
