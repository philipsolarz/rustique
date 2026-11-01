//! The `deepcopy` memo.
//!
//! The kernel only completes calls in which no user code runs, so no hook ever sees the
//! memo while the kernel works. `deepcopy(x)` keeps it natively (`id(x) -> copy` in
//! insertion order, plus the keep-alive list `copy.py` stores under `id(memo)`), and drops
//! it as CPython's dict would be dropped. `deepcopy(x, memo)` with an exact dict reads the
//! caller's dict (pure lookups of int keys) and buffers every write natively; on success the
//! writes are applied to the dict in CPython's order — new keys in insertion order, the
//! keep-alive list created at its position (`memo[id(memo)] = [x]`) or appended to — so the
//! caller finds exactly the memo `copy.py` would have left. A call that restarts in the
//! original leaves the caller's dict untouched.

use std::ptr::null_mut;

use pyo3::ffi;

use super::rt::*;

/// Native image of the entries a `deepcopy` call adds to its memo.
pub struct Native {
    /// `(id(x), copy)` in insertion order (values strong).
    entries: SVec<(usize, O), 16>,
    index: hashbrown::HashMap<usize, u32, foldhash::fast::FixedState>,
    /// Objects appended to the keep-alive list (strong).
    keep: SVec<O, 16>,
    /// Number of entries inserted before the keep-alive list was created (its position).
    ka_pos: usize,
}

const LINEAR: usize = 12;

impl Native {
    #[inline(always)]
    fn new() -> Native {
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

    /// `memo[id(x)] = y` (y borrowed; a new key goes last, an existing one keeps its place).
    #[inline(always)]
    unsafe fn set(&mut self, x: O, y: O) {
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

    /// Release every reference in the order CPython's memo dict releases them when it dies:
    /// entries in insertion order, the keep-alive list (back to front, as `list_dealloc`)
    /// at its position.
    unsafe fn release(&mut self) {
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

/// What `memo[id(memo)]` held in the caller's dict when the first `_keep_alive` ran.
enum Ka {
    /// Not looked up yet.
    Unknown,
    /// Absent: `_keep_alive` creates `[x]` at `ka_pos`.
    Create,
    /// An exact list (owned by the dict, which nothing else touches meanwhile): appended to.
    Append(O),
}

/// The memo of one kernel call.
pub struct Memo {
    native: Native,
    /// The caller's exact dict (strong), or null for `deepcopy(x)`.
    base: O,
    ka: Ka,
}

impl Memo {
    /// `base`: null or an exact dict (borrowed).
    #[inline(always)]
    pub unsafe fn new(base: O) -> Memo {
        // SAFETY: a live dict (or null).
        let base = if base.is_null() { base } else { unsafe { incref(base) } };
        Memo { native: Native::new(), base, ka: Ka::Unknown }
    }

    #[inline(always)]
    pub fn has_base(&self) -> bool {
        !self.base.is_null()
    }

    /// `memo.get(id(x))` (new reference or null). Err: an error (restart).
    #[inline(always)]
    pub unsafe fn lookup(&self, x: O) -> R<O> {
        if let Some(i) = self.native.find(x as usize) {
            // SAFETY: live value.
            return Ok(unsafe { incref(self.native.entries.as_slice()[i].1) });
        }
        if self.base.is_null() {
            return Ok(null_mut());
        }
        // SAFETY: an exact dict and an int key: no Python code runs.
        unsafe {
            let d = Own::new(ffi::PyLong_FromVoidPtr(x.cast()))?;
            dict_get(self.base, d.get())
        }
    }

    /// `memo[id(x)] = y`.
    #[inline(always)]
    pub unsafe fn set(&mut self, x: O, y: O) {
        // SAFETY: live objects.
        unsafe { self.native.set(x, y) }
    }

    /// `_keep_alive(x, memo)`. Err: the caller's `memo[id(memo)]` is not an exact list (its
    /// `append` could run user code): restart.
    #[inline(always)]
    pub unsafe fn keep_alive(&mut self, x: O) -> R<()> {
        // SAFETY: live objects; pure dict lookup.
        unsafe {
            if matches!(self.ka, Ka::Unknown) {
                self.ka = Ka::Create;
                if !self.base.is_null() {
                    let k = Own::new(ffi::PyLong_FromVoidPtr(self.base.cast()))?;
                    let l = dict_get(self.base, k.get())?;
                    if !l.is_null() {
                        decref(l);
                        if ty(l) != &raw mut ffi::PyList_Type {
                            return Err(());
                        }
                        self.ka = Ka::Append(l);
                    }
                }
            }
            let n = &mut self.native;
            if n.ka_pos == usize::MAX {
                n.ka_pos = n.entries.len();
            }
            n.keep.push(incref(x));
            Ok(())
        }
    }

    /// Success: apply the buffered writes to the caller's dict (if any), in CPython's order.
    /// Inserting int keys into an exact dict runs no Python code; Err only on allocation
    /// failure (the caller then raises it: the call had no other observable effect).
    pub unsafe fn commit(&mut self) -> R<()> {
        if self.base.is_null() {
            return Ok(());
        }
        // SAFETY: an exact dict; int keys and arbitrary values.
        unsafe {
            let d = self.base;
            let n = &self.native;
            let e = n.entries.as_slice();
            let keep = n.keep.as_slice();
            let create = matches!(self.ka, Ka::Create) && !keep.is_empty();
            let insert_ka = || -> R<()> {
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
                if create && i == n.ka_pos {
                    insert_ka()?;
                }
                let key = Own::new(ffi::PyLong_FromVoidPtr(k as *mut _))?;
                if ffi::PyDict_SetItem(d, key.get(), v) < 0 {
                    return Err(());
                }
            }
            if create && n.ka_pos == e.len() {
                insert_ka()?;
            }
            if let Ka::Append(l) = self.ka {
                for &o in keep {
                    if ffi::PyList_Append(l, o) < 0 {
                        return Err(());
                    }
                }
            }
            Ok(())
        }
    }
}

impl Drop for Memo {
    fn drop(&mut self) {
        // SAFETY: owned references (attached thread: memos live inside kernel calls).
        unsafe {
            self.native.release();
            xdecref(self.base);
        }
    }
}
