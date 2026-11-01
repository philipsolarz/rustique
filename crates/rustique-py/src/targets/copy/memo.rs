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
    keys: Vec<usize>,
    vals: Vec<O>, // strong
    index: hashbrown::HashMap<usize, u32, foldhash::fast::FixedState>,
    keep: Vec<O>, // strong; the list at memo[id(memo)]
    /// Number of entries inserted before the keep-alive list was (position of id(memo)).
    ka_pos: usize,
}

const LINEAR: usize = 12;

impl Native {
    pub fn new() -> Native {
        Native {
            keys: Vec::new(),
            vals: Vec::new(),
            index: hashbrown::HashMap::with_hasher(foldhash::fast::FixedState::with_seed(0)),
            keep: Vec::new(),
            ka_pos: usize::MAX,
        }
    }

    #[inline(always)]
    fn find(&self, k: usize) -> Option<usize> {
        if self.keys.len() <= LINEAR {
            self.keys.iter().position(|&x| x == k)
        } else {
            self.index.get(&k).map(|&i| i as usize)
        }
    }

    /// Borrowed value for `id(x)`.
    #[inline(always)]
    pub fn get(&self, x: O) -> Option<O> {
        self.find(x as usize).map(|i| self.vals[i])
    }

    /// `memo[id(x)] = y` (y borrowed; a new key goes last, an existing one keeps its place).
    #[inline(always)]
    pub unsafe fn set(&mut self, x: O, y: O) {
        let k = x as usize;
        // SAFETY: live object.
        unsafe { incref(y) };
        if let Some(i) = self.find(k) {
            let old = std::mem::replace(&mut self.vals[i], y);
            // SAFETY: owned.
            unsafe { decref(old) };
            return;
        }
        let i = self.keys.len();
        self.keys.push(k);
        self.vals.push(y);
        if i == LINEAR {
            for (j, &key) in self.keys.iter().enumerate() {
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
            self.ka_pos = self.keys.len();
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
            let n = self.keys.len();
            for i in 0..=n {
                if i == self.ka_pos {
                    let list = Own::new(ffi::PyList_New(1))?;
                    ffi::PyList_SET_ITEM(list.get(), 0, incref(self.keep[0]));
                    for &o in &self.keep[1..] {
                        if ffi::PyList_Append(list.get(), o) < 0 {
                            return Err(());
                        }
                    }
                    let key = Own::new(ffi::PyLong_FromVoidPtr(d.get().cast()))?;
                    if ffi::PyDict_SetItem(d.get(), key.get(), list.get()) < 0 {
                        return Err(());
                    }
                }
                if i < n {
                    let key = Own::new(ffi::PyLong_FromVoidPtr(self.keys[i] as *mut _))?;
                    if ffi::PyDict_SetItem(d.get(), key.get(), self.vals[i]) < 0 {
                        return Err(());
                    }
                }
            }
            self.release();
            Ok(d.into_raw())
        }
    }

    /// Drop every reference in the order CPython's memo dict releases them when it dies:
    /// entries in insertion order, the keep-alive list (released back to front, as
    /// `list_dealloc`) at its position.
    pub unsafe fn release(&mut self) {
        let n = self.keys.len();
        let keep = std::mem::take(&mut self.keep);
        let vals = std::mem::take(&mut self.vals);
        let ka = self.ka_pos;
        self.keys.clear();
        self.index.clear();
        self.ka_pos = usize::MAX;
        for i in 0..=n {
            if i == ka {
                for &o in keep.iter().rev() {
                    // SAFETY: owned.
                    unsafe { decref(o) };
                }
            }
            if i < n {
                // SAFETY: owned.
                unsafe { decref(vals[i]) };
            }
        }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: owned references (attached thread: engines live inside kernel calls).
        unsafe { self.release() }
    }
}

/// The memo of one `deepcopy` call tree.
pub enum Memo {
    Native(Native),
    /// A real dict (the caller's, or materialised). Strong reference.
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
    pub unsafe fn lookup(&mut self, x: O, minor: u8) -> R<Look> {
        match self {
            Memo::Native(n) => Ok(match n.get(x) {
                // SAFETY: live value.
                Some(y) => Look::Hit(unsafe { incref(y) }),
                None => Look::Miss(null_mut()),
            }),
            // SAFETY: exact dict memo; the same dict operations as copy.py.
            Memo::Dict(m) => unsafe {
                let d = Own::new(ffi::PyLong_FromVoidPtr(x.cast()))?;
                if minor >= 15 {
                    match ffi::PyDict_Contains(*m, d.get()) {
                        r if r < 0 => Err(()),
                        0 => Ok(Look::Miss(d.into_raw())),
                        _ => Ok(Look::Hit(nn(ffi::PyObject_GetItem(*m, d.get()))?)),
                    }
                } else {
                    let y = dict_get(*m, d.get())?;
                    if y.is_null() { Ok(Look::Miss(d.into_raw())) } else { Ok(Look::Hit(y)) }
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
